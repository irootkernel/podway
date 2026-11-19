//! Candidate Engine fences, current reads and success recovery on owned v16 fixtures.

use super::int_group001_store::{alter_record, fixture as retained_fixture};
use super::int_group002_transactions::{
    Fixture, TIME, action, activation, add_counter_chain, base_document, canonical, group_document,
    inflate_current_guidance, mutation,
};
use super::int_group003_replan::local_decision_group;
use podway_core::execution_groups::*;
use podway_core::{
    BlockerId, CheckResultInputBasisV2, CriterionId, DomainError, GoalCriterionV2,
    GoalDefinitionV2, GoalOutcome, GoalStatementV2, GraphMutationErrorV2, GraphNodeId, ItemId,
    OptionId, ReasonV2, RecordedItemValueV2, Revision, SessionLifecycle, Sha256Digest, UnixMillis,
    recorded_value_digest_v2,
};
use podway_engine::direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1};
use podway_engine::successor_execution::*;
use podway_protocol::RequestIdV1;
use podway_store::IdempotencyKeyV1;
use podway_store::direct_store::{
    DirectMutationResultV1, DirectOutcomeScopeV1, DirectStoreErrorV1,
};
use podway_store::successor_store::CandidateStoreError;
use rusqlite::Connection;
use serde_json::{Value, json};

fn observe(fixture: &mut Fixture) -> SuccessorObservation {
    observe_successor(&mut fixture.store, &fixture.workspace, &fixture.session).unwrap()
}

fn document(observation: &SuccessorObservation) -> Value {
    serde_json::from_str(observation.projection.as_str()).unwrap()
}

fn response(result: &DirectMutationResultV1) -> Value {
    serde_json::from_str::<Value>(result.outcome().response().as_str()).unwrap()["result"].clone()
}

fn key(label: &str) -> IdempotencyKeyV1 {
    IdempotencyKeyV1::new(label).unwrap()
}

fn request_id() -> RequestIdV1 {
    RequestIdV1::new(NativeDirectIdSourceV1.next_session_id().into_inner()).unwrap()
}

fn request(
    fixture: &Fixture,
    observation: &SuccessorObservation,
    name: &str,
) -> SuccessorMutationRequest {
    SuccessorMutationRequest {
        response: podway_engine::successor_execution::SuccessorMutationResponseV1::Fixture,
        workspace: fixture.workspace.clone(),
        key: key(name),
        request_id: request_id(),
        cursor: observation.cursor.clone(),
    }
}

fn item_request(
    fixture: &Fixture,
    observation: &SuccessorObservation,
    name: &str,
) -> SuccessorItemRequest {
    SuccessorItemRequest {
        response: podway_engine::successor_execution::SuccessorMutationResponseV1::Fixture,
        workspace: fixture.workspace.clone(),
        key: key(name),
        request_id: request_id(),
        target: observation.cursor.execution.attempt.clone().unwrap(),
        selected_group: observation.cursor.selected_group.clone(),
    }
}

fn write(item: &str, revision: u64, value: Option<RecordedItemValueV2>) -> GroupItemWriteV1 {
    GroupItemWriteV1 {
        item: ItemId::new(item).unwrap(),
        expected_revision: Revision::new(revision),
        value,
    }
}

fn text(item: &str, revision: u64, value: &str) -> GroupItemWriteV1 {
    write(
        item,
        revision,
        Some(RecordedItemValueV2::text(value).unwrap()),
    )
}

fn optional_text(item: &str) -> Value {
    json!({"id":item, "type":"text", "prompt":"Record a value.", "required":false})
}

fn at(
    fixture: &mut Fixture,
    name: &str,
    operation: SuccessorCursorMutation,
) -> DirectMutationResultV1 {
    let observation = observe(fixture);
    let request = request(fixture, &observation, name);
    issue_cursor_mutation(
        &mut fixture.store,
        request,
        operation,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap()
}

fn put(fixture: &mut Fixture, name: &str, value: GroupItemWriteV1) -> DirectMutationResultV1 {
    let observation = observe(fixture);
    let request = item_request(fixture, &observation, name);
    record_successor_item(&mut fixture.store, request, value, || TIME).unwrap()
}

fn activate(
    fixture: &mut Fixture,
    group: &Value,
    name: &str,
    work: Vec<PriorWorkDispositionV1>,
) -> DirectMutationResultV1 {
    let observed = observe(fixture);
    let state = fixture.state().0;
    let mut input = activation(&fixture.workspace, &state, &canonical(group), name);
    input.mutation.cursor = observed.cursor;
    input.plan = current_plan_binding(
        &mut fixture.store,
        &fixture.workspace,
        &input.mutation.cursor,
    )
    .unwrap();
    input.prior_work = work;
    activate_group(&mut fixture.store, input, || TIME, &NativeDirectIdSourceV1).unwrap()
}

fn checked_reason(error: SuccessorExecutionError, expected: &str) {
    let reason = match &error {
        SuccessorExecutionError::Invalid(reason)
        | SuccessorExecutionError::Domain(DomainError::InvalidState { reason })
        | SuccessorExecutionError::Store(CandidateStoreError::Domain(
            DomainError::InvalidState { reason },
        ))
        | SuccessorExecutionError::Store(CandidateStoreError::Corrupt(reason)) => *reason,
        _ => panic!("Expected {expected}, got {error:?}"),
    };
    assert_eq!(reason, expected);
}

fn reason(value: &str) -> ReasonV2 {
    ReasonV2::new(value).unwrap()
}

fn attempt_conflict(
    error: SuccessorExecutionError,
    stale: &SuccessorObservation,
    live: &SuccessorObservation,
) {
    assert!(
        matches!(error, SuccessorExecutionError::Domain(DomainError::AttemptNotCurrent { expected, actual })
        if &expected == stale.cursor.execution.attempt.as_ref().unwrap().attempt()
            && actual.as_ref() == live.cursor.execution.attempt.as_ref().map(ScopedAttemptV1::attempt))
    );
}

fn selection_conflict(
    error: SuccessorExecutionError,
    stale: &SuccessorObservation,
    live: &SuccessorObservation,
) {
    assert!(
        matches!(error, SuccessorExecutionError::SelectedActivationMismatch { expected, actual }
        if expected == stale.cursor.selected_group && actual == live.cursor.selected_group)
    );
}

fn retained_rows(fixture: &Fixture) -> Vec<(String, String)> {
    let connection = Connection::open(&fixture.file).unwrap();
    [
        "SELECT record_json, record_digest FROM successor_sessions ORDER BY session_id",
        "SELECT record_json, record_digest FROM successor_activations ORDER BY session_id, ordinal",
        "SELECT record_json, record_digest FROM successor_attempts ORDER BY session_id, ordinal",
    ]
    .into_iter()
    .flat_map(|query| {
        let mut statement = connection.prepare(query).unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    })
    .collect()
}

#[test]
fn semantic_item_noops_keep_retained_acceptance_and_original_success() {
    let mut group = group_document();
    group["graph"]["nodes"][0]["delegation"] = delegation_declaration();
    group["node_definitions"]["work"]["items"] =
        json!([optional_text("alpha"), optional_text("beta")]);
    let mut fixture = Fixture::new(&base_document(), true);
    activate(&mut fixture, &group, "launch", vec![]);
    put(&mut fixture, "value", text("alpha", 0, "original"));
    let observed = observe(&mut fixture);
    let package =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &observed.cursor)
            .unwrap();
    let accept = request(&fixture, &observed, "accept");
    accept_successor_delegation(&mut fixture.store, accept, package, || TIME).unwrap();
    let current = observe(&mut fixture);
    let before = fixture.state();
    let rows = retained_rows(&fixture);
    let edited_at = UnixMillis::new(TIME.get() + 1);
    let identical = item_request(&fixture, &current, "identical");
    let written = record_successor_item(
        &mut fixture.store,
        identical,
        text("alpha", 1, "original"),
        || edited_at,
    )
    .unwrap();
    assert!(
        fixture.state() == before,
        "identical set changed retained state or receipt"
    );
    assert_eq!(response(&written)["changed"], false);
    assert_eq!(written.outcome().recorded_at(), edited_at);
    assert_eq!(retained_rows(&fixture), rows);

    let cleared = item_request(&fixture, &current, "empty-clear");
    let cleared =
        record_successor_item(&mut fixture.store, cleared, write("beta", 0, None), || {
            edited_at
        })
        .unwrap();
    assert_eq!(response(&cleared)["changed"], false);
    let same = request(&fixture, &current, "unchanged-pair");
    let unchanged = record_successor_many(
        &mut fixture.store,
        same,
        vec![write("beta", 0, None), text("alpha", 1, "original")],
        || edited_at,
    )
    .unwrap();
    assert_eq!(response(&unchanged)["changed"], false);
    assert_eq!(
        response(&unchanged)["delegation"],
        document(&current)["delegation"]
    );
    assert_eq!(fixture.ledger(), (6, 6, 6));
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(retained_rows(&fixture), rows);

    let stale_slot = item_request(&fixture, &current, "stale-unchanged");
    assert!(
        matches!(record_successor_item(&mut fixture.store, stale_slot, text("alpha", 0, "original"), || edited_at),
        Err(SuccessorExecutionError::ItemMutation(GraphMutationErrorV2::ItemRevisionConflict { expected, actual }))
            if expected == Revision::ZERO && actual == Revision::new(1))
    );
    fixture.compare_rejected(&before, "stale-unchanged", (6, 6, 6));

    let mix = request(&fixture, &current, "mixed");
    let mixed = record_successor_many(
        &mut fixture.store,
        mix,
        vec![text("alpha", 1, "original"), text("beta", 0, "changed")],
        || edited_at,
    )
    .unwrap();
    assert_eq!(response(&mixed)["changed"], true);
    let after = fixture.state();
    assert_eq!(
        after.0.fence().revision,
        before.0.fence().revision.checked_next().unwrap()
    );
    let prior = before.0.active_attempt().unwrap();
    let active = after.0.active_attempt().unwrap();
    assert_eq!(active.slots()[0], prior.slots()[0]);
    assert_eq!(active.slots()[1].revision(), Revision::new(1));
    assert_eq!(active.slots()[1].updated_at(), edited_at);
    assert!(active.delegation().is_none());
    let stale_batch = request(&fixture, &current, "stale-noop-batch");
    assert!(
        matches!(record_successor_many(&mut fixture.store, stale_batch, vec![text("alpha", 1, "original"), text("beta", 1, "changed")],
        || panic!("stale revision cannot be excused by equal values")),
        Err(SuccessorExecutionError::Domain(DomainError::PreconditionFailed { expected, actual }))
            if expected == before.0.fence().revision && actual == after.0.fence().revision)
    );
    fixture.compare_rejected(&after, "stale-noop-batch", (7, 7, 7));

    fixture.reopen();
    let old = request(&fixture, &current, "unchanged-pair");
    let replay = record_successor_many(
        &mut fixture.store,
        old,
        vec![text("alpha", 1, "original"), write("beta", 0, None)],
        || panic!("an old success replays ahead of live fences"),
    )
    .unwrap();
    let original =
        lookup_successor_outcome(&fixture.store, &fixture.workspace, &key("unchanged-pair"))
            .unwrap()
            .unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.outcome(), unchanged.outcome());
    assert_eq!(
        original.response().as_str(),
        unchanged.outcome().response().as_str()
    );
    assert_eq!(fixture.state(), after);
    assert_eq!(fixture.ledger(), (7, 7, 7));
}

