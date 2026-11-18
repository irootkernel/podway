//! Real-process verification of candidate history retention and explicit deletion.

use super::driver::{HistoryQuery as Query, HistoryRequest, Lifecycle, Location, Target};
use super::*;
use crate::int_group002_transactions::TIME;
use podway_core::execution_groups::{MAX_GROUP_EXECUTIONS_V1, ScopedAttemptV1};
use podway_core::{RecordedItemValueV2, Sha256Digest, UnixMillis, recorded_value_digest_v2};
use podway_engine::direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;

fn history(
    fixture: &mut Fixture,
    target: &Target,
    query: Query,
    limit: usize,
    continuation: Option<&str>,
) -> Reply {
    fixture.history(HistoryRequest {
        target: target.clone(),
        query,
        limit,
        continuation: continuation.map(str::to_owned),
    })
}

fn page(reply: Reply) -> Value {
    let Reply::History(response) = reply else {
        panic!("expected a bounded history response, received {reply:?}");
    };
    assert!(response.len() <= podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    serde_json::from_str(&response).unwrap()
}

fn assess(fixture: &mut Fixture, prefix: &str, memo: Option<&str>) {
    let mut edits = vec![
        Edit {
            item: "verdict".into(),
            expected: 0,
            value: ItemValue::Choice("failure".into()),
        },
        Edit {
            item: "stage".into(),
            expected: 0,
            value: ItemValue::Choice("scored".into()),
        },
        Edit {
            item: "score".into(),
            expected: 0,
            value: ItemValue::Number(80),
        },
    ];
    if let Some(memo) = memo {
        edits.push(text("memo", 0, memo));
    }
    write(fixture, &format!("{prefix}-assess"), Action::Batch(edits));
    write(fixture, &format!("{prefix}-decision"), Action::Complete);
    write(fixture, &format!("{prefix}-replan"), Action::Replan);
    write(
        fixture,
        &format!("{prefix}-plan"),
        Action::Record(text("plan", 0, "Next plan")),
    );
}

struct References {
    first: GroupExecutionIdV1,
    second: GroupExecutionIdV1,
    definition: Sha256Digest,
    source: ScopedAttemptV1,
}

impl References {
    fn read(&self, fixture: &mut Fixture, target: &Target) -> Vec<Value> {
        let item = ItemId::new("memo").unwrap();
        let queries = [
            Query::Group(self.first.clone()),
            Query::Dag(Some(self.first.clone()), self.definition.clone()),
            Query::Attempts(Some(self.first.clone()), self.definition.clone()),
            Query::Item(self.source.clone(), item),
            Query::PriorWork(self.second.clone(), self.definition.clone()),
            Query::Blockers(self.source.clone()),
            Query::GoalRevisions,
        ];
        queries
            .into_iter()
            .map(|query| {
                let mut response = page(history(fixture, target, query, 64, None));
                assert_eq!(response["workspace"], json!(target.workspace));
                assert_eq!(response["session"], json!(target.session));
                assert_eq!(response["session_revision"], json!(target.revision));
                let expected_location = match target.location {
                    Location::Current => "current",
                    Location::Archived => "archived",
                };
                assert_eq!(response["session_location"], expected_location);
                assert_eq!(response["truncated"], false);
                let fields = response.as_object_mut().unwrap();
                fields.remove("session_revision");
                fields.remove("session_location");
                response
            })
            .collect()
    }
}

fn launch_group(
    fixture: &mut Fixture,
    key: &str,
    group: &Value,
    prior_work: Vec<PriorWork>,
) -> Outcome {
    let probe = fixture.read(&[]);
    success(
        fixture.run(&activate(&probe, key, group, prior_work)),
        false,
    )
}

#[test]
fn cold_history_survives_ordinary_pruning_exhaustion_and_archive() {
    use Location::{Archived, Current};

    let mut document = local_decision_group(true);
    document["node_definitions"]["work"]["items"]
        .as_array_mut()
        .unwrap()
        .push(optional_text("memo"));
    let canonical_group = canonical(&document);
    let group_digest = canonical_group.digest();
    let mut fixture = Fixture::create(&base_document(), true);
    let original = launch_group(&mut fixture, "first-activate", &document, vec![]);
    let first = fixture.read(&[]);
    let first_group = execution(&first.groups[0]);
    let source = first.cursor.attempt.unwrap();
    let memo = "Retain the original work across replans and pruning.";
    let item_digest = recorded_value_digest_v2(&RecordedItemValueV2::text(memo).unwrap()).unwrap();
    assess(&mut fixture, "first", Some(memo));
    launch_group(
        &mut fixture,
        "second-activate",
        &document,
        vec![PriorWork {
            execution: first_group.clone(),
            item: Some(HistoricalReferenceV1::Item(HistoricalItemV1::new(
                source.attempt().clone(),
                ItemId::new("memo").unwrap(),
                item_digest.clone(),
            ))),
        }],
    );
    let second = fixture.read(&[]);
    let second_group = execution(&second.groups[1]);
    assess(&mut fixture, "second", None);
    let refs = References {
        first: first_group,
        second: second_group,
        definition: group_digest.clone(),
        source,
    };
    let early = fixture.read(&["first-activate"]);
    let early_target = fixture.target(Current, early.cursor.revision);
    let early_views = refs.read(&mut fixture, &early_target);
    assert_eq!(early_views[0]["group"]["closure"]["cause"], "replan");
    assert_eq!(early_views[0]["rows"][0]["choice"], "redesign");
    assert_eq!(
        early_views[0]["rows"][0]["assessment"][0]["items"][2]["scalar"],
        80
    );
    assert_eq!(
        early_views[1]["detail"]["definition_digest"],
        json!(group_digest)
    );
    let graph = podway_config::execution_groups::project_retained_successor_graph(&canonical_group)
        .unwrap();
    assert_eq!(early_views[1]["data"], graph.projection());
    assert_eq!(
        early_views[1]["detail"]["projection_digest"],
        json!(graph.digest())
    );
    assert_eq!(early_views[2]["total"], 2);
    assert_eq!(early_views[2]["rows"][0]["attempt"], json!(refs.source));
    assert_eq!(early_views[2]["rows"][0]["lifecycle"], "completed");
    assert_eq!(early_views[3]["data"], memo);
    assert_eq!(early_views[3]["detail"]["value_digest"], json!(item_digest));
    assert_eq!(
        early_views[4]["rows"][0]["source"]["attempt"],
        json!(refs.source)
    );
    assert_eq!(
        early_views[4]["rows"][0]["source"]["item"]["value_digest"],
        json!(item_digest)
    );
    let early_group_page = page(history(&mut fixture, &early_target, Query::Groups, 1, None));
    let early_token = early_group_page["next_page_token"].as_str().unwrap();

    // Each child uses bounded groups of genuine committed Engine operations. All others reopen.
    let remaining = MAX_GROUP_EXECUTIONS_V1 - early.groups.len();
    for count in (0..remaining)
        .step_by(8)
        .map(|start| (remaining - start).min(8))
    {
        fixture.grow(&document, group_digest, u8::try_from(count).unwrap());
    }
    let before_prune = fixture.inspect(&["first-activate"]);
    assert_eq!(before_prune.retained.2 as usize, MAX_GROUP_EXECUTIONS_V1);
    assert!(before_prune.ledger.2 > 200);
    assert_eq!(before_prune.first_journal_sequence, Some(original.sequence));
    let exhausted = fixture.read(&[]);
    assert_eq!(
        exhausted.observation.as_ref().unwrap()["remaining_activations"],
        0
    );
    let mut aged = Call::new(
        &exhausted,
        "age-pruning",
        Action::Record(text("plan", 1, "Current plan after pruning")),
    );
    aged.now = UnixMillis::new(TIME.get() + 31 * 24 * 60 * 60 * 1000);
    success(fixture.run(&aged), false);
    let aged = fixture.read(&["first-activate"]);
    let aged_target = fixture.target(Current, aged.cursor.revision);
    let complete_retention = fixture.inspect(&["first-activate", "one-over"]);
    assert_eq!(complete_retention.retained, before_prune.retained);
    assert_eq!(complete_retention.ledger.2, 200);
    assert!(complete_retention.first_journal_sequence.unwrap() > original.sequence);
    assert_eq!(complete_retention.ledger.1, before_prune.ledger.1 + 1);
    assert_eq!(
        complete_retention.outcomes["first-activate"].as_ref(),
        Some(&original)
    );
    let bytes = fixture.persistent_bytes();
    assert_eq!(refs.read(&mut fixture, &aged_target), early_views);
    rejection(
        history(
            &mut fixture,
            &aged_target,
            Query::Groups,
            1,
            Some(early_token),
        ),
        Refusal::StaleHistoryToken,
    );
    let front = page(history(&mut fixture, &aged_target, Query::Groups, 17, None));
    assert_eq!(front["total"], MAX_GROUP_EXECUTIONS_V1);
    assert_eq!(front["truncated"], true);
    let mut rows = front["rows"].as_array().unwrap().clone();
    let mut token = front["next_page_token"].as_str().map(str::to_owned);
    while let Some(next) = token {
        let tail = page(history(
            &mut fixture,
            &aged_target,
            Query::Groups,
            17,
            Some(&next),
        ));
        assert_eq!(tail["total"], MAX_GROUP_EXECUTIONS_V1);
        rows.extend_from_slice(tail["rows"].as_array().unwrap());
        token = tail["next_page_token"].as_str().map(str::to_owned);
        assert_eq!(tail["truncated"], token.is_some());
    }
    assert_eq!(rows.len(), MAX_GROUP_EXECUTIONS_V1);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row["activation_sequence"], index + 1);
        assert_eq!(row["execution"], aged.groups[index]["id"]);
    }
    assert_eq!(rows[0], early_group_page["rows"][0]);
    rejection(
        fixture.run(&activate(
            &aged,
            "one-over",
            &document,
            vec![retain(&aged.groups[0], None)],
        )),
        Refusal::Bound {
            field: "group executions".into(),
            actual: (MAX_GROUP_EXECUTIONS_V1 + 1) as u64,
            maximum: MAX_GROUP_EXECUTIONS_V1 as u64,
        },
    );
    rejection(
        fixture.lifecycle(Lifecycle::Archive(aged_target)),
        Refusal::ArchivePhase,
    );
    assert_eq!(
        fixture.inspect(&["first-activate", "one-over"]),
        complete_retention
    );
    assert_eq!(fixture.persistent_bytes(), bytes);

    write(&mut fixture, "cancel-exhausted", Action::Cancel);
    let terminal = fixture.read(&["cancel-exhausted"]);
    let terminal_target = fixture.target(Current, terminal.cursor.revision);
    let terminal_page = page(history(
        &mut fixture,
        &terminal_target,
        Query::Groups,
        1,
        None,
    ));
    let before_archive = fixture.inspect(&["first-activate", "cancel-exhausted", "one-over"]);
    let stale = Target {
        revision: Revision::ZERO,
        ..terminal_target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Archive(stale)),
        Refusal::Revision {
            expected: Revision::ZERO,
            actual: terminal.cursor.revision,
        },
    );
    assert_eq!(
        fixture.inspect(&["first-activate", "cancel-exhausted", "one-over"]),
        before_archive
    );
    assert!(matches!(
        fixture.lifecycle(Lifecycle::Archive(terminal_target.clone())),
        Reply::Lifecycle
    ));
    let archived = Target {
        location: Archived,
        ..terminal_target.clone()
    };
    let after_archive = fixture.inspect(&["first-activate", "cancel-exhausted", "one-over"]);
    assert_eq!(after_archive.ledger, before_archive.ledger);
    assert_eq!(after_archive.retained, before_archive.retained);
    assert_eq!(after_archive.outcomes, before_archive.outcomes);
    assert_ne!(after_archive.fingerprint, before_archive.fingerprint);
    let bytes = fixture.persistent_bytes();
    assert_eq!(refs.read(&mut fixture, &archived), early_views);
    let archived_groups = page(history(&mut fixture, &archived, Query::Groups, 64, None));
    assert_eq!(archived_groups["rows"], json!(rows));
    rejection(
        history(
            &mut fixture,
            &archived,
            Query::Groups,
            1,
            terminal_page["next_page_token"].as_str(),
        ),
        Refusal::StaleHistoryToken,
    );
    rejection(
        history(&mut fixture, &terminal_target, Query::Groups, 1, None),
        Refusal::ArchivedSession,
    );
    assert_eq!(
        fixture.inspect(&["first-activate", "cancel-exhausted", "one-over"]),
        after_archive
    );
    assert_eq!(fixture.persistent_bytes(), bytes);
    fixture.finish();
}

