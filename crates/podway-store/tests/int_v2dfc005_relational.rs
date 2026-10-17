//! Direct relational persistence ports: goals, memory, delegation and archives.

use podway_core::{
    ActorAttributionV2, AttemptId, BlockerId, CanonicalProcedureJsonV1,
    CriterionAssessmentReasonV2, CriterionAssessmentResultV2, CriterionCitationV2, CriterionId,
    CriterionStatusV2, GoalCriterionV2, GoalDefinitionV2, GoalRevisionNumberV2,
    GoalRevisionReasonV2, GoalStatementV2, GraphNodeId, ItemId, OptionId, ProcedureSnapshotId,
    ProcedureSourceLabelV1, ReasonV2, Revision, SessionId, SessionTraceV2, Sha256Digest,
    TerminalDispositionV2, UnixMillis, canonicalize_json_v1,
};
use podway_store::{
    ActiveItemMutationV2, DelegationReceiptV1, GraphInitialGoalV2, GraphMutationErrorV2,
    GraphNodeCounterV2, GraphSessionStateV2, IdempotencyKeyV1, ProcedureSnapshotV2, StoreErrorV1,
    StoreRecordKindV1, StoreValueErrorV1, WorkflowMemoryStateV2,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1, DirectStoreV1,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::int_v2dfc005_direct_store::{deadline, digest, fixture, request, store, success};

fn attempt(number: u64) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap()
}

fn criteria() -> GoalDefinitionV2 {
    GoalDefinitionV2::new(vec![
        GoalCriterionV2::new(
            CriterionId::new("z-proof").unwrap(),
            "The recorded proof survives reopen.",
        )
        .unwrap(),
        GoalCriterionV2::new(
            CriterionId::new("a-safety").unwrap(),
            "State and outcome commit together.",
        )
        .unwrap(),
    ])
    .unwrap()
}

fn snapshot(document: Value) -> ProcedureSnapshotV2 {
    let canonical = canonicalize_json_v1(&document).unwrap();
    let hash =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000010").unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        hash,
        ProcedureSourceLabelV1::file("direct-relational.yaml").unwrap(),
        UnixMillis::new(20),
    )
    .unwrap()
}

fn prepared(document: Value) -> GraphSessionStateV2 {
    GraphSessionStateV2::prepared(
        Revision::new(1),
        "Direct relational fixture",
        snapshot(document),
        SessionId::new("00000000-0000-4000-8000-000000000011").unwrap(),
        UnixMillis::new(20),
    )
    .unwrap()
}

fn goal_graph() -> GraphSessionStateV2 {
    prepared(
        json!({"schema":"podway.procedure/v2","id":"direct-goal","version":"1",
        "name":"Direct goal","purpose":"Persist goal and evidence history.","goal_tracking":true,
        "node_definitions":{
            "work":{"type":"action","title":"Record","intent":"Record proof.","items":[
                {"id":"proof","type":"text","prompt":"Proof","required":true}]},
            "assess":{"type":"decision","title":"Assess","objective":"Assess the goal.","prompt":"Achieved?",
                "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},
                    {"id":"superseded","label":"Superseded"}],"reason":{"required":true},
                "assessment":{"target":"session_goal","outcomes":{"achieved":"achieved",
                    "not-achieved":"not_achieved","superseded":"superseded"}}},
            "finish":{"type":"action","title":"Finish","intent":"Finish."}},
        "graph":{"entry":"work","nodes":[
            {"id":"work","use":"work","next":"assess"},
            {"id":"assess","use":"assess","evidence_from":[{"node":"work","required":true}],
                "routes":{"achieved":{"to":"finish","effect":"advance"},
                    "not-achieved":{"to":"finish","effect":"advance"},"superseded":{"to":"finish","effect":"advance"}}},
            {"id":"finish","use":"finish","terminal":true}]},
        "manual_rework":{"allowed_targets":["work","assess"]}}),
    )
}

