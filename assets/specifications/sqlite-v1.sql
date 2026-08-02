PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;

CREATE TABLE schema_migrations (
    version             INTEGER PRIMARY KEY,
    name                TEXT NOT NULL UNIQUE,
    checksum            TEXT NOT NULL CHECK (checksum GLOB 'sha256:[0-9a-f]*'),
    applied_at_ms       INTEGER NOT NULL
) STRICT;

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

CREATE TABLE procedure_snapshots (
    snapshot_id          TEXT PRIMARY KEY,
    schema_id            TEXT NOT NULL CHECK (schema_id = 'podway.procedure/v1'),
    procedure_id         TEXT NOT NULL,
    procedure_version    TEXT NOT NULL,
    name                 TEXT NOT NULL,
    digest               TEXT NOT NULL CHECK (digest GLOB 'sha256:[0-9a-f]*'),
    canonical_json       TEXT NOT NULL CHECK (json_valid(canonical_json)),
    source_kind          TEXT NOT NULL CHECK (source_kind IN ('preset', 'file')),
    source_label         TEXT NOT NULL,
    created_at_ms        INTEGER NOT NULL
) STRICT;

CREATE TABLE task_sessions (
    singleton               INTEGER PRIMARY KEY CHECK (singleton = 1),
    session_id              TEXT NOT NULL UNIQUE,
    task_title              TEXT NOT NULL,
    procedure_snapshot_id   TEXT NOT NULL REFERENCES procedure_snapshots(snapshot_id) ON DELETE RESTRICT,
    lifecycle               TEXT NOT NULL CHECK (lifecycle IN ('running', 'completed', 'cancelled')),
    session_revision        INTEGER NOT NULL CHECK (session_revision >= 1),
    active_stage_id         TEXT,
    active_attempt_id       TEXT,
    created_at_ms           INTEGER NOT NULL,
    completed_at_ms         INTEGER,
    cancelled_at_ms         INTEGER,
    cancel_reason           TEXT,
    CHECK (
      (lifecycle = 'running' AND active_stage_id IS NOT NULL AND active_attempt_id IS NOT NULL AND completed_at_ms IS NULL AND cancelled_at_ms IS NULL)
      OR
      (lifecycle = 'completed' AND active_stage_id IS NULL AND active_attempt_id IS NULL AND completed_at_ms IS NOT NULL AND cancelled_at_ms IS NULL)
      OR
      (lifecycle = 'cancelled' AND active_stage_id IS NULL AND active_attempt_id IS NULL AND cancelled_at_ms IS NOT NULL AND cancel_reason IS NOT NULL)
    )
) STRICT;

CREATE TABLE stage_progress (
    session_id              TEXT NOT NULL REFERENCES task_sessions(session_id) ON DELETE CASCADE,
    stage_id                TEXT NOT NULL,
    stage_index             INTEGER NOT NULL CHECK (stage_index >= 0),
    progress_state          TEXT NOT NULL CHECK (progress_state IN ('pending', 'current', 'done', 'skipped', 'redo', 'abandoned')),
    latest_attempt_number   INTEGER NOT NULL DEFAULT 0 CHECK (latest_attempt_number >= 0),
    latest_attempt_id       TEXT,
    PRIMARY KEY (session_id, stage_id),
    UNIQUE (session_id, stage_index)
) STRICT;

CREATE UNIQUE INDEX ux_stage_progress_one_current
ON stage_progress(session_id)
WHERE progress_state = 'current';

