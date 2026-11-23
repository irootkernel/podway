//! Real Git worktrees and current-generation workspace commands without a daemon.
use podway_core::{
    AttemptId, BlockerId, DelegationReceiptIdV1, ProcedureSnapshotId, SessionId, Sha256Digest,
    UnixMillis, WorkspaceId,
};
use podway_engine::{
    direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1},
    direct_workspace::{
        DirectWorkspaceErrorV1, DirectWorkspaceMutationRequestV1, DirectWorkspaceReadRequestV1,
        doctor_workspace_v1, initialize_workspace_v1, repair_workspace_v1, show_workspace_v1,
    },
    execution::ExecutionClockV1,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{RequestIdV1, validate_daemon_free_document_v1};
use podway_store::{IdempotencyKeyV1, direct_store::DirectStoreErrorV1};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::ffi::OsStrExt,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use tempfile::TempDir;
const RESPONSE: &str = "00000000-0000-4000-8000-000000000001";
const DIFFERENT: &str = "00000000-0000-4000-8000-000000000002";

#[cfg(feature = "test-support")]
#[test]
fn successor_public_results_bind_both_generations_to_the_native_cursor() {
    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::DirectReadCommandV1,
        direct_items::DirectItemCommandV1,
        direct_start::DirectStartCommandV1,
        successor_execution::{
            DirectSuccessorMutationV1, DirectSuccessorWriteV1, SuccessorCursor,
            SuccessorCursorMutation,
        },
    };
    use podway_protocol::{
        ContractGenerationV1::{Generation1, Generation2},
        ItemMutationPreconditionsWireV1, ItemSetV1, Rfc3339MillisV1, SessionStartCommandV1,
        SessionStartSourceV1, SessionStartV1, decode_scoped_item_record_many_input_v1,
    };

    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    let workspace = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "public-init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap()
    .as_value()["workspace"]["uuid"]
        .as_str()
        .map(|uuid| WorkspaceId::new(uuid).unwrap())
        .unwrap();
    let preset = podway_presets::lookup_v3("small-change-v3").unwrap();
    std::fs::write(root.path().join("successor.yaml"), preset.base_yaml).unwrap();

    let submit = |command, key: Option<&str>, generation, return_observation| {
        let response = execute_direct_v1(
            &resolver,
            DirectRequestV1 {
                request_id: RequestIdV1::new(RESPONSE).unwrap(),
                selector: selector(root.path()),
                expected_workspace_id: Some(workspace.clone()),
                command,
                idempotency_key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
                deadline: deadline(),
                observed_at: Rfc3339MillisV1::new("2026-10-07T00:00:00.000Z").unwrap(),
                timeout_ms: 5000,
                generation,
                return_observation,
            },
            &clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        match response {
            DirectResponseV1::Success(output) => output.into_value(),
            DirectResponseV1::Failure(error) => panic!("{}", error.as_value()),
        }
    };
    let start = submit(
        DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "successor.yaml".to_owned(),
                },
                expected_procedure_digest: Some(Sha256Digest::new(preset.base_digest).unwrap()),
                task_title: "Verify native scoped output".into(),
                dry_run: false,
            },
            preconditions: None,
        })),
        Some("public-start"),
        Generation1,
        true,
    );
    assert_eq!(
        start["result"]["schema"],
        "podway.mutation-observation-result/v2"
    );
    let session = SessionId::new(start["session"]["id"].as_str().unwrap()).unwrap();
    let prepared = &start["result"]["observation"];
    assert!(prepared["cursor"]["attempt"].is_null());
    let begin = submit(
        DirectCommandV1::SuccessorMutation(DirectSuccessorMutationV1::Cursor {
            actor: None,
            cursor: SuccessorCursor {
                execution: podway_core::execution_groups::ExecutionCursorFenceV1 {
                    session: session.clone(),
                    revision: podway_core::Revision::ZERO,
                    goal_revision: None,
                    attempt: None,
                },
                selected_group: None,
            },
            operation: SuccessorCursorMutation::Begin(None),
        }),
        Some("public-begin"),
        Generation1,
        true,
    );
    let observe = |generation| {
        submit(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::Observe,
                expected_session_id: Some(session.clone()),
            },
            None,
            generation,
            false,
        )
    };
    let generation1 = observe(Generation1);
    let generation2 = observe(Generation2);
    assert_eq!(
        generation1["result"]["schema"],
        "podway.observation-result/v7"
    );
    assert_eq!(
        generation2["result"]["schema"],
        "podway.observation-result/v8"
    );
    assert_eq!(
        generation1["result"]["cursor"],
        generation2["result"]["cursor"]
    );
    assert_eq!(begin["result"]["observation"], generation2["result"]);
    let mut invalid = begin;
    invalid["result"]["mutation_result"]["operation"] = serde_json::json!("session.complete");
    assert!(!validate_daemon_free_document_v1(
        "podway.output/v4",
        &invalid
    ));

    let current = observe(Generation2);
    let attempt: podway_core::execution_groups::ScopedAttemptV1 =
        serde_json::from_value(current["result"]["cursor"]["attempt"].clone()).unwrap();
    let edit = DirectCommandV1::SuccessorWrite(DirectSuccessorWriteV1::Edit {
        attempt: attempt.clone(),
        selected_group: None,
        command: DirectItemCommandV1::Set(ItemSetV1 {
            item_id: podway_core::ItemId::new("scope-summary").unwrap(),
            value: "Record the precise test fixture scope.".into(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: session.clone(),
                expected_attempt_id: attempt.attempt().clone(),
                expected_item_revision: podway_core::Revision::ZERO,
            },
        }),
    });
    let edited = submit(edit.clone(), Some("public-set"), Generation1, false);
    assert_eq!(edited["result"]["items"][0]["revision"], 1);
    assert_eq!(edited["result"]["items"][0]["previous_revision"], 0);
    assert!(edited["result"]["items"][0]["value_digest"].is_string());
    assert_eq!(edited, submit(edit, Some("public-set"), Generation2, false));

    let input = serde_json::json!({
        "schema": "podway.scoped-item-record-many-input/v1",
        "context": {
            "workspace_uuid": workspace,
            "idempotency_key": "public-batch",
            "cursor": {
                "attempt": attempt,
                "session_revision": edited["session"]["revision_after"],
                "goal_revision": null,
                "selected_group": null,
            },
        },
        "operations": [{
            "item_id": "scope-summary", "expected_item_revision": 1,
            "record": {"type": "text", "value": "Use the updated fixture scope."},
        }],
    });
    let input = decode_scoped_item_record_many_input_v1(input.to_string().as_bytes()).unwrap();
    let updated = submit(
        DirectCommandV1::SuccessorWrite(DirectSuccessorWriteV1::Atomic {
            cursor: SuccessorCursor {
                execution: input.context.cursor.execution(),
                selected_group: input.context.cursor.selected_group,
            },
            operations: input.operations,
        }),
        Some(input.context.idempotency_key.as_str()),
        Generation1,
        true,
    );
    assert_eq!(
        updated["result"]["mutation_result"]["items"][0]["revision"],
        2
    );
    assert_eq!(
        updated["result"]["observation"],
        observe(Generation2)["result"]
    );
}

