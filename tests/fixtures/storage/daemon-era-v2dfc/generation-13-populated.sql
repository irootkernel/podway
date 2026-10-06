-- Frozen V2DFC-005 legacy producer output; see README.md.
PRAGMA foreign_keys=OFF;
PRAGMA journal_mode=WAL;
BEGIN TRANSACTION;
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
INSERT INTO "idempotency_records" VALUES('delegation-receipt-key','sha256:9999999999999999999999999999999999999999999999999999999999999999','00000000-0000-4000-9000-000000000406','session','00000000-0000-4000-8000-000000000403','{"command":{"kind":"delegation_accept"},"graph_session_projection":{"entry_graph_node_id":"work","goal_defined":false,"goal_tracking":false,"lifecycle":"running","operation":{"attempt_id":"00000000-0000-4000-8000-000000000404","kind":"delegation_accept","receipt_id":"00000000-0000-4000-8000-000000000405"},"procedure_digest":"sha256:5a0ad8a6d7f8671a69f6c41349cb01d210d790a6dc2869968e708926cd2a83e6","revision_after":2,"revision_before":1,"session_id":"00000000-0000-4000-8000-000000000403","task_title":"Delegated task"},"job":{"identity_sequence":1,"job_id":"00000000-0000-4000-9000-000000000406","request_digest":"sha256:9999999999999999999999999999999999999999999999999999999999999999"},"job_projection":{"claimed_at":6,"finished_at":7,"state":"succeeded","submitted_at":5},"response_context":{"command":"delegation.accept","request_id":"00000000-0000-4000-8000-000000000407","workspace_root":"/tmp/podway-v2sdg004-receipt","workspace_sequence":1,"workspace_uuid":"00000000-0000-4000-8000-000000000401"},"result":{"kind":"success","payload":{"attempt_id":"00000000-0000-4000-8000-000000000404","kind":"delegation_accepted","package_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","receipt_id":"00000000-0000-4000-8000-000000000405","result_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","revision_after":2,"revision_before":1,"session_id":"00000000-0000-4000-8000-000000000403","workspace_uuid":"00000000-0000-4000-8000-000000000401"}},"schema":"podway.store-graph-terminal/v2"}',5,7);
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
    terminal_response_json  TEXT CHECK (terminal_response_json IS NULL OR json_valid(terminal_response_json)), response_context_json TEXT
