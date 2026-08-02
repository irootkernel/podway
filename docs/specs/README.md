# Specifications

These documents define the behavior Podway must satisfy. They cover the CLI and
daemon as one product and are grouped by concern:

- [Product](product/): goals, vocabulary, workflows, and supported boundaries.
- [Domain](domain/): procedures, items, transitions, rework, and presets.
- [Interfaces](interfaces/): CLI, JSON, IPC, automation, errors, and exit codes.
- [Storage](storage/): SQLite state, transactions, recovery, and retention.
- [Operations](operations/): trust, observability, installation, and packaging.
- [Quality](quality/): tests, acceptance criteria, and traceability.

Machine-readable counterparts live under [`assets/`](../../assets/) and
[`contracts/`](../../contracts/). Task status and implementation-order notes do
not belong in specifications; use the [roadmap](../roadmap/) or [TODO](../todo/).
