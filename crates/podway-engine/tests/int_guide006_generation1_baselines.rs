//! GUIDE-006: frozen generation-1 Engine baselines.
//!
//! Pins the complete `podway.output/v4` envelopes of `podway.observation-result/v5`
//! in the five dossier states (prepared, running, running with the dense truncated
//! mutation-template window, completed without a disposition, cancelled with a
//! disposition, delegated prepared) plus the generation-1 `CRITERION_CITATION_INVALID`
//! and `WORKSPACE_CONTRACT_MISMATCH` `podway.error/v2` envelopes, all driven through
//! the typed Direct Engine (`execute_direct_v1`).
//!
//! Freeze rule (Coordinator decision D4): the expected-value files under
//! `guide006_baselines/` are frozen at the GUIDE-006 commit. Through GUIDE-011,
//! `git diff --exit-code <guide006-commit> -- crates/podway-engine/tests/guide006_baselines`
//! must stay empty, and every hunk of this module's own diff must be a mechanical
//! dossier :302-304 update (for example the GUIDE-009 generation fields on the
//! `DirectRequestV1` literals, dossier :446-448) or an added explicit-generation-1
//! reuse call. Any change to the normalizer rules, the deterministic identifier or
//! clock sources, the fixture Procedure sources, the scenario steps or the
//! assertions is a freeze violation that needs Task-Evaluator review. This module
//! stays registered in `int_suite.rs`, so every later `make test` reruns these
//! cases unchanged.
//!
//! Baseline policy (Coordinator decisions D1/D2): each expected value is the exact
//! normalized serialization plus one trailing newline, loaded read-only with
//! `include_str!`. On mismatch the normalized actual is written to
//! `CARGO_TARGET_TMPDIR/guide006/<test>.actual`; installing a baseline is a
//! Coordinator-approved `cp` of that file. Nothing in this module writes into
//! `guide006_baselines/`.
//!
//! Interpretation (Coordinator decision D7): the pinned outputs are the
//! default-generation JSON envelopes; the human rendering leg is intentionally not
//! pinned because the dossier lists human-surface text among intended changes
//! (:580-588).
//!
//! Negative tests (amendment A3) run their scenarios live inside the test; no raw
//! actual is committed. Error scenarios (amendment A4) are routed through
//! `execute_direct_v1`, whose failure envelopes are rendered inside the Engine, so
//! the `DirectRequestV1` adjustment hook alone lets GUIDE-009 select an explicit
//! generation without changing any helper signature here.

use std::{fs, path::Path};

use podway_core::{
    AttemptId, BlockerId, CriterionId, DelegationReceiptIdV1, ItemId, ProcedureSnapshotId,
    Revision, SessionId, WorkspaceId,
};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{DirectCursorCommandV1, DirectIdSourceV1, DirectReadCommandV1},
    direct_items::DirectItemCommandV1,
    direct_start::DirectStartCommandV1,
    execution::{ExecutionClockV1, prepare_workspace_procedure_source_v2},
    v2_read_service::GraphStatusTierV2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{NativeGitResolverV1, WorktreeSelectorV1};
use podway_protocol::{
    GoalAssessCriterionV2, GoalCriterionWireV2, InitialGoalWireV2, ItemCheckV1,
    ItemMutationPreconditionsWireV1, ItemSetV1, ProcedureV2MutationCommandV1, RequestIdV1,
    Rfc3339MillisV1, SessionBeginV1, SessionCancelV1, SessionCompleteV1,
    SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1, SessionStartCommandV1,
    SessionStartSourceV1, SessionStartV1, SessionTerminalDispositionV1, TerminalDispositionInputV1,
};
use podway_store::{GraphSessionStateV2, IdempotencyKeyV1, direct_store::DirectStoreV1};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

/// The fixed millisecond value behind the reused deterministic clocks: the
/// `fixture()` bootstrap stamps `NOW` and the reused `Clock` returns `NOW + 1`
/// (`int_v2dfc006_direct_execution.rs:35` and `:2528`, `:2541-2546`). The two
/// allowlisted timestamps are derived from this constant, not retyped.
pub(crate) const GUIDE006_NOW: u64 = 1_700_000_000_000;

