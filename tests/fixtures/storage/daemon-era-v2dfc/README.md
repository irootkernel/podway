# Frozen daemon-era classification recipes

`generation-1-to-13-ddl.json` freezes each exact cumulative migration input from
`assets/specifications/sqlite-v1.sql` through `sqlite-v13.sql` at
`e7f9bd268a765ae093799d516b89f2022a67ff6b`, with its original path and SHA-256.
Apply its ordered inputs only through the selected generation. The real CLI
classification tests retain both a checkpointed main header and a generation
found only in the committed WAL, across all thirteen generations. This rejected
input recipe survives deletion of the canonical daemon-era migrations and the
feasibility prototype; it does not define the current Store schema.

`generation-13-populated.sql` is a self-contained text dump of a generation-13
Store created by the unchanged legacy Rust producer at
`6e771f265391309733d9aef796056bcabac7f047`, before daemon/job retirement. The
V2DFC-005 exporter uses the existing `int_v2sdg004_receipt` fixture, admits and
claims a real delegation job, then commits its successor, immutable receipt and
terminal result through `SqliteStoreV1`. It contains one current session, one
completed job and one delegation receipt. No database image is tracked.

Replay the SQL with WAL autocheckpointing disabled. For a checkpointed header,
truncate-checkpoint before updating the workspace timestamp; for a WAL-only
header, retain the initial WAL. Copy the main file and WAL while that fixture
connection remains open, without copying SHM. The direct Store classifier must
reject either layout without changing bytes, inodes, names or marker files.

`markers.json` freezes the exact marker-presence payload used by the legacy
`V2DFC-001` G10 fixture producer. All four paths, alone and combined, are
classification inputs. Their payload is deliberately opaque to classification;
these recipes do not claim legacy marker-protocol validation or resumption.

The SQL/data recipe preserves historical queue and response shapes only as
rejected legacy input. It is not a second canonical DDL, a migration source, or
an executable contract for the direct Store. Keep it when the legacy producer
and canonical generation-1–13 assets are deleted.
