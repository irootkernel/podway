//! V2REL-002 production projection proof for the complete `next` payload budget.

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    procedure_placement_budget_v2, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ArtifactValueV1, AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2,
    AuthoringDiagnosticCode, BlockerId, BlockerState, CriterionId, EvidenceReferenceSnapshotV2,
    GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2, GoalRevisionRecordV2, GoalStatementV2,
    GraphNodeId, ItemId, ItemTypeV1, ProcedureSnapshotId, RecordedItemValueV2,
    ResolvedEvidenceReferenceV2, Revision, SessionAttemptV2, SessionId, SessionLifecycle,
    SessionTraceV2, Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId,
};
use podway_daemon::{
    execution::{
        ProcedureProviderV1, ProcedureV2SourceAdmissionErrorV1, prepare_custom_procedure_v2_start,
        workspace_procedure_snapshot_from_bytes_v2,
    },
    v2_read_service::project_graph_next_v2,
};
use podway_protocol::{
    CommandNameV1, OutputEnvelopeInputV3, OutputEnvelopeV3, RequestIdV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, SessionLifecycleV1, SessionOutputV1, WorkspaceOutputV1,
    decode_response_payload_v2, decode_single_frame_v1, encode_frame_v1,
    encode_response_payload_v2, validate_frame_payload_length,
};
use podway_store::{
    AttemptMetadataV2, AttemptWorkflowMemoryV2, BlockerStateV2, DurableWorktreeIdentityV1,
    EvidenceResolutionStateV2, GoalStateV2, GraphNodeCounterV2, GraphSessionStateV2,
    GraphWorkspaceViewV2, ItemSlotStateV2, ProcedureSnapshotV2, ValidatedWorkspaceRootV1,
    WorkflowMemoryStateV2, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};

const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000007001";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000007002";
const SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000007003";
const REQUEST_ID: &str = "00000000-0000-4000-8000-000000007004";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SELECTED: [&str; 16] = [
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

#[derive(Clone, Copy)]
struct BytesProcedure<'a>(&'a [u8]);

impl ProcedureProviderV1 for BytesProcedure<'_> {
    fn load_workspace_procedure_snapshot_v2(
        &self,
        _: &WorkspaceBindingV1,
        procedure: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV2, ProcedureV2SourceAdmissionErrorV1> {
        workspace_procedure_snapshot_from_bytes_v2(procedure, self.0, snapshot_id, created_at)
    }
}

fn identifier(prefix: char, index: usize) -> String {
    format!("{prefix}{}{:02}", "0".repeat(61), index)
}

fn item_id(index: usize) -> String {
    maximum_item_id(index, 41)
}

fn maximum_item_id(index: usize, padding: usize) -> String {
    format!("i{}{:02}", "0".repeat(padding), index)
}

