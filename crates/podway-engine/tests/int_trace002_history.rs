//! Historical DAG, attempt, and value reads through the admitted candidate Store.

use super::int_group002_transactions::{
    Fixture, TIME, activation, base_document, canonical, definition, group_document, mutation,
};
use super::int_group003_replan::{assessment, complete, feedback, local_decision_group};
use podway_config::execution_groups::{project_retained_successor_graph, project_successor_graph};
use podway_core::execution_groups::{
    ExecutionScopeV1, GroupExecutionIdV1, GroupItemWriteV1, PriorWorkChoiceV1,
    PriorWorkDispositionV1, ScopedAttemptV1, ScopedDelegationBindingV1,
};
use podway_core::{
    ArtifactValueV1, BlockerId, BlockerState, BlockerStateV2, CheckResultExecutorV2,
    CheckResultInputBasisV2, CheckResultOutcomeV2, CheckResultValueV2, DomainError, GraphNodeId,
    ItemId, MAX_EVIDENCE_PAGE_BYTES_V2, MAX_EVIDENCE_RESPONSE_BYTES_V2, MAX_LIST_ENTRIES_V2,
    MAX_LIST_ENTRY_SCALARS_V2, MAX_LIST_TOTAL_SCALARS_V2, MAX_OPEN_BLOCKERS_V2,
    MAX_TEXT_SCALARS_V2, OperationId, ReasonV2, RecordedItemValueV2, Revision, UnixMillis,
    recorded_value_digest_v2, workflow_v2::item_value_json_v2,
};
use podway_engine::direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1};
use podway_engine::successor_execution::*;
use podway_store::CanonicalExecutionJsonV1;
use podway_store::direct_store::DirectStoreErrorV1;
use podway_store::successor_store::CandidateStoreError;
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn item_page(
    fixture: &mut Fixture,
    location: CandidateSessionLocation,
    attempt: &ScopedAttemptV1,
    item: &str,
    continuation: Option<&str>,
) -> Result<Value, SuccessorExecutionError> {
    let workspace = fixture.workspace.clone();
    let session = fixture.session.clone();
    let target = SuccessorHistorySession {
        workspace: &workspace,
        session: &session,
        location,
    };
    read_successor_history_item(
        &mut fixture.store,
        target,
        attempt,
        &ItemId::new(item).unwrap(),
        continuation,
    )
    .map(content)
}

fn content(page: CanonicalExecutionJsonV1) -> Value {
    assert!(page.as_str().len() <= MAX_EVIDENCE_RESPONSE_BYTES_V2);
    let parsed: Value = serde_json::from_str(page.as_str()).unwrap();
    assert!(serde_json::to_vec(&parsed["data"]).unwrap().len() <= MAX_EVIDENCE_PAGE_BYTES_V2);
    parsed
}

fn history_rows(page: CanonicalExecutionJsonV1) -> Value {
    assert!(page.as_str().len() <= podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    serde_json::from_str(page.as_str()).unwrap()
}

fn page(limit: usize, continuation: Option<&str>) -> SuccessorHistoryPage<'_> {
    SuccessorHistoryPage {
        limit,
        continuation,
    }
}

fn at_offset(token: &str, offset: usize) -> String {
    format!("{}:{offset:016x}", token.rsplit_once(':').unwrap().0)
}

