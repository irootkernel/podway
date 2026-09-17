CREATE TABLE v2_delegation_receipts (
    attempt_id   TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    receipt_id   TEXT NOT NULL UNIQUE,
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json) AND length(CAST(receipt_json AS BLOB)) <= 45000),
    FOREIGN KEY (session_id, attempt_id) REFERENCES v2_attempts(session_id, attempt_id) ON DELETE CASCADE
) STRICT;

CREATE TRIGGER v2_delegation_receipts_immutable
BEFORE UPDATE ON v2_delegation_receipts
BEGIN
    SELECT RAISE(ABORT, 'delegation receipt is immutable');
END;

PRAGMA user_version = 12;
