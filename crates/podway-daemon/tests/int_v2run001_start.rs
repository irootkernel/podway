//! Focused V2RUN-001 coverage for confirmed custom Procedure v2 start preparation.

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{
    AttemptId, AttemptLifecycle, ProcedureSnapshotId, Revision, SessionId, SessionLifecycle,
    Sha256Digest, UnixMillis, WorkspaceId,
};
use podway_daemon::execution::{
    ExecutionBoundaryErrorV1, ProcedureProviderV1, ProcedureV2SourceAdmissionErrorV1,
    ProcedureV2StartPreparationErrorV1, prepare_custom_procedure_v2_start,
    prepare_preset_procedure_v2_start, workspace_procedure_snapshot_from_bytes_v2,
};
use podway_store::{
    DurableWorktreeIdentityV1, ProcedureSnapshotV2, ValidatedWorkspaceRootV1, WorkspaceBindingV1,
};

const EQUIVALENT_YAML: &[u8] =
    include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml");
const EQUIVALENT_JSON: &[u8] =
    include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.json");
const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000000002";
const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000003";
const SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000000004";
const IDENTITY_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone, Copy)]
struct ByteProcedureV2<'a> {
    path: &'a str,
    source: &'a [u8],
}

#[derive(Clone)]
struct PinnedPresetV2 {
    snapshot: ProcedureSnapshotV2,
    pinned_digest: Sha256Digest,
}

impl ProcedureProviderV1 for PinnedPresetV2 {
    fn load_preset_snapshot(
        &self,
        _preset: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<podway_core::ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        panic!("v2 preset must not enter the v1 preset loader")
    }

    fn load_workspace_procedure_snapshot(
        &self,
        _workspace: &WorkspaceBindingV1,
        _procedure: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<podway_core::ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        panic!("v2 preset must not read a workspace source")
    }

    fn load_preset_snapshot_v2(
        &self,
        preset: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<Option<(ProcedureSnapshotV2, Sha256Digest)>, ProcedureV2SourceAdmissionErrorV1>
    {
        assert_eq!(preset, "injected-v2");
        Ok(Some((self.snapshot.clone(), self.pinned_digest.clone())))
    }
}

impl ProcedureProviderV1 for ByteProcedureV2<'_> {
    fn load_preset_snapshot(
        &self,
        _preset: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<podway_core::ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        panic!("V2RUN-001 custom-start fixture must not resolve a preset")
    }

    fn load_workspace_procedure_snapshot(
        &self,
        _workspace: &WorkspaceBindingV1,
        _procedure: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<podway_core::ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        panic!("positive Procedure v2 dispatch must not enter the retained v1 loader")
    }

