# Examples

- [`.podway/config.yaml`](.podway/config.yaml): reference tracked workspace configuration.
- [Custom procedure](.podway/procedures/custom-bug-fix.yaml): valid worktree-local procedure.
- [Example session](example-session.md): complete CLI walkthrough with retry and return.
- [Agent session](agent-session.md): the same workflow driven through the JSON automation contract.
- [Procedure v2 workflow](v2-workflow.md): public preset admission, decision, rework,
  goal, and stable JSON-field walkthrough.
- [Handoff and resume](handoff-session.md): taking over a task from recorded state alone.
- [Integration session](integration-session.md): governing the merge of parallel worktrees as its own session.
- [Authoring a custom procedure](authoring-procedures.md): converting a team checklist into a procedure.
- [`json/status-result.json`](json/status-result.json): status result payload.
- [`json/compact-status-output.json`](json/compact-status-output.json): bounded idle status envelope.
- [`json/next-result.json`](json/next-result.json): next result payload.
- [`json/daemon-status-result.json`](json/daemon-status-result.json): daemon status result payload.
- [`json/output-complete.json`](json/output-complete.json): complete command success envelope.
- [`json/error-required-items.json`](json/error-required-items.json): structured domain error.
- [`json/ipc-complete-request.json`](json/ipc-complete-request.json): IPC mutation request.
- [`json/registry.json`](json/registry.json): minimal global registry.

These JSON files are manifest-covered known answers. Request and response
examples are decoder-verified; schemas and specifications remain normative.