#[test]
fn atomic_record_many_rejects_invalid_inputs_and_the_direct_byte_bound() {
    use podway_core::{MAX_ACTIVE_ITEM_MUTATIONS_V2, MAX_TEXT_SCALARS_V2};
    let mut group = group_document();
    group["node_definitions"]["work"]["items"] = json!(["alpha", "beta", "gamma"].map(|id| {
        let mut item = optional_text(id);
        item["max_length"] = json!(MAX_TEXT_SCALARS_V2);
        item
    }));
    let mut fixture = Fixture::new(&base_document(), true);
    activate(&mut fixture, &group, "activate", vec![]);
    let observed = observe(&mut fixture);
    let before = fixture.state();
    let counts = fixture.ledger();
    let empty = request(&fixture, &observed, "empty");
    assert!(matches!(
        record_successor_many(&mut fixture.store, empty, vec![], || TIME),
        Err(SuccessorExecutionError::ItemMutation(
            GraphMutationErrorV2::ItemMutationBatchEmpty
        ))
    ));
    fixture.compare_rejected(&before, "empty", counts);

    let too_many = request(&fixture, &observed, "too-many");
    let writes = (0..=MAX_ACTIVE_ITEM_MUTATIONS_V2)
        .map(|index| text(&format!("slot-{index}"), 0, "request size control"))
        .collect();
    assert!(
        matches!(record_successor_many(&mut fixture.store, too_many, writes,
        || panic!("operation count fails before the writer")),
        Err(SuccessorExecutionError::ItemMutation(GraphMutationErrorV2::TooManyItemMutations { maximum }))
            if maximum as usize == MAX_ACTIVE_ITEM_MUTATIONS_V2)
    );
    fixture.compare_rejected(&before, "too-many", counts);

    let unknown = request(&fixture, &observed, "unknown");
    assert!(matches!(record_successor_many(&mut fixture.store, unknown,
        vec![text("alpha", 0, "valid first slot"), text("unknown", 0, "undeclared")], || TIME),
        Err(SuccessorExecutionError::Domain(DomainError::ItemNotFound { item_id })) if item_id.as_str() == "unknown"));
    fixture.compare_rejected(&before, "unknown", counts);

    let wrong_type = request(&fixture, &observed, "wrong-type");
    checked_reason(
        record_successor_many(
            &mut fixture.store,
            wrong_type,
            vec![
                text("alpha", 0, "valid first slot"),
                write("beta", 0, Some(RecordedItemValueV2::integer(1))),
            ],
            || TIME,
        )
        .unwrap_err(),
        "recorded value does not match its item declaration",
    );
    fixture.compare_rejected(&before, "wrong-type", counts);

    let oversized = format!("X{}", "\u{0001}".repeat(MAX_TEXT_SCALARS_V2 as usize - 1));
    let values = || ["alpha", "beta", "gamma"].map(|item| text(item, 0, &oversized));
    let mut model = before.0.clone();
    assert!(
        model
            .record_many(&before.0.fence(), values().into(), TIME)
            .unwrap()
    );
    let bytes = request(&fixture, &observed, "direct-bytes");
    checked_reason(
        record_successor_many(&mut fixture.store, bytes, values().into(), || {
            panic!("oversized canonical input cannot reach the writer")
        })
        .unwrap_err(),
        "candidate request exceeds the direct input bound",
    );
    fixture.compare_rejected(&before, "direct-bytes", counts);
}

