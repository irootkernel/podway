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

## L lifecycle costs

The same frozen CLI runner also measures `reset`, `archive` and `start` on the
retained API-grown L fixture with 20 warm-ups and 220 measured calls per command.
Every tuple restores the original L image; archive follows cancellation and an
explicit not-required disposition, and start follows archive. Only the named
command invocation is timed; preparation calls are retained and canonically
validated separately. `L_lifecycle` reports p50, p95, p99 and maximum. These
whole-invocation costs are informational: G7 registers no daemon comparator for
these three commands, and G8 limits open cost rather than total lifecycle time.
The registered G1/G2 transaction thresholds and G7/G8 limits remain unchanged.
Python optimization is refused before creating evidence or fixtures, and a G7
regression is recorded as `G7-failed` rather than inferred from G8 success.

## Internal-improvement comparisons

The [OPTIM, BOUND and READS roadmap](../roadmap/README.md#current-internal-improvements)
is separate from the historical qualification protocol above. On 2026-10-16 the
owner withdrew OPTIM latency, count, and memory measurement. OPTIM does not
freeze a current-Direct baseline, comparator, work-count ceiling, or memory
gate. Established G1-G10 qualification thresholds are unchanged historical
release evidence. A missing OPTIM baseline does not block OPTIM implementation
and is not inherited evidence for a later epic.

Accepted OPTIM evidence is the recorded implementation and functional
characterization: request-scoped read sharing, single-draft batch application,
and identity, overflow, rollback, and replay behavior. That evidence supplies
no performance baseline, comparator, or count ceiling.

### Separately registered improvement scope

BOUND and READS do not inherit OPTIM probe sets, baselines, comparators, or
count ceilings. A comparison either epic owns becomes acceptance evidence only
after that epic registers its fixtures, oracles, and owners in its own scope.
Accepted OPTIM evidence is not that registration.
BOUND-006 completed the existing complete native matrix below for boundary
qualification. That selection adds no OPTIM baseline, comparator or count ceiling.
Its accepted outcome is recorded in the
[BOUND delivery record](../roadmap/README.md#bound-delivery-record); READS scope remains planned.

### Native scale execution

A later epic that registers native qualification uses the existing Make entry
point, under AGENTS.md Gaori routing, with an absolute owned evidence directory:

```bash
make test-native-qualification NATIVE_EVIDENCE_PARENT=<absolute-owned-directory>
```

This invokes [the native qualification runner](../../tools/run_native_qualification.py).
Its existing matrix includes the three ignored scale cases:

- `podway-store`: `int_v2dfc005_scale::direct_multi_process_g1_g2_l_and_maximum`
- `podway-engine`: `int_v2dfc006_scale::engine_g1_g2_grown_l_300`
- `podway-engine`: `int_v2dfc006_scale::engine_g1_g2_grown_l_1024`

Require explicit execution receipts and preserve each case's current blocking
versus informational metric policy. Do not turn a maximum-graph informational
G2 result into a new absolute threshold. `make test` and ordinary `make test-int`
do not execute these `#[ignore]` cases. Running the native entry point also runs
its other registered cases; do not silently trim that matrix. This is native
qualification, not a claim of a new UTM run, distribution or installed activation.
It is not an OPTIM latency, count, or memory gate.

### READS registered unchanged-query baseline

READS-001 registers its own scoped data-access characterization; it extends no
OPTIM baseline. The counters live in `podway-store` `query_accounting`
behind the `test-support` feature: one prepare per statement compilation
(including the implicit prepare inside `Connection::query_row`), one execute
per `query_row`/`query_map`/statement `query` initiation, and one delivered
row per row-mapper invocation or successful streaming advance, including
rows whose conversion subsequently fails. A pending boundary row is counted
once when delivered, even though its TEXT conversion waits for its owner. Each
delivered row also counts its variable-length column bytes (TEXT/BLOB bytes
only; scalars and NULLs count zero). These are scoped measurements over the
registered read, preflight and prepared-admission call sites in `v2_state`,
`v2_memory`, `v2_goal`, `v2_delegation` and the direct-store read methods —
never a total SQL counter. Uncovered by registration: open-time schema
inventory and admission PRAGMAs, integrity verification, maintenance,
predecessor and mutation-transaction reads.

Logical retention units (not allocator or heap claims) cover the item-slot
family: per-attempt raw serialized payload bytes held from row acquisition to
consumption (peak and window peak), fresh decoded payload bytes (cumulative
and held by reconstructed results), measured cached-input payload bytes for
prepared-graph revalidation, payload pointer-identity reuse versus fresh
decode, container clones, and the combined simultaneous peak of raw plus
decoded plus cached input. Failed scopes and failed whole-graph
reconstructions release their bytes at scope end and are reported exactly.

Registered fixture recipes (newly chosen for READS Store-level registration;
CLI qualification keeps its own G7/G8 recipes). Each recipe is one linear
graph alternating work and goal-assessment decision nodes; every completed
decision attempt records a decision, criterion results and a goal
assessment, and the two-revision recipes end with a terminal-reactivation
rework repeat attempt, so the attempt count is one above the node count:

| Fixture | Nodes | Attempts | Items/attempt | Slot rows | Goal revisions | Assessment owners | Criterion results | Citation rows | Blockers | Evidence rows |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| S | 5 | 6 | 2 | 12 | 2 | 2 | 4 | 6 | 1 | 1 |
| M | 24 | 25 | 8 | 200 | 2 | 11 | 44 | 48 | 6 | 1 |
| grown-L | 40 | 41 | 16 | 656 | 2 | 19 | 57 | 60 | 4 | 1 |
| maximum | 1,024 | 1,025 | 1 | 1,025 | 2 | 511 | 1,022 | 1,024 | 8 | 1 |
| empty | 1 | 1 | 1 (unset) | 1 | 1 | 0 | 0 | 0 | 0 | 0 |
| single | 1 | 1 | 2 | 2 | 1 | 0 | 0 | 0 | 0 | 0 |

Actual unchanged-query baselines, measured as one cold `current_graph` read
per fixture. The instrumented build ran from the READS-001 working tree at
base `052f8625f9c5944b90faf3c907e9a385d133be1c` with the observers as
uncommitted working-tree changes (a truthful dirty-source snapshot, not a
claim that a pristine commit produced it); the separately frozen pristine
baseline CLI is SHA-256
`28e52cc7be6f228bd862f716166121f6881ddbb8eec16643802eccaf434f7344`
(provenance file records base `052f8625`, accepted BOUND `036e86ed`):

| Fixture | Prepares | Executes | Delivered rows | TEXT/BLOB column bytes | Fresh payload bytes | Raw window peak | Combined peak |
|---|---:|---:|---:|---:|---:|---:|---:|
| S | 43 | 43 | 65 | 6,819 | 792 | 132 | 858 |
| M | 140 | 140 | 438 | 231,250 | 205,200 | 8,208 | 206,226 |
| grown-L | 201 | 201 | 995 | 36,141,030 | 36,097,056 | 880,416 | 36,152,082 |
| maximum | 4,118 | 4,118 | 8,215 | 822,518 | 51,250 | 50 | 51,300 |
| empty | 23 | 23 | 16 | 2,089 | 0 | 0 | 0 |
| single | 23 | 23 | 17 | 2,421 | 72 | 72 | 108 |

The grown-L database reached 36,642,816 bytes (registered floor 16 MiB).
Fresh payload bytes equal the sum of canonical encoded slot sizes on every
recipe. Prepared selective reload on M (25 attempts, 200 slots): unchanged
0 header executes and 0 point reloads (pure stamp reuse); sparse 25 header
executes, 1 point reload, 199 pointer-proven payload reuses; dense 25, 8,
192, with cached-input bytes measured from the live prepared graph before
the window and included in the combined peak.

Future-code predicates registered for READS-002 (never asserted against the
unchanged path): changed-slot reload batches of at most 128 identity pairs
(≤256 identity bind parameters, other parameters accounted), changed-only
reload executions ≤ ceil(changed/128), strict execution decrease against the
registered baselines on repeated-family fixtures, small/empty/single
controls with no unnecessary increase under a three-pair no-worse policy,
and prepare counts asserted separately.

### READS cold family acquisition

The Store acquires slots, blockers, evidence references, goal criteria and
criterion citations with one lazy statement and execution per family when its
parent collection is nonempty. Statement and cursor ownership remains local to
one reconstruction. Each existing validator consumes its owner's rows in the
original order; boundary lookahead converts integer routing keys only. Criteria
are restricted to existing parent revisions, while the terminal cross-count
still detects orphans after assessment validation. Results and citations share
the full `(trace_sequence, criterion_ordinal, result rowid)` key so corrupt ties
cannot merge citation groups.

The registered cold fixtures now measure these prepare/execute pairs:

| Fixture | Original | Bulk | Delivered rows | Combined payload peak |
|---|---:|---:|---:|---:|
| S | 43 / 43 | 24 / 24 | 65 | 858 |
| M | 140 / 140 | 24 / 24 | 438 | 206,226 |
| grown-L | 201 / 201 | 24 / 24 | 995 | 36,152,082 |
| maximum | 4,118 / 4,118 | 24 / 24 | 8,215 | 51,300 |
| empty | 23 / 23 | 23 / 23 | 16 | 0 |
| single | 23 / 23 | 23 / 23 | 17 | 108 |

Fresh payload bytes and raw peaks match the original table exactly. Variable
column bytes also match for equal inputs: the original hex-encoded fixture root
occupied 154 bytes; tests add the current root field's exact length to the
remaining original bytes. For example, moving fixtures to the owned temporary
directory added 44 root bytes, yielding 6,863 total bytes on S. The observer
reports those actual bytes without subtraction or normalization.

`SQLITE_TOOBIG` can arise inside a bulk step before the failing row has an
identifiable owner, including when a sorter scans a later owner's oversized
cell. That family retires its cursor, discards the current unconfirmed
collection and uses the original owner selectors through the same validation
callbacks. The original error winner and validator stages stay intact; extra
corrupt-input executions and reacquired rows remain counted. No length limit,
error mapper, transaction or deadline changes. The real Store EXPLAIN check
pins cold slots to a plan without payload repacking; the other four families'
compatible-writer field bounds remain well below the configured row limit.

Additional tied-result multi-fault characterization used the original per-owner
loaders and then the byte-exact bulk implementation with identical fixtures.
These additional baselines were captured retrospectively after the first cold
candidate; they do not satisfy the planned pre-change capture timing. The
comparison preserves defined validation stages and routing without making
SQLite's undefined relative order among corrupt ties a contract. Both loaders
collect two citation rows before an ordinal-gap check when the second row has
invalid UTF-8. With valid citations, a tied result's postcheck still precedes the
next group's conversion; a strictly earlier postcheck also precedes invalid
first-row TEXT in future tied groups. Bulk scalar lookahead honestly adds one
acquired row in the latter cases, without changing the validator's pull/post
entries.

### READS prepared reload acquisition

A mixed reload discovers slot headers once for the session. Valid owners retain
scalar stamps; changed cached slots queue pairs in specification order, and new
owners queue all rows in collection order. Unchanged cached values keep their
prepared payload pointers. One global pair list packs identities across owners
into fixed 128-pair statements, with exactly 256 identity binds, zero other binds
and NULL tail padding. Prepares and executions are asserted separately.

| Registered reload | Original headers / cold / points | Current headers / chunks / points |
|---|---:|---:|
| 128 revised + 128 new | 2 / 1 / 128 | 1 / 2 / 0 |
| 2 revised + 4 new | 2 / 1 / 2 | 1 / 1 / 0 |
| Larger-first encoded values (100, 40) | 2 / 0 / 2 | 1 / 1 / 0 |

Fresh payloads, container copies, pointer reuse and logical raw/combined peaks
retain their registered oracles. The heterogeneous combined peak remains
exactly cached-input bytes plus 200. The small nonaligned case proves a single
chunk spans two owners. Pure unchanged prepared reuse still executes neither
header discovery nor payload chunks.

The chunk statement orders `(ordinal, attempt UUID, rowid)` metadata in a
`LIMIT -1` subquery, then uses a `CROSS JOIN` with both rowid and attempt equality
to seek each slot. Explicit outer ordering preserves requested positions. On
pinned SQLite 3.51.3, the real Store EXPLAIN test pins one four-field metadata
record, ordering before rowid seek, payload columns after that seek, and no
payload sorter or automatic index. The same connection proves query-only mode,
the unchanged 1,064,960-byte length limit and 256 parameters. A compatible list
is grown from empty by 125 acknowledged `Add` operations to a single
1,000,000-scalar value, then reloaded cold and mixed. This complements bounded
large-payload fixtures without substituting an oversized text-domain value.

Metadata remains separate from payload retention measurements. On the pinned
64-bit target, each production row stamp is 32 bytes and each pair is 16 bytes.
For N valid headers and M queued pairs, routing retains N stamps, M pairs,
ceil(M/128) lazy statement cells, at most that many prepared handles and one live
chunk cursor. Per-owner header staging/classification maps and duplicate guards
are transient; only the first failing owner's raw header prefix remains for
exact-position replay. Discovery scratch has its own balance, and the combined
identity-text peak includes scratch retained across older owners' scope exits.
These logical units exclude collection capacity, allocator overhead and SQLite
native metadata buffers; they are not heap or RSS measurements. A SQL-only
allocator diagnostic found bounded metadata staging instead of the rejected
payload-sorter proposal; it does not establish total Store memory equivalence.

Any chunk-step TooBig retires the entire remaining stream. Cached owners keep
already validated slots and use original point selectors at remaining changed
positions. A new owner drops its partial raw collection and repeats its original
whole-owner collection before specification checks. Real Store faults before
and after the 128-pair boundary assert delivered prefixes, conversion stages,
no later chunk, unchanged database/WAL bytes and no rejected outcome. Missing
rows visible during discovery refuse at the original specification position;
that test does not pretend a queued row can disappear inside one snapshot.
Deferred header faults and early errors release retained discovery scratch on
unwind, including when an earlier owner's payload is the winning fault.

### READS projection copy elimination

READS-003 removes the surviving owned copies inside Engine read projections
without touching validation or output contracts. Extraction sites that cloned
an object map back out of a constructed `Value` (delegation package and
receipt results, prepared-next, the direct archive-show result, the
archive-list result) now move the map out or build it directly, and the
owned archive/disposition/status members, history entries, archive-list
summaries and observation list strings move into their final results instead
of being reserialized: the `json!` macro finalizes every expression through
`to_value`, which allocates a fresh copy even for owned inputs. The evidence
response bound measures the result map directly with the same
length, `usize::MAX` fallback and saturating entry-overhead arithmetic; a
borrowed `HistoryWindowMeasure` carrier charges trial history windows the
same bytes the owned construction produces. Observation list projection and
the history window measure each candidate in place (push, measure, pop),
keeping the admitted prefix, `false`-spelling budget and the trailing-error
lookahead that only flags truncation.

These changes eliminate the per-candidate and per-entry clone work and the
extraction copies; they do not make projection linear-time, because every
trial still serializes the whole growing prefix and the encoded byte buffers
still allocate. Borrowed members (delegation package and receipt values,
receipt summaries) are still reserialized once under the present interfaces,
and the outcome-lookup envelope plus cursor/begin mutation result builders
are outside the eight read variants and unchanged. Store graph
reconstruction stays eager; `ReadScope` still admits and memoizes the
validated Procedure lazily over that Store-validated state. In-crate unit
tests pin borrowed-versus-owned byte identity, exact-fit and one-byte-over
budget boundaries, the swallowed trailing error after a full window, and
pointer identity of the moved owned strings and entries.

### READS typed fence-preflight facts

READS-004 replaces the CLI's automatic fence-preflight boundary: instead of
executing a Standard `session.status` read and parsing its public envelope, the
CLI calls an Engine-owned typed facts operation
(`execute_direct_preflight_facts_v1`) that mirrors the original admission and
validation order — workspace resolution, Store read admission with the same
lease/gate/transaction lifetimes, one clock read, the eager current view, the
expected-session fence, and the complete shared Standard projection with its
schema validation. The internal Standard result map serves validation only: it
is retained through the exact envelope-equivalent Stage 5 checks and dropped
before owned typed facts return. Those Stage 5 checks reproduce the original
envelope's fallible conditions in their original order — the generated-at
clock formatting, the workspace constructor bound, the document depth and byte
limits (each variable member serialized individually, the actual workspace
sequence digits, and one pinned framing constant), and the absolute-root
pattern — without constructing, parsing, or reconstructing a public envelope.
Facts themselves derive directly from typed validated state.

Equivalence evidence: C1 component checks pin the framing constant, depth
counting and byte accounting against real serialization and the real protocol
validator (schema-valid synthetic oracle maps are labeled as non-Core-admitted
and not production-window reachable), and compare the extracted production
Stage 5 sequence with the original envelope constructor across clock, root and
multi-fault variants with exact `ProtocolError` variants and attribution.
Engine integration tests prove byte-identical public failures for the real
reachable classes (corrupt Store admission, persisted timestamp corruption,
out-of-range clock, wrong expected session), field-identical facts on success,
and the post-facts guard release for both shared and cold lease paths with a
real competing writer committing inside the gap. CLI stdin commands carry
process-local debug counters proving zero facts/status preflight activity;
a genuine automatic preflight and an explicit status read provide positive
controls, and a release-profile candidate emits no instrumentation.

Honest residual cost: the eager Standard projection and its schema validation,
plus the component serializations feeding the exact byte check, remain on the
preflight path. No latency, RSS or zero-copy claim is made here; READS-005
owns matched comparison. The other status consumers keep their existing paths
unchanged: dynamic completion's separate best-effort query, the later
interactive-start status read after `SESSION_START_DECISION_REQUIRED`, and
the stateful replacement/reset disposition observations that do read status —
while the offline fresh-start preview makes no status read at all, as
[System Architecture](../architecture/system.md) keeps distinct.

### READS registered latency comparison protocol

READS freezes its matched comparison before any optimization. Comparisons
run on the same host and fixtures; 20 warm-up plus 220 measured calls per
cell; three predetermined paired blocks alternating candidate/BOUND order;
per-cell median-block p50 and, separately, p99 candidate ≤ BOUND; the
original historical G7/G8 thresholds and failure/stop policies apply
independently to every applicable block; no +10% margin, pooling, posthoc
tolerance or selected reruns; explicit and automatic-fence diagnostic cells
are both required because G7 explicit fences alone cannot prove preflight
gains; G7 retains its history-growth recipe and G8 its exact mode-tuple
restoration and replay/upper-bound classifications. READS-001 registers the
protocol and the frozen baseline CLI; READS-005 executes it. Nothing is
measured as latency in READS-001.

### Evidence and completion boundaries

Functional checks validate response, state, and replay identity. Use
deterministic clocks and IDs for semantic comparisons; do not normalize exact
stored replay responses or discard error-order changes. Current Stores continue
in owned fixtures without reset or migration. Unsupported, newer, and foreign
Stores still fail closed. That continuation is functional evidence, not an
inherited OPTIM metric.

When a later epic registers its own comparison, keep raw samples, fixture
copies, profiler output, and frozen binaries under ignored owned evidence roots
according to AGENTS.md. Durable documentation retains source identity,
methodology, selected cells, verdicts, and material limitations, not private
runtime paths. Never bind executable tests to a temporary TODO dossier.
Development-gate success, a benchmark pass, Epic acceptance, and a stable
release are separate claims. No comparison grants commit, publication,
installation, runtime retirement, or external-consumer writes.
