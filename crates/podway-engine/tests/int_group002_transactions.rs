//! Actual successor Engine and SQLite transactions in isolated candidate fixtures.

use podway_config::ProcedureDocumentFormat;
use podway_config::execution_groups::{
    SuccessorDefinition, ValidatedSuccessorDefinition, parse_successor_document,
    validate_successor_definition,
};
use podway_core::execution_groups::*;
use podway_core::{
    AttemptLifecycle, CheckResultInputBasisV2, DomainError, ItemId, RecordedItemValueV2, Revision,
    SessionId, SessionLifecycle, UnixMillis, WorkspaceId,
};
use podway_engine::direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1};
use podway_engine::successor_execution::{
    GroupActivationRequest, SuccessorExecutionError, SuccessorMutationRequest, activate_group,
    return_from_group,
};
use podway_protocol::RequestIdV1;
use podway_store::direct_store::{
    DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreErrorV1,
};
use podway_store::successor_store::{
    CandidateDefinition, CandidateExecutionStore, CandidateSources, CandidateStoreError,
    CandidateSuccess, persist_candidate_execution,
};
use podway_store::{CanonicalExecutionJsonV1, IdempotencyKeyV1, ValidatedWorkspaceRootV1};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::PathBuf;
use tempfile::TempDir;

pub(super) const TIME: UnixMillis = UnixMillis::new(1_700_000_000_000);

pub(super) fn action(title: &str) -> Value {
    json!({ "type": "action", "title": title, "intent": "Perform the declared task." })
}

fn item(id: &str) -> Value {
    json!({"id":id, "type":"text", "prompt":"Record the value.", "required":true})
}

pub(super) fn base_document() -> Value {
    let mut design = action("Design");
    design["items"] = json!([item("plan")]);
    json!({
        "schema":"podway.procedure/v3", "id":"candidate-base", "version":"1",
        "name":"Candidate base", "purpose":"Verify the transaction boundary.",
        "node_definitions":{"design":design, "review":action("Review"), "close":action("Close")},
        "graph":{"entry":"design","nodes":[
            {"id":"design","use":"design","next":"review"},
            {"id":"review","use":"review","next":"close",
             "evidence_from":[{"scope":"execution_result","required":true,"items":["report"]}]},
            {"id":"close","use":"close","terminal":true}
        ]},
        "execution_group":{"anchor":"design","success_return":"review",
            "plan_inputs":["plan"],"result_items":[item("report")],"replan_decisions":[]}
    })
}

pub(super) fn group_document() -> Value {
    let mut finish = action("Finish");
    finish["items"] = json!([item("report")]);
    json!({
        "schema":"podway.execution-group/v1","id":"candidate-group","version":"1",
        "name":"Candidate group","purpose":"Exercise a real group transition.",
        "plan_inputs":[item("plan")],
        "node_definitions":{"work":action("Work"),"finish":finish},
        "graph":{"entry":"work","nodes":[
            {"id":"work","use":"work","next":"finish",
             "evidence_from":[{"scope":"plan","required":true,"items":["plan"]}]},
            {"id":"finish","use":"finish","terminal":true}
        ]}
    })
}

pub(super) fn canonical(document: &Value) -> ValidatedSuccessorDefinition {
    admit(&document.to_string()).unwrap()
}

fn admit(text: &str) -> std::result::Result<ValidatedSuccessorDefinition, CandidateStoreError> {
    let rejected = |_| CandidateStoreError::Corrupt("fixture source failed Config admission");
    validate_successor_definition(
        parse_successor_document(text.as_bytes(), ProcedureDocumentFormat::Json)
            .map_err(rejected)?,
    )
    .map_err(rejected)
}

fn definition(text: &str) -> std::result::Result<CandidateDefinition, CandidateStoreError> {
    let valid = admit(text)?;
    assert_eq!(valid.canonical_json().as_str(), text);
    Ok(match valid.definition() {
        SuccessorDefinition::Base(base) => CandidateDefinition::Base(base.clone()),
        SuccessorDefinition::Group(group) => CandidateDefinition::Group(group.clone()),
    })
}

fn set_text(state: &mut ExecutionSessionV3, item: &str, revision: Revision, value: &str) {
    let target = state.active_attempt().unwrap().target().clone();
    state
        .record_item(
            &target,
            &ItemId::new(item).unwrap(),
            revision,
            Some(RecordedItemValueV2::text(value).unwrap()),
            TIME,
        )
        .unwrap();
}

