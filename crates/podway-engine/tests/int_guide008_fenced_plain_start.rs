//! GUIDE-008: fenced plain start verified inside the Engine.
//!
//! Plain `session.start` accepts the paired optional `--if-session-id` and
//! `--if-session-revision` fences (`AUT-LIF-016`, ADR-0039 decision 6).
//! The Engine verifies supplied fences inside the start writer transaction
//! after the idempotency lookup, so an exact replay returns its original
//! success even after the original session was archived or replaced, and a
//! fresh mismatch returns the fenced replacement path's identity or revision
//! error with no effect and no outcome. Lifecycle semantics stay those of
//! plain start, and a fenced dry run verifies the fences and the lifecycle
//! read-only instead of taking the offline preview shortcut.
//!
//! Every lifecycle state is driven through real engine commands on a real
//! native Store fixture (`fixture()` from `int_v2dfc006_direct_execution`).

use podway_core::{Revision, SessionId, SessionLifecycle};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{DirectCursorCommandV1, NativeDirectIdSourceV1},
    direct_start::DirectStartCommandV1,
    execution::{PreparedProcedureSourceV2, prepare_workspace_procedure_source_v2},
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{NativeGitResolverV1, WorktreeSelectorV1};
use podway_protocol::{
    ProcedureV2MutationCommandV1, RequestIdV1, Rfc3339MillisV1, SessionBeginV1, SessionCancelV1,
    SessionCompleteV1, SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1,
    SessionStartCommandV1, SessionStartSourceV1, SessionStartV1, SessionTerminalDispositionV1,
    TerminalDispositionInputV1, validate_daemon_free_document_v1,
};
use podway_store::{GraphSessionStateV2, IdempotencyKeyV1, direct_store::DirectStoreV1};
use serde_json::Value;
use tempfile::TempDir;

use super::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

const WORK_PROCEDURE: &[u8] =
    br#"{"schema":"podway.procedure/v2","id":"guide008-work","version":"1",
    "name":"Guide008 work","purpose":"Exercise fenced plain start lifecycle states.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work."}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

fn other_session_id() -> SessionId {
    SessionId::new("00000000-0000-4000-8000-0000000000e8").unwrap()
}

struct Guide008Engine {
    _temporary: TempDir,
    selector: WorktreeSelectorV1,
    store: DirectStoreV1,
    resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
}

impl Guide008Engine {
    /// A workspace fixture with the shared Procedure source and no session.
    fn bare() -> Self {
        let (temporary, selector, store, _) = fixture();
        std::fs::write(temporary.path().join("work.json"), WORK_PROCEDURE).unwrap();
        Self {
            _temporary: temporary,
            selector,
            store,
            resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
        }
    }

    fn admitted() -> PreparedProcedureSourceV2 {
        prepare_workspace_procedure_source_v2("work.json", WORK_PROCEDURE).unwrap()
    }

