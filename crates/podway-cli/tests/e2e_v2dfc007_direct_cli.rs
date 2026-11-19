//! Retained commands execute through the real CLI and worktree-local direct Store.

use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::cli_fixture::{
    Fixture, GuardedChild, assert_read004_counters, output_with_stdin_counted, podway_binary,
    reads004_preflight_counters,
};
use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::{Value, json};

fn initialized_schema_generation(fixture: &Fixture) -> u32 {
    rusqlite::Connection::open_with_flags(
        fixture.0.join(".podway/runtime/state.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .pragma_query_value(None, "user_version", |row| row.get(0))
    .unwrap()
}

#[test]
fn prepared_cursor_and_item_mutations_report_session_not_running_without_effects() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "prepared errors",
        "--idempotency-key",
        "start",
    ]);
    let durable = || {
        let mut snapshot = fixture.podway_snapshot();
        snapshot.retain(|entry| entry.0 != std::path::Path::new("runtime/state.sqlite3-shm"));
        snapshot
    };
    let before = durable();
    for (command, arguments) in [
        ("session.complete", vec!["complete"]),
        ("session.retry", vec!["retry", "--reason", "repeat"]),
        ("session.block", vec!["block", "--reason", "blocked"]),
        ("session.unblock", vec!["unblock", "--all"]),
        ("session.skip", vec!["skip", "--reason", "skip"]),
        ("session.cancel", vec!["cancel", "--reason", "cancel"]),
        (
            "session.decide",
            vec!["decide", "--option", "continue", "--reason", "continue"],
        ),
        ("item.check", vec!["check", "verification"]),
        ("item.set", vec!["set", "scope-summary", "scope"]),
        (
            "goal.revise",
            vec![
                "goal",
                "revise",
                "--goal",
                "goal",
                "--criterion",
                "done=done",
                "--rework-to",
                "inspect",
                "--reason",
                "revised",
                "--basis-description",
                "fixture",
                "--basis-digest",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                "--feedback",
                "Record the actual changed goal basis.",
            ],
        ),
        (
            "goal.assess_criterion",
            vec![
                "goal",
                "assess-criterion",
                "done",
                "--status",
                "satisfied",
                "--reason",
                "done",
            ],
        ),
    ] {
        let mut args = arguments;
        args.extend(["--idempotency-key", command]);
        let output = fixture.output(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["command"], command);
        assert_eq!(error.as_value()["code"], "SESSION_NOT_RUNNING");
        assert_eq!(durable(), before);
        assert_eq!(
            fixture.run(&["outcome", "lookup", "--idempotency-key", command])["result"]["found"],
            false
        );
        assert_eq!(durable(), before);
    }
    for (arguments, code, exit) in [
        (
            vec![
                "complete",
                "--if-session-revision",
                "1",
                "--idempotency-key",
                "stale-prepared",
            ],
            "SESSION_REVISION_CONFLICT",
            4,
        ),
        (
            vec![
                "complete",
                "--if-session-id",
                "00000000-0000-4000-8000-000000000001",
                "--idempotency-key",
                "wrong-session",
            ],
            "SESSION_ID_MISMATCH",
            4,
        ),
        (
            vec![
                "complete",
                "--if-workspace-uuid",
                "00000000-0000-4000-8000-000000000001",
                "--idempotency-key",
                "wrong-workspace",
            ],
            "WORKSPACE_UUID_MISMATCH",
            4,
        ),
        (
            vec!["check", "!!!", "--idempotency-key", "invalid-item"],
            "REQUEST_INVALID",
            2,
        ),
        (
            vec![
                "unblock",
                "not-a-uuid",
                "--idempotency-key",
                "invalid-blocker",
            ],
            "REQUEST_INVALID",
            2,
        ),
        (
            vec!["complete", "--idempotency-key", ""],
            "REQUEST_INVALID",
            2,
        ),
        (vec!["complete"], "REQUEST_INVALID", 2),
    ] {
        let output = fixture.output(&arguments);
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{arguments:?}: {output:?}"
        );
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], code);
        assert_eq!(durable(), before);
    }
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "retry",
        "--reason",
        "repeat running attempt",
        "--idempotency-key",
        "retry-running",
    ]);
}

#[test]
fn v2dfc017_oversize_record_and_set_stdin_refuse_without_runtime_effects() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "sw-dev-v3",
        "--task",
        "stdin bounds",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.podway_snapshot();
    for arguments in [
        &["record", "--stdin"][..],
        &[
            "set",
            "unknown-item",
            "--stdin",
            "--idempotency-key",
            "oversize-set",
        ][..],
    ] {
        let output = fixture.output_with_stdin(arguments, &vec![b'a'; 1_048_577]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "REQUEST_TOO_LARGE");
        fixture.assert_podway_snapshot(&before);
    }
}

#[test]
fn v2dfc016_storeless_residual_contention_emits_a_working_read_only_recipe() {
    let fixture = Fixture::new();
    let init = fixture.run(&["init", "--idempotency-key", "init"]);
    let uuid = init["workspace"]["uuid"].as_str().unwrap();
    fixture.run(&[
        "workspace",
        "remove",
        "--force",
        "--yes",
        "--if-workspace-uuid",
        uuid,
        "--idempotency-key",
        "remove-initial",
    ]);
    let runtime = fixture.0.join(".podway/runtime");
    fs::create_dir_all(&runtime).unwrap();
    let holder = fs::File::open(&runtime).unwrap();
    holder.try_lock().unwrap();
    let args = [
        "--timeout",
        "20ms",
        "workspace",
        "remove",
        "--force",
        "--yes",
        "--if-workspace-uuid",
        uuid,
        "--idempotency-key",
        "remove-residual",
    ];
    let before = fixture.podway_snapshot();
    let started = Instant::now();
    let output = fixture.output(&args);
    assert!(started.elapsed() < Duration::from_secs(2));
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
    assert_eq!(error.as_value()["details"]["boundary"], "maintenance_lock");
    let recipe = &error.as_value()["details"]["recovery"];
    assert_eq!(recipe["action"], "diagnose_writer");
    assert_eq!(recipe["command"], "version");
    assert_eq!(recipe["requires_explicit_authorization"], false);
    assert_eq!(
        recipe["argv"],
        json!(["podway", "--json", "version", "--identity"])
    );
    let argv: Vec<&str> = recipe["argv"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|value| value.as_str().unwrap())
        .collect();
    let identity = fixture.raw_output(&argv);
    assert!(identity.status.success(), "{identity:?}");
    let identity: OutputEnvelopeV4 = serde_json::from_slice(&identity.stdout).unwrap();
    assert_eq!(identity.as_value()["command"], "version");
    fixture.assert_podway_snapshot(&before);
    drop(holder);
    let removed = fixture.run(&args);
    assert_eq!(removed["result"]["already_absent"], true);
    assert_eq!(removed["result"]["podway_directory_removed"], true);
    assert!(!fixture.0.join(".podway").exists());
}

#[test]
fn v2dfc016_human_removal_preserves_optional_force_and_recovery() {
    let fixture = Fixture::new();
    let init = fixture.run(&["init", "--idempotency-key", "init"]);
    let uuid = init["workspace"]["uuid"].as_str().unwrap();
    let args = [
        "workspace",
        "remove",
        "--yes",
        "--if-workspace-uuid",
        uuid,
        "--idempotency-key",
        "unforced-remove",
    ];
    let before = fixture.podway_snapshot();
    let rejected = fixture.output(&args);
    assert_eq!(rejected.status.code(), Some(2));
    fixture.assert_podway_snapshot(&before);
    let removed = fixture.raw_output(&args);
    assert!(removed.status.success(), "{:?}", removed);
    assert!(!fixture.0.join(".podway").exists());

    for stage in ["intent-published", "marker-published"] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let uuid = init["workspace"]["uuid"].as_str().unwrap();
        let args = [
            "workspace",
            "remove",
            "--yes",
            "--if-workspace-uuid",
            uuid,
            "--idempotency-key",
            "unforced-remove",
        ];
        let mut owner = fixture.pause_at_human_maintenance_boundary(&args, stage);
        owner.0.as_mut().unwrap().kill().unwrap();
        assert_eq!(owner.0.take().unwrap().wait().unwrap().signal(), Some(9));
        let before = fixture.podway_snapshot();
        let output = fixture.output(&["status"]);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        let recipe = &error.as_value()["details"]["recovery"];
        let mut expected = vec!["podway"];
        expected.extend(args);
        assert_eq!(recipe["argv"], json!(expected));
        fixture.assert_podway_snapshot(&before);
        let argv: Vec<&str> = recipe["argv"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|value| value.as_str().unwrap())
            .collect();
        let resumed = fixture.raw_output(&argv);
        assert!(resumed.status.success(), "{:?}", resumed);
        assert!(!fixture.0.join(".podway").exists());
    }
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let removed = fixture.removal_recipe_in_pty(&[
        "workspace",
        "remove",
        "--idempotency-key",
        "unfenced-unforced-remove",
    ]);
    let rendered = String::from_utf8(removed.stdout).unwrap();
    assert!(removed.status.success(), "{rendered}");
    assert_eq!(rendered.matches("podway-test-pty-exit:0").count(), 1);
    assert!(rendered.contains("Type that exact path to remove all .podway content: "));
    assert!(!fixture.0.join(".podway").exists());
}

#[test]
fn v2dfc016_interrupted_init_reports_intent_and_its_exact_working_recipe() {
    for repair in [false, true] {
        let fixture = Fixture::new();
        let key = "--original-init-key";
        let mut args = vec!["init"];
        if repair {
            args.push("--repair");
        }
        args.extend(["--idempotency-key", key]);
        fixture.kill_at_maintenance_boundary(&args, "before-store-publication");
        let marker_path = fixture.0.join(".podway/runtime/maintenance-intent.json");
        let original = fs::read(&marker_path).unwrap();
        let marker: Value = serde_json::from_slice(&original).unwrap();
        let before = fixture.podway_snapshot();
        let output = fixture.output(&["status"]);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(4));
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        assert_eq!(error.as_value()["details"]["boundary"], "intent_marker");
        let mut expected = vec!["podway", "--json"];
        expected.extend(args.iter().copied());
        assert_eq!(
            error.as_value()["details"]["recovery"]["argv"],
            json!(expected)
        );
        assert_eq!(fixture.podway_snapshot(), before);
        for foreign in ["writer", "root", "predecessor", "request-digest"] {
            let mut altered = marker.clone();
            match foreign {
                "writer" => {
                    altered["contract_manifest_digest"] =
                        json!(format!("sha256:{}", "b".repeat(64)))
                }
                "root" => {
                    altered["root_identity"]["common_dir_identity"] =
                        json!(format!("sha256:{}", "b".repeat(64)))
                }
                "predecessor" => altered["command"] = json!("workspace.migrate"),
                "request-digest" => {
                    altered["request_digest"] = json!(format!("sha256:{}", "b".repeat(64)))
                }
                _ => unreachable!(),
            }
            fs::write(&marker_path, serde_json::to_vec(&altered).unwrap()).unwrap();
            let before = fixture.podway_snapshot();
            let output = fixture.output(&["status"]);
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
            let (exit, code) = if foreign == "request-digest" {
                (5, "WORKSPACE_STATE_UNREADABLE")
            } else {
                (4, "WORKSPACE_MAINTENANCE")
            };
            assert_eq!(output.status.code(), Some(exit), "{foreign}");
            assert_eq!(error.as_value()["code"], code, "{foreign}");
            if foreign != "request-digest" {
                assert_eq!(error.as_value()["details"]["boundary"], "intent_marker");
            }
            let recipe = &error.as_value()["details"]["recovery"];
            assert_eq!(
                recipe["argv"],
                json!(["podway", "--json", "version", "--identity"])
            );
            let identity = Command::new(podway_binary())
                .args(["--json", "version", "--identity"])
                .current_dir(&fixture.0)
                .env_remove("HOME")
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .unwrap();
            assert!(identity.status.success(), "{foreign}");
            let identity: OutputEnvelopeV4 = serde_json::from_slice(&identity.stdout).unwrap();
            assert_eq!(identity.as_value()["command"], "version");
            assert_eq!(fixture.podway_snapshot(), before);
        }
        fs::write(marker_path, &original).unwrap();
        let mut mismatched = vec!["init"];
        if !repair {
            mismatched.push("--repair");
        }
        mismatched.extend(["--idempotency-key", key]);
        let before = fixture.podway_snapshot();
        let rejected = fixture.output(&mismatched);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&rejected.stdout).unwrap();
        assert_eq!(rejected.status.code(), Some(4));
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        assert_eq!(error.as_value()["details"]["boundary"], "intent_marker");
        assert_eq!(
            error.as_value()["details"]["recovery"]["argv"],
            json!(expected)
        );
        fixture.assert_podway_snapshot(&before);
        let result = fixture.run(&args);
        assert_eq!(result["workspace"]["uuid"], marker["workspace_uuid"]);
        assert_eq!(result["generated_at"], marker["created_at"]);
        assert_eq!(fixture.run(&args), result);
        assert!(
            !fixture
                .0
                .join(".podway/runtime/maintenance-intent.json")
                .exists()
        );
        assert_eq!(fixture.run(&["doctor"])["result"]["healthy"], true);
    }
}

