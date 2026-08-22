PRAGMA legacy_alter_table = ON;

ALTER TABLE v2_resolved_evidence_references RENAME TO v2_resolved_evidence_references_v8;

CREATE TABLE v2_resolved_evidence_references (
    attempt_id                  TEXT NOT NULL REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    source_graph_node_id        TEXT NOT NULL,
    reference_ordinal           INTEGER NOT NULL CHECK (reference_ordinal >= 0),
    required                    INTEGER NOT NULL CHECK (required IN (0, 1)),
    selected_item_ids_json      TEXT NOT NULL CHECK (json_valid(selected_item_ids_json)),
    state                       TEXT NOT NULL CHECK (state IN ('resolved', 'skipped', 'unresolved')),
    source_attempt_id           TEXT REFERENCES v2_attempts(attempt_id) ON DELETE RESTRICT,
    source_attempt_number       INTEGER CHECK (source_attempt_number IS NULL OR source_attempt_number >= 1),
    items_digest                TEXT CHECK (items_digest IS NULL OR items_digest GLOB 'sha256:[0-9a-f]*'),
    resolved_at_ms              INTEGER,
    PRIMARY KEY (attempt_id, reference_ordinal),
    CHECK (
      (state IN ('resolved', 'skipped') AND source_attempt_id IS NOT NULL AND source_attempt_number IS NOT NULL AND items_digest IS NOT NULL AND resolved_at_ms IS NOT NULL)
      OR
      (state = 'unresolved' AND source_attempt_id IS NULL AND source_attempt_number IS NULL AND items_digest IS NULL AND resolved_at_ms IS NULL)
    )
) STRICT;

INSERT INTO v2_resolved_evidence_references (
    attempt_id, source_graph_node_id, reference_ordinal, required,
    selected_item_ids_json, state, source_attempt_id, source_attempt_number,
    items_digest, resolved_at_ms
)
SELECT attempt_id, source_graph_node_id, reference_ordinal, required,
       selected_item_ids_json, state, source_attempt_id, source_attempt_number,
       items_digest, resolved_at_ms
FROM v2_resolved_evidence_references_v8;

DROP TABLE v2_resolved_evidence_references_v8;

PRAGMA legacy_alter_table = OFF;

PRAGMA user_version = 9;
