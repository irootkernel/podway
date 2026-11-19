//! Retained runtime, fencing, and rendering through the single production binary.

use std::fs;

use podway_protocol::{ErrorEnvelopeV2, OutputEnvelopeV4};
use serde_json::Value;

use super::cli_fixture::Fixture;

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

fn scoped_runtime_transition(
    fixture: &Fixture,
    semantic: &[&str],
    key: &str,
    scoped: bool,
    explicit: bool,
    json: bool,
) -> Value {
    let before = scoped_runtime_observe(fixture);
    let cursor = &before["result"]["cursor"];
    let workspace = before["workspace"]["uuid"].as_str().unwrap();
    let mut fences = vec!["--if-workspace-uuid".to_owned(), workspace.to_owned()];
    if scoped {
        fences.extend(["--if-cursor".to_owned(), cursor.to_string()]);
    } else {
        fences.extend([
            "--if-session-id".to_owned(),
            cursor["session"].as_str().unwrap().to_owned(),
            "--if-session-revision".to_owned(),
            cursor["session_revision"].to_string(),
        ]);
    }
    let mut args = semantic
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    args.extend(["--idempotency-key".to_owned(), key.to_owned()]);
    if explicit {
        args.extend(fences.clone());
    }
    fn render(args: &[String]) -> Vec<&str> {
        args.iter().map(String::as_str).collect()
    }
    let output = if json {
        fixture.output(&render(&args))
    } else {
        fixture.raw_output(&render(&args))
    };
    assert!(output.status.success(), "{semantic:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", key]);
    let original = lookup["result"]["outcome"]["response"].clone();
    assert!(original.is_object(), "{lookup}");
    if json {
        assert_eq!(
            serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
                .unwrap()
                .into_value(),
            original
        );
    } else {
        let human = String::from_utf8(output.stdout).unwrap();
        assert!(human.contains("workspace:"), "{human}");
        assert!(
            human.contains(original["result"]["schema"].as_str().unwrap()),
            "{human}"
        );
    }
    let after_route = if semantic[0] == "reset" {
        &["workspace", "show"][..]
    } else {
        &["status"][..]
    };
    let after = fixture.run(after_route);
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
        args.extend(fences);
    }
    assert_eq!(fixture.run(&render(&args)), original);
    assert_eq!(fixture.run(after_route)["result"], after["result"]);
    original
}

fn scoped_runtime_retry_fixture() -> Fixture {
    use serde_json::json;
    scoped_runtime_start(&scoped_runtime_base(
        json!({
            "work":{"type":"action","title":"Work","intent":"Record current evidence.",
                "items":[scoped_runtime_source("result",200)]},
            "review":{"type":"decision","title":"Review","objective":"Evaluate the current result.",
                "prompt":"Accept this result?","reason":{"required":true},
                "options":[{"id":"accept","label":"Accept"},{"id":"reject","label":"Reject"}]}
        }),
        vec![
            json!({"id":"work","use":"work","next":"review"}),
            json!({"id":"review","use":"review",
            "evidence_from":[{"scope":"local","node":"work","required":true,"items":["result"]}],
            "routes":{"accept":{"effect":"advance","to":"plan"},"reject":{"effect":"rework","to":"work"}}}),
        ],
        false,
    ))
}

fn scoped_runtime_skip_fixture() -> Fixture {
    use serde_json::json;
    scoped_runtime_start(&scoped_runtime_base(
        json!({
            "prepare":{"type":"action","title":"Prepare","intent":"Reach the skippable prefix."},
            "source":{"type":"action","title":"Source","intent":"Record optional evidence.",
                "items":[scoped_runtime_source("required-note",200)]},
            "consumer":{"type":"action","title":"Consumer","intent":"Use available evidence."}
        }),
        vec![
            json!({"id":"prepare","use":"prepare","next":"source"}),
            json!({"id":"source","use":"source","next":"consumer","skip":{"allowed":true,"reason_required":true}}),
            json!({"id":"consumer","use":"consumer","next":"plan","skip":{"allowed":true,"reason_required":false},
            "evidence_from":[{"scope":"local","node":"source","required":false}]}),
        ],
        false,
    ))
}

fn scoped_runtime_source(item: &str, limit: u64) -> Value {
    serde_json::json!({"id":item,"type":"text","prompt":"Record the source value.",
        "required":true,"max_length":limit})
}

fn scoped_runtime_base(mut definitions: Value, mut nodes: Vec<Value>, tracked: bool) -> Value {
    let entry = nodes
        .first()
        .map(|node| node["id"].clone())
        .unwrap_or_else(|| serde_json::json!("plan"));
    definitions["plan"] = serde_json::json!({
        "type":"action","title":"Plan","intent":"Activate the explicitly selected work group."
    });
    definitions["finish"] = serde_json::json!({
        "type":"action","title":"Finish","intent":"Record the terminal disposition basis."
    });
    let return_node = if tracked {
        definitions["assess"] = serde_json::json!({
            "type":"decision","title":"Assess","objective":"Assess the selected group result.",
            "prompt":"What is the resulting goal outcome?",
            "options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},
                {"id":"superseded","label":"Superseded"}],
            "reason":{"required":true},
            "assessment":{"target":"session_goal","outcomes":{
                "achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"
            }}
        });
        "assess"
    } else {
        "finish"
    };
    nodes.push(serde_json::json!({"id":"plan","use":"plan","next":return_node}));
    if tracked {
        nodes.push(serde_json::json!({
            "id":"assess","use":"assess",
            "evidence_from":[{"scope":"execution_result","required":true,"items":["result"]}],
            "routes":{
                "achieved":{"effect":"advance","to":"finish"},
                "not-achieved":{"effect":"advance","to":"finish"},
                "superseded":{"effect":"advance","to":"finish"},
            }
        }));
    }
    nodes.push(serde_json::json!({"id":"finish","use":"finish","terminal":true}));
    let results = if tracked {
        vec![scoped_runtime_source("result", 200)]
    } else {
        vec![]
    };
    let mut base = serde_json::json!({
        "schema":"podway.procedure/v3","id":"scoped-runtime-regression","version":"1",
        "name":"Scoped runtime regression","purpose":"Exercise retained current and historical state.",
        "node_definitions":definitions,
        "graph":{"entry":entry,"nodes":nodes},
        "execution_group":{"anchor":"plan","success_return":return_node,"plan_inputs":[],
            "result_items":results,"replan_decisions":[]},
    });
    if tracked {
        base["goal_tracking"] = serde_json::json!(true);
    }
    base
}

fn scoped_runtime_start(base: &Value) -> Fixture {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    fs::write(fixture.0.join("scoped.json"), base.to_string()).unwrap();
    let preview = fixture.run(&["procedure", "preview", "scoped.json"]);
    fixture.run(&[
        "start",
        "--procedure",
        "scoped.json",
        "--expect-procedure-digest",
        preview["result"]["digest"].as_str().unwrap(),
        "--task",
        "Scoped runtime regression",
        "--idempotency-key",
        "start",
    ]);
    fixture.run(&["begin", "--idempotency-key", "begin"]);
    fixture
}

fn scoped_runtime_observe(fixture: &Fixture) -> Value {
    fixture.run(&["--contract-generation", "2", "observe"])
}

fn scoped_runtime_template<'a>(observation: &'a Value, command: &str) -> &'a Value {
    observation["result"]["mutation_templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|template| template["command"] == command)
        .unwrap_or_else(|| {
            panic!(
                "missing {command} template at {}",
                observation["result"]["cursor"]
            )
        })
}