fn seed(direct: &DirectStoreV1, graph: &GraphSessionStateV2) {
    let seed = request("relational-seed");
    direct
        .mutate(
            &seed,
            deadline(),
            || UnixMillis::new(20),
            |transaction| {
                transaction.now();
                transaction.create_graph(graph)?;
                Ok(success(&seed, json!({"seeded":true}), transaction.now()))
            },
        )
        .unwrap();
}

#[test]
fn bound001_deferred_memory_admission_and_cold_read_remain_distinct() {
    // Shape admission accepts each bound independently. Domain admission is
    // intentionally deferred until a transition needs the item declaration.
    let snapshot = snapshot(json!({
        "schema":"podway.procedure/v2", "id":"deferred-memory", "version":"1",
        "name":"Deferred memory", "purpose":"Preserve persisted admission timing.",
        "node_definitions":{"work":{"type":"action", "title":"Work", "intent":"Record.",
            "items":[{"id":"proof", "type":"text", "prompt":"Proof", "required":true,
                "min_length":2, "max_length":1}]}},
        "graph":{"entry":"work", "nodes":[{"id":"work", "use":"work", "terminal":true}]}
    }));
    assert_eq!(
        GraphSessionStateV2::prepared(
            Revision::new(1),
            "Deferred memory",
            snapshot.clone(),
            SessionId::new("00000000-0000-4000-8000-000000000011").unwrap(),
            UnixMillis::new(20),
        ),
        Err(podway_core::ProcedureStateErrorV2::new(
            "Procedure v2 item declaration is invalid"
        ))
    );
    let counters = snapshot
        .graph_nodes()
        .iter()
        .map(|node| GraphNodeCounterV2::new(node.graph_node_id().clone(), 0, 0))
        .collect();
    let graph = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "Deferred memory",
        snapshot,
        SessionTraceV2::prepared(SessionId::new("00000000-0000-4000-8000-000000000011").unwrap()),
        counters,
        vec![],
        WorkflowMemoryStateV2::new(vec![], vec![], vec![]).unwrap(),
        UnixMillis::new(20),
        None,
        None,
        None,
    )
    .unwrap();
    let check = |state: &GraphSessionStateV2| {
        assert_eq!(
            state.begin_v2(Revision::new(1), attempt(21), None, UnixMillis::new(21)),
            Err(GraphMutationErrorV2::SessionRevisionConflict {
                expected: Revision::new(1),
                actual: Revision::ZERO,
            })
        );
        assert_eq!(
            state.begin_v2(Revision::ZERO, attempt(21), None, UnixMillis::new(21)),
            Err(GraphMutationErrorV2::InvalidState(
                podway_core::ProcedureStateErrorV2::new("Procedure v2 item declaration is invalid")
            ))
        );
    };
    check(&graph);
    let (_root, direct) = fixture();
    seed(&direct, &graph);
    let reopened = store(&direct.layout().root().to_path_buf());
    assert!(matches!(
        reopened.prepare_graph(deadline()),
        Err(DirectStoreErrorV1::State(StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Snapshot,
        }))
    ));
}

#[test]
fn bound002_core_state_cannot_bypass_persisted_shape_admission() {
    let canonical = canonicalize_json_v1(&json!({
        "schema":"podway.procedure/v2", "id":"unadmitted", "version":"1",
        "name":"Unadmitted shape", "purpose":"Keep Store admission independent.",
        "unknown":true,
        "node_definitions":{"work":{"type":"action", "title":"Work", "intent":"Record."}},
        "graph":{"entry":"work", "nodes":[{"id":"work", "use":"work", "terminal":true}]}
    }))
    .unwrap();
    let hash =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let snapshot = ProcedureSnapshotV2::from_admitted_document(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000010").unwrap(),
        podway_core::CanonicalProcedureDocumentV2::new(
            CanonicalProcedureJsonV1::new(canonical).unwrap(),
            hash,
        )
        .unwrap(),
        ProcedureSourceLabelV1::file("unadmitted.json").unwrap(),
        UnixMillis::new(20),
    )
    .unwrap();
    let graph = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Unadmitted shape",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000000011").unwrap(),
        UnixMillis::new(20),
    )
    .unwrap();
    let (_root, direct) = fixture();
    let before = direct
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    let request = request("unadmitted-shape");
    let error = direct
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(20),
            |transaction| {
                transaction.now();
                transaction.create_graph(&graph)?;
                Ok(success(&request, json!({"seeded":true}), transaction.now()))
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        DirectStoreErrorV1::State(StoreErrorV1::InvalidStateV1(
            StoreValueErrorV1::InvalidProcedureV2State {
                reason: "Procedure v2 snapshot does not satisfy the canonical schema",
            }
        ))
    ));
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, before);
            assert!(snapshot.current_graph()?.is_none());
            assert!(snapshot.outcome(request.key())?.is_none());
            Ok(())
        })
        .unwrap();
}

