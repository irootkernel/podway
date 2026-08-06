# Handoff and Resume

Podway keeps task state in the worktree ([worktree-local state](../architecture-decision-records/0004-worktree-local-state.md)), not in any actor's conversation, terminal history, or memory. Whoever can run the CLI in that worktree can take the task over exactly where it stopped: after a closed terminal, an ended or compacted agent session, or a planned handoff between a human and an agent.

## Take over in two reads

A successor starts with no context and asks the worktree:

```bash
podway status --json
podway next --json
```

That answers, without any prior transcript:

- which task and procedure govern (`task.title`, `task.procedure`);
- which stage is active, on which attempt (`current`);
- what is already `done`, what is marked `redo`, what remains `pending` (`stages`);
- which items are recorded, with their values and revisions (`items`);
- which blockers are open (`blockers`);
- what to do next, as ready-to-run argv suggestions (`next --json`).

The [agent session](agent-session.md) walkthrough shows how an automated successor continues from exactly this read.

## Scenario: an agent vanishes mid-stage

An agent works a `bug-fix` session through the `fix` stage and into `verify`, then its runtime ends — a crash, a compacted context window, or a plain shutdown. Nothing was handed off.

A second agent, or the human owner, later runs `podway status --json` in the same worktree and sees the state in [`json/status-result.json`](json/status-result.json): `verify` is current on attempt 2, `original-failure-resolved` is already recorded, two required items are still missing, and `review` is marked `redo` from an earlier return. `podway next --json` supplies the exact remaining commands. The successor continues the stage — and it cannot silently skip the redo work or the open gate, because `complete` keeps failing closed until the required items are recorded.

Handoff in the other direction works the same way: a human performs a few stages interactively and an agent picks the session up from `status --json` — or a human takes over from an agent with the plain-text views:

```bash
podway status --verbose
podway next
```

## What survives what

- An actor or CLI crash loses nothing: state changes are applied by the daemon, and acknowledged queued jobs survive a daemon restart while the worktree remains at its registered location ([reliability goals](../specs/product/goals-and-non-goals.md#reliability-goals)).
- A response lost mid-mutation is reconciled read-only with `podway job lookup --idempotency-key <key>` — see [recover a lost response](agent-session.md#recover-a-lost-response).
- Deleting the worktree deletes the session with it. Podway state is local and disposable by design; it is working state, not an archive.
