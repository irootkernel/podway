//! GUIDE-009: generation-2 observation windows and explicit generation-1
//! equality against the frozen GUIDE-006 baselines.
//!
//! The frozen GUIDE-006 scenario helpers carry an `adjust` hook that runs on
//! every Engine request, so an explicit generation reaches the observe read
//! without changing any helper signature. The two terminal states the frozen
//! scenarios do not cover (completed with a disposition, cancelled without
//! one) are built by the local harness below from the same deterministic
//! fixture and identifier sources.

use std::fs;

use podway_core::{ItemId, Revision};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    direct_items::DirectItemCommandV1,
    execution::prepare_workspace_procedure_source_v2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{NativeGitResolverV1, WorktreeSelectorV1};
use podway_protocol::{
    ContractGenerationV1, ItemCheckV1, ItemMutationPreconditionsWireV1,
    ProcedureV2MutationCommandV1, RequestIdV1, Rfc3339MillisV1, SessionBeginV1, SessionCancelV1,
    SessionCompleteV1, SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1,
    SessionStartCommandV1, SessionStartSourceV1, SessionStartV1, SessionTerminalDispositionV1,
    TerminalDispositionInputV1, validate_daemon_free_document_v1,
};
use podway_store::{GraphSessionStateV2, IdempotencyKeyV1, direct_store::DirectStoreV1};
use serde_json::Value;
use tempfile::TempDir;

use crate::int_guide006_generation1_baselines::{
    Guide006EngineActual, Guide006Ids, guide006_criterion_citation_invalid,
    guide006_engine_baseline, guide006_observe_cancelled_with_disposition,
    guide006_observe_completed_without_disposition, guide006_observe_delegated_prepared,
    guide006_observe_prepared, guide006_observe_running,
    guide006_observe_running_dense_truncated_window, guide006_workspace_contract_mismatch,
};
use crate::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

fn select_generation(request: &mut DirectRequestV1, generation: ContractGenerationV1) {
    request.generation = generation;
}

fn select_generation1(request: &mut DirectRequestV1) {
    select_generation(request, ContractGenerationV1::Generation1);
}

fn select_generation2(request: &mut DirectRequestV1) {
    select_generation(request, ContractGenerationV1::Generation2);
}

fn envelope_of(actual: &Guide006EngineActual) -> Value {
    serde_json::from_str(&actual.raw).expect("the Engine actual is one envelope")
}

/// Asserts the v6 family, protocol validity (schema plus the GUIDE-007
/// generation correlations, including R1-R4).
fn assert_v6_document(envelope: &Value) -> &Value {
    let result = &envelope["result"];
    assert_eq!(result["schema"], "podway.observation-result/v6");
    assert!(
        validate_daemon_free_document_v1("podway.observation-result/v6", result),
        "the v6 observation must satisfy its contract and correlations"
    );
    result
}

