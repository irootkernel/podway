//! Retained canonical preset paths through the typed direct Engine boundary.

use podway_core::{CriterionId, GraphNodeId, ItemId, OptionId, Revision};
use podway_engine::{
    direct_api::DirectCommandV1,
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    direct_items::DirectItemCommandV1,
    direct_start::DirectStartCommandV1,
    execution::prepare_preset_procedure_source_v2,
    v2_read_service::GraphStatusTierV2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::NativeGitResolverV1;
use podway_protocol::{
    GoalAssessCriterionV2, GoalCriterionWireV2, InitialGoalWireV2, ItemMutationPreconditionsWireV1,
    ItemRecordManyV1, ItemSetV1, ProcedureV2MutationCommandV1, ReworkPreconditionsWireV2,
    SessionBeginV1, SessionCompleteV1, SessionDecideV2, SessionIdentityPreconditionsWireV1,
    SessionReworkV2, SessionStartSourceV1, SessionStartV1, decode_item_record_many_operations_v1,
};
use serde_json::{Map, Value, json};

use super::{
    int_v2dfc006_direct_execution::{Clock, digest, fixture},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};

struct Session {
    engine: Engine,
    next_key: u64,
    criterion: Option<&'static str>,
}

impl Session {
    fn new(preset: &str, criterion: Option<&'static str>) -> Self {
        let (temporary, selector, store, _) = fixture();
        let engine = Engine {
            temporary,
            selector,
            store,
            resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
        };
        let source = prepare_preset_procedure_source_v2(preset).unwrap().unwrap();
        let mut session = Self {
            engine,
            next_key: 0,
            criterion,
        };
        let start = session.mutate(
            DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartV1 {
                source: SessionStartSourceV1::Preset {
                    preset: preset.into(),
                },
                expected_procedure_digest: Some(source.digest().clone()),
                task_title: "Retained canonical preset reference path".into(),
                dry_run: false,
            })),
            "start",
        );
        assert_eq!(start["procedure_digest"], json!(source.digest()));
        let graph = session.engine.graph();
        let initial_goal = criterion.map(|criterion| InitialGoalWireV2 {
            goal: "Complete the bounded path from fresh recorded evidence.".into(),
            criteria: vec![GoalCriterionWireV2 {
                criterion_id: CriterionId::new(criterion).unwrap(),
                statement: "Fresh evidence supports the declared terminal outcome.".into(),
            }],
            actor: None,
        });
        session.mutate(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal,
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: graph.trace().session_id().clone(),
                        expected_session_revision: Revision::ZERO,
                    },
                },
            )),
            "begin",
        );
        session
    }

    fn key(&mut self, label: &str) -> String {
        self.next_key += 1;
        format!("reference-{label}-{}", self.next_key)
    }

    fn mutate(&mut self, command: DirectCommandV1, label: &str) -> Map<String, Value> {
        let key = self.key(label);
        let request = self.engine.request(command, &key);
        let original = self
            .engine
            .output(self.engine.execute(request.clone(), &Clock));
        assert_eq!(
            self.engine.output(self.engine.execute(request, &NoClock)),
            original
        );
        original["result"].as_object().unwrap().clone()
    }
}

fn status(session: &mut Session) -> Map<String, Value> {
    session
        .engine
        .read(DirectReadCommandV1::Status {
            tier: GraphStatusTierV2::Verbose,
            history_before: None,
        })
        .as_object()
        .unwrap()
        .clone()
}

fn assert_node(session: &mut Session, expected: &str) -> Map<String, Value> {
    let current = status(session);
    assert_eq!(current["current"]["node"]["graph_node_id"], expected);
    current
}

fn set_item(session: &mut Session, item: &str, value: Value) {
    let graph = session.engine.graph();
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    let item_id = ItemId::new(item).unwrap();
    let revision = graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap()
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id)
        .unwrap()
        .revision();
    session.mutate(
        DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
            item_id,
            value: value.as_str().unwrap().into(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_attempt_id: attempt.clone(),
                expected_item_revision: revision,
            },
        })),
        item,
    );
}

fn record_many(session: &mut Session, operations: Value, label: &str) {
    session.mutate(
        DirectCommandV1::RecordMany(ItemRecordManyV1 {
            operations: decode_item_record_many_operations_v1(operations).unwrap(),
            preconditions: session.engine.fences(),
        }),
        label,
    );
}

fn complete(session: &mut Session) -> Map<String, Value> {
    session.mutate(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: session.engine.fences(),
        })),
        "complete",
    )
}

