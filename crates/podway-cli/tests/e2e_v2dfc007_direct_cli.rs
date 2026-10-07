//! Retained commands execute through the real CLI and worktree-local direct Store.

use std::{
    fs,
    io::Read,
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::cli_fixture::{Fixture, GuardedChild, podway_binary};
use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::{Value, json};

#[test]
fn interrupted_removal_rejects_changed_workspace_fences_without_consuming_the_intent() {
    let fixture = Fixture::new();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    let workspace = initialized["workspace"]["uuid"].as_str().unwrap();
    let args = [
        "workspace",
        "remove",
        "--force",
        "--yes",
        "--if-workspace-uuid",
        workspace,
        "--idempotency-key",
        "remove",
    ];
    fixture.kill_at_maintenance_boundary(&args, "marker-published");
    let before = fixture.podway_snapshot();
    let wrong = uuid::Uuid::new_v4().to_string();
    let output = fixture.output(&[
        "workspace",
        "remove",
        "--force",
        "--yes",
        "--if-workspace-uuid",
        &wrong,
        "--idempotency-key",
        "remove",
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "WORKSPACE_UUID_MISMATCH");
    assert_eq!(
        error.as_value()["details"]["expected_workspace_uuid"],
        wrong
    );
    assert_eq!(
        error.as_value()["details"]["actual_workspace_uuid"],
        workspace
    );
    fixture.assert_podway_snapshot(&before);
    assert_eq!(fixture.run(&args)["command"], "workspace.remove");
    assert!(!fixture.0.join(".podway").exists());
}

#[test]
fn reset_all_preserves_the_explicit_original_workspace_fence_and_exact_replay() {
    let fixture = Fixture::new();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    let workspace = initialized["workspace"]["uuid"].as_str().unwrap();
    fixture.run(&["workspace", "show"]);
    let before = fixture.podway_snapshot();
    let wrong = uuid::Uuid::new_v4().to_string();
    let output = fixture.output(&[
        "reset",
        "--all",
        "--yes",
        "--if-workspace-uuid",
        &wrong,
        "--idempotency-key",
        "wrong-reset",
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "WORKSPACE_UUID_MISMATCH");
    assert_eq!(
        error.as_value()["details"]["expected_workspace_uuid"],
        wrong
    );
    assert_eq!(
        error.as_value()["details"]["actual_workspace_uuid"],
        workspace
    );
    fixture.assert_podway_snapshot(&before);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "wrong-reset"])["result"]["found"],
        false
    );
    let args = [
        "reset",
        "--all",
        "--yes",
        "--if-workspace-uuid",
        workspace,
        "--idempotency-key",
        "reset",
    ];
    let reset = fixture.run(&args);
    assert_eq!(reset["command"], "workspace.reset_all");
    assert_ne!(reset["workspace"]["uuid"], workspace);
    assert_eq!(fixture.run(&args), reset);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "reset"])["result"]["outcome"]["response"],
        reset
    );
    assert_eq!(
        fixture.run(&["doctor", "--deep"])["result"]["healthy"],
        true
    );
}

#[test]
fn every_legacy_generation_refuses_without_new_files_then_explicitly_resets() {
    use sha2::{Digest, Sha256};
    let recipe: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/storage/daemon-era-v2dfc/generation-1-to-13-ddl.json"
    ))
    .unwrap();
    let migrations = recipe["migrations"].as_array().unwrap();
    assert_eq!(migrations.len(), 13);
    for (index, migration) in migrations.iter().enumerate() {
        assert_eq!(migration["generation"], index + 1);
        assert_eq!(
            migration["sha256"],
            format!(
                "sha256:{:x}",
                Sha256::digest(migration["sql_utf8"].as_str().unwrap().as_bytes())
            )
        );
    }
    for generation in 1..=13 {
        for checkpointed in [false, true] {
            let fixture = Fixture::new();
            fixture.run(&["init", "--idempotency-key", "init"]);
            let runtime = fixture.0.join(".podway/runtime");
            fs::remove_dir_all(&runtime).unwrap();
            fs::create_dir(&runtime).unwrap();
            let staging = fixture.0.join("legacy-producer");
            fs::create_dir(&staging).unwrap();
            let connection = rusqlite::Connection::open(staging.join("state.sqlite3")).unwrap();
            connection
                .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            for migration in &migrations[..generation] {
                connection
                    .execute_batch(migration["sql_utf8"].as_str().unwrap())
                    .unwrap();
            }
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, usize>(0))
                    .unwrap(),
                generation
            );
            if checkpointed {
                connection
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .unwrap();
            }
            connection.execute_batch("CREATE TABLE v2dfc001_hot_wal (value INTEGER NOT NULL); INSERT INTO v2dfc001_hot_wal VALUES (1), (2), (3);").unwrap();
            for suffix in ["", "-wal"] {
                let bytes = fs::read(staging.join(format!("state.sqlite3{suffix}"))).unwrap();
                let destination = runtime.join(format!("state.sqlite3{suffix}"));
                fs::write(&destination, &bytes).unwrap();
                assert_eq!(fs::read(destination).unwrap(), bytes);
            }
            drop(connection);
            let before = fixture.podway_snapshot();
            for args in [
                vec!["status"],
                vec!["doctor"],
                vec!["init", "--idempotency-key", "rejected-init"],
                vec![
                    "set",
                    "scope-summary",
                    "must not write",
                    "--idempotency-key",
                    "rejected-set",
                ],
            ] {
                let output = fixture.output(&args);
                assert_eq!(
                    output.status.code(),
                    Some(5),
                    "generation {generation}, checkpointed {checkpointed}: {output:?}"
                );
                let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(error.as_value()["code"], "LEGACY_RUNTIME_STATE_UNSUPPORTED");
                assert_eq!(
                    error.as_value()["details"]["store_schema_version"],
                    generation
                );
                fixture.assert_podway_snapshot(&before);
            }
            let reset = fixture.run(&["reset", "--all", "--yes", "--idempotency-key", "reset"]);
            assert_eq!(
                fixture.run(&["reset", "--all", "--yes", "--idempotency-key", "reset"]),
                reset
            );
            assert_eq!(
                fixture.run(&["doctor", "--deep"])["result"]["healthy"],
                true
            );
        }
    }
}