/// Asserts one lifecycle template: command, authorization class, the exact
/// semantic argv prefix, and the R1 fence tail built from the preconditions.
fn assert_lifecycle_template(template: &Value, command: &str, prefix: &[&str], authorized: bool) {
    assert_eq!(template["command"], command, "{template}");
    assert_eq!(template["requires_explicit_authorization"], authorized);
    assert_eq!(template["authority"], "optimistic_concurrency_only");
    assert_eq!(template["idempotency_key_required"], true);
    assert!(template.get("stdin_template").is_none());
    let argv: Vec<&str> = template["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(argv.len(), prefix.len() + 8, "{command}: {argv:?}");
    assert_eq!(&argv[..prefix.len()], prefix, "{command}");
    let revision = template["preconditions"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let tail: Vec<&str> = argv[prefix.len()..].to_vec();
    let expected: Vec<&str> = vec![
        "--if-workspace-uuid",
        template["preconditions"]["workspace_uuid"]
            .as_str()
            .unwrap(),
        "--if-session-id",
        template["preconditions"]["session_id"].as_str().unwrap(),
        "--if-session-revision",
        revision.as_str(),
        "--idempotency-key",
        "<idempotency-key>",
    ];
    assert_eq!(tail, expected, "{command}");
}

/// R4: every lifecycle template's preconditions equal the status session.
fn assert_preconditions_equal_status_session(result: &Value) {
    for template in result["mutation_templates"].as_array().unwrap() {
        assert_eq!(
            template["preconditions"]["session_id"],
            result["status"]["session"]["id"]
        );
        assert_eq!(
            template["preconditions"]["session_revision"],
            result["status"]["session"]["revision"]
        );
    }
}

fn assert_prepared_window(result: &Value) {
    assert_eq!(result["mutation_templates_total"], 4);
    assert_eq!(result["mutation_templates_truncated"], false);
    let templates = result["mutation_templates"].as_array().unwrap();
    assert_eq!(templates.len(), 4);
    assert_lifecycle_template(&templates[0], "session.begin", &["podway", "begin"], false);
    assert_lifecycle_template(&templates[1], "session.reset", &["podway", "reset"], true);
    assert_lifecycle_template(
        &templates[2],
        "session.start_replace",
        &[
            "podway",
            "start",
            "--on-existing",
            "delete",
            "--preset",
            "<preset>",
            "--task",
            "<task>",
        ],
        true,
    );
    assert_lifecycle_template(
        &templates[3],
        "session.start_replace",
        &[
            "podway",
            "start",
            "--on-existing",
            "delete",
            "--procedure",
            "<procedure-file>",
            "--expect-procedure-digest",
            "<procedure-digest>",
            "--task",
            "<task>",
        ],
        true,
    );
    assert_preconditions_equal_status_session(result);
}

fn assert_undisposed_terminal_window(result: &Value) {
    assert_eq!(result["mutation_templates_total"], 2);
    assert_eq!(result["mutation_templates_truncated"], false);
    let templates = result["mutation_templates"].as_array().unwrap();
    assert_eq!(templates.len(), 2);
    assert_lifecycle_template(
        &templates[0],
        "session.terminal_disposition",
        &[
            "podway",
            "disposition",
            "handed-off",
            "--summary",
            "<summary>",
            "--reference",
            "<reference>",
        ],
        false,
    );
    assert_lifecycle_template(
        &templates[1],
        "session.terminal_disposition",
        &[
            "podway",
            "disposition",
            "not-required",
            "--reason",
            "<reason>",
        ],
        false,
    );
    assert_preconditions_equal_status_session(result);
}

fn assert_disposed_terminal_window(result: &Value) {
    assert_eq!(result["mutation_templates_total"], 4);
    assert_eq!(result["mutation_templates_truncated"], false);
    let templates = result["mutation_templates"].as_array().unwrap();
    assert_eq!(templates.len(), 4);
    assert_lifecycle_template(
        &templates[0],
        "session.archive",
        &["podway", "archive"],
        true,
    );
    assert_lifecycle_template(&templates[1], "session.reset", &["podway", "reset"], true);
    assert_lifecycle_template(
        &templates[2],
        "session.start",
        &[
            "podway", "start", "--preset", "<preset>", "--task", "<task>",
        ],
        true,
    );
    assert_lifecycle_template(
        &templates[3],
        "session.start",
        &[
            "podway",
            "start",
            "--procedure",
            "<procedure-file>",
            "--expect-procedure-digest",
            "<procedure-digest>",
            "--task",
            "<task>",
        ],
        true,
    );
    assert_preconditions_equal_status_session(result);
}

#[test]
fn generation2_prepared_window_offers_four_fenced_lifecycle_templates() {
    let actual = guide006_observe_prepared(&select_generation2);
    let envelope = envelope_of(&actual);
    let result = assert_v6_document(&envelope);
    assert_prepared_window(result);
}

#[test]
fn generation2_undisposed_terminal_window_offers_both_disposition_templates() {
    let actual = guide006_observe_completed_without_disposition(&select_generation2);
    let envelope = envelope_of(&actual);
    let result = assert_v6_document(&envelope);
    assert_undisposed_terminal_window(result);
}

#[test]
fn generation2_disposed_terminal_window_offers_archive_reset_and_fenced_plain_starts() {
    let actual = guide006_observe_cancelled_with_disposition(&select_generation2);
    let envelope = envelope_of(&actual);
    let result = assert_v6_document(&envelope);
    assert_disposed_terminal_window(result);
}

#[test]
fn explicit_generation1_equals_the_frozen_generation1_baselines() {
    for (name, actual) in [
        (
            "guide006_observation_v5_prepared_baseline",
            guide006_observe_prepared(&select_generation1),
        ),
        (
            "guide006_observation_v5_running_baseline",
            guide006_observe_running(&select_generation1),
        ),
        (
            "guide006_observation_v5_running_dense_truncated_window_baseline",
            guide006_observe_running_dense_truncated_window(&select_generation1),
        ),
        (
            "guide006_observation_v5_completed_without_disposition_baseline",
            guide006_observe_completed_without_disposition(&select_generation1),
        ),
        (
            "guide006_observation_v5_cancelled_with_disposition_baseline",
            guide006_observe_cancelled_with_disposition(&select_generation1),
        ),
        (
            "guide006_observation_v5_delegated_prepared_baseline",
            guide006_observe_delegated_prepared(&select_generation1),
        ),
        (
            "guide006_criterion_citation_invalid_details_baseline",
            guide006_criterion_citation_invalid(&select_generation1),
        ),
        (
            "guide006_workspace_contract_mismatch_details_baseline",
            guide006_workspace_contract_mismatch(&select_generation1),
        ),
    ] {
        assert_eq!(
            actual.baseline_text(),
            guide006_engine_baseline(name),
            "explicit generation 1 must equal the frozen baseline {name}"
        );
    }
}

#[test]
fn generation2_running_dense_window_matches_the_generation1_baseline() {
    let actual = guide006_observe_running_dense_truncated_window(&select_generation2);
    // The comparison runs through the GUIDE-006 normalization, like the frozen
    // baseline itself, so only the worktree root is replaced.
    let envelope: Value = serde_json::from_str(&actual.normalized()).unwrap();
    let result = assert_v6_document(&envelope);
    assert_eq!(result["status"]["session"]["lifecycle"], "running");
    let baseline: Value = serde_json::from_str(guide006_engine_baseline(
        "guide006_observation_v5_running_dense_truncated_window_baseline",
    ))
    .unwrap();
    assert_eq!(baseline["result"]["schema"], "podway.observation-result/v5");
    // Running windows stay identical to v5, byte for byte, after the same
    // normalization the frozen baseline carries.
    assert_eq!(
        result["mutation_templates"],
        baseline["result"]["mutation_templates"]
    );
    assert_eq!(
        result["mutation_templates_total"],
        baseline["result"]["mutation_templates_total"]
    );
    assert_eq!(
        result["mutation_templates_truncated"],
        baseline["result"]["mutation_templates_truncated"]
    );
    assert_eq!(result["active_items"], baseline["result"]["active_items"]);
    assert_eq!(
        result["active_items_total"],
        baseline["result"]["active_items_total"]
    );
    assert_eq!(
        result["active_items_truncated"],
        baseline["result"]["active_items_truncated"]
    );
}

/// One action node with a required confirm, the same shape the frozen
/// scenarios use, so `complete` is reachable from the local harness.
const WORK: &[u8] = br#"{"schema":"podway.procedure/v2","id":"guide009-work","version":"1",
    "name":"Guide009 work","purpose":"Exercise the remaining terminal states.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work.",
        "items":[{"id":"confirm","type":"confirm","prompt":"Confirm.","required":true}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

/// Local harness for the two terminal states the frozen scenarios do not
/// build: cancelled without a disposition and completed with one.
struct Guide009Engine {
    _temporary: TempDir,
    selector: WorktreeSelectorV1,
    store: DirectStoreV1,
    resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
    ids: Guide006Ids,
}

impl Guide009Engine {
    fn new() -> Self {
        let (temporary, selector, store, _) = fixture();
        fs::write(temporary.path().join("work.json"), WORK).unwrap();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let engine = Self {
            _temporary: temporary,
            selector,
            store,
            resolver,
            ids: Guide006Ids::new(),
        };
        engine.start("guide009-start");
        engine
    }

    fn request(&self, command: DirectCommandV1, key: &str) -> DirectRequestV1 {
        DirectRequestV1 {
            selector: self.selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000616").unwrap(),
            command,
            idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
            timeout_ms: 5000,
            generation: ContractGenerationV1::Generation1,
            return_observation: false,
        }
    }

    fn execute(&self, command: DirectCommandV1, key: &str) -> DirectResponseV1 {
        execute_direct_v1(
            &self.resolver,
            self.request(command, key),
            &Clock,
            &self.ids,
        )
        .unwrap()
    }

    fn success(&self, command: DirectCommandV1, key: &str) -> Value {
        match self.execute(command, key) {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => {
                panic!("unexpected Engine rejection: {}", error.into_value())
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

    fn start(&self, key: &str) {
        let admitted = prepare_workspace_procedure_source_v2("work.json", WORK).unwrap();
        self.success(
            DirectCommandV1::Start(podway_engine::direct_start::DirectStartCommandV1::Start(
                SessionStartCommandV1 {
                    start: SessionStartV1 {
                        source: SessionStartSourceV1::Procedure {
                            procedure: "work.json".into(),
                        },
                        expected_procedure_digest: Some(admitted.digest().clone()),
                        task_title: "Guide009 terminal matrix".into(),
                        dry_run: false,
                    },
                    preconditions: None,
                },
            )),
            key,
        );
    }

    fn begin(&self, key: &str) {
        let session_id = self.graph().trace().session_id().clone();
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal: None,
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: session_id,
                        expected_session_revision: Revision::ZERO,
                    },
                },
            )),
            key,
        );
    }

    fn check_confirm(&self, key: &str) {
        let graph = self.graph();
        let attempt = graph.trace().active_attempt().unwrap().attempt_id().clone();
        let item = ItemId::new("confirm").unwrap();
        let revision = graph
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == &attempt)
            .unwrap()
            .item_slots()
            .iter()
            .find(|slot| slot.item_id() == &item)
            .unwrap()
            .revision();
        self.success(
            DirectCommandV1::Item(DirectItemCommandV1::Check(ItemCheckV1 {
                item_id: item,
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: graph.trace().session_id().clone(),
                    expected_attempt_id: attempt,
                    expected_item_revision: revision,
                },
            })),
            key,
        );
    }

    fn mutation_fences(&self) -> SessionMutationPreconditionsWireV1 {
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

    fn complete(&self, key: &str) {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: self.mutation_fences(),
            })),
            key,
        );
    }

    fn cancel(&self, key: &str) {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
                reason: "Cancelled for the generation-2 terminal matrix.".into(),
                preconditions: self.mutation_fences(),
            })),
            key,
        );
    }

    fn disposition_not_required(&self, key: &str) {
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                SessionTerminalDispositionV1 {
                    disposition: TerminalDispositionInputV1::NotRequired {
                        reason: "No external hand-off required.".into(),
                    },
                    actor: None,
                    preconditions: self.identity_fences(),
                },
            )),
            key,
        );
    }

    fn observe(&self, generation: ContractGenerationV1) -> Value {
        let session_id = self.graph().trace().session_id().clone();
        let mut request = self.request(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::Observe,
                expected_session_id: Some(session_id),
            },
            "guide009-observe",
        );
        select_generation(&mut request, generation);
        match execute_direct_v1(&self.resolver, request, &Clock, &self.ids).unwrap() {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => {
                panic!("unexpected Engine rejection: {}", error.into_value())
            }
        }
    }
}