#[test]
fn v2dfc016_intent_only_reset_and_removal_report_the_observed_boundary() {
    for removal in [false, true] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let uuid = init["workspace"]["uuid"].as_str().unwrap();
        let args = if removal {
            vec![
                "workspace",
                "remove",
                "--force",
                "--yes",
                "--if-workspace-uuid",
                uuid,
                "--idempotency-key",
                "--original-remove-key",
            ]
        } else {
            vec![
                "reset",
                "--all",
                "--yes",
                "--if-workspace-uuid",
                uuid,
                "--idempotency-key",
                "--original-reset-key",
            ]
        };
        fixture.kill_at_maintenance_boundary(&args, "intent-published");
        let before = fixture.podway_snapshot();
        let human = fixture.raw_output(&["status"]);
        assert_eq!(human.status.code(), Some(4));
        let rendered = String::from_utf8(human.stderr).unwrap();
        assert!(rendered.contains("reason: "), "{rendered}");
        assert!(rendered.contains("original idempotency key"), "{rendered}");
        let output = fixture.output(&["init", "--idempotency-key", "contender-init"]);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        assert_eq!(error.as_value()["details"]["boundary"], "intent_marker");
        let mut expected = vec!["podway", "--json"];
        expected.extend(args.iter().copied());
        assert_eq!(
            error.as_value()["details"]["recovery"]["argv"],
            json!(expected)
        );
        assert_eq!(fixture.podway_snapshot(), before);
        let config = fixture.0.join(".podway/config.yaml");
        let original = fs::read(&config).unwrap();
        fs::write(&config, b"invalid: [").unwrap();
        let output = fixture.output(&["init", "--idempotency-key", "contender-init"]);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        assert_eq!(error.as_value()["details"]["boundary"], "intent_marker");
        fs::write(config, original).unwrap();
        fixture.run(&args);
        if removal {
            assert!(!fixture.0.join(".podway").exists());
        } else {
            assert_ne!(
                fixture.run(&["workspace", "show"])["workspace"]["uuid"],
                init["workspace"]["uuid"]
            );
        }
    }
}

#[test]
fn v2dfc016_final_removal_blocks_native_contenders_without_recreating_locks() {
    for stage in ["after-first-removal-marker", "after-last-removal-marker"] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let uuid = init["workspace"]["uuid"].as_str().unwrap();
        let removal = [
            "workspace",
            "remove",
            "--force",
            "--yes",
            "--if-workspace-uuid",
            uuid,
            "--idempotency-key",
            "original-remove",
        ];
        let mut owner = fixture.pause_at_maintenance_boundary(&removal, stage);
        let before = fixture.podway_snapshot();
        for contender in [
            vec!["init", "--idempotency-key", "contender-init"],
            vec![
                "reset",
                "--all",
                "--yes",
                "--if-workspace-uuid",
                uuid,
                "--idempotency-key",
                "contender-reset",
            ],
            removal.to_vec(),
        ] {
            let mut args = vec!["--timeout", "100ms"];
            args.extend(contender);
            let output = fixture.output(&args);
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                error.as_value()["code"],
                "WORKSPACE_MAINTENANCE",
                "{stage}: {args:?}"
            );
            if stage == "after-last-removal-marker" {
                let recipe = &error.as_value()["details"]["recovery"];
                assert_eq!(recipe["action"], "diagnose_writer", "{args:?}");
                assert_eq!(recipe["command"], "version", "{args:?}");
                let argv: Vec<_> = recipe["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|part| part.as_str().unwrap())
                    .collect();
                let diagnosis = fixture.raw_output(&argv[1..]);
                assert!(diagnosis.status.success(), "{args:?}");
            }
            assert_eq!(fixture.podway_snapshot(), before, "{stage}: {args:?}");
        }
        owner.0.as_mut().unwrap().kill().unwrap();
        let killed = owner.0.take().unwrap().wait_with_output().unwrap();
        assert_eq!(killed.status.signal(), Some(9));
        assert!(killed.stdout.is_empty());
        fixture.run(&removal);
        assert!(!fixture.0.join(".podway").exists());
        let fresh = fixture.run(&["init", "--idempotency-key", "fresh-init"]);
        assert_ne!(fresh["workspace"]["uuid"], init["workspace"]["uuid"]);
        let before = fixture.podway_snapshot();
        let stale = fixture.output(&removal);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&stale.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_UUID_MISMATCH");
        assert_eq!(fixture.podway_snapshot(), before);
    }
}

#[test]
fn v2dfc016_previous_direct_generation_has_a_valid_refusal_and_reset_recipe() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let generation = initialized_schema_generation(&fixture) - 1;
    let connection =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    connection
        .execute_batch(&format!(
            "PRAGMA user_version={generation}; PRAGMA wal_checkpoint(TRUNCATE)"
        ))
        .unwrap();
    drop(connection);
    let before = fixture.podway_snapshot();
    let mut recovery = None;
    for args in [
        vec!["status"],
        vec!["doctor"],
        vec!["workspace", "show"],
        vec!["init", "--idempotency-key", "contender-init"],
    ] {
        let output = fixture.output(&args);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "LEGACY_RUNTIME_STATE_UNSUPPORTED");
        assert_eq!(
            error.as_value()["details"]["store_schema_version"],
            generation
        );
        assert_eq!(fixture.podway_snapshot(), before, "{args:?}");
        recovery = Some(error.as_value()["details"]["recovery"].clone());
    }
    let recovery = recovery.unwrap();
    let argv: Vec<_> = recovery["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| match part.as_str().unwrap() {
            "<new-idempotency-key>" => "previous-direct-reset",
            other => other,
        })
        .collect();
    let reset = fixture.raw_output(&argv[1..]);
    assert!(reset.status.success());
    assert_eq!(initialized_schema_generation(&fixture), generation + 1);
    fixture.run(&["doctor"]);
}

#[test]
fn v2dfc015_invalid_current_stores_require_unfenced_confirmed_recovery() {
    for defect in [
        "writer",
        "missing-table",
        "extra-table",
        "missing-row",
        "malformed-writer-row",
        "corrupt-page",
        "corrupt-current-data",
        "moved",
        "copied",
    ] {
        let mut fixture = Fixture::new();
        let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
        let old_uuid = initialized["workspace"]["uuid"].as_str().unwrap();
        let expected_code = match defect {
            "writer" => "WORKSPACE_CONTRACT_MISMATCH",
            "moved" | "copied" => "WORKSPACE_ID_CONFLICT",
            _ => "WORKSPACE_STATE_UNREADABLE",
        };
        match defect {
            "moved" => {
                let moved = fixture.0.with_extension("moved");
                fs::rename(&fixture.0, &moved).unwrap();
                fixture.0 = moved;
            }
            "copied" => {
                let target = Fixture::new();
                target.run(&["init", "--idempotency-key", "target-init"]);
                let source = fixture.0.join(".podway/runtime/state.sqlite3");
                fs::copy(source, target.0.join(".podway/runtime/state.sqlite3")).unwrap();
                fixture = target;
            }
            "corrupt-current-data" => corrupt_current_data_pages(&fixture),
            "malformed-writer-row" => corrupt_writer_row_type(&fixture),
            "corrupt-page" => {
                let path = fixture.0.join(".podway/runtime/state.sqlite3");
                let database = fs::OpenOptions::new().write(true).open(path).unwrap();
                // Retain the authentic SQLite/current-generation header while
                // truncating required pages, so qualification reads real corruption.
                database.set_len(100).unwrap();
            }
            _ => {
                let connection =
                    rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3"))
                        .unwrap();
                let sql = match defect {
                    "writer" => {
                        "UPDATE writer_contract SET contract_manifest_digest='sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'"
                    }
                    "missing-table" => "DROP TABLE writer_contract",
                    "extra-table" => "CREATE TABLE unrecognized(value INTEGER)",
                    "missing-row" => "DELETE FROM writer_contract",
                    _ => unreachable!(),
                };
                connection.execute_batch(sql).unwrap();
                connection
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                    .unwrap();
            }
        }
        let guidance = fixture.raw_output(&["status"]);
        let human = String::from_utf8(guidance.stderr).unwrap();
        assert!(human.contains("unfenced reset --all"), "{defect}: {human}");
        let refusal = fixture.output(&["status"]);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&refusal.stdout).unwrap();
        if expected_code != "WORKSPACE_ID_CONFLICT" {
            let argv = error.as_value()["details"]["recovery"]["argv"]
                .as_array()
                .unwrap();
            assert_eq!(
                argv,
                &vec![
                    json!("podway"),
                    json!("--json"),
                    json!("version"),
                    json!("--identity")
                ]
            );
            assert_eq!(
                fixture.run(&["version", "--identity"])["command"],
                "version"
            );
        }
        let before = fixture.podway_snapshot();
        for args in [
            vec![
                "reset",
                "--all",
                "--yes",
                "--if-workspace-uuid",
                old_uuid,
                "--idempotency-key",
                "recover",
            ],
            vec![
                "workspace",
                "remove",
                "--force",
                "--yes",
                "--if-workspace-uuid",
                old_uuid,
                "--idempotency-key",
                "remove",
            ],
        ] {
            let output = fixture.output(&args);
            assert!(!output.status.success(), "{defect}: {output:?}");
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(error.as_value()["code"], expected_code, "{defect}");
            fixture.assert_podway_snapshot(&before);
        }
        let reset_args = ["reset", "--all", "--yes", "--idempotency-key", "recover"];
        let recovered = fixture.run(&reset_args);
        assert_ne!(recovered["workspace"]["uuid"], old_uuid, "{defect}");
        assert_eq!(fixture.run(&reset_args), recovered, "{defect}");
        assert_eq!(
            fixture.run(&["outcome", "lookup", "--idempotency-key", "init"])["result"]["found"],
            false
        );
        assert_eq!(
            fixture.run(&["doctor", "--deep"])["result"]["healthy"],
            true
        );
    }
}

#[test]
fn v2dfc015_confirmed_unfenced_removal_discards_invalid_runtime() {
    for defect in [
        "copied",
        "writer",
        "missing-table",
        "missing-row",
        "malformed-writer-row",
        "corrupt-current-data",
    ] {
        let source = Fixture::new();
        let initialized = source.run(&["init", "--idempotency-key", "init"]);
        let fixture = Fixture::new();
        let target_initialized = fixture.run(&["init", "--idempotency-key", "target-init"]);
        if defect == "copied" {
            fs::copy(
                source.0.join(".podway/runtime/state.sqlite3"),
                fixture.0.join(".podway/runtime/state.sqlite3"),
            )
            .unwrap();
        } else if defect == "corrupt-current-data" {
            corrupt_current_data_pages(&fixture);
        } else if defect == "malformed-writer-row" {
            corrupt_writer_row_type(&fixture);
        } else {
            let connection =
                rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3"))
                    .unwrap();
            let sql = match defect {
                "writer" => {
                    "UPDATE writer_contract SET contract_manifest_digest='sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'"
                }
                "missing-table" => "DROP TABLE writer_contract",
                "missing-row" => "DELETE FROM writer_contract",
                _ => unreachable!(),
            };
            connection.execute_batch(sql).unwrap();
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
        }
        let before = fixture.podway_snapshot();
        let refused = fixture.output(&[
            "workspace",
            "remove",
            "--force",
            "--yes",
            "--if-workspace-uuid",
            if defect == "copied" {
                initialized["workspace"]["uuid"].as_str().unwrap()
            } else {
                target_initialized["workspace"]["uuid"].as_str().unwrap()
            },
            "--idempotency-key",
            "remove",
        ]);
        assert!(!refused.status.success());
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&refused.stdout).unwrap();
        let expected = match defect {
            "copied" => "WORKSPACE_ID_CONFLICT",
            "writer" => "WORKSPACE_CONTRACT_MISMATCH",
            _ => "WORKSPACE_STATE_UNREADABLE",
        };
        assert_eq!(error.as_value()["code"], expected, "{defect}");
        fixture.assert_podway_snapshot(&before);
        let removed = fixture.confirm_removal_in_pty();
        let rendered = String::from_utf8(removed.stdout).unwrap();
        assert_eq!(
            rendered.matches("podway-test-pty-exit:0").count(),
            1,
            "{defect}: {rendered}"
        );
        assert!(removed.status.success(), "{defect}: {rendered}");
        assert!(!fixture.0.join(".podway").exists());
        assert!(fixture.0.join(".git/config").exists());
        assert_eq!(
            source.run(&["workspace", "show"])["workspace"]["uuid"],
            initialized["workspace"]["uuid"]
        );
    }
}

