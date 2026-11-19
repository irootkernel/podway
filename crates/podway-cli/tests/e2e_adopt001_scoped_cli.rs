//! Exercise successor authoring and scoped mutations through the production CLI.

use std::{
    fs::{self, File, TryLockError},
    io::Read,
    os::unix::process::ExitStatusExt,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::cli_fixture::{Fixture, GuardedChild, bounded_output, podway_binary};
use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::{Value, json};

fn observe(fixture: &Fixture) -> Value {
    fixture.run(&["--contract-generation", "2", "observe"])
}

fn durable_snapshot(fixture: &Fixture) -> super::cli_fixture::PodwaySnapshot {
    let mut snapshot = fixture.podway_snapshot();
    // A writer can update SQLite's transient WAL index before rejecting its transaction.
    snapshot.retain(|entry| entry.0 != std::path::Path::new("runtime/state.sqlite3-shm"));
    snapshot
}

fn scoped(fixture: &Fixture, state: &Value, command: &[&str], key: &str) -> Value {
    let cursor = state["result"]["cursor"].to_string();
    let mut argv = command.to_vec();
    argv.extend([
        "--if-workspace-uuid",
        state["workspace"]["uuid"].as_str().unwrap(),
        "--if-cursor",
        &cursor,
        "--idempotency-key",
        key,
    ]);
    fixture.run(&argv)
}

const FIXTURE_BASIS: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn assert_refusal(fixture: &Fixture, output: std::process::Output, code: &str, key: Option<&str>) {
    assert!(
        !output.status.success() && output.stderr.is_empty(),
        "{output:?}"
    );
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], code, "{}", error.as_value());
    assert_eq!(
        output.status.code().map(i64::from),
        error.as_value()["exit_code"].as_i64()
    );
    if let Some(key) = key {
        let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", key]);
        assert_eq!(lookup["result"]["found"], false, "{key}");
    }
}

fn fixture_activation(state: &Value, definition: &Value, plan: &Value, key: &str) -> Value {
    let mut cursor = state["result"]["cursor"].as_object().unwrap().clone();
    cursor.remove("session");
    json!({
        "schema":"podway.group-activate-input/v1",
        "context":{"workspace_uuid":state["workspace"]["uuid"],"idempotency_key":key,"cursor":cursor},
        "source":{"kind":"default"},
        "expected_definition":definition,
        "plan":plan,
        "predecessor":null,
        "working_copy_basis":{"descriptor":"Owned deterministic fixture","digest":FIXTURE_BASIS},
        "prior_work":[],
    })
}

fn run_activation(fixture: &Fixture, input: &Value) -> std::process::Output {
    fixture.output_with_stdin(ACTIVATE_ARGS, input.to_string().as_bytes())
}

const ACTIVATE_ARGS: &[&str] = &["group", "activate", "--stdin", "--timeout", "10s"];
const REPLAN_ARGS: &[&str] = &["group", "replan", "--stdin", "--timeout", "10s"];
const REPORT_ARGS: &[&str] = &["record", "--stdin", "--timeout", "10s"];

fn activation_ready_fixture() -> (Fixture, Value, Value) {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Native process contention and recovery",
        "--idempotency-key",
        "start",
    ]);
    scoped(&fixture, &observe(&fixture), &["begin"], "begin");
    scoped(
        &fixture,
        &observe(&fixture),
        &[
            "set",
            "scope-summary",
            "Record qualifying evidence in the owned fixture.",
            "--if-item-revision",
            "0",
        ],
        "scope",
    );
    let before = observe(&fixture);
    let mut request = stdin_template(&before, "group.activate");
    request["context"]["idempotency_key"] = json!("activate-first");
    request["working_copy_basis"] =
        json!({"descriptor":"Owned fixture basis","digest":FIXTURE_BASIS});
    (fixture, before, request)
}

fn successful_process(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
        .unwrap()
        .into_value()
}

fn lookup_process_outcome(fixture: &Fixture, key: &str) -> Value {
    fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"].clone()
}

fn workspace_sequence(output: &Value) -> u64 {
    output["workspace"]["latest_workspace_sequence"]
        .as_u64()
        .unwrap()
}

fn launch_scoped_process(
    fixture: &Fixture,
    argv: &[&str],
    input: Option<&Value>,
    stdout: Stdio,
) -> GuardedChild {
    let stdin = input.map_or_else(Stdio::null, |input| {
        // A preloaded, unlinked fixture file avoids blocking the supervisor
        // while feeding stdin to a child about to contend or be killed.
        let path = fixture
            .0
            .join(format!("process-input-{}", uuid::Uuid::new_v4()));
        fs::write(&path, input.to_string()).unwrap();
        let file = File::open(&path).unwrap();
        fs::remove_file(path).unwrap();
        Stdio::from(file)
    });
    let child = Command::new(podway_binary())
        .arg("--worktree")
        .arg(&fixture.0)
        .arg("--json")
        .args(argv)
        .current_dir(&fixture.0)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    GuardedChild::new(child)
}

fn hold_fixture_sqlite_writer(fixture: &Fixture) -> rusqlite::Connection {
    let writer =
        rusqlite::Connection::open(fixture.0.join(".podway/runtime/state.sqlite3")).unwrap();
    // Match the CLI's no-checkpoint-on-close policy. This connection exists
    // only to hold SQLite's write lock and must not retire the WAL or SHM.
    // While BEGIN is active, opening and closing Store files in this process
    // can release SQLite's process-owned POSIX locks, even on read-only opens.
    writer
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
}

fn assert_live(child: &mut GuardedChild) {
    assert!(
        child.0.as_mut().unwrap().try_wait().unwrap().is_none(),
        "the real CLI child exited before its boundary was observed"
    );
}

fn wait_for_scoped_store_lease(fixture: &Fixture, children: &mut [&mut GuardedChild]) {
    // The raw SQLite writer holds no native maintenance lease. An exclusive
    // probe blocked by a child witnesses real Store admission while SQLite
    // still prevents every contender from beginning its writer transaction.
    let deadline = Instant::now() + Duration::from_secs(5);
    let lease = File::open(fixture.0.join(".podway/runtime/maintenance.lock")).unwrap();
    loop {
        for child in children.iter_mut() {
            assert_live(child);
        }
        match lease.try_lock() {
            Ok(()) => lease.unlock().unwrap(),
            Err(TryLockError::WouldBlock) => return,
            Err(error) => panic!("unable to inspect the owned Store lease: {error}"),
        }
        assert!(Instant::now() < deadline, "no contender reached the Store");
        thread::sleep(Duration::from_millis(5));
    }
}

fn scoped_item_args<'a>(
    workspace: &'a str,
    cursor: &'a str,
    item: &'a str,
    value: &'a str,
    key: &'a str,
) -> [&'a str; 13] {
    [
        "set",
        item,
        value,
        "--if-workspace-uuid",
        workspace,
        "--if-cursor",
        cursor,
        "--if-item-revision",
        "0",
        "--idempotency-key",
        key,
        "--timeout",
        "10s",
    ]
}

fn assert_one_activation(fixture: &Fixture, before: &Value, committed: &Value) -> Value {
    let after = observe(fixture);
    let session = before["result"]["cursor"]["session"].as_str().unwrap();
    let list = fixture.run(&["group", "list", "--session", session]);
    assert_eq!(after["result"]["retained_group_count"], 1);
    assert_eq!(list["result"]["total"], 1);
    assert_eq!(
        after["result"]["cursor"]["selected_group"],
        committed["result"]["execution_id"]
    );
    assert_eq!(
        after["result"]["cursor"]["attempt"]["scope"]["group_execution_id"],
        committed["result"]["execution_id"]
    );
    assert_eq!(after["result"]["cursor"]["attempt"]["node"], "implement");
    assert_eq!(workspace_sequence(&after), workspace_sequence(before) + 1);
    after
}

#[test]
fn native_process_activation_contenders_converge_on_one_execution() {
    for same_key in [true, false] {
        let (fixture, before, request) = activation_ready_fixture();
        let mut second = request.clone();
        if !same_key {
            second["context"]["idempotency_key"] = json!("activate-second");
        }
        let writer = hold_fixture_sqlite_writer(&fixture);
        let mut first =
            launch_scoped_process(&fixture, ACTIVATE_ARGS, Some(&request), Stdio::piped());
        let mut other =
            launch_scoped_process(&fixture, ACTIVATE_ARGS, Some(&second), Stdio::piped());
        wait_for_scoped_store_lease(&fixture, &mut [&mut first, &mut other]);
        writer.execute_batch("ROLLBACK").unwrap();
        let first = bounded_output(first, ACTIVATE_ARGS);
        let other = bounded_output(other, ACTIVATE_ARGS);
        drop(writer);
        let (committed, successful_key) = if same_key {
            let original = successful_process(&first);
            assert_eq!(successful_process(&other), original);
            assert_eq!(first.stdout, other.stdout);
            (original, "activate-first")
        } else {
            assert_ne!(first.status.success(), other.status.success());
            let (accepted, refused, accepted_key, rejected_key) = if first.status.success() {
                (first, other, "activate-first", "activate-second")
            } else {
                (other, first, "activate-second", "activate-first")
            };
            let after = observe(&fixture);
            let detail = serde_json::from_slice::<ErrorEnvelopeV2>(&refused.stdout).unwrap();
            assert_eq!(
                detail.as_value()["details"]["expected_attempt_id"],
                before["result"]["cursor"]["attempt"]["attempt"]
            );
            assert_eq!(
                detail.as_value()["details"]["actual_attempt_id"],
                after["result"]["cursor"]["attempt"]["attempt"]
            );
            let before_rejection_replay = durable_snapshot(&fixture);
            assert_refusal(&fixture, refused, "ATTEMPT_NOT_CURRENT", Some(rejected_key));
            let rejected = if rejected_key == "activate-first" {
                &request
            } else {
                &second
            };
            assert_refusal(
                &fixture,
                run_activation(&fixture, rejected),
                "ATTEMPT_NOT_CURRENT",
                Some(rejected_key),
            );
            assert_eq!(durable_snapshot(&fixture), before_rejection_replay);
            (successful_process(&accepted), accepted_key)
        };
        assert_one_activation(&fixture, &before, &committed);
        assert_eq!(
            lookup_process_outcome(&fixture, successful_key)["outcome"]["response"],
            committed
        );
    }
}