const GUIDE006_SESSION_ID: &str = "00000000-0000-4000-8000-000000000610";
/// Attempt identifiers must stay globally unique within one session
/// (`WorkflowMemoryStateV2::new` rejects duplicates), so the ID source hands
/// them out sequentially from this bounded fixed set; the self-check allowlist
/// covers exactly its members.
const GUIDE006_ATTEMPT_IDS: [&str; 8] = [
    "00000000-0000-4000-8000-000000000611",
    "00000000-0000-4000-8000-000000000617",
    "00000000-0000-4000-8000-000000000618",
    "00000000-0000-4000-8000-000000000619",
    "00000000-0000-4000-8000-000000000620",
    "00000000-0000-4000-8000-000000000621",
    "00000000-0000-4000-8000-000000000622",
    "00000000-0000-4000-8000-000000000623",
];
const GUIDE006_SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000000612";
const GUIDE006_WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000613";
const GUIDE006_BLOCKER_ID: &str = "00000000-0000-4000-8000-000000000614";
const GUIDE006_RECEIPT_ID: &str = "00000000-0000-4000-8000-000000000615";
const GUIDE006_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000616";
const GUIDE006_OBSERVED_AT: &str = "2023-11-14T22:13:20.001Z";

/// Deterministic replacement for `NativeDirectIdSourceV1` so every generated
/// identifier in the pinned envelopes is a fixed constant (amendment A1). One
/// instance lives for one scenario; attempts are drawn sequentially from the
/// bounded set above and every other kind keeps its constant.
pub(crate) struct Guide006Ids {
    attempts: std::sync::atomic::AtomicUsize,
}

impl Guide006Ids {
    pub(crate) fn new() -> Self {
        Self {
            attempts: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Default for Guide006Ids {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectIdSourceV1 for Guide006Ids {
    fn next_workspace_id(&self) -> WorkspaceId {
        WorkspaceId::new(GUIDE006_WORKSPACE_ID).unwrap()
    }
    fn next_session_id(&self) -> SessionId {
        SessionId::new(GUIDE006_SESSION_ID).unwrap()
    }
    fn next_attempt_id(&self) -> AttemptId {
        use std::sync::atomic::Ordering;
        let index = self.attempts.fetch_add(1, Ordering::Relaxed);
        assert!(
            index < GUIDE006_ATTEMPT_IDS.len(),
            "a GUIDE-006 scenario must not need more than {} attempts",
            GUIDE006_ATTEMPT_IDS.len()
        );
        AttemptId::new(GUIDE006_ATTEMPT_IDS[index]).unwrap()
    }
    fn next_blocker_id(&self) -> BlockerId {
        BlockerId::new(GUIDE006_BLOCKER_ID).unwrap()
    }
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        ProcedureSnapshotId::new(GUIDE006_SNAPSHOT_ID).unwrap()
    }
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
        DelegationReceiptIdV1::new(GUIDE006_RECEIPT_ID).unwrap()
    }
}

/// One action node with a required confirm and three optional items; the common
/// source of the prepared, running, completed and cancelled scenarios.
const WORK_PROCEDURE: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide006-work","version":"1",
    "name":"Guide006 work","purpose":"Exercise generation-1 observation states.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work.",
        "items":[
            {"id":"confirm","type":"confirm","prompt":"Confirm.","required":true},
            {"id":"note","type":"text","prompt":"Record a note.","required":false,"max_length":100},
            {"id":"pick","type":"choice","prompt":"Pick.","required":false,"choices":["yes","no"]},
            {"id":"count","type":"integer","prompt":"Count.","required":false,"minimum":0,"maximum":9}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

/// A delegated terminal action node; prepared observation gains `status.delegation`
/// (`v2_read_service.rs:1281-1286`). Delegation shape from `int_v2run002_views.rs:59-73`.
const DELEGATED_PROCEDURE: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide006-delegated","version":"1",
    "name":"Guide006 delegated","purpose":"Exercise the delegated prepared observation.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Review the result.",
        "items":[{"id":"result","type":"text","prompt":"Record the final result.","required":true}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true,
        "delegation":{"binding_ref":"local-specialist","role_ref":"reviewer",
            "role_snapshot_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "max_rounds":3,"after_accept":"retain"}}]}}"#;

/// A goal-tracking procedure whose decision node carries an optional local item,
/// so a goal assessment can cite an unrecorded item and fail the generation-1
/// citation admission rule (the GUIDE-004 runtime recipe at
/// `e2e_v2dfc007_direct_cli.rs:3506-3578`, on the `goal-outcomes.yaml` shape).
const GOAL_PROCEDURE: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide006-goal-citation","version":"2",
    "name":"Guide006 goal citation","purpose":"Exercise the generation-1 criterion citation admission rule.",
    "goal_tracking":true,
    "node_definitions":{
        "work":{"type":"action","title":"Produce evidence","intent":"Record the evidence considered by the goal assessment.",
            "items":[{"id":"result","type":"text","prompt":"Record the result.","required":true,"max_length":1000}]},
        "assess":{"type":"decision","title":"Assess the goal","objective":"Select the outcome determined by the recorded criteria.",
            "prompt":"Which goal outcome applies?",
            "items":[{"id":"assessment-note","type":"text","prompt":"Record local criterion evidence.","required":false,"max_length":100}],
            "options":[
                {"id":"achieved","label":"Achieved"},
                {"id":"not-achieved","label":"Not achieved"},
                {"id":"superseded","label":"Superseded"}],
            "reason":{"required":true},
            "assessment":{"target":"session_goal","outcomes":{
                "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"}}},
        "finish":{"type":"action","title":"Finish","intent":"Finish after a fresh goal assessment."}},
    "graph":{"entry":"perform","nodes":[
        {"id":"perform","use":"work","next":"decide"},
        {"id":"decide","use":"assess","evidence_from":[{"node":"perform","required":true,"items":["result"]}],
            "routes":{"achieved":{"to":"finish","effect":"advance"},
                "not-achieved":{"to":"finish","effect":"advance"},
                "superseded":{"to":"finish","effect":"advance"}}},
        {"id":"finish","use":"finish","terminal":true}]},
    "manual_rework":{"allowed_targets":["perform"]}}"#;

/// The 128-item 300-character `prompt`/`help` shape of `int_v2run002_views.rs:6567-6620`:
/// the active-item window stays complete while the mutation-template window cuts.
fn dense_procedure() -> Vec<u8> {
    let items: Vec<Value> = (0..128)
        .map(|index| {
            json!({
                "id": format!("wide-{index:03}"),
                "type": "text",
                "prompt": "p".repeat(300),
                "help": "h".repeat(300),
                "required": false,
                "max_length": 64
            })
        })
        .collect();
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "wide-observation",
        "version": "2",
        "name": "Wide observation",
        "purpose": "Exercise the observation active-item window.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Record work",
                "intent": "Record many wide items.",
                "items": items
            }
        },
        "graph": {
            "entry": "work",
            "nodes": [{"id": "work", "use": "work", "terminal": true}]
        }
    }))
    .unwrap()
}