fn scoped_runtime_recipe(
    fixture: &Fixture,
    command: &str,
    item: Option<&str>,
    replacements: &[(&str, &str)],
    key: &str,
) -> Value {
    let before = scoped_runtime_observe(fixture);
    let template = scoped_runtime_template(&before, command);
    let item_revision = item.map(|id| {
        before["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["item"] == id)
            .unwrap()["revision"]
            .to_string()
    });
    let argv = template["argv"].as_array().unwrap();
    assert_eq!(
        &argv[..2],
        &[serde_json::json!("podway"), serde_json::json!("--json")]
    );
    let args: Vec<_> = argv[2..]
        .iter()
        .map(|argument| {
            let argument = argument.as_str().unwrap();
            match argument {
                "<idempotency-key>" => key,
                "<item-id>" => item.unwrap(),
                "<item-revision>" => item_revision.as_deref().unwrap(),
                _ => replacements
                    .iter()
                    .find_map(|(placeholder, value)| (*placeholder == argument).then_some(*value))
                    .unwrap_or(argument),
            }
        })
        .collect();
    assert!(args.iter().all(|arg| !arg.starts_with('<')), "{args:?}");
    let original = fixture.output(&args);
    assert!(original.status.success(), "{command}: {original:?}");
    assert_eq!(fixture.output(&args).stdout, original.stdout);
    let original: Value = serde_json::from_slice::<OutputEnvelopeV4>(&original.stdout)
        .unwrap()
        .into_value();
    assert_eq!(original["command"], command);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["outcome"]["response"],
        original
    );
    assert_eq!(
        scoped_runtime_observe(fixture)["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        before["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );
    original
}

fn scoped_runtime_activate(fixture: &Fixture, tracked: bool) {
    scoped_runtime_activate_with_key(fixture, tracked, "activate");
}

fn scoped_runtime_activate_with_key(fixture: &Fixture, tracked: bool, key: &str) {
    let mut perform =
        serde_json::json!({"type":"action","title":"Perform","intent":"Record the group result."});
    if tracked {
        perform["items"] = serde_json::json!([scoped_runtime_source("result", 200)]);
    }
    let group = serde_json::json!({
        "schema":"podway.execution-group/v1","id":"scoped-runtime-group","version":"1",
        "name":"Runtime regression group","purpose":"Settle one admitted group.",
        "plan_inputs":[],
        "node_definitions":{"perform":perform},
        "graph":{"entry":"perform","nodes":[{"id":"perform","use":"perform","terminal":true}]}
    })
    .to_string();
    fs::write(fixture.0.join("group.json"), &group).unwrap();
    let digest = fixture.run(&["procedure", "preview", "group.json"])["result"]["digest"].clone();
    let before = scoped_runtime_observe(fixture);
    let template = scoped_runtime_template(&before, "group.activate");
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = serde_json::json!(key);
    if input["predecessor"].is_string() {
        assert_eq!(input["prior_work"], "<prior-work-dispositions-array>");
        input["prior_work"] = serde_json::json!([{
            "group_execution_id": input["predecessor"], "choice": "revalidate",
            "rationale": "Retain the prior fixture work and obtain a fresh result for the revised goal."
        }]);
    }
    input["source"] = serde_json::json!({"kind":"document","format":"json","source":group});
    input["expected_definition"] = digest;
    input["working_copy_basis"] = serde_json::json!({
        "descriptor":"Owned runtime regression worktree",
        "digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111",
    });
    if let Some(predecessor) = input["predecessor"].as_str() {
        let snapshot = || {
            let mut files = fixture.podway_snapshot();
            files.retain(|entry| entry.0 != std::path::Path::new("runtime/state.sqlite3-shm"));
            files
        };
        let before_mutations = snapshot();
        let invalid_prior_work = [
            ("no-prior-work", serde_json::json!([])),
            (
                "unknown-prior-group",
                serde_json::json!([{
                    "group_execution_id":"00000000-0000-4000-8000-000000000001",
                    "choice":"revalidate", "rationale":"Reject the unknown execution."
                }]),
            ),
            (
                "mismatched-prior-attempt",
                serde_json::json!([{
                    "group_execution_id":predecessor, "choice":"revalidate",
                    "rationale":"Reject the attempt outside the preceding execution.",
                    "attempt":before["result"]["cursor"]["attempt"]["attempt"]
                }]),
            ),
        ];
        for (key, prior_work) in invalid_prior_work {
            let mut invalid = input.clone();
            invalid["context"]["idempotency_key"] = serde_json::json!(key);
            invalid["prior_work"] = prior_work;
            let error = scoped_runtime_error(
                fixture.output_with_stdin(
                    &["group", "activate", "--stdin"],
                    invalid.to_string().as_bytes(),
                ),
                1,
                "GROUP_ACTIVATION_NOT_ALLOWED",
            );
            assert_eq!(error["details"], serde_json::json!({}));
            assert_eq!(scoped_runtime_observe(fixture)["result"], before["result"]);
            assert_eq!(
                fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["found"],
                false
            );
            assert_eq!(snapshot(), before_mutations);
        }
    }
    let original = fixture.output_with_stdin(
        &["group", "activate", "--stdin"],
        input.to_string().as_bytes(),
    );
    assert!(original.status.success(), "{original:?}");
    assert_eq!(
        fixture
            .output_with_stdin(
                &["group", "activate", "--stdin"],
                input.to_string().as_bytes()
            )
            .stdout,
        original.stdout
    );
}

fn scoped_runtime_history_item(fixture: &Fixture, attempt: &Value, item: &str) -> Value {
    fixture.run(&[
        "history",
        "item",
        "--session",
        attempt["session"].as_str().unwrap(),
        "--attempt",
        &attempt.to_string(),
        "--item",
        item,
    ])["result"]
        .clone()
}

fn scoped_runtime_error(output: std::process::Output, exit: i32, code: &str) -> Value {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], code);
    error
}

#[test]
fn calls003_help_and_human_observation_explain_reads_records_and_counters() {
    let base = scoped_runtime_base(serde_json::json!({}), vec![], false);
    let fixture = scoped_runtime_start(&base);
    let before = scoped_runtime_observe(&fixture);
    let human = fixture.raw_output(&["observe"]);
    assert!(human.status.success(), "{human:?}");
    let text = String::from_utf8(human.stdout).unwrap();
    for phrase in [
        "current scoped cursor",
        "active item revisions",
        "Generation 2 adds executable templates",
        "history queries",
        "Record typed items through record --stdin",
        "input basis, executor, outcome and output digest",
        "does not execute or attest",
        "result.items[].revision",
        "latest_workspace_sequence",
    ] {
        assert!(text.contains(phrase), "missing {phrase}: {text}");
    }
    assert_eq!(scoped_runtime_observe(&fixture)["result"], before["result"]);
    for (topic, phrases) in [
        (
            "session.observe",
            &[
                "current scoped cursor",
                "executable templates",
                "history queries",
                "does not execute or attest",
            ][..],
        ),
        (
            "item.record_many",
            &[
                "podway.scoped-item-record-many-input/v1",
                "input basis, executor, outcome and output digest",
                "result.items[].revision",
            ][..],
        ),
        ("item.set", &["--if-cursor", "result.items[].revision"][..]),
        (
            "workspace.init",
            &["initialization marker", "latest_workspace_sequence"][..],
        ),
        (
            "session.archive_list",
            &["inactive sessions", "latest_workspace_sequence"][..],
        ),
    ] {
        let help = fixture.run(&["help", topic]);
        let text = help["result"]["text"].as_str().unwrap();
        for phrase in phrases {
            assert!(text.contains(phrase), "{topic}: missing {phrase}");
        }
    }
}

#[test]
fn generated_item_recipes_preserve_all_types_clearing_and_attachment_evidence() {
    use serde_json::json;
    let base = scoped_runtime_base(
        json!({
            "capture":{
                "type":"action","title":"Capture","intent":"Record representative item types.","items":[
                    {"id":"confirmed","type":"confirm","prompt":"Confirm capture.","required":true},
                    {"id":"summary","type":"text","prompt":"Summarize capture.","required":true},
                    {"id":"outcome","type":"choice","prompt":"Choose an outcome.","required":true,
                        "choices":["accepted","rejected"]},
                    {"id":"count","type":"integer","prompt":"Record the count.","required":true,"minimum":0,"maximum":10},
                    {"id":"findings","type":"list","prompt":"Record findings.","required":true,
                        "min_items":1,"max_items":3,"unique":true},
                    {"id":"report","type":"artifact","prompt":"Attach the report.","required":true,
                        "allowed_media_types":["text/plain"]},
                    {"id":"internal-note","type":"text","prompt":"Record an unselected note.","required":false}
                ]
            },
            "consume":{
                "type":"action","title":"Consume","intent":"Use selected current evidence.","items":[
                    {"id":"consumed","type":"confirm","prompt":"Confirm consumption.","required":true}
                ]
            }
        }),
        vec![
            json!({"id":"capture-results","use":"capture","next":"consume-results"}),
            json!({"id":"consume-results","use":"consume","next":"plan",
            "evidence_from":[{"scope":"local","node":"capture-results","required":true,
                "items":["summary","findings","report"]}]}),
        ],
        false,
    );
    let fixture = scoped_runtime_start(&base);
    let attachment = fixture.0.join("evidence.txt");
    let contents = "Retained attachment evidence.";
    fs::write(&attachment, contents).unwrap();
    let capture = scoped_runtime_observe(&fixture)["result"]["cursor"]["attempt"].clone();

    scoped_runtime_recipe(&fixture, "item.check", Some("confirmed"), &[], "confirmed");
    for (item, value) in [
        ("summary", "Focused checks passed."),
        ("outcome", "accepted"),
        ("count", "7"),
    ] {
        scoped_runtime_recipe(
            &fixture,
            "item.set",
            Some(item),
            &[
                ("<value>", value),
                ("<text>", value),
                ("<choice>", value),
                ("<integer>", value),
            ],
            item,
        );
    }
    scoped_runtime_recipe(
        &fixture,
        "item.add",
        Some("findings"),
        &[("<value>", "first finding")],
        "add",
    );
    scoped_runtime_recipe(
        &fixture,
        "item.attach",
        Some("report"),
        &[("<path>", "evidence.txt")],
        "attach",
    );
    for (item, expected) in [
        ("confirmed", json!({"kind":"confirm"})),
        ("summary", json!("Focused checks passed.")),
        ("outcome", json!({"kind":"choice","value":"accepted"})),
        ("count", json!({"kind":"integer","value":7})),
        ("findings", json!(["first finding"])),
    ] {
        assert_eq!(
            scoped_runtime_history_item(&fixture, &capture, item)["data"],
            expected,
            "{item}"
        );
    }
    scoped_runtime_recipe(&fixture, "item.uncheck", Some("confirmed"), &[], "uncheck");
    assert!(scoped_runtime_history_item(&fixture, &capture, "confirmed")["data"].is_null());
    scoped_runtime_recipe(&fixture, "item.check", Some("confirmed"), &[], "recheck");
    scoped_runtime_recipe(
        &fixture,
        "item.remove",
        Some("findings"),
        &[("<value>", "first finding")],
        "remove",
    );
    scoped_runtime_recipe(
        &fixture,
        "item.add",
        Some("findings"),
        &[("<value>", "retained")],
        "readd",
    );
    assert_eq!(
        scoped_runtime_history_item(&fixture, &capture, "findings")["data"],
        json!(["retained"])
    );
    scoped_runtime_recipe(&fixture, "item.clear", Some("summary"), &[], "clear");
    assert!(scoped_runtime_history_item(&fixture, &capture, "summary")["data"].is_null());
    scoped_runtime_recipe(
        &fixture,
        "item.set",
        Some("summary"),
        &[
            ("<value>", "Restored summary."),
            ("<text>", "Restored summary."),
        ],
        "restore",
    );
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "capture-complete");

    let evidence = || {
        fixture.run(&[
            "evidence",
            "read",
            "--source",
            "capture-results",
            "--item",
            "report",
        ])
    };
    let present = evidence();
    assert_eq!(present["result"]["detail"]["item"], "report");
    assert_eq!(present["result"]["detail"]["source_attempt"], capture);
    assert_eq!(
        present["result"]["detail"]["artifact_availability"],
        "present"
    );
    assert_eq!(present["result"]["data"]["location"], "evidence.txt");
    fs::remove_file(&attachment).unwrap();
    let missing = evidence();
    assert_eq!(
        missing["result"]["detail"]["artifact_availability"],
        "missing"
    );
    assert_eq!(missing["result"]["data"], present["result"]["data"]);
    fs::write(&attachment, contents).unwrap();
    scoped_runtime_recipe(&fixture, "item.check", Some("consumed"), &[], "consume");
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "consume-complete");
    scoped_runtime_activate(&fixture, false);
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "return");
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "finish-complete");
    assert_eq!(fixture.run(&["status"])["result"]["lifecycle"], "completed");
}

