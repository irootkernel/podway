# Specifications

These documents define the behavior Podway must satisfy. They cover the CLI and
daemon as one product and are grouped by concern:

- [Product](product/): goals, vocabulary, workflows, and supported boundaries.
- [Domain](domain/): procedures, items, transitions, rework, and presets.
- [Interfaces](interfaces/): CLI, JSON, IPC, automation, errors, and exit codes.
- [Storage](storage/): SQLite state, transactions, recovery, and retention.
- [Operations](operations/): trust, observability, installation, packaging, and the Aquarium development service.
- [Runtime reset](operations/runtime-reset.md): read-only ordinary account-runtime planning and reserved retirement, preservation, coordination and exact recovery contracts.
- [Quality](quality/): tests, acceptance criteria, and traceability.

## Supervised delegation

- [Host-neutral supervised delegation](interfaces/supervised-delegation.md): the opt-in contract, including Direct-work compatibility, package/receipt identity, runtime gates, and separate integration/rollout boundaries. Native routes and explicit custom-Procedure admission are implemented in source; this entry does not claim an installed stable-release capability.

Machine-readable counterparts live under [`assets/`](../../assets/) and
[`contracts/`](../../contracts/). Task status and implementation-order notes do
not belong in specifications; use the [roadmap](../roadmap/) or [TODO](../todo/).
