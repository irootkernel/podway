# Direct CLI Performance Qualification

## Authority and evidence boundary

[ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md) owns the
accepted direct model. [Testing and Conformance](../specs/quality/testing-and-conformance.md)
owns its verification requirements. This guide preserves the historical
feasibility comparator and matched reproduction method after the temporary
V2DFC dossier is retired. It is not a report that a present candidate passed.
Current results must identify the actual release CLI and helper bytes, source
revision, host, controlled fixtures, measured samples and cleanup separately.

The `V2DFC-001` prototype gate was measured on October 5, 2026 UTC, on an Apple
M5 Pro (18 logical CPUs, 64 GiB), macOS 27.0.1 (26A434), AC power, Low Power Mode
off, release profile and Rust 1.97.1 with bundled SQLite. Other host work shared
those runs; recorded load rose from about 4 to 8–10. Base was `32082fe` plus the
`V2DFC-001` prototype. The final G1/G2/G4/G5/G9 prototype differed from the first
G3/G6/G7/G8/G10/E1/E4 candidate only in other harnesses and maintenance-missing
error handling that those latter checks did not exercise. Logs are local
provenance; this guide preserves measured values, not private runtime identities.

Times below are milliseconds as **p50 / p95 / p99 / maximum**, using nearest-rank
percentiles with warm-up excluded. A historical pass proves feasibility, not
current implementation, distribution readiness, or installed-runtime retirement.

## Original feasibility and adoption requirements

| ID | Check | Method | Pass threshold |
|---|---|---|---|
| G1 | Multi-process correctness | 8 OS processes × 1,000 fenced mutations plus 2 observers; same-key retries; 5 s busy timeout | 0 lost or duplicated effects (final state equals serial replay of the outcome log); 0 stale-fence commits; 0 `SQLITE_BUSY` reaching a caller; observers never decode an invariant violation |
| G2 | Wait and fairness | Instrument the IMMEDIATE wait and the lock hold during G1 | Wait p99 ≤ 250 ms and maximum ≤ 1 s; hold p99 ≤ 50 ms; the slowest process finishes within 2× the fastest |
| G3 | Over-deadline behavior | A controller holds the write lock past a 500 ms deadline | A structured busy error within the deadline plus 100 ms; no partial effect and no outcome record |
| G4 | Crash boundaries | `SIGKILL` at the five registered boundaries below, at least 20 each, plus 200 random kills; then lookup and same-key retry | The cheap open succeeds after every kill; each key has either one effect with a record and replay, or no effect with not-found and exactly one apply on retry; a final `integrity_check` passes |
| G5 | Maintenance exclusion | 100 reset-all and removal cycles with a kill at each marker step, against 4 writers and 4 readers | 0 acknowledged commits into an unlinked or superseded file; contenders see only success or the maintenance error; interrupted maintenance completes exactly once; exclusive acquisition p99 ≤ 2 s |
| G6 | Preparation races | 1,000 state or artifact changes between preparation and commit | 100% rejected; 0 stale commits |
| G7 | Full-invocation latency | `observe`, `set`, `complete`, and `record --stdin` with 16 items on S, M, and L, daemon path versus direct path | Direct p50 and p99 no worse than the daemon's for every command and fixture |
| G8 | Open cost | Read-only and mutation opens on L, warm (SHM present) and cold (hot WAL, no SHM) | Warm p99 ≤ 100 ms; cold p99 ≤ 1 s |
| G9 | WAL bound | 10-minute soak with 8 writers and 4 readers | WAL ≤ 64 MiB throughout and ≤ 4 MiB within 5 s after load stops |
| G10 | Write-free legacy classification | Generation 1, 4, 10, and 13 fixtures with a hot WAL and markers | A structured legacy result; the SHA-256 of every file is unchanged |

The original methods used native OS processes, real SQLite and supported
commands. G1/G2/G5/G9 started a fresh 300-node linear session with one reused
definition with 16 optional text items (`max_length: 65536`); G1/G5 wrote
64 characters, G9 wrote 4,096. G1 mixed set/record/complete 78:20:2, replayed every success with the
same key and retried stale fences with a new key. G2 measured every IMMEDIATE
attempt, including commits/replays/rejections; hold ended at COMMIT or returned
rollback. Fresh-session fairness is not an L-scale or 1,024-node performance proof.

G3 held the write lock until after each 500 ms deadline attempt returned. G4 used
an 8-node chain, 20 kills at each of the five boundaries (before BEGIN, after BEGIN,
after changed state before outcome, after COMMIT before output, and maintenance
marker publication/deletion), plus 200 kills that actually terminated a live
writer. Random delays spanned the median writer lifetime. G5 performed 100 cycles
of reset and removal/re-creation with a kill at each operation marker step and
four writers/four readers. It must include the final marker/lock-unlink windows.
G6 used a 3-node chain with a local artifact and 1,000 preparation races.

