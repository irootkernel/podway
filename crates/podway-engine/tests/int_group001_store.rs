//! Candidate-only Store admission using real successor definitions.

use podway_config::ProcedureDocumentFormat;
use podway_config::execution_groups::{
    SuccessorDefinition, ValidatedSuccessorDefinition, parse_successor_document,
    validate_successor_definition,
};
use podway_core::execution_groups::*;
use podway_core::{
    AttemptId, BlockerId, BlockerState, BlockerStateV2, CheckResultInputBasisV2, CriterionId,
    GoalCriterionV2, GoalDefinitionV2, GoalStatementV2, ItemId, RecordedItemValueV2, Revision,
    SessionId, UnixMillis, canonicalize_json_v1, recorded_value_digest_v2,
};
use podway_core::{GoalOutcome, OptionId, ReasonV2};
use podway_store::ValidatedWorkspaceRootV1;
use podway_store::direct_store::{
    DirectStoreClassificationV1, DirectWorkspaceLayoutV1, classify_direct_store_v1,
};
use podway_store::successor_store::*;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const BASE: &str = r#"schema: podway.procedure/v3
id: test-design
version: "1"
name: Test design
purpose: Qualify retained execution state.
goal_tracking: true
node_definitions:
  design:
    type: action
    title: Design
    intent: Declare a plan.
    items:
      - id: plan
        type: text
        prompt: Describe the plan.
        required: true
  guard:
    type: decision
    title: Assess outcome
    objective: Assess the current goal.
    prompt: Did the group satisfy the goal?
    options:
      - id: accept
        label: Accept
      - id: reject
        label: Reject
      - id: supersede
        label: Supersede
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        accept: achieved
        reject: not_achieved
        supersede: superseded
  close:
    type: action
    title: Close
    intent: Close the task.
graph:
  entry: design
  nodes:
    - id: design
      use: design
      next: guard
    - id: guard
      use: guard
      routes:
        accept:
          effect: advance
          to: close
        reject:
          effect: advance
          to: close
        supersede:
          effect: advance
          to: close
    - id: close
      use: close
      terminal: true
execution_group:
  anchor: design
  success_return: guard
  plan_inputs: [plan]
  result_items: []
  replan_decisions: []
"#;

const GROUP: &str = r#"schema: podway.execution-group/v1
id: test-work
version: "1"
name: Test work
purpose: Verify retained attempt values.
plan_inputs:
  - id: plan
    type: text
    prompt: Read the plan.
    required: true
node_definitions:
  work:
    type: action
    title: Work
    intent: Perform only the current plan.
    items:
      - id: note
        type: text
        prompt: Describe the attempt.
        required: false
  done:
    type: action
    title: Return
    intent: Return to the base.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      next: done
      evidence_from:
        - scope: plan
          required: true
          items: [plan]
    - id: done
      use: done
      terminal: true
"#;

fn parse(source: &str, format: ProcedureDocumentFormat) -> ValidatedSuccessorDefinition {
    validate_successor_definition(parse_successor_document(source.as_bytes(), format).unwrap())
        .unwrap()
}

fn admit_definition(source: &str) -> Result<CandidateDefinition, CandidateStoreError> {
    let invalid_source = |_| CandidateStoreError::Corrupt("definition failed Config admission");
    let parsed = parse_successor_document(source.as_bytes(), ProcedureDocumentFormat::Json)
        .map_err(invalid_source)?;
    let definition = validate_successor_definition(parsed).map_err(invalid_source)?;
    if definition.canonical_json().as_str() != source {
        return Err(CandidateStoreError::Corrupt(
            "definition bytes are not normalized",
        ));
    }
    Ok(match definition.definition() {
        SuccessorDefinition::Base(base) => CandidateDefinition::Base(base.clone()),
        SuccessorDefinition::Group(group) => CandidateDefinition::Group(group.clone()),
    })
}

fn attempt(sequence: u8) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{sequence:012}")).unwrap()
}

fn goal(statement: &str) -> ExecutionGoalV3 {
    ExecutionGoalV3 {
        statement: GoalStatementV2::new(statement).unwrap(),
        criteria: GoalDefinitionV2::new(vec![
            GoalCriterionV2::new(CriterionId::new("retained").unwrap(), "Preserve work").unwrap(),
        ])
        .unwrap(),
    }
}

fn record(state: &mut ExecutionSessionV3, item: &str, text: &str, revision: Revision) {
    let target = state.active_attempt().unwrap().target().clone();
    state
        .record_item(
            &target,
            &ItemId::new(item).unwrap(),
            revision,
            Some(RecordedItemValueV2::text(text).unwrap()),
            UnixMillis::new(2),
        )
        .unwrap();
}

fn fixture_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("podway-group001-")
        .tempdir_in(std::fs::canonicalize("/tmp").unwrap())
        .unwrap()
}

pub(super) fn fixture() -> (ExecutionSessionV3, CandidateSources) {
    fixture_with_note("Second retained item", false)
}

fn fixture_with_note(note: &str, extra_edit: bool) -> (ExecutionSessionV3, CandidateSources) {
    fixture_with_prior_work(note, extra_edit, 1, "Keep the earlier note.")
}