#[cfg(feature = "test-support")]
#[test]
fn successor_native_delegation_keeps_one_receipt_for_the_scoped_attempt() {
    use super::int_group002_transactions::{
        activation, base_document, canonical, definition, group_document, mutation as scope,
    };
    use podway_core::{ItemId, ReasonV2, Revision};
    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::DirectReadCommandV1,
        direct_start::{DirectStartCommandV1, DirectStartRequestV1},
        successor_execution::{
            DirectSuccessorMutationV1, DirectSuccessorWriteV1, SuccessorCursor,
            SuccessorCursorMutation, SuccessorItemRequest, activate_group, issue_cursor_mutation,
            record_successor_item, start_native_successor,
        },
    };
    use podway_protocol::{
        ContractGenerationV1, ItemRecordManyDispositionV1, ItemRecordManyOperationV1,
        ItemRecordValueV1, Rfc3339MillisV1, SessionStartCommandV1, SessionStartSourceV1,
        SessionStartV1, decode_scoped_delegation_accept_input_v1,
    };
    use podway_store::successor_store::{CandidateStoreError, NativeSuccessorStore};
    use serde_json::{Value, json};

    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    let ids = NativeDirectIdSourceV1;
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init-scoped-delegation"),
        false,
        &clock,
        &ids,
    )
    .unwrap();
    let (resolved, store) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    let workspace = resolved.store_identity().workspace_uuid().clone();
    let base = canonical(&base_document());
    std::fs::write(
        root.path().join("delegated-base.json"),
        base.canonical_json().as_str(),
    )
    .unwrap();
    start_native_successor(
        &resolver,
        DirectStartRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: Some(workspace.clone()),
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("delegated-start").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartCommandV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "delegated-base.json".to_owned(),
                    },
                    expected_procedure_digest: Some(base.digest().clone()),
                    task_title: "Exercise scoped delegated work".to_owned(),
                    dry_run: false,
                },
                preconditions: None,
            }),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let read = || {
        store
            .read_admitting_typed(
                deadline(),
                |_| Ok::<_, CandidateStoreError>(()),
                |(), snapshot| snapshot.successor_state(definition),
            )
            .unwrap()
            .unwrap()
            .unwrap()
            .0
    };

    let mut adapter = NativeSuccessorStore::new(&store, deadline(), |_| Ok(()));
    issue_cursor_mutation(
        &mut adapter,
        scope(&workspace, &read(), "delegated-begin"),
        SuccessorCursorMutation::Begin(None),
        || clock.now(),
        &ids,
    )
    .unwrap();
    let active = read().active_attempt().unwrap().target().clone();
    record_successor_item(
        &mut adapter,
        SuccessorItemRequest {
            response: podway_engine::successor_execution::SuccessorMutationResponseV1::Fixture,
            workspace: workspace.clone(),
            key: IdempotencyKeyV1::new("delegated-plan").unwrap(),
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            target: active,
            selected_group: None,
        },
        podway_core::execution_groups::GroupItemWriteV1 {
            item: ItemId::new("plan").unwrap(),
            expected_revision: Revision::ZERO,
            value: Some(
                podway_core::RecordedItemValueV2::text("Record the deterministic fixture output.")
                    .unwrap(),
            ),
        },
        || clock.now(),
    )
    .unwrap();
    let mut group = group_document();
    group["node_definitions"]["work"]["items"] = json!([
        {"id":"worker-output","type":"text","prompt":"Record the accepted worker output.","required":true}
    ]);
    group["graph"]["nodes"][0]["delegation"] = json!({
        "binding_ref":"fixture-binding", "role_ref":"deterministic-worker",
        "role_snapshot_digest":digest(), "max_rounds":2, "after_accept":"release",
    });
    let group = canonical(&group);
    activate_group(
        &mut adapter,
        activation(&workspace, &read(), &group, "delegated-activate"),
        || clock.now(),
        &ids,
    )
    .unwrap();
    let target = read().active_attempt().unwrap().target().clone();
    assert!(matches!(
        target.scope(),
        podway_core::execution_groups::ExecutionScopeV1::Group { .. }
    ));
    let session = target.session().clone();

    let invoke = |command, key: Option<&str>| {
        execute_direct_v1(
            &resolver,
            DirectRequestV1 {
                request_id: RequestIdV1::new(RESPONSE).unwrap(),
                selector: selector(root.path()),
                expected_workspace_id: Some(workspace.clone()),
                command,
                idempotency_key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
                deadline: deadline(),
                observed_at: Rfc3339MillisV1::new("2026-11-19T00:00:00.000Z").unwrap(),
                timeout_ms: 5000,
                generation: ContractGenerationV1::Generation2,
                return_observation: false,
            },
            &clock,
            &ids,
        )
        .unwrap()
    };
    let success = |response| match response {
        DirectResponseV1::Success(output) => output.into_value(),
        DirectResponseV1::Failure(error) => panic!("{}", error.as_value()),
    };
    let reject = |response, code| match response {
        DirectResponseV1::Failure(error) => {
            assert_eq!(error.as_value()["code"], code, "{}", error.as_value())
        }
        DirectResponseV1::Success(output) => panic!("{}", output.as_value()),
    };
    let package = || {
        success(invoke(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::DelegationPackage,
                expected_session_id: Some(session.clone()),
            },
            None,
        ))
    };
    let write = |key, expected: u64, value: &str| {
        success(invoke(
            DirectCommandV1::SuccessorWrite(DirectSuccessorWriteV1::Atomic {
                cursor: SuccessorCursor::from_state(&read()),
                operations: vec![ItemRecordManyOperationV1 {
                    item_id: ItemId::new("worker-output").unwrap(),
                    expected_item_revision: Revision::new(expected),
                    disposition: ItemRecordManyDispositionV1::Record {
                        record: ItemRecordValueV1::Text {
                            value: value.into(),
                        },
                    },
                }],
            }),
            Some(key),
        ))
    };
    let complete = |key| {
        invoke(
            DirectCommandV1::SuccessorMutation(DirectSuccessorMutationV1::Cursor {
                actor: None,
                cursor: SuccessorCursor::from_state(&read()),
                operation: SuccessorCursorMutation::Complete,
            }),
            Some(key),
        )
    };
    let before = package();
    write("delegated-output", 0, "Accepted fixture output.");
    let after = package();
    assert_eq!(
        before["result"]["package_digest"],
        after["result"]["package_digest"]
    );
    assert_eq!(after["result"]["package"]["target"], json!(target));
    reject(complete("receipt-required"), "DELEGATION_RECEIPT_REQUIRED");

    let acceptance = |key: &str, result: &Value| {
        let payload = json!({
            "schema":"podway.delegation-accept-input/v2",
            "context":{
                "workspace_uuid":workspace, "idempotency_key":key,
                "cursor":{
                    "attempt":result["cursor"]["attempt"], "session_revision":result["cursor"]["session_revision"],
                    "goal_revision":result["cursor"]["goal_revision"], "selected_group":result["cursor"]["selected_group"],
                },
            },
            "package_digest":result["package_digest"],
            "execution":{
                "binding_ref":"fixture-binding", "adapter_id":"test-adapter", "adapter_version":"1",
                "primary_host":"managed", "scope_kind":"session", "scope_ref":"test-scope",
                "supervisor_ref":"test-supervisor", "worker_ref":"fixture-worker", "task_ref":"fixture-task",
                "role_ref":"deterministic-worker", "role_snapshot_digest":digest(),
                "agent_configuration_digest":digest(), "accepted_task_digest":digest(),
            },
            "rounds_used":1, "result_digest":digest(), "summary":"Accepted the deterministic fixture.",
            "worker_disposition":"released", "accepted":true,
        });
        let input =
            decode_scoped_delegation_accept_input_v1(payload.to_string().as_bytes()).unwrap();
        invoke(DirectCommandV1::SuccessorDelegationAccept(input), Some(key))
    };
    let original = success(acceptance("accept-fixture", &after["result"]));
    assert_eq!(
        original,
        success(acceptance("accept-fixture", &after["result"]))
    );
    let receipt = || {
        success(invoke(
            DirectCommandV1::Read {
                command: DirectReadCommandV1::DelegationReceipt {
                    session_id: session.clone(),
                    attempt_id: target.attempt().clone(),
                },
                expected_session_id: Some(session.clone()),
            },
            None,
        ))
    };
    let accepted = receipt();
    assert_eq!(accepted["result"]["acceptance_state"], "accepted");
    assert_eq!(
        accepted["result"]["receipt"]["receipt_id"],
        original["result"]["receipt_id"]
    );
    assert_eq!(
        accepted["result"]["receipt"]["binding"]["target"],
        json!(target)
    );

    write("change-accepted-output", 1, "Edited fixture output.");
    reject(complete("receipt-stale"), "DELEGATION_RECEIPT_STALE");
    reject(
        acceptance("duplicate-fixture", &package()["result"]),
        "DELEGATION_ALREADY_ACCEPTED",
    );
    let stale = receipt();
    assert_eq!(stale["result"]["acceptance_state"], "stale");
    assert_eq!(accepted["result"]["receipt"], stale["result"]["receipt"]);

    success(invoke(
        DirectCommandV1::SuccessorMutation(DirectSuccessorMutationV1::Cursor {
            actor: None,
            cursor: SuccessorCursor::from_state(&read()),
            operation: SuccessorCursorMutation::Retry(
                ReasonV2::new("Replace the demonstrated stale attempt.").unwrap(),
            ),
        }),
        Some("retry-delegated"),
    ));
    let retried = package();
    assert_ne!(
        after["result"]["package_digest"],
        retried["result"]["package_digest"]
    );
    write("retry-output", 0, "Accepted retry fixture output.");
    success(acceptance("accept-retry-fixture", &package()["result"]));
    success(complete("complete-with-receipt"));
    assert_eq!(
        read().active_attempt().unwrap().target().node().as_str(),
        "finish"
    );
    assert_eq!(
        receipt()["result"]["receipt"],
        accepted["result"]["receipt"]
    );
}