The owner accepted two limits after measurement: while a maintenance marker
remains, a cheap open may refuse with `WORKSPACE_MAINTENANCE` instead of succeeding
(G4); after completed removal and before re-creation, `WORKSPACE_NOT_INITIALIZED`
is unavoidable (G5). The historical contender log did not identify every
response's exact phase, so that G5 explanation was code-derived, not a stronger
measured boundary proof. Production regression tests independently cover the
corresponding transitions. Never silently erase these post-measurement limits
or call the original raw methods unchanged.

G9 held one idle SQLite connection throughout the 10-minute run, ruling out WAL
shrinkage solely by last-close deletion. Eight writers/four readers ran; each
writer owed a completion every 192nd attempt up to 36 and retried until it landed.
G10 covered generations 1/4/10/13, each in the database header and committed WAL
page 1, each legacy marker alone, and generation 13 with all markers: 13 cases.
The classifier must use the last checksum-valid committed WAL page 1 without
writing; 512/1,024-byte page compatibility and partial/uncommitted tails need their
permanent regression coverage.

E1 recorded invocation phases and result-validator compilation without a pass
threshold; the prototype compiled validators for roughly 63 ms, or 43–68% of L cost,
justifying lazy per-schema compilation. E2 inventoried test port/retirement;
E3 checked that retained behavior needs no resident process; E4 measured full
integrity/quick-check cost on L, justifying explicit doctor/maintenance checks.
The 1,024-node prototype run was information-only (wait p99 392.0 ms, hold p99 34.4 ms,
slowest/fastest 1.11); it did not satisfy the fresh-session 250 ms wait threshold.
Production L-scale and 1,024-node changed-row validation require their own retained
Engine/Store qualification evidence rather than that fresh-session baseline.

## G7 historical comparator

The four-command iteration is `observe`, `set` of 4,096 characters,
`record --stdin` of 16 × 4,096 characters, then `complete` advancing the chain.
Mutations carry original explicit fences and caller-owned keys. Each fixture
block uses 20 warm-up plus 220 measured iterations and retains growing history
throughout the block; restoring a fresh session for each command would change
the comparator. The original daemon path included its shipped session-status
preflight for set/complete. It used an isolated release-qualification daemon,
not the installed product or per-call managed binary hashing.

Fixtures are built with supported commands using the 300-node Procedure above:
S begins at its first node. M fills the first node with six batches of 16 × 55,000-character
record batches and completes it. L repeats that recipe until its main database
reaches at least 43,384,832 bytes; the historical baseline reached 45,150,208 bytes
after seven nodes. Record actual new-generation sizes and filled nodes: a byte
layout difference does not authorize reducing the workload or size floor.

Original block order was daemon S/M/L then direct S/M/L. Each path restored its
original fixture snapshot at the same path before its block. History grew within
each block; first-to-last 24 iteration means rose from 2.1 to 10.5 s on daemon S and
10.8 to 19.4 s on daemon L, versus 0.35 to 0.58 s and 0.41 to 0.64 s on the prototype. A later
managed-mode S control ran on the grown Store and was information-only.

| Fixture | Command | Daemon | Direct |
|---|---|---|---|
| S | `observe` | 684.4 / 1,056.5 / 1,088.8 / 1,092.7 | 90.6 / 106.2 / 112.0 / 118.9 |
| S | `set` | 2,163.7 / 3,405.2 / 3,518.2 / 3,525.0 | 113.6 / 142.4 / 150.7 / 154.1 |
| S | `record --stdin` | 1,659.3 / 2,616.8 / 2,669.5 / 2,697.0 | 133.4 / 169.9 / 184.7 / 202.4 |
| S | `complete` | 2,210.2 / 3,494.5 / 3,609.9 / 3,689.3 | 133.7 / 171.9 / 194.4 / 208.3 |
| M | `observe` | 806.7 / 1,198.0 / 1,290.0 / 1,603.6 | 94.2 / 106.0 / 109.5 / 119.8 |
| M | `set` | 2,605.7 / 3,863.6 / 4,027.0 / 6,445.3 | 119.4 / 142.3 / 147.9 / 162.9 |
| M | `record --stdin` | 1,961.8 / 2,945.5 / 3,545.5 / 3,914.9 | 137.1 / 167.8 / 183.0 / 205.0 |
| M | `complete` | 2,631.0 / 3,906.6 / 4,422.4 / 4,743.9 | 139.6 / 174.1 / 183.4 / 248.5 |
| L | `observe` | 1,562.9 / 1,929.4 / 1,977.0 / 2,093.9 | 98.4 / 111.1 / 121.0 / 127.1 |
| L | `set` | 5,075.6 / 6,357.6 / 7,659.0 / 8,435.0 | 130.9 / 157.1 / 165.1 / 207.7 |
| L | `record --stdin` | 3,808.6 / 4,832.0 / 5,194.8 / 5,615.7 | 153.3 / 183.5 / 188.9 / 196.2 |
| L | `complete` | 5,121.5 / 6,512.4 / 7,054.9 / 7,747.9 | 156.8 / 192.6 / 203.4 / 222.6 |

