-- Unreleased execution-group Store under ADR-0044. No predecessor is declared.
-- Decision records retain the exact scoped attempt for each declared evidence source.
PRAGMA foreign_keys = ON;
PRAGMA trusted_schema = OFF;

CREATE TABLE workspace_state (
    singleton                   INTEGER PRIMARY KEY CHECK (singleton = 1),
    workspace_uuid              TEXT NOT NULL UNIQUE,
    git_common_fingerprint      TEXT NOT NULL,
    git_worktree_fingerprint    TEXT NOT NULL,
    last_validated_root         TEXT NOT NULL,
    next_workspace_sequence     INTEGER NOT NULL DEFAULT 0 CHECK (next_workspace_sequence >= 0),
    created_at_ms               INTEGER NOT NULL,
    updated_at_ms               INTEGER NOT NULL
) STRICT;

CREATE TABLE operational_journal (
    journal_id          INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at_ms      INTEGER NOT NULL,
    level               TEXT NOT NULL CHECK (level IN ('error', 'warn', 'info', 'debug')),
    event_name          TEXT NOT NULL,
    workspace_sequence  INTEGER,
    summary             TEXT NOT NULL,
    details_json        TEXT CHECK (details_json IS NULL OR json_valid(details_json))
) STRICT;

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
    archived                  INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    FOREIGN KEY (base_digest, base_kind) REFERENCES successor_definitions (digest, kind)
) STRICT;

CREATE UNIQUE INDEX ix_successor_sessions_current
ON successor_sessions(archived) WHERE archived = 0;

CREATE TABLE successor_session_metadata (
    session_id                TEXT PRIMARY KEY REFERENCES successor_sessions (session_id) ON DELETE CASCADE,
    task_title                TEXT NOT NULL CHECK (length(task_title) BETWEEN 1 AND 500),
    archive_slot              INTEGER UNIQUE CHECK (archive_slot BETWEEN 1 AND 4294967295),
    archived_at_ms            INTEGER CHECK (archived_at_ms >= 0),
    archived_sequence         INTEGER CHECK (archived_sequence >= 1),
    CHECK ((archive_slot IS NULL) = (archived_at_ms IS NULL)),
    CHECK ((archive_slot IS NULL) = (archived_sequence IS NULL))
) STRICT;

CREATE TABLE successor_terminal_dispositions (
    session_id                TEXT NOT NULL REFERENCES successor_session_metadata (session_id) ON DELETE CASCADE,
    terminal_revision         INTEGER NOT NULL CHECK (terminal_revision >= 1),
    record_json               TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 65536
    ),
    record_digest             TEXT NOT NULL,
    PRIMARY KEY (session_id, terminal_revision)
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

CREATE TABLE successor_delegation_receipts (
    session_id                TEXT NOT NULL,
    attempt_id                TEXT NOT NULL,
    receipt_id                TEXT NOT NULL UNIQUE,
    record_json               TEXT NOT NULL CHECK (
        json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 45000
    ),
    record_digest             TEXT NOT NULL,
    PRIMARY KEY (session_id, attempt_id),
    FOREIGN KEY (session_id, attempt_id) REFERENCES successor_attempts (session_id, attempt_id) ON DELETE CASCADE
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

CREATE TABLE mutation_outcomes (
    idempotency_key         TEXT PRIMARY KEY CHECK (length(CAST(idempotency_key AS BLOB)) BETWEEN 1 AND 256),
    request_digest          TEXT NOT NULL CHECK (
        length(request_digest) = 71 AND substr(request_digest, 1, 7) = 'sha256:'
        AND substr(request_digest, 8) NOT GLOB '*[^0-9a-f]*'
    ),
    command_name            TEXT NOT NULL CHECK (length(command_name) BETWEEN 1 AND 128),
    workspace_sequence      INTEGER NOT NULL UNIQUE CHECK (workspace_sequence >= 1),
    scope_kind              TEXT NOT NULL CHECK (scope_kind IN ('workspace', 'session')),
    scope_session_id        TEXT,
    response_json           TEXT NOT NULL CHECK (
        json_valid(response_json) AND length(CAST(response_json AS BLOB)) <= 1048576
        AND json_extract(response_json, '$.schema') IS 'podway.output/v4'
    ),
    recorded_at_ms          INTEGER NOT NULL,
    CHECK ((scope_kind = 'session' AND scope_session_id IS NOT NULL)
        OR (scope_kind = 'workspace' AND scope_session_id IS NULL))
) STRICT;

CREATE INDEX ix_mutation_outcomes_scope
ON mutation_outcomes(scope_kind, scope_session_id, recorded_at_ms, workspace_sequence);

PRAGMA user_version = 16;
