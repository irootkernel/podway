//! Session and native worktree identity refusals through the direct boundary.

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline},
    int_v2dfc006_fixture_ports::{DECISION_PROCEDURE, RETRY_PROCEDURE, STATE_PROCEDURE},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::{GraphNodeId, OptionId, Revision};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    direct_sessions::DirectSessionCommandV1,
    direct_start::DirectStartCommandV1,
    execution::prepare_workspace_procedure_source_v2,
    v2_read_service::GraphStatusTierV2,
};
use podway_protocol::{
    ProcedureV2MutationCommandV1 as P, ReworkPreconditionsWireV2, SessionBeginV1,
    SessionCompleteV1, SessionDecideV2, SessionDeletionModeV1, SessionIdentityPreconditionsWireV1,
    SessionResetV1, SessionReworkV2, SessionStartReplaceV1, SessionStartSourceV1, SessionStartV1,
};
use podway_store::IdempotencyKeyV1;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Barrier},
    thread,
};

fn identity(engine: &Engine) -> SessionIdentityPreconditionsWireV1 {
    let graph = engine.graph();
    SessionIdentityPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
    }
}
fn clear(engine: &Engine) {
    engine.success(
        DirectCommandV1::Session(DirectSessionCommandV1::Reset(SessionResetV1 {
            mode: SessionDeletionModeV1::Force {
                progress_summary: "Release the old task scope.".into(),
            },
            dry_run: false,
            preconditions: identity(engine),
        })),
        "clear",
    );
}
fn refusal(engine: &Engine, command: DirectCommandV1, key: &str, actual: Value) {
    let before = engine
        .store
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap();
    let sequence = engine.sequence();
    let request = engine.request(command, key);
    for _ in 0..2 {
        let DirectResponseV1::Failure(error) = engine.execute(request.clone(), &NoClock) else {
            panic!("stale identity applied")
        };
        let value = error.into_value();
        assert_eq!(value["code"], "SESSION_ID_MISMATCH");
        assert_eq!(value["details"]["actual_session_id"], actual);
        assert!(value["details"].get("admission").is_none());
        assert_eq!(
            engine
                .store
                .read(deadline(), |snapshot| snapshot.current_graph())
                .unwrap(),
            before
        );
        assert_eq!(engine.sequence(), sequence);
        assert!(
            engine
                .store
                .read(deadline(), |snapshot| snapshot
                    .outcome(&IdempotencyKeyV1::new(key).unwrap()))
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn decide_after_session_clear_cannot_resurrect_the_old_task_scope() {
    let mut engine = Engine::new(DECISION_PROCEDURE);
    engine.set("result", "ready", "result");
    engine.complete("work");
    let command = DirectCommandV1::Procedure(P::SessionDecide(SessionDecideV2 {
        option_id: OptionId::new("accept").unwrap(),
        reason: "Accept the old result.".into(),
        actor: None,
        expected_goal_revision: None,
        preconditions: engine.fences(),
    }));
    clear(&engine);
    engine.cold();
    refusal(&engine, command, "stale-decide", Value::Null);
}

#[test]
fn complete_after_session_clear_cannot_resurrect_the_old_task_scope() {
    let mut engine = Engine::new(DECISION_PROCEDURE);
    let command = DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
        preconditions: engine.fences(),
    }));
    clear(&engine);
    engine.cold();
    refusal(&engine, command, "stale-complete", Value::Null);
}

