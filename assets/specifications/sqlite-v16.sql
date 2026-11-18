-- Unreleased execution-group Store under ADR-0044. No predecessor is declared.
PRAGMA foreign_keys = ON;
PRAGMA trusted_schema = OFF;

CREATE TABLE writer_contract (
    singleton                 INTEGER PRIMARY KEY CHECK (singleton = 1),
    contract_manifest_digest  TEXT NOT NULL CHECK (
        length(contract_manifest_digest) = 71
        AND substr(contract_manifest_digest, 1, 7) = 'sha256:'
        AND substr(contract_manifest_digest, 8) NOT GLOB '*[^0-9a-f]*'
    ),
    generation_id             TEXT NOT NULL CHECK (length(generation_id) BETWEEN 1 AND 64),
    recorded_at_ms            INTEGER NOT NULL
) STRICT;

CREATE TABLE successor_definitions (
    digest                    TEXT PRIMARY KEY,
    kind                      TEXT NOT NULL CHECK (kind IN ('base', 'group')),
    document                  TEXT NOT NULL CHECK (
        length(CAST(document AS BLOB)) BETWEEN 1 AND 1048576 AND json_valid(document)
    ),
    UNIQUE (digest, kind)
) STRICT;

CREATE TABLE successor_sessions (
    session_id                TEXT PRIMARY KEY,
    base_digest               TEXT NOT NULL,
    base_kind                 TEXT GENERATED ALWAYS AS ('base') STORED,
    record_json               TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 1048576
    ),
    record_digest             TEXT NOT NULL,
    FOREIGN KEY (base_digest, base_kind) REFERENCES successor_definitions (digest, kind)
) STRICT;

CREATE TABLE successor_activations (
    session_id                TEXT NOT NULL REFERENCES successor_sessions (session_id) ON DELETE CASCADE,
    ordinal                   INTEGER NOT NULL CHECK (ordinal BETWEEN 1 AND 64),
    execution_id              TEXT NOT NULL,
    definition_digest         TEXT NOT NULL,
    definition_kind           TEXT GENERATED ALWAYS AS ('group') STORED,
    design_attempt_id         TEXT NOT NULL,
    predecessor_id            TEXT,
    record_json               TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 2097152
    ),
    record_digest             TEXT NOT NULL,
    PRIMARY KEY (session_id, ordinal),
    UNIQUE (session_id, execution_id),
    UNIQUE (session_id, execution_id, definition_digest),
    FOREIGN KEY (definition_digest, definition_kind) REFERENCES successor_definitions (digest, kind),
    FOREIGN KEY (session_id, design_attempt_id) REFERENCES successor_attempts (session_id, attempt_id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (session_id, predecessor_id) REFERENCES successor_activations (session_id, execution_id),
    CHECK ((ordinal = 1) = (predecessor_id IS NULL))
) STRICT;

CREATE TABLE successor_attempts (
    session_id                TEXT NOT NULL REFERENCES successor_sessions (session_id) ON DELETE CASCADE,
    ordinal                   INTEGER NOT NULL CHECK (ordinal BETWEEN 1 AND 65536),
    attempt_id                TEXT NOT NULL,
    execution_id              TEXT,
    definition_digest         TEXT,
    record_json               TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 150994944
    ),
    record_digest             TEXT NOT NULL,
    PRIMARY KEY (session_id, ordinal),
    UNIQUE (session_id, attempt_id),
    FOREIGN KEY (session_id, execution_id, definition_digest)
        REFERENCES successor_activations (session_id, execution_id, definition_digest)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK ((execution_id IS NULL) = (definition_digest IS NULL))
) STRICT;

CREATE TABLE successor_goal_revisions (
    session_id                TEXT NOT NULL REFERENCES successor_sessions (session_id) ON DELETE CASCADE,
    ordinal                   INTEGER NOT NULL CHECK (ordinal BETWEEN 2 AND 65536),
    source_attempt_id         TEXT NOT NULL,
    successor_attempt_id      TEXT NOT NULL,
    record_json               TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 1048576
    ),
    record_digest             TEXT NOT NULL,
    PRIMARY KEY (session_id, ordinal),
    FOREIGN KEY (session_id, source_attempt_id) REFERENCES successor_attempts (session_id, attempt_id),
    FOREIGN KEY (session_id, successor_attempt_id) REFERENCES successor_attempts (session_id, attempt_id)
) STRICT;

PRAGMA user_version = 16;
