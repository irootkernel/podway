//! Focused V2RUN-002 coverage for deterministic Procedure v2 status and next projections.

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2, parse_procedure_document,
    procedure_placement_budget_v2, validate_procedure_v2,
};
use podway_core::{
    ActorAttributionV2, ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2,
    AttemptValidityV2, BlockerId, BlockerState, CanonicalProcedureJsonV1, CheckResultExecutorV2,
    CheckResultInputBasisV2, CheckResultOutcomeV2, CheckResultValueV2, CriterionAssessmentReasonV2,
    CriterionAssessmentResultV2, CriterionCitationV2, CriterionId, CriterionStatusV2,
    DecisionRecordInputV2, DecisionRecordV2, EvidenceReferenceSnapshotV2, GoalAssessmentRecordV2,
    GoalCriterionV2, GoalDefinitionV2, GoalOutcome, GoalRevisionNumberV2, GoalRevisionReasonV2,
    GoalRevisionRecordV2, GoalStatementV2, GraphNodeId, ItemId, MAX_CHECK_RESULT_VALUE_BYTES_V2,
    MAX_EVIDENCE_RESPONSE_BYTES_V2, NodeDefinitionId, OperationId, OptionId, ProcedureSnapshotId,
    ReasonV2, RecordedItemValueV2, ResolvedEvidenceReferenceV2, ResolvedEvidenceSetV2, Revision,
    SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, TraceSequenceV2,
    TransitionEffectV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_daemon::{
    execution::{
        ProcedureProviderV1, ProcedureV2SourceAdmissionErrorV1, prepare_custom_procedure_v2_start,
        workspace_procedure_snapshot_from_bytes_v2,
    },
    v2_read_service::{
        EvidenceReadErrorV2, GraphStatusTierV2, GraphViewErrorV2, project_evidence_read_v1,
        project_graph_next_v2, project_graph_observation_v1, project_graph_status_v2,
    },
};
use podway_protocol::{
    CommandNameV1, MAX_FRAME_PAYLOAD_BYTES_V1, OutputEnvelopeInputV3, OutputEnvelopeV3,
    RequestIdV1, ResponseEnvelopeV2, Rfc3339MillisV1, SessionLifecycleV1, SessionOutputV1,
    WorkspaceOutputV1, decode_response_payload_v2, decode_single_frame_v1, encode_frame_v1,
    encode_response_payload_v2, validate_frame_payload_length,
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
const MAXIMUM_NEXT_RECIPE: &[u8] =
    include_bytes!("../../../tests/fixtures/v2/payload/maximum-next-recipe.json");
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
const APPLICABILITY_PROCEDURE: &[u8] = br#"{
  "schema":"podway.procedure/v2",
  "id":"applicability-classes",
  "version":"2",
  "name":"Applicability classes",
  "purpose":"Enumerate every item and placement applicability class.",
  "goal_tracking":true,
  "node_definitions":{
    "work":{
      "type":"action",
      "title":"Work",
      "intent":"Record every item kind.",
      "items":[
        {"id":"confirm","type":"confirm","prompt":"Confirm.","required":true},
        {"id":"text","type":"text","prompt":"Text.","required":true},
        {"id":"choice","type":"choice","prompt":"Choice.","required":true,"choices":["yes","no"]},
        {"id":"integer","type":"integer","prompt":"Integer.","required":true,"minimum":0,"maximum":9},
        {"id":"list","type":"list","prompt":"List.","required":true,"min_items":1,"max_items":2,"max_item_length":8,"unique":true},
        {"id":"artifact","type":"artifact","prompt":"Artifact.","required":true,"allowed_media_types":["text/plain"]}
      ]
    },
    "choose":{
      "type":"decision",
      "title":"Choose",
      "objective":"Choose a route.",
      "prompt":"Which route?",
      "items":[{"id":"basis","type":"text","prompt":"Basis.","required":true}],
      "options":[{"id":"left","label":"Left"},{"id":"right","label":"Right"}],
      "reason":{"required":true}
    },
    "assess":{
      "type":"decision",
      "title":"Assess",
      "objective":"Assess the goal.",
      "prompt":"Which outcome?",
      "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},{"id":"superseded","label":"Superseded"}],
      "reason":{"required":true},
      "assessment":{"target":"session_goal","outcomes":{"achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"}}
    },
    "finish":{"type":"action","title":"Finish","intent":"Finish."}
  },
  "graph":{
    "entry":"work",
    "nodes":[
      {"id":"work","use":"work","skip":{"allowed":true,"reason_required":false},"next":"choose"},
      {"id":"choose","use":"choose","routes":{"left":{"to":"assess","effect":"advance"},"right":{"to":"assess","effect":"advance"}}},
      {"id":"assess","use":"assess","routes":{"achieved":{"to":"finish","effect":"advance"},"not-achieved":{"to":"finish","effect":"advance"},"superseded":{"to":"finish","effect":"advance"}}},
      {"id":"finish","use":"finish","terminal":true}
    ]
  },
  "manual_rework":{"allowed_targets":["work"]}
}"#;
const CHECK_RESULT_GUIDANCE_PROCEDURE: &[u8] = br#"{
  "schema":"podway.procedure/v2",
  "id":"check-result-guidance",
  "version":"1",
  "name":"Check result guidance",
  "purpose":"Project safe recording guidance.",
  "node_definitions":{
    "verify":{
      "type":"action",
      "title":"Verify",
      "intent":"Record caller-supplied external check results.",
      "items":[
        {
          "id":"optional-check",
          "type":"check_result",
          "prompt":"Optionally record another result.",
          "required":false,
          "operation_id":"optional-operation",
          "operation_digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          "accepted_outcomes":["inconclusive"]
        },
        {
          "id":"required-check",
          "type":"check_result",
          "prompt":"Record the required result.",
          "required":true,
          "operation_id":"make-test",
          "operation_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "accepted_outcomes":["pass","fail"]
        }
      ]
    }
  },
  "graph":{"entry":"verify","nodes":[{"id":"verify","use":"verify","terminal":true}]}
}"#;

#[derive(Clone, Copy)]
struct ByteProcedureV2<'a> {
    path: &'a str,
    source: &'a [u8],
}

impl ProcedureProviderV1 for ByteProcedureV2<'_> {
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

fn with_v025_list_snapshot(current: GraphSessionStateV2) -> GraphSessionStateV2 {
    let mut document: Value =
        serde_json::from_str(current.snapshot().canonical_json().as_str()).unwrap();
    let list = document["node_definitions"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .filter_map(|definition| definition.get_mut("items")?.as_array_mut())
        .flatten()
        .find(|item| item["type"] == "list")
        .unwrap()
        .as_object_mut()
        .unwrap();
    assert_eq!(list.remove("max_total_length"), Some(json!(1_000_000)));
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    assert_ne!(&digest, current.snapshot().digest());
    let snapshot = ProcedureSnapshotV2::new(
        current.snapshot().snapshot_id().clone(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        current.snapshot().source().clone(),
        current.snapshot().created_at(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        current.workspace_revision(),
        current.task_title(),
        snapshot,
        current.trace().clone(),
        current.counters().to_vec(),
        current.attempt_metadata().to_vec(),
        current.workflow_memory().clone(),
        current.goal_state().clone(),
        current.created_at(),
        current.completed_at(),
        current.cancelled_at(),
        current.cancel_reason().map(str::to_owned),
    )
    .unwrap()
}

fn validated_json_procedure(source: &[u8]) -> ValidatedProcedureV2 {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source, ProcedureDocumentFormat::Json).unwrap()
    else {
        panic!("fixture must be a Procedure v2 document")
    };
    validate_procedure_v2(parsed).unwrap()
}

fn view(state: GraphSessionStateV2) -> GraphWorkspaceViewV2 {
    view_with_terminal_disposition(state, false)
}

fn view_with_terminal_disposition(
    state: GraphSessionStateV2,
    current_terminal_disposition: bool,
) -> GraphWorkspaceViewV2 {
    GraphWorkspaceViewV2::new(
        identity(),
        Some(state),
        0,
        None,
        1,
        UnixMillis::new(1_700_000_000_100),
    )
    .with_current_terminal_disposition(current_terminal_disposition)
}

fn prepared_state() -> GraphSessionStateV2 {
    let running = fresh_state("workflow.yaml", EQUIVALENT_YAML);
    GraphSessionStateV2::prepared(
        running.workspace_revision(),
        running.task_title(),
        running.snapshot().clone(),
        SessionId::new(SESSION_ID).unwrap(),
        running.created_at(),
    )
    .unwrap()
}

fn output(command: &str, result: Map<String, Value>) -> OutputEnvelopeV3 {
    let rendered_result = serde_json::to_string(&result).unwrap();
    OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-09T00:00:00.000Z").unwrap(),
        workspace: None,
        job: None,
        session: None,
        result,
        warnings: Vec::new(),
    })
    .unwrap_or_else(|error| panic!("{error:?}: {rendered_result}"))
}

fn maximum_production_output(command: &str, result: Map<String, Value>) -> OutputEnvelopeV3 {
    let warnings = (0..4)
        .map(|_| {
            json!({
                "code": "A".repeat(64),
                "path": "\0".repeat(256),
                "message": "\0".repeat(512),
            })
            .as_object()
            .unwrap()
            .clone()
        })
        .collect();
    OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-09T00:00:00.000Z").unwrap(),
        workspace: Some(
            WorkspaceOutputV1::new(
                WorkspaceId::new(WORKSPACE_ID).unwrap(),
                "\0".repeat(4_096),
                u64::MAX,
            )
            .unwrap(),
        ),
        job: None,
        session: Some(
            SessionOutputV1::new(
                SessionId::new(SESSION_ID).unwrap(),
                "\0".repeat(500),
                SessionLifecycleV1::Running,
                Revision::new(u64::MAX),
                Revision::new(u64::MAX),
            )
            .unwrap(),
        ),
        result,
        warnings,
    })
    .unwrap()
}

fn assert_maximum_production_output_v2(
    command: &str,
    result: Map<String, Value>,
    maximum_bytes: usize,
) {
    let output = maximum_production_output(command, result);
    let workspace = output.workspace().unwrap();
    assert_eq!(workspace.root().chars().count(), 4_096);
    assert_eq!(workspace.latest_workspace_sequence(), u64::MAX);
    let session = output.session().unwrap();
    assert_eq!(session.title().chars().count(), 500);
    assert_eq!(session.revision_before(), Revision::new(u64::MAX));
    assert_eq!(session.revision_after(), Revision::new(u64::MAX));
    assert_eq!(output.warnings().len(), 4);
    assert!(output.warnings().iter().all(|warning| {
        warning["code"].as_str().unwrap().chars().count() == 64
            && warning["path"].as_str().unwrap().chars().count() == 256
            && warning["message"].as_str().unwrap().chars().count() == 512
            && warning["code"] == "A".repeat(64)
            && ["path", "message"].into_iter().all(|field| {
                warning[field]
                    .as_str()
                    .is_some_and(|text| text.chars().all(|character| character == '\0'))
            })
    }));

    let response = ResponseEnvelopeV2::OutputV2(output);
    let encoded = encode_response_payload_v2(&response).unwrap();
    assert!(
        encoded.len() <= maximum_bytes,
        "maximum production {command} envelope exceeded {maximum_bytes} bytes: {}",
        encoded.len()
    );
    validate_frame_payload_length(encoded.len()).unwrap();
    assert_eq!(decode_response_payload_v2(&encoded).unwrap(), response);
    let frame = encode_frame_v1(&encoded).unwrap();
    assert_eq!(decode_single_frame_v1(&frame).unwrap(), encoded);
}

fn assert_output_v2(command: &str, result: Map<String, Value>) {
    let output = output(command, result);
    let response = ResponseEnvelopeV2::OutputV2(output.clone());
    let encoded = encode_response_payload_v2(&response).unwrap();
    validate_frame_payload_length(encoded.len()).unwrap();
    assert_eq!(decode_response_payload_v2(&encoded).unwrap(), response);
    let frame = encode_frame_v1(&encoded).unwrap();
    assert_eq!(decode_single_frame_v1(&frame).unwrap(), encoded);
}

