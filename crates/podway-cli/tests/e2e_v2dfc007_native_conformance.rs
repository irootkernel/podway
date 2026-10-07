//! Consumer identity and workspace lifecycle through the single production CLI.

use std::{collections::BTreeSet, fs, process::Command};

use podway_protocol::ErrorEnvelopeV2;
use serde_json::Value;

use super::cli_fixture::Fixture;

const GOAL: &str = include_str!("../../../tests/fixtures/v2/procedures/goal-outcomes.yaml");

fn failure(fixture: &Fixture, argv: &[&str], code: &str, exit: i32) -> Value {
    let output = fixture.output(argv);
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], code);
    error
}

fn prepared(fixture: &Fixture) -> Value {
    fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(fixture.0.join("procedure.yaml"), GOAL).unwrap();
    let preview = fixture.run(&["procedure", "preview", "procedure.yaml"]);
    let start = fixture.run(&[
        "start",
        "--procedure",
        "procedure.yaml",
        "--expect-procedure-digest",
        preview["result"]["procedure_digest"].as_str().unwrap(),
        "--task",
        "Native consumer conformance",
        "--idempotency-key",
        "start",
    ]);
    assert_eq!(
        start["result"]["procedure_digest"],
        preview["result"]["procedure_digest"]
    );
    assert_eq!(start["result"]["session_state"], "prepared");
    assert_eq!(start["result"]["revision"], 0);
    assert!(start["result"]["active_attempt"].is_null());
    assert_eq!(
        fixture.run(&["status"])["result"]["procedure"]["digest"],
        preview["result"]["procedure_digest"]
    );
    start
}

#[test]
fn compact_status_keeps_only_bounded_metadata_and_current_identity() {
    let fixture = Fixture::new();
    prepared(&fixture);
    fixture.run(&[
        "begin",
        "--goal",
        "Bounded public status.",
        "--criterion",
        "bounded=Status contains metadata only.",
        "--idempotency-key",
        "begin",
    ]);
    let compact = fixture.run(&["status", "--compact"]);
    assert_eq!(compact["command"], "session.status");
    let result = compact["result"].as_object().unwrap();
    assert_eq!(
        result.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "counters",
            "current",
            "goal_defined",
            "goal_revision",
            "goal_tracking",
            "items",
            "procedure",
            "schema",
            "session",
            "trace_length",
        ])
    );
    assert_eq!(result["schema"], "podway.compact-status-result/v5");
    assert_eq!(result["goal_defined"], true);
    assert_eq!(result["goal_revision"], 1);
    assert!(
        compact["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .is_some_and(|sequence| sequence >= 3)
    );
    let encoded = serde_json::to_string(&compact).unwrap();
    assert!(encoded.len() < 262_144);
    for forbidden in [
        "\"instructions\"",
        "\"prompt\"",
        "\"task\"",
        "\"title\"",
        "\"value\"",
        "\"queue\"",
        "\"job\"",
    ] {
        assert!(!encoded.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn procedure_workspace_and_session_conflicts_leave_no_mutation_or_outcome() {
    let fixture = Fixture::new();
    let started = prepared(&fixture);
    let before = fixture.run(&["observe"]);
    let wrong = uuid::Uuid::new_v4().to_string();
    let workspace = failure(
        &fixture,
        &["status", "--if-workspace-uuid", &wrong],
        "WORKSPACE_UUID_MISMATCH",
        4,
    );
    assert_eq!(
        workspace["details"]["actual_workspace_uuid"],
        before["workspace"]["uuid"]
    );
    assert_eq!(workspace["retryable"], false);
    let session = failure(
        &fixture,
        &["status", "--if-session-id", &wrong],
        "SESSION_ID_MISMATCH",
        4,
    );
    assert_eq!(
        session["details"]["actual_session_id"],
        started["session"]["id"]
    );
    assert_eq!(session["retryable"], false);
    let bad_digest = format!("sha256:{}", "f".repeat(64));
    let mismatch = failure(
        &fixture,
        &[
            "start",
            "--procedure",
            "procedure.yaml",
            "--expect-procedure-digest",
            &bad_digest,
            "--task",
            "Mismatch",
            "--on-existing",
            "delete",
            "--idempotency-key",
            "bad-digest",
        ],
        "PROCEDURE_DIGEST_MISMATCH",
        4,
    );
    assert_eq!(mismatch["retryable"], false);
    assert_eq!(
        mismatch["details"]["actual_procedure_digest"],
        started["result"]["procedure_digest"]
    );
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
    for key in ["bad-digest", "never-recorded"] {
        let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", key]);
        assert_eq!(
            lookup["result"]["schema"],
            "podway.outcome-lookup-result/v1"
        );
        assert_eq!(lookup["result"]["found"], false);
    }
}

#[test]
fn plain_init_rejects_an_existing_workspace_but_replays_the_original_success() {
    let fixture = Fixture::new();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&["workspace", "show"]);
    let before = fixture.podway_snapshot();
    assert_eq!(
        fixture.run(&["init", "--idempotency-key", "init"]),
        initialized
    );
    // Exact mutation replay may update SQLite's volatile shared WAL index.
    // Its authoritative files and identities must still remain unchanged.
    assert_eq!(
        fixture
            .podway_snapshot()
            .into_iter()
            .filter(|entry| entry.0 != std::path::Path::new("runtime/state.sqlite3-shm"))
            .collect::<Vec<_>>(),
        before
            .into_iter()
            .filter(|entry| entry.0 != std::path::Path::new("runtime/state.sqlite3-shm"))
            .collect::<Vec<_>>()
    );
    let before = fixture.podway_snapshot();
    failure(
        &fixture,
        &["init", "--idempotency-key", "second-init"],
        "WORKSPACE_ALREADY_INITIALIZED",
        1,
    );
    fixture.assert_podway_snapshot(&before);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "second-init"])["result"]["found"],
        false
    );
}