#[cfg(feature = "test-support")]
#[test]
fn successor_start_keeps_original_outcome_after_source_disappears() {
    use podway_engine::{
        direct_execution::DirectExecutionErrorV1,
        direct_start::{DirectStartCommandV1, DirectStartRequestV1},
        successor_execution::start_native_successor,
    };
    use podway_protocol::{SessionStartCommandV1, SessionStartSourceV1, SessionStartV1};
    use serde_json::Value;

    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init-successor"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let path = root.path().join("successor.yaml");
    let preset = podway_presets::lookup_v3("small-change-v3").unwrap();
    std::fs::write(&path, preset.base_yaml).unwrap();
    assert_eq!(
        std::fs::read(root.path().join(".podway/config.yaml")).unwrap(),
        b"schema: podway.workspace/v1\nprocedure_paths:\n  - .podway/procedures\nui:\n  show_stage_in_prompt: false\n",
    );

    let request = |key: &str, dry_run: bool, digest: Option<Sha256Digest>| DirectStartRequestV1 {
        selector: selector(root.path()),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: Some(IdempotencyKeyV1::new(key).unwrap()),
        deadline: deadline(),
        command: DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "successor.yaml".to_owned(),
                },
                expected_procedure_digest: digest,
                task_title: "Use a pinned successor Procedure".to_owned(),
                dry_run,
            },
            preconditions: None,
        }),
    };
    let preview = start_native_successor(
        &resolver,
        request("preview", true, None),
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let preview: Value = serde_json::from_str(preview.as_str()).unwrap();
    assert_eq!(preview["result"]["procedure_schema"], "podway.procedure/v3");
    assert_eq!(preview["result"]["procedure_digest"], preset.base_digest);
    assert!(preview["result"].get("session_id").is_none());

    let digest = Sha256Digest::new(preset.base_digest).unwrap();
    let start = || request("start-successor", false, Some(digest.clone()));
    let first =
        start_native_successor(&resolver, start(), &clock, &NativeDirectIdSourceV1).unwrap();
    let prepared: Value = serde_json::from_str(first.as_str()).unwrap();
    assert_eq!(prepared["result"]["revision"], 0);
    assert_eq!(
        prepared["session"]["title"],
        "Use a pinned successor Procedure"
    );
    assert!(matches!(
        start_native_successor(
            &resolver,
            request("fresh-existing", false, Some(digest.clone())),
            &NoEffects,
            &NoEffects
        ),
        Err(DirectExecutionErrorV1::StartDecisionRequired { .. })
    ));

    std::fs::remove_file(path).unwrap();
    let replay = start_native_successor(&resolver, start(), &NoEffects, &NoEffects).unwrap();
    assert_eq!(replay, first);
    assert!(matches!(
        start_native_successor(
            &resolver,
            request("fresh-missing", false, Some(digest)),
            &NoEffects,
            &NoEffects
        ),
        Err(DirectExecutionErrorV1::Start(_))
    ));
}

