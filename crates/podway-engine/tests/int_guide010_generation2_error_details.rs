//! GUIDE-010: generation-2 error details.
//!
//! Pins the generation-2 detail families of the two GUIDE-010 error codes
//! through the typed Direct Engine (`execute_direct_v1`) and the public
//! renderer:
//! - `podway.criterion-citation-invalid-details/v1` carries complete citable
//!   lists on the admission failure path at the 8/128 domain caps, with
//!   every live-reachable exclusion, and empty. The duplicate-detection
//!   failure path is unreachable: the typed Engine rejects duplicated
//!   citations as `REQUEST_INVALID` before Core admission, and
//!   `CriterionAssessmentResultV2::new` refuses them by construction; the
//!   Core unit test `criterion_citation_carrier_tests`
//!   (`podway-core/src/graph_v2.rs`) proves the carrier at Core level.
//! - `podway.workspace-contract-mismatch-details/v2` maps the stored writer
//!   digest through the compiled released-manifest table, or to null for an
//!   unknown digest, with the mismatch-specific generation-2 recovery reason.
//!
//! A `Resolved`-but-non-`Valid` evidence source needs no fixture: a valid
//! consumer attempt cannot hold stale evidence, and the recording path
//! requires a running valid active attempt, so the admission loop never sees
//! one.
//!
//! Generation 1 keeps the frozen generation-1 families; byte-level identity
//! stays owned by the GUIDE-006 baselines and `int_guide009`.

use podway_core::{
    AttemptId, BlockerId, CriterionId, DelegationReceiptIdV1, GraphNodeId, ItemId, OptionId,
    ProcedureSnapshotId, Revision, SessionId, WorkspaceId,
};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_error::{DirectErrorContextV1, render_direct_error_v1},
    direct_execution::{DirectCursorCommandV1, DirectExecutionErrorV1, DirectIdSourceV1},
    direct_items::DirectItemCommandV1,
    direct_start::DirectStartCommandV1,
    execution::prepare_workspace_procedure_source_v2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{NativeGitResolverV1, WorktreeSelectorV1};
use podway_protocol::{
    CommandNameV1, ContractGenerationV1, GoalAssessCriterionV2, GoalCriterionWireV2,
    InitialGoalWireV2, ItemMutationPreconditionsWireV1, ItemRecordManyDispositionV1,
    ItemRecordManyOperationV1, ItemRecordManyV1, ItemRecordValueV1, ItemSetV1,
    ProcedureV2MutationCommandV1, RELEASED_CONTRACT_MANIFESTS_V1, RequestIdV1, Rfc3339MillisV1,
    SessionBeginV1, SessionCompleteV1, SessionDecideV2, SessionIdentityPreconditionsWireV1,
    SessionMutationPreconditionsWireV1, SessionSkipV1, SessionStartCommandV1, SessionStartSourceV1,
    SessionStartV1,
};
use podway_store::{
    GraphSessionStateV2, IdempotencyKeyV1,
    direct_store::{DirectStoreErrorV1, DirectStoreV1},
};
use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

use crate::int_guide006_generation1_baselines::{
    guide006_criterion_citation_invalid, guide006_workspace_contract_mismatch,
};
use crate::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

const GUIDE010_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000716";
const GUIDE010_OBSERVED_AT: &str = "2023-11-14T22:13:20.002Z";
const GUIDE010_SESSION_ID: &str = "00000000-0000-4000-8000-000000000717";
const GUIDE010_SNAPSHOT_ID: &str = "00000000-0000-4000-8000-000000000718";
const GUIDE010_WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000719";
const GUIDE010_BLOCKER_ID: &str = "00000000-0000-4000-8000-000000000720";
const GUIDE010_RECEIPT_ID: &str = "00000000-0000-4000-8000-000000000721";