fn archived_goal_history(fixture: &mut Fixture, key: &str, group: &Value) -> (Outcome, Target) {
    write(
        fixture,
        &format!("{key}-goal"),
        Action::DefineGoal("Initial goal".into()),
    );
    let outcome = launch_group(fixture, key, group, vec![]);
    write(
        fixture,
        &format!("{key}-revise"),
        Action::ReviseGoal {
            title: "Revised goal".into(),
            reactivate: false,
        },
    );
    write(fixture, &format!("{key}-cancel"), Action::Cancel);
    let state = fixture.read(&[]);
    let current = fixture.target(Location::Current, state.cursor.revision);
    assert!(matches!(
        fixture.lifecycle(Lifecycle::Archive(current.clone())),
        Reply::Lifecycle
    ));
    (
        outcome,
        Target {
            location: Location::Archived,
            ..current
        },
    )
}

fn definition_digests(fixture: &Fixture) -> BTreeSet<Sha256Digest> {
    // Read only immutable candidate definitions in this owned fixture.
    let db = Connection::open_with_flags(
        fixture.root().join("candidate.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    db.prepare("SELECT digest FROM successor_definitions")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|digest| Sha256Digest::new(digest.unwrap()).unwrap())
        .collect()
}

#[test]
fn explicit_fixture_deletion_fences_history_and_outcomes_separately() {
    use Location::{Archived, Current};
    let base = base_document();
    let mut tracked = tracked_base();
    tracked["graph"]["entry"] = json!("design");
    tracked["graph"]["nodes"].as_array_mut().unwrap().remove(0);
    tracked["node_definitions"]
        .as_object_mut()
        .unwrap()
        .remove("prefix");
    tracked["manual_rework"]["allowed_targets"] = json!(["review"]);
    let group = group_document();
    let mut target_only = group.clone();
    target_only["id"] = json!("purge-target-only");
    let target_digest = canonical(&target_only).digest().clone();
    let shared_digest = canonical(&group).digest().clone();
    let base_digest = canonical(&base).digest().clone();
    let tracked_digest = canonical(&tracked).digest().clone();
    let mut fixture = Fixture::create(&tracked, true);
    let (original, archived_target) =
        archived_goal_history(&mut fixture, "archived-outcome", &target_only);
    let archived_current = Target {
        location: Current,
        ..archived_target.clone()
    };
    let original_goals = page(history(
        &mut fixture,
        &archived_target,
        Query::GoalRevisions,
        64,
        None,
    ));
    assert_eq!(original_goals["total"], 1);
    assert_eq!(original_goals["rows"][0]["goal_revision"], 2);
    let original_group = page(history(
        &mut fixture,
        &archived_target,
        Query::Groups,
        64,
        None,
    ));
    assert_eq!(
        original_group["rows"][0]["definition_digest"],
        json!(target_digest)
    );
    assert_eq!(
        original_goals["rows"][0]["predecessor"],
        original_group["rows"][0]["execution"]
    );
    assert!(matches!(
        fixture.lifecycle(Lifecycle::Install(tracked, None)),
        Reply::Installed { .. }
    ));
    let (sibling, sibling_target) = archived_goal_history(&mut fixture, "sibling-outcome", &group);
    let sibling_goals = page(history(
        &mut fixture,
        &sibling_target,
        Query::GoalRevisions,
        64,
        None,
    ));
    assert_eq!(sibling_goals["total"], 1);
    assert_eq!(sibling_goals["rows"][0]["goal_revision"], 2);
    let sibling_history = page(history(
        &mut fixture,
        &sibling_target,
        Query::Groups,
        64,
        None,
    ))["rows"]
        .clone();
    assert_eq!(
        sibling_goals["rows"][0]["predecessor"],
        sibling_history[0]["execution"]
    );
    assert!(matches!(
        fixture.lifecycle(Lifecycle::Install(base.clone(), None)),
        Reply::Installed { .. }
    ));
    let current = launch_group(&mut fixture, "current-outcome", &group, vec![]);
    let active = fixture.read(&[]);
    let current_target = fixture.target(Current, active.cursor.revision);
    assert_eq!(
        definition_digests(&fixture),
        BTreeSet::from([
            target_digest,
            shared_digest.clone(),
            base_digest.clone(),
            tracked_digest.clone()
        ]),
    );
    // Close a normal child before capturing the quiescent DB/WAL baseline.
    let stable = fixture.inspect(&["archived-outcome", "current-outcome"]);
    assert_eq!(stable.retained, (4, 3, 3, 8, 2));
    let bytes = fixture.persistent_bytes();

    let wrong_workspace = Target {
        workspace: NativeDirectIdSourceV1.next_workspace_id(),
        ..archived_target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Remove(wrong_workspace)),
        Refusal::Workspace,
    );
    let wrong_session = Target {
        session: NativeDirectIdSourceV1.next_session_id(),
        ..archived_target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Remove(wrong_session)),
        Refusal::MissingSession,
    );
    let stale = Target {
        revision: Revision::ZERO,
        ..archived_target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Remove(stale)),
        Refusal::Revision {
            expected: Revision::ZERO,
            actual: archived_target.revision,
        },
    );
    rejection(
        fixture.lifecycle(Lifecycle::Remove(archived_current)),
        Refusal::ArchivedSession,
    );
    let current_as_archived = Target {
        location: Archived,
        ..current_target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Remove(current_as_archived)),
        Refusal::MissingSession,
    );
    rejection(
        fixture.lifecycle(Lifecycle::Remove(current_target.clone())),
        Refusal::Domain("candidate session removal requires a terminal session".into()),
    );
    let identity_in_use = || {
        Refusal::Domain(
            "candidate current slot or retained installation identity is already present".into(),
        )
    };
    rejection(
        fixture.lifecycle(Lifecycle::Install(base.clone(), None)),
        identity_in_use(),
    );
    assert_eq!(
        fixture.inspect(&["archived-outcome", "current-outcome"]),
        stable
    );
    assert_eq!(fixture.persistent_bytes(), bytes);

    assert!(matches!(
        fixture.lifecycle(Lifecycle::Remove(archived_target.clone())),
        Reply::Lifecycle
    ));
    let purged = fixture.inspect(&["archived-outcome", "current-outcome"]);
    assert_eq!(purged.ledger, stable.ledger);
    assert_eq!(purged.retained, (3, 2, 2, 5, 1));
    assert_eq!(
        definition_digests(&fixture),
        BTreeSet::from([shared_digest.clone(), base_digest, tracked_digest.clone()]),
    );
    assert_eq!(
        purged.outcomes["archived-outcome"].as_ref(),
        Some(&original)
    );
    assert_eq!(purged.outcomes["current-outcome"].as_ref(), Some(&current));
    rejection(
        history(&mut fixture, &archived_target, Query::Groups, 64, None),
        Refusal::MissingSession,
    );
    rejection(
        history(
            &mut fixture,
            &archived_target,
            Query::GoalRevisions,
            64,
            None,
        ),
        Refusal::MissingSession,
    );
    assert_eq!(
        page(history(
            &mut fixture,
            &sibling_target,
            Query::GoalRevisions,
            64,
            None
        )),
        sibling_goals,
    );
    assert_eq!(
        page(history(
            &mut fixture,
            &current_target,
            Query::Groups,
            64,
            None
        ))["total"],
        1
    );

    write(&mut fixture, "cancel-current", Action::Cancel);
    let current = fixture.read(&[]);
    let current_target = fixture.target(Current, current.cursor.revision);
    let stale = Target {
        revision: Revision::ZERO,
        ..current_target.clone()
    };
    let stable = fixture.inspect(&["archived-outcome", "current-outcome", "cancel-current"]);
    let bytes = fixture.persistent_bytes();
    rejection(
        fixture.lifecycle(Lifecycle::Remove(stale)),
        Refusal::Revision {
            expected: Revision::ZERO,
            actual: current_target.revision,
        },
    );
    assert_eq!(
        fixture.inspect(&["archived-outcome", "current-outcome", "cancel-current"]),
        stable
    );
    assert_eq!(fixture.persistent_bytes(), bytes);
    assert!(matches!(
        fixture.lifecycle(Lifecycle::Remove(current_target.clone())),
        Reply::Lifecycle
    ));
    let keys = [
        "archived-outcome",
        "current-outcome",
        "cancel-current",
        "sibling-outcome",
    ];
    let removed = fixture.inspect(&keys);
    assert_eq!(removed.retained, (2, 1, 1, 3, 1));
    assert_eq!(
        definition_digests(&fixture),
        BTreeSet::from([shared_digest, tracked_digest])
    );
    assert_eq!(removed.ledger.0, stable.ledger.0);
    assert_eq!(removed.ledger.2, stable.ledger.2);
    assert_eq!(
        removed.outcomes["archived-outcome"].as_ref(),
        Some(&original)
    );
    assert!(removed.outcomes["current-outcome"].is_none());
    assert!(removed.outcomes["cancel-current"].is_none());
    assert_eq!(removed.outcomes["sibling-outcome"].as_ref(), Some(&sibling));
    assert_eq!(
        page(history(
            &mut fixture,
            &sibling_target,
            Query::Groups,
            64,
            None
        ))["rows"],
        sibling_history
    );
    assert_eq!(
        page(history(
            &mut fixture,
            &sibling_target,
            Query::GoalRevisions,
            64,
            None
        )),
        sibling_goals,
    );
    rejection(
        history(&mut fixture, &current_target, Query::Groups, 64, None),
        Refusal::MissingSession,
    );
    let bytes = fixture.persistent_bytes();
    rejection(
        fixture.lifecycle(Lifecycle::Install(
            base.clone(),
            Some(sibling_target.session.clone()),
        )),
        identity_in_use(),
    );
    rejection(
        fixture.lifecycle(Lifecycle::Install(
            base.clone(),
            Some(archived_target.session),
        )),
        identity_in_use(),
    );
    assert_eq!(fixture.inspect(&keys), removed);
    assert_eq!(fixture.persistent_bytes(), bytes);

    assert!(matches!(
        fixture.lifecycle(Lifecycle::Install(base.clone(), None)),
        Reply::Installed { .. }
    ));
    launch_group(&mut fixture, "workspace-outcome", &group, vec![]);
    let runtime = fixture.read(&[]);
    let target = fixture.target(Current, runtime.cursor.revision);
    let stale = Target {
        revision: Revision::ZERO,
        ..target.clone()
    };
    let stable = fixture.inspect(&["workspace-outcome", "archived-outcome"]);
    let bytes = fixture.persistent_bytes();
    rejection(
        fixture.lifecycle(Lifecycle::Reset(stale, base.clone())),
        Refusal::Revision {
            expected: Revision::ZERO,
            actual: target.revision,
        },
    );
    let wrong_workspace = Target {
        workspace: NativeDirectIdSourceV1.next_workspace_id(),
        ..target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Reset(wrong_workspace, base.clone())),
        Refusal::Workspace,
    );
    let wrong_session = Target {
        session: NativeDirectIdSourceV1.next_session_id(),
        ..target.clone()
    };
    rejection(
        fixture.lifecycle(Lifecycle::Reset(wrong_session, base.clone())),
        Refusal::MissingSession,
    );
    rejection(
        fixture.lifecycle(Lifecycle::Reset(sibling_target.clone(), base.clone())),
        Refusal::Domain(
            "candidate workspace reset requires a current target and fresh identities".into(),
        ),
    );
    assert_eq!(
        fixture.inspect(&["workspace-outcome", "archived-outcome"]),
        stable
    );
    assert_eq!(fixture.persistent_bytes(), bytes);
    let git_head = std::fs::read(fixture.root().join(".git").join("HEAD")).unwrap();
    let work = std::fs::read(fixture.root().join("work.txt")).unwrap();
    let Reply::Installed { workspace, session } =
        fixture.lifecycle(Lifecycle::Reset(target.clone(), base))
    else {
        panic!("expected explicit workspace reset");
    };
    assert_ne!(workspace, target.workspace);
    assert_ne!(session, target.session);
    let reset = fixture.inspect(&["workspace-outcome", "archived-outcome", "sibling-outcome"]);
    assert_eq!(reset.retained, (1, 1, 0, 1, 0));
    assert_eq!(reset.ledger, (0, 0, 0));
    assert!(reset.outcomes.values().all(Option::is_none));
    rejection(
        history(&mut fixture, &target, Query::Groups, 64, None),
        Refusal::Workspace,
    );
    let new_target = Target {
        workspace,
        ..target
    };
    rejection(
        history(&mut fixture, &new_target, Query::Groups, 64, None),
        Refusal::MissingSession,
    );
    let fresh = fixture.read(&[]);
    assert!(fresh.groups.is_empty());
    assert_eq!(
        std::fs::read(fixture.root().join(".git").join("HEAD")).unwrap(),
        git_head
    );
    assert_eq!(
        std::fs::read(fixture.root().join("work.txt")).unwrap(),
        work
    );
    fixture.finish();
}