#[cfg(feature = "test-support")]
#[test]
fn successor_activation_checks_the_exact_required_artifact_after_admission() {
    use super::int_group002_transactions::{
        activation, base_document, canonical, definition, group_document, mutation as scope,
    };
    use podway_core::{
        DomainError, ItemId, RecordedItemValueV2, Revision, execution_groups::GroupItemWriteV1,
    };
    use podway_engine::{
        direct_start::{DirectStartCommandV1, DirectStartRequestV1},
        native_execution::NativeDirectWorkspaceV1,
        successor_execution::{
            SuccessorCursorMutation, SuccessorExecutionError, SuccessorItemRequest, activate_group,
            issue_cursor_mutation, prepare_successor_progress, record_successor_item,
            start_native_successor, verify_successor_progress,
        },
    };
    use podway_protocol::{SessionStartCommandV1, SessionStartSourceV1, SessionStartV1};
    use podway_store::successor_store::{
        CandidateStoreError, NativeSuccessorStore, SuccessorReadStore,
    };
    use serde_json::{Value, json};

    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    let ids = NativeDirectIdSourceV1;
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init-progress"),
        false,
        &clock,
        &ids,
    )
    .unwrap();
    let (resolved, store, binding) = resolver
        .resolve_existing_with_binding(selector(root.path()), None, deadline())
        .unwrap();
    let workspace = binding.identity().workspace_uuid();
    let native = NativeDirectWorkspaceV1::new(resolved);
    let mut base = base_document();
    base["node_definitions"]["design"]["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "proof", "type": "artifact", "prompt": "Record the plan file.", "required": true
        }));
    let base = canonical(&base);
    std::fs::write(
        root.path().join("base.json"),
        base.canonical_json().as_str(),
    )
    .unwrap();
    start_native_successor(
        &resolver,
        DirectStartRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: Some(workspace.clone()),
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("start-progress").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartCommandV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "base.json".to_owned(),
                    },
                    expected_procedure_digest: Some(base.digest().clone()),
                    task_title: "Progress artifact fixture".to_owned(),
                    dry_run: false,
                },
                preconditions: None,
            }),
        },
        &clock,
        &ids,
    )
    .unwrap();
    let read = || {
        store
            .read_admitting_typed(
                deadline(),
                |_| Ok::<_, CandidateStoreError>(()),
                |(), snapshot| {
                    Ok::<_, CandidateStoreError>((
                        snapshot.successor_state(definition)?.unwrap().0,
                        snapshot.committed_sequence()?,
                    ))
                },
            )
            .unwrap()
            .unwrap()
    };

    let mut store_adapter = NativeSuccessorStore::new(&store, deadline(), |_| Ok(()));
    issue_cursor_mutation(
        &mut store_adapter,
        scope(workspace, &read().0, "begin-progress"),
        SuccessorCursorMutation::Begin(None),
        || clock.now(),
        &ids,
    )
    .unwrap();
    let proof = root.path().join("proof.txt");
    std::fs::write(&proof, "original artifact bytes").unwrap();
    let artifact = native
        .prepare_local_artifact_v1(&binding, "proof.txt", None)
        .unwrap();
    let active = read().0.active_attempt().unwrap().target().clone();
    for (key, item, value) in [
        (
            "write-plan",
            "plan",
            RecordedItemValueV2::text("Work from the adopted plan.").unwrap(),
        ),
        (
            "write-proof",
            "proof",
            RecordedItemValueV2::artifact(artifact.artifact().clone()),
        ),
    ] {
        record_successor_item(
            &mut store_adapter,
            SuccessorItemRequest {
                response: podway_engine::successor_execution::SuccessorMutationResponseV1::Fixture,
                workspace: workspace.clone(),
                key: IdempotencyKeyV1::new(key).unwrap(),
                request_id: RequestIdV1::new(RESPONSE).unwrap(),
                target: active.clone(),
                selected_group: None,
            },
            GroupItemWriteV1 {
                item: ItemId::new(item).unwrap(),
                expected_revision: Revision::ZERO,
                value: Some(value),
            },
            || clock.now(),
        )
        .unwrap();
    }
    let (ready, sequence) = read();
    let group = canonical(&group_document());
    let activate = || activation(workspace, &ready, &group, "activate-progress");
    let prepared = prepare_successor_progress(&native, &binding, &ready).unwrap();
    let verify =
        |binding: &_, state: &_| verify_successor_progress(&native, binding, state, &prepared);
    std::fs::write(&proof, "changed artifact bytes").unwrap();
    let mut guarded =
        NativeSuccessorStore::new(&store, deadline(), |_| Ok(())).with_progress_verifier(&verify);
    assert!(matches!(
        activate_group(
            &mut guarded,
            activate(),
            || panic!("stale progress spent clock"),
            &NoEffects
        ),
        Err(SuccessorExecutionError::Domain(
            DomainError::ArtifactChanged
        ))
    ));
    assert_eq!(read(), (ready.clone(), sequence));
    assert!(
        guarded
            .lookup_outcome(
                workspace,
                &IdempotencyKeyV1::new("activate-progress").unwrap()
            )
            .unwrap()
            .is_none()
    );

    std::fs::write(&proof, "original artifact bytes").unwrap();
    let prepared = prepare_successor_progress(&native, &binding, &ready).unwrap();
    let verify =
        |binding: &_, state: &_| verify_successor_progress(&native, binding, state, &prepared);
    let mut guarded =
        NativeSuccessorStore::new(&store, deadline(), |_| Ok(())).with_progress_verifier(&verify);
    let committed = activate_group(&mut guarded, activate(), || clock.now(), &ids).unwrap();
    let output: Value = serde_json::from_str(committed.outcome().response().as_str()).unwrap();
    assert_eq!(output["session"]["title"], "Progress artifact fixture");

    std::fs::remove_file(proof).unwrap();
    let unreachable = |_: &_, _: &_| panic!("replay must bypass progress validation");
    let mut guarded = NativeSuccessorStore::new(&store, deadline(), |_| Ok(()))
        .with_progress_verifier(&unreachable);
    let replay = activate_group(
        &mut guarded,
        activate(),
        || panic!("replay spent clock"),
        &NoEffects,
    )
    .unwrap();
    assert_eq!(replay.outcome(), committed.outcome());
}