/// The raw and volatile context of one scenario run. Normalization replaces only
/// the worktree root (Engine injected sources keep every other value fixed), and
/// the workspace UUID is derived from the live Store binding so the self-check
/// allowlist never retypes it.
pub(crate) struct Guide006EngineActual {
    pub raw: String,
    pub worktree_root: String,
    pub workspace_uuid: String,
}

impl Guide006EngineActual {
    pub fn normalized(&self) -> String {
        guide006_normalize_engine(&self.raw, &self.worktree_root)
    }
    pub fn baseline_text(&self) -> String {
        format!("{}\n", self.normalized())
    }
}

/// Engine normalization is root-only (amendment A1): a success envelope's
/// `workspace.root` must equal the fixture worktree root exactly once and is
/// replaced; an error envelope carries no workspace member, and the root must
/// then appear nowhere. Every other violation fails loudly.
pub(crate) fn guide006_normalize_engine(text: &str, worktree_root: &str) -> String {
    let envelope: Value = serde_json::from_str(text).expect("the Engine actual is one envelope");
    let occurrences = text.matches(worktree_root).count();
    match envelope
        .get("workspace")
        .and_then(|workspace| workspace.get("root"))
    {
        Some(root) => {
            assert_eq!(
                root.as_str(),
                Some(worktree_root),
                "the Engine envelope workspace root must be the fixture worktree root"
            );
            assert_eq!(
                occurrences, 1,
                "the Engine envelope must contain exactly one worktree-root occurrence"
            );
            text.replace(worktree_root, "<worktree-root>")
        }
        None => {
            assert_eq!(
                occurrences, 0,
                "an Engine envelope without a workspace member must not contain the worktree root"
            );
            text.to_owned()
        }
    }
}

/// Baseline loader for GUIDE-009 reuse (Coordinator decision D6); read-only.
pub(crate) fn guide006_engine_baseline(name: &str) -> &'static str {
    match name {
        "guide006_observation_v5_prepared_baseline" => {
            include_str!("guide006_baselines/guide006_observation_v5_prepared_baseline.json")
        }
        "guide006_observation_v5_running_baseline" => {
            include_str!("guide006_baselines/guide006_observation_v5_running_baseline.json")
        }
        "guide006_observation_v5_running_dense_truncated_window_baseline" => include_str!(
            "guide006_baselines/guide006_observation_v5_running_dense_truncated_window_baseline.json"
        ),
        "guide006_observation_v5_completed_without_disposition_baseline" => include_str!(
            "guide006_baselines/guide006_observation_v5_completed_without_disposition_baseline.json"
        ),
        "guide006_observation_v5_cancelled_with_disposition_baseline" => include_str!(
            "guide006_baselines/guide006_observation_v5_cancelled_with_disposition_baseline.json"
        ),
        "guide006_observation_v5_delegated_prepared_baseline" => include_str!(
            "guide006_baselines/guide006_observation_v5_delegated_prepared_baseline.json"
        ),
        "guide006_criterion_citation_invalid_details_baseline" => include_str!(
            "guide006_baselines/guide006_criterion_citation_invalid_details_baseline.json"
        ),
        "guide006_workspace_contract_mismatch_details_baseline" => include_str!(
            "guide006_baselines/guide006_workspace_contract_mismatch_details_baseline.json"
        ),
        unknown => panic!("unknown GUIDE-006 Engine baseline: {unknown}"),
    }
}