fn logical_store(fixture: &Fixture) -> Vec<(String, Vec<Vec<SqlValue>>)> {
    let db = Connection::open_with_flags(&fixture.file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    tables
        .into_iter()
        .map(|table| {
            let mut query = db
                .prepare(&format!(
                    "SELECT * FROM \"{}\" ORDER BY rowid",
                    table.replace('"', "\"\"")
                ))
                .unwrap();
            let columns = query.column_count();
            let rows = query
                .query_map([], |row| {
                    (0..columns).map(|column| row.get(column)).collect()
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            (table, rows)
        })
        .collect()
}

#[test]
fn local_rework_preserves_exact_attempt_provenance_and_pageable_blockers() {
    use CandidateSessionLocation::{Archived, Current};

    let mut document = group_document();
    document["manual_rework"] = json!({"allowed_targets": ["work"]});
    document["node_definitions"]["work"]["items"] = json!([
        {"id":"memo", "type":"text", "prompt":"Record the work.", "required":false},
        {"id":"unset", "type":"text", "prompt":"Leave an empty slot.", "required":false}
    ]);
    document["graph"]["nodes"][0]["delegation"] = json!({
        "binding_ref":"native", "role_ref":"worker",
        "role_snapshot_digest":format!("sha256:{}", "9".repeat(64)),
        "max_rounds":1, "after_accept":"retain",
    });
    let group = canonical(&document);
    let mut fixture = Fixture::new(&base_document(), true);
    let (design, _) = fixture.state();
    let later = |offset| UnixMillis::new(TIME.get() + offset);
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "activate"),
        || later(1),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (mut work, _) = fixture.state();
    let source = work.active_attempt().unwrap().target().clone();
    let item = ItemId::new("memo").unwrap();
    let value = RecordedItemValueV2::text("Retain this original work.").unwrap();
    for (revision, stored) in [
        (0, RecordedItemValueV2::text("First draft.").unwrap()),
        (1, value.clone()),
    ] {
        let request = mutation(&fixture.workspace, &work, &format!("record-{revision}"));
        record_successor_many(
            &mut fixture.store,
            request,
            vec![GroupItemWriteV1 {
                item: item.clone(),
                expected_revision: Revision::new(revision),
                value: Some(stored),
            }],
            || later(2 + revision),
        )
        .unwrap();
        work = fixture.state().0;
    }
    let request = mutation(&fixture.workspace, &work, "accept-delegation");
    let package =
        current_delegation_package(&mut fixture.store, &fixture.workspace, &request.cursor)
            .unwrap();
    let original_delegation =
        ScopedDelegationBindingV1::new(source.clone(), package.package_digest().clone());
    accept_successor_delegation(&mut fixture.store, request, package, || later(4)).unwrap();
    let (accepted, _) = fixture.state();
    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &accepted, "complete-work"),
        SuccessorCursorMutation::Complete,
        || later(5),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (mut finishing, sources) = fixture.state();
    let abandoned = finishing.active_attempt().unwrap().target().clone();
    let blocker = |ordinal: usize, resolved: bool| {
        BlockerStateV2::new(
            BlockerId::new(format!("00000000-0000-4000-8000-{ordinal:012}")).unwrap(),
            abandoned.attempt().clone(),
            format!("Blocker {ordinal}"),
            if resolved {
                BlockerState::Resolved
            } else {
                BlockerState::Open
            },
            if ordinal > MAX_OPEN_BLOCKERS_V2 {
                later(8)
            } else {
                later(6)
            },
            resolved.then(|| later(7)),
        )
        .unwrap()
    };
    finishing
        .replace_blockers(
            &finishing.fence(),
            (1..=MAX_OPEN_BLOCKERS_V2)
                .map(|i| blocker(i, false))
                .collect(),
            later(6),
        )
        .unwrap();
    let mut blockers: Vec<_> = (1..=MAX_OPEN_BLOCKERS_V2)
        .map(|i| blocker(i, true))
        .collect();
    finishing
        .replace_blockers(&finishing.fence(), blockers.clone(), later(7))
        .unwrap();
    blockers.push(blocker(MAX_OPEN_BLOCKERS_V2 + 1, false));
    finishing
        .replace_blockers(&finishing.fence(), blockers.clone(), later(8))
        .unwrap();
    fixture.edit_fixture(&finishing, &sources);
    let reason = "Repeat the delegated work.";
    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &finishing, "rework"),
        SuccessorCursorMutation::Rework {
            target: GraphNodeId::new("work").unwrap(),
            reason: ReasonV2::new(reason).unwrap(),
        },
        || later(9),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (reworked, _) = fixture.state();
    let fresh = reworked.active_attempt().unwrap().target().clone();
    assert_ne!(fresh, source);
    let workspace = fixture.workspace.clone();
    let session = fixture.session.clone();
    let target = SuccessorHistorySession {
        workspace: &workspace,
        session: &session,
        location: Current,
    };
    let execution = reworked.selected().unwrap().identity().execution();
    let definition_scope = SuccessorHistoryDefinition::Group {
        execution,
        digest: group.digest(),
    };
    let before_reads = logical_store(&fixture);
    let attempts = history_rows(
        list_successor_history_attempts(
            &mut fixture.store,
            target,
            definition_scope,
            page(64, None),
        )
        .unwrap(),
    );
    let rows = attempts["rows"].as_array().unwrap();
    assert_eq!(attempts["total"], 3);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        json!({
            "attempt": source, "trace_sequence": 2, "lifecycle": "completed", "validity": "stale",
            "opened_at": later(1), "closed_at": later(5), "actor": null, "reason": null, "delegation": original_delegation,
            "decision_entry": null, "selected_group": null, "decision": null, "blocker_count": 0,
            "items": [
                {"item":"memo", "item_type":"text", "revision":2, "recorded":true},
                {"item":"unset", "item_type":"text", "revision":0, "recorded":false}
            ],
        })
    );
    assert_eq!(rows[1]["trace_sequence"], 3);
    assert_eq!(rows[1]["attempt"], json!(abandoned));
    assert_eq!(rows[1]["lifecycle"], "abandoned");
    assert_eq!(rows[1]["validity"], "stale");
    assert_eq!(rows[1]["opened_at"], json!(later(5)));
    assert_eq!(rows[1]["closed_at"], json!(later(9)));
    assert_eq!(rows[1]["reason"], reason);
    assert_eq!(rows[1]["blocker_count"], blockers.len());
    assert_eq!(rows[2]["trace_sequence"], 4);
    assert_eq!(rows[2]["attempt"], json!(fresh));
    assert_eq!(rows[2]["lifecycle"], "active");
    assert_eq!(rows[2]["validity"], "valid");
    assert_eq!(rows[2]["opened_at"], json!(later(9)));
    assert_eq!(rows[2]["closed_at"], Value::Null);
    assert_eq!(rows[2]["delegation"], Value::Null);
    assert_eq!(
        rows[2]["items"][0],
        json!({"item":"memo", "item_type":"text", "revision":0, "recorded":false})
    );
    let original_item = item_page(&mut fixture, Current, &source, "memo", None).unwrap();
    assert_eq!(original_item["data"], value.as_text().unwrap());
    assert_eq!(original_item["detail"]["item_revision"], 2);
    assert_eq!(
        original_item["detail"]["value_digest"],
        json!(recorded_value_digest_v2(&value).unwrap())
    );
    assert_eq!(
        original_item["detail"]["source_attempt"]["attempt"],
        json!(source)
    );
    assert_eq!(
        original_item["detail"]["source_attempt"]["validity"],
        "stale"
    );
    assert_eq!(
        original_item["detail"]["source_attempt"]["delegation"],
        json!(original_delegation)
    );
    assert_eq!(
        item_page(&mut fixture, Current, &fresh, "memo", None).unwrap()["detail"]["recorded"],
        false
    );

    let expected: Vec<_> = blockers.iter().map(|blocker| json!({
        "blocker_id": blocker.blocker_id(), "attempt_id": abandoned.attempt(), "reason": blocker.reason(),
        "created_at": blocker.created_at(), "resolved_at": blocker.resolved_at(),
        "state": if blocker.state() == BlockerState::Resolved { "resolved" } else { "open" },
    })).collect();
    let read_blockers = |fixture: &mut Fixture, target| {
        let first = history_rows(
            list_successor_history_blockers(&mut fixture.store, target, &abandoned, page(64, None))
                .unwrap(),
        );
        assert_eq!(first["attempt"], json!(abandoned));
        assert_eq!(first["total"], expected.len());
        assert_eq!(first["truncated"], true);
        assert_eq!(first["rows"], json!(&expected[..64]));
        let continuation = first["next_page_token"].as_str();
        assert!(continuation.is_some());
        assert!(matches!(
            list_successor_history_blockers(
                &mut fixture.store,
                target,
                &source,
                page(64, continuation)
            ),
            Err(SuccessorExecutionError::StaleHistoryPageToken)
        ));
        let second = history_rows(
            list_successor_history_blockers(
                &mut fixture.store,
                target,
                &abandoned,
                page(64, continuation),
            )
            .unwrap(),
        );
        assert_eq!(second["total"], expected.len());
        assert_eq!(second["rows"], json!(&expected[64..]));
        assert_eq!(second["truncated"], false);
        assert_eq!(second["next_page_token"], Value::Null);
    };
    read_blockers(&mut fixture, target);
    assert_eq!(logical_store(&fixture), before_reads);

    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &reworked, "cancel"),
        SuccessorCursorMutation::Cancel(ReasonV2::new("Finish the history fixture.").unwrap()),
        || later(10),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let terminal = fixture.state().0.fence().revision;
    fixture
        .store
        .archive_fixture_session(&fixture.workspace, &fixture.session, terminal, definition)
        .unwrap();
    fixture.reopen();
    let archived = SuccessorHistorySession {
        location: Archived,
        ..target
    };
    let before_reads = logical_store(&fixture);
    read_blockers(&mut fixture, archived);
    let archived_attempts = history_rows(
        list_successor_history_attempts(
            &mut fixture.store,
            archived,
            definition_scope,
            page(64, None),
        )
        .unwrap(),
    );
    assert_eq!(archived_attempts["rows"][0], rows[0]);
    assert_eq!(archived_attempts["rows"][1], rows[1]);
    let archived_item = item_page(&mut fixture, Archived, &source, "memo", None).unwrap();
    assert_eq!(archived_item["detail"], original_item["detail"]);
    assert_eq!(archived_item["data"], original_item["data"]);
    assert_eq!(logical_store(&fixture), before_reads);
}

