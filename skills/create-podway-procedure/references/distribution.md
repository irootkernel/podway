# Distributed Procedure Bundles

Read this reference only when a repository ships Procedure source to other worktrees or maintains installed copies.

## Bind the consumer contract

- Keep one consumer-owned canonical source. Installed or repository-local copies must be byte-identical and must never become a second authoring source.
- Call consumer-owned files managed Procedures, not Podway built-in presets. Only the catalog embedded in the Podway binary is a built-in preset.
- Identify the first immutable stable Podway release that supports every authored feature. Do not ship against an unreleased branch, silently widen a supported release range, or validate only with a newer incompatible binary.
- Preserve admitted sessions as immutable snapshots. A new Procedure version affects only later starts; never rewrite or reinterpret runtime history during managed-file replacement.
- Treat copying, replacing, installing, or deleting a managed Procedure as a separate consumer-authorized mutation. Active-session disposition does not grant file replacement authority.

## Require executable path evidence

- Inventory every accepted clean, failure, conditional, guarded, goal-outcome, branch-specific, and declared manual-rework path. Give each case a stable ID, expected result, and exact runtime proof locator.
- Treat Procedure authoring as authority to prepare this inventory, not to initialize a workspace, start a session, or control a daemon. Exercise the canonical bytes only under separate runtime authorization, using `$use-podway` when available.
- When runtime qualification is authorized, exercise the canonical bytes through the public CLI and an isolated daemon/worktree. Static parsing, a graph projection, or a copied upstream preset test does not prove the consumer Procedure's runtime paths.
- Test failure and stale-evidence behavior needed by the graph, not only successful closeout. Include paging continuation and stale-token rejection only when the Procedure intentionally selects multi-page evidence.
- Run the consumer repository's complete development gate after focused qualification. Keep raw runtime evidence in ignored local storage and commit only the durable inventory and test implementation required by repository policy.

## Deliver a versioned bundle

1. Update the canonical Procedure and its version.
2. Validate the exact canonical bytes with the minimum supported released Podway line.
3. Refresh every managed copy byte-for-byte and verify its digest.
4. Update consumer skills, documentation, installation diagnostics, and compatibility pins that name the Procedure, version, items, options, routes, or digest.
5. Hand off the exact preview start suggestion and accepted-path inventory. Under separate runtime authorization, run the qualification and complete consumer gate.
6. Report source and copy digests, supported Podway release, runtime proof, existing-session behavior, and publication state separately.

Do not dynamically generate a distributed Procedure during consumer setup or normal workflow execution. Ship reviewed static source so users can inspect the exact graph before it is admitted.
