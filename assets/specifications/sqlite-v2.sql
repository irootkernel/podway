ALTER TABLE jobs ADD COLUMN response_context_json TEXT
CHECK (response_context_json IS NULL OR json_valid(response_context_json));

PRAGMA user_version = 2;
