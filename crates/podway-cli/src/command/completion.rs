//! Generated shell completion scripts for the public v1 CLI grammar.

use std::fmt::Write as _;

/// Supported completion script targets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    /// Emits a self-contained completion script. Dynamic candidates are best-effort only: the
    /// hidden CLI endpoint performs one bounded read-only query and emits nothing on failure.
    pub fn script(self) -> String {
        match self {
            Self::Bash => bash_script(),
            Self::Zsh => zsh_script(),
            Self::Fish => fish_script(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Flag {
    long: &'static str,
    takes_value: bool,
}

#[derive(Clone, Copy)]
struct Route {
    /// Space-separated CLI words, not the canonical wire command name.
    words: &'static str,
    flags: &'static [&'static Flag],
    values: &'static str,
    dynamic: Option<&'static str>,
}

const JSON: Flag = Flag {
    long: "json",
    takes_value: false,
};
const DEV: Flag = Flag {
    long: "dev",
    takes_value: false,
};
const WORKTREE: Flag = Flag {
    long: "worktree",
    takes_value: true,
};
const TIMEOUT: Flag = Flag {
    long: "timeout",
    takes_value: true,
};
const SOCKET: Flag = Flag {
    long: "socket",
    takes_value: true,
};
const NO_COLOR: Flag = Flag {
    long: "no-color",
    takes_value: false,
};
const QUIET: Flag = Flag {
    long: "quiet",
    takes_value: false,
};
const IDENTITY: Flag = Flag {
    long: "identity",
    takes_value: false,
};
const IDEMPOTENCY_KEY: Flag = Flag {
    long: "idempotency-key",
    takes_value: true,
};
const DETACH: Flag = Flag {
    long: "detach",
    takes_value: false,
};
const IF_WORKSPACE_UUID: Flag = Flag {
    long: "if-workspace-uuid",
    takes_value: true,
};
const IF_SESSION_ID: Flag = Flag {
    long: "if-session-id",
    takes_value: true,
};
const IF_SESSION_REVISION: Flag = Flag {
    long: "if-session-revision",
    takes_value: true,
};
const IF_ATTEMPT: Flag = Flag {
    long: "if-attempt",
    takes_value: true,
};
const IF_ITEM_REVISION: Flag = Flag {
    long: "if-item-revision",
    takes_value: true,
};
const YES: Flag = Flag {
    long: "yes",
    takes_value: false,
};
const WARNINGS_AS_ERRORS: Flag = Flag {
    long: "warnings-as-errors",
    takes_value: false,
};
const CANONICAL: Flag = Flag {
    long: "canonical",
    takes_value: false,
};
const CHECK: Flag = Flag {
    long: "check",
    takes_value: false,
};
const WRITE: Flag = Flag {
    long: "write",
    takes_value: false,
};
const FORMAT: Flag = Flag {
    long: "format",
    takes_value: true,
};
const TEMPLATE: Flag = Flag {
    long: "template",
    takes_value: true,
};
const DAEMON_PATH: Flag = Flag {
    long: "daemon-path",
    takes_value: true,
};
const PURGE_LOGS: Flag = Flag {
    long: "purge-logs",
    takes_value: false,
};
const FOLLOW: Flag = Flag {
    long: "follow",
    takes_value: false,
};
const LINES: Flag = Flag {
    long: "lines",
    takes_value: true,
};
const REPAIR: Flag = Flag {
    long: "repair",
    takes_value: false,
};
const DEEP: Flag = Flag {
    long: "deep",
    takes_value: false,
};
const PRESET: Flag = Flag {
    long: "preset",
    takes_value: true,
};
const PROCEDURE: Flag = Flag {
    long: "procedure",
    takes_value: true,
};
const TASK: Flag = Flag {
    long: "task",
    takes_value: true,
};
const REPLACE: Flag = Flag {
    long: "replace",
    takes_value: false,
};
const DRY_RUN: Flag = Flag {
    long: "dry-run",
    takes_value: false,
};
const VERBOSE: Flag = Flag {
    long: "verbose",
    takes_value: false,
};
const WAIT_FOR_IDLE: Flag = Flag {
    long: "wait-for-idle",
    takes_value: false,
};
const COMPACT: Flag = Flag {
    long: "compact",
    takes_value: false,
};
const AFTER_JOB: Flag = Flag {
    long: "after-job",
    takes_value: true,
};
const REASON: Flag = Flag {
    long: "reason",
    takes_value: true,
};
const TO: Flag = Flag {
    long: "to",
    takes_value: true,
};
const ALL: Flag = Flag {
    long: "all",
    takes_value: false,
};
const FORCE: Flag = Flag {
    long: "force",
    takes_value: false,
};
const STDIN: Flag = Flag {
    long: "stdin",
    takes_value: false,
};
const IGNORE_MISSING: Flag = Flag {
    long: "ignore-missing",
    takes_value: false,
};
const REFERENCE: Flag = Flag {
    long: "reference",
    takes_value: true,
};
const DIGEST: Flag = Flag {
    long: "digest",
    takes_value: true,
};
const EXPECT_PROCEDURE_DIGEST: Flag = Flag {
    long: "expect-procedure-digest",
    takes_value: true,
};
const SIZE: Flag = Flag {
    long: "size",
    takes_value: true,
};
const MEDIA_TYPE: Flag = Flag {
    long: "media-type",
    takes_value: true,
};
const STATE: Flag = Flag {
    long: "state",
    takes_value: true,
};

const DISPLAY_FLAGS: &[&Flag] = &[&JSON, &NO_COLOR, &QUIET];
const DAEMON_READ_FLAGS: &[&Flag] = &[&JSON, &WORKTREE, &TIMEOUT, &SOCKET, &NO_COLOR, &QUIET];
const SESSION_MUTATION_FLAGS: &[&Flag] = &[
    &JSON,
    &WORKTREE,
    &TIMEOUT,
    &SOCKET,
    &NO_COLOR,
    &QUIET,
    &IDEMPOTENCY_KEY,
    &DETACH,
    &IF_WORKSPACE_UUID,
    &IF_SESSION_ID,
    &IF_SESSION_REVISION,
    &IF_ATTEMPT,
];
const ITEM_MUTATION_FLAGS: &[&Flag] = &[
    &JSON,
    &WORKTREE,
    &TIMEOUT,
    &SOCKET,
    &NO_COLOR,
    &QUIET,
    &IDEMPOTENCY_KEY,
    &DETACH,
    &IF_WORKSPACE_UUID,
    &IF_SESSION_ID,
    &IF_ATTEMPT,
    &IF_ITEM_REVISION,
];
const START_FLAGS: &[&Flag] = &[
    &JSON,
    &WORKTREE,
    &TIMEOUT,
    &SOCKET,
    &NO_COLOR,
    &QUIET,
    &IDEMPOTENCY_KEY,
    &DETACH,
    &IF_WORKSPACE_UUID,
    &IF_SESSION_ID,
    &IF_SESSION_REVISION,
    &PRESET,
    &PROCEDURE,
    &EXPECT_PROCEDURE_DIGEST,
    &TASK,
    &REPLACE,
    &DRY_RUN,
    &YES,
];
const RESET_FLAGS: &[&Flag] = &[
    &JSON,
    &WORKTREE,
    &TIMEOUT,
    &SOCKET,
    &NO_COLOR,
    &QUIET,
    &IDEMPOTENCY_KEY,
    &DETACH,
    &IF_WORKSPACE_UUID,
    &IF_SESSION_ID,
    &IF_SESSION_REVISION,
    &ALL,
    &FORCE,
    &DRY_RUN,
    &YES,
];

/// This is the only command/flag grammar source. Every shell renderer below derives its nested
/// verbs, leaf flags, static values, and dynamic candidate kind from these routes.
const ROUTES: &[Route] = &[
    Route {
        words: "help",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "version",
        flags: &[&JSON, &NO_COLOR, &QUIET, &IDENTITY],
        values: "",
        dynamic: None,
    },
    Route {
        words: "completions",
        flags: DISPLAY_FLAGS,
        values: "bash zsh fish",
        dynamic: None,
    },
    Route {
        words: "procedure validate",
        flags: &[&JSON, &NO_COLOR, &QUIET, &WARNINGS_AS_ERRORS],
        values: "",
        dynamic: None,
    },
    Route {
        words: "procedure show",
        flags: &[&JSON, &NO_COLOR, &QUIET, &CANONICAL],
        values: "",
        dynamic: None,
    },
    Route {
        words: "procedure format",
        flags: &[&JSON, &NO_COLOR, &QUIET, &CHECK, &WRITE],
        values: "",
        dynamic: None,
    },
    Route {
        words: "procedure vet",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "procedure graph",
        flags: &[&JSON, &NO_COLOR, &QUIET, &FORMAT],
        values: "json mermaid",
        dynamic: None,
    },
    Route {
        words: "procedure lint",
        flags: &[&JSON, &NO_COLOR, &QUIET, &WARNINGS_AS_ERRORS],
        values: "",
        dynamic: None,
    },
    Route {
        words: "procedure check",
        flags: &[&JSON, &NO_COLOR, &QUIET, &WARNINGS_AS_ERRORS],
        values: "",
        dynamic: None,
    },
    Route {
        words: "procedure scaffold",
        flags: &[&JSON, &NO_COLOR, &QUIET, &TEMPLATE],
        values: "minimal",
        dynamic: None,
    },
    Route {
        words: "procedure convert",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "preset list",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "preset show",
        flags: DISPLAY_FLAGS,
        values: "analysis bug-fix docs-only sw-dev",
        dynamic: None,
    },
    Route {
        words: "preset explain",
        flags: DISPLAY_FLAGS,
        values: "analysis bug-fix docs-only sw-dev",
        dynamic: None,
    },
    Route {
        words: "daemon install",
        flags: &[&JSON, &NO_COLOR, &QUIET, &SOCKET, &DAEMON_PATH],
        values: "",
        dynamic: None,
    },
    Route {
        words: "daemon uninstall",
        flags: &[&JSON, &NO_COLOR, &QUIET, &YES, &PURGE_LOGS],
        values: "",
        dynamic: None,
    },
    Route {
        words: "daemon start",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "daemon stop",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "daemon restart",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "daemon status",
        flags: DISPLAY_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "terminate",
        flags: &[&JSON, &DEV, &TIMEOUT, &NO_COLOR, &QUIET],
        values: "",
        dynamic: None,
    },
    Route {
        words: "daemon logs",
        flags: &[&JSON, &NO_COLOR, &QUIET, &FOLLOW, &LINES],
        values: "",
        dynamic: None,
    },
    Route {
        words: "init",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &REPAIR,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "doctor",
        flags: &[
            &JSON, &WORKTREE, &TIMEOUT, &SOCKET, &NO_COLOR, &QUIET, &DEEP,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "workspace show",
        flags: DAEMON_READ_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "workspace repair",
        flags: DAEMON_READ_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "start",
        flags: START_FLAGS,
        values: "analysis bug-fix docs-only sw-dev",
        dynamic: None,
    },
    Route {
        words: "status",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &VERBOSE,
            &WAIT_FOR_IDLE,
            &COMPACT,
            &AFTER_JOB,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "next",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &WAIT_FOR_IDLE,
            &AFTER_JOB,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "complete",
        flags: SESSION_MUTATION_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "skip",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &IF_ATTEMPT,
            &REASON,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "retry",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &IF_ATTEMPT,
            &REASON,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "return",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &IF_ATTEMPT,
            &TO,
            &REASON,
            &DRY_RUN,
        ],
        values: "",
        dynamic: Some("returns"),
    },
    Route {
        words: "block",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &IF_ATTEMPT,
            &REASON,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "unblock",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &IF_ATTEMPT,
            &ALL,
        ],
        values: "",
        dynamic: Some("blockers"),
    },
    Route {
        words: "cancel",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &IF_ATTEMPT,
            &REASON,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "reopen",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_SESSION_REVISION,
            &TO,
            &REASON,
            &DRY_RUN,
        ],
        values: "",
        dynamic: Some("returns"),
    },
    Route {
        words: "reset",
        flags: RESET_FLAGS,
        values: "",
        dynamic: None,
    },
    Route {
        words: "check",
        flags: ITEM_MUTATION_FLAGS,
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "uncheck",
        flags: ITEM_MUTATION_FLAGS,
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "set",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_ATTEMPT,
            &IF_ITEM_REVISION,
            &STDIN,
        ],
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "add",
        flags: ITEM_MUTATION_FLAGS,
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "remove",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_ATTEMPT,
            &IF_ITEM_REVISION,
            &IGNORE_MISSING,
        ],
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "attach",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
            &DETACH,
            &IF_WORKSPACE_UUID,
            &IF_SESSION_ID,
            &IF_ATTEMPT,
            &IF_ITEM_REVISION,
            &REFERENCE,
            &DIGEST,
            &SIZE,
            &MEDIA_TYPE,
        ],
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "clear",
        flags: ITEM_MUTATION_FLAGS,
        values: "",
        dynamic: Some("items"),
    },
    Route {
        words: "job list",
        flags: &[
            &JSON, &WORKTREE, &TIMEOUT, &SOCKET, &NO_COLOR, &QUIET, &STATE,
        ],
        values: "queued running succeeded failed cancelled",
        dynamic: None,
    },
    Route {
        words: "job lookup",
        flags: &[
            &JSON,
            &WORKTREE,
            &TIMEOUT,
            &SOCKET,
            &NO_COLOR,
            &QUIET,
            &IDEMPOTENCY_KEY,
        ],
        values: "",
        dynamic: None,
    },
    Route {
        words: "job status",
        flags: DAEMON_READ_FLAGS,
        values: "",
        dynamic: Some("jobs"),
    },
    Route {
        words: "job wait",
        flags: DAEMON_READ_FLAGS,
        values: "",
        dynamic: Some("jobs"),
    },
    Route {
        words: "job cancel",
        flags: DAEMON_READ_FLAGS,
        values: "",
        dynamic: Some("jobs"),
    },
];