#[cfg(feature = "test-support")]
#[test]
fn successor_start_replaces_only_under_the_selected_native_policy() {
    use podway_engine::{
        direct_start::{DirectStartCommandV1, DirectStartRequestV1},
        successor_execution::{
            SuccessorCursor, SuccessorCursorMutation, SuccessorMutationRequest,
            issue_cursor_mutation, start_native_successor,
        },
    };
    use podway_protocol::{
        SessionDeletionModeV1, SessionIdentityPreconditionsWireV1, SessionStartCommandV1,
        SessionStartReplaceV1, SessionStartSourceV1, SessionStartV1, TerminalDispositionInputV1,
    };
    use podway_store::successor_store::{CandidateStoreError, NativeSuccessorStore};

    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init-successor"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (resolved, store) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    let workspace = resolved.store_identity().workspace_uuid();
    let mut native = NativeSuccessorStore::new(&store, deadline(), |_| Ok(()));
    let input = || SessionStartV1 {
        source: SessionStartSourceV1::Preset {
            preset: "small-change-v3".into(),
        },
        expected_procedure_digest: None,
        task_title: "Scoped successor".into(),
        dry_run: false,
    };
    let start = |key: &str, command: DirectStartCommandV1| {
        start_native_successor(
            &resolver,
            DirectStartRequestV1 {
                selector: selector(root.path()),
                expected_workspace_id: Some(workspace.clone()),
                request_id: RequestIdV1::new(RESPONSE).unwrap(),
                key: Some(IdempotencyKeyV1::new(key).unwrap()),
                deadline: deadline(),
                command,
            },
            &clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap()
    };
    let read = || {
        store
            .read_admitting_typed(
                deadline(),
                |_| Ok::<_, CandidateStoreError>(()),
                |(), snapshot| {
                    snapshot.successor_state(super::int_group002_transactions::definition)
                },
            )
            .unwrap()
            .unwrap()
            .unwrap()
            .0
    };
    let replace = |mode, fence: podway_core::execution_groups::ExecutionCursorFenceV1| {
        DirectStartCommandV1::Replace(SessionStartReplaceV1 {
            start: input(),
            mode,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: fence.session,
                expected_session_revision: fence.revision,
            },
        })
    };

    start(
        "original",
        DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: input(),
            preconditions: None,
        }),
    );
    let original = read().fence();
    start(
        "replace-eligible",
        replace(SessionDeletionModeV1::Eligible, original.clone()),
    );
    let prepared = read();
    assert_ne!(prepared.fence().session, original.session);
    let scope = |key: &str, state| SuccessorMutationRequest {
        response: podway_engine::successor_execution::SuccessorMutationResponseV1::Fixture,
        workspace: workspace.clone(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        cursor: SuccessorCursor::from_state(state),
    };
    issue_cursor_mutation(
        &mut native,
        scope("begin-preserved", &prepared),
        SuccessorCursorMutation::Begin(None),
        || clock.now(),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let running = read().fence();
    start(
        "replacement-preserving-running",
        replace(
            SessionDeletionModeV1::Supersede {
                reason: "Retain the current trace".into(),
                actor: None,
            },
            running.clone(),
        ),
    );
    let replacement = read();
    issue_cursor_mutation(
        &mut native,
        scope("begin-cancelled", &replacement),
        SuccessorCursorMutation::Begin(None),
        || clock.now(),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    issue_cursor_mutation(
        &mut native,
        scope("end-replacement", &read()),
        SuccessorCursorMutation::Cancel(podway_core::ReasonV2::new("Finish fixture").unwrap()),
        || clock.now(),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let terminal = read().fence();
    start(
        "replacement-with-disposition",
        replace(
            SessionDeletionModeV1::DisposeAndArchive {
                disposition: TerminalDispositionInputV1::NotRequired {
                    reason: "Owned fixture result".into(),
                },
                actor: None,
            },
            terminal.clone(),
        ),
    );
    assert_ne!(read().fence().session, terminal.session);
    store
        .read_admitting_typed(
            deadline(),
            |_| Ok::<_, CandidateStoreError>(()),
            |(), snapshot| {
                assert!(snapshot.successor_metadata(&original.session)?.is_none());
                let preserved = snapshot
                    .archived_successor_state(
                        &running.session,
                        super::int_group002_transactions::definition,
                    )?
                    .unwrap()
                    .0;
                assert_eq!(
                    preserved.lifecycle(),
                    podway_core::SessionLifecycle::Cancelled
                );
                let preserved_disposition = snapshot
                    .successor_disposition(&running.session, preserved.fence().revision)?
                    .unwrap();
                assert_eq!(
                    preserved_disposition.successor_session_id(),
                    Some(&terminal.session)
                );
                assert!(
                    snapshot
                        .archived_successor_state(
                            &terminal.session,
                            super::int_group002_transactions::definition
                        )?
                        .is_some()
                );
                Ok::<_, CandidateStoreError>(())
            },
        )
        .unwrap()
        .unwrap();
}

#[test]
fn bounded_preliminary_retries_preserve_native_static_layout_and_durable_binding_failures() {
    use podway_engine::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    use podway_git::{
        DurableWorktreeIdentityV1, GitRepresentationProblemV1, GitResolveErrorV1,
        GitResolverContractV1, ValidatedWorktreeV1,
    };
    struct CountedNative(std::sync::Arc<AtomicUsize>);
    impl GitResolverContractV1 for CountedNative {
        fn resolve(
            &self,
            selector: WorktreeSelectorV1,
        ) -> Result<ValidatedWorktreeV1, GitResolveErrorV1> {
            self.0.fetch_add(1, Ordering::SeqCst);
            NativeGitResolverV1::new().resolve(selector)
        }
    }
    let unsafe_root = fixture();
    std::fs::write(unsafe_root.path().join(".podway"), b"ordinary file").unwrap();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let counted =
        DirectWorkspaceResolverV1::for_v15_fixture(CountedNative(calls.clone()), digest());
    for existing in [false, true] {
        let error = if existing {
            counted
                .resolve_existing(selector(unsafe_root.path()), None, deadline())
                .unwrap_err()
        } else {
            counted
                .resolve_candidate(selector(unsafe_root.path()))
                .unwrap_err()
        };
        assert!(matches!(
            error,
            DirectWorkspaceResolutionErrorV1::Resolution(WorkspaceResolutionErrorV1::Git {
                source: GitResolveErrorV1::Representation {
                    problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout
                },
                ..
            })
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 16);
    assert_eq!(
        std::fs::read(unsafe_root.path().join(".podway")).unwrap(),
        b"ordinary file"
    );
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (resolved, _) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    let identity = resolved.worktree().identity();
    let wrong = WorkspaceId::new(DIFFERENT).unwrap();
    let fenced = WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        Some(
            DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
                wrong.clone(),
                identity.common_directory_fingerprint().clone(),
                identity.worktree_administration_fingerprint().clone(),
                identity.root_directory_fingerprint().unwrap().clone(),
                identity.last_validated_root().clone(),
            ),
        ),
        selector(root.path()).path().clone(),
    )
    .unwrap();
    let before = files(&root.path().join(".podway"));
    let error = show_workspace_v1(
        &resolver,
        DirectWorkspaceReadRequestV1 {
            selector: fenced,
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            deadline: deadline(),
        },
        &NoEffects,
    )
    .unwrap_err();
    assert!(
        matches!(error,DirectWorkspaceErrorV1::Resolution(DirectWorkspaceResolutionErrorV1::Resolution(
        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {expected,..})) if expected==wrong)
    );
    assert_eq!(files(&root.path().join(".podway")), before);
}
struct Clock(AtomicUsize);
impl ExecutionClockV1 for Clock {
    fn now(&self) -> UnixMillis {
        self.0.fetch_add(1, Ordering::SeqCst);
        UnixMillis::new(1_700_000_000_000)
    }
}
pub(super) struct NoEffects;
impl ExecutionClockV1 for NoEffects {
    fn now(&self) -> UnixMillis {
        panic!("rejection/replay read clock")
    }
}
impl DirectIdSourceV1 for NoEffects {
    fn next_workspace_id(&self) -> WorkspaceId {
        panic!("rejection/replay generated UUID")
    }
    fn next_session_id(&self) -> SessionId {
        panic!("unexpected session UUID")
    }
    fn next_attempt_id(&self) -> AttemptId {
        panic!("unexpected attempt UUID")
    }
    fn next_blocker_id(&self) -> BlockerId {
        panic!("unexpected blocker UUID")
    }
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        panic!("unexpected snapshot UUID")
    }
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
        panic!("unexpected receipt UUID")
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}
pub(super) fn selector(path: &std::path::Path) -> WorktreeSelectorV1 {
    // Temporary roots can have symlinked parents, including macOS /var.
    let path = std::fs::canonicalize(path).expect("canonical fixture worktree");
    WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            path.as_os_str().as_bytes(),
            DiagnosticPathDisplayV1::new(path.to_string_lossy()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}
pub(super) fn fixture() -> TempDir {
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    root
}
pub(super) fn mutation(path: &std::path::Path, key: &str) -> DirectWorkspaceMutationRequestV1 {
    DirectWorkspaceMutationRequestV1 {
        selector: selector(path),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        deadline: deadline(),
    }
}
fn read(path: &std::path::Path) -> DirectWorkspaceReadRequestV1 {
    DirectWorkspaceReadRequestV1 {
        selector: selector(path),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(DIFFERENT).unwrap(),
        deadline: deadline(),
    }
}

#[test]
fn workspace_fixture_resolves_a_symlinked_temporary_parent() {
    let parent = tempfile::tempdir().unwrap();
    let root = fixture();
    let alias = parent.path().join("temporary-parent");
    std::os::unix::fs::symlink(root.path().parent().unwrap(), &alias).unwrap();
    let selected = alias.join(root.path().file_name().unwrap());
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let output = initialize_workspace_v1(
        &resolver,
        mutation(&selected, "init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let shown = show_workspace_v1(&resolver, read(&selected), &Clock(AtomicUsize::new(0))).unwrap();
    assert_eq!(
        shown.as_value()["workspace"],
        output.as_value()["workspace"]
    );
    assert!(root.path().join(".podway/runtime/state.sqlite3").is_file());
}
fn files(root: &std::path::Path) -> BTreeMap<String, String> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        result: &mut BTreeMap<String, String>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
#[test]
fn init_show_doctor_and_repair_validate_and_replay_without_runtime_or_read_writes() {
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    assert!(matches!(
        show_workspace_v1(&resolver, read(root.path()), &NoEffects),
        Err(DirectWorkspaceErrorV1::Resolution(_))
    ));
    let initial = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        initial.as_value()
    ));
    assert_eq!(
        initial.as_value()["workspace"]["latest_workspace_sequence"],
        1
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    assert!(
        !root
            .path()
            .join(".podway/runtime/maintenance-intent.json")
            .exists()
    );
    assert!(
        !std::fs::read_to_string(root.path().join(".podway/config.yaml"))
            .unwrap()
            .contains("job_queue")
    );
    let mut replay = mutation(root.path(), "init");
    replay.request_id = RequestIdV1::new(DIFFERENT).unwrap();
    assert_eq!(
        initialize_workspace_v1(&resolver, replay, false, &NoEffects, &NoEffects).unwrap(),
        initial
    );
    assert!(matches!(
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "init"),
            true,
            &NoEffects,
            &NoEffects
        ),
        Err(DirectWorkspaceErrorV1::Store(
            DirectStoreErrorV1::IdempotencyKeyReused
        ))
    ));
    let before = files(&root.path().join(".podway"));
    let show = show_workspace_v1(&resolver, read(root.path()), &clock).unwrap();
    assert_eq!(show.as_value()["result"]["root_rebind_required"], false);
    for deep in [false, true] {
        let result = doctor_workspace_v1(&resolver, read(root.path()), deep, &clock).unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            result.as_value()
        ));
        assert_eq!(result.as_value()["result"]["healthy"], true);
    }
    assert_eq!(
        files(&root.path().join(".podway")),
        before,
        "read commands changed runtime bytes"
    );
    let repair = repair_workspace_v1(&resolver, mutation(root.path(), "repair"), &clock).unwrap();
    assert_eq!(repair.as_value()["result"]["changed"], false);
    assert_eq!(
        repair.as_value()["workspace"]["latest_workspace_sequence"],
        2
    );
    assert_eq!(
        repair_workspace_v1(&resolver, mutation(root.path(), "repair"), &NoEffects).unwrap(),
        repair
    );
    let mut fenced = read(root.path());
    fenced.expected_workspace_id = Some(WorkspaceId::new(DIFFERENT).unwrap());
    assert!(matches!(
        show_workspace_v1(&resolver, fenced, &NoEffects),
        Err(DirectWorkspaceErrorV1::Resolution(_))
    ));
    let foreign = DirectWorkspaceResolverV1::for_v15_fixture(
        NativeGitResolverV1::new(),
        Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
    );
    assert!(matches!(
        doctor_workspace_v1(&foreign, read(root.path()), true, &NoEffects),
        Err(DirectWorkspaceErrorV1::Resolution(_))
    ));
}
#[test]
fn existing_plain_init_refuses_new_keys_but_preserves_original_replay_and_repair() {
    use podway_engine::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let original = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let before = files(&root.path().join(".podway"));
    let rejected = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "second-init"),
        false,
        &NoEffects,
        &NoEffects,
    );
    assert!(matches!(
        rejected,
        Err(DirectWorkspaceErrorV1::Resolution(
            DirectWorkspaceResolutionErrorV1::Resolution(
                WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent
            )
        ))
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    let (_, store) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    store
        .read(deadline(), |snapshot| {
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("second-init").unwrap())?
                    .is_none()
            );
            assert_eq!(snapshot.committed_sequence()?, 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "init"),
            false,
            &NoEffects,
            &NoEffects,
        )
        .unwrap(),
        original
    );
    let repair = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "repair-init"),
        true,
        &Clock(AtomicUsize::new(0)),
        &NoEffects,
    )
    .unwrap();
    assert_eq!(
        repair.as_value()["workspace"]["uuid"],
        original.as_value()["workspace"]["uuid"]
    );
    assert_eq!(
        repair.as_value()["workspace"]["latest_workspace_sequence"],
        2
    );
    assert_eq!(
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "repair-init"),
            true,
            &NoEffects,
            &NoEffects,
        )
        .unwrap(),
        repair
    );
}