fn corrupt_writer_row_type(fixture: &Fixture) {
    let connection =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    let canonical: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='writer_contract'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let relaxed = canonical.strip_suffix(" STRICT").unwrap();
    // This isolated corruption fixture restores the exact canonical STRICT
    // schema before admission, leaving a real persisted type-invalid row.
    connection
        .execute_batch("PRAGMA writable_schema=ON")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE sqlite_schema SET sql=?1 WHERE name='writer_contract'",
                [relaxed],
            )
            .unwrap(),
        1
    );
    connection
        .execute_batch(
            "PRAGMA writable_schema=RESET; UPDATE writer_contract SET recorded_at_ms='corrupt'; PRAGMA writable_schema=ON",
        )
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE sqlite_schema SET sql=?1 WHERE name='writer_contract'",
                [&canonical],
            )
            .unwrap(),
        1
    );
    connection
        .execute_batch("PRAGMA writable_schema=RESET; PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='writer_contract'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        canonical
    );
    assert!(matches!(
        connection.query_row("SELECT recorded_at_ms FROM writer_contract", [], |row| {
            row.get::<_, i64>(0)
        }),
        Err(rusqlite::Error::InvalidColumnType(..))
    ));
}

fn corrupt_current_data_pages(fixture: &Fixture) {
    let path = fixture.0.join(".podway/runtime/state.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let page_size: u64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap();
    let pages = connection.prepare(
        "SELECT rootpage FROM sqlite_schema WHERE tbl_name IN ('mutation_outcomes','successor_sessions') AND rootpage > 0",
    ).unwrap().query_map([], |row| row.get::<_, u64>(0)).unwrap()
        .collect::<Result<Vec<_>, _>>().unwrap();
    drop(connection);
    let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
    for page in pages {
        file.seek(SeekFrom::Start((page - 1) * page_size)).unwrap();
        file.write_all(&vec![0; usize::try_from(page_size).unwrap()])
            .unwrap();
    }
}

#[test]
fn v2dfc015_removal_confirmation_success_preserves_fence_policy() {
    for invalid in [false, true] {
        let fixture = Fixture::new();
        fixture.run(&["init", "--idempotency-key", "init"]);
        if invalid {
            corrupt_current_data_pages(&fixture);
        }
        let response = format!("{}\n", fixture.0.display());
        let output = fixture.removal_answer_in_pty(&response, true);
        let rendered = String::from_utf8(output.stdout).unwrap();
        assert!(output.status.success(), "{rendered}");
        assert_eq!(rendered.matches("podway-test-pty-exit:0").count(), 1);
        assert!(rendered.contains("Type that exact path to remove all .podway content: "));
        assert!(!fixture.0.join(".podway").exists());
        assert!(fixture.0.join(".git/config").exists());
    }
    let fixture = Fixture::new();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    let output = fixture.fenced_removal_in_pty(initialized["workspace"]["uuid"].as_str().unwrap());
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{rendered}");
    assert_eq!(rendered.matches("podway-test-pty-exit:0").count(), 1);
    assert!(!rendered.contains("Type that exact path to remove all .podway content: "));
    assert!(!fixture.0.join(".podway").exists());
    assert!(fixture.0.join(".git/config").exists());
}

#[test]
fn v2dfc015_unfenced_removal_rejects_a_wrong_path_even_with_yes() {
    for invalid in [false, true] {
        for yes in [false, true] {
            let fixture = Fixture::new();
            fixture.run(&["init", "--idempotency-key", "init"]);
            if invalid {
                corrupt_current_data_pages(&fixture);
            }
            let before = fixture.podway_snapshot();
            let wrong = format!("{}/\n", fixture.0.display());
            let output = fixture.removal_answer_in_pty(&wrong, yes);
            let rendered = String::from_utf8(output.stdout).unwrap();
            assert!(
                rendered.contains("explicit confirmation is required"),
                "{rendered}"
            );
            assert!(
                rendered.contains("--yes --if-workspace-uuid <uuid>"),
                "{rendered}"
            );
            assert!(rendered.contains("exact worktree path"), "{rendered}");
            assert!(rendered.contains("podway-test-pty-exit:2"), "{rendered}");
            fixture.assert_podway_snapshot(&before);
        }
    }
}

#[test]
fn v2dfc015_newer_wal_prefix_refuses_overbound_tail_and_markers() {
    use sha2::{Digest as _, Sha256};
    use std::os::unix::fs::MetadataExt;
    type ImageEntry = (std::path::PathBuf, u64, u64, u64, Option<[u8; 32]>);
    fn image(root: &std::path::Path) -> Vec<ImageEntry> {
        let mut pending = vec![root.to_path_buf()];
        let mut entries = Vec::new();
        while let Some(path) = pending.pop() {
            let metadata = fs::symlink_metadata(&path).unwrap();
            let digest = if metadata.is_file() {
                let mut file = fs::File::open(&path).unwrap();
                let mut hash = Sha256::new();
                let mut buffer = [0; 65_536];
                loop {
                    let count = file.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
                Some(hash.finalize().into())
            } else {
                assert!(metadata.is_dir());
                pending.extend(
                    fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                None
            };
            entries.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                metadata.dev(),
                metadata.ino(),
                metadata.len(),
                digest,
            ));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }
    for marker in [None, Some("reset.marker"), Some("reset-all.marker")] {
        let fixture = Fixture::new();
        let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
        let workspace = initialized["workspace"]["uuid"].as_str().unwrap();
        let current = initialized_schema_generation(&fixture);
        let newer = current + 1;
        let runtime = fixture.0.join(".podway/runtime");
        let staging = fixture.0.join("newer-producer");
        fs::create_dir(&staging).unwrap();
        fs::copy(runtime.join("state.sqlite3"), staging.join("state.sqlite3")).unwrap();
        let producer = rusqlite::Connection::open(staging.join("state.sqlite3")).unwrap();
        producer
            .execute_batch(&format!(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version={newer}"
            ))
            .unwrap();
        assert_eq!(
            producer
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            newer
        );
        fs::remove_dir_all(&runtime).unwrap();
        fs::create_dir(&runtime).unwrap();
        for suffix in ["", "-wal"] {
            fs::copy(
                staging.join(format!("state.sqlite3{suffix}")),
                runtime.join(format!("state.sqlite3{suffix}")),
            )
            .unwrap();
        }
        drop(producer);
        let main = fs::read(runtime.join("state.sqlite3")).unwrap();
        assert_eq!(
            u32::from_be_bytes(main[60..64].try_into().unwrap()),
            current
        );
        let wal = fs::OpenOptions::new()
            .write(true)
            .open(runtime.join("state.sqlite3-wal"))
            .unwrap();
        wal.set_len(512 * 1024 * 1024 + 1).unwrap();
        drop(wal);
        if let Some(marker) = marker {
            fs::write(runtime.join(marker), b"retained foreign marker").unwrap();
        }
        let before = image(&fixture.0.join(".podway"));
        for args in [
            vec!["status"],
            vec!["reset", "--all", "--yes", "--idempotency-key", "reset"],
            vec![
                "workspace",
                "remove",
                "--force",
                "--yes",
                "--if-workspace-uuid",
                workspace,
                "--idempotency-key",
                "remove",
            ],
        ] {
            let output = fixture.output(&args);
            assert_eq!(
                output.status.code(),
                Some(5),
                "{marker:?}/{args:?}: {output:?}"
            );
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                error.as_value()["code"],
                "WORKSPACE_SCHEMA_UNSUPPORTED",
                "{marker:?}/{args:?}"
            );
            assert_eq!(error.as_value()["details"]["store_schema_version"], newer);
            assert!(error.as_value()["details"].get("recovery").is_none());
        }
        assert_eq!(image(&fixture.0.join(".podway")), before);
    }
}

#[test]
fn v2dfc015_owned_reset_marker_recovers_after_a_bounded_wal_scan_failure() {
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    let old = fixture.run(&["init", "--idempotency-key", "init"]);
    let args = [
        "reset",
        "--all",
        "--yes",
        "--idempotency-key",
        "marked-reset",
    ];
    fixture.kill_at_maintenance_boundary(&args, "marker-published");
    let runtime = fixture.0.join(".podway/runtime");
    let database = runtime.join("state.sqlite3");
    let wal = runtime.join("state.sqlite3-wal");
    let main = fs::read(&database).unwrap();
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&wal)
        .unwrap();
    file.set_len(512 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let before = fs::metadata(&wal).unwrap();
    let output = fixture.output(&["status"]);
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
    assert_eq!(fs::read(&database).unwrap(), main);
    let after = fs::metadata(&wal).unwrap();
    assert_eq!(
        (after.dev(), after.ino(), after.len()),
        (before.dev(), before.ino(), before.len())
    );
    let recovered = fixture.run(&args);
    assert_ne!(recovered["workspace"]["uuid"], old["workspace"]["uuid"]);
    assert_eq!(fixture.run(&args), recovered);
    assert_eq!(
        fixture.run(&["doctor", "--deep"])["result"]["healthy"],
        true
    );
}

#[test]
fn v2dfc015_newer_generations_refuse_every_store_admission_without_new_files() {
    let probe = Fixture::new();
    probe.run(&["init", "--idempotency-key", "init"]);
    let current = initialized_schema_generation(&probe);
    for generation in [current + 1, i32::MAX as u32] {
        for checkpointed in [false, true] {
            for marker in [None, Some("reset-all.marker"), Some("reset.marker")] {
                let fixture = Fixture::new();
                let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
                let workspace = initialized["workspace"]["uuid"].as_str().unwrap();
                let runtime = fixture.0.join(".podway/runtime");
                let staging = fixture.0.join("newer-producer");
                fs::create_dir(&staging).unwrap();
                fs::copy(runtime.join("state.sqlite3"), staging.join("state.sqlite3")).unwrap();
                let connection = rusqlite::Connection::open(staging.join("state.sqlite3")).unwrap();
                connection
                    .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0")
                    .unwrap();
                connection
                    .pragma_update(None, "user_version", generation)
                    .unwrap();
                assert_eq!(
                    connection
                        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                        .unwrap(),
                    generation,
                );
                if checkpointed {
                    connection
                        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                        .unwrap();
                }
                fs::remove_dir_all(&runtime).unwrap();
                fs::create_dir(&runtime).unwrap();
                for suffix in ["", "-wal"] {
                    fs::copy(
                        staging.join(format!("state.sqlite3{suffix}")),
                        runtime.join(format!("state.sqlite3{suffix}")),
                    )
                    .unwrap();
                }
                drop(connection);
                if let Some(marker) = marker {
                    fs::write(runtime.join(marker), b"foreign marker").unwrap();
                }
                let before = fixture.podway_snapshot();
                for args in [
                    vec!["workspace", "show"],
                    vec!["doctor", "--deep"],
                    vec!["status"],
                    vec!["observe"],
                    vec!["next"],
                    vec!["outcome", "lookup", "--idempotency-key", "init"],
                    vec!["init", "--idempotency-key", "init"],
                    vec!["workspace", "repair", "--idempotency-key", "repair"],
                    vec![
                        "start",
                        "--preset",
                        "small-change-v3",
                        "--task",
                        "refuse",
                        "--idempotency-key",
                        "start",
                    ],
                    vec!["begin", "--idempotency-key", "begin"],
                    vec!["set", "scope-summary", "refuse", "--idempotency-key", "set"],
                    vec!["complete", "--idempotency-key", "complete"],
                    vec!["reset", "--all", "--yes", "--idempotency-key", "reset"],
                    vec![
                        "workspace",
                        "remove",
                        "--force",
                        "--yes",
                        "--if-workspace-uuid",
                        workspace,
                        "--idempotency-key",
                        "remove",
                    ],
                ] {
                    let output = fixture.output(&args);
                    assert_eq!(
                        output.status.code(),
                        Some(5),
                        "{generation}/{checkpointed}/{marker:?}/{args:?}: {output:?}"
                    );
                    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
                    let error = error.as_value();
                    assert_eq!(
                        error["code"], "WORKSPACE_SCHEMA_UNSUPPORTED",
                        "{generation}/{checkpointed}/{marker:?}/{args:?}: {error}"
                    );
                    assert_eq!(
                        error["details"]["schema"],
                        "podway.workspace-schema-unsupported-details/v2"
                    );
                    assert_eq!(error["details"]["store_schema_version"], generation);
                    assert_eq!(error["details"]["supported_store_schema_version"], current);
                    assert!(
                        error["details"]["guidance"]
                            .as_str()
                            .unwrap()
                            .contains(&format!("generation {generation}"))
                    );
                    assert!(error["details"].get("recovery").is_none());
                    fixture.assert_podway_snapshot(&before);
                }
                let human = fixture.raw_output(&["doctor"]);
                assert_eq!(human.status.code(), Some(5));
                assert!(String::from_utf8(human.stderr).unwrap().contains(&format!(
                    "hint: Use a Podway version that supports Store generation {generation}."
                )));
                fixture.assert_podway_snapshot(&before);
                fs::write(fixture.0.join(".podway/config.yaml"), b"invalid: [").unwrap();
                let before_invalid_config = fixture.podway_snapshot();
                for args in [
                    vec!["init", "--idempotency-key", "config-init"],
                    vec!["status"],
                    vec!["workspace", "repair", "--idempotency-key", "config-repair"],
                    vec![
                        "workspace",
                        "remove",
                        "--force",
                        "--yes",
                        "--if-workspace-uuid",
                        workspace,
                        "--idempotency-key",
                        "config-remove",
                    ],
                    vec![
                        "reset",
                        "--all",
                        "--yes",
                        "--idempotency-key",
                        "config-reset",
                    ],
                ] {
                    let output = fixture.output(&args);
                    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
                    assert_eq!(error.as_value()["code"], "WORKSPACE_SCHEMA_UNSUPPORTED");
                    fixture.assert_podway_snapshot(&before_invalid_config);
                }
            }
        }
    }
}

#[test]
fn v2dfc012_zero_budget_executes_idle_reads_mutations_and_maintenance() {
    let fixture = Fixture::new();
    let initialized = fixture.run(&["--timeout", "0ms", "init", "--idempotency-key", "init"]);
    for command in [vec!["workspace", "show"], vec!["doctor", "--deep"]] {
        fixture.run(&[&["--timeout", "0ms"][..], &command[..]].concat());
    }
    fixture.run(&[
        "--timeout",
        "0ms",
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "zero budget",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["--timeout", "0ms", "begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "--timeout",
        "0ms",
        "set",
        "scope-summary",
        "committed",
        "--idempotency-key",
        "set",
    ]);
    let outcome = fixture.run(&[
        "--timeout",
        "0ms",
        "outcome",
        "lookup",
        "--idempotency-key",
        "set",
    ]);
    assert_eq!(outcome["result"]["found"], true);
    let reset = fixture.run(&[
        "--timeout",
        "0ms",
        "reset",
        "--all",
        "--force",
        "--yes",
        "--idempotency-key",
        "reset",
    ]);
    assert_ne!(reset["workspace"]["uuid"], initialized["workspace"]["uuid"]);
    assert_eq!(
        fixture.run(&["--timeout", "0ms", "doctor", "--deep"])["result"]["healthy"],
        true
    );
}

#[test]
fn v2dfc012_start_interview_after_six_seconds_continues_without_changes() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "original",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let rendered = fixture.continue_in_pty_after_prompt(Duration::from_secs(6));
    assert!(
        rendered.contains("No new session was created"),
        "{rendered}"
    );
    assert!(
        rendered.contains(before["result"]["cursor"]["session"].as_str().unwrap()),
        "{rendered}"
    );
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
}

#[test]
fn v2dfc012_destructive_confirmation_does_not_renew_the_wait_budget() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let before = fixture.run(&["workspace", "show"]);
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
    let (output, elapsed) = fixture.confirm_reset_in_pty_after_prompt(Duration::from_secs(6));
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}: {output:?}");
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert!(rendered.contains("WORKSPACE_MAINTENANCE"), "{rendered}");
    assert_eq!(
        rendered.matches("podway-test-pty-exit:4").count(),
        1,
        "{rendered}"
    );
    drop(paused);
    assert_eq!(
        fixture.run(&["workspace", "show"])["result"],
        before["result"]
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "confirmed-reset"])["result"]["found"],
        false
    );
}

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
    super::e2e_adopt001_scoped_cli::assert_scoped_delegation_replay();
}

