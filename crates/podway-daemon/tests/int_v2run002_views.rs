//! Focused V2RUN-002 coverage for deterministic Procedure v2 status and next projections.

use podway_core::{
    ActorAttributionV2, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, BlockerId,
    BlockerState, CriterionAssessmentReasonV2, CriterionAssessmentResultV2, CriterionCitationV2,
    CriterionId, CriterionStatusV2, DecisionRecordInputV2, DecisionRecordV2,
    EvidenceReferenceSnapshotV2, GoalAssessmentRecordV2, GoalCriterionV2, GoalDefinitionV2,
    GoalOutcome, GoalRevisionNumberV2, GoalRevisionRecordV2, GoalStatementV2, GraphNodeId, ItemId,
    NodeDefinitionId, OptionId, ProcedureSnapshotId, ReasonV2, RecordedItemValueV2,
    ResolvedEvidenceReferenceV2, ResolvedEvidenceSetV2, Revision, SessionAttemptV2, SessionId,
    SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2, TransitionEffectV2,
    UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_daemon::{
    execution::{
        ExecutionBoundaryErrorV1, ProcedureProviderV1, ProcedureV2SourceAdmissionErrorV1,
        prepare_custom_procedure_v2_start, workspace_procedure_snapshot_from_bytes_v2,
    },
    v2_read_service::{GraphStatusTierV2, project_graph_next_v2, project_graph_status_v2},
};
use podway_protocol::{
    CommandNameV1, OutputEnvelopeInputV2, OutputEnvelopeV2, RequestIdV1, Rfc3339MillisV1,
    validate_frame_payload_length,
};
use podway_store::{
    AttemptCriterionAssessmentStateV2, AttemptMetadataV2, AttemptWorkflowMemoryV2, BlockerStateV2,
    CriterionAssessmentStateV2, DurableWorktreeIdentityV1, EvidenceResolutionStateV2, GoalStateV2,
    GraphNodeCounterV2, GraphSessionStateV2, GraphWorkspaceViewV2, ItemSlotStateV2,
    ProcedureSnapshotV2, ValidatedWorkspaceRootV1, WorkflowMemoryStateV2, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const EQUIVALENT_YAML: &[u8] =
    include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml");
const EQUIVALENT_JSON: &[u8] =
    include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.json");
const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000002001";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000002002";
const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000002003";
const SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000002004";
const REQUEST_ID: &str = "00000000-0000-4000-8000-000000002005";
const IDENTITY_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ASSESSMENT_PROCEDURE: &[u8] = br#"{
  "schema":"podway.procedure/v2",
  "id":"assessment-view",
  "version":"2",
  "name":"Assessment view",
  "purpose":"Exercise deterministic assessment guidance.",
  "goal_tracking":true,
  "node_definitions":{
    "assess":{
      "type":"decision",
      "title":"Assess goal",
      "objective":"Determine the goal outcome.",
      "prompt":"Which outcome applies?",
      "items":[{"id":"note","type":"text","prompt":"Record assessment evidence.","required":false}],
      "options":[
        {"id":"achieved","label":"Achieved"},
        {"id":"not-achieved","label":"Not achieved"},
        {"id":"superseded","label":"Superseded"}
      ],
      "reason":{"required":true},
      "assessment":{"target":"session_goal","outcomes":{
        "achieved":"achieved",
        "not-achieved":"not_achieved",
        "superseded":"superseded"
      }}
    },
    "finish":{"type":"action","title":"Finish","intent":"Finish the work."}
  },
  "graph":{
    "entry":"assess-goal",
    "nodes":[
      {"id":"assess-goal","use":"assess","routes":{
        "achieved":{"to":"finish","effect":"advance"},
        "not-achieved":{"to":"finish","effect":"advance"},
        "superseded":{"to":"finish","effect":"advance"}
      }},
      {"id":"finish","use":"finish","evidence_from":[{"node":"assess-goal","required":true}],"terminal":true}
    ]
  }
}"#;
const REASSESSMENT_PROCEDURE: &[u8] = br#"{
  "schema":"podway.procedure/v2",
  "id":"reassessment-view",
  "version":"2",
  "name":"Reassessment view",
  "purpose":"Exercise a fresh assessment after an earlier completed assessment.",
  "goal_tracking":true,
  "node_definitions":{
    "assess":{
      "type":"decision",
      "title":"Assess goal",
      "objective":"Determine the goal outcome.",
      "prompt":"Which outcome applies?",
      "items":[{"id":"note","type":"text","prompt":"Record assessment evidence.","required":false}],
      "options":[
        {"id":"achieved","label":"Achieved"},
        {"id":"not-achieved","label":"Not achieved"},
        {"id":"superseded","label":"Superseded"}
      ],
      "reason":{"required":true},
      "assessment":{"target":"session_goal","outcomes":{
        "achieved":"achieved",
        "not-achieved":"not_achieved",
        "superseded":"superseded"
      }}
    },
    "work":{"type":"action","title":"Work","intent":"Continue the work."},
    "finish":{"type":"action","title":"Finish","intent":"Finish the work."}
  },
  "graph":{
    "entry":"assess-first",
    "nodes":[
      {"id":"assess-first","use":"assess","routes":{
        "achieved":{"to":"work","effect":"advance"},
        "not-achieved":{"to":"work","effect":"advance"},
        "superseded":{"to":"work","effect":"advance"}
      }},
      {"id":"work","use":"work","next":"assess-current"},
      {"id":"assess-current","use":"assess","routes":{
        "achieved":{"to":"finish","effect":"advance"},
        "not-achieved":{"to":"finish","effect":"advance"},
        "superseded":{"to":"finish","effect":"advance"}
      }},
      {"id":"finish","use":"finish","terminal":true}
    ]
  }
}"#;

#[derive(Clone, Copy)]
struct ByteProcedureV2<'a> {
    path: &'a str,
    source: &'a [u8],
}

impl ProcedureProviderV1 for ByteProcedureV2<'_> {
    fn load_preset_snapshot(
        &self,
        _preset: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<podway_core::ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        panic!("the focused custom Procedure v2 fixture must not resolve a preset")
    }

    fn load_workspace_procedure_snapshot(
        &self,
        _workspace: &WorkspaceBindingV1,
        _procedure: &str,
        _snapshot_id: ProcedureSnapshotId,
        _created_at: UnixMillis,
    ) -> Result<podway_core::ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        panic!("the focused Procedure v2 fixture must not enter the retained v1 loader")
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

