# Deferred Feedback

This register tracks small review findings that are valid but intentionally not
handled in the originating change. Epic-sized findings belong in [TODO](../todo/).

## Open

| ID | Source | Feedback | Deferral reason | Revisit condition | Promoted task |
|---|---|---|---|---|---|
| DF-001 | V2FRT-002 maintainability review | Registry temporary-file formatting in `crates/podway-daemon/src/registry.rs` and reset classification in `crates/podway-service/src/runtime_reset_v1/filesystem.rs` share a naming convention without a common helper. | Current producer and reader agree; changing this established writer is unnecessary for read-only reset planning. | Before changing the registry temporary-file naming convention, give its formatter and classifier a shared owner and verify reset inventory compatibility. | — |
| DF-002 | V2SDG-003 maintainability review | The `ItemTypeV1` wire-name mapping is repeated in the delegation package projector and daemon read projection. | Exhaustive Rust matches and current schema checks keep the seven names aligned; consolidating both consumers crosses this task's reviewed scope. | Before adding or renaming an item type, give its wire name one owner and verify both projections and schemas. | — |
| DF-003 | V2SDG-003 maintainability review | Delegation package static-budget reserves depend on bounded goal and evidence values owned elsewhere. | Current limits fit the reserves and the projection has an actual-byte backstop; exporting all bounds solely for this task would widen the public core API. | Before changing goal, selector, identifier, or item bounds, derive or assert the worst-case reserve against those limits and verify admission-time rejection. | — |

## Resolved

| ID | Source | Resolution | Evidence | Promoted task |
|---|---|---|---|---|

Use sequential `DF-###` identifiers. Move an entry to Resolved when it is fixed,
rejected with evidence, or promoted into the roadmap.