#[test]
fn goal_begin_define_assess_decide_and_completed_revision_preserve_fences_and_replay() {
    for begin_with_goal in [true, false] {
        let (fixture, before, _) =
            super::e2e_adopt001_scoped_cli::scoped_criterion_fixture(begin_with_goal);
        assert_eq!(before["result"]["goal"]["revision"], 1);
        assert_eq!(before["result"]["goal"]["actor"], "fixture-coordinator");
        let cursor = before["result"]["cursor"].to_string();
        let workspace = before["workspace"]["uuid"].as_str().unwrap();
        let args = [
            "goal",
            "assess-criterion",
            "proof",
            "--status",
            "satisfied",
            "--reason",
            "The resulting evidence is current.",
            "--evidence",
            "@execution-result",
            "--actor",
            "fixture-evaluator",
            "--if-workspace-uuid",
            workspace,
            "--if-cursor",
            &cursor,
            "--idempotency-key",
            "assess",
        ];
        let first = fixture.run(&args);
        assert_eq!(fixture.run(&args), first);
        assert_eq!(
            exact_outcome(&fixture, "assess")["outcome"]["response"],
            first
        );
        let mut stale = args;
        let key_index = stale.len() - 1;
        stale[key_index] = "too-late";
        scoped_error(fixture.output(&stale), "SESSION_REVISION_CONFLICT");
        assert_eq!(exact_outcome(&fixture, "too-late")["found"], false);
        let decision = fixture.run(&[
            "decide",
            "--option",
            "achieved",
            "--reason",
            "The criterion is satisfied.",
            "--actor",
            "fixture-coordinator",
            "--idempotency-key",
            "decision",
            "--with-observation",
        ]);
        assert_eq!(
            decision["result"]["observation"]["goal"]["outcome"],
            "achieved"
        );
        let closed = fixture.run(&[
            "complete",
            "--idempotency-key",
            "close",
            "--with-observation",
        ]);
        assert_eq!(closed["result"]["observation"]["lifecycle"], "completed");
        let completed = fixture.run(&["--contract-generation", "2", "observe"]);
        let exact = completed["result"]["cursor"].to_string();
        let reactivation = [
            "goal",
            "revise",
            "--goal",
            "Check the next result.",
            "--criterion",
            "retest=The result is fresh.",
            "--rework-to",
            "plan",
            "--reason",
            "A fresh result is required.",
            "--reactivate",
            "--basis-description",
            "Owned fixture state.",
            "--basis-digest",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "--feedback",
            "Preserve the earlier result as history.",
            "--actor",
            "fixture-coordinator",
            "--if-cursor",
            &exact,
            "--if-workspace-uuid",
            workspace,
            "--idempotency-key",
            "reactivate",
        ];
        let first = fixture.run(&reactivation);
        assert_eq!(fixture.run(&reactivation), first);
        assert_eq!(first["result"]["cursor"]["attempt"]["node"], "plan");
        assert_eq!(first["result"]["cursor"]["goal_revision"], 2);
        assert_eq!(first["result"]["cursor"]["selected_group"], Value::Null);
        let current = fixture.run(&["observe"]);
        assert_eq!(current["result"]["lifecycle"], "running");
        assert_eq!(current["result"]["retained_group_count"], 1);
    }
}

#[test]
fn calls003_documented_check_result_decodes_and_records_with_observed_fences() {
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "bug-fix-v3",
        "--task",
        "Qualify typed check recording",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&[
        "begin",
        "--goal",
        "Record typed check evidence.",
        "--criterion",
        "typed=The result remains scoped.",
        "--idempotency-key",
        "begin",
    ]);
    for (item, value) in [
        ("reproduction-status", "reproduced"),
        ("observed-behavior", "A deterministic regression fixture."),
        (
            "expected-behavior",
            "Observe a recorded, scoped check result.",
        ),
        ("regression-check", "Run the fixture authoring check."),
    ] {
        fixture.run(&["set", item, value, "--idempotency-key", item]);
    }
    let plan = fixture.run(&["--contract-generation", "2", "observe"]);
    let mut activate = scoped_template(&plan, "group.activate", "activate");
    activate["working_copy_basis"] = json!({
        "descriptor":"Owned typed-check fixture",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111"
    });
    scoped_result(fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        activate.to_string().as_bytes(),
    ));
    for (item, value) in [
        (
            "cause",
            "The owned fixture formerly omitted the declared type.",
        ),
        ("affected-boundary", "The fixture scope."),
    ] {
        fixture.run(&["set", item, value, "--idempotency-key", item]);
    }
    fixture.run(&["complete", "--idempotency-key", "diagnosis"]);
    fixture.run(&[
        "set",
        "fix-summary",
        "Supply the declared type.",
        "--idempotency-key",
        "fix",
    ]);
    fixture.run(&[
        "add",
        "changed-boundaries",
        "Fixture inputs",
        "--idempotency-key",
        "boundary",
    ]);
    fixture.run(&["complete", "--idempotency-key", "implementation"]);

    let source = fixture.0.join("fixture-check.yaml");
    fs::write(
        &source,
        include_bytes!("../../../assets/presets/bug-fix-v3.yaml"),
    )
    .unwrap();
    fixture.run(&["procedure", "format", "fixture-check.yaml", "--write"]);
    let input_digest = format!("sha256:{:x}", Sha256::digest(fs::read(&source).unwrap()));
    let checked = fixture.output(&["procedure", "check", "fixture-check.yaml"]);
    assert!(
        checked.status.success() && checked.stderr.is_empty(),
        "{checked:?}"
    );
    let output_digest = format!("sha256:{:x}", Sha256::digest(&checked.stdout));

    let observed = fixture.run(&["--contract-generation", "2", "observe"]);
    assert_eq!(observed["result"]["cursor"]["attempt"]["node"], "verify");
    let declaration = &observed["result"]["guidance"]["definition"]["items"][0];
    assert_eq!(declaration["id"], "verification-result");
    let mut record = scoped_template(&observed, "item.record_many", "check-result");
    record["operations"] = json!([{
        "item_id":"verification-result","expected_item_revision":0,"record":{
            "type":"check_result",
            "operation_id":declaration["operation_id"],
            "operation_digest":declaration["operation_digest"],
            "input_basis":{"descriptor":"Canonical owned Procedure file","digest":input_digest},
            "executor":{"name":"podway procedure check","version":env!("CARGO_PKG_VERSION")},
            "outcome":"pass","summary":"The owned canonical fixture passed its authoring check.",
            "output_digest":output_digest,
        }
    }]);
    let bytes = serde_json::to_vec(&record).unwrap();
    podway_protocol::decode_scoped_item_record_many_input_v1(&bytes).unwrap();
    let flags = &["record", "--stdin", "--with-observation"];
    let accepted = scoped_result(fixture.output_with_stdin(flags, &bytes));
    assert_eq!(accepted["result"]["observation"]["guidance"]["ready"], true);
    assert_eq!(accepted["result"]["observation"]["items"][0]["revision"], 1);
    assert_eq!(
        scoped_result(fixture.output_with_stdin(flags, &bytes)),
        accepted
    );
    assert_eq!(
        exact_outcome(&fixture, "check-result")["outcome"]["response"],
        accepted
    );
    fixture.run(&["complete", "--idempotency-key", "checked"]);
}

fn valid_test_stderr(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        text.lines().all(|line| {
            let mut fields = line.split(':');
            let count = match fields.next() {
                Some("podway-test-preflight-count") => 2,
                Some("podway-test-graph-hint-count") => 3,
                _ => return false,
            };
            let values: Vec<_> = fields.collect();
            values.len() == count && values.iter().all(|value| value.parse::<u64>().is_ok())
        })
    })
}

fn scoped_result(output: std::process::Output) -> Value {
    assert!(
        output.status.success() && valid_test_stderr(&output.stderr),
        "{output:?}"
    );
    serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value()
}

fn scoped_error(output: std::process::Output, code: &str) -> Value {
    assert!(
        !output.status.success() && valid_test_stderr(&output.stderr),
        "{output:?}"
    );
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], code, "{}", error.as_value());
    error.into_value()
}