#[test]
fn rework_after_session_replacement_preserves_the_successor_identity() {
    let mut engine = Engine::new(RETRY_PROCEDURE);
    let old = engine.fences();
    let command = DirectCommandV1::Procedure(P::SessionRework(SessionReworkV2 {
        target_graph_node_id: GraphNodeId::new("work").unwrap(),
        reason: "Revisit the old work.".into(),
        actor: None,
        preconditions: ReworkPreconditionsWireV2 {
            expected_session_id: old.expected_session_id,
            expected_session_revision: old.expected_session_revision,
            expected_attempt_id: Some(old.expected_attempt_id),
        },
    }));
    engine.success(
        DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "source.yaml".into(),
                },
                expected_procedure_digest: Some(
                    prepare_workspace_procedure_source_v2(
                        "source.yaml",
                        RETRY_PROCEDURE.as_bytes(),
                    )
                    .unwrap()
                    .digest()
                    .clone(),
                ),
                task_title: "Successor task scope".into(),
                dry_run: false,
            },
            mode: SessionDeletionModeV1::Delete {
                confirmed: true,
                progress_summary: Some("Old progress recorded.".into()),
            },
            preconditions: identity(&engine),
        })),
        "replace",
    );
    let actual = serde_json::to_value(engine.graph().trace().session_id()).unwrap();
    engine.cold();
    refusal(&engine, command, "stale-rework", actual);
}

#[test]
fn ineligible_completed_start_rejects_without_retaining_a_failure() {
    let mut engine = Engine::new(STATE_PROCEDURE);
    engine.success(engine.skip(None), "work");
    engine.success(engine.skip(None), "finish");
    let command = DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
        start: SessionStartV1 {
            source: SessionStartSourceV1::Procedure {
                procedure: "source.yaml".into(),
            },
            expected_procedure_digest: Some(
                prepare_workspace_procedure_source_v2("source.yaml", STATE_PROCEDURE.as_bytes())
                    .unwrap()
                    .digest()
                    .clone(),
            ),
            task_title: "Rejected undisposed successor".into(),
            dry_run: false,
        },
        mode: SessionDeletionModeV1::Eligible,
        preconditions: identity(&engine),
    }));
    engine.reject(
        command.clone(),
        "ineligible",
        "SESSION_RESET_NOT_ELIGIBLE",
        &NoClock,
    );
    engine.cold();
    engine.reject(
        command,
        "ineligible",
        "SESSION_RESET_NOT_ELIGIBLE",
        &NoClock,
    );
}

#[test]
fn deleted_worktree_rejects_begin_before_any_effect() {
    let mut engine = Engine::new(STATE_PROCEDURE);
    bind_selector(&mut engine);
    let command = DirectCommandV1::Procedure(P::SessionBegin(SessionBeginV1 {
        initial_goal: None,
        preconditions: SessionIdentityPreconditionsWireV1 {
            expected_session_id: engine.graph().trace().session_id().clone(),
            expected_session_revision: Revision::ZERO,
        },
    }));
    let request = engine.request(command, "gone-begin");
    std::fs::remove_dir_all(engine.temporary.path()).unwrap();
    let DirectResponseV1::Failure(error) = engine.execute(request, &NoClock) else {
        panic!("deleted root admitted")
    };
    assert_eq!(error.into_value()["code"], "WORKTREE_GONE");
}

#[test]
fn deleted_worktree_rejects_a_prepared_mutation_without_recreating_runtime() {
    let mut engine = Engine::new(STATE_PROCEDURE);
    bind_selector(&mut engine);
    let request = engine.request(engine.retry("No resurrection.".into()), "gone-retry");
    engine
        .resolver
        .resolve_existing(engine.selector.clone(), None, deadline())
        .unwrap();
    std::fs::remove_dir_all(engine.temporary.path()).unwrap();
    let DirectResponseV1::Failure(error) = engine.execute(request, &NoClock) else {
        panic!("deleted root recreated")
    };
    assert_eq!(error.into_value()["code"], "WORKTREE_GONE");
    assert!(!engine.temporary.path().exists());
}

fn bind_selector(engine: &mut Engine) {
    let (resolved, _) = engine
        .resolver
        .resolve_existing(engine.selector.clone(), None, deadline())
        .unwrap();
    engine.selector = podway_git::WorktreeSelectorV1::new(
        engine.selector.version(),
        Some(resolved.worktree().identity().clone()),
        engine.selector.path().clone(),
    )
    .unwrap();
}

#[test]
fn runtime_database_replacement_before_begin_rejects_the_pinned_generation() {
    database_replacement_before_begin(false);
}