const GENERATION2_CITATION_SCHEMA: &str = "podway.criterion-citation-invalid-details/v1";
const GENERATION2_MISMATCH_SCHEMA: &str = "podway.workspace-contract-mismatch-details/v2";
const GENERATION1_CITATION_SCHEMA: &str = "podway.v2-runtime-error-details/v2";
const GENERATION1_MISMATCH_SCHEMA: &str = "podway.workspace-contract-mismatch-details/v1";
const GENERATION1_MISMATCH_REASON: &str = "Inspect writer identity. Restore access or use the original writer. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path. Newer generations stay refused.";

/// Deterministic identifiers for GUIDE-010 scenarios. Unlike the bounded
/// GUIDE-006 set, attempt identifiers come from an unbounded counter: the cap
/// fixture alone needs nine attempts (eight sources and the consumer).
struct Guide010Ids {
    attempts: std::sync::atomic::AtomicUsize,
}

impl Guide010Ids {
    fn new() -> Self {
        Self {
            attempts: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Default for Guide010Ids {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectIdSourceV1 for Guide010Ids {
    fn next_workspace_id(&self) -> WorkspaceId {
        WorkspaceId::new(GUIDE010_WORKSPACE_ID).unwrap()
    }
    fn next_session_id(&self) -> SessionId {
        SessionId::new(GUIDE010_SESSION_ID).unwrap()
    }
    fn next_attempt_id(&self) -> AttemptId {
        use std::sync::atomic::Ordering;
        let index = self.attempts.fetch_add(1, Ordering::Relaxed);
        AttemptId::new(format!("00000000-0000-4000-8000-{:012x}", 0x730 + index)).unwrap()
    }
    fn next_blocker_id(&self) -> BlockerId {
        BlockerId::new(GUIDE010_BLOCKER_ID).unwrap()
    }
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        ProcedureSnapshotId::new(GUIDE010_SNAPSHOT_ID).unwrap()
    }
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
        DelegationReceiptIdV1::new(GUIDE010_RECEIPT_ID).unwrap()
    }
}

/// The cap fixture's evidence sources in `evidence_from` declaration order,
/// which deliberately differs from lexicographic order so the asserted list
/// order proves declaration order rather than sorting.
const CAP_SOURCES: [&str; 8] = [
    "cap-src-h",
    "cap-src-f",
    "cap-src-d",
    "cap-src-b",
    "cap-src-a",
    "cap-src-c",
    "cap-src-e",
    "cap-src-g",
];

/// The cap fixture's 128 consumer items, declared from `cap-127` down to
/// `cap-000` so declaration order differs from lexicographic order.
const CAP_ITEMS: [&str; 128] = [
    "cap-127", "cap-126", "cap-125", "cap-124", "cap-123", "cap-122", "cap-121", "cap-120",
    "cap-119", "cap-118", "cap-117", "cap-116", "cap-115", "cap-114", "cap-113", "cap-112",
    "cap-111", "cap-110", "cap-109", "cap-108", "cap-107", "cap-106", "cap-105", "cap-104",
    "cap-103", "cap-102", "cap-101", "cap-100", "cap-099", "cap-098", "cap-097", "cap-096",
    "cap-095", "cap-094", "cap-093", "cap-092", "cap-091", "cap-090", "cap-089", "cap-088",
    "cap-087", "cap-086", "cap-085", "cap-084", "cap-083", "cap-082", "cap-081", "cap-080",
    "cap-079", "cap-078", "cap-077", "cap-076", "cap-075", "cap-074", "cap-073", "cap-072",
    "cap-071", "cap-070", "cap-069", "cap-068", "cap-067", "cap-066", "cap-065", "cap-064",
    "cap-063", "cap-062", "cap-061", "cap-060", "cap-059", "cap-058", "cap-057", "cap-056",
    "cap-055", "cap-054", "cap-053", "cap-052", "cap-051", "cap-050", "cap-049", "cap-048",
    "cap-047", "cap-046", "cap-045", "cap-044", "cap-043", "cap-042", "cap-041", "cap-040",
    "cap-039", "cap-038", "cap-037", "cap-036", "cap-035", "cap-034", "cap-033", "cap-032",
    "cap-031", "cap-030", "cap-029", "cap-028", "cap-027", "cap-026", "cap-025", "cap-024",
    "cap-023", "cap-022", "cap-021", "cap-020", "cap-019", "cap-018", "cap-017", "cap-016",
    "cap-015", "cap-014", "cap-013", "cap-012", "cap-011", "cap-010", "cap-009", "cap-008",
    "cap-007", "cap-006", "cap-005", "cap-004", "cap-003", "cap-002", "cap-001", "cap-000",
];

fn cap_procedure() -> Vec<u8> {
    let items: Vec<Value> = CAP_ITEMS
        .iter()
        .map(|item| {
            json!({"id": item, "type": "text", "prompt": "Cap item.", "required": false,
                "max_length": 16})
        })
        .collect();
    let references: Vec<Value> = CAP_SOURCES
        .iter()
        .map(|source| json!({"node": source, "required": true}))
        .collect();
    let mut nodes: Vec<Value> = CAP_SOURCES
        .iter()
        .enumerate()
        .map(|(index, source)| {
            json!({"id": source, "use": "cap-src",
                "next": CAP_SOURCES.get(index + 1).map_or("cap-decide", |next| next)})
        })
        .collect();
    nodes.push(
        json!({"id": "cap-decide", "use": "cap-assess", "evidence_from": references,
        "routes": {"achieved": {"to": "cap-finish", "effect": "advance"},
            "not-achieved": {"to": "cap-finish", "effect": "advance"},
            "superseded": {"to": "cap-finish", "effect": "advance"}}}),
    );
    nodes.push(json!({"id": "cap-finish", "use": "cap-finish", "terminal": true}));
    serde_json::to_vec(&json!({
        "schema": "podway.procedure/v2", "id": "guide010-caps", "version": "2",
        "name": "Guide010 caps", "purpose": "Exercise the generation-2 citation-list caps.",
        "goal_tracking": true,
        "node_definitions": {
            "cap-src": {"type": "action", "title": "Cap source",
                "intent": "Resolve one required reference."},
            "cap-assess": {"type": "decision", "title": "Assess the goal",
                "objective": "Select the outcome determined by the recorded criteria.",
                "prompt": "Which goal outcome applies?", "items": items,
                "options": [{"id": "achieved", "label": "Achieved"},
                    {"id": "not-achieved", "label": "Not achieved"},
                    {"id": "superseded", "label": "Superseded"}],
                "reason": {"required": true},
                "assessment": {"target": "session_goal", "outcomes": {
                    "achieved": "achieved", "not-achieved": "not_achieved",
                    "superseded": "superseded"}}},
            "cap-finish": {"type": "action", "title": "Finish", "intent": "Finish."}},
        "graph": {"entry": CAP_SOURCES[0], "nodes": nodes},
        "manual_rework": {"allowed_targets": [CAP_SOURCES[0]]}
    }))
    .unwrap()
}

/// One resolved source, one skipped source, one unvisited alternate-branch
/// source, one duplicated source and one unrecorded declared item, so the
/// citable lists exclude every non-citable state and the duplicate appears
/// exactly once.
const EXCLUSION_PROCEDURE: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide010-exclusions","version":"2",
    "name":"Guide010 exclusions","purpose":"Exercise generation-2 citation-list exclusions.",
    "goal_tracking":true,
    "node_definitions":{
        "work":{"type":"action","title":"Produce evidence","intent":"Record the resolved source.",
            "items":[{"id":"result","type":"text","prompt":"Record the result.","required":true,"max_length":1000}]},
        "side":{"type":"action","title":"Optional side work","intent":"Skipped before the consumer."},
        "lone":{"type":"action","title":"Unvisited branch","intent":"Never attempted."},
        "pick":{"type":"decision","title":"Pick the branch","objective":"Choose the main branch.",
            "prompt":"Which branch?","options":[{"id":"main","label":"Main"},{"id":"alt","label":"Alt"}],
            "reason":{"required":true}},
        "assess":{"type":"decision","title":"Assess the goal",
            "objective":"Select the outcome determined by the recorded criteria.",
            "prompt":"Which goal outcome applies?",
            "items":[{"id":"keep","type":"text","prompt":"Kept.","required":false,"max_length":100},
                {"id":"drop","type":"text","prompt":"Dropped.","required":false,"max_length":100}],
            "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},
                {"id":"superseded","label":"Superseded"}],
            "reason":{"required":true},"assessment":{"target":"session_goal","outcomes":{
                "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"}}},
        "finish":{"type":"action","title":"Finish","intent":"Finish."}},
    "graph":{"entry":"pick","nodes":[
        {"id":"pick","use":"pick","routes":{"main":{"to":"source-r","effect":"advance"},
            "alt":{"to":"lone","effect":"advance"}}},
        {"id":"source-r","use":"work","next":"source-s"},
        {"id":"source-s","use":"side","next":"decide","skip":{"allowed":true,"reason_required":false}},
        {"id":"lone","use":"lone","next":"source-r"},
        {"id":"decide","use":"assess","evidence_from":[
            {"node":"source-r","required":true,"items":["result"]},
            {"node":"source-s","required":false},
            {"node":"lone","required":false},
            {"node":"source-r","required":false,"items":["result"]}],
            "routes":{"achieved":{"to":"finish","effect":"advance"},
                "not-achieved":{"to":"finish","effect":"advance"},
                "superseded":{"to":"finish","effect":"advance"}}},
        {"id":"finish","use":"finish","terminal":true}]},
    "manual_rework":{"allowed_targets":["source-r"]}}"#;