fn roots() -> Vec<&'static str> {
    let mut roots = Vec::new();
    for route in ROUTES {
        let root = route
            .words
            .split_once(' ')
            .map_or(route.words, |(root, _)| root);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

fn children(parent: &str) -> Vec<&'static str> {
    ROUTES
        .iter()
        .filter_map(|route| {
            route
                .words
                .strip_prefix(parent)
                .and_then(|suffix| suffix.strip_prefix(' '))
                .filter(|child| !child.contains(' '))
        })
        .collect()
}

fn parents() -> Vec<&'static str> {
    let mut parents = Vec::new();
    for route in ROUTES {
        match route.words.split_once(' ') {
            Some((parent, _)) if !parents.contains(&parent) => parents.push(parent),
            _ => {}
        }
    }
    parents
}

fn shared_flags() -> Vec<&'static Flag> {
    ROUTES[0]
        .flags
        .iter()
        .copied()
        .filter(|flag| ROUTES.iter().all(|route| route.flags.contains(flag)))
        .collect()
}

fn static_candidates(route: &Route) -> Vec<String> {
    route
        .flags
        .iter()
        .map(|flag| format!("--{}", flag.long))
        .chain(route.values.split_whitespace().map(str::to_owned))
        .collect()
}

fn bash_script() -> String {
    let mut script = String::from("# podway bash completion (generated from ROUTES)\n");
    script.push_str("_podway_dynamic() {\n  local kind=$1 worktree=\"\" socket=\"\" index word\n  for ((index = 1; index < COMP_CWORD; ++index)); do\n    word=${COMP_WORDS[index]}\n    case \"$word\" in\n      --worktree) ((++index)); worktree=${COMP_WORDS[index]} ;;\n      --worktree=*) worktree=${word#--worktree=} ;;\n      --socket) ((++index)); socket=${COMP_WORDS[index]} ;;\n      --socket=*) socket=${word#--socket=} ;;\n    esac\n  done\n  local -a endpoint=() workspace=()\n  [[ -n \"$socket\" ]] && endpoint=(--socket \"$socket\")\n  [[ -n \"$worktree\" ]] && workspace=(--worktree \"$worktree\")\n  command podway \"${endpoint[@]}\" \"${workspace[@]}\" __complete \"$kind\" 2>/dev/null\n}\n");
    script.push_str("_podway_route() {\n  local word root=\"\" index expecting_worktree=0\n");
    script.push_str("  for ((index = 1; index < COMP_CWORD; ++index)); do\n");
    script.push_str("    word=${COMP_WORDS[index]}\n");
    script.push_str("    if [[ $expecting_worktree -eq 1 ]]; then\n      expecting_worktree=0\n      continue\n    fi\n    case \"$word\" in\n      --worktree) expecting_worktree=1; continue ;;\n      --worktree=*) continue ;;\n    esac\n");
    script.push_str("    if [[ -z \"$root\" ]]; then\n      case \"$word\" in\n");
    for root in roots() {
        let _ = writeln!(script, "        {root}) root={root} ;;");
    }
    script.push_str("      esac\n      continue\n    fi\n");
    script.push_str("    case \"$root\" in\n");
    for parent in parents() {
        let children = children(parent).join("|");
        let _ = writeln!(
            script,
            "      {parent}) case \"$word\" in {children}) printf '%s\\n' \"$root $word\"; return ;; esac ;;"
        );
    }
    script.push_str("    esac\n  done\n  printf '%s\\n' \"${root:-root}\"\n}\n");
    script.push_str("_podway_candidates() {\n  case \"$1\" in\n");
    let root_candidates = roots()
        .into_iter()
        .map(str::to_owned)
        .chain(
            shared_flags()
                .into_iter()
                .map(|flag| format!("--{}", flag.long)),
        )
        .collect::<Vec<_>>();
    write_bash_candidates(&mut script, "root", &root_candidates, None);
    for parent in parents() {
        let children = children(parent)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        write_bash_candidates(&mut script, parent, &children, None);
    }
    for route in ROUTES {
        write_bash_candidates(
            &mut script,
            route.words,
            &static_candidates(route),
            route.dynamic,
        );
    }
    script.push_str("  esac\n}\n");
    script.push_str("_podway() {\n  local cur=\"${COMP_WORDS[COMP_CWORD]}\" route\n");
    script.push_str("  route=$(_podway_route)\n");
    script.push_str(
        "  COMPREPLY=( $(compgen -W \"$(_podway_candidates \"$route\")\" -- \"$cur\") )\n}\n",
    );
    script.push_str("complete -F _podway podway\n");
    script
}

