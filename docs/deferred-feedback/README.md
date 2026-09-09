# Deferred Feedback

This register tracks small review findings that are valid but intentionally not
handled in the originating change. Epic-sized findings belong in [TODO](../todo/).

## Open

| ID | Source | Feedback | Deferral reason | Revisit condition | Promoted task |
|---|---|---|---|---|---|
| DF-001 | V2FRT-002 maintainability review | Registry temporary-file formatting in `crates/podway-daemon/src/registry.rs` and reset classification in `crates/podway-service/src/runtime_reset_v1/filesystem.rs` share a naming convention without a common helper. | Current producer and reader agree; changing this established writer is unnecessary for read-only reset planning. | Before changing the registry temporary-file naming convention, give its formatter and classifier a shared owner and verify reset inventory compatibility. | — |

## Resolved

| ID | Source | Resolution | Evidence | Promoted task |
|---|---|---|---|---|

Use sequential `DF-###` identifiers. Move an entry to Resolved when it is fixed,
rejected with evidence, or promoted into the roadmap.