#[test]
fn v2run002_fresh_action_projects_closed_compact_standard_verbose_and_next_views() {
    let view = view(fresh_state("workflow.yaml", EQUIVALENT_YAML));
    let compact = project_graph_status_v2(&view, GraphStatusTierV2::Compact, None).unwrap();
    let standard = project_graph_status_v2(&view, GraphStatusTierV2::Standard, None).unwrap();
    let verbose = project_graph_status_v2(&view, GraphStatusTierV2::Verbose, None).unwrap();
    let next = project_graph_next_v2(&view).unwrap();

    assert_eq!(compact["schema"], "podway.compact-status-result/v3");
    assert_eq!(standard["schema"], "podway.status-result/v3");
    assert_eq!(standard["tier"], "standard");
    assert_eq!(verbose["schema"], "podway.status-result/v3");
    assert_eq!(verbose["tier"], "verbose");
    assert_eq!(next["schema"], "podway.next-result/v3");

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
    assert_eq!(
        next["allowed_actions"],
        json!([
            "item.set",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.define"
        ])
    );
    assert_eq!(
        next["suggestions"],
        json!([
            {
                "command": "item.set",
                "argv": ["podway", "set", "result", "<text>"],
                "item_id": "result"
            },
            {
                "command": "session.retry",
                "argv": ["podway", "retry", "--reason", "<reason>"]
            },
            {
                "command": "goal.define",
                "argv": [
                    "podway", "goal", "define", "--goal", "<goal>", "--criterion",
                    "<criterion>"
                ]
            }
        ])
    );
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
fn v2agt002_observation_projects_typed_items_and_fenced_templates() {
    let view = view(fresh_state("applicability.json", APPLICABILITY_PROCEDURE));
    let observation = project_graph_observation_v1(&view).unwrap();

    assert_eq!(observation["schema"], "podway.observation-result/v3");
    assert_eq!(observation["status"]["session"]["lifecycle"], "running");
    assert_eq!(observation["guidance"]["node"]["graph_node_id"], "work");
    let items = observation["active_items"].as_array().unwrap();
    assert_eq!(items.len(), 6);
    assert_eq!(items[0]["type"], "confirm");
    assert_eq!(items[1]["constraints"]["max_length"], 4_000);
    assert_eq!(items[2]["constraints"]["choices"], json!(["yes", "no"]));
    assert_eq!(items[3]["constraints"]["minimum"], 0);
    assert_eq!(items[3]["constraints"]["maximum"], 9);
    assert_eq!(items[4]["constraints"]["max_items"], 2);
    assert_eq!(
        items[5]["constraints"]["allowed_media_types"],
        json!(["text/plain"])
    );
    assert!(items.iter().all(|item| item["value"].is_null()));

    let templates = observation["mutation_templates"].as_array().unwrap();
    assert_eq!(templates.len(), 13);
    for action in observation["guidance"]["allowed_actions"]
        .as_array()
        .unwrap()
    {
        assert!(
            templates
                .iter()
                .any(|template| template["command"] == *action),
            "observation omitted a template for {action}"
        );
    }
    for template in templates {
        assert_eq!(template["authority"], "optimistic_concurrency_only");
        assert_eq!(template["idempotency_key_required"], true);
        assert!(template["requires_explicit_authorization"].is_boolean());
        assert_eq!(template["preconditions"]["workspace_uuid"], WORKSPACE_ID);
        assert_eq!(template["preconditions"]["session_id"], SESSION_ID);
        let argv = template["argv"].as_array().unwrap();
        assert!(argv.contains(&json!("--if-workspace-uuid")));
        assert!(argv.contains(&json!("--if-session-id")));
        assert!(argv.contains(&json!("--idempotency-key")));
        if template["command"].as_str().unwrap().starts_with("item.") {
            assert_eq!(template["preconditions"]["attempt_id"], ATTEMPT_ID);
            assert!(template["preconditions"]["item_revision"].is_number());
            assert!(argv.contains(&json!("--if-attempt")));
            assert!(argv.contains(&json!("--if-item-revision")));
        } else {
            assert_eq!(template["preconditions"]["session_revision"], 1);
            assert!(argv.contains(&json!("--if-session-revision")));
        }
        if template["command"] == "session.reset" {
            assert!(argv.windows(2).any(|arguments| {
                arguments == [json!("--progress-summary"), json!("<progress-summary>")]
            }));
            assert!(argv.contains(&json!("--yes")));
        }
    }
    assert_output_v2("session.observe", observation);
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
fn v2scl003_v025_list_snapshot_rehydrates_without_rotating_identity() {
    let current = fresh_state("applicability.json", APPLICABILITY_PROCEDURE);
    let view = view(with_v025_list_snapshot(current));
    let stored_digest = view.graph_state().unwrap().snapshot().digest().as_str();

    for tier in [
        GraphStatusTierV2::Compact,
        GraphStatusTierV2::Standard,
        GraphStatusTierV2::Verbose,
    ] {
        let status = project_graph_status_v2(&view, tier, None).unwrap();
        assert_eq!(status["procedure"]["digest"], stored_digest);
    }
    let next = project_graph_next_v2(&view).unwrap();
    assert_eq!(next["procedure_digest"], stored_digest);
    let observation = project_graph_observation_v1(&view).unwrap();
    assert_eq!(observation["status"]["procedure"]["digest"], stored_digest);
    let list = observation["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == "list")
        .unwrap();
    assert_eq!(list["constraints"]["max_total_length"], 1_000_000);
}

#[test]
fn v2scl003_v025_completed_disposed_session_remains_observable() {
    const PROCEDURE: &[u8] = br#"{
      "schema":"podway.procedure/v2",
      "id":"legacy-completed-list",
      "version":"2",
      "name":"Legacy completed list",
      "purpose":"Exercise a completed v0.2.5 list snapshot.",
      "node_definitions":{"finish":{
        "type":"action",
        "title":"Finish",
        "intent":"Finish the session.",
        "items":[{"id":"findings","type":"list","prompt":"Record findings.","required":false}]
      }},
      "graph":{"entry":"finish","nodes":[{"id":"finish","use":"finish","terminal":true}]}
    }"#;
    let running = fresh_state("legacy-completed-list.json", PROCEDURE);
    let attempt_id = running
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let current = running
        .complete_active_action_v2(
            running.trace().revision(),
            &attempt_id,
            None,
            UnixMillis::new(1_700_000_000_001),
        )
        .unwrap()
        .into_state();
    let state = with_v025_list_snapshot(current);
    let stored_digest = state.snapshot().digest().as_str().to_owned();
    let view = view_with_terminal_disposition(state, true);

    for tier in [
        GraphStatusTierV2::Compact,
        GraphStatusTierV2::Standard,
        GraphStatusTierV2::Verbose,
    ] {
        let status = project_graph_status_v2(&view, tier, None).unwrap();
        assert_eq!(status["procedure"]["digest"], stored_digest);
    }
    assert_eq!(
        project_graph_next_v2(&view),
        Err(GraphViewErrorV2::TerminalSessionHasNoNext)
    );
    let observation = project_graph_observation_v1(&view).unwrap();
    let commands = observation["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|template| template["command"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(commands, vec!["session.reset", "session.start_replace"]);
}

#[test]
fn v2ast005_check_result_guidance_is_bounded_copyable_and_required_first() {
    let initial_view = view(fresh_state(
        "check-result-guidance.json",
        CHECK_RESULT_GUIDANCE_PROCEDURE,
    ));
    let compact = project_graph_status_v2(&initial_view, GraphStatusTierV2::Compact, None).unwrap();
    assert!(
        compact["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] == "check_result")
    );
    assert_output_v2("session.status", compact);
    let next = project_graph_next_v2(&initial_view).unwrap();
    assert!(
        next["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("item.record_many"))
    );
    let suggestions = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "item.record_many")
        .collect::<Vec<_>>();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0]["argv"],
        json!(["podway", "record", "--stdin"])
    );
    assert_eq!(suggestions[0]["item_id"], "required-check");

    let observation = project_graph_observation_v1(&initial_view).unwrap();
    let active = observation["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == "required-check")
        .unwrap();
    assert_eq!(active["constraints"]["operation_id"], "make-test");
    assert_eq!(
        active["constraints"]["operation_digest"],
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        active["constraints"]["accepted_outcomes"],
        json!(["pass", "fail"])
    );

    let templates = observation["mutation_templates"].as_array().unwrap();
    let record_templates = templates
        .iter()
        .filter(|template| template["command"] == "item.record_many")
        .collect::<Vec<_>>();
    assert_eq!(record_templates.len(), 1);
    let template = record_templates[0];
    assert_eq!(template["argv"], json!(["podway", "record", "--stdin"]));
    assert_eq!(template["preconditions"]["workspace_uuid"], WORKSPACE_ID);
    assert_eq!(template["preconditions"]["session_id"], SESSION_ID);
    assert_eq!(template["preconditions"]["attempt_id"], ATTEMPT_ID);
    assert_eq!(template["preconditions"]["session_revision"], 1);
    assert_eq!(template["preconditions"]["item_revision"], 0);
    let stdin = template["stdin_template"].as_str().unwrap();
    assert!(stdin.len() <= 16_384);
    let stdin: Value = serde_json::from_str(stdin).unwrap();
    assert_eq!(stdin["workspace_uuid"], WORKSPACE_ID);
    assert_eq!(stdin["session_id"], SESSION_ID);
    assert_eq!(stdin["session_revision"], 1);
    assert_eq!(stdin["attempt_id"], ATTEMPT_ID);
    assert_eq!(stdin["idempotency_key"], "<idempotency-key>");
    assert_eq!(stdin["operations"][0]["item_id"], "required-check");
    assert_eq!(stdin["operations"][0]["expected_item_revision"], 0);
    assert_eq!(stdin["operations"][0]["record"]["type"], "check_result");
    assert_eq!(
        stdin["operations"][0]["record"]["operation_id"],
        "make-test"
    );
    assert_eq!(
        stdin["operations"][0]["record"]["operation_digest"],
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(stdin["operations"][0]["record"]["outcome"], "<outcome>");
    assert!(
        stdin["operations"][0]["record"]
            .get("accepted_outcomes")
            .is_none()
    );
    assert!(templates.iter().all(|candidate| {
        candidate["command"] == "item.record_many" || candidate.get("stdin_template").is_none()
    }));

    let satisfied_required = view(guidance_state_with_required_check_result(
        CheckResultOutcomeV2::Pass,
    ));
    let satisfied_required_next = project_graph_next_v2(&satisfied_required).unwrap();
    let fallback = satisfied_required_next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|suggestion| suggestion["command"] == "item.record_many")
        .unwrap();
    assert_eq!(fallback["item_id"], "optional-check");

    let unsatisfied_required = view(guidance_state_with_required_check_result(
        CheckResultOutcomeV2::Inconclusive,
    ));
    let unsatisfied_next = project_graph_next_v2(&unsatisfied_required).unwrap();
    let retry = unsatisfied_next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|suggestion| suggestion["command"] == "item.record_many")
        .unwrap();
    assert_eq!(retry["item_id"], "required-check");
    let unsatisfied_observation = project_graph_observation_v1(&unsatisfied_required).unwrap();
    let retry_template = unsatisfied_observation["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| template["command"] == "item.record_many")
        .unwrap();
    let retry_stdin: Value =
        serde_json::from_str(retry_template["stdin_template"].as_str().unwrap()).unwrap();
    assert_eq!(retry_stdin["operations"][0]["item_id"], "required-check");
    assert_eq!(retry_stdin["operations"][0]["expected_item_revision"], 1);

    assert_output_v2("session.next", next);
    assert_output_v2("session.observe", observation);
    assert_output_v2("session.next", satisfied_required_next);
    assert_output_v2("session.next", unsatisfied_next);
    assert_output_v2("session.observe", unsatisfied_observation);
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

fn conservative_json_charge(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(value) => {
            if *value {
                4
            } else {
                5
            }
        }
        Value::Number(value) => value.to_string().len(),
        Value::String(value) => value.chars().count().saturating_mul(6),
        Value::Array(values) => values
            .iter()
            .map(|value| 8_usize.saturating_add(conservative_json_charge(value)))
            .sum(),
        Value::Object(fields) => fields
            .values()
            .map(|value| 64_usize.saturating_add(conservative_json_charge(value)))
            .sum(),
    }
}

fn escape_heavy_next_source() -> Vec<u8> {
    let escape_heavy = "\0".repeat(1_000);
    let items = (0..20)
        .map(|index| {
            json!({
                "id": format!("item-{index}"),
                "type": "text",
                "prompt": "\0".repeat(300),
                "required": true,
                "max_length": 16_384
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "escape-heavy-next",
        "version": "2",
        "name": "Escape-heavy next",
        "purpose": "Exercise production JSON escaping at the static next boundary.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "\0".repeat(120),
                "intent": "\0".repeat(300),
                "description": escape_heavy,
                "instructions": vec!["\0".repeat(1_000); 12],
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

fn escape_heavy_next_state(source: &[u8]) -> GraphSessionStateV2 {
    fresh_state("escape-heavy.json", source)
}

fn maximum_compact_view() -> GraphWorkspaceViewV2 {
    let state = combined_maximum_verbose_state();
    GraphWorkspaceViewV2::new(
        identity(),
        Some(state),
        0,
        None,
        u64::MAX,
        UnixMillis::new(u64::MAX),
    )
}

fn status_values_boundary_state() -> GraphSessionStateV2 {
    let items = (0..64)
        .map(|index| {
            json!({
                "id": format!("item-{index:02}"),
                "type": "text",
                "prompt": format!("Record value {index:02}."),
                "required": false,
                "max_length": 16_384
            })
        })
        .collect::<Vec<_>>();
    let source = serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "status-values-boundary",
        "version": "2",
        "name": "Status values boundary",
        "purpose": "Exercise the complete-value status window boundary.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Work",
                "intent": "Record bounded values.",
                "items": items
            }
        },
        "graph": {
            "entry": "work",
            "nodes": [{"id": "work", "use": "work", "terminal": true}]
        }
    }))
    .unwrap();
    let state = fresh_state("status-values-boundary.json", &source);
    let original_attempt = state.workflow_memory().attempts().first().unwrap();
    let value = RecordedItemValueV2::text("😀".repeat(3_000)).unwrap();
    let item_slots = original_attempt
        .item_slots()
        .iter()
        .map(|slot| {
            ItemSlotStateV2::new(
                slot.attempt_id().clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(value.clone()),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect();
    let attempt = AttemptWorkflowMemoryV2::new(
        original_attempt.attempt_id().clone(),
        item_slots,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        state.workspace_revision(),
        state.task_title(),
        state.snapshot().clone(),
        state.trace().clone(),
        state.counters().to_vec(),
        state.attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![attempt], Vec::new(), Vec::new()).unwrap(),
        state.goal_state().clone(),
        state.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

fn decision_rework_history_state() -> GraphSessionStateV2 {
    let source = br#"{
      "schema":"podway.procedure/v2",
      "id":"history-boundary",
      "version":"2",
      "name":"History boundary",
      "purpose":"Exercise decision and rework history windows.",
      "node_definitions":{
        "review":{
          "type":"decision",
          "title":"Review",
          "objective":"Choose the route.",
          "prompt":"Accept?",
          "options":[{"id":"accept","label":"Accept"}],
          "reason":{"required":true}
        },
        "finish":{"type":"action","title":"Finish","intent":"Finish."}
      },
      "graph":{
        "entry":"review",
        "nodes":[
          {"id":"review","use":"review","routes":{"accept":{"to":"finish","effect":"advance"}}},
          {"id":"finish","use":"finish","terminal":true}
        ]
      },
      "manual_rework":{"allowed_targets":["review"]}
    }"#;
    let mut state = fresh_state("history-boundary.json", source);
    for cycle in 0_u64..7 {
        let review_attempt = state.trace().active_attempt().unwrap().attempt_id().clone();
        state = state
            .decide_active_route_v2(
                state.trace().revision(),
                &review_attempt,
                OptionId::new("accept").unwrap(),
                AttemptId::new(format!("00000000-0000-4000-8001-{cycle:012x}")).unwrap(),
                Some(ReasonV2::new(format!("Accept cycle {cycle}.")).unwrap()),
                None,
                UnixMillis::new(1_700_000_001_000 + cycle * 2),
            )
            .unwrap()
            .into_state();
        let finish_attempt = state.trace().active_attempt().unwrap().attempt_id().clone();
        state = state
            .manual_rework_v2(
                state.trace().revision(),
                Some(&finish_attempt),
                GraphNodeId::new("review").unwrap(),
                AttemptId::new(format!("00000000-0000-4000-8002-{cycle:012x}")).unwrap(),
                ReasonV2::new(format!("Revisit cycle {cycle}.")).unwrap(),
                None,
                UnixMillis::new(1_700_000_001_001 + cycle * 2),
            )
            .unwrap()
            .into_state();
    }
    state
}

fn goal_history_state() -> GraphSessionStateV2 {
    let mut procedure: Value = serde_json::from_slice(ASSESSMENT_PROCEDURE).unwrap();
    procedure.as_object_mut().unwrap().insert(
        "manual_rework".to_owned(),
        json!({"allowed_targets":["assess-goal"]}),
    );
    let procedure = serde_json::to_vec(&procedure).unwrap();
    let mut state =
        decided_assessment_state_from_source(DecidedFixtureLifecycle::Running, &procedure);
    for revision in 2_u64..=3 {
        let finish_attempt = state.trace().active_attempt().unwrap().attempt_id().clone();
        let assessment_attempt =
            AttemptId::new(format!("00000000-0000-4000-8003-{revision:012x}")).unwrap();
        state = state
            .revise_goal_v2(
                state.trace().revision(),
                Some(&finish_attempt),
                GoalRevisionNumberV2::new(revision - 1),
                GoalStatementV2::new(format!("Deliver revision {revision}.")).unwrap(),
                assessment_goal_definition(),
                GraphNodeId::new("assess-goal").unwrap(),
                assessment_attempt.clone(),
                GoalRevisionReasonV2::new(format!("Revise goal to {revision}.")).unwrap(),
                None,
                false,
                UnixMillis::new(1_700_000_002_000 + revision * 10),
            )
            .unwrap()
            .into_state();
        for criterion in ["correct", "tested"] {
            state = state
                .assess_goal_criterion_v2(
                    state.trace().revision(),
                    &assessment_attempt,
                    GoalRevisionNumberV2::new(revision),
                    assessment_result(criterion).result().clone(),
                    None,
                    UnixMillis::new(1_700_000_002_001 + revision * 10),
                )
                .unwrap()
                .into_state();
        }
        state = state
            .decide_active_route_with_goal_revision_v2(
                state.trace().revision(),
                &assessment_attempt,
                OptionId::new("achieved").unwrap(),
                AttemptId::new(format!("00000000-0000-4000-8004-{revision:012x}")).unwrap(),
                Some(GoalRevisionNumberV2::new(revision)),
                Some(ReasonV2::new(format!("Revision {revision} is achieved.")).unwrap()),
                None,
                UnixMillis::new(1_700_000_002_002 + revision * 10),
            )
            .unwrap()
            .into_state();
    }
    state
}

fn maximum_reachable_identifier(prefix: &str, index: usize) -> String {
    let prefix = format!("{prefix}-{index:02}-");
    format!("{prefix}{}", "x".repeat(64 - prefix.len()))
}

fn combined_maximum_procedure() -> Vec<u8> {
    let node_ids = (0..64)
        .map(|index| maximum_reachable_identifier("node", index))
        .collect::<Vec<_>>();
    let assessment_definition = maximum_reachable_identifier("assess", 0);
    let work_definition = maximum_reachable_identifier("work", 0);
    let transit_definition = maximum_reachable_identifier("transit", 0);
    let achieved = maximum_reachable_identifier("achieved", 0);
    let not_achieved = maximum_reachable_identifier("not-achieved", 0);
    let superseded = maximum_reachable_identifier("superseded", 0);
    let items = (0..64)
        .map(|index| {
            json!({
                "id": maximum_reachable_identifier("item", index),
                "type": "text",
                "prompt": "p".repeat(300),
                "help": "h".repeat(1_000),
                "required": false,
                "max_length": 16_384
            })
        })
        .collect::<Vec<_>>();
    let mut nodes = Vec::with_capacity(64);
    nodes.push(json!({
        "id": node_ids[0],
        "use": assessment_definition,
        "routes": {
            achieved.clone(): {"to": node_ids[63], "effect": "advance"},
            not_achieved.clone(): {"to": node_ids[1], "effect": "rework"},
            superseded.clone(): {"to": node_ids[63], "effect": "advance"}
        }
    }));
    for index in 1..63 {
        nodes.push(json!({
            "id": node_ids[index],
            "use": &work_definition,
            "next": if index == 62 { &node_ids[0] } else { &node_ids[index + 1] }
        }));
    }
    nodes.push(json!({"id": node_ids[63], "use": &transit_definition, "terminal": true}));
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": maximum_reachable_identifier("procedure", 0),
        "version": "v".repeat(64),
        "name": "n".repeat(120),
        "purpose": "p".repeat(500),
        "goal_tracking": true,
        "node_definitions": {
            assessment_definition.clone(): {
                "type": "decision",
                "title": "a".repeat(120),
                "objective": "o".repeat(300),
                "prompt": "q".repeat(300),
                "options": [
                    {"id": achieved, "label": "a".repeat(120)},
                    {"id": not_achieved, "label": "n".repeat(120)},
                    {"id": superseded, "label": "s".repeat(120)}
                ],
                "reason": {"required": true},
                "assessment": {"target": "session_goal", "outcomes": {
                    maximum_reachable_identifier("achieved", 0): "achieved",
                    maximum_reachable_identifier("not-achieved", 0): "not_achieved",
                    maximum_reachable_identifier("superseded", 0): "superseded"
                }}
            },
            work_definition.clone(): {
                "type": "action",
                "title": "w".repeat(120),
                "intent": "i".repeat(300),
                "description": "d".repeat(1_000),
                "instructions": vec!["j".repeat(1_000); 12],
                "items": items
            },
            transit_definition.clone(): {
                "type": "action",
                "title": "t".repeat(120),
                "intent": "i".repeat(300)
            }
        },
        "graph": {"entry": node_ids[1], "nodes": nodes},
        "manual_rework": {"allowed_targets": [node_ids[1]]}
    }))
    .unwrap()
}

fn maximum_goal_definition() -> GoalDefinitionV2 {
    GoalDefinitionV2::new(
        (0..16)
            .map(|index| {
                GoalCriterionV2::new(
                    CriterionId::new(maximum_reachable_identifier("criterion", index)).unwrap(),
                    "c".repeat(300),
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap()
}

fn assess_maximum_goal(
    mut state: GraphSessionStateV2,
    next_attempt_number: &mut u64,
    now: &mut u64,
) -> GraphSessionStateV2 {
    let active = state.trace().active_attempt().unwrap().attempt_id().clone();
    let revision = state.goal_state().current_revision().unwrap();
    for criterion in maximum_goal_definition().criteria() {
        state = state
            .assess_goal_criterion_v2(
                state.trace().revision(),
                &active,
                revision,
                CriterionAssessmentResultV2::new(
                    criterion.id().clone(),
                    CriterionStatusV2::Unsatisfied,
                    CriterionAssessmentReasonV2::new("r".repeat(2_000)).unwrap(),
                    Vec::new(),
                )
                .unwrap(),
                Some(ActorAttributionV2::new("a".repeat(256)).unwrap()),
                UnixMillis::new(*now),
            )
            .unwrap()
            .into_state();
        *now += 1;
    }
    let fresh = AttemptId::new(format!(
        "00000000-0000-4000-9000-{:012x}",
        *next_attempt_number
    ))
    .unwrap();
    *next_attempt_number += 1;
    let not_achieved = OptionId::new(maximum_reachable_identifier("not-achieved", 0)).unwrap();
    let outcome = state
        .decide_active_route_with_goal_revision_v2(
            state.trace().revision(),
            &active,
            not_achieved,
            fresh,
            Some(revision),
            Some(ReasonV2::new("d".repeat(2_000)).unwrap()),
            Some(ActorAttributionV2::new("a".repeat(256)).unwrap()),
            UnixMillis::new(*now),
        )
        .unwrap();
    *now += 1;
    outcome.into_state()
}

fn advance_maximum_actions(
    mut state: GraphSessionStateV2,
    count: usize,
    next_attempt_number: &mut u64,
    now: &mut u64,
) -> GraphSessionStateV2 {
    for _ in 0..count {
        let active = state.trace().active_attempt().unwrap().attempt_id().clone();
        let fresh = AttemptId::new(format!(
            "00000000-0000-4000-9000-{:012x}",
            *next_attempt_number
        ))
        .unwrap();
        *next_attempt_number += 1;
        state = state
            .complete_active_action_v2(
                state.trace().revision(),
                &active,
                Some(fresh),
                UnixMillis::new(*now),
            )
            .unwrap()
            .into_state();
        *now += 1;
    }
    state
}

fn combined_maximum_verbose_state() -> GraphSessionStateV2 {
    let procedure = combined_maximum_procedure();
    let mut state = fresh_state("combined-maximum.json", &procedure)
        .bind_initial_goal_at_start_v2(
            GoalStatementV2::new("g".repeat(1_000)).unwrap(),
            maximum_goal_definition(),
            Some(ActorAttributionV2::new("a".repeat(256)).unwrap()),
            UnixMillis::new(1_700_000_003_000),
        )
        .unwrap()
        .into_state();
    let mut next_attempt_number = 1_u64;
    let mut now = 1_700_000_003_001_u64;
    for cycle in 0..7 {
        state = advance_maximum_actions(state, 62, &mut next_attempt_number, &mut now);
        state = assess_maximum_goal(state, &mut next_attempt_number, &mut now);
        if cycle < 2 {
            let active = state.trace().active_attempt().unwrap().attempt_id().clone();
            let fresh = AttemptId::new(format!(
                "00000000-0000-4000-9000-{:012x}",
                next_attempt_number
            ))
            .unwrap();
            next_attempt_number += 1;
            state = state
                .revise_goal_v2(
                    state.trace().revision(),
                    Some(&active),
                    GoalRevisionNumberV2::new(cycle + 1),
                    GoalStatementV2::new("g".repeat(1_000)).unwrap(),
                    maximum_goal_definition(),
                    GraphNodeId::new(maximum_reachable_identifier("node", 1)).unwrap(),
                    fresh,
                    GoalRevisionReasonV2::new("r".repeat(1_000)).unwrap(),
                    Some(ActorAttributionV2::new("a".repeat(256)).unwrap()),
                    false,
                    UnixMillis::new(now),
                )
                .unwrap()
                .into_state();
            now += 1;
        }
    }
    state = advance_maximum_actions(state, 40, &mut next_attempt_number, &mut now);

    let current = state.trace().active_attempt().unwrap().attempt_id().clone();
    let memories = state
        .workflow_memory()
        .attempts()
        .iter()
        .map(|memory| {
            if memory.attempt_id() != &current {
                return memory.clone();
            }
            let slots = memory
                .item_slots()
                .iter()
                .map(|slot| {
                    ItemSlotStateV2::new(
                        slot.attempt_id().clone(),
                        slot.item_id().clone(),
                        slot.item_type(),
                        Revision::new(u64::MAX),
                        Some(RecordedItemValueV2::text("😀".repeat(3_000)).unwrap()),
                        slot.created_at(),
                        UnixMillis::new(now),
                    )
                    .unwrap()
                })
                .collect();
            AttemptWorkflowMemoryV2::new(
                memory.attempt_id().clone(),
                slots,
                memory.blockers().to_vec(),
                memory.evidence().to_vec(),
            )
            .unwrap()
        })
        .collect();
    let trace = SessionTraceV2::from_parts(
        state.trace().session_id().clone(),
        state.trace().lifecycle(),
        Revision::new(u64::MAX),
        state.trace().attempts().to_vec(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(u64::MAX),
        "t".repeat(500),
        state.snapshot().clone(),
        trace,
        state.counters().to_vec(),
        state.attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(
            memories,
            state.workflow_memory().decisions().to_vec(),
            state.workflow_memory().reworks().to_vec(),
        )
        .unwrap(),
        state.goal_state().clone(),
        state.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

#[test]
fn v2rel002_maximum_compact_status_with_large_u64_fields_fits_automation_cap() {
    let compact =
        project_graph_status_v2(&maximum_compact_view(), GraphStatusTierV2::Compact, None).unwrap();
    let counters = compact["counters"].as_array().unwrap();
    assert_eq!(counters.len(), 64);
    assert!(counters.iter().all(|counter| {
        counter["graph_node_id"]
            .as_str()
            .is_some_and(|id| id.len() == 64)
    }));
    assert_eq!(compact["procedure"]["id"].as_str().unwrap().len(), 64);
    assert_eq!(compact["procedure"]["version"].as_str().unwrap().len(), 64);
    assert_eq!(compact["items"].as_array().unwrap().len(), 64);
    assert!(compact["items"].as_array().unwrap().iter().all(|item| {
        item["item_id"].as_str().is_some_and(|id| id.len() == 64) && item["revision"] == u64::MAX
    }));
    assert_eq!(
        compact["current"]["node"]["graph_node_id"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        compact["current"]["node"]["node_definition_id"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(compact["session"]["revision"], u64::MAX);
    assert_eq!(compact["queue"]["latest_workspace_sequence"], u64::MAX);
    assert_maximum_production_output_v2("session.status", compact, 262_144);
}

#[test]
fn v2rel002_standard_item_values_stop_before_the_first_whole_value_over_budget() {
    const STATUS_VALUES_MAX: usize = 262_144;
    let standard = project_graph_status_v2(
        &view(status_values_boundary_state()),
        GraphStatusTierV2::Standard,
        None,
    )
    .unwrap();
    let values = standard["item_values"].as_array().unwrap();
    assert_eq!(standard["items_total"], 64);
    assert_eq!(standard["items_truncated"], true);
    assert!(!values.is_empty() && values.len() < 64);
    assert!(serde_json::to_vec(values).unwrap().len() <= STATUS_VALUES_MAX);
    for value in values {
        assert_eq!(value["value"].as_str().unwrap().chars().count(), 2_048);
        assert_eq!(value["value_truncated"], true);
    }
    let next_index = values.len();
    let mut with_next = values.clone();
    with_next.push(json!({
        "item_id": format!("item-{next_index:02}"),
        "value": "😀".repeat(2_048),
        "value_truncated": true,
    }));
    assert!(
        serde_json::to_vec(&with_next).unwrap().len() > STATUS_VALUES_MAX,
        "the first omitted complete value did not cross STATUS_VALUES_MAX"
    );
    assert_output_v2("session.status", standard);
}

fn assert_truncated_history_window(history: &Value, expected_len: usize) {
    let entries = history["entries"].as_array().unwrap();
    assert_eq!(entries.len(), expected_len);
    assert_eq!(history["trace_truncated"], true);
    let sequences = entries
        .iter()
        .map(|entry| entry["trace_sequence"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert!(
        sequences.windows(2).all(|pair| pair[0] > pair[1]),
        "retained history is not strictly newest-first: {sequences:?}"
    );
    assert_eq!(
        history["trace_window"]["first_sequence"],
        *sequences.iter().min().unwrap()
    );
    assert_eq!(
        history["trace_window"]["last_sequence"],
        *sequences.iter().max().unwrap()
    );
    assert!(serde_json::to_vec(history).unwrap().len() <= 65_536);
}

fn assert_dual_bounded_history_window(
    view: &GraphWorkspaceViewV2,
    verbose: &Map<String, Value>,
    history: &str,
    family_count_cap: usize,
) {
    const TRACE_WINDOW_MAX: usize = 65_536;
    let retained = verbose[history]["entries"].as_array().unwrap();
    assert_eq!(retained.len(), family_count_cap);
    let oldest_retained = retained
        .iter()
        .map(|entry| entry["trace_sequence"].as_u64().unwrap())
        .min()
        .unwrap();
    let older = project_graph_status_v2(
        view,
        GraphStatusTierV2::Verbose,
        Some(TraceSequenceV2::new(oldest_retained)),
    )
    .unwrap();
    let first_omitted = older[history]["entries"]
        .as_array()
        .unwrap()
        .first()
        .unwrap_or_else(|| panic!("{history} has no first omitted entry"));
    let omitted_sequence = first_omitted["trace_sequence"].as_u64().unwrap();
    assert!(omitted_sequence < oldest_retained);

    let mut complete_candidate = retained.clone();
    complete_candidate.push(first_omitted.clone());
    let candidate = json!({
        "entries": complete_candidate,
        "trace_truncated": false,
        "trace_window": {
            "first_sequence": omitted_sequence,
            "last_sequence": retained
                .iter()
                .map(|entry| entry["trace_sequence"].as_u64().unwrap())
                .max()
                .unwrap(),
        }
    });
    let candidate_bytes = serde_json::to_vec(&candidate).unwrap().len();
    assert!(
        candidate_bytes > TRACE_WINDOW_MAX || retained.len() == family_count_cap,
        "{history} omitted a complete entry before either dual bound was reached"
    );
}

#[test]
fn v2rel002_every_verbose_history_family_marks_actual_omissions_and_exact_window() {
    let trace = project_graph_status_v2(
        &view(wide_running_state()),
        GraphStatusTierV2::Verbose,
        None,
    )
    .unwrap();
    assert_truncated_history_window(&trace["current_trace_history"], 32);
    assert_eq!(
        trace["current_trace_history"]["trace_window"]["first_sequence"],
        33
    );
    assert_eq!(
        trace["current_trace_history"]["trace_window"]["last_sequence"],
        64
    );
    assert_output_v2("session.status", trace);

    let workflow = project_graph_status_v2(
        &view(decision_rework_history_state()),
        GraphStatusTierV2::Verbose,
        None,
    )
    .unwrap();
    assert_truncated_history_window(&workflow["stale_attempt_history"], 1);
    assert_truncated_history_window(&workflow["decision_history"], 1);
    assert_truncated_history_window(&workflow["rework_history"], 6);
    assert_output_v2("session.status", workflow);

    let goal = project_graph_status_v2(
        &view(goal_history_state()),
        GraphStatusTierV2::Verbose,
        None,
    )
    .unwrap();
    assert_truncated_history_window(&goal["stale_goal_revision_history"], 1);
    assert_truncated_history_window(&goal["stale_goal_assessment_history"], 1);
    assert_output_v2("session.status", goal);
}

#[test]
fn v2rel002_one_maximum_verbose_projection_combines_every_bounded_family() {
    const STATUS_VALUES_MAX: usize = 262_144;
    const FRAME_MAX: usize = 1_048_576;
    let combined_view = view(combined_maximum_verbose_state());
    let verbose =
        project_graph_status_v2(&combined_view, GraphStatusTierV2::Verbose, None).unwrap();

    assert_eq!(verbose["counters"].as_array().unwrap().len(), 64);
    assert!(
        verbose["counters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|counter| {
                counter["graph_node_id"]
                    .as_str()
                    .is_some_and(|id| id.len() == 64)
            })
    );
    assert_eq!(verbose["procedure"]["id"].as_str().unwrap().len(), 64);
    assert_eq!(verbose["procedure"]["version"].as_str().unwrap().len(), 64);
    assert_eq!(
        verbose["current"]["node"]["graph_node_id"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(verbose["session"]["revision"], u64::MAX);

    assert_eq!(verbose["items_total"], 64);
    assert_eq!(verbose["items_truncated"], true);
    let item_values = verbose["item_values"].as_array().unwrap();
    let item_values_bytes = serde_json::to_vec(item_values).unwrap().len();
    assert!(
        (250_000..=STATUS_VALUES_MAX).contains(&item_values_bytes),
        "combined item window was not near its cap: {item_values_bytes} bytes"
    );
    assert!(item_values.iter().all(|item| {
        item["item_id"].as_str().is_some_and(|id| id.len() == 64)
            && item["value"].as_str().unwrap().chars().count() == 2_048
            && item["value_truncated"] == true
    }));

    for (history, maximum) in [
        ("current_trace_history", 32),
        ("stale_attempt_history", 1),
        ("decision_history", 1),
        ("rework_history", 6),
        ("stale_goal_revision_history", 1),
        ("stale_goal_assessment_history", 1),
    ] {
        assert_truncated_history_window(&verbose[history], maximum);
        assert_dual_bounded_history_window(&combined_view, &verbose, history, maximum);
    }

    assert_maximum_production_output_v2("session.status", verbose, FRAME_MAX);
}

#[test]
fn v2rel002_escape_heavy_next_charge_dominates_every_encoded_component() {
    let source = escape_heavy_next_source();
    let validated = validated_json_procedure(&source);
    let production_budget =
        procedure_placement_budget_v2(&validated, &GraphNodeId::new("work").unwrap()).unwrap();
    let next = Value::Object(
        project_graph_next_v2(&view(escape_heavy_next_state(&source)))
            .expect("project maximum next"),
    );
    let fields = next.as_object().unwrap();
    let static_fields = [
        "goal_tracking",
        "title",
        "intent",
        "description",
        "instructions",
        "missing_required_item_count",
        "missing_required_items",
        "terminal",
        "allowed_actions",
        "suggestions",
        "allowed_manual_rework_targets",
    ];
    let static_projection = Value::Object(
        static_fields
            .iter()
            .map(|field| ((*field).to_owned(), fields[*field].clone()))
            .collect(),
    );
    let static_encoded = serde_json::to_vec(&static_projection).unwrap();
    assert!(
        u64::try_from(static_encoded.len()).unwrap() <= production_budget.next_static(),
        "production static charge under-counted escape-heavy next: encoded={}, charged={}",
        static_encoded.len(),
        production_budget.next_static(),
    );
    assert!(
        production_budget.next_static() <= podway_config::NEXT_STATIC_BUDGET,
        "admitted escape-heavy procedure exceeded its static next budget: {}",
        production_budget.next_static(),
    );
    assert_eq!(fields["title"].as_str().unwrap().chars().count(), 120);
    assert_eq!(fields["intent"].as_str().unwrap().chars().count(), 300);
    assert_eq!(
        fields["description"].as_str().unwrap().chars().count(),
        1_000
    );
    assert_eq!(fields["instructions"].as_array().unwrap().len(), 12);
    assert_eq!(
        fields["missing_required_items"].as_array().unwrap().len(),
        20
    );
    assert_output_v2("session.next", next.as_object().unwrap().clone());
}

fn component(fields: &Map<String, Value>, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .map(|name| ((*name).to_owned(), fields[*name].clone()))
            .collect(),
    )
}

fn maximum_payload_identifier(prefix: char, index: usize) -> String {
    format!("{prefix}{}{:02}", "0".repeat(61), index)
}

fn item_identifier(index: usize) -> String {
    format!("i{}{:02}", "0".repeat(23), index)
}

fn maximum_next_readback() -> (Vec<Value>, Vec<Value>) {
    let references = (0..8)
        .map(|index| {
            json!({
                "source_graph_node_id": maximum_payload_identifier('s', index),
                "source_title": "\0".repeat(120),
                "source_attempt_id": format!("00000000-0000-4000-8000-{:012x}", 0x5000 + index),
                "source_attempt_number": u64::MAX,
                "items_digest": IDENTITY_DIGEST,
                "state": "resolved"
            })
        })
        .collect::<Vec<_>>();
    let mut readback = references
        .iter()
        .map(|reference| {
            let mut value = reference.clone();
            value
                .as_object_mut()
                .unwrap()
                .insert("items".into(), json!([]));
            value
        })
        .collect::<Vec<_>>();

    // V2SCL-004 moved complete values out of the readback surface, so the maximum is no longer a
    // byte-packing problem: the response carries at most 128 item metadata records across all
    // references. V2SCL-005 owns proving the new six-bucket allocation table against this fixture.
    const READBACK_ITEM_METADATA_MAX: usize = 128;
    for item_index in 0..READBACK_ITEM_METADATA_MAX {
        let source_index = item_index % readback.len();
        readback[source_index]["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "item_id": item_identifier(item_index),
                "type": "text",
                "revision": u64::MAX,
                "value_digest": format!("sha256:{}", "a".repeat(64)),
                "total_size": 65_536,
                "size_unit": "scalars"
            }));
    }
    for reference in &mut readback {
        let total = reference["items"].as_array().unwrap().len();
        reference
            .as_object_mut()
            .unwrap()
            .insert("items_total".into(), json!(total));
        reference
            .as_object_mut()
            .unwrap()
            .insert("items_truncated".into(), json!(false));
    }
    (references, readback)
}

fn complete_maximum_next_result() -> Map<String, Value> {
    let allowed_actions = [
        "item.set",
        "session.retry",
        "session.unblock",
        "session.cancel",
        "session.reset",
        "session.rework",
        "goal.revise",
        "goal.assess_criterion",
    ];
    let mut suggestions = (0..64)
        .map(|index| {
            let item_id = item_identifier(index);
            json!({
                "command": "item.set",
                "argv": ["podway", "set", item_id, "<text>"],
                "item_id": item_id
            })
        })
        .collect::<Vec<_>>();
    suggestions.push(json!({
        "command": "session.retry",
        "argv": ["podway", "retry", "--reason", "<reason>"]
    }));
    suggestions.extend((0..16).map(|index| {
        let criterion_id = maximum_payload_identifier('c', index);
        json!({
            "command": "goal.assess_criterion",
            "argv": [
                "podway",
                "goal",
                "assess-criterion",
                criterion_id,
                "--status",
                "<status>",
                "--reason",
                "<reason>"
            ]
        })
    }));

    let (references, readback) = maximum_next_readback();
    let blockers = (0..64)
        .map(|index| {
            json!({
                "blocker_id": format!("00000000-0000-4000-8000-{:012x}", 0x6000 + index),
                "reason": "\0".repeat(1_000),
                "created_at": "2026-08-09T00:00:00.000Z"
            })
        })
        .scan(Vec::<Value>::new(), |window, blocker| {
            let mut candidate = window.clone();
            candidate.push(blocker);
            if conservative_json_charge(&json!({
                "blockers": candidate,
                "blockers_total": 64,
                "blockers_truncated": true
            })) <= 49_152
            {
                *window = candidate;
                Some(Some(window.last().unwrap().clone()))
            } else {
                Some(None)
            }
        })
        .flatten()
        .collect::<Vec<_>>();

    json!({
        "schema": "podway.next-result/v3",
        "procedure_schema": "podway.procedure/v2",
        "procedure_digest": IDENTITY_DIGEST,
        "goal_tracking": true,
        "goal_defined": true,
        "goal_revision": u64::MAX,
        "latest_goal_outcome": "achieved",
        "goal": {
            "revision": u64::MAX,
            "statement": "\0".repeat(1_000),
            "criteria": (0..16).map(|index| json!({
                "criterion_id": maximum_payload_identifier('c', index),
                "statement": "\0".repeat(300),
                "status": "unassessed"
            })).collect::<Vec<_>>(),
        },
        "node": {
            "node_definition_id": "work-definition",
            "graph_node_id": "work-placement",
            "node_type": "decision"
        },
        "attempt": {
            "attempt_id": ATTEMPT_ID,
            "attempt_number": u64::MAX
        },
        "trace_length": u64::MAX,
        "counters": (0..64).map(|index| json!({
            "graph_node_id": maximum_payload_identifier('n', index),
            "attempt_count": u64::MAX,
            "rework_traversal_count": u64::MAX
        })).collect::<Vec<_>>(),
        "queue": {
            "pending_mutations": true,
            "queued_count": u32::MAX,
            "running_job_id": "00000000-0000-4000-8000-000000006100",
            "latest_workspace_sequence": u64::MAX
        },
        "revision": u64::MAX,
        "readiness": {
            "items_satisfied": false,
            "unblocked": false,
            "goal_ready": true,
            "can_advance": false
        },
        "title": "\0".repeat(120),
        "description": "\0".repeat(1_000),
        "objective": "\0".repeat(300),
        "prompt": "\0".repeat(500),
        "reason_policy": {"required": true, "prompt": "\0".repeat(300)},
        "missing_required_item_count": 64,
        "missing_required_items": (0..64).map(|index| json!({
            "item_id": item_identifier(index),
            "prompt": "\0".repeat(300)
        })).collect::<Vec<_>>(),
        "options": (0..8).map(|index| json!({
            "option_id": maximum_payload_identifier('o', index),
            "label": "\0".repeat(120),
            "criteria": "\0".repeat(500)
        })).collect::<Vec<_>>(),
        "allowed_option_ids": [],
        "evidence_guidance": vec!["\0".repeat(200); 8],
        "allowed_manual_rework_targets": (0..64)
            .map(|index| maximum_payload_identifier('n', index))
            .collect::<Vec<_>>(),
        "allowed_actions": allowed_actions,
        "missing_required_items_truncated": false,
        "suggestions": suggestions,
        "suggestions_total": 128,
        "suggestions_truncated": false,
        "references": references,
        "readback": readback,
        "readback_items_total": 128,
        "readback_items_truncated": false,
        "blockers_total": 64,
        "blockers": blockers,
        "blockers_truncated": true
    })
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn v2rel002_complete_maximum_next_binds_component_charges_to_production_framing() {
    const FRAME_BYTES: usize = 1_048_576;
    const BUDGETS: [usize; 6] = [65_536, 262_144, 524_288, 73_728, 49_152, 40_960];
    let recipe: Value = serde_json::from_slice(MAXIMUM_NEXT_RECIPE).unwrap();
    assert_eq!(recipe["frame_bytes"], FRAME_BYTES);
    assert_eq!(recipe["charged_bytes"], 1_015_808);
    assert_eq!(recipe["headroom_bytes"], 32_768);
    for (name, budget) in [
        ("ENVELOPE_RESERVE", BUDGETS[0]),
        ("NEXT_STATIC_BUDGET", BUDGETS[1]),
        ("READBACK_BUDGET", BUDGETS[2]),
        ("GOAL_DISPLAY_MAX", BUDGETS[3]),
        ("BLOCKER_WINDOW_MAX", BUDGETS[4]),
        ("COUNTERS_MAX", BUDGETS[5]),
    ] {
        assert_eq!(recipe["components"][name], budget);
    }
    assert_eq!(BUDGETS.iter().sum::<usize>(), 1_015_808);
    assert_eq!(FRAME_BYTES - BUDGETS.iter().sum::<usize>(), 32_768);

    let result = complete_maximum_next_result();
    let static_suggestions = result["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| {
            !matches!(
                suggestion["command"].as_str().unwrap(),
                "goal.assess_criterion" | "goal.define"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let goal_suggestions = result["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| {
            matches!(
                suggestion["command"].as_str().unwrap(),
                "goal.assess_criterion" | "goal.define"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(goal_suggestions.len(), 16);
    assert_eq!(result["blockers_total"], 64);
    assert!(result["blockers"].as_array().unwrap().len() >= 7);
    assert_eq!(result["counters"].as_array().unwrap().len(), 64);
    assert_eq!(result["references"].as_array().unwrap().len(), 8);
    assert_eq!(result["readback"].as_array().unwrap().len(), 8);
    let assigned_result_fields = [
        "schema",
        "procedure_schema",
        "procedure_digest",
        "goal_tracking",
        "goal_defined",
        "goal_revision",
        "latest_goal_outcome",
        "goal",
        "node",
        "attempt",
        "trace_length",
        "counters",
        "queue",
        "revision",
        "readiness",
        "title",
        "description",
        "objective",
        "prompt",
        "reason_policy",
        "missing_required_item_count",
        "missing_required_items",
        "missing_required_items_truncated",
        "options",
        "allowed_option_ids",
        "evidence_guidance",
        "allowed_manual_rework_targets",
        "allowed_actions",
        "suggestions",
        "suggestions_total",
        "suggestions_truncated",
        "references",
        "readback",
        "readback_items_total",
        "readback_items_truncated",
        "blockers_total",
        "blockers",
        "blockers_truncated",
    ];
    assert_eq!(
        assigned_result_fields
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        result
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        "every field applicable to the schema-valid action shape must be assigned once"
    );

    let mut envelope_fields = component(
        &result,
        &[
            "schema",
            "procedure_schema",
            "procedure_digest",
            "node",
            "attempt",
            "queue",
            "revision",
            "missing_required_item_count",
            "readiness",
            "suggestions_total",
            "suggestions_truncated",
            "readback_items_total",
            "readback_items_truncated",
        ],
    );
    let mut static_fields = component(
        &result,
        &[
            "allowed_actions",
            "allowed_manual_rework_targets",
            "allowed_option_ids",
            "description",
            "evidence_guidance",
            "missing_required_items",
            "missing_required_items_truncated",
            "objective",
            "options",
            "prompt",
            "reason_policy",
            "title",
        ],
    );
    static_fields
        .as_object_mut()
        .unwrap()
        .insert("suggestions".into(), json!(static_suggestions));
    let readback_fields = component(&result, &["readback", "references"]);
    let mut goal_fields = component(
        &result,
        &[
            "goal",
            "goal_defined",
            "goal_revision",
            "goal_tracking",
            "latest_goal_outcome",
        ],
    );
    goal_fields
        .as_object_mut()
        .unwrap()
        .insert("suggestions".into(), json!(goal_suggestions));
    let blocker_fields = component(
        &result,
        &["blockers", "blockers_total", "blockers_truncated"],
    );
    let counter_fields = component(&result, &["counters", "trace_length"]);

    let warnings = (0..4)
        .map(|_| {
            json!({
                "code": "A".repeat(64),
                "path": "\0".repeat(256),
                "message": "\0".repeat(512)
            })
            .as_object()
            .unwrap()
            .clone()
        })
        .collect::<Vec<_>>();
    let workspace = WorkspaceOutputV1::new(
        WorkspaceId::new(WORKSPACE_ID).unwrap(),
        "\0".repeat(4_096),
        u64::MAX,
    )
    .unwrap();
    let session = SessionOutputV1::new(
        SessionId::new(SESSION_ID).unwrap(),
        "\0".repeat(500),
        SessionLifecycleV1::Running,
        Revision::new(u64::MAX),
        Revision::new(u64::MAX),
    )
    .unwrap();
    let output = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("session.next").unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-09T00:00:00.000Z").unwrap(),
        workspace: Some(workspace),
        job: None,
        session: Some(session),
        result: result.clone(),
        warnings,
    })
    .unwrap();
    let output_value = serde_json::to_value(&output).unwrap();
    assert_eq!(output_value["warnings"].as_array().unwrap().len(), 4);
    let envelope_object = output_value.as_object().unwrap();
    envelope_fields.as_object_mut().unwrap().extend(
        [
            "request_id",
            "command",
            "generated_at",
            "workspace",
            "job",
            "session",
            "warnings",
        ]
        .into_iter()
        .filter_map(|field| {
            envelope_object
                .get(field)
                .map(|value| (field.to_owned(), value.clone()))
        }),
    );
    envelope_fields
        .as_object_mut()
        .unwrap()
        .insert("output_schema".into(), envelope_object["schema"].clone());
    envelope_fields
        .as_object_mut()
        .unwrap()
        .insert("frame_length_prefix_bytes".into(), json!(4));

    let components = [
        ("ENVELOPE_RESERVE", envelope_fields, BUDGETS[0]),
        ("NEXT_STATIC_BUDGET", static_fields, BUDGETS[1]),
        ("READBACK_BUDGET", readback_fields, BUDGETS[2]),
        ("GOAL_DISPLAY_MAX", goal_fields, BUDGETS[3]),
        ("BLOCKER_WINDOW_MAX", blocker_fields, BUDGETS[4]),
        ("COUNTERS_MAX", counter_fields, BUDGETS[5]),
    ];
    for (name, value, budget) in &components {
        let encoded = serde_json::to_vec(value).unwrap().len();
        let charged = conservative_json_charge(value);
        assert!(
            encoded <= charged,
            "{name}: encoded={encoded}, charged={charged}"
        );
        assert!(
            charged <= *budget,
            "{name}: charged={charged}, budget={budget}"
        );
    }
    assert!(BUDGETS[1] - conservative_json_charge(&components[1].1) < 16_384);
    // The readback surface no longer carries values, so it cannot fill the superseded 512 KiB
    // value budget and a tight-fit assertion here would be meaningless. Every component is still
    // proved within budget above, and the composed frame is proved below. V2SCL-005 owns
    // re-deriving the six named allocations ADR-0024 defines and asserting tightness against them.
    assert!(conservative_json_charge(&components[2].1) <= BUDGETS[2]);

    let response = ResponseEnvelopeV2::OutputV2(output);
    let encoded = encode_response_payload_v2(&response).unwrap();
    assert!(
        encoded.len() <= FRAME_BYTES,
        "encoded next used {} bytes",
        encoded.len()
    );
    validate_frame_payload_length(encoded.len()).unwrap();
    assert_eq!(decode_response_payload_v2(&encoded).unwrap(), response);
    let frame = encode_frame_v1(&encoded).unwrap();
    assert_eq!(decode_single_frame_v1(&frame).unwrap(), encoded);
}

const SELECTED_READBACK_ITEM_IDS: [&str; 16] = [
    "confirm",
    "text",
    "choice",
    "integer",
    "list",
    "artifact",
    "selected-00",
    "selected-01",
    "selected-02",
    "selected-03",
    "selected-04",
    "selected-05",
    "selected-06",
    "selected-07",
    "selected-08",
    "selected-09",
];

/// Assemble a completed `source` attempt and an active `consume` attempt into one session state.
///
/// Every read-back fixture needs the same two-attempt shape, so only the recorded values and the
/// evidence selection differ between them.
fn source_and_consumer_state(
    base: &GraphSessionStateV2,
    source_memory: AttemptWorkflowMemoryV2,
    consumer_memory: AttemptWorkflowMemoryV2,
) -> GraphSessionStateV2 {
    source_and_consumer_state_with_validity(
        base,
        source_memory,
        consumer_memory,
        AttemptValidityV2::Valid,
    )
}

/// The same shape with the source attempt's validity chosen by the caller.
///
/// An invalidated source attempt is how a declared reference goes stale, which the read path must
/// classify differently from a page token that lost its snapshot.
fn source_and_consumer_state_with_validity(
    base: &GraphSessionStateV2,
    source_memory: AttemptWorkflowMemoryV2,
    consumer_memory: AttemptWorkflowMemoryV2,
    source_validity: AttemptValidityV2,
) -> GraphSessionStateV2 {
    let source_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let consumer_attempt_id = second_attempt_id();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(2),
        vec![
            SessionAttemptV2::new(
                source_attempt_id.clone(),
                GraphNodeId::new("source").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::FIRST,
                AttemptLifecycle::Completed,
                source_validity,
                None,
            )
            .unwrap(),
            SessionAttemptV2::new(
                consumer_attempt_id.clone(),
                GraphNodeId::new("consume").unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(2),
                AttemptLifecycle::Active,
                AttemptValidityV2::Valid,
                None,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(2),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        vec![
            GraphNodeCounterV2::new(GraphNodeId::new("source").unwrap(), 1, 0),
            GraphNodeCounterV2::new(GraphNodeId::new("consume").unwrap(), 1, 0),
        ],
        vec![
            AttemptMetadataV2::new(
                source_attempt_id,
                UnixMillis::new(1_700_000_000_000),
                Some(UnixMillis::new(1_700_000_000_010)),
                None,
            )
            .unwrap(),
            AttemptMetadataV2::new(
                consumer_attempt_id,
                UnixMillis::new(1_700_000_000_010),
                None,
                None,
            )
            .unwrap(),
        ],
        WorkflowMemoryStateV2::new(vec![source_memory, consumer_memory], Vec::new(), Vec::new())
            .unwrap(),
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

/// Two references to one source node, each selecting a different item.
fn duplicate_source_readback_state(source: &[u8]) -> GraphSessionStateV2 {
    let base = fresh_state("duplicate-source-readback.json", source);
    let source_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let consumer_attempt_id = second_attempt_id();
    let slots = base.workflow_memory().attempts()[0]
        .item_slots()
        .iter()
        .map(|slot| {
            ItemSlotStateV2::new(
                source_attempt_id.clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(
                    RecordedItemValueV2::text(format!("value-{}", slot.item_id().as_str()))
                        .unwrap(),
                ),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let source_memory =
        AttemptWorkflowMemoryV2::new(source_attempt_id.clone(), slots, Vec::new(), Vec::new())
            .unwrap();
    let source_digest = source_memory.recorded_items_digest().unwrap();
    let reference = |ordinal: u32, item: &str| {
        EvidenceResolutionStateV2::new(
            ordinal,
            true,
            vec![ItemId::new(item).unwrap()],
            ResolvedEvidenceReferenceV2::resolved(
                EvidenceReferenceSnapshotV2::new(
                    GraphNodeId::new("source").unwrap(),
                    source_attempt_id.clone(),
                    AttemptNumberV2::FIRST,
                    source_digest.clone(),
                    UnixMillis::new(1_700_000_000_010),
                )
                .unwrap(),
            ),
        )
        .unwrap()
    };
    let consumer_memory = AttemptWorkflowMemoryV2::new(
        consumer_attempt_id,
        Vec::new(),
        Vec::new(),
        vec![reference(0, "first"), reference(1, "second")],
    )
    .unwrap();
    source_and_consumer_state(&base, source_memory, consumer_memory)
}

/// One text item large enough that a single page cannot carry it.
///
/// The maximum-selected fixture caps text at 16,384 scalars, which is terminal in one page, so the
/// multi-page continuation of a text value needs its own minimal Procedure.
fn paged_text_readback_source() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "paged-text-readback",
        "version": "2",
        "name": "Paged text read-back",
        "purpose": "Exercise multi-page continuation of one text value.",
        "node_definitions": {
            "source": {
                "type": "action",
                "title": "Record source",
                "intent": "Record one maximal text value.",
                "items": [
                    {"id": "text", "type": "text", "prompt": "Text.", "required": true, "max_length": 65_536}
                ]
            },
            "consume": {
                "type": "action",
                "title": "Consume source",
                "intent": "Read the recorded text one page at a time."
            }
        },
        "graph": {
            "entry": "source",
            "nodes": [
                {"id": "source", "use": "source", "next": "consume"},
                {
                    "id": "consume",
                    "use": "consume",
                    "evidence_from": [{"node": "source", "required": true, "items": ["text"]}],
                    "terminal": true
                }
            ]
        }
    }))
    .unwrap()
}

fn paged_text_readback_state(source: &[u8], text: &str) -> GraphSessionStateV2 {
    paged_text_readback_state_with_validity(source, text, AttemptValidityV2::Valid)
}

fn paged_text_readback_state_with_validity(
    source: &[u8],
    text: &str,
    source_validity: AttemptValidityV2,
) -> GraphSessionStateV2 {
    let base = fresh_state("paged-text-readback.json", source);
    let source_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let consumer_attempt_id = second_attempt_id();
    let slot = base
        .workflow_memory()
        .attempts()
        .first()
        .unwrap()
        .item_slots()
        .first()
        .unwrap()
        .clone();
    let source_memory = AttemptWorkflowMemoryV2::new(
        source_attempt_id.clone(),
        vec![
            ItemSlotStateV2::new(
                source_attempt_id.clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(RecordedItemValueV2::text(text).unwrap()),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let source_digest = source_memory.recorded_items_digest().unwrap();
    let consumer_memory = AttemptWorkflowMemoryV2::new(
        consumer_attempt_id,
        Vec::new(),
        Vec::new(),
        vec![
            EvidenceResolutionStateV2::new(
                0,
                true,
                vec![ItemId::new("text").unwrap()],
                ResolvedEvidenceReferenceV2::resolved(
                    EvidenceReferenceSnapshotV2::new(
                        GraphNodeId::new("source").unwrap(),
                        source_attempt_id,
                        AttemptNumberV2::FIRST,
                        source_digest,
                        UnixMillis::new(1_700_000_000_010),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    source_and_consumer_state_with_validity(&base, source_memory, consumer_memory, source_validity)
}

/// The wide node with every item recorded, so the value-bearing collections are at their widest.
fn wide_node_state_with_recorded_values(source: &[u8]) -> GraphSessionStateV2 {
    let base = wide_node_state(source);
    let attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let slots = base.workflow_memory().attempts()[0]
        .item_slots()
        .iter()
        .map(|slot| {
            let value = match slot.item_type() {
                podway_core::ItemTypeV1::Choice => {
                    RecordedItemValueV2::choice("choice-0".to_owned()).unwrap()
                }
                _ => RecordedItemValueV2::text("recorded").unwrap(),
            };
            ItemSlotStateV2::new(
                attempt_id.clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(value),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let memory = AttemptWorkflowMemoryV2::new(attempt_id, slots, Vec::new(), Vec::new()).unwrap();
    GraphSessionStateV2::new_with_goal_state(
        base.workspace_revision(),
        base.task_title(),
        base.snapshot().clone(),
        base.trace().clone(),
        base.counters().to_vec(),
        base.attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![memory], Vec::new(), Vec::new()).unwrap(),
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

/// One running attempt at the named placement of a multi-placement graph.
fn wide_graph_state(source: &[u8], placement: &str) -> GraphSessionStateV2 {
    let base = fresh_state("wide-graph.json", source);
    let attempt = SessionAttemptV2::new(
        AttemptId::new(ATTEMPT_ID).unwrap(),
        GraphNodeId::new(placement).unwrap(),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
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
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

/// One running attempt at a node declaring `items`, with nothing recorded.
///
/// The bounded windows (`missing_required_items`, observation choices) are properties of the
/// current node, so proving they truncate needs a node wider than the window itself.
fn wide_node_state(source: &[u8]) -> GraphSessionStateV2 {
    let base = fresh_state("wide-node.json", source);
    let attempt = SessionAttemptV2::new(
        AttemptId::new(ATTEMPT_ID).unwrap(),
        GraphNodeId::new("work").unwrap(),
        AttemptNumberV2::FIRST,
        TraceSequenceV2::FIRST,
        AttemptLifecycle::Active,
        AttemptValidityV2::Valid,
        None,
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(1),
        vec![attempt],
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
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

/// Three evidence references carrying 36 selected text items plus one list of oversized entries.
///
/// One reference selects at most 16 items, so the 32-slot preview budget is only reachable across
/// several references — and the maximum-selected fixture deliberately stays inside one.
const WIDE_SELECTION_SOURCES: [&str; 3] = ["source-a", "source-b", "source-c"];
const WIDE_SELECTION_ITEMS_PER_SOURCE: usize = 12;

fn wide_selection_item_ids(source_index: usize) -> Vec<String> {
    let mut ids: Vec<String> = (0..WIDE_SELECTION_ITEMS_PER_SOURCE)
        .map(|index| format!("wide-{source_index}-{index:02}"))
        .collect();
    if source_index == 0 {
        ids.push("wide-list".to_owned());
    }
    ids
}

fn wide_selection_readback_source() -> Vec<u8> {
    let mut definitions = Map::new();
    let mut nodes = Vec::new();
    for (source_index, source) in WIDE_SELECTION_SOURCES.iter().enumerate() {
        let items: Vec<Value> = wide_selection_item_ids(source_index)
            .into_iter()
            .map(|id| {
                if id == "wide-list" {
                    json!({
                        "id": id,
                        "type": "list",
                        "prompt": "Record entries.",
                        "required": true,
                        "max_items": 8,
                        "max_item_length": 1_000,
                        "unique": false
                    })
                } else {
                    json!({
                        "id": id,
                        "type": "text",
                        "prompt": "Record text.",
                        "required": true,
                        "max_length": 64
                    })
                }
            })
            .collect();
        definitions.insert(
            (*source).to_owned(),
            json!({
                "type": "action",
                "title": "Record source",
                "intent": "Record selected values.",
                "items": items
            }),
        );
        let next = WIDE_SELECTION_SOURCES
            .get(source_index + 1)
            .copied()
            .unwrap_or("consume");
        nodes.push(json!({"id": source, "use": source, "next": next}));
    }
    definitions.insert(
        "consume".to_owned(),
        json!({
            "type": "action",
            "title": "Consume sources",
            "intent": "Read every selected value."
        }),
    );
    let references: Vec<Value> = WIDE_SELECTION_SOURCES
        .iter()
        .enumerate()
        .map(|(source_index, source)| {
            json!({
                "node": source,
                "required": true,
                "items": wide_selection_item_ids(source_index)
            })
        })
        .collect();
    nodes.push(json!({
        "id": "consume",
        "use": "consume",
        "evidence_from": references,
        "terminal": true
    }));
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "wide-selection-readback",
        "version": "2",
        "name": "Wide selection read-back",
        "purpose": "Exercise the preview slot count and the per-slot byte budget.",
        "node_definitions": definitions,
        "graph": {"entry": WIDE_SELECTION_SOURCES[0], "nodes": nodes}
    }))
    .unwrap()
}

fn wide_selection_readback_state(source: &[u8], text: &str, entry: &str) -> GraphSessionStateV2 {
    let base = fresh_state("wide-selection-readback.json", source);
    let consumer_attempt_id = second_attempt_id();
    let created_at = UnixMillis::new(1_700_000_000_000);
    let mut attempt_ids = Vec::new();
    let mut memories = Vec::new();
    let mut references = Vec::new();
    let mut trace_attempts = Vec::new();
    let mut counters = Vec::new();
    let mut metadata = Vec::new();
    for (source_index, source_node) in WIDE_SELECTION_SOURCES.iter().enumerate() {
        let attempt_id =
            AttemptId::new(format!("00000000-0000-4000-8000-00000000300{source_index}")).unwrap();
        let ids = wide_selection_item_ids(source_index);
        let slots = ids
            .iter()
            .map(|id| {
                let value = if id == "wide-list" {
                    RecordedItemValueV2::list(vec![entry.to_owned(); 4]).unwrap()
                } else {
                    RecordedItemValueV2::text(text).unwrap()
                };
                ItemSlotStateV2::new(
                    attempt_id.clone(),
                    ItemId::new(id).unwrap(),
                    value.item_type(),
                    Revision::new(1),
                    Some(value),
                    created_at,
                    UnixMillis::new(created_at.get() + 1),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let memory =
            AttemptWorkflowMemoryV2::new(attempt_id.clone(), slots, Vec::new(), Vec::new())
                .unwrap();
        references.push(
            EvidenceResolutionStateV2::new(
                source_index as u32,
                true,
                ids.iter().map(|id| ItemId::new(id).unwrap()).collect(),
                ResolvedEvidenceReferenceV2::resolved(
                    EvidenceReferenceSnapshotV2::new(
                        GraphNodeId::new(*source_node).unwrap(),
                        attempt_id.clone(),
                        AttemptNumberV2::FIRST,
                        memory.recorded_items_digest().unwrap(),
                        UnixMillis::new(1_700_000_000_010),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        );
        trace_attempts.push(
            SessionAttemptV2::new(
                attempt_id.clone(),
                GraphNodeId::new(*source_node).unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(source_index as u64 + 1),
                AttemptLifecycle::Completed,
                AttemptValidityV2::Valid,
                None,
            )
            .unwrap(),
        );
        counters.push(GraphNodeCounterV2::new(
            GraphNodeId::new(*source_node).unwrap(),
            1,
            0,
        ));
        metadata.push(
            AttemptMetadataV2::new(
                attempt_id.clone(),
                created_at,
                Some(UnixMillis::new(1_700_000_000_010)),
                None,
            )
            .unwrap(),
        );
        attempt_ids.push(attempt_id);
        memories.push(memory);
    }
    memories.push(
        AttemptWorkflowMemoryV2::new(
            consumer_attempt_id.clone(),
            Vec::new(),
            Vec::new(),
            references,
        )
        .unwrap(),
    );
    trace_attempts.push(
        SessionAttemptV2::new(
            consumer_attempt_id.clone(),
            GraphNodeId::new("consume").unwrap(),
            AttemptNumberV2::FIRST,
            TraceSequenceV2::new(WIDE_SELECTION_SOURCES.len() as u64 + 1),
            AttemptLifecycle::Active,
            AttemptValidityV2::Valid,
            None,
        )
        .unwrap(),
    );
    counters.push(GraphNodeCounterV2::new(
        GraphNodeId::new("consume").unwrap(),
        1,
        0,
    ));
    metadata.push(
        AttemptMetadataV2::new(
            consumer_attempt_id,
            UnixMillis::new(1_700_000_000_010),
            None,
            None,
        )
        .unwrap(),
    );
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(WIDE_SELECTION_SOURCES.len() as u64 + 1),
        trace_attempts,
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(WIDE_SELECTION_SOURCES.len() as u64 + 1),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        counters,
        metadata,
        WorkflowMemoryStateV2::new(memories, Vec::new(), Vec::new()).unwrap(),
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

fn maximum_selected_readback_source() -> Vec<u8> {
    let choice_value = "\0".repeat(120);
    let media_type = format!("{}/{}", "a".repeat(127), "b".repeat(127));
    let mut items = vec![
        json!({"id": "confirm", "type": "confirm", "prompt": "Confirm.", "required": true}),
        json!({"id": "text", "type": "text", "prompt": "Text.", "required": true, "max_length": 16_384}),
        json!({"id": "choice", "type": "choice", "prompt": "Choose.", "required": true, "choices": [choice_value]}),
        json!({"id": "integer", "type": "integer", "prompt": "Count.", "required": true}),
        json!({"id": "list", "type": "list", "prompt": "List.", "required": true, "max_items": 200, "max_item_length": 308, "unique": false}),
        json!({"id": "artifact", "type": "artifact", "prompt": "Artifact.", "required": true, "allowed_media_types": [media_type]}),
    ];
    items.extend((0..10).map(|index| {
        json!({
            "id": format!("selected-{index:02}"),
            "type": "confirm",
            "prompt": "Confirm selected evidence.",
            "required": true
        })
    }));
    items.push(json!({
        "id": "unselected",
        "type": "text",
        "prompt": "This value must remain outside read-back.",
        "required": true,
        "max_length": 16_384
    }));
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "maximum-selected-readback",
        "version": "2",
        "name": "Maximum selected read-back",
        "purpose": "Exercise selectors and admitted read-back value bounds.",
        "node_definitions": {
            "source": {
                "type": "action",
                "title": "Record source",
                "intent": "Record bounded values.",
                "items": items
            },
            "consume": {
                "type": "action",
                "title": "Consume source",
                "intent": "Read selected bounded values."
            }
        },
        "graph": {
            "entry": "source",
            "nodes": [
                {"id": "source", "use": "source", "next": "consume"},
                {
                    "id": "consume",
                    "use": "consume",
                    "evidence_from": [{
                        "node": "source",
                        "required": true,
                        "items": SELECTED_READBACK_ITEM_IDS
                    }],
                    "terminal": true
                }
            ]
        }
    }))
    .unwrap()
}

fn maximum_selected_readback_state(source: &[u8]) -> GraphSessionStateV2 {
    let base = fresh_state("maximum-selected-readback.json", source);
    let source_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let consumer_attempt_id = second_attempt_id();
    let source_memory = base.workflow_memory().attempts().first().unwrap();
    let values = source_memory
        .item_slots()
        .iter()
        .map(|slot| {
            let value = match slot.item_id().as_str() {
                "confirm" | "selected-00" | "selected-01" | "selected-02" | "selected-03"
                | "selected-04" | "selected-05" | "selected-06" | "selected-07" | "selected-08"
                | "selected-09" => RecordedItemValueV2::confirm(),
                "text" => RecordedItemValueV2::text("\0".repeat(16_384)).unwrap(),
                "choice" => RecordedItemValueV2::choice("\0".repeat(120)).unwrap(),
                "integer" => RecordedItemValueV2::integer(i64::MAX),
                "list" => RecordedItemValueV2::list(vec!["\0".repeat(308); 200]).unwrap(),
                "artifact" => RecordedItemValueV2::artifact(
                    ArtifactValueV1::external_reference(
                        "\\".repeat(4_000),
                        Sha256Digest::new(format!("sha256:{}", "f".repeat(64))).unwrap(),
                        u64::MAX,
                        format!("{}/{}", "a".repeat(127), "b".repeat(127)),
                    )
                    .unwrap(),
                ),
                "unselected" => RecordedItemValueV2::text("unselected-secret").unwrap(),
                other => panic!("unexpected read-back fixture item {other}"),
            };
            ItemSlotStateV2::new(
                source_attempt_id.clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(value),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let source_memory =
        AttemptWorkflowMemoryV2::new(source_attempt_id.clone(), values, Vec::new(), Vec::new())
            .unwrap();
    let source_digest = source_memory.recorded_items_digest().unwrap();
    let consumer_memory = AttemptWorkflowMemoryV2::new(
        consumer_attempt_id.clone(),
        Vec::new(),
        Vec::new(),
        vec![
            EvidenceResolutionStateV2::new(
                0,
                true,
                SELECTED_READBACK_ITEM_IDS
                    .iter()
                    .map(|id| ItemId::new(*id).unwrap())
                    .collect(),
                ResolvedEvidenceReferenceV2::resolved(
                    EvidenceReferenceSnapshotV2::new(
                        GraphNodeId::new("source").unwrap(),
                        source_attempt_id.clone(),
                        AttemptNumberV2::FIRST,
                        source_digest,
                        UnixMillis::new(1_700_000_000_010),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    source_and_consumer_state(&base, source_memory, consumer_memory)
}

fn check_result_readback_source() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "check-result-readback",
        "version": "1",
        "name": "Check result read-back",
        "purpose": "Read one complete structurally bound external result.",
        "node_definitions": {
            "source": {
                "type": "action",
                "title": "Record result",
                "intent": "Record the caller-supplied result.",
                "items": [{
                    "id": "verification",
                    "type": "check_result",
                    "prompt": "Record verification.",
                    "required": true,
                    "operation_id": "make-test",
                    "operation_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "accepted_outcomes": ["pass"]
                }]
            },
            "consume": {
                "type": "action",
                "title": "Consume result",
                "intent": "Read the selected result."
            }
        },
        "graph": {
            "entry": "source",
            "nodes": [
                {"id": "source", "use": "source", "next": "consume"},
                {
                    "id": "consume",
                    "use": "consume",
                    "evidence_from": [{"node": "source", "required": true, "items": ["verification"]}],
                    "terminal": true
                }
            ]
        }
    }))
    .unwrap()
}

fn check_result_readback_state(source: &[u8]) -> GraphSessionStateV2 {
    check_result_readback_state_with_value(source, recorded_check_result())
}

fn check_result_readback_state_with_value(
    source: &[u8],
    value: RecordedItemValueV2,
) -> GraphSessionStateV2 {
    let base = fresh_state("check-result-readback.json", source);
    let source_attempt_id = AttemptId::new(ATTEMPT_ID).unwrap();
    let consumer_attempt_id = second_attempt_id();
    let original = &base.workflow_memory().attempts()[0].item_slots()[0];
    let slot = ItemSlotStateV2::new(
        source_attempt_id.clone(),
        original.item_id().clone(),
        original.item_type(),
        Revision::new(1),
        Some(value),
        original.created_at(),
        UnixMillis::new(original.created_at().get() + 1),
    )
    .unwrap();
    let source_memory = AttemptWorkflowMemoryV2::new(
        source_attempt_id.clone(),
        vec![slot],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let source_digest = source_memory.recorded_items_digest().unwrap();
    let consumer_memory = AttemptWorkflowMemoryV2::new(
        consumer_attempt_id,
        Vec::new(),
        Vec::new(),
        vec![
            EvidenceResolutionStateV2::new(
                0,
                true,
                vec![ItemId::new("verification").unwrap()],
                ResolvedEvidenceReferenceV2::resolved(
                    EvidenceReferenceSnapshotV2::new(
                        GraphNodeId::new("source").unwrap(),
                        source_attempt_id,
                        AttemptNumberV2::FIRST,
                        source_digest,
                        UnixMillis::new(1_700_000_000_010),
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    source_and_consumer_state(&base, source_memory, consumer_memory)
}

fn recorded_check_result_with_outcome(outcome: CheckResultOutcomeV2) -> RecordedItemValueV2 {
    RecordedItemValueV2::check_result(
        CheckResultValueV2::new(
            OperationId::new("make-test").unwrap(),
            Sha256Digest::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            CheckResultInputBasisV2::new(
                "HEAD and dirty-tree snapshot",
                Sha256Digest::new(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .unwrap(),
            )
            .unwrap(),
            CheckResultExecutorV2::new("gaori", "1.0.0").unwrap(),
            outcome,
            "The external operation reported pass.",
            Sha256Digest::new(
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )
            .unwrap(),
        )
        .unwrap(),
    )
}

fn recorded_check_result() -> RecordedItemValueV2 {
    recorded_check_result_with_outcome(CheckResultOutcomeV2::Pass)
}

fn maximal_recorded_check_result() -> RecordedItemValueV2 {
    RecordedItemValueV2::check_result(
        CheckResultValueV2::new(
            OperationId::new("make-test").unwrap(),
            Sha256Digest::new(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            CheckResultInputBasisV2::new(
                "\u{10ffff}".repeat(512),
                Sha256Digest::new(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .unwrap(),
            )
            .unwrap(),
            CheckResultExecutorV2::new("\u{10ffff}".repeat(128), "\u{10ffff}".repeat(64)).unwrap(),
            CheckResultOutcomeV2::Pass,
            "\u{10ffff}".repeat(2_000),
            Sha256Digest::new(
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )
            .unwrap(),
        )
        .unwrap(),
    )
}

fn guidance_state_with_required_check_result(outcome: CheckResultOutcomeV2) -> GraphSessionStateV2 {
    let base = fresh_state(
        "check-result-guidance.json",
        CHECK_RESULT_GUIDANCE_PROCEDURE,
    );
    let original_attempt = &base.workflow_memory().attempts()[0];
    let slots = original_attempt
        .item_slots()
        .iter()
        .map(|original| {
            if original.item_id().as_str() != "required-check" {
                return original.clone();
            }
            ItemSlotStateV2::new(
                original_attempt.attempt_id().clone(),
                original.item_id().clone(),
                original.item_type(),
                Revision::new(1),
                Some(recorded_check_result_with_outcome(outcome)),
                original.created_at(),
                UnixMillis::new(original.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect();
    let memory = AttemptWorkflowMemoryV2::new(
        original_attempt.attempt_id().clone(),
        slots,
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        base.workspace_revision(),
        base.task_title(),
        base.snapshot().clone(),
        base.trace().clone(),
        base.counters().to_vec(),
        base.attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![memory], Vec::new(), Vec::new()).unwrap(),
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

fn current_recorded_check_result_state(source: &[u8]) -> GraphSessionStateV2 {
    let base = fresh_state("check-result-readback.json", source);
    let original_attempt = &base.workflow_memory().attempts()[0];
    let original = &original_attempt.item_slots()[0];
    let slot = ItemSlotStateV2::new(
        original_attempt.attempt_id().clone(),
        original.item_id().clone(),
        original.item_type(),
        Revision::new(1),
        Some(recorded_check_result()),
        original.created_at(),
        UnixMillis::new(original.created_at().get() + 1),
    )
    .unwrap();
    let memory = AttemptWorkflowMemoryV2::new(
        original_attempt.attempt_id().clone(),
        vec![slot],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(1),
        base.task_title(),
        base.snapshot().clone(),
        base.trace().clone(),
        base.counters().to_vec(),
        base.attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![memory], Vec::new(), Vec::new()).unwrap(),
        base.goal_state().clone(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

#[test]
fn v2ast005_status_and_observation_use_only_the_fixed_check_result_projection() {
    let source = check_result_readback_source();
    let session_view = view(current_recorded_check_result_state(&source));
    let status = project_graph_status_v2(&session_view, GraphStatusTierV2::Standard, None).unwrap();
    let status_value = &status["item_values"][0]["value"];
    assert_eq!(status_value["operation_id"], "make-test");
    assert_eq!(status_value["outcome"], "pass");
    assert!(status_value.get("descriptor").is_none());
    assert!(status_value.get("executor").is_none());
    assert!(status_value.get("summary").is_none());
    assert_eq!(status["item_values"][0]["value_truncated"], false);

    let observation = project_graph_observation_v1(&session_view).unwrap();
    assert_eq!(observation["active_items"][0]["value"], *status_value);
    assert_eq!(observation["active_items"][0]["value_truncated"], false);
    assert!(
        !observation["guidance"]["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("item.record_many"))
    );
    assert!(
        observation["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|template| template.get("stdin_template").is_none())
    );
    assert_output_v2("session.status", status);
    assert_output_v2("session.observe", observation);
}

#[test]
fn v2ast005_check_result_preview_and_readback_separate_projection_from_complete_value() {
    let source = check_result_readback_source();
    let session_view = view(check_result_readback_state(&source));
    let next = project_graph_next_v2(&session_view).unwrap();
    let metadata = &next["readback"][0]["items"][0];
    assert_eq!(metadata["type"], "check_result");
    assert_eq!(metadata["size_unit"], "bytes");
    assert_eq!(metadata["preview_truncated"], false);
    assert!(metadata.get("next_page_token").is_none());
    assert_eq!(
        metadata["preview"],
        json!({
            "operation_id": "make-test",
            "outcome": "pass",
            "operation_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "input_basis": {"digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
            "output_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        })
    );
    for omitted in ["descriptor", "executor", "summary"] {
        assert!(!metadata["preview"].to_string().contains(omitted));
    }

    let read = project_evidence_read_v1(
        &session_view,
        &GraphNodeId::new("source").unwrap(),
        &ItemId::new("verification").unwrap(),
        None,
    )
    .unwrap();
    let complete = &read["page"]["data"];
    assert_eq!(
        complete["input_basis"]["descriptor"],
        "HEAD and dirty-tree snapshot"
    );
    assert_eq!(
        complete["executor"],
        json!({"name": "gaori", "version": "1.0.0"})
    );
    assert_eq!(complete["summary"], "The external operation reported pass.");
    let encoded = canonicalize_json_v1(complete).unwrap().len();
    assert_eq!(read["total_size"], encoded);
    assert_eq!(read["page"]["size"], encoded);
    assert_eq!(read["page"]["offset"], 0);
    assert_eq!(read["size_unit"], "bytes");
    assert_eq!(read["truncated"], false);
    assert_eq!(read["next_page_token"], Value::Null);
    assert_output_v2("session.next", next);
    assert_output_v2("evidence.read", read);
}

#[test]
fn v2ast006_maximal_check_result_readback_is_one_complete_bounded_page() {
    let source = check_result_readback_source();
    let value = maximal_recorded_check_result();
    let complete =
        canonicalize_json_v1(&serde_json::to_value(value.as_check_result().unwrap()).unwrap())
            .unwrap();
    assert!(complete.len() <= MAX_CHECK_RESULT_VALUE_BYTES_V2);

    let session_view = view(check_result_readback_state_with_value(&source, value));
    let read = project_evidence_read_v1(
        &session_view,
        &GraphNodeId::new("source").unwrap(),
        &ItemId::new("verification").unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(read["total_size"], complete.len());
    assert_eq!(read["page"]["size"], complete.len());
    assert_eq!(read["page"]["offset"], 0);
    assert_eq!(
        read["page"]["data"],
        serde_json::from_str::<Value>(&complete).unwrap()
    );
    assert_eq!(read["truncated"], false);
    assert_eq!(read["next_page_token"], Value::Null);

    let response = ResponseEnvelopeV2::OutputV2(output("evidence.read", read));
    let encoded = encode_response_payload_v2(&response).unwrap();
    assert!(encoded.len() <= MAX_EVIDENCE_RESPONSE_BYTES_V2);
    assert!(encoded.len() <= MAX_FRAME_PAYLOAD_BYTES_V1);
}

#[test]
fn v2rel002_selected_readback_respects_value_and_wire_bounds() {
    const FRAME_MAX: usize = 1_048_576;
    let source = maximum_selected_readback_source();
    let validated = validated_json_procedure(&source);
    let production_budget =
        procedure_placement_budget_v2(&validated, &GraphNodeId::new("consume").unwrap()).unwrap();
    let next = project_graph_next_v2(&view(maximum_selected_readback_state(&source))).unwrap();
    let readback = next["readback"].as_array().unwrap();
    assert_eq!(readback.len(), 1);
    let items = readback[0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 16);
    let ids = items
        .iter()
        .map(|item| item["item_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "confirm",
            "text",
            "choice",
            "integer",
            "list",
            "artifact",
            "selected-00",
            "selected-01",
            "selected-02",
            "selected-03",
            "selected-04",
            "selected-05",
            "selected-06",
            "selected-07",
            "selected-08",
            "selected-09",
        ]
    );
    assert!(
        !serde_json::to_string(readback)
            .unwrap()
            .contains("unselected-secret")
    );

    let item = |id: &str| {
        items
            .iter()
            .find(|item| item["item_id"] == id)
            .unwrap_or_else(|| panic!("selected item {id} is absent"))
    };
    // V2SCL-004 replaced the complete value with identity, digest, and size. Every item still
    // reports its exact logical size, so a caller knows what a page read will yield before asking.
    for id in [
        "confirm",
        "text",
        "choice",
        "integer",
        "list",
        "artifact",
        "selected-00",
    ] {
        let metadata = item(id);
        assert!(metadata.get("value").is_none(), "{id} must carry no value");
        assert!(
            metadata["value_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(metadata["revision"].is_number());
    }
    assert_eq!(item("confirm")["total_size"], 1);
    assert_eq!(item("confirm")["size_unit"], "value");
    assert_eq!(item("text")["total_size"], 16_384);
    assert_eq!(item("text")["size_unit"], "scalars");
    assert_eq!(item("choice")["total_size"], 1);
    assert_eq!(item("integer")["total_size"], 1);
    assert_eq!(item("list")["total_size"], 200);
    assert_eq!(item("list")["size_unit"], "entries");
    assert_eq!(item("artifact")["total_size"], 1);

    // The reference names its items explicitly, so the first 32 receive a bounded preview and a
    // truncated preview carries the token that resumes exactly where it stopped.
    let text_preview = item("text")["preview"].as_str().unwrap();
    assert_eq!(text_preview.chars().count(), 938);
    assert_eq!(item("text")["preview_truncated"], true);
    assert!(
        item("text")["next_page_token"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 256
    );
    assert_eq!(item("confirm")["preview_truncated"], false);
    assert!(item("confirm").get("next_page_token").is_none());

    let readback_component = json!({
        "references": next["references"].clone(),
        "readback": readback,
    });
    let encoded_readback = serde_json::to_vec(&readback_component).unwrap();
    // The authoring charge is only useful if it is a real upper bound on what the runtime emits.
    // V2SCL-005 dropped the old tightness claim, which measured a charge that carried complete
    // values; what has to hold now is that the metadata charge still covers the metadata, that
    // the preview and token allocations cover what previews actually emit, and that each stays
    // inside its published ceiling.
    let previews: usize = readback
        .iter()
        .flat_map(|reference| reference["items"].as_array().unwrap().iter())
        .filter_map(|item| item.get("preview"))
        .map(|preview| serde_json::to_vec(preview).unwrap().len())
        .sum();
    let tokens: usize = readback
        .iter()
        .flat_map(|reference| reference["items"].as_array().unwrap().iter())
        .filter_map(|item| item.get("next_page_token"))
        .map(|token| serde_json::to_vec(token).unwrap().len())
        .sum();
    assert!(
        u64::try_from(encoded_readback.len() - previews - tokens).unwrap()
            <= production_budget.readback(),
        "metadata charge under-counted the serialized projection: encoded={}, charged={}",
        encoded_readback.len() - previews - tokens,
        production_budget.readback(),
    );
    assert!(u64::try_from(previews).unwrap() <= production_budget.evidence_preview());
    assert!(u64::try_from(tokens).unwrap() <= production_budget.page_tokens());
    assert!(production_budget.readback() <= podway_config::READBACK_BUDGET);
    assert!(production_budget.evidence_preview() <= podway_config::EVIDENCE_PREVIEW_BUDGET);
    assert!(production_budget.page_tokens() <= podway_config::PAGE_TOKEN_BUDGET);

    let response = ResponseEnvelopeV2::OutputV2(output("session.next", next.clone()));
    let encoded = encode_response_payload_v2(&response).unwrap();
    assert!(encoded.len() <= FRAME_MAX);
    validate_frame_payload_length(encoded.len()).unwrap();
    assert_eq!(decode_response_payload_v2(&encoded).unwrap(), response);
    let frame = encode_frame_v1(&encoded).unwrap();
    assert_eq!(decode_single_frame_v1(&frame).unwrap(), encoded);
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
    let bounded_observation = project_graph_observation_v1(&bounded_view).unwrap();
    assert_eq!(bounded["blockers_truncated"], true);
    assert!(bounded.get("items_truncated").is_some());
    assert_eq!(bounded["item_values"][0]["value_truncated"], true);
    assert!(
        serde_json::to_vec(&bounded["blocker_window"])
            .unwrap()
            .len()
            <= 49_152
    );
    assert_eq!(
        bounded_observation["active_items"][0]["value_truncated"],
        true
    );
    assert!(
        serde_json::to_vec(&bounded_observation["active_items"][0]["value"])
            .unwrap()
            .len()
            <= 1_024
    );

    for (command, result) in [
        ("session.status", compact),
        ("session.status", verbose),
        ("session.status", bounded),
        ("session.next", wide_next),
        ("session.next", bounded_next),
        ("session.observe", bounded_observation),
    ] {
        let envelope = output(command, result);
        let encoded = serde_json::to_vec(&envelope).unwrap();
        assert!(encoded.len() <= 1_048_576);
        validate_frame_payload_length(encoded.len()).unwrap();
        let decoded: OutputEnvelopeV3 = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, envelope);
    }
}

fn maximum_satisfied_confirm_state() -> GraphSessionStateV2 {
    let items = (0..64)
        .map(|index| {
            json!({
                "id": format!("confirm-{index:02}"),
                "type": "confirm",
                "prompt": "Confirm the bounded result.",
                "required": true
            })
        })
        .collect::<Vec<_>>();
    let source = serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "maximum-satisfied-confirm-observation",
        "version": "2",
        "name": "Maximum satisfied confirm observation",
        "purpose": "Exercise the maximum dense observation mutation-template set.",
        "node_definitions": {
            "finish": {
                "type": "action",
                "title": "Confirm every result",
                "intent": "Record the bounded confirmations.",
                "items": items
            }
        },
        "graph": {
            "entry": "finish",
            "nodes": [{"id": "finish", "use": "finish", "terminal": true}]
        },
        "manual_rework": {"allowed_targets": ["finish"]}
    }))
    .unwrap();
    let base = fresh_state("maximum-satisfied-confirm-observation.json", &source);
    let active = base.workflow_memory().attempts().first().unwrap();
    let slots = active
        .item_slots()
        .iter()
        .map(|slot| {
            ItemSlotStateV2::new(
                slot.attempt_id().clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(RecordedItemValueV2::confirm()),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect();
    let memory =
        AttemptWorkflowMemoryV2::new(active.attempt_id().clone(), slots, Vec::new(), Vec::new())
            .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        base.workspace_revision(),
        base.task_title(),
        base.snapshot().clone(),
        base.trace().clone(),
        base.counters().to_vec(),
        base.attempt_metadata().to_vec(),
        WorkflowMemoryStateV2::new(vec![memory], Vec::new(), Vec::new()).unwrap(),
        base.goal_state().clone(),
        base.created_at(),
        base.completed_at(),
        base.cancelled_at(),
        base.cancel_reason().map(str::to_owned),
    )
    .unwrap()
}

#[test]
fn v2agt007_dense_observation_templates_fit_the_public_schema_and_frame() {
    let observation = project_graph_observation_v1(&view(maximum_satisfied_confirm_state()))
        .expect("maximum dense observation must project");
    let templates = observation["mutation_templates"].as_array().unwrap();
    let templates_total = observation["mutation_templates_total"].as_u64().unwrap() as usize;
    // V2SCL-005 bounds this window by bytes, so a dense fixture now cuts. The exact total still
    // exercises the former under-sized schema limit, and the flag reports the cut.
    assert!(
        templates_total > 192,
        "the fixture must exercise the former under-sized schema limit"
    );
    assert_eq!(
        observation["mutation_templates_truncated"],
        json!(templates.len() < templates_total)
    );
    let schema: Value = serde_json::from_slice(include_bytes!(
        "../../../assets/schemas/observation-result-v1.schema.json"
    ))
    .unwrap();
    let schema_maximum = schema["properties"]["mutation_templates"]["maxItems"]
        .as_u64()
        .unwrap() as usize;
    assert!(
        templates.len() <= schema_maximum,
        "{} templates exceed the public schema maximum {schema_maximum}",
        templates.len()
    );
    for action in observation["guidance"]["allowed_actions"]
        .as_array()
        .unwrap()
    {
        assert!(
            templates
                .iter()
                .any(|template| template["command"] == *action),
            "dense observation omitted a template for {action}"
        );
    }
    assert_output_v2("session.observe", observation);
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
        let observation = project_graph_observation_v1(&view).unwrap();
        assert!(observation["guidance"].is_null());
        assert_eq!(observation["active_items"], json!([]));
        let templates = observation["mutation_templates"].as_array().unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["command"], "session.terminal_disposition");
        assert!(templates.iter().all(|template| {
            template["preconditions"]["session_revision"]
                == observation["status"]["session"]["revision"]
        }));
        assert_output_v2("session.observe", observation);

        let disposed_view =
            view_with_terminal_disposition(decided_assessment_state(lifecycle, false), true);
        let disposed = project_graph_observation_v1(&disposed_view).unwrap();
        let templates = disposed["mutation_templates"].as_array().unwrap();
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0]["command"], "session.reset");
        assert_eq!(templates[1]["command"], "session.start_replace");
        assert_output_v2("session.observe", disposed);
    }
}

#[test]
fn v2lif005_prepared_observation_exposes_only_fenced_eligible_actions() {
    let observation = project_graph_observation_v1(&view(prepared_state())).unwrap();
    assert_eq!(observation["status"]["session"]["lifecycle"], "prepared");
    assert!(observation["status"]["current"].is_null());
    assert_eq!(observation["active_items"], json!([]));
    let templates = observation["mutation_templates"].as_array().unwrap();
    assert_eq!(templates.len(), 3);
    assert_eq!(templates[0]["command"], "session.begin");
    assert_eq!(templates[1]["command"], "session.reset");
    assert_eq!(templates[2]["command"], "session.start_replace");
    assert!(templates.iter().all(|template| {
        template["preconditions"]["session_revision"] == 0
            && template["preconditions"]["workspace_uuid"] == WORKSPACE_ID
            && template["preconditions"]["session_id"] == SESSION_ID
    }));
    assert_output_v2("session.observe", observation);
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
    let verbose = project_graph_status_v2(&view, GraphStatusTierV2::Verbose, None).unwrap();
    let history_decision = &verbose["decision_history"]["entries"][0];
    assert!(history_decision.get("assessment").is_none());
    assert!(history_decision.get("criterion_results").is_none());
    assert_output_v2("session.next", next);
    assert_output_v2("session.status", verbose);
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
    assert_eq!(
        next["allowed_actions"],
        json!([
            "item.set",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "goal.assess_criterion"
        ])
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
    assert_eq!(next["options"].as_array().unwrap().len(), 3);
    assert_eq!(next["options"][0]["option_id"], "achieved");
    assert_eq!(next["allowed_option_ids"], json!(["achieved"]));
    assert_eq!(
        next["allowed_actions"],
        json!([
            "item.set",
            "session.decide",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset"
        ])
    );
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

    assert_eq!(
        next["allowed_actions"],
        json!([
            "item.set",
            "session.skip",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.define"
        ])
    );
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

fn populate_active_items_for_applicability(state: &GraphSessionStateV2) -> GraphSessionStateV2 {
    let active = state.trace().active_attempt().unwrap();
    let replacement = state
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == active.attempt_id())
        .unwrap();
    let slots = replacement
        .item_slots()
        .iter()
        .map(|slot| {
            let value = match slot.item_id().as_str() {
                "confirm" => RecordedItemValueV2::confirm(),
                "text" | "basis" => RecordedItemValueV2::text("recorded").unwrap(),
                "choice" => RecordedItemValueV2::choice("yes").unwrap(),
                "integer" => RecordedItemValueV2::integer(1),
                "list" => RecordedItemValueV2::list(vec!["one".to_owned()]).unwrap(),
                "artifact" => RecordedItemValueV2::artifact(
                    ArtifactValueV1::external_reference(
                        "artifact.txt",
                        Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
                        8,
                        "text/plain",
                    )
                    .unwrap(),
                ),
                other => panic!("unexpected applicability item {other}"),
            };
            ItemSlotStateV2::new(
                slot.attempt_id().clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(value),
                slot.created_at(),
                UnixMillis::new(slot.created_at().get() + 1),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let replacement = AttemptWorkflowMemoryV2::new(
        replacement.attempt_id().clone(),
        slots,
        replacement.blockers().to_vec(),
        replacement.evidence().to_vec(),
    )
    .unwrap();
    let attempts = state
        .workflow_memory()
        .attempts()
        .iter()
        .map(|memory| {
            if memory.attempt_id() == active.attempt_id() {
                replacement.clone()
            } else {
                memory.clone()
            }
        })
        .collect();
    let workflow_memory = WorkflowMemoryStateV2::new(
        attempts,
        state.workflow_memory().decisions().to_vec(),
        state.workflow_memory().reworks().to_vec(),
    )
    .unwrap();
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

fn suggestion_commands(next: &Map<String, Value>) -> Vec<&str> {
    next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["command"].as_str().unwrap())
        .collect()
}

fn assert_next_applicability(
    state: GraphSessionStateV2,
    expected_actions: &[&str],
    expected_suggestions: &[&str],
) -> Map<String, Value> {
    let next = project_graph_next_v2(&view(state)).unwrap();
    assert_eq!(
        next["allowed_actions"],
        json!(expected_actions),
        "allowed action applicability drifted"
    );
    assert_eq!(
        suggestion_commands(&next),
        expected_suggestions,
        "forward-progress suggestion applicability drifted"
    );
    next
}

#[test]
fn v2run002_exhaustive_reachable_applicability_classes_close_actions_and_suggestions() {
    let missing = fresh_state("applicability.json", APPLICABILITY_PROCEDURE);
    let missing_next = assert_next_applicability(
        missing.clone(),
        &[
            "item.check",
            "item.set",
            "item.add",
            "item.attach",
            "session.skip",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.define",
        ],
        &[
            "item.check",
            "item.set",
            "item.set",
            "item.set",
            "item.add",
            "item.attach",
            "session.retry",
            "session.skip",
            "goal.define",
        ],
    );
    let missing_item_commands = missing_next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion.get("item_id").is_some())
        .map(|suggestion| {
            (
                suggestion["item_id"].as_str().unwrap(),
                suggestion["command"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        missing_item_commands,
        vec![
            ("confirm", "item.check"),
            ("text", "item.set"),
            ("choice", "item.set"),
            ("integer", "item.set"),
            ("list", "item.add"),
            ("artifact", "item.attach"),
        ]
    );

    let defined = missing
        .bind_initial_goal_at_start_v2(
            GoalStatementV2::new("Prove projector closure.").unwrap(),
            assessment_goal_definition(),
            None,
            UnixMillis::new(1_700_000_000_001),
        )
        .unwrap()
        .into_state();
    let ready = populate_active_items_for_applicability(&defined);
    let ready_next = assert_next_applicability(
        ready.clone(),
        &[
            "item.check",
            "item.uncheck",
            "item.clear",
            "item.set",
            "item.add",
            "item.remove",
            "item.attach",
            "session.complete",
            "session.skip",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.revise",
        ],
        &["session.complete", "session.retry", "session.skip"],
    );

    let active_id = ready.trace().active_attempt().unwrap().attempt_id().clone();
    let blocked = ready
        .block_active_attempt_v2(
            ready.trace().revision(),
            &active_id,
            BlockerId::new("00000000-0000-4000-8000-000000002099").unwrap(),
            "Blocked for the applicability class.",
            UnixMillis::new(1_700_000_000_002),
        )
        .unwrap()
        .into_state();
    let blocked_next = assert_next_applicability(
        blocked,
        &[
            "item.check",
            "item.uncheck",
            "item.clear",
            "item.set",
            "item.add",
            "item.remove",
            "item.attach",
            "session.skip",
            "session.retry",
            "session.block",
            "session.unblock",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.revise",
        ],
        &["session.retry", "session.skip"],
    );

    let decision = ready
        .complete_active_action_v2(
            ready.trace().revision(),
            &active_id,
            Some(second_attempt_id()),
            UnixMillis::new(1_700_000_000_003),
        )
        .unwrap()
        .into_state();
    let decision_missing_next = assert_next_applicability(
        decision.clone(),
        &[
            "item.set",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.revise",
        ],
        &["item.set", "session.retry"],
    );
    let decision_ready = populate_active_items_for_applicability(&decision);
    let decision_next = assert_next_applicability(
        decision_ready.clone(),
        &[
            "item.set",
            "item.clear",
            "session.decide",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.revise",
        ],
        &["session.decide", "session.decide", "session.retry"],
    );
    let decide_options = decision_next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "session.decide")
        .map(|suggestion| suggestion["argv"][3].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(decide_options, vec!["left", "right"]);

    let decision_attempt_id = decision_ready
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let blocked_decision = decision_ready
        .block_active_attempt_v2(
            decision_ready.trace().revision(),
            &decision_attempt_id,
            BlockerId::new("00000000-0000-4000-8000-000000002097").unwrap(),
            "Block the decision applicability class.",
            UnixMillis::new(1_700_000_000_004),
        )
        .unwrap()
        .into_state();
    let blocked_decision_next = assert_next_applicability(
        blocked_decision,
        &[
            "item.set",
            "item.clear",
            "session.retry",
            "session.block",
            "session.unblock",
            "session.cancel",
            "session.reset",
            "session.rework",
            "goal.revise",
        ],
        &["session.retry"],
    );

    let partial = project_graph_next_v2(&view(goal_assessment_state(vec![assessment_result(
        "correct",
    )])))
    .unwrap();
    let unassessed = partial["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "goal.assess_criterion")
        .map(|suggestion| suggestion["argv"][3].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(unassessed, vec!["tested"]);
    assert!(
        !partial["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("session.decide"))
    );

    let complete = project_graph_next_v2(&view(goal_assessment_state(vec![
        assessment_result("correct"),
        assessment_result("tested"),
    ])))
    .unwrap();
    let complete_decisions = complete["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "session.decide")
        .collect::<Vec<_>>();
    assert_eq!(complete_decisions.len(), 1);
    assert_eq!(complete_decisions[0]["argv"][3], "achieved");
    assert!(
        !complete["allowed_actions"]
            .as_array()
            .unwrap()
            .contains(&json!("goal.assess_criterion"))
    );

    let terminal_ready = assert_next_applicability(
        decided_assessment_state(DecidedFixtureLifecycle::Running, false),
        &[
            "session.complete",
            "session.retry",
            "session.block",
            "session.cancel",
            "session.reset",
        ],
        &["session.complete", "session.retry"],
    );

    let mut reactivation_source: Value = serde_json::from_slice(ASSESSMENT_PROCEDURE).unwrap();
    reactivation_source.as_object_mut().unwrap().insert(
        "manual_rework".to_owned(),
        json!({"allowed_targets":["assess-goal"]}),
    );
    let reactivation_source = serde_json::to_vec(&reactivation_source).unwrap();
    let completed_for_reactivation = decided_assessment_state_from_source(
        DecidedFixtureLifecycle::Completed,
        &reactivation_source,
    );
    let reactivated = completed_for_reactivation
        .manual_rework_v2(
            completed_for_reactivation.trace().revision(),
            None,
            GraphNodeId::new("assess-goal").unwrap(),
            AttemptId::new("00000000-0000-4000-8000-000000002098").unwrap(),
            ReasonV2::new("Reassess the completed session.").unwrap(),
            None,
            UnixMillis::new(1_700_000_000_030),
        )
        .unwrap()
        .into_state();
    assert_eq!(reactivated.trace().lifecycle(), SessionLifecycle::Running);
    let reactivated_next = project_graph_next_v2(&view(reactivated)).unwrap();
    assert_eq!(
        reactivated_next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|suggestion| suggestion["command"] == "goal.assess_criterion")
            .count(),
        2
    );

    for lifecycle in [
        DecidedFixtureLifecycle::Completed,
        DecidedFixtureLifecycle::Cancelled,
    ] {
        assert!(project_graph_next_v2(&view(decided_assessment_state(lifecycle, false))).is_err());
    }

    let check_result_next = project_graph_next_v2(&view(fresh_state(
        "check-result-guidance.json",
        CHECK_RESULT_GUIDANCE_PROCEDURE,
    )))
    .unwrap();

    let observed_actions = [
        &missing_next,
        &ready_next,
        &blocked_next,
        &decision_missing_next,
        &decision_next,
        &blocked_decision_next,
        &partial,
        &complete,
        &terminal_ready,
        &reactivated_next,
        &check_result_next,
    ]
    .into_iter()
    .flat_map(|next| next["allowed_actions"].as_array().unwrap())
    .filter_map(Value::as_str)
    .collect::<std::collections::BTreeSet<_>>();
    let expected_projector_actions = [
        "goal.assess_criterion",
        "goal.define",
        "goal.revise",
        "item.add",
        "item.attach",
        "item.check",
        "item.clear",
        "item.remove",
        "item.record_many",
        "item.set",
        "item.uncheck",
        "session.block",
        "session.cancel",
        "session.complete",
        "session.decide",
        "session.reset",
        "session.retry",
        "session.rework",
        "session.skip",
        "session.unblock",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(observed_actions, expected_projector_actions);
    let routes: Value =
        serde_json::from_slice(include_bytes!("../../../contracts/command-routes.json")).unwrap();
    let registered = routes["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|route| route["command"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(observed_actions.is_subset(&registered));
    for command in expected_projector_actions {
        assert!(
            registered.contains(command),
            "projector action route {command} is unregistered"
        );
    }
}

/// V2SCL-004: a selected evidence item is readable one bounded page at a time, and every rejected
/// read names its exact reason without changing state.
#[test]
fn v2scl004_evidence_read_pages_deterministically_and_fails_exactly() {
    let source = maximum_selected_readback_source();
    let state = maximum_selected_readback_state(&source);
    let session_view = view(state);
    let node = GraphNodeId::new("source").unwrap();
    let text = ItemId::new("text").unwrap();

    let first = project_evidence_read_v1(&session_view, &node, &text, None)
        .expect("the first page of a selected text item is readable");
    assert_eq!(first["schema"], "podway.evidence-read-result/v1");
    assert_eq!(first["item"]["item_id"], "text");
    assert_eq!(first["item"]["type"], "text");
    assert_eq!(first["size_unit"], "scalars");
    assert_eq!(first["total_size"], 16_384);
    assert_eq!(first["page"]["offset"], 0);
    assert_eq!(first["page_token_version"], 1);

    // 16,384 scalars fit inside one 256 KiB page, so this value is terminal in a single read and
    // the daemon issues no token a caller could resume from.
    assert_eq!(first["truncated"], false);
    assert_eq!(first["next_page_token"], Value::Null);
    assert_eq!(
        first["page"]["data"].as_str().unwrap().chars().count(),
        16_384
    );
    assert_eq!(first["page"]["size"], 16_384);

    // Reading is a pure query: the same request answers identically.
    let repeated = project_evidence_read_v1(&session_view, &node, &text, None).unwrap();
    assert_eq!(repeated, first);

    // A list item reports entries, not scalars. Its 200 entries of 308 scalars exceed one page, so
    // the read continues through a token and every page carries whole entries.
    let list = ItemId::new("list").unwrap();
    let first_list = project_evidence_read_v1(&session_view, &node, &list, None).unwrap();
    assert_eq!(first_list["size_unit"], "entries");
    assert_eq!(first_list["total_size"], 200);
    assert_eq!(first_list["truncated"], true);
    assert_eq!(first_list["page"]["offset"], 0);
    let first_entries = first_list["page"]["data"].as_array().unwrap();
    assert!(first_entries.iter().all(|entry| entry.is_string()));
    let first_size = first_entries.len();
    assert_eq!(first_list["page"]["size"], first_size);
    assert!(first_size > 0 && first_size < 200);

    let continuation = podway_protocol::EvidencePageTokenV1::decode(
        first_list["next_page_token"].as_str().unwrap(),
    )
    .expect("the continuation token decodes");
    assert_eq!(continuation.offset(), first_size as u64);
    let second_list =
        project_evidence_read_v1(&session_view, &node, &list, Some(&continuation)).unwrap();
    assert_eq!(second_list["page"]["offset"], first_size);
    assert_eq!(second_list["truncated"], false);
    assert_eq!(second_list["next_page_token"], Value::Null);
    let second_entries = second_list["page"]["data"].as_array().unwrap();
    assert_eq!(first_size + second_entries.len(), 200);

    // Concatenating the pages reproduces the value exactly: no entry is split, dropped, or repeated.
    let mut rejoined = first_entries.clone();
    rejoined.extend(second_entries.iter().cloned());
    assert_eq!(rejoined.len(), 200);
    assert!(
        rejoined
            .iter()
            .all(|entry| entry.as_str().unwrap().chars().count() == 308),
        "every rejoined entry must be a whole 308-scalar entry, never a split one"
    );

    // A token replays the same page while its snapshot is current.
    let replayed =
        project_evidence_read_v1(&session_view, &node, &list, Some(&continuation)).unwrap();
    assert_eq!(replayed, second_list);

    // An item the reference does not select is not readable, and neither is an undeclared source.
    let unselected = ItemId::new("unselected-secret").unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &node, &unselected, None),
        Err(EvidenceReadErrorV2::NotAvailable { .. })
    ));
    let undeclared = GraphNodeId::new("elsewhere").unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &undeclared, &text, None),
        Err(EvidenceReadErrorV2::NotAvailable { .. })
    ));

    // A token bound to another session, consumer, or item never reaches a state comparison.
    let digest = first["value_digest"].as_str().unwrap();
    let consumer = first["consumer"]["attempt_id"].as_str().unwrap();
    let source_attempt = first["source"]["attempt_id"].as_str().unwrap();
    let session_id = "00000000-0000-4000-8000-0000000000ff";
    let foreign = podway_protocol::EvidencePageTokenV1::new(
        session_id,
        consumer,
        source_attempt,
        "text",
        digest,
        0,
    )
    .unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &node, &text, Some(&foreign)),
        Err(EvidenceReadErrorV2::TokenNotBound)
    ));

    // A token whose bound value digest moved is a recoverable conflict, not a silent other page.
    let status = project_graph_status_v2(&session_view, GraphStatusTierV2::Standard, None).unwrap();
    let live_session = status["session"]["id"].as_str().unwrap().to_owned();
    let stale = podway_protocol::EvidencePageTokenV1::new(
        &live_session,
        consumer,
        source_attempt,
        "text",
        &format!("sha256:{}", "b".repeat(64)),
        0,
    )
    .unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &node, &text, Some(&stale)),
        Err(EvidenceReadErrorV2::TokenStale { .. })
    ));

    // An offset at or past the logical end is exhausted; the daemon never issues such a token.
    let exhausted = podway_protocol::EvidencePageTokenV1::new(
        &live_session,
        consumer,
        source_attempt,
        "text",
        digest,
        16_384,
    )
    .unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &node, &text, Some(&exhausted)),
        Err(EvidenceReadErrorV2::TokenExhausted { .. })
    ));
}

#[test]
fn v2scl005_the_widest_compact_status_fits_its_allocation() {
    // The 32 KiB status allocation is the tightest of the five, and its rationale was a comment
    // rather than a measurement. The widest a definition can make it is 128 items with maximal
    // identifiers on a graph with the most placements it may declare, so that is what is measured.
    let items: Vec<Value> = (0..128)
        .map(|index| {
            json!({
                "id": format!("{}{index:03}", "i".repeat(61)),
                "type": "artifact",
                "prompt": "Attach.",
                "required": true
            })
        })
        .collect();
    let source = serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "widest-compact-status",
        "version": "2",
        "name": "Widest compact status",
        "purpose": "Measure the widest compact status a definition can produce.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Record work",
                "intent": "Record items with maximal identifiers.",
                "items": items
            }
        },
        "graph": {
            "entry": format!("{}000", "n".repeat(61)),
            "nodes": (0..64)
                .map(|index| {
                    let id = format!("{}{index:03}", "n".repeat(61));
                    if index == 63 {
                        json!({"id": id, "use": "work", "terminal": true})
                    } else {
                        json!({
                            "id": id,
                            "use": "work",
                            "next": format!("{}{:03}", "n".repeat(61), index + 1)
                        })
                    }
                })
                .collect::<Vec<_>>()
        }
    }))
    .unwrap();
    // Counters are the other half of the widest case: a graph declares at most 64 placements and
    // their identifiers reach 64 scalars. Their counts cannot be made adversarially large — state
    // validation ties every counter to the trace, so the schema's u64 ceiling is not independently
    // reachable — but the identifier width and the placement count are, and both are present here.
    let session_view = view(wide_graph_state(&source, &format!("{}000", "n".repeat(61))));

    let compact = project_graph_status_v2(&session_view, GraphStatusTierV2::Compact, None).unwrap();
    assert_eq!(compact["items"].as_array().unwrap().len(), 128);
    assert_eq!(compact["counters"].as_array().unwrap().len(), 64);
    let charged = serde_json::to_vec(&compact).unwrap().len();
    // Pinned so a widening of the item or placement ceiling shows up here as a number rather than
    // as an observation that suddenly cannot be answered.
    assert_eq!(charged, 27_628);
    assert!(
        charged <= podway_core::MAX_OBSERVATION_STATUS_BYTES_V2,
        "the widest compact status charged {charged} bytes against {}",
        podway_core::MAX_OBSERVATION_STATUS_BYTES_V2
    );

    // The same member inside an observation must fit the same allocation.
    let observation = project_graph_observation_v1(&session_view).unwrap();
    let embedded = serde_json::to_vec(&observation["status"]).unwrap().len();
    assert!(
        embedded <= podway_core::MAX_OBSERVATION_STATUS_BYTES_V2,
        "the embedded status charged {embedded} bytes"
    );
    assert_output_v2("session.observe", observation);
}

#[test]
fn v2scl005_observation_windows_are_bounded_by_bytes_and_report_their_exact_total() {
    // 128 optional items with maximal prompts is the widest active-item window a vetted definition
    // can produce: the source projection budget refuses a wider document, and the static
    // allocation refuses the same items as required, because each one then also charges a
    // missing-item detail. The byte window is therefore sized to hold every admissible
    // definition, and this one reports a complete, exact total.
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
    let source = serde_json::to_vec(&json!({
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
    .unwrap();
    let session_view = view(wide_node_state(&source));

    let observation = project_graph_observation_v1(&session_view).unwrap();
    let active_items = observation["active_items"].as_array().unwrap();
    let total = observation["active_items_total"].as_u64().unwrap() as usize;
    assert_eq!(total, 128, "the exact count must survive the window");
    assert_eq!(
        active_items.len(),
        total,
        "the widest admissible definition must fit the active-item allocation"
    );
    assert_eq!(observation["active_items_truncated"], json!(false));
    // 128 items generate more templates than the template allocation holds, so this window does
    // cut. The exact total stays exact and the flag says so: that is the contract.
    let templates = observation["mutation_templates"].as_array().unwrap();
    let templates_total = observation["mutation_templates_total"].as_u64().unwrap() as usize;
    assert_eq!(templates_total, 133);
    assert!(
        templates.len() < templates_total,
        "the template window must cut at its allocation"
    );
    assert_eq!(observation["mutation_templates_truncated"], json!(true));
    let charged: usize = templates
        .iter()
        .map(|template| serde_json::to_vec(template).unwrap().len() + 8)
        .sum();
    assert!(
        charged <= 64 * 1_024,
        "the template window charged {charged} bytes"
    );

    // The admitted window must fit its allocation, and the response must fit the frame.
    let admitted: usize = active_items
        .iter()
        .map(|item| serde_json::to_vec(item).unwrap().len() + 8)
        .sum();
    assert!(
        admitted <= 128 * 1_024,
        "the active-item window charged {admitted} bytes"
    );
    let response = ResponseEnvelopeV2::OutputV2(output("session.observe", observation.clone()));
    let encoded = podway_protocol::encode_response_payload_v2(&response).unwrap();
    assert!(
        encoded.len() <= 1_048_576,
        "observation encoded to {} bytes",
        encoded.len()
    );

    // The compact status member is the tightest allocation of the five, so its worst case is
    // measured rather than described. Identifiers reach 64 scalars and a graph declares 64
    // placements, which is the widest status a definition can produce.
    let widest_status = serde_json::to_vec(&observation["status"]).unwrap().len();
    assert!(
        widest_status <= 32 * 1_024,
        "the widest compact status charged {widest_status} bytes against its 32 KiB allocation"
    );

    // Every observation member fits its own published allocation, which is what makes the
    // composition a proof rather than a description.
    for (member, allocation) in [
        ("status", 32 * 1_024),
        ("guidance", 736 * 1_024),
        ("active_items", 128 * 1_024),
        ("mutation_templates", 64 * 1_024),
    ] {
        let encoded = serde_json::to_vec(&observation[member]).unwrap().len();
        assert!(
            encoded <= allocation,
            "{member} charged {encoded} bytes against {allocation}"
        );
    }

    // 128 active items exceed the 64 the compact tiers used to publish, so validating the whole
    // observation against `observation-result/v3` is what proves the raised ceilings agree.
    assert_output_v2("session.observe", observation.clone());

    // Every template argv element stays inside its published scalar bound.
    for template in observation["mutation_templates"].as_array().unwrap() {
        for element in template["argv"].as_array().unwrap() {
            assert!(
                element.as_str().unwrap().chars().count()
                    <= podway_core::MAX_TEMPLATE_ARGV_SCALARS_V2,
                "template argv element exceeded its bound: {element}"
            );
        }
    }
}

#[test]
fn v2scl004_bounded_windows_report_their_exact_total_when_they_truncate() {
    // 70 required items exceed the 64-entry detail window, and 9 choices exceed the 8-choice
    // observation window. ADR-0024 forbids a silent cut, so each window must publish the exact
    // total and say it was truncated.
    let mut items: Vec<Value> = (0..70)
        .map(|index| {
            json!({
                "id": format!("required-{index:02}"),
                "type": "text",
                "prompt": format!("Record value {index}."),
                "required": true,
                "max_length": 64
            })
        })
        .collect();
    items.push(json!({
        "id": "wide-choice",
        "type": "choice",
        "prompt": "Choose one.",
        "required": true,
        "choices": (0..9).map(|index| format!("choice-{index}")).collect::<Vec<_>>()
    }));
    let source = serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "wide-node",
        "version": "2",
        "name": "Wide node",
        "purpose": "Exercise the bounded missing-item and choice windows.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Record work",
                "intent": "Record more items than one window can carry.",
                "items": items
            }
        },
        "graph": {
            "entry": "work",
            "nodes": [{"id": "work", "use": "work", "terminal": true}]
        }
    }))
    .unwrap();
    let session_view = view(wide_node_state(&source));

    let next = project_graph_next_v2(&session_view).unwrap();
    // The exact count stays exact; only the detail array is windowed.
    assert_eq!(next["missing_required_item_count"], 71);
    assert_eq!(next["missing_required_items"].as_array().unwrap().len(), 64);
    assert_eq!(next["missing_required_items_truncated"], true);

    // The standard status tier publishes the same missing set as identifiers. Its schema capped
    // that array at 64, so raising items per definition to 128 made it able to violate its own
    // contract. `status-result/v3` is released and served since v0.2.4, so the cap was raised to
    // what a definition can declare rather than the array being windowed: windowing it would have
    // needed a new required member, and a released family never grows one.
    let status = project_graph_status_v2(&session_view, GraphStatusTierV2::Standard, None).unwrap();
    assert_eq!(
        status["missing_required_item_ids"]
            .as_array()
            .unwrap()
            .len(),
        71,
        "the identifier array carries every missing item rather than being cut"
    );
    // The same count also travels inside `current`, whose shared definition capped it at 64. The
    // envelope validates the published schemas, so this is what proves the audit reached that
    // copy too rather than only the top-level field.
    assert_eq!(status["current"]["missing_required_item_count"], 71);
    assert_output_v2("session.status", status.clone());
    let compact = project_graph_status_v2(&session_view, GraphStatusTierV2::Compact, None).unwrap();
    assert_eq!(compact["current"]["missing_required_item_count"], 71);
    assert_output_v2("session.status", compact);

    // The same widening applies to recorded values. `item_values` is bounded by bytes rather than
    // by count, so 128 small values all reach the response, and `items_total` describes them; both
    // had ceilings a 128-item definition can exceed. Validating the envelope is what proves the
    // corrected ceilings agree with what the daemon emits.
    let recorded = view(wide_node_state_with_recorded_values(&source));
    let recorded_status =
        project_graph_status_v2(&recorded, GraphStatusTierV2::Standard, None).unwrap();
    assert_eq!(recorded_status["items_total"], 71);
    assert_eq!(
        recorded_status["item_values"].as_array().unwrap().len(),
        71,
        "every recorded value reaches the response, so its ceiling must hold them"
    );
    assert_eq!(recorded_status["items_truncated"], false);
    assert_output_v2("session.status", recorded_status);

    let observation = project_graph_observation_v1(&session_view).unwrap();
    let choice = observation["active_items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == "wide-choice")
        .expect("the choice item is active");
    assert_eq!(
        choice["constraints"]["choices"].as_array().unwrap().len(),
        8
    );
    assert_eq!(choice["constraints"]["choices_total"], 9);
    assert_eq!(choice["constraints"]["choices_truncated"], true);
}

#[test]
fn v2scl004_previews_stop_at_the_slot_budget_and_at_the_slot_count() {
    // 40 selected text items exceed the 32 preview slots, and one list entry alone exceeds a
    // slot's byte budget. Both cases must still publish complete metadata.
    let text = "\0".repeat(64);
    let entry = "\0".repeat(1_000);
    let source = wide_selection_readback_source();
    let session_view = view(wide_selection_readback_state(&source, &text, &entry));
    let next = project_graph_next_v2(&session_view).unwrap();
    let references = next["readback"].as_array().unwrap();
    assert_eq!(references.len(), 3);
    // Previews are spent in declaration order across every reference, so the budget is a property
    // of the response rather than of one reference.
    let items = references
        .iter()
        .flat_map(|reference| reference["items"].as_array().unwrap().iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 37);

    let previewed = items
        .iter()
        .filter(|item| item.get("preview").is_some())
        .count();
    assert_eq!(
        previewed, 32,
        "the preview slot budget must stop preview emission"
    );
    // Slots are spent in order, so once the budget runs out no later item carries an excerpt.
    let last_previewed = items
        .iter()
        .rposition(|item| item.get("preview").is_some())
        .expect("some item carries a preview");
    // Every item past the budget keeps its identity, digest, and size: only the excerpt is gone.
    for item in items.iter().skip(last_previewed + 1) {
        assert!(item.get("preview").is_none());
        assert!(item.get("preview_truncated").is_none());
        assert!(item.get("next_page_token").is_none());
        assert!(
            item["value_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(item["total_size"].is_number());
    }
    assert!(
        last_previewed + 1 < items.len(),
        "the fixture must be wider than the preview budget"
    );

    // A list whose first entry alone exceeds the slot admits nothing, and an empty excerpt is
    // published as no preview at all rather than spending a slot on nothing.
    let list = items
        .iter()
        .find(|item| item["item_id"] == "wide-list")
        .expect("the list item is selected");
    assert!(list.get("preview").is_none());
    assert_eq!(list["total_size"], 4);
    assert_eq!(list["size_unit"], "entries");
}

#[test]
fn v2scl004_an_empty_recorded_value_reads_as_one_empty_terminal_page() {
    // A text item with no minimum may legally record the empty string. Refusing to serve it would
    // make a recorded value unreadable and would report a token failure for a tokenless request.
    let source = paged_text_readback_source();
    let session_view = view(paged_text_readback_state(&source, ""));
    let node = GraphNodeId::new("source").unwrap();
    let item = ItemId::new("text").unwrap();

    let page = project_evidence_read_v1(&session_view, &node, &item, None).unwrap();
    assert_eq!(page["total_size"], 0);
    assert_eq!(page["size_unit"], "scalars");
    assert_eq!(page["page"]["offset"], 0);
    assert_eq!(page["page"]["size"], 0);
    assert_eq!(page["page"]["data"], "");
    assert_eq!(page["truncated"], false);
    assert_eq!(page["next_page_token"], Value::Null);

    // There is still no second page, so a token past the end stays exhausted.
    let status = project_graph_status_v2(&session_view, GraphStatusTierV2::Standard, None).unwrap();
    let beyond = podway_protocol::EvidencePageTokenV1::new(
        status["session"]["id"].as_str().unwrap(),
        page["consumer"]["attempt_id"].as_str().unwrap(),
        page["source"]["attempt_id"].as_str().unwrap(),
        "text",
        page["value_digest"].as_str().unwrap(),
        1,
    )
    .unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &node, &item, Some(&beyond)),
        Err(EvidenceReadErrorV2::TokenExhausted { .. })
    ));

    // An empty value has nothing to excerpt, so it carries no preview and spends no slot.
    let next = project_graph_next_v2(&session_view).unwrap();
    let metadata = &next["readback"].as_array().unwrap()[0]["items"]
        .as_array()
        .unwrap()[0];
    assert_eq!(metadata["total_size"], 0);
    assert!(metadata.get("preview").is_none());
    assert!(metadata.get("next_page_token").is_none());
}

#[test]
fn v2scl004_a_second_reference_to_one_source_stays_readable() {
    // Nothing forbids two evidence references to the same source node with different selectors,
    // and read-back publishes metadata for both. A lookup that consulted only the first would make
    // evidence the same response just advertised unreadable, which is the contradiction this
    // guards against.
    let source = serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2",
        "id": "duplicate-source-readback",
        "version": "2",
        "name": "Duplicate source read-back",
        "purpose": "Exercise two references to one source node.",
        "node_definitions": {
            "source": {
                "type": "action",
                "title": "Record source",
                "intent": "Record two values.",
                "items": [
                    {"id": "first", "type": "text", "prompt": "First.", "required": true, "max_length": 64},
                    {"id": "second", "type": "text", "prompt": "Second.", "required": true, "max_length": 64}
                ]
            },
            "consume": {
                "type": "action",
                "title": "Consume source",
                "intent": "Read both values."
            }
        },
        "graph": {
            "entry": "source",
            "nodes": [
                {"id": "source", "use": "source", "next": "consume"},
                {
                    "id": "consume",
                    "use": "consume",
                    "evidence_from": [
                        {"node": "source", "required": true, "items": ["first"]},
                        {"node": "source", "required": true, "items": ["second"]}
                    ],
                    "terminal": true
                }
            ]
        }
    }))
    .unwrap();
    let session_view = view(duplicate_source_readback_state(&source));
    let node = GraphNodeId::new("source").unwrap();

    // Both references publish metadata, so both items must answer a read.
    let next = project_graph_next_v2(&session_view).unwrap();
    let advertised: Vec<&str> = next["readback"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|reference| reference["items"].as_array().unwrap().iter())
        .map(|item| item["item_id"].as_str().unwrap())
        .collect();
    assert_eq!(advertised, vec!["first", "second"]);

    for item_id in ["first", "second"] {
        let item = ItemId::new(item_id).unwrap();
        let page = project_evidence_read_v1(&session_view, &node, &item, None)
            .unwrap_or_else(|error| panic!("{item_id} was advertised but unreadable: {error:?}"));
        assert_eq!(page["item"]["item_id"], item_id);
        assert_eq!(page["page"]["data"], format!("value-{item_id}"));
    }

    // An item neither reference selects is still refused.
    let absent = ItemId::new("third").unwrap();
    assert!(matches!(
        project_evidence_read_v1(&session_view, &node, &absent, None),
        Err(EvidenceReadErrorV2::NotAvailable { .. })
    ));
}

#[test]
fn v2scl004_evidence_read_separates_a_stale_reference_from_a_stale_token() {
    let text = "\0".repeat(1_024);
    let source = paged_text_readback_source();
    let node = GraphNodeId::new("source").unwrap();
    let item = ItemId::new("text").unwrap();

    let live = view(paged_text_readback_state(&source, &text));
    let page = project_evidence_read_v1(&live, &node, &item, None).unwrap();
    let consumer = page["consumer"]["attempt_id"].as_str().unwrap().to_owned();
    let source_attempt = page["source"]["attempt_id"].as_str().unwrap().to_owned();
    let digest = page["value_digest"].as_str().unwrap().to_owned();
    let status = project_graph_status_v2(&live, GraphStatusTierV2::Standard, None).unwrap();
    let session_id = status["session"]["id"].as_str().unwrap().to_owned();

    // A valid consumer holding a stale source is not a representable persisted state, so the
    // reference-stale arm is fail-closed defense rather than a reachable outcome. Proving the
    // invariant is what keeps it that way.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let stale_source = std::panic::catch_unwind(|| {
        paged_text_readback_state_with_validity(&source, &text, AttemptValidityV2::Stale)
    });
    std::panic::set_hook(previous_hook);
    assert!(
        stale_source.is_err(),
        "a valid consumer must not be able to hold a stale evidence source"
    );

    // A token bound to a different item never reaches a state comparison, so an invented token
    // cannot report whether some other item exists.
    let wrong_item = podway_protocol::EvidencePageTokenV1::new(
        &session_id,
        &consumer,
        &source_attempt,
        "some-other-item",
        &digest,
        0,
    )
    .unwrap();
    assert!(matches!(
        project_evidence_read_v1(&live, &node, &item, Some(&wrong_item)),
        Err(EvidenceReadErrorV2::TokenNotBound)
    ));

    // A token from an earlier consumer attempt is a moved snapshot, not a forged token: the caller
    // re-reads state and restarts the item, so it is recoverable rather than malformed.
    let earlier_consumer = podway_protocol::EvidencePageTokenV1::new(
        &session_id,
        ATTEMPT_ID,
        &source_attempt,
        "text",
        &digest,
        0,
    )
    .unwrap();
    assert!(matches!(
        project_evidence_read_v1(&live, &node, &item, Some(&earlier_consumer)),
        Err(EvidenceReadErrorV2::TokenStale { .. })
    ));
}

#[test]
fn v2scl004_evidence_read_continues_a_multi_page_text_value() {
    // Worst-case escaping charges six bytes per scalar, so a 256 KiB page carries 43,690 of them
    // and a maximal 65,536-scalar text needs exactly two pages.
    let text = "\0".repeat(65_536);
    let source = paged_text_readback_source();
    let session_view = view(paged_text_readback_state(&source, &text));
    let node = GraphNodeId::new("source").unwrap();
    let item = ItemId::new("text").unwrap();

    let first = project_evidence_read_v1(&session_view, &node, &item, None).unwrap();
    assert_eq!(first["total_size"], 65_536);
    assert_eq!(first["size_unit"], "scalars");
    assert_eq!(first["page"]["offset"], 0);
    assert_eq!(first["truncated"], true);
    let first_page = first["page"]["data"].as_str().unwrap().to_owned();
    assert_eq!(first_page.chars().count(), 43_690);
    assert_eq!(first["page"]["size"], 43_690);

    let token =
        podway_protocol::EvidencePageTokenV1::decode(first["next_page_token"].as_str().unwrap())
            .unwrap();
    assert_eq!(token.offset(), 43_690);
    let second = project_evidence_read_v1(&session_view, &node, &item, Some(&token)).unwrap();
    assert_eq!(second["page"]["offset"], 43_690);
    assert_eq!(second["truncated"], false);
    assert_eq!(second["next_page_token"], Value::Null);
    let second_page = second["page"]["data"].as_str().unwrap().to_owned();
    assert_eq!(second_page.chars().count(), 65_536 - 43_690);

    // The pages concatenate back to the recorded value with nothing split, dropped, or repeated.
    assert_eq!(format!("{first_page}{second_page}"), text);
    assert_eq!(first["value_digest"], second["value_digest"]);

    // AUT-EVR-004 bounds the complete encoded response, including its token and envelope, not only
    // the page data. A maximal page is the widest response there is, so this is where that bound
    // has to hold.
    for page in [&first, &second] {
        let response = ResponseEnvelopeV2::OutputV2(output("evidence.read", page.clone()));
        let encoded = encode_response_payload_v2(&response).unwrap();
        assert!(
            encoded.len() <= 320 * 1_024,
            "an evidence read response encoded to {} bytes",
            encoded.len()
        );
    }

    // The preview token published in the progression response resumes the same value, so a caller
    // never has to re-read a page it already saw.
    let next = project_graph_next_v2(&session_view).unwrap();
    let metadata = &next["readback"].as_array().unwrap()[0]["items"]
        .as_array()
        .unwrap()[0];
    assert_eq!(metadata["preview"].as_str().unwrap().chars().count(), 938);
    assert_eq!(metadata["preview_truncated"], true);
    let preview_token =
        podway_protocol::EvidencePageTokenV1::decode(metadata["next_page_token"].as_str().unwrap())
            .unwrap();
    assert_eq!(preview_token.offset(), 938);
    let resumed =
        project_evidence_read_v1(&session_view, &node, &item, Some(&preview_token)).unwrap();
    assert_eq!(resumed["page"]["offset"], 938);
    assert_eq!(
        resumed["page"]["data"].as_str().unwrap().chars().count(),
        43_690
    );
}

#[test]
fn v2scl004_evidence_read_answers_indivisible_values_in_one_terminal_page() {
    let source = maximum_selected_readback_source();
    let session_view = view(maximum_selected_readback_state(&source));
    let node = GraphNodeId::new("source").unwrap();

    // A confirm, choice, integer, or artifact value has no interior boundary to page at, so each
    // one answers completely at offset zero and publishes no continuation token.
    for (id, expected) in [
        ("confirm", json!(true)),
        ("choice", json!("\0".repeat(120))),
        ("integer", json!(i64::MAX)),
    ] {
        let item = ItemId::new(id).unwrap();
        let page = project_evidence_read_v1(&session_view, &node, &item, None).unwrap();
        assert_eq!(page["size_unit"], "value", "{id}");
        assert_eq!(page["total_size"], 1, "{id}");
        assert_eq!(page["page"]["offset"], 0, "{id}");
        assert_eq!(page["page"]["size"], 1, "{id}");
        assert_eq!(page["truncated"], false, "{id}");
        assert_eq!(page["next_page_token"], Value::Null, "{id}");
        assert_eq!(page["page"]["data"], expected, "{id}");

        // There is no second page to ask for: any non-zero offset is exhausted, never empty.
        let status =
            project_graph_status_v2(&session_view, GraphStatusTierV2::Standard, None).unwrap();
        let beyond = podway_protocol::EvidencePageTokenV1::new(
            status["session"]["id"].as_str().unwrap(),
            page["consumer"]["attempt_id"].as_str().unwrap(),
            page["source"]["attempt_id"].as_str().unwrap(),
            id,
            page["value_digest"].as_str().unwrap(),
            1,
        )
        .unwrap();
        assert!(
            matches!(
                project_evidence_read_v1(&session_view, &node, &item, Some(&beyond)),
                Err(EvidenceReadErrorV2::TokenExhausted { .. })
            ),
            "{id} accepted an offset past its only page"
        );
    }

    let artifact = ItemId::new("artifact").unwrap();
    let page = project_evidence_read_v1(&session_view, &node, &artifact, None).unwrap();
    assert_eq!(page["item"]["type"], "artifact");
    assert_eq!(page["total_size"], 1);
    assert_eq!(page["truncated"], false);
    assert_eq!(page["next_page_token"], Value::Null);
    // The artifact page carries the complete descriptor, which is what makes it worth reading.
    assert_eq!(page["page"]["data"]["location_type"], "reference");
    assert_eq!(
        page["page"]["data"]["sha256_digest"],
        format!("sha256:{}", "f".repeat(64))
    );
    assert_eq!(page["page"]["data"]["size_bytes"], u64::MAX);
    assert_eq!(
        page["page"]["data"]["location"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        4_000
    );
}

#[test]
fn v2scl004_evidence_previews_admit_only_whole_entries_inside_the_slot() {
    let source = maximum_selected_readback_source();
    let next = project_graph_next_v2(&view(maximum_selected_readback_state(&source))).unwrap();
    let items = next["readback"].as_array().unwrap()[0]["items"]
        .as_array()
        .unwrap();
    let list = items
        .iter()
        .find(|item| item["item_id"] == "list")
        .expect("the list item is selected");

    // Entries are 308 scalars, so three fit inside the 938-scalar slot and a fourth would exceed
    // it. A preview never splits an entry, so it stops at three rather than charging 1,232.
    let preview = list["preview"].as_array().unwrap();
    assert_eq!(preview.len(), 3);
    assert!(
        preview
            .iter()
            .all(|entry| entry.as_str().unwrap().chars().count() == 308)
    );
    assert_eq!(list["preview_truncated"], true);
    let token =
        podway_protocol::EvidencePageTokenV1::decode(list["next_page_token"].as_str().unwrap())
            .unwrap();
    assert_eq!(token.offset(), 3);

    // Every published preview stays inside the slot, which is what keeps the response bounded.
    for item in items {
        let Some(preview) = item.get("preview") else {
            continue;
        };
        let scalars = match preview {
            Value::String(text) => text.chars().count(),
            Value::Array(entries) => entries
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(|entry| entry.chars().count())
                        .unwrap_or(0)
                })
                .sum(),
            _ => 0,
        };
        assert!(
            scalars <= 938,
            "preview for {} charged {scalars} scalars",
            item["item_id"]
        );
    }
}