/// A goal-assessment decision without `evidence_from` and without a recorded
/// item, so both citable lists are empty.
const EMPTY_PROCEDURE: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide010-empty","version":"2",
    "name":"Guide010 empty lists","purpose":"Exercise empty generation-2 citation lists.",
    "goal_tracking":true,
    "node_definitions":{
        "work":{"type":"action","title":"Produce evidence","intent":"Record the work result.",
            "items":[{"id":"result","type":"text","prompt":"Record the result.","required":true,"max_length":1000}]},
        "assess":{"type":"decision","title":"Assess the goal",
            "objective":"Select the outcome determined by the recorded criteria.",
            "prompt":"Which goal outcome applies?",
            "items":[{"id":"note","type":"text","prompt":"Record local evidence.","required":false,"max_length":100}],
            "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},
                {"id":"superseded","label":"Superseded"}],
            "reason":{"required":true},"assessment":{"target":"session_goal","outcomes":{
                "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"}}},
        "finish":{"type":"action","title":"Finish","intent":"Finish."}},
    "graph":{"entry":"perform","nodes":[
        {"id":"perform","use":"work","next":"decide"},
        {"id":"decide","use":"assess","routes":{"achieved":{"to":"finish","effect":"advance"},
            "not-achieved":{"to":"finish","effect":"advance"},
            "superseded":{"to":"finish","effect":"advance"}}},
        {"id":"finish","use":"finish","terminal":true}]},
    "manual_rework":{"allowed_targets":["perform"]}}"#;