#[test]
fn generated_cursor_and_goal_recipes_require_only_declared_placeholder_values() {
    use serde_json::json;
    let retry = scoped_runtime_base(
        json!({
            "work":{"type":"action","title":"Work","intent":"Record current evidence.",
                "items":[scoped_runtime_source("result",200)]},
            "review":{"type":"decision","title":"Review","objective":"Accept current evidence.",
                "prompt":"Is the work sufficient?","options":[{"id":"accept","label":"Accept"}],
                "reason":{"required":true}}
        }),
        vec![
            json!({"id":"work","use":"work","next":"review"}),
            json!({"id":"review","use":"review","routes":{"accept":{"effect":"advance","to":"plan"}},
            "evidence_from":[{"scope":"local","node":"work","required":true,"items":["result"]}]}),
        ],
        false,
    );
    let fixture = scoped_runtime_start(&retry);
    scoped_runtime_recipe(
        &fixture,
        "session.retry",
        None,
        &[("<reason>", "Retry the retained work.")],
        "retry",
    );
    scoped_runtime_recipe(
        &fixture,
        "item.set",
        Some("result"),
        &[
            ("<value>", "Current evidence."),
            ("<text>", "Current evidence."),
        ],
        "result",
    );
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "work-complete");
    scoped_runtime_recipe(
        &fixture,
        "session.decide",
        None,
        &[
            ("<option-id>", "accept"),
            ("<reason>", "The evidence supports acceptance."),
        ],
        "accept",
    );
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"]["cursor"]["attempt"]["node"],
        "plan"
    );

    let skip = scoped_runtime_base(
        json!({
            "prepare":{"type":"action","title":"Prepare","intent":"Start the optional path."},
            "source":{"type":"action","title":"Source","intent":"Record optional source evidence."},
            "consumer":{"type":"action","title":"Consumer","intent":"Use available source evidence."}
        }),
        vec![
            json!({"id":"prepare","use":"prepare","next":"source"}),
            json!({"id":"source","use":"source","next":"consumer","skip":{"allowed":true,"reason_required":true}}),
            json!({"id":"consumer","use":"consumer","next":"plan","skip":{"allowed":true,"reason_required":false},
            "evidence_from":[{"scope":"local","node":"source","required":false}]}),
        ],
        false,
    );
    let fixture = scoped_runtime_start(&skip);
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "prepare-complete");
    scoped_runtime_recipe(
        &fixture,
        "session.skip",
        None,
        &[("<reason>", "The source is inapplicable.")],
        "source-skip",
    );
    scoped_runtime_recipe(&fixture, "session.skip", None, &[], "consumer-skip");
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"]["cursor"]["attempt"]["node"],
        "plan"
    );

    let goal = scoped_runtime_base(json!({}), vec![], true);
    let fixture = scoped_runtime_start(&goal);
    scoped_runtime_recipe(
        &fixture,
        "goal.define",
        None,
        &[
            ("<goal>", "Verify the retained result."),
            (
                "<criterion-id>=<statement>",
                "tested=The retained checks pass.",
            ),
        ],
        "define",
    );
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"]["goal"]["criteria"][0]["id"],
        "tested"
    );
    scoped_runtime_activate(&fixture, true);
    scoped_runtime_recipe(
        &fixture,
        "item.set",
        Some("result"),
        &[
            ("<value>", "The checks passed."),
            ("<text>", "The checks passed."),
        ],
        "result",
    );
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "return");
    scoped_runtime_recipe(
        &fixture,
        "goal.assess_criterion",
        None,
        &[
            ("<criterion-id>", "tested"),
            ("<status>", "satisfied"),
            ("<reason>", "The retained check passed."),
        ],
        "assess",
    );
    let assessed = scoped_runtime_observe(&fixture);
    assert_eq!(
        assessed["result"]["goal"]["criteria"][0]["status"],
        "satisfied"
    );
    assert_eq!(assessed["result"]["goal"]["outcome"], "achieved");
    scoped_runtime_recipe(
        &fixture,
        "session.decide",
        None,
        &[
            ("<option-id>", "achieved"),
            ("<reason>", "The criterion is satisfied."),
        ],
        "decide",
    );
    scoped_runtime_recipe(&fixture, "session.complete", None, &[], "finish-complete");
    assert_eq!(fixture.run(&["status"])["result"]["lifecycle"], "completed");
}