fn fixture_with_prior_work(
    note: &str,
    extra_edit: bool,
    dispositions: usize,
    rationale: &str,
) -> (ExecutionSessionV3, CandidateSources) {
    let canonical = |source: &str| {
        let authored = parse(source, ProcedureDocumentFormat::Yaml);
        parse(
            authored.canonical_json().as_str(),
            ProcedureDocumentFormat::Json,
        )
    };
    let base = canonical(BASE);
    let group = canonical(GROUP);
    let SuccessorDefinition::Base(base_definition) = base.definition() else {
        panic!("fixture base has wrong schema");
    };
    let SuccessorDefinition::Group(group_definition) = group.definition() else {
        panic!("fixture group has wrong schema");
    };
    let session_id = SessionId::new("00000000-0000-4000-8000-000000000100").unwrap();
    let first_group = GroupExecutionIdV1::new("00000000-0000-4000-8000-000000000201").unwrap();
    let fresh_group = GroupExecutionIdV1::new("00000000-0000-4000-8000-000000000202").unwrap();
    let basis =
        CheckResultInputBasisV2::new("Candidate worktree basis", base.digest().clone()).unwrap();
    let time = UnixMillis::new(2);
    let mut state = ExecutionSessionV3::begin(
        session_id.clone(),
        base_definition.clone(),
        base.digest().clone(),
        Some(goal("First goal")),
        attempt(1),
        UnixMillis::new(1),
    )
    .unwrap();
    let activate = |state: &mut ExecutionSessionV3, execution, predecessor, prior_work, entry| {
        let fence = state.fence();
        let identity = ActivationIdentityV1::new(ActivationIdentityInputV1 {
            execution,
            session: session_id.clone(),
            base_digest: base.digest().clone(),
            definition_digest: group.digest().clone(),
            anchor: base_definition.boundary().anchor().clone(),
            goal_revision: fence.goal_revision,
            plan: state.plan_binding().unwrap(),
            predecessor,
            working_copy_basis: basis.clone(),
            prior_work,
        })
        .unwrap();
        state
            .activate(
                &fence,
                identity,
                group_definition.clone(),
                group.digest(),
                attempt(entry),
                time,
            )
            .unwrap();
    };
    record(&mut state, "plan", "First design", Revision::ZERO);
    activate(&mut state, first_group.clone(), None, vec![], 2);
    record(&mut state, "note", "Superseded edit", Revision::ZERO);
    record(&mut state, "note", "First retained item", Revision::new(1));
    let output =
        recorded_value_digest_v2(&RecordedItemValueV2::text("First retained item").unwrap())
            .unwrap();
    state
        .replace_blockers(
            &state.fence(),
            vec![
                BlockerStateV2::new(
                    BlockerId::new("00000000-0000-4000-8000-000000000050").unwrap(),
                    attempt(2),
                    "Delay",
                    BlockerState::Open,
                    time,
                    None,
                )
                .unwrap(),
            ],
            time,
        )
        .unwrap();
    state
        .revise_goal(
            &state.fence(),
            ExecutionGoalChangeV3 {
                goal: goal("New goal"),
                target: base_definition.boundary().anchor().clone(),
                feedback: GroupClosureFeedbackV1::new(
                    "Revise scope".to_owned(),
                    basis.clone(),
                    "Preserve partial work".to_owned(),
                )
                .unwrap(),
                reactivate: false,
            },
            attempt(3),
            time,
        )
        .unwrap();
    record(&mut state, "plan", "New design", Revision::ZERO);
    activate(
        &mut state,
        fresh_group,
        Some(first_group.clone()),
        (0..dispositions)
            .map(|_| {
                PriorWorkDispositionV1::new(
                    first_group.clone(),
                    Some(HistoricalReferenceV1::Item(HistoricalItemV1::new(
                        attempt(2),
                        ItemId::new("note").unwrap(),
                        output.clone(),
                    ))),
                    PriorWorkChoiceV1::Retain,
                    rationale.to_owned(),
                )
                .unwrap()
            })
            .collect(),
        4,
    );
    if extra_edit {
        record(
            &mut state,
            "note",
            "Intermediate alternate note",
            Revision::ZERO,
        );
        record(&mut state, "note", note, Revision::new(1));
    } else {
        record(&mut state, "note", note, Revision::ZERO);
    }
    state
        .complete(&state.fence(), Some(attempt(5)), time)
        .unwrap();
    state
        .complete(&state.fence(), Some(attempt(6)), time)
        .unwrap();
    let sources = [&base, &group]
        .into_iter()
        .map(|value| {
            (
                value.digest().clone(),
                value.canonical_json().as_str().to_owned(),
            )
        })
        .collect();
    (state, sources)
}

fn load(
    connection: &mut Connection,
    state: &ExecutionSessionV3,
) -> Result<ExecutionSessionV3, CandidateStoreError> {
    let transaction = connection.transaction().unwrap();
    let result = load_candidate_execution(&transaction, &state.fence().session, admit_definition)
        .map(|(state, _)| state);
    transaction.rollback().unwrap();
    result
}