    fn plain_start(
        &self,
        fences: Option<SessionIdentityPreconditionsWireV1>,
        dry_run: bool,
    ) -> DirectCommandV1 {
        DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "work.json".into(),
                },
                expected_procedure_digest: Some(Self::admitted().digest().clone()),
                task_title: "Guide008 fenced successor".into(),
                dry_run,
            },
            preconditions: fences,
        }))
    }

    fn request(&self, command: DirectCommandV1, key: Option<&str>) -> DirectRequestV1 {
        DirectRequestV1 {
            selector: self.selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000098").unwrap(),
            command,
            idempotency_key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
            timeout_ms: 5000,
            generation: podway_protocol::ContractGenerationV1::Generation1,
        }
    }

    fn execute(&self, command: DirectCommandV1, key: Option<&str>) -> DirectResponseV1 {
        let response = execute_direct_v1(
            &self.resolver,
            self.request(command, key),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        match &response {
            DirectResponseV1::Success(output) => assert!(validate_daemon_free_document_v1(
                "podway.output/v4",
                output.as_value()
            )),
            DirectResponseV1::Failure(error) => assert!(validate_daemon_free_document_v1(
                "podway.error/v2",
                &error.clone().into_value()
            )),
        }
        response
    }

    fn success(&self, command: DirectCommandV1, key: Option<&str>) -> Value {
        match self.execute(command, key) {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => {
                panic!("unexpected Engine rejection: {}", error.into_value())
            }
        }
    }

    fn failure(&self, command: DirectCommandV1, key: Option<&str>) -> Value {
        match self.execute(command, key) {
            DirectResponseV1::Failure(error) => error.into_value(),
            DirectResponseV1::Success(output) => {
                panic!("unexpected Engine success: {}", output.as_value())
            }
        }
    }

    fn has_current_session(&self) -> bool {
        self.store.prepare_graph(deadline()).unwrap().is_some()
    }

    fn graph(&self) -> GraphSessionStateV2 {
        self.store
            .prepare_graph(deadline())
            .unwrap()
            .expect("the scenario keeps a current session")
            .graph()
            .clone()
    }

    fn sequence(&self) -> u64 {
        self.store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap()
    }

    fn identity(&self) -> SessionIdentityPreconditionsWireV1 {
        let graph = self.graph();
        SessionIdentityPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
        }
    }

    fn fences(&self) -> SessionMutationPreconditionsWireV1 {
        let graph = self.graph();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph
                .trace()
                .active_attempt()
                .expect("the running scenario has an active attempt")
                .attempt_id()
                .clone(),
        }
    }

    fn outcome_found(&self, key: &str) -> bool {
        self.store
            .read(deadline(), |snapshot| {
                snapshot.outcome(&IdempotencyKeyV1::new(key).unwrap())
            })
            .unwrap()
            .is_some()
    }

    fn lookup(&self, key: &str) -> Value {
        self.success(
            DirectCommandV1::OutcomeLookup {
                key: IdempotencyKeyV1::new(key).unwrap(),
            },
            None,
        )
    }

    /// An unfenced plain start; the state driver for the prepared scenario and
    /// the unfenced side of the key-reuse matrix.
    fn start_unfenced(&self, key: &str) -> Value {
        self.success(self.plain_start(None, false), Some(key))
    }

    fn begin(&self, key: &str) {
        let session_id = self.graph().trace().session_id().clone();
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal: None,
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: session_id,
                        expected_session_revision: Revision::ZERO,
                    },
                },
            )),
            Some(key),
        );
    }

    fn complete(&self, key: &str) {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: self.fences(),
            })),
            Some(key),
        );
    }

    fn cancel(&self, key: &str) {
        self.success(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
                reason: "Cancelled for the GUIDE-008 matrix.".into(),
                preconditions: self.fences(),
            })),
            Some(key),
        );
    }

    fn dispose(&self, key: &str) {
        self.success(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                SessionTerminalDispositionV1 {
                    disposition: TerminalDispositionInputV1::NotRequired {
                        reason: "No external hand-off required.".into(),
                    },
                    actor: None,
                    preconditions: self.identity(),
                },
            )),
            Some(key),
        );
    }

    fn prepared() -> Self {
        let engine = Self::bare();
        engine.start_unfenced("guide008-seed-start");
        engine
    }

    fn running() -> Self {
        let engine = Self::prepared();
        engine.begin("guide008-seed-begin");
        engine
    }

    fn completed_without_disposition() -> Self {
        let engine = Self::running();
        engine.complete("guide008-seed-complete");
        engine
    }

    fn cancelled_without_disposition() -> Self {
        let engine = Self::running();
        engine.cancel("guide008-seed-cancel");
        engine
    }

    fn completed_with_disposition() -> Self {
        let engine = Self::completed_without_disposition();
        engine.dispose("guide008-seed-disposition");
        engine
    }

    fn cancelled_with_disposition() -> Self {
        let engine = Self::cancelled_without_disposition();
        engine.dispose("guide008-seed-disposition");
        engine
    }
}

#[test]
fn guide008_fenced_plain_start_lifecycle_matrix_and_no_session() {
    // No session: the fence's premise fails closed with the replacement
    // path's identity error, and nothing is created.
    let engine = Guide008Engine::bare();
    let fences = SessionIdentityPreconditionsWireV1 {
        expected_session_id: other_session_id(),
        expected_session_revision: Revision::ZERO,
    };
    let error = engine.failure(
        engine.plain_start(Some(fences), false),
        Some("matrix-no-session"),
    );
    assert_eq!(error["code"], "SESSION_ID_MISMATCH", "{error}");
    assert_eq!(error["exit_code"], 4);
    assert_eq!(
        error["details"]["schema"],
        "podway.session-id-mismatch-details/v3"
    );
    assert_eq!(
        error["details"]["expected_session_id"],
        other_session_id().as_str()
    );
    assert_eq!(error["details"]["actual_session_id"], Value::Null);
    assert!(!engine.has_current_session());
    assert!(!engine.outcome_found("matrix-no-session"));

    // Every existing non-disposed state keeps plain-start semantics: the
    // decision error, an untouched session, and no outcome.
    for (label, engine) in [
        ("prepared", Guide008Engine::prepared()),
        ("running", Guide008Engine::running()),
        (
            "completed-without-disposition",
            Guide008Engine::completed_without_disposition(),
        ),
        (
            "cancelled-without-disposition",
            Guide008Engine::cancelled_without_disposition(),
        ),
    ] {
        let identity = engine.identity();
        let before = engine.graph();
        let key = format!("matrix-{label}");
        let error = engine.failure(engine.plain_start(Some(identity), false), Some(&key));
        assert_eq!(
            error["code"], "SESSION_START_DECISION_REQUIRED",
            "{label}: {error}"
        );
        assert_eq!(error["exit_code"], 5, "{label}");
        assert_eq!(
            error["details"]["current_terminal_disposition"], false,
            "{label}"
        );
        assert_eq!(
            engine.graph(),
            before,
            "{label}: the session is never deleted"
        );
        assert!(!engine.outcome_found(&key), "{label}");
    }

    // The two disposed terminal states archive the matching predecessor and
    // start the successor.
    for (label, engine) in [
        (
            "completed-with-disposition",
            Guide008Engine::completed_with_disposition(),
        ),
        (
            "cancelled-with-disposition",
            Guide008Engine::cancelled_with_disposition(),
        ),
    ] {
        let identity = engine.identity();
        let predecessor = engine.graph().trace().session_id().clone();
        let key = format!("matrix-{label}");
        let output = engine.success(engine.plain_start(Some(identity), false), Some(&key));
        assert_eq!(output["command"], "session.start", "{label}");
        assert_eq!(
            output["result"]["existing_session"]["action"], "archive",
            "{label}"
        );
        assert_eq!(
            output["result"]["existing_session"]["session_id"],
            predecessor.as_str(),
            "{label}"
        );
        let successor = engine.graph();
        assert_ne!(successor.trace().session_id(), &predecessor, "{label}");
        assert_eq!(
            successor.trace().lifecycle(),
            SessionLifecycle::Prepared,
            "{label}"
        );
        assert_eq!(successor.trace().revision(), Revision::ZERO, "{label}");
        assert!(engine.outcome_found(&key), "{label}");
    }
}