#[test]
fn evidence_continuations_preserve_unicode_pages_and_rework_invalidates_old_tokens() {
    use serde_json::json;
    let mut base = scoped_runtime_base(
        json!({
            "work":{"type":"action","title":"Work","intent":"Record wide current evidence.",
                "items":[scoped_runtime_source("result",65536)]},
            "review":{"type":"decision","title":"Review","objective":"Check current evidence.",
                "prompt":"Is the result current?","options":[{"id":"accept","label":"Accept"}],
                "reason":{"required":true}}
        }),
        vec![
            json!({"id":"work","use":"work","next":"review"}),
            json!({"id":"review","use":"review","routes":{"accept":{"effect":"advance","to":"plan"}},
            "evidence_from":[{"scope":"local","node":"work","required":true,"items":["result"]}]}),
        ],
        false,
    );
    base["manual_rework"] = json!({"allowed_targets":["work"]});
    let read = ["evidence", "read", "--source", "work", "--item", "result"];
    let wide = "🦦".repeat(65_536);
    for explicit in [false, true] {
        let fixture = scoped_runtime_start(&base);
        let set = fixture.output_with_stdin(
            &["set", "result", "--stdin", "--idempotency-key", "wide"],
            wide.as_bytes(),
        );
        assert!(set.status.success(), "{set:?}");
        fixture.run(&["complete", "--idempotency-key", "work-complete"]);
        let before = scoped_runtime_observe(&fixture);
        let cursor = &before["result"]["cursor"];
        let consumer = &cursor["attempt"];
        let consumer_id = consumer["attempt"].as_str().unwrap();
        let session = cursor["session"].as_str().unwrap();
        let workspace = before["workspace"]["uuid"].as_str().unwrap();

        let mut initial = read.to_vec();
        initial.extend([
            "--if-workspace-uuid",
            workspace,
            "--if-session-id",
            session,
            "--if-attempt",
            consumer_id,
        ]);
        let first = fixture.run(&initial);
        let page = &first["result"];
        assert_eq!(page["schema"], "podway.scoped-evidence-read-result/v1");
        assert_eq!(page["detail"]["consumer"], *consumer);
        assert_eq!(page["unit"], "unicode_scalar");
        assert_eq!(page["total"], 65_536);
        assert_eq!(page["offset"], 0);
        assert_eq!(page["size"], 65_535);
        assert_eq!(page["truncated"], true);
        assert!(serde_json::to_vec(page.get("data").unwrap()).unwrap().len() <= 256 * 1024);
        assert!(serde_json::to_vec(&first).unwrap().len() <= 320 * 1024);
        let token = page["next_page_token"].as_str().unwrap();
        let mut continuation = read.to_vec();
        continuation.extend(["--page-token", token, "--if-attempt", consumer_id]);
        let next = fixture.run(&continuation);
        assert_eq!(next["result"]["offset"], 65_535);
        assert_eq!(next["result"]["size"], 1);
        assert_eq!(next["result"]["truncated"], false);
        assert!(next["result"]["next_page_token"].is_null());
        assert_eq!(
            format!(
                "{}{}",
                page["data"].as_str().unwrap(),
                next["result"]["data"].as_str().unwrap()
            ),
            wide
        );
        assert_eq!(scoped_runtime_observe(&fixture)["result"], before["result"]);

        let mut invalid = read.to_vec();
        invalid.extend(["--if-attempt", consumer_id, "--if-session-revision", "0"]);
        scoped_runtime_error(fixture.output(&invalid), 2, "REQUEST_INVALID");
        let mut rework = vec![
            "rework",
            "--to",
            "work",
            "--reason",
            "Refresh the evidence.",
            "--actor",
            "owner",
            "--idempotency-key",
            "manual-rework",
        ];
        let cursor_argument = cursor.to_string();
        let fences = [
            "--if-workspace-uuid",
            workspace,
            "--if-cursor",
            &cursor_argument,
        ];
        if explicit {
            rework.extend(fences);
        }
        let original = fixture.output(&rework);
        assert!(original.status.success(), "{original:?}");
        if !explicit {
            rework.extend(fences);
        }
        assert_eq!(fixture.output(&rework).stdout, original.stdout);
        let history = fixture.run(&["status", "--verbose"]);
        let abandoned = history["result"]["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attempt| attempt["attempt"] == *consumer)
            .unwrap();
        assert_eq!(abandoned["lifecycle"], "abandoned");
        assert_eq!(abandoned["validity"], "stale");
        assert_eq!(abandoned["actor"], "owner");
        assert_eq!(abandoned["reason"], "Refresh the evidence.");

        fixture.run(&[
            "set",
            "result",
            "Replacement evidence.",
            "--idempotency-key",
            "replacement",
        ]);
        fixture.run(&["complete", "--idempotency-key", "replacement-complete"]);
        let after = scoped_runtime_observe(&fixture);
        let mut previous = read.to_vec();
        previous.extend(["--if-session-id", session, "--if-attempt", consumer_id]);
        let stale = scoped_runtime_error(fixture.output(&previous), 4, "ATTEMPT_NOT_CURRENT");
        assert_eq!(stale["details"]["expected_attempt_id"], consumer_id);
        assert_eq!(
            stale["details"]["actual_attempt_id"],
            after["result"]["cursor"]["attempt"]["attempt"]
        );
        let mut previous_page = read.to_vec();
        previous_page.extend(["--page-token", token]);
        let stale = scoped_runtime_error(
            fixture.output(&previous_page),
            4,
            "HISTORY_PAGE_TOKEN_STALE",
        );
        assert_eq!(stale["retryable"], true);
        assert_eq!(scoped_runtime_observe(&fixture)["result"], after["result"]);
    }
}

#[test]
fn running_start_policies_preserve_or_delete_the_prior_session_with_partial_fences() {
    for preserve in [false, true] {
        for partial in [false, true] {
            let fixture =
                scoped_runtime_start(&scoped_runtime_base(serde_json::json!({}), vec![], false));
            let before = fixture.run(&["observe"]);
            let session = before["result"]["cursor"]["session"].as_str().unwrap();
            let undecided = fixture.output(&[
                "start",
                "--preset",
                "small-change-v3",
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
                "small-change-v3",
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
            let old_revision = before["result"]["cursor"]["session_revision"]
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
                assert_eq!(
                    archived["result"]["observation"]["lifecycle_guidance"]["activity"],
                    "archived"
                );
                assert_eq!(archived["result"]["observation"]["lifecycle"], "cancelled");
                assert_eq!(archived["result"]["disposition"]["kind"], "superseded");
                assert_eq!(archived["result"]["disposition"]["actor"], "owner");
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
            "small-change-v3",
            "--task",
            "original",
            "--idempotency-key",
            "start",
        ]);
        let before = fixture.run(&["status"]);
        let session = before["result"]["cursor"]["session"].as_str().unwrap();
        let prefix = [
            "start",
            "--preset",
            "small-change-v3",
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
        let revision = before["result"]["cursor"]["session_revision"]
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
    let fixture = guide008_terminal_fixture();
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["cursor"]["session"].as_str().unwrap();
    let disposed = scoped_runtime_transition(
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
            "small-change-v3",
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
        "small-change-v3",
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
    assert_eq!(
        list["result"]["sessions"][0]["archived_workspace_revision"],
        successor["workspace"]["latest_workspace_sequence"]
    );
    assert_eq!(
        list["result"]["sessions"][0]["session_revision"],
        before["result"]["cursor"]["session_revision"]
    );
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
    let after = fixture.run(&["status"]);
    assert_eq!(after["result"]["cursor"], active["result"]["cursor"]);
    assert_eq!(after["session"], active["session"]);
    assert_eq!(
        after["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        active["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );
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
    let fixture = scoped_runtime_start(&scoped_runtime_base(serde_json::json!({}), vec![], false));
    fixture.run(&[
        "cancel",
        "--reason",
        "Retain fixture records.",
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
    let before = scoped_runtime_observe(&fixture);
    let cursor = &before["result"]["cursor"];
    assert!(cursor["attempt"].is_null());
    let session = cursor["session"].as_str().unwrap();
    let revision = cursor["session_revision"].to_string();
    let workspace = before["workspace"]["uuid"].as_str().unwrap();

    let original = fixture.output(&["archive", "--idempotency-key", "archive"]);
    assert!(original.status.success(), "{original:?}");
    let archived = serde_json::from_slice::<OutputEnvelopeV4>(&original.stdout)
        .unwrap()
        .into_value();
    assert_eq!(archived["result"]["activity"], "inactive");
    assert_eq!(archived["result"]["session_id"], session);
    assert_eq!(
        archived["result"]["session_revision"],
        cursor["session_revision"]
    );
    scoped_runtime_error(fixture.output(&["status"]), 1, "SESSION_NOT_FOUND");
    assert_eq!(fixture.run(&["archive", "list"])["result"]["count"], 1);
    assert_eq!(
        fixture.run(&["workspace", "show"])["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap(),
        before["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap()
            + 1
    );
    assert_eq!(
        fixture
            .output(&[
                "archive",
                "--idempotency-key",
                "archive",
                "--if-session-id",
                session,
                "--if-session-revision",
                &revision
            ])
            .stdout,
        original.stdout
    );
    assert_eq!(
        fixture
            .output(&[
                "archive",
                "--idempotency-key",
                "archive",
                "--if-session-id",
                session,
                "--if-session-revision",
                &revision,
                "--if-workspace-uuid",
                workspace
            ])
            .stdout,
        original.stdout
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "archive"])["result"]["outcome"]["response"],
        archived
    );
    let readback = fixture.run(&[
        "--contract-generation",
        "2",
        "archive",
        "show",
        "--verbose",
        "--session-id",
        session,
    ]);
    let archived_state = &readback["result"]["observation"];
    assert_eq!(
        readback["result"]["attempt_history"]["attempts"][0]["attempt"]["session"],
        session
    );
    let empty_page = fixture.run(&[
        "archive",
        "show",
        "--session-id",
        session,
        "--verbose",
        "--history-before",
        "1",
    ]);
    assert_eq!(
        empty_page["result"]["attempt_history"]["attempts"],
        serde_json::json!([])
    );
    assert_eq!(archived_state["lifecycle"], "cancelled");
    assert_eq!(archived_state["cursor"], *cursor);
    assert_eq!(archived_state["lifecycle_guidance"]["activity"], "archived");
    assert_eq!(archived_state["mutation_templates"], serde_json::json!([]));
}

#[test]
fn block_unblock_cancel_and_force_reset_preserve_automatic_and_explicit_fences_and_rendering() {
    use serde_json::json;
    for explicit in [false, true] {
        for json in [false, true] {
            let base = scoped_runtime_base(
                json!({
                    "prepare":{"type":"action","title":"Prepare","intent":"Record readiness.","items":[
                        {"id":"ready","type":"confirm","prompt":"Confirm readiness.","required":true}
                    ]}
                }),
                vec![json!({"id":"prepare","use":"prepare","next":"plan"})],
                false,
            );
            let fixture = scoped_runtime_start(&base);
            fixture.run(&["check", "ready", "--idempotency-key", "ready"]);
            let initial = scoped_runtime_observe(&fixture);
            let attempt = &initial["result"]["cursor"]["attempt"];
            assert_eq!(initial["result"]["guidance"]["ready"], true);
            let block = scoped_runtime_transition(
                &fixture,
                &["block", "--reason", "waiting for an external decision"],
                "block",
                true,
                explicit,
                json,
            );
            assert_eq!(block["command"], "session.block");
            let status = scoped_runtime_observe(&fixture);
            assert_eq!(status["result"]["guidance"]["items"][0]["satisfied"], true);
            assert_eq!(status["result"]["guidance"]["ready"], false);
            let blocker = status["result"]["open_blockers"][0].as_str().unwrap();
            let history = || {
                fixture.run(&[
                    "history",
                    "blockers",
                    "--session",
                    attempt["session"].as_str().unwrap(),
                    "--attempt",
                    &attempt.to_string(),
                ])
            };
            let open = history();
            assert_eq!(
                open["result"]["rows"][0]["reason"],
                "waiting for an external decision"
            );
            assert_eq!(open["result"]["rows"][0]["state"], "open");
            let unblocked = scoped_runtime_transition(
                &fixture,
                &["unblock", blocker],
                "unblock-one",
                true,
                explicit,
                json,
            );
            assert_eq!(unblocked["command"], "session.unblock");
            assert_eq!(history()["result"]["rows"][0]["state"], "resolved");
            assert_eq!(
                scoped_runtime_observe(&fixture)["result"]["guidance"]["ready"],
                true
            );

            scoped_runtime_transition(
                &fixture,
                &["block", "--reason", "waiting again"],
                "block-again",
                true,
                explicit,
                json,
            );
            scoped_runtime_transition(
                &fixture,
                &["unblock", "--all"],
                "unblock-all",
                true,
                explicit,
                json,
            );
            assert_eq!(
                scoped_runtime_observe(&fixture)["result"]["open_blockers"],
                json!([])
            );
            let cancelled = scoped_runtime_transition(
                &fixture,
                &["cancel", "--reason", "the task is no longer needed"],
                "cancel",
                true,
                explicit,
                json,
            );
            assert_eq!(cancelled["result"]["lifecycle"], "cancelled");
            let terminal = scoped_runtime_observe(&fixture);
            assert_eq!(terminal["result"]["lifecycle"], "cancelled");
            assert!(terminal["result"]["cursor"]["attempt"].is_null());
            let history = fixture.run(&["status", "--verbose"]);
            let last = &history["result"]["attempts"][0];
            assert_eq!(last["lifecycle"], "abandoned");
            assert_eq!(last["reason"], "the task is no longer needed");
            let reset = scoped_runtime_transition(
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
            assert_eq!(
                reset["result"]["schema"],
                "podway.successor-session-reset-result/v1"
            );
            assert_eq!(reset["result"]["reset"], true);
            assert_eq!(reset["result"]["mode"], "force");
            scoped_runtime_error(fixture.output(&["status"]), 1, "SESSION_NOT_FOUND");
        }
    }
}

#[test]
fn retry_and_skip_preserve_clean_attempts_reasons_and_exact_replay_in_json_and_text() {
    for explicit in [false, true] {
        for json in [false, true] {
            let fixture = scoped_runtime_retry_fixture();
            fixture.run(&[
                "set",
                "result",
                "Keep in history only.",
                "--idempotency-key",
                "seed",
            ]);
            let before = scoped_runtime_observe(&fixture);
            let old = &before["result"]["cursor"]["attempt"];
            let retry = scoped_runtime_transition(
                &fixture,
                &["retry", "--reason", "Repeat with a clean attempt."],
                "retry",
                true,
                explicit,
                json,
            );
            assert_eq!(retry["command"], "session.retry");
            assert_ne!(retry["result"]["cursor"]["attempt"], *old);
            assert_eq!(retry["result"]["cursor"]["attempt"]["node"], "work");
            let current = scoped_runtime_observe(&fixture);
            assert_eq!(current["result"]["items"][0]["recorded"], false);
            let history = fixture.run(&["status", "--verbose"]);
            let prior = history["result"]["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|attempt| attempt["attempt"] == *old)
                .unwrap();
            assert_eq!(prior["lifecycle"], "abandoned");
            assert_eq!(prior["validity"], "stale");
            assert_eq!(prior["reason"], "Repeat with a clean attempt.");
            assert_eq!(
                scoped_runtime_history_item(&fixture, old, "result")["data"],
                "Keep in history only."
            );

            fixture.run(&[
                "set",
                "result",
                "Fresh source evidence.",
                "--idempotency-key",
                "seed-again",
            ]);
            let complete = scoped_runtime_transition(
                &fixture,
                &["complete"],
                "complete",
                true,
                explicit,
                json,
            );
            let review = &complete["result"]["cursor"]["attempt"];
            assert_eq!(review["node"], "review");
            let retry = scoped_runtime_transition(
                &fixture,
                &["retry", "--reason", "Repeat the decision."],
                "decision-retry",
                true,
                explicit,
                json,
            );
            assert_eq!(retry["result"]["cursor"]["attempt"]["node"], "review");
            assert_ne!(retry["result"]["cursor"]["attempt"], *review);

            let fixture = scoped_runtime_skip_fixture();
            reject(
                &fixture,
                &["skip", "--reason", "Not eligible."],
                "rejected-skip",
                "STAGE_NOT_SKIPPABLE",
            );
            scoped_runtime_transition(&fixture, &["complete"], "prepare", true, explicit, json);
            fixture.run(&[
                "set",
                "required-note",
                "Keep for retrospective reads.",
                "--idempotency-key",
                "skip-seed",
            ]);
            let attempt = scoped_runtime_observe(&fixture)["result"]["cursor"]["attempt"].clone();
            let skip = scoped_runtime_transition(
                &fixture,
                &["skip", "--reason", "Not applicable."],
                "skip-source",
                true,
                explicit,
                json,
            );
            assert_eq!(skip["command"], "session.skip");
            assert_eq!(skip["result"]["cursor"]["attempt"]["node"], "consumer");
            let history = fixture.run(&["status", "--verbose"]);
            let skipped = history["result"]["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["attempt"] == attempt)
                .unwrap();
            assert_eq!(skipped["lifecycle"], "skipped");
            assert_eq!(skipped["reason"], "Not applicable.");
            scoped_runtime_transition(&fixture, &["skip"], "skip-consumer", true, explicit, json);
            let history = fixture.run(&["status", "--verbose"]);
            assert_eq!(history["result"]["attempts"][1]["lifecycle"], "skipped");
            assert!(history["result"]["attempts"][1]["reason"].is_null());
            scoped_runtime_activate(&fixture, false);
            scoped_runtime_transition(&fixture, &["complete"], "return", true, explicit, json);
            let terminal =
                scoped_runtime_transition(&fixture, &["complete"], "finish", true, explicit, json);
            assert_eq!(terminal["result"]["lifecycle"], "completed");
            assert!(scoped_runtime_observe(&fixture)["result"]["cursor"]["attempt"].is_null());
        }
    }
}

#[test]
fn goal_definition_preserves_actor_and_stable_json_human_and_quiet_rendering() {
    let base = scoped_runtime_base(serde_json::json!({}), vec![], true);
    for mode in ["json", "human", "quiet"] {
        let fixture = scoped_runtime_start(&base);
        let mut args = vec![
            "goal",
            "define",
            "--goal",
            "Ship the scoped CLI.",
            "--criterion",
            "stable-json=JSON is stable.",
            "--actor",
            "reviewer",
            "--idempotency-key",
            "define",
        ];
        match mode {
            "json" => args.push("--json"),
            "quiet" => args.push("--quiet"),
            _ => {}
        }
        let output = fixture.raw_output(&args);
        assert!(output.status.success(), "{mode}: {output:?}");
        assert!(output.stderr.is_empty());
        match mode {
            "json" => {
                let envelope = serde_json::from_slice::<OutputEnvelopeV4>(&output.stdout)
                    .unwrap()
                    .into_value();
                assert_eq!(envelope["command"], "goal.define");
                assert_eq!(
                    envelope["result"]["schema"],
                    "podway.successor-cursor-mutation-result/v1"
                );
                assert_eq!(envelope["result"]["cursor"]["goal_revision"], 1);
            }
            "human" => {
                let text = String::from_utf8(output.stdout).unwrap();
                assert!(text.contains("Ship the scoped CLI."), "{text}");
                assert!(text.contains("stable-json"), "{text}");
            }
            "quiet" => assert!(output.stdout.is_empty()),
            _ => unreachable!(),
        }
        let original = fixture.run(&["outcome", "lookup", "--idempotency-key", "define"])["result"]
            ["outcome"]["response"]
            .clone();
        let observed = scoped_runtime_observe(&fixture);
        assert_eq!(observed["result"]["goal"]["actor"], "reviewer");
        assert_eq!(original["result"]["goal"]["actor"], "reviewer");
    }
}

#[test]
fn general_decisions_evidence_queries_and_verbose_history_preserve_fences_and_actor() {
    for explicit in [false, true] {
        let fixture = scoped_runtime_retry_fixture();
        let mut decisions = Vec::new();
        for round in 0..2 {
            fixture.run(&[
                "set",
                "result",
                "Retained source evidence.",
                "--idempotency-key",
                &format!("source-{round}"),
            ]);
            scoped_runtime_transition(
                &fixture,
                &["complete"],
                &format!("complete-{round}"),
                true,
                explicit,
                true,
            );
            let before = scoped_runtime_observe(&fixture);
            let cursor = &before["result"]["cursor"];
            let session = cursor["session"].as_str().unwrap();
            let selected = fixture.run(&[
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
            assert_eq!(selected["command"], "evidence.read");
            assert_eq!(
                selected["result"]["schema"],
                "podway.scoped-evidence-read-result/v1"
            );
            assert_eq!(selected["result"]["detail"]["item"], "result");
            assert_eq!(
                selected["result"]["detail"]["source_attempt"]["node"],
                "work"
            );
            assert_eq!(selected["result"]["detail"]["consumer"], cursor["attempt"]);
            assert_eq!(selected["result"]["data"], "Retained source evidence.");
            assert_eq!(selected["result"]["truncated"], false);
            assert!(selected["result"]["next_page_token"].is_null());
            assert_eq!(selected["workspace"], before["workspace"]);
            assert_eq!(scoped_runtime_observe(&fixture)["result"], before["result"]);
            let decision = scoped_runtime_transition(
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
                &format!("reject-{round}"),
                true,
                explicit,
                true,
            );
            assert_eq!(decision["command"], "session.decide");
            assert_eq!(decision["result"]["cursor"]["attempt"]["node"], "work");
            assert_ne!(decision["result"]["cursor"]["attempt"], cursor["attempt"]);
            decisions.push(cursor["attempt"].clone());
        }
        let before = fixture.run(&["status"]);
        let session = before["result"]["cursor"]["session"].as_str().unwrap();
        let argv = [
            "status",
            "--verbose",
            "--history-before",
            "42",
            "--if-session-id",
            session,
        ];
        let verbose = fixture.run(&argv);
        let rows = verbose["result"]["attempts"].as_array().unwrap();
        assert!(
            rows.windows(2)
                .all(|pair| pair[0]["trace_sequence"].as_u64().unwrap()
                    > pair[1]["trace_sequence"].as_u64().unwrap())
        );
        assert!(
            rows.iter()
                .all(|row| row["trace_sequence"].as_u64().unwrap() < 42)
        );
        let recorded = rows
            .iter()
            .filter(|row| row["decision"].is_object())
            .collect::<Vec<_>>();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0]["attempt"], decisions[1]);
        assert_eq!(recorded[1]["attempt"], decisions[0]);
        for row in &recorded {
            assert_eq!(row["decision"]["option"], "reject");
            assert_eq!(row["decision"]["reason"], "Review found a gap.");
            assert_eq!(row["actor"], "owner");
            assert!(row["decision"]["goal_assessment"].is_null());
        }
        let human = fixture.raw_output(&argv);
        assert!(human.status.success(), "{human:?}");
        let text = String::from_utf8(human.stdout).unwrap();
        let body: Value = serde_json::from_str(text.split_once("result: ").unwrap().1).unwrap();
        assert_eq!(body["attempts"], verbose["result"]["attempts"]);
        let cutoff = recorded[0]["trace_sequence"].to_string();
        let page = fixture.run(&["status", "--verbose", "--history-before", &cutoff]);
        let earlier = page["result"]["attempts"].as_array().unwrap();
        assert_eq!(
            earlier
                .iter()
                .filter(|row| row["decision"].is_object())
                .count(),
            1
        );
        assert!(earlier.iter().any(|row| row["attempt"] == decisions[0]));
        scoped_runtime_error(
            fixture.output(&["status", "--verbose", "--history-before", "0"]),
            2,
            "REQUEST_INVALID",
        );
        assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    }
}

#[test]
fn bound005_stdin_identity_and_flag_errors_keep_their_priority() {
    use serde_json::json;
    let fixture = scoped_runtime_retry_fixture();
    let before = scoped_runtime_observe(&fixture);
    let snapshot = fixture.podway_snapshot();
    for (input, message) in [
        (&[0xff][..], "stdin text must be valid UTF-8"),
        (
            &b"valid text"[..],
            "--if-session-revision does not apply to item commands",
        ),
    ] {
        let output = fixture.output_with_stdin(
            &["set", "!!!", "--stdin", "--if-session-revision", "1"],
            input,
        );
        assert!(output.stderr.is_empty());
        let error = scoped_runtime_error(output, 2, "REQUEST_INVALID");
        assert_eq!(error["message"], message);
        assert_eq!(error["command"], "item.set");
        fixture.assert_podway_snapshot(&snapshot);
    }
    let template = scoped_runtime_template(&before, "item.record_many");
    let mut input: Value =
        serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
    input["context"]["idempotency_key"] = json!("stdin-owned");
    input["operations"] = json!([{"item_id":"result","expected_item_revision":0,
        "record":{"type":"text","value":"record"}}]);
    let output = fixture.output_with_stdin(
        &[
            "record",
            "--stdin",
            "--idempotency-key",
            "duplicate-key",
            "--yes",
        ],
        input.to_string().as_bytes(),
    );
    assert!(output.stderr.is_empty());
    let error = scoped_runtime_error(output, 2, "REQUEST_INVALID");
    assert!(
        error["message"].as_str().unwrap().contains("stdin"),
        "{error}"
    );
    assert_eq!(error["command"], "item.record_many");
    fixture.assert_podway_snapshot(&snapshot);
}

#[test]
fn bound005_fresh_and_stateful_previews_keep_distinct_admission() {
    let fixture = scoped_runtime_start(&scoped_runtime_base(serde_json::json!({}), vec![], false));
    let status = fixture.run(&["status"]);
    let revision = status["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
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
    let fresh = fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Preview",
        "--dry-run",
        "--timeout",
        "0ms",
    ]);
    assert_eq!(fresh["command"], "session.start");
    assert_eq!(fresh["result"]["dry_run"], true);
    let output = fixture.output(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "Preview",
        "--dry-run",
        "--on-existing",
        "delete",
        "--progress-summary",
        "Preview only.",
        "--timeout",
        "0ms",
        "--if-workspace-uuid",
        status["workspace"]["uuid"].as_str().unwrap(),
        "--if-session-id",
        status["result"]["cursor"]["session"].as_str().unwrap(),
        "--if-session-revision",
        &revision,
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "WORKSPACE_MAINTENANCE");
    assert_eq!(error["command"], "session.start_replace");
    assert_eq!(error["details"]["boundary"], "maintenance_lock");
    assert_eq!(error["details"]["timeout_ms"], 0);
    assert!(output.stderr.is_empty());
    drop(paused);
    fixture.assert_podway_snapshot(&before);
}

/// Complete a scoped fixture before recording a disposition.
fn guide008_terminal_fixture() -> Fixture {
    let fixture = scoped_runtime_start(&scoped_runtime_base(serde_json::json!({}), vec![], false));
    scoped_runtime_activate(&fixture, false);
    fixture.run(&["complete", "--idempotency-key", "guide008-work-complete"]);
    fixture.run(&["complete", "--idempotency-key", "guide008-finish-complete"]);
    fixture
}

/// Complete a scoped fixture and settle its disposition.
fn guide008_disposed_terminal_fixture() -> Fixture {
    let fixture = guide008_terminal_fixture();
    scoped_runtime_transition(
        &fixture,
        &[
            "disposition",
            "not-required",
            "--reason",
            "No external handoff is required.",
        ],
        "guide008-disposition",
        false,
        false,
        true,
    );
    fixture
}

fn guide008_fenced_start_argv<'a>(session: &'a str, revision: &'a str) -> Vec<&'a str> {
    vec![
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "successor",
        "--if-session-id",
        session,
        "--if-session-revision",
        revision,
    ]
}

#[test]
fn guide008_cli_fenced_plain_start_archives_disposed_terminal_with_replay_and_lookup() {
    let fixture = guide008_disposed_terminal_fixture();
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["cursor"]["session"].as_str().unwrap();
    let revision = terminal["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let mut argv = guide008_fenced_start_argv(session, &revision);
    argv.extend_from_slice(&["--idempotency-key", "guide008-start"]);
    let successor = fixture.run(&argv);
    assert_eq!(successor["command"], "session.start");
    assert_eq!(successor["result"]["existing_session"]["action"], "archive");
    assert_eq!(
        successor["result"]["existing_session"]["session_id"],
        session
    );
    assert_eq!(fixture.run(&argv), successor, "exact replay after archival");
    let lookup = fixture.run(&["outcome", "lookup", "--idempotency-key", "guide008-start"]);
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], successor);
    let list = fixture.run(&["archive", "list"]);
    assert_eq!(list["result"]["count"], 1);
    assert_eq!(list["result"]["sessions"][0]["session_id"], session);

    // Reusing the key with different fences fails as key reuse before any
    // live fence check, keeps the state, and the original still replays.
    let state_before_reuse = fixture.run(&["status"]);
    let stale_revision = (revision.parse::<u64>().unwrap() + 1).to_string();
    let mut reused = guide008_fenced_start_argv(session, &stale_revision);
    reused.extend_from_slice(&["--idempotency-key", "guide008-start"]);
    let output = fixture.output(&reused);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "IDEMPOTENCY_KEY_REUSED");
    assert_eq!(
        fixture.run(&["status"])["result"],
        state_before_reuse["result"]
    );
    assert_eq!(fixture.run(&argv), successor);
}

#[test]
fn guide008_cli_fenced_plain_start_refuses_nonterminal_states_without_deletion() {
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let seeded = fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "original",
        "--idempotency-key",
        "seed",
    ]);
    let session = seeded["session"]["id"].as_str().unwrap();
    let before = fixture.run(&["observe"]);
    let mut prepared = guide008_fenced_start_argv(session, "0");
    prepared.extend_from_slice(&["--idempotency-key", "guide008-prepared"]);
    let output = fixture.output(&prepared);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "SESSION_START_DECISION_REQUIRED");
    assert_eq!(error["details"]["lifecycle"], "prepared");
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);

    let fixture = scoped_runtime_start(&scoped_runtime_base(serde_json::json!({}), vec![], false));
    let status = fixture.run(&["status"]);
    let session = status["result"]["cursor"]["session"].as_str().unwrap();
    let revision = status["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let before = fixture.run(&["observe"]);
    let mut running = guide008_fenced_start_argv(session, &revision);
    running.extend_from_slice(&["--idempotency-key", "guide008-running"]);
    let output = fixture.output(&running);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "SESSION_START_DECISION_REQUIRED");
    assert_eq!(error["details"]["lifecycle"], "running");
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);

    let fixture = guide008_terminal_fixture();
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["cursor"]["session"].as_str().unwrap();
    let revision = terminal["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let before = fixture.run(&["observe"]);
    let mut terminal = guide008_fenced_start_argv(session, &revision);
    terminal.extend_from_slice(&["--idempotency-key", "guide008-terminal"]);
    let output = fixture.output(&terminal);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "SESSION_START_DECISION_REQUIRED");
    assert_eq!(error["details"]["current_terminal_disposition"], false);
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "guide008-terminal"
        ])["result"]["found"],
        false
    );
}