fn decide_at(session: &mut Session, node: &str, option: &str) -> Map<String, Value> {
    assert_node(session, node);
    session.mutate(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
            SessionDecideV2 {
                option_id: OptionId::new(option).unwrap(),
                reason: "The fresh recorded evidence supports this declared route.".into(),
                actor: Some("Direct preset reference path".into()),
                preconditions: session.engine.fences(),
                expected_goal_revision: session.criterion.map(|_| 1),
            },
        )),
        "decide",
    )
}

fn rework(session: &mut Session, target: &str) -> Map<String, Value> {
    let graph = session.engine.graph();
    session.mutate(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionRework(
            SessionReworkV2 {
                target_graph_node_id: GraphNodeId::new(target).unwrap(),
                reason: "Exercise this exact declared manual rework target.".into(),
                actor: Some("Direct preset reference path".into()),
                preconditions: ReworkPreconditionsWireV2 {
                    expected_session_id: graph.trace().session_id().clone(),
                    expected_session_revision: graph.trace().revision(),
                    expected_attempt_id: graph
                        .trace()
                        .active_attempt()
                        .map(|attempt| attempt.attempt_id().clone()),
                },
            },
        )),
        "rework",
    )
}

fn assess_criterion(session: &mut Session, criterion_status: &str, evidence: &str) {
    assert_node(session, "assess-goal");
    session.mutate(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriterion(
            GoalAssessCriterionV2 {
                criterion_id: CriterionId::new(session.criterion.unwrap()).unwrap(),
                status: criterion_status.into(),
                reason: "The selected fresh evidence supports this criterion outcome.".into(),
                evidence: if criterion_status == "not_applicable" {
                    vec![]
                } else {
                    vec![GraphNodeId::new(evidence).unwrap()]
                },
                items: vec![],
                actor: Some("Direct preset reference path".into()),
                preconditions: session.engine.fences(),
                expected_goal_revision: 1,
            },
        )),
        "assess",
    );
}