fn exact_record_fixture() -> (Fixture, Value) {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let collect = json!({"type":"action","title":"Record values","intent":"Retain scoped inputs.","items":[
        {"id":"note","type":"text","prompt":"Record the note.","required":true},
        {"id":"evidence","type":"text","prompt":"Record the evidence.","required":false},
        {"id":"count","type":"integer","prompt":"Record the count.","required":true}
    ]});
    let close = json!({"type":"action","title":"Close","intent":"Finish the fixture."});
    let base = json!({
        "schema":"podway.procedure/v3","id":"record-control","version":"1",
        "name":"Record control","purpose":"Verify fenced atomic recording.",
        "node_definitions":{"collect":collect,"finish":close},
        "graph":{"entry":"anchor","nodes":[
            {"id":"anchor","use":"collect","next":"finish"},
            {"id":"finish","use":"finish","terminal":true}
        ]},
        "execution_group":{"anchor":"anchor","success_return":"finish","plan_inputs":["note","count"],"result_items":[],"replan_decisions":[]}
    });
    fs::write(fixture.0.join("record-control.json"), base.to_string()).unwrap();
    let digest = fixture.run(&["procedure", "preview", "record-control.json"]);
    fixture.run(&[
        "start",
        "--procedure",
        "record-control.json",
        "--expect-procedure-digest",
        digest["result"]["digest"].as_str().unwrap(),
        "--task",
        "Scoped record control",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let observed = fixture.run(&["--contract-generation", "2", "observe"]);
    (fixture, observed)
}

fn scoped_template(state: &Value, command: &str, key: &str) -> Value {
    let template = state["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["command"] == command)
        .unwrap();
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!(key);
    input
}

fn exact_outcome(fixture: &Fixture, key: &str) -> Value {
    fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"].clone()
}

#[test]
fn record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight() {
    let (fixture, observe) = exact_record_fixture();
    let mut input = scoped_template(&observe, "item.record_many", "batch");
    input["operations"] = json!([
        {"item_id":"note","expected_item_revision":0,"record":{"type":"text","value":"Retain the note."}},
        {"item_id":"count","expected_item_revision":0,"record":{"type":"integer","value":4}},
    ]);
    let bytes = serde_json::to_vec(&input).unwrap();
    podway_protocol::decode_scoped_item_record_many_input_v1(&bytes).unwrap();

    let arguments = &["record", "--stdin", "--with-observation"];
    let output = output_with_stdin_counted(&fixture, arguments, &bytes);
    assert_read004_counters(&output, (0, 0));
    let accepted = scoped_result(output);
    let after = &accepted["result"]["observation"];
    assert_eq!(after["cursor"]["session_revision"], 2);
    assert_eq!(
        after["cursor"]["attempt"],
        observe["result"]["cursor"]["attempt"]
    );
    assert_eq!(
        after["items"],
        json!([
            {"item":"note","recorded":true,"revision":1},
            {"item":"evidence","recorded":false,"revision":0},
            {"item":"count","recorded":true,"revision":1}
        ])
    );
    assert_eq!(
        exact_outcome(&fixture, "batch")["outcome"]["response"],
        accepted
    );

    fixture.run(&[
        "set",
        "evidence",
        "Independent write",
        "--idempotency-key",
        "independent",
    ]);
    let replay = output_with_stdin_counted(&fixture, arguments, &bytes);
    assert_read004_counters(&replay, (0, 0));
    assert_eq!(scoped_result(replay), accepted);
    let before = fixture.run(&["observe"]);
    let mut reused = input.clone();
    reused["operations"][0]["record"]["value"] = json!("A different request.");
    scoped_error(
        fixture.output_with_stdin(arguments, reused.to_string().as_bytes()),
        "IDEMPOTENCY_KEY_REUSED",
    );

    let live = fixture.run(&["--contract-generation", "2", "observe"]);
    let mut invalid = scoped_template(&live, "item.record_many", "late-invalid");
    invalid["operations"] = json!([
        {"item_id":"note","expected_item_revision":1,"record":{"type":"text","value":"Must roll back."}},
        {"item_id":"count","expected_item_revision":99,"record":{"type":"integer","value":5}}
    ]);
    scoped_error(
        fixture.output_with_stdin(arguments, invalid.to_string().as_bytes()),
        "ITEM_REVISION_CONFLICT",
    );
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
    assert_eq!(exact_outcome(&fixture, "late-invalid")["found"], false);
}

#[test]
fn reads004_candidate_reports_no_preflight_counters_when_uninstrumented() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let output = output_with_stdin_counted(&fixture, &["record", "--stdin"], b"not-json");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    if std::env::var_os("PODWAY_READS004_UNINSTRUMENTED_CANDIDATE").is_some() {
        assert_eq!(reads004_preflight_counters(&output), None);
    } else if super::cli_fixture::reads004_counter_harness() {
        assert_eq!(reads004_preflight_counters(&output), Some((0, 0)));
        // Positive controls on the selected debug binary: a real session.
        fixture.run(&[
            "start",
            "--preset",
            "small-change-v3",
            "--task",
            "counter control",
            "--idempotency-key",
            "control-start",
        ]);
        fixture.run(&["begin", "--idempotency-key", "control-begin"]);
        let status = output_with_stdin_counted(&fixture, &["status"], b"");
        assert_eq!(status.status.code(), Some(0), "{status:?}");
        assert_eq!(reads004_preflight_counters(&status), Some((0, 1)));
        let automatic = output_with_stdin_counted(
            &fixture,
            &[
                "set",
                "scope-summary",
                "counter control value",
                "--idempotency-key",
                "control-set",
            ],
            b"",
        );
        assert_eq!(automatic.status.code(), Some(0), "{automatic:?}");
        assert_eq!(reads004_preflight_counters(&automatic), Some((1, 0)));
    }
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
        "small-change-v3",
        "--task",
        "output boundary",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let workspace = before["workspace"]["uuid"].as_str().unwrap();
    let session = before["result"]["cursor"]["session"].as_str().unwrap();
    let attempt = before["result"]["cursor"]["attempt"]["attempt"]
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
        after["result"]["cursor"]["session_revision"]
            .as_u64()
            .unwrap(),
        before["result"]["cursor"]["session_revision"]
            .as_u64()
            .unwrap()
            + 1
    );
    let mut changed = arguments;
    changed[2] = "a changed request must not replay";
    let output = fixture.output(&changed);
    assert_eq!(output.status.code(), Some(2));
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");
    changed = arguments;
    changed[4] = "lost-output-fresh-key";
    let output = fixture.output(&changed);
    assert_eq!(output.status.code(), Some(4));
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "ITEM_REVISION_CONFLICT");
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "lost-output-fresh-key"
        ])["result"]["found"],
        false
    );
    let rejected = fixture.run(&["status"]);
    assert_eq!(rejected["result"], after["result"]);
    assert_eq!(
        rejected["workspace"]["latest_workspace_sequence"],
        after["workspace"]["latest_workspace_sequence"]
    );
}

#[test]
fn calls004_workspace_retention_miss_preserves_known_effect_and_read_only_lookup() {
    let fixture = Fixture::new();
    let original = fixture.run(&["init", "--idempotency-key", "calls004-old-init"]);
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "calls004-old-init"
        ])["result"]["outcome"]["response"],
        original
    );
    fs::write(
        fixture.0.join("external-result.txt"),
        b"The client completed an external effect.\n",
    )
    .unwrap();
    for index in 0..100 {
        fixture.run(&[
            "workspace",
            "repair",
            "--idempotency-key",
            &format!("calls004-repair-{index}"),
        ]);
    }
    let before = fixture.run(&["workspace", "show"]);
    let snapshot = fixture.podway_snapshot();
    for generation in ["1", "2"] {
        let missing = fixture.run(&[
            "--contract-generation",
            generation,
            "outcome",
            "lookup",
            "--idempotency-key",
            "calls004-old-init",
        ]);
        assert_eq!(missing["result"]["found"], false);
        assert_eq!(missing["result"]["outcome"], Value::Null);
        let latest = fixture.run(&[
            "--contract-generation",
            generation,
            "outcome",
            "lookup",
            "--idempotency-key",
            "calls004-repair-99",
        ]);
        assert_eq!(latest["result"]["found"], true);
    }
    fixture.assert_podway_snapshot(&snapshot);
    assert_eq!(
        fixture.run(&["workspace", "show"])["workspace"]["latest_workspace_sequence"],
        before["workspace"]["latest_workspace_sequence"]
    );
    assert_eq!(before["workspace"]["uuid"], original["workspace"]["uuid"]);
    assert!(fixture.0.join(".podway/config.yaml").is_file());
    assert_eq!(
        fs::read(fixture.0.join("external-result.txt")).unwrap(),
        b"The client completed an external effect.\n"
    );
}