#[test]
fn incompatible_writer_identity_refuses_concurrent_reads_mutations_and_old_replays() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let database = fixture.0.join(".podway/runtime/state.sqlite3");
    // The owned fixture represents a Store written by an undeclared contract.
    let connection = rusqlite::Connection::open(&database).unwrap();
    let foreign_digest = format!("sha256:{}", "f".repeat(64));
    connection
        .execute(
            "UPDATE writer_contract SET contract_manifest_digest=?1 WHERE singleton=1",
            [&foreign_digest],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(connection);
    let before = fixture.podway_snapshot();
    let outputs = thread::scope(|scope| {
        let read = scope.spawn(|| fixture.output(&["status"]));
        let mutation =
            scope.spawn(|| fixture.output(&["init", "--idempotency-key", "rejected-init"]));
        [
            (read.join().unwrap(), "session.status"),
            (mutation.join().unwrap(), "workspace.init"),
        ]
    });
    for (output, command) in outputs.into_iter().chain([
        (
            fixture.output(&["init", "--idempotency-key", "init"]),
            "workspace.init",
        ),
        (fixture.output(&["doctor", "--deep"]), "workspace.doctor"),
        (
            fixture.output(&["outcome", "lookup", "--idempotency-key", "init"]),
            "outcome.lookup",
        ),
        (
            fixture.output(&["complete", "--idempotency-key", "rejected-complete"]),
            "session.complete",
        ),
    ]) {
        assert_eq!(output.status.code(), Some(5), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_CONTRACT_MISMATCH");
        assert_eq!(error.as_value()["command"], command);
        assert_eq!(
            error.as_value()["details"]["actual_contract_manifest_digest"],
            foreign_digest
        );
        assert_ne!(
            error.as_value()["details"]["expected_contract_manifest_digest"],
            foreign_digest
        );
        fixture.assert_podway_snapshot(&before);
    }
}

#[test]
fn delegation_stdin_acceptance_receipt_and_completion_preserve_the_real_attempt() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(
        fixture.0.join("delegated.yaml"),
        include_str!("../../../tests/fixtures/v2/procedures/delegated-native-routes.yaml"),
    )
    .unwrap();
    let preview = fixture.run(&["procedure", "preview", "delegated.yaml"]);
    fixture.run(&[
        "start",
        "--procedure",
        "delegated.yaml",
        "--expect-procedure-digest",
        preview["result"]["procedure_digest"].as_str().unwrap(),
        "--task",
        "delegated flow",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let package = fixture.run(&["delegation", "package"]);
    assert_eq!(
        package["result"]["schema"],
        "podway.delegation-package-result/v1"
    );
    fixture.run(&["check", "done", "--idempotency-key", "check"]);
    let before = fixture.run(&["status"]);
    let status = &before["result"];
    let session = status["session"]["id"].as_str().unwrap();
    let attempt = status["current"]["attempt"]["attempt_id"].as_str().unwrap();
    let workspace = before["workspace"]["uuid"].as_str().unwrap();
    let explicit_package = fixture.run(&[
        "delegation",
        "package",
        "--if-workspace-uuid",
        workspace,
        "--if-session-id",
        session,
    ]);
    let mut current_package = package["result"].clone();
    current_package["session_revision"] = status["session"]["revision"].clone();
    assert_eq!(explicit_package["result"], current_package);
    assert_eq!(explicit_package["result"]["session_id"], session);
    let receipt_args = [
        "delegation",
        "receipt",
        "--session",
        session,
        "--attempt",
        attempt,
    ];
    let missing = fixture.output(&receipt_args);
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&missing.stdout)
            .unwrap()
            .as_value()["code"],
        "DELEGATION_RECEIPT_REQUIRED"
    );
    let missing = fixture.output(&["complete", "--idempotency-key", "missing-receipt"]);
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&missing.stdout)
            .unwrap()
            .as_value()["code"],
        "DELEGATION_RECEIPT_REQUIRED"
    );
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    let digest = |letter: char| format!("sha256:{}", letter.to_string().repeat(64));
    let input = json!({"schema":"podway.delegation-accept-input/v1",
        "workspace_uuid":before["workspace"]["uuid"],"session_id":session,
        "session_revision":status["session"]["revision"],"attempt_id":attempt,"idempotency_key":"accept",
        "package_digest":package["result"]["package_digest"],"execution":{"binding_ref":"local-specialist",
            "adapter_id":"fixture","adapter_version":"1","primary_host":"external","scope_kind":"test",
            "scope_ref":"scope-1","supervisor_ref":"supervisor-1","worker_ref":"worker-1","task_ref":"task-1",
            "role_ref":"reviewer","role_snapshot_digest":digest('a'),"agent_configuration_digest":digest('b'),
            "accepted_task_digest":digest('c')},"rounds_used":3,"result_digest":digest('d'),
        "summary":"The final result was reviewed.","worker_disposition":"idle","accepted":true});
    let mut invalid = input.clone();
    invalid["idempotency_key"] = json!("wrong-role");
    invalid["execution"]["role_ref"] = json!("other-role");
    let output = fixture.output_with_stdin(
        &["delegation", "accept", "--stdin"],
        &serde_json::to_vec(&invalid).unwrap(),
    );
    assert!(!output.status.success(), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "DELEGATION_BINDING_MISMATCH");
    assert_eq!(error.as_value()["details"]["reason"], "role_mismatch");
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "wrong-role"])["result"]["found"],
        false
    );
    let bytes = serde_json::to_vec(&input).unwrap();
    for (body, code) in [
        (vec![b' '; 32 * 1024 + 1], "REQUEST_TOO_LARGE"),
        (vec![b'x'; 32 * 1024], "REQUEST_INVALID"),
        (b"not-json".to_vec(), "REQUEST_INVALID"),
    ] {
        let output = fixture.output_with_stdin(&["delegation", "accept", "--stdin"], &body);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
                .unwrap()
                .as_value()["code"],
            code
        );
    }
    let mut missing_summary = input.clone();
    missing_summary.as_object_mut().unwrap().remove("summary");
    let mut invalid_bodies = vec![missing_summary];
    for field in ["session_revision", "rounds_used"] {
        let mut invalid = input.clone();
        invalid[field] = json!(1.0);
        invalid_bodies.push(invalid);
    }
    for invalid in invalid_bodies {
        let output = fixture.output_with_stdin(
            &["delegation", "accept", "--stdin"],
            &serde_json::to_vec(&invalid).unwrap(),
        );
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(
            serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
                .unwrap()
                .as_value()["code"],
            "REQUEST_INVALID"
        );
    }
    let output = fixture.output_with_stdin(
        &[
            "--if-session-id",
            session,
            "delegation",
            "accept",
            "--stdin",
        ],
        &bytes,
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
            .unwrap()
            .as_value()["code"],
        "REQUEST_INVALID"
    );
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "accept"])["result"]["found"],
        false
    );
    let output = fixture.output_with_stdin(&["delegation", "accept", "--stdin"], &bytes);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let original = serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(original["command"], "delegation.accept");
    assert_eq!(
        original["result"]["schema"],
        "podway.delegation-accept-result/v2"
    );
    let output = fixture.output_with_stdin(&["delegation", "accept", "--stdin"], &bytes);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
            .unwrap()
            .into_value(),
        original
    );
    let receipt = fixture.run(&receipt_args);
    assert_eq!(receipt["command"], "delegation.receipt");
    assert_eq!(receipt["result"]["applicability"], "accepted");
    assert_eq!(receipt["result"]["reason"], "current_receipt");
    let explicit_receipt = fixture.run(&[
        "delegation",
        "receipt",
        "--session",
        session,
        "--attempt",
        attempt,
        "--if-workspace-uuid",
        workspace,
    ]);
    assert_eq!(explicit_receipt["result"], receipt["result"]);
    assert_eq!(
        receipt["result"]["receipt"]["execution"],
        input["execution"]
    );
    for field in [
        "package_digest",
        "rounds_used",
        "result_digest",
        "summary",
        "worker_disposition",
    ] {
        assert_eq!(receipt["result"]["receipt"][field], input[field], "{field}");
    }
    let output = fixture.output(&[
        "--if-session-id",
        session,
        "delegation",
        "receipt",
        "--session",
        session,
        "--attempt",
        attempt,
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
            .unwrap()
            .as_value()["code"],
        "REQUEST_INVALID"
    );
    let completed = fixture.run(&["complete", "--idempotency-key", "complete"]);
    assert_eq!(completed["result"]["session_state"], "completed");
    let historical = fixture.run(&receipt_args);
    assert_eq!(historical["result"]["applicability"], "historical");
    assert_eq!(historical["result"]["reason"], "historical_attempt");
    assert_eq!(
        historical["result"]["receipt"],
        receipt["result"]["receipt"]
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "accept"])["result"]["outcome"]["response"],
        original
    );
}