CREATE TABLE attempts (
    attempt_id          TEXT PRIMARY KEY,
    session_id          TEXT NOT NULL,
    stage_id            TEXT NOT NULL,
    attempt_number      INTEGER NOT NULL CHECK (attempt_number >= 1),
    lifecycle           TEXT NOT NULL CHECK (lifecycle IN ('active', 'completed', 'skipped', 'abandoned')),
    started_at_ms       INTEGER NOT NULL,
    ended_at_ms         INTEGER,
    reason              TEXT,
    FOREIGN KEY (session_id, stage_id) REFERENCES stage_progress(session_id, stage_id) ON DELETE CASCADE,
    UNIQUE (session_id, stage_id, attempt_number),
    CHECK ((lifecycle = 'active' AND ended_at_ms IS NULL) OR (lifecycle <> 'active' AND ended_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX ux_attempts_one_active
ON attempts(session_id)
WHERE lifecycle = 'active';

CREATE INDEX ix_attempts_stage
ON attempts(session_id, stage_id, attempt_number DESC);

CREATE TABLE item_slots (
    attempt_id          TEXT NOT NULL REFERENCES attempts(attempt_id) ON DELETE CASCADE,
    item_id             TEXT NOT NULL,
    item_type           TEXT NOT NULL CHECK (item_type IN ('confirm', 'text', 'choice', 'integer', 'list', 'artifact')),
    item_revision       INTEGER NOT NULL DEFAULT 0 CHECK (item_revision >= 0),
    value_json          TEXT CHECK (value_json IS NULL OR json_valid(value_json)),
    created_at_ms       INTEGER NOT NULL,
    updated_at_ms       INTEGER NOT NULL,
    PRIMARY KEY (attempt_id, item_id)
) STRICT;

CREATE TABLE blockers (
    blocker_id          TEXT PRIMARY KEY,
    attempt_id          TEXT NOT NULL REFERENCES attempts(attempt_id) ON DELETE CASCADE,
    reason              TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (state IN ('open', 'resolved')),
    created_at_ms       INTEGER NOT NULL,
    resolved_at_ms      INTEGER,
    CHECK ((state = 'open' AND resolved_at_ms IS NULL) OR (state = 'resolved' AND resolved_at_ms IS NOT NULL))
) STRICT;

CREATE INDEX ix_blockers_attempt_state
ON blockers(attempt_id, state, created_at_ms);

CREATE TABLE jobs (
    job_id                  TEXT PRIMARY KEY,
    workspace_sequence      INTEGER NOT NULL UNIQUE CHECK (workspace_sequence >= 1),
    idempotency_key         TEXT NOT NULL,
    request_digest          TEXT NOT NULL CHECK (request_digest GLOB 'sha256:[0-9a-f]*'),
    command_name            TEXT NOT NULL,
    canonical_request_json  TEXT NOT NULL CHECK (json_valid(canonical_request_json)),
    state                   TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    session_id              TEXT,
    submitted_at_ms         INTEGER NOT NULL,
    claimed_at_ms           INTEGER,
    finished_at_ms          INTEGER,
    terminal_response_json  TEXT CHECK (terminal_response_json IS NULL OR json_valid(terminal_response_json)),
    CHECK (
      (state IN ('queued', 'running') AND finished_at_ms IS NULL AND terminal_response_json IS NULL)
      OR
      (state IN ('succeeded', 'failed', 'cancelled') AND finished_at_ms IS NOT NULL AND terminal_response_json IS NOT NULL)
    )
) STRICT;

CREATE INDEX ix_jobs_state_sequence
ON jobs(state, workspace_sequence);

CREATE INDEX ix_jobs_terminal_time
ON jobs(finished_at_ms)
WHERE state IN ('succeeded', 'failed', 'cancelled');

CREATE TABLE idempotency_records (
    idempotency_key         TEXT PRIMARY KEY,
    request_digest          TEXT NOT NULL CHECK (request_digest GLOB 'sha256:[0-9a-f]*'),
    job_id                  TEXT NOT NULL,
    scope_kind              TEXT NOT NULL CHECK (scope_kind IN ('workspace', 'session')),
    scope_session_id        TEXT,
    terminal_response_json  TEXT CHECK (terminal_response_json IS NULL OR json_valid(terminal_response_json)),
    created_at_ms           INTEGER NOT NULL,
    updated_at_ms           INTEGER NOT NULL,
    CHECK ((scope_kind = 'session' AND scope_session_id IS NOT NULL) OR (scope_kind = 'workspace' AND scope_session_id IS NULL))
) STRICT;

CREATE INDEX ix_idempotency_scope
ON idempotency_records(scope_kind, scope_session_id, created_at_ms);

CREATE TABLE operational_journal (
    journal_id          INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at_ms      INTEGER NOT NULL,
    level               TEXT NOT NULL CHECK (level IN ('error', 'warn', 'info', 'debug')),
    event_name          TEXT NOT NULL,
    workspace_sequence  INTEGER,
    job_id              TEXT,
    summary             TEXT NOT NULL,
    details_json        TEXT CHECK (details_json IS NULL OR json_valid(details_json))
) STRICT;

CREATE INDEX ix_operational_journal_time
ON operational_journal(recorded_at_ms);

PRAGMA user_version = 1;