fn write_bash_candidates(
    script: &mut String,
    route: &str,
    candidates: &[String],
    dynamic: Option<&str>,
) {
    let _ = writeln!(script, "    \"{route}\")");
    if !candidates.is_empty() {
        let _ = writeln!(script, "      printf '%s\\n' {}", candidates.join(" "));
    }
    if let Some(kind) = dynamic {
        let _ = writeln!(script, "      _podway_dynamic {kind}");
    }
    script.push_str("      ;;\n");
}

fn zsh_script() -> String {
    let mut script = String::from("#compdef podway\n# Generated from ROUTES.\n");
    script.push_str("_podway_dynamic() {\n  local kind=$1 worktree=\"\" socket=\"\" index word\n  for ((index = 2; index < CURRENT; ++index)); do\n    word=$words[index]\n    case \"$word\" in\n      --worktree) ((++index)); worktree=$words[index] ;;\n      --worktree=*) worktree=${word#--worktree=} ;;\n      --socket) ((++index)); socket=$words[index] ;;\n      --socket=*) socket=${word#--socket=} ;;\n    esac\n  done\n  local -a endpoint=() workspace=()\n  [[ -n \"$socket\" ]] && endpoint=(--socket \"$socket\")\n  [[ -n \"$worktree\" ]] && workspace=(--worktree \"$worktree\")\n  command podway \"${endpoint[@]}\" \"${workspace[@]}\" __complete \"$kind\" 2>/dev/null\n}\n");
    script.push_str("_podway_route() {\n  local word root=\"\" index expecting_worktree=0\n");
    script.push_str("  for ((index = 2; index < CURRENT; ++index)); do\n");
    script.push_str("    word=$words[index]\n");
    script.push_str("    if [[ $expecting_worktree -eq 1 ]]; then\n      expecting_worktree=0\n      continue\n    fi\n    case \"$word\" in\n      --worktree) expecting_worktree=1; continue ;;\n      --worktree=*) continue ;;\n    esac\n");
    script.push_str("    if [[ -z \"$root\" ]]; then\n      case \"$word\" in\n");
    for root in roots() {
        let _ = writeln!(script, "        {root}) root={root} ;;");
    }
    script.push_str("      esac\n      continue\n    fi\n");
    script.push_str("    case \"$root\" in\n");
    for parent in parents() {
        let children = children(parent).join("|");
        let _ = writeln!(
            script,
            "      {parent}) case \"$word\" in {children}) print -r -- \"$root $word\"; return ;; esac ;;"
        );
    }
    script.push_str("    esac\n  done\n  print -r -- \"${root:-root}\"\n}\n");
    script.push_str("_podway_candidates() {\n  case \"$1\" in\n");
    let root_candidates = roots()
        .into_iter()
        .map(str::to_owned)
        .chain(
            shared_flags()
                .into_iter()
                .map(|flag| format!("--{}", flag.long)),
        )
        .collect::<Vec<_>>();
    write_zsh_candidates(&mut script, "root", &root_candidates, None);
    for parent in parents() {
        let children = children(parent)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        write_zsh_candidates(&mut script, parent, &children, None);
    }
    for route in ROUTES {
        write_zsh_candidates(
            &mut script,
            route.words,
            &static_candidates(route),
            route.dynamic,
        );
    }
    script.push_str("  esac\n}\n");
    script.push_str("_podway() {\n  local route\n  local -a candidates\n");
    script.push_str("  route=$(_podway_route)\n");
    script.push_str("  candidates=(\"${(@f)$(_podway_candidates \"$route\")}\")\n");
    script.push_str("  compadd -- $candidates\n}\ncompdef _podway podway\n");
    script
}