#[test]
fn destructive_lifecycle_preserves_user_files_and_config_and_reinitializes_identity() {
    let fixture = Fixture::new();
    let started = prepared(&fixture);
    let user = fixture.0.join("retained.txt");
    fs::write(&user, "preserved outside Podway\n").unwrap();
    let config = fs::read(fixture.0.join(".podway/config.yaml")).unwrap();
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let reset = fixture.run(&[
        "reset",
        "--yes",
        "--progress-summary",
        "Discard only the fixture session.",
        "--idempotency-key",
        "session-reset",
    ]);
    assert_eq!(reset["result"]["schema"], "podway.session-reset-result/v2");
    assert_eq!(reset["result"]["reset"], true);
    failure(&fixture, &["status"], "SESSION_NOT_FOUND", 1);
    let reset = fixture.run(&[
        "reset",
        "--all",
        "--yes",
        "--if-workspace-uuid",
        started["workspace"]["uuid"].as_str().unwrap(),
        "--idempotency-key",
        "workspace-reset",
    ]);
    assert_ne!(reset["workspace"]["uuid"], started["workspace"]["uuid"]);
    assert_eq!(
        fs::read(fixture.0.join(".podway/config.yaml")).unwrap(),
        config
    );
    assert_eq!(
        fs::read_to_string(&user).unwrap(),
        "preserved outside Podway\n"
    );
    failure(&fixture, &["status"], "SESSION_NOT_FOUND", 1);
    let repaired = fixture.run(&["workspace", "repair", "--idempotency-key", "repair"]);
    assert_eq!(repaired["workspace"]["uuid"], reset["workspace"]["uuid"]);
    assert_eq!(
        fixture.run(&["workspace", "repair", "--idempotency-key", "repair"]),
        repaired
    );
    assert_eq!(
        fixture.run(&["workspace", "repair", "--idempotency-key", "new-repair"])["result"]["changed"],
        false
    );
    let removed = fixture.run(&[
        "workspace",
        "remove",
        "--force",
        "--yes",
        "--if-workspace-uuid",
        reset["workspace"]["uuid"].as_str().unwrap(),
        "--idempotency-key",
        "remove",
    ]);
    assert_eq!(
        removed["result"]["schema"],
        "podway.workspace-removal-result/v2"
    );
    assert_eq!(
        removed["result"]["workspace_uuid"],
        reset["workspace"]["uuid"]
    );
    assert!(!fixture.0.join(".podway").exists());
    assert!(fixture.0.join(".git").exists());
    assert_eq!(
        fs::read_to_string(&user).unwrap(),
        "preserved outside Podway\n"
    );
    let initialized = fixture.run(&["init", "--idempotency-key", "reinit"]);
    assert_ne!(initialized["workspace"]["uuid"], reset["workspace"]["uuid"]);
    assert_eq!(
        fixture.run(&["workspace", "show"])["workspace"]["uuid"],
        initialized["workspace"]["uuid"]
    );
}

