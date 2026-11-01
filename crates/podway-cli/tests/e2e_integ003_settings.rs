//! Explicit settings queries remain independent of runtime admission.

use serde_json::{Value, json};
use std::{
    fs,
    process::{Command, Output, Stdio},
};

fn query(fixture: &super::cli_fixture::Fixture, home: &std::path::Path, args: &[&str]) -> Output {
    let child = Command::new(super::cli_fixture::podway_binary())
        .arg("--worktree")
        .arg(&fixture.0)
        .args(args)
        .current_dir(&fixture.0)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    super::cli_fixture::bounded_output(super::cli_fixture::GuardedChild::new(child), args)
}

#[test]
fn integ003_preinit_query_uses_one_closed_family_without_runtime_identity() {
    let fixture = super::cli_fixture::Fixture::new();
    let home = fixture.0.join("owned-home");
    fs::create_dir(&home).unwrap();
    for generation in ["1", "2"] {
        let output = query(
            &fixture,
            &home,
            &[
                "--json",
                "--contract-generation",
                generation,
                "integration",
                "show",
                "dolgorae",
            ],
        );
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        podway_protocol::OutputEnvelopeV4::from_value(value.clone()).unwrap();
        assert_eq!(value["command"], "integration.show");
        assert_eq!(
            value["result"],
            json!({"schema":"podway.integration-settings-result/v1", "integration":"dolgorae", "enabled":false, "source":"default"})
        );
        assert!(value.get("workspace").is_none() && value.get("session").is_none());
        assert!(!fixture.0.join(".podway").exists());
        assert!(!home.join(".config").exists());
    }
}

fn decoded(output: Output, code: i32) -> Value {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    if code == 0 {
        podway_protocol::OutputEnvelopeV4::from_value(value.clone()).unwrap();
    } else {
        podway_protocol::ErrorEnvelopeV2::from_value(value.clone()).unwrap();
    }
    value
}

#[test]
fn integ003_permission_sources_and_validation_order_are_independent_of_runtime() {
    let fixture = super::cli_fixture::Fixture::new();
    let home = fixture.0.join("owned-home");
    fs::create_dir_all(home.join(".config/podway")).unwrap();
    fs::create_dir(fixture.0.join(".podway")).unwrap();
    // A regular file cannot be a runtime directory or a maintenance root.
    fs::write(fixture.0.join(".podway/runtime"), b"never-open-runtime").unwrap();
    let project = fixture.0.join(".podway/config.yaml");
    let user = home.join(".config/podway/config.yaml");
    let args = ["--json", "integration", "show", "dolgorae"];
    for project_value in [None, Some(false), Some(true)] {
        for user_value in [None, Some(false), Some(true)] {
            fs::write(&project, match project_value {
                None => "schema: podway.workspace/v3\nintegrations: {dolgorae: {}}\n".to_owned(),
                Some(value) => format!("schema: podway.workspace/v3\nintegrations: {{dolgorae: {{enabled: {value}}}}}\n"),
            }).unwrap();
            fs::write(&user, match user_value {
                None => "schema: podway.user-config/v1\nintegrations: {}\n".to_owned(),
                Some(value) => format!("schema: podway.user-config/v1\nintegrations: {{dolgorae: {{enabled: {value}}}}}\n"),
            }).unwrap();
            let before = fixture.podway_snapshot();
            let value = decoded(query(&fixture, &home, &args), 0);
            assert_eq!(
                value["result"]["enabled"],
                project_value.or(user_value).unwrap_or(false)
            );
            assert_eq!(
                value["result"]["source"],
                if project_value.is_some() {
                    "project"
                } else if user_value.is_some() {
                    "user"
                } else {
                    "default"
                }
            );
            fixture.assert_podway_snapshot(&before);
        }
    }
    for invalid in [
        "",
        "invalid",
        "schema: podway.user-config/v1\nintegrations: {dolgorae: {enabled: 'true'}}\n",
    ] {
        fs::write(&user, invalid).unwrap();
        let value = decoded(query(&fixture, &home, &args), 5);
        assert_eq!(value["code"], "USER_CONFIG_INVALID");
        assert_eq!(
            value["message"],
            "User configuration is invalid or unavailable."
        );
        assert_eq!(value["details"], json!({}));
        assert_eq!(value["retryable"], false);
        assert!(
            !String::from_utf8(serde_json::to_vec(&value).unwrap())
                .unwrap()
                .contains(home.to_str().unwrap())
        );
    }
    fs::write(&project, "invalid-project").unwrap();
    for invalid_home in [home.clone(), fixture.0.join("missing"), "relative".into()] {
        let value = decoded(query(&fixture, &invalid_home, &args), 5);
        assert_eq!(value["code"], "WORKSPACE_CONFIG_INVALID");
        assert_eq!(value["details"], json!({}));
    }
    fs::remove_file(&project).unwrap();
    for invalid_home in [fixture.0.join("missing"), "relative".into(), "".into()] {
        assert_eq!(
            decoded(query(&fixture, &invalid_home, &args), 5)["code"],
            "USER_CONFIG_INVALID"
        );
    }
    assert_eq!(
        decoded(fixture.output(&["integration", "show", "dolgorae"]), 5)["code"],
        "USER_CONFIG_INVALID"
    );
}