#[test]
fn atomic_record_many_core_aggregate_admits_exact_limit_and_rejects_one_extra_scalar() {
    use podway_core::{
        MAX_ATTEMPT_CONTENT_SCALARS_V2, MAX_LIST_ENTRIES_V2, MAX_LIST_ENTRY_SCALARS_V2,
        MAX_LIST_TOTAL_SCALARS_V2,
    };
    let total = MAX_ATTEMPT_CONTENT_SCALARS_V2 as usize;
    let per_item = MAX_LIST_TOTAL_SCALARS_V2 as usize;
    let slots = total.div_ceil(per_item);
    let item_id = |index| format!("list-{index:02}");
    let mut group = group_document();
    group["node_definitions"]["work"]["items"] = json!((0..slots).map(|index| json!({
        "id": item_id(index), "type": "list", "prompt": "Record bounded work.", "required": false,
        "max_items": MAX_LIST_ENTRIES_V2, "max_item_length": MAX_LIST_ENTRY_SCALARS_V2,
        "max_total_length": MAX_LIST_TOTAL_SCALARS_V2, "unique": false,
    })).collect::<Vec<_>>());
    let mut fixture = Fixture::new(&base_document(), true);
    activate(&mut fixture, &group, "activate", vec![]);
    let persisted = fixture.state();
    let mut core = persisted.0.clone();
    let list = |length: usize| {
        let entry_cap = MAX_LIST_ENTRY_SCALARS_V2 as usize;
        let entries = (0..length.div_ceil(entry_cap))
            .map(|index| "x".repeat((length - index * entry_cap).min(entry_cap)))
            .collect();
        RecordedItemValueV2::list(entries).unwrap()
    };
    let writes = (0..slots)
        .map(|index| {
            write(
                &item_id(index),
                0,
                Some(list((total - index * per_item).min(per_item))),
            )
        })
        .collect();
    assert!(core.record_many(&core.fence(), writes, TIME).unwrap());
    let exact = core.clone();
    let last = slots - 1;
    let oversize = write(&item_id(last), 1, Some(list(total - last * per_item + 1)));
    assert_eq!(
        core.record_many(&core.fence(), vec![oversize], TIME)
            .unwrap_err(),
        GraphMutationErrorV2::Domain(podway_core::attempt_content_bound_error_v2(
            MAX_ATTEMPT_CONTENT_SCALARS_V2 + 1
        ))
    );
    assert!(
        core == exact,
        "Core aggregate rejection changed a retained slot or revision"
    );
    fixture.compare_rejected(&persisted, "core-aggregate", (1, 1, 1));
}

#[test]
fn current_binding_refusals_distinguish_persisted_faults_and_leave_outcome_recovery_available() {
    let mut fixture = Fixture::new(&base_document(), true);
    let design = fixture.state();
    let group = canonical(&group_document());
    let issued = activation(&fixture.workspace, &design.0, &group, "launch");
    let original =
        activate_group(&mut fixture.store, issued, || TIME, &NativeDirectIdSourceV1).unwrap();
    let connection = Connection::open(&fixture.file).unwrap();
    let foreign = NativeDirectIdSourceV1.next_workspace_id();
    let change_workspace = |uuid| {
        connection
            .execute(
                "UPDATE workspace_state SET workspace_uuid=?1 WHERE singleton=1",
                [uuid],
            )
            .unwrap()
    };
    assert_eq!(change_workspace(foreign.as_str()), 1);
    checked_reason(
        observe_successor(&mut fixture.store, &fixture.workspace, &fixture.session).unwrap_err(),
        "candidate persisted workspace binding differs",
    );
    assert_eq!(change_workspace(fixture.workspace.as_str()), 1);

    // Leave the requested session header intact and remove only its referenced group definition.
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    assert_eq!(
        connection
            .execute("DELETE FROM successor_definitions WHERE kind='group'", [])
            .unwrap(),
        1
    );
    assert!(matches!(
        observe_successor(&mut fixture.store, &fixture.workspace, &fixture.session),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Sql(
            rusqlite::Error::QueryReturnedNoRows
        )))
    ));
    assert_eq!(
        lookup_successor_outcome(&fixture.store, &fixture.workspace, &key("launch"))
            .unwrap()
            .as_ref(),
        Some(original.outcome())
    );
    let exact_retry = activation(&fixture.workspace, &design.0, &group, "launch");
    let replay = activate_group(
        &mut fixture.store,
        exact_retry,
        || panic!("replay precedes live state loading"),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert_eq!(replay.outcome(), original.outcome());
    assert!(replay.replayed());
    assert_eq!(fixture.ledger(), (1, 1, 1));
}

#[test]
fn private_issuer_skips_a_declared_action_and_decides_within_the_selected_group() {
    let mut group = local_decision_group(true);
    group["graph"]["entry"] = json!("prepare");
    group["node_definitions"]["prepare"] = action("Prepare");
    group["graph"]["nodes"].as_array_mut().unwrap().insert(
        0,
        json!({
            "id":"prepare", "use":"prepare", "next":"work",
            "skip":{"allowed":true,"reason_required":true},
        }),
    );
    let mut fixture = Fixture::new(&base_document(), true);
    activate(&mut fixture, &group, "launch", vec![]);
    let selection = observe(&mut fixture).cursor.selected_group;
    at(
        &mut fixture,
        "skip",
        SuccessorCursorMutation::Skip(Some(reason("Inputs are already prepared."))),
    );
    let work = observe(&mut fixture);
    assert_eq!(
        work.cursor
            .execution
            .attempt
            .as_ref()
            .unwrap()
            .node()
            .as_str(),
        "work"
    );
    assert_eq!(
        fixture.state().0.attempt_history()[1].lifecycle(),
        podway_core::AttemptLifecycle::Skipped
    );
    let input = request(&fixture, &work, "assess");
    record_successor_many(
        &mut fixture.store,
        input,
        vec![
            write(
                "verdict",
                0,
                Some(RecordedItemValueV2::choice("success").unwrap()),
            ),
            write(
                "stage",
                0,
                Some(RecordedItemValueV2::choice("not-scored").unwrap()),
            ),
            write("score", 0, Some(RecordedItemValueV2::integer(0))),
        ],
        || TIME,
    )
    .unwrap();
    at(
        &mut fixture,
        "complete-work",
        SuccessorCursorMutation::Complete,
    );
    at(
        &mut fixture,
        "choose",
        SuccessorCursorMutation::Decide {
            option: OptionId::new("finish").unwrap(),
            reason: reason("Take the declared forward route."),
            goal_outcome: None,
        },
    );
    fixture.reopen();
    let finish = observe(&mut fixture);
    assert_eq!(finish.cursor.selected_group, selection);
    let finish = finish.cursor.execution.attempt.unwrap();
    assert_eq!(finish.node().as_str(), "finish");
    let state = fixture.state().0;
    let closed = &state.attempt_history()[3];
    assert_eq!(closed.target().scope(), finish.scope());
    assert_eq!(closed.decision().unwrap().option.as_str(), "finish");
}

#[test]
fn current_projection_and_mutation_share_the_response_boundary() {
    let mut base = base_document();
    let plan = base["node_definitions"]["design"]["items"][0].clone();
    inflate_current_guidance(&mut base, "design");
    base["node_definitions"]["design"]["items"][0] = plan;
    add_counter_chain(&mut base, "review", "close", 600);
    let mut fixture = Fixture::new(&base, false);
    let before = fixture.state();
    assert!(matches!(
        observe_successor(&mut fixture.store, &fixture.workspace, &fixture.session),
        Err(SuccessorExecutionError::ResponseBudget)
    ));
    let input = mutation(&fixture.workspace, &before.0, "response-budget");
    assert!(matches!(
        record_successor_many(
            &mut fixture.store,
            input,
            vec![text("plan", 0, "Prepare work.")],
            || TIME
        ),
        Err(SuccessorExecutionError::ResponseBudget)
    ));
    fixture.compare_rejected(&before, "response-budget", (0, 0, 0));
}