#[test]
fn an_absent_item_reaches_the_domain_error_without_creating_an_outcome() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
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
    for (removal, fenced, force) in [
        (false, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let workspace = init["workspace"]["uuid"].as_str().unwrap();
        let mut arguments = if removal {
            vec!["workspace", "remove"]
        } else {
            vec!["reset", "--all"]
        };
        if force {
            arguments.push("--force");
        }
        arguments.push("--yes");
        if fenced {
            arguments.extend(["--if-workspace-uuid", workspace]);
        }
        arguments.extend(["--idempotency-key", "original-maintenance-key"]);
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
        assert_eq!(argv, arguments);
        let before = fixture.podway_snapshot();
        let mut changed = arguments.clone();
        if removal {
            changed.remove(2);
        } else if fenced {
            changed.drain(3..5);
        } else {
            changed.splice(3..3, ["--if-workspace-uuid", workspace]);
        }
        let rejected = fixture.output(&changed);
        assert_eq!(
            rejected.status.code(),
            Some(if removal { 2 } else { 4 }),
            "{rejected:?}"
        );
        let rejected: ErrorEnvelopeV2 = serde_json::from_slice(&rejected.stdout).unwrap();
        assert_eq!(
            rejected.as_value()["code"],
            if removal {
                "REQUEST_INVALID"
            } else {
                "WORKSPACE_MAINTENANCE"
            }
        );
        fixture.assert_podway_snapshot(&before);
        let resumed = fixture.run(&argv);
        assert_eq!(resumed["command"], recipe["command"]);
        if removal {
            assert!(!fixture.0.join(".podway").exists());
            assert!(fixture.0.join(".git/config").exists());
        } else {
            assert_eq!(fixture.run(&arguments), resumed);
            let rejected = fixture.output(&changed);
            assert_eq!(rejected.status.code(), Some(2), "{rejected:?}");
            let rejected: ErrorEnvelopeV2 = serde_json::from_slice(&rejected.stdout).unwrap();
            assert_eq!(rejected.as_value()["code"], "IDEMPOTENCY_KEY_REUSED");
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
fn maintenance_recipes_preserve_flag_like_keys_without_granting_flag_authority() {
    for (removal, key) in [
        (false, "--yes"),
        (false, "--mode"),
        (false, "--"),
        (true, "--force"),
    ] {
        let fixture = Fixture::new();
        let init = fixture.run(&["init", "--idempotency-key", "init"]);
        let workspace = init["workspace"]["uuid"].as_str().unwrap();
        let before = fixture.podway_snapshot();
        let unconfirmed = fixture.output(&["reset", "--all", "--idempotency-key", "--yes"]);
        assert_eq!(unconfirmed.status.code(), Some(2), "{unconfirmed:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&unconfirmed.stdout).unwrap();
        assert!(
            ["REQUEST_INVALID", "CONFIRMATION_REQUIRED"]
                .contains(&error.as_value()["code"].as_str().unwrap())
        );
        fixture.assert_podway_snapshot(&before);
        let key_argument = format!("--idempotency-key={key}");
        let mut arguments = if removal {
            vec!["workspace", "remove", "--force"]
        } else {
            vec!["reset", "--all"]
        };
        arguments.extend(["--yes", "--if-workspace-uuid", workspace, &key_argument]);
        fixture.kill_at_maintenance_boundary(&arguments, "marker-published");
        let observed = fixture.output(&["status"]);
        assert_eq!(observed.status.code(), Some(4), "{observed:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&observed.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        let recipe = &error.as_value()["details"]["recovery"];
        assert_eq!(recipe["requires_explicit_authorization"], true);
        let argv: Vec<_> = recipe["argv"].as_array().unwrap()[2..]
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let mut expected = arguments[..arguments.len() - 1].to_vec();
        expected.extend(["--idempotency-key", key]);
        assert_eq!(argv, expected);
        let resumed = fixture.run(&argv);
        assert_eq!(resumed["command"], recipe["command"]);
        if removal {
            assert!(!fixture.0.join(".podway").exists());
            assert!(fixture.0.join(".git/config").exists());
        } else {
            assert_eq!(fixture.run(&arguments), resumed);
            assert_eq!(fixture.run(&argv), resumed);
            assert_eq!(
                fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["outcome"]
                    ["response"],
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
        "small-change-v3",
        "--task",
        "retain this session",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = fixture.run(&["status"]);
    let arguments = [
        "set",
        "scope-summary",
        "after contention",
        "--if-workspace-uuid",
        initialized["workspace"]["uuid"].as_str().unwrap(),
        "--if-session-id",
        before["result"]["cursor"]["session"].as_str().unwrap(),
        "--if-attempt",
        before["result"]["cursor"]["attempt"]["attempt"]
            .as_str()
            .unwrap(),
        "--if-item-revision",
        "0",
        "--idempotency-key",
        "contended-sqlite",
    ];
    let writer =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Original fences bypass implicit status preflight and reach the SQLite writer.
    for (timeout, millis) in [("0ms", 0), ("20ms", 20)] {
        let started = Instant::now();
        let output = fixture.output(&[&["--timeout", timeout][..], &arguments[..]].concat());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_BUSY");
        assert_eq!(error.as_value()["command"], "item.set");
        assert_eq!(error.as_value()["retryable"], true);
        assert_eq!(error.as_value()["details"]["boundary"], "sqlite_write_lock");
        assert_eq!(error.as_value()["details"]["timeout_ms"], millis);
    }
    writer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "contended-sqlite",])["result"]["found"],
        false
    );
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
        "small-change-v3",
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
    for (timeout, millis) in [("0ms", 0), ("20ms", 20)] {
        let started = Instant::now();
        let output = fixture.output(&[
            "--timeout",
            timeout,
            "set",
            "scope-summary",
            "must not commit",
            "--idempotency-key",
            "contended",
        ]);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the invocation must not restart its lock budget"
        );
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "WORKSPACE_MAINTENANCE");
        assert_eq!(error.as_value()["details"]["boundary"], "maintenance_lock");
        assert_eq!(error.as_value()["details"]["timeout_ms"], millis);
        assert_eq!(
            error.as_value()["details"]["recovery"]["action"],
            "refresh_state"
        );
        assert_eq!(
            error.as_value()["details"]["recovery"]["command"],
            "session.observe"
        );
        assert_eq!(
            error.as_value()["details"]["recovery"]["argv"],
            json!(["podway", "--json", "observe"])
        );
        assert_eq!(
            error.as_value()["command"],
            "item.set",
            "internal status preflight must preserve the requested command identity"
        );
        assert_eq!(error.as_value()["retryable"], true);
    }
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
        "small-change-v3",
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
        assert_eq!(before["result"]["lifecycle"], expected);
        let rendered = fixture.continue_in_pty();
        assert!(
            rendered.contains("No new session was created"),
            "{rendered}"
        );
        assert!(
            rendered.contains(before["result"]["cursor"]["session"].as_str().unwrap()),
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
            "small-change-v3",
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
        "small-change-v3",
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
    let cursor = &before["result"]["cursor"];
    let session_id = cursor["session"].as_str().unwrap();
    let attempt_id = cursor["attempt"]["attempt"].as_str().unwrap();
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
    let flow = fixture.run(&["--contract-generation", "2", "observe"]);
    let recipes = flow["result"]["mutation_templates"].as_array().unwrap();
    let recipe = recipes
        .iter()
        .find(|value| value["command"] == "group.activate")
        .unwrap();
    let mut input: Value =
        serde_json::from_str(recipe["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!("activate");
    input["working_copy_basis"] = json!({
        "descriptor":"Owned conformance fixture",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111"
    });
    let activated = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        input.to_string().as_bytes(),
    );
    assert!(activated.status.success(), "{activated:?}");
    assert_eq!(
        fixture.run(&["observe"])["result"]["cursor"]["attempt"]["node"],
        "implement"
    );
    assert!(!fixture.0.join("podwayd.sock").exists());
}

#[test]
fn bound005_completion_keeps_its_best_effort_budget_during_maintenance() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Completion budget",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let ready = fixture.raw_output(&["__complete", "items"]);
    assert_eq!(ready.status.code(), Some(0));
    assert!(!ready.stdout.is_empty());
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
    let before = fixture.podway_snapshot();
    let started = Instant::now();
    let output = fixture.raw_output(&["__complete", "items", "--timeout", "10s"]);
    assert!(started.elapsed() < Duration::from_secs(2), "{output:?}");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
    drop(paused);
    fixture.assert_podway_snapshot(&before);
}

#[test]
fn admit003_real_cli_wires_reuse_and_preserves_previews_key_errors_and_later_invocations() {
    use super::cli_fixture::assert_admit003_hint_counters;

    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Successor invocation",
        "--idempotency-key",
        "start",
    ]);
    let prepared = fixture.run(&["observe"]);
    let begin = output_with_stdin_counted(&fixture, &["begin", "--idempotency-key", "begin"], b"");
    assert_read004_counters(&begin, (1, 0));
    assert_admit003_hint_counters(&begin, (0, 0, 0));
    let begun = scoped_result(begin);
    assert_eq!(
        begun["result"]["cursor"]["session"],
        prepared["result"]["cursor"]["session"]
    );
    let cursor = begun["result"]["cursor"].to_string();
    let workspace = begun["workspace"]["uuid"].as_str().unwrap();
    let flags = [
        "set",
        "scope-summary",
        "Record the current plan.",
        "--if-cursor",
        &cursor,
        "--if-workspace-uuid",
        workspace,
        "--if-item-revision",
        "0",
        "--idempotency-key",
        "scope",
    ];
    let direct = output_with_stdin_counted(&fixture, &flags, b"");
    assert_read004_counters(&direct, (0, 0));
    let original = scoped_result(direct);
    let replay = output_with_stdin_counted(&fixture, &flags, b"");
    assert_read004_counters(&replay, (0, 0));
    assert_eq!(scoped_result(replay), original);

    let before = fixture.run(&["observe"]);
    let preview = fixture.run(&["reset", "--dry-run"]);
    assert_eq!(preview["result"]["dry_run"], true);
    for args in [
        &["complete"][..],
        &["complete", "--idempotency-key", ""][..],
    ] {
        let invalid = output_with_stdin_counted(&fixture, args, b"");
        assert_read004_counters(&invalid, (0, 0));
        scoped_error(invalid, "REQUEST_INVALID");
    }
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
    let later = output_with_stdin_counted(
        &fixture,
        &[
            "block",
            "--reason",
            "Retain current state",
            "--idempotency-key",
            "block",
        ],
        b"",
    );
    assert_read004_counters(&later, (1, 0));
    assert_eq!(scoped_result(later)["result"]["lifecycle"], "running");
    assert_eq!(
        exact_outcome(&fixture, "scope")["outcome"]["response"],
        original
    );
}

fn guide004_normalized_error_stdout(stdout: &[u8]) -> String {
    let text = String::from_utf8(stdout.to_vec()).unwrap();
    let envelope: Value = serde_json::from_str(&text).unwrap();
    let request_id = envelope["request_id"].as_str().unwrap().to_owned();
    let generated_at = envelope["generated_at"].as_str().unwrap().to_owned();
    text.replace(
        &format!("\"request_id\":\"{request_id}\""),
        "\"request_id\":\"<request-id>\"",
    )
    .replace(
        &format!("\"generated_at\":\"{generated_at}\""),
        "\"generated_at\":\"<generated-at>\"",
    )
}

/// Frozen from the unmodified renderer at base e9b03ab before any GUIDE-004
/// render.rs edit; only the volatile values above (plus the expected digest)
/// are normalized. Any other byte change to these envelopes fails the tests.
const GUIDE004_WRITER_MISMATCH_JSON_BASELINE: &str = "{\"code\":\"WORKSPACE_CONTRACT_MISMATCH\",\"command\":\"session.status\",\"details\":{\"actual_contract_manifest_digest\":\"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\",\"expected_contract_manifest_digest\":\"<expected-digest>\",\"recovery\":{\"action\":\"diagnose_writer\",\"argv\":[\"podway\",\"--json\",\"version\",\"--identity\"],\"command\":\"version\",\"reason\":\"Inspect writer identity. Restore access or use the original writer. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path. Newer generations stay refused.\",\"requires_explicit_authorization\":false},\"schema\":\"podway.workspace-contract-mismatch-details/v1\"},\"exit_code\":5,\"generated_at\":\"<generated-at>\",\"message\":\"The Store writer contract is not supported by this binary.\",\"request_id\":\"<request-id>\",\"retryable\":false,\"schema\":\"podway.error/v2\"}\n";

#[test]
fn guide004_human_writer_mismatch_hint_names_digests_and_recovery() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let stored = format!("sha256:{}", "f".repeat(64));
    let connection =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE writer_contract SET contract_manifest_digest=?1 WHERE singleton=1",
            [&stored],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(connection);
    let identity = fixture.run(&["version", "--identity"]);
    let current = identity["result"]["contract_manifest_digest"]
        .as_str()
        .unwrap()
        .to_owned();

    // JSON leg first: the complete envelope bytes stay identical to the frozen
    // pre-change baseline after normalizing only volatile values.
    let refusal = fixture.output(&["status"]);
    assert_eq!(refusal.status.code(), Some(5), "{refusal:?}");
    assert!(refusal.stderr.is_empty(), "{refusal:?}");
    let envelope: Value = serde_json::from_slice(&refusal.stdout).unwrap();
    assert_eq!(envelope["code"], json!("WORKSPACE_CONTRACT_MISMATCH"));
    assert_eq!(
        envelope["details"]["actual_contract_manifest_digest"],
        json!(stored)
    );
    assert_eq!(
        envelope["details"]["expected_contract_manifest_digest"],
        json!(current)
    );
    let normalized = guide004_normalized_error_stdout(&refusal.stdout).replace(
        &format!("\"expected_contract_manifest_digest\":\"{current}\""),
        "\"expected_contract_manifest_digest\":\"<expected-digest>\"",
    );
    assert_eq!(normalized, GUIDE004_WRITER_MISMATCH_JSON_BASELINE);

    // Human leg: exact stderr naming both digests and both recovery routes.
    let human = fixture.raw_output(&["status"]);
    assert_eq!(human.status.code(), Some(5), "{human:?}");
    let stderr = String::from_utf8(human.stderr.clone()).unwrap();
    assert_eq!(
        stderr,
        format!(
            "error: WORKSPACE_CONTRACT_MISMATCH: The Store writer contract is not supported by this binary.\n\
             hint: stored writer manifest digest: {stored}\n\
             hint: this binary manifest digest: {current}\n\
             hint: continue with the writer whose version --identity reports the stored digest, or explicitly confirm an unfenced reset --all\n\
             reason: Inspect writer identity. Restore access or use the original writer. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path. Newer generations stay refused.\n"
        ),
        "{stderr}"
    );
}

#[test]
fn guide004_human_citation_hint_states_the_admission_rule() {
    let (fixture, before, _) = super::e2e_adopt001_scoped_cli::scoped_criterion_fixture(true);
    let args = [
        "goal",
        "assess-criterion",
        "proof",
        "--status",
        "satisfied",
        "--reason",
        "Check the scoped hint.",
        "--item",
        "note",
        "--idempotency-key",
        "reject-hint",
    ];
    let human = fixture.raw_output(&args);
    assert_eq!(human.status.code(), Some(1), "{human:?}");
    assert!(human.stdout.is_empty());
    let hint = String::from_utf8(human.stderr).unwrap();
    assert!(hint.contains("CRITERION_CITATION_INVALID"), "{hint}");
    assert!(
        hint.contains("recorded") && hint.contains("source") && hint.contains("four"),
        "{hint}"
    );
    let error = scoped_error(fixture.output(&args), "CRITERION_CITATION_INVALID");
    assert_eq!(
        error["details"]["decision"],
        before["result"]["cursor"]["attempt"]
    );
    assert_eq!(error["details"]["reason"], "unrecorded_item");
    assert_eq!(exact_outcome(&fixture, "reject-hint")["found"], false);
}

const GUIDE010_KNOWN_WRITER_REASON: &str = "Inspect writer identity. Restore access or use the original v0.2.14 writer. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path.";
const GUIDE010_UNKNOWN_WRITER_REASON: &str = "Inspect writer identity. Restore access or use the original writer matching the stored digest. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path.";

/// GUIDE-010: under generation 2 the writer mismatch maps the stored digest
/// through the released-manifest table (`v0.2.14` reports `"v0.2.14"`, an
/// unknown digest reports null) with the mismatch-specific recovery reason and
/// an exact human leg; the default and an explicit generation 1 stay
/// byte-identical to the GUIDE-004 baseline, including the human leg.
#[test]
fn guide010_generation2_writer_mismatch_reports_released_and_unknown_writers() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let identity = fixture.run(&["version", "--identity"]);
    let current = identity["result"]["contract_manifest_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let store_digest = |stored: &str| {
        let connection =
            rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE writer_contract SET contract_manifest_digest=?1 WHERE singleton=1",
                [stored],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    };
    let released = "sha256:c699977dd232891041f4063d66295b94004702875855baace876f8a7e0c6391d";
    let unknown = format!("sha256:{}", "f".repeat(64));

    for (stored, release, reason, release_line) in [
        (
            released,
            json!("v0.2.14"),
            GUIDE010_KNOWN_WRITER_REASON,
            "stored writer release: v0.2.14",
        ),
        (
            unknown.as_str(),
            Value::Null,
            GUIDE010_UNKNOWN_WRITER_REASON,
            "stored writer release: unknown",
        ),
    ] {
        store_digest(stored);
        let refusal = fixture.raw_output(&["--json", "--contract-generation", "2", "status"]);
        assert_eq!(refusal.status.code(), Some(5), "{refusal:?}");
        assert!(refusal.stderr.is_empty(), "{refusal:?}");
        // The validated envelope type re-runs schema and release-agreement
        // validation on the emitted generation-2 details.
        let validated: ErrorEnvelopeV2 = serde_json::from_slice(&refusal.stdout).unwrap();
        let details = &validated.as_value()["details"];
        assert_eq!(
            validated.as_value()["code"],
            json!("WORKSPACE_CONTRACT_MISMATCH")
        );
        assert_eq!(
            details["schema"],
            json!("podway.workspace-contract-mismatch-details/v2")
        );
        assert_eq!(details["actual_contract_manifest_digest"], json!(stored));
        assert_eq!(details["expected_contract_manifest_digest"], json!(current));
        assert_eq!(details["actual_product_version"], release);
        assert_eq!(details["recovery"]["action"], json!("diagnose_writer"));
        assert_eq!(details["recovery"]["reason"], json!(reason));

        let human = fixture.raw_output(&["--contract-generation", "2", "status"]);
        assert_eq!(human.status.code(), Some(5), "{human:?}");
        let stderr = String::from_utf8(human.stderr.clone()).unwrap();
        assert_eq!(
            stderr,
            format!(
                "error: WORKSPACE_CONTRACT_MISMATCH: The Store writer contract is not supported by this binary.\n\
                 hint: stored writer manifest digest: {stored}\n\
                 hint: this binary manifest digest: {current}\n\
                 hint: {release_line}\n\
                 hint: continue with the writer whose version --identity reports the stored digest, or explicitly confirm an unfenced reset --all\n\
                 reason: {reason}\n"
            ),
            "{stderr}"
        );
    }

    // Generation 1 unchanged: the unknown digest is the GUIDE-004 baseline's
    // stored digest, so the default and an explicit generation 1 normalize to
    // the frozen envelope and the human leg equals the GUIDE-004 text.
    for arguments in [
        vec!["--json", "status"],
        vec!["--json", "--contract-generation", "1", "status"],
    ] {
        let refusal = fixture.raw_output(&arguments);
        assert_eq!(refusal.status.code(), Some(5), "{refusal:?}");
        let normalized = guide004_normalized_error_stdout(&refusal.stdout).replace(
            &format!("\"expected_contract_manifest_digest\":\"{current}\""),
            "\"expected_contract_manifest_digest\":\"<expected-digest>\"",
        );
        assert_eq!(normalized, GUIDE004_WRITER_MISMATCH_JSON_BASELINE);
    }
    let human = fixture.raw_output(&["--contract-generation", "1", "status"]);
    assert_eq!(human.status.code(), Some(5), "{human:?}");
    let stderr = String::from_utf8(human.stderr.clone()).unwrap();
    assert_eq!(
        stderr,
        format!(
            "error: WORKSPACE_CONTRACT_MISMATCH: The Store writer contract is not supported by this binary.\n\
             hint: stored writer manifest digest: {unknown}\n\
             hint: this binary manifest digest: {current}\n\
             hint: continue with the writer whose version --identity reports the stored digest, or explicitly confirm an unfenced reset --all\n\
             reason: Inspect writer identity. Restore access or use the original writer. To discard invalid state, retire other writers and confirm an unfenced reset --all with a new key; unfenced removal needs a terminal and exact path. Newer generations stay refused.\n"
        ),
        "{stderr}"
    );
}