#[test]
fn guide008_cli_fenced_plain_start_rejects_partial_and_stale_fences() {
    let fixture = guide008_disposed_terminal_fixture();
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["cursor"]["session"].as_str().unwrap();
    let workspace = terminal["workspace"]["uuid"].as_str().unwrap();
    let revision = terminal["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let base = [
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "successor",
    ];

    let mut lone_id = base.to_vec();
    lone_id.extend_from_slice(&["--if-session-id", session]);
    let output = fixture.output(&lone_id);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "REQUEST_INVALID");
    assert_eq!(error["command"], "session.start");
    assert_eq!(
        error["message"],
        "plain start requires --if-session-id and --if-session-revision together"
    );
    reject(&fixture, &lone_id, "guide008-lone-id", "REQUEST_INVALID");
    let mut lone_revision = base.to_vec();
    lone_revision.extend_from_slice(&["--if-session-revision", &revision]);
    reject(
        &fixture,
        &lone_revision,
        "guide008-lone-revision",
        "REQUEST_INVALID",
    );
    let mut attempt = base.to_vec();
    attempt.extend_from_slice(&[
        "--if-session-id",
        session,
        "--if-session-revision",
        &revision,
        "--if-attempt",
        "00000000-0000-4000-8000-0000000000e8",
    ]);
    reject(&fixture, &attempt, "guide008-attempt", "REQUEST_INVALID");

    let stale_id = guide008_fenced_start_argv(workspace, &revision);
    reject(
        &fixture,
        &stale_id,
        "guide008-stale-id",
        "SESSION_ID_MISMATCH",
    );
    let stale_revision = (revision.parse::<u64>().unwrap() + 1).to_string();
    let stale_revision = guide008_fenced_start_argv(session, &stale_revision);
    reject(
        &fixture,
        &stale_revision,
        "guide008-stale-revision",
        "SESSION_REVISION_CONFLICT",
    );
}