mod software_development {
    use super::*;
    fn batch(session: &mut Session, values: Vec<(&str, Value)>) {
        record_many(session,Value::Array(values.into_iter().map(|(id,record)|
            json!({"item_id":id,"expected_item_revision":0,"record":record})).collect()),"sw-batch");
    }
    fn plan(session: &mut Session) {
        assert_node(session, "plan");
        batch(
            session,
            vec![
                (
                    "scope-summary",
                    json!({"type":"text","value":"Exercise the complete isolated reference path."}),
                ),
                (
                    "success-criteria",
                    json!({"type":"list","value":["Fresh evidence supports the terminal outcome."]}),
                ),
                (
                    "risk-summary",
                    json!({"type":"text","value":"Keep all state within the disposable worktree."}),
                ),
            ],
        );
        complete(session);
    }
    fn implement(session: &mut Session) {
        assert_node(session, "implement");
        batch(
            session,
            vec![
                (
                    "implementation-summary",
                    json!({"type":"text","value":"Implemented the bounded candidate."}),
                ),
                (
                    "changed-boundaries",
                    json!({"type":"list","value":["Disposable state boundary"]}),
                ),
            ],
        );
        complete(session);
    }
    fn verification(session: &mut Session, outcome: &str, observations: bool) {
        assert_node(session, "verify");
        let mut values = vec![(
            "verification-result",
            json!({"type":"check_result",
            "operation_id":"software-change-verification",
            "operation_digest":"sha256:b904aefd4dbd6b01337645fd34b1424efc9faf3d22c936de666305335076969e",
            "input_basis":{"descriptor":"Disposable reference candidate","digest":format!("sha256:{}","a".repeat(64))},
            "executor":{"name":"Native Engine reference fixture","version":"1"},"outcome":outcome,
            "summary":"The caller records this outcome without execution or attestation by Podway.",
            "output_digest":format!("sha256:{}","b".repeat(64))}),
        )];
        if observations {
            values.push((
                "verification-observations",
                json!({"type":"list","value":["The caller reports a bounded verification gap."]}),
            ));
        }
        batch(session, values);
    }
    fn document(session: &mut Session, updated: bool, summary: bool) {
        assert_node(session, "document");
        set_item(
            session,
            "documentation-state",
            json!(if updated { "updated" } else { "not-applicable" }),
        );
        if summary {
            set_item(
                session,
                "documentation-summary",
                json!("Documented the current candidate behavior."),
            );
        }
    }
    fn review(
        session: &mut Session,
        implementation: u64,
        documentation: u64,
        findings: Vec<String>,
    ) {
        assert_node(session, "review");
        batch(
            session,
            vec![
                (
                    "review-summary",
                    json!({"type":"text","value":"Reviewed the fresh candidate and its recorded evidence."}),
                ),
                (
                    "unresolved-implementation-findings",
                    json!({"type":"integer","value":implementation}),
                ),
                (
                    "unresolved-documentation-findings",
                    json!({"type":"integer","value":documentation}),
                ),
                ("review-findings", json!({"type":"list","value":findings})),
            ],
        );
        complete(session);
    }
    fn reach_document(session: &mut Session) {
        implement(session);
        verification(session, "pass", false);
        complete(session);
        decide_at(session, "evaluate-verification", "acceptable");
        assert_node(session, "document");
    }
    fn finish(session: &mut Session, result: &str, option: &str) {
        document(session, false, false);
        complete(session);
        review(session, 0, 0, vec![]);
        decide_at(session, "evaluate-review", "approved");
        assess_criterion(session, result, "verify");
        decide_at(session, "assess-goal", option);
        set_item(
            session,
            "closeout-note",
            json!("The recorded outcome is ready for closeout."),
        );
        complete(session);
        let terminal = status(session);
        assert_eq!(terminal["session"]["lifecycle"], "completed");
        assert_eq!(
            terminal["goal"]["determined_outcome"],
            option.replace('-', "_")
        );
    }
    fn missing(session: &mut Session, label: &str) {
        let key = session.key(label);
        let command = DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: session.engine.fences(),
        }));
        session
            .engine
            .reject(command, &key, "REQUIRED_ITEMS_MISSING", &Clock);
    }
    #[test]
    fn sw_dev_clean_and_every_goal_outcome_reach_native_closeout() {
        for (result, option) in [
            ("satisfied", "achieved"),
            ("unsatisfied", "not-achieved"),
            ("not_applicable", "superseded"),
        ] {
            let mut session = Session::new("sw-dev-v2", Some("verified"));
            plan(&mut session);
            reach_document(&mut session);
            finish(&mut session, result, option);
        }
    }
    #[test]
    fn sw_dev_verification_retry_and_documentation_condition_preserve_freshness() {
        for outcome in ["fail", "inconclusive"] {
            let mut session = Session::new("sw-dev-v2", Some("verified"));
            plan(&mut session);
            implement(&mut session);
            verification(&mut session, outcome, false);
            missing(&mut session, "missing-observations");
            batch(
                &mut session,
                vec![(
                    "verification-observations",
                    json!({"type":"list","value":["Fresh observations require another implementation attempt."]}),
                )],
            );
            complete(&mut session);
            let result = decide_at(&mut session, "evaluate-verification", "retry");
            assert_eq!(result["effect"], "rework");
            assert_node(&mut session, "implement");
            let before = status(&mut session);
            session.engine.cold();
            assert_eq!(status(&mut session), before);
            reach_document(&mut session);
            document(&mut session, true, false);
            missing(&mut session, "missing-doc-summary");
            set_item(
                &mut session,
                "documentation-summary",
                json!("The candidate has a durable documentation update."),
            );
            complete(&mut session);
            review(&mut session, 0, 0, vec![]);
            decide_at(&mut session, "evaluate-review", "approved");
            assess_criterion(&mut session, "satisfied", "verify");
            decide_at(&mut session, "assess-goal", "achieved");
            set_item(
                &mut session,
                "closeout-note",
                json!("The corrected candidate is complete."),
            );
            complete(&mut session);
        }
    }
    #[test]
    fn sw_dev_review_phase_owner_routes_and_paging_reject_stale_evidence() {
        let mut session = Session::new("sw-dev-v2", Some("verified"));
        plan(&mut session);
        reach_document(&mut session);
        document(&mut session, false, false);
        complete(&mut session);
        let findings: Vec<_> = (0..24)
            .map(|n| format!("Implementation finding {n:03}: {}", "x".repeat(3500)))
            .collect();
        review(&mut session, 1, 1, findings.clone());
        let command = |token| DirectReadCommandV1::Evidence {
            source: GraphNodeId::new("review").unwrap(),
            item_id: ItemId::new("review-findings").unwrap(),
            token,
        };
        let mut page = session.engine.read(command(None));
        let stale = page["next_page_token"].as_str().unwrap().to_owned();
        let digest = page["value_digest"].clone();
        let mut observed = Vec::new();
        loop {
            observed.extend(
                page["page"]["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned()),
            );
            let Some(token) = page["next_page_token"].as_str() else {
                break;
            };
            let token = podway_protocol::EvidencePageTokenV1::decode(token).unwrap();
            session.engine.cold();
            page = session.engine.read(command(Some(token)));
            assert_eq!(page["value_digest"], digest);
        }
        assert_eq!(observed, findings);
        let result = decide_at(&mut session, "evaluate-review", "implementation-changes");
        assert_eq!(result["target_graph_node_id"], "implement");
        reach_document(&mut session);
        document(&mut session, false, false);
        complete(&mut session);
        review(
            &mut session,
            0,
            1,
            vec!["One documentation finding remains.".into()],
        );
        let key = session.key("stale-page");
        session.engine.reject(
            DirectCommandV1::Read {
                command: command(Some(
                    podway_protocol::EvidencePageTokenV1::decode(&stale).unwrap(),
                )),
                expected_session_id: None,
            },
            &key,
            "EVIDENCE_PAGE_TOKEN_STALE",
            &Clock,
        );
        let result = decide_at(&mut session, "evaluate-review", "documentation-changes");
        assert_eq!(result["target_graph_node_id"], "document");
        finish(&mut session, "satisfied", "achieved");
    }
    #[test]
    fn sw_dev_accepts_every_declared_manual_rework_target_and_then_closes() {
        for target in ["plan", "implement", "verify", "document", "review"] {
            let mut session = Session::new("sw-dev-v2", Some("verified"));
            plan(&mut session);
            reach_document(&mut session);
            document(&mut session, false, false);
            complete(&mut session);
            review(&mut session, 0, 0, vec![]);
            let result = rework(&mut session, target);
            assert_eq!(result["to_graph_node_id"], target);
            match target {
                "plan" => {
                    plan(&mut session);
                    reach_document(&mut session);
                }
                "implement" => reach_document(&mut session),
                "verify" => {
                    verification(&mut session, "pass", false);
                    complete(&mut session);
                    decide_at(&mut session, "evaluate-verification", "acceptable");
                }
                "document" | "review" => {}
                _ => unreachable!(),
            }
            if target == "review" {
                review(&mut session, 0, 0, vec![]);
                decide_at(&mut session, "evaluate-review", "approved");
                assess_criterion(&mut session, "satisfied", "verify");
                decide_at(&mut session, "assess-goal", "achieved");
                set_item(
                    &mut session,
                    "closeout-note",
                    json!("The manually reworked path is complete."),
                );
                complete(&mut session);
            } else {
                finish(&mut session, "satisfied", "achieved");
            }
        }
    }
}