fn write_zsh_candidates(
    script: &mut String,
    route: &str,
    candidates: &[String],
    dynamic: Option<&str>,
) {
    let _ = writeln!(script, "    \"{route}\")");
    if !candidates.is_empty() {
        let _ = writeln!(script, "      print -rl -- {}", candidates.join(" "));
    }
    if let Some(kind) = dynamic {
        let _ = writeln!(script, "      _podway_dynamic {kind}");
    }
    script.push_str("      ;;\n");
}

fn fish_script() -> String {
    let mut script = String::from("# podway fish completion (generated from ROUTES)\n");
    script.push_str("function __podway_dynamic\n  set -l worktree\n  set -l socket\n  set -l expecting_worktree 0\n  set -l expecting_socket 0\n  for word in (commandline -opc)\n    if test $expecting_worktree -eq 1\n      set worktree \"$word\"\n      set expecting_worktree 0\n      continue\n    end\n    if test $expecting_socket -eq 1\n      set socket \"$word\"\n      set expecting_socket 0\n      continue\n    end\n    switch \"$word\"\n      case --worktree\n        set expecting_worktree 1\n      case '--worktree=*'\n        set worktree (string replace -- '--worktree=' '' \"$word\")\n      case --socket\n        set expecting_socket 1\n      case '--socket=*'\n        set socket (string replace -- '--socket=' '' \"$word\")\n    end\n  end\n  set -l endpoint\n  set -l workspace\n  if test -n \"$socket\"; set endpoint --socket \"$socket\"; end\n  if test -n \"$worktree\"; set workspace --worktree \"$worktree\"; end\n  command podway $endpoint $workspace __complete $argv 2>/dev/null\nend\n");
    script.push_str("function __podway_route\n  set -l root\n  set -l expecting_worktree 0\n");
    script.push_str("  for word in (commandline -opc)\n    if test \"$word\" = podway\n      continue\n    end\n    if test $expecting_worktree -eq 1\n      set expecting_worktree 0\n      continue\n    end\n    switch \"$word\"\n      case --worktree\n        set expecting_worktree 1\n        continue\n      case '--worktree=*'\n        continue\n    end\n");
    script.push_str("    if test -z \"$root\"\n      switch \"$word\"\n");
    for root in roots() {
        let _ = writeln!(script, "        case {root}; set root {root}");
    }
    script.push_str("      end\n      continue\n    end\n");
    script.push_str("    switch \"$root\"\n");
    for parent in parents() {
        let children = children(parent).join(" ");
        let _ = writeln!(
            script,
            "      case {parent}\n        switch \"$word\"\n          case {children}; echo \"$root $word\"; return\n        end"
        );
    }
    script.push_str("    end\n  end\n  if test -n \"$root\"\n    echo \"$root\"\n  else\n    echo root\n  end\nend\n");
    script.push_str("function __podway_route_is\n  test (__podway_route) = \"$argv[1]\"\nend\n");
    script.push_str("complete -c podway -f\n");
    let roots = roots().join(" ");
    let _ = writeln!(
        script,
        "complete -c podway -n '__podway_route_is root' -a '{roots}'"
    );
    for flag in shared_flags() {
        write_fish_flag(&mut script, "root", flag);
    }
    for parent in parents() {
        let children = children(parent).join(" ");
        let _ = writeln!(
            script,
            "complete -c podway -n '__podway_route_is \"{parent}\"' -a '{children}'"
        );
    }
    for route in ROUTES {
        for flag in route.flags {
            write_fish_flag(&mut script, route.words, flag);
        }
        if !route.values.is_empty() {
            let _ = writeln!(
                script,
                "complete -c podway -n '__podway_route_is \"{}\"' -a '{}'",
                route.words, route.values
            );
        }
        if let Some(kind) = route.dynamic {
            let _ = writeln!(
                script,
                "complete -c podway -n '__podway_route_is \"{}\"' -a '(__podway_dynamic {kind})'",
                route.words
            );
        }
    }
    script
}

fn write_fish_flag(script: &mut String, route: &str, flag: &Flag) {
    let _ = write!(
        script,
        "complete -c podway -n '__podway_route_is \"{route}\"' -l {}",
        flag.long
    );
    if flag.takes_value {
        script.push_str(" -r");
    }
    script.push('\n');
}