#[test]
fn native_process_scoped_item_contenders_preserve_slot_fences() {
    let (fixture, _, request) = activation_ready_fixture();
    successful_process(&run_activation(&fixture, &request));
    scoped(
        &fixture,
        &observe(&fixture),
        &[
            "set",
            "implementation-summary",
            "Prepared the owned concurrency fixture.",
            "--if-item-revision",
            "0",
        ],
        "implement",
    );
    scoped(
        &fixture,
        &observe(&fixture),
        &["complete"],
        "complete-implementation",
    );
    let before = observe(&fixture);
    assert_eq!(before["result"]["cursor"]["attempt"]["node"], "verify");
    assert_eq!(
        before["result"]["cursor"]["attempt"]["scope"]["kind"],
        "group"
    );
    let workspace = before["workspace"]["uuid"].as_str().unwrap();
    let cursor = before["result"]["cursor"].to_string();
    let commands = [
        scoped_item_args(
            workspace,
            &cursor,
            "verification-command",
            "fixture command A",
            "command-a",
        ),
        scoped_item_args(
            workspace,
            &cursor,
            "verification-command",
            "fixture command B",
            "command-b",
        ),
        scoped_item_args(
            workspace,
            &cursor,
            "verification-exit-status",
            "0",
            "exit-status",
        ),
    ];
    let writer = hold_fixture_sqlite_writer(&fixture);
    let mut contenders =
        commands.map(|argv| launch_scoped_process(&fixture, &argv, None, Stdio::piped()));
    wait_for_scoped_store_lease(&fixture, &mut contenders.iter_mut().collect::<Vec<_>>());
    writer.execute_batch("ROLLBACK").unwrap();
    let [first, second, exit_status] = contenders;
    let first = bounded_output(first, &commands[0]);
    let second = bounded_output(second, &commands[1]);
    let exit_status = bounded_output(exit_status, &commands[2]);
    drop(writer);
    assert_ne!(
        first.status.success(),
        second.status.success(),
        "same-slot contender A ({}): {} / {}; contender B ({}): {} / {}; different slot ({}): {} / {}",
        first.status,
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr),
        second.status,
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr),
        exit_status.status,
        String::from_utf8_lossy(&exit_status.stdout),
        String::from_utf8_lossy(&exit_status.stderr),
    );
    let (accepted, refused, accepted_key, rejected_key, reject_argv) = if first.status.success() {
        (first, second, "command-a", "command-b", commands[1])
    } else {
        (second, first, "command-b", "command-a", commands[0])
    };
    let accepted = successful_process(&accepted);
    let exit_status = successful_process(&exit_status);
    let after = observe(&fixture);
    let durable_after = durable_snapshot(&fixture);
    assert_refusal(
        &fixture,
        refused,
        "ITEM_REVISION_CONFLICT",
        Some(rejected_key),
    );
    assert_refusal(
        &fixture,
        fixture.output(&reject_argv),
        "ITEM_REVISION_CONFLICT",
        Some(rejected_key),
    );
    assert_eq!(
        lookup_process_outcome(&fixture, accepted_key)["outcome"]["response"],
        accepted
    );
    assert_eq!(
        lookup_process_outcome(&fixture, "exit-status")["outcome"]["response"],
        exit_status
    );
    assert_eq!(workspace_sequence(&after), workspace_sequence(&before) + 2);
    assert_eq!(
        after["result"]["cursor"]["attempt"],
        before["result"]["cursor"]["attempt"]
    );
    assert_eq!(
        after["result"]["cursor"]["selected_group"],
        before["result"]["cursor"]["selected_group"]
    );
    assert_eq!(
        after["result"]["items"],
        json!([
            {"item":"verification-command","recorded":true,"revision":1},
            {"item":"verification-exit-status","recorded":true,"revision":1},
        ])
    );
    assert_eq!(observe(&fixture)["result"], after["result"]);
    assert_eq!(durable_snapshot(&fixture), durable_after);
}

#[test]
fn native_process_activation_sigkill_before_commit_leaves_the_key_retryable() {
    let (fixture, before, request) = activation_ready_fixture();
    let durable = durable_snapshot(&fixture);
    let writer = hold_fixture_sqlite_writer(&fixture);
    let mut child = launch_scoped_process(&fixture, ACTIVATE_ARGS, Some(&request), Stdio::piped());
    wait_for_scoped_store_lease(&fixture, &mut [&mut child]);

    // A separate CLI process must see the actual SQLite writer lock. The
    // maintenance lease alone only witnesses that the victim reached Store admission.
    let mut probe = request.clone();
    probe["context"]["idempotency_key"] = json!("held-writer-probe");
    let blocked = fixture.output_with_stdin(
        &["group", "activate", "--stdin", "--timeout", "0ms"],
        probe.to_string().as_bytes(),
    );
    assert_eq!(blocked.status.code(), Some(4), "{blocked:?}");
    assert!(blocked.stderr.is_empty(), "{blocked:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&blocked.stdout).unwrap();
    let error = error.as_value();
    assert_eq!(error["code"], "WORKSPACE_BUSY", "{error}");
    assert_eq!(error["command"], "group.activate");
    assert_eq!(error["details"]["boundary"], "sqlite_write_lock");
    assert_eq!(error["details"]["timeout_ms"], 0);
    assert_live(&mut child);
    child.0.as_mut().unwrap().kill().unwrap();
    let killed = bounded_output(child, ACTIVATE_ARGS);
    assert_eq!(killed.status.signal(), Some(9));
    assert!(killed.stdout.is_empty() && killed.stderr.is_empty());
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);
    assert_eq!(durable_snapshot(&fixture), durable);
    assert_eq!(observe(&fixture)["result"], before["result"]);
    for key in ["activate-first", "held-writer-probe"] {
        assert_eq!(lookup_process_outcome(&fixture, key)["found"], false);
    }
    let committed = successful_process(&run_activation(&fixture, &request));
    assert_one_activation(&fixture, &before, &committed);
    assert_eq!(
        lookup_process_outcome(&fixture, "activate-first")["outcome"]["response"],
        committed
    );
}