#[test]
fn goal_begin_define_assess_decide_and_completed_revision_preserve_fences_and_replay() {
    for (initial_goal, explicit_revision) in [(true, false), (false, true)] {
        let fixture = Fixture::new();
        fixture.run(&["init", "--idempotency-key", "init"]);
        fs::write(
            fixture.0.join("goal.yaml"),
            include_str!("../../../tests/fixtures/v2/procedures/goal-outcomes.yaml").replace(
                "    options:\n",
                concat!(
                    "    items:\n",
                    "      - id: assessment-note\n",
                    "        type: text\n",
                    "        prompt: Record local criterion evidence.\n",
                    "        required: false\n",
                    "        max_length: 100\n",
                    "    options:\n",
                ),
            ),
        )
        .unwrap();
        let preview = fixture.run(&["procedure", "preview", "goal.yaml"]);
        let prepared = fixture.run(&[
            "start",
            "--procedure",
            "goal.yaml",
            "--expect-procedure-digest",
            preview["result"]["procedure_digest"].as_str().unwrap(),
            "--task",
            "goal flow",
            "--idempotency-key",
            "start",
        ]);
        let criteria = [
            "--goal",
            "Verify the retained goal.",
            "--criterion",
            "correct=The result is correct.",
            "--criterion",
            "tested=The result is tested.",
            "--actor",
            "owner",
        ];
        let mut begin = vec!["begin", "--idempotency-key", "begin"];
        if initial_goal {
            begin.extend_from_slice(&criteria);
        }
        let original_begin = fixture.run(&begin);
        begin.extend_from_slice(&[
            "--if-workspace-uuid",
            prepared["workspace"]["uuid"].as_str().unwrap(),
            "--if-session-id",
            prepared["session"]["id"].as_str().unwrap(),
            "--if-session-revision",
            "0",
        ]);
        assert_eq!(fixture.run(&begin), original_begin);
        if !initial_goal {
            let mut define = vec!["goal", "define", "--idempotency-key", "define"];
            define.extend_from_slice(&criteria);
            let before_define = fixture.run(&["status"]);
            let revision = before_define["result"]["session"]["revision"]
                .as_u64()
                .unwrap()
                .to_string();
            define.extend_from_slice(&[
                "--if-workspace-uuid",
                before_define["workspace"]["uuid"].as_str().unwrap(),
                "--if-session-id",
                before_define["result"]["session"]["id"].as_str().unwrap(),
                "--if-session-revision",
                &revision,
            ]);
            let original = fixture.run(&define);
            assert_eq!(fixture.run(&define), original);
        }
        assert_eq!(fixture.run(&["status"])["result"]["goal_revision"], 1);
        fixture.run(&[
            "set",
            "result",
            "Evidence supports this result.",
            "--idempotency-key",
            "evidence",
        ]);
        fixture.run(&["complete", "--idempotency-key", "complete-work"]);
        fixture.run(&[
            "set",
            "assessment-note",
            "Decision-local evidence.",
            "--idempotency-key",
            "assessment-note",
        ]);
        let before_assessment = fixture.run(&["status"]);
        let missing = fixture.output(&[
            "decide",
            "--option",
            "achieved",
            "--reason",
            "Needs criterion results.",
            "--idempotency-key",
            "missing-results",
        ]);
        assert_eq!(missing.status.code(), Some(1), "{missing:?}");
        assert_eq!(
            serde_json::from_slice::<ErrorEnvelopeV2>(&missing.stdout)
                .unwrap()
                .as_value()["code"],
            "CRITERION_RESULT_MISSING"
        );
        assert_eq!(
            fixture.run(&["status"])["result"],
            before_assessment["result"]
        );
        for criterion in ["correct", "tested"] {
            let assessment = fixture.run(&[
                "goal",
                "assess-criterion",
                criterion,
                "--status",
                "satisfied",
                "--reason",
                "The retained evidence meets this criterion.",
                "--evidence",
                "perform",
                "--item",
                "assessment-note",
                "--actor",
                "reviewer",
                "--idempotency-key",
                criterion,
            ]);
            assert_eq!(assessment["result"]["result"]["criterion_id"], criterion);
            assert_eq!(
                assessment["result"]["result"]["citations"],
                json!([
                    {"reference_graph_node_id":"perform"},
                    {"local_item_id":"assessment-note"}
                ])
            );
        }
        let decide = [
            "decide",
            "--option",
            "achieved",
            "--reason",
            "Both recorded criteria are satisfied.",
            "--actor",
            "owner",
            "--idempotency-key",
            "decide",
        ];
        let original_decide = fixture.run(&decide);
        assert_eq!(original_decide["result"]["record"]["actor"], "owner");
        // Original fences are explicit for replay after the cursor has advanced.
        let old_status = &before_assessment["result"];
        let revision = original_decide["result"]["record"]["session_revision"]
            .as_u64()
            .unwrap()
            - 1;
        let revision = revision.to_string();
        let mut replay = decide.to_vec();
        replay.extend_from_slice(&[
            "--if-workspace-uuid",
            before_assessment["workspace"]["uuid"].as_str().unwrap(),
            "--if-session-id",
            old_status["session"]["id"].as_str().unwrap(),
            "--if-session-revision",
            &revision,
            "--if-attempt",
            old_status["current"]["attempt"]["attempt_id"]
                .as_str()
                .unwrap(),
            "--if-goal-revision",
            "1",
        ]);
        assert_eq!(fixture.run(&replay), original_decide);
        fixture.run(&["complete", "--idempotency-key", "finish"]);
        let terminal = fixture.run(&["status"]);
        assert_eq!(terminal["result"]["session"]["lifecycle"], "completed");
        assert!(terminal["result"]["current"].is_null());
        let mut revise = vec![
            "goal",
            "revise",
            "--goal",
            "Revise the retained goal.",
            "--criterion",
            "correct=The revised result is correct.",
            "--criterion",
            "tested=The revised result is tested.",
            "--rework-to",
            "perform",
            "--reason",
            "The required outcome changed.",
            "--reactivate",
            "--actor",
            "owner",
            "--idempotency-key",
            "revise",
        ];
        let terminal_revision = terminal["result"]["session"]["revision"]
            .as_u64()
            .unwrap()
            .to_string();
        if explicit_revision {
            revise.extend_from_slice(&[
                "--if-workspace-uuid",
                terminal["workspace"]["uuid"].as_str().unwrap(),
                "--if-session-id",
                terminal["result"]["session"]["id"].as_str().unwrap(),
                "--if-session-revision",
                &terminal_revision,
                "--if-goal-revision",
                "1",
            ]);
        }
        let original = fixture.run(&revise);
        assert_eq!(original["result"]["actor"], "owner");
        let active = fixture.run(&["status"]);
        assert_eq!(active["result"]["goal_revision"], 2);
        assert_eq!(active["result"]["session"]["lifecycle"], "running");
        assert_eq!(
            active["result"]["current"]["node"]["graph_node_id"],
            "perform"
        );
        if !explicit_revision {
            revise.extend_from_slice(&[
                "--if-workspace-uuid",
                terminal["workspace"]["uuid"].as_str().unwrap(),
                "--if-session-id",
                terminal["result"]["session"]["id"].as_str().unwrap(),
                "--if-session-revision",
                &terminal_revision,
                "--if-goal-revision",
                "1",
            ]);
        }
        assert_eq!(fixture.run(&revise), original);
        assert_eq!(fixture.run(&["status"])["result"], active["result"]);
        let history = fixture.run(&["status", "--verbose"]);
        let old_goal = &history["result"]["stale_goal_revision_history"]["entries"][0];
        assert_eq!(old_goal["revision"], 1);
        assert_eq!(old_goal["actor"], "owner");
        assert_eq!(old_goal["statement"], "Verify the retained goal.");
    }
}

