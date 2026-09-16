PRAGMA legacy_alter_table = ON;

ALTER TABLE v2_task_sessions RENAME TO v2_task_sessions_v10;
DROP INDEX IF EXISTS ux_v2_task_sessions_one_current;
DROP INDEX IF EXISTS ix_v2_task_sessions_archive_order;

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

INSERT INTO v2_task_sessions (
    session_id, activity, archive_slot, archived_at_ms, archived_workspace_revision,
    task_title, procedure_snapshot_id, lifecycle, session_revision,
    latest_trace_sequence, active_graph_node_id, active_attempt_id,
    active_trace_sequence, goal_tracking, current_goal_revision, created_at_ms,
    completed_at_ms, cancelled_at_ms, cancel_reason
)
SELECT
    session_id, activity, archive_slot, archived_at_ms, archived_workspace_revision,
    task_title, procedure_snapshot_id, lifecycle, session_revision,
    latest_trace_sequence, active_graph_node_id, active_attempt_id,
    active_trace_sequence, goal_tracking, current_goal_revision, created_at_ms,
    completed_at_ms, cancelled_at_ms, cancel_reason
FROM v2_task_sessions_v10;

CREATE UNIQUE INDEX ux_v2_task_sessions_one_current
ON v2_task_sessions(activity)
WHERE activity = 'current';

CREATE INDEX ix_v2_task_sessions_archive_order
ON v2_task_sessions(archived_at_ms DESC, session_id DESC)
WHERE activity = 'inactive';

DROP TABLE v2_task_sessions_v10;

PRAGMA legacy_alter_table = OFF;

PRAGMA user_version = 11;