#[test]
fn native_process_activation_sigkill_after_commit_recovers_without_stdout() {
    use nix::{
        errno::Errno,
        fcntl::{FcntlArg, OFlag, fcntl},
        unistd::{pipe, write},
    };

    let (fixture, before, request) = activation_ready_fixture();
    let (read_end, write_end) = pipe().unwrap();
    let flags = OFlag::from_bits_truncate(fcntl(&write_end, FcntlArg::F_GETFL).unwrap());
    fcntl(&write_end, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
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
    fcntl(&write_end, FcntlArg::F_SETFL(flags)).unwrap();
    let mut child = launch_scoped_process(
        &fixture,
        ACTIVATE_ARGS,
        Some(&request),
        Stdio::from(write_end),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let original = loop {
        assert_live(&mut child);
        let result = lookup_process_outcome(&fixture, "activate-first");
        if result["found"] == true {
            break result["outcome"]["response"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "no commit before stdout delivery"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert_live(&mut child);
    child.0.as_mut().unwrap().kill().unwrap();
    let killed = child.0.take().unwrap().wait_with_output().unwrap();
    assert_eq!(killed.status.signal(), Some(9));
    assert!(killed.stderr.is_empty());
    let mut bytes = Vec::new();
    File::from(read_end)
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(
        bytes,
        vec![b'x'; prefilled],
        "no activation output byte was delivered"
    );
    let activated = assert_one_activation(&fixture, &before, &original);
    scoped(
        &fixture,
        &activated,
        &[
            "set",
            "implementation-summary",
            "A subsequent scoped mutation.",
            "--if-item-revision",
            "0",
        ],
        "subsequent-report",
    );
    let current = observe(&fixture);
    assert_eq!(
        workspace_sequence(&current),
        workspace_sequence(&before) + 2
    );
    let durable = durable_snapshot(&fixture);
    let replay = run_activation(&fixture, &request);
    assert_eq!(successful_process(&replay), original);
    assert_eq!(run_activation(&fixture, &request).stdout, replay.stdout);
    assert_eq!(
        lookup_process_outcome(&fixture, "activate-first")["outcome"]["response"],
        original
    );
    assert_eq!(observe(&fixture)["result"], current["result"]);
    assert_eq!(durable_snapshot(&fixture), durable);
}

#[test]
fn successor_cli_dry_run_and_fenced_default_activation() {
    let fixture = Fixture::new();
    let preset = fixture.run(&["preset", "show", "small-change-v3"]);
    assert_eq!(
        preset["result"]["schema"],
        "podway.successor-preset-result/v1"
    );
    let preview = fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Scoped CLI",
        "--dry-run",
    ]);
    assert_eq!(preview["result"]["procedure_schema"], "podway.procedure/v3");
    assert!(preview.get("session").is_none());
    assert!(!fixture.0.join(".podway").exists());

    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Scoped CLI",
        "--idempotency-key",
        "start",
    ]);
    let prepared = observe(&fixture);
    assert_eq!(prepared["result"]["cursor"]["session_revision"], 0);
    assert!(prepared["result"]["cursor"]["attempt"].is_null());
    scoped(&fixture, &prepared, &["begin"], "begin");
    let anchor = observe(&fixture);
    assert_eq!(
        anchor["result"]["cursor"]["attempt"]["scope"]["kind"],
        "base"
    );
    assert_eq!(anchor["result"]["cursor"]["attempt"]["node"], "inspect");
    scoped(
        &fixture,
        &anchor,
        &[
            "set",
            "scope-summary",
            "Check scoped CLI transitions",
            "--if-item-revision",
            "0",
        ],
        "scope",
    );
    let ready = observe(&fixture);
    let template = ready["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| template["command"] == "group.activate")
        .unwrap();
    let mut request: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    request["context"]["idempotency_key"] = json!("wrong-digest");
    request["working_copy_basis"] = json!({
        "descriptor": "owned source fixture",
        "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000001",
    });
    let expected = request["expected_definition"].clone();
    request["expected_definition"] =
        json!("sha256:0000000000000000000000000000000000000000000000000000000000000002");
    let mut stale_cursor = fixture_activation(
        &anchor,
        &expected,
        &ready["result"]["plan"],
        "stale-activation-cursor",
    );
    stale_cursor["context"]["idempotency_key"] = json!("stale-activation-cursor");
    assert_refusal(
        &fixture,
        run_activation(&fixture, &stale_cursor),
        "SESSION_REVISION_CONFLICT",
        Some("stale-activation-cursor"),
    );
    let mut empty_anchor = fixture_activation(
        &anchor,
        &expected,
        &ready["result"]["plan"],
        "stale-selection",
    );
    empty_anchor["context"]["cursor"] = request["context"]["cursor"].clone();
    empty_anchor["context"]["cursor"]["selected_group"] =
        json!("00000000-0000-4000-8000-000000000123");
    assert_refusal(
        &fixture,
        run_activation(&fixture, &empty_anchor),
        "GROUP_SELECTION_STALE",
        Some("stale-selection"),
    );
    let mut wrong_plan = request.clone();
    wrong_plan["expected_definition"] = expected.clone();
    wrong_plan["context"]["idempotency_key"] = json!("stale-plan");
    wrong_plan["plan"]["item_set_digest"] = json!(FIXTURE_BASIS);
    assert_refusal(
        &fixture,
        run_activation(&fixture, &wrong_plan),
        "GROUP_PLAN_STALE",
        Some("stale-plan"),
    );
    assert_eq!(observe(&fixture)["result"], ready["result"]);

    let before = fixture.podway_snapshot();
    let rejected = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        request.to_string().as_bytes(),
    );
    assert_eq!(rejected.status.code(), Some(4));
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "GROUP_DEFINITION_MISMATCH");
    assert_eq!(fixture.podway_snapshot(), before);
    let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", "wrong-digest"]);
    assert!(lookup["result"]["outcome"].is_null());

    request["context"]["idempotency_key"] = json!("activate");
    request["expected_definition"] = expected;
    let activate = || {
        fixture.output_with_stdin(
            &["group", "activate", "--stdin"],
            request.to_string().as_bytes(),
        )
    };
    let activated = activate();
    assert!(activated.status.success(), "{activated:?}");
    let result = serde_json::from_slice::<OutputEnvelopeV4>(&activated.stdout)
        .unwrap()
        .into_value();
    assert_eq!(
        result["result"]["closed_attempt"],
        ready["result"]["cursor"]["attempt"]
    );
    assert_eq!(result["result"]["active_attempt"]["scope"]["kind"], "group");
    let persisted = observe(&fixture);
    assert_eq!(
        persisted["result"]["cursor"]["attempt"],
        result["result"]["active_attempt"]
    );
    assert_eq!(activate().stdout, activated.stdout);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "activate"])["result"]["outcome"]["response"],
        result
    );
    let generation1 = fixture.run(&["observe"]);
    assert_eq!(
        generation1["result"]["schema"],
        "podway.observation-result/v7"
    );
    assert_eq!(
        generation1["result"]["cursor"],
        persisted["result"]["cursor"]
    );

    let session = persisted["session"]["id"].as_str().unwrap();
    let execution = result["result"]["execution_id"].as_str().unwrap();
    let definition = persisted["result"]["selection"]["definition_digest"]
        .as_str()
        .unwrap();
    assert_refusal(
        &fixture,
        fixture.output(&[
            "group",
            "show",
            "--session",
            session,
            "--execution",
            "00000000-0000-4000-8000-000000000123",
        ]),
        "GROUP_EXECUTION_NOT_FOUND",
        None,
    );
    let cursor = persisted["result"]["cursor"].to_string();
    let workspace = persisted["workspace"]["uuid"].as_str().unwrap();
    assert_refusal(
        &fixture,
        fixture.output(&[
            "group",
            "return",
            "--if-workspace-uuid",
            workspace,
            "--if-cursor",
            &cursor,
            "--idempotency-key",
            "wrong-return-placement",
        ]),
        "GROUP_RETURN_NOT_ALLOWED",
        Some("wrong-return-placement"),
    );
    let wrong_anchor = fixture_activation(
        &persisted,
        &json!(definition),
        &ready["result"]["plan"],
        "wrong-anchor",
    );
    assert_refusal(
        &fixture,
        run_activation(&fixture, &wrong_anchor),
        "GROUP_ACTIVATION_NOT_ALLOWED",
        Some("wrong-anchor"),
    );
    let before_rework = durable_snapshot(&fixture);
    for (target, key, code) in [
        (
            "inspect",
            "cross-scope-rework",
            "MANUAL_REWORK_TARGET_NOT_ALLOWED",
        ),
        (
            "verify",
            "unvisited-rework",
            "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
        ),
    ] {
        let rework = fixture.output(&[
            "rework",
            "--to",
            target,
            "--reason",
            "Exercise manual rework target admission.",
            "--if-workspace-uuid",
            workspace,
            "--if-cursor",
            &cursor,
            "--idempotency-key",
            key,
        ]);
        assert_eq!(rework.status.code(), Some(1), "{rework:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&rework.stdout).unwrap();
        assert_eq!(error.as_value()["details"]["target_graph_node_id"], target);
        assert_refusal(&fixture, rework, code, Some(key));
    }
    assert_eq!(durable_snapshot(&fixture), before_rework);
    assert_eq!(observe(&fixture)["result"], persisted["result"]);
    let before_history = fixture.podway_snapshot();
    let evidence = fixture.run(&[
        "evidence",
        "read",
        "--source",
        "@plan",
        "--item",
        "scope-summary",
    ]);
    assert_eq!(
        evidence["result"]["schema"],
        "podway.scoped-evidence-read-result/v1"
    );
    assert_eq!(
        evidence["result"]["detail"]["source_attempt"],
        ready["result"]["cursor"]["attempt"]
    );
    assert_eq!(evidence["result"]["data"], "Check scoped CLI transitions");
    let groups = fixture.run(&["group", "list", "--session", session]);
    assert_eq!(groups["result"]["total"], 1);
    assert_eq!(groups["result"]["rows"][0]["execution"], execution);
    let dag = fixture.run(&[
        "history",
        "dag",
        "--session",
        session,
        "--group-execution",
        execution,
        "--definition-digest",
        definition,
    ]);
    assert_eq!(
        dag["result"]["schema"],
        "podway.successor-history-dag-result/v1"
    );
    assert_eq!(fixture.podway_snapshot(), before_history);

    scoped(
        &fixture,
        &persisted,
        &["cancel", "--reason", "End the owned fixture"],
        "cancel",
    );
    fixture.run(&[
        "disposition",
        "not-required",
        "--reason",
        "The owned fixture has no external handoff.",
        "--idempotency-key",
        "disposition",
    ]);
    let archived = fixture.run(&["archive", "--idempotency-key", "archive"]);
    assert_eq!(archived["result"]["retained_group_count"], 1);
    let groups = fixture.run(&["group", "list", "--session", session, "--archived"]);
    assert_eq!(groups["result"]["rows"][0]["execution"], execution);
    let show = fixture.run(&["archive", "show", "--session-id", session]);
    assert_eq!(show["result"]["archive"]["session_id"], session);
}
#[test]
fn successor_cli_activates_each_tracked_default_and_refuses_retired_ids() {
    let retired = Fixture::new();
    for arguments in [
        &["preset", "show", "sw-dev-v2"][..],
        &[
            "start",
            "--preset",
            "sw-dev-v2",
            "--task",
            "Retired preset",
            "--dry-run",
        ][..],
    ] {
        assert_refusal(
            &retired,
            retired.output(arguments),
            "PRESET_NOT_FOUND",
            None,
        );
    }
    assert!(!retired.0.join(".podway").exists());

    let presets = [
        (
            "sw-dev-v3",
            "implement",
            &[
                ("scope-summary", "text", "Bound the fixture change."),
                ("success-criteria", "list", "Observe a selected group."),
                ("risk-summary", "text", "Fixture only."),
            ][..],
        ),
        (
            "analysis-v3",
            "collect",
            &[
                ("analysis-question", "text", "Which scope is selected?"),
                ("decision-context", "text", "Fixture admission."),
                ("scope-and-constraints", "list", "Use the current anchor."),
            ][..],
        ),
        (
            "bug-fix-v3",
            "diagnose",
            &[
                ("reproduction-status", "choice", "reproduced"),
                ("observed-behavior", "text", "A declared fixture defect."),
                (
                    "expected-behavior",
                    "text",
                    "The expected fixture condition.",
                ),
                (
                    "regression-check",
                    "text",
                    "The deterministic fixture check.",
                ),
            ][..],
        ),
    ];
    for (preset, entry, items) in presets {
        let fixture = Fixture::new();
        let catalog = fixture.run(&["preset", "show", preset]);
        fixture.run(&["init", "--idempotency-key", "init"]);
        fixture.run(&[
            "start",
            "--preset",
            preset,
            "--task",
            "Default activation",
            "--idempotency-key",
            "start",
        ]);
        fixture.run(&["begin", "--idempotency-key", "begin"]);
        let initial = observe(&fixture);
        let invalid = fixture_activation(
            &initial,
            &catalog["result"]["group_digest"],
            &json!({
                "design_attempt":initial["result"]["cursor"]["attempt"]["attempt"], "item_set_digest":FIXTURE_BASIS, "items":[],
            }),
            "missing-goal",
        );
        assert_refusal(
            &fixture,
            run_activation(&fixture, &invalid),
            "SESSION_GOAL_MISSING",
            Some("missing-goal"),
        );
        assert_eq!(observe(&fixture)["result"], initial["result"]);
        fixture.run(&[
            "goal",
            "define",
            "--goal",
            "Admit the tracked fixture group.",
            "--criterion",
            "fixture=The default group is selected.",
            "--idempotency-key",
            "goal",
        ]);
        let anchor = observe(&fixture);
        let no_items = fixture_activation(
            &anchor,
            &catalog["result"]["group_digest"],
            &invalid["plan"],
            "missing-plan-items",
        );
        assert_refusal(
            &fixture,
            run_activation(&fixture, &no_items),
            "REQUIRED_ITEMS_MISSING",
            Some("missing-plan-items"),
        );
        assert_eq!(observe(&fixture)["result"], anchor["result"]);
        let mut record = stdin_template(&anchor, "item.record_many");
        record["context"]["idempotency_key"] = json!("plan");
        record["operations"] = Value::Array(items.iter().map(|(id, kind, value)| {
            let value = if *kind == "list" { json!([value]) } else { json!(value) };
            json!({"item_id":id,"expected_item_revision":0,"record":{"type":kind,"value":value}})
        }).collect());
        let recorded = fixture.output_with_stdin(
            &["record", "--stdin", "--with-observation"],
            record.to_string().as_bytes(),
        );
        let recorded: OutputEnvelopeV4 = serde_json::from_slice(&recorded.stdout).unwrap();
        let plan = &recorded.as_value()["result"]["observation"];
        let mut activation = stdin_template(&json!({"result":plan}), "group.activate");
        activation["context"]["idempotency_key"] = json!("activate");
        activation["working_copy_basis"] =
            json!({"descriptor":"Owned tracked fixture","digest":FIXTURE_BASIS});
        let output = run_activation(&fixture, &activation);
        assert!(output.status.success(), "{preset}: {output:?}");
        let output: OutputEnvelopeV4 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.as_value()["result"]["active_attempt"]["node"], entry);
        let after = observe(&fixture);
        assert_eq!(
            after["result"]["cursor"]["attempt"]["scope"]["kind"],
            "group"
        );
        assert_eq!(after["result"]["goal"]["revision"], 1);
    }
}

