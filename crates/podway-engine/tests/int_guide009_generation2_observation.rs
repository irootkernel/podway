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
