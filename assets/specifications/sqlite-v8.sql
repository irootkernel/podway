PRAGMA legacy_alter_table = ON;

ALTER TABLE v2_terminal_dispositions RENAME TO v2_terminal_dispositions_v7;

CREATE TABLE v2_terminal_dispositions (
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
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

INSERT INTO v2_terminal_dispositions (
    session_id, terminal_session_revision, kind, summary, stable_reference,
    reason, successor_session_id, actor, recorded_at_ms
)
SELECT session_id, terminal_session_revision, kind, summary, stable_reference,
       reason, NULL, actor, recorded_at_ms
FROM v2_terminal_dispositions_v7;

DROP TABLE v2_terminal_dispositions_v7;

PRAGMA legacy_alter_table = OFF;

PRAGMA user_version = 8;
