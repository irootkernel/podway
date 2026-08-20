PRAGMA legacy_alter_table = ON;

ALTER TABLE v2_item_slots RENAME TO v2_item_slots_v5;

CREATE TABLE v2_item_slots (
    attempt_id                  TEXT NOT NULL REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    item_id                     TEXT NOT NULL,
    item_type                   TEXT NOT NULL CHECK (item_type IN ('confirm', 'text', 'choice', 'integer', 'list', 'artifact', 'check_result')),
    item_revision               INTEGER NOT NULL DEFAULT 0 CHECK (item_revision >= 0),
    value_json                  TEXT CHECK (value_json IS NULL OR json_valid(value_json)),
    created_at_ms               INTEGER NOT NULL,
    updated_at_ms               INTEGER NOT NULL,
    PRIMARY KEY (attempt_id, item_id)
) STRICT;

INSERT INTO v2_item_slots (
    attempt_id, item_id, item_type, item_revision, value_json,
    created_at_ms, updated_at_ms
)
SELECT
    attempt_id, item_id, item_type, item_revision, value_json,
    created_at_ms, updated_at_ms
FROM v2_item_slots_v5;

DROP TABLE v2_item_slots_v5;

PRAGMA legacy_alter_table = OFF;

PRAGMA user_version = 6;