fn identity() -> DurableWorktreeIdentityV1 {
    let digest = Sha256Digest::new(DIGEST).unwrap();
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

fn maximum_source() -> Vec<u8> {
    maximum_source_with_item_padding(41)
}

fn maximum_source_with_item_padding(item_padding: usize) -> Vec<u8> {
    let mut source_items = vec![
        json!({"id":"confirm","type":"confirm","prompt":"Confirm.","required":true}),
        json!({"id":"text","type":"text","prompt":"Text.","required":true,"max_length":16_384}),
        json!({"id":"choice","type":"choice","prompt":"Choose.","required":true,"choices":["\0".repeat(120)]}),
        json!({"id":"integer","type":"integer","prompt":"Count.","required":true}),
        json!({"id":"list","type":"list","prompt":"List.","required":true,"max_items":200,"max_item_length":308,"unique":false}),
        json!({"id":"artifact","type":"artifact","prompt":"Artifact.","required":true,"allowed_media_types":[format!("{}/{}","a".repeat(127),"b".repeat(127))]}),
    ];
    source_items.extend((0..10).map(|index| {
        json!({"id":format!("selected-{index:02}"),"type":"confirm","prompt":"Confirm.","required":true})
    }));
    source_items.push(json!({"id":"unselected","type":"text","prompt":"Secret.","required":true,"max_length":16_384}));

    let consumer_items = (0..64)
        .map(|index| {
            json!({
            "id":maximum_item_id(index, item_padding), "type":"text", "prompt":"값".repeat(300),
                "required":true, "max_length":16_384
            })
        })
        .collect::<Vec<_>>();
    let filler_ids = (0..61)
        .map(|index| identifier('n', index))
        .collect::<Vec<_>>();
    let mut nodes = vec![json!({"id":"source","use":"source","next":filler_ids[0]})];
    for (index, id) in filler_ids.iter().enumerate() {
        let next = filler_ids
            .get(index + 1)
            .cloned()
            .unwrap_or_else(|| "consume".into());
        nodes.push(json!({"id":id,"use":"filler","next":next}));
    }
    nodes.push(json!({
        "id":"consume", "use":"consume",
        "evidence_from":[{"node":"source","required":true,"items":SELECTED}],
        "routes":{
            "achieved":{"to":"finish","effect":"advance"},
            "not-achieved":{"to":"finish","effect":"advance"},
            "superseded":{"to":"finish","effect":"advance"}
        }
    }));
    nodes.push(json!({"id":"finish","use":"filler","terminal":true}));
    let mut targets = vec![
        "source".to_owned(),
        "consume".to_owned(),
        "finish".to_owned(),
    ];
    targets.extend(filler_ids);
    serde_json::to_vec(&json!({
        "schema":"podway.procedure/v2", "id":"maximum-production-next", "version":"2",
        "name":"Maximum production next",
        "purpose":"Exercise every reachable next payload component through admission and projection.",
        "goal_tracking":true,
        "node_definitions":{
            "source":{"type":"action","title":"Record source","intent":"Record values.","items":source_items},
            "filler":{"type":"action","title":"Traverse","intent":"Continue."},
            "consume":{
                "type":"decision", "title":"값".repeat(120), "description":"값".repeat(1_000),
                "objective":"값".repeat(300), "prompt":"값".repeat(500), "items":consumer_items,
                "options":[
                    {"id":"achieved","label":"값".repeat(120),"criteria":"값".repeat(500)},
                    {"id":"not-achieved","label":"값".repeat(120),"criteria":"값".repeat(500)},
                    {"id":"superseded","label":"값".repeat(120),"criteria":"값".repeat(500)}
                ],
                "reason":{"required":true,"prompt":"값".repeat(300)},
                "evidence_guidance":vec!["값".repeat(200);8],
                "assessment":{"target":"session_goal","outcomes":{
                    "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"
                }}
            }
        },
        "graph":{"entry":"source","nodes":nodes},
        "manual_rework":{"allowed_targets":targets}
    }))
    .unwrap()
}

fn source_value(id: &str) -> RecordedItemValueV2 {
    match id {
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
        other => panic!("unexpected source item {other}"),
    }
}

fn maximum_state(source: &[u8]) -> GraphSessionStateV2 {
    let provider = BytesProcedure(source);
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "maximum-production-next.json",
        source,
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap();
    let base = prepare_custom_procedure_v2_start(
        &provider,
        &binding(),
        "maximum-production-next.json",
        Some(snapshot.digest()),
        "Maximum production next",
        SessionId::new(SESSION_ID).unwrap(),
        AttemptId::new("00000000-0000-4000-8007-000000000000").unwrap(),
        ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        UnixMillis::new(1_700_000_000_000),
    )
    .unwrap();
    let graph_ids = std::iter::once("source".to_owned())
        .chain((0..61).map(|index| identifier('n', index)))
        .chain(std::iter::once("consume".to_owned()))
        .collect::<Vec<_>>();
    let attempt_ids = (0..63)
        .map(|index| AttemptId::new(format!("00000000-0000-4000-8007-{index:012x}")).unwrap())
        .collect::<Vec<_>>();
    let trace_attempts = graph_ids
        .iter()
        .zip(&attempt_ids)
        .enumerate()
        .map(|(index, (node, attempt))| {
            SessionAttemptV2::new(
                attempt.clone(),
                GraphNodeId::new(node.clone()).unwrap(),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new(index as u64 + 1),
                if index == 62 {
                    AttemptLifecycle::Active
                } else {
                    AttemptLifecycle::Completed
                },
                AttemptValidityV2::Valid,
                (index == 62).then_some(GoalRevisionNumberV2::FIRST),
            )
            .unwrap()
        })
        .collect();
    let metadata = attempt_ids
        .iter()
        .enumerate()
        .map(|(index, attempt)| {
            let started = UnixMillis::new(1_700_000_010_000 + index as u64);
            AttemptMetadataV2::new(
                attempt.clone(),
                started,
                (index < 62).then(|| UnixMillis::new(started.get() + 1)),
                None,
            )
            .unwrap()
        })
        .collect();

    let source_slots = base.workflow_memory().attempts()[0]
        .item_slots()
        .iter()
        .map(|slot| {
            ItemSlotStateV2::new(
                attempt_ids[0].clone(),
                slot.item_id().clone(),
                slot.item_type(),
                Revision::new(1),
                Some(source_value(slot.item_id().as_str())),
                UnixMillis::new(1_700_000_010_000),
                UnixMillis::new(1_700_000_010_001),
            )
            .unwrap()
        })
        .collect();
    let source_memory =
        AttemptWorkflowMemoryV2::new(attempt_ids[0].clone(), source_slots, Vec::new(), Vec::new())
            .unwrap();
    let source_digest = source_memory.recorded_items_digest().unwrap();
    let consumer_slots = (0..64)
        .map(|index| {
            ItemSlotStateV2::new(
                attempt_ids[62].clone(),
                ItemId::new(item_id(index)).unwrap(),
                ItemTypeV1::Text,
                Revision::ZERO,
                None,
                UnixMillis::new(1_700_000_010_062),
                UnixMillis::new(1_700_000_010_062),
            )
            .unwrap()
        })
        .collect();
    let blockers = (0..64)
        .map(|index| {
            BlockerStateV2::new(
                BlockerId::new(format!("00000000-0000-4000-8008-{index:012x}")).unwrap(),
                attempt_ids[62].clone(),
                format!("{index:02}-{}", "\0".repeat(997)),
                BlockerState::Open,
                UnixMillis::new(1_700_000_020_000 + index),
                None,
            )
            .unwrap()
        })
        .collect();
    let evidence = EvidenceResolutionStateV2::new(
        0,
        true,
        SELECTED
            .iter()
            .map(|id| ItemId::new(*id).unwrap())
            .collect(),
        ResolvedEvidenceReferenceV2::resolved(
            EvidenceReferenceSnapshotV2::new(
                GraphNodeId::new("source").unwrap(),
                attempt_ids[0].clone(),
                AttemptNumberV2::FIRST,
                source_digest,
                UnixMillis::new(1_700_000_010_062),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let consumer_memory = AttemptWorkflowMemoryV2::new(
        attempt_ids[62].clone(),
        consumer_slots,
        blockers,
        vec![evidence],
    )
    .unwrap();
    let mut memories = vec![source_memory];
    memories.extend((1..62).map(|index| {
        AttemptWorkflowMemoryV2::new(
            attempt_ids[index].clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
    }));
    memories.push(consumer_memory);

    let goal_definition = GoalDefinitionV2::new(
        (0..16)
            .map(|index| {
                GoalCriterionV2::new(
                    CriterionId::new(identifier('c', index)).unwrap(),
                    "\0".repeat(300),
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap();
    let goal_revision = GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        None,
        GoalStatementV2::new("\0".repeat(1_000)).unwrap(),
        goal_definition,
        None,
        None,
        false,
        None,
        TraceSequenceV2::new(63),
        UnixMillis::new(1_700_000_010_062),
    )
    .unwrap();
    let trace = SessionTraceV2::from_parts(
        SessionId::new(SESSION_ID).unwrap(),
        SessionLifecycle::Running,
        Revision::new(63),
        trace_attempts,
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        Revision::new(63),
        base.task_title(),
        base.snapshot().clone(),
        trace,
        graph_ids
            .iter()
            .map(|node| GraphNodeCounterV2::new(GraphNodeId::new(node.clone()).unwrap(), 1, 0))
            .chain(std::iter::once(GraphNodeCounterV2::new(
                GraphNodeId::new("finish").unwrap(),
                0,
                0,
            )))
            .collect(),
        metadata,
        WorkflowMemoryStateV2::new(memories, Vec::new(), Vec::new()).unwrap(),
        GoalStateV2::new(
            Some(GoalRevisionNumberV2::FIRST),
            vec![goal_revision],
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
        base.created_at(),
        None,
        None,
        None,
    )
    .unwrap()
}

fn charge(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(string) => string.chars().count() * 6,
        Value::Array(values) => values.iter().map(|value| 8 + charge(value)).sum(),
        Value::Object(fields) => fields.values().map(|value| 64 + charge(value)).sum(),
    }
}

fn fields(result: &Map<String, Value>, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .filter_map(|name| {
                result
                    .get(*name)
                    .map(|value| ((*name).to_owned(), value.clone()))
            })
            .collect(),
    )
}

const ENVELOPE_SELECTORS: &[&str] = &[
    "schema",
    "procedure_schema",
    "procedure_digest",
    "node",
    "attempt",
    "queue",
    "revision",
    "missing_required_item_count",
    "readiness",
];
const STATIC_SELECTORS: &[&str] = &[
    "allowed_actions",
    "allowed_manual_rework_targets",
    "description",
    "evidence_guidance",
    "instructions",
    "intent",
    "missing_required_items",
    "next_graph_node_id",
    "objective",
    "options",
    "prompt",
    "reason_policy",
    "skip",
    "terminal",
    "title",
];
const READBACK_SELECTORS: &[&str] = &["readback", "references"];
const GOAL_SELECTORS: &[&str] = &[
    "goal",
    "goal_defined",
    "goal_revision",
    "goal_tracking",
    "latest_goal_outcome",
];
const BLOCKER_SELECTORS: &[&str] = &["blockers", "blockers_total", "blockers_truncated"];
const COUNTER_SELECTORS: &[&str] = &["counters", "trace_length"];

#[test]
fn v2rel002_admitted_maximum_next_uses_the_production_projector_and_frame() {
    let source = maximum_source();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(&source, ProcedureDocumentFormat::Json).unwrap()
    else {
        panic!("fixture must remain Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let source_text = std::str::from_utf8(&source).unwrap();
    let findings = vet_procedure_v2(
        &validated,
        &AuthoringContext::new(
            "maximum-production-next.json",
            source_text,
            ProcedureDocumentFormat::Json,
        ),
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.code().severity() != podway_core::AuthoringSeverity::Error),
        "maximum fixture must pass production vet: {findings:?}"
    );
    let production_budget =
        procedure_placement_budget_v2(&validated, &GraphNodeId::new("consume").unwrap()).unwrap();
    assert!(production_budget.next_static() <= podway_config::NEXT_STATIC_BUDGET);
    assert!(production_budget.next_static() >= 260_000);
    assert!(production_budget.readback() <= podway_config::READBACK_BUDGET);
    assert!(production_budget.readback() >= 490_000);

    let rejected_source = maximum_source_with_item_padding(42);
    let ParsedProcedure::V2(rejected) =
        parse_procedure_document(&rejected_source, ProcedureDocumentFormat::Json).unwrap()
    else {
        unreachable!()
    };
    let rejected = validate_procedure_v2(rejected).unwrap();
    let rejected_text = std::str::from_utf8(&rejected_source).unwrap();
    let rejected_findings = vet_procedure_v2(
        &rejected,
        &AuthoringContext::new(
            "maximum-production-next-over.json",
            rejected_text,
            ProcedureDocumentFormat::Json,
        ),
    );
    assert!(rejected_findings.iter().any(|finding| {
        finding.code() == AuthoringDiagnosticCode::NextStaticBudgetExceeded
            && finding.graph_node_id() == Some("consume")
    }));

    let view = GraphWorkspaceViewV2::new(
        identity(),
        Some(maximum_state(&source)),
        0,
        None,
        u64::MAX,
        UnixMillis::new(1_700_000_030_000),
    );
    let next = project_graph_next_v2(&view).unwrap();
    assert_eq!(next["node"]["node_type"], "decision");
    assert_eq!(next["missing_required_items"].as_array().unwrap().len(), 64);
    assert_eq!(next["goal"]["criteria"].as_array().unwrap().len(), 16);
    assert_eq!(
        next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|suggestion| suggestion["command"] == "goal.assess_criterion")
            .count(),
        16
    );
    assert_eq!(next["counters"].as_array().unwrap().len(), 64);
    assert_eq!(next["blockers_total"], 64);
    assert_eq!(next["blockers_truncated"], true);
    assert_eq!(
        next["allowed_manual_rework_targets"]
            .as_array()
            .unwrap()
            .len(),
        63
    );
    assert_eq!(next["references"].as_array().unwrap().len(), 1);
    assert_eq!(next["readback"][0]["items"].as_array().unwrap().len(), 16);
    for conditionally_inapplicable in [
        "instructions",
        "intent",
        "next_graph_node_id",
        "skip",
        "terminal",
        "latest_goal_outcome",
    ] {
        assert!(
            !next.contains_key(conditionally_inapplicable),
            "{conditionally_inapplicable} must remain absent for this valid decision projection"
        );
    }

    let mut assigned = ENVELOPE_SELECTORS
        .iter()
        .chain(STATIC_SELECTORS)
        .chain(READBACK_SELECTORS)
        .chain(GOAL_SELECTORS)
        .chain(BLOCKER_SELECTORS)
        .chain(COUNTER_SELECTORS)
        .filter(|selector| next.contains_key(**selector))
        .copied()
        .collect::<Vec<_>>();
    assigned.push("suggestions");
    assigned.sort_unstable();
    let mut actual = next.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    assert_eq!(
        assigned, actual,
        "every emitted next field is assigned exactly once"
    );

    let static_suggestions = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] != "goal.assess_criterion")
        .cloned()
        .collect::<Vec<_>>();
    let goal_suggestions = next["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|suggestion| suggestion["command"] == "goal.assess_criterion")
        .cloned()
        .collect::<Vec<_>>();
    let mut static_component = fields(&next, STATIC_SELECTORS);
    static_component
        .as_object_mut()
        .unwrap()
        .insert("suggestions".into(), json!(static_suggestions));
    let static_encoded = serde_json::to_vec(&static_component).unwrap().len();
    assert!(u64::try_from(static_encoded).unwrap() <= production_budget.next_static());

    let readback_component = fields(&next, READBACK_SELECTORS);
    let readback_encoded = serde_json::to_vec(&readback_component).unwrap().len();
    assert!(u64::try_from(readback_encoded).unwrap() <= production_budget.readback());
    let mut goal_component = fields(&next, GOAL_SELECTORS);
    goal_component
        .as_object_mut()
        .unwrap()
        .insert("suggestions".into(), json!(goal_suggestions));
    for (name, value, maximum, minimum) in [
        ("goal", goal_component, 73_728, 39_000),
        ("blockers", fields(&next, BLOCKER_SELECTORS), 49_152, 40_000),
        ("counters", fields(&next, COUNTER_SELECTORS), 40_960, 0),
    ] {
        let encoded = serde_json::to_vec(&value).unwrap().len();
        let charged = charge(&value);
        assert!(
            encoded <= charged,
            "{name}: encoded={encoded}, charged={charged}"
        );
        assert!(
            encoded <= maximum,
            "{name}: encoded={encoded}, maximum={maximum}"
        );
        assert!(
            encoded >= minimum,
            "{name}: encoded={encoded}, minimum={minimum}"
        );
    }
    assert!(
        charge(&fields(&next, COUNTER_SELECTORS)) >= 30_000,
        "counter worst-case charge must materially exercise COUNTERS_MAX"
    );

    let warnings = (0..4)
        .map(|_| {
            json!({"code":"A".repeat(64),"path":"\0".repeat(256),"message":"\0".repeat(512)})
                .as_object()
                .unwrap()
                .clone()
        })
        .collect();
    let output = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(REQUEST_ID).unwrap(),
        command: CommandNameV1::new("session.next").unwrap(),
        generated_at: Rfc3339MillisV1::new("2026-08-12T00:00:00.000Z").unwrap(),
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
                Revision::new(63),
                Revision::new(63),
            )
            .unwrap(),
        ),
        result: next.clone(),
        warnings,
    })
    .unwrap();
    let output_value = serde_json::to_value(&output).unwrap();
    let output_fields = output_value.as_object().unwrap();
    let mut envelope_component = fields(&next, ENVELOPE_SELECTORS);
    envelope_component.as_object_mut().unwrap().extend(
        [
            "schema",
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
            output_fields
                .get(field)
                .map(|value| (format!("output_{field}"), value.clone()))
        }),
    );
    envelope_component
        .as_object_mut()
        .unwrap()
        .insert("frame_length_prefix_bytes".into(), json!(4));
    let envelope_encoded = serde_json::to_vec(&envelope_component).unwrap().len();
    let envelope_charged = charge(&envelope_component);
    assert!(envelope_encoded <= envelope_charged);
    assert!(
        envelope_charged <= 65_536,
        "ENVELOPE_RESERVE charged {envelope_charged} bytes"
    );
    let response = ResponseEnvelopeV2::OutputV2(output);
    let encoded = encode_response_payload_v2(&response).unwrap();
    validate_frame_payload_length(encoded.len()).unwrap();
    assert_eq!(decode_response_payload_v2(&encoded).unwrap(), response);
    let frame = encode_frame_v1(&encoded).unwrap();
    assert_eq!(decode_single_frame_v1(&frame).unwrap(), encoded);
}