#[test]
fn successor_cli_refuses_over_budget_activation_without_an_outcome() {
    let fixture = Fixture::new();
    let catalog = fixture.run(&["preset", "show", "small-change-v3"]);
    let mut base = catalog["result"]["procedure"].clone();
    base["node_definitions"]["bridge"] =
        json!({"type":"action","title":"Bridge","intent":"Exercise a bounded counter."});
    let nodes = base["graph"]["nodes"].as_array_mut().unwrap();
    nodes[0]["next"] = json!("bridge-000");
    base["execution_group"]["success_return"] = json!("bridge-000");
    let nodes = base["graph"]["nodes"].as_array_mut().unwrap();
    let return_evidence = nodes[1]["evidence_from"].take();
    nodes[1].as_object_mut().unwrap().remove("evidence_from");
    for index in 0..900 {
        let next = if index == 899 {
            "closeout".to_owned()
        } else {
            format!("bridge-{:03}", index + 1)
        };
        let mut node = json!({"id":format!("bridge-{index:03}"),"use":"bridge","next":next});
        if index == 0 {
            node["evidence_from"] = return_evidence.clone();
        }
        nodes.push(node);
    }
    // YAML byte escapes keep the request within the stdin budget. The admitted
    // canonical JSON uses six bytes per control character in its result.
    let prompt = format!("P{}", r"\x01".repeat(299));
    let help = format!("H{}", r"\x01".repeat(999));
    let mut extras = String::new();
    for index in 0..127 {
        use std::fmt::Write as _;
        writeln!(extras, "      - id: budget-{index:03}").unwrap();
        writeln!(extras, "        type: text").unwrap();
        writeln!(extras, "        prompt: \"{prompt}\"").unwrap();
        writeln!(extras, "        help: \"{help}\"").unwrap();
        writeln!(extras, "        required: false").unwrap();
    }
    let shipped_group = include_str!("../../../assets/presets/small-change-default-v3.yaml");
    let next_definition = "  verify-change:\n";
    let (implement, verify_and_later) = shipped_group.split_once(next_definition).unwrap();
    let compact_group = format!("{implement}{extras}{next_definition}{verify_and_later}");
    std::fs::write(fixture.0.join("budget-base.json"), base.to_string()).unwrap();
    std::fs::write(fixture.0.join("budget-group.yaml"), &compact_group).unwrap();
    let base_digest = fixture.run(&["procedure", "preview", "budget-base.json"]);
    let group_digest = fixture.run(&["procedure", "preview", "budget-group.yaml"]);
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--procedure",
        "budget-base.json",
        "--expect-procedure-digest",
        base_digest["result"]["digest"].as_str().unwrap(),
        "--task",
        "Budget limit",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "set",
        "scope-summary",
        "The scope for this owned fixture.",
        "--idempotency-key",
        "plan",
    ]);
    let before = observe(&fixture);
    let mut activation = fixture_activation(
        &before,
        &group_digest["result"]["digest"],
        &before["result"]["plan"],
        "over-budget",
    );
    activation["source"] = json!({"kind":"document","format":"yaml","source":compact_group});
    assert_refusal(
        &fixture,
        run_activation(&fixture, &activation),
        "RESPONSE_BUDGET_EXCEEDED",
        Some("over-budget"),
    );
    assert_eq!(observe(&fixture)["result"], before["result"]);
}

#[test]
fn successor_goal_actor_and_unblock_all_round_trip_through_cli() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "analysis-v3",
        "--task",
        "Retain actor and blocker state",
        "--idempotency-key",
        "start",
    ]);
    let prepared = observe(&fixture);
    let begin = [
        "begin",
        "--goal",
        "Establish the source evidence.",
        "--criterion",
        "scope=The scope is recorded.",
        "--actor",
        "fixture-author",
    ];
    let original = scoped(&fixture, &prepared, &begin, "attributed-begin");
    let state = observe(&fixture);
    assert_eq!(state["result"]["goal"]["actor"], "fixture-author");
    assert_eq!(
        scoped(&fixture, &prepared, &begin, "attributed-begin"),
        original,
    );
    fixture.run(&[
        "block",
        "--reason",
        "First missing fixture fact.",
        "--idempotency-key",
        "first",
    ]);
    fixture.run(&[
        "block",
        "--reason",
        "Second missing fixture fact.",
        "--idempotency-key",
        "second",
    ]);
    let blocked = observe(&fixture);
    assert_eq!(
        blocked["result"]["open_blockers"].as_array().unwrap().len(),
        2
    );
    let after = scoped(&fixture, &blocked, &["unblock", "--all"], "resolve-all");
    let cleared = observe(&fixture);
    assert_eq!(cleared["result"]["open_blockers"], json!([]));
    assert_eq!(
        after["session"]["revision_after"].as_u64().unwrap(),
        blocked["session"]["revision_after"].as_u64().unwrap() + 1,
    );
    assert_eq!(
        scoped(&fixture, &blocked, &["unblock", "--all"], "resolve-all"),
        after
    );
    let before = fixture.podway_snapshot();
    let denied = fixture.output(&["unblock", "--all", "--idempotency-key", "no-open"]);
    assert!(!denied.status.success());
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&denied.stdout)
            .unwrap()
            .as_value()["code"],
        "REQUEST_INVALID"
    );
    fixture.assert_podway_snapshot(&before);
}
fn prepare_small_change_review(
    score: i64,
    verdict: Option<&str>,
    with_report: bool,
) -> (Fixture, Value) {
    let (fixture, _, mut activate) = activation_ready_fixture();
    if verdict.is_none() || with_report {
        let preset = fixture.run(&["preset", "show", "small-change-v3"]);
        let mut document = preset["result"]["default_group"].clone();
        if verdict.is_none() {
            let assessor = &mut document["node_definitions"]["assess-review"]["items"];
            let verdict_item = assessor
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|item| item["id"] == "assessment-verdict")
                .unwrap();
            verdict_item["required"] = json!(false);
        }
        if with_report {
            document["node_definitions"]["review-change"]["items"] = json!([
                {"id":"review-note","type":"text","prompt":"Retain the optional current report.","required":false}
            ]);
        }
        let source = document.to_string();
        std::fs::write(fixture.0.join("guard-fixture.json"), &source).unwrap();
        let validated = fixture.run(&["procedure", "validate", "guard-fixture.json"]);
        activate["source"] = json!({"kind": "document", "format": "json", "source": source});
        activate["expected_definition"] = validated["result"]["digest"].clone();
    }
    successful_process(&run_activation(&fixture, &activate));
    fixture.run(&[
        "set",
        "implementation-summary",
        "A deterministic test input.",
        "--idempotency-key",
        "implementation",
    ]);
    fixture.run(&["complete", "--idempotency-key", "complete-implementation"]);
    fixture.run(&[
        "set",
        "verification-command",
        "fixture assertion",
        "--idempotency-key",
        "command",
    ]);
    fixture.run(&[
        "set",
        "verification-exit-status",
        "1",
        "--idempotency-key",
        "status",
    ]);
    fixture.run(&["complete", "--idempotency-key", "complete-verification"]);
    assert_eq!(observe(&fixture)["result"]["next_decision_entry"], 1);
    if let Some(verdict) = verdict {
        fixture.run(&[
            "set",
            "assessment-verdict",
            verdict,
            "--idempotency-key",
            "verdict",
        ]);
    }
    fixture.run(&[
        "set",
        "assessment-stage",
        "scored",
        "--idempotency-key",
        "stage",
    ]);
    fixture.run(&[
        "set",
        "assessment-score",
        &score.to_string(),
        "--idempotency-key",
        "score",
    ]);
    fixture.run(&[
        "set",
        "scored-basis",
        "Deterministic rejection-control basis.",
        "--idempotency-key",
        "basis",
    ]);
    fixture.run(&["complete", "--idempotency-key", "settle-assessment"]);
    let decision = observe(&fixture);
    assert_eq!(decision["result"]["decision_entry"], 1);
    (fixture, decision)
}