fn guide006_assert_baseline(name: &str, actual: &Guide006EngineActual) {
    let baseline = guide006_engine_baseline(name);
    let normalized = actual.baseline_text();
    if baseline != normalized {
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("guide006");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{name}.actual"));
        fs::write(&path, normalized).unwrap();
        panic!(
            "GUIDE-006 Engine baseline mismatch for {name}; the normalized actual was written to {}",
            path.display()
        );
    }
}

struct Guide006Engine {
    temporary: TempDir,
    selector: WorktreeSelectorV1,
    store: DirectStoreV1,
    resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
    ids: Guide006Ids,
}

impl Guide006Engine {
    fn bare(path: &str, source: &[u8]) -> Self {
        let (temporary, selector, store, _) = fixture();
        fs::write(temporary.path().join(path), source).unwrap();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        Self {
            temporary,
            selector,
            store,
            resolver,
            ids: Guide006Ids::new(),
        }
    }

    fn new(path: &str, source: &[u8]) -> Self {
        let engine = Self::bare(path, source);
        engine.start_session(path, source, "guide006-start", &|_| {});
        engine
    }

    fn start_session<F: Fn(&mut DirectRequestV1)>(
        &self,
        path: &str,
        source: &[u8],
        key: &str,
        adjust: &F,
    ) {
        let admitted = prepare_workspace_procedure_source_v2(path, source).unwrap();
        self.success(
            DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: path.into(),
                    },
                    expected_procedure_digest: Some(admitted.digest().clone()),
                    task_title: "Guide006 generation-1 baseline".into(),
                    dry_run: false,
                },
                preconditions: None,
            })),
            key,
            adjust,
        );
    }

    fn request<F: Fn(&mut DirectRequestV1)>(
        &self,
        command: DirectCommandV1,
        key: &str,
        adjust: &F,
    ) -> DirectRequestV1 {
        let mut request = DirectRequestV1 {
            selector: self.selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(GUIDE006_REQUEST_ID).unwrap(),
            command,
            idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new(GUIDE006_OBSERVED_AT).unwrap(),
            timeout_ms: 5000,
        };
        adjust(&mut request);
        request
    }

    fn execute<F: Fn(&mut DirectRequestV1)>(
        &self,
        command: DirectCommandV1,
        key: &str,
        adjust: &F,
    ) -> DirectResponseV1 {
        execute_direct_v1(
            &self.resolver,
            self.request(command, key, adjust),
            &Clock,
            &self.ids,
        )
        .unwrap()
    }

    fn success<F: Fn(&mut DirectRequestV1)>(
        &self,
        command: DirectCommandV1,
        key: &str,
        adjust: &F,
    ) -> Value {
        match self.execute(command, key, adjust) {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => {
                panic!("unexpected Engine rejection: {}", error.into_value())
            }
        }
    }

    fn failure<F: Fn(&mut DirectRequestV1)>(
        &self,
        command: DirectCommandV1,
        key: &str,
        adjust: &F,
    ) -> Value {
        match self.execute(command, key, adjust) {
            DirectResponseV1::Failure(error) => error.into_value(),
            DirectResponseV1::Success(output) => {
                panic!("unexpected Engine success: {}", output.as_value())
            }
        }
    }

    fn graph(&self) -> GraphSessionStateV2 {
        self.store
            .prepare_graph(deadline())
            .unwrap()
            .unwrap()
            .graph()
            .clone()
    }

    fn fences(&self) -> SessionMutationPreconditionsWireV1 {
        let graph = self.graph();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
        }
    }

    fn identity_fences(&self) -> SessionIdentityPreconditionsWireV1 {
        let graph = self.graph();
        SessionIdentityPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
        }
    }

    fn item_revision(&self, item: &str) -> Revision {
        let graph = self.graph();
        let attempt = graph.trace().active_attempt().unwrap().attempt_id();
        let item = ItemId::new(item).unwrap();
        graph
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == attempt)
            .unwrap()
            .item_slots()
            .iter()
            .find(|slot| slot.item_id() == &item)
            .unwrap()
            .revision()
    }

    fn begin<F: Fn(&mut DirectRequestV1)>(
        &self,
        initial_goal: Option<InitialGoalWireV2>,
        key: &str,
        adjust: &F,
    ) {
        let session_id = self.graph().trace().session_id().clone();
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal,
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: session_id,
                        expected_session_revision: Revision::ZERO,
                    },
                },
            )),
            key,
            adjust,
        );
    }

    fn set<F: Fn(&mut DirectRequestV1)>(&self, item: &str, value: &str, key: &str, adjust: &F) {
        let graph = self.graph();
        let attempt = graph.trace().active_attempt().unwrap().attempt_id().clone();
        self.success(
            DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
                item_id: ItemId::new(item).unwrap(),
                value: value.into(),
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: graph.trace().session_id().clone(),
                    expected_attempt_id: attempt,
                    expected_item_revision: self.item_revision(item),
                },
            })),
            key,
            adjust,
        );
    }

    fn check<F: Fn(&mut DirectRequestV1)>(&self, item: &str, key: &str, adjust: &F) {
        let graph = self.graph();
        let attempt = graph.trace().active_attempt().unwrap().attempt_id().clone();
        self.success(
            DirectCommandV1::Item(DirectItemCommandV1::Check(ItemCheckV1 {
                item_id: ItemId::new(item).unwrap(),
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: graph.trace().session_id().clone(),
                    expected_attempt_id: attempt,
                    expected_item_revision: self.item_revision(item),
                },
            })),
            key,
            adjust,
        );
    }

    fn complete<F: Fn(&mut DirectRequestV1)>(&self, key: &str, adjust: &F) {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: self.fences(),
            })),
            key,
            adjust,
        );
    }

    fn observe<F: Fn(&mut DirectRequestV1)>(&self, key: &str, adjust: &F) -> Value {
        let session_id = self.graph().trace().session_id().clone();
        self.success(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::Observe,
                expected_session_id: Some(session_id),
            },
            key,
            adjust,
        )
    }

    fn worktree_root(&self) -> String {
        fs::canonicalize(self.temporary.path())
            .unwrap()
            .display()
            .to_string()
    }

    fn workspace_uuid(&self) -> String {
        self.store
            .read(deadline(), |snapshot| {
                Ok(snapshot.binding().identity().workspace_uuid().to_string())
            })
            .unwrap()
    }

    fn actual(&self, envelope: Value) -> Guide006EngineActual {
        Guide006EngineActual {
            raw: serde_json::to_string(&envelope).unwrap(),
            worktree_root: self.worktree_root(),
            workspace_uuid: self.workspace_uuid(),
        }
    }
}