#[test]
fn integ003_route_rejects_fences_names_and_options_before_configuration_io() {
    let fixture = super::cli_fixture::Fixture::new();
    let home = fixture.0.join("missing-home");
    fs::create_dir(fixture.0.join(".podway")).unwrap();
    fs::write(fixture.0.join(".podway/config.yaml"), "invalid-project").unwrap();
    for extra in [
        vec!["--idempotency-key", "query-key"],
        vec![
            "--if-workspace-uuid",
            "00000000-0000-4000-8000-000000000001",
        ],
        vec!["--if-session-id", "00000000-0000-4000-8000-000000000001"],
        vec!["--if-session-revision", "1"],
        vec!["--if-attempt", "00000000-0000-4000-8000-000000000001"],
        vec!["--if-item-revision", "1"],
        vec!["--if-goal-revision", "1"],
        vec!["--yes"],
        vec!["--force"],
        vec!["--all"],
        vec!["--stdin"],
        vec!["--unknown"],
        vec!["--contract-generation", "3"],
    ] {
        let mut args = vec!["--json", "integration", "show", "dolgorae"];
        args.extend(extra);
        assert_eq!(
            decoded(query(&fixture, &home, &args), 2)["code"],
            "REQUEST_INVALID",
            "{args:?}"
        );
    }
    assert_eq!(
        decoded(
            query(
                &fixture,
                &home,
                &["--json", "integration", "show", "unknown"]
            ),
            2
        )["code"],
        "REQUEST_INVALID"
    );
    assert_eq!(
        decoded(
            query(
                &fixture,
                &home,
                &["--json", "integration", "show", "dolgorae", "--detach"]
            ),
            2
        )["code"],
        "COMMAND_RETIRED"
    );
    // Git failure precedes invalid HOME, but follows name/generation validation.
    fs::remove_dir_all(fixture.0.join(".git")).unwrap();
    let value = decoded(
        query(
            &fixture,
            &home,
            &["--json", "integration", "show", "dolgorae"],
        ),
        5,
    );
    assert_eq!(value["code"], "NOT_A_GIT_WORKTREE");
}