#[test]
fn cold_reopen_reconstructs_the_exact_group_trace() {
    let fixture_dir = fixture_dir();
    let path = fixture_dir.path().join("state.sqlite3");
    let (state, sources) = fixture();
    let mut connection = create_candidate_fixture(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    transaction.commit().unwrap();
    drop(connection);
    let mut reopened = reopen_candidate_fixture(&path).unwrap();
    let transaction = reopened.transaction().unwrap();
    let (current, reopened_sources) =
        load_candidate_execution(&transaction, &state.fence().session, admit_definition).unwrap();
    assert_eq!(current, state);
    assert_eq!(reopened_sources, sources);
    transaction.rollback().unwrap();
    assert_eq!(current.group_history().len(), 2);
    assert_eq!(
        current.group_history()[0].outcome(),
        GroupExecutionOutcomeV1::Abandoned
    );
    assert_eq!(
        current.group_history()[1].outcome(),
        GroupExecutionOutcomeV1::Completed
    );
    assert_eq!(
        current.next_decision_entry(current.active_attempt().unwrap().target().node()),
        Some(2)
    );
    assert_eq!(current.goal_revision_history().len(), 1);
    let distinct_definitions: u64 = reopened
        .query_row("SELECT count(*) FROM successor_definitions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(distinct_definitions, 2);
}

fn persist(connection: &mut Connection, state: &ExecutionSessionV3, sources: &CandidateSources) {
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, state, sources).unwrap();
    transaction.commit().unwrap();
}

pub(super) fn alter_record(
    connection: &Connection,
    selection: &str,
    update: &str,
    pointer: &str,
    value: Value,
) {
    let payload: String = connection
        .query_row(selection, [], |row| row.get(0))
        .unwrap();
    let mut payload: Value = serde_json::from_str(&payload).unwrap();
    *payload.pointer_mut(pointer).unwrap() = value;
    let bytes = canonicalize_json_v1(&payload).unwrap();
    let digest = format!("sha256:{:x}", Sha256::digest(bytes.as_bytes()));
    connection.execute(update, params![bytes, digest]).unwrap();
}

fn valid_substitute_definition(sources: &CandidateSources, state: &ExecutionSessionV3) -> String {
    let group = state.group_history()[0].identity().definition_digest();
    let mut value: Value = serde_json::from_str(&sources[group]).unwrap();
    value["version"] = json!("2");
    let replacement = parse(&value.to_string(), ProcedureDocumentFormat::Json);
    assert_ne!(replacement.digest(), group);
    replacement.canonical_json().as_str().to_owned()
}

#[test]
fn rejects_tampered_definitions_values_and_rehashed_cross_scope_records() {
    let (state, sources) = fixture();
    let replacement = valid_substitute_definition(&sources, &state);
    let cases = [
        (
            "definition kind, digest, canonical shape, or size differs",
            "UPDATE successor_definitions SET document = ?1 WHERE kind = 'group'",
            Some(replacement.as_str()),
        ),
        (
            "retained record checksum differs",
            "UPDATE successor_attempts SET record_json = '{}' WHERE ordinal = 2",
            None,
        ),
    ];
    for (expected, sql, parameter) in cases {
        let root = fixture_dir();
        let db = root.path().join("fixture.sqlite3");
        let mut connection = create_candidate_fixture(&db).unwrap();
        persist(&mut connection, &state, &sources);
        if let Some(parameter) = parameter {
            connection.execute(sql, [parameter]).unwrap();
        } else {
            connection.execute(sql, []).unwrap();
        }
        drop(connection);
        assert!(
            matches!(
                load(&mut reopen_candidate_fixture(&db).unwrap(), &state),
                Err(CandidateStoreError::Corrupt(reason)) if reason == expected
            ),
            "{sql}"
        );
    }
    let attempt_selection = "SELECT record_json FROM successor_attempts WHERE ordinal = 2";
    let attempt_update =
        "UPDATE successor_attempts SET record_json = ?1, record_digest = ?2 WHERE ordinal = 2";
    let activation_selection = "SELECT record_json FROM successor_activations WHERE ordinal = 2";
    let activation_update =
        "UPDATE successor_activations SET record_json = ?1, record_digest = ?2 WHERE ordinal = 2";
    let session_selection = "SELECT record_json FROM successor_sessions";
    let session_update = "UPDATE successor_sessions SET record_json = ?1, record_digest = ?2";
    let cases = [
        (
            "retained attempt row identity differs",
            attempt_selection,
            attempt_update,
            "/target/scope/group/group_execution_id",
            json!("00000000-0000-4000-8000-000000000202"),
        ),
        (
            "retained item digest differs from its value",
            attempt_selection,
            attempt_update,
            "/slots/0/value_digest",
            json!(format!("sha256:{}", "a".repeat(64))),
        ),
        (
            "retained activation row identity differs",
            activation_selection,
            activation_update,
            "/execution",
            json!("00000000-0000-4000-8000-000000000299"),
        ),
        (
            "retained session identity differs",
            session_selection,
            session_update,
            "/id",
            json!("00000000-0000-4000-8000-000000000777"),
        ),
        (
            "retained trace differs from replayed execution",
            activation_selection,
            activation_update,
            "/outcome",
            json!("running"),
        ),
    ];
    for (expected, selection, update, pointer, value) in cases {
        let root = fixture_dir();
        let db = root.path().join("fixture.sqlite3");
        let mut connection = create_candidate_fixture(&db).unwrap();
        persist(&mut connection, &state, &sources);
        alter_record(&connection, selection, update, pointer, value);
        drop(connection);
        let error = load(&mut reopen_candidate_fixture(&db).unwrap(), &state).unwrap_err();
        match error {
            CandidateStoreError::Corrupt(reason)
            | CandidateStoreError::Domain(podway_core::DomainError::InvalidState { reason }) => {
                assert_eq!(reason, expected, "{pointer}");
            }
            error => panic!("{pointer}: unexpected admission error: {error:?}"),
        }
    }
}

#[test]
fn rejects_rehashed_noncanonical_retained_payload() {
    let (state, sources) = fixture();
    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    persist(&mut connection, &state, &sources);
    let payload: String = connection
        .query_row(
            "SELECT record_json FROM successor_attempts WHERE ordinal = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let reencoded = format!(" {payload}");
    let digest = format!("sha256:{:x}", Sha256::digest(reencoded.as_bytes()));
    connection
        .execute(
            "UPDATE successor_attempts SET record_json = ?1, record_digest = ?2 WHERE ordinal = 2",
            params![reencoded, digest],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        load(&mut reopen_candidate_fixture(&db).unwrap(), &state),
        Err(CandidateStoreError::Corrupt(
            "retained record is not canonical"
        ))
    ));
}

#[test]
fn production_v2_store_classifies_candidate_generation_without_opening_it() {
    let fixture_dir = fixture_dir();
    let root = fixture_dir.path().canonicalize().unwrap();
    let layout =
        DirectWorkspaceLayoutV1::new(ValidatedWorkspaceRootV1::from_path(&root).unwrap()).unwrap();
    std::fs::create_dir_all(layout.runtime_directory()).unwrap();
    let path = layout.database();
    drop(create_candidate_fixture(&path).unwrap());
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        classify_direct_store_v1(&layout).unwrap(),
        DirectStoreClassificationV1::Newer { generation: 16 }
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!layout.shm().exists());
}

