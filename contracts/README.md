# Executable Repository Contracts

This directory contains versioned inputs and evidence consumed directly by Podway's
repository verification tools. It is intentionally separate from `docs/`, which is
the canonical home for human-readable product and contributor documentation.

- root JSON files define repository, receipt, and evidence schemas;
- `interfaces/` freezes internal interface contracts;
- `locks/` contains generated, content-addressed Phase 0 baselines;
- `handoffs/` contains generated receipts that bind producers and consumers;
- `evidence/` contains host-neutral, content-addressed attestations referenced by
  tracked receipts.

Raw logs, mutable reports, fuzz corpora, and machine-specific output belong under
ignored `artifacts/`. A tracked contract must never depend on those files. Publish
stable verification evidence explicitly with:

```sh
python3 tools/run_verification.py --attest
```

Regenerate locks and handoffs only through `tools/phase0_receipts.py`; do not edit
their digests manually.