#[test]
fn record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(fixture.0.join("batch.yaml"), concat!(
        "schema: podway.procedure/v2\nid: batch\nversion: '1'\nname: Atomic batch\npurpose: Record typed values atomically.\n",
        "node_definitions:\n  work:\n    type: action\n    title: Work\n    intent: Record the batch.\n",
        "    items:\n      - id: alpha\n        type: integer\n        prompt: Record a number.\n        required: false\n",
        "      - id: zeta\n        type: text\n        prompt: Record text.\n        required: false\n",
        "graph:\n  entry: work\n  nodes:\n    - id: work\n      use: work\n      terminal: true\n"
    )).unwrap();
    let preview = fixture.run(&["procedure", "preview", "batch.yaml"]);
    let digest = preview["result"]["procedure_digest"].as_str().unwrap();
    fixture.run(&[
        "start",
        "--procedure",
        "batch.yaml",
        "--expect-procedure-digest",
        digest,
        "--task",
        "batch identity",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "set",
        "zeta",
        "clear this text",
        "--idempotency-key",
        "seed-zeta",
    ]);
    let before = fixture.run(&["status"]);
    let status = &before["result"];
    let zeta = status["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == "zeta")
        .unwrap();
    let input = json!({
        "schema":"podway.item-record-many-input/v1",
        "workspace_uuid":before["workspace"]["uuid"],
        "session_id":status["session"]["id"],
        "session_revision":status["session"]["revision"],
        "attempt_id":status["current"]["attempt"]["attempt_id"],
        "idempotency_key":"record-batch",
        "operations":[
            {"item_id":"zeta","expected_item_revision":zeta["revision"],"clear":true},
            {"item_id":"alpha","expected_item_revision":0,"record":{"type":"integer","value":42}}
        ]
    });
    let bytes = serde_json::to_vec(&input).unwrap();
    let output = fixture.output_with_stdin(&["record", "--stdin"], &bytes);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let response = serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(response["command"], "item.record_many");
    assert_eq!(response["result"]["items"][0]["item_id"], "alpha");
    assert_eq!(response["result"]["items"][1]["item_id"], "zeta");
    let replay = fixture.output_with_stdin(&["record", "--stdin"], &bytes);
    assert_eq!(replay.status.code(), Some(0), "{replay:?}");
    assert_eq!(
        serde_json::from_slice::<OutputEnvelopeV4>(&replay.stdout)
            .unwrap()
            .into_value(),
        response
    );
    let after = fixture.run(&["status"]);
    let observed = fixture.run(&["observe"]);
    let items = observed["result"]["active_items"].as_array().unwrap();
    let alpha = items
        .iter()
        .find(|item| item["item_id"] == "alpha")
        .unwrap();
    let zeta = items.iter().find(|item| item["item_id"] == "zeta").unwrap();
    assert_eq!(alpha["value"], 42);
    assert_eq!(alpha["revision"], 1);
    assert_eq!(zeta["value"], Value::Null);
    assert_eq!(zeta["revision"], 2);
    assert_eq!(
        after["result"]["session"]["revision"].as_u64().unwrap(),
        status["session"]["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "record-batch"])["result"]["outcome"]
            ["response"],
        response
    );
    let mut stale = input;
    stale["idempotency_key"] = json!("rejected-batch");
    let output =
        fixture.output_with_stdin(&["record", "--stdin"], &serde_json::to_vec(&stale).unwrap());
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "SESSION_REVISION_CONFLICT");
    assert_eq!(fixture.run(&["status"])["result"], after["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "rejected-batch"])["result"]["found"],
        false
    );
    stale["idempotency_key"] = json!("rejected-item-fence");
    stale["session_revision"] = after["result"]["session"]["revision"].clone();
    stale["operations"] = json!([
        {"item_id":"alpha","expected_item_revision":1,"record":{"type":"integer","value":43}},
        {"item_id":"zeta","expected_item_revision":0,"clear":true}
    ]);
    let output =
        fixture.output_with_stdin(&["record", "--stdin"], &serde_json::to_vec(&stale).unwrap());
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "ITEM_REVISION_CONFLICT");
    assert_eq!(fixture.run(&["observe"])["result"], observed["result"]);
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "rejected-item-fence"
        ])["result"]["found"],
        false
    );
}