#[test]
fn a_conflicting_row_or_stale_session_cannot_report_a_persisted_success() {
    let (state, sources) = fixture();
    let stale = ExecutionSessionV3::begin(
        state.fence().session,
        state.base_definition().clone(),
        state.base_digest().clone(),
        state.initial_goal().cloned(),
        attempt(1),
        UnixMillis::new(1),
    )
    .unwrap();
    let mut shorter = stale.clone();
    let last_revision = state.fence().revision.get();
    for revision in 0..=last_revision {
        let value = if revision == last_revision {
            "Alternate history".to_owned()
        } else {
            format!("Alternate history draft {revision}")
        };
        record(&mut shorter, "plan", &value, Revision::new(revision));
    }
    assert!(shorter.fence().revision > state.fence().revision);
    let variants = [
        (
            Some("UPDATE successor_attempts SET attempt_id = 'unexpected' WHERE ordinal = 5"),
            &state,
        ),
        (
            Some("UPDATE successor_goal_revisions SET record_json = '{}' WHERE ordinal = 2"),
            &state,
        ),
        (None, &stale),
        (None, &shorter),
    ];
    for (tamper, candidate) in variants {
        let root = fixture_dir();
        let db = root.path().join("fixture.sqlite3");
        let mut connection = create_candidate_fixture(&db).unwrap();
        let transaction = connection.transaction().unwrap();
        persist_candidate_execution(&transaction, &state, &sources).unwrap();
        transaction.commit().unwrap();

        let transaction = connection.transaction().unwrap();
        if let Some(sql) = tamper {
            transaction.execute(sql, []).unwrap();
        }
        assert!(matches!(
            persist_candidate_execution(&transaction, candidate, &sources),
            Err(CandidateStoreError::Corrupt(_))
        ));
        transaction.rollback().unwrap();
        assert_eq!(load(&mut connection, &state).unwrap(), state);
    }
}

