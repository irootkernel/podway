# Documentation

## Placement

- Put component boundaries and data flow in `docs/architecture/`.
- Record durable cross-cutting decisions as ADRs.
- Put required final behavior in `docs/specs/`.
- Put practical development technique in `docs/implementation-tips/`.
- Put unadopted candidates in `docs/todo/` and small postponed review findings in
  `docs/deferred-feedback/`.
- Put adopted work and status only in `docs/roadmap/`.
- Put build-consumed data under `assets/`, never under `docs/`.

## Editing rules

- Write public and contributor Markdown in English.
- Link to the narrowest stable heading that supports a claim.
- Keep examples descriptive; machine assets and specifications remain normative.
- Preserve accepted ADR history and archived roadmap history.
- Update all affected links after moving or renaming a document.

Run `python3 tools/verify_docs.py` for fast feedback and the complete `make test`
gate before release acceptance.