#[test]
fn public_goal_flow_preserves_rework_and_stale_attempt_history_across_processes() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(
        fixture.0.join("public.yaml"),
        include_str!("../../../tests/fixtures/v2/procedures/public-native-goal.yaml"),
    )
    .unwrap();
    let preview = fixture.run(&["procedure", "preview", "public.yaml"]);
    let digest = preview["result"]["procedure_digest"].as_str().unwrap();
    fixture.run(&[
        "start",
        "--procedure",
        "public.yaml",
        "--expect-procedure-digest",
        digest,
        "--task",
        "Public goal lifecycle",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&[
        "begin",
        "--goal",
        "Verify the public lifecycle.",
        "--criterion",
        "qualified=Rework and closeout retain their identities.",
        "--actor",
        "owner",
        "--idempotency-key",
        "begin",
    ]);
    let initial = fixture.run(&["status"]);
    let session = initial["result"]["session"]["id"].as_str().unwrap();
    let first_attempt = initial["result"]["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap();
    fixture.run(&[
        "set",
        "result",
        "First evidence.",
        "--idempotency-key",
        "first-result",
    ]);
    fixture.run(&["complete", "--idempotency-key", "first-complete"]);
    let reworked = fixture.run(&[
        "decide",
        "--option",
        "revise",
        "--reason",
        "Refresh the evidence.",
        "--actor",
        "reviewer",
        "--idempotency-key",
        "revise",
    ]);
    assert_eq!(reworked["result"]["effect"], "rework");
    assert_eq!(reworked["result"]["target_graph_node_id"], "work");
    let cold = fixture.run(&["status", "--verbose"]);
    assert_eq!(cold["result"]["session"]["id"], session);
    assert_eq!(cold["result"]["goal_revision"], 1);
    assert_eq!(cold["result"]["procedure"]["digest"], digest);
    assert_ne!(
        cold["result"]["current"]["attempt"]["attempt_id"],
        first_attempt
    );
    assert_eq!(
        cold["result"]["decision_history"]["entries"][0]["option_id"],
        "revise"
    );
    assert!(
        !cold["result"]["rework_history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !cold["result"]["stale_attempt_history"]["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fixture.run(&[
        "set",
        "result",
        "Replacement evidence.",
        "--idempotency-key",
        "second-result",
    ]);
    fixture.run(&["complete", "--idempotency-key", "second-complete"]);
    let accepted = fixture.run(&[
        "decide",
        "--option",
        "accept",
        "--reason",
        "Evidence is complete.",
        "--actor",
        "reviewer",
        "--idempotency-key",
        "accept",
    ]);
    assert_eq!(accepted["result"]["target_graph_node_id"], "assess");
    let assessed = fixture.run(&[
        "goal",
        "assess-criterion",
        "qualified",
        "--status",
        "satisfied",
        "--reason",
        "Rework preserved the public state.",
        "--evidence",
        "work",
        "--actor",
        "reviewer",
        "--idempotency-key",
        "assess",
    ]);
    assert_eq!(assessed["result"]["complete"], true);
    assert_eq!(assessed["result"]["determined_outcome"], "achieved");
    fixture.run(&[
        "decide",
        "--option",
        "achieved",
        "--reason",
        "The criterion is met.",
        "--actor",
        "owner",
        "--idempotency-key",
        "achieved",
    ]);
    fixture.run(&[
        "set",
        "closeout",
        "Verified final state.",
        "--idempotency-key",
        "closeout",
    ]);
    fixture.run(&["complete", "--idempotency-key", "finish"]);
    let terminal = fixture.run(&["status"]);
    assert_eq!(terminal["result"]["session"]["id"], session);
    assert_eq!(terminal["result"]["session"]["lifecycle"], "completed");
    assert!(terminal["result"]["current"].is_null());
    assert_eq!(terminal["result"]["latest_goal_outcome"], "achieved");
    fixture.run(&[
        "disposition",
        "not-required",
        "--reason",
        "No external handoff is required.",
        "--idempotency-key",
        "disposition",
    ]);
    let reset = fixture.run(&["reset", "--idempotency-key", "reset"]);
    assert_eq!(reset["result"]["mode"], "eligible");
    assert_eq!(reset["result"]["reset"], true);
    failure(&fixture, &["status"], "SESSION_NOT_FOUND", 1);
}

#[test]
fn workspace_survives_git_object_replacement_at_the_same_canonical_paths() {
    let fixture = Fixture::new();
    let initialized = fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(fixture.0.join("retained.txt"), "retained\n").unwrap();
    fs::remove_dir_all(fixture.0.join(".git")).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&fixture.0)
            .status()
            .unwrap()
            .success()
    );
    for _ in 0..2 {
        assert_eq!(
            fixture.run(&["workspace", "show"])["workspace"]["uuid"],
            initialized["workspace"]["uuid"]
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.0.join("retained.txt")).unwrap(),
        "retained\n"
    );
}