pub(super) struct Fixture {
    _root: TempDir,
    root: ValidatedWorkspaceRootV1,
    pub(super) file: PathBuf,
    pub(super) store: CandidateExecutionStore,
    pub(super) workspace: WorkspaceId,
    pub(super) session: SessionId,
}

impl Fixture {
    pub(super) fn new(base: &Value, plan_ready: bool) -> Self {
        let session = NativeDirectIdSourceV1.next_session_id();
        let admitted = canonical(base);
        let CandidateDefinition::Base(base) =
            definition(admitted.canonical_json().as_str()).unwrap()
        else {
            panic!("test document must be a base");
        };
        let mut state = ExecutionSessionV3::begin(
            session.clone(),
            base,
            admitted.digest().clone(),
            None,
            NativeDirectIdSourceV1.next_attempt_id(),
            TIME,
        )
        .unwrap();
        if plan_ready {
            set_text(&mut state, "plan", Revision::ZERO, "Prepare a report.");
        }
        let sources = CandidateSources::from([(
            admitted.digest().clone(),
            admitted.canonical_json().as_str().to_owned(),
        )]);
        Self::with_state(&state, &sources)
    }

    fn with_state(state: &ExecutionSessionV3, sources: &CandidateSources) -> Self {
        let owner = tempfile::Builder::new()
            .prefix("podway-group002-")
            .tempdir_in(std::fs::canonicalize("/tmp").unwrap())
            .unwrap();
        let root = ValidatedWorkspaceRootV1::from_path(owner.path()).unwrap();
        let file = owner.path().join("candidate.sqlite3");
        let workspace = NativeDirectIdSourceV1.next_workspace_id();
        let session = state.fence().session;
        let store = CandidateExecutionStore::create_fixture(
            &file,
            root.clone(),
            workspace.clone(),
            state,
            sources,
        )
        .unwrap();
        Self {
            _root: owner,
            root,
            file,
            store,
            workspace,
            session,
        }
    }

    pub(super) fn state(&mut self) -> (ExecutionSessionV3, CandidateSources) {
        self.store.snapshot(&self.session, definition).unwrap()
    }

    pub(super) fn edit_fixture(&self, state: &ExecutionSessionV3, sources: &CandidateSources) {
        let mut connection = Connection::open(&self.file).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON")
            .unwrap();
        let transaction = connection.transaction().unwrap();
        persist_candidate_execution(&transaction, state, sources).unwrap();
        transaction.commit().unwrap();
    }