/// The GUIDE-004 goal fixture (`goal-outcomes.yaml` plus an optional
/// decision-local `assessment-note` item), driven to the active goal
/// assessment. `with_evidence` keeps the `perform` evidence reference and
/// `record_note` records the decision-local item.
#[test]
fn calls004_citations_preserve_optional_actor_eligibility_duplicates_and_total_bound() {
    let (fixture, observed, _) = super::e2e_adopt001_scoped_cli::scoped_criterion_fixture(true);
    let common = [
        "goal",
        "assess-criterion",
        "proof",
        "--status",
        "satisfied",
        "--reason",
        "Check the scope.",
        "--idempotency-key",
        "assessment",
    ];
    let mut duplicate = common.to_vec();
    duplicate.extend([
        "--evidence",
        "@execution-result",
        "--evidence",
        "@execution-result",
    ]);
    let error = scoped_error(fixture.output(&duplicate), "CRITERION_CITATION_INVALID");
    assert_eq!(error["details"]["reason"], "duplicate");
    let mut excessive = common.to_vec();
    excessive.extend([
        "--item", "alpha", "--item", "beta", "--item", "gamma", "--item", "delta", "--item",
        "epsilon",
    ]);
    scoped_error(fixture.output(&excessive), "REQUEST_INVALID");
    assert_eq!(exact_outcome(&fixture, "assessment")["found"], false);
    let mut unrecorded = common.to_vec();
    unrecorded.extend(["--item", "note"]);
    scoped_error(fixture.output(&unrecorded), "CRITERION_CITATION_INVALID");
    assert_eq!(
        fixture.run(&["--contract-generation", "2", "observe"])["result"],
        observed["result"]
    );
    fixture.run(&[
        "set",
        "note",
        "Retained decision note.",
        "--idempotency-key",
        "note",
    ]);
    let mut admitted = common.to_vec();
    admitted.extend([
        "--item",
        "note",
        "--evidence",
        "@execution-result",
        "--actor",
        "fixture-evaluator",
    ]);
    fixture.run(&admitted);
    assert_eq!(
        fixture.run(&["observe"])["result"]["goal"]["outcome"],
        "achieved"
    );
    assert_eq!(exact_outcome(&fixture, "assessment")["found"], true);
}

#[test]
fn guide010_generation2_citation_details_list_citable_sources() {
    super::e2e_adopt001_scoped_cli::assert_scoped_citation_cases();
}

#[test]
fn maint002_indeterminate_committed_wal_refuses_owned_and_unmarked_destruction() {
    fn checksum(mut sum: (u32, u32), bytes: &[u8]) -> (u32, u32) {
        for pair in bytes.chunks_exact(8) {
            let first = u32::from_le_bytes(pair[..4].try_into().unwrap());
            let second = u32::from_le_bytes(pair[4..].try_into().unwrap());
            sum.0 = sum.0.wrapping_add(first).wrapping_add(sum.1);
            sum.1 = sum.1.wrapping_add(second).wrapping_add(sum.0);
        }
        sum
    }
    let frozen_writer: Value =
        serde_json::from_str(GUIDE004_WRITER_MISMATCH_JSON_BASELINE).unwrap();
    let frozen_recovery = &frozen_writer["details"]["recovery"];
    // A checksum-valid committed page-one image with no trustworthy SQLite header.
    let mut wal = vec![0; 32 + 24 + 512];
    wal[..4].copy_from_slice(&0x377f0682_u32.to_be_bytes());
    wal[4..8].copy_from_slice(&3_007_000_u32.to_be_bytes());
    wal[8..12].copy_from_slice(&512_u32.to_be_bytes());
    wal[16..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let sum = checksum((0, 0), &wal[..24]);
    wal[24..28].copy_from_slice(&sum.0.to_be_bytes());
    wal[28..32].copy_from_slice(&sum.1.to_be_bytes());
    wal[32..36].copy_from_slice(&1_u32.to_be_bytes());
    wal[36..40].copy_from_slice(&1_u32.to_be_bytes());
    wal[40..48].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let sum = checksum(checksum(sum, &wal[32..40]), &wal[56..]);
    wal[48..52].copy_from_slice(&sum.0.to_be_bytes());
    wal[52..56].copy_from_slice(&sum.1.to_be_bytes());
    for marked in [false, true] {
        for remove in [false, true] {
            let fixture = Fixture::new();
            let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
            let uuid = initialized["workspace"]["uuid"].as_str().unwrap();
            let reset = [
                "reset",
                "--all",
                "--yes",
                "--idempotency-key",
                "unknown-reset",
            ];
            if marked {
                fixture.kill_at_maintenance_boundary(&reset, "marker-published");
            }
            let runtime = fixture.0.join(".podway/runtime");
            let shm = runtime.join("state.sqlite3-shm");
            if shm.exists() {
                fs::remove_file(&shm).unwrap();
            }
            fs::write(runtime.join("state.sqlite3-wal"), &wal).unwrap();
            assert!(!shm.exists());
            let before = fixture.podway_snapshot();
            let removal = [
                "workspace",
                "remove",
                "--force",
                "--yes",
                "--if-workspace-uuid",
                uuid,
                "--idempotency-key",
                "unknown-remove",
            ];
            let args: &[&str] = if remove { &removal } else { &reset };
            let mut normalized_machine = None;
            for generation in [None, Some("1"), Some("2")] {
                let mut request = Vec::new();
                if let Some(generation) = generation {
                    request.extend_from_slice(&["--contract-generation", generation]);
                }
                request.extend_from_slice(args);
                let output = fixture.output(&request);
                assert_eq!(output.status.code(), Some(5));
                assert!(output.stderr.is_empty());
                let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(
                    error.as_value()["code"],
                    "WORKSPACE_STATE_UNREADABLE",
                    "marked={marked}, remove={remove}, generation={generation:?}"
                );
                assert_eq!(error.as_value()["retryable"], false);
                assert_eq!(
                    error.as_value()["details"]["schema"],
                    "podway.workspace-recovery-details/v2"
                );
                assert_eq!(error.as_value()["details"]["recovery"], *frozen_recovery);
                let normalized = guide004_normalized_error_stdout(&output.stdout);
                if let Some(expected) = &normalized_machine {
                    assert_eq!(&normalized, expected);
                } else {
                    normalized_machine = Some(normalized);
                }
                fixture.assert_podway_snapshot(&before);
                assert!(!shm.exists());

                let human = fixture.raw_output(&request);
                assert_eq!(human.status.code(), Some(5));
                assert!(human.stdout.is_empty());
                let stderr = String::from_utf8(human.stderr).unwrap();
                assert!(stderr.contains("error: WORKSPACE_STATE_UNREADABLE:"));
                let preservation = stderr
                    .find("hint: Preserve indeterminate state. Store-independent version --identity diagnoses the writer; it establishes neither a supported generation nor a trustworthy workspace UUID or deletion authority.\n")
                    .expect("human unreadable-state guidance must preserve indeterminate state before discard advice");
                let qualification = stderr
                    .find("hint: The following discard advice applies only after supported-generation admission, original request authority and stable filesystem witnesses. Confirmation or force does not replace these conditions.\n")
                    .expect("human discard advice must be qualified by generation, request authority and witnesses");
                let reason = stderr.find("reason: ").unwrap();
                assert!(preservation < qualification && qualification < reason);
                assert_eq!(
                    &stderr[reason..],
                    format!("reason: {}\n", frozen_recovery["reason"].as_str().unwrap())
                );
                fixture.assert_podway_snapshot(&before);
                assert!(!shm.exists());
            }
        }
    }
}

#[test]
fn maint003_newer_removal_refuses_before_human_confirmation() {
    for (wal, invalid_config) in [(false, false), (false, true), (true, false), (true, true)] {
        let fixture = Fixture::new();
        fixture.run(&["init", "--idempotency-key", "init"]);
        let database = fixture.0.join(".podway/runtime/state.sqlite3");
        let mut bytes = fs::read(&database).unwrap();
        let generation = u32::from_be_bytes(bytes[60..64].try_into().unwrap());
        if wal {
            let staging = fixture.0.join("newer-producer");
            fs::create_dir(&staging).unwrap();
            fs::write(staging.join("state.sqlite3"), &bytes).unwrap();
            let producer = rusqlite::Connection::open(staging.join("state.sqlite3")).unwrap();
            producer
                .execute_batch(&format!(
                    "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version={}",
                    generation + 1
                ))
                .unwrap();
            let runtime = database.parent().unwrap();
            // Only the producer opens SQLite; the tested runtime receives native
            // main/WAL bytes and deliberately has no SHM for the probe to reuse.
            for suffix in ["", "-wal"] {
                fs::copy(
                    staging.join(format!("state.sqlite3{suffix}")),
                    runtime.join(format!("state.sqlite3{suffix}")),
                )
                .unwrap();
            }
            let shm = runtime.join("state.sqlite3-shm");
            if shm.exists() {
                fs::remove_file(shm).unwrap();
            }
            drop(producer);
        } else {
            bytes[60..64].copy_from_slice(&(generation + 1).to_be_bytes());
            fs::write(&database, bytes).unwrap();
        }
        if invalid_config {
            fs::write(fixture.0.join(".podway/config.yaml"), b"invalid: [\n").unwrap();
        }
        let before = fixture.podway_snapshot();
        let output = fixture.human_removal_in_pty(
            &[
                "workspace",
                "remove",
                "--force",
                "--idempotency-key",
                "remove",
            ],
            b"",
            false,
            || {},
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("podway-test-pty-exit:5"), "{text}");
        assert!(!text.contains("Type that exact path"), "{text}");
        assert!(!text.contains("Resolved worktree root:"), "{text}");
        assert!(text.contains("generation"), "{text}");
        fixture.assert_podway_snapshot(&before);
    }
}

#[test]
fn maint003_unforced_active_removal_has_only_a_human_typed_force_hint() {
    for running in [false, true] {
        let fixture = Fixture::new();
        fixture.run(&["init", "--idempotency-key", "init"]);
        fixture.run(&[
            "start",
            "--preset",
            "small-change-v3",
            "--task",
            "retain",
            "--idempotency-key",
            "start",
        ]);
        if running {
            fixture.run(&["begin", "--idempotency-key", "begin"]);
        }
        let before = fixture.podway_snapshot();
        let answer = format!("{}\n", fixture.0.display());
        let output = fixture.human_removal_in_pty(
            &["workspace", "remove", "--idempotency-key", "remove"],
            answer.as_bytes(),
            true,
            || {},
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("podway-test-pty-exit:2"), "{text}");
        assert!(text.contains("--force"), "{text}");
        assert!(
            text.contains("new") && text.contains("confirmation"),
            "{text}"
        );
        fixture.assert_podway_snapshot(&before);
        let unrelated = fixture.raw_output(&["workspace", "remove", "--force", "--yes"]);
        assert!(
            !String::from_utf8(unrelated.stderr)
                .unwrap()
                .contains("new workspace remove --force")
        );
    }
}

#[test]
fn maint003_control_path_display_keeps_only_original_confirmation_authority() {
    let mut fixture = Fixture::new();
    let renamed = fixture.0.with_file_name(format!(
        "{}-tab\t-escape\u{1b}-internal\r-return",
        fixture.0.file_name().unwrap().to_str().unwrap()
    ));
    fs::rename(&fixture.0, &renamed).unwrap();
    fixture.0 = renamed;
    fixture.run(&["init", "--idempotency-key", "init"]);
    let before = fixture.podway_snapshot();
    let original = fixture.0.to_str().unwrap();
    let displayed = original
        .replace('\t', "\\t")
        .replace('\u{1b}', "\\u{1b}")
        .replace('\r', "\\r");
    let alias = format!("{displayed}\n");
    let rejected = fixture.human_removal_in_pty(
        &[
            "workspace",
            "remove",
            "--force",
            "--idempotency-key",
            "remove",
        ],
        alias.as_bytes(),
        true,
        || {},
    );
    let text = String::from_utf8(rejected.stdout).unwrap();
    assert!(text.contains("podway-test-pty-exit:2"), "{text}");
    assert!(
        text.contains(&format!("Resolved worktree root: {displayed}\r\n")),
        "{text}"
    );
    fixture.assert_podway_snapshot(&before);
    // The terminal's literal-next key quotes internal CR so canonical input
    // delivers the original byte instead of translating it into a line ending.
    let answer = format!("{}\n", original.replace('\r', "\u{16}\r"));
    let accepted = fixture.human_removal_in_pty(
        &[
            "workspace",
            "remove",
            "--force",
            "--idempotency-key",
            "remove",
        ],
        answer.as_bytes(),
        true,
        || {},
    );
    let text = String::from_utf8(accepted.stdout).unwrap();
    assert!(text.contains("podway-test-pty-exit:0"), "{text}");
    assert!(!fixture.0.join(".podway").exists());
    assert!(fixture.0.join(".git").is_dir());
}

#[test]
fn maint003_non_roundtrippable_paths_refuse_before_prompt_without_effects() {
    for suffix in [b"embedded\nline".as_slice(), b"trailing\r", b"trailing\n"] {
        let mut fixture = Fixture::new();
        let mut name = fixture.0.file_name().unwrap().as_encoded_bytes().to_vec();
        name.extend_from_slice(suffix);
        let renamed = fixture
            .0
            .with_file_name(std::str::from_utf8(&name).unwrap());
        fs::rename(&fixture.0, &renamed).unwrap();
        fixture.0 = renamed;
        fixture.run(&["init", "--idempotency-key", "init"]);
        let before = fixture.podway_snapshot();
        let output = fixture.human_removal_in_pty(
            &[
                "workspace",
                "remove",
                "--force",
                "--idempotency-key",
                "remove",
            ],
            b"",
            false,
            || {},
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("podway-test-pty-exit:2"), "{text}");
        assert!(
            text.contains("interactive removal cannot confirm"),
            "{text}"
        );
        assert!(!text.contains("Type that exact path"), "{text}");
        assert!(!text.contains("Resolved worktree root:"), "{text}");
        fixture.assert_podway_snapshot(&before);
    }
}

#[test]
fn maint003_post_confirmation_generation_change_is_not_admitted_by_the_precheck() {
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let database = fixture.0.join(".podway/runtime/state.sqlite3");
    let mut newer = fs::read(&database).unwrap();
    let generation = u32::from_be_bytes(newer[60..64].try_into().unwrap());
    newer[60..64].copy_from_slice(&(generation + 1).to_be_bytes());
    let original = fs::metadata(&database).unwrap();
    let replacement = fixture.0.join("replacement.sqlite3");
    fs::write(&replacement, &newer).unwrap();
    let prepared = fs::metadata(&replacement).unwrap();
    assert_ne!(
        (original.dev(), original.ino()),
        (prepared.dev(), prepared.ino())
    );
    let runtime = database.parent().unwrap();
    let held = fs::File::open(runtime.join("maintenance.lock")).unwrap();
    held.lock().unwrap();
    let directory = fs::File::open(runtime).unwrap();
    let mut after_replacement = None;
    let output = fixture.human_removal_in_pty(
        &[
            "workspace",
            "remove",
            "--force",
            "--idempotency-key",
            "remove",
        ],
        format!("{}\n", fixture.0.display()).as_bytes(),
        true,
        || {
            // The answer has been sent. Only normal removal admission takes
            // this directory lock; the advisory precheck never acquires it.
            let limit = Instant::now() + Duration::from_secs(2);
            loop {
                match directory.try_lock() {
                    Err(std::fs::TryLockError::WouldBlock) => break,
                    Ok(()) => directory.unlock().unwrap(),
                    Err(error) => panic!("directory lock witness failed: {error}"),
                }
                assert!(
                    Instant::now() < limit,
                    "confirmed CLI must reach exclusive admission"
                );
                thread::sleep(Duration::from_millis(1));
            }
            fs::rename(&replacement, &database).unwrap();
            let replaced = fs::metadata(&database).unwrap();
            assert_eq!(
                (replaced.dev(), replaced.ino()),
                (prepared.dev(), prepared.ino())
            );
            assert_ne!(
                (replaced.dev(), replaced.ino()),
                (original.dev(), original.ino())
            );
            after_replacement = Some(fixture.podway_snapshot());
            held.unlock().unwrap();
        },
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("podway-test-pty-exit:5"), "{text}");
    assert!(
        text.contains("generation") || text.contains("WORKSPACE_MAINTENANCE"),
        "{text}"
    );
    fixture.assert_podway_snapshot(&after_replacement.unwrap());
}

#[test]
fn maint003_removal_confirmation_keeps_the_original_wait_deadline() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let before = fixture.podway_snapshot();
    let paused = fixture.pause_at_maintenance_boundary(
        &[
            "reset",
            "--all",
            "--yes",
            "--idempotency-key",
            "paused-reset",
        ],
        "lease-acquired",
    );
    let (output, elapsed) = fixture.confirm_removal_in_pty_after_prompt(Duration::from_secs(6));
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}: {output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("podway-test-pty-exit:4"), "{text}");
    assert!(text.contains("WORKSPACE_MAINTENANCE"), "{text}");
    assert!(!text.contains("new workspace remove --force"), "{text}");
    drop(paused);
    fixture.assert_podway_snapshot(&before);
}