fn is_uuid_shaped(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let mut rest = value;
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            let (dash, tail) = rest.split_at(1);
            if dash != "-" {
                return false;
            }
            rest = tail;
        }
        if rest.len() < *width {
            return false;
        }
        let (group, tail) = rest.split_at(*width);
        if !group
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return false;
        }
        rest = tail;
    }
    rest.is_empty()
}

fn is_timestamp_shaped(value: &str) -> bool {
    fn digits(bytes: &[u8]) -> bool {
        !bytes.is_empty() && bytes.iter().all(|byte| byte.is_ascii_digit())
    }
    let bytes = value.as_bytes();
    bytes.len() == 24
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && digits(&bytes[0..4])
        && digits(&bytes[5..7])
        && digits(&bytes[8..10])
        && digits(&bytes[11..13])
        && digits(&bytes[14..16])
        && digits(&bytes[17..19])
        && digits(&bytes[20..23])
}

fn collect_shaped_strings(value: &Value, shaped: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if is_uuid_shaped(text) || is_timestamp_shaped(text) {
                shaped.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_shaped_strings(item, shaped);
            }
        }
        Value::Object(members) => {
            for member in members.values() {
                collect_shaped_strings(member, shaped);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub(crate) fn guide006_observe_prepared<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::new("work.json", WORK_PROCEDURE);
    engine.actual(engine.observe("guide006-observe", adjust))
}

pub(crate) fn guide006_observe_running<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::new("work.json", WORK_PROCEDURE);
    engine.begin(None, "guide006-begin", adjust);
    engine.set("note", "A recorded note.", "guide006-set-note", adjust);
    engine.actual(engine.observe("guide006-observe", adjust))
}