#[test]
fn shipped_small_change_replan_requires_the_current_guarded_score() {
    const BASIS: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000001";
    for (score, verdict) in [(79, Some("failure")), (80, Some("failure")), (80, None)] {
        let (fixture, decision) = prepare_small_change_review(score, verdict, false);

        let eligible = verdict.is_some() && score == 80;
        let before = durable_snapshot(&fixture);
        assert_current_decision_guards(&fixture, &decision, score, verdict, &before);
        let template = decision["result"]["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|recipe| recipe["command"] == "group.replan");
        assert_eq!(template.is_some(), eligible);
        assert_eq!(
            decision["result"]["guidance"]["available_operations"]
                .as_array()
                .unwrap()
                .contains(&json!("group.replan")),
            eligible,
        );
        let mut request = if eligible {
            stdin_template(&decision, "group.replan")
        } else {
            let mut cursor = decision["result"]["cursor"].as_object().unwrap().clone();
            cursor.remove("session");
            json!({
                "schema":"podway.group-replan-input/v1",
                "option":"replan",
                "context":{"workspace_uuid":decision["workspace"]["uuid"],"cursor":cursor}
            })
        };
        request["context"]["idempotency_key"] = json!("replan");
        request["working_copy_basis"] =
            json!({"descriptor":"Owned fixture after work", "digest": BASIS});
        request["reason"] = json!("The fixture exercises the exact declared guard boundary.");
        request["feedback"] = json!("Retain the original work as history for this fixture.");
        let replan = || {
            fixture.output_with_stdin(
                &["group", "replan", "--stdin"],
                request.to_string().as_bytes(),
            )
        };
        let output = replan();
        if !eligible {
            assert!(!output.status.success());
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
            let error = error.as_value();
            assert_eq!(error["command"], "group.replan");
            assert_eq!(error["code"], "OPTION_GUARD_UNSATISFIED");
            assert_eq!(
                error["details"]["decision"],
                decision["result"]["cursor"]["attempt"]
            );
            assert_eq!(error["details"]["option"], request["option"]);
            let expected = if verdict.is_some() {
                "unmet"
            } else {
                "unevaluable"
            };
            assert_eq!(error["details"]["state"], expected);
            let predicates = error["details"]["predicates"].as_array().unwrap();
            assert_eq!(predicates[2]["item"], "assessment-score");
            assert_eq!(
                predicates[2]["state"],
                if verdict.is_some() { "unmet" } else { "met" }
            );
            assert_eq!(predicates[2]["source"]["scope"]["kind"], "group");
            assert_eq!(durable_snapshot(&fixture), before);
            assert_eq!(observe(&fixture)["result"], decision["result"]);
            assert!(fixture.run(&["outcome","lookup","--idempotency-key","replan"])["result"]["outcome"].is_null());
            if verdict.is_some() {
                let advanced = scoped(
                    &fixture,
                    &decision,
                    &[
                        "decide",
                        "--option",
                        "changes-requested",
                        "--reason",
                        "The current guard authorizes local correction.",
                    ],
                    "allowed-local-option",
                );
                assert_eq!(advanced["result"]["cursor"]["attempt"]["node"], "implement");
            }
        } else {
            assert!(output.status.success(), "{output:?}");
            let result: OutputEnvelopeV4 = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                result.as_value()["result"]["closed_attempt"],
                decision["result"]["cursor"]["attempt"]
            );
            let next = observe(&fixture);
            assert_eq!(next["result"]["cursor"]["attempt"]["node"], "inspect");
            assert_eq!(next["result"]["cursor"]["attempt"]["scope"]["kind"], "base");
            assert_eq!(next["result"]["retained_group_count"], 1);
            assert_eq!(replan().stdout, output.stdout);
            assert_eq!(
                fixture.run(&["outcome", "lookup", "--idempotency-key", "replan"])["result"]["outcome"]
                    ["response"],
                *result.as_value()
            );
        }
    }
}

fn process_replan_request(decision: &Value, key: &str) -> Value {
    let mut request = stdin_template(decision, "group.replan");
    request["context"]["idempotency_key"] = json!(key);
    request["working_copy_basis"] =
        json!({"descriptor":"Owned fixture basis","digest":FIXTURE_BASIS});
    request["reason"] = json!("The current admitted failed assessment supports replacement.");
    request["feedback"] = json!("Retain the original scoped work and assessment.");
    request
}

#[test]
fn native_process_replan_and_atomic_report_contend_on_the_same_cursor() {
    let (fixture, decision) = prepare_small_change_review(80, Some("failure"), true);
    let replan = process_replan_request(&decision, "contending-replan");
    let mut report = stdin_template(&decision, "item.record_many");
    report["context"]["idempotency_key"] = json!("contending-report");
    report["operations"] = json!([{
        "item_id":"review-note", "expected_item_revision":0,
        "record":{"type":"text","value":"The current note was recorded."}
    }]);
    let writer = hold_fixture_sqlite_writer(&fixture);
    let mut change = launch_scoped_process(&fixture, REPLAN_ARGS, Some(&replan), Stdio::piped());
    let mut record = launch_scoped_process(&fixture, REPORT_ARGS, Some(&report), Stdio::piped());
    wait_for_scoped_store_lease(&fixture, &mut [&mut change, &mut record]);
    writer.execute_batch("ROLLBACK").unwrap();
    let change = bounded_output(change, REPLAN_ARGS);
    let record = bounded_output(record, REPORT_ARGS);
    drop(writer);
    assert_ne!(change.status.success(), record.status.success());
    let landed = observe(&fixture);
    assert_eq!(
        workspace_sequence(&landed),
        workspace_sequence(&decision) + 1
    );
    let new_anchor = if change.status.success() {
        let result = successful_process(&change);
        assert_eq!(
            result["result"]["closed_attempt"],
            decision["result"]["cursor"]["attempt"]
        );
        assert_refusal(
            &fixture,
            record,
            "ATTEMPT_NOT_CURRENT",
            Some("contending-report"),
        );
        assert_eq!(
            lookup_process_outcome(&fixture, "contending-replan")["outcome"]["response"],
            result
        );
        landed
    } else {
        let result = successful_process(&record);
        assert_refusal(
            &fixture,
            change,
            "SESSION_REVISION_CONFLICT",
            Some("contending-replan"),
        );
        assert_eq!(
            lookup_process_outcome(&fixture, "contending-report")["outcome"]["response"],
            result
        );
        assert_eq!(
            landed["result"]["cursor"]["attempt"],
            decision["result"]["cursor"]["attempt"]
        );
        assert_eq!(
            landed["result"]["items"],
            json!([{"item":"review-note","recorded":true,"revision":1}])
        );
        let fresh_replan = process_replan_request(&landed, "fresh-replan");
        let accepted = fixture.output_with_stdin(REPLAN_ARGS, fresh_replan.to_string().as_bytes());
        let accepted = successful_process(&accepted);
        assert_eq!(
            accepted["result"]["closed_attempt"],
            decision["result"]["cursor"]["attempt"]
        );
        observe(&fixture)
    };
    assert_eq!(new_anchor["result"]["cursor"]["attempt"]["node"], "inspect");
    assert_eq!(
        new_anchor["result"]["cursor"]["attempt"]["scope"]["kind"],
        "base"
    );
    assert_eq!(new_anchor["result"]["retained_group_count"], 1);
    let stable = durable_snapshot(&fixture);
    report["context"]["idempotency_key"] = json!("late-report");
    assert_refusal(
        &fixture,
        fixture.output_with_stdin(REPORT_ARGS, report.to_string().as_bytes()),
        "ATTEMPT_NOT_CURRENT",
        Some("late-report"),
    );
    assert_eq!(observe(&fixture)["result"], new_anchor["result"]);
    assert_eq!(durable_snapshot(&fixture), stable);
}

#[test]
fn scoped_cli_refuses_invalid_base_sources_with_closed_details() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let cases = [
        ("schema: [", "PROCEDURE_INVALID", "invalid_document", None),
        (
            "schema: podway.procedure/v2",
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "unsupported_schema",
            Some("podway.procedure/v2"),
        ),
        (
            "schema: podway.execution-group/v1",
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "unsupported_schema",
            Some("podway.execution-group/v1"),
        ),
    ];
    for (source, code, reason, known_schema) in cases {
        std::fs::write(fixture.0.join("procedure.yaml"), source).unwrap();
        for generation in ["1", "2"] {
            let output = fixture.output(&[
                "--contract-generation",
                generation,
                "start",
                "--procedure",
                "procedure.yaml",
                "--task",
                "Refuse malformed base",
                "--idempotency-key",
                "refuse-source",
            ]);
            assert!(!output.status.success(), "{output:?}");
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(error.as_value()["code"], code);
            assert_eq!(
                error.as_value()["details"],
                json!({
                    "schema": "podway.procedure-source-error-details/v1",
                    "expected_schema": "podway.procedure/v3",
                    "reason": reason,
                    "known_source_schema": known_schema,
                })
            );
            assert!(
                fixture.run(&["outcome", "lookup", "--idempotency-key", "refuse-source"])["result"]
                    ["outcome"]
                    .is_null()
            );
        }
    }
}