#[test]
fn generation2_terminal_matrix_covers_completed_and_cancelled_with_and_without_disposition() {
    let cancelled_without = Guide009Engine::new();
    cancelled_without.begin("guide009-begin");
    cancelled_without.cancel("guide009-cancel");
    let envelope = cancelled_without.observe(ContractGenerationV1::Generation2);
    let result = assert_v6_document(&envelope);
    assert_eq!(result["status"]["session"]["lifecycle"], "cancelled");
    assert_undisposed_terminal_window(result);

    let completed_with = Guide009Engine::new();
    completed_with.begin("guide009-begin");
    completed_with.check_confirm("guide009-check-confirm");
    completed_with.complete("guide009-complete");
    completed_with.disposition_not_required("guide009-disposition");
    let envelope = completed_with.observe(ContractGenerationV1::Generation2);
    let result = assert_v6_document(&envelope);
    assert_eq!(result["status"]["session"]["lifecycle"], "completed");
    assert_disposed_terminal_window(result);
}

const AGENT005_PROCEDURE: &[u8] = br#"schema: podway.procedure/v2
id: agent005-observation
version: "1"
name: AGENT-005 observation lifecycle
purpose: Exercise all six commit-bound observation routes.
description: AGENT-005 test procedure.
goal_tracking: true
node_definitions:
  work:
    type: action
    title: Work
    intent: Record items before completing into the decision node.
    instructions:
      - Record items.
    items:
      - id: confirm
        type: confirm
        prompt: Confirm work.
        required: true
      - id: artifact
        type: artifact
        prompt: Attach artifact.
        required: false
  gate:
    type: decision
    title: Gate
    objective: Determine the goal outcome.
    prompt: Which outcome applies?
    options:
      - id: ship
        label: Ship
      - id: hold
        label: Hold
      - id: drop
        label: Drop
    reason:
      required: true
      prompt: Explain the assessment.
    assessment:
      target: session_goal
      outcomes:
        ship: achieved
        hold: not_achieved
        drop: superseded
  finish:
    type: action
    title: Finish
    intent: Complete the terminal node.
    instructions:
      - Complete finish.
    items:
      - id: done
        type: confirm
        prompt: Confirm finish.
        required: false
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: gate
    - id: gate
      use: gate
      evidence_from:
        - node: work
          required: true
          items:
            - confirm
      routes:
        ship:
          to: finish
          effect: advance
        hold:
          to: finish
          effect: advance
        drop:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - work
"#;

struct Agent005Ids {
    next: std::sync::atomic::AtomicU64,
}

impl Agent005Ids {
    fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn uuid(&self, prefix: u16) -> String {
        let value = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("{prefix:04x}0000-0000-4000-8000-{value:012x}")
    }
}