CHECK (response_context_json IS NULL OR json_valid(response_context_json)),
    CHECK (
      (state IN ('queued', 'running') AND finished_at_ms IS NULL AND terminal_response_json IS NULL)
      OR
      (state IN ('succeeded', 'failed', 'cancelled') AND finished_at_ms IS NOT NULL AND terminal_response_json IS NOT NULL)
    )
) STRICT;
INSERT INTO "jobs" VALUES('00000000-0000-4000-9000-000000000406',1,'delegation-receipt-key','sha256:9999999999999999999999999999999999999999999999999999999999999999','delegation.accept','{"canonical_execution_json":"{\"attached_artifact\":null,\"command\":\"delegation.accept\",\"execution_version\":8,\"fresh_attempt_id\":null,\"fresh_blocker_id\":null,\"payload\":{\"accepted\":true,\"attempt_id\":\"00000000-0000-4000-8000-000000000404\",\"execution\":{\"accepted_task_digest\":\"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\",\"adapter_id\":\"adapter\",\"adapter_version\":\"1\",\"agent_configuration_digest\":\"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\",\"binding_ref\":\"local-specialist\",\"primary_host\":\"external\",\"role_ref\":\"reviewer\",\"role_snapshot_digest\":\"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"scope_kind\":\"session\",\"scope_ref\":\"scope-1\",\"supervisor_ref\":\"supervisor-1\",\"task_ref\":\"task-1\",\"worker_ref\":\"worker-1\"},\"idempotency_key\":\"delegation-receipt-key\",\"package_digest\":\"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\",\"result_digest\":\"sha256:1111111111111111111111111111111111111111111111111111111111111111\",\"rounds_used\":1,\"schema\":\"podway.delegation-accept-input/v1\",\"session_id\":\"00000000-0000-4000-8000-000000000403\",\"session_revision\":1,\"summary\":\"Reviewed result\",\"worker_disposition\":\"idle\",\"workspace_uuid\":\"00000000-0000-4000-8000-000000000401\"},\"preconditions\":{},\"selector\":{},\"workspace_id\":\"00000000-0000-4000-8000-000000000401\"}","command":{"kind":"delegation_accept"},"execution_flavor":"procedure_v2","expected_session_id":"00000000-0000-4000-8000-000000000403","preconditions":{"expected_attempt_id":"00000000-0000-4000-8000-000000000404","expected_item_id":null,"expected_item_revision":null,"expected_session_revision":1},"schema":"podway.store-graph-command/v1"}','succeeded','00000000-0000-4000-8000-000000000403',5,6,7,'{"command":{"kind":"delegation_accept"},"graph_session_projection":{"entry_graph_node_id":"work","goal_defined":false,"goal_tracking":false,"lifecycle":"running","operation":{"attempt_id":"00000000-0000-4000-8000-000000000404","kind":"delegation_accept","receipt_id":"00000000-0000-4000-8000-000000000405"},"procedure_digest":"sha256:5a0ad8a6d7f8671a69f6c41349cb01d210d790a6dc2869968e708926cd2a83e6","revision_after":2,"revision_before":1,"session_id":"00000000-0000-4000-8000-000000000403","task_title":"Delegated task"},"job":{"identity_sequence":1,"job_id":"00000000-0000-4000-9000-000000000406","request_digest":"sha256:9999999999999999999999999999999999999999999999999999999999999999"},"job_projection":{"claimed_at":6,"finished_at":7,"state":"succeeded","submitted_at":5},"response_context":{"command":"delegation.accept","request_id":"00000000-0000-4000-8000-000000000407","workspace_root":"/tmp/podway-v2sdg004-receipt","workspace_sequence":1,"workspace_uuid":"00000000-0000-4000-8000-000000000401"},"result":{"kind":"success","payload":{"attempt_id":"00000000-0000-4000-8000-000000000404","kind":"delegation_accepted","package_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","receipt_id":"00000000-0000-4000-8000-000000000405","result_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","revision_after":2,"revision_before":1,"session_id":"00000000-0000-4000-8000-000000000403","workspace_uuid":"00000000-0000-4000-8000-000000000401"}},"schema":"podway.store-graph-terminal/v2"}','{"command":"delegation.accept","request_id":"00000000-0000-4000-8000-000000000407","workspace_root":"/tmp/podway-v2sdg004-receipt","workspace_sequence":1,"workspace_uuid":"00000000-0000-4000-8000-000000000401"}');
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
CREATE TABLE schema_migrations (
    version             INTEGER PRIMARY KEY,
    name                TEXT NOT NULL UNIQUE,
    checksum            TEXT NOT NULL CHECK (checksum GLOB 'sha256:[0-9a-f]*'),
    applied_at_ms       INTEGER NOT NULL
) STRICT;
INSERT INTO "schema_migrations" VALUES(1,'schema-0-uninitialized','sha256:20ea04d9635b8e1632e6d3aa5f3a888eaca49307b43ade9b9991363b30607423',1);
INSERT INTO "schema_migrations" VALUES(2,'schema-1-response-context','sha256:3432d426ee69484e691977e336f271fcbc222f332bd05400d7166b459bcf05fc',1);
INSERT INTO "schema_migrations" VALUES(3,'schema-2-procedure-v2-state','sha256:45548f0dfa89ec404b6b85f441b3aa7ba4f8adbfb9d34cd26a6f79c691d2156f',1);
INSERT INTO "schema_migrations" VALUES(4,'schema-3-procedure-v2-only','sha256:c23e156e50d082abf7e881fd209907329044001869479a7d75f0ad8db0a22a12',1);
INSERT INTO "schema_migrations" VALUES(5,'schema-4-prepared-session-lifecycle','sha256:e213bd58ad3199be1242daf3e03e10b1bb8deb129125af3648bc7954bd620bed',1);
INSERT INTO "schema_migrations" VALUES(6,'schema-5-external-check-result-items','sha256:5df18e0f7d2d1f396d4fcb00dcb20110f1a62e0416522832711d2961074333dd',1);
INSERT INTO "schema_migrations" VALUES(7,'schema-6-retained-terminal-sessions','sha256:f61189c9c892619db72ae49f127a10ba589d0f0056cfd2db855c115f709a563b',1);
INSERT INTO "schema_migrations" VALUES(8,'schema-7-superseded-session-disposition','sha256:3ebf7d19ab160413b9fe205ee803bcf6d530bd11eccfb3fea1e03a378b580e94',1);
INSERT INTO "schema_migrations" VALUES(9,'schema-8-duplicate-evidence-references','sha256:699cf40bbc3b3232467ebca4c96f7a2412701fe743c590ee2ea1637f1a038499',1);
INSERT INTO "schema_migrations" VALUES(10,'schema-9-runtime-mode-binding','sha256:b785cb2802e5a36ddd2395636e0502c71e45568bb18738dbd0eec1de3212be68',1);
INSERT INTO "schema_migrations" VALUES(11,'schema-10-unbounded-inactive-sessions','sha256:ee2514bb9d728d4ab3326e5eef18a1fc2d4d64d06619dea8a5547ccdddf7c9c9',1);
INSERT INTO "schema_migrations" VALUES(12,'schema-11-delegation-receipts','sha256:83b447fc7bcd0a1b6e8d8bafcf833a07015da950a279a262fd0be88ed5bb4625',1);
INSERT INTO "schema_migrations" VALUES(13,'schema-12-canonical-path-identity','sha256:b7e957d0d2c3ffecf45c209658994bd69d1bf30b49e1ec96f4d5008c648a16a8',1);
CREATE TABLE v2_attempts (
    attempt_id                  TEXT PRIMARY KEY,
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
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
INSERT INTO "v2_attempts" VALUES('00000000-0000-4000-8000-000000000404','00000000-0000-4000-8000-000000000403','00000000-0000-4000-8000-000000000402','work','work',1,1,'active','valid',NULL,4,NULL,NULL);
CREATE TABLE v2_blockers (
    blocker_id                  TEXT PRIMARY KEY,
    attempt_id                  TEXT NOT NULL REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    reason                      TEXT NOT NULL,
    state                       TEXT NOT NULL CHECK (state IN ('open', 'resolved')),
    created_at_ms               INTEGER NOT NULL,
    resolved_at_ms              INTEGER,
    CHECK ((state = 'open' AND resolved_at_ms IS NULL) OR (state = 'resolved' AND resolved_at_ms IS NOT NULL))
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
CREATE TABLE v2_decision_records (
    attempt_id                  TEXT PRIMARY KEY REFERENCES v2_attempts(attempt_id) ON DELETE CASCADE,
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
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
CREATE TABLE v2_delegation_receipts (
    attempt_id   TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    receipt_id   TEXT NOT NULL UNIQUE,
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json) AND length(CAST(receipt_json AS BLOB)) <= 45000),
    FOREIGN KEY (session_id, attempt_id) REFERENCES v2_attempts(session_id, attempt_id) ON DELETE CASCADE
) STRICT;
INSERT INTO "v2_delegation_receipts" VALUES('00000000-0000-4000-8000-000000000404','00000000-0000-4000-8000-000000000403','00000000-0000-4000-8000-000000000405','{"accepted":true,"accepted_at":"2026-09-17T00:00:00.000Z","accepted_items_digest":"sha256:9c18719049a235bff24e61323e212ce320777ee717e2ac34fe660769b1e14b57","attempt_id":"00000000-0000-4000-8000-000000000404","delegation":{"after_accept":"retain","binding_ref":"local-specialist","max_rounds":3,"role_ref":"reviewer","role_snapshot_digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"execution":{"accepted_task_digest":"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","adapter_id":"adapter","adapter_version":"1","agent_configuration_digest":"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","binding_ref":"local-specialist","primary_host":"external","role_ref":"reviewer","role_snapshot_digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","scope_kind":"session","scope_ref":"scope-1","supervisor_ref":"supervisor-1","task_ref":"task-1","worker_ref":"worker-1"},"package_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","placement_id":"work","procedure_digest":"sha256:5a0ad8a6d7f8671a69f6c41349cb01d210d790a6dc2869968e708926cd2a83e6","receipt_id":"00000000-0000-4000-8000-000000000405","result_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","rounds_used":1,"schema":"podway.delegation-receipt/v1","session_id":"00000000-0000-4000-8000-000000000403","summary":"Reviewed result","worker_disposition":"idle","workspace_uuid":"00000000-0000-4000-8000-000000000401"}');
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
CREATE TABLE v2_goal_revisions (
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
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
CREATE TABLE v2_graph_node_counters (
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
    snapshot_id                 TEXT NOT NULL,
    graph_node_id               TEXT NOT NULL,
    attempt_count               INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    rework_traversal_count      INTEGER NOT NULL DEFAULT 0 CHECK (rework_traversal_count >= 0),
    PRIMARY KEY (session_id, graph_node_id),
    FOREIGN KEY (snapshot_id, graph_node_id) REFERENCES v2_graph_nodes(snapshot_id, graph_node_id) ON DELETE RESTRICT
) STRICT;
INSERT INTO "v2_graph_node_counters" VALUES('00000000-0000-4000-8000-000000000403','00000000-0000-4000-8000-000000000402','work',1,0);
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
INSERT INTO "v2_graph_nodes" VALUES('00000000-0000-4000-8000-000000000402','work','work',0,'action',0,'{"delegation":{"after_accept":"retain","binding_ref":"local-specialist","max_rounds":3,"role_ref":"reviewer","role_snapshot_digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"id":"work","terminal":true,"use":"work"}');
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
INSERT INTO "v2_procedure_snapshots" VALUES('00000000-0000-4000-8000-000000000402','podway.procedure/v2','delegated-receipt','1','Delegated receipt','Verify immutable receipt persistence.','sha256:5a0ad8a6d7f8671a69f6c41349cb01d210d790a6dc2869968e708926cd2a83e6','{"graph":{"entry":"work","nodes":[{"delegation":{"after_accept":"retain","binding_ref":"local-specialist","max_rounds":3,"role_ref":"reviewer","role_snapshot_digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"id":"work","terminal":true,"use":"work"}]},"id":"delegated-receipt","name":"Delegated receipt","node_definitions":{"work":{"intent":"Complete the work.","title":"Work","type":"action"}},"purpose":"Verify immutable receipt persistence.","schema":"podway.procedure/v2","version":"1"}','file','delegated.yaml',0,2);
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
CREATE TABLE v2_rework_records (
    session_id                  TEXT NOT NULL REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
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
INSERT INTO "v2_task_sessions" VALUES('00000000-0000-4000-8000-000000000403','current',NULL,NULL,NULL,'Delegated task','00000000-0000-4000-8000-000000000402','running',2,1,'work','00000000-0000-4000-8000-000000000404',1,0,NULL,3,NULL,NULL,NULL);
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
CREATE TABLE v2_workspace_state (
    singleton               INTEGER PRIMARY KEY CHECK (singleton = 1),
    current_session_id      TEXT NOT NULL UNIQUE REFERENCES v2_task_sessions(session_id) ON DELETE CASCADE,
    workspace_revision      INTEGER NOT NULL DEFAULT 0 CHECK (workspace_revision >= 0),
    FOREIGN KEY (singleton) REFERENCES workspace_state(singleton) ON DELETE CASCADE
) STRICT;
INSERT INTO "v2_workspace_state" VALUES(1,'00000000-0000-4000-8000-000000000403',3);
CREATE TABLE workspace_state (
    singleton                   INTEGER PRIMARY KEY CHECK (singleton = 1),
    workspace_uuid              TEXT NOT NULL UNIQUE,
    git_common_fingerprint      TEXT NOT NULL,
    git_worktree_fingerprint    TEXT NOT NULL,
    last_validated_root         TEXT NOT NULL,
    next_workspace_sequence     INTEGER NOT NULL DEFAULT 0 CHECK (next_workspace_sequence >= 0),
    created_at_ms               INTEGER NOT NULL,
    updated_at_ms               INTEGER NOT NULL
, runtime_mode TEXT NOT NULL DEFAULT 'prod'
CHECK (
    length(runtime_mode) BETWEEN 1 AND 64
    AND runtime_mode NOT GLOB '*[^a-z0-9-]*'
    AND runtime_mode GLOB '[a-z]*'
    AND runtime_mode NOT GLOB '*--*'
    AND substr(runtime_mode, -1, 1) <> '-'
)) STRICT;
INSERT INTO "workspace_state" VALUES(1,'00000000-0000-4000-8000-000000000401','sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','podway.unix-path/v1:2f746d702f706f647761792d76327364673030342d72656365697074',1,1,7,'prod');
CREATE INDEX ix_jobs_state_sequence
ON jobs(state, workspace_sequence);
CREATE INDEX ix_jobs_terminal_time
ON jobs(finished_at_ms)
WHERE state IN ('succeeded', 'failed', 'cancelled');
CREATE INDEX ix_idempotency_scope
ON idempotency_records(scope_kind, scope_session_id, created_at_ms);
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
DELETE FROM "sqlite_sequence";
COMMIT;
PRAGMA user_version=13;