pub(crate) fn guide006_observe_running_dense_truncated_window<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let source = dense_procedure();
    let engine = Guide006Engine::new("dense.json", &source);
    engine.begin(None, "guide006-begin", adjust);
    engine.actual(engine.observe("guide006-observe", adjust))
}

pub(crate) fn guide006_observe_completed_without_disposition<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::new("work.json", WORK_PROCEDURE);
    engine.begin(None, "guide006-begin", adjust);
    engine.check("confirm", "guide006-check-confirm", adjust);
    engine.complete("guide006-complete", adjust);
    engine.actual(engine.observe("guide006-observe", adjust))
}

pub(crate) fn guide006_observe_cancelled_with_disposition<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::new("work.json", WORK_PROCEDURE);
    engine.begin(None, "guide006-begin", adjust);
    engine.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "Cancelled by GUIDE-006.".into(),
            preconditions: engine.fences(),
        })),
        "guide006-cancel",
        adjust,
    );
    engine.success(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
            SessionTerminalDispositionV1 {
                disposition: TerminalDispositionInputV1::NotRequired {
                    reason: "No external hand-off required.".into(),
                },
                actor: Some("owner".into()),
                preconditions: engine.identity_fences(),
            },
        )),
        "guide006-disposition",
        adjust,
    );
    engine.actual(engine.observe("guide006-observe", adjust))
}

pub(crate) fn guide006_observe_delegated_prepared<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::new("delegated.json", DELEGATED_PROCEDURE);
    engine.actual(engine.observe("guide006-observe", adjust))
}

pub(crate) fn guide006_criterion_citation_invalid<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::new("goal.json", GOAL_PROCEDURE);
    engine.begin(
        Some(InitialGoalWireV2 {
            goal: "Verify the retained goal.".into(),
            criteria: vec![GoalCriterionWireV2 {
                criterion_id: CriterionId::new("correct").unwrap(),
                statement: "The result is correct.".into(),
            }],
            actor: Some("owner".into()),
        }),
        "guide006-begin",
        adjust,
    );
    engine.set(
        "result",
        "Evidence supports this result.",
        "guide006-set-result",
        adjust,
    );
    engine.complete("guide006-complete", adjust);
    engine.set(
        "assessment-note",
        "Decision-local evidence.",
        "guide006-set-assessment-note",
        adjust,
    );
    let expected_goal_revision = engine
        .graph()
        .goal_state()
        .current_revision()
        .map_or(1, |revision| revision.get());
    engine.actual(engine.failure(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriterion(
            GoalAssessCriterionV2 {
                criterion_id: CriterionId::new("correct").unwrap(),
                status: "satisfied".into(),
                reason: "The cited item has no recorded value.".into(),
                evidence: vec![],
                items: vec![ItemId::new("unrecorded-note").unwrap()],
                actor: Some("owner".into()),
                preconditions: engine.fences(),
                expected_goal_revision,
            },
        )),
        "guide006-citation",
        adjust,
    ))
}