mod small_change {
    use super::*;
    fn decide(session: &mut Session, option: &str) -> Map<String, Value> {
        decide_at(session, "review", option)
    }
    fn retry(session: &mut Session) -> Map<String, Value> {
        session.mutate(
            session
                .engine
                .retry("Repeat this node with fresh evidence.".into()),
            "retry",
        )
    }
    fn complete_action(session: &mut Session, node: &str) {
        assert_node(session, node);
        match node {
            "inspect" => set_item(
                session,
                "scope-summary",
                json!("The change is bounded to the requested implementation and tests."),
            ),
            "implement" => set_item(
                session,
                "implementation-summary",
                json!("Implemented the bounded change without unrelated edits."),
            ),
            "verify" => {
                set_item(session, "verification-command", json!("make test"));
                set_item(session, "verification-exit-status", json!("0"));
            }
            "closeout" => set_item(
                session,
                "closeout-note",
                json!("The reviewed and verified small change is complete."),
            ),
            _ => panic!("unsupported action node {node}"),
        }
        complete(session);
    }
    fn reach_review(session: &mut Session) {
        for node in ["inspect", "implement", "verify"] {
            complete_action(session, node);
        }
        assert_node(session, "review");
    }
    fn finish(session: &mut Session) {
        let decision = decide(session, "ready");
        assert_eq!(decision["target_graph_node_id"], "closeout");
        complete_action(session, "closeout");
        let terminal = status(session);
        assert_eq!(terminal["session"]["lifecycle"], "completed");
        assert!(!terminal.contains_key("goal"));
    }

    #[test]
    fn small_change_happy_path_closes_without_a_goal() {
        let mut session = Session::new("small-change-v2", None);

        reach_review(&mut session);
        finish(&mut session);
    }

    #[test]
    fn small_change_review_rework_returns_to_implementation_and_then_closes() {
        let mut session = Session::new("small-change-v2", None);

        reach_review(&mut session);
        let decision = decide(&mut session, "changes-requested");
        assert_eq!(decision["target_graph_node_id"], "implement");
        assert_eq!(decision["effect"], "rework");
        complete_action(&mut session, "implement");
        complete_action(&mut session, "verify");
        finish(&mut session);
    }