/// The GUIDE-006 goal-citation procedure shape: one resolved source and one
/// recorded decision-local item.
struct Guide010Engine {
    temporary: TempDir,
    selector: WorktreeSelectorV1,
    store: DirectStoreV1,
    resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
    ids: Guide010Ids,
}

impl Guide010Engine {
    fn new(path: &str, source: &[u8]) -> Self {
        let (temporary, selector, store, _) = fixture();
        fs::write(temporary.path().join(path), source).unwrap();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let engine = Self {
            temporary,
            selector,
            store,
            resolver,
            ids: Guide010Ids::new(),
        };
        engine.start_session(path, source, "guide010-start", &|_| {});
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
                    task_title: "Guide010 generation-2 details".into(),
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
            request_id: RequestIdV1::new(GUIDE010_REQUEST_ID).unwrap(),
            command,
            idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new(GUIDE010_OBSERVED_AT).unwrap(),
            timeout_ms: 5000,
            generation: ContractGenerationV1::Generation1,
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

    fn worktree_root(&self) -> String {
        fs::canonicalize(self.temporary.path())
            .unwrap()
            .display()
            .to_string()
    }

    fn fences(&self) -> SessionMutationPreconditionsWireV1 {
        let graph = self.graph();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
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

    fn begin<F: Fn(&mut DirectRequestV1)>(&self, key: &str, adjust: &F) {
        let session_id = self.graph().trace().session_id().clone();
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal: Some(InitialGoalWireV2 {
                        goal: "Verify the retained goal.".into(),
                        criteria: vec![GoalCriterionWireV2 {
                            criterion_id: CriterionId::new("correct").unwrap(),
                            statement: "The result is correct.".into(),
                        }],
                        actor: Some("owner".into()),
                    }),
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

    fn decide<F: Fn(&mut DirectRequestV1)>(&self, option: &str, key: &str, adjust: &F) {
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
                SessionDecideV2 {
                    option_id: OptionId::new(option).unwrap(),
                    reason: "Choose the main branch.".into(),
                    actor: Some("owner".into()),
                    preconditions: self.fences(),
                    expected_goal_revision: None,
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

    fn record_all<F: Fn(&mut DirectRequestV1)>(&self, key: &str, adjust: &F) {
        let operations: Vec<ItemRecordManyOperationV1> = CAP_ITEMS
            .iter()
            .map(|item| ItemRecordManyOperationV1 {
                item_id: ItemId::new(*item).unwrap(),
                expected_item_revision: Revision::ZERO,
                disposition: ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Text {
                        value: "recorded".into(),
                    },
                },
            })
            .collect();
        self.success(
            DirectCommandV1::RecordMany(ItemRecordManyV1 {
                operations,
                preconditions: self.fences(),
            }),
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

    fn skip<F: Fn(&mut DirectRequestV1)>(&self, key: &str, adjust: &F) {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Skip(SessionSkipV1 {
                reason: Some("Side work is not needed.".into()),
                preconditions: self.fences(),
            })),
            key,
            adjust,
        );
    }

    /// One rejected `goal.assess_criterion` request; the rejection renders
    /// with the request's own contract generation.
    fn assess_failure<F: Fn(&mut DirectRequestV1)>(
        &self,
        evidence: Vec<GraphNodeId>,
        items: Vec<ItemId>,
        key: &str,
        adjust: &F,
    ) -> Value {
        let expected_goal_revision = self
            .graph()
            .goal_state()
            .current_revision()
            .map_or(1, |revision| revision.get());
        self.failure(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriterion(
                GoalAssessCriterionV2 {
                    criterion_id: CriterionId::new("correct").unwrap(),
                    status: "satisfied".into(),
                    reason: "The citation does not name citable state.".into(),
                    evidence,
                    items,
                    actor: Some("owner".into()),
                    preconditions: self.fences(),
                    expected_goal_revision,
                },
            )),
            key,
            adjust,
        )
    }
}

fn select_generation2(request: &mut DirectRequestV1) {
    request.generation = ContractGenerationV1::Generation2;
}

fn keep_generation1(request: &mut DirectRequestV1) {
    request.generation = ContractGenerationV1::Generation1;
}

fn assert_generation2_citation_details(envelope: &Value, nodes: Value, items: Value) {
    assert_eq!(envelope["code"], json!("CRITERION_CITATION_INVALID"));
    assert_eq!(
        envelope["details"]["schema"],
        json!(GENERATION2_CITATION_SCHEMA)
    );
    assert_eq!(envelope["details"]["kind"], Value::Null);
    assert_eq!(envelope["details"]["citable_evidence_source_nodes"], nodes);
    assert_eq!(envelope["details"]["citable_item_ids"], items);
}

fn assert_generation1_citation_details(envelope: &Value) {
    assert_eq!(envelope["code"], json!("CRITERION_CITATION_INVALID"));
    assert_eq!(
        envelope["details"]["schema"],
        json!(GENERATION1_CITATION_SCHEMA)
    );
    assert_eq!(
        envelope["details"]["kind"],
        json!("CRITERION_CITATION_INVALID")
    );
    assert!(
        envelope["details"]
            .get("citable_evidence_source_nodes")
            .is_none()
    );
    assert!(envelope["details"].get("citable_item_ids").is_none());
}

fn render_writer_mismatch(actual: &str, generation: ContractGenerationV1) -> Value {
    let error = DirectExecutionErrorV1::Store(DirectStoreErrorV1::WriterMismatch {
        expected: podway_core::Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
        actual: actual.to_owned(),
    });
    let context = DirectErrorContextV1 {
        request_id: RequestIdV1::new(GUIDE010_REQUEST_ID).unwrap(),
        command: CommandNameV1::new("session.status").unwrap(),
        observed_at: Rfc3339MillisV1::new(GUIDE010_OBSERVED_AT).unwrap(),
        idempotency_key: None,
        timeout_ms: 5000,
        generation,
        maintenance: None,
    };
    render_direct_error_v1(&error, &context)
        .unwrap_or_else(|failure| panic!("the mismatch must validate: {failure:?}"))
        .into_value()
}

#[test]
fn generation2_citation_details_reach_both_caps_in_declaration_order() {
    let engine = Guide010Engine::new("cap.json", &cap_procedure());
    engine.begin("guide010-begin", &select_generation2);
    for index in 0..CAP_SOURCES.len() {
        engine.complete(&format!("guide010-src-{index}"), &select_generation2);
    }
    engine.record_all("guide010-record", &select_generation2);
    let expected_nodes: Vec<&str> = CAP_SOURCES.to_vec();
    let expected_items: Vec<&str> = CAP_ITEMS.to_vec();

    // Per-citation path: an Evidence citation of a node that is not one of
    // the consumer's sources, so the admission lookup cannot match it.
    let rejected = engine.assess_failure(
        vec![GraphNodeId::new("cap-absent").unwrap()],
        Vec::new(),
        "guide010-rejected",
        &select_generation2,
    );
    assert_generation2_citation_details(&rejected, json!(expected_nodes), json!(expected_items));
    assert_eq!(
        rejected["details"]["citation"],
        json!({"reference_graph_node_id": "cap-absent"})
    );

    // An undeclared item citation also reaches the admission path with the
    // same complete lists.
    let undeclared = engine.assess_failure(
        Vec::new(),
        vec![ItemId::new("cap-absent").unwrap()],
        "guide010-undeclared",
        &select_generation2,
    );
    assert_generation2_citation_details(&undeclared, json!(expected_nodes), json!(expected_items));
    assert_eq!(
        undeclared["details"]["citation"],
        json!({"local_item_id": "cap-absent"})
    );

    // The duplicate-detection failure path of the Core rule is not reachable:
    // the typed Direct Engine rejects a duplicated citation as exit-2
    // `REQUEST_INVALID` before Core admission (`direct_procedure.rs`), and
    // `CriterionAssessmentResultV2::new` refuses duplicates by construction,
    // so the Core carrier test owns the Core-level proof.
    let duplicated = engine.assess_failure(
        vec![
            GraphNodeId::new("cap-src-b").unwrap(),
            GraphNodeId::new("cap-src-b").unwrap(),
        ],
        Vec::new(),
        "guide010-duplicated",
        &select_generation2,
    );
    assert_eq!(duplicated["code"], json!("REQUEST_INVALID"));
    assert_eq!(duplicated["exit_code"], json!(2));
}

#[test]
fn generation2_citation_lists_exclude_unresolved_skipped_duplicate_and_unrecorded() {
    let engine = Guide010Engine::new("exclusions.json", EXCLUSION_PROCEDURE);
    engine.begin("guide010-begin", &select_generation2);
    engine.decide("main", "guide010-decide", &select_generation2);
    engine.set(
        "result",
        "Evidence supports this result.",
        "guide010-set",
        &select_generation2,
    );
    engine.complete("guide010-complete", &select_generation2);
    engine.skip("guide010-skip", &select_generation2);
    engine.set(
        "keep",
        "Kept decision-local evidence.",
        "guide010-keep",
        &select_generation2,
    );

    // The resolved source appears exactly once despite its duplicate
    // declaration; the skipped and unvisited sources and the unrecorded
    // declared item are excluded, and citing each excluded state still fails
    // on the per-citation path with the same generation-2 details.
    let expected_nodes = json!(["source-r"]);
    let expected_items = json!(["keep"]);
    let unrecorded = engine.assess_failure(
        Vec::new(),
        vec![ItemId::new("drop").unwrap()],
        "guide010-unrecorded",
        &select_generation2,
    );
    assert_generation2_citation_details(
        &unrecorded,
        expected_nodes.clone(),
        expected_items.clone(),
    );
    assert_eq!(
        unrecorded["details"]["citation"],
        json!({"local_item_id": "drop"})
    );
    for (source, key) in [
        ("lone", "guide010-unresolved"),
        ("source-s", "guide010-skipped"),
    ] {
        let envelope = engine.assess_failure(
            vec![GraphNodeId::new(source).unwrap()],
            Vec::new(),
            key,
            &select_generation2,
        );
        assert_generation2_citation_details(
            &envelope,
            expected_nodes.clone(),
            expected_items.clone(),
        );
        assert_eq!(
            envelope["details"]["citation"],
            json!({"reference_graph_node_id": source})
        );
    }
}

#[test]
fn generation2_citation_lists_can_be_empty() {
    let engine = Guide010Engine::new("empty.json", EMPTY_PROCEDURE);
    engine.begin("guide010-begin", &select_generation2);
    engine.set(
        "result",
        "Evidence supports this result.",
        "guide010-set",
        &select_generation2,
    );
    engine.complete("guide010-complete", &select_generation2);
    let envelope = engine.assess_failure(
        Vec::new(),
        vec![ItemId::new("note").unwrap()],
        "guide010-empty",
        &select_generation2,
    );
    assert_generation2_citation_details(&envelope, json!([]), json!([]));
    assert_eq!(
        envelope["details"]["citation"],
        json!({"local_item_id": "note"})
    );
    // Error envelopes carry no workspace member, so the worktree root never
    // leaks into the generation-2 details.
    assert!(
        !serde_json::to_string(&envelope)
            .unwrap()
            .contains(&engine.worktree_root())
    );
}

#[test]
fn generation1_citation_details_keep_the_frozen_shape() {
    let cap = Guide010Engine::new("cap.json", &cap_procedure());
    cap.begin("guide010-begin", &keep_generation1);
    for index in 0..CAP_SOURCES.len() {
        cap.complete(&format!("guide010-src-{index}"), &keep_generation1);
    }
    cap.record_all("guide010-record", &keep_generation1);
    let rejected = cap.assess_failure(
        vec![GraphNodeId::new("cap-absent").unwrap()],
        Vec::new(),
        "guide010-rejected",
        &keep_generation1,
    );
    assert_generation1_citation_details(&rejected);

    let exclusions = Guide010Engine::new("exclusions.json", EXCLUSION_PROCEDURE);
    exclusions.begin("guide010-begin", &|_| {});
    exclusions.decide("main", "guide010-decide", &|_| {});
    exclusions.set(
        "result",
        "Evidence supports this result.",
        "guide010-set",
        &|_| {},
    );
    exclusions.complete("guide010-complete", &|_| {});
    exclusions.skip("guide010-skip", &|_| {});
    let unrecorded = exclusions.assess_failure(
        Vec::new(),
        vec![ItemId::new("drop").unwrap()],
        "guide010-unrecorded",
        &|_| {},
    );
    assert_generation1_citation_details(&unrecorded);
}

#[test]
fn generation2_writer_mismatch_maps_every_released_and_unknown_digest() {
    for (release, digest) in RELEASED_CONTRACT_MANIFESTS_V1 {
        let envelope = render_writer_mismatch(digest, ContractGenerationV1::Generation2);
        assert_eq!(envelope["code"], json!("WORKSPACE_CONTRACT_MISMATCH"));
        assert_eq!(
            envelope["details"]["schema"],
            json!(GENERATION2_MISMATCH_SCHEMA)
        );
        assert_eq!(
            envelope["details"]["actual_contract_manifest_digest"],
            json!(digest)
        );
        assert_eq!(
            envelope["details"]["actual_product_version"],
            json!(release)
        );
        let reason = format!(
            "Inspect writer identity. Restore access or use the original {release} writer. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path."
        );
        assert!(
            reason.chars().count() <= 256,
            "{release} reason exceeds the recovery bound"
        );
        assert_eq!(envelope["details"]["recovery"]["reason"], json!(reason));
    }

    let unknown = format!("sha256:{}", "f".repeat(64));
    let envelope = render_writer_mismatch(&unknown, ContractGenerationV1::Generation2);
    assert_eq!(
        envelope["details"]["schema"],
        json!(GENERATION2_MISMATCH_SCHEMA)
    );
    assert_eq!(envelope["details"]["actual_product_version"], Value::Null);
    assert_eq!(
        envelope["details"]["recovery"]["reason"],
        json!(
            "Inspect writer identity. Restore access or use the original writer matching the stored digest. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path."
        )
    );

    let generation1 = render_writer_mismatch(&unknown, ContractGenerationV1::Generation1);
    assert_eq!(
        generation1["details"]["schema"],
        json!(GENERATION1_MISMATCH_SCHEMA)
    );
    assert!(
        generation1["details"]
            .get("actual_product_version")
            .is_none()
    );
    assert_eq!(
        generation1["details"]["recovery"]["reason"],
        json!(GENERATION1_MISMATCH_REASON)
    );
}

#[test]
fn generation2_details_from_the_guide006_scenarios() {
    let mismatch = guide006_workspace_contract_mismatch(&select_generation2);
    let envelope: Value = serde_json::from_str(&mismatch.raw).unwrap();
    assert_eq!(envelope["code"], json!("WORKSPACE_CONTRACT_MISMATCH"));
    assert_eq!(
        envelope["details"]["schema"],
        json!(GENERATION2_MISMATCH_SCHEMA)
    );
    assert_eq!(
        envelope["details"]["actual_contract_manifest_digest"],
        json!(format!("sha256:{}", "f".repeat(64)))
    );
    assert_eq!(envelope["details"]["actual_product_version"], Value::Null);
    assert_eq!(
        envelope["details"]["recovery"]["reason"],
        json!(
            "Inspect writer identity. Restore access or use the original writer matching the stored digest. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path."
        )
    );

    let citation = guide006_criterion_citation_invalid(&select_generation2);
    let envelope: Value = serde_json::from_str(&citation.raw).unwrap();
    assert_eq!(envelope["code"], json!("CRITERION_CITATION_INVALID"));
    assert_generation2_citation_details(&envelope, json!(["perform"]), json!(["assessment-note"]));
    assert_eq!(
        envelope["details"]["citation"],
        json!({"local_item_id": "unrecorded-note"})
    );
}
