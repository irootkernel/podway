# Frozen Dolgorae metadata contracts

This repository-only offline registry retains unmodified Git blob bytes from
Dolgorae commit `5fa36ad896a5bb417bd3ef14e1e4e96caa599d80`. `lock.json` binds
all files to their original paths, commit and SHA-256; its resource map includes
embedded `$id` resources with their JSON pointers. In particular,
`specialist-policy-input/v3` is embedded in `workspace-execution/v1`, rather than
being a separate file. Reference resolution must respect each resource's scope.

The roots are stateless version output v1, Machine output v4, capabilities v2
and error contract v4. The complete original reference closure contains 46 schema
files and 48 resources. The original capabilities example is a deterministic
positive control. `PROTOCOL_VERSION_UNSUPPORTED` and `UNSUPPORTED_SCHEMA_VERSION`
are the verifier's narrow unsupported-error mapping; both are declared by the
frozen error v1 contract and validated through its v4 successor.

Schema IDs are offline keys, never fetch locations. Missing, altered, ambiguous
or unresolved resources make verification incomplete. Do not alias versions,
rewrite source bytes or fall back to a neighboring checkout. The verifier uses
full JSON Schema validation, including embedded resources and UTF-8 byte bounds.

These resources belong to repository tooling. They are outside `assets/schemas`
and the public contract manifest's explicit asset inventory, and are not shipped
as Podway runtime schemas or added to the Dolgorae adapter catalog. Source
qualification still includes repository-owned `contracts/` files. This registry
proves no backend availability, execution, receipt or activation qualification.