fn stdin_template(state: &Value, command: &str) -> Value {
    let template = state["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == command)
        .unwrap();
    serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap()
}

fn recipe_argv(state: &Value, command: &str, fields: &[(&str, &str)]) -> Vec<String> {
    let template = state["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|recipe| recipe["command"] == command)
        .unwrap();
    let argv = template["argv"].as_array().unwrap();
    assert_eq!(&argv[..2], &[json!("podway"), json!("--json")]);
    argv[2..]
        .iter()
        .map(|token| {
            let token = token.as_str().unwrap();
            fields
                .iter()
                .find_map(|(from, to)| (*from == token).then_some(*to))
                .unwrap_or(token)
                .to_owned()
        })
        .collect()
}

fn execute_recipe(
    fixture: &Fixture,
    state: &Value,
    command: &str,
    fields: &[(&str, &str)],
) -> Value {
    let argv = recipe_argv(state, command, fields);
    fixture.run(&argv.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn scoped_cli_lifecycle_guidance_and_executable_recipes_are_state_bound() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Recipe fixture",
        "--idempotency-key",
        "start",
    ]);
    let prepared = observe(&fixture);
    let generation_one = fixture.run(&["observe"]);
    assert_eq!(
        generation_one["result"]["lifecycle_guidance"],
        prepared["result"]["lifecycle_guidance"]
    );
    assert!(generation_one["result"].get("mutation_templates").is_none());
    let templates = prepared["result"]["mutation_templates"].as_array().unwrap();
    assert_eq!(templates.len(), 4);
    assert_eq!(templates[0]["requires_explicit_authorization"], false);
    assert_eq!(templates[1]["command"], "session.reset");
    assert_eq!(templates[1]["requires_explicit_authorization"], true);
    let mut wrong = prepared.clone();
    wrong["result"]["mutation_templates"][1]["preconditions"]["session_revision"] = json!(1);
    assert!(OutputEnvelopeV4::from_value(wrong).is_err());
    let mut wrong = prepared.clone();
    let argv = wrong["result"]["mutation_templates"][1]["argv"]
        .as_array_mut()
        .unwrap();
    let index = argv
        .iter()
        .position(|value| value == "--if-session-revision")
        .unwrap();
    argv[index + 1] = json!("1");
    assert!(OutputEnvelopeV4::from_value(wrong).is_err());

    execute_recipe(
        &fixture,
        &prepared,
        "session.start_replace",
        &[
            ("<preset>", "small-change-v3"),
            ("<task>", "Prepared replacement"),
            ("<idempotency-key>", "replace"),
        ],
    );
    let replacement = observe(&fixture);
    assert_ne!(replacement["session"]["id"], prepared["session"]["id"]);
    execute_recipe(
        &fixture,
        &replacement,
        "session.begin",
        &[("<idempotency-key>", "begin")],
    );
    let obsolete = recipe_argv(
        &replacement,
        "session.reset",
        &[("<idempotency-key>", "obsolete")],
    );
    let output = fixture.output(&obsolete.iter().map(String::as_str).collect::<Vec<_>>());
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "SESSION_REVISION_CONFLICT");
    fixture.run(&[
        "cancel",
        "--reason",
        "The owned fixture is complete",
        "--idempotency-key",
        "cancel",
    ]);
    let terminal = observe(&fixture);
    assert_eq!(
        terminal["result"]["lifecycle_guidance"]["current_terminal_disposition"],
        false
    );
    assert_eq!(
        terminal["result"]["mutation_templates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let disposition_fields = [
        ("<summary>", "Fixture handoff"),
        ("<reference>", "fixture-evidence"),
        ("<idempotency-key>", "disposition"),
    ];
    let disposition = execute_recipe(
        &fixture,
        &terminal,
        "session.terminal_disposition",
        &disposition_fields,
    );
    let disposed = observe(&fixture);
    assert_eq!(
        disposed["result"]["lifecycle_guidance"]["current_terminal_disposition"],
        true
    );
    let snapshot = durable_snapshot(&fixture);
    let duplicate = recipe_argv(
        &terminal,
        "session.terminal_disposition",
        &[
            disposition_fields[0],
            disposition_fields[1],
            ("<idempotency-key>", "duplicate-disposition"),
        ],
    );
    let rejected = fixture.output(&duplicate.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(rejected.status.code(), Some(1));
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(
        error.as_value()["code"],
        "SESSION_TERMINAL_DISPOSITION_ALREADY_RECORDED"
    );
    assert_eq!(error.as_value()["details"], json!({}));
    assert_eq!(
        execute_recipe(
            &fixture,
            &terminal,
            "session.terminal_disposition",
            &disposition_fields
        ),
        disposition,
    );
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "duplicate-disposition"
        ])["result"]["found"],
        false,
    );
    assert_eq!(durable_snapshot(&fixture), snapshot);
    execute_recipe(
        &fixture,
        &disposed,
        "session.start",
        &[
            ("<preset>", "small-change-v3"),
            ("<task>", "Terminal successor"),
            ("<idempotency-key>", "start-successor"),
        ],
    );
    let archived = fixture.run(&[
        "--contract-generation",
        "2",
        "archive",
        "show",
        "--session-id",
        disposed["session"]["id"].as_str().unwrap(),
    ]);
    let archive_observation = &archived["result"]["observation"];
    assert_eq!(
        archive_observation["lifecycle_guidance"]["activity"],
        "archived"
    );
    assert_eq!(archive_observation["mutation_templates"], json!([]));
    let default_archive = fixture.run(&[
        "archive",
        "show",
        "--session-id",
        disposed["session"]["id"].as_str().unwrap(),
    ]);
    assert_eq!(default_archive["result"], archived["result"]);
    let successor = observe(&fixture);
    execute_recipe(
        &fixture,
        &successor,
        "session.reset",
        &[("<idempotency-key>", "reset")],
    );
    assert!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "obsolete"])["result"]["outcome"]
            .is_null()
    );
}

pub(super) fn assert_scoped_delegation_replay() {
    const DIGEST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Check scoped delegation",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture.run(&[
        "set",
        "scope-summary",
        "Verify the scoped receipt",
        "--idempotency-key",
        "plan",
    ]);
    let before = observe(&fixture);
    let placement = "    - id: implement\n      use: implement-change\n";
    let source = include_str!("../../../assets/presets/small-change-default-v3.yaml");
    assert_eq!(source.matches(placement).count(), 1);
    let source = source.replacen(
        placement,
        &format!(
            "{placement}      delegation:\n        binding_ref: fixture-binding\n        role_ref: fixture-worker\n        role_snapshot_digest: {DIGEST}\n        max_rounds: 2\n        after_accept: release\n"
        ),
        1,
    );
    std::fs::write(fixture.0.join("delegated-group.yaml"), &source).unwrap();
    let preview = fixture.run(&["procedure", "preview", "delegated-group.yaml"]);
    let mut activation = stdin_template(&before, "group.activate");
    activation["context"]["idempotency_key"] = json!("activate");
    activation["source"] = json!({"kind":"document","format":"yaml","source":source});
    activation["expected_definition"] = preview["result"]["digest"].clone();
    activation["working_copy_basis"] = json!({"descriptor":"owned test worktree","digest":DIGEST});
    let activated = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        activation.to_string().as_bytes(),
    );
    assert!(activated.status.success(), "{activated:?}");

    let package = fixture.run(&["delegation", "package"]);
    fixture.run(&[
        "set",
        "implementation-summary",
        "Owned fixture work",
        "--idempotency-key",
        "output",
    ]);
    let current = fixture.run(&["delegation", "package"]);
    assert_eq!(
        package["result"]["package_digest"],
        current["result"]["package_digest"]
    );
    let missing = fixture.output(&["complete", "--idempotency-key", "before-receipt"]);
    let missing = serde_json::from_slice::<ErrorEnvelopeV2>(&missing.stdout).unwrap();
    assert_eq!(missing.as_value()["code"], "DELEGATION_RECEIPT_REQUIRED");

    let current = &current["result"];
    let mut cursor = current["cursor"].as_object().unwrap().clone();
    cursor.remove("session");
    let request = json!({
        "schema":"podway.delegation-accept-input/v2",
        "context":{
            "workspace_uuid":before["workspace"]["uuid"], "idempotency_key":"accept", "cursor":cursor,
        },
        "package_digest":current["package_digest"],
        "execution":{
            "binding_ref":"fixture-binding", "adapter_id":"fixture-adapter", "adapter_version":"1",
            "primary_host":"managed", "scope_kind":"session", "scope_ref":"fixture-scope",
            "supervisor_ref":"fixture-supervisor", "worker_ref":"fixture-worker", "task_ref":"fixture-task",
            "role_ref":"fixture-worker", "role_snapshot_digest":DIGEST,
            "agent_configuration_digest":DIGEST, "accepted_task_digest":DIGEST,
        },
        "rounds_used":1, "result_digest":DIGEST, "summary":"Accepted the fixture work.",
        "worker_disposition":"released", "accepted":true,
    }).to_string();
    let mut rejected: Value = serde_json::from_str(&request).unwrap();
    rejected["context"]["idempotency_key"] = json!("wrong-binding");
    rejected["execution"]["role_ref"] = json!("mismatched-role");
    let failure = fixture.output_with_stdin(
        &["delegation", "accept", "--stdin"],
        rejected.to_string().as_bytes(),
    );
    let error: ErrorEnvelopeV2 = serde_json::from_slice(&failure.stdout).unwrap();
    assert_eq!(error.as_value()["code"], "DELEGATION_BINDING_MISMATCH");
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "wrong-binding"])["result"]["found"],
        false
    );
    let accept =
        || fixture.output_with_stdin(&["delegation", "accept", "--stdin"], request.as_bytes());
    let accepted = accept();
    assert!(accepted.status.success(), "{accepted:?}");
    let output = serde_json::from_slice::<OutputEnvelopeV4>(&accepted.stdout).unwrap();
    assert_eq!(
        output.as_value()["result"]["schema"],
        "podway.delegation-accept-result/v3"
    );
    assert_eq!(accept().stdout, accepted.stdout);
    let target = &current["cursor"]["attempt"];
    let receipt = fixture.run(&[
        "delegation",
        "receipt",
        "--session",
        target["session"].as_str().unwrap(),
        "--attempt",
        target["attempt"].as_str().unwrap(),
    ]);
    assert_eq!(receipt["result"]["target"], *target);
    assert_eq!(receipt["result"]["acceptance_state"], "accepted");
    assert_eq!(
        receipt["result"]["receipt"]["receipt_id"],
        output.as_value()["result"]["receipt_id"],
    );
    let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", "accept"]);
    assert_eq!(lookup["result"]["outcome"]["response"], *output.as_value());
    fixture.run(&["complete", "--idempotency-key", "after-receipt"]);
    let advanced = observe(&fixture);
    let mut stale: Value = serde_json::from_str(&request).unwrap();
    stale["context"]["idempotency_key"] = json!("stale-group-receipt");
    let output = fixture.output_with_stdin(
        &["delegation", "accept", "--stdin"],
        stale.to_string().as_bytes(),
    );
    assert_refusal(
        &fixture,
        output,
        "ATTEMPT_NOT_CURRENT",
        Some("stale-group-receipt"),
    );
    assert_eq!(observe(&fixture)["result"], advanced["result"]);

    let historical = fixture.run(&[
        "delegation",
        "receipt",
        "--session",
        target["session"].as_str().unwrap(),
        "--attempt",
        target["attempt"].as_str().unwrap(),
    ]);
    assert_eq!(historical["result"]["applicability"], "historical");
    assert_eq!(
        historical["result"]["receipt"],
        receipt["result"]["receipt"]
    );
    assert_eq!(
        observe(&fixture)["result"]["cursor"]["attempt"]["node"],
        "verify"
    );
}