fn session_request(
    graph: &GraphSessionStateV2,
    key: &str,
    command: &str,
) -> DirectMutationRequestV1 {
    DirectMutationRequestV1::new(
        IdempotencyKeyV1::new(key).unwrap(),
        digest('d'),
        command,
        "00000000-0000-4000-8000-000000000002",
        DirectOutcomeScopeV1::Session(graph.trace().session_id().clone()),
    )
    .unwrap()
}

fn advance(
    direct: &DirectStoreV1,
    key: &str,
    command: &str,
    time: u64,
    transition: impl FnOnce(
        &GraphSessionStateV2,
        UnixMillis,
    ) -> Result<GraphSessionStateV2, GraphMutationErrorV2>,
) -> GraphSessionStateV2 {
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    let prior = prepared.graph();
    let request = session_request(prior, key, command);
    let mut expected = None;
    direct
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(time),
            |transaction| {
                let prior = transaction.use_prepared_graph(Some(&prepared))?.unwrap();
                let next = transition(&prior, transaction.now())?;
                transaction.replace_graph(
                    prior.workspace_revision(),
                    prior.trace().revision(),
                    &next,
                )?;
                expected = Some(next);
                Ok(success(
                    &request,
                    json!({"changed":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let expected = expected.unwrap();
    let reopened = store(&direct.layout().root().to_path_buf());
    reopened
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(expected.clone()));
            Ok(())
        })
        .unwrap();
    expected
}

#[test]
fn goal_memory_assessment_and_reactivation_round_trip_in_author_order() {
    let (_root, direct) = fixture();
    seed(&direct, &goal_graph());
    advance(&direct, "begin", "session.begin", 21, |prior, now| {
        prior
            .begin_v2(
                Revision::ZERO,
                attempt(21),
                Some(GraphInitialGoalV2::new(
                    GoalStatementV2::new("Persist the complete graph.").unwrap(),
                    criteria(),
                    Some(ActorAttributionV2::new("planner").unwrap()),
                )),
                now,
            )
            .map(|outcome| outcome.into_state())
    });
    advance(&direct, "proof", "item.set", 22, |prior, now| {
        prior
            .mutate_active_item_v2(
                &attempt(21),
                &ItemId::new("proof").unwrap(),
                Revision::ZERO,
                ActiveItemMutationV2::Set {
                    value: "Verified state".into(),
                },
                now,
            )
            .map(|outcome| outcome.into_state())
    });
    let blocker = BlockerId::new("00000000-0000-4000-8000-000000000022").unwrap();
    advance(&direct, "block", "session.block", 23, |prior, now| {
        prior
            .block_active_attempt_v2(
                prior.trace().revision(),
                &attempt(21),
                blocker.clone(),
                "Await the independent proof.",
                now,
            )
            .map(|outcome| outcome.into_state())
    });
    advance(&direct, "unblock", "session.unblock", 24, |prior, now| {
        prior
            .unblock_active_attempt_v2(
                prior.trace().revision(),
                &attempt(21),
                Some(&blocker),
                false,
                now,
            )
            .map(|outcome| outcome.into_state())
    });
    advance(
        &direct,
        "complete-work",
        "session.complete",
        25,
        |prior, now| {
            prior
                .complete_active_action_v2(
                    prior.trace().revision(),
                    &attempt(21),
                    Some(attempt(23)),
                    now,
                )
                .map(|outcome| outcome.into_state())
        },
    );
    // Record in reverse call order; persisted output must retain criterion author order.
    for (index, id) in ["a-safety", "z-proof"].into_iter().enumerate() {
        advance(
            &direct,
            id,
            "session.assess",
            26 + index as u64,
            |prior, now| {
                prior
                    .assess_goal_criterion_v2(
                        prior.trace().revision(),
                        &attempt(23),
                        GoalRevisionNumberV2::FIRST,
                        CriterionAssessmentResultV2::new(
                            CriterionId::new(id).unwrap(),
                            CriterionStatusV2::Satisfied,
                            CriterionAssessmentReasonV2::new("The source proof is committed.")
                                .unwrap(),
                            vec![CriterionCitationV2::Evidence(
                                GraphNodeId::new("work").unwrap(),
                            )],
                        )
                        .unwrap(),
                        Some(ActorAttributionV2::new("reviewer").unwrap()),
                        now,
                    )
                    .map(|outcome| outcome.into_state())
            },
        );
    }
    let assessed = advance(&direct, "achieved", "session.decide", 28, |prior, now| {
        prior
            .decide_active_route_with_goal_revision_v2(
                prior.trace().revision(),
                &attempt(23),
                OptionId::new("achieved").unwrap(),
                attempt(24),
                Some(GoalRevisionNumberV2::FIRST),
                Some(ReasonV2::new("All criteria are satisfied.").unwrap()),
                Some(ActorAttributionV2::new("reviewer").unwrap()),
                now,
            )
            .map(|outcome| outcome.into_state())
    });
    assert_eq!(
        assessed.goal_state().assessments()[0]
            .criterion_results()
            .iter()
            .map(|result| result.criterion_id().as_str())
            .collect::<Vec<_>>(),
        vec!["z-proof", "a-safety"]
    );
    let completed = advance(&direct, "finish", "session.complete", 29, |prior, now| {
        prior
            .complete_active_action_v2(prior.trace().revision(), &attempt(24), None, now)
            .map(|outcome| outcome.into_state())
    });
    let revised = advance(
        &direct,
        "revise",
        "session.goal.revise",
        30,
        |prior, now| {
            prior
                .revise_goal_v2(
                    prior.trace().revision(),
                    None,
                    GoalRevisionNumberV2::FIRST,
                    GoalStatementV2::new("Reassess the updated requirements.").unwrap(),
                    criteria(),
                    GraphNodeId::new("work").unwrap(),
                    attempt(25),
                    GoalRevisionReasonV2::new("Requirements changed.").unwrap(),
                    None,
                    true,
                    now,
                )
                .map(|outcome| outcome.into_state())
        },
    );
    assert_eq!(revised.goal_state().revisions().len(), 2);
    assert_eq!(
        revised.goal_state().assessments(),
        completed.goal_state().assessments()
    );
    assert!(
        revised
            .goal_state()
            .latest_fresh_assessment(revised.trace())
            .is_none()
    );
    assert_eq!(
        revised.workflow_memory().attempts()[0],
        completed.workflow_memory().attempts()[0]
    );
    direct.verify_integrity(deadline()).unwrap();
}

fn delegated() -> GraphSessionStateV2 {
    prepared(json!({"schema":"podway.procedure/v2","id":"direct-delegation","version":"1",
        "name":"Direct delegation","purpose":"Persist immutable receipts.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Review work."}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true,
            "delegation":{"binding_ref":"local-specialist","role_ref":"reviewer","role_snapshot_digest":digest('c'),
                "max_rounds":3,"after_accept":"retain"}}]}})).begin_v2(Revision::ZERO,attempt(31),None,UnixMillis::new(21))
        .unwrap().into_state()
}