Qualification compares every current direct cell's p50 and p99 to the recorded
daemon column. All 12 cells must be no worse. The recorded prototype column is
historical context, not the current direct target or an alternative threshold.
Do not execute the removed daemon or reinstall an old product to reproduce it.

## G8 historical open comparator and matching protocol

The original G8 reported real isolated open costs, including lock, marker,
schema and writer checks. It followed G7 on S/M. For L, restore the original L
fixture, run E1's 20 warm-up plus 200 measured four-command iterations, then construct
the paired hot-WAL fixture. The historical L main database was 78,041,088 bytes;
each paired WAL was about 4.2 MB. Do not use the L Store grown by the 240 G7 iterations
as E1's starting state.

| Fixture | Database bytes | Read-only warm | Mutation warm | Read-only cold | Mutation cold |
|---|---:|---|---|---|---|
| S | 37,335,040 | 0.29 / 0.34 / 0.41 / 0.45 | 0.29 / 0.33 / 0.38 / 0.41 | 1.33 / 1.50 / 1.69 / 1.83 | 1.37 / 1.55 / 1.66 / 1.83 |
| M | 43,581,440 | 0.29 / 0.34 / 0.43 / 0.46 | 0.30 / 0.38 / 0.46 / 0.60 | 1.38 / 5.29 / 6.00 / 6.32 | 1.33 / 1.53 / 1.64 / 2.42 |
| L | 78,041,088 | 0.29 / 0.32 / 0.39 / 0.41 | 0.29 / 0.35 / 0.42 / 0.45 | 1.40 / 1.73 / 2.07 / 2.42 | 1.40 / 1.53 / 1.79 / 2.24 |

For the real CLI, retain one genuine warm tuple with SHM present and one genuine
cold tuple with hot committed WAL but no SHM, preserving their original database,
WAL, configuration, Procedure and matching replay identity. Construct hot WAL
through supported CLI writes while an owned helper pins a read snapshot, then
SIGKILL/reap the live owned reader/writer; prove an acknowledged outcome is absent
from the main-only immutable view but present in the coherent Store. This is
fixture proof, not a COMMIT-I/O-unknown injection.

Run each warm/cold read/mutation cell with 20 warm-up plus 220 measured samples,
restoring its exact original tuple before **each** sample with no live owned
fixture process. Current read endpoint is `workspace.show`; mutation endpoint
is the exact original `workspace.repair` replay. It must return the identical
stored envelope, UUID and result; no newer key/fence or mutation growth is
substituted. DB/WAL/configuration/Procedure hashes remain equal to the registered
mode preimage; record volatile SHM before/after separately. Never claim SHM bytes
are immutable across SQLite opens.

The current helper measures the complete CLI invocation as a conservative upper
bound on open cost. L warm p99 must be ≤ 100 ms and L cold p99 must be ≤ 1,000 ms. Passing that
upper bound proves the open threshold. Exceeding it is
`unverified-by-upper-bound`, not proof that isolated open itself failed and not
permission to relax thresholds. S/M open cells remain informational. Validate
every successful workload response through the shipped canonical validator.

## Running current qualification

Use the repository's pinned Rust toolchain, native Apple Silicon on AC power,
and release profile. Build the current CLI and Engine integration-suite helper
with `--locked`. The fixture-only helper test is
`int_v2dfc007_cli_qualification::qualified_reader_and_output_validator`; its
historical work ID does not restrict measurement to an older executable.

```bash
cargo build --release --locked -p podway-cli --bin podway
cargo test --release --locked -p podway-engine --test int_suite --no-run
python3 tools/qualify_direct_cli.py \
  --cli <absolute-release-podway-path> \
  --helper <absolute-release-engine-int-suite-path> \
  --evidence-parent <absolute-owned-scratch-directory>
```

Resolve the helper from that exact Cargo build output, not a newest-file glob.
For agent work, use the configured Gaori transport and owned scratch policy in
AGENTS.md. The helper freezes executable copies and records their SHA-256,
version identity, source HEAD/status/diff/untracked source, host/OS/power/load,
fixture recipes, registered mode tuples, per-call argv/input/output/exit status,
nearest-rank sample summaries and all response-validation results. These are
performance provenance, not Orca review target captures.

An incomplete setup is not a passing sample. G7 threshold failure must be
reported without automatic rerun; follow the owner's stop decision. Do not run
concurrently with another source-mutating check or change the candidate during
measurement. Success requires valid full reports, all 12 G7 cells, all four required
L G8 cells, canonical output validation, unchanged frozen executable hashes and
zero owned child processes. Cleanup is required after failure/interruption too;
retained owned fixture files are private local evidence, not installed state or
tracked promoted evidence. No test here installs software, starts a daemon,
uses account state, proves a stable release, or establishes downstream adoption.
