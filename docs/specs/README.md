# Specifications

These documents define the behavior Podway must satisfy. They cover the CLI and
its in-process Direct Engine as one product and are grouped by concern:

- [Product](product/): goals, vocabulary, workflows, and supported boundaries.
- [Domain](domain/): procedures, items, transitions, rework, and presets.
- [Interfaces](interfaces/): direct CLI, JSON, automation, errors, and exit codes.
- [Storage](storage/): SQLite state, transactions, recovery, and retention.
- [Operations](operations/): trust, observability, installation, packaging and legacy cutover.
- [Historical runtime reset](operations/runtime-reset.md): the retired account-runtime contract, retained for historical reference; it is not a current command surface.
- [Quality](quality/): tests, acceptance criteria, and traceability.

## Supervised delegation

- [Host-neutral supervised delegation](interfaces/supervised-delegation.md): the opt-in contract, including Direct-work compatibility, package/receipt identity, runtime gates, and separate integration/rollout boundaries. Native routes and explicit custom-Procedure admission are implemented in source; this entry does not claim an installed stable-release capability.

Machine-readable counterparts live under [`assets/`](../../assets/) and
[`contracts/`](../../contracts/). Task status and implementation-order notes do
not belong in specifications; use the [roadmap](../roadmap/) or [TODO](../todo/).