#[test]
fn plain_init_refuses_an_outcome_pruned_before_its_authoritative_lookup() {
    use podway_engine::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    use podway_store::direct_store::DirectTransactionBoundaryV1;
    use std::sync::{Arc, Mutex};
    const CHILD_ROOT: &str = "PODWAY_V2DFC006_RETENTION_RACE_CHILD_ROOT";
    const TEST: &str = "int_v2dfc006_workspace::plain_init_refuses_an_outcome_pruned_before_its_authoritative_lookup";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let resolver =
            DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
        repair_workspace_v1(
            &resolver,
            mutation(std::path::Path::new(&root), "prune-original-outcome"),
            &Clock(AtomicUsize::new(0)),
        )
        .unwrap();
        return;
    }
    // Both an exact original init and a conflicting repair key can disappear
    // after the preliminary lookup. Neither may become a new plain init.
    for conflicting in [false, true] {
        let root = fixture();
        let resolver =
            DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "init"),
            false,
            &Clock(AtomicUsize::new(0)),
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        let key = if conflicting {
            repair_workspace_v1(
                &resolver,
                mutation(root.path(), "original-repair"),
                &Clock(AtomicUsize::new(0)),
            )
            .unwrap();
            "original-repair"
        } else {
            "init"
        };
        for index in 0..99 {
            repair_workspace_v1(
                &resolver,
                mutation(root.path(), &format!("retained-repair-{index}")),
                &Clock(AtomicUsize::new(0)),
            )
            .unwrap();
        }
        let (_, store) = resolver
            .resolve_existing(selector(root.path()), None, deadline())
            .unwrap();
        store
            .read(deadline(), |snapshot| {
                assert!(
                    snapshot
                        .outcome(&IdempotencyKeyV1::new(key).unwrap())?
                        .is_some()
                );
                Ok(())
            })
            .unwrap();
        let root_path = root.path().to_path_buf();
        let after_pruning = Arc::new(Mutex::new(None));
        let captured = after_pruning.clone();
        let racing =
            DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest())
                .with_store_boundary_observer_v1(move |boundary| {
                    if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                        let child = Command::new(std::env::current_exe().unwrap())
                            .args(["--exact", TEST, "--nocapture"])
                            .env(CHILD_ROOT, &root_path)
                            .output()
                            .unwrap();
                        assert!(child.status.success(), "{child:?}");
                        let mut preserved = files(&root_path.join(".podway"));
                        // BEGIN/rollback may change only SQLite's volatile index.
                        preserved.remove("runtime/state.sqlite3-shm");
                        *captured.lock().unwrap() = Some(preserved);
                    }
                });
        let rejected = initialize_workspace_v1(
            &racing,
            mutation(root.path(), key),
            false,
            &NoEffects,
            &NoEffects,
        );
        assert!(matches!(
            rejected,
            Err(DirectWorkspaceErrorV1::Resolution(
                DirectWorkspaceResolutionErrorV1::Resolution(
                    WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent
                )
            ))
        ));
        let mut after = files(&root.path().join(".podway"));
        after.remove("runtime/state.sqlite3-shm");
        assert_eq!(Some(after), *after_pruning.lock().unwrap());
        store
            .read(deadline(), |snapshot| {
                assert!(
                    snapshot
                        .outcome(&IdempotencyKeyV1::new(key).unwrap())?
                        .is_none()
                );
                assert_eq!(snapshot.committed_sequence()?, 101 + u64::from(conflicting));
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn moved_root_is_reported_without_write_and_rebound_only_by_repair_commit() {
    let parent = fixture();
    let old = parent.path().join("old");
    assert!(
        Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "Fixture"
            ])
            .current_dir(parent.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&old)
            .arg("HEAD")
            .current_dir(parent.path())
            .status()
            .unwrap()
            .success()
    );
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(&old, "init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let new = parent.path().join("new");
    assert!(
        Command::new("git")
            .args(["worktree", "move"])
            .arg(&old)
            .arg(&new)
            .current_dir(parent.path())
            .status()
            .unwrap()
            .success()
    );
    let before = files(&new.join(".podway"));
    let show = show_workspace_v1(&resolver, read(&new), &clock).unwrap();
    assert_eq!(show.as_value()["result"]["root_rebind_required"], true);
    assert_eq!(files(&new.join(".podway")), before);
    let repair = repair_workspace_v1(&resolver, mutation(&new, "repair"), &clock).unwrap();
    assert_eq!(repair.as_value()["result"]["changed"], true);
    assert_eq!(
        show_workspace_v1(&resolver, read(&new), &clock)
            .unwrap()
            .as_value()["result"]["root_rebind_required"],
        false
    );
    assert_eq!(
        repair_workspace_v1(&resolver, mutation(&new, "repair"), &NoEffects).unwrap(),
        repair
    );
}

#[test]
fn init_crash_child() {
    use podway_engine::direct_workspace::initialize_workspace_with_boundary_observer_v1;
    use podway_store::direct_store::DirectTransactionBoundaryV1;
    let Some(path) = std::env::var_os("PODWAY_WORKSPACE_INIT_FIXTURE") else {
        return;
    };
    let phase = std::env::var("PODWAY_WORKSPACE_INIT_PHASE").unwrap();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    initialize_workspace_with_boundary_observer_v1(
        &resolver,
        mutation(std::path::Path::new(&path), "crashed-init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
        move |boundary| {
            let selected = match phase.as_str() {
                "before_begin" => DirectTransactionBoundaryV1::BeforeBegin,
                "before_outcome" => DirectTransactionBoundaryV1::AfterStateBeforeOutcome,
                "before_publication" => DirectTransactionBoundaryV1::BeforeMaintenancePublication,
                "after_publication" => DirectTransactionBoundaryV1::AfterCommitBeforeOutput,
                _ => panic!("unknown owned crash phase"),
            };
            if boundary == selected {
                std::process::exit(77);
            }
        },
    )
    .unwrap();
    panic!("owned crash boundary was not observed");
}

#[test]
fn exact_init_retry_resumes_private_or_published_generation_and_other_requests_fail_closed() {
    use podway_engine::workspace::DirectWorkspaceResolutionErrorV1;
    for phase in [
        "before_begin",
        "before_outcome",
        "before_publication",
        "after_publication",
    ] {
        let root = fixture();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "int_v2dfc006_workspace::init_crash_child",
                "--nocapture",
            ])
            .env("PODWAY_WORKSPACE_INIT_FIXTURE", root.path())
            .env("PODWAY_WORKSPACE_INIT_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(77),
            "{phase}: actual Engine crash child did not stop at boundary"
        );
        let resolver =
            DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
        assert!(
            matches!(
                show_workspace_v1(&resolver, read(root.path()), &NoEffects),
                Err(DirectWorkspaceErrorV1::Resolution(
                    DirectWorkspaceResolutionErrorV1::Store(DirectStoreErrorV1::Maintenance)
                ))
            ),
            "{phase}"
        );
        assert!(
            matches!(
                initialize_workspace_v1(
                    &resolver,
                    mutation(root.path(), "another-init"),
                    false,
                    &NoEffects,
                    &NoEffects
                ),
                Err(DirectWorkspaceErrorV1::Store(
                    DirectStoreErrorV1::Maintenance
                ))
            ),
            "{phase}"
        );
        let resumed = initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "crashed-init"),
            false,
            &NoEffects,
            &NoEffects,
        )
        .unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            resumed.as_value()
        ));
        assert_eq!(
            resumed.as_value()["workspace"]["latest_workspace_sequence"],
            1
        );
        assert_eq!(
            initialize_workspace_v1(
                &resolver,
                mutation(root.path(), "crashed-init"),
                false,
                &NoEffects,
                &NoEffects
            )
            .unwrap(),
            resumed
        );
        doctor_workspace_v1(
            &resolver,
            read(root.path()),
            true,
            &Clock(AtomicUsize::new(0)),
        )
        .unwrap();
    }
}

