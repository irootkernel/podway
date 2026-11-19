//! Consumer identity and workspace lifecycle through the single production CLI.

use std::{collections::BTreeSet, fs, process::Command};

use podway_protocol::ErrorEnvelopeV2;
use serde_json::{Value, json};

use super::cli_fixture::Fixture;

const BASE_FILE: &str = "procedure.json";
const GROUP_FILE: &str = "group.json";

fn result_item() -> Value {
    json!({"id":"result","type":"text","prompt":"Record the result.","required":true})
}

fn base() -> Value {
    json!({
        "schema":"podway.procedure/v3","id":"native-conformance","version":"1",
        "name":"Native conformance","purpose":"Exercise the public lifecycle and retained work.",
        "goal_tracking":true,
        "node_definitions":{
            "plan":{"type":"action","title":"Plan","intent":"Record the work basis.","items":[
                {"id":"plan-note","type":"text","prompt":"Record the plan.","required":true}
            ]},
            "assess":{
                "type":"decision","title":"Assess","objective":"Assess the selected evidence.",
                "prompt":"What is the current goal outcome?",
                "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},
                    {"id":"superseded","label":"Superseded"}],
                "reason":{"required":true},
                "assessment":{"target":"session_goal","outcomes":{
                    "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"
                }}
            },
            "finish":{"type":"action","title":"Finish","intent":"Record the closeout.","items":[
                {"id":"closeout","type":"text","prompt":"Record the closeout.","required":true}
            ]}
        },
        "graph":{"entry":"plan","nodes":[
            {"id":"plan","use":"plan","next":"assess"},
            {"id":"assess","use":"assess",
                "evidence_from":[{"scope":"execution_result","required":true,"items":["result"]}],
                "routes":{
                    "achieved":{"effect":"advance","to":"finish"},
                    "not-achieved":{"effect":"advance","to":"finish"},
                    "superseded":{"effect":"advance","to":"finish"}
                }
            },
            {"id":"finish","use":"finish","terminal":true}
        ]},
        "execution_group":{"anchor":"plan","success_return":"assess","plan_inputs":["plan-note"],
            "result_items":[result_item()],"replan_decisions":[]
        }
    })
}

fn group() -> Value {
    json!({
        "schema":"podway.execution-group/v1","id":"native-conformance-group","version":"1",
        "name":"Native conformance group","purpose":"Exercise local rework with retained evidence.",
        "plan_inputs":[{"id":"plan-note","type":"text","prompt":"Read the plan.","required":true}],
        "node_definitions":{
            "work":{"type":"action","title":"Work","intent":"Record the work.","items":[result_item()]},
            "review":{
                "type":"decision","title":"Review work","objective":"Check the current result.",
                "prompt":"Is the evidence ready?",
                "options":[{"id":"accept","label":"Accept"},{"id":"revise","label":"Revise"}],
                "reason":{"required":true}
            },
            "export":{"type":"action","title":"Export","intent":"Settle the result.","items":[result_item()]}
        },
        "graph":{"entry":"work","nodes":[
            {"id":"work","use":"work","next":"review",
                "evidence_from":[{"scope":"plan","required":true,"items":["plan-note"]}]},
            {"id":"review","use":"review",
                "evidence_from":[{"scope":"local","node":"work","required":true,"items":["result"]}],
                "routes":{"accept":{"effect":"advance","to":"export"},"revise":{"effect":"rework","to":"work"}}},
            {"id":"export","use":"export","terminal":true,
                "evidence_from":[{"scope":"local","node":"work","required":true,"items":["result"]}]}
        ]}
    })
}

fn observe(fixture: &Fixture) -> Value {
    fixture.run(&["--contract-generation", "2", "observe"])
}

fn begin(fixture: &Fixture, goal: &str, criterion: &str) {
    fixture.run(&[
        "begin",
        "--goal",
        goal,
        "--criterion",
        criterion,
        "--actor",
        "owner",
        "--idempotency-key",
        "begin",
    ]);
}

