PRAGMA legacy_alter_table = ON;

ALTER TABLE v2_task_sessions RENAME TO v2_task_sessions_v4;

CREATE TABLE v2_task_sessions (
    singleton                   INTEGER PRIMARY KEY CHECK (singleton = 1),
    session_id                  TEXT NOT NULL UNIQUE,
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
    CHECK (goal_tracking = 1 OR current_goal_revision IS NULL)
) STRICT;

INSERT INTO v2_task_sessions (
    singleton, session_id, task_title, procedure_snapshot_id, lifecycle,
    session_revision, latest_trace_sequence, active_graph_node_id,
    active_attempt_id, active_trace_sequence, goal_tracking,
    current_goal_revision, created_at_ms, completed_at_ms, cancelled_at_ms,
    cancel_reason
)
SELECT
    singleton, session_id, task_title, procedure_snapshot_id, lifecycle,
    session_revision, latest_trace_sequence, active_graph_node_id,
    active_attempt_id, active_trace_sequence, goal_tracking,
    current_goal_revision, created_at_ms, completed_at_ms, cancelled_at_ms,
    cancel_reason
FROM v2_task_sessions_v4;

DROP TABLE v2_task_sessions_v4;

PRAGMA legacy_alter_table = OFF;

CREATE TABLE v2_terminal_dispositions (
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
    terminal_session_revision   INTEGER NOT NULL CHECK (terminal_session_revision >= 1),
    kind                        TEXT NOT NULL CHECK (kind IN ('handed_off', 'not_required')),
    summary                     TEXT,
    stable_reference            TEXT,
    reason                      TEXT,
    actor                       TEXT,
    recorded_at_ms              INTEGER NOT NULL,
    PRIMARY KEY (session_id, terminal_session_revision),
    CHECK (summary IS NULL OR length(summary) BETWEEN 1 AND 4000),
    CHECK (stable_reference IS NULL OR length(stable_reference) BETWEEN 1 AND 4000),
    CHECK (reason IS NULL OR length(reason) BETWEEN 1 AND 4000),
    CHECK (actor IS NULL OR length(actor) BETWEEN 1 AND 256),
    CHECK (
      (kind = 'handed_off' AND summary IS NOT NULL AND stable_reference IS NOT NULL AND reason IS NULL)
      OR
      (kind = 'not_required' AND summary IS NULL AND stable_reference IS NULL AND reason IS NOT NULL)
    )
) STRICT;

PRAGMA user_version = 5;