#[test]
fn guide008_cli_fenced_plain_start_dry_run_never_takes_the_offline_shortcut() {
    let fixture = guide008_disposed_terminal_fixture();
    let before = fixture.run(&["observe"]);
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["cursor"]["session"].as_str().unwrap();
    let revision = terminal["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let mut preview = guide008_fenced_start_argv(session, &revision);
    preview.push("--dry-run");
    let previewed = fixture.run(&preview);
    assert_eq!(previewed["command"], "session.start");
    assert_eq!(previewed["result"]["dry_run"], true);
    assert_eq!(previewed["result"]["existing_session"]["action"], "archive");
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);

    let stale_revision = (revision.parse::<u64>().unwrap() + 1).to_string();
    let mut stale = guide008_fenced_start_argv(session, &stale_revision);
    stale.push("--dry-run");
    let output = fixture.output(&stale);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let error = serde_json::from_slice::<ErrorEnvelopeV2>(&output.stdout)
        .unwrap()
        .into_value();
    assert_eq!(error["code"], "SESSION_REVISION_CONFLICT");
    assert_eq!(fixture.run(&["observe"])["result"], before["result"]);
}

#[test]
fn guide008_cli_fenced_start_from_a_terminal_never_interviews() {
    // Primary case: an undisposed terminal session, whose interview choices
    // disposition, archive and replace.
    let fixture = guide008_terminal_fixture();
    let terminal = fixture.run(&["status"]);
    let session = terminal["result"]["cursor"]["session"].as_str().unwrap();
    let revision = terminal["result"]["cursor"]["session_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let before = fixture.run(&["status"]);
    let output = fixture.fenced_start_in_pty(session, &revision, "guide008-terminal");
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        rendered.matches("podway-test-pty-exit:5").count(),
        1,
        "{rendered}"
    );
    assert!(
        rendered.contains("SESSION_START_DECISION_REQUIRED"),
        "{rendered}"
    );
    assert!(!rendered.contains("Choose ["), "{rendered}");
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "guide008-terminal"
        ])["result"]["found"],
        false
    );

    // Second case: a prepared session.
    let fixture = Fixture::new();
    fixture.run(&["init", "--idempotency-key", "init"]);
    let seeded = fixture.run(&[
        "start",
        "--preset",
        "small-change-v3",
        "--task",
        "original",
        "--idempotency-key",
        "seed",
    ]);
    let session = seeded["session"]["id"].as_str().unwrap();
    let before = fixture.run(&["status"]);
    let output = fixture.fenced_start_in_pty(session, "0", "guide008-prepared");
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        rendered.matches("podway-test-pty-exit:5").count(),
        1,
        "{rendered}"
    );
    assert!(
        rendered.contains("SESSION_START_DECISION_REQUIRED"),
        "{rendered}"
    );
    assert!(!rendered.contains("Choose ["), "{rendered}");
    assert_eq!(fixture.run(&["status"])["result"], before["result"]);
}