    #[test]
    fn small_change_retry_discards_the_verify_attempt_and_requires_fresh_items() {
        let mut session = Session::new("small-change-v2", None);

        complete_action(&mut session, "inspect");
        complete_action(&mut session, "implement");
        assert_node(&mut session, "verify");
        set_item(
            &mut session,
            "verification-command",
            json!("cargo test --locked"),
        );
        set_item(&mut session, "verification-exit-status", json!("0"));
        let before = status(&mut session);
        let old_attempt = before["current"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let retried = retry(&mut session);
        assert_ne!(retried["to_attempt_id"], old_attempt);
        let after = assert_node(&mut session, "verify");
        assert_eq!(after["current"]["missing_required_item_count"], 2);
        assert!(after["item_values"].as_array().unwrap().is_empty());
        complete_action(&mut session, "verify");
        finish(&mut session);
    }

    #[test]
    fn small_change_manual_rework_accepts_each_declared_target_and_then_closes() {
        let mut session = Session::new("small-change-v2", None);

        reach_review(&mut session);
        for target in ["verify", "implement", "inspect"] {
            let result = rework(&mut session, target);
            assert_eq!(result["to_graph_node_id"], target);
            match target {
                "verify" => complete_action(&mut session, "verify"),
                "implement" => {
                    complete_action(&mut session, "implement");
                    complete_action(&mut session, "verify");
                }
                "inspect" => {
                    complete_action(&mut session, "inspect");
                    complete_action(&mut session, "implement");
                    complete_action(&mut session, "verify");
                }
                _ => unreachable!(),
            }
            assert_node(&mut session, "review");
        }
        finish(&mut session);
    }
}

mod bug_fix {
    use super::*;
    fn decide(session: &mut Session, node: &str, option: &str) -> Map<String, Value> {
        decide_at(session, node, option)
    }
    fn assess(session: &mut Session, criterion_status: &str, option: &str) {
        assess_criterion(session, criterion_status, "verify");
        decide(session, "assess-goal", option);
        assert_node(session, "closeout");
        set_item(
            session,
            "closeout-note",
            json!("The declared goal outcome reached closeout."),
        );
        complete(session);
        let terminal = status(session);
        assert_eq!(terminal["session"]["lifecycle"], "completed");
        assert_eq!(
            terminal["goal"]["determined_outcome"],
            option.replace('-', "_")
        );
    }
    fn complete_reproduce(session: &mut Session) {
        assert_node(session, "reproduce");
        set_item(session, "reproduction-status", json!("reproduced"));
        set_item(
            session,
            "observed-behavior",
            json!("The disposable request produces the reproduced defect."),
        );
        set_item(
            session,
            "expected-behavior",
            json!("The disposable request completes without the defect."),
        );
        set_item(
            session,
            "regression-check",
            json!("Run the disposable regression check."),
        );
        complete(session);
    }
    fn complete_diagnose(session: &mut Session) {
        assert_node(session, "diagnose");
        set_item(
            session,
            "cause",
            json!("The disposable state transition accepts a stale boundary."),
        );
        set_item(
            session,
            "affected-boundary",
            json!("Disposable session transition"),
        );
        complete(session);
    }
    fn complete_implement(session: &mut Session) {
        assert_node(session, "implement");
        record_many(
            session,
            json!([
                {"item_id":"fix-summary","expected_item_revision":0,"record":{"type":"text","value":"Reject the stale disposable transition."}},
                {"item_id":"changed-boundaries","expected_item_revision":0,"record":{"type":"list","value":["Disposable session transition"]}}
            ]),
            "implementation",
        );
        complete(session);
    }
    fn complete_verify(session: &mut Session, outcome: &str) {
        assert_node(session, "verify");
        record_many(
            session,
            json!([{
                "item_id":"verification-result",
                "expected_item_revision":0,
                "record":{
                    "type":"check_result",
                    "operation_id":"bug-fix-verification",
                    "operation_digest":"sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979",
                    "input_basis":{
                        "descriptor":"Disposable bug-fix candidate",
                        "digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    },
                    "executor":{"name":"V2REF-005 runtime dogfood","version":"1"},
                    "outcome":outcome,
                    "summary":format!("The caller records the {outcome} result in English."),
                    "output_digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }
            }]),
            "verification",
        );
        complete(session);
    }
    fn complete_review(session: &mut Session, findings: i64) {
        assert_node(session, "review");
        let mut operations = vec![
            json!({"item_id":"review-summary","expected_item_revision":0,"record":{"type":"text","value":"The disposable bug fix was reviewed against its fresh evidence."}}),
            json!({"item_id":"unresolved-valid-findings","expected_item_revision":0,"record":{"type":"integer","value":findings}}),
        ];
        if findings > 0 {
            operations.push(json!({"item_id":"review-findings","expected_item_revision":0,"record":{"type":"list","value":["One disposable finding requires implementation rework."]}}));
        }
        record_many(session, Value::Array(operations), "review");
        complete(session);
    }
    fn reach_verification(session: &mut Session) {
        complete_reproduce(session);
        complete_diagnose(session);
        complete_implement(session);
    }
    fn reach_assessment(session: &mut Session) {
        reach_verification(session);
        complete_verify(session, "pass");
        decide(session, "evaluate-verification", "passed");
        complete_review(session, 0);
        decide(session, "evaluate-review", "approved");
        assert_node(session, "assess-goal");
    }
    fn rebuild_to_assessment(session: &mut Session, target: &str) {
        match target {
            "reproduce" => complete_reproduce(session),
            "diagnose" => {}
            "implement" | "verify" | "review" => {}
            _ => unreachable!(),
        }
        if matches!(target, "reproduce" | "diagnose") {
            complete_diagnose(session);
        }
        if matches!(target, "reproduce" | "diagnose" | "implement") {
            complete_implement(session);
        }
        if matches!(target, "reproduce" | "diagnose" | "implement" | "verify") {
            complete_verify(session, "pass");
            decide(session, "evaluate-verification", "passed");
        }
        complete_review(session, 0);
        decide(session, "evaluate-review", "approved");
        assert_node(session, "assess-goal");
    }

    #[test]
    fn v2ref005_bug_fix_guarded_failure_and_review_rework_reach_achieved_closeout() {
        let mut session = Session::new("bug-fix-v2", Some("verified"));

        reach_verification(&mut session);
        complete_verify(&mut session, "inconclusive");
        let retry = decide(&mut session, "evaluate-verification", "retry");
        assert_eq!(retry["effect"], "rework");
        assert_eq!(retry["target_graph_node_id"], "implement");
        complete_implement(&mut session);
        complete_verify(&mut session, "pass");
        decide(&mut session, "evaluate-verification", "passed");
        complete_review(&mut session, 1);
        let changes = decide(&mut session, "evaluate-review", "changes-requested");
        assert_eq!(changes["effect"], "rework");
        assert_eq!(changes["target_graph_node_id"], "implement");
        complete_implement(&mut session);
        complete_verify(&mut session, "pass");
        decide(&mut session, "evaluate-verification", "passed");
        complete_review(&mut session, 0);
        decide(&mut session, "evaluate-review", "approved");
        assess(&mut session, "satisfied", "achieved");
    }

    #[test]
    fn v2ref005_bug_fix_goal_not_achieved_and_superseded_paths_are_reachable() {
        for (criterion_status, option) in [
            ("unsatisfied", "not-achieved"),
            ("not_applicable", "superseded"),
        ] {
            let mut session = Session::new("bug-fix-v2", Some("verified"));
            reach_assessment(&mut session);
            assess(&mut session, criterion_status, option);
        }
    }

    #[test]
    fn v2ref005_bug_fix_accepts_every_declared_manual_rework_target() {
        let mut session = Session::new("bug-fix-v2", Some("verified"));
        reach_assessment(&mut session);

        for target in ["review", "verify", "implement", "diagnose", "reproduce"] {
            let result = rework(&mut session, target);
            assert_eq!(result["to_graph_node_id"], target);
            rebuild_to_assessment(&mut session, target);
        }
        assess(&mut session, "satisfied", "achieved");
    }
}

mod analysis {
    use super::*;
    fn decide(session: &mut Session, node: &str, option: &str) -> Map<String, Value> {
        decide_at(session, node, option)
    }
    fn assess(session: &mut Session, criterion_status: &str, option: &str) {
        assess_criterion(session, criterion_status, "synthesize");
        decide(session, "assess-goal", option);
        assert_node(session, "closeout");
        record_many(
            session,
            json!([{"item_id":"closeout-note","expected_item_revision":0,"record":{"type":"text","value":"The declared goal outcome reached closeout."}}]),
            "closeout",
        );
        complete(session);
        let terminal = status(session);
        assert_eq!(terminal["session"]["lifecycle"], "completed");
        assert_eq!(
            terminal["goal"]["determined_outcome"],
            option.replace('-', "_")
        );
    }
    fn complete_define(session: &mut Session) {
        assert_node(session, "define");
        record_many(
            session,
            json!([
                {"item_id":"analysis-question","expected_item_revision":0,"record":{"type":"text","value":"Which bounded option is best supported by the repository evidence?"}},
                {"item_id":"decision-context","expected_item_revision":0,"record":{"type":"text","value":"Support one maintainership decision."}},
                {"item_id":"scope-and-constraints","expected_item_revision":0,"record":{"type":"list","value":["Use repository-owned evidence only."]}}
            ]),
            "define",
        );
        complete(session);
    }
    fn complete_collect(session: &mut Session) {
        assert_node(session, "collect");
        record_many(
            session,
            json!([
                {"item_id":"sources","expected_item_revision":0,"record":{"type":"list","value":["Canonical specification", "Executable runtime test"]}},
                {"item_id":"source-limitations","expected_item_revision":0,"record":{"type":"list","value":["The evidence is bounded to this disposable scenario."]}}
            ]),
            "collect",
        );
        complete(session);
    }
    fn complete_analyze(session: &mut Session) {
        assert_node(session, "analyze");
        record_many(
            session,
            json!([
                {"item_id":"analysis-summary","expected_item_revision":0,"record":{"type":"text","value":"The specification and runtime evidence support the bounded option."}},
                {"item_id":"key-findings","expected_item_revision":0,"record":{"type":"list","value":["The canonical and runtime evidence agree."]}},
                {"item_id":"assumptions","expected_item_revision":0,"record":{"type":"list","value":[]}}
            ]),
            "analyze",
        );
        complete(session);
    }
    fn complete_challenge(session: &mut Session, blocking_gaps: Vec<&str>) {
        assert_node(session, "challenge");
        record_many(
            session,
            json!([
                {"item_id":"counterarguments","expected_item_revision":0,"record":{"type":"list","value":["A broader environment could produce a different result."]}},
                {"item_id":"uncertainty","expected_item_revision":0,"record":{"type":"text","value":"The conclusion is limited to the recorded evidence boundary."}},
                {"item_id":"blocking-gaps","expected_item_revision":0,"record":{"type":"list","value":blocking_gaps}}
            ]),
            "challenge",
        );
        complete(session);
    }
    fn complete_synthesis(session: &mut Session) {
        assert_node(session, "synthesize");
        record_many(
            session,
            json!([
                {"item_id":"conclusion","expected_item_revision":0,"record":{"type":"text","value":"The bounded option is supported by the fresh evidence."}},
                {"item_id":"recommendation","expected_item_revision":0,"record":{"type":"text","value":"Adopt the bounded option and retain the stated limitation."}},
                {"item_id":"confidence","expected_item_revision":0,"record":{"type":"choice","value":"high"}},
                {"item_id":"residual-risks","expected_item_revision":0,"record":{"type":"list","value":["Evidence outside the declared scope may change the conclusion."]}}
            ]),
            "synthesis",
        );
        complete(session);
    }
    fn complete_review(session: &mut Session, source_findings: i64, analysis_findings: i64) {
        assert_node(session, "review");
        let mut operations = vec![
            json!({"item_id":"review-summary","expected_item_revision":0,"record":{"type":"text","value":"The source basis and analysis claims were reviewed separately."}}),
            json!({"item_id":"unresolved-source-findings","expected_item_revision":0,"record":{"type":"integer","value":source_findings}}),
            json!({"item_id":"unresolved-analysis-findings","expected_item_revision":0,"record":{"type":"integer","value":analysis_findings}}),
        ];
        if source_findings > 0 {
            operations.push(json!({"item_id":"source-findings","expected_item_revision":0,"record":{"type":"list","value":["One source finding requires collection rework."]}}));
        }
        if analysis_findings > 0 {
            operations.push(json!({"item_id":"analysis-findings","expected_item_revision":0,"record":{"type":"list","value":["One reasoning finding requires analysis rework."]}}));
        }
        record_many(session, Value::Array(operations), "review");
        complete(session);
    }
    fn reach_evidence_decision(session: &mut Session) {
        complete_define(session);
        complete_collect(session);
        complete_analyze(session);
        complete_challenge(session, Vec::new());
        assert_node(session, "evaluate-evidence");
    }
    fn rebuild_from(session: &mut Session, target: &str) {
        if target == "define" {
            complete_define(session);
        }
        if matches!(target, "define" | "collect") {
            complete_collect(session);
        }
        if matches!(target, "define" | "collect" | "analyze") {
            complete_analyze(session);
        }
        if matches!(target, "define" | "collect" | "analyze" | "challenge") {
            complete_challenge(session, Vec::new());
            decide(session, "evaluate-evidence", "sufficient");
        }
        if matches!(
            target,
            "define" | "collect" | "analyze" | "challenge" | "synthesize"
        ) {
            complete_synthesis(session);
        }
        complete_review(session, 0, 0);
        decide(session, "evaluate-review", "approved");
        assert_node(session, "assess-goal");
    }

    #[test]
    fn v2ref_analysis_guarded_and_phase_owner_rework_reach_achieved_closeout() {
        let mut session = Session::new("analysis-v2", Some("decision-ready"));

        complete_define(&mut session);
        complete_collect(&mut session);
        complete_analyze(&mut session);
        complete_challenge(
            &mut session,
            vec!["One authoritative source is still required."],
        );
        let research = decide(&mut session, "evaluate-evidence", "more-research");
        assert_eq!(research["effect"], "rework");
        assert_eq!(research["target_graph_node_id"], "collect");
        complete_collect(&mut session);
        complete_analyze(&mut session);
        complete_challenge(&mut session, Vec::new());
        decide(&mut session, "evaluate-evidence", "sufficient");
        complete_synthesis(&mut session);

        complete_review(&mut session, 1, 0);
        let source = decide(&mut session, "evaluate-review", "source-changes");
        assert_eq!(source["target_graph_node_id"], "collect");
        complete_collect(&mut session);
        complete_analyze(&mut session);
        complete_challenge(&mut session, Vec::new());
        decide(&mut session, "evaluate-evidence", "sufficient");
        complete_synthesis(&mut session);
        complete_review(&mut session, 0, 1);
        let analysis = decide(&mut session, "evaluate-review", "analysis-changes");
        assert_eq!(analysis["target_graph_node_id"], "analyze");
        complete_analyze(&mut session);
        complete_challenge(&mut session, Vec::new());
        decide(&mut session, "evaluate-evidence", "sufficient");
        complete_synthesis(&mut session);
        complete_review(&mut session, 0, 0);
        decide(&mut session, "evaluate-review", "approved");
        assess(&mut session, "satisfied", "achieved");
    }

    #[test]
    fn v2ref_analysis_conditional_review_findings_block_completion_when_missing() {
        for (source_count, analysis_count, missing_item) in
            [(1, 0, "source-findings"), (0, 1, "analysis-findings")]
        {
            let mut session = Session::new("analysis-v2", Some("decision-ready"));
            reach_evidence_decision(&mut session);
            decide(&mut session, "evaluate-evidence", "sufficient");
            complete_synthesis(&mut session);
            assert_node(&mut session, "review");
            record_many(
                &mut session,
                json!([
                    {"item_id":"review-summary","expected_item_revision":0,"record":{"type":"text","value":"The review records one unresolved owned finding."}},
                    {"item_id":"unresolved-source-findings","expected_item_revision":0,"record":{"type":"integer","value":source_count}},
                    {"item_id":"unresolved-analysis-findings","expected_item_revision":0,"record":{"type":"integer","value":analysis_count}}
                ]),
                "conditional-review",
            );
            let blocked = status(&mut session);
            assert_eq!(blocked["current"]["missing_required_item_count"], 1);
            assert!(
                blocked["missing_required_item_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|id| id == missing_item)
            );
            let command =
                DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                    preconditions: session.engine.fences(),
                }));
            session.engine.reject(
                command,
                "blocked-complete",
                "REQUIRED_ITEMS_MISSING",
                &Clock,
            );
        }
    }