impl podway_engine::direct_execution::DirectIdSourceV1 for Agent005Ids {
    fn next_workspace_id(&self) -> podway_core::WorkspaceId {
        podway_core::WorkspaceId::new(self.uuid(1)).unwrap()
    }
    fn next_session_id(&self) -> podway_core::SessionId {
        podway_core::SessionId::new(self.uuid(2)).unwrap()
    }
    fn next_attempt_id(&self) -> podway_core::AttemptId {
        podway_core::AttemptId::new(self.uuid(3)).unwrap()
    }
    fn next_blocker_id(&self) -> podway_core::BlockerId {
        podway_core::BlockerId::new(self.uuid(4)).unwrap()
    }
    fn next_procedure_snapshot_id(&self) -> podway_core::ProcedureSnapshotId {
        podway_core::ProcedureSnapshotId::new(self.uuid(5)).unwrap()
    }
    fn next_delegation_receipt_id(&self) -> podway_core::DelegationReceiptIdV1 {
        podway_core::DelegationReceiptIdV1::new(self.uuid(6)).unwrap()
    }
}

struct Agent005Harness {
    temporary: TempDir,
    selector: WorktreeSelectorV1,
    store: DirectStoreV1,
    resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
    ids: Agent005Ids,
}

impl Agent005Harness {
    fn new() -> Self {
        let (temporary, selector, store, _) = fixture();
        fs::write(temporary.path().join("agent005.yaml"), AGENT005_PROCEDURE).unwrap();
        fs::write(temporary.path().join("artifact.txt"), b"agent005-artifact").unwrap();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        Self {
            temporary,
            selector,
            store,
            resolver,
            ids: Agent005Ids::new(),
        }
    }

    fn sequence(&self) -> u64 {
        self.store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap()
    }

    fn graph(&self) -> GraphSessionStateV2 {
        self.store
            .prepare_graph(deadline())
            .unwrap()
            .unwrap()
            .graph()
            .clone()
    }

    fn request(
        &self,
        command: DirectCommandV1,
        key: Option<&str>,
        generation: ContractGenerationV1,
        return_observation: bool,
    ) -> DirectRequestV1 {
        DirectRequestV1 {
            selector: self.selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(self.ids.uuid(7)).unwrap(),
            command,
            idempotency_key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
            timeout_ms: 5000,
            generation,
            return_observation,
        }
    }

    fn execute(&self, request: DirectRequestV1) -> DirectResponseV1 {
        execute_direct_v1(&self.resolver, request, &Clock, &self.ids).unwrap()
    }

    fn fresh_observe(&self) -> Value {
        let session_id = self.graph().trace().session_id().clone();
        let request = self.request(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::Observe,
                expected_session_id: Some(session_id),
            },
            None,
            ContractGenerationV1::Generation2,
            false,
        );
        match self.execute(request) {
            DirectResponseV1::Success(output) => output.into_value(),
            DirectResponseV1::Failure(error) => {
                panic!("unexpected observe failure: {}", error.into_value())
            }
        }
    }

    fn lookup(&self, key: &str) -> Value {
        let request = self.request(
            DirectCommandV1::OutcomeLookup {
                key: IdempotencyKeyV1::new(key).unwrap(),
            },
            None,
            ContractGenerationV1::Generation2,
            false,
        );
        match self.execute(request) {
            DirectResponseV1::Success(output) => {
                let value = output.into_value();
                assert!(validate_daemon_free_document_v1("podway.output/v4", &value));
                assert!(validate_daemon_free_document_v1(
                    "podway.outcome-lookup-result/v1",
                    &value["result"]
                ));
                value
            }
            DirectResponseV1::Failure(error) => {
                panic!("unexpected lookup failure: {}", error.into_value())
            }
        }
    }

    fn commit_with_observation(
        &self,
        command: DirectCommandV1,
        key: &str,
        generation: ContractGenerationV1,
        expected_inner_schema: &str,
    ) -> Value {
        let before_seq = self.sequence();
        let response = self.execute(self.request(command, Some(key), generation, true));
        let DirectResponseV1::Success(output) = response else {
            panic!("unexpected rejection for {key}: {:?}", response);
        };
        let value = output.into_value();
        let after_seq = self.sequence();
        assert_eq!(after_seq, before_seq + 1);
        assert!(validate_daemon_free_document_v1("podway.output/v4", &value));
        assert!(validate_daemon_free_document_v1(
            "podway.mutation-observation-result/v1",
            &value["result"]
        ));
        assert_eq!(value["workspace"]["latest_workspace_sequence"], after_seq);
        assert_eq!(value["result"]["workspace_sequence"], after_seq);
        assert_eq!(
            value["result"]["mutation_result"]["schema"],
            expected_inner_schema
        );
        assert_eq!(
            value["result"]["observation"]["schema"],
            "podway.observation-result/v6"
        );
        let fresh = self.fresh_observe();
        assert_eq!(value["result"]["observation"], fresh["result"]);

        let looked_up = self.lookup(key);
        assert_eq!(looked_up["result"]["found"], true);
        assert_eq!(looked_up["result"]["outcome"]["response"], value);
        value
    }
}