#[test]
fn independent_item_writes_and_atomic_batch_preserve_exact_old_success_on_reconnect() {
    let mut group = group_document();
    group["node_definitions"]["work"]["items"] =
        json!([optional_text("alpha"), optional_text("beta")]);
    let mut fixture = Fixture::new(&base_document(), true);
    activate(&mut fixture, &group, "launch", vec![]);
    let entry = observe(&mut fixture);
    let alpha = item_request(&fixture, &entry, "alpha");
    let beta = item_request(&fixture, &entry, "beta");
    let plan = read_current_evidence(
        &mut fixture.store,
        &fixture.workspace,
        &entry.cursor,
        &EvidenceSourceV3::Plan,
        &ItemId::new("plan").unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.source.node().as_str(), "design");
    assert_eq!(plan.value.as_text(), Some("Prepare a report."));
    assert_eq!(
        plan.item.value_digest(),
        &recorded_value_digest_v2(&plan.value).unwrap()
    );
    let first =
        record_successor_item(&mut fixture.store, alpha, text("alpha", 0, "a"), || TIME).unwrap();
    fixture.reopen();
    let second =
        record_successor_item(&mut fixture.store, beta, text("beta", 0, "b"), || TIME).unwrap();
    assert_eq!(first.outcome().sequence(), 2);
    assert_eq!(second.outcome().sequence(), 3);
    assert_eq!(response(&first)["items"][1]["revision"], 0);
    assert_eq!(response(&second)["items"][1]["revision"], 1);

    let before = fixture.state();
    let same = item_request(&fixture, &entry, "same-item");
    assert!(matches!(
        record_successor_item(&mut fixture.store, same, text("alpha", 0, "stale"), || TIME),
        Err(SuccessorExecutionError::ItemMutation(GraphMutationErrorV2::ItemRevisionConflict { expected, actual }))
            if expected == Revision::ZERO && actual == Revision::new(1)
    ));
    fixture.compare_rejected(&before, "same-item", (3, 3, 3));
    let stale = request(&fixture, &entry, "stale-batch");
    assert!(matches!(
        record_successor_many(
            &mut fixture.store,
            stale,
            vec![text("alpha", 1, "ready")],
            || panic!("cursor must fence before clock"),
        )
        , Err(SuccessorExecutionError::Domain(DomainError::PreconditionFailed { expected, actual }))
            if expected == entry.cursor.execution.revision && actual == before.0.fence().revision));
    fixture.compare_rejected(&before, "stale-batch", (3, 3, 3));

    let current = observe(&mut fixture);
    let mut omitted = request(&fixture, &current, "omitted-attempt");
    omitted.cursor.execution.attempt = None;
    assert!(
        matches!(record_successor_many(&mut fixture.store, omitted, vec![text("alpha", 1, "ready")],
        || panic!("missing attempt fence denies before clock")),
        Err(SuccessorExecutionError::MissingAttempt { actual }) if &actual == current.cursor.execution.attempt.as_ref().unwrap().attempt())
    );
    fixture.compare_rejected(&before, "omitted-attempt", (3, 3, 3));
    let bad = request(&fixture, &current, "bad-batch");
    assert!(matches!(
        record_successor_many(
            &mut fixture.store,
            bad,
            vec![text("alpha", 1, "ready"), text("beta", 0, "stale")],
            || TIME
        ),
        Err(SuccessorExecutionError::ItemMutation(
            GraphMutationErrorV2::ItemRevisionConflict { expected, actual }
        )) if expected == Revision::ZERO && actual == Revision::new(1)
    ));
    fixture.compare_rejected(&before, "bad-batch", (3, 3, 3));
    let duplicate = request(&fixture, &current, "duplicate");
    assert!(matches!(
        record_successor_many(
            &mut fixture.store,
            duplicate,
            vec![text("alpha", 1, "a"), text("alpha", 0, "stale duplicate")],
            || TIME,
        )
        , Err(SuccessorExecutionError::ItemMutation(GraphMutationErrorV2::DuplicateItemMutation { item_id }))
            if item_id.as_str() == "alpha"));
    fixture.compare_rejected(&before, "duplicate", (3, 3, 3));
    let batch = request(&fixture, &current, "batch");
    let batch_result = record_successor_many(
        &mut fixture.store,
        batch,
        vec![write("beta", 1, None), text("alpha", 1, "both")],
        || TIME,
    )
    .unwrap();
    let recorded = observe(&mut fixture);
    assert_eq!(
        recorded.cursor.execution.revision,
        current.cursor.execution.revision.checked_next().unwrap()
    );
    let mut expected = document(&recorded);
    expected["changed"] = json!(true);
    assert_eq!(expected, response(&batch_result));
    assert_eq!(
        document(&recorded)["items"],
        json!([
            {"item":"alpha", "revision":2,"recorded":true}, {"item":"beta", "revision":2,"recorded":false},
        ])
    );
    at(
        &mut fixture,
        "finish-work",
        SuccessorCursorMutation::Complete,
    );
    fixture.reopen();
    let latest = observe(&mut fixture);
    let latest_state = fixture.state();
    assert_eq!(latest.workspace_sequence, 5);
    assert_ne!(
        latest.cursor.execution.attempt,
        current.cursor.execution.attempt
    );
    let original = lookup_successor_outcome(&fixture.store, &fixture.workspace, &key("alpha"))
        .unwrap()
        .unwrap();
    assert_eq!(&original, first.outcome());
    assert_eq!(
        original.scope(),
        &DirectOutcomeScopeV1::Session(fixture.session.clone())
    );
    assert!(
        lookup_successor_outcome(&fixture.store, &fixture.workspace, &key("unknown"))
            .unwrap()
            .is_none()
    );
    let foreign = NativeDirectIdSourceV1.next_workspace_id();
    assert!(
        matches!(lookup_successor_outcome(&fixture.store, &foreign, &key("alpha")),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Direct(DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch { expected, actual })))
            if expected == foreign && actual == fixture.workspace)
    );
    assert!(
        matches!(observe_successor(&mut fixture.store, &foreign, &fixture.session),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Direct(DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch { expected, actual })))
            if expected == foreign && actual == fixture.workspace)
    );
    let replay = item_request(&fixture, &entry, "alpha");
    let replayed = record_successor_item(&mut fixture.store, replay, text("alpha", 0, "a"), || {
        panic!("replay must not use the live clock")
    })
    .unwrap();
    assert!(replayed.replayed());
    assert_eq!(replayed.outcome(), &original);
    let replay = request(&fixture, &current, "batch");
    let replayed = record_successor_many(
        &mut fixture.store,
        replay,
        vec![text("alpha", 1, "both"), write("beta", 1, None)],
        || panic!("exact reordered batch must replay"),
    )
    .unwrap();
    assert_eq!(replayed.outcome(), batch_result.outcome());
    assert!(replayed.replayed());

    let late = item_request(&fixture, &entry, "late-item");
    attempt_conflict(
        record_successor_item(&mut fixture.store, late, text("alpha", 2, "late"), || {
            panic!("scope is stale")
        })
        .unwrap_err(),
        &entry,
        &latest,
    );
    fixture.compare_rejected(&latest_state, "late-item", (5, 5, 5));
    let old_evidence = read_current_evidence(
        &mut fixture.store,
        &fixture.workspace,
        &entry.cursor,
        &EvidenceSourceV3::Plan,
        &ItemId::new("plan").unwrap(),
    );
    attempt_conflict(old_evidence.unwrap_err(), &entry, &latest);
    assert!(
        read_current_evidence(
            &mut fixture.store,
            &fixture.workspace,
            &latest.cursor,
            &EvidenceSourceV3::Plan,
            &ItemId::new("plan").unwrap()
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(document(&observe(&mut fixture)), document(&latest));

    let mut wrong_session = request(&fixture, &latest, "foreign-session");
    let unexpected_session = NativeDirectIdSourceV1.next_session_id();
    wrong_session.cursor.execution.session = unexpected_session.clone();
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            wrong_session,
            SuccessorCursorMutation::Complete,
            || panic!("foreign session must be rejected before transition"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(DomainError::SessionIdentityMismatch { expected, actual: None }))
            if expected == unexpected_session
    ));
    fixture.compare_rejected(&latest_state, "foreign-session", (5, 5, 5));

    // Activation and finish-work contribute two revisions. The plan slot requires one
    // revision and the two work slots require at least two revisions between them.
    // A rehashed revision of four is therefore below the minimum of five.
    let connection = Connection::open(&fixture.file).unwrap();
    alter_record(
        &connection,
        "SELECT record_json FROM successor_sessions",
        "UPDATE successor_sessions SET record_json = ?1, record_digest = ?2",
        "/revision",
        json!(4),
    );
    drop(connection);
    fixture.reopen();
    checked_reason(
        observe_successor(&mut fixture.store, &fixture.workspace, &fixture.session).unwrap_err(),
        "retained trace differs from replayed execution",
    );
}