fn activate(fixture: &Fixture) -> Value {
    let source = group().to_string();
    fs::write(fixture.0.join(GROUP_FILE), &source).unwrap();
    let digest = fixture.run(&["procedure", "preview", GROUP_FILE])["result"]["digest"].clone();
    let ready = observe(fixture);
    let template = ready["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| template["command"] == "group.activate")
        .unwrap();
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!("activate");
    input["source"] = json!({"kind":"document","format":"json","source":source});
    input["expected_definition"] = digest;
    input["working_copy_basis"] = json!({
        "descriptor":"Owned native conformance fixture",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111"
    });
    let output = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        input.to_string().as_bytes(),
    );
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice::<podway_protocol::OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value()
}

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
    fs::write(fixture.0.join(BASE_FILE), base().to_string()).unwrap();
    let preview = fixture.run(&["procedure", "preview", BASE_FILE]);
    let start = fixture.run(&[
        "start",
        "--procedure",
        BASE_FILE,
        "--expect-procedure-digest",
        preview["result"]["digest"].as_str().unwrap(),
        "--task",
        "Native consumer conformance",
        "--idempotency-key",
        "start",
    ]);
    assert_eq!(
        start["result"]["procedure_digest"],
        preview["result"]["digest"]
    );
    assert_eq!(start["result"]["session_state"], "prepared");
    assert_eq!(start["result"]["revision"], 0);
    assert!(start["result"]["active_attempt"].is_null());
    assert_eq!(
        fixture.run(&["status"])["result"]["base"]["digest"],
        preview["result"]["digest"]
    );
    start
}