fn identity() -> DurableWorktreeIdentityV1 {
    let digest = Sha256Digest::new(IDENTITY_DIGEST).unwrap();
    DurableWorktreeIdentityV1::new(
        digest.clone(),
        WorkspaceId::new(WORKSPACE_ID).unwrap(),
        digest,
    )
}

fn binding() -> WorkspaceBindingV1 {
    WorkspaceBindingV1::new(
        identity(),
        ValidatedWorkspaceRootV1::from_encoded("podway.unix-path/v1:2f776f726b74726565").unwrap(),
    )
}

fn fresh_state(path: &str, source: &[u8]) -> GraphSessionStateV2 {
    let provider = ByteProcedureV2 { path, source };
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        path,
        source,
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap();
    prepare_custom_procedure_v2_start(
        &provider,
        &binding(),
        path,
        Some(snapshot.digest()),
        "Project deterministic Procedure v2 views",
        SessionId::new(SESSION_ID).unwrap(),
        AttemptId::new(ATTEMPT_ID).unwrap(),
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap()
}

fn view(state: GraphSessionStateV2) -> GraphWorkspaceViewV2 {
    GraphWorkspaceViewV2::new(
        identity(),
        Some(state),
        0,
        None,
        1,
        UnixMillis::new(1_700_000_000_100),
    )
}

fn output(command: &str, result: Map<String, Value>) -> OutputEnvelopeV2 {
    OutputEnvelopeV2::new(OutputEnvelopeInputV2 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-09T00:00:00.000Z").unwrap(),
        workspace: None,
        job: None,
        session: None,
        result,
        warnings: Vec::new(),
    })
    .unwrap()
}

fn assert_output_v2(command: &str, result: Map<String, Value>) {
    let output = output(command, result);
    let encoded = serde_json::to_vec(&output).unwrap();
    validate_frame_payload_length(encoded.len()).unwrap();
    let decoded: OutputEnvelopeV2 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, output);
}