#[test]
fn retry_counter_replay_and_same_digest_replacement_keep_current_and_retained_evidence_distinct() {
    let mut group = local_decision_group(true);
    group["graph"]["nodes"][0]["delegation"] = delegation_declaration();
    group["node_definitions"]["choose"]["items"] = json!([optional_text("memo")]);
    let definition = canonical(&group);
    let mut fixture = Fixture::new(&base_document(), true);
    let anchor = observe(&mut fixture);
    let original_plan =
        current_plan_binding(&mut fixture.store, &fixture.workspace, &anchor.cursor).unwrap();
    activate(&mut fixture, &group, "launch", vec![]);
    let entry = observe(&mut fixture);
    assert_eq!(document(&entry)["next_decision_entry"], 1);
    assert_eq!(
        document(&entry)["remaining_activations"],
        MAX_GROUP_EXECUTIONS_V1 - 1
    );
    assert_eq!(
        document(&entry)["remaining_attempts"],
        MAX_SESSION_ATTEMPTS_V1 - 2
    );
    let empty_package =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &entry.cursor).unwrap();
    let batch = request(&fixture, &entry, "assess");
    record_successor_many(
        &mut fixture.store,
        batch,
        vec![
            write(
                "verdict",
                0,
                Some(RecordedItemValueV2::choice("failure").unwrap()),
            ),
            write(
                "stage",
                0,
                Some(RecordedItemValueV2::choice("scored").unwrap()),
            ),
            write("score", 0, Some(RecordedItemValueV2::integer(80))),
        ],
        || TIME,
    )
    .unwrap();
    let assessed = observe(&mut fixture);
    let before_accept = fixture.state();
    let counts = fixture.ledger();
    let missing_acceptance = request(&fixture, &assessed, "complete-without-acceptance");
    checked_reason(
        issue_cursor_mutation(
            &mut fixture.store,
            missing_acceptance,
            SuccessorCursorMutation::Complete,
            || TIME,
            &NativeDirectIdSourceV1,
        )
        .unwrap_err(),
        "current delegated action acceptance is required",
    );
    fixture.compare_rejected(&before_accept, "complete-without-acceptance", counts);
    let stale_accept = request(&fixture, &assessed, "stale-package");
    checked_reason(
        accept_successor_delegation(
            &mut fixture.store,
            stale_accept,
            empty_package.clone(),
            || TIME,
        )
        .unwrap_err(),
        "delegation acceptance does not apply to the current action/package",
    );
    fixture.compare_rejected(&before_accept, "stale-package", counts);
    let accepted_package =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &assessed.cursor)
            .unwrap();
    assert_eq!(empty_package.target(), accepted_package.target());
    assert_ne!(
        empty_package.package_digest(),
        accepted_package.package_digest()
    );
    let accept = request(&fixture, &assessed, "accept");
    let accepted =
        accept_successor_delegation(&mut fixture.store, accept, accepted_package.clone(), || {
            TIME
        })
        .unwrap();
    assert_eq!(response(&accepted)["delegation"], json!(&accepted_package));
    let live = observe(&mut fixture);
    let duplicate = request(&fixture, &live, "duplicate-acceptance");
    let state = fixture.state();
    let counts = fixture.ledger();
    checked_reason(
        accept_successor_delegation(
            &mut fixture.store,
            duplicate,
            accepted_package.clone(),
            || TIME,
        )
        .unwrap_err(),
        "candidate action already has an acceptance",
    );
    fixture.compare_rejected(&state, "duplicate-acceptance", counts);
    at(
        &mut fixture,
        "assessment-complete",
        SuccessorCursorMutation::Complete,
    );
    let first = observe(&mut fixture);
    assert_eq!(document(&first)["decision_entry"], 1);
    let retry =
        || SuccessorCursorMutation::Retry(reason("Consider the same current evidence again."));
    let retry_request = request(&fixture, &first, "retry-first");
    let old = issue_cursor_mutation(
        &mut fixture.store,
        retry_request,
        retry(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert_eq!(response(&old)["decision_entry"], 2);
    at(&mut fixture, "retry-second", retry());
    fixture.reopen();
    let third = observe(&mut fixture);
    assert_eq!(document(&third)["decision_entry"], 3);
    let before = fixture.state();
    let counts = fixture.ledger();
    let stale_retry = item_request(&fixture, &first, "write-before-retry");
    attempt_conflict(
        record_successor_item(
            &mut fixture.store,
            stale_retry,
            text("memo", 0, "late"),
            || panic!("retry replaced this attempt"),
        )
        .unwrap_err(),
        &first,
        &third,
    );
    fixture.compare_rejected(&before, "write-before-retry", counts);
    let score = read_current_evidence(
        &mut fixture.store,
        &fixture.workspace,
        &third.cursor,
        &EvidenceSourceV3::Local(GraphNodeId::new("work").unwrap()),
        &ItemId::new("score").unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        score.source,
        *entry.cursor.execution.attempt.as_ref().unwrap()
    );
    assert_eq!(score.value.as_integer(), Some(80));
    let replay_request = request(&fixture, &first, "retry-first");
    let replay = issue_cursor_mutation(
        &mut fixture.store,
        replay_request,
        retry(),
        || panic!("a replay cannot open an attempt"),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert_eq!(replay.outcome(), old.outcome());

    let replan_request = request(&fixture, &third, "redesign");
    let base = score.source.base_digest().clone();
    let feedback = ReplanFeedbackV1::new(
        third.cursor.execution.attempt.clone().unwrap(),
        OptionId::new("redesign").unwrap(),
        "Reevaluate and retain valid work".into(),
        CheckResultInputBasisV2::new("Observed source", base.clone()).unwrap(),
        "Plan only the remaining work".into(),
    )
    .unwrap();
    replan_group(
        &mut fixture.store,
        replan_request,
        feedback,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let anchor = observe(&mut fixture);
    assert!(anchor.cursor.selected_group.is_none());
    let before = fixture.state();
    let counts = fixture.ledger();
    let stale_replan = request(&fixture, &third, "batch-before-replan");
    attempt_conflict(
        record_successor_many(
            &mut fixture.store,
            stale_replan,
            vec![text("memo", 0, "late")],
            || panic!("replan closed this attempt"),
        )
        .unwrap_err(),
        &third,
        &anchor,
    );
    fixture.compare_rejected(&before, "batch-before-replan", counts);
    assert!(
        document(&anchor)["counters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["scope"] == "base")
    );
    assert!(
        read_current_evidence(
            &mut fixture.store,
            &fixture.workspace,
            &anchor.cursor,
            &EvidenceSourceV3::Local(GraphNodeId::new("work").unwrap()),
            &ItemId::new("score").unwrap(),
        )
        .unwrap()
        .is_none()
    );
    put(
        &mut fixture,
        "next-plan",
        text("plan", 0, "Retain the assessment and replace the plan."),
    );
    let before = fixture.state();
    let counts = fixture.ledger();
    let prior_group = third.cursor.selected_group.clone().unwrap();
    let work = |digest| {
        PriorWorkDispositionV1::new(
            prior_group.clone(),
            Some(HistoricalReferenceV1::Item(HistoricalItemV1::new(
                score.source.attempt().clone(),
                score.item.item().clone(),
                digest,
            ))),
            PriorWorkChoiceV1::Retain,
            "Retain exact assessment provenance.".into(),
        )
        .unwrap()
    };

    let missing = activation(&fixture.workspace, &before.0, &definition, "missing-work");
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            missing,
            || panic!("prior-work shape refusal precedes the clock"),
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::GroupActivationPriorWorkInvalid(
            DomainError::InvalidState {
                reason: "activation history requires one to 128 prior-work dispositions after the first group",
            },
        ))
    ));
    fixture.compare_rejected(&before, "missing-work", counts);
    let mut stale = activation(&fixture.workspace, &before.0, &definition, "stale-work");
    stale.prior_work = vec![work(base)];
    assert!(matches!(
        activate_group(&mut fixture.store, stale, || TIME, &NativeDirectIdSourceV1),
        Err(SuccessorExecutionError::GroupActivationPriorWorkInvalid(
            DomainError::InvalidState {
                reason: "prior work value digest differs from retained history",
            },
        ))
    ));
    fixture.compare_rejected(&before, "stale-work", counts);
    let mut stale_plan = activation(&fixture.workspace, &before.0, &definition, "old-plan");
    stale_plan.prior_work = vec![work(score.item.value_digest().clone())];
    stale_plan.plan = original_plan;
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            stale_plan,
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::GroupPlanStale)
    ));
    fixture.compare_rejected(&before, "old-plan", counts);

    activate(
        &mut fixture,
        &group,
        "replacement",
        vec![work(score.item.value_digest().clone())],
    );
    let fresh = observe(&mut fixture);
    assert_ne!(fresh.cursor.selected_group, third.cursor.selected_group);
    assert_eq!(document(&fresh)["next_decision_entry"], 1);
    let decision_counter = document(&fresh)["counters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|counter| counter["scope"] == "group" && counter["node"] == "choose")
        .unwrap()
        .clone();
    assert_eq!(decision_counter["attempt_count"], 0);
    assert!(decision_counter["decision_entry"].is_null());
    let fresh_state = fixture.state();
    assert_eq!(
        fresh_state.0.group_history()[0],
        before.0.group_history()[0]
    );
    assert_eq!(
        fresh_state.0.group_history()[1]
            .identity()
            .definition_digest(),
        fresh_state.0.group_history()[0]
            .identity()
            .definition_digest()
    );
    let counts = fixture.ledger();
    let late = item_request(&fixture, &entry, "late-group-write");
    selection_conflict(
        record_successor_item(&mut fixture.store, late, write("score", 1, None), || {
            panic!("old selected group")
        })
        .unwrap_err(),
        &entry,
        &fresh,
    );
    fixture.compare_rejected(&fresh_state, "late-group-write", counts);
    let mut crossed_attempt = item_request(&fixture, &entry, "crossed-attempt");
    crossed_attempt.selected_group = fresh.cursor.selected_group.clone();
    attempt_conflict(
        record_successor_item(
            &mut fixture.store,
            crossed_attempt,
            write("score", 0, None),
            || panic!("old scope/attempt"),
        )
        .unwrap_err(),
        &entry,
        &fresh,
    );
    fixture.compare_rejected(&fresh_state, "crossed-attempt", counts);
    let wrong_receipt = request(&fixture, &fresh, "wrong-receipt");
    checked_reason(
        accept_successor_delegation(
            &mut fixture.store,
            wrong_receipt,
            accepted_package.clone(),
            || TIME,
        )
        .unwrap_err(),
        "delegation acceptance does not apply to the current action/package",
    );
    fixture.compare_rejected(&fresh_state, "wrong-receipt", counts);
    let original_accept = request(&fixture, &assessed, "accept");
    let replay_accept = accept_successor_delegation(
        &mut fixture.store,
        original_accept,
        accepted_package,
        || panic!("accepted response is historical replay"),
    )
    .unwrap();
    assert_eq!(replay_accept.outcome(), accepted.outcome());
    assert!(document(&observe(&mut fixture))["delegation"].is_null());

    let replay = request(&fixture, &first, "retry-first");
    let mut wrong_selection = replay;
    wrong_selection.cursor.selected_group = fresh.cursor.selected_group.clone();
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            wrong_selection,
            retry(),
            || panic!("changed identity must reject"),
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Direct(
            DirectStoreErrorV1::IdempotencyKeyReused
        )))
    ));
    assert_eq!(fixture.ledger(), counts);
    assert_eq!(fixture.state(), fresh_state);
    assert_eq!(
        lookup_successor_outcome(&fixture.store, &fixture.workspace, &key("retry-first"))
            .unwrap()
            .as_ref(),
        Some(old.outcome())
    );
}