#[test]
fn maint003_native_non_utf8_argument_cannot_select_a_lossy_alias() {
    use std::os::unix::ffi::OsStringExt;
    let mut fixture = Fixture::new();
    let name = fixture.0.file_name().unwrap().to_str().unwrap().to_owned();
    let alias = fixture
        .0
        .with_file_name(format!("{name}-replacement-\u{fffd}"));
    fs::rename(&fixture.0, &alias).unwrap();
    fixture.0 = alias;
    fixture.run(&["init", "--idempotency-key", "init"]);
    let mut invalid_name = format!("{name}-replacement-").into_bytes();
    invalid_name.push(0xff);
    let invalid = fixture
        .0
        .with_file_name(std::ffi::OsString::from_vec(invalid_name));
    // This actual fixture filesystem cannot materialize that native filename.
    // Retain the real OS evidence rather than pretending it reached confirmation.
    let error = fs::create_dir(&invalid).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(nix::libc::EILSEQ));
    assert_eq!(invalid.to_string_lossy(), fixture.0.to_string_lossy());
    let before = fixture.podway_snapshot();
    let output = fixture.removal_at_native_path_in_pty(&invalid);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("podway-test-pty-exit:2"), "{text}");
    assert!(
        text.contains("workspace removal requires an existing Git worktree"),
        "{text}"
    );
    assert!(!text.contains("Type that exact path"), "{text}");
    assert!(!text.contains("Resolved worktree root:"), "{text}");
    fixture.assert_podway_snapshot(&before);
    assert!(fixture.0.join(".git").is_dir());
}

#[test]
fn agent002_stdin_fenced_mutations_share_conflict_rejection_and_preserve_set_stdin_preflight() {
    let (fixture, initial) = exact_record_fixture();
    let mut input = scoped_template(&initial, "item.record_many", "batch");
    input["operations"] = json!([
        {"item_id":"note","expected_item_revision":0,"record":{"type":"text","value":"Retain the note."}}
    ]);
    for (flag, value) in [
        (
            "--if-workspace-uuid",
            "00000000-0000-4000-8000-000000000001",
        ),
        ("--if-session-id", "00000000-0000-4000-8000-000000000001"),
        ("--if-session-revision", "1"),
        ("--if-attempt", "00000000-0000-4000-8000-000000000001"),
        ("--if-item-revision", "0"),
        ("--if-cursor", "{}"),
        ("--idempotency-key", "another-key"),
    ] {
        let bytes = input.to_string();
        let attempt = output_with_stdin_counted(
            &fixture,
            &["record", "--stdin", flag, value],
            bytes.as_bytes(),
        );
        assert_read004_counters(&attempt, (0, 0));
        scoped_error(attempt, "REQUEST_INVALID");
    }
    assert_eq!(exact_outcome(&fixture, "batch")["found"], false);
    assert_eq!(
        fixture.run(&["--contract-generation", "2", "observe"])["result"],
        initial["result"]
    );
    let set = output_with_stdin_counted(
        &fixture,
        &["set", "note", "--stdin", "--idempotency-key", "set-note"],
        b"Current input
",
    );
    assert_read004_counters(&set, (1, 0));
    assert_eq!(scoped_result(set)["result"]["items"][0]["revision"], 1);
    assert_eq!(exact_outcome(&fixture, "set-note")["found"], true);
    assert_eq!(exact_outcome(&fixture, "batch")["found"], false);
}

#[test]
fn agent003_goal_assess_criteria_stdin_fences_conflicts_and_preflight_free_execution() {
    let (fixture, before, _) = super::e2e_adopt001_scoped_cli::scoped_criterion_fixture(true);
    let mut input = scoped_template(&before, "goal.assess_criteria", "assessment");
    input["actor"] = json!("fixture-evaluator");
    input["assessments"] = json!([{
        "criterion":"proof","status":"satisfied","reason":"The group returned an eligible result.",
        "citations":[{"kind":"execution_result"}]
    }]);
    let args = &["goal", "assess-criteria", "--stdin", "--with-observation"];
    for (field, value, expected) in [
        ("goal_revision", json!(2), "GOAL_REVISION_STALE"),
        ("session_revision", json!(999), "SESSION_REVISION_CONFLICT"),
    ] {
        let mut stale = input.clone();
        stale["context"]["cursor"][field] = value;
        let output = output_with_stdin_counted(&fixture, args, stale.to_string().as_bytes());
        assert_read004_counters(&output, (0, 0));
        scoped_error(output, expected);
        assert_eq!(exact_outcome(&fixture, "assessment")["found"], false);
    }
    let bytes = input.to_string();
    let conflict = output_with_stdin_counted(
        &fixture,
        &[
            "goal",
            "assess-criteria",
            "--stdin",
            "--if-goal-revision",
            "1",
        ],
        bytes.as_bytes(),
    );
    assert_read004_counters(&conflict, (0, 0));
    scoped_error(conflict, "REQUEST_INVALID");
    assert_eq!(
        fixture.run(&["--contract-generation", "2", "observe"])["result"],
        before["result"]
    );
    let output = output_with_stdin_counted(&fixture, args, bytes.as_bytes());
    assert_read004_counters(&output, (0, 0));
    let accepted = scoped_result(output);
    assert_eq!(
        accepted["result"]["observation"]["goal"]["outcome"],
        "achieved"
    );
    assert_eq!(
        scoped_result(fixture.output_with_stdin(args, bytes.as_bytes())),
        accepted
    );
    assert_eq!(
        exact_outcome(&fixture, "assessment")["outcome"]["response"],
        accepted
    );
}
