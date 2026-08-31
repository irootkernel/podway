ALTER TABLE workspace_state
ADD COLUMN runtime_mode TEXT NOT NULL DEFAULT 'prod'
CHECK (
    length(runtime_mode) BETWEEN 1 AND 64
    AND runtime_mode NOT GLOB '*[^a-z0-9-]*'
    AND runtime_mode GLOB '[a-z]*'
    AND runtime_mode NOT GLOB '*--*'
    AND substr(runtime_mode, -1, 1) <> '-'
);

PRAGMA user_version = 10;