    pub(super) fn ledger(&self) -> (u64, u64, u64) {
        let connection = Connection::open(&self.file).unwrap();
        connection
            .query_row(
                "SELECT next_workspace_sequence,
             (SELECT count(*) FROM mutation_outcomes),
             (SELECT count(*) FROM operational_journal) FROM workspace_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    pub(super) fn reopen(&mut self) {
        self.store = CandidateExecutionStore::reopen_fixture(
            &self.file,
            self.root.clone(),
            self.workspace.clone(),
        )
        .unwrap();
    }

    pub(super) fn compare_rejected(
        &mut self,
        before: &(ExecutionSessionV3, CandidateSources),
        key: &str,
        counts: (u64, u64, u64),
    ) {
        assert_eq!(&self.state(), before);
        assert!(
            self.store
                .outcome(&IdempotencyKeyV1::new(key).unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(self.ledger(), counts);
    }
}

pub(super) fn mutation(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
    key: &str,
) -> SuccessorMutationRequest {
    SuccessorMutationRequest {
        workspace: workspace.clone(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        request_id: RequestIdV1::new(NativeDirectIdSourceV1.next_session_id().into_inner())
            .unwrap(),
        cursor: state.fence(),
    }
}

pub(super) fn activation(
    workspace: &WorkspaceId,
    state: &ExecutionSessionV3,
    group: &ValidatedSuccessorDefinition,
    key: &str,
) -> GroupActivationRequest {
    GroupActivationRequest {
        mutation: mutation(workspace, state, key),
        document: group.canonical_json().as_str().to_owned(),
        format: ProcedureDocumentFormat::Json,
        expected_definition: group.digest().clone(),
        plan: state.plan_binding().unwrap(),
        predecessor: state
            .group_history()
            .last()
            .map(|entry| entry.identity().execution().clone()),
        working_copy_basis: CheckResultInputBasisV2::new(
            "Observed fixture basis",
            state.base_digest().clone(),
        )
        .unwrap(),
        prior_work: Vec::new(),
    }
}

fn advance_fixture_to_terminal(
    fixture: &mut Fixture,
    put_report: bool,
) -> (ExecutionSessionV3, CandidateSources) {
    let (mut state, sources) = fixture.state();
    while state.active_attempt().unwrap().target().node().as_str() != "finish" {
        state
            .complete(
                &state.fence(),
                Some(NativeDirectIdSourceV1.next_attempt_id()),
                TIME,
            )
            .unwrap();
    }
    if put_report {
        set_text(&mut state, "report", Revision::ZERO, "The group finished.");
    }
    fixture.edit_fixture(&state, &sources);
    (state, sources)
}

#[test]
fn activation_and_return_commit_exact_state_and_original_session_scoped_success() {
    let mut fixture = Fixture::new(&base_document(), true);
    let group = canonical(&group_document());
    let (design, initial_sources) = fixture.state();
    assert!(!initial_sources.contains_key(group.digest()));
    let activated = activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "activate"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let activation_key = IdempotencyKeyV1::new("activate").unwrap();
    assert_eq!(activated.outcome().sequence(), 1);
    assert_eq!(
        activated.outcome().scope(),
        &DirectOutcomeScopeV1::Session(fixture.session.clone())
    );
    assert_eq!(fixture.ledger(), (1, 1, 1));
    let (running, retained_sources) = fixture.state();
    assert_eq!(
        &retained_sources[group.digest()],
        group.canonical_json().as_str()
    );
    assert_eq!(
        running.group_history()[0].outcome(),
        GroupExecutionOutcomeV1::Running
    );
    assert_eq!(
        running.attempt_history()[0].lifecycle(),
        AttemptLifecycle::Completed
    );
    assert_eq!(
        running.active_attempt().unwrap().target().node().as_str(),
        "work"
    );

    let (terminal, _) = advance_fixture_to_terminal(&mut fixture, true);
    let returned = return_from_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &terminal, "return"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let return_key = IdempotencyKeyV1::new("return").unwrap();
    assert_eq!(
        returned.outcome().scope(),
        &DirectOutcomeScopeV1::Session(fixture.session.clone())
    );
    assert_eq!(returned.outcome().sequence(), 2);
    assert_eq!(fixture.ledger(), (2, 2, 2));
    let (base, _) = fixture.state();
    assert_eq!(base.lifecycle(), SessionLifecycle::Running);
    assert_eq!(
        base.active_attempt().unwrap().target().scope(),
        &ExecutionScopeV1::Base
    );
    assert_eq!(
        base.active_attempt().unwrap().target().node().as_str(),
        "review"
    );
    assert_eq!(
        base.group_history()[0].outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    let (source, report) = base
        .current_evidence(
            &EvidenceSourceV3::ExecutionResult,
            &ItemId::new("report").unwrap(),
        )
        .unwrap();
    assert_eq!(source, terminal.active_attempt().unwrap().target());
    assert_eq!(
        report.value().unwrap().as_text(),
        Some("The group finished.")
    );
    let envelope: Value = serde_json::from_str(returned.outcome().response().as_str()).unwrap();
    assert_eq!(envelope["session"]["id"], fixture.session.as_str());
    assert!(podway_protocol::OutputEnvelopeV4::from_value(envelope.clone()).is_err());

    assert_eq!(envelope["result"]["counters"].as_array().unwrap().len(), 5);
    assert_eq!(envelope["result"]["guidance"]["placement"]["id"], "review");

    fixture.reopen();
    let activation_replay = activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "activate"),
        || panic!("replay reads no clock"),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let return_replay = return_from_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &terminal, "return"),
        || panic!("replay reads no clock"),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(activation_replay.replayed() && return_replay.replayed());
    assert_eq!(activation_replay.outcome(), activated.outcome());
    assert_eq!(return_replay.outcome(), returned.outcome());
    assert_eq!(
        fixture.store.outcome(&activation_key).unwrap().as_ref(),
        Some(activated.outcome())
    );
    assert_eq!(
        fixture.store.outcome(&return_key).unwrap().as_ref(),
        Some(returned.outcome())
    );
    assert_eq!(fixture.state().0, base);
    assert_eq!(fixture.ledger(), (2, 2, 2));

    let mut changed = activation(&fixture.workspace, &design, &group, "activate");
    changed.working_copy_basis =
        CheckResultInputBasisV2::new("Different basis", design.base_digest().clone()).unwrap();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            changed,
            || panic!("key conflict reads no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Direct(
            DirectStoreErrorV1::IdempotencyKeyReused
        )))
    ));
    assert_eq!(fixture.ledger(), (2, 2, 2));
}