fn full_text(first: Value, mut continuation: impl FnMut(&str) -> Value) -> String {
    let mut page = first;
    let total = page["total"].as_u64().unwrap();
    let mut offset = 0;
    let mut output = String::new();
    loop {
        assert_eq!(page["offset"], offset);
        let chunk = page["data"].as_str().unwrap();
        let size = chunk.chars().count() as u64;
        assert_eq!(page["size"], size);
        output.push_str(chunk);
        offset += size;
        match page["next_page_token"].as_str() {
            Some(token) => {
                assert!(size > 0 && offset < total);
                assert_eq!(page["truncated"], true);
                let previous_bytes = serde_json::to_vec(chunk).unwrap().len();
                page = continuation(token);
                assert_eq!(page["total"], total);
                let first_scalar = page["data"].as_str().unwrap().chars().next().unwrap();
                let scalar_bytes = serde_json::to_vec(&first_scalar).unwrap().len() - 2;
                assert!(previous_bytes + scalar_bytes > MAX_EVIDENCE_PAGE_BYTES_V2);
            }
            None => {
                assert_eq!(offset, total);
                assert_eq!(page["truncated"], false);
                return output;
            }
        }
    }
}

fn dense_graph() -> Value {
    let mut group = group_document();
    let options = (0..8)
        .map(|index| format!("go-{index}"))
        .collect::<Vec<_>>();
    group["node_definitions"]["route"] = json!({
        "type":"decision", "title":"Choose the next node", "objective":"Exercise the whole graph.",
        "prompt":"Choose the next route.", "reason":{"required":true},
        "options":options.iter().map(|id| json!({"id":id,"label":id})).collect::<Vec<_>>(),
    });
    let ids = (0..1022)
        .map(|index| format!("decision-{index:04}-{}", "x".repeat(45)))
        .collect::<Vec<_>>();
    let nodes = group["graph"]["nodes"].as_array_mut().unwrap();
    nodes[0]["next"] = json!(ids[0]);
    for (index, id) in ids.iter().enumerate() {
        let next = ids.get(index + 1).map_or("finish", String::as_str);
        let routes = options
            .iter()
            .map(|option| (option.clone(), json!({"effect":"advance","to":next})))
            .collect::<serde_json::Map<_, _>>();
        nodes.push(json!({"id":id,"use":"route","routes":routes}));
    }
    group
}