#[test]
fn admit003_context_database_replacement_before_begin_keeps_writer_refusal() {
    database_replacement_before_begin(true);
}

fn database_replacement_before_begin(contextual: bool) {
    use podway_engine::workspace::DirectWorkspaceResolverV1;
    use podway_git::NativeGitResolverV1;
    use podway_store::direct_store::DirectTransactionBoundaryV1;
    let mut engine = Engine::new(STATE_PROCEDURE);
    let mut context = podway_engine::direct_api::DirectInvocationContextV1::default();
    if contextual {
        assert!(matches!(
            podway_engine::direct_api::execute_direct_preflight_facts_with_context_v1(
                &engine.resolver,
                super::int_v2dfc006_races::reads004_facts_request(&engine.selector),
                &Clock,
                &mut context,
            )
            .unwrap(),
            podway_engine::direct_execution::DirectPreflightFactsResponseV1::Facts(_)
        ));
    }
    let before = engine.graph();
    let sequence = engine.sequence();
    let database = engine.store.layout().database();
    let saved = engine
        .temporary
        .path()
        .join("fault-owned-original-database");
    let source = database.clone();
    let backup = saved.clone();
    engine.resolver = DirectWorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        super::int_v2dfc006_direct_execution::digest(),
    )
    .with_store_boundary_observer_v1(move |boundary| {
        if boundary == DirectTransactionBoundaryV1::BeforeBegin {
            // Hostile replacement is isolated to this owned fixture, after the
            // native open has pinned its original descriptor.
            std::fs::rename(&source, &backup).unwrap();
            std::fs::copy(&backup, &source).unwrap();
        }
    });
    let request = engine.request(engine.retry("Reject replacement.".into()), "replaced");
    let response = if contextual {
        podway_engine::direct_api::execute_direct_with_context_v1(
            &engine.resolver,
            request,
            &NoClock,
            &podway_engine::direct_execution::NativeDirectIdSourceV1,
            &mut context,
        )
        .unwrap()
    } else {
        engine.execute(request, &NoClock)
    };
    std::fs::rename(saved, database).unwrap();
    engine.cold();
    let DirectResponseV1::Failure(error) = response else {
        panic!("replaced generation committed")
    };
    assert_eq!(error.into_value()["code"], "WORKSPACE_MAINTENANCE");
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    assert!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot
                .outcome(&IdempotencyKeyV1::new("replaced").unwrap()))
            .unwrap()
            .is_none()
    );
    engine.store.verify_integrity(deadline()).unwrap();
}

