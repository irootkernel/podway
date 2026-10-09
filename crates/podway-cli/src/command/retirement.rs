use std::{ffi::OsString, os::unix::ffi::OsStrExt};

use clap::{Arg, Command};
use serde::Deserialize;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct RetiredSurface {
    pub name: String,
    pub replacement: Option<String>,
    pub command: String,
}

#[derive(Debug, Default)]
pub(super) struct Classification {
    pub retired: Option<RetiredSurface>,
    pub json_requested: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    schema: String,
    availability: String,
    commands: Vec<RetiredCommand>,
    flags: Vec<RetiredFlag>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredCommand {
    command: String,
    replacement: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredFlag {
    flag: String,
    replacement: Option<String>,
}

fn retired_arity(name: &[u8]) -> Option<bool> {
    match name {
        b"mode" | b"socket" | b"after-job" => Some(true),
        b"dev" | b"detach" | b"wait-for-idle" => Some(false),
        _ => None,
    }
}

fn long_argument<'a>(scope: &'a Command, name: &[u8]) -> Option<&'a Arg> {
    scope.get_arguments().find(|arg| {
        arg.get_long().is_some_and(|long| long.as_bytes() == name)
            || arg
                .get_all_aliases()
                .is_some_and(|aliases| aliases.iter().any(|alias| alias.as_bytes() == name))
    })
}

fn surface(table: &Table, name: &str, command: &str) -> RetiredSurface {
    let replacement = table
        .commands
        .iter()
        .find(|entry| entry.command == name)
        .and_then(|entry| entry.replacement.clone());
    RetiredSurface {
        name: name.to_owned(),
        replacement,
        command: command.to_owned(),
    }
}

pub(super) fn help_guidance(topic: &str) -> Result<Option<String>, serde_json::Error> {
    let table: Table = serde_json::from_str(include_str!(
        "../../../../assets/specifications/retired-commands-v1.json"
    ))?;
    let name = match topic {
        "daemon" => "daemon *",
        "runtime" | "runtime.reset" => "runtime reset *",
        "workspace.mode" => "workspace mode *",
        "job" => "job *",
        _ => topic,
    };
    Ok(table
        .commands
        .iter()
        .find(|entry| entry.command == name)
        .map(|entry| match &entry.replacement {
            Some(replacement) => format!("This command is retired. Use {replacement} instead."),
            None => "This command is retired. No replacement exists.".to_owned(),
        }))
}