pub(super) fn scoped_criterion_fixture(begin_with_goal: bool) -> (Fixture, Value, Value) {
    scoped_criterion_fixture_with_criteria(begin_with_goal, &["proof=The result is current."])
}

fn scoped_criterion_fixture_with_criteria(
    begin_with_goal: bool,
    criteria: &[&str],
) -> (Fixture, Value, Value) {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let result_item = json!({
        "id":"result", "type":"text", "prompt":"Record the result.", "required":true
    });
    let base = json!({
        "schema":"podway.procedure/v3", "id":"criterion-base", "version":"1",
        "name":"Criterion fixture", "purpose":"Exercise a scoped evidence citation.",
        "goal_tracking":true,
        "node_definitions":{
            "plan":{"type":"action","title":"Plan","intent":"Prepare the evidence."},
            "assess":{
                "type":"decision","title":"Assess","objective":"Assess current scoped evidence.",
                "prompt":"Choose the criterion result.",
                "items":[{"id":"note","type":"text","prompt":"Record an assessment note.","required":false}],
                "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},
                    {"id":"superseded","label":"Superseded"}],
                "reason":{"required":true},
                "assessment":{"target":"session_goal","outcomes":{
                    "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"
                }}
            },
            "close":{"type":"action","title":"Close","intent":"Finish the assessment."}
        },
        "graph":{"entry":"plan","nodes":[
            {"id":"plan","use":"plan","next":"assess"},
            {"id":"assess","use":"assess",
                "evidence_from":[
                    {"scope":"execution_result","required":true,"items":["result"]},
                    {"scope":"execution_result","required":false,"items":["result"]}
                ],
                "routes":{
                    "achieved":{"effect":"advance","to":"close"},
                    "not-achieved":{"effect":"advance","to":"close"},
                    "superseded":{"effect":"advance","to":"close"}
                }
            },
            {"id":"close","use":"close","terminal":true}
        ]},
        "execution_group":{"anchor":"plan","success_return":"assess","plan_inputs":[],
            "result_items":[result_item.clone()],"replan_decisions":[]
        }
    });
    let group = json!({
        "schema":"podway.execution-group/v1","id":"criterion-group","version":"1",
        "name":"Criterion group","purpose":"Record one current result.",
        "plan_inputs":[],
        "node_definitions":{"collect":{"type":"action","title":"Collect","intent":"Record the result.",
            "items":[result_item]}},
        "graph":{"entry":"collect","nodes":[{"id":"collect","use":"collect","terminal":true}]}
    });
    for (name, source) in [("procedure.json", &base), ("group.json", &group)] {
        std::fs::write(fixture.0.join(name), source.to_string()).unwrap();
    }
    let preview = fixture.run(&["procedure", "preview", "procedure.json"]);
    fixture.run(&[
        "start",
        "--procedure",
        "procedure.json",
        "--expect-procedure-digest",
        preview["result"]["digest"].as_str().unwrap(),
        "--task",
        "Scoped criteria",
        "--idempotency-key",
        "start",
    ]);
    let mut goal = vec![
        "--goal",
        "Check the result.",
        "--actor",
        "fixture-coordinator",
    ];
    for &criterion in criteria {
        goal.extend(["--criterion", criterion]);
    }
    if begin_with_goal {
        fixture.run(&[&["begin"][..], &goal, &["--idempotency-key", "begin"]].concat());
    } else {
        fixture.run(&["begin", "--idempotency-key", "begin"]);
        fixture.run(
            &[
                &["goal", "define"][..],
                &goal,
                &["--idempotency-key", "define"],
            ]
            .concat(),
        );
    }
    let preview = fixture.run(&["procedure", "preview", "group.json"]);
    let mut activate = stdin_template(&observe(&fixture), "group.activate");
    activate["context"]["idempotency_key"] = json!("activate");
    activate["source"] = json!({"kind":"document","format":"json","source":group.to_string()});
    activate["expected_definition"] = preview["result"]["digest"].clone();
    activate["working_copy_basis"] = json!({
        "descriptor":"Fixture state.",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111",
    });
    let activation = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        activate.to_string().as_bytes(),
    );
    assert!(activation.status.success(), "{activation:?}");
    fixture.run(&[
        "set",
        "result",
        "The fixture recorded a current result.",
        "--idempotency-key",
        "record",
    ]);
    let group_attempt = observe(&fixture)["result"]["cursor"]["attempt"].clone();
    fixture.run(&["complete", "--idempotency-key", "return"]);
    let current = observe(&fixture);
    assert_eq!(current["result"]["cursor"]["attempt"]["node"], "assess");

    (fixture, current, group_attempt)
}

pub(super) fn assert_scoped_citation_cases() {
    let (fixture, current, group_attempt) = scoped_criterion_fixture(true);

    for generation in ["1", "2"] {
        let output = fixture.output(&[
            "--contract-generation",
            generation,
            "goal",
            "assess-criterion",
            "proof",
            "--status",
            "satisfied",
            "--reason",
            "Test a rejected citation.",
            "--item",
            "note",
            "--idempotency-key",
            "reject-citation",
        ]);
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        let error = error.as_value();
        assert_eq!(error["code"], "CRITERION_CITATION_INVALID");
        let details = &error["details"];
        assert_eq!(
            details["schema"],
            format!("podway.scoped-criterion-citation-invalid-details/v{generation}")
        );
        assert_eq!(details["decision"], current["result"]["cursor"]["attempt"]);
        assert_eq!(details["citation"], json!({"kind":"item","item":"note"}));
        assert_eq!(details["reason"], "unrecorded_item");
        if generation == "1" {
            assert!(details.get("citable_evidence").is_none());
        } else {
            assert_eq!(
                details["citable_evidence"],
                json!([
                    {"selector":{"kind":"execution_result"},"attempt":group_attempt}
                ])
            );
            assert_eq!(details["citable_item_ids"], json!([]));
        }
    }
    assert_eq!(observe(&fixture)["result"], current["result"]);
    assert!(fixture.run(&["outcome","lookup","--idempotency-key","reject-citation"])
        ["result"]["outcome"].is_null());
    fixture.run(&[
        "goal",
        "assess-criterion",
        "proof",
        "--status",
        "satisfied",
        "--reason",
        "The bound group recorded the result.",
        "--evidence",
        "@execution-result",
        "--idempotency-key",
        "accept-citation",
    ]);
    assert_eq!(observe(&fixture)["result"]["goal"]["outcome"], "achieved");
}