#[test]
fn concurrent_retries_and_cold_long_traversal_keep_exactly_one_cursor() {
    let mut engine = Engine::new(RETRY_PROCEDURE);
    let requests = [
        engine.request(engine.retry("left".into()), "left"),
        engine.request(engine.retry("right".into()), "right"),
    ];
    let barrier = Arc::new(Barrier::new(3));
    let responses = thread::scope(|scope| {
        let handles: Vec<_> = requests
            .into_iter()
            .map(|request| {
                let barrier = Arc::clone(&barrier);
                let engine = &engine;
                scope.spawn(move || {
                    barrier.wait();
                    engine.execute(request, &Clock)
                })
            })
            .collect();
        barrier.wait();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        responses
            .iter()
            .filter(|response| matches!(response, DirectResponseV1::Success(_)))
            .count(),
        1
    );
    for response in responses {
        if let DirectResponseV1::Failure(error) = response {
            assert_eq!(error.into_value()["code"], "SESSION_REVISION_CONFLICT");
        }
    }
    let graph = engine.graph();
    assert_eq!(graph.trace().attempts().len(), 2);
    assert_eq!(graph.trace().active_attempt().unwrap().number().get(), 2);
    std::fs::remove_file(engine.temporary.path().join("source.yaml")).unwrap();
    for n in 0..40 {
        engine.success(
            engine.retry(format!("unbounded retry {n}")),
            &format!("retry-{n}"),
        );
        if n == 19 {
            engine.cold();
        }
    }
    let final_status = engine.read(DirectReadCommandV1::Status {
        tier: GraphStatusTierV2::Verbose,
        history_before: None,
    });
    assert_eq!(final_status["trace_length"], 42);
    assert_eq!(final_status["current"]["attempt"]["attempt_number"], 42);
    assert_eq!(final_status["counters"][0]["attempt_count"], 42);
    assert_eq!(final_status["counters"][0]["rework_traversal_count"], 0);
    assert_eq!(
        final_status["current_trace_history"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let stale = final_status["stale_attempt_history"]["entries"]
        .as_array()
        .unwrap();
    assert!(!stale.is_empty() && stale.len() <= 32 && stale.len() < 41);
    assert_eq!(
        final_status["stale_attempt_history"]["trace_truncated"],
        true
    );
    assert!(final_status.get("queue").is_none());
    assert_eq!(
        engine
            .graph()
            .trace()
            .active_attempt()
            .unwrap()
            .number()
            .get(),
        42
    );
    assert_eq!(final_status["session"]["lifecycle"], json!("running"));
}

#[test]
fn admit003_changed_session_and_procedure_snapshot_discard_the_hint_before_writer_fences() {
    use podway_engine::direct_api::{
        DirectInvocationContextV1, execute_direct_preflight_facts_with_context_v1,
        execute_direct_with_context_v1,
    };
    use podway_engine::direct_execution::{DirectPreflightFactsResponseV1, NativeDirectIdSourceV1};
    use podway_store::test_support::query_account::QueryAccountant;
    let engine = Engine::new(RETRY_PROCEDURE);
    let old = engine.graph();
    let mut context = DirectInvocationContextV1::default();
    assert!(matches!(
        execute_direct_preflight_facts_with_context_v1(
            &engine.resolver,
            super::int_v2dfc006_races::reads004_facts_request(&engine.selector),
            &Clock,
            &mut context
        )
        .unwrap(),
        DirectPreflightFactsResponseV1::Facts(_)
    ));
    let stale = engine.request(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: engine.fences(),
        })),
        "admit003-old-session",
    );
    std::fs::write(engine.temporary.path().join("source.yaml"), STATE_PROCEDURE).unwrap();
    engine.success(
        DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "source.yaml".into(),
                },
                expected_procedure_digest: Some(
                    prepare_workspace_procedure_source_v2(
                        "source.yaml",
                        STATE_PROCEDURE.as_bytes(),
                    )
                    .unwrap()
                    .digest()
                    .clone(),
                ),
                task_title: "New session and snapshot".into(),
                dry_run: false,
            },
            mode: SessionDeletionModeV1::Delete {
                confirmed: true,
                progress_summary: Some("Replace the prior task.".into()),
            },
            preconditions: identity(&engine),
        })),
        "admit003-new-session",
    );
    let successor = engine.graph();
    assert_ne!(successor.trace().session_id(), old.trace().session_id());
    assert_ne!(
        successor.snapshot().snapshot_id(),
        old.snapshot().snapshot_id()
    );
    assert_ne!(successor.snapshot().digest(), old.snapshot().digest());
    let sequence = engine.sequence();
    let accountant = QueryAccountant::start();
    let response = execute_direct_with_context_v1(
        &engine.resolver,
        stale,
        &NoClock,
        &NativeDirectIdSourceV1,
        &mut context,
    )
    .unwrap();
    assert!(accountant.finish().retention.graph_scopes > 0);
    let DirectResponseV1::Failure(error) = response else {
        panic!("old session mutated its successor")
    };
    assert_eq!(error.as_value()["code"], "SESSION_ID_MISMATCH");
    assert_eq!(engine.graph(), successor);
    assert_eq!(engine.sequence(), sequence);
    assert!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot.outcome(
                &IdempotencyKeyV1::new("admit003-old-session").unwrap()
            ))
            .unwrap()
            .is_none()
    );
    engine.store.verify_integrity(deadline()).unwrap();
}