#[test]
fn insufficient_plan_return_output_and_stale_identity_do_not_publish_success() {
    let mut fixture = Fixture::new(&base_document(), false);
    let group = canonical(&group_document());
    let empty = fixture.state();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            activation(&fixture.workspace, &empty.0, &group, "empty"),
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::RequiredItemsMissing
        ))
    ));
    fixture.compare_rejected(&empty, "empty", (0, 0, 0));
    let mut prepared = empty.0.clone();
    set_text(&mut prepared, "plan", Revision::ZERO, "Ready");
    fixture.edit_fixture(&prepared, &empty.1);
    let ready = fixture.state();

    let mut wrong_plan = activation(&fixture.workspace, &ready.0, &group, "plan");
    wrong_plan.plan = empty.0.plan_binding().unwrap();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            wrong_plan,
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::InvalidState { .. }
        ))
    ));
    fixture.compare_rejected(&ready, "plan", (0, 0, 0));

    let mut wrong_digest = activation(&fixture.workspace, &ready.0, &group, "digest");
    wrong_digest.expected_definition = ready.0.base_digest().clone();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            wrong_digest,
            || panic!("invalid source reads no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Invalid(
            "prepared group definition digest differs"
        ))
    ));
    fixture.compare_rejected(&ready, "digest", (0, 0, 0));

    let mut invalid = group_document();
    invalid["plan_inputs"][0]["max_length"] = json!(100);
    let incompatible = canonical(&invalid);
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            activation(&fixture.workspace, &ready.0, &incompatible, "ports"),
            || panic!("incompatible ports read no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Config(_))
    ));
    fixture.compare_rejected(&ready, "ports", (0, 0, 0));

    let mut wrong_scope = activation(&fixture.workspace, &ready.0, &group, "workspace");
    wrong_scope.mutation.workspace = NativeDirectIdSourceV1.next_workspace_id();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            wrong_scope,
            || panic!("workspace refusal reads no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Store(
            CandidateStoreError::Corrupt("candidate workspace identity differs")
        ))
    ));
    fixture.compare_rejected(&ready, "workspace", (0, 0, 0));

    assert!(matches!(
        activate_group(
            &mut fixture.store,
            activation(&fixture.workspace, &empty.0, &group, "stale"),
            || panic!("stale command reads no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Invalid(
            "candidate cursor fence differs"
        ))
    ));
    fixture.compare_rejected(&ready, "stale", (0, 0, 0));
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &ready.0, &group, "new"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();

    let running = fixture.state();
    assert!(matches!(
        return_from_group(
            &mut fixture.store,
            mutation(&fixture.workspace, &running.0, "wrong-return"),
            || panic!("nonterminal refusal reads no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Invalid(
            "candidate return requires the group terminal placement"
        ))
    ));
    fixture.compare_rejected(&running, "wrong-return", (1, 1, 1));

    let terminal = advance_fixture_to_terminal(&mut fixture, false);
    assert!(matches!(
        return_from_group(
            &mut fixture.store,
            mutation(&fixture.workspace, &terminal.0, "missing-output"),
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::RequiredItemsMissing
        ))
    ));
    fixture.compare_rejected(&terminal, "missing-output", (1, 1, 1));
}

pub(super) fn inflate_current_guidance(document: &mut Value, definition: &str) {
    let prompt = format!("A{}", "\u{0001}".repeat(299));
    let help = format!("H{}", "\u{0001}".repeat(999));
    document["node_definitions"][definition]["items"] = Value::Array(
        (0..128)
            .map(|index| {
                json!({
                    "id":format!("budget-{index:03}"), "type":"text",
                    "prompt":prompt, "help":help, "required":false,
                })
            })
            .collect(),
    );
}

