//! Retained runtime assertions through the actual typed direct Engine boundary.

use podway_core::{ItemId, Revision, SessionLifecycle};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1, NativeDirectIdSourceV1},
    direct_items::DirectItemCommandV1,
    direct_start::DirectStartCommandV1,
    execution::{ExecutionClockV1, prepare_workspace_procedure_source_v2},
    v2_read_service::GraphStatusTierV2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{NativeGitResolverV1, WorktreeSelectorV1};
use podway_protocol::{
    ItemMutationPreconditionsWireV1, ItemSetV1, ProcedureV2MutationCommandV1, RequestIdV1,
    Rfc3339MillisV1, SessionBeginV1, SessionCompleteV1, SessionIdentityPreconditionsWireV1,
    SessionMutationPreconditionsWireV1, SessionRetryV1, SessionSkipV1, SessionStartSourceV1,
    SessionStartV1, validate_daemon_free_document_v1,
};
use podway_store::{GraphSessionStateV2, IdempotencyKeyV1, direct_store::DirectStoreV1};
use serde_json::Value;
use tempfile::TempDir;

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture},
    int_v2dfc006_fixture_ports::{
        DECISION_PROCEDURE, RETRY_PROCEDURE, SKIP_PROCEDURE, STATE_PROCEDURE,
    },
};

pub(super) struct NoClock;
impl ExecutionClockV1 for NoClock {
    fn now(&self) -> podway_core::UnixMillis {
        panic!("replay or input/fence rejection read the clock")
    }
}

pub(super) struct Engine {
    pub(super) temporary: TempDir,
    pub(super) selector: WorktreeSelectorV1,
    pub(super) store: DirectStoreV1,
    pub(super) resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
}
impl Engine {
    pub(super) fn new(source: &str) -> Self {
        let (temporary, selector, store, _) = fixture();
        std::fs::write(temporary.path().join("source.yaml"), source).unwrap();
        let engine = Self {
            temporary,
            selector,
            store,
            resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
        };
        engine.success(
            DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "source.yaml".into(),
                },
                expected_procedure_digest: Some(
                    prepare_workspace_procedure_source_v2("source.yaml", source.as_bytes())
                        .unwrap()
                        .digest()
                        .clone(),
                ),
                task_title: "Retained runtime port".into(),
                dry_run: false,
            })),
            "start",
        );
        let graph = engine.graph();
        engine.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal: None,
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: graph.trace().session_id().clone(),
                        expected_session_revision: Revision::ZERO,
                    },
                },
            )),
            "begin",
        );
        engine
    }
    pub(super) fn request(&self, command: DirectCommandV1, key: &str) -> DirectRequestV1 {
        DirectRequestV1 {
            selector: self.selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000099").unwrap(),
            command,
            idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
            timeout_ms: 5000,
        }
    }
    pub(super) fn execute(
        &self,
        request: DirectRequestV1,
        clock: &impl ExecutionClockV1,
    ) -> DirectResponseV1 {
        let response =
            execute_direct_v1(&self.resolver, request, clock, &NativeDirectIdSourceV1).unwrap();
        match &response {
            DirectResponseV1::Success(output) => assert!(validate_daemon_free_document_v1(
                "podway.output/v4",
                output.as_value()
            )),
            DirectResponseV1::Failure(error) => assert!(validate_daemon_free_document_v1(
                "podway.error/v2",
                &error.clone().into_value()
            )),
        }
        response
    }
    pub(super) fn success(&self, command: DirectCommandV1, key: &str) -> Value {
        self.output(self.execute(self.request(command, key), &Clock))
    }
    pub(super) fn output(&self, response: DirectResponseV1) -> Value {
        match response {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => {
                panic!("unexpected rejection: {:?}", error.into_value())
            }
        }
    }
    pub(super) fn reject(
        &self,
        command: DirectCommandV1,
        key: &str,
        code: &str,
        clock: &impl ExecutionClockV1,
    ) {
        let before = self.graph();
        let sequence = self.sequence();
        let DirectResponseV1::Failure(error) = self.execute(self.request(command, key), clock)
        else {
            panic!("expected {code}")
        };
        assert_eq!(error.into_value()["code"], code);
        assert_eq!(self.graph(), before);
        assert_eq!(self.sequence(), sequence);
        assert!(
            self.store
                .read(deadline(), |snapshot| snapshot
                    .outcome(&IdempotencyKeyV1::new(key).unwrap()))
                .unwrap()
                .is_none()
        );
    }
    pub(super) fn graph(&self) -> GraphSessionStateV2 {
        self.store
            .prepare_graph(deadline())
            .unwrap()
            .unwrap()
            .graph()
            .clone()
    }
    pub(super) fn sequence(&self) -> u64 {
        self.store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap()
    }
    pub(super) fn fences(&self) -> SessionMutationPreconditionsWireV1 {
        let graph = self.graph();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
        }
    }
    pub(super) fn set(&self, item: &str, value: &str, key: &str) {
        let graph = self.graph();
        let attempt = graph.trace().active_attempt().unwrap().attempt_id();
        let item = ItemId::new(item).unwrap();
        let revision = graph
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == attempt)
            .unwrap()
            .item_slots()
            .iter()
            .find(|slot| slot.item_id() == &item)
            .unwrap()
            .revision();
        self.success(
            DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
                item_id: item,
                value: value.into(),
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: graph.trace().session_id().clone(),
                    expected_attempt_id: attempt.clone(),
                    expected_item_revision: revision,
                },
            })),
            key,
        );
    }
    pub(super) fn complete(&self, key: &str) -> Value {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: self.fences(),
            })),
            key,
        )
    }
    pub(super) fn skip(&self, reason: Option<String>) -> DirectCommandV1 {
        DirectCommandV1::Cursor(DirectCursorCommandV1::Skip(SessionSkipV1 {
            reason,
            preconditions: self.fences(),
        }))
    }
    pub(super) fn retry(&self, reason: String) -> DirectCommandV1 {
        DirectCommandV1::Cursor(DirectCursorCommandV1::Retry(SessionRetryV1 {
            reason,
            preconditions: self.fences(),
        }))
    }
    pub(super) fn read(&self, command: DirectReadCommandV1) -> Value {
        self.success(
            DirectCommandV1::Read {
                command,
                expected_session_id: Some(self.graph().trace().session_id().clone()),
            },
            "unused-read-key",
        )["result"]
            .clone()
    }
    pub(super) fn cold(&mut self) {
        self.resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    }
}