#[test]
fn populated_legacy_hot_wal_and_markers_fail_closed_then_reset_with_a_caller_key() {
    use std::os::unix::fs::MetadataExt;
    const RECIPE: &str = include_str!(
        "../../../tests/fixtures/storage/daemon-era-v2dfc/generation-13-populated.sql"
    );
    const MARKERS: &str =
        include_str!("../../../tests/fixtures/storage/daemon-era-v2dfc/markers.json");
    for checkpointed in [false, true] {
        for selected in 0..6 {
            let fixture = Fixture::new();
            fixture.run(&["init", "--idempotency-key", "init"]);
            let runtime = fixture.0.join(".podway/runtime");
            fs::remove_dir_all(&runtime).unwrap();
            fs::create_dir(&runtime).unwrap();
            let staging = fixture.0.join("legacy-producer");
            fs::create_dir(&staging).unwrap();
            let connection = rusqlite::Connection::open(staging.join("state.sqlite3")).unwrap();
            connection
                .execute_batch("PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            connection.execute_batch(RECIPE).unwrap();
            for table in ["jobs", "v2_delegation_receipts", "v2_task_sessions"] {
                let count: u64 = connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(count, 1);
            }
            if checkpointed {
                connection
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .unwrap();
            }
            connection
                .execute_batch("UPDATE workspace_state SET updated_at_ms=8 WHERE singleton=1;")
                .unwrap();
            let mut paths = Vec::new();
            for suffix in ["", "-wal"] {
                let source = staging.join(format!("state.sqlite3{suffix}"));
                let bytes = fs::read(&source).unwrap();
                let target = runtime.join(format!("state.sqlite3{suffix}"));
                fs::write(&target, &bytes).unwrap();
                assert_eq!(fs::read(&target).unwrap(), bytes);
                paths.push(target);
            }
            drop(connection);
            let markers: Vec<Value> = serde_json::from_str(MARKERS).unwrap();
            for (index, marker) in markers.iter().enumerate() {
                if selected == index + 1 || selected == 5 {
                    let path = fixture.0.join(marker["relative_path"].as_str().unwrap());
                    fs::write(&path, marker["bytes_utf8"].as_str().unwrap()).unwrap();
                    paths.push(path);
                }
            }
            let before: Vec<_> = paths
                .iter()
                .map(|path| {
                    let metadata = fs::metadata(path).unwrap();
                    (fs::read(path).unwrap(), metadata.dev(), metadata.ino())
                })
                .collect();
            let mut recovery = None;
            let tree_before = fixture.podway_snapshot();
            for arguments in [
                vec!["status"],
                vec!["doctor"],
                vec!["init", "--idempotency-key", "rejected-init"],
                vec![
                    "set",
                    "scope-summary",
                    "must not write",
                    "--idempotency-key",
                    "rejected-set",
                ],
            ] {
                let output = fixture.output(&arguments);
                assert_eq!(output.status.code(), Some(5), "{arguments:?}: {output:?}");
                let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(error.as_value()["code"], "LEGACY_RUNTIME_STATE_UNSUPPORTED");
                assert_eq!(
                    error.as_value()["details"]["recovery"]["requires_explicit_authorization"],
                    true
                );
                recovery = Some(error.as_value()["details"]["recovery"].clone());
                for (path, (bytes, dev, ino)) in paths.iter().zip(&before) {
                    assert_eq!(&fs::read(path).unwrap(), bytes);
                    let metadata = fs::metadata(path).unwrap();
                    assert_eq!((metadata.dev(), metadata.ino()), (*dev, *ino));
                }
                assert!(!runtime.join("state.sqlite3-shm").exists());
                assert!(!runtime.join("maintenance.lock").exists());
                fixture.assert_podway_snapshot(&tree_before);
            }
            let recipe = recovery.unwrap();
            assert_eq!(
                recipe["argv"].as_array().unwrap().last().unwrap(),
                "<new-idempotency-key>"
            );
            let argv: Vec<_> = recipe["argv"].as_array().unwrap()[2..]
                .iter()
                .map(|value| match value.as_str().unwrap() {
                    "<new-idempotency-key>" => "caller-owned-reset",
                    other => other,
                })
                .collect();
            let reset = fixture.run(&argv);
            assert_eq!(reset["command"], "workspace.reset_all");
            assert_eq!(fixture.run(&argv), reset);
            assert_eq!(
                fixture.run(&["doctor", "--deep"])["result"]["healthy"],
                true
            );
            assert!(fixture.0.join(".git/config").exists());
        }
    }
}

#[test]
fn sigkill_after_commit_before_stdout_preserves_the_original_outcome_and_exact_replay() {
    use nix::{
        errno::Errno,
        fcntl::{FcntlArg, OFlag, fcntl},
        unistd::{pipe, write},
    };
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "output boundary",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let workspace = before["workspace"]["uuid"].as_str().unwrap();
    let session = before["result"]["session"]["id"].as_str().unwrap();
    let attempt = before["result"]["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap();
    let arguments = [
        "set",
        "scope-summary",
        "committed before output",
        "--idempotency-key",
        "lost-output",
        "--if-workspace-uuid",
        workspace,
        "--if-session-id",
        session,
        "--if-attempt",
        attempt,
        "--if-item-revision",
        "0",
    ];
    let (read_end, write_end) = pipe().unwrap();
    let original_flags = OFlag::from_bits_truncate(fcntl(&write_end, FcntlArg::F_GETFL).unwrap());
    fcntl(
        &write_end,
        FcntlArg::F_SETFL(original_flags | OFlag::O_NONBLOCK),
    )
    .unwrap();
    let mut prefilled = 0;
    loop {
        match write(&write_end, b"x") {
            Ok(1) => prefilled += 1,
            Err(Errno::EAGAIN) => break,
            other => panic!("unexpected pipe prefill result: {other:?}"),
        }
        assert!(
            prefilled < 1_048_576,
            "the pipe capacity must remain bounded"
        );
    }
    assert!(prefilled > 0);
    fcntl(&write_end, FcntlArg::F_SETFL(original_flags)).unwrap();
    let child = Command::new(podway_binary())
        .arg("--worktree")
        .arg(&fixture.0)
        .arg("--json")
        .args(arguments)
        .current_dir(&fixture.0)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::from(write_end))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = GuardedChild::new(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    let original = loop {
        assert!(
            guard.0.as_mut().unwrap().try_wait().unwrap().is_none(),
            "the mutation must remain blocked on stdout"
        );
        let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", "lost-output"]);
        if lookup["result"]["found"] == true {
            break lookup["result"]["outcome"]["response"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "the mutation must commit before stdout delivery"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert!(guard.0.as_mut().unwrap().try_wait().unwrap().is_none());
    guard.0.as_mut().unwrap().kill().unwrap();
    let output = guard.0.take().unwrap().wait_with_output().unwrap();
    assert_eq!(output.status.signal(), Some(9));
    assert!(output.stderr.is_empty());
    let mut pipe_bytes = Vec::new();
    fs::File::from(read_end)
        .take(1_048_577)
        .read_to_end(&mut pipe_bytes)
        .unwrap();
    assert_eq!(
        pipe_bytes,
        vec![b'x'; prefilled],
        "no mutation stdout byte may have been delivered"
    );
    assert_eq!(
        fixture.run(&arguments),
        original,
        "same-key retry must retain the entire original response"
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "lost-output"])["result"]["outcome"]
            ["response"],
        original
    );
    let after = fixture.run(&["status"]);
    assert_eq!(
        after["workspace"]["latest_workspace_sequence"],
        original["workspace"]["latest_workspace_sequence"]
    );
    assert_eq!(
        after["result"]["session"]["revision"].as_u64().unwrap(),
        before["result"]["session"]["revision"].as_u64().unwrap() + 1
    );
}

#[test]
fn an_absent_item_reaches_the_domain_error_without_creating_an_outcome() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "item lookup",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let output = fixture.output(&[
        "set",
        "prior-item",
        "value",
        "--idempotency-key",
        "missing-item",
    ]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "ITEM_NOT_FOUND");
    assert_eq!(error.as_value()["command"], "item.set");
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "missing-item"])["result"]["found"],
        false
    );
}