fn receipt(graph: &GraphSessionStateV2) -> DelegationReceiptV1 {
    let declaration: Value =
        serde_json::from_str(graph.snapshot().graph_nodes()[0].canonical_placement_json()).unwrap();
    DelegationReceiptV1::new(json!({"schema":"podway.delegation-receipt/v1",
        "receipt_id":"00000000-0000-4000-8000-000000000032","workspace_uuid":"00000000-0000-4000-8000-000000000001",
        "procedure_digest":graph.snapshot().digest(),"session_id":graph.trace().session_id(),"placement_id":"work",
        "attempt_id":attempt(31),"delegation":declaration["delegation"],"package_digest":digest('d'),
        "execution":{"binding_ref":"local-specialist","adapter_id":"adapter","adapter_version":"1","primary_host":"external",
            "scope_kind":"session","scope_ref":"scope-1","supervisor_ref":"supervisor-1","worker_ref":"worker-1",
            "task_ref":"task-1","role_ref":"reviewer","role_snapshot_digest":digest('c'),
            "agent_configuration_digest":digest('e'),"accepted_task_digest":digest('f')},
        "rounds_used":1,"result_digest":digest('1'),"summary":"Reviewed result","worker_disposition":"idle","accepted":true,
        "accepted_items_digest":podway_store::v2_delegation::accepted_items_digest_v1(graph).unwrap(),
        "accepted_at":"1970-01-01T00:00:00.030Z"})).unwrap()
}