#[test]
fn guide008_fenced_plain_start_rejects_stale_identity_and_revision() {
    let engine = Guide008Engine::completed_with_disposition();
    let identity = engine.identity();
    let before = engine.graph();

    let mut stale_identity = identity.clone();
    stale_identity.expected_session_id = other_session_id();
    let error = engine.failure(
        engine.plain_start(Some(stale_identity.clone()), false),
        Some("stale-identity"),
    );
    assert_eq!(error["code"], "SESSION_ID_MISMATCH", "{error}");
    assert_eq!(error["exit_code"], 4);
    assert_eq!(
        error["details"]["schema"],
        "podway.session-id-mismatch-details/v3"
    );
    assert_eq!(
        error["details"]["expected_session_id"],
        other_session_id().as_str()
    );
    assert_eq!(
        error["details"]["actual_session_id"],
        identity.expected_session_id.as_str()
    );
    assert_eq!(engine.graph(), before, "a stale identity has no effect");
    assert!(!engine.outcome_found("stale-identity"));

    let mut stale_revision = identity.clone();
    stale_revision.expected_session_revision =
        Revision::new(identity.expected_session_revision.get() + 1);
    let error = engine.failure(
        engine.plain_start(Some(stale_revision), false),
        Some("stale-revision"),
    );
    assert_eq!(error["code"], "SESSION_REVISION_CONFLICT", "{error}");
    assert_eq!(error["exit_code"], 4);
    assert_eq!(
        error["details"]["schema"],
        "podway.revision-conflict-details/v3"
    );
    assert_eq!(
        error["details"]["expected_revision"],
        identity.expected_session_revision.get() + 1
    );
    assert_eq!(
        error["details"]["current_revision"],
        identity.expected_session_revision.get()
    );
    assert_eq!(engine.graph(), before, "a stale revision has no effect");
    assert!(!engine.outcome_found("stale-revision"));
}

#[test]
fn guide008_fenced_plain_start_replay_and_lookup_survive_successor_archival_and_replacement() {
    let engine = Guide008Engine::completed_with_disposition();
    let identity = engine.identity();
    let original = engine.success(
        engine.plain_start(Some(identity.clone()), false),
        Some("kept"),
    );
    assert_eq!(original["command"], "session.start");

    // The exact replay skips the fence check because the canonical identity
    // still matches the stored outcome.
    let replay = engine.success(
        engine.plain_start(Some(identity.clone()), false),
        Some("kept"),
    );
    assert_eq!(
        replay, original,
        "exact replay returns the original success"
    );
    let lookup = engine.lookup("kept");
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["scope"], "session");
    assert_eq!(lookup["result"]["outcome"]["response"], original);

    // A fresh key fenced against the archived predecessor mismatches against
    // the successor that the fenced start created.
    let error = engine.failure(
        engine.plain_start(Some(identity.clone()), false),
        Some("fresh-old-fences"),
    );
    assert_eq!(error["code"], "SESSION_ID_MISMATCH", "{error}");

    // Archive the session the kept key created, then repeat both reads.
    engine.begin("successor-begin");
    engine.complete("successor-complete");
    engine.dispose("successor-disposition");
    let successor_identity = engine.identity();
    engine.success(
        engine.plain_start(Some(successor_identity), false),
        Some("successor-start"),
    );
    let replay = engine.success(engine.plain_start(Some(identity), false), Some("kept"));
    assert_eq!(
        replay, original,
        "exact replay survives the successor's archival"
    );
    let lookup = engine.lookup("kept");
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], original);
}