#[test]
fn published_maintenance_recipes_resume_the_real_cli_with_original_identity() {
    for removal in [false, true] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let workspace = init["workspace"]["uuid"].as_str().unwrap();
        let arguments = if removal {
            vec![
                "workspace",
                "remove",
                "--force",
                "--yes",
                "--if-workspace-uuid",
                workspace,
                "--idempotency-key",
                "original-maintenance-key",
            ]
        } else {
            vec![
                "reset",
                "--all",
                "--force",
                "--yes",
                "--idempotency-key",
                "original-maintenance-key",
            ]
        };
        fixture.kill_at_maintenance_boundary(&arguments, "marker-published");
        let observed = Command::new(podway_binary())
            .arg("--worktree")
            .arg(&fixture.0)
            .args(["--json", "status"])
            .current_dir(&fixture.0)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        assert_eq!(observed.status.code(), Some(4), "{observed:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&observed.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        let recipe = &error.as_value()["details"]["recovery"];
        assert_eq!(recipe["requires_explicit_authorization"], true);
        let argv = recipe["argv"].as_array().unwrap();
        assert_eq!(argv[0], "podway");
        assert_eq!(argv[1], "--json");
        let argv: Vec<_> = argv[2..]
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let resumed = fixture.run(&argv);
        assert_eq!(resumed["command"], recipe["command"]);
        if removal {
            assert!(!fixture.0.join(".podway").exists());
            assert!(fixture.0.join(".git/config").exists());
        } else {
            assert_eq!(fixture.run(&arguments), resumed);
            assert_eq!(
                fixture.run(&[
                    "outcome",
                    "lookup",
                    "--idempotency-key",
                    "original-maintenance-key"
                ])["result"]["outcome"]["response"],
                resumed
            );
        }
    }
}