#[test]
fn integ003_human_help_and_completions_explain_permission_and_admit_only_query_options() {
    let fixture = super::cli_fixture::Fixture::new();
    let home = fixture.0.join("owned-home");
    fs::create_dir(&home).unwrap();
    let output = query(
        &fixture,
        &home,
        &[
            "integration",
            "show",
            "dolgorae",
            "--timeout",
            "1s",
            "--no-color",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    for phrase in [
        "disabled",
        "source: default",
        "does not establish connection readiness",
        "external adapter",
    ] {
        assert!(text.contains(phrase), "{text}");
    }
    for (args, phrase) in [
        (vec!["help", "integration.show"], "dolgorae"),
        (vec!["help", "integration", "show"], "dolgorae"),
        (vec!["help", "integration"], "integration.show"),
        (vec!["-h"], "integration.show"),
        (vec!["--help"], "integration.show"),
    ] {
        let output = fixture.raw_output(&args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(String::from_utf8(output.stdout).unwrap().contains(phrase));
    }
    for shell in ["bash", "zsh", "fish"] {
        let value = fixture.run(&["completions", shell]);
        assert!(value.to_string().contains("integration show"), "{shell}");
    }
}

#[test]
fn integ003_query_reads_no_stdin_and_keeps_home_root_and_other_worktree_separate() {
    let home = super::cli_fixture::Fixture::new();
    let other = super::cli_fixture::Fixture::new();
    fs::create_dir(home.0.join(".podway")).unwrap();
    fs::write(
        home.0.join(".podway/config.yaml"),
        "schema: podway.workspace/v3\nintegrations: {dolgorae: {enabled: false}}\n",
    )
    .unwrap();
    fs::create_dir_all(home.0.join(".config/podway")).unwrap();
    fs::write(
        home.0.join(".config/podway/config.yaml"),
        "schema: podway.user-config/v1\nintegrations: {dolgorae: {enabled: true}}\n",
    )
    .unwrap();
    for (fixture, source, enabled) in [(&home, "project", false), (&other, "user", true)] {
        let mut child = Command::new(super::cli_fixture::podway_binary())
            .arg("--worktree")
            .arg(&fixture.0)
            .args(["--json", "integration", "show", "dolgorae"])
            .current_dir(&fixture.0)
            .env("HOME", &home.0)
            .env("XDG_CONFIG_HOME", other.0.join("ignored"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let producer = child.stdin.take().unwrap();
        let value = decoded(
            super::cli_fixture::bounded_output(
                super::cli_fixture::GuardedChild::new(child),
                &["integration", "show", "dolgorae"],
            ),
            0,
        );
        drop(producer);
        assert_eq!(value["result"]["source"], source);
        assert_eq!(value["result"]["enabled"], enabled);
    }
    assert!(!other.0.join(".podway").exists());
    assert!(!home.0.join(".podway/runtime").exists());
}

#[test]
fn integ003_public_query_maps_unsafe_user_inputs_and_direct_commands_ignore_them() {
    use std::os::unix::fs::symlink;
    let fixture = super::cli_fixture::Fixture::new();
    let home = fixture.0.join("owned-home");
    fs::create_dir_all(home.join(".config/podway")).unwrap();
    let user = home.join(".config/podway/config.yaml");
    let args = ["--json", "integration", "show", "dolgorae"];
    symlink(fixture.0.join("unread-user"), &user).unwrap();
    assert_eq!(
        decoded(query(&fixture, &home, &args), 5)["code"],
        "USER_CONFIG_INVALID"
    );
    fs::remove_file(&user).unwrap();
    nix::unistd::mkfifo(&user, nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert_eq!(
        decoded(query(&fixture, &home, &args), 5)["code"],
        "USER_CONFIG_INVALID"
    );
    fs::remove_file(&user).unwrap();
    fs::write(&user, vec![b' '; 65_537]).unwrap();
    assert_eq!(
        decoded(query(&fixture, &home, &args), 5)["code"],
        "USER_CONFIG_INVALID"
    );
    decoded(
        query(
            &fixture,
            &home,
            &["--json", "init", "--idempotency-key", "integ003-init"],
        ),
        0,
    );
    for enabled in [true, false] {
        fs::write(
            fixture.0.join(".podway/config.yaml"),
            format!(
                "schema: podway.workspace/v3\nintegrations: {{dolgorae: {{enabled: {enabled}}}}}\n"
            ),
        )
        .unwrap();
        decoded(query(&fixture, &home, &["--json", "workspace", "show"]), 0);
        decoded(
            query(
                &fixture,
                &fixture.0.join("missing-home"),
                &["--json", "workspace", "show"],
            ),
            0,
        );
    }
}

#[test]
fn integ003_unsafe_project_paths_map_to_project_error_without_reset_advice() {
    use std::os::unix::fs::symlink;
    for boundary in ["directory", "file"] {
        let fixture = super::cli_fixture::Fixture::new();
        let target = fixture.0.join("owned-unread-target");
        let path = if boundary == "directory" {
            fixture.0.join(".podway")
        } else {
            fs::create_dir(fixture.0.join(".podway")).unwrap();
            fixture.0.join(".podway/config.yaml")
        };
        symlink(&target, path).unwrap();
        let args = ["--json", "integration", "show", "dolgorae"];
        let value = decoded(query(&fixture, &fixture.0.join("missing-home"), &args), 5);
        assert_eq!(value["code"], "WORKSPACE_CONFIG_INVALID");
        assert_eq!(value["details"], json!({}));
        let human = query(
            &fixture,
            &fixture.0.join("missing-home"),
            &["integration", "show", "dolgorae"],
        );
        assert_eq!(human.status.code(), Some(5));
        let error = String::from_utf8(human.stderr).unwrap();
        assert!(error.contains("WORKSPACE_CONFIG_INVALID"));
        assert!(!error.contains("reset") && !error.contains(target.to_str().unwrap()));
        assert!(!target.exists());
    }
}