#[test]
fn scoped_assessment_recipes_require_an_admissible_partial_set() {
    let (fixture, before, _) = scoped_criterion_fixture_with_criteria(
        true,
        &[
            "proof=The result is current.",
            "review=The result was reviewed.",
        ],
    );
    scoped(
        &fixture,
        &before,
        &[
            "set",
            "note",
            "An optional assessment note.",
            "--if-item-revision",
            "0",
        ],
        "set-partial-note",
    );
    scoped(
        &fixture,
        &observe(&fixture),
        &[
            "goal",
            "assess-criterion",
            "proof",
            "--status",
            "satisfied",
            "--reason",
            "The note supports this criterion.",
            "--item",
            "note",
        ],
        "cite-partial-note",
    );
    let partial = observe(&fixture);
    assert_eq!(
        partial["result"]["goal"]["criteria"][0]["status"],
        "satisfied"
    );
    assert!(partial["result"]["goal"]["criteria"][1]["status"].is_null());
    assert_eq!(
        stdin_template(&partial, "goal.assess_criteria")["context"]["cursor"]["attempt"],
        partial["result"]["cursor"]["attempt"]
    );
    scoped(
        &fixture,
        &partial,
        &["clear", "note", "--if-item-revision", "1"],
        "clear-partial-note",
    );
    let unavailable = observe(&fixture);
    assert_eq!(
        unavailable["result"]["goal"]["criteria"][0]["status"],
        "satisfied"
    );
    assert!(unavailable["result"]["goal"]["criteria"][1]["status"].is_null());
    let operations = unavailable["result"]["guidance"]["available_operations"]
        .as_array()
        .unwrap();
    assert!(!operations.contains(&json!("goal.assess_criterion")));
    assert!(!operations.contains(&json!("goal.assess_criteria")));
    assert!(
        unavailable["result"]["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|template| template["command"] != "goal.assess_criteria")
    );
    scoped(
        &fixture,
        &unavailable,
        &[
            "set",
            "note",
            "The current citation is available again.",
            "--if-item-revision",
            "2",
        ],
        "restore-partial-note",
    );
    let restored = observe(&fixture);
    assert_eq!(
        stdin_template(&restored, "goal.assess_criteria")["context"]["cursor"]["attempt"],
        restored["result"]["cursor"]["attempt"]
    );
}

#[test]
fn scoped_clear_of_cited_optional_item_preserves_work_and_requires_fresh_assessment() {
    let (fixture, current, _) = scoped_criterion_fixture(true);
    scoped(
        &fixture,
        &current,
        &[
            "set",
            "note",
            "A current assessment note.",
            "--if-item-revision",
            "0",
        ],
        "set-note",
    );
    scoped(
        &fixture,
        &observe(&fixture),
        &[
            "goal",
            "assess-criterion",
            "proof",
            "--status",
            "satisfied",
            "--reason",
            "The optional note supports this criterion.",
            "--item",
            "note",
        ],
        "cite-note",
    );
    let cited = observe(&fixture);
    assert_eq!(cited["result"]["goal"]["outcome"], "achieved");
    assert_eq!(
        cited["result"]["guidance"]["allowed_option_ids"],
        json!(["achieved"])
    );

    let clear = || {
        scoped(
            &fixture,
            &cited,
            &["clear", "note", "--if-item-revision", "1"],
            "clear-note",
        )
    };
    let cleared = clear();
    assert_eq!(cleared["command"], "item.clear");
    assert_eq!(cleared["result"]["items"][0]["revision"], 2);
    assert_eq!(clear(), cleared);
    let original = fixture.run(&["outcome", "lookup", "--idempotency-key", "clear-note"]);
    assert_eq!(original["result"]["outcome"]["response"], cleared);

    let unavailable = observe(&fixture);
    assert_eq!(
        unavailable["result"]["cursor"]["attempt"],
        cited["result"]["cursor"]["attempt"]
    );
    assert_eq!(
        unavailable["result"]["items"],
        json!([{"item":"note","revision":2,"recorded":false}])
    );
    assert!(unavailable["result"]["goal"]["outcome"].is_null());
    assert_eq!(
        unavailable["result"]["guidance"]["allowed_option_ids"],
        json!([])
    );
    assert!(
        unavailable["result"]["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|template| template["command"] != "session.decide"
                && template["command"] != "goal.assess_criteria")
    );
    let operations = unavailable["result"]["guidance"]["available_operations"]
        .as_array()
        .unwrap();
    assert!(!operations.contains(&json!("goal.assess_criterion")));
    assert!(!operations.contains(&json!("goal.assess_criteria")));
    let legacy_selector = fixture.run(&["--contract-generation", "1", "observe"]);
    assert_eq!(
        legacy_selector["result"]["guidance"],
        unavailable["result"]["guidance"]
    );

    let cursor = unavailable["result"]["cursor"].to_string();
    let workspace = unavailable["workspace"]["uuid"].as_str().unwrap();
    let before = durable_snapshot(&fixture);
    let denied = fixture.output(&[
        "decide",
        "--option",
        "achieved",
        "--reason",
        "The cited note is gone.",
        "--if-workspace-uuid",
        workspace,
        "--if-cursor",
        &cursor,
        "--idempotency-key",
        "stale-citation-decision",
    ]);
    assert_eq!(denied.status.code(), Some(2), "{denied:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&denied.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "REQUEST_INVALID");
    assert_eq!(error["details"], json!({}));
    let lookup = fixture.run(&[
        "outcome",
        "lookup",
        "--idempotency-key",
        "stale-citation-decision",
    ]);
    assert_eq!(lookup["result"]["found"], false);
    assert_eq!(durable_snapshot(&fixture), before);

    scoped(
        &fixture,
        &unavailable,
        &[
            "retry",
            "--reason",
            "Re-assess against the retained, current execution result.",
        ],
        "fresh-decision",
    );
    let fresh = observe(&fixture);
    assert_ne!(
        fresh["result"]["cursor"]["attempt"],
        unavailable["result"]["cursor"]["attempt"]
    );
    assert_eq!(fresh["result"]["guidance"]["allowed_option_ids"], json!([]));
    assert_eq!(
        stdin_template(&fresh, "goal.assess_criteria")["context"]["cursor"]["attempt"],
        fresh["result"]["cursor"]["attempt"]
    );
    scoped(
        &fixture,
        &fresh,
        &[
            "goal",
            "assess-criterion",
            "proof",
            "--status",
            "satisfied",
            "--reason",
            "The current group result supports the criterion.",
            "--evidence",
            "@execution-result",
        ],
        "fresh-assessment",
    );
    let assessed = observe(&fixture);
    assert_eq!(
        assessed["result"]["guidance"]["allowed_option_ids"],
        json!(["achieved"])
    );
    scoped(
        &fixture,
        &assessed,
        &[
            "decide",
            "--option",
            "achieved",
            "--reason",
            "The fresh assessment is complete.",
        ],
        "fresh-decide",
    );
    let next = observe(&fixture);
    assert_eq!(next["result"]["cursor"]["attempt"]["node"], "close");
    assert_eq!(next["result"]["goal"]["outcome"], "achieved");
}

#[test]
fn scoped_status_tiers_and_dynamic_completion_share_current_facts() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Prove bounded status tiers",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    let before = observe(&fixture);
    fixture.run(&[
        "retry",
        "--reason",
        "Replace the planning attempt",
        "--idempotency-key",
        "retry",
    ]);
    let snapshot = fixture.podway_snapshot();
    let current = observe(&fixture);

    let compact = fixture.run(&["status", "--compact"]);
    assert_eq!(
        compact["result"]["schema"],
        "podway.scoped-compact-status-result/v1"
    );
    assert_eq!(compact["result"]["cursor"], current["result"]["cursor"]);
    assert_eq!(compact["result"]["trace_length"], 2);
    assert!(compact["result"].get("guidance").is_none());
    assert!(compact["result"].get("attempts").is_none());

    for command in ["session.next", "session.status"] {
        let mut invalid = current.clone();
        invalid["command"] = json!(command);
        assert!(OutputEnvelopeV4::from_value(invalid).is_err(), "{command}");
    }

    let standard = fixture.run(&["status"]);
    assert_eq!(standard["result"]["schema"], "podway.observation-result/v7");
    assert_eq!(
        standard["result"],
        fixture.run(&["--contract-generation", "2", "status"])["result"]
    );
    assert_eq!(standard["result"], fixture.run(&["next"])["result"]);
    assert_eq!(
        standard["result"],
        fixture.run(&["--contract-generation", "2", "next"])["result"]
    );
    assert_eq!(standard["result"]["cursor"], current["result"]["cursor"]);
    let verbose = fixture.run(&["--contract-generation", "2", "status", "--verbose"]);
    assert_eq!(
        verbose["result"]["schema"],
        "podway.scoped-verbose-status-result/v1"
    );
    assert_eq!(verbose["result"]["observation"], standard["result"]);
    assert_eq!(
        verbose["result"],
        fixture.run(&["status", "--verbose"])["result"]
    );
    assert_eq!(verbose["result"]["attempts"][0]["trace_sequence"], 2);
    assert_eq!(verbose["result"]["attempts"][1]["trace_sequence"], 1);
    assert_eq!(
        verbose["result"]["attempts"][1]["attempt"],
        before["result"]["cursor"]["attempt"]
    );
    assert_eq!(verbose["result"]["attempts"][1]["validity"], "stale");
    assert!(verbose["result"]["history_before"].is_null());

    let older = fixture.run(&["status", "--verbose", "--history-before", "2"]);
    assert_eq!(older["result"]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(older["result"]["attempts"][0]["trace_sequence"], 1);
    assert_eq!(
        older["result"]["observation"]["cursor"],
        current["result"]["cursor"]
    );

    let completions = fixture.raw_output(&["__complete", "items"]);
    assert!(completions.status.success() && completions.stderr.is_empty());
    assert_eq!(
        String::from_utf8(completions.stdout).unwrap().trim(),
        "scope-summary"
    );
    fixture.assert_podway_snapshot(&snapshot);
}

fn assert_current_decision_guards(
    fixture: &Fixture,
    decision: &Value,
    score: i64,
    verdict: Option<&str>,
    before: &[(std::path::PathBuf, Option<Vec<u8>>, u64, u64)],
) {
    let current = &decision["result"];
    let guidance = &current["guidance"];
    let generation1 = fixture.run(&["--contract-generation", "1", "observe"]);
    assert_eq!(
        generation1["result"]["schema"],
        "podway.observation-result/v7"
    );
    assert_eq!(current["schema"], "podway.observation-result/v8");
    assert_eq!(generation1["result"]["guidance"], *guidance);
    let allowed = if verdict.is_some() {
        json!(["changes-requested"])
    } else {
        json!([])
    };
    assert_eq!(guidance["allowed_option_ids"], allowed);
    assert_eq!(
        guidance["replan_option_ids"],
        if verdict.is_some() && score == 80 {
            json!(["replan"])
        } else {
            json!([])
        }
    );
    let statuses = guidance["option_guard_statuses"].as_array().unwrap();
    assert_eq!(statuses.len(), 3);
    let expected = if verdict.is_some() {
        "unmet"
    } else {
        "unevaluable"
    };
    assert_eq!(statuses[0]["option_id"], "ready");
    assert_eq!(statuses[0]["state"], expected);
    assert_eq!(
        statuses[0]["predicates"][0]["source"]["node"],
        "assess-review"
    );
    assert_eq!(statuses[0]["predicates"][0]["item"], "assessment-verdict");

    let cursor = current["cursor"].to_string();
    let workspace = decision["workspace"]["uuid"].as_str().unwrap();
    for (generation, option, key, code) in [
        (
            "1",
            "ready",
            "guard-rejected-g1",
            "OPTION_GUARD_UNSATISFIED",
        ),
        (
            "2",
            "ready",
            "guard-rejected-g2",
            "OPTION_GUARD_UNSATISFIED",
        ),
        ("2", "undeclared", "undeclared-option", "OPTION_NOT_ALLOWED"),
        ("2", "replan", "wrong-replan-route", "ROUTE_NOT_ALLOWED"),
    ] {
        let output = fixture.output(&[
            "--contract-generation",
            generation,
            "decide",
            "--option",
            option,
            "--reason",
            "This exact option must be rejected.",
            "--if-workspace-uuid",
            workspace,
            "--if-cursor",
            &cursor,
            "--idempotency-key",
            key,
        ]);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&output.stdout).unwrap();
        let error = error.as_value();
        assert_eq!(error["command"], "session.decide");
        assert_eq!(error["code"], code);
        assert_eq!(error["retryable"], false);
        let details = &error["details"];
        assert_eq!(details["decision"], current["cursor"]["attempt"]);
        assert_eq!(details["option"], option);
        match code {
            "OPTION_GUARD_UNSATISFIED" => {
                assert_eq!(
                    details["schema"],
                    "podway.scoped-option-guard-unsatisfied-details/v1"
                );
                assert_eq!(details["state"], expected);
                assert_eq!(details["predicates"], statuses[0]["predicates"]);
            }
            "OPTION_NOT_ALLOWED" => {
                assert_eq!(details["allowed_option_ids"], allowed);
            }
            "ROUTE_NOT_ALLOWED" => {
                assert_eq!(
                    details["argv"],
                    json!(["podway", "--json", "group", "replan", "--stdin"])
                );
                assert_eq!(details["input_schema"], "podway.group-replan-input/v1");
            }
            _ => unreachable!("the matrix declares a closed set"),
        }
        let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", key]);
        assert!(lookup["result"]["outcome"].is_null());
    }
    let mut after = fixture.podway_snapshot();
    after.retain(|entry| entry.0 != std::path::Path::new("runtime/state.sqlite3-shm"));
    assert_eq!(after, before);
    assert_eq!(observe(fixture)["result"], *current);
}