#[test]
fn guide008_fenced_plain_start_same_key_different_fences_is_key_reuse() {
    let engine = Guide008Engine::completed_with_disposition();
    let identity = engine.identity();
    let original = engine.success(
        engine.plain_start(Some(identity.clone()), false),
        Some("reused"),
    );
    let before = engine.graph();

    let mut other_id = identity.clone();
    other_id.expected_session_id = other_session_id();
    let reuse_cases: [(&str, Option<SessionIdentityPreconditionsWireV1>); 2] =
        [("other-id", Some(other_id)), ("none", None)];
    for (label, fences) in reuse_cases {
        let error = engine.failure(engine.plain_start(fences, false), Some("reused"));
        assert_eq!(error["code"], "IDEMPOTENCY_KEY_REUSED", "{label}: {error}");
        assert_eq!(error["exit_code"], 2, "{label}");
        assert_eq!(engine.graph(), before, "{label}: reuse has no effect");
    }
    let mut other_revision = identity.clone();
    other_revision.expected_session_revision =
        Revision::new(identity.expected_session_revision.get() + 7);
    let error = engine.failure(
        engine.plain_start(Some(other_revision), false),
        Some("reused"),
    );
    assert_eq!(error["code"], "IDEMPOTENCY_KEY_REUSED", "{error}");
    assert_eq!(engine.graph(), before);

    // The other direction: an unfenced success reused with fences.
    let unfenced = Guide008Engine::bare();
    let unfenced_original = unfenced.start_unfenced("unfenced-reused");
    let before_unfenced = unfenced.graph();
    let error = unfenced.failure(
        unfenced.plain_start(Some(identity.clone()), false),
        Some("unfenced-reused"),
    );
    assert_eq!(error["code"], "IDEMPOTENCY_KEY_REUSED", "{error}");
    assert_eq!(unfenced.graph(), before_unfenced);
    let unfenced_lookup = unfenced.lookup("unfenced-reused");
    assert_eq!(unfenced_lookup["result"]["found"], true);
    assert_eq!(
        unfenced_lookup["result"]["outcome"]["response"],
        unfenced_original
    );
    assert_eq!(
        unfenced.start_unfenced("unfenced-reused"),
        unfenced_original,
        "the unfenced original still replays exactly"
    );

    // The original outcome is retained and still replays exactly.
    let replay = engine.success(engine.plain_start(Some(identity), false), Some("reused"));
    assert_eq!(replay, original);
    let lookup = engine.lookup("reused");
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], original);
}

#[test]
fn guide008_fenced_plain_start_dry_run_verifies_fences_and_lifecycle_read_only() {
    // A matching disposed terminal session previews the archive read-only.
    let engine = Guide008Engine::completed_with_disposition();
    let identity = engine.identity();
    let before = engine.graph();
    let sequence = engine.sequence();
    let preview = engine.success(engine.plain_start(Some(identity.clone()), true), None);
    assert_eq!(preview["command"], "session.start");
    assert_eq!(preview["result"]["dry_run"], true);
    assert_eq!(preview["result"]["existing_session"]["action"], "archive");
    assert!(preview.get("session").map(Value::is_null).unwrap_or(true));
    assert_eq!(engine.graph(), before, "the dry run has no effect");
    assert_eq!(engine.sequence(), sequence, "the dry run commits nothing");

    // Stale fences and non-disposed states report the same refusals read-only.
    let mut stale_revision = identity.clone();
    stale_revision.expected_session_revision =
        Revision::new(identity.expected_session_revision.get() + 1);
    let error = engine.failure(engine.plain_start(Some(stale_revision), true), None);
    assert_eq!(error["code"], "SESSION_REVISION_CONFLICT", "{error}");
    assert_eq!(engine.graph(), before);

    let prepared = Guide008Engine::prepared();
    let before = prepared.graph();
    let error = prepared.failure(prepared.plain_start(Some(prepared.identity()), true), None);
    assert_eq!(error["code"], "SESSION_START_DECISION_REQUIRED", "{error}");
    assert_eq!(prepared.graph(), before);

    let empty = Guide008Engine::bare();
    let error = empty.failure(
        empty.plain_start(
            Some(SessionIdentityPreconditionsWireV1 {
                expected_session_id: other_session_id(),
                expected_session_revision: Revision::ZERO,
            }),
            true,
        ),
        None,
    );
    assert_eq!(error["code"], "SESSION_ID_MISMATCH", "{error}");
    assert!(!empty.has_current_session());
}