#[test]
fn v2run005_production_skip_discards_values_advances_terminates_restarts_and_replays() {
    let mut engine = Engine::new(SKIP_PROCEDURE);
    engine.reject(
        engine.skip(Some("not eligible".into())),
        "reject-prepare",
        "STAGE_NOT_SKIPPABLE",
        &Clock,
    );
    assert_eq!(
        engine.complete("complete-prepare")["result"]["to_graph_node_id"],
        "source"
    );
    engine.set("optional-note", "must be discarded", "set-optional");
    let source = engine.read(DirectReadCommandV1::Status {
        tier: GraphStatusTierV2::Verbose,
        history_before: None,
    });
    assert_eq!(source["current"]["missing_required_item_count"], 1);
    assert_eq!(source["items_total"], 1);
    let source_attempt = engine.fences().expected_attempt_id;
    engine.reject(
        engine.skip(None),
        "missing-reason",
        "STAGE_NOT_SKIPPABLE",
        &Clock,
    );
    let request = engine.request(
        engine.skip(Some("source is not applicable".into())),
        "skip-source",
    );
    let original = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(original["result"]["from_graph_node_id"], "source");
    assert_eq!(original["result"]["to_graph_node_id"], "consumer");
    assert_eq!(
        original["result"]["from_attempt_id"],
        serde_json::json!(source_attempt)
    );
    assert_eq!(
        engine.output(engine.execute(request.clone(), &NoClock)),
        original
    );
    let next = engine.read(DirectReadCommandV1::Next);
    assert_eq!(next["readback"][0]["state"], "skipped");
    assert!(next["readback"][0]["items"].as_array().unwrap().is_empty());
    std::fs::remove_file(engine.temporary.path().join("source.yaml")).unwrap();
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    assert_eq!(
        engine.read(DirectReadCommandV1::Next)["readback"],
        next["readback"]
    );
    assert_eq!(
        engine.success(engine.skip(None), "skip-consumer")["result"]["to_graph_node_id"],
        "finish"
    );
    let terminal = engine.success(engine.skip(None), "skip-finish");
    assert_eq!(terminal["result"]["session_state"], "completed");
    assert!(terminal["result"].get("to_attempt_id").is_none());
    assert_eq!(
        engine.graph().trace().lifecycle(),
        SessionLifecycle::Completed
    );
}

#[test]
fn v2run005_skip_reason_uses_the_v2_unicode_scalar_boundary_before_admission() {
    let engine = Engine::new(SKIP_PROCEDURE);
    engine.complete("prepare");
    engine.reject(
        engine.skip(Some("한".repeat(2001))),
        "too-long",
        "REQUEST_INVALID",
        &NoClock,
    );
    engine.reject(
        engine.skip(Some(" \t\n".into())),
        "blank",
        "REQUEST_INVALID",
        &NoClock,
    );
    let maximum = "한".repeat(2000);
    assert_eq!(
        engine.success(engine.skip(Some(maximum.clone())), "maximum")["result"]["reason"],
        maximum
    );
}

