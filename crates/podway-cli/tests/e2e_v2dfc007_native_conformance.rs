//! Consumer identity and workspace lifecycle through the single production CLI.

use std::{collections::BTreeSet, fs, process::Command};

use podway_core::{MAX_EVIDENCE_PAGE_BYTES_V2, MAX_TEXT_SCALARS_V2};
use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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
    activate_document(fixture, group())
}

fn activate_document(fixture: &Fixture, document: Value) -> Value {
    let mut input = activation_input(fixture, document);
    input["context"]["idempotency_key"] = json!("activate");
    input["working_copy_basis"] = json!({
        "descriptor":"Owned native conformance fixture",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111"
    });
    submit_activation(fixture, &input)
}

fn activation_input(fixture: &Fixture, document: Value) -> Value {
    let source = document.to_string();
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
    input["source"] = json!({"kind":"document","format":"json","source":source});
    input["expected_definition"] = digest;
    input
}

fn submit_activation(fixture: &Fixture, input: &Value) -> Value {
    let output = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        input.to_string().as_bytes(),
    );
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value()
}

#[test]
fn successor_cli_retains_first_group_work_across_an_empty_replacement() {
    let fixture = Fixture::new();
    let started = prepared(&fixture);
    let session = started["session"]["id"].as_str().unwrap();
    let source = fixture.0.join("work.txt");
    fs::write(&source, b"Original worktree contents.\n").unwrap();
    let basis = || format!("sha256:{:x}", Sha256::digest(fs::read(&source).unwrap()));
    let criterion = "fresh=Current evidence must be recorded again.";
    begin(&fixture, "Retain fixture work.", criterion);

    let activate = |key: &str, document: Value, predecessor: &Value, prior: &Value| {
        let mut input = activation_input(&fixture, document);
        input["context"]["idempotency_key"] = json!(key);
        input["predecessor"] = predecessor.clone();
        input["prior_work"] = prior.clone();
        input["working_copy_basis"] =
            json!({"descriptor":"Exact fixture work.txt bytes","digest":basis()});
        submit_activation(&fixture, &input)["result"]["execution_id"].clone()
    };
    let plan = |key: &str, note: &str| {
        fixture.run(&["set", "plan-note", note, "--idempotency-key", key]);
    };
    let revise = |key: &str, title: &str| {
        fixture.run(&[
            "goal",
            "revise",
            "--goal",
            title,
            "--criterion",
            criterion,
            "--rework-to",
            "plan",
            "--reason",
            "The remaining work changed.",
            "--basis-description",
            "Exact fixture work.txt bytes",
            "--basis-digest",
            &basis(),
            "--feedback",
            "Review the retained work under the new goal.",
            "--idempotency-key",
            key,
        ])
    };
    plan("plan-one", "First plan");
    let first_basis = basis();
    let first = activate("first", group(), &Value::Null, &json!([]));
    let first_state = observe(&fixture);
    let attempt = &first_state["result"]["cursor"]["attempt"];
    fixture.run(&[
        "set",
        "result",
        "Completed work from the first group.",
        "--idempotency-key",
        "first-result",
    ]);
    fixture.run(&["complete", "--idempotency-key", "first-work-complete"]);
    let history = [
        "history",
        "item",
        "--session",
        session,
        "--attempt",
        &attempt.to_string(),
        "--item",
        "result",
    ];
    let original = fixture.run(&history);
    let original = &original["result"];
    let retained = json!([{"group_execution_id":first,"choice":"revalidate","rationale":"Confirm the existing result under the revised plan.",
                           "item":{"attempt":attempt["attempt"],"item":"result","value_digest":original["detail"]["value_digest"]}}]);

    fs::write(&source, b"Keep completed work and update its boundary.\n").unwrap();
    let revised_basis = basis();
    assert_ne!(first_basis, revised_basis);
    revise("revision-two", "Reconcile the existing result.");
    plan("plan-two", "Second plan");
    let second = activate("second", group(), &first, &retained);
    let empty = observe(&fixture);
    assert_ne!(second, first);
    assert_eq!(
        empty["result"]["selection"]["definition_digest"],
        first_state["result"]["selection"]["definition_digest"]
    );
    assert_eq!(empty["result"]["items"][0]["revision"], 0);
    assert_eq!(empty["result"]["items"][0]["item"], "result");
    assert_eq!(empty["result"]["items"][0]["recorded"], false);

    revise("revision-three", "Qualify the new execution definition.");
    plan("plan-three", "Third plan");
    let mut replacement = group();
    replacement["name"] = json!("Revised execution definition");
    let third = activate("third", replacement, &second, &retained);
    let selected = observe(&fixture);
    assert_ne!(third, second);
    assert_eq!(selected["result"]["cursor"]["goal_revision"], 3);
    assert_ne!(
        selected["result"]["selection"]["definition_digest"],
        empty["result"]["selection"]["definition_digest"]
    );
    assert_eq!(selected["result"]["items"][0]["recorded"], false);
    assert_eq!(
        fs::read(&source).unwrap(),
        b"Keep completed work and update its boundary.\n"
    );

    let before = fixture.podway_snapshot();
    let list = fixture.run(&["group", "list", "--session", session]);
    let rows = list["result"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for (row, execution) in rows.iter().zip([&first, &second, &third]) {
        assert_eq!(&row["execution"], execution);
    }
    assert_eq!(rows[0]["outcome"], "abandoned");
    assert_eq!(rows[1]["outcome"], "abandoned");
    assert_eq!(rows[2]["predecessor"], second);
    for (execution, expected) in [(&first, first_basis), (&third, revised_basis)] {
        let shown = fixture.run(&[
            "group",
            "show",
            "--session",
            session,
            "--execution",
            execution.as_str().unwrap(),
        ]);
        assert_eq!(
            shown["result"]["group"]["working_copy_basis"]["digest"],
            expected
        );
    }
    let prior = fixture.run(&[
        "group",
        "prior-work",
        "--session",
        session,
        "--execution",
        third.as_str().unwrap(),
        "--definition-digest",
        selected["result"]["selection"]["definition_digest"]
            .as_str()
            .unwrap(),
    ]);
    assert_eq!(prior["result"]["total"], 1);
    let reference = &prior["result"]["rows"][0]["source"];
    assert_eq!(reference["group"]["execution"], first);
    assert_eq!(
        reference["item"]["value_digest"],
        original["detail"]["value_digest"]
    );
    let current = fixture.run(&history);
    assert_eq!(current["result"]["data"], original["data"]);
    assert_eq!(
        current["result"]["detail"]["value_digest"],
        original["detail"]["value_digest"]
    );
    fixture.assert_podway_snapshot(&before);
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
fn retained_item_pages_are_bounded_and_read_only_through_archive_and_purge() {
    let fixture = Fixture::new();
    let started = prepared(&fixture);
    let session = started["session"]["id"].as_str().unwrap();
    begin(
        &fixture,
        "Exercise history bounds.",
        "retained=Paged work survives archiving.",
    );
    fixture.run(&[
        "set",
        "plan-note",
        "Keep earlier evidence.",
        "--idempotency-key",
        "plan",
    ]);

    let mut document = group();
    document["node_definitions"]["work"]["items"]
        .as_array_mut()
        .unwrap()
        .push(
            json!({"id":"memo","type":"text","prompt":"Record a memo.","required":false,
                     "min_length":0,"max_length":MAX_TEXT_SCALARS_V2}),
        );
    let activated = activate_document(&fixture, document);
    let execution = activated["result"]["execution_id"].as_str().unwrap();
    let state = observe(&fixture);
    let attempt = state["result"]["cursor"]["attempt"].to_string();
    let text: String = "\u{0001}\u{0002}\u{0003}\u{0004}\u{0005}\u{0006}\u{0007}🧭éa"
        .chars()
        .cycle()
        .take(MAX_TEXT_SCALARS_V2 as usize)
        .collect();
    assert!(serde_json::to_vec(&text).unwrap().len() > MAX_EVIDENCE_PAGE_BYTES_V2);
    let record = |key: &str, value: &str, observation: &Value, revision| {
        let mut cursor = observation["result"]["cursor"].as_object().unwrap().clone();
        cursor.remove("session");
        let request = json!({
            "schema":"podway.scoped-item-record-many-input/v1",
            "context":{"workspace_uuid":observation["workspace"]["uuid"],"idempotency_key":key,"cursor":cursor},
            "operations":[{"item_id":"memo","expected_item_revision":revision,"record":{"type":"text","value":value}}]
        });
        fixture.output_with_stdin(
            &["record", "--stdin"],
            &serde_json::to_vec(&request).unwrap(),
        )
    };
    let recorded = record("max-memo", &text, &state, 0);
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stdout)
    );
    let recorded = serde_json::from_slice::<OutputEnvelopeV4>(&recorded.stdout)
        .unwrap()
        .into_value();
    let after = observe(&fixture);
    let excessive = record("one-over", &format!("{text}x"), &after, 1);
    assert!(!excessive.status.success());
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&excessive.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "REQUEST_INVALID");
    assert_eq!(observe(&fixture)["result"], after["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "one-over"])["result"]["found"],
        false
    );

    fixture.run(&[
        "cancel",
        "--reason",
        "Retain the interrupted group.",
        "--idempotency-key",
        "cancel",
    ]);
    let item = [
        "history",
        "item",
        "--session",
        session,
        "--attempt",
        &attempt,
        "--item",
        "memo",
    ];
    let pages = |query: &[&str]| {
        let before = fixture.podway_snapshot();
        let first = fixture.run(query);
        let first = &first["result"];
        assert_eq!(first["schema"], "podway.successor-history-item-result/v1");
        assert_eq!(
            first["detail"]["source_attempt"]["attempt"],
            serde_json::from_str::<Value>(&attempt).unwrap()
        );
        assert_eq!(first["detail"]["item_revision"], 1);
        assert_eq!(first["total"], MAX_TEXT_SCALARS_V2);
        assert_eq!(first["offset"], 0);
        assert_eq!(first["unit"], "unicode_scalar");
        let token = first["next_page_token"].as_str().unwrap();
        let second = fixture.run(&[query, &["--page-token", token]].concat());
        let second = &second["result"];
        assert_eq!(second["detail"], first["detail"]);
        assert_eq!(second["offset"], first["size"]);
        assert_eq!(second["total"], first["total"]);
        assert!(second["next_page_token"].is_null());
        for page in [first, second] {
            let data = page["data"].as_str().unwrap();
            assert_eq!(data.chars().count() as u64, page["size"].as_u64().unwrap());
            assert!(serde_json::to_vec(data).unwrap().len() <= MAX_EVIDENCE_PAGE_BYTES_V2);
        }
        let rejoined = [
            first["data"].as_str().unwrap(),
            second["data"].as_str().unwrap(),
        ]
        .concat();
        assert!(
            rejoined == text,
            "the exact value must survive page reassembly"
        );
        fixture.assert_podway_snapshot(&before);
        (token.to_owned(), first["detail"].clone())
    };
    let (current_token, detail) = pages(&item);
    fixture.run(&[
        "disposition",
        "not-required",
        "--reason",
        "Owned test fixture.",
        "--idempotency-key",
        "disposition",
    ]);
    fixture.run(&["archive", "--idempotency-key", "archive"]);
    let archived_item = [&item[..], &["--archived"]].concat();
    let (_, archived_detail) = pages(&archived_item);
    assert_eq!(archived_detail, detail);
    let before = fixture.podway_snapshot();
    failure(
        &fixture,
        &[&archived_item[..], &["--page-token", &current_token]].concat(),
        "HISTORY_PAGE_TOKEN_STALE",
        4,
    );
    let group = fixture.run(&[
        "group",
        "show",
        "--session",
        session,
        "--archived",
        "--execution",
        execution,
    ]);
    assert_eq!(group["result"]["total"], 0);
    assert_eq!(group["result"]["group"]["summary"]["execution"], execution);
    assert_eq!(
        group["result"]["group"]["summary"]["outcome"],
        "terminated_by_cancel"
    );
    assert_eq!(group["result"]["group"]["closure"]["cause"], "cancel");
    fixture.assert_podway_snapshot(&before);

    let archive = fixture.run(&["archive", "list"]);
    let revision = archive["result"]["sessions"][0]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let purge = fixture.run(&[
        "archive",
        "purge",
        "--session-id",
        session,
        "--if-session-revision",
        &revision,
        "--yes",
        "--idempotency-key",
        "purge",
    ]);
    assert_eq!(purge["result"]["purged"], true);
    let before_missing = fixture.podway_snapshot();
    let missing = failure(&fixture, &archived_item, "SESSION_ID_MISMATCH", 4);
    assert_eq!(missing["details"]["expected_session_id"], session);
    assert!(missing["details"]["actual_session_id"].is_null());
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "max-memo"])["result"]["outcome"]["response"],
        recorded
    );
    fixture.assert_podway_snapshot(&before_missing);
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