pub(crate) fn guide006_workspace_contract_mismatch<F: Fn(&mut DirectRequestV1)>(
    adjust: &F,
) -> Guide006EngineActual {
    let engine = Guide006Engine::bare("work.json", WORK_PROCEDURE);
    // Capture the volatile context first: rewriting the writer row makes every
    // later Store read, including the binding lookup, fail with the same
    // mismatch this scenario pins.
    let worktree_root = engine.worktree_root();
    let workspace_uuid = engine.workspace_uuid();
    let connection = rusqlite::Connection::open(engine.store.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE writer_contract SET contract_manifest_digest=?1 WHERE singleton=1",
            [format!("sha256:{}", "f".repeat(64))],
        )
        .unwrap();
    drop(connection);
    let envelope = engine.failure(
        DirectCommandV1::Read {
            command: DirectReadCommandV1::Status {
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: None,
        },
        "guide006-status",
        adjust,
    );
    Guide006EngineActual {
        raw: serde_json::to_string(&envelope).unwrap(),
        worktree_root,
        workspace_uuid,
    }
}

#[test]
fn guide006_observation_v5_prepared_baseline() {
    let actual = guide006_observe_prepared(&|_| {});
    guide006_assert_baseline("guide006_observation_v5_prepared_baseline", &actual);
}

#[test]
fn guide006_observation_v5_running_baseline() {
    let actual = guide006_observe_running(&|_| {});
    guide006_assert_baseline("guide006_observation_v5_running_baseline", &actual);
}

#[test]
fn guide006_observation_v5_running_dense_truncated_window_baseline() {
    let actual = guide006_observe_running_dense_truncated_window(&|_| {});
    let envelope: Value = serde_json::from_str(&actual.raw).unwrap();
    assert_eq!(envelope["result"]["active_items_total"], json!(128));
    assert_eq!(envelope["result"]["active_items_truncated"], json!(false));
    assert_eq!(envelope["result"]["mutation_templates_total"], json!(133));
    assert_eq!(
        envelope["result"]["mutation_templates_truncated"],
        json!(true)
    );
    guide006_assert_baseline(
        "guide006_observation_v5_running_dense_truncated_window_baseline",
        &actual,
    );
}

#[test]
fn guide006_observation_v5_completed_without_disposition_baseline() {
    let actual = guide006_observe_completed_without_disposition(&|_| {});
    guide006_assert_baseline(
        "guide006_observation_v5_completed_without_disposition_baseline",
        &actual,
    );
}

#[test]
fn guide006_observation_v5_cancelled_with_disposition_baseline() {
    let actual = guide006_observe_cancelled_with_disposition(&|_| {});
    guide006_assert_baseline(
        "guide006_observation_v5_cancelled_with_disposition_baseline",
        &actual,
    );
}

#[test]
fn guide006_observation_v5_delegated_prepared_baseline() {
    let actual = guide006_observe_delegated_prepared(&|_| {});
    guide006_assert_baseline(
        "guide006_observation_v5_delegated_prepared_baseline",
        &actual,
    );
}

#[test]
fn guide006_criterion_citation_invalid_details_baseline() {
    let actual = guide006_criterion_citation_invalid(&|_| {});
    guide006_assert_baseline(
        "guide006_criterion_citation_invalid_details_baseline",
        &actual,
    );
}

#[test]
fn guide006_workspace_contract_mismatch_details_baseline() {
    let actual = guide006_workspace_contract_mismatch(&|_| {});
    guide006_assert_baseline(
        "guide006_workspace_contract_mismatch_details_baseline",
        &actual,
    );
}

#[test]
fn guide006_engine_perturbed_actuals_differ_from_their_baselines() {
    let running = guide006_observe_running(&|_| {});
    let running_baseline = guide006_engine_baseline("guide006_observation_v5_running_baseline");
    assert_eq!(running.baseline_text(), running_baseline);

    // Control: the unperturbed envelope through the same parse-and-reserialize
    // path the perturbations below use still normalizes to the baseline.
    let reserialized = Guide006EngineActual {
        raw: serde_json::to_string(&serde_json::from_str::<Value>(&running.raw).unwrap()).unwrap(),
        worktree_root: running.worktree_root.clone(),
        workspace_uuid: running.workspace_uuid.clone(),
    };
    assert_eq!(reserialized.baseline_text(), running_baseline);

    // One mutation-template argv element changed.
    let mut envelope: Value = serde_json::from_str(&running.raw).unwrap();
    envelope["result"]["mutation_templates"][0]["argv"]
        .as_array_mut()
        .unwrap()
        .push(json!("--guide006-perturbation"));
    let perturbed = Guide006EngineActual {
        raw: serde_json::to_string(&envelope).unwrap(),
        worktree_root: running.worktree_root.clone(),
        workspace_uuid: running.workspace_uuid.clone(),
    };
    assert_ne!(perturbed.baseline_text(), running_baseline);

    // One active-item revision changed.
    let mut envelope: Value = serde_json::from_str(&running.raw).unwrap();
    envelope["result"]["active_items"][0]["revision"] = json!(9);
    let perturbed = Guide006EngineActual {
        raw: serde_json::to_string(&envelope).unwrap(),
        worktree_root: running.worktree_root.clone(),
        workspace_uuid: running.workspace_uuid.clone(),
    };
    assert_ne!(perturbed.baseline_text(), running_baseline);

    // One identifier occurrence replaced by another distinct identifier from the
    // same actual (amendment A2: changing multiplicities, not a consistent swap).
    let perturbed = Guide006EngineActual {
        raw: running
            .raw
            .replacen(GUIDE006_SESSION_ID, GUIDE006_ATTEMPT_IDS[0], 1),
        worktree_root: running.worktree_root.clone(),
        workspace_uuid: running.workspace_uuid.clone(),
    };
    assert_ne!(perturbed.baseline_text(), running_baseline);

    // One result member removed.
    let mut envelope: Value = serde_json::from_str(&running.raw).unwrap();
    envelope["result"]
        .as_object_mut()
        .unwrap()
        .remove("active_items_total");
    let perturbed = Guide006EngineActual {
        raw: serde_json::to_string(&envelope).unwrap(),
        worktree_root: running.worktree_root.clone(),
        workspace_uuid: running.workspace_uuid.clone(),
    };
    assert_ne!(perturbed.baseline_text(), running_baseline);

    // One error-details field changed on the live mismatch envelope.
    let mismatch = guide006_workspace_contract_mismatch(&|_| {});
    let mismatch_baseline =
        guide006_engine_baseline("guide006_workspace_contract_mismatch_details_baseline");
    assert_eq!(mismatch.baseline_text(), mismatch_baseline);
    let mismatch_reserialized = Guide006EngineActual {
        raw: serde_json::to_string(&serde_json::from_str::<Value>(&mismatch.raw).unwrap()).unwrap(),
        worktree_root: mismatch.worktree_root.clone(),
        workspace_uuid: mismatch.workspace_uuid.clone(),
    };
    assert_eq!(mismatch_reserialized.baseline_text(), mismatch_baseline);
    let mut envelope: Value = serde_json::from_str(&mismatch.raw).unwrap();
    envelope["details"]["actual_contract_manifest_digest"] = json!("sha256:guide006-perturbation");
    let perturbed = Guide006EngineActual {
        raw: serde_json::to_string(&envelope).unwrap(),
        worktree_root: mismatch.worktree_root.clone(),
        workspace_uuid: mismatch.workspace_uuid.clone(),
    };
    assert_ne!(perturbed.baseline_text(), mismatch_baseline);
}

#[test]
fn guide006_engine_normalizer_requires_the_worktree_root_and_hides_every_volatile_literal() {
    fn rejection_reason(rejected: std::thread::Result<()>) -> String {
        let payload = rejected.expect_err("the normalizer must reject this input");
        if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_owned()
        } else {
            panic!("the rejection payload carried no message");
        }
    }

    // (a) A success-shaped text whose workspace root is not the fixture
    // worktree root is rejected for exactly that reason.
    let reason = rejection_reason(std::panic::catch_unwind(|| {
        guide006_normalize_engine(
            "{\"workspace\":{\"root\":\"elsewhere\"},\"result\":{}}",
            "/absent/root",
        );
    }));
    assert!(
        reason.contains("the Engine envelope workspace root must be the fixture worktree root"),
        "{reason}"
    );

    // A success-shaped text that contains the worktree root twice is rejected
    // for exactly that reason.
    let reason = rejection_reason(std::panic::catch_unwind(|| {
        guide006_normalize_engine(
            "{\"workspace\":{\"root\":\"/absent/root\"},\"result\":{\"note\":\"/absent/root\"}}",
            "/absent/root",
        );
    }));
    assert!(
        reason.contains("the Engine envelope must contain exactly one worktree-root occurrence"),
        "{reason}"
    );

    // An error-shaped text without a workspace member that still contains the
    // worktree root is rejected for exactly that reason.
    let reason = rejection_reason(std::panic::catch_unwind(|| {
        guide006_normalize_engine(
            "{\"code\":\"X\",\"details\":{\"path\":\"/absent/root\"}}",
            "/absent/root",
        );
    }));
    assert!(
        reason.contains(
            "an Engine envelope without a workspace member must not contain the worktree root"
        ),
        "{reason}"
    );

    // (b) The derivation tie: the reused Clock is exactly GUIDE006_NOW + 1.
    assert_eq!(Clock.now().get(), GUIDE006_NOW + 1);

    // Every UUID-shaped or timestamp-shaped literal that survives Engine
    // normalization must be one of the injected values (amendment A1).
    let actual = guide006_observe_running(&|_| {});
    let mut shaped = Vec::new();
    collect_shaped_strings(
        &serde_json::from_str::<Value>(&actual.normalized()).unwrap(),
        &mut shaped,
    );
    let mut allowed: Vec<String> = vec![actual.workspace_uuid.clone()];
    allowed.extend(
        [
            GUIDE006_WORKSPACE_ID,
            GUIDE006_SESSION_ID,
            GUIDE006_SNAPSHOT_ID,
            GUIDE006_BLOCKER_ID,
            GUIDE006_RECEIPT_ID,
            GUIDE006_REQUEST_ID,
        ]
        .map(str::to_owned),
    );
    allowed.extend(GUIDE006_ATTEMPT_IDS.map(str::to_owned));
    allowed.extend([
        Rfc3339MillisV1::from_unix_millis(GUIDE006_NOW)
            .unwrap()
            .as_str()
            .to_owned(),
        Rfc3339MillisV1::from_unix_millis(GUIDE006_NOW + 1)
            .unwrap()
            .as_str()
            .to_owned(),
    ]);
    for literal in shaped {
        assert!(
            allowed.contains(&literal),
            "an unexpected shaped literal survived Engine normalization: {literal}"
        );
    }
}