#[test]
fn a_sqlite_writer_bounds_contention_without_recording_a_failed_outcome() {
    let fixture = Fixture::new();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "retain this session",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let writer =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    let output = fixture.output(&[
        "--timeout",
        "20ms",
        "set",
        "scope-summary",
        "must not commit",
        "--idempotency-key",
        "contended-sqlite",
    ]);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "WORKSPACE_BUSY");
    assert_eq!(error.as_value()["command"], "item.set");
    assert_eq!(error.as_value()["retryable"], true);
    writer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "contended-sqlite",])["result"]["found"],
        false
    );
    let arguments = [
        "set",
        "scope-summary",
        "after contention",
        "--if-workspace-uuid",
        initialized["workspace"]["uuid"].as_str().unwrap(),
        "--if-session-id",
        before["result"]["session"]["id"].as_str().unwrap(),
        "--if-attempt",
        before["result"]["current"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap(),
        "--if-item-revision",
        "0",
        "--idempotency-key",
        "contended-sqlite",
    ];
    let accepted = fixture.run(&arguments);
    assert_eq!(fixture.run(&arguments), accepted);
}

#[test]
fn a_competing_process_honors_the_deadline_and_attributes_preflight_failure_to_the_mutation() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "keep this session",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let paused = fixture.pause_at_maintenance_boundary(
        &[
            "reset",
            "--all",
            "--force",
            "--yes",
            "--idempotency-key",
            "paused-reset",
        ],
        "lease-acquired",
    );
    let started = std::time::Instant::now();
    let output = Command::new(podway_binary())
        .arg("--worktree")
        .arg(&fixture.0)
        .args([
            "--json",
            "--timeout",
            "20ms",
            "set",
            "scope-summary",
            "must not commit",
            "--idempotency-key",
            "contended",
        ])
        .current_dir(&fixture.0)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the invocation must not restart its lock budget"
    );
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
    assert_eq!(error.as_value()["details"]["boundary"], "maintenance_lock");
    assert_eq!(error.as_value()["details"]["timeout_ms"], 20);
    assert_eq!(
        error.as_value()["command"],
        "item.set",
        "internal status preflight must preserve the requested command identity"
    );
    assert_eq!(error.as_value()["retryable"], true);
    drop(paused);
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "contended"])["result"]["found"],
        false
    );
}