#[test]
fn reopened_state_can_finish_and_reactivate_with_the_original_history() {
    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let (state, sources) = fixture();
    let mut connection = create_candidate_fixture(&db).unwrap();
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    transaction.commit().unwrap();
    let mut state = load(&mut connection, &state).unwrap();
    let time = UnixMillis::new(3);
    state
        .decide(
            &state.fence(),
            OptionId::new("accept").unwrap(),
            ReasonV2::new("Evaluation complete").unwrap(),
            Some(GoalOutcome::Achieved),
            attempt(7),
            time,
        )
        .unwrap();
    state.complete(&state.fence(), None, time).unwrap();
    let terminal = state.clone();
    assert_eq!(
        terminal.attempt_history()[5].validity(),
        podway_core::AttemptValidityV2::Valid
    );
    state
        .revise_goal(
            &state.fence(),
            ExecutionGoalChangeV3 {
                goal: goal("Reactivated goal"),
                target: state.base_definition().boundary().anchor().clone(),
                feedback: GroupClosureFeedbackV1::new(
                    "Renew the goal".to_owned(),
                    CheckResultInputBasisV2::new("Renewed basis", state.base_digest().clone())
                        .unwrap(),
                    "Carry exact old records".to_owned(),
                )
                .unwrap(),
                reactivate: true,
            },
            attempt(8),
            UnixMillis::new(4),
        )
        .unwrap();
    assert_eq!(
        state.attempt_history()[5].validity(),
        podway_core::AttemptValidityV2::Stale
    );
    assert_eq!(
        state.attempt_history()[5].decision(),
        terminal.attempt_history()[5].decision()
    );
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &terminal, &sources).unwrap();
    transaction.commit().unwrap();
    assert_eq!(load(&mut connection, &terminal).unwrap(), terminal);
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    transaction.commit().unwrap();
    drop(connection);
    let mut reopened = reopen_candidate_fixture(&db).unwrap();
    assert_eq!(load(&mut reopened, &state).unwrap(), state);
    assert_eq!(
        state.group_history()[1].closure().unwrap().cause,
        SelectionClosureCauseV1::Completion
    );
    assert!(state.selected().is_none());
}

fn blocker(state: &ExecutionSessionV3, id: u8, created_at: UnixMillis) -> BlockerStateV2 {
    BlockerStateV2::new(
        BlockerId::new(format!("00000000-0000-4000-8000-{id:012}")).unwrap(),
        state.active_attempt().unwrap().target().attempt().clone(),
        "Waiting for input",
        BlockerState::Open,
        created_at,
        None,
    )
    .unwrap()
}

#[test]
fn cold_reopen_preserves_blocker_order_by_creation_time_then_id() {
    let (mut state, sources) = fixture();
    let first = UnixMillis::new(3);
    let second = UnixMillis::new(4);
    let earlier = blocker(&state, 71, first);
    let later = blocker(&state, 70, second);
    let tie_break = blocker(&state, 69, second);
    state
        .replace_blockers(&state.fence(), vec![earlier.clone()], first)
        .unwrap();
    state
        .replace_blockers(
            &state.fence(),
            vec![later.clone(), earlier.clone(), tie_break.clone()],
            second,
        )
        .unwrap();
    assert_eq!(
        state.active_attempt().unwrap().blockers(),
        &[earlier, tie_break, later]
    );
    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    transaction.commit().unwrap();
    drop(connection);
    assert_eq!(
        load(&mut reopen_candidate_fixture(&db).unwrap(), &state).unwrap(),
        state
    );
}

#[test]
fn cold_reopen_rejects_blocker_id_reused_across_attempts() {
    let (mut state, sources) = fixture();
    let time = UnixMillis::new(3);
    let added = blocker(&state, 70, time);
    state
        .replace_blockers(&state.fence(), vec![added], time)
        .unwrap();
    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    transaction.commit().unwrap();
    let ordinal = state.attempt_history().len();
    let record: String = connection
        .query_row(
            "SELECT record_json FROM successor_attempts WHERE ordinal = ?1",
            [ordinal],
            |row| row.get(0),
        )
        .unwrap();
    let mut record: Value = serde_json::from_str(&record).unwrap();
    record["blockers"][0]["id"] = serde_json::json!(
        state.attempt_history()[1].blockers()[0]
            .blocker_id()
            .as_str()
    );
    let encoded = canonicalize_json_v1(&record).unwrap();
    let checksum = format!("sha256:{:x}", Sha256::digest(encoded.as_bytes()));
    connection
        .execute(
            "UPDATE successor_attempts SET record_json = ?1, record_digest = ?2 WHERE ordinal = ?3",
            params![encoded, checksum, ordinal],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        load(&mut reopen_candidate_fixture(&db).unwrap(), &state),
        Err(CandidateStoreError::Domain(
            podway_core::DomainError::InvalidState {
                reason: "blocker identity is reused across retained attempts"
            }
        ))
    ));
}

#[test]
fn a_higher_revision_cannot_replace_a_closed_attempt_payload() {
    let (state, sources) = fixture();
    let (alternate, alternate_sources) =
        fixture_with_note("A later branch's version of closed work", true);
    assert_eq!(alternate_sources, sources);
    assert!(alternate.fence().revision > state.fence().revision);
    assert_eq!(
        alternate.attempt_history()[3].target(),
        state.attempt_history()[3].target()
    );
    assert_eq!(alternate.group_history(), state.group_history());
    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    transaction.commit().unwrap();

    let transaction = connection.transaction().unwrap();
    assert!(matches!(
        persist_candidate_execution(&transaction, &alternate, &sources),
        Err(CandidateStoreError::Corrupt(
            "closed retained attempt payload differs"
        ))
    ));
    transaction.rollback().unwrap();
    assert_eq!(load(&mut connection, &state).unwrap(), state);
}