#[test]
fn v2run005_production_rejects_decision_skip() {
    let engine = Engine::new(DECISION_PROCEDURE);
    engine.set("result", "ready", "work-result");
    engine.complete("work");
    engine.reject(
        engine.skip(Some("decision must choose".into())),
        "decision-skip",
        "STAGE_NOT_SKIPPABLE",
        &Clock,
    );
}

#[test]
fn v2run004_production_retry_is_clean_durable_replayable_and_re_resolves_evidence() {
    let mut engine = Engine::new(RETRY_PROCEDURE);
    engine.set("result", "discard this action value", "set-original");
    let old = engine.fences().expected_attempt_id;
    let request = engine.request(engine.retry("redo action".into()), "retry-action");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    assert_ne!(original["result"]["to_attempt_id"], serde_json::json!(old));
    assert_eq!(
        engine.output(engine.execute(request.clone(), &NoClock)),
        original
    );
    let current = engine.read(DirectReadCommandV1::Status {
        tier: GraphStatusTierV2::Verbose,
        history_before: None,
    });
    assert_eq!(current["items_total"], 0);
    engine.set("result", "fresh action evidence", "set-fresh");
    engine.complete("complete-work");
    let before = engine.read(DirectReadCommandV1::Next);
    engine.set("note", "discard decision note", "decision-note");
    let decision = engine.request(engine.retry("redo decision".into()), "retry-decision");
    let decision_once = engine.output(engine.execute(decision.clone(), &Clock));
    assert_eq!(
        engine.read(DirectReadCommandV1::Next)["readback"],
        before["readback"]
    );
    assert_eq!(
        engine.read(DirectReadCommandV1::Status {
            tier: GraphStatusTierV2::Verbose,
            history_before: None
        })["items_total"],
        0
    );
    std::fs::remove_file(engine.temporary.path().join("source.yaml")).unwrap();
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    assert_eq!(
        engine.output(engine.execute(decision, &NoClock)),
        decision_once
    );
}

#[test]
fn v2run004_retry_reason_uses_the_v2_unicode_scalar_boundary_before_admission() {
    let engine = Engine::new(RETRY_PROCEDURE);
    engine.reject(
        engine.retry("한".repeat(2001)),
        "too-long",
        "REQUEST_INVALID",
        &NoClock,
    );
    engine.reject(
        engine.retry(" \t\n".into()),
        "blank",
        "REQUEST_INVALID",
        &NoClock,
    );
    let maximum = "한".repeat(2000);
    assert_eq!(
        engine.success(engine.retry(maximum.clone()), "maximum")["result"]["reason"],
        maximum
    );
}

#[test]
fn v2run006_block_limit_unblock_all_and_blocked_skip_are_exact() {
    use podway_protocol::{SessionBlockV1, SessionUnblockV1};
    let mut engine = Engine::new(STATE_PROCEDURE);
    for n in 0..64 {
        engine.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Block(SessionBlockV1 {
                reason: format!("blocker {n}"),
                preconditions: engine.fences(),
            })),
            &format!("block-{n}"),
        );
        assert_eq!(
            engine.graph().workflow_memory().attempts()[0]
                .blockers()
                .len(),
            n + 1
        );
    }
    engine.reject(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Block(SessionBlockV1 {
            reason: "overflow".into(),
            preconditions: engine.fences(),
        })),
        "overflow",
        "BLOCKER_LIMIT_REACHED",
        &Clock,
    );
    engine.cold();
    let status = engine.read(DirectReadCommandV1::Status {
        tier: GraphStatusTierV2::Standard,
        history_before: None,
    });
    assert_eq!(status["session"]["lifecycle"], "running");
    assert_eq!(status["current"]["blockers_total"], 64);
    assert_eq!(status["current"]["readiness"]["unblocked"], false);
    assert_eq!(status["current"]["readiness"]["can_advance"], false);
    engine.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Unblock(SessionUnblockV1 {
            blocker_id: None,
            all: true,
            preconditions: engine.fences(),
        })),
        "unblock-all",
    );
    assert_eq!(
        engine.graph().workflow_memory().attempts()[0]
            .blockers()
            .iter()
            .filter(|blocker| blocker.state() == podway_core::BlockerState::Open)
            .count(),
        0
    );
    engine.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Block(SessionBlockV1 {
            reason: "Skip remains legal".into(),
            preconditions: engine.fences(),
        })),
        "block-before-skip",
    );
    assert_eq!(
        engine.success(engine.skip(None), "skip-work")["result"]["to_graph_node_id"],
        "finish"
    );
    let graph = engine.graph();
    assert_eq!(
        graph.workflow_memory().attempts()[0]
            .blockers()
            .iter()
            .filter(|blocker| blocker.state() == podway_core::BlockerState::Open)
            .count(),
        1
    );
    assert!(graph.workflow_memory().attempts()[1].blockers().is_empty());
    engine.success(engine.skip(None), "skip-finish");
    assert_eq!(
        engine.graph().trace().lifecycle(),
        SessionLifecycle::Completed
    );
}