#[test]
fn plain_start_replays_exactly_and_enter_preserves_prepared_running_and_terminal_sessions() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let arguments = [
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "original",
        "--idempotency-key",
        "start",
    ];
    let first = fixture.run(&arguments);
    assert_eq!(
        fixture.run(&arguments),
        first,
        "plain start must reach outcome replay before existing-session decisions"
    );
    for expected in ["prepared", "running", "cancelled"] {
        match expected {
            "running" => {
                fixture.run(&["begin", "--idempotency-key", "begin"]);
            }
            "cancelled" => {
                fixture.run(&[
                    "cancel",
                    "--reason",
                    "retain for inspection",
                    "--idempotency-key",
                    "cancel",
                ]);
            }
            _ => {}
        }
        let before = fixture.run(&["status"]);
        assert_eq!(before["result"]["session"]["lifecycle"], expected);
        let rendered = fixture.continue_in_pty();
        assert!(
            rendered.contains("No new session was created"),
            "{rendered}"
        );
        assert!(
            rendered.contains(before["result"]["session"]["id"].as_str().unwrap()),
            "{rendered}"
        );
        assert_eq!(
            fixture.run(&["status"])["result"],
            before["result"],
            "Enter must not change the session, revision, attempt, or items"
        );
    }
}

#[test]
fn reset_all_resumes_after_sigkill_at_real_durable_boundaries() {
    for stage in [
        "intent-published",
        "marker-published",
        "runtime-cleared",
        "phase-advanced",
    ] {
        let fixture = Fixture::new();
        fixture.run(&["init", "--idempotency-key", "init"]);
        fixture.run(&[
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "retire this session",
            "--idempotency-key",
            "start",
        ]);
        let arguments = [
            "reset",
            "--all",
            "--force",
            "--yes",
            "--idempotency-key",
            "reset",
        ];
        fixture.kill_at_maintenance_boundary(&arguments, stage);
        let resumed = fixture.run(&arguments);
        assert_eq!(resumed["command"], "workspace.reset_all");
        assert_eq!(
            fixture.run(&arguments),
            resumed,
            "{stage}: reset replay must retain the original committed success"
        );
        assert_eq!(
            fixture.run(&["outcome", "lookup", "--idempotency-key", "reset"])["result"]["outcome"]
                ["response"],
            resumed
        );
        assert_eq!(
            fixture.run(&["archive", "list"])["result"]["sessions"],
            serde_json::json!([])
        );
    }
}

#[test]
fn removal_resumes_after_sigkill_through_final_marker_retirement() {
    for stage in [
        "marker-published",
        "runtime-cleared",
        "phase-advanced",
        "before-final-removal",
        "after-first-removal-marker",
        "after-last-removal-marker",
        "directories-removed",
    ] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let workspace = init["workspace"]["uuid"].as_str().unwrap();
        let git_config = fs::read(fixture.0.join(".git/config")).unwrap();
        fs::write(
            fixture.0.join(".podway/owned-note"),
            b"remove workspace content",
        )
        .unwrap();
        let arguments = [
            "workspace",
            "remove",
            "--force",
            "--yes",
            "--if-workspace-uuid",
            workspace,
            "--idempotency-key",
            "remove",
        ];
        fixture.kill_at_maintenance_boundary(&arguments, stage);
        let resumed = fixture.run(&arguments);
        assert_eq!(resumed["command"], "workspace.remove", "{stage}");
        assert!(!fixture.0.join(".podway").exists(), "{stage}");
        assert_eq!(
            fs::read(fixture.0.join(".git/config")).unwrap(),
            git_config,
            "{stage}: Git state must be preserved"
        );
    }
}

#[test]
fn retained_commands_commit_replay_lookup_and_reject_stale_fences_in_the_real_binary() {
    let fixture = Fixture::new();
    let init = fixture.run(&["init", "--idempotency-key", "init"]);
    assert_eq!(init["command"], "workspace.init");
    let start = fixture.run(&[
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "direct CLI",
        "--idempotency-key",
        "start",
    ]);
    assert_eq!(start["result"]["session_state"], "prepared");
    assert_eq!(
        fixture.run(&["begin", "--idempotency-key", "begin"])["command"],
        "session.begin"
    );
    let before = fixture.run(&["observe"]);
    let status = &before["result"]["status"];
    let session_id = status["session"]["id"].as_str().unwrap();
    let attempt_id = status["current"]["attempt"]["attempt_id"].as_str().unwrap();
    let workspace_id = init["workspace"]["uuid"].as_str().unwrap();
    let arguments = [
        "set",
        "scope-summary",
        "bounded direct behavior",
        "--if-workspace-uuid",
        workspace_id,
        "--if-session-id",
        session_id,
        "--if-attempt",
        attempt_id,
        "--if-item-revision",
        "0",
        "--idempotency-key",
        "record-scope",
    ];
    let first = fixture.run(&arguments);
    assert_eq!(first["command"], "item.set");
    assert_eq!(
        fixture.run(&arguments),
        first,
        "replay must preserve the entire original success"
    );
    let lookup = fixture.run(&[
        "outcome",
        "lookup",
        "--idempotency-key",
        "record-scope",
        "--if-workspace-uuid",
        workspace_id,
    ]);
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], first);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "never-recorded"])["result"]["found"],
        false
    );
    let output = Command::new(podway_binary())
        .arg("--worktree")
        .arg(&fixture.0)
        .args([
            "--json",
            "set",
            "scope-summary",
            "different value",
            "--if-session-id",
            session_id,
            "--if-attempt",
            attempt_id,
            "--if-item-revision",
            "0",
            "--idempotency-key",
            "stale",
        ])
        .current_dir(&fixture.0)
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "ITEM_REVISION_CONFLICT");
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "stale"])["result"]["found"],
        false
    );
    fixture.run(&["status", "--compact"]);
    fixture.run(&["complete", "--idempotency-key", "complete-inspect"]);
    assert_eq!(
        fixture.run(&["observe"])["result"]["status"]["current"]["node"]["graph_node_id"],
        "implement"
    );
    assert!(!fixture.0.join("podwayd.sock").exists());
}