    fn load_workspace_procedure_snapshot_v2(
        &self,
        _workspace: &WorkspaceBindingV1,
        procedure: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV2, ProcedureV2SourceAdmissionErrorV1> {
        assert_eq!(procedure, self.path);
        workspace_procedure_snapshot_from_bytes_v2(procedure, self.source, snapshot_id, created_at)
    }
}

fn binding() -> WorkspaceBindingV1 {
    let digest = Sha256Digest::new(IDENTITY_DIGEST).unwrap();
    WorkspaceBindingV1::new(
        DurableWorktreeIdentityV1::new(
            digest.clone(),
            WorkspaceId::new(WORKSPACE_ID).unwrap(),
            digest,
        ),
        ValidatedWorkspaceRootV1::from_encoded("podway.unix-path/v1:2f776f726b74726565").unwrap(),
    )
}

fn snapshot(source: &[u8], path: &str) -> ProcedureSnapshotV2 {
    workspace_procedure_snapshot_from_bytes_v2(
        path,
        source,
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap()
}

fn prepare(
    provider: &ByteProcedureV2<'_>,
    expected: Option<&Sha256Digest>,
) -> Result<podway_store::GraphSessionStateV2, ProcedureV2StartPreparationErrorV1> {
    prepare_custom_procedure_v2_start(
        provider,
        &binding(),
        provider.path,
        expected,
        "Implement confirmed Procedure v2 start",
        SessionId::new(SESSION_ID).unwrap(),
        AttemptId::new(ATTEMPT_ID).unwrap(),
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
}

#[test]
fn v2run001_exact_custom_digest_prepares_one_complete_fresh_cursor() {
    let provider = ByteProcedureV2 {
        path: "workflow.yaml",
        source: EQUIVALENT_YAML,
    };
    let expected = snapshot(provider.source, provider.path).digest().clone();
    let state = prepare(&provider, Some(&expected)).unwrap();

    assert_eq!(state.workspace_revision(), Revision::new(1));
    assert_eq!(state.trace().revision(), Revision::new(1));
    assert_eq!(state.trace().lifecycle(), SessionLifecycle::Running);
    assert_eq!(state.trace().session_id().as_str(), SESSION_ID);
    assert_eq!(state.snapshot().digest(), &expected);
    assert_eq!(
        state.snapshot().canonical_json().as_str(),
        snapshot(provider.source, provider.path)
            .canonical_json()
            .as_str()
    );
    assert_eq!(state.counters().len(), state.snapshot().graph_nodes().len());

    let active = state
        .trace()
        .active_attempt()
        .expect("fresh v2 start has one cursor");
    assert_eq!(active.attempt_id().as_str(), ATTEMPT_ID);
    assert_eq!(
        active.graph_node_id(),
        state.snapshot().entry_graph_node_id()
    );
    assert_eq!(active.lifecycle(), AttemptLifecycle::Active);
    assert_eq!(state.trace().attempts().len(), 1);
    assert_eq!(state.attempt_metadata().len(), 1);
    assert_eq!(state.workflow_memory().attempts().len(), 1);
    assert!(state.goal_state().current_revision().is_none());

    for counter in state.counters() {
        let expected_count = u64::from(counter.graph_node_id() == active.graph_node_id());
        assert_eq!(counter.attempt_count(), expected_count);
        assert_eq!(counter.rework_traversal_count(), 0);
    }
}

#[test]
fn v2run001_missing_or_wrong_custom_digest_returns_no_prepared_state() {
    let provider = ByteProcedureV2 {
        path: "workflow.yaml",
        source: EQUIVALENT_YAML,
    };
    let actual = snapshot(provider.source, provider.path).digest().clone();

    match prepare(&provider, None).unwrap_err() {
        ProcedureV2StartPreparationErrorV1::DigestConfirmationRequired {
            procedure_digest: received,
        } => {
            assert_eq!(received, actual);
        }
        other => panic!("missing digest returned the wrong error: {other:?}"),
    }

    let wrong = Sha256Digest::new(
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .unwrap();
    match prepare(&provider, Some(&wrong)).unwrap_err() {
        ProcedureV2StartPreparationErrorV1::ProcedureDigestMismatch {
            expected,
            actual: received,
        } => {
            assert_eq!(expected, wrong);
            assert_eq!(received, actual);
        }
        other => panic!("wrong digest returned the wrong error: {other:?}"),
    }
}

#[test]
fn v2run001_yaml_and_json_formatting_share_one_confirmed_semantic_digest() {
    let yaml = snapshot(EQUIVALENT_YAML, "workflow.yaml");
    let json = snapshot(EQUIVALENT_JSON, "workflow.json");

    assert_eq!(yaml.digest(), json.digest());
    assert_eq!(yaml.canonical_json(), json.canonical_json());

    let provider = ByteProcedureV2 {
        path: "workflow.json",
        source: EQUIVALENT_JSON,
    };
    let state = prepare(&provider, Some(yaml.digest())).unwrap();
    assert_eq!(state.snapshot().digest(), yaml.digest());
}

#[test]
fn v2run001_v1_document_remains_owned_by_the_v1_parser() {
    let source = br#"schema: podway.procedure/v1
id: retained
version: "1"
name: Retained v1
stages:
  - id: work
    title: Work
rework:
  allow_return_to: any_previous
"#;
    assert!(matches!(
        parse_procedure_document(source, ProcedureDocumentFormat::Yaml).unwrap(),
        ParsedProcedure::V1(_)
    ));
    assert!(matches!(
        workspace_procedure_snapshot_from_bytes_v2(
            "retained.yaml",
            source,
            ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
            UnixMillis::new(1),
        ),
        Err(ProcedureV2SourceAdmissionErrorV1::NotProcedureV2)
    ));

    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(EQUIVALENT_YAML, ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("Procedure v2 fixture was reinterpreted as v1")
    };
    assert!(validate_procedure_v2(parsed).is_ok());
}

#[test]
fn v2run001_optional_entry_evidence_starts_unresolved() {
    let source = String::from_utf8(EQUIVALENT_YAML.to_vec())
        .unwrap()
        .replace(
            "    - id: perform\n      use: work\n      next: decide\n",
            "    - id: perform\n      use: work\n      evidence_from:\n        - node: finish\n          required: false\n          items:\n            - result\n      next: decide\n",
        );
    let source = source.as_bytes();
    let provider = ByteProcedureV2 {
        path: "workflow.yaml",
        source,
    };
    let expected = snapshot(provider.source, provider.path).digest().clone();
    let state = prepare(&provider, Some(&expected)).unwrap();
    let memory = &state.workflow_memory().attempts()[0];

    assert_eq!(memory.evidence().len(), 1);
    assert!(!memory.evidence()[0].required());
    assert!(memory.evidence()[0].resolution().is_unresolved());
}

#[test]
fn v2run001_injected_preset_requires_its_independent_pinned_digest() {
    let admitted = snapshot(EQUIVALENT_YAML, "workflow.yaml");
    let matching = PinnedPresetV2 {
        pinned_digest: admitted.digest().clone(),
        snapshot: admitted.clone(),
    };
    let state = prepare_preset_procedure_v2_start(
        &matching,
        "injected-v2",
        "Start pinned preset",
        SessionId::new(SESSION_ID).unwrap(),
        AttemptId::new(ATTEMPT_ID).unwrap(),
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap()
    .unwrap();
    assert_eq!(state.snapshot().digest(), admitted.digest());

    let mismatched = PinnedPresetV2 {
        snapshot: admitted,
        pinned_digest: Sha256Digest::new(
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap(),
    };
    assert!(matches!(
        prepare_preset_procedure_v2_start(
            &mismatched,
            "injected-v2",
            "Start pinned preset",
            SessionId::new(SESSION_ID).unwrap(),
            AttemptId::new(ATTEMPT_ID).unwrap(),
            ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
            UnixMillis::new(1_700_000_000_000),
        ),
        Err(ProcedureV2StartPreparationErrorV1::PinnedPresetDigestMismatch { .. })
    ));
}