/// Classify syntax positions only. Values remain native bytes, and this function has no I/O.
pub(super) fn classify(
    arguments: &[OsString],
    grammar: &Command,
) -> Result<Classification, serde_json::Error> {
    let table: Table = serde_json::from_str(include_str!(
        "../../../../assets/specifications/retired-commands-v1.json"
    ))?;
    debug_assert_eq!(table.schema, "podway.retired-command-table/v1");
    debug_assert!(!table.availability.is_empty());
    let mut result = Classification::default();
    let mut scope = grammar;
    let mut path: Vec<&str> = Vec::new();
    let mut pending: Option<&Arg> = None;
    let mut retired_value = false;
    let mut positional = 0;
    let mut family: Option<&str> = None;
    let mut refine_family = false;
    let mut retained = vec![arguments.first().cloned().unwrap_or_default()];
    let mut unknown_syntax = false;

    for argument in arguments.iter().skip(1) {
        let bytes = argument.as_bytes();
        if let Some(arg) = pending.take() {
            let hyphen = bytes.len() > 1 && bytes.starts_with(b"-");
            if !hyphen || arg.is_allow_hyphen_values_set() {
                retained.push(argument.clone());
                continue;
            }
        }
        if retired_value {
            retired_value = false;
            if bytes.len() <= 1 || !bytes.starts_with(b"-") {
                continue;
            }
        }
        if bytes == b"--" {
            break;
        }
        if let Some(option) = bytes.strip_prefix(b"--") {
            let (name, attached) = match option.iter().position(|byte| *byte == b'=') {
                Some(index) => (&option[..index], true),
                None => (option, false),
            };
            if name == b"json" && !attached {
                result.json_requested = true;
            }
            if !unknown_syntax && let Some(takes_value) = retired_arity(name) {
                if result.retired.is_none() {
                    let entry = table
                        .flags
                        .iter()
                        .find(|entry| {
                            entry.flag.as_bytes()
                                == bytes.split(|byte| *byte == b'=').next().unwrap_or_default()
                        })
                        .expect("the closed retirement overlay matches the canonical table");
                    result.retired = Some(RetiredSurface {
                        name: entry.flag.clone(),
                        replacement: entry.replacement.clone(),
                        command: "cli".to_owned(),
                    });
                }
                retired_value = takes_value && !attached;
                continue;
            }
            let Some(arg) = long_argument(scope, name).or_else(|| long_argument(grammar, name))
            else {
                if family.is_some() {
                    // Retired families have no execution grammar. Their tail cannot dispatch;
                    // an attached value remains opaque while a later global --json still applies.
                    continue;
                }
                unknown_syntax = true;
                continue;
            };
            retained.push(argument.clone());
            if !attached && arg.get_num_args().is_some_and(|range| range.takes_values()) {
                pending = Some(arg);
            }
            continue;
        }
        if bytes.len() > 1 && bytes.starts_with(b"-") {
            if family.is_some() {
                continue;
            }
            unknown_syntax = true;
            continue;
        }
        if unknown_syntax {
            continue;
        }
        if let Some(prefix) = family {
            if refine_family {
                refine_family = false;
                let candidate = argument.to_str().map(|child| format!("{prefix}.{child}"));
                if let Some(candidate) = candidate
                    && table
                        .commands
                        .iter()
                        .any(|entry| entry.command == candidate)
                    && let Some(retired) = &mut result.retired
                    && retired.name.ends_with(" *")
                {
                    *retired = surface(&table, &candidate, &candidate);
                }
            }
            if let Some(child) = scope.find_subcommand(argument) {
                scope = child;
            }
            continue;
        }

        let retired_family = match (path.as_slice(), bytes) {
            ([], b"daemon") => Some(("daemon", "daemon *")),
            (["runtime"], b"reset") => Some(("runtime.reset", "runtime reset *")),
            (["workspace"], b"mode") => Some(("workspace.mode", "workspace mode *")),
            _ => None,
        };
        if let Some((prefix, name)) = retired_family {
            if result.retired.is_none() {
                result.retired = Some(surface(&table, name, "cli"));
            }
            family = Some(prefix);
            refine_family = true;
            if let Some(child) = scope.find_subcommand(argument) {
                scope = child;
            }
            continue;
        }
        if path.is_empty() && bytes == b"terminate" {
            if result.retired.is_none() {
                result.retired = Some(surface(&table, "daemon.terminate", "daemon.terminate"));
            }
            family = Some("daemon");
            continue;
        }
        if path.as_slice() == ["job"]
            && matches!(bytes, b"list" | b"status" | b"wait" | b"cancel" | b"lookup")
        {
            let name = format!("job.{}", argument.to_str().expect("matched ASCII child"));
            if result.retired.is_none() {
                result.retired = Some(surface(&table, &name, &name));
            }
            family = Some("job");
            continue;
        }
        // These two prefixes remain classifier metadata after their execution grammar is removed.
        if path.is_empty() && matches!(bytes, b"runtime" | b"job") {
            path.push(if bytes == b"runtime" {
                "runtime"
            } else {
                "job"
            });
            if let Some(child) = scope.find_subcommand(argument) {
                scope = child;
            }
            continue;
        }
        if let Some(child) = scope.find_subcommand(argument) {
            path.push(child.get_name());
            scope = child;
            positional = 0;
            retained.push(argument.clone());
        } else if scope.get_positionals().nth(positional).is_some() {
            positional += 1;
            retained.push(argument.clone());
        } else {
            unknown_syntax = true;
        }
    }
    if let Some(retired) = &mut result.retired
        && retired.name.starts_with("--")
        && let Some(context) = super::parse_failure_command_context(&retained)
    {
        retired.command = context.route.to_owned();
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn classified(arguments: &[&str]) -> Classification {
        let mut grammar = super::super::Cli::command();
        grammar.build();
        let arguments = std::iter::once("podway")
            .chain(arguments.iter().copied())
            .map(OsString::from)
            .collect::<Vec<_>>();
        classify(&arguments, &grammar).unwrap()
    }

    #[test]
    fn commands_and_missing_or_attached_retired_flags_are_recognized() {
        for (arguments, name, command) in [
            (
                vec!["daemon", "status", "--json"],
                "daemon.status",
                "daemon.status",
            ),
            (vec!["daemon", "mystery"], "daemon *", "cli"),
            (
                vec!["runtime", "reset", "mystery"],
                "runtime reset *",
                "cli",
            ),
            (
                vec!["workspace", "mode", "mystery"],
                "workspace mode *",
                "cli",
            ),
            (
                vec!["runtime", "reset", "apply"],
                "runtime.reset.apply",
                "runtime.reset.apply",
            ),
            (
                vec!["workspace", "--json", "mode", "plan"],
                "workspace.mode.plan",
                "workspace.mode.plan",
            ),
            (vec!["terminate"], "daemon.terminate", "daemon.terminate"),
            (vec!["job", "lookup"], "job.lookup", "job.lookup"),
            (
                vec!["--mode", "daemon", "status"],
                "--mode",
                "session.status",
            ),
            (
                vec!["status", "--after-job"],
                "--after-job",
                "session.status",
            ),
            (vec!["status", "--dev=false"], "--dev", "session.status"),
            (
                vec!["record", "--stdin", "--detach"],
                "--detach",
                "item.record_many",
            ),
        ] {
            let result = classified(&arguments);
            let retired = result.retired.unwrap();
            assert_eq!(retired.name, name, "{arguments:?}");
            assert_eq!(retired.command, command, "{arguments:?}");
            assert_eq!(
                retired.replacement.as_deref(),
                (name == "job.lookup").then_some("outcome.lookup")
            );
        }
        assert!(classified(&["daemon", "status", "--json"]).json_requested);
        assert!(
            classified(&[
                "daemon",
                "install",
                "--daemon-path",
                "/missing/podwayd",
                "--json"
            ])
            .json_requested
        );
        assert!(!classified(&["daemon", "install", "--daemon-path=--json"]).json_requested);
        assert!(!classified(&["daemon", "install", "--", "--json"]).json_requested);
    }

    #[test]
    fn retained_values_help_topics_and_delimiters_are_opaque() {
        for arguments in [
            vec!["--worktree", "daemon", "status"],
            vec!["--worktree=daemon", "status"],
            vec!["start", "--preset", "sw-dev-v2", "--task=--mode"],
            vec!["help", "daemon"],
            vec!["help", "runtime", "reset"],
            vec!["set", "item", "--", "--mode"],
            vec!["status", "--unknown", "--detach"],
            vec!["job", "mystery"],
            vec!["runtime", "mystery"],
            vec!["--modeful", "status"],
        ] {
            assert!(classified(&arguments).retired.is_none(), "{arguments:?}");
        }
        assert!(!classified(&["set", "item", "--", "--json"]).json_requested);
        assert!(!classified(&["start", "--task=--json"]).json_requested);
    }

    #[test]
    fn late_json_respects_unknown_syntax_and_delimiters() {
        for arguments in [
            vec!["unknown", "--json"],
            vec!["status", "--unknown", "--json"],
            vec!["status", "-x", "--json"],
            vec![
                "status",
                "--unknown",
                "--idempotency-key",
                "--json",
                "--json",
            ],
        ] {
            assert!(classified(&arguments).json_requested, "{arguments:?}");
        }
        for arguments in [
            vec!["unknown", "--", "--json"],
            vec!["status", "--unknown", "--", "--json"],
            vec!["status", "-x", "--", "--json"],
            vec!["unknown", "--task=--json"],
            vec!["status", "--unknown", "--idempotency-key", "--json"],
        ] {
            assert!(!classified(&arguments).json_requested, "{arguments:?}");
        }
    }

    #[test]
    fn native_non_utf8_values_are_not_command_words() {
        use std::os::unix::ffi::OsStringExt;
        let mut grammar = super::super::Cli::command();
        grammar.build();
        let arguments = vec![
            OsString::from("podway"),
            OsString::from("--worktree"),
            OsString::from_vec(b"daemon\xff".to_vec()),
            OsString::from("status"),
        ];
        assert!(classify(&arguments, &grammar).unwrap().retired.is_none());
    }
}
