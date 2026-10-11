# Risk Register

The IDs below preserve the established risk inventory. A row marked retired is
historical: ADR-0038 removed its owning infrastructure, rather than qualifying the
old runtime behavior for the direct product.

| ID | Risk | Likelihood | Impact | Trigger or signal | Mitigation | Primary owner |
|---|---|---:|---:|---|---|---|
| `R-01` | Crash or response loss creates duplicate direct effects | medium | critical | cold retry creates a second revision or attempt | atomic state/success outcome, exact-key replay, DS/DE crash proofs | Store/Engine |
| `R-02` | Stale prepared input mutates a later graph node | medium | critical | command succeeds after current attempt changes | transaction-time attempt/revision fences; no automatic retargeting | Domain/Engine |
| `R-03` | Concurrent item edits silently overwrite | medium | high | last writer wins without conflict | persistent item revisions and compare-and-set | Domain/Store |
| `R-04` | Path or replacement race escapes workspace | low | critical | source, artifact or runtime opens outside owning root | descriptor-safe containment, nofollow opens, native adversarial tests | Git/Engine |
| `R-05` | Retired: daemon restart leaves acknowledged jobs idle | retired | historical | no current job or restart-recovery path exists | ADR-0038 removes durable admission; current crash recovery belongs to R-01/R-22 | Architecture |
| `R-06` | Worktree move or copied runtime creates wrong ownership | medium | high | Store binding differs from canonical Git identity | role-tagged canonical path identity, native revalidation, explicit rebind/reset | Git/Engine |
| `R-07` | Retired: LaunchAgent keep-alive conflicts with stop/upgrade | retired | historical | no current LaunchAgent owner exists | old installation requires separately authorized retirement through its original binary/controller | Operator |
| `R-08` | Procedure authoring becomes too complex | medium | high | users bypass progression or use only confirmations | bounded data-only items, validated presets, generated next-command recipes | Product/CLI |
| `R-09` | Product drifts into evidence archive or attestation | medium | high | export/revocation features enter critical path | non-goals and structural-assertion trust model | Architecture |
| `R-10` | Large artifact preparation delays commands | medium | medium | preparation dominates latency | streaming hash outside state transaction, native race checks, measured cost | Engine/Git |
| `R-11` | Required local artifact changes after attach | high | medium | completion uses stale metadata | outside-transaction completion hash with stable descriptor stamps; no-byte stamp and slot/worktree revalidation inside the transaction | Domain/Engine |
| `R-12` | Malformed YAML/JSON exhausts resources or executes code | low | high | alias expansion, tags, excessive depth or aggregate input | strict parser and typed Direct API bounds, duplicate-key rejection, no includes/hooks | Config/Protocol |
| `R-13` | Public assets, consumer contract or writer identity drift | medium | high | decoder failure or foreign Store write | closed schema generations, manifest asset closure, writer refusal, verifier tests | Protocol/Store |
| `R-14` | Retired: service qualification depends on launchd availability | retired | historical | no current service test contract exists | direct native scenarios qualify the actual packaged CLI in scratch worktrees | Release/QA |
| `R-15` | SQLite and domain invariants diverge | medium | high | doctor finds corrupt graph/outcome/schema | canonical v15 DDL, transaction validation and explicit integrity checks | Store/Domain |
| `R-16` | Retired: global registry accumulates task content | retired | historical | no current account registry exists | no per-user Podway state; task state stays worktree-local | Architecture |
| `R-17` | Diagnostics expose task or credential content | medium | medium | item text appears in journal or raw infrastructure output | fixed journal summaries, bounded public error details, no payload log sink | Engine/Security |
| `R-18` | Destructive reset/removal is accidental | low | high | runtime lost without informed intent | current lifecycle eligibility, confirmation, UUID fences and exact recovery guidance | CLI/Engine |
| `R-19` | Non-native or debug-hook-enabled executable is distributed | medium | high | relabeled/cross-built binary or live release test hook | native Apple Silicon gate, architecture inspection, exact packaged scenarios and disabled-hook probe | Release |
| `R-20` | Same-user reliability is mistaken for authentication | medium | medium | users rely on receipts/digests as audit proof | explicit trust limits, no access key or cryptographic authority claim | Product/Security |
| `R-21` | Next-command recipes are unsafe to copy | medium | high | wrong fences, key or undeclared movement | structured argv with caller placeholders and real-CLI recipe tests | CLI/Product |
| `R-22` | Interrupted reset-all/removal loses recoverable intent | low | high | missing original response or abandoned markers | native exclusion, exact typed marker retry, D03/DE05/DE06 and real CLI interruption tests | Engine/Store |
| `R-23` | Supply-chain or licensing defect blocks release | low | high | dependency review or reproducibility checks fail | lockfile, narrow dependencies, `make test-prepare`, truthful signing status | Release |
| `R-24` | Overengineering delays bounded completion | medium | high | speculative frameworks or unadopted capabilities expand scope | existing crate boundaries, staged roadmap ownership and explicit adoption | Project lead |

## Risk review cadence

Review current critical/high risks at integration milestones. A triggered defect
requires a regression matched to its actual boundary. Closing a current risk
requires passing evidence or an accepted design change; existence of a test is
not a passing claim. Historical retirement does not authorize stopping or
removing an installed service. Performance, release qualification and external
consumer adoption remain distinct from development-gate success.