#[test]
fn compact_status_keeps_only_bounded_metadata_and_current_identity() {
    let fixture = Fixture::new();
    let started = prepared(&fixture);
    begin(
        &fixture,
        "Bounded public status.",
        "bounded=Status contains metadata only.",
    );
    fixture.run(&[
        "set",
        "plan-note",
        "Private fixture item contents.",
        "--idempotency-key",
        "plan",
    ]);
    let current = observe(&fixture);
    let compact = fixture.run(&["status", "--compact"]);
    assert_eq!(compact["command"], "session.status");
    let result = compact["result"].as_object().unwrap();
    assert_eq!(
        result.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "base",
            "cursor",
            "items",
            "lifecycle",
            "retained_group_count",
            "schema",
            "trace_length",
            "workspace",
            "workspace_sequence",
        ])
    );
    assert_eq!(result["schema"], "podway.scoped-compact-status-result/v1");
    assert_eq!(
        result["base"]["digest"],
        started["result"]["procedure_digest"]
    );
    assert_eq!(result["base"]["goal_tracking"], true);
    assert_eq!(result["cursor"], current["result"]["cursor"]);
    assert_eq!(result["cursor"]["goal_revision"], 1);
    assert_eq!(
        result["items"],
        json!([{"item":"plan-note","revision":1,"recorded":true}])
    );
    assert_eq!(result["trace_length"], 1);
    assert!(
        compact["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .is_some_and(|seq| seq >= 4)
    );
    assert!(serde_json::to_string(&compact).unwrap().len() < 262_144);
    let encoded = serde_json::to_string(result).unwrap();
    for forbidden in [
        "\"instructions\"",
        "\"prompt\"",
        "\"task\"",
        "\"title\"",
        "\"value\"",
        "\"queue\"",
        "\"job\"",
        "Private fixture item contents.",
    ] {
        assert!(!encoded.contains(forbidden), "{forbidden}");
    }
    let generation_two = fixture.run(&["--contract-generation", "2", "status", "--compact"]);
    assert_eq!(generation_two["result"], compact["result"]);
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
            BASE_FILE,
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
    let after = fixture.run(&["observe"]);
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after["workspace"], before["workspace"]);
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
    begin(
        &fixture,
        "Track the disposable task.",
        "task=Retain the user files.",
    );
    let reset = fixture.run(&[
        "reset",
        "--yes",
        "--progress-summary",
        "Discard only the fixture session.",
        "--idempotency-key",
        "session-reset",
    ]);
    assert_eq!(
        reset["result"]["schema"],
        "podway.successor-session-reset-result/v1"
    );
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
    let started = prepared(&fixture);
    let digest = started["result"]["procedure_digest"].as_str().unwrap();
    let session = started["session"]["id"].as_str().unwrap();
    begin(
        &fixture,
        "Verify the public lifecycle.",
        "qualified=Rework preserves attempt identities.",
    );
    fixture.run(&[
        "set",
        "plan-note",
        "Retain the earlier work.",
        "--idempotency-key",
        "plan",
    ]);
    let activated = activate(&fixture);
    let execution = activated["result"]["execution_id"].as_str().unwrap();
    let first = observe(&fixture);
    let first_attempt = &first["result"]["cursor"]["attempt"];
    let group_digest = first["result"]["selection"]["definition_digest"]
        .as_str()
        .unwrap();
    assert_eq!(first_attempt["scope"]["kind"], "group");
    fixture.run(&[
        "set",
        "result",
        "First evidence.",
        "--idempotency-key",
        "first-result",
    ]);
    fixture.run(&["complete", "--idempotency-key", "first-complete"]);
    let review = observe(&fixture);
    let review_attempt = &review["result"]["cursor"]["attempt"];
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
    assert_eq!(reworked["result"]["cursor"]["attempt"]["node"], "work");

    let cold = fixture.run(&["status", "--verbose"]);
    assert_eq!(
        cold["result"]["schema"],
        "podway.scoped-verbose-status-result/v1"
    );
    let observation = &cold["result"]["observation"];
    assert_eq!(observation["cursor"]["session"], session);
    assert_eq!(observation["cursor"]["goal_revision"], 1);
    assert_eq!(observation["base"]["digest"], digest);
    assert_ne!(observation["cursor"]["attempt"], *first_attempt);
    assert_eq!(observation["cursor"], reworked["result"]["cursor"]);
    let attempts = cold["result"]["attempts"].as_array().unwrap();
    assert!(
        attempts
            .windows(2)
            .all(|pair| pair[0]["trace_sequence"].as_u64().unwrap()
                > pair[1]["trace_sequence"].as_u64().unwrap())
    );
    let first_row = attempts
        .iter()
        .find(|row| row["attempt"] == *first_attempt)
        .unwrap();
    assert_eq!(first_row["lifecycle"], "completed");
    assert_eq!(first_row["validity"], "stale");
    let recorded_review = attempts
        .iter()
        .find(|row| row["attempt"] == *review_attempt)
        .unwrap();
    assert_eq!(recorded_review["decision"]["option"], "revise");
    assert_eq!(recorded_review["actor"], "reviewer");
    let history = fixture.run(&[
        "history",
        "attempts",
        "--session",
        session,
        "--group-execution",
        execution,
        "--definition-digest",
        group_digest,
    ]);
    assert_eq!(
        history["result"]["schema"],
        "podway.successor-history-attempts-result/v1"
    );
    assert_eq!(history["result"]["total"], 3);
    assert_eq!(history["result"]["rows"][0], *first_row);
    let retained = fixture.run(&[
        "history",
        "item",
        "--session",
        session,
        "--attempt",
        &first_attempt.to_string(),
        "--item",
        "result",
    ]);
    assert_eq!(
        retained["result"]["detail"]["source_attempt"]["attempt"],
        *first_attempt
    );
    assert_eq!(retained["result"]["data"], "First evidence.");

    let before_stale = observe(&fixture);
    failure(
        &fixture,
        &[
            "set",
            "result",
            "Obsolete evidence.",
            "--if-workspace-uuid",
            first["workspace"]["uuid"].as_str().unwrap(),
            "--if-cursor",
            &first["result"]["cursor"].to_string(),
            "--if-item-revision",
            "1",
            "--idempotency-key",
            "stale-attempt",
        ],
        "ATTEMPT_NOT_CURRENT",
        4,
    );
    assert_eq!(observe(&fixture)["result"], before_stale["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "stale-attempt"])["result"]["found"],
        false
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
    assert_eq!(accepted["result"]["cursor"]["attempt"]["node"], "export");
    fixture.run(&[
        "set",
        "result",
        "Replacement evidence.",
        "--idempotency-key",
        "export",
    ]);
    fixture.run(&["complete", "--idempotency-key", "return"]);
    assert_eq!(
        observe(&fixture)["result"]["cursor"]["attempt"]["scope"]["kind"],
        "base"
    );
    fixture.run(&[
        "goal",
        "assess-criterion",
        "qualified",
        "--status",
        "satisfied",
        "--reason",
        "Rework preserved the public state.",
        "--evidence",
        "@execution-result",
        "--actor",
        "reviewer",
        "--idempotency-key",
        "assess",
    ]);
    let assessed = observe(&fixture);
    assert_eq!(
        assessed["result"]["goal"]["criteria"][0]["status"],
        "satisfied"
    );
    assert_eq!(assessed["result"]["goal"]["outcome"], "achieved");
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
    assert_eq!(terminal["result"]["cursor"]["session"], session);
    assert_eq!(terminal["result"]["lifecycle"], "completed");
    assert!(terminal["result"]["cursor"]["attempt"].is_null());
    assert_eq!(terminal["result"]["goal"]["outcome"], "achieved");
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