    #[test]
    fn v2ref_analysis_goal_not_achieved_and_superseded_paths_are_reachable() {
        for (criterion_status, option) in [
            ("unsatisfied", "not-achieved"),
            ("not_applicable", "superseded"),
        ] {
            let mut session = Session::new("analysis-v2", Some("decision-ready"));
            reach_evidence_decision(&mut session);
            decide(&mut session, "evaluate-evidence", "sufficient");
            complete_synthesis(&mut session);
            complete_review(&mut session, 0, 0);
            decide(&mut session, "evaluate-review", "approved");
            assess(&mut session, criterion_status, option);
        }
    }

    #[test]
    fn v2ref_analysis_accepts_every_declared_manual_rework_target() {
        let mut session = Session::new("analysis-v2", Some("decision-ready"));
        reach_evidence_decision(&mut session);
        decide(&mut session, "evaluate-evidence", "sufficient");
        complete_synthesis(&mut session);
        complete_review(&mut session, 0, 0);
        decide(&mut session, "evaluate-review", "approved");

        for target in [
            "review",
            "synthesize",
            "challenge",
            "analyze",
            "collect",
            "define",
        ] {
            let result = rework(&mut session, target);
            assert_eq!(result["to_graph_node_id"], target);
            rebuild_from(&mut session, target);
        }
        assess(&mut session, "satisfied", "achieved");
    }
}