#[test]
fn persists_full_core_admitted_prior_work_with_escaped_rationales() {
    let rationale = format!("A{}", "\u{0001}".repeat(1_999));
    assert_eq!(rationale.chars().count(), 2_000);
    let (state, sources) = fixture_with_prior_work(
        "Retain exact records",
        false,
        MAX_PRIOR_WORK_DISPOSITIONS_V1,
        &rationale,
    );
    let prior_work = state.group_history()[1].identity().prior_work();
    assert_eq!(prior_work.len(), MAX_PRIOR_WORK_DISPOSITIONS_V1);
    assert!(
        prior_work
            .iter()
            .all(|entry| entry.rationale() == rationale)
    );
    let root = fixture_dir();
    let path = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    persist_candidate_execution(&transaction, &state, &sources).unwrap();
    let record_bytes: usize = transaction
        .query_row(
            "SELECT length(CAST(record_json AS BLOB)) FROM successor_activations WHERE ordinal = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(record_bytes > 1_048_576);
    transaction.commit().unwrap();
    drop(connection);
    assert_eq!(
        load(&mut reopen_candidate_fixture(&path).unwrap(), &state).unwrap(),
        state
    );
}

#[test]
fn rejects_lower_revision_of_same_length_trace() {
    let (stale, sources) = fixture();
    let mut current = stale.clone();
    let time = UnixMillis::new(3);
    let added = blocker(&current, 71, time);
    current
        .replace_blockers(&current.fence(), vec![added], time)
        .unwrap();
    assert_eq!(
        current.attempt_history().len(),
        stale.attempt_history().len()
    );
    assert!(current.fence().revision > stale.fence().revision);

    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    persist(&mut connection, &current, &sources);
    let transaction = connection.transaction().unwrap();
    assert!(matches!(
        persist_candidate_execution(&transaction, &stale, &sources),
        Err(CandidateStoreError::Corrupt(
            "conflicting retained session identity or revision"
        ))
    ));
    transaction.rollback().unwrap();
    assert_eq!(load(&mut connection, &current).unwrap(), current);
}

#[test]
fn persistence_rejects_changed_immutable_activation_and_definition() {
    let (state, sources) = fixture();
    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    persist(&mut connection, &state, &sources);
    let selection = "SELECT record_json FROM successor_activations WHERE ordinal = 1";
    let update =
        "UPDATE successor_activations SET record_json = ?1, record_digest = ?2 WHERE ordinal = 1";

    for (pointer, value) in [
        (
            "/working_copy_basis/descriptor",
            json!("Replaced activation basis"),
        ),
        (
            "/closure/feedback/feedback",
            json!("Replaced prior closure feedback"),
        ),
        ("/outcome", json!("completed")),
    ] {
        let transaction = connection.transaction().unwrap();
        alter_record(&transaction, selection, update, pointer, value);
        assert!(
            matches!(
                persist_candidate_execution(&transaction, &state, &sources),
                Err(CandidateStoreError::Corrupt(
                    "conflicting retained activation identity or outcome"
                ))
            ),
            "{pointer}"
        );
        transaction.rollback().unwrap();
    }

    let transaction = connection.transaction().unwrap();
    let replacement = valid_substitute_definition(&sources, &state);
    transaction
        .execute(
            "UPDATE successor_definitions SET document = ?1 WHERE kind = 'group'",
            [replacement],
        )
        .unwrap();
    assert!(matches!(
        persist_candidate_execution(&transaction, &state, &sources),
        Err(CandidateStoreError::Corrupt(
            "immutable definition bytes differ"
        ))
    ));
    transaction.rollback().unwrap();
    assert_eq!(load(&mut connection, &state).unwrap(), state);
}

#[test]
fn candidate_writer_admission_checks_generation_identity_and_schema() {
    let cases = [
        (
            "PRAGMA user_version = 15",
            "unsupported candidate store generation",
        ),
        (
            "UPDATE writer_contract SET generation_id = 'different-writer'",
            "candidate writer identity differs",
        ),
        (
            "UPDATE writer_contract SET contract_manifest_digest = 'sha256:0000000000000000000000000000000000000000000000000000000000000000'",
            "candidate writer identity differs",
        ),
        (
            "CREATE TABLE unrelated (extra TEXT) STRICT",
            "candidate schema inventory differs",
        ),
    ];
    for (sql, expected) in cases {
        let root = fixture_dir();
        let path = root.path().join("fixture.sqlite3");
        let connection = create_candidate_fixture(&path).unwrap();
        connection.execute_batch(sql).unwrap();
        drop(connection);
        assert!(
            matches!(
                reopen_candidate_fixture(&path),
                Err(CandidateStoreError::Corrupt(reason)) if reason == expected
            ),
            "{sql}"
        );
    }
}

#[test]
fn corrupt_ordinals_and_oversized_text_fail_before_record_decode() {
    let (state, sources) = fixture();
    let cases = [
        (
            "UPDATE successor_activations SET ordinal = 65, record_json = '{}' WHERE ordinal = 2",
            "retained activation ordinal differs",
        ),
        (
            "UPDATE successor_goal_revisions SET ordinal = 65537, record_json = '{}' WHERE ordinal = 2",
            "retained goal revision ordinal differs",
        ),
        (
            "UPDATE successor_sessions SET record_json = CAST(zeroblob(1048577) AS TEXT)",
            "retained text exceeds its byte bound",
        ),
        (
            "UPDATE successor_activations SET record_json = CAST(zeroblob(2097153) AS TEXT) WHERE ordinal = 2",
            "retained text exceeds its byte bound",
        ),
        (
            "UPDATE successor_goal_revisions SET record_json = CAST(zeroblob(1048577) AS TEXT)",
            "retained text exceeds its byte bound",
        ),
        (
            "UPDATE successor_definitions SET document = CAST(zeroblob(1048577) AS TEXT) WHERE kind = 'group'",
            "retained text exceeds its byte bound",
        ),
    ];
    for (sql, expected) in cases {
        let root = fixture_dir();
        let path = root.path().join("fixture.sqlite3");
        let mut connection = create_candidate_fixture(&path).unwrap();
        persist(&mut connection, &state, &sources);
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON")
            .unwrap();
        connection.execute_batch(sql).unwrap();
        drop(connection);
        assert!(
            matches!(
                load(&mut reopen_candidate_fixture(&path).unwrap(), &state),
                Err(CandidateStoreError::Corrupt(reason)) if reason == expected
            ),
            "{sql}"
        );
    }
}

fn group_with_replan_and_delegation(
    package: &podway_core::Sha256Digest,
) -> ValidatedSuccessorDefinition {
    let mut document: Value = serde_norway::from_str(GROUP).unwrap();
    document["id"] = json!("replanned-work");
    document["manual_rework"] = json!({"allowed_targets": ["work"]});
    document["node_definitions"]["prepare"] = json!({
        "type": "action", "title": "Prepare", "intent": "Inspect the plan.",
    });
    document["node_definitions"]["work"]["items"] = json!([
        {"id": "verdict", "type": "choice", "prompt": "Record verdict.", "required": true, "choices": ["success", "failure"]},
        {"id": "confirmed", "type": "confirm", "prompt": "Confirm record.", "required": false},
        {"id": "score", "type": "integer", "prompt": "Record score.", "required": false},
        {"id": "findings", "type": "list", "prompt": "Record findings.", "required": false},
        {"id": "report", "type": "artifact", "prompt": "Reference report.", "required": false},
    ]);
    document["node_definitions"]["review"] = json!({
        "type": "decision", "title": "Review", "objective": "Decide whether to replan.",
        "prompt": "Choose.", "reason": {"required": true},
        "options": [
            {"id": "continue", "label": "Continue"},
            {"id": "replan", "label": "Replan", "guards": [{
                "evidence": {"scope": "local", "node": "work", "item": "verdict"},
                "equals": "failure",
            }]},
        ],
    });
    document["graph"]["entry"] = json!("prepare");
    let nodes = document["graph"]["nodes"].as_array_mut().unwrap();
    nodes[0]["next"] = json!("review");
    nodes[0]["delegation"] = json!({
        "binding_ref": "native", "role_ref": "worker", "role_snapshot_digest": package,
        "max_rounds": 1, "after_accept": "retain",
    });
    nodes.insert(
        0,
        json!({
            "id": "prepare", "use": "prepare", "next": "work",
            "skip": {"allowed": true, "reason_required": true},
        }),
    );
    nodes.insert(2, json!({
        "id": "review", "use": "review",
        "evidence_from": [
            {"scope": "local", "node": "work", "required": true, "items": ["verdict"]}
        ],
        "routes": {"continue": {"effect": "advance", "to": "done"}, "replan": {"effect": "replan"}},
    }));
    parse(&document.to_string(), ProcedureDocumentFormat::Json)
}

fn record_delegated_work(
    state: &mut ExecutionSessionV3,
    package: &podway_core::Sha256Digest,
    time: UnixMillis,
) {
    let target = state.active_attempt().unwrap().target().clone();
    let report = podway_core::ArtifactValueV1::local_path(
        "artifacts/retained.json",
        package.clone(),
        42,
        "application/json",
    )
    .unwrap();
    for (item, value) in [
        ("verdict", RecordedItemValueV2::choice("failure").unwrap()),
        ("confirmed", RecordedItemValueV2::confirm()),
        ("score", RecordedItemValueV2::integer(80)),
        (
            "findings",
            RecordedItemValueV2::list(vec!["First".to_owned(), "Second".to_owned()]).unwrap(),
        ),
        ("report", RecordedItemValueV2::artifact(report)),
    ] {
        state
            .record_item(
                &target,
                &ItemId::new(item).unwrap(),
                Revision::ZERO,
                Some(value),
                time,
            )
            .unwrap();
    }
    let binding = ScopedDelegationBindingV1::new(target, package.clone());
    state
        .accept_delegation(&state.fence(), binding, package, time)
        .unwrap();
}

#[test]
fn cold_reopen_retains_guarded_replan_cancellation_and_delegated_values() {
    let (mut state, mut sources) = fixture();
    let package = state.base_digest().clone();
    let definition = group_with_replan_and_delegation(&package);
    let SuccessorDefinition::Group(group) = definition.definition() else {
        panic!("fixture has wrong kind");
    };
    sources.insert(
        definition.digest().clone(),
        definition.canonical_json().as_str().to_owned(),
    );
    let time = UnixMillis::new(2);
    let anchor = state.base_definition().boundary().anchor().clone();
    let basis = CheckResultInputBasisV2::new("Retained candidate", package.clone()).unwrap();
    state
        .revise_goal(
            &state.fence(),
            ExecutionGoalChangeV3 {
                goal: goal("Exercise retained branches"),
                target: anchor.clone(),
                reactivate: false,
                feedback: GroupClosureFeedbackV1::new(
                    "Prepare another group".to_owned(),
                    basis.clone(),
                    "Retain earlier work".to_owned(),
                )
                .unwrap(),
            },
            attempt(7),
            time,
        )
        .unwrap();
    let activate = |state: &mut ExecutionSessionV3, sequence, entry| {
        record(
            state,
            "plan",
            "Retain exact current evidence",
            Revision::ZERO,
        );
        let prior = state
            .group_history()
            .last()
            .unwrap()
            .identity()
            .execution()
            .clone();
        let fence = state.fence();
        let identity = ActivationIdentityV1::new(ActivationIdentityInputV1 {
            execution: GroupExecutionIdV1::new(format!("00000000-0000-4000-8000-{sequence:012}"))
                .unwrap(),
            session: fence.session.clone(),
            base_digest: state.base_digest().clone(),
            definition_digest: definition.digest().clone(),
            anchor: anchor.clone(),
            goal_revision: fence.goal_revision,
            plan: state.plan_binding().unwrap(),
            predecessor: Some(prior.clone()),
            working_copy_basis: basis.clone(),
            prior_work: vec![
                PriorWorkDispositionV1::new(
                    prior,
                    None,
                    PriorWorkChoiceV1::Retain,
                    "Keep prior group".to_owned(),
                )
                .unwrap(),
            ],
        })
        .unwrap();
        state
            .activate(
                &fence,
                identity,
                group.clone(),
                definition.digest(),
                attempt(entry),
                time,
            )
            .unwrap();
    };
    let explanation = |text| ReasonV2::new(text).unwrap();
    activate(&mut state, 203, 8);
    state
        .skip(
            &state.fence(),
            Some(explanation("Prepared externally")),
            attempt(9),
            time,
        )
        .unwrap();
    let work = state.active_attempt().unwrap().target().node().clone();
    state
        .retry(
            &state.fence(),
            explanation("Retry preparation"),
            attempt(10),
            time,
        )
        .unwrap();
    record_delegated_work(&mut state, &package, time);
    state
        .complete(&state.fence(), Some(attempt(11)), time)
        .unwrap();
    state
        .rework(
            &state.fence(),
            &work,
            explanation("Repeat investigation"),
            attempt(12),
            time,
        )
        .unwrap();
    record_delegated_work(&mut state, &package, time);
    state
        .complete(&state.fence(), Some(attempt(13)), time)
        .unwrap();
    let decision = state.active_attempt().unwrap().target().clone();
    state
        .replan(
            &state.fence(),
            ReplanFeedbackV1::new(
                decision,
                OptionId::new("replan").unwrap(),
                "Failure guard matched".to_owned(),
                basis.clone(),
                "Preserve retained delegated records".to_owned(),
            )
            .unwrap(),
            attempt(14),
            time,
        )
        .unwrap();

    let root = fixture_dir();
    let db = root.path().join("fixture.sqlite3");
    let mut connection = create_candidate_fixture(&db).unwrap();
    persist(&mut connection, &state, &sources);
    drop(connection);
    let reopened = load(&mut reopen_candidate_fixture(&db).unwrap(), &state).unwrap();
    assert_eq!(reopened, state);
    let replan = reopened.group_history()[2].closure().unwrap();
    assert_eq!(replan.cause, SelectionClosureCauseV1::Replan);
    assert_eq!(
        reopened.attempt_history()[12]
            .decision()
            .unwrap()
            .guard_values
            .len(),
        1
    );
    let completed_work = &reopened.attempt_history()[11];
    assert_eq!(
        completed_work
            .slots()
            .iter()
            .filter(|slot| slot.value().is_some())
            .count(),
        5
    );
    assert!(
        completed_work
            .delegation()
            .unwrap()
            .applies_to(completed_work.target(), &package)
    );

    activate(&mut state, 204, 15);
    state
        .skip(
            &state.fence(),
            Some(explanation("Prepared externally")),
            attempt(16),
            time,
        )
        .unwrap();
    record_delegated_work(&mut state, &package, time);
    state
        .cancel(&state.fence(), explanation("Cancel selected group"), time)
        .unwrap();
    let cancelled_root = fixture_dir();
    let cancelled_db = cancelled_root.path().join("fixture.sqlite3");
    let mut cancelled_store = create_candidate_fixture(&cancelled_db).unwrap();
    persist(&mut cancelled_store, &state, &sources);
    drop(cancelled_store);
    let cancelled = load(
        &mut reopen_candidate_fixture(&cancelled_db).unwrap(),
        &state,
    )
    .unwrap();
    assert_eq!(cancelled, state);
    let group = &cancelled.group_history()[3];
    assert_eq!(group.outcome(), GroupExecutionOutcomeV1::TerminatedByCancel);
    assert_eq!(
        group.closure().unwrap().cause,
        SelectionClosureCauseV1::Cancel
    );
    assert!(cancelled.active_attempt().is_none());
}