#[test]
fn v2run002_fresh_action_projects_closed_compact_standard_verbose_and_next_views() {
    let view = view(fresh_state("workflow.yaml", EQUIVALENT_YAML));
    let compact = project_graph_status_v2(&view, GraphStatusTierV2::Compact, None).unwrap();
    let standard = project_graph_status_v2(&view, GraphStatusTierV2::Standard, None).unwrap();
    let verbose = project_graph_status_v2(&view, GraphStatusTierV2::Verbose, None).unwrap();
    let next = project_graph_next_v2(&view).unwrap();

    assert_eq!(compact["schema"], "podway.compact-status-result/v2");
    assert_eq!(standard["schema"], "podway.status-result/v2");
    assert_eq!(standard["tier"], "standard");
    assert_eq!(verbose["schema"], "podway.status-result/v2");
    assert_eq!(verbose["tier"], "verbose");
    assert_eq!(next["schema"], "podway.next-result/v2");

    for projection in [&compact, &standard, &verbose, &next] {
        assert_eq!(projection["trace_length"], 1);
        assert_eq!(projection["counters"].as_array().unwrap().len(), 3);
        assert_eq!(projection["queue"]["pending_mutations"], false);
        assert_eq!(projection["queue"]["queued_count"], 0);
        assert!(projection["queue"]["running_job_id"].is_null());
        assert_eq!(projection["queue"]["latest_workspace_sequence"], 1);
    }
    for projection in [&compact, &standard, &verbose] {
        assert_eq!(projection["session"]["id"], SESSION_ID);
        assert_eq!(projection["session"]["revision"], 1);
        assert_eq!(projection["current"]["node"]["graph_node_id"], "perform");
        assert_eq!(projection["current"]["attempt"]["attempt_id"], ATTEMPT_ID);
        assert_eq!(projection["current"]["missing_required_item_count"], 1);
    }
    assert_eq!(next["node"]["graph_node_id"], "perform");
    assert_eq!(next["attempt"]["attempt_id"], ATTEMPT_ID);
    assert_eq!(next["missing_required_item_count"], 1);
    assert_eq!(next["missing_required_items"][0]["item_id"], "result");
    assert_eq!(next["goal_tracking"], true);
    assert_eq!(next["goal_defined"], false);
    let suggestion_commands = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["command"].as_str().unwrap())
        .collect::<Vec<_>>();
    for expected in ["item.set", "session.retry", "goal.define"] {
        assert!(
            suggestion_commands.contains(&expected),
            "fresh guidance omitted {expected}"
        );
    }
    assert!(
        next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|suggestion| suggestion["command"] == "item.set"
                && suggestion["item_id"] == "result")
    );
    for forbidden in [
        "purpose",
        "goal",
        "missing_required_item_ids",
        "blocker_window",
        "item_values",
        "references",
        "current_trace_history",
        "stale_attempt_history",
        "decision_history",
        "rework_history",
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
        "suggestions",
        "readback",
        "instructions",
        "prompt",
    ] {
        assert!(
            !compact.contains_key(forbidden),
            "compact status leaked {forbidden}"
        );
    }
    assert_eq!(
        verbose["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(verbose["current_trace_history"]["trace_truncated"], false);
    assert_eq!(
        verbose["current_trace_history"]["trace_window"]["first_sequence"],
        1
    );
    assert_eq!(
        verbose["current_trace_history"]["trace_window"]["last_sequence"],
        1
    );
    for history in [
        "stale_attempt_history",
        "decision_history",
        "rework_history",
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
    ] {
        assert!(!standard.contains_key(history));
        assert_eq!(verbose[history]["trace_truncated"], false);
        assert!(verbose[history]["trace_window"].is_null());
    }

    let compact_bytes = serde_json::to_vec(&output("session.status", compact.clone())).unwrap();
    assert!(
        compact_bytes.len() <= 262_144,
        "compact status exceeded its automation envelope cap: {} bytes",
        compact_bytes.len()
    );
    assert_output_v2("session.status", compact);
    assert_output_v2("session.status", standard);
    assert_output_v2("session.status", verbose);
    assert_output_v2("session.next", next);
}

#[test]
fn v2run002_projection_is_bound_to_the_canonical_snapshot_not_source_encoding() {
    let yaml = view(fresh_state("workflow.yaml", EQUIVALENT_YAML));
    let json = view(fresh_state("workflow.json", EQUIVALENT_JSON));

    for tier in [
        GraphStatusTierV2::Compact,
        GraphStatusTierV2::Standard,
        GraphStatusTierV2::Verbose,
    ] {
        assert_eq!(
            project_graph_status_v2(&yaml, tier, None).unwrap(),
            project_graph_status_v2(&json, tier, None).unwrap()
        );
    }
    assert_eq!(
        project_graph_next_v2(&yaml).unwrap(),
        project_graph_next_v2(&json).unwrap()
    );
}

#[test]
fn v2run002_history_cursor_is_exclusive_and_shared_by_every_verbose_window() {
    let view = view(fresh_state("workflow.yaml", EQUIVALENT_YAML));
    let verbose = project_graph_status_v2(
        &view,
        GraphStatusTierV2::Verbose,
        Some(TraceSequenceV2::FIRST),
    )
    .unwrap();

    assert!(
        verbose["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(verbose["current_trace_history"]["trace_truncated"], false);
    assert!(verbose["current_trace_history"]["trace_window"].is_null());
    for history in [
        "stale_attempt_history",
        "decision_history",
        "rework_history",
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
    ] {
        assert!(verbose[history]["entries"].as_array().unwrap().is_empty());
        assert_eq!(verbose[history]["trace_truncated"], false);
        assert!(verbose[history]["trace_window"].is_null());
    }
}

fn blocker_and_item_state() -> GraphSessionStateV2 {
    let source = String::from_utf8(EQUIVALENT_YAML.to_vec())
        .unwrap()
        .replace("max_length: 1000", "max_length: 16384");
    let state = fresh_state("workflow.yaml", source.as_bytes());
    let original_attempt = state.workflow_memory().attempts().first().unwrap();
    let original_slot = original_attempt.item_slots().first().unwrap();
    let value = "값".repeat(3_000);
    let item_slot = ItemSlotStateV2::new(
        original_slot.attempt_id().clone(),
        original_slot.item_id().clone(),
        original_slot.item_type(),
        Revision::new(1),
        Some(RecordedItemValueV2::text(value).unwrap()),
        original_slot.created_at(),
        UnixMillis::new(original_slot.created_at().get() + 1),
    )
    .unwrap();
    let blockers = (0_u64..64)
        .map(|index| {
            BlockerStateV2::new(
                BlockerId::new(format!("00000000-0000-4000-8000-{index:012x}")).unwrap(),
                original_attempt.attempt_id().clone(),
                format!("{index:02}-{}", "b".repeat(997)),
                BlockerState::Open,
                UnixMillis::new(1_700_000_001_000 + index),
                None,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let attempt = AttemptWorkflowMemoryV2::new(
        original_attempt.attempt_id().clone(),
        vec![item_slot],
        blockers,
        original_attempt.evidence().to_vec(),
    )
    .unwrap();
    let workflow_memory =
        WorkflowMemoryStateV2::new(vec![attempt], Vec::new(), Vec::new()).unwrap();
    GraphSessionStateV2::new_with_goal_state(
        state.workspace_revision(),
        state.task_title(),
        state.snapshot().clone(),
        state.trace().clone(),
        state.counters().to_vec(),
        state.attempt_metadata().to_vec(),
        workflow_memory,
        state.goal_state().clone(),
        state.created_at(),
        state.completed_at(),
        state.cancelled_at(),
        state.cancel_reason().map(str::to_owned),
    )
    .unwrap()
}

#[test]
fn v2run002_blocker_and_item_windows_are_newest_first_complete_and_display_bounded() {
    let view = view(blocker_and_item_state());

    let standard = project_graph_status_v2(&view, GraphStatusTierV2::Standard, None).unwrap();
    assert_eq!(standard["current"]["blockers_total"], 64);
    assert_eq!(standard["blockers_truncated"], true);
    let blockers = standard["blocker_window"].as_array().unwrap();
    assert!(!blockers.is_empty() && blockers.len() < 64);
    assert_eq!(
        blockers.first().unwrap()["blocker_id"],
        "00000000-0000-4000-8000-00000000003f"
    );
    for pair in blockers.windows(2) {
        assert!(pair[0]["created_at"].as_str().unwrap() > pair[1]["created_at"].as_str().unwrap());
    }
    assert_eq!(standard["items_total"], 1);
    assert_eq!(standard["items_truncated"], false);
    assert_eq!(
        standard["item_values"][0]["value"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        2_048
    );
    assert_eq!(standard["item_values"][0]["value_truncated"], true);

    let next = project_graph_next_v2(&view).unwrap();
    assert_eq!(next["blockers_total"], 64);
    assert_eq!(next["blockers_truncated"], true);
    assert_eq!(next["blockers"], standard["blocker_window"]);
    assert!(
        !next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.block"))
    );

    assert_output_v2("session.status", standard);
    assert_output_v2("session.next", next);
}

#[test]
fn v2run002_missing_graph_and_nonidle_compact_reads_fail_closed() {
    let missing = GraphWorkspaceViewV2::new(
        identity(),
        None,
        0,
        None,
        1,
        UnixMillis::new(1_700_000_000_100),
    );
    assert!(project_graph_status_v2(&missing, GraphStatusTierV2::Compact, None).is_err());
    assert!(project_graph_status_v2(&missing, GraphStatusTierV2::Standard, None).is_err());
    assert!(project_graph_next_v2(&missing).is_err());

    let busy = GraphWorkspaceViewV2::new(
        identity(),
        Some(fresh_state("workflow.yaml", EQUIVALENT_YAML)),
        1,
        None,
        2,
        UnixMillis::new(1_700_000_000_100),
    );
    assert!(project_graph_status_v2(&busy, GraphStatusTierV2::Compact, None).is_err());
    let standard = project_graph_status_v2(&busy, GraphStatusTierV2::Standard, None).unwrap();
    assert_eq!(standard["queue"]["pending_mutations"], true);
    assert_eq!(standard["queue"]["queued_count"], 1);
    assert_eq!(standard["queue"]["latest_workspace_sequence"], 2);
    let next = project_graph_next_v2(&busy).unwrap();
    assert_eq!(next["queue"], standard["queue"]);
}

#[test]
fn v2run002_revision_fields_use_the_session_fence_not_the_workspace_fence() {
    let state = fresh_state("workflow.yaml", EQUIVALENT_YAML);
    let state = GraphSessionStateV2::new_with_goal_state(
        Revision::new(9),
        state.task_title(),
        state.snapshot().clone(),
        state.trace().clone(),
        state.counters().to_vec(),
        state.attempt_metadata().to_vec(),
        state.workflow_memory().clone(),
        state.goal_state().clone(),
        state.created_at(),
        state.completed_at(),
        state.cancelled_at(),
        state.cancel_reason().map(str::to_owned),
    )
    .unwrap();
    let view = view(state);

    let status = project_graph_status_v2(&view, GraphStatusTierV2::Standard, None).unwrap();
    let next = project_graph_next_v2(&view).unwrap();

    assert_eq!(status["session"]["revision"], 1);
    assert_eq!(next["revision"], 1);
}

fn assessment_result(id: &str) -> CriterionAssessmentStateV2 {
    CriterionAssessmentStateV2::new(
        CriterionAssessmentResultV2::new(
            CriterionId::new(id).unwrap(),
            CriterionStatusV2::Satisfied,
            CriterionAssessmentReasonV2::new("The criterion is satisfied.").unwrap(),
            Vec::new(),
        )
        .unwrap(),
        None,
        UnixMillis::new(1_700_000_000_001),
    )
}

fn assessment_goal_definition() -> GoalDefinitionV2 {
    GoalDefinitionV2::new(vec![
        GoalCriterionV2::new(
            CriterionId::new("correct").unwrap(),
            "The result is correct.",
        )
        .unwrap(),
        GoalCriterionV2::new(CriterionId::new("tested").unwrap(), "The result is tested.").unwrap(),
    ])
    .unwrap()
}

fn assessment_goal_revision() -> GoalRevisionRecordV2 {
    GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        None,
        GoalStatementV2::new("Deliver a correct and tested result.").unwrap(),
        assessment_goal_definition(),
        None,
        None,
        false,
        Some(ActorAttributionV2::new("planner").unwrap()),
        TraceSequenceV2::FIRST,
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap()
}

fn goal_assessment_state(results: Vec<CriterionAssessmentStateV2>) -> GraphSessionStateV2 {
    let base = fresh_state("assessment.json", ASSESSMENT_PROCEDURE);
    let attempt = SessionAttemptV2::new(
        AttemptId::new(ATTEMPT_ID).unwrap(),
        podway_core::GraphNodeId::new("assess-goal").unwrap(),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        Some(GoalRevisionNumberV2::FIRST),
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
    )
    .unwrap();
    let attempt_assessments = if results.is_empty() {
        Vec::new()
    } else {
        vec![
            AttemptCriterionAssessmentStateV2::new(
                AttemptId::new(ATTEMPT_ID).unwrap(),
                GoalRevisionNumberV2::FIRST,
                results,
            )
            .unwrap(),
        ]
    };
    let goal = GoalStateV2::new(
        Some(GoalRevisionNumberV2::FIRST),
        vec![assessment_goal_revision()],
        attempt_assessments,
        Vec::new(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        base.workspace_revision(),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        base.counters().to_vec(),
        base.attempt_metadata().to_vec(),
        base.workflow_memory().clone(),
        goal,
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum DecidedFixtureLifecycle {
    Running,
    Completed,
    Cancelled,
}

fn second_attempt_id() -> AttemptId {
    AttemptId::new("00000000-0000-4000-8000-000000002006").unwrap()
}

fn reassessment_state() -> GraphSessionStateV2 {
    let base = fresh_state("reassessment.json", REASSESSMENT_PROCEDURE);
    let first_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let work_attempt_id = second_attempt_id();
    let current_attempt_id = AttemptId::new("00000000-0000-4000-8000-000000002007").unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(3),
        vec![
            SessionAttemptV2::new(
                first_attempt_id.clone(),
                GraphNodeId::new("assess-first").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::FIRST,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap(),
            SessionAttemptV2::new(
                work_attempt_id.clone(),
                GraphNodeId::new("work").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(2),
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap(),
            SessionAttemptV2::new(
                current_attempt_id.clone(),
                GraphNodeId::new("assess-current").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(3),
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap(),
        ],
    )
    .unwrap();

    let first_memory = base.workflow_memory().attempts().first().unwrap().clone();
    let note = first_memory.item_slots().first().unwrap();
    let work_memory =
        AttemptWorkflowMemoryV2::new(work_attempt_id.clone(), Vec::new(), Vec::new(), Vec::new())
            .unwrap();
    let current_memory = AttemptWorkflowMemoryV2::new(
        current_attempt_id.clone(),
        vec![
            ItemSlotStateV2::new(
                current_attempt_id.clone(),
                note.item_id().clone(),
                note.item_type(),
                Revision::ZERO,
                None,
                UnixMillis::new(1_700_000_000_020),
                UnixMillis::new(1_700_000_000_020),
            )
            .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let evidence = ResolvedEvidenceSetV2::new(Vec::new()).unwrap();
    let decision = DecisionRecordV2::new(DecisionRecordInputV2 {
        trace: TraceSequenceV2::FIRST,
        session_id: SessionId::new(SESSION_ID).unwrap(),
        session_revision: Revision::new(2),
        procedure_snapshot_id: base.snapshot().snapshot_id().clone(),
        procedure_digest: base.snapshot().digest().clone(),
        graph_node_id: GraphNodeId::new("assess-first").unwrap(),
        node_definition_id: NodeDefinitionId::new("assess").unwrap(),
        attempt_id: first_attempt_id.clone(),
        attempt_number: AttemptNumberV2::FIRST,
        goal_revision: Some(GoalRevisionNumberV2::FIRST),
        selected_option: OptionId::new("achieved").unwrap(),
        route_effect: TransitionEffectV2::Advance,
        route_target: GraphNodeId::new("work").unwrap(),
        reason: ReasonV2::new("The first assessment was achieved.").unwrap(),
        evidence: evidence.clone(),
        actor: None,
        recorded_at: UnixMillis::new(1_700_000_000_010),
    })
    .unwrap();
    let workflow_memory = WorkflowMemoryStateV2::new(
        vec![first_memory, work_memory, current_memory],
        vec![decision],
        Vec::new(),
    )
    .unwrap();
    let criterion_states = vec![assessment_result("correct"), assessment_result("tested")];
    let goal_assessment = GoalAssessmentRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        GoalOutcome::Achieved,
        criterion_states
            .iter()
            .map(|state| state.result().clone())
            .collect(),
        evidence,
        None,
        first_attempt_id.clone(),
        GraphNodeId::new("assess-first").unwrap(),
        TraceSequenceV2::FIRST,
        UnixMillis::new(1_700_000_000_010),
    )
    .unwrap();
    let goal = GoalStateV2::new(
        Some(GoalRevisionNumberV2::FIRST),
        vec![assessment_goal_revision()],
        vec![
            AttemptCriterionAssessmentStateV2::new(
                first_attempt_id.clone(),
                GoalRevisionNumberV2::FIRST,
                criterion_states,
            )
            .unwrap(),
        ],
        vec![goal_assessment],
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(9),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        vec![
            GraphNodeCounterV2::new(GraphNodeId::new("assess-first").unwrap(), 1, 0),
            GraphNodeCounterV2::new(GraphNodeId::new("work").unwrap(), 1, 0),
            GraphNodeCounterV2::new(GraphNodeId::new("assess-current").unwrap(), 1, 0),
            GraphNodeCounterV2::new(GraphNodeId::new("finish").unwrap(), 0, 0),
        ],
        vec![
            AttemptMetadataV2::new(
                first_attempt_id,
                UnixMillis::new(1_700_000_000_000),
                Some(UnixMillis::new(1_700_000_000_010)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                work_attempt_id,
                UnixMillis::new(1_700_000_000_010),
                Some(UnixMillis::new(1_700_000_000_020)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                current_attempt_id,
                UnixMillis::new(1_700_000_000_020),
                None,
                None,
            )
            .unwrap(),
        ],
        workflow_memory,
        goal,
        UnixMillis::new(1_700_000_000_000),
        None,
        None,
        None,
    )
    .unwrap()
}

fn rich_assessment_result(id: &str, reason: &str) -> CriterionAssessmentStateV2 {
    CriterionAssessmentStateV2::new(
        CriterionAssessmentResultV2::new(
            CriterionId::new(id).unwrap(),
            CriterionStatusV2::Satisfied,
            CriterionAssessmentReasonV2::new(reason).unwrap(),
            vec![CriterionCitationV2::Item(ItemId::new("note").unwrap())],
        )
        .unwrap(),
        None,
        UnixMillis::new(1_700_000_000_002),
    )
}

fn decided_assessment_state(
    lifecycle: DecidedFixtureLifecycle,
    assessment_is_stale: bool,
) -> GraphSessionStateV2 {
    if assessment_is_stale {
        assert!(matches!(lifecycle, DecidedFixtureLifecycle::Cancelled));
        let mut procedure: Value = serde_json::from_slice(ASSESSMENT_PROCEDURE).unwrap();
        procedure.as_object_mut().unwrap().insert(
            "manual_rework".to_owned(),
            json!({"allowed_targets":["assess-goal"]}),
        );
        let procedure = serde_json::to_vec(&procedure).unwrap();
        let finish_attempt_id = second_attempt_id();
        let fresh_assessment_attempt_id =
            AttemptId::new("00000000-0000-4000-8000-000000002008").unwrap();
        let reworked =
            decided_assessment_state_from_source(DecidedFixtureLifecycle::Running, &procedure)
                .manual_rework_v2(
                    Revision::new(2),
                    Some(&finish_attempt_id),
                    GraphNodeId::new("assess-goal").unwrap(),
                    fresh_assessment_attempt_id.clone(),
                    ReasonV2::new("Reassess before cancellation.").unwrap(),
                    None,
                    UnixMillis::new(1_700_000_000_020),
                )
                .unwrap()
                .into_state();
        return reworked
            .cancel_active_session_v2(
                Revision::new(3),
                &fresh_assessment_attempt_id,
                ReasonV2::new("Cancelled by test.").unwrap(),
                UnixMillis::new(1_700_000_000_030),
            )
            .unwrap()
            .into_state();
    }

    decided_assessment_state_from_source(lifecycle, ASSESSMENT_PROCEDURE)
}

fn decided_assessment_state_from_source(
    lifecycle: DecidedFixtureLifecycle,
    procedure: &[u8],
) -> GraphSessionStateV2 {
    let base = fresh_state("assessment.json", procedure);
    let first_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let second_attempt_id = second_attempt_id();
    let finish_validity = match lifecycle {
        DecidedFixtureLifecycle::Running | DecidedFixtureLifecycle::Completed => {
            AttemptValidityV2::Valid
        }
        DecidedFixtureLifecycle::Cancelled => AttemptValidityV2::Stale,
    };
    let finish_lifecycle = match lifecycle {
        DecidedFixtureLifecycle::Running => AttemptLifecycle::Active,
        DecidedFixtureLifecycle::Completed => AttemptLifecycle::Completed,
        DecidedFixtureLifecycle::Cancelled => AttemptLifecycle::Abandoned,
    };
    let session_lifecycle = match lifecycle {
        DecidedFixtureLifecycle::Running => SessionLifecycle::Running,
        DecidedFixtureLifecycle::Completed => SessionLifecycle::Completed,
        DecidedFixtureLifecycle::Cancelled => SessionLifecycle::Cancelled,
    };
    let session_revision = match lifecycle {
        DecidedFixtureLifecycle::Running => Revision::new(2),
        DecidedFixtureLifecycle::Completed | DecidedFixtureLifecycle::Cancelled => Revision::new(3),
    };
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        session_lifecycle,
        session_revision,
        vec![
            SessionAttemptV2::new(
                first_attempt_id.clone(),
                GraphNodeId::new("assess-goal").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::FIRST,
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap(),
            SessionAttemptV2::new(
                second_attempt_id.clone(),
                GraphNodeId::new("finish").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(2),
                finish_lifecycle,
                finish_validity,
                Some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap(),
        ],
    )
    .unwrap();

    let original_assessment_memory = base.workflow_memory().attempts().first().unwrap();
    let original_note = original_assessment_memory.item_slots().first().unwrap();
    let note = ItemSlotStateV2::new(
        first_attempt_id.clone(),
        original_note.item_id().clone(),
        original_note.item_type(),
        Revision::new(1),
        Some(RecordedItemValueV2::text("Assessment evidence.").unwrap()),
        original_note.created_at(),
        UnixMillis::new(1_700_000_000_001),
    )
    .unwrap();
    let assessment_memory =
        AttemptWorkflowMemoryV2::new(first_attempt_id.clone(), vec![note], Vec::new(), Vec::new())
            .unwrap();
    let evidence = ResolvedEvidenceSetV2::new(Vec::new()).unwrap();
    let decision = DecisionRecordV2::new(DecisionRecordInputV2 {
        trace: TraceSequenceV2::FIRST,
        session_id: SessionId::new(SESSION_ID).unwrap(),
        session_revision: Revision::new(2),
        procedure_snapshot_id: base.snapshot().snapshot_id().clone(),
        procedure_digest: base.snapshot().digest().clone(),
        graph_node_id: GraphNodeId::new("assess-goal").unwrap(),
        node_definition_id: NodeDefinitionId::new("assess").unwrap(),
        attempt_id: first_attempt_id.clone(),
        attempt_number: AttemptNumberV2::FIRST,
        goal_revision: Some(GoalRevisionNumberV2::FIRST),
        selected_option: OptionId::new("achieved").unwrap(),
        route_effect: TransitionEffectV2::Advance,
        route_target: GraphNodeId::new("finish").unwrap(),
        reason: ReasonV2::new("Every criterion is satisfied.").unwrap(),
        evidence: evidence.clone(),
        actor: None,
        recorded_at: UnixMillis::new(1_700_000_000_010),
    })
    .unwrap();
    let finish_memory = AttemptWorkflowMemoryV2::new(
        second_attempt_id.clone(),
        Vec::new(),
        Vec::new(),
        vec![
            EvidenceResolutionStateV2::new(
                0,
                true,
                Vec::new(),
                ResolvedEvidenceReferenceV2::resolved(
                    EvidenceReferenceSnapshotV2::new(
                        GraphNodeId::new("assess-goal").unwrap(),
                        first_attempt_id.clone(),
                        AttemptNumberV2::FIRST,
                        assessment_memory.recorded_items_digest().unwrap(),
                        UnixMillis::new(1_700_000_000_010),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let workflow_memory = WorkflowMemoryStateV2::new(
        vec![assessment_memory, finish_memory],
        vec![decision],
        Vec::new(),
    )
    .unwrap();

    let criterion_states = vec![
        rich_assessment_result("correct", "The note demonstrates correctness."),
        rich_assessment_result("tested", "The note records test coverage."),
    ];
    let criterion_results = criterion_states
        .iter()
        .map(|state| state.result().clone())
        .collect::<Vec<_>>();
    let goal_assessment = GoalAssessmentRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        GoalOutcome::Achieved,
        criterion_results,
        evidence,
        None,
        first_attempt_id.clone(),
        GraphNodeId::new("assess-goal").unwrap(),
        TraceSequenceV2::FIRST,
        UnixMillis::new(1_700_000_000_010),
    )
    .unwrap();
    let goal = GoalStateV2::new(
        Some(GoalRevisionNumberV2::FIRST),
        vec![assessment_goal_revision()],
        vec![
            AttemptCriterionAssessmentStateV2::new(
                first_attempt_id.clone(),
                GoalRevisionNumberV2::FIRST,
                criterion_states,
            )
            .unwrap(),
        ],
        vec![goal_assessment],
    )
    .unwrap();

    let finish_ended_at = match lifecycle {
        DecidedFixtureLifecycle::Running => None,
        DecidedFixtureLifecycle::Completed | DecidedFixtureLifecycle::Cancelled => {
            Some(UnixMillis::new(1_700_000_000_020))
        }
    };
    let finish_terminal_reason = match lifecycle {
        DecidedFixtureLifecycle::Cancelled => Some("Cancelled by test.".to_owned()),
        DecidedFixtureLifecycle::Running | DecidedFixtureLifecycle::Completed => None,
    };
    let metadata = vec![
        AttemptMetadataV2::new(
            first_attempt_id,
            UnixMillis::new(1_700_000_000_000),
            Some(UnixMillis::new(1_700_000_000_010)),
            None,
        )
        .unwrap(),
        AttemptMetadataV2::new(
            second_attempt_id,
            UnixMillis::new(1_700_000_000_010),
            finish_ended_at,
            finish_terminal_reason,
        )
        .unwrap(),
    ];
    let (completed_at, cancelled_at, cancel_reason) = match lifecycle {
        DecidedFixtureLifecycle::Running => (None, None, None),
        DecidedFixtureLifecycle::Completed => {
            (Some(UnixMillis::new(1_700_000_000_020)), None, None)
        }
        DecidedFixtureLifecycle::Cancelled => (
            None,
            Some(UnixMillis::new(1_700_000_000_020)),
            Some("Cancelled by test.".to_owned()),
        ),
    };
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(9),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        vec![
            GraphNodeCounterV2::new(GraphNodeId::new("assess-goal").unwrap(), 1, 0),
            GraphNodeCounterV2::new(GraphNodeId::new("finish").unwrap(), 1, 0),
        ],
        metadata,
        workflow_memory,
        goal,
        UnixMillis::new(1_700_000_000_000),
        completed_at,
        cancelled_at,
        cancel_reason,
    )
    .unwrap()
}

fn expected_store_goal_assessment_digest(state: &GraphSessionStateV2) -> String {
    let assessment = state.goal_state().assessments().first().unwrap();
    let decision = state.workflow_memory().decisions().first().unwrap();
    let criterion_results = assessment
        .criterion_results()
        .iter()
        .map(|result| {
            let citations = result
                .citations()
                .iter()
                .map(|citation| match citation {
                    CriterionCitationV2::Evidence(source) => {
                        json!({"kind":"evidence","source_graph_node_id":source.as_str()})
                    }
                    CriterionCitationV2::Item(item) => {
                        json!({"item_id":item.as_str(),"kind":"item"})
                    }
                })
                .collect::<Vec<_>>();
            json!({
                "citations": citations,
                "criterion_id": result.criterion_id().as_str(),
                "reason": result.reason().as_str(),
                "status": result.status().as_str(),
            })
        })
        .collect::<Vec<_>>();
    let evidence = assessment
        .evidence()
        .references()
        .iter()
        .map(|reference| match reference {
            ResolvedEvidenceReferenceV2::Resolved(value) => json!({
                "items_digest": value.items_digest().as_str(),
                "resolved_at_ms": value.resolved_at().get(),
                "source_attempt_id": value.source_attempt_id().as_str(),
                "source_attempt_number": value.source_attempt_number().get(),
                "source_graph_node_id": value.source_node().as_str(),
                "state": "resolved",
            }),
            ResolvedEvidenceReferenceV2::Skipped(value) => json!({
                "items_digest": value.items_digest().as_str(),
                "resolved_at_ms": value.resolved_at().get(),
                "source_attempt_id": value.source_attempt_id().as_str(),
                "source_attempt_number": value.source_attempt_number().get(),
                "source_graph_node_id": value.source_node().as_str(),
                "state": "skipped",
            }),
            ResolvedEvidenceReferenceV2::Unresolved { source_node } => json!({
                "source_graph_node_id": source_node.as_str(),
                "state": "unresolved",
            }),
        })
        .collect::<Vec<_>>();
    let canonical = canonicalize_json_v1(&json!({
        "actor": assessment.actor().map(ActorAttributionV2::as_str),
        "criterion_results": criterion_results,
        "decision_attempt_id": assessment.decision_attempt_id().as_str(),
        "decision_graph_node_id": assessment.decision_graph_node_id().as_str(),
        "decision_trace_sequence": assessment.decision_trace().get(),
        "evidence": evidence,
        "goal_revision": assessment.goal_revision().get(),
        "mode": assessment.mode().as_str(),
        "outcome": assessment.outcome().as_str(),
        "recorded_at_ms": assessment.recorded_at().get(),
        "route_effect": decision.route_effect().as_str(),
        "route_target_graph_node_id": decision.route_target().as_str(),
        "selected_option_id": decision.selected_option().as_str(),
        "session_id": state.trace().session_id().as_str(),
    }))
    .unwrap();
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

fn wide_running_state() -> GraphSessionStateV2 {
    let nodes = (0_u64..64)
        .map(|index| {
            let id = format!("node-{index}");
            if index == 63 {
                json!({"id": id, "use": "work", "terminal": true})
            } else {
                json!({"id": id, "use": "work", "next": format!("node-{}", index + 1)})
            }
        })
        .collect::<Vec<_>>();
    let source = serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "wide-view",
        "version": "2",
        "name": "Wide view",
        "purpose": "Exercise bounded status encoding.",
        "node_definitions": {
            "work": {"type": "action", "title": "Work", "intent": "Continue."}
        },
        "graph": {"entry": "node-0", "nodes": nodes}
    }))
    .unwrap();
    let base = fresh_state("wide.json", &source);
    let attempts = (0_u64..64)
        .map(|index| {
            SessionAttemptV2::new(
                AttemptId::new(format!("00000000-0000-4000-8000-{:012x}", 0x3000 + index)).unwrap(),
                GraphNodeId::new(format!("node-{index}")).unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(index + 1),
                if index == 63 {
                    AttemptLifecycle::Active
                } else {
                    AttemptLifecycle::Completed
                },
                AttemptValidityV2::Valid,
                None,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let metadata = attempts
        .iter()
        .enumerate()
        .map(|(index, attempt)| {
            let started_at = UnixMillis::new(1_700_000_000_000 + index as u64);
            AttemptMetadataV2::new(
                attempt.attempt_id().clone(),
                started_at,
                (index < 63).then(|| UnixMillis::new(started_at.get() + 1)),
                None,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let counters = attempts
        .iter()
        .map(|attempt| GraphNodeCounterV2::new(attempt.graph_node_id().clone(), 1, 0))
        .collect::<Vec<_>>();
    let workflow = WorkflowMemoryStateV2::new(
        attempts
            .iter()
            .map(|attempt| {
                AttemptWorkflowMemoryV2::new(
                    attempt.attempt_id().clone(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                )
                .unwrap()
            })
            .collect(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(64),
        attempts,
    )
    .unwrap();
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(64),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        counters,
        metadata,
        workflow,
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

#[test]
fn v2run002_maxish_projector_outputs_remain_within_window_and_frame_budgets() {
    let wide = view(wide_running_state());
    let compact = project_graph_status_v2(&wide, GraphStatusTierV2::Compact, None).unwrap();
    let verbose = project_graph_status_v2(&wide, GraphStatusTierV2::Verbose, None).unwrap();
    let wide_next = project_graph_next_v2(&wide).unwrap();
    assert_eq!(compact["counters"].as_array().unwrap().len(), 64);
    let compact_bytes = serde_json::to_vec(&output("session.status", compact.clone())).unwrap();
    assert!(compact_bytes.len() <= 262_144);
    assert_eq!(
        verbose["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(verbose["current_trace_history"]["trace_truncated"], true);
    for history in [
        "current_trace_history",
        "stale_attempt_history",
        "decision_history",
        "rework_history",
        "stale_goal_revision_history",
        "stale_goal_assessment_history",
    ] {
        assert!(verbose[history].get("trace_truncated").is_some());
        assert!(verbose[history].get("trace_window").is_some());
        let encoded = serde_json::to_vec(&verbose[history]).unwrap();
        assert!(
            encoded.len() <= 65_536,
            "{history} exceeded its 64 KiB window: {} bytes",
            encoded.len()
        );
    }

    let bounded_view = view(blocker_and_item_state());
    let bounded = project_graph_status_v2(&bounded_view, GraphStatusTierV2::Verbose, None).unwrap();
    let bounded_next = project_graph_next_v2(&bounded_view).unwrap();
    assert_eq!(bounded["blockers_truncated"], true);
    assert!(bounded.get("items_truncated").is_some());
    assert_eq!(bounded["item_values"][0]["value_truncated"], true);
    assert!(
        serde_json::to_vec(&bounded["blocker_window"])
            .unwrap()
            .len()
            <= 49_152
    );

    for (command, result) in [
        ("session.status", compact),
        ("session.status", verbose),
        ("session.status", bounded),
        ("session.next", wide_next),
        ("session.next", bounded_next),
    ] {
        let envelope = output(command, result);
        let encoded = serde_json::to_vec(&envelope).unwrap();
        assert!(encoded.len() <= 1_048_576);
        validate_frame_payload_length(encoded.len()).unwrap();
        let decoded: OutputEnvelopeV2 = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, envelope);
    }
}

#[test]
fn v2run002_terminal_status_preserves_goal_without_a_current_attempt() {
    for (lifecycle, expected) in [
        (DecidedFixtureLifecycle::Completed, "completed"),
        (DecidedFixtureLifecycle::Cancelled, "cancelled"),
    ] {
        let view = view(decided_assessment_state(lifecycle, false));
        for tier in [GraphStatusTierV2::Standard, GraphStatusTierV2::Verbose] {
            let status = project_graph_status_v2(&view, tier, None).unwrap();
            assert_eq!(status["session"]["lifecycle"], expected);
            assert!(status["current"].is_null());
            assert_eq!(status["goal"]["revision"], 1);
            assert_eq!(
                status["goal"]["statement"],
                "Deliver a correct and tested result."
            );
            assert_eq!(status["goal"]["criteria"].as_array().unwrap().len(), 2);
            assert_output_v2("session.status", status);
        }
        assert!(project_graph_next_v2(&view).is_err());
    }
}

#[test]
fn v2run002_valid_and_stale_attempt_histories_are_disjoint_and_cursor_bound() {
    let view = view(decided_assessment_state(
        DecidedFixtureLifecycle::Cancelled,
        false,
    ));
    let verbose = project_graph_status_v2(&view, GraphStatusTierV2::Verbose, None).unwrap();
    assert_eq!(
        verbose["current_trace_history"]["entries"][0]["trace_sequence"],
        1
    );
    assert_eq!(
        verbose["stale_attempt_history"]["entries"][0]["trace_sequence"],
        2
    );

    let before_two = project_graph_status_v2(
        &view,
        GraphStatusTierV2::Verbose,
        Some(TraceSequenceV2::new(2)),
    )
    .unwrap();
    assert_eq!(
        before_two["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        before_two["stale_attempt_history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        before_two["stale_attempt_history"]["trace_truncated"],
        false
    );

    let before_one = project_graph_status_v2(
        &view,
        GraphStatusTierV2::Verbose,
        Some(TraceSequenceV2::FIRST),
    )
    .unwrap();
    assert!(
        before_one["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        before_one["stale_attempt_history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        before_one["current_trace_history"]["trace_truncated"],
        false
    );
    assert_eq!(
        before_one["stale_attempt_history"]["trace_truncated"],
        false
    );
    assert_output_v2("session.status", verbose);
    assert_output_v2("session.status", before_two);
    assert_output_v2("session.status", before_one);
}

#[test]
fn v2run002_stale_goal_assessment_digest_matches_the_store_canonical_record_digest() {
    let state = decided_assessment_state(DecidedFixtureLifecycle::Cancelled, true);
    let expected = expected_store_goal_assessment_digest(&state);
    let verbose = project_graph_status_v2(&view(state), GraphStatusTierV2::Verbose, None).unwrap();
    let stale = &verbose["stale_goal_assessment_history"]["entries"];
    assert_eq!(stale.as_array().unwrap().len(), 1);
    assert_eq!(stale[0]["record_digest"], expected);
    assert_output_v2("session.status", verbose);
}

#[test]
fn v2run002_goal_assessment_decision_readback_retains_reasons_and_citations() {
    let view = view(decided_assessment_state(
        DecidedFixtureLifecycle::Running,
        false,
    ));
    let next = project_graph_next_v2(&view).unwrap();
    assert_eq!(next["readback"].as_array().unwrap().len(), 1);
    let decision = &next["readback"][0]["decision_record"];
    assert!(decision.get("actor").is_none());
    assert_eq!(decision["assessment"], "session_goal");
    assert_eq!(decision["assessment_mode"], "assessment");
    assert_eq!(decision["goal_outcome"], "achieved");
    assert_eq!(decision["criterion_results"].as_array().unwrap().len(), 2);
    assert_eq!(
        decision["criterion_results"][0]["reason"],
        "The note demonstrates correctness."
    );
    assert_eq!(
        decision["criterion_results"][0]["citations"],
        json!([{"local_item_id": "note"}])
    );
    assert_eq!(
        decision["criterion_results"][1]["reason"],
        "The note records test coverage."
    );
    assert_output_v2("session.next", next);
}

#[test]
fn v2run002_goal_assessment_guidance_tracks_completion_and_determined_outcome() {
    let partial = view(goal_assessment_state(vec![assessment_result("correct")]));
    let next = project_graph_next_v2(&partial).unwrap();
    let status = project_graph_status_v2(&partial, GraphStatusTierV2::Standard, None).unwrap();
    assert_eq!(next["readiness"]["goal_ready"], false);
    assert_eq!(next["readiness"]["can_advance"], false);
    assert!(status["allowed_option_ids"].as_array().unwrap().is_empty());
    assert!(
        !next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.decide"))
    );
    assert!(
        !next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("goal.revise"))
    );
    let assess = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "goal.assess_criterion")
        .collect::<Vec<_>>();
    assert_eq!(assess.len(), 1);
    let assess_argv = assess[0]["argv"].as_array().unwrap();
    assert!(assess_argv.windows(3).any(|window| window
        == [
            json!("assess-criterion"),
            json!("tested"),
            json!("--status")
        ]));
    assert_output_v2("session.next", next);

    let complete = view(goal_assessment_state(vec![
        assessment_result("correct"),
        assessment_result("tested"),
    ]));
    let next = project_graph_next_v2(&complete).unwrap();
    let status = project_graph_status_v2(&complete, GraphStatusTierV2::Standard, None).unwrap();
    assert_eq!(next["readiness"]["goal_ready"], true);
    assert_eq!(next["readiness"]["can_advance"], true);
    assert_eq!(next["goal"]["determined_outcome"], "achieved");
    assert_eq!(status["allowed_option_ids"], json!(["achieved"]));
    assert_eq!(next["options"].as_array().unwrap().len(), 1);
    assert_eq!(next["options"][0]["option_id"], "achieved");
    assert!(
        !next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("goal.assess_criterion"))
    );
    let decide = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "session.decide")
        .collect::<Vec<_>>();
    assert_eq!(decide.len(), 1);
    let decide_argv = decide[0]["argv"].as_array().unwrap();
    assert!(
        decide_argv
            .windows(2)
            .any(|window| window == [json!("--option"), json!("achieved")])
    );
    assert_output_v2("session.next", next);
}

#[test]
fn v2run002_active_reassessment_does_not_inherit_a_prior_fresh_assessment() {
    let view = view(reassessment_state());
    let status = project_graph_status_v2(&view, GraphStatusTierV2::Standard, None).unwrap();
    let next = project_graph_next_v2(&view).unwrap();

    assert_eq!(status["current"]["node"]["graph_node_id"], "assess-current");
    assert!(status["goal"].get("determined_outcome").is_none());
    assert!(
        status["goal"]["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .all(|criterion| criterion["status"] == "unassessed")
    );
    assert!(next["goal"].get("determined_outcome").is_none());
    assert_eq!(next["readiness"]["goal_ready"], false);
    assert_eq!(next["readiness"]["can_advance"], false);
    assert!(
        !next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.decide"))
    );
    assert_eq!(
        next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|suggestion| suggestion["command"] == "goal.assess_criterion")
            .count(),
        2
    );
    assert_output_v2("session.status", status);
    assert_output_v2("session.next", next);
}

#[test]
fn v2run002_skip_guidance_reflects_the_placement_reason_policy() {
    let source = String::from_utf8(EQUIVALENT_YAML.to_vec())
        .unwrap()
        .replace(
            "      use: work\n      next: decide",
            "      use: work\n      skip:\n        allowed: true\n        reason_required: true\n      next: decide",
        )
        .replace(
            "        - node: perform\n          required: true",
            "        - node: perform\n          required: false",
        );
    let view = view(fresh_state("skippable.yaml", source.as_bytes()));
    let next = project_graph_next_v2(&view).unwrap();

    assert!(
        next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.skip"))
    );
    assert_eq!(
        next["skip"],
        json!({"allowed": true, "reason_required": true})
    );
    let skip = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|suggestion| suggestion["command"] == "session.skip")
        .unwrap();
    assert_eq!(
        skip["argv"],
        json!(["podway", "skip", "--reason", "<text>"])
    );
    assert_output_v2("session.next", next);
}
