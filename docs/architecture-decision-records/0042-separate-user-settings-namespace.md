# ADR-0042: Separate User Settings from the Workspace Namespace

- Status: Accepted
- Date: 2026-11-01
- Acceptance owner: Master, through approval of the exact INTEG revision diff
- Supersedes in part: [ADR-0041](0041-user-integration-settings.md), only the user-settings location in decision 1
- Preserves: all other decisions of [ADR-0041](0041-user-integration-settings.md)
- Delivery owner: [INTEG](../roadmap/README.md#integ-dolgorae-integration-readiness)
- Detailed SOT: [Integration readiness](../todo/TODO-podway-dolgorae-integration-readiness.md)

This decision takes effect on exact document approval and application.
Implementation remains pending under INTEG; it authorizes no product execution,
installation, Dolgorae change or configuration migration.

## Context

ADR-0041 chose `~/.podway/config.yaml` for user settings. Podway also supports
Git worktrees whose root is the selected HOME. In such a worktree that file is
project configuration, and complete workspace removal owns its `.podway` tree.
The distinct closed user/project schemas cannot describe one shared document.
Renaming a file inside that tree would still leave its removal ownership wrong.

## Decision

The sole user-settings location is `$HOME/.config/podway/config.yaml`, where
HOME is selected by ADR-0041's existing lossless, nonempty, absolute-directory
rule. This is a fixed HOME-derived path, not XDG discovery. Do not read
`XDG_CONFIG_HOME`, search the old user path or introduce fallback/migration.
Worktree configuration remains `<worktree>/.podway/config.yaml`.

This supersedes only ADR-0041 decision 1's user-settings location. Preserve its
read-only narrow exception, schema families, precedence, validation, bounded
native reads, Direct independence, executor ownership and activation boundaries.
Below the selected home root, nofollow-read `.config`, `podway` and `config.yaml`
without creating them. Missing directories/file mean no override; unsafe,
unreadable or changed inputs retain the selected user-configuration error.

User settings are outside workspace init, runtime, maintenance and deletion
ownership. A HOME-root worktree and a separate worktree must interpret their
project and user documents independently. Initialization must not overwrite
user settings; confirmed removal of the HOME-root worktree's `.podway` tree
must preserve `$HOME/.config/podway/config.yaml`.

## Rejected alternatives

- Keep the original shared file: distinct closed schemas conflict in HOME-root worktrees.
- Rename inside `~/.podway`: complete workspace removal still deletes user settings.
- Add HOME-specific Direct admission/removal rules: makes ordinary work depend on home.
- Add XDG or old-path fallback: adds discovery and ambiguity beyond the selected need.

## Consequences

The fixed path separates configuration and removal ownership without changing
supported worktree roots. INTEG-001 reconciles affected authority and guidance;
INTEG-002 verifies HOME-root and separate-worktree isolation. Accepted ADR-0041
remains historical authority with a supersession link; its body is not rewritten.