fn delegation_declaration() -> Value {
    json!({
        "binding_ref":"native", "role_ref":"worker", "role_snapshot_digest":format!("sha256:{}", "9".repeat(64)),
        "max_rounds":1, "after_accept":"retain",
    })
}

#[test]
fn opted_in_base_anchor_requires_acceptance_of_the_current_plan() {
    let mut base = tracked_base();
    base["graph"]["nodes"][1]["delegation"] = delegation_declaration();
    let mut fixture = Fixture::new(&base, false);
    at(&mut fixture, "prefix", SuccessorCursorMutation::Complete);
    put(
        &mut fixture,
        "plan",
        text("plan", 0, "Initial plan before the goal"),
    );
    let before_goal = observe(&mut fixture);
    let before_package =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &before_goal.cursor)
            .unwrap();
    let accept = request(&fixture, &before_goal, "accept-before-goal");
    accept_successor_delegation(&mut fixture.store, accept, before_package.clone(), || TIME)
        .unwrap();
    at(
        &mut fixture,
        "goal",
        SuccessorCursorMutation::DefineGoal(goal("Define the package goal")),
    );
    let anchor = observe(&mut fixture);
    assert_eq!(
        anchor.cursor.execution.attempt,
        before_goal.cursor.execution.attempt
    );
    assert!(document(&anchor)["delegation"].is_null());
    let package =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &anchor.cursor).unwrap();
    assert_ne!(package.package_digest(), before_package.package_digest());
    assert_eq!(package.target().scope(), &ExecutionScopeV1::Base);
    let accept = request(&fixture, &anchor, "accept-first-plan");
    accept_successor_delegation(&mut fixture.store, accept, package.clone(), || TIME).unwrap();
    put(
        &mut fixture,
        "change-plan",
        text("plan", 1, "Change the design after acceptance"),
    );
    let edited = observe(&mut fixture);
    assert!(document(&edited)["delegation"].is_null());
    let admission = group_document();
    let state = fixture.state();
    let counts = fixture.ledger();
    let launch = activation(
        &fixture.workspace,
        &state.0,
        &canonical(&admission),
        "missing-approval",
    );
    checked_reason(
        activate_group(&mut fixture.store, launch, || TIME, &NativeDirectIdSourceV1).unwrap_err(),
        "current delegated anchor acceptance is required",
    );
    fixture.compare_rejected(&state, "missing-approval", counts);
    let changed =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &edited.cursor).unwrap();
    assert_ne!(changed, package);
    let accept = request(&fixture, &edited, "approve-new-plan");
    accept_successor_delegation(&mut fixture.store, accept, changed, || TIME).unwrap();
    activate(&mut fixture, &admission, "start-current-plan", vec![]);
    assert!(observe(&mut fixture).cursor.selected_group.is_some());
}