#[test]
fn deep_doctor_detects_changed_required_artifact_without_writing_or_reading_clock() {
    use podway_core::{ItemId, Revision};
    use podway_engine::{
        direct_execution::{DirectBeginRequestV1, begin_session_v1},
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        direct_start::{DirectStartCommandV1, DirectStartRequestV1, start_session_v1},
        execution::prepare_workspace_procedure_source_v2,
    };
    use podway_protocol::{
        ItemAttachSourceV1, ItemAttachV1, ItemMutationPreconditionsWireV1, SessionBeginV1,
        SessionIdentityPreconditionsWireV1, SessionStartCommandV1, SessionStartSourceV1,
        SessionStartV1,
    };
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let source = br#"{"schema":"podway.procedure/v2","id":"diagnostic","version":"1","name":"Diagnostic","purpose":"Check real artifact diagnostics.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Attach evidence.","items":[{"id":"report","type":"artifact","prompt":"Attach report.","required":true}]}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;
    std::fs::write(root.path().join("procedure.json"), source).unwrap();
    std::fs::write(root.path().join("report.txt"), "Original report").unwrap();
    let prepared = prepare_workspace_procedure_source_v2("procedure.json", source).unwrap();
    let start = start_session_v1(
        &resolver,
        DirectStartRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("start").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartCommandV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "procedure.json".into(),
                    },
                    expected_procedure_digest: Some(prepared.digest().clone()),
                    task_title: "Diagnose current evidence".into(),
                    dry_run: false,
                },
                preconditions: None,
            }),
        },
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let session = SessionId::new(start.as_value()["session"]["id"].as_str().unwrap()).unwrap();
    let begin = begin_session_v1(
        &resolver,
        DirectBeginRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("begin").unwrap(),
            deadline: deadline(),
            input: SessionBeginV1 {
                initial_goal: None,
                preconditions: SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session.clone(),
                    expected_session_revision: Revision::ZERO,
                },
            },
        },
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let attempt = AttemptId::new(
        begin.as_value()["result"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    execute_item_v1(
        &resolver,
        DirectItemRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("attach").unwrap(),
            deadline: deadline(),
            command: DirectItemCommandV1::Attach(ItemAttachV1 {
                item_id: ItemId::new("report").unwrap(),
                source: ItemAttachSourceV1::Path {
                    path: "report.txt".into(),
                    media_type: None,
                },
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: session,
                    expected_attempt_id: attempt,
                    expected_item_revision: Revision::ZERO,
                },
            }),
        },
        &clock,
    )
    .unwrap();
    let healthy = doctor_workspace_v1(&resolver, read(root.path()), true, &clock).unwrap();
    assert_eq!(
        healthy.as_value()["result"]["current_required_local_artifacts_checked"],
        1
    );
    std::fs::write(root.path().join("report.txt"), "Changed report").unwrap();
    let before = files(&root.path().join(".podway"));
    assert!(matches!(
        doctor_workspace_v1(&resolver, read(root.path()), true, &NoEffects),
        Err(DirectWorkspaceErrorV1::Boundary(_))
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    doctor_workspace_v1(&resolver, read(root.path()), false, &clock).unwrap();
}

#[test]
fn direct_config_admission_rejects_nonprod_and_invalid_bytes_before_store_or_effects() {
    use podway_engine::direct_api::{
        DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1,
    };
    use podway_protocol::Rfc3339MillisV1;
    let root = fixture();
    let base = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    initialize_workspace_v1(
        &base,
        mutation(root.path(), "config-init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let opens = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = opens.clone();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest())
        .with_store_boundary_observer_v1(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        });
    let config = root.path().join(".podway/config.yaml");
    for bytes in [
        b"schema: podway.workspace/v2\nmode: dev\n".to_vec(),
        b"schema: podway.workspace/v2\nmode: release-qualification\n".to_vec(),
        b"schema: invalid\n".to_vec(),
        vec![0xff],
        vec![b'x'; podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1 + 1],
    ] {
        std::fs::write(&config, bytes).unwrap();
        let before = files(&root.path().join(".podway"));
        for (index, command) in [
            DirectCommandV1::WorkspaceShow,
            DirectCommandV1::WorkspaceRepair,
            DirectCommandV1::WorkspaceInit { repair: false },
        ]
        .into_iter()
        .enumerate()
        {
            let response = execute_direct_v1(
                &resolver,
                DirectRequestV1 {
                    request_id: RequestIdV1::new(RESPONSE).unwrap(),
                    selector: selector(root.path()),
                    expected_workspace_id: None,
                    command,
                    idempotency_key: Some(
                        IdempotencyKeyV1::new(format!("invalid-config-{index}")).unwrap(),
                    ),
                    deadline: deadline(),
                    observed_at: Rfc3339MillisV1::new("2026-10-07T00:00:00.000Z").unwrap(),
                    timeout_ms: 5000,
                    generation: podway_protocol::ContractGenerationV1::Generation1,
                    return_observation: false,
                },
                &NoEffects,
                &NoEffects,
            )
            .unwrap();
            let DirectResponseV1::Failure(error) = response else {
                panic!("invalid config admitted")
            };
            let value = error.as_value();
            assert_eq!(value["code"], "WORKSPACE_CONFIG_INVALID");
            assert_eq!(value["exit_code"], 5);
            assert!(
                value["details"]["guidance"]
                    .as_str()
                    .unwrap()
                    .contains("Remove")
            );
            assert_eq!(files(&root.path().join(".podway")), before);
            assert_eq!(
                opens.load(Ordering::SeqCst),
                0,
                "invalid config opened Store before rejection"
            );
        }
    }
    let outside = tempfile::tempdir().unwrap();
    let outside_config = outside.path().join("config.yaml");
    std::fs::write(&outside_config, b"schema: podway.workspace/v1\n").unwrap();
    std::fs::remove_file(&config).unwrap();
    std::os::unix::fs::symlink(&outside_config, &config).unwrap();
    assert!(matches!(
        resolver.resolve_existing(selector(root.path()), None, deadline()),
        Err(
            podway_engine::workspace::DirectWorkspaceResolutionErrorV1::Resolution(
                podway_engine::workspace::WorkspaceResolutionErrorV1::WorkspaceConfigInvalid
            )
        )
    ));
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(&outside_config).unwrap(),
        b"schema: podway.workspace/v1\n"
    );
    std::fs::remove_file(&config).unwrap();
    for bytes in [
        b"schema: podway.workspace/v1\njob_queue: {max_pending: 1}\n".as_slice(),
        b"schema: podway.workspace/v2\n",
        b"schema: podway.workspace/v2\nmode: prod\n",
    ] {
        std::fs::write(&config, bytes).unwrap();
        show_workspace_v1(
            &base,
            DirectWorkspaceReadRequestV1 {
                selector: selector(root.path()),
                expected_workspace_id: None,
                request_id: RequestIdV1::new(RESPONSE).unwrap(),
                deadline: deadline(),
            },
            &Clock(AtomicUsize::new(0)),
        )
        .unwrap();
    }
    std::fs::write(&config, b"schema: podway.workspace/v2\nmode: dev\n").unwrap();
    let request = podway_engine::direct_maintenance::DirectMaintenanceRequestV1 {
        selector: selector(root.path()),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new("invalid-config-reset").unwrap(),
        confirmed: true,
        force: false,
        deadline: deadline(),
    };
    podway_engine::direct_maintenance::reset_all_v1(
        &base,
        request,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let request = podway_engine::direct_maintenance::DirectMaintenanceRequestV1 {
        selector: selector(root.path()),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new("invalid-config-remove").unwrap(),
        confirmed: true,
        force: true,
        deadline: deadline(),
    };
    podway_engine::direct_maintenance::remove_workspace_v1(
        &base,
        request,
        &Clock(AtomicUsize::new(0)),
    )
    .unwrap();
    assert!(!root.path().join(".podway").exists());
}

#[test]
fn bootstrap_rejects_existing_nonprod_config_without_creating_runtime_or_store() {
    let root = fixture();
    std::fs::create_dir(root.path().join(".podway")).unwrap();
    let config = root.path().join(".podway/config.yaml");
    std::fs::write(&config, b"schema: podway.workspace/v2\nmode: dev\n").unwrap();
    let before = files(&root.path().join(".podway"));
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let error = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "invalid-bootstrap"),
        false,
        &NoEffects,
        &NoEffects,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DirectWorkspaceErrorV1::Resolution(
            podway_engine::workspace::DirectWorkspaceResolutionErrorV1::Resolution(
                podway_engine::workspace::WorkspaceResolutionErrorV1::WorkspaceConfigInvalid
            )
        )
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    assert!(!root.path().join(".podway/runtime").exists());
}

/// STATE-002: the doctor reuses the verifier's admitted current graph in both
/// modes. Each run performs exactly one current-graph admission and one
/// disposition load; the deep run still rehashes its required artifacts from
/// the admitted graph before the clock, and the shallow run skips them.
#[test]
fn state002_doctor_reuses_verifier_admission_deep_and_non_deep() {
    use super::int_v2dfc006_domain_ports::state002_executes;
    use podway_core::{ItemId, Revision};
    use podway_engine::{
        direct_execution::{DirectBeginRequestV1, begin_session_v1},
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        direct_start::{DirectStartCommandV1, DirectStartRequestV1, start_session_v1},
        execution::prepare_workspace_procedure_source_v2,
    };
    use podway_protocol::{
        ItemAttachSourceV1, ItemAttachV1, ItemMutationPreconditionsWireV1, SessionBeginV1,
        SessionIdentityPreconditionsWireV1, SessionStartCommandV1, SessionStartSourceV1,
        SessionStartV1,
    };
    use podway_store::test_support::query_account::QueryAccountant;
    // Fixture: one current running session with one required local artifact
    // and no archived sessions, from the deep-doctor family.
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "state002-init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let source = br#"{"schema":"podway.procedure/v2","id":"diagnostic-state002","version":"1","name":"Diagnostic","purpose":"Check shared admitted state.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Attach evidence.","items":[{"id":"report","type":"artifact","prompt":"Attach report.","required":true}]}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;
    std::fs::write(root.path().join("procedure.json"), source).unwrap();
    std::fs::write(root.path().join("report.txt"), "Original report").unwrap();
    let prepared = prepare_workspace_procedure_source_v2("procedure.json", source).unwrap();
    let start = start_session_v1(
        &resolver,
        DirectStartRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("state002-start").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartCommandV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "procedure.json".into(),
                    },
                    expected_procedure_digest: Some(prepared.digest().clone()),
                    task_title: "Diagnose shared admitted state".into(),
                    dry_run: false,
                },
                preconditions: None,
            }),
        },
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let session = SessionId::new(start.as_value()["session"]["id"].as_str().unwrap()).unwrap();
    let begin = begin_session_v1(
        &resolver,
        DirectBeginRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("state002-begin").unwrap(),
            deadline: deadline(),
            input: SessionBeginV1 {
                initial_goal: None,
                preconditions: SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session.clone(),
                    expected_session_revision: Revision::ZERO,
                },
            },
        },
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let attempt = AttemptId::new(
        begin.as_value()["result"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    execute_item_v1(
        &resolver,
        DirectItemRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("state002-attach").unwrap(),
            deadline: deadline(),
            command: DirectItemCommandV1::Attach(ItemAttachV1 {
                item_id: ItemId::new("report").unwrap(),
                source: ItemAttachSourceV1::Path {
                    path: "report.txt".into(),
                    media_type: None,
                },
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: session,
                    expected_attempt_id: attempt,
                    expected_item_revision: Revision::ZERO,
                },
            }),
        },
        &clock,
    )
    .unwrap();
    let check_status = |value: &serde_json::Value, id: &str| -> serde_json::Value {
        value["result"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("missing doctor check {id}"))["status"]
            .clone()
    };
    for deep in [false, true] {
        let accountant = QueryAccountant::start();
        let healthy = doctor_workspace_v1(&resolver, read(root.path()), deep, &clock).unwrap();
        let report = accountant.finish();
        let value = healthy.as_value();
        assert_eq!(value["result"]["healthy"], true);
        assert_eq!(value["result"]["deep"], deep);
        assert_eq!(
            state002_executes(
                &report.sql,
                "SELECT COUNT(*) FROM v2_task_sessions WHERE activity = 'current'"
            ),
            1,
            "doctor ({deep}) must admit the current graph exactly once"
        );
        assert_eq!(
            state002_executes(&report.sql, "FROM v2_terminal_dispositions"),
            1,
            "doctor ({deep}) must load dispositions exactly once"
        );
        if deep {
            assert_eq!(
                value["result"]["current_required_local_artifacts_checked"],
                1
            );
            assert_eq!(
                check_status(value, "current_required_local_artifacts"),
                "pass"
            );
        } else {
            assert_eq!(
                value["result"]["current_required_local_artifacts_checked"],
                0
            );
            assert_eq!(
                check_status(value, "current_required_local_artifacts"),
                "skipped"
            );
        }
    }
}
