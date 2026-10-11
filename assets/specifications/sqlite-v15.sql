-- Current direct Store baseline under ADR-0038.
-- Daemon-era generations are rejected, never migrated.
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
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

CREATE TABLE v2_procedure_snapshots (
    snapshot_id             TEXT PRIMARY KEY,
    schema_id               TEXT NOT NULL CHECK (schema_id = 'podway.procedure/v2'),
    procedure_id            TEXT NOT NULL,
    procedure_version       TEXT NOT NULL,
    name                    TEXT NOT NULL,
    purpose                 TEXT NOT NULL,
    digest                  TEXT NOT NULL CHECK (digest GLOB 'sha256:[0-9a-f]*'),
    canonical_json          TEXT NOT NULL CHECK (json_valid(canonical_json)),
    source_kind             TEXT NOT NULL CHECK (source_kind IN ('preset', 'file')),
    source_label            TEXT NOT NULL,
    goal_tracking           INTEGER NOT NULL CHECK (goal_tracking IN (0, 1)),
    created_at_ms           INTEGER NOT NULL
) STRICT;

CREATE TABLE v2_graph_nodes (
    snapshot_id             TEXT NOT NULL REFERENCES v2_procedure_snapshots(snapshot_id) ON DELETE CASCADE,
    graph_node_id           TEXT NOT NULL,
    node_definition_id      TEXT NOT NULL,
    placement_index         INTEGER NOT NULL CHECK (placement_index >= 0),
    node_type               TEXT NOT NULL CHECK (node_type IN ('action', 'decision')),
    goal_assessment         INTEGER NOT NULL CHECK (goal_assessment IN (0, 1)),
    canonical_placement_json TEXT NOT NULL CHECK (json_valid(canonical_placement_json)),
    PRIMARY KEY (snapshot_id, graph_node_id),
    UNIQUE (snapshot_id, placement_index),
    CHECK (goal_assessment = 0 OR node_type = 'decision')
) STRICT;