#[test]
fn goal_revision_closes_a_selected_completed_execution_with_its_terminal_record() {
    let (before, sources) = retained_fixture();
    let mut fixture = Fixture::with_state(&before, &sources);
    let selected = before.selected().unwrap();
    assert_eq!(selected.outcome(), GroupExecutionOutcomeV1::Completed);
    at(
        &mut fixture,
        "revise-completed-selection",
        revise(
            "Replace the scoped goal",
            "design",
            false,
            before.base_digest(),
        ),
    );
    fixture.reopen();
    let (after, definitions) = fixture.state();
    assert_eq!(sources, definitions);
    let closed = after.group_history().last().unwrap();
    assert_eq!(closed.identity(), selected.identity());
    assert_eq!(closed.terminal_attempt(), selected.terminal_attempt());
    assert_eq!(closed.outcome(), GroupExecutionOutcomeV1::Completed);
    let revision = after.goal_revision_history().last().unwrap();
    assert_eq!(closed.closure(), Some(&revision.closure));
    assert_eq!(
        revision.closure.cause,
        SelectionClosureCauseV1::GoalRevision
    );
    assert!(after.selected().is_none());
    assert_eq!(
        after.active_attempt().unwrap().target(),
        &revision.successor
    );
}

pub(super) fn tracked_base() -> Value {
    let mut base = base_document();
    base["goal_tracking"] = json!(true);
    base["node_definitions"]["prefix"] = action("Prefix");
    base["graph"]["entry"] = json!("prefix");
    base["node_definitions"]["review"] = json!({
        "type":"decision", "title":"Review", "objective":"Assess the goal.", "prompt":"Assess the goal.",
        "reason":{"required":true},
        "options":[{"id":"accept", "label":"Accept"},{"id":"reject", "label":"Reject"},
            {"id":"supersede", "label":"Supersede"}],
        "assessment":{"target":"session_goal", "outcomes":{"accept":"achieved", "reject":"not_achieved", "supersede":"superseded"}}
    });
    let nodes = base["graph"]["nodes"].as_array_mut().unwrap();
    let evidence = nodes[1]["evidence_from"].clone();
    nodes[1] = json!({"id":"review", "use":"review", "evidence_from":evidence,
        "routes":{"accept":{"effect":"advance", "to":"close"},"reject":{"effect":"advance", "to":"close"},
                  "supersede":{"effect":"advance", "to":"close"}}});
    nodes.insert(0, json!({"id":"prefix", "use":"prefix", "next":"design"}));
    base["manual_rework"] = json!({"allowed_targets":["prefix","review"]});
    base
}

fn goal(statement: &str) -> ExecutionGoalV3 {
    ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement).unwrap(),
        criteria: GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(
                CriterionId::new("known-work").unwrap(),
                "Preserve completed work.",
            )
            .unwrap(),
        ])
        .unwrap(),
    }
}

fn revise(
    title: &str,
    target: &str,
    reactivate: bool,
    basis: &Sha256Digest,
) -> SuccessorCursorMutation {
    SuccessorCursorMutation::ReviseGoal(ExecutionGoalChangeV3 {
        goal: goal(title),
        target: GraphNodeId::new(target).unwrap(),
        feedback: GroupClosureFeedbackV1::new(
            "Revise the selected plan".into(),
            CheckResultInputBasisV2::new("Observed fixture", basis.clone()).unwrap(),
            "Preserve the basis of the earlier run.".into(),
        )
        .unwrap(),
        reactivate,
    })
}

fn retain(group: GroupExecutionIdV1) -> PriorWorkDispositionV1 {
    PriorWorkDispositionV1::new(
        group,
        None,
        PriorWorkChoiceV1::Revalidate,
        "Consider the prior run when planning.".into(),
    )
    .unwrap()
}

fn finish_group(fixture: &mut Fixture, prefix: &str) -> SuccessorObservation {
    at(
        fixture,
        &format!("{prefix}-work"),
        SuccessorCursorMutation::Complete,
    );
    let terminal = observe(fixture);
    assert_eq!(document(&terminal)["next_decision_entry"], 1);
    put(
        fixture,
        &format!("{prefix}-report"),
        text("report", 0, "Group result"),
    );
    let updated = observe(fixture);
    let call = request(fixture, &updated, &format!("{prefix}-return"));
    return_from_group(&mut fixture.store, call, || TIME, &NativeDirectIdSourceV1).unwrap();
    observe(fixture)
}