#[test]
fn delegation_disposition_archive_purge_and_session_reset_obey_retention() {
    let (_root, direct) = fixture();
    let graph = delegated();
    seed(&direct, &graph);
    let accepted = receipt(&graph);
    let next = graph.accept_delegation_successor_v1(&attempt(31)).unwrap();
    let accept = session_request(&graph, "accept", "delegation.accept");
    direct
        .mutate(
            &accept,
            deadline(),
            || UnixMillis::new(30),
            |transaction| {
                transaction.now();
                transaction.accept_delegation(
                    graph.workspace_revision(),
                    graph.trace().revision(),
                    &next,
                    &digest('d'),
                    &accepted,
                )?;
                Ok(success(
                    &accept,
                    json!({"accepted":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(
                snapshot
                    .view(UnixMillis::new(31))?
                    .delegation_receipts()
                    .current_receipt(),
                Some(&accepted)
            );
            Ok(())
        })
        .unwrap();
    // A second immutable acceptance fails and rolls back the attempted revision change.
    let duplicate = session_request(&next, "duplicate", "delegation.accept");
    let second = next.accept_delegation_successor_v1(&attempt(31)).unwrap();
    assert!(
        direct
            .mutate(
                &duplicate,
                deadline(),
                || UnixMillis::new(31),
                |transaction| {
                    transaction.now();
                    transaction.accept_delegation(
                        next.workspace_revision(),
                        next.trace().revision(),
                        &second,
                        &digest('d'),
                        &accepted,
                    )?;
                    Ok(success(
                        &duplicate,
                        json!({"accepted":true}),
                        transaction.now(),
                    ))
                }
            )
            .is_err()
    );
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.current_graph()?, Some(next.clone()));
            assert!(snapshot.outcome(duplicate.key())?.is_none());
            Ok(())
        })
        .unwrap();
    let completed = advance(&direct, "complete", "session.complete", 32, |prior, now| {
        prior
            .complete_active_action_v2(prior.trace().revision(), &attempt(31), None, now)
            .map(|outcome| outcome.into_state())
    });
    let disposition = TerminalDispositionV2::handed_off(
        completed.trace().session_id().clone(),
        completed.trace().revision(),
        "Delivered to the owner",
        "commit:abc123",
        None,
        UnixMillis::new(33),
    )
    .unwrap();
    let dispose = session_request(&completed, "disposition", "session.terminal_disposition");
    direct
        .mutate(
            &dispose,
            deadline(),
            || UnixMillis::new(33),
            |transaction| {
                transaction.now();
                transaction.record_disposition(&disposition)?;
                Ok(success(
                    &dispose,
                    json!({"recorded":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let archive = session_request(&completed, "archive", "session.archive");
    direct
        .mutate(
            &archive,
            deadline(),
            || UnixMillis::new(34),
            |transaction| {
                transaction.now();
                transaction
                    .archive_graph(completed.workspace_revision(), completed.trace().revision())?;
                Ok(success(
                    &archive,
                    json!({"archived":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.current_graph()?.is_none());
            assert!(
                snapshot
                    .archived_graph(completed.trace().session_id())?
                    .is_some()
            );
            assert!(snapshot.outcome(accept.key())?.is_some());
            Ok(())
        })
        .unwrap();
    let purge = session_request(&completed, "purge", "session.purge");
    direct
        .mutate(
            &purge,
            deadline(),
            || UnixMillis::new(35),
            |transaction| {
                transaction.now();
                transaction
                    .purge_archive(completed.trace().session_id(), completed.trace().revision())?;
                Ok(success(&purge, json!({"purged":true}), transaction.now()))
            },
        )
        .unwrap();
    direct
        .read(deadline(), |snapshot| {
            assert!(
                snapshot
                    .archived_graph(completed.trace().session_id())?
                    .is_none()
            );
            assert!(snapshot.outcome(accept.key())?.is_some());
            assert!(snapshot.outcome(archive.key())?.is_some());
            Ok(())
        })
        .unwrap();
    // Reset deletes outcomes only for its own current session, not a purged historical session.
    let another = super::int_v2dfc005_direct_store::grown_graph(2, 0);
    let create = request("second-session");
    direct
        .mutate(
            &create,
            deadline(),
            || UnixMillis::new(40),
            |transaction| {
                transaction.now();
                transaction.create_graph(&another)?;
                Ok(success(&create, json!({"created":true}), transaction.now()))
            },
        )
        .unwrap();
    let local = session_request(&another, "current-outcome", "item.set");
    direct
        .mutate(
            &local,
            deadline(),
            || UnixMillis::new(41),
            |transaction| {
                transaction.now();
                Ok(success(&local, json!({"changed":false}), transaction.now()))
            },
        )
        .unwrap();
    let reset = request("reset-session");
    direct
        .mutate(
            &reset,
            deadline(),
            || UnixMillis::new(42),
            |transaction| {
                transaction.now();
                transaction
                    .clear_graph(another.workspace_revision(), another.trace().revision())?;
                Ok(success(&reset, json!({"reset":true}), transaction.now()))
            },
        )
        .unwrap();
    direct
        .read(deadline(), |snapshot| {
            assert!(snapshot.current_graph()?.is_none());
            assert!(snapshot.outcome(local.key())?.is_none());
            assert!(snapshot.outcome(accept.key())?.is_some());
            assert!(snapshot.outcome(reset.key())?.is_some());
            Ok(())
        })
        .unwrap();
    direct.verify_integrity(deadline()).unwrap();
}

// Ports the legacy all-item and retry/history persistence assertions through
// the real direct transaction and cold snapshot boundaries.
#[test]
fn every_item_type_retry_and_manual_rework_preserve_cold_relational_history() {
    use podway_core::{
        ArtifactValueV1, CheckResultExecutorV2, CheckResultInputBasisV2, CheckResultOutcomeV2,
        CheckResultValueV2, OperationId,
    };
    let (_root, direct) = fixture();
    let graph=prepared(json!({"schema":"podway.procedure/v2","id":"direct-all-items","version":"1",
        "name":"All items","purpose":"Round-trip every recorded item type and stale history.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record values.","items":[
            {"id":"confirm","type":"confirm","prompt":"Confirm","required":false},
            {"id":"text","type":"text","prompt":"Text","required":false},
            {"id":"choice","type":"choice","prompt":"Choice","required":false,"choices":["green","red"]},
            {"id":"integer","type":"integer","prompt":"Integer","required":false},
            {"id":"list","type":"list","prompt":"List","required":false},
            {"id":"artifact","type":"artifact","prompt":"Artifact","required":false},
            {"id":"check","type":"check_result","prompt":"Check","required":false,
                "operation_id":"make-test","operation_digest":digest('a'),"accepted_outcomes":["pass"]}]},
            "finish":{"type":"action","title":"Finish","intent":"Finish."}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","next":"finish"},
            {"id":"finish","use":"finish","terminal":true}]},"manual_rework":{"allowed_targets":["work"]}}))
        .begin_v2(Revision::ZERO,attempt(41),None,UnixMillis::new(21)).unwrap().into_state();
    seed(&direct, &graph);
    let check = CheckResultValueV2::new(
        OperationId::new("make-test").unwrap(),
        digest('a'),
        CheckResultInputBasisV2::new("fixture inputs", digest('b')).unwrap(),
        CheckResultExecutorV2::new("fixture", "1").unwrap(),
        CheckResultOutcomeV2::Pass,
        "Passed.",
        digest('c'),
    )
    .unwrap();
    let mutations = [
        ("confirm", ActiveItemMutationV2::Check),
        (
            "text",
            ActiveItemMutationV2::Set {
                value: "ready".into(),
            },
        ),
        (
            "choice",
            ActiveItemMutationV2::Set {
                value: "green".into(),
            },
        ),
        ("integer", ActiveItemMutationV2::Set { value: "-7".into() }),
        (
            "list",
            ActiveItemMutationV2::Add {
                value: "locked".into(),
            },
        ),
        (
            "artifact",
            ActiveItemMutationV2::Attach {
                value: ArtifactValueV1::local_path(
                    "reports/result.txt",
                    digest('c'),
                    42,
                    "text/plain",
                )
                .unwrap(),
            },
        ),
        (
            "check",
            ActiveItemMutationV2::RecordCheckResult { value: check },
        ),
    ];
    for (offset, (item, mutation)) in mutations.into_iter().enumerate() {
        advance(
            &direct,
            &format!("all-item-{item}"),
            "item.set",
            22 + offset as u64,
            |prior, now| {
                let result = prior.mutate_active_item_v2(
                    &attempt(41),
                    &ItemId::new(item).unwrap(),
                    Revision::ZERO,
                    mutation,
                    now,
                )?;
                assert!(result.changed());
                assert_eq!(result.item_revision(), Revision::new(1));
                Ok(result.into_state())
            },
        );
    }
    let recorded = direct
        .prepare_graph(deadline())
        .unwrap()
        .unwrap()
        .graph()
        .clone();
    assert!(
        recorded.workflow_memory().attempts()[0]
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_some())
    );
    let retried = advance(
        &direct,
        "retry-all-items",
        "session.retry",
        30,
        |prior, now| {
            prior
                .retry_active_attempt_v2(
                    prior.trace().revision(),
                    &attempt(41),
                    attempt(42),
                    ReasonV2::new("Fresh recording.").unwrap(),
                    now,
                )
                .map(|outcome| outcome.into_state())
        },
    );
    assert_eq!(
        retried.workflow_memory().attempts()[0],
        recorded.workflow_memory().attempts()[0]
    );
    assert!(
        retried.workflow_memory().attempts()[1]
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none() && slot.revision() == Revision::ZERO)
    );
    advance(
        &direct,
        "complete-retry",
        "session.complete",
        31,
        |prior, now| {
            prior
                .complete_active_action_v2(
                    prior.trace().revision(),
                    &attempt(42),
                    Some(attempt(43)),
                    now,
                )
                .map(|outcome| outcome.into_state())
        },
    );
    let complete = advance(
        &direct,
        "complete-finish",
        "session.complete",
        32,
        |prior, now| {
            prior
                .complete_active_action_v2(prior.trace().revision(), &attempt(43), None, now)
                .map(|outcome| outcome.into_state())
        },
    );
    let reworked = advance(
        &direct,
        "manual-reactivation",
        "session.rework",
        33,
        |prior, now| {
            prior
                .manual_rework_v2(
                    prior.trace().revision(),
                    None,
                    GraphNodeId::new("work").unwrap(),
                    attempt(44),
                    ReasonV2::new("Reopen the completed work.").unwrap(),
                    None,
                    now,
                )
                .map(|outcome| outcome.into_state())
        },
    );
    assert_eq!(
        &reworked.workflow_memory().attempts()[..3],
        complete.workflow_memory().attempts()
    );
    assert_eq!(reworked.workflow_memory().reworks().len(), 1);
    assert!(
        reworked.workflow_memory().attempts()[3]
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
    let cancelled = advance(
        &direct,
        "cancel-reworked",
        "session.cancel",
        34,
        |prior, now| {
            prior
                .cancel_active_session_v2(
                    prior.trace().revision(),
                    &attempt(44),
                    ReasonV2::new("Stop the fixture.").unwrap(),
                    now,
                )
                .map(|outcome| outcome.into_state())
        },
    );
    assert_eq!(
        &cancelled.workflow_memory().attempts()[..3],
        complete.workflow_memory().attempts()
    );
    direct.verify_integrity(deadline()).unwrap();
}

#[test]
fn corrupted_graph_rows_fail_closed_and_doctor_detects_an_orphan_snapshot() {
    use podway_store::direct_store::DirectStoreErrorV1;
    for corruption in ["counter", "digest", "memory", "orphan"] {
        let (_root, direct) = fixture();
        let graph = goal_graph();
        seed(&direct, &graph);
        if corruption != "orphan" {
            advance(
                &direct,
                "begin-corruption",
                "session.begin",
                21,
                |prior, now| {
                    prior
                        .begin_v2(
                            Revision::ZERO,
                            attempt(51),
                            Some(GraphInitialGoalV2::new(
                                GoalStatementV2::new("Preserve invariants.").unwrap(),
                                criteria(),
                                None,
                            )),
                            now,
                        )
                        .map(|outcome| outcome.into_state())
                },
            );
        }
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        connection
            .set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                true,
            )
            .unwrap();
        let sql = match corruption {
            "counter" => {
                "UPDATE v2_graph_node_counters SET attempt_count=7 WHERE graph_node_id='work'"
            }
            "digest" => {
                "UPDATE v2_procedure_snapshots SET digest='sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'"
            }
            "memory" => "DELETE FROM v2_item_slots",
            "orphan" => {
                "INSERT INTO v2_procedure_snapshots(snapshot_id,schema_id,procedure_id,procedure_version,name,purpose,digest,canonical_json,source_kind,source_label,goal_tracking,created_at_ms) SELECT '00000000-0000-4000-8000-000000000099',schema_id,procedure_id,procedure_version,name,purpose,digest,canonical_json,source_kind,source_label,goal_tracking,created_at_ms FROM v2_procedure_snapshots"
            }
            _ => unreachable!(),
        };
        connection.execute(sql, []).unwrap();
        drop(connection);
        let main = std::fs::read(direct.layout().database()).unwrap();
        let wal = std::fs::read(direct.layout().wal()).unwrap();
        if corruption != "orphan" {
            assert!(
                matches!(
                    direct.read(deadline(), |snapshot| snapshot.current_graph()),
                    Err(DirectStoreErrorV1::State(_))
                ),
                "{corruption}"
            );
            let request = request("corrupt-effect");
            assert!(
                direct
                    .mutate(
                        &request,
                        deadline(),
                        || UnixMillis::new(30),
                        |transaction| {
                            transaction.current_graph()?;
                            panic!("corrupt graph must not reach the transition")
                        }
                    )
                    .is_err()
            );
        }
        assert!(direct.verify_integrity(deadline()).is_err(), "{corruption}");
        assert_eq!(std::fs::read(direct.layout().database()).unwrap(), main);
        assert_eq!(std::fs::read(direct.layout().wal()).unwrap(), wal);
    }
}