CREATE TABLE v2_graph_node_counters (
    session_id                  TEXT NOT NULL REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    snapshot_id                 TEXT NOT NULL,
    graph_node_id               TEXT NOT NULL,
    attempt_count               INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    rework_traversal_count      INTEGER NOT NULL DEFAULT 0 CHECK (rework_traversal_count >= 0),
    PRIMARY KEY (session_id, graph_node_id),
    FOREIGN KEY (snapshot_id, graph_node_id) REFERENCES v2_graph_nodes(snapshot_id, graph_node_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE v2_attempts (
    attempt_id                  TEXT PRIMARY KEY,
    session_id                  TEXT NOT NULL REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    snapshot_id                 TEXT NOT NULL,
    graph_node_id               TEXT NOT NULL,
    node_definition_id          TEXT NOT NULL,
    attempt_number              INTEGER NOT NULL CHECK (attempt_number >= 1),
    trace_sequence              INTEGER NOT NULL CHECK (trace_sequence >= 1),
    lifecycle                   TEXT NOT NULL CHECK (lifecycle IN ('active', 'completed', 'skipped', 'abandoned')),
    validity                    TEXT NOT NULL CHECK (validity IN ('valid', 'stale')),
    goal_revision               INTEGER CHECK (goal_revision IS NULL OR goal_revision >= 1),
    started_at_ms               INTEGER NOT NULL,
    ended_at_ms                 INTEGER,
    terminal_reason             TEXT,
    UNIQUE (session_id, attempt_id),
    UNIQUE (session_id, graph_node_id, attempt_number),
    UNIQUE (session_id, trace_sequence),
    FOREIGN KEY (snapshot_id, graph_node_id) REFERENCES v2_graph_nodes(snapshot_id, graph_node_id) ON DELETE RESTRICT,
    FOREIGN KEY (session_id, goal_revision) REFERENCES v2_goal_revisions(session_id, goal_revision) DEFERRABLE INITIALLY DEFERRED,
    CHECK ((lifecycle = 'active' AND ended_at_ms IS NULL) OR (lifecycle <> 'active' AND ended_at_ms IS NOT NULL)),
    CHECK (lifecycle <> 'active' OR validity = 'valid'),
    CHECK (lifecycle <> 'abandoned' OR validity = 'stale')
) STRICT;

CREATE TABLE v2_blockers (
    blocker_id                  TEXT PRIMARY KEY,
    attempt_id                  TEXT NOT NULL REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    reason                      TEXT NOT NULL,
    state                       TEXT NOT NULL CHECK (state IN ('open', 'resolved')),
    created_at_ms               INTEGER NOT NULL,
    resolved_at_ms              INTEGER,
    CHECK ((state = 'open' AND resolved_at_ms IS NULL) OR (state = 'resolved' AND resolved_at_ms IS NOT NULL))
) STRICT;

CREATE TABLE v2_decision_records (
    attempt_id                  TEXT PRIMARY KEY REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    session_id                  TEXT NOT NULL REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    trace_sequence              INTEGER NOT NULL CHECK (trace_sequence >= 1),
    session_revision            INTEGER NOT NULL CHECK (session_revision >= 1),
    procedure_snapshot_id       TEXT NOT NULL REFERENCES v2_procedure_snapshots(snapshot_id) ON DELETE RESTRICT,
    procedure_digest            TEXT NOT NULL CHECK (procedure_digest GLOB 'sha256:[0-9a-f]*'),
    graph_node_id               TEXT NOT NULL,
    node_definition_id          TEXT NOT NULL,
    attempt_number              INTEGER NOT NULL CHECK (attempt_number >= 1),
    goal_revision               INTEGER CHECK (goal_revision IS NULL OR goal_revision >= 1),
    selected_option_id          TEXT NOT NULL,
    route_effect                TEXT NOT NULL CHECK (route_effect IN ('advance', 'rework')),
    route_target_graph_node_id  TEXT NOT NULL,
    reason                      TEXT NOT NULL,
    actor                       TEXT,
    recorded_at_ms              INTEGER NOT NULL,
    UNIQUE (session_id, trace_sequence)
) STRICT;

CREATE TABLE v2_rework_records (
    session_id                  TEXT NOT NULL REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    trace_sequence              INTEGER NOT NULL CHECK (trace_sequence >= 1),
    kind                        TEXT NOT NULL CHECK (kind IN ('declared', 'manual')),
    from_graph_node_id          TEXT NOT NULL,
    to_graph_node_id            TEXT NOT NULL,
    target_attempt_id           TEXT NOT NULL UNIQUE REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    reason                      TEXT NOT NULL,
    reactivated                 INTEGER NOT NULL CHECK (reactivated IN (0, 1)),
    actor                       TEXT,
    recorded_at_ms              INTEGER NOT NULL,
    PRIMARY KEY (session_id, trace_sequence)
) STRICT;

CREATE TABLE v2_goal_revisions (
    session_id                  TEXT NOT NULL REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    goal_revision               INTEGER NOT NULL CHECK (goal_revision >= 1),
    predecessor_revision        INTEGER,
    statement                   TEXT NOT NULL,
    reason                      TEXT,
    rework_to_graph_node_id     TEXT,
    reactivated                 INTEGER NOT NULL CHECK (reactivated IN (0, 1)),
    actor                       TEXT,
    binding_trace_sequence      INTEGER NOT NULL CHECK (binding_trace_sequence >= 1),
    created_at_ms               INTEGER NOT NULL,
    PRIMARY KEY (session_id, goal_revision),
    FOREIGN KEY (session_id, predecessor_revision) REFERENCES v2_goal_revisions(session_id, goal_revision),
    FOREIGN KEY (session_id, binding_trace_sequence) REFERENCES v2_attempts(session_id, trace_sequence) DEFERRABLE INITIALLY DEFERRED,
    CHECK (
      (goal_revision = 1 AND predecessor_revision IS NULL AND reason IS NULL AND rework_to_graph_node_id IS NULL AND reactivated = 0)
      OR
      (goal_revision > 1 AND predecessor_revision IS NOT NULL AND predecessor_revision = goal_revision - 1 AND reason IS NOT NULL AND rework_to_graph_node_id IS NOT NULL)
    )
) STRICT;

CREATE TABLE v2_goal_criteria (
    session_id                  TEXT NOT NULL,
    goal_revision               INTEGER NOT NULL,
    criterion_id                TEXT NOT NULL,
    criterion_ordinal           INTEGER NOT NULL CHECK (criterion_ordinal >= 0),
    statement                   TEXT NOT NULL,
    PRIMARY KEY (session_id, goal_revision, criterion_id),
    UNIQUE (session_id, goal_revision, criterion_ordinal),
    FOREIGN KEY (session_id, goal_revision) REFERENCES v2_goal_revisions(session_id, goal_revision) ON DELETE CASCADE
) STRICT;

CREATE TABLE v2_criterion_assessment_results (
    attempt_id                  TEXT NOT NULL REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    session_id                  TEXT NOT NULL,
    goal_revision               INTEGER NOT NULL,
    criterion_id                TEXT NOT NULL,
    status                      TEXT NOT NULL CHECK (status IN ('satisfied', 'unsatisfied', 'not_applicable')),
    mode                        TEXT NOT NULL CHECK (mode IN ('assessment', 'applicability')),
    reason                      TEXT NOT NULL,
    actor                       TEXT,
    recorded_at_ms              INTEGER NOT NULL,
    PRIMARY KEY (attempt_id, criterion_id),
    FOREIGN KEY (session_id, goal_revision, criterion_id) REFERENCES v2_goal_criteria(session_id, goal_revision, criterion_id) ON DELETE RESTRICT,
    CHECK ((mode = 'assessment' AND status IN ('satisfied', 'unsatisfied')) OR (mode = 'applicability' AND status = 'not_applicable'))
) STRICT;

CREATE TABLE v2_criterion_citations (
    attempt_id                  TEXT NOT NULL,
    criterion_id                TEXT NOT NULL,
    citation_ordinal            INTEGER NOT NULL CHECK (citation_ordinal >= 0),
    citation_kind               TEXT NOT NULL CHECK (citation_kind IN ('evidence', 'item')),
    source_graph_node_id        TEXT,
    item_id                     TEXT,
    PRIMARY KEY (attempt_id, criterion_id, citation_ordinal),
    FOREIGN KEY (attempt_id, criterion_id) REFERENCES v2_criterion_assessment_results(attempt_id, criterion_id) ON DELETE CASCADE,
    CHECK (
      (citation_kind = 'evidence' AND source_graph_node_id IS NOT NULL AND item_id IS NULL)
      OR
      (citation_kind = 'item' AND source_graph_node_id IS NULL AND item_id IS NOT NULL)
    )
) STRICT;

CREATE TABLE v2_goal_assessments (
    decision_attempt_id         TEXT PRIMARY KEY REFERENCES v2_decision_records(attempt_id) ON DELETE CASCADE,
    session_id                  TEXT NOT NULL,
    goal_revision               INTEGER NOT NULL,
    decision_trace_sequence     INTEGER NOT NULL CHECK (decision_trace_sequence >= 1),
    outcome                     TEXT NOT NULL CHECK (outcome IN ('achieved', 'not_achieved', 'superseded')),
    mode                        TEXT NOT NULL CHECK (mode IN ('assessment', 'applicability')),
    selected_option_id          TEXT NOT NULL,
    route_effect                TEXT NOT NULL CHECK (route_effect IN ('advance', 'rework')),
    route_target_graph_node_id  TEXT NOT NULL,
    actor                       TEXT,
    recorded_at_ms              INTEGER NOT NULL,
    record_digest               TEXT NOT NULL CHECK (record_digest GLOB 'sha256:[0-9a-f]*'),
    UNIQUE (session_id, decision_trace_sequence),
    FOREIGN KEY (session_id, goal_revision) REFERENCES v2_goal_revisions(session_id, goal_revision) ON DELETE RESTRICT
) STRICT;

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

CREATE TABLE v2_workspace_state (
    singleton               INTEGER PRIMARY KEY CHECK (singleton = 1),
    current_session_id      TEXT NOT NULL UNIQUE REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    workspace_revision      INTEGER NOT NULL DEFAULT 0 CHECK (workspace_revision >= 0),
    FOREIGN KEY (singleton) REFERENCES workspace_state(singleton) ON DELETE CASCADE
) STRICT;

CREATE TABLE v2_terminal_dispositions (
    session_id                  TEXT NOT NULL REFERENCES "v2_task_sessions"(session_id) ON DELETE CASCADE,
    terminal_session_revision   INTEGER NOT NULL CHECK (terminal_session_revision >= 1),
    kind                        TEXT NOT NULL CHECK (kind IN ('handed_off', 'not_required', 'superseded')),
    summary                     TEXT,
    stable_reference            TEXT,
    reason                      TEXT,
    successor_session_id        TEXT,
    actor                       TEXT,
    recorded_at_ms              INTEGER NOT NULL,
    PRIMARY KEY (session_id, terminal_session_revision),
    CHECK (summary IS NULL OR length(summary) BETWEEN 1 AND 4000),
    CHECK (stable_reference IS NULL OR length(stable_reference) BETWEEN 1 AND 4000),
    CHECK (reason IS NULL OR length(reason) BETWEEN 1 AND 4000),
    CHECK (successor_session_id IS NULL OR length(successor_session_id) BETWEEN 1 AND 64),
    CHECK (actor IS NULL OR length(actor) BETWEEN 1 AND 256),
    CHECK (
      (kind = 'handed_off' AND summary IS NOT NULL AND stable_reference IS NOT NULL AND reason IS NULL AND successor_session_id IS NULL)
      OR
      (kind = 'not_required' AND summary IS NULL AND stable_reference IS NULL AND reason IS NOT NULL AND successor_session_id IS NULL)
      OR
      (kind = 'superseded' AND summary IS NULL AND stable_reference IS NULL AND reason IS NOT NULL AND successor_session_id IS NOT NULL AND successor_session_id <> session_id)
    )
) STRICT;

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

CREATE TABLE v2_task_sessions (
    session_id                  TEXT PRIMARY KEY,
    activity                    TEXT NOT NULL CHECK (activity IN ('current', 'inactive')),
    archive_slot                INTEGER UNIQUE CHECK (archive_slot IS NULL OR (archive_slot >= 1 AND archive_slot <= 4294967295)),
    archived_at_ms              INTEGER,
    archived_workspace_revision INTEGER CHECK (archived_workspace_revision IS NULL OR archived_workspace_revision >= 0),
    task_title                  TEXT NOT NULL,
    procedure_snapshot_id       TEXT NOT NULL REFERENCES v2_procedure_snapshots(snapshot_id) ON DELETE RESTRICT,
    lifecycle                   TEXT NOT NULL CHECK (lifecycle IN ('prepared', 'running', 'completed', 'cancelled')),
    session_revision            INTEGER NOT NULL CHECK (session_revision >= 0),
    latest_trace_sequence       INTEGER NOT NULL DEFAULT 0 CHECK (latest_trace_sequence >= 0),
    active_graph_node_id        TEXT,
    active_attempt_id           TEXT,
    active_trace_sequence       INTEGER CHECK (active_trace_sequence IS NULL OR active_trace_sequence >= 1),
    goal_tracking               INTEGER NOT NULL CHECK (goal_tracking IN (0, 1)),
    current_goal_revision       INTEGER CHECK (current_goal_revision IS NULL OR current_goal_revision >= 1),
    created_at_ms               INTEGER NOT NULL,
    completed_at_ms             INTEGER,
    cancelled_at_ms             INTEGER,
    cancel_reason               TEXT,
    CHECK (
      (lifecycle = 'prepared' AND session_revision = 0 AND latest_trace_sequence = 0 AND active_graph_node_id IS NULL AND active_attempt_id IS NULL AND active_trace_sequence IS NULL AND current_goal_revision IS NULL AND completed_at_ms IS NULL AND cancelled_at_ms IS NULL AND cancel_reason IS NULL)
      OR
      (lifecycle = 'running' AND session_revision >= 1 AND active_graph_node_id IS NOT NULL AND active_attempt_id IS NOT NULL AND active_trace_sequence IS NOT NULL AND completed_at_ms IS NULL AND cancelled_at_ms IS NULL AND cancel_reason IS NULL)
      OR
      (lifecycle = 'completed' AND session_revision >= 1 AND active_graph_node_id IS NULL AND active_attempt_id IS NULL AND active_trace_sequence IS NULL AND completed_at_ms IS NOT NULL AND cancelled_at_ms IS NULL AND cancel_reason IS NULL)
      OR
      (lifecycle = 'cancelled' AND session_revision >= 1 AND active_graph_node_id IS NULL AND active_attempt_id IS NULL AND active_trace_sequence IS NULL AND completed_at_ms IS NULL AND cancelled_at_ms IS NOT NULL AND cancel_reason IS NOT NULL)
    ),
    CHECK (active_trace_sequence IS NULL OR active_trace_sequence <= latest_trace_sequence),
    CHECK (goal_tracking = 1 OR current_goal_revision IS NULL),
    CHECK (
      (activity = 'current' AND archive_slot IS NULL AND archived_at_ms IS NULL AND archived_workspace_revision IS NULL)
      OR
      (activity = 'inactive' AND lifecycle IN ('completed', 'cancelled') AND archive_slot IS NOT NULL AND archived_at_ms IS NOT NULL AND archived_workspace_revision IS NOT NULL)
    )
) STRICT;

CREATE TABLE v2_delegation_receipts (
    attempt_id   TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    receipt_id   TEXT NOT NULL UNIQUE,
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json) AND length(CAST(receipt_json AS BLOB)) <= 45000),
    FOREIGN KEY (session_id, attempt_id) REFERENCES v2_attempts(session_id, attempt_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX ix_operational_journal_time
ON operational_journal(recorded_at_ms);

CREATE UNIQUE INDEX ux_v2_attempts_one_active
ON v2_attempts(session_id)
WHERE lifecycle = 'active';

CREATE UNIQUE INDEX ux_v2_attempts_one_valid_per_node
ON v2_attempts(session_id, graph_node_id)
WHERE validity = 'valid';

CREATE INDEX ix_v2_attempts_trace
ON v2_attempts(session_id, trace_sequence DESC);

CREATE INDEX ix_v2_blockers_attempt_state
ON v2_blockers(attempt_id, state, created_at_ms DESC);

CREATE UNIQUE INDEX ux_v2_task_sessions_one_current
ON v2_task_sessions(activity)
WHERE activity = 'current';

CREATE INDEX ix_v2_task_sessions_archive_order
ON v2_task_sessions(archived_at_ms DESC, session_id DESC)
WHERE activity = 'inactive';

CREATE TRIGGER v2_delegation_receipts_immutable
BEFORE UPDATE ON v2_delegation_receipts
BEGIN
    SELECT RAISE(ABORT, 'delegation receipt is immutable');
END;

CREATE TABLE writer_contract (
    singleton                INTEGER PRIMARY KEY CHECK (singleton = 1),
    contract_manifest_digest TEXT NOT NULL CHECK (
        length(contract_manifest_digest) = 71
        AND substr(contract_manifest_digest, 1, 7) = 'sha256:'
        AND substr(contract_manifest_digest, 8) NOT GLOB '*[^0-9a-f]*'
    ),
    generation_id            TEXT NOT NULL CHECK (length(generation_id) BETWEEN 1 AND 64),
    recorded_at_ms           INTEGER NOT NULL
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

PRAGMA user_version = 15;