#[test]
fn base_review_fences_goal_closure_reactivation_and_completed_group_cancellation() {
    let mut group = group_document();
    group["node_definitions"]["work"]["items"] = json!([optional_text("memo")]);
    let mut fixture = Fixture::new(&tracked_base(), false);
    at(&mut fixture, "prefix", SuccessorCursorMutation::Complete);
    at(
        &mut fixture,
        "legal-prefix",
        SuccessorCursorMutation::Rework {
            target: GraphNodeId::new("prefix").unwrap(),
            reason: reason("Revisit a declared prefix target before activation"),
        },
    );
    at(
        &mut fixture,
        "return-to-design",
        SuccessorCursorMutation::Complete,
    );
    put(
        &mut fixture,
        "first-plan",
        text("plan", 0, "First tracked plan"),
    );
    let before = fixture.state();
    let counts = fixture.ledger();
    let no_goal = activation(&fixture.workspace, &before.0, &canonical(&group), "no-goal");
    assert!(matches!(
        activate_group(
            &mut fixture.store,
            no_goal,
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ItemMutation(
            GraphMutationErrorV2::SessionGoalMissing
        ))
    ));
    fixture.compare_rejected(&before, "no-goal", counts);
    at(
        &mut fixture,
        "goal",
        SuccessorCursorMutation::DefineGoal(goal("Initial goal")),
    );
    activate(&mut fixture, &group, "group-first", vec![]);
    let first = observe(&mut fixture);
    let group_first = first.cursor.selected_group.clone().unwrap();
    let before = fixture.state();
    let counts = fixture.ledger();
    let basis = before.0.base_digest().clone();

    let wrong_target = request(&fixture, &first, "wrong-goal-target");
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            wrong_target,
            revise("Wrong target", "prefix", false, &basis),
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ItemMutation(
            GraphMutationErrorV2::GoalRevisionTargetNotAllowed { target_graph_node_id }
        )) if target_graph_node_id.as_str() == "prefix"
    ));
    fixture.compare_rejected(&before, "wrong-goal-target", counts);
    let mut stale_goal = request(&fixture, &first, "missing-goal-fence");
    stale_goal.cursor.execution.goal_revision = None;
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            stale_goal,
            revise("Stale goal", "design", false, &basis),
            || panic!("goal fence must precede clock"),
            &NativeDirectIdSourceV1,
        )
        , Err(SuccessorExecutionError::GoalRevisionMismatch { expected: None, actual })
            if actual == first.cursor.execution.goal_revision));
    fixture.compare_rejected(&before, "missing-goal-fence", counts);

    at(
        &mut fixture,
        "revise-running",
        revise("Second goal", "design", false, &basis),
    );
    fixture.reopen();
    let revised = fixture.state();
    let anchor = observe(&mut fixture);
    let counts = fixture.ledger();
    let late = item_request(&fixture, &first, "write-before-goal-revise");
    selection_conflict(
        record_successor_item(&mut fixture.store, late, text("memo", 0, "late"), || {
            panic!("goal revision closed this selection")
        })
        .unwrap_err(),
        &first,
        &anchor,
    );
    fixture.compare_rejected(&revised, "write-before-goal-revise", counts);
    let revised = revised.0;
    assert_eq!(revised.fence().goal_revision.unwrap().get(), 2);
    assert!(revised.selected().is_none());
    assert_eq!(
        revised.group_history()[0].outcome(),
        GroupExecutionOutcomeV1::Abandoned
    );
    let revision = revised.goal_revision_history().last().unwrap();
    assert_eq!(revision.predecessor.as_ref(), Some(&group_first));
    assert_eq!(
        revision.source,
        *first.cursor.execution.attempt.as_ref().unwrap()
    );
    assert_eq!(
        revision.closure.cause,
        SelectionClosureCauseV1::GoalRevision
    );
    assert_eq!(
        revision
            .closure
            .feedback
            .as_ref()
            .unwrap()
            .working_copy_basis()
            .digest(),
        &basis
    );
    assert_eq!(
        revision.successor,
        *revised.active_attempt().unwrap().target()
    );

    put(
        &mut fixture,
        "second-plan",
        text("plan", 0, "Plan under the second goal"),
    );
    activate(
        &mut fixture,
        &group,
        "group-second",
        vec![retain(group_first.clone())],
    );
    let review = finish_group(&mut fixture, "second");
    let group_second = review.cursor.selected_group.clone().unwrap();
    assert_eq!(
        review.cursor.execution.attempt.as_ref().unwrap().scope(),
        &ExecutionScopeV1::Base
    );
    assert_eq!(document(&review)["decision_entry"], 1);
    let report = read_current_evidence(
        &mut fixture.store,
        &fixture.workspace,
        &review.cursor,
        &EvidenceSourceV3::ExecutionResult,
        &ItemId::new("report").unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(
        matches!(report.source.scope(), ExecutionScopeV1::Group { group_execution_id, .. } if group_execution_id == &group_second)
    );
    let before = fixture.state();
    let counts = fixture.ledger();
    let mut crossed = request(&fixture, &review, "crossed-group");
    crossed.cursor.selected_group = Some(group_first.clone());
    let choose = || SuccessorCursorMutation::Decide {
        option: OptionId::new("accept").unwrap(),
        reason: reason("The goal is satisfied."),
        goal_outcome: Some(GoalOutcome::Achieved),
    };
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            crossed,
            choose(),
            || panic!("base cursor must fence activation"),
            &NativeDirectIdSourceV1,
        )
        , Err(SuccessorExecutionError::SelectedActivationMismatch { expected: Some(expected), actual: Some(actual) })
            if expected == group_first && actual == group_second));
    fixture.compare_rejected(&before, "crossed-group", counts);
    let prefix = request(&fixture, &review, "late-prefix");
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            prefix,
            SuccessorCursorMutation::Rework {
                target: GraphNodeId::new("prefix").unwrap(),
                reason: reason("Cross the activation boundary"),
            },
            || panic!("rejected manual rework must not read the clock"),
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ItemMutation(
            GraphMutationErrorV2::ManualReworkTargetNotAllowed { target_graph_node_id },
        )) if target_graph_node_id.as_str() == "prefix"
    ));
    fixture.compare_rejected(&before, "late-prefix", counts);

    at(
        &mut fixture,
        "local-suffix",
        SuccessorCursorMutation::Rework {
            target: GraphNodeId::new("review").unwrap(),
            reason: reason("Revisit the selected suffix review"),
        },
    );
    assert_eq!(document(&observe(&mut fixture))["decision_entry"], 2);
    at(&mut fixture, "assessed", choose());
    at(&mut fixture, "completed", SuccessorCursorMutation::Complete);
    fixture.reopen();
    let complete = observe(&mut fixture);
    assert!(complete.cursor.execution.attempt.is_none());
    assert!(complete.cursor.selected_group.is_none());
    let before = fixture.state();
    let counts = fixture.ledger();
    assert_eq!(before.0.lifecycle(), SessionLifecycle::Completed);
    assert_eq!(before.0.current_goal_outcome(), Some(GoalOutcome::Achieved));
    let terminal = before.0.group_history()[1].clone();
    assert_eq!(
        terminal.closure().unwrap().cause,
        SelectionClosureCauseV1::Completion
    );
    let no_reactivate = request(&fixture, &complete, "no-reactivation");
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            no_reactivate,
            revise("Third goal", "design", false, &basis),
            || TIME,
            &NativeDirectIdSourceV1,
        ),
        Err(SuccessorExecutionError::ItemMutation(
            GraphMutationErrorV2::ReactivationFlagRequired
        ))
    ));
    fixture.compare_rejected(&before, "no-reactivation", counts);
    at(
        &mut fixture,
        "reactivate",
        revise("Third goal", "design", true, &basis),
    );
    let reopened = fixture.state().0;
    let last = reopened.goal_revision_history().last().unwrap();
    assert_eq!(last.revision.get(), 3);
    assert_eq!(last.predecessor.as_ref(), Some(&group_second));
    assert_eq!(last.closure.cause, SelectionClosureCauseV1::Reactivation);
    assert_eq!(reopened.group_history()[1], terminal);
    assert!(reopened.current_goal_outcome().is_none());
    assert!(
        reopened
            .active_attempt()
            .unwrap()
            .slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );

    put(
        &mut fixture,
        "third-plan",
        text("plan", 0, "Plan under the reactivated goal"),
    );
    activate(
        &mut fixture,
        &group,
        "group-third",
        vec![retain(group_second)],
    );
    let review = finish_group(&mut fixture, "third");
    let completed_terminal = fixture
        .state()
        .0
        .selected()
        .unwrap()
        .terminal_attempt()
        .unwrap()
        .clone();
    at(
        &mut fixture,
        "cancel",
        SuccessorCursorMutation::Cancel(reason("Stop the current session.")),
    );
    fixture.reopen();
    let stopped = fixture.state();
    assert_eq!(stopped.0.lifecycle(), SessionLifecycle::Cancelled);
    let retained = stopped.0.group_history().last().unwrap();
    assert_eq!(retained.outcome(), GroupExecutionOutcomeV1::Completed);
    assert_eq!(retained.terminal_attempt(), Some(&completed_terminal));
    assert_eq!(
        retained.closure().unwrap().cause,
        SelectionClosureCauseV1::Cancel
    );
    assert_eq!(
        &retained.closure().unwrap().source,
        review.cursor.execution.attempt.as_ref().unwrap()
    );
    let stopped_view = observe(&mut fixture);
    let may_not_reactivate = request(&fixture, &stopped_view, "cancelled-retry");
    let counts = fixture.ledger();
    checked_reason(
        issue_cursor_mutation(
            &mut fixture.store,
            may_not_reactivate,
            revise("After cancel", "design", true, &basis),
            || TIME,
            &NativeDirectIdSourceV1,
        )
        .unwrap_err(),
        "goal revision requires a running session or explicit completed-session reactivation",
    );
    fixture.compare_rejected(&stopped, "cancelled-retry", counts);
}

#[test]
fn blockers_are_scoped_and_recover_without_a_rejection_outcome() {
    let mut fixture = Fixture::new(&base_document(), true);
    activate(&mut fixture, &group_document(), "activate", vec![]);
    let open = at(
        &mut fixture,
        "block",
        SuccessorCursorMutation::Block(reason("Waiting for a correction.")),
    );
    let view = response(&open);
    let blocker = BlockerId::new(view["open_blockers"][0].as_str().unwrap()).unwrap();
    let current = observe(&mut fixture);
    let state = fixture.state();
    let counts = fixture.ledger();
    let blocked = request(&fixture, &current, "blocked-complete");
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            blocked,
            SuccessorCursorMutation::Complete,
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::BlockersPresent
        ))
    ));
    fixture.compare_rejected(&state, "blocked-complete", counts);
    at(
        &mut fixture,
        "unblock",
        SuccessorCursorMutation::Unblock(blocker.clone()),
    );
    at(
        &mut fixture,
        "blocked-complete",
        SuccessorCursorMutation::Complete,
    );
    let fresh = observe(&mut fixture);
    let state = fixture.state();
    let counts = fixture.ledger();
    let late = request(&fixture, &fresh, "late-unblock");
    assert!(matches!(
        issue_cursor_mutation(
            &mut fixture.store,
            late,
            SuccessorCursorMutation::Unblock(blocker),
            || TIME,
            &NativeDirectIdSourceV1
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::BlockerNotCurrent
        ))
    ));
    fixture.compare_rejected(&state, "late-unblock", counts);
}