#[test]
fn agent003_goal_assess_criteria_partial_full_decide_and_if_goal_revision_flag_gate() {
    use serde_json::json;
    let mut base = scoped_runtime_base(
        json!({
            "inspect": {"type":"action","title":"Inspect","intent":"Inspect the input."},
            "triage": {"type":"action","title":"Triage","intent":"Triage before planning."},
        }),
        vec![
            json!({"id":"inspect","use":"inspect","next":"triage"}),
            json!({"id":"triage","use":"triage","next":"plan"}),
        ],
        true,
    );
    base["manual_rework"] = json!({"allowed_targets":["triage"]});
    let fixture = scoped_runtime_start(&base);
    fixture.run(&[
        "goal",
        "define",
        "--goal",
        "Verify the assessment flag gate.",
        "--criterion",
        "z-reproduced=The failure was reproduced.",
        "--criterion",
        "a-verified=The regression test passes.",
        "--actor",
        "agent-to",
        "--idempotency-key",
        "define-goal",
    ]);
    let revision_request = |target, expected, key| {
        [
            "goal",
            "revise",
            "--goal",
            "Verify the changed requirements.",
            "--criterion",
            "verified=The required behavior is verified.",
            "--rework-to",
            target,
            "--reason",
            "Use the current goal revision policy.",
            "--basis-description",
            "Owned fixture basis.",
            "--basis-digest",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "--feedback",
            "Retain the prior group result for later investigation.",
            "--if-goal-revision",
            expected,
            "--idempotency-key",
            key,
        ]
    };
    let pre_anchor = scoped_runtime_observe(&fixture);
    assert_eq!(pre_anchor["result"]["cursor"]["attempt"]["node"], "inspect");
    for (target, key, code) in [
        (
            "finish",
            "undeclared-revision-target",
            "GOAL_REVISION_TARGET_NOT_ALLOWED",
        ),
        (
            "triage",
            "unvisited-revision-target",
            "MANUAL_REWORK_TARGET_NOT_ON_TRACE",
        ),
    ] {
        let refusal =
            scoped_runtime_error(fixture.output(&revision_request(target, "1", key)), 1, code);
        assert_eq!(refusal["details"]["target_graph_node_id"], target);
        assert_eq!(
            scoped_runtime_observe(&fixture)["result"],
            pre_anchor["result"]
        );
        assert_eq!(
            fixture.run(&["outcome", "lookup", "--idempotency-key", key])["result"]["found"],
            false,
        );
    }
    fixture.run(&["complete", "--idempotency-key", "complete-inspect"]);
    fixture.run(&["complete", "--idempotency-key", "complete-triage"]);
    let defined = scoped_runtime_observe(&fixture);
    assert_eq!(defined["result"]["cursor"]["goal_revision"], 1);
    scoped_runtime_error(
        fixture.output(&[
            "complete",
            "--if-goal-revision",
            "1",
            "--idempotency-key",
            "bad-complete",
        ]),
        2,
        "REQUEST_INVALID",
    );
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"],
        defined["result"]
    );

    scoped_runtime_activate(&fixture, true);
    fixture.run(&[
        "set",
        "result",
        "The first execution supplied evidence.",
        "--idempotency-key",
        "first-result",
    ]);
    let first_terminal = scoped_runtime_observe(&fixture);
    let first_execution = first_terminal["result"]["selection"]["execution"]
        .as_str()
        .unwrap();
    let first_return = scoped_runtime_recipe(&fixture, "group.return", None, &[], "return-first");
    assert_eq!(first_return["result"]["execution_id"], first_execution);
    assert_eq!(
        first_return["result"]["closed_attempt"],
        first_terminal["result"]["cursor"]["attempt"]
    );
    let before_revision = scoped_runtime_observe(&fixture);
    let wrong_target = scoped_runtime_error(
        fixture.output(&revision_request("assess", "1", "wrong-revision-target")),
        1,
        "GOAL_REVISION_TARGET_NOT_ALLOWED",
    );
    assert_eq!(wrong_target["details"]["target_graph_node_id"], "assess");
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"],
        before_revision["result"]
    );
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "wrong-revision-target"
        ])["result"]["found"],
        false,
    );
    fixture.run(&[
        "goal",
        "revise",
        "--goal",
        "Repair the defect and verify both criteria.",
        "--criterion",
        "z-reproduced=The defect was reproduced.",
        "--criterion",
        "a-verified=The fix was verified.",
        "--rework-to",
        "plan",
        "--reason",
        "Refine the current criteria.",
        "--basis-description",
        "Retain the prior fixture evidence.",
        "--basis-digest",
        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "--feedback",
        "Require a fresh group result for the revised goal.",
        "--if-goal-revision",
        "1",
        "--idempotency-key",
        "revise-goal-1",
    ]);
    let revised = scoped_runtime_observe(&fixture);
    assert_eq!(revised["result"]["cursor"]["goal_revision"], 2);
    assert_eq!(revised["result"]["cursor"]["attempt"]["node"], "plan");
    let session = revised["session"]["id"].as_str().unwrap();
    scoped_runtime_activate_with_key(&fixture, true, "activate-revised");
    let second = scoped_runtime_observe(&fixture);
    let selection = &second["result"]["selection"];
    let execution = selection["execution"].as_str().unwrap();
    let definition = selection["definition_digest"].as_str().unwrap();
    assert_eq!(
        definition,
        first_terminal["result"]["selection"]["definition_digest"]
    );
    assert_ne!(execution, first_execution);
    let old_cursor = first_terminal["result"]["cursor"].to_string();
    let stale = fixture.output(&[
        "group",
        "return",
        "--if-cursor",
        &old_cursor,
        "--if-workspace-uuid",
        second["workspace"]["uuid"].as_str().unwrap(),
        "--idempotency-key",
        "return-old-group",
    ]);
    scoped_runtime_error(stale, 4, "ATTEMPT_NOT_CURRENT");
    assert_eq!(scoped_runtime_observe(&fixture)["result"], second["result"]);
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "return-old-group"])["result"]["found"],
        false,
    );
    let before_history = fixture.podway_snapshot();

    let group = fixture.run(&[
        "group",
        "show",
        "--session",
        session,
        "--execution",
        execution,
    ]);
    assert_eq!(
        group["result"]["schema"],
        "podway.successor-group-show-result/v1"
    );
    assert_eq!(group["result"]["group"]["summary"]["execution"], execution);
    assert_eq!(
        group["result"]["group"]["summary"]["definition_digest"],
        definition
    );
    let prior_work_args = [
        "group",
        "prior-work",
        "--session",
        session,
        "--execution",
        execution,
        "--definition-digest",
    ];
    let previous = fixture.run(&[&prior_work_args[..], &[definition]].concat());
    assert_eq!(
        previous["result"]["schema"],
        "podway.successor-history-prior-work-result/v1"
    );
    assert_eq!(previous["result"]["total"], 1);
    assert_eq!(previous["result"]["rows"][0]["choice"], "revalidate");
    assert_eq!(
        previous["result"]["rows"][0]["source"]["group"]["execution"],
        first_execution
    );
    scoped_runtime_error(
        fixture.output(
            &[
                &prior_work_args[..],
                &["sha256:0000000000000000000000000000000000000000000000000000000000000000"],
            ]
            .concat(),
        ),
        4,
        "GROUP_DEFINITION_MISMATCH",
    );

    let revisions = fixture.run(&["history", "goal-revisions", "--session", session]);
    assert_eq!(
        revisions["result"]["schema"],
        "podway.successor-goal-revision-history-result/v1"
    );
    assert_eq!(revisions["result"]["total"], 1);
    let revision = &revisions["result"]["rows"][0];
    assert_eq!(revision["goal_revision"], 2);
    assert_eq!(revision["predecessor"], first_execution);
    assert_eq!(
        revision["successor"],
        revised["result"]["cursor"]["attempt"]
    );
    assert_eq!(fixture.podway_snapshot(), before_history);
    fixture.run(&[
        "set",
        "result",
        "The new group recorded current evidence.",
        "--idempotency-key",
        "second-result",
    ]);
    fixture.run(&["complete", "--idempotency-key", "return-second"]);

    fn proposal(current: &Value, key: &str, criterion: &str) -> Value {
        let template = scoped_runtime_template(current, "goal.assess_criteria");
        let mut input: Value =
            serde_json::from_str(template["stdin_template"].as_str().unwrap()).unwrap();
        input["context"]["idempotency_key"] = serde_json::json!(key);
        input["actor"] = serde_json::json!("agent-to");
        input["assessments"] = serde_json::json!([{
            "criterion":criterion, "status":"satisfied", "reason":"Verified from the current group result.",
            "citations":[{"kind":"execution_result"}],
        }]);
        input
    }
    let args = ["goal", "assess-criteria", "--stdin", "--with-observation"];
    let at_assess = scoped_runtime_observe(&fixture);
    let mut stale = proposal(&at_assess, "batch-stale-goal", "a-verified");
    stale["context"]["cursor"]["goal_revision"] = json!(1);
    scoped_runtime_error(
        fixture.output_with_stdin(&args, stale.to_string().as_bytes()),
        4,
        "GOAL_REVISION_STALE",
    );
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"],
        at_assess["result"]
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "batch-stale-goal"])["result"]["found"],
        false
    );

    let partial = proposal(&at_assess, "batch-partial-1", "a-verified");
    let first = fixture.output_with_stdin(&args, partial.to_string().as_bytes());
    assert!(first.status.success(), "{first:?}");
    let first_result = serde_json::from_slice::<OutputEnvelopeV4>(&first.stdout)
        .unwrap()
        .into_value();
    assert_eq!(first_result["command"], "goal.assess_criteria");
    assert_eq!(
        first_result["result"]["schema"],
        "podway.mutation-observation-result/v2"
    );
    let observation = &first_result["result"]["observation"];
    assert_eq!(observation["goal"]["criteria"][0]["id"], "z-reproduced");
    assert!(observation["goal"]["criteria"][0]["status"].is_null());
    assert_eq!(observation["goal"]["criteria"][1]["id"], "a-verified");
    assert_eq!(observation["goal"]["criteria"][1]["status"], "satisfied");
    assert!(observation["goal"]["outcome"].is_null());

    let control = scoped_runtime_observe(&fixture);
    let early = fixture.output(&[
        "decide",
        "--option",
        "achieved",
        "--reason",
        "Second criterion is missing.",
        "--if-goal-revision",
        "2",
        "--idempotency-key",
        "decide-early",
    ]);
    let error = scoped_runtime_error(early, 1, "CRITERION_RESULT_MISSING");
    assert_eq!(
        error["details"]["missing_criterion_ids"],
        json!(["z-reproduced"])
    );
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"],
        control["result"]
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "decide-early"])["result"]["found"],
        false
    );

    let second = proposal(&control, "batch-partial-2", "z-reproduced");
    let result = fixture.output_with_stdin(&args, second.to_string().as_bytes());
    assert!(result.status.success(), "{result:?}");
    let result = serde_json::from_slice::<OutputEnvelopeV4>(&result.stdout)
        .unwrap()
        .into_value();
    let complete = &result["result"]["observation"]["goal"];
    assert_eq!(complete["criteria"][0]["status"], "satisfied");
    assert_eq!(complete["criteria"][1]["status"], "satisfied");
    assert_eq!(complete["outcome"], "achieved");
    let decided = fixture.run(&[
        "decide",
        "--option",
        "achieved",
        "--reason",
        "All criteria are satisfied.",
        "--if-goal-revision",
        "2",
        "--idempotency-key",
        "decide-final",
    ]);
    assert_eq!(decided["command"], "session.decide");
    assert_eq!(
        fixture
            .output_with_stdin(&args, partial.to_string().as_bytes())
            .stdout,
        first.stdout
    );
    assert_eq!(
        fixture.run(&["outcome", "lookup", "--idempotency-key", "batch-partial-1"])["result"]["outcome"]
            ["response"],
        first_result
    );
    fixture.run(&["complete", "--idempotency-key", "finish"]);
    let terminal = scoped_runtime_observe(&fixture);
    let scoped_runtime_error = scoped_runtime_error(
        fixture.output(&revision_request("plan", "2", "missing-reactivation")),
        1,
        "REACTIVATION_FLAG_REQUIRED",
    );
    assert_eq!(
        scoped_runtime_error["details"]["kind"],
        "REACTIVATION_FLAG_REQUIRED"
    );
    assert_eq!(
        scoped_runtime_observe(&fixture)["result"],
        terminal["result"]
    );
    assert_eq!(
        fixture.run(&[
            "outcome",
            "lookup",
            "--idempotency-key",
            "missing-reactivation"
        ])["result"]["found"],
        false,
    );
}