pub(super) fn add_counter_chain(document: &mut Value, from: &str, end: &str, length: usize) {
    document["node_definitions"]["bridge"] = action("Bridge");
    let nodes = document["graph"]["nodes"].as_array_mut().unwrap();
    nodes.iter_mut().find(|node| node["id"] == from).unwrap()["next"] = json!("bridge-000");
    for index in 0..length {
        let next = if index + 1 == length {
            end.to_owned()
        } else {
            format!("bridge-{:03}", index + 1)
        };
        nodes.push(json!({"id":format!("bridge-{index:03}"), "use":"bridge", "next":next}));
    }
}

#[test]
fn activation_rejects_oversized_combined_guidance_and_counters_before_persistence() {
    let mut base = base_document();
    add_counter_chain(&mut base, "review", "close", 900);
    let mut group = group_document();
    inflate_current_guidance(&mut group, "work");
    let group = canonical(&group);
    assert!(group.canonical_json().as_str().len() < podway_protocol::MAX_DIRECT_INPUT_BYTES_V1);
    let mut fixture = Fixture::new(&base, true);
    let prior = fixture.state();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            activation(&fixture.workspace, &prior.0, &group, "over-activate"),
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ResponseBudget)
    ));
    fixture.compare_rejected(&prior, "over-activate", (0, 0, 0));
}

#[test]
fn return_rejects_oversized_combined_guidance_and_counters_before_persistence() {
    let mut base = base_document();
    inflate_current_guidance(&mut base, "review");
    let mut group = group_document();
    add_counter_chain(&mut group, "work", "finish", 900);
    let group = canonical(&group);
    let mut fixture = Fixture::new(&base, true);
    let original = fixture.state();
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &original.0, &group, "activation"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let prior = advance_fixture_to_terminal(&mut fixture, true);
    assert!(matches!(
        return_from_group(
            &mut fixture.store,
            mutation(&fixture.workspace, &prior.0, "over-return"),
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ResponseBudget)
    ));
    fixture.compare_rejected(&prior, "over-return", (1, 1, 1));
}

#[test]
fn shared_success_validator_rolls_back_candidate_state_written_in_the_transaction() {
    let mut fixture = Fixture::new(&base_document(), false);
    let before = fixture.state();
    let input = mutation(&fixture.workspace, &before.0, "rejected-success");
    let request = DirectMutationRequestV1::new(
        input.key,
        before.0.base_digest().clone(),
        "group.activate",
        input.request_id.as_str(),
        DirectOutcomeScopeV1::Session(fixture.session.clone()),
    )
    .unwrap();
    let result =
        fixture
            .store
            .mutate::<CandidateStoreError>(&fixture.workspace, &request, |transaction| {
                let (mut state, sources) = transaction.load(definition)?;
                set_text(
                    &mut state,
                    "plan",
                    Revision::ZERO,
                    "This uncommitted state must be rolled back.",
                );
                transaction.persist(&state, &sources)?;
                assert_eq!(transaction.load(definition)?.0, state);
                Ok(CandidateSuccess {
                    response: CanonicalExecutionJsonV1::new("{}".to_owned()).unwrap(),
                    now: TIME,
                })
            });
    assert!(matches!(
        result,
        Err(CandidateStoreError::Direct(
            DirectStoreErrorV1::InvalidInput {
                reason: "incomplete or mismatched canonical success envelope",
            }
        ))
    ));
    fixture.compare_rejected(&before, "rejected-success", (0, 0, 0));
}