#[test]
fn admitted_dag_above_the_authoring_response_bound_is_fully_pageable() {
    let definition = canonical(&dense_graph());
    let projected = project_retained_successor_graph(&definition).unwrap();
    assert!(project_successor_graph(&definition).is_err());
    assert!(projected.projection().len() > podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1);
    let mut fixture = Fixture::new(&base_document(), true);
    let (design, _) = fixture.state();
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &definition, "dense-dag"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (running, _) = fixture.state();
    let execution = running.selected().unwrap().identity().execution();
    let workspace = fixture.workspace.clone();
    let session = fixture.session.clone();
    let target = SuccessorHistorySession {
        workspace: &workspace,
        session: &session,
        location: CandidateSessionLocation::Current,
    };
    let group = SuccessorHistoryDefinition::Group {
        execution,
        digest: definition.digest(),
    };
    let before_reads = logical_store(&fixture);
    let first =
        content(read_successor_history_dag(&mut fixture.store, target, group, None).unwrap());
    assert_eq!(
        first["detail"]["definition_digest"],
        json!(definition.digest())
    );
    assert_eq!(
        first["detail"]["projection_digest"],
        json!(projected.digest())
    );
    let token = first["next_page_token"].as_str().unwrap();
    for offset in [0, first["total"].as_u64().unwrap() as usize + 1] {
        assert!(matches!(
            read_successor_history_dag(
                &mut fixture.store,
                target,
                group,
                Some(&at_offset(token, offset))
            ),
            Err(SuccessorExecutionError::InvalidHistoryPageToken)
        ));
    }
    assert!(matches!(
        read_successor_history_dag(
            &mut fixture.store,
            target,
            SuccessorHistoryDefinition::Base(running.base_digest()),
            Some(token)
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    assert!(matches!(
        read_successor_history_dag(
            &mut fixture.store,
            target,
            SuccessorHistoryDefinition::Group {
                execution,
                digest: running.base_digest()
            },
            None
        ),
        Err(SuccessorExecutionError::GroupDefinitionMismatch)
    ));
    let dag = full_text(first, |token| {
        content(read_successor_history_dag(&mut fixture.store, target, group, Some(token)).unwrap())
    });
    assert_eq!(dag, projected.projection());
    assert_eq!(
        format!("sha256:{:x}", Sha256::digest(dag.as_bytes())),
        projected.digest().as_str()
    );
    let graph: Value = serde_json::from_str(&dag).unwrap();
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 1024);
    assert_eq!(graph["edges"].as_array().unwrap().len(), 1022 * 8 + 1);
    assert_eq!(logical_store(&fixture), before_reads);
}

fn value_group(operation_digest: &podway_core::Sha256Digest) -> Value {
    let mut group = local_decision_group(true);
    group["node_definitions"]["work"]["items"].as_array_mut().unwrap().extend([
        json!({"id":"memo","type":"text","prompt":"Record a long memo.","required":false,
            "min_length":0,"max_length":MAX_TEXT_SCALARS_V2}),
        json!({"id":"empty","type":"text","prompt":"Record an empty memo.","required":false,
            "min_length":0,"max_length":MAX_TEXT_SCALARS_V2}),
        json!({"id":"unset","type":"text","prompt":"Leave a slot empty.","required":false}),
        json!({"id":"notes","type":"list","prompt":"Record retained notes.","required":false,
            "max_items":MAX_LIST_ENTRIES_V2,"max_item_length":MAX_LIST_ENTRY_SCALARS_V2,
            "max_total_length":MAX_LIST_TOTAL_SCALARS_V2,"unique":false}),
        json!({"id":"report","type":"artifact","prompt":"Reference a report.","required":false}),
        json!({"id":"external","type":"artifact","prompt":"Reference an external result.","required":false}),
        json!({"id":"check","type":"check_result","prompt":"Record the original check outcome.","required":false,
            "operation_id":"verify","operation_digest":operation_digest,"accepted_outcomes":["pass","fail"]}),
    ]);
    group
}

fn escaped_scalars(count: usize) -> String {
    "\u{0001}\u{0002}\u{0003}\u{0004}\u{0005}\u{0006}\\\"🧭éa"
        .chars()
        .cycle()
        .take(count)
        .collect()
}

#[test]
fn scoped_attempts_values_and_artifact_presence_survive_replacement_and_archive() {
    use CandidateSessionLocation::{Archived, Current};

    let mut fixture = Fixture::new(&base_document(), true);
    let (design, _) = fixture.state();
    let digest = design.base_digest().clone();
    let group = canonical(&value_group(&digest));
    activate_group(
        &mut fixture.store,
        activation(&fixture.workspace, &design, &group, "first"),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (mut work, sources) = fixture.state();
    let attempt = work.active_attempt().unwrap().target().clone();
    let first_group = work.selected().unwrap().identity().execution().clone();
    let text = escaped_scalars(MAX_TEXT_SCALARS_V2 as usize);
    let mut entries = vec![escaped_scalars(MAX_LIST_ENTRY_SCALARS_V2 as usize)];
    let rest = MAX_LIST_TOTAL_SCALARS_V2 as usize - MAX_LIST_ENTRY_SCALARS_V2 as usize;
    let count = MAX_LIST_ENTRIES_V2 as usize - 1;
    entries
        .extend((0..count).map(|i| escaped_scalars(rest / count + usize::from(i < rest % count))));
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.chars().count())
            .sum::<usize>(),
        MAX_LIST_TOTAL_SCALARS_V2 as usize
    );
    let local = RecordedItemValueV2::artifact(
        ArtifactValueV1::local_path("reports/report.txt", digest.clone(), 4, "text/plain").unwrap(),
    );
    let external = RecordedItemValueV2::artifact(
        ArtifactValueV1::external_reference("opaque:result", digest.clone(), 4, "text/plain")
            .unwrap(),
    );
    let check = RecordedItemValueV2::check_result(
        CheckResultValueV2::new(
            OperationId::new("verify").unwrap(),
            digest.clone(),
            CheckResultInputBasisV2::new("Original source", digest.clone()).unwrap(),
            CheckResultExecutorV2::new("fixture", "1").unwrap(),
            CheckResultOutcomeV2::Fail,
            "Retain the original failed outcome.",
            digest.clone(),
        )
        .unwrap(),
    );
    let memo = RecordedItemValueV2::text(&text).unwrap();
    work.record_item(
        &attempt,
        &ItemId::new("memo").unwrap(),
        Revision::ZERO,
        Some(RecordedItemValueV2::text("Superseded draft.").unwrap()),
        TIME,
    )
    .unwrap();
    for (item, value) in [
        ("memo", memo.clone()),
        ("notes", RecordedItemValueV2::list(entries.clone()).unwrap()),
        ("empty", RecordedItemValueV2::text("").unwrap()),
        ("report", local.clone()),
        ("external", external.clone()),
        ("check", check.clone()),
    ] {
        work.record_item(
            &attempt,
            &ItemId::new(item).unwrap(),
            Revision::new(u64::from(item == "memo")),
            Some(value),
            TIME,
        )
        .unwrap();
    }
    assessment(&mut work, "failure", "scored", Some(80));
    complete(&mut work);
    fixture.edit_fixture(&work, &sources);
    let reports = fixture.file.parent().unwrap().join("reports");
    std::fs::create_dir(&reports).unwrap();
    std::fs::write(reports.join("report.txt"), "data").unwrap();
    let before_reads = logical_store(&fixture);
    let front = item_page(&mut fixture, Current, &attempt, "memo", None).unwrap();
    assert_eq!(front["total"], MAX_TEXT_SCALARS_V2);
    assert_eq!(front["detail"]["item_revision"], 2);
    assert_eq!(
        front["detail"]["value_digest"],
        json!(recorded_value_digest_v2(&memo).unwrap())
    );
    assert_eq!(front["detail"]["source_attempt"]["attempt"], json!(attempt));
    assert!(front["next_page_token"].is_string());
    let token = front["next_page_token"].as_str().unwrap();
    for malformed in [
        "invalid-token".to_owned(),
        at_offset(token, 0),
        at_offset(token, MAX_TEXT_SCALARS_V2 as usize),
        at_offset(token, MAX_TEXT_SCALARS_V2 as usize + 1),
    ] {
        assert!(matches!(
            item_page(&mut fixture, Current, &attempt, "memo", Some(&malformed)),
            Err(SuccessorExecutionError::InvalidHistoryPageToken)
        ));
    }
    assert!(matches!(
        item_page(&mut fixture, Current, &attempt, "notes", Some(token)),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let metadata = podway_git::test_support::with_artifact_read_observer_v1(
        |_, _| panic!("history availability must not open artifact bytes"),
        || item_page(&mut fixture, Current, &attempt, "report", None).unwrap(),
    );
    assert_eq!(metadata["detail"]["artifact_availability"], "present");
    assert_eq!(metadata["data"], item_value_json_v2(&local).unwrap());
    let opaque = item_page(&mut fixture, Current, &attempt, "external", None).unwrap();
    assert_eq!(opaque["detail"]["artifact_availability"], "unverified");
    assert_eq!(opaque["data"], item_value_json_v2(&external).unwrap());
    let result = item_page(&mut fixture, Current, &attempt, "check", None).unwrap();
    assert_eq!(result["data"], item_value_json_v2(&check).unwrap());
    assert_eq!(
        result["detail"]["value_digest"],
        json!(recorded_value_digest_v2(&check).unwrap())
    );
    let empty = item_page(&mut fixture, Current, &attempt, "empty", None).unwrap();
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["data"], "");
    assert_eq!(empty["detail"]["recorded"], true);
    let absent = item_page(&mut fixture, Current, &attempt, "unset", None).unwrap();
    assert_eq!(absent["data"], Value::Null);
    assert_eq!(absent["detail"]["recorded"], false);
    assert_eq!(absent["detail"]["value_digest"], Value::Null);
    assert!(matches!(
        item_page(&mut fixture, Current, &attempt, "absent", None),
        Err(SuccessorExecutionError::HistoryItemNotFound)
    ));
    assert_eq!(logical_store(&fixture), before_reads);

    let decision = feedback(&work, "redesign");
    replan_group(
        &mut fixture.store,
        mutation(&fixture.workspace, &work, "redesign"),
        decision.clone(),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(matches!(
        item_page(&mut fixture, Current, &attempt, "memo", Some(token)),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let (mut redesigned, sources) = fixture.state();
    let anchor = redesigned.active_attempt().unwrap().target().clone();
    redesigned
        .record_item(
            &anchor,
            &ItemId::new("plan").unwrap(),
            Revision::ZERO,
            Some(RecordedItemValueV2::text("Fresh plan.").unwrap()),
            TIME,
        )
        .unwrap();
    fixture.edit_fixture(&redesigned, &sources);
    let mut replace = activation(&fixture.workspace, &redesigned, &group, "second");
    replace.prior_work.push(
        PriorWorkDispositionV1::new(
            first_group.clone(),
            None,
            PriorWorkChoiceV1::Revalidate,
            "Recheck the earlier evidence.".into(),
        )
        .unwrap(),
    );
    activate_group(
        &mut fixture.store,
        replace,
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (current, _) = fixture.state();
    let second_group = current.selected().unwrap().identity().execution().clone();
    let workspace = fixture.workspace.clone();
    let session = fixture.session.clone();
    let target = SuccessorHistorySession {
        workspace: &workspace,
        session: &session,
        location: Current,
    };
    let first = SuccessorHistoryDefinition::Group {
        execution: &first_group,
        digest: group.digest(),
    };
    let second = SuccessorHistoryDefinition::Group {
        execution: &second_group,
        digest: group.digest(),
    };
    let before_reads = logical_store(&fixture);
    assert!(matches!(
        read_successor_history_dag(
            &mut fixture.store,
            target,
            SuccessorHistoryDefinition::Base(group.digest()),
            None
        ),
        Err(SuccessorExecutionError::HistoryGroupExecutionRequired)
    ));
    let unknown_execution =
        GroupExecutionIdV1::new("00000000-0000-4000-8000-000000000001").unwrap();
    assert!(matches!(
        list_successor_history_attempts(
            &mut fixture.store,
            target,
            SuccessorHistoryDefinition::Group {
                execution: &unknown_execution,
                digest: group.digest()
            },
            page(1, None)
        ),
        Err(SuccessorExecutionError::HistoryGroupNotFound)
    ));
    let unknown_workspace = NativeDirectIdSourceV1.next_workspace_id();
    assert!(matches!(
        read_successor_history_item(
            &mut fixture.store,
            SuccessorHistorySession { workspace: &unknown_workspace, ..target },
            &attempt, &ItemId::new("memo").unwrap(), None,
        ),
        Err(SuccessorExecutionError::Store(CandidateStoreError::Direct(
            DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch { expected, actual }
        ))) if expected == unknown_workspace && actual == workspace
    ));
    let unknown_session = NativeDirectIdSourceV1.next_session_id();
    assert!(matches!(
        read_successor_history_dag(
            &mut fixture.store,
            SuccessorHistorySession { session: &unknown_session, ..target },
            first, None,
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::SessionIdentityMismatch { expected, actual: None }
        )) if expected == unknown_session
    ));
    let earlier_dag =
        content(read_successor_history_dag(&mut fixture.store, target, first, None).unwrap());
    let replacement_dag =
        content(read_successor_history_dag(&mut fixture.store, target, second, None).unwrap());
    assert_ne!(
        earlier_dag["detail"]["scope"],
        replacement_dag["detail"]["scope"]
    );
    assert_eq!(earlier_dag["data"], replacement_dag["data"]);
    let base_dag = content(
        read_successor_history_dag(
            &mut fixture.store,
            target,
            SuccessorHistoryDefinition::Base(&digest),
            None,
        )
        .unwrap(),
    );
    let base_projection: Value = serde_json::from_str(base_dag["data"].as_str().unwrap()).unwrap();
    assert_eq!(
        base_projection["execution_group"],
        json!({"anchor": "design", "success_return": "review"})
    );
    let first_attempt: Value = serde_json::from_str(
        list_successor_history_attempts(
            &mut fixture.store,
            target,
            first,
            SuccessorHistoryPage {
                limit: 1,
                continuation: None,
            },
        )
        .unwrap()
        .as_str(),
    )
    .unwrap();
    assert_eq!(first_attempt["total"], 2);
    assert_eq!(first_attempt["rows"][0]["trace_sequence"], 2);
    assert_eq!(first_attempt["rows"][0]["attempt"], json!(attempt));
    assert_eq!(first_attempt["rows"][0]["lifecycle"], "completed");
    let attempt_token = first_attempt["next_page_token"].as_str();
    assert!(matches!(
        list_successor_history_attempts(
            &mut fixture.store,
            target,
            second,
            SuccessorHistoryPage {
                limit: 1,
                continuation: attempt_token
            },
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let second_attempt: Value = serde_json::from_str(
        list_successor_history_attempts(
            &mut fixture.store,
            target,
            first,
            SuccessorHistoryPage {
                limit: 1,
                continuation: attempt_token,
            },
        )
        .unwrap()
        .as_str(),
    )
    .unwrap();
    assert_eq!(
        second_attempt["rows"][0]["attempt"],
        json!(decision.decision())
    );
    assert_eq!(second_attempt["rows"][0]["trace_sequence"], 3);
    assert_eq!(second_attempt["rows"][0]["decision"]["option"], "redesign");
    assert_eq!(second_attempt["next_page_token"], Value::Null);
    let base: Value = serde_json::from_str(
        list_successor_history_attempts(
            &mut fixture.store,
            target,
            SuccessorHistoryDefinition::Base(&digest),
            SuccessorHistoryPage {
                limit: 64,
                continuation: None,
            },
        )
        .unwrap()
        .as_str(),
    )
    .unwrap();
    assert_eq!(base["total"], 2);
    assert_eq!(base["rows"][1]["trace_sequence"], 4);

    let wrong = ScopedAttemptV1::new(
        session.clone(),
        digest,
        ExecutionScopeV1::Group {
            group_execution_id: second_group,
            definition_digest: group.digest().clone(),
        },
        attempt.node().clone(),
        attempt.attempt().clone(),
    );
    assert!(matches!(
        item_page(&mut fixture, Current, &wrong, "memo", None),
        Err(SuccessorExecutionError::HistoryAttemptNotFound)
    ));
    std::fs::remove_file(reports.join("report.txt")).unwrap();
    let missing = item_page(&mut fixture, Current, &attempt, "report", None).unwrap();
    assert_eq!(missing["detail"]["artifact_availability"], "missing");
    assert_eq!(missing["data"], metadata["data"]);
    assert_eq!(
        missing["detail"]["value_digest"],
        metadata["detail"]["value_digest"]
    );
    std::os::unix::fs::symlink(&fixture.file, reports.join("report.txt")).unwrap();
    assert_eq!(
        item_page(&mut fixture, Current, &attempt, "report", None).unwrap()["detail"]["artifact_availability"],
        "unverified"
    );
    std::fs::remove_file(reports.join("report.txt")).unwrap();
    std::fs::remove_dir(&reports).unwrap();
    let parent_missing = item_page(&mut fixture, Current, &attempt, "report", None).unwrap();
    assert_eq!(parent_missing, missing);
    std::os::unix::fs::symlink(fixture.file.parent().unwrap(), &reports).unwrap();
    assert_eq!(
        item_page(&mut fixture, Current, &attempt, "report", None).unwrap()["detail"]["artifact_availability"],
        "unverified"
    );
    std::fs::remove_file(&reports).unwrap();
    std::fs::write(&reports, "Not a directory.").unwrap();
    assert_eq!(
        item_page(&mut fixture, Current, &attempt, "report", None).unwrap()["detail"]["artifact_availability"],
        "unverified"
    );
    std::fs::remove_file(&reports).unwrap();
    assert_eq!(logical_store(&fixture), before_reads);

    issue_cursor_mutation(
        &mut fixture.store,
        mutation(&fixture.workspace, &current, "cancel"),
        SuccessorCursorMutation::Cancel(ReasonV2::new("End the history fixture.").unwrap()),
        || TIME,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let terminal = fixture.state().0.fence().revision;
    let terminal_page = item_page(&mut fixture, Current, &attempt, "memo", None).unwrap();
    assert_eq!(terminal_page["session_revision"], json!(terminal));
    fixture
        .store
        .archive_fixture_session(&fixture.workspace, &fixture.session, terminal, definition)
        .unwrap();
    fixture.reopen();
    let before_reads = logical_store(&fixture);
    let archived_target = SuccessorHistorySession {
        location: Archived,
        ..target
    };
    let archived_dag = content(
        read_successor_history_dag(&mut fixture.store, archived_target, first, None).unwrap(),
    );
    assert_eq!(archived_dag["detail"], earlier_dag["detail"]);
    assert_eq!(archived_dag["data"], earlier_dag["data"]);
    let archived_attempts: Value = serde_json::from_str(
        list_successor_history_attempts(
            &mut fixture.store,
            archived_target,
            first,
            SuccessorHistoryPage {
                limit: 64,
                continuation: None,
            },
        )
        .unwrap()
        .as_str(),
    )
    .unwrap();
    assert_eq!(
        archived_attempts["rows"],
        json!([first_attempt["rows"][0], second_attempt["rows"][0]])
    );
    let front = item_page(&mut fixture, Archived, &attempt, "memo", None).unwrap();
    assert_eq!(front["session_revision"], terminal_page["session_revision"]);
    assert!(matches!(
        item_page(
            &mut fixture,
            Archived,
            &attempt,
            "memo",
            terminal_page["next_page_token"].as_str()
        ),
        Err(SuccessorExecutionError::StaleHistoryPageToken)
    ));
    let joined = full_text(front, |token| {
        item_page(&mut fixture, Archived, &attempt, "memo", Some(token)).unwrap()
    });
    assert_eq!(joined, text);
    let mut next = None;
    let mut actual = Vec::new();
    loop {
        let part = item_page(&mut fixture, Archived, &attempt, "notes", next.as_deref()).unwrap();
        assert_eq!(part["offset"], actual.len());
        assert_eq!(part["total"], entries.len());
        let values = part["data"].as_array().unwrap();
        assert_eq!(part["size"], values.len());
        actual.extend(
            values
                .iter()
                .map(|entry| entry.as_str().unwrap().to_owned()),
        );
        next = part["next_page_token"].as_str().map(str::to_owned);
        assert_eq!(part["truncated"], next.is_some());
        if next.is_none() {
            break;
        }
        assert!(
            serde_json::to_vec(values).unwrap().len()
                + 1
                + serde_json::to_vec(&entries[actual.len()]).unwrap().len()
                > MAX_EVIDENCE_PAGE_BYTES_V2
        );
    }
    assert_eq!(actual, entries);
    let archived_report = item_page(&mut fixture, Archived, &attempt, "report", None).unwrap();
    assert_eq!(archived_report["detail"], missing["detail"]);
    assert_eq!(archived_report["data"], missing["data"]);
    assert_eq!(logical_store(&fixture), before_reads);
}