#[test]
fn agent005_all_six_selected_routes_return_coherent_v6_observation_and_support_replay_and_lookup() {
    use podway_core::{CriterionId, OptionId};
    use podway_protocol::{
        CriterionAssessmentManyEntryWireV1, GoalAssessCriteriaV2, GoalCriterionWireV2,
        InitialGoalWireV2, ItemRecordManyDispositionV1, ItemRecordManyOperationV1,
        ItemRecordManyV1, ItemRecordValueV1, SessionDecideV2,
    };

    let harness = Agent005Harness::new();
    let admitted =
        prepare_workspace_procedure_source_v2("agent005.yaml", AGENT005_PROCEDURE).unwrap();

    // 1. Plain session.start with Generation1 request still embeds v6 observation.
    let start_cmd = DirectCommandV1::Start(
        podway_engine::direct_start::DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "agent005.yaml".into(),
                },
                expected_procedure_digest: Some(admitted.digest().clone()),
                task_title: "AGENT-005 initial start".into(),
                dry_run: false,
            },
            preconditions: None,
        }),
    );
    let start_out = harness.commit_with_observation(
        start_cmd.clone(),
        "agent005-start-1",
        ContractGenerationV1::Generation1,
        "podway.session-start-result/v4",
    );
    assert_eq!(start_out["session"]["revision_before"], 0);
    assert_eq!(start_out["session"]["revision_after"], 0);

    // 2. session.begin with initial goal.
    let session_id = harness.graph().trace().session_id().clone();
    let begin_cmd =
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: Some(InitialGoalWireV2 {
                goal: "Complete AGENT-005 lifecycle.".into(),
                criteria: vec![
                    GoalCriterionWireV2 {
                        criterion_id: CriterionId::new("c1").unwrap(),
                        statement: "First criterion.".into(),
                    },
                    GoalCriterionWireV2 {
                        criterion_id: CriterionId::new("c2").unwrap(),
                        statement: "Second criterion.".into(),
                    },
                ],
                actor: Some("agent005-tester".into()),
            }),
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::ZERO,
            },
        }));
    let begin_out = harness.commit_with_observation(
        begin_cmd.clone(),
        "agent005-begin-1",
        ContractGenerationV1::Generation2,
        "podway.session-begin-result/v2",
    );
    assert_eq!(begin_out["session"]["revision_before"], 0);
    assert_eq!(begin_out["session"]["revision_after"], 1);

    // 3a. item.record_many (changing, with confirm + artifact).
    let graph = harness.graph();
    let attempt_id = graph.trace().active_attempt().unwrap().attempt_id().clone();
    let record_cmd = DirectCommandV1::RecordMany(ItemRecordManyV1 {
        operations: vec![
            ItemRecordManyOperationV1 {
                item_id: ItemId::new("confirm").unwrap(),
                expected_item_revision: Revision::ZERO,
                disposition: ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Confirm { value: true },
                },
            },
            ItemRecordManyOperationV1 {
                item_id: ItemId::new("artifact").unwrap(),
                expected_item_revision: Revision::ZERO,
                disposition: ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Artifact {
                        path: Some("artifact.txt".into()),
                        reference: None,
                        digest: None,
                        size_bytes: None,
                        media_type: None,
                    },
                },
            },
        ],
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session_id.clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: attempt_id.clone(),
        },
    });
    let record_out = harness.commit_with_observation(
        record_cmd.clone(),
        "agent005-record-many-1",
        ContractGenerationV1::Generation1,
        "podway.item-record-many-result/v2",
    );
    assert_eq!(record_out["session"]["revision_before"], 1);
    assert_eq!(record_out["session"]["revision_after"], 2);
    assert_eq!(record_out["result"]["mutation_result"]["changed"], true);

    // 3b. item.record_many (no-op: confirm already checked at revision 1).
    let noop_record_cmd = DirectCommandV1::RecordMany(ItemRecordManyV1 {
        operations: vec![ItemRecordManyOperationV1 {
            item_id: ItemId::new("confirm").unwrap(),
            expected_item_revision: Revision::new(1),
            disposition: ItemRecordManyDispositionV1::Record {
                record: ItemRecordValueV1::Confirm { value: true },
            },
        }],
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: session_id.clone(),
            expected_session_revision: Revision::new(2),
            expected_attempt_id: attempt_id.clone(),
        },
    });
    let noop_record_out = harness.commit_with_observation(
        noop_record_cmd.clone(),
        "agent005-record-many-noop",
        ContractGenerationV1::Generation2,
        "podway.item-record-many-result/v2",
    );
    assert_eq!(noop_record_out["session"]["revision_before"], 2);
    assert_eq!(noop_record_out["session"]["revision_after"], 2);
    assert_eq!(
        noop_record_out["result"]["mutation_result"]["changed"],
        false
    );

    // 4. session.complete (non-terminal: work -> gate).
    let complete_work_cmd =
        DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(2),
                expected_attempt_id: attempt_id,
            },
        }));
    let complete_work_out = harness.commit_with_observation(
        complete_work_cmd.clone(),
        "agent005-complete-work",
        ContractGenerationV1::Generation2,
        "podway.stage-transition-result/v3",
    );
    assert_eq!(complete_work_out["session"]["revision_before"], 2);
    assert_eq!(complete_work_out["session"]["revision_after"], 3);

    // 5. goal.assess_criteria (batch assessment of c1 and c2).
    let graph = harness.graph();
    let gate_attempt = graph.trace().active_attempt().unwrap().attempt_id().clone();
    let assess_cmd = DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriteria(
        GoalAssessCriteriaV2 {
            expected_goal_revision: 1,
            actor: Some("agent005-tester".into()),
            assessments: vec![
                CriterionAssessmentManyEntryWireV1 {
                    criterion_id: CriterionId::new("c1").unwrap(),
                    status: "satisfied".into(),
                    reason: "Work node completed.".into(),
                    evidence: vec![podway_core::GraphNodeId::new("work").unwrap()],
                    items: vec![],
                },
                CriterionAssessmentManyEntryWireV1 {
                    criterion_id: CriterionId::new("c2").unwrap(),
                    status: "satisfied".into(),
                    reason: "Artifact attached.".into(),
                    evidence: vec![podway_core::GraphNodeId::new("work").unwrap()],
                    items: vec![],
                },
            ],
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(3),
                expected_attempt_id: gate_attempt.clone(),
            },
        },
    ));
    let assess_out = harness.commit_with_observation(
        assess_cmd.clone(),
        "agent005-assess-criteria-1",
        ContractGenerationV1::Generation1,
        "podway.criterion-assessment-many-result/v1",
    );
    assert_eq!(assess_out["session"]["revision_before"], 3);
    assert_eq!(assess_out["session"]["revision_after"], 4);
    assert_eq!(assess_out["result"]["mutation_result"]["complete"], true);

    // 6. session.decide (gate -> finish).
    let decide_cmd = DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
        SessionDecideV2 {
            option_id: OptionId::new("ship").unwrap(),
            reason: "All criteria met.".into(),
            actor: Some("agent005-tester".into()),
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(4),
                expected_attempt_id: gate_attempt,
            },
            expected_goal_revision: Some(1),
        },
    ));
    let decide_out = harness.commit_with_observation(
        decide_cmd.clone(),
        "agent005-decide-1",
        ContractGenerationV1::Generation2,
        "podway.decision-result/v2",
    );
    assert_eq!(decide_out["session"]["revision_before"], 4);
    assert_eq!(decide_out["session"]["revision_after"], 5);

    // 7. Terminal session.complete on finish node.
    let finish_attempt = harness
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let complete_finish_cmd =
        DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(5),
                expected_attempt_id: finish_attempt,
            },
        }));
    let complete_finish_out = harness.commit_with_observation(
        complete_finish_cmd.clone(),
        "agent005-complete-finish",
        ContractGenerationV1::Generation2,
        "podway.stage-transition-result/v3",
    );
    assert_eq!(complete_finish_out["session"]["revision_before"], 5);
    assert_eq!(complete_finish_out["session"]["revision_after"], 6);
    assert_eq!(
        complete_finish_out["result"]["observation"]["status"]["session"]["lifecycle"],
        "completed"
    );

    // Record terminal disposition (flag-absent) so plain start can archive and replace.
    let disp_resp = harness.execute(harness.request(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
            SessionTerminalDispositionV1 {
                disposition: TerminalDispositionInputV1::NotRequired {
                    reason: "Disposed for plain start replacement.".into(),
                },
                actor: None,
                preconditions: SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(6),
                },
            },
        )),
        Some("agent005-disposition"),
        ContractGenerationV1::Generation2,
        false,
    ));
    assert!(matches!(disp_resp, DirectResponseV1::Success(_)));

    // 8. Disposed-terminal plain session.start with --with-observation.
    let disposed_start_cmd = DirectCommandV1::Start(
        podway_engine::direct_start::DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "agent005.yaml".into(),
                },
                expected_procedure_digest: Some(admitted.digest().clone()),
                task_title: "AGENT-005 disposed replacement start".into(),
                dry_run: false,
            },
            preconditions: Some(SessionIdentityPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(6),
            }),
        }),
    );
    let disposed_start_out = harness.commit_with_observation(
        disposed_start_cmd.clone(),
        "agent005-start-disposed",
        ContractGenerationV1::Generation2,
        "podway.session-start-result/v4",
    );
    assert_eq!(disposed_start_out["session"]["revision_before"], 6);
    assert_eq!(disposed_start_out["session"]["revision_after"], 0);
    assert_ne!(disposed_start_out["session"]["id"], session_id.as_str());

    // Delete local procedure file, then verify historical same-key replay
    // across all committed routes returns the stored wrapper without advancing sequence.
    fs::remove_file(harness.temporary.path().join("agent005.yaml")).unwrap();
    let seq_before_replays = harness.sequence();
    for (cmd, key, original) in [
        (
            disposed_start_cmd,
            "agent005-start-disposed",
            &disposed_start_out,
        ),
        (begin_cmd, "agent005-begin-1", &begin_out),
        (record_cmd, "agent005-record-many-1", &record_out),
        (
            noop_record_cmd,
            "agent005-record-many-noop",
            &noop_record_out,
        ),
        (
            complete_work_cmd,
            "agent005-complete-work",
            &complete_work_out,
        ),
        (assess_cmd, "agent005-assess-criteria-1", &assess_out),
        (decide_cmd, "agent005-decide-1", &decide_out),
        (
            complete_finish_cmd,
            "agent005-complete-finish",
            &complete_finish_out,
        ),
    ] {
        let replay = harness.execute(harness.request(
            cmd,
            Some(key),
            ContractGenerationV1::Generation2,
            true,
        ));
        let DirectResponseV1::Success(replay_out) = replay else {
            panic!("replay failed for {key}: {replay:?}");
        };
        let replay_val = replay_out.into_value();
        assert_eq!(replay_val, *original);
    }
    assert_eq!(harness.sequence(), seq_before_replays);
}

