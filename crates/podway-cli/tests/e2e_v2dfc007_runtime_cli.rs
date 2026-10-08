//! Retained runtime, fencing, and rendering through the single production binary.

use std::fs;

use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::Value;

use super::cli_fixture::Fixture;

const STATE: &str =
    include_str!("../../../tests/fixtures/v2/procedures/state-derivation-procedure.yaml");
const RETRY: &str = include_str!("../../../tests/fixtures/v2/procedures/retry-procedure.yaml");
const SKIP: &str = include_str!("../../../tests/fixtures/v2/procedures/skip-procedure.yaml");

fn running_fixture(procedure: &str) -> Fixture {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(fixture.0.join("procedure.yaml"), procedure).unwrap();
    let preview = fixture.run(&["procedure", "preview", "procedure.yaml"]);
    fixture.run(&[
        "start",
        "--procedure",
        "procedure.yaml",
        "--expect-procedure-digest",
        preview["result"]["procedure_digest"].as_str().unwrap(),
        "--task",
        "Retained runtime",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture
}

fn fences(before: &Value, attempt: bool) -> Vec<String> {
    let status = &before["result"];
    let mut argv = vec![
        "--if-workspace-uuid".into(),
        before["workspace"]["uuid"].as_str().unwrap().into(),
        "--if-session-id".into(),
        status["session"]["id"].as_str().unwrap().into(),
        "--if-session-revision".into(),
        status["session"]["revision"].as_u64().unwrap().to_string(),
    ];
    if attempt {
        argv.extend([
            "--if-attempt".into(),
            status["current"]["attempt"]["attempt_id"]
                .as_str()
                .unwrap()
                .into(),
        ]);
    }
    argv
}

fn transition(
    fixture: &Fixture,
    args: &[&str],
    key: &str,
    attempt: bool,
    explicit: bool,
    json: bool,
) -> Value {
    let before = fixture.run(&["status"]);
    let original_fences = fences(&before, attempt);
    let mut argv: Vec<String> = args.iter().map(|arg| (*arg).into()).collect();
    argv.extend(["--idempotency-key".into(), key.into()]);
    if explicit {
        argv.extend(original_fences.clone());
    }
    if json {
        argv.push("--json".into());
    }
    let output = fixture.raw_output(&argv.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let original = fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["outcome"]["response"].clone();
    assert!(original.is_object());
    if json {
        assert_eq!(
            serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
                .unwrap()
                .into_value(),
            original
        );
    } else {
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("workspace:"), "{text}");
        assert!(
            text.contains(original["result"]["schema"].as_str().unwrap()),
            "{text}"
        );
    }
    let read_after = if matches!(
        original["command"].as_str(),
        Some("session.reset" | "session.archive")
    ) {
        vec!["workspace", "show"]
    } else {
        vec!["status"]
    };
    let after = fixture.run(&read_after);
    assert_eq!(
        after["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        before["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );
    if !explicit {
        argv.extend(original_fences);
    }
    argv.retain(|arg| arg != "--json");
    assert_eq!(
        fixture.run(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
        original
    );
    assert_eq!(fixture.run(&read_after)["result"], after["result"]);
    original
}

fn reject(fixture: &Fixture, args: &[&str], key: &str, code: &str) {
    let before = fixture.run(&["observe"]);
    let mut argv = args.to_vec();
    argv.extend_from_slice(&["--idempotency-key", key]);
    let output = fixture.output(&argv);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
            .unwrap()
            .as_value()["code"],
        code
    );
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["found"],
        false
    );
}

fn generated_mutation(
    fixture: &Fixture,
    command: &str,
    item: Option<&str>,
    replacements: &[(&str, &str)],
    key: &str,
) -> Value {
    let before = fixture.run(&["observe"]);
    let template = before["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| {
            template["command"] == command && item.is_none_or(|item| template["argv"][2] == item)
        })
        .unwrap_or_else(|| panic!("missing template: {command}, {item:?}"));
    let argv: Vec<String> = template["argv"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|value| {
            let literal = value.as_str().unwrap();
            if literal == "<idempotency-key>" {
                return key.to_owned();
            }
            replacements
                .iter()
                .find_map(|(placeholder, value)| (*placeholder == literal).then_some(*value))
                .unwrap_or(literal)
                .to_owned()
        })
        .collect();
    assert!(argv.iter().all(|arg| !arg.starts_with('<')), "{argv:?}");
    let args: Vec<_> = argv.iter().map(String::as_str).collect();
    let original = fixture.run(&args);
    assert_eq!(original["command"], command);
    assert_eq!(fixture.run(&args), original);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["outcome"]["response"],
        original
    );
    assert_eq!(
        fixture.run(&["observe"])["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        before["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );
    original
}

#[test]
fn generated_item_recipes_preserve_all_types_clearing_and_attachment_evidence() {
    let fixture = running_fixture(include_str!(
        "../../../tests/fixtures/v2/procedures/action-readback-procedure.yaml"
    ));
    fs::write(
        fixture.0.join("evidence.txt"),
        "Retained attachment evidence.",
    )
    .unwrap();
    let cases = [
        ("item.check", "confirmed", "", ""),
        ("item.set", "summary", "<text>", "Focused checks passed."),
        ("item.set", "outcome", "<choice>", "accepted"),
        ("item.set", "count", "<integer>", "7"),
        ("item.add", "findings", "<value>", "daemon"),
        ("item.attach", "report", "<path>", "evidence.txt"),
    ];
    for (command, item, placeholder, value) in cases {
        generated_mutation(&fixture, command, Some(item), &[(placeholder, value)], item);
    }
    let observed = fixture.run(&["observe"]);
    let items = observed["result"]["active_items"].as_array().unwrap();
    for (item, expected) in [
        ("confirmed", serde_json::json!(true)),
        ("summary", serde_json::json!("Focused checks passed.")),
        ("outcome", serde_json::json!("accepted")),
        ("count", serde_json::json!(7)),
        ("findings", serde_json::json!(["daemon"])),
    ] {
        assert_eq!(
            items.iter().find(|entry| entry["item_id"] == item).unwrap()["value"],
            expected
        );
    }
    generated_mutation(&fixture, "item.uncheck", Some("confirmed"), &[], "uncheck");
    generated_mutation(&fixture, "item.check", Some("confirmed"), &[], "recheck");
    generated_mutation(
        &fixture,
        "item.remove",
        Some("findings"),
        &[("<value>", "daemon")],
        "remove",
    );
    generated_mutation(
        &fixture,
        "item.add",
        Some("findings"),
        &[("<value>", "retained")],
        "readd",
    );
    generated_mutation(&fixture, "item.clear", Some("summary"), &[], "clear");
    let cleared = fixture.run(&["observe"]);
    assert!(
        cleared["result"]["active_items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["item_id"] == "summary")
            .unwrap()["value"]
            .is_null()
    );
    generated_mutation(
        &fixture,
        "item.set",
        Some("summary"),
        &[("<text>", "Restored summary.")],
        "restore",
    );
    generated_mutation(&fixture, "session.complete", None, &[], "capture-complete");
    let report = fixture.run(&[
        "evidence",
        "read",
        "--source",
        "capture-results",
        "--item",
        "report",
    ]);
    assert_eq!(report["result"]["item"]["item_id"], "report");
    assert_eq!(report["result"]["page"]["data"]["location"], "evidence.txt");
    generated_mutation(&fixture, "item.check", Some("consumed"), &[], "consume");
    generated_mutation(&fixture, "session.complete", None, &[], "consume-complete");
    generated_mutation(&fixture, "session.complete", None, &[], "finish-complete");
    assert_eq!(
        fixture.run(&["status"])["result"]["session"]["lifecycle"],
        "completed"
    );
}

#[test]
fn generated_cursor_and_goal_recipes_require_only_declared_placeholder_values() {
    let fixture = running_fixture(RETRY);
    generated_mutation(
        &fixture,
        "session.retry",
        None,
        &[("<reason>", "Retry the retained work.")],
        "retry",
    );
    generated_mutation(
        &fixture,
        "item.set",
        Some("result"),
        &[("<text>", "Retained source evidence.")],
        "result",
    );
    generated_mutation(&fixture, "session.complete", None, &[], "work-complete");
    generated_mutation(
        &fixture,
        "session.decide",
        None,
        &[("<reason>", "The evidence supports acceptance.")],
        "decide",
    );
    generated_mutation(&fixture, "session.complete", None, &[], "finish-complete");
    let fixture = running_fixture(SKIP);
    generated_mutation(&fixture, "session.complete", None, &[], "prepare-complete");
    generated_mutation(
        &fixture,
        "session.skip",
        None,
        &[("<text>", "The isolated source is inapplicable.")],
        "source-skip",
    );
    generated_mutation(&fixture, "session.skip", None, &[], "consumer-skip");
    generated_mutation(&fixture, "session.skip", None, &[], "finish-skip");
    let fixture = running_fixture(include_str!(
        "../../../tests/fixtures/v2/procedures/goal-outcomes.yaml"
    ));
    generated_mutation(
        &fixture,
        "goal.define",
        None,
        &[
            ("<goal>", "Verify the retained result."),
            ("<criterion>", "tested=The retained checks pass."),
        ],
        "define",
    );
    generated_mutation(
        &fixture,
        "item.set",
        Some("result"),
        &[("<text>", "The checks passed.")],
        "result",
    );
    generated_mutation(&fixture, "session.complete", None, &[], "work-complete");
    let assessed = generated_mutation(
        &fixture,
        "goal.assess_criterion",
        None,
        &[
            ("<status>", "satisfied"),
            ("<reason>", "The retained check passed."),
        ],
        "tested",
    );
    assert_eq!(assessed["result"]["result"]["criterion_id"], "tested");
    assert_eq!(assessed["result"]["determined_outcome"], "achieved");
    generated_mutation(
        &fixture,
        "session.decide",
        None,
        &[("<reason>", "The criterion is satisfied.")],
        "decide",
    );
    generated_mutation(&fixture, "session.complete", None, &[], "finish-complete");
    assert_eq!(
        fixture.run(&["status"])["result"]["session"]["lifecycle"],
        "completed"
    );
}

#[test]
fn evidence_continuations_preserve_unicode_pages_and_rework_invalidates_old_tokens() {
    let procedure = format!(
        "{}\nmanual_rework:\n  allowed_targets:\n    - work\n",
        RETRY.replace("max_length: 200", "max_length: 65536")
    );
    for explicit in [false, true] {
        let fixture = running_fixture(&procedure);
        let wide = "한".repeat(50_000);
        let set = fixture.output_with_stdin(
            &["set", "result", "--stdin", "--idempotency-key", "wide"],
            wide.as_bytes(),
        );
        assert_eq!(set.status.code(), Some(0), "{set:?}");
        fixture.run(&["complete", "--idempotency-key", "work-complete"]);
        let before = fixture.run(&["observe"]);
        let session = before["result"]["status"]["session"]["id"]
            .as_str()
            .unwrap();
        let consumer_attempt = before["result"]["status"]["current"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap();
        let workspace = before["workspace"]["uuid"].as_str().unwrap();
        let first = fixture.run(&[
            "evidence",
            "read",
            "--source",
            "work",
            "--item",
            "result",
            "--if-workspace-uuid",
            workspace,
            "--if-session-id",
            session,
            "--if-attempt",
            consumer_attempt,
        ]);
        assert_eq!(first["result"]["total_size"], 50_000);
        assert_eq!(first["result"]["truncated"], true);
        let token = first["result"]["next_page_token"].as_str().unwrap();
        let next = fixture.run(&[
            "evidence",
            "read",
            "--source",
            "work",
            "--item",
            "result",
            "--page-token",
            token,
            "--if-attempt",
            consumer_attempt,
        ]);
        assert_eq!(next["result"]["page"]["offset"], 43_690);
        assert_eq!(next["result"]["truncated"], false);
        assert!(next["result"]["next_page_token"].is_null());
        assert_eq!(
            format!(
                "{}{}",
                first["result"]["page"]["data"].as_str().unwrap(),
                next["result"]["page"]["data"].as_str().unwrap()
            ),
            wide
        );
        assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
        let invalid_revision = fixture.output(&[
            "evidence",
            "read",
            "--source",
            "work",
            "--item",
            "result",
            "--if-attempt",
            consumer_attempt,
            "--if-session-revision",
            "0",
        ]);
        assert_eq!(
            invalid_revision.status.code(),
            Some(2),
            "{invalid_revision:?}"
        );
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&invalid_revision.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "REQUEST_INVALID");
        assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
        transition(
            &fixture,
            &[
                "rework",
                "--to",
                "work",
                "--reason",
                "Refresh the evidence.",
                "--actor",
                "owner",
            ],
            "manual-rework",
            true,
            explicit,
            true,
        );
        let history = fixture.run(&["status", "--verbose"]);
        let record = &history["result"]["rework_history"]["entries"][0];
        assert_eq!(record["actor"], "owner");
        assert_eq!(record["reason"], "Refresh the evidence.");
        fixture.run(&[
            "set",
            "result",
            "Replacement evidence.",
            "--idempotency-key",
            "replacement",
        ]);
        fixture.run(&["complete", "--idempotency-key", "replacement-complete"]);
        let before = fixture.run(&["observe"]);
        let stale_first = fixture.output(&[
            "evidence",
            "read",
            "--source",
            "work",
            "--item",
            "result",
            "--if-session-id",
            session,
            "--if-attempt",
            consumer_attempt,
        ]);
        assert_eq!(stale_first.status.code(), Some(4), "{stale_first:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&stale_first.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "ATTEMPT_NOT_CURRENT");
        assert_eq!(
            error.as_value()["details"]["expected_attempt_id"],
            consumer_attempt
        );
        assert_eq!(
            error.as_value()["details"]["actual_attempt_id"],
            before["result"]["status"]["current"]["attempt"]["attempt_id"]
        );
        assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
        let stale = fixture.output(&[
            "evidence",
            "read",
            "--source",
            "work",
            "--item",
            "result",
            "--page-token",
            token,
        ]);
        assert_eq!(stale.status.code(), Some(4), "{stale:?}");
        let error: ErrorEnvelopeV2 = serde_json::from_slice(&stale.stdout).unwrap();
        assert_eq!(error.as_value()["code"], "EVIDENCE_PAGE_TOKEN_STALE");
        assert_eq!(error.as_value()["retryable"], true);
        assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
    }
}

#[test]
fn running_start_policies_preserve_or_delete_the_prior_session_with_partial_fences() {
    for preserve in [false, true] {
        for partial in [false, true] {
            let fixture = running_fixture(STATE);
            let before = fixture.run(&["observe"]);
            let session = before["result"]["status"]["session"]["id"]
                .as_str()
                .unwrap();
            let undecided = fixture.output(&[
                "start",
                "--preset",
                "small-change-v2",
                "--task",
                "successor",
                "--idempotency-key",
                "undecided",
            ]);
            assert_eq!(undecided.status.code(), Some(5), "{undecided:?}");
            let error: ErrorEnvelopeV2 = serde_json::from_slice(&undecided.stdout).unwrap();
            assert_eq!(error.as_value()["code"], "SESSION_START_DECISION_REQUIRED");
            assert_eq!(
                error.as_value()["details"]["allowed_actions"],
                serde_json::json!(["continue", "preserve", "delete"])
            );
            assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
            let mut argv = vec![
                "start",
                "--preset",
                "small-change-v2",
                "--task",
                "successor",
                "--on-existing",
                if preserve { "preserve" } else { "delete" },
            ];
            if preserve {
                argv.extend([
                    "--supersede-reason",
                    "Preserve prior work.",
                    "--actor",
                    "owner",
                ]);
            } else {
                argv.extend([
                    "--progress-summary",
                    "Delete the isolated prior work.",
                    "--yes",
                ]);
            }
            if partial {
                argv.extend(["--if-session-id", session]);
            }
            let mut preview = argv.clone();
            preview.retain(|arg| *arg != "--yes");
            preview.push("--dry-run");
            let previewed = fixture.run(&preview);
            assert_eq!(previewed["command"], "session.start_replace");
            assert_eq!(previewed["result"]["dry_run"], true);
            assert_eq!(
                previewed["result"]["existing_session"]["session_id"],
                session
            );
            assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
            argv.extend(["--idempotency-key", "replacement"]);
            let started = fixture.run(&argv);
            assert_eq!(started["command"], "session.start_replace");
            assert_eq!(started["result"]["session_state"], "prepared");
            assert_ne!(started["session"]["id"], session);
            let old_revision = before["result"]["status"]["session"]["revision"]
                .as_u64()
                .unwrap()
                .to_string();
            let mut replay = argv.clone();
            if !partial {
                replay.extend(["--if-session-id", session]);
            }
            replay.extend([
                "--if-workspace-uuid",
                before["workspace"]["uuid"].as_str().unwrap(),
                "--if-session-revision",
                &old_revision,
            ]);
            assert_eq!(fixture.run(&replay), started);
            let archives = fixture.run(&["archive", "list"]);
            assert_eq!(archives["result"]["count"], u64::from(preserve));
            if preserve {
                assert_eq!(archives["result"]["sessions"][0]["session_id"], session);
                let archived =
                    fixture.run(&["archive", "show", "--session-id", session, "--verbose"]);
                assert_eq!(archived["result"]["activity"], "inactive");
                assert_eq!(
                    archived["result"]["state"]["session"]["lifecycle"],
                    "cancelled"
                );
                assert_eq!(
                    archived["result"]["terminal_disposition"]["kind"],
                    "superseded"
                );
                assert_eq!(archived["result"]["terminal_disposition"]["actor"], "owner");
            } else {
                let absent = fixture.output(&["archive", "show", "--session-id", session]);
                assert_eq!(
                    serde_json::from_slice::<ErrorEnvelopeV2>(&absent.stdout)
                        .unwrap()
                        .as_value()["code"],
                    "SESSION_ARCHIVE_NOT_FOUND"
                );
            }
        }
    }
}

#[test]
fn prepared_replacement_preserves_partial_identity_fences_and_rejects_inapplicable_fields() {
    for partial in [false, true] {
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
        let before = fixture.run(&["status"]);
        let session = before["result"]["session"]["id"].as_str().unwrap();
        let prefix = [
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "successor",
            "--on-existing",
        ];
        let mut preserve = prefix.to_vec();
        preserve.extend([
            "preserve",
            "--supersede-reason",
            "Prepared work is not running.",
        ]);
        reject(&fixture, &preserve, "invalid-preserve", "REQUEST_INVALID");
        for (flag, key) in [
            ("--progress-summary", "invalid-summary"),
            ("--supersede-reason", "invalid-reason"),
            ("--actor", "invalid-actor"),
        ] {
            let mut invalid = prefix.to_vec();
            invalid.extend(["delete", flag, "Inapplicable value."]);
            reject(&fixture, &invalid, key, "REQUEST_INVALID");
        }
        let wrong_session = uuid::Uuid::new_v4().to_string();
        let mut wrong = prefix.to_vec();
        wrong.extend(["delete", "--if-session-id", &wrong_session]);
        reject(&fixture, &wrong, "wrong-session", "SESSION_ID_MISMATCH");
        let mut stale = prefix.to_vec();
        stale.extend(["delete", "--if-session-revision", "1"]);
        reject(
            &fixture,
            &stale,
            "wrong-revision",
            "SESSION_REVISION_CONFLICT",
        );
        let mut argv = prefix.to_vec();
        argv.extend(["delete", "--idempotency-key", "replace"]);
        if partial {
            argv.extend(["--if-session-id", session]);
        }
        let started = fixture.run(&argv);
        assert_eq!(started["command"], "session.start_replace");
        assert_eq!(started["result"]["session_state"], "prepared");
        assert_ne!(started["session"]["id"], session);
        let revision = before["result"]["session"]["revision"]
            .as_u64()
            .unwrap()
            .to_string();
        if !partial {
            argv.extend(["--if-session-id", session]);
        }
        argv.extend([
            "--if-session-revision",
            &revision,
            "--if-workspace-uuid",
            before["workspace"]["uuid"].as_str().unwrap(),
        ]);
        assert_eq!(fixture.run(&argv), started);
        assert_eq!(fixture.run(&["archive", "list"])["result"]["count"], 0);
    }
}

#[test]
fn disposed_terminal_start_archives_atomically_and_purge_honors_the_archived_revision() {
    let fixture = running_fixture(STATE);
    fixture.run(&["check", "ready", "--idempotency-key", "ready"]);
    fixture.run(&["complete", "--idempotency-key", "work-complete"]);
    fixture.run(&["complete", "--idempotency-key", "finish-complete"]);
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["session"]["id"].as_str().unwrap();
    let disposed = transition(
        &fixture,
        &[
            "disposition",
            "not-required",
            "--reason",
            "No external handoff is required.",
            "--actor",
            "owner",
        ],
        "dispose",
        false,
        false,
        true,
    );
    assert_eq!(disposed["command"], "session.terminal_disposition");
    for (policy, key, extra) in [
        ("delete", "inapplicable-delete", vec!["--yes"]),
        (
            "preserve",
            "inapplicable-preserve",
            vec!["--supersede-reason", "Inapplicable policy."],
        ),
    ] {
        let mut invalid = vec![
            "start",
            "--preset",
            "small-change-v2",
            "--task",
            "successor",
            "--on-existing",
            policy,
        ];
        invalid.extend(extra);
        reject(&fixture, &invalid, key, "REQUEST_INVALID");
    }
    let before = fixture.run(&["observe"]);
    let argv = [
        "start",
        "--preset",
        "small-change-v2",
        "--task",
        "successor",
        "--idempotency-key",
        "successor",
    ];
    let successor = fixture.run(&argv);
    assert_eq!(successor["result"]["existing_session"]["action"], "archive");
    assert_eq!(fixture.run(&argv), successor);
    let active = fixture.run(&["status"]);
    let list = fixture.run(&["archive", "list"]);
    assert_eq!(list["result"]["count"], 1);
    assert_eq!(list["result"]["sessions"][0]["session_id"], session);
    let revision = list["result"]["sessions"][0]["session_revision"]
        .as_u64()
        .unwrap();
    let stale_revision = (revision - 1).to_string();
    reject(
        &fixture,
        &[
            "archive",
            "purge",
            "--session-id",
            session,
            "--if-session-revision",
            &stale_revision,
            "--yes",
        ],
        "stale-purge",
        "SESSION_REVISION_CONFLICT",
    );
    let revision = revision.to_string();
    let purge = [
        "archive",
        "purge",
        "--session-id",
        session,
        "--if-session-revision",
        &revision,
        "--if-workspace-uuid",
        before["workspace"]["uuid"].as_str().unwrap(),
        "--yes",
        "--idempotency-key",
        "purge",
    ];
    let purged = fixture.run(&purge);
    assert_eq!(purged["result"]["purged"], true);
    assert_eq!(fixture.run(&purge), purged);
    assert_eq!(fixture.run(&["archive", "list"])["result"]["count"], 0);
    assert_eq!(fixture.run(&["status"])["result"], active["result"]);
    let missing = fixture.output(&["archive", "show", "--session-id", session, "--verbose"]);
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&missing.stdout)
            .unwrap()
            .as_value()["code"],
        "SESSION_ARCHIVE_NOT_FOUND"
    );
}

#[test]
fn bare_terminal_archive_uses_no_active_attempt_and_replays_after_current_session_removal() {
    let fixture = running_fixture(STATE);
    fixture.run(&[
        "cancel",
        "--reason",
        "Retain completed work.",
        "--idempotency-key",
        "cancel",
    ]);
    fixture.run(&[
        "disposition",
        "not-required",
        "--reason",
        "No handoff is required.",
        "--idempotency-key",
        "dispose",
    ]);
    let before = fixture.run(&["status"]);
    assert!(before["result"]["current"].is_null());
    let archived = transition(&fixture, &["archive"], "archive", false, false, true);
    assert_eq!(archived["result"]["activity"], "inactive");
    let absent = fixture.output(&["status"]);
    assert_eq!(
        serde_json::from_slice::<ErrorEnvelopeV2>(&absent.stdout)
            .unwrap()
            .as_value()["code"],
        "SESSION_NOT_FOUND"
    );
    assert_eq!(fixture.run(&["archive", "list"])["result"]["count"], 1);
    let session = before["result"]["session"]["id"].as_str().unwrap();
    assert_eq!(
        fixture.run(&["archive", "show", "--session-id", session])["result"]["state"]["session"]["lifecycle"],
        "cancelled"
    );
}

#[test]
fn block_unblock_cancel_and_force_reset_preserve_automatic_and_explicit_fences_and_rendering() {
    for explicit in [false, true] {
        for json in [false, true] {
            let fixture = running_fixture(STATE);
            fixture.run(&["check", "ready", "--idempotency-key", "ready"]);
            let blocked = transition(
                &fixture,
                &["block", "--reason", "waiting for an external decision"],
                "block",
                true,
                explicit,
                json,
            );
            assert_eq!(blocked["command"], "session.block");
            assert_eq!(blocked["result"]["transition"], "block");
            assert_eq!(
                blocked["result"]["reason"],
                "waiting for an external decision"
            );
            let status = fixture.run(&["status"]);
            assert_eq!(
                status["result"]["current"]["readiness"]["items_satisfied"],
                true
            );
            assert_eq!(status["result"]["current"]["readiness"]["unblocked"], false);
            assert_eq!(
                status["result"]["current"]["readiness"]["can_advance"],
                false
            );
            let blocker = blocked["result"]["blocker_id"].as_str().unwrap();
            let unblocked = transition(
                &fixture,
                &["unblock", blocker],
                "unblock-one",
                true,
                explicit,
                json,
            );
            assert_eq!(unblocked["command"], "session.unblock");
            assert_eq!(unblocked["result"]["blocker_id"], blocker);
            assert_eq!(
                fixture.run(&["status"])["result"]["current"]["readiness"]["can_advance"],
                true
            );
            transition(
                &fixture,
                &["block", "--reason", "waiting again"],
                "block-again",
                true,
                explicit,
                json,
            );
            let unblocked = transition(
                &fixture,
                &["unblock", "--all"],
                "unblock-all",
                true,
                explicit,
                json,
            );
            assert_eq!(unblocked["result"]["transition"], "unblock");
            assert_eq!(
                fixture.run(&["status"])["result"]["current"]["blockers_total"],
                0
            );
            let cancelled = transition(
                &fixture,
                &["cancel", "--reason", "the task is no longer needed"],
                "cancel",
                true,
                explicit,
                json,
            );
            assert_eq!(cancelled["command"], "session.cancel");
            assert_eq!(cancelled["result"]["session_state"], "cancelled");
            let terminal = fixture.run(&["status"]);
            assert_eq!(terminal["result"]["session"]["lifecycle"], "cancelled");
            assert!(terminal["result"]["current"].is_null());
            let history = fixture.run(&["status", "--verbose"]);
            assert_eq!(
                history["result"]["stale_attempt_history"]["entries"][0]["terminal_reason"],
                "the task is no longer needed"
            );
            let reset = transition(
                &fixture,
                &[
                    "reset",
                    "--yes",
                    "--progress-summary",
                    "Discard the isolated fixture session.",
                ],
                "reset",
                false,
                explicit,
                json,
            );
            assert_eq!(reset["command"], "session.reset");
            assert_eq!(reset["result"]["schema"], "podway.session-reset-result/v2");
            assert_eq!(reset["result"]["reset"], true);
            assert_eq!(reset["result"]["mode"], "force");
            let missing = fixture.output(&["status"]);
            assert_eq!(
                serde_json::from_slice::<ErrorEnvelopeV2>(&missing.stdout)
                    .unwrap()
                    .as_value()["code"],
                "SESSION_NOT_FOUND"
            );
        }
    }
}

#[test]
fn retry_and_skip_preserve_clean_attempts_reasons_and_exact_replay_in_json_and_text() {
    for explicit in [false, true] {
        for json in [false, true] {
            let fixture = running_fixture(RETRY);
            fixture.run(&[
                "set",
                "result",
                "discard on retry",
                "--idempotency-key",
                "seed",
            ]);
            let before = fixture.run(&["status"]);
            let retry = transition(
                &fixture,
                &["retry", "--reason", "repeat from clean state"],
                "retry",
                true,
                explicit,
                json,
            );
            assert_eq!(retry["command"], "session.retry");
            assert_eq!(retry["result"]["transition"], "retry");
            assert_eq!(retry["result"]["reason"], "repeat from clean state");
            let after = fixture.run(&["observe"]);
            assert_ne!(
                after["result"]["guidance"]["attempt"]["attempt_id"],
                before["result"]["current"]["attempt"]["attempt_id"]
            );
            assert_eq!(after["result"]["guidance"]["attempt"]["attempt_number"], 2);
            assert_eq!(after["result"]["active_items"][0]["value"], Value::Null);
            fixture.run(&[
                "set",
                "result",
                "retained source evidence",
                "--idempotency-key",
                "seed-again",
            ]);
            let complete = transition(&fixture, &["complete"], "complete", true, explicit, json);
            assert_eq!(complete["command"], "session.complete");
            assert_eq!(complete["result"]["to_graph_node_id"], "review");
            let decision_retry = transition(
                &fixture,
                &["retry", "--reason", "repeat without a status probe"],
                "decision-retry",
                true,
                explicit,
                json,
            );
            assert_eq!(decision_retry["result"]["transition"], "retry");
            assert_eq!(
                fixture.run(&["status"])["result"]["current"]["node"]["graph_node_id"],
                "review"
            );

            let fixture = running_fixture(SKIP);
            reject(
                &fixture,
                &["skip", "--reason", "not eligible"],
                "rejected-skip",
                "STAGE_NOT_SKIPPABLE",
            );
            transition(&fixture, &["complete"], "prepare", true, explicit, json);
            fixture.run(&[
                "set",
                "required-note",
                "discard on skip",
                "--idempotency-key",
                "skip-seed",
            ]);
            let skip = transition(
                &fixture,
                &["skip", "--reason", "not applicable"],
                "skip-source",
                true,
                explicit,
                json,
            );
            assert_eq!(skip["command"], "session.skip");
            assert_eq!(skip["result"]["transition"], "skip");
            assert_eq!(skip["result"]["reason"], "not applicable");
            assert_eq!(skip["result"]["to_graph_node_id"], "consumer");
            let skip = transition(&fixture, &["skip"], "skip-consumer", true, explicit, json);
            assert!(skip["result"].get("reason").is_none());
            let terminal = transition(&fixture, &["skip"], "skip-finish", true, explicit, json);
            assert_eq!(terminal["result"]["session_state"], "completed");
            assert!(fixture.run(&["status"])["result"]["current"].is_null());
        }
    }
}

#[test]
fn goal_definition_preserves_actor_and_stable_json_human_and_quiet_rendering() {
    for mode in ["json", "human", "quiet"] {
        let fixture = running_fixture(include_str!(
            "../../../tests/fixtures/v2/procedures/goal-outcomes.yaml"
        ));
        let mut argv = vec![
            "goal",
            "define",
            "--goal",
            "Ship the v2 platform CLI.",
            "--criterion",
            "stable-json=JSON is stable.",
            "--actor",
            "reviewer",
            "--idempotency-key",
            "define",
        ];
        match mode {
            "json" => argv.push("--json"),
            "quiet" => argv.push("--quiet"),
            _ => {}
        }
        let output = fixture.raw_output(&argv);
        assert_eq!(output.status.code(), Some(0), "{mode}: {output:?}");
        assert!(output.stderr.is_empty());
        match mode {
            "json" => {
                let envelope = serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
                    .unwrap()
                    .into_value();
                assert_eq!(envelope["command"], "goal.define");
                assert_eq!(
                    envelope["result"]["schema"],
                    "podway.goal-definition-result/v2"
                );
                assert_eq!(envelope["result"]["goal_revision"], 1);
            }
            "human" => {
                let text = String::from_utf8(output.stdout).unwrap();
                assert!(text.contains("Ship the v2 platform CLI."), "{text}");
                assert!(text.contains("stable-json"), "{text}");
            }
            "quiet" => assert!(output.stdout.is_empty()),
            _ => unreachable!(),
        }
        let original = fixture.run(&["outcome", "lookup", "--idempotency-key", "define"])["result"]
            ["outcome"]["response"]
            .clone();
        assert_eq!(original["result"]["actor"], "reviewer");
    }
}

#[test]
fn general_decisions_evidence_queries_and_verbose_history_preserve_fences_and_actor() {
    for explicit in [false, true] {
        let fixture = running_fixture(RETRY);
        for round in 0..2 {
            let set_key = format!("source-{round}");
            fixture.run(&[
                "set",
                "result",
                "Retained source evidence.",
                "--idempotency-key",
                &set_key,
            ]);
            let complete_key = format!("complete-{round}");
            transition(&fixture, &["complete"], &complete_key, true, explicit, true);
            let before = fixture.run(&["status"]);
            let session = before["result"]["session"]["id"].as_str().unwrap();
            let evidence = fixture.run(&[
                "evidence",
                "read",
                "--source",
                "work",
                "--item",
                "result",
                "--if-session-id",
                session,
                "--if-workspace-uuid",
                before["workspace"]["uuid"].as_str().unwrap(),
            ]);
            assert_eq!(evidence["command"], "evidence.read");
            assert_eq!(
                evidence["result"]["schema"],
                "podway.evidence-read-result/v1"
            );
            assert_eq!(evidence["result"]["item"]["item_id"], "result");
            assert_eq!(evidence["result"]["source"]["graph_node_id"], "work");
            assert_eq!(evidence["result"]["consumer"]["graph_node_id"], "review");
            assert_eq!(
                evidence["result"]["page"]["data"],
                "Retained source evidence."
            );
            assert_eq!(evidence["result"]["truncated"], false);
            assert!(evidence["result"]["next_page_token"].is_null());
            assert_eq!(fixture.run(&["status"])["result"], before["result"]);
            assert_eq!(
                evidence["workspace"]["latest_workspace_sequence"],
                before["workspace"]["latest_workspace_sequence"]
            );
            let key = format!("reject-{round}");
            let decision = transition(
                &fixture,
                &[
                    "decide",
                    "--option",
                    "reject",
                    "--reason",
                    "Review found a gap.",
                    "--actor",
                    "owner",
                ],
                &key,
                true,
                explicit,
                true,
            );
            assert_eq!(decision["command"], "session.decide");
            assert_eq!(decision["result"]["record"]["actor"], "owner");
            assert_eq!(
                decision["result"]["record"]["reason"],
                "Review found a gap."
            );
            assert!(decision["result"]["record"]["goal_revision"].is_null());
            assert_eq!(decision["result"]["record"]["effect"], "rework");
            assert_eq!(
                fixture.run(&["status"])["result"]["current"]["node"]["graph_node_id"],
                "work"
            );
        }
        let before = fixture.run(&["status"]);
        let session = before["result"]["session"]["id"].as_str().unwrap();
        let argv = [
            "status",
            "--verbose",
            "--history-before",
            "42",
            "--if-session-id",
            session,
        ];
        let json = fixture.run(&argv);
        for (field, count, truncated) in
            [("decision_history", 1, true), ("rework_history", 2, false)]
        {
            let history = &json["result"][field];
            assert_eq!(history["entries"].as_array().unwrap().len(), count);
            assert_eq!(history["trace_truncated"], truncated);
            let entries = history["entries"].as_array().unwrap();
            assert!(
                entries
                    .windows(2)
                    .all(|pair| pair[0]["trace_sequence"].as_u64().unwrap()
                        > pair[1]["trace_sequence"].as_u64().unwrap())
            );
            assert!(
                entries
                    .iter()
                    .all(|entry| entry["trace_sequence"].as_u64().unwrap() < 42)
            );
        }
        let human = fixture.raw_output(&argv);
        assert_eq!(human.status.code(), Some(0), "{human:?}");
        let text = String::from_utf8(human.stdout).unwrap();
        let rendered: Value = serde_json::from_str(
            text.split_once("result: ")
                .expect("verbose status retains the generic result")
                .1,
        )
        .unwrap();
        assert_eq!(
            rendered["decision_history"],
            json["result"]["decision_history"]
        );
        assert_eq!(rendered["rework_history"], json["result"]["rework_history"]);
        let cutoff = json["result"]["decision_history"]["entries"][0]["trace_sequence"]
            .as_u64()
            .unwrap()
            .to_string();
        let page = fixture.run(&["status", "--verbose", "--history-before", &cutoff]);
        assert_eq!(
            page["result"]["rework_history"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            page["result"]["decision_history"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(page["result"]["decision_history"]["trace_truncated"], false);
        assert!(
            page["result"]["decision_history"]["entries"][0]["trace_sequence"]
                .as_u64()
                .unwrap()
                < json["result"]["decision_history"]["entries"][0]["trace_sequence"]
                    .as_u64()
                    .unwrap()
        );
        let invalid = fixture.output(&["status", "--verbose", "--history-before", "0"]);
        assert_eq!(invalid.status.code(), Some(2));
        assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    }
}
