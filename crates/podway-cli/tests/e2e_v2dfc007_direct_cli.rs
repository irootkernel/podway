//! Retained commands execute through the real CLI and worktree-local direct Store.

use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::cli_fixture::{Fixture, GuardedChild, podway_binary};
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
fn v2dfc017_oversize_record_and_set_stdin_refuse_without_runtime_effects() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "sw-dev-v2",
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
        "SELECT rootpage FROM sqlite_schema WHERE tbl_name IN ('mutation_outcomes','v2_task_sessions') AND rootpage > 0",
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
                        "small-change-v2",
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
                        "podway.workspace-schema-unsupported-details/v1"
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
        "small-change-v2",
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
        "small-change-v2",
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
        rendered.contains(before["result"]["session"]["id"].as_str().unwrap()),
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
        "small-change-v2",
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