#[test]
fn agent005_bidirectional_idempotency_key_conflict_and_unsupported_route_refusal() {
    use podway_core::{CriterionId, OptionId};
    use podway_engine::direct_api::{
        DirectInvocationContextV1, execute_direct_preflight_facts_with_context_v1,
        execute_direct_with_context_v1,
    };
    use podway_engine::direct_execution::DirectPreflightFactsRequestV1;
    use podway_protocol::{
        CriterionAssessmentManyEntryWireV1, GoalAssessCriteriaV2, GoalAssessCriterionV2,
        GoalCriterionWireV2, GoalDefineV2, InitialGoalWireV2, ItemRecordManyDispositionV1,
        ItemRecordManyOperationV1, ItemRecordManyV1, ItemRecordValueV1, SessionDecideV2,
        SessionSkipV1,
    };

    let admitted =
        prepare_workspace_procedure_source_v2("agent005.yaml", AGENT005_PROCEDURE).unwrap();
    let start_cmd = DirectCommandV1::Start(
        podway_engine::direct_start::DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "agent005.yaml".into(),
                },
                expected_procedure_digest: Some(admitted.digest().clone()),
                task_title: "AGENT-005 bidirectional mode conflict".into(),
                dry_run: false,
            },
            preconditions: None,
        }),
    );

    let mut last_harness = None;
    for (initial_mode, conflict_mode, dir_label) in
        [(false, true, "false->true"), (true, false, "true->false")]
    {
        let harness = Agent005Harness::new();
        let DirectResponseV1::Success(start_ok) = harness.execute(harness.request(
            start_cmd.clone(),
            Some("k-start"),
            ContractGenerationV1::Generation2,
            initial_mode,
        )) else {
            panic!("expected initial start success ({dir_label})");
        };
        assert_eq!(
            start_ok.as_value()["result"]["schema"] == "podway.mutation-observation-result/v1",
            initial_mode,
            "start schema ({dir_label})"
        );
        // Delete procedure source before testing mode-conflict reuse on session.start!
        fs::remove_file(harness.temporary.path().join("agent005.yaml")).unwrap();
        let seq_before_start = harness.sequence();
        let rev_before_start = harness.graph().trace().revision();
        let reuse_start = harness.execute(harness.request(
            start_cmd.clone(),
            Some("k-start"),
            ContractGenerationV1::Generation2,
            conflict_mode,
        ));
        let DirectResponseV1::Failure(err) = reuse_start else {
            panic!("expected IDEMPOTENCY_KEY_REUSED on start ({dir_label})");
        };
        assert_eq!(
            err.as_value()["code"],
            "IDEMPOTENCY_KEY_REUSED",
            "start ({dir_label})"
        );
        assert_eq!(err.as_value()["exit_code"], 2, "start ({dir_label})");
        assert_eq!(
            harness.sequence(),
            seq_before_start,
            "start ({dir_label}) seq"
        );
        assert_eq!(
            harness.graph().trace().revision(),
            rev_before_start,
            "start ({dir_label}) rev"
        );

        let session_id = harness.graph().trace().session_id().clone();
        let begin_cmd = DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
            SessionBeginV1 {
                initial_goal: Some(InitialGoalWireV2 {
                    goal: "Test conflict.".into(),
                    criteria: vec![GoalCriterionWireV2 {
                        criterion_id: CriterionId::new("c1").unwrap(),
                        statement: "Criterion 1.".into(),
                    }],
                    actor: None,
                }),
                preconditions: SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::ZERO,
                },
            },
        ));
        let DirectResponseV1::Success(begin_ok) = harness.execute(harness.request(
            begin_cmd.clone(),
            Some("k-begin"),
            ContractGenerationV1::Generation2,
            initial_mode,
        )) else {
            panic!("expected initial begin success ({dir_label})");
        };
        assert_eq!(
            begin_ok.as_value()["result"]["schema"] == "podway.mutation-observation-result/v1",
            initial_mode,
            "begin schema ({dir_label})"
        );
        let seq_before_begin = harness.sequence();
        let rev_before_begin = harness.graph().trace().revision();
        let DirectResponseV1::Failure(err) = harness.execute(harness.request(
            begin_cmd,
            Some("k-begin"),
            ContractGenerationV1::Generation2,
            conflict_mode,
        )) else {
            panic!("expected IDEMPOTENCY_KEY_REUSED on begin ({dir_label})");
        };
        assert_eq!(
            err.as_value()["code"],
            "IDEMPOTENCY_KEY_REUSED",
            "begin ({dir_label})"
        );
        assert_eq!(err.as_value()["exit_code"], 2, "begin ({dir_label})");
        assert_eq!(
            harness.sequence(),
            seq_before_begin,
            "begin ({dir_label}) seq"
        );
        assert_eq!(
            harness.graph().trace().revision(),
            rev_before_begin,
            "begin ({dir_label}) rev"
        );

        let attempt_id = harness
            .graph()
            .trace()
            .active_attempt()
            .unwrap()
            .attempt_id()
            .clone();
        let record_cmd = DirectCommandV1::RecordMany(ItemRecordManyV1 {
            operations: vec![ItemRecordManyOperationV1 {
                item_id: ItemId::new("confirm").unwrap(),
                expected_item_revision: Revision::ZERO,
                disposition: ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Confirm { value: true },
                },
            }],
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(1),
                expected_attempt_id: attempt_id.clone(),
            },
        });
        let DirectResponseV1::Success(record_ok) = harness.execute(harness.request(
            record_cmd.clone(),
            Some("k-record"),
            ContractGenerationV1::Generation2,
            initial_mode,
        )) else {
            panic!("expected initial record_many success ({dir_label})");
        };
        assert_eq!(
            record_ok.as_value()["result"]["schema"] == "podway.mutation-observation-result/v1",
            initial_mode,
            "record_many schema ({dir_label})"
        );
        let seq_before_record = harness.sequence();
        let rev_before_record = harness.graph().trace().revision();
        let DirectResponseV1::Failure(err) = harness.execute(harness.request(
            record_cmd,
            Some("k-record"),
            ContractGenerationV1::Generation2,
            conflict_mode,
        )) else {
            panic!("expected IDEMPOTENCY_KEY_REUSED on record_many ({dir_label})");
        };
        assert_eq!(
            err.as_value()["code"],
            "IDEMPOTENCY_KEY_REUSED",
            "record_many ({dir_label})"
        );
        assert_eq!(err.as_value()["exit_code"], 2, "record_many ({dir_label})");
        assert_eq!(
            harness.sequence(),
            seq_before_record,
            "record_many ({dir_label}) seq"
        );
        assert_eq!(
            harness.graph().trace().revision(),
            rev_before_record,
            "record_many ({dir_label}) rev"
        );

        let complete_cmd =
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(2),
                    expected_attempt_id: attempt_id.clone(),
                },
            }));
        let DirectResponseV1::Success(complete_ok) = harness.execute(harness.request(
            complete_cmd.clone(),
            Some("k-complete"),
            ContractGenerationV1::Generation2,
            initial_mode,
        )) else {
            panic!("expected initial complete success ({dir_label})");
        };
        assert_eq!(
            complete_ok.as_value()["result"]["schema"] == "podway.mutation-observation-result/v1",
            initial_mode,
            "complete schema ({dir_label})"
        );
        let seq_before_complete = harness.sequence();
        let rev_before_complete = harness.graph().trace().revision();
        let DirectResponseV1::Failure(err) = harness.execute(harness.request(
            complete_cmd,
            Some("k-complete"),
            ContractGenerationV1::Generation2,
            conflict_mode,
        )) else {
            panic!("expected IDEMPOTENCY_KEY_REUSED on complete ({dir_label})");
        };
        assert_eq!(
            err.as_value()["code"],
            "IDEMPOTENCY_KEY_REUSED",
            "complete ({dir_label})"
        );
        assert_eq!(err.as_value()["exit_code"], 2, "complete ({dir_label})");
        assert_eq!(
            harness.sequence(),
            seq_before_complete,
            "complete ({dir_label}) seq"
        );
        assert_eq!(
            harness.graph().trace().revision(),
            rev_before_complete,
            "complete ({dir_label}) rev"
        );

        let gate_attempt = harness
            .graph()
            .trace()
            .active_attempt()
            .unwrap()
            .attempt_id()
            .clone();
        let assess_cmd = DirectCommandV1::Procedure(
            ProcedureV2MutationCommandV1::GoalAssessCriteria(GoalAssessCriteriaV2 {
                expected_goal_revision: 1,
                actor: None,
                assessments: vec![CriterionAssessmentManyEntryWireV1 {
                    criterion_id: CriterionId::new("c1").unwrap(),
                    status: "satisfied".into(),
                    reason: "Done.".into(),
                    evidence: vec![podway_core::GraphNodeId::new("work").unwrap()],
                    items: vec![],
                }],
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(3),
                    expected_attempt_id: gate_attempt.clone(),
                },
            }),
        );
        let DirectResponseV1::Success(assess_ok) = harness.execute(harness.request(
            assess_cmd.clone(),
            Some("k-assess"),
            ContractGenerationV1::Generation2,
            initial_mode,
        )) else {
            panic!("expected initial assess_criteria success ({dir_label})");
        };
        assert_eq!(
            assess_ok.as_value()["result"]["schema"] == "podway.mutation-observation-result/v1",
            initial_mode,
            "assess_criteria schema ({dir_label})"
        );
        let seq_before_assess = harness.sequence();
        let rev_before_assess = harness.graph().trace().revision();
        let DirectResponseV1::Failure(err) = harness.execute(harness.request(
            assess_cmd,
            Some("k-assess"),
            ContractGenerationV1::Generation2,
            conflict_mode,
        )) else {
            panic!("expected IDEMPOTENCY_KEY_REUSED on assess_criteria ({dir_label})");
        };
        assert_eq!(
            err.as_value()["code"],
            "IDEMPOTENCY_KEY_REUSED",
            "assess_criteria ({dir_label})"
        );
        assert_eq!(
            err.as_value()["exit_code"],
            2,
            "assess_criteria ({dir_label})"
        );
        assert_eq!(
            harness.sequence(),
            seq_before_assess,
            "assess_criteria ({dir_label}) seq"
        );
        assert_eq!(
            harness.graph().trace().revision(),
            rev_before_assess,
            "assess_criteria ({dir_label}) rev"
        );

        let decide_cmd = DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
            SessionDecideV2 {
                option_id: OptionId::new("ship").unwrap(),
                reason: "Ship.".into(),
                actor: None,
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(4),
                    expected_attempt_id: gate_attempt.clone(),
                },
                expected_goal_revision: Some(1),
            },
        ));
        let DirectResponseV1::Success(decide_ok) = harness.execute(harness.request(
            decide_cmd.clone(),
            Some("k-decide"),
            ContractGenerationV1::Generation2,
            initial_mode,
        )) else {
            panic!("expected initial decide success ({dir_label})");
        };
        assert_eq!(
            decide_ok.as_value()["result"]["schema"] == "podway.mutation-observation-result/v1",
            initial_mode,
            "decide schema ({dir_label})"
        );
        let seq_before_decide = harness.sequence();
        let rev_before_decide = harness.graph().trace().revision();
        let DirectResponseV1::Failure(err) = harness.execute(harness.request(
            decide_cmd,
            Some("k-decide"),
            ContractGenerationV1::Generation2,
            conflict_mode,
        )) else {
            panic!("expected IDEMPOTENCY_KEY_REUSED on decide ({dir_label})");
        };
        assert_eq!(
            err.as_value()["code"],
            "IDEMPOTENCY_KEY_REUSED",
            "decide ({dir_label})"
        );
        assert_eq!(err.as_value()["exit_code"], 2, "decide ({dir_label})");
        assert_eq!(
            harness.sequence(),
            seq_before_decide,
            "decide ({dir_label}) seq"
        );
        assert_eq!(
            harness.graph().trace().revision(),
            rev_before_decide,
            "decide ({dir_label}) rev"
        );

        last_harness = Some((harness, session_id));
    }

    let (harness_true_to_false, session_id) = last_harness.unwrap();

    // Unsupported routes with return_observation: true must fail closed with REQUEST_INVALID (exit_code 2)
    // before consuming a preflight graph hint or mutating the Store.
    let finish_attempt = harness_true_to_false
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let mut context = DirectInvocationContextV1::default();
    let _ = execute_direct_preflight_facts_with_context_v1(
        &harness_true_to_false.resolver,
        DirectPreflightFactsRequestV1 {
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000606").unwrap(),
            selector: harness_true_to_false.selector.clone(),
            expected_workspace_id: None,
            expected_session_id: Some(session_id.clone()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
            timeout_ms: 5000,
            generation: ContractGenerationV1::Generation2,
        },
        &Clock,
        &mut context,
    )
    .unwrap();

    let seq_before_unsupported = harness_true_to_false.sequence();
    for unsupported in [
        DirectCommandV1::Start(podway_engine::direct_start::DirectStartCommandV1::Start(
            SessionStartCommandV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "agent005.yaml".into(),
                    },
                    expected_procedure_digest: None,
                    task_title: "Dry run".into(),
                    dry_run: true,
                },
                preconditions: None,
            },
        )),
        DirectCommandV1::Cursor(DirectCursorCommandV1::Skip(SessionSkipV1 {
            reason: Some("Unsupported".into()),
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(5),
                expected_attempt_id: finish_attempt.clone(),
            },
        })),
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalDefine(GoalDefineV2 {
            goal: "Unsupported".into(),
            criteria: vec![],
            actor: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_session_revision: Revision::new(5),
            },
        })),
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriterion(
            GoalAssessCriterionV2 {
                criterion_id: CriterionId::new("c1").unwrap(),
                status: "satisfied".into(),
                reason: "Unsupported single assess".into(),
                evidence: vec![],
                items: vec![],
                actor: None,
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(5),
                    expected_attempt_id: finish_attempt.clone(),
                },
                expected_goal_revision: 1,
            },
        )),
        DirectCommandV1::Item(DirectItemCommandV1::Check(ItemCheckV1 {
            item_id: ItemId::new("done").unwrap(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: session_id.clone(),
                expected_attempt_id: finish_attempt,
                expected_item_revision: Revision::ZERO,
            },
        })),
        DirectCommandV1::Read {
            command: DirectReadCommandV1::Observe,
            expected_session_id: Some(session_id),
        },
        DirectCommandV1::WorkspaceShow,
        DirectCommandV1::IntegrationShow,
    ] {
        let resp = execute_direct_with_context_v1(
            &harness_true_to_false.resolver,
            harness_true_to_false.request(
                unsupported,
                Some("k-unsupported"),
                ContractGenerationV1::Generation2,
                true,
            ),
            &Clock,
            &harness_true_to_false.ids,
            &mut context,
        )
        .unwrap();
        let DirectResponseV1::Failure(err) = resp else {
            panic!("unsupported route succeeded with return_observation: true");
        };
        assert_eq!(err.as_value()["code"], "REQUEST_INVALID");
        assert_eq!(err.as_value()["exit_code"], 2);
    }
    assert_eq!(harness_true_to_false.sequence(), seq_before_unsupported);
}