#[test]
fn activation_revalidates_exact_retained_work_and_predecessor() {
    let (mut state, sources) = super::int_group001_store::fixture();
    let base = state.base_digest().clone();
    let goal = state.current_goal().unwrap().clone();
    let anchor = state.base_definition().boundary().anchor().clone();
    state
        .revise_goal(
            &state.fence(),
            ExecutionGoalChangeV3 {
                goal,
                target: anchor,
                reactivate: false,
                feedback: GroupClosureFeedbackV1::new(
                    "Readmit retained work".to_owned(),
                    CheckResultInputBasisV2::new("Fixture basis", base.clone()).unwrap(),
                    "Verify binding before activating".to_owned(),
                )
                .unwrap(),
            },
            NativeDirectIdSourceV1.next_attempt_id(),
            TIME,
        )
        .unwrap();
    set_text(
        &mut state,
        "plan",
        Revision::ZERO,
        "Preserve the exact history.",
    );
    let prior_state = (state, sources);
    let mut fixture = Fixture::with_state(&prior_state.0, &prior_state.1);
    let past_group = &prior_state.0.group_history()[0];
    let group = admit(&prior_state.1[past_group.identity().definition_digest()]).unwrap();
    let prior_attempt = &prior_state.0.attempt_history()[1];
    let prior_slot = &prior_attempt.slots()[0];
    let exact_value = podway_core::recorded_value_digest_v2(prior_slot.value().unwrap()).unwrap();
    let work = |digest| {
        PriorWorkDispositionV1::new(
            past_group.identity().execution().clone(),
            Some(HistoricalReferenceV1::Item(HistoricalItemV1::new(
                prior_attempt.target().attempt().clone(),
                prior_slot.item_id().clone(),
                digest,
            ))),
            PriorWorkChoiceV1::Retain,
            "Retain this exact work.".to_owned(),
        )
        .unwrap()
    };

    let mut wrong_digest = activation(&fixture.workspace, &prior_state.0, &group, "prior-value");
    wrong_digest.prior_work = vec![work(base)];
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            wrong_digest,
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(DomainError::InvalidState {
            reason: "prior work value digest differs from retained history"
        }))
    ));
    fixture.compare_rejected(&prior_state, "prior-value", (0, 0, 0));

    let mut wrong_predecessor =
        activation(&fixture.workspace, &prior_state.0, &group, "predecessor");
    wrong_predecessor.predecessor = Some(past_group.identity().execution().clone());
    wrong_predecessor.prior_work = vec![work(exact_value.clone())];
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            wrong_predecessor,
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(DomainError::InvalidState {
            reason: "activation identity does not bind the exact current design"
        }))
    ));
    fixture.compare_rejected(&prior_state, "predecessor", (0, 0, 0));

    let mut valid = activation(&fixture.workspace, &prior_state.0, &group, "prior-exact");
    let prior_work = work(exact_value);
    valid.prior_work = vec![prior_work.clone()];
    let success =
        activate_group(&mut fixture.store, valid, || TIME, &NativeDirectIdSourceV1).unwrap();
    let (state, stored_sources) = fixture.state();
    assert_eq!(stored_sources, prior_state.1);
    assert_eq!(&state.group_history()[..2], prior_state.0.group_history());
    let selected = state.selected().unwrap().identity();
    assert_eq!(selected.prior_work(), &[prior_work]);
    assert_ne!(selected.execution(), past_group.identity().execution());
    assert_eq!(
        selected.definition_digest(),
        past_group.identity().definition_digest()
    );
    assert_eq!(success.outcome().sequence(), 1);
}

#[test]
fn activation_reserves_lookup_headroom_even_when_the_inner_response_fits() {
    let mut base = base_document();
    add_counter_chain(&mut base, "review", "close", 250);
    let mut group = group_document();
    inflate_current_guidance(&mut group, "work");
    let group = canonical(&group);
    let mut fixture = Fixture::new(&base, true);
    let prior = fixture.state();
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            activation(&fixture.workspace, &prior.0, &group, "lookup"),
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::OutcomeLookupBudget)
    ));
    fixture.compare_rejected(&prior, "lookup", (0, 0, 0));
}

#[test]
fn activation_rejects_excess_prior_work_before_preparing_request_bytes() {
    let mut fixture = Fixture::new(&base_document(), true);
    let group = canonical(&group_document());
    let prior = fixture.state();
    let mut request = activation(&fixture.workspace, &prior.0, &group, "excess-work");
    let work = PriorWorkDispositionV1::new(
        GroupExecutionIdV1::new(NativeDirectIdSourceV1.next_session_id().into_inner()).unwrap(),
        None,
        PriorWorkChoiceV1::Retain,
        "Never serialized in this rejected request.".into(),
    )
    .unwrap();
    request.prior_work = vec![work; MAX_PRIOR_WORK_DISPOSITIONS_V1 + 1];
    request.document.clear(); // The count check must precede definition preparation.
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            request,
            || panic!("count refusal reads no clock"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::BoundExceeded {
                field: "prior work dispositions",
                actual: 129,
                maximum: 128,
                unit: podway_core::BoundUnitV2::Items,
            }
        ))
    ));
    fixture.compare_rejected(&prior, "excess-work", (0, 0, 0));
}
