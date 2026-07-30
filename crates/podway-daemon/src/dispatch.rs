//! Production routing for the complete G006 daemon command contract.
//!
//! The dispatcher has no Store, SQLite, Git, configuration, or scheduler-registry authority. Those
//! capabilities remain behind the injected runtime seams below. In particular, a workspace value is
//! opaque to this module, so routing cannot accidentally turn a display path into an identity key.

use podway_core::{AttemptId, JobId, Revision, SessionId, Sha256Digest, WorkspaceId};
use podway_protocol::{
    CompactStatusResultV1, ErrorCodeV1, ErrorEnvelopeInputV1, ErrorEnvelopeV1, ExitCodeV1,
    IdempotencyKeyV1, JobOutputV1, JobStateV1, NextResultV1, OperationV1, OutputEnvelopeInputV1,
    OutputEnvelopeV1, QueryWaitV1, RequestEnvelopeV1, ResponseEnvelopeV1, Rfc3339MillisV1,
    SessionOutputV1, SliceCommandV1, SliceRequestV1, StatusResultV1, WorkspaceOutputV1,
    WorktreeSelectorWireV1,
};
use serde_json::{Map, Value, json};

use crate::server::RequestDispatcherV1;

/// The longest diagnostic that may be emitted by this dispatcher.
///
/// All catalog messages are static. Dynamic Store, SQLite, Git, filesystem, and configuration
/// errors are deliberately represented only by [`DispatchFailureKindV1`] and never reach a public
/// envelope.
/// `workspace.reset_all` is synchronous maintenance; custom mutation waiting is unsupported.
pub const RESET_ALL_WAIT_TIMEOUT_MILLIS_V1: u64 = 30_000;

pub const MAX_PUBLIC_DIAGNOSTIC_SCALARS_V1: usize = 512;

/// A safe, structured subset of error details that a dispatcher may expose.
///
/// The fields intentionally exclude paths, SQL text, arbitrary error strings, and configuration
/// values. This keeps correlation and retry information useful without making diagnostic output an
/// accidental data-exfiltration channel.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DispatchErrorDetailsV1 {
    job_id: Option<JobId>,
    job_sequence: Option<u64>,
    expected_revision: Option<Revision>,
    current_revision: Option<Revision>,
    attempt_mismatch: Option<Box<AttemptMismatchDetailsV1>>,
    identity_conflict: Option<Box<IdentityConflictDetailsV1>>,
    procedure_digest_mismatch: Option<Box<ProcedureDigestMismatchDetailsV1>>,
    outcome_unknown_key: Option<Box<IdempotencyKeyV1>>,
    maximum_open_blockers: Option<Box<usize>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IdentityConflictDetailsV1 {
    Workspace {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    Session {
        expected: SessionId,
        actual: Option<SessionId>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttemptMismatchDetailsV1 {
    expected: AttemptId,
    actual: Option<AttemptId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcedureDigestMismatchDetailsV1 {
    expected: Sha256Digest,
    actual: Sha256Digest,
}

impl DispatchErrorDetailsV1 {
    pub fn with_unknown_outcome(mut self, idempotency_key: IdempotencyKeyV1) -> Self {
        self.outcome_unknown_key = Some(Box::new(idempotency_key));
        self
    }

    pub fn with_admission_identity(mut self, job_id: JobId, job_sequence: u64) -> Self {
        self.job_id = Some(job_id);
        self.job_sequence = Some(job_sequence);
        self
    }

    pub fn with_job(mut self, job: &JobOutputV1) -> Self {
        self.job_id = Some(job.id().clone());
        self.job_sequence = Some(job.sequence());
        self
    }

    pub fn with_expected_revision(mut self, revision: Revision) -> Self {
        self.expected_revision = Some(revision);
        self
    }

    pub fn with_current_revision(mut self, revision: Revision) -> Self {
        self.current_revision = Some(revision);
        self
    }

    pub fn with_attempt_mismatch(mut self, expected: AttemptId, actual: Option<AttemptId>) -> Self {
        self.attempt_mismatch = Some(Box::new(AttemptMismatchDetailsV1 { expected, actual }));
        self
    }

    pub fn with_workspace_uuid_mismatch(
        mut self,
        expected: WorkspaceId,
        actual: WorkspaceId,
    ) -> Self {
        self.identity_conflict = Some(Box::new(IdentityConflictDetailsV1::Workspace {
            expected,
            actual,
        }));
        self
    }

    pub fn with_session_id_mismatch(
        mut self,
        expected: SessionId,
        actual: Option<SessionId>,
    ) -> Self {
        self.identity_conflict = Some(Box::new(IdentityConflictDetailsV1::Session {
            expected,
            actual,
        }));
        self
    }

    pub fn with_procedure_digest_mismatch(
        mut self,
        expected: Sha256Digest,
        actual: Sha256Digest,
    ) -> Self {
        self.procedure_digest_mismatch = Some(Box::new(ProcedureDigestMismatchDetailsV1 {
            expected,
            actual,
        }));
        self
    }

    pub fn with_blocker_limit(mut self, maximum_open_blockers: usize) -> Self {
        self.maximum_open_blockers = Some(Box::new(maximum_open_blockers));
        self
    }

    pub(crate) fn into_json(self, requires_admission: bool) -> Map<String, Value> {
        if let Some(idempotency_key) = self.outcome_unknown_key {
            return json!({
                "schema": "podway.mutation-outcome-unknown-details/v1",
                "outcome": "unknown",
                "idempotency_key": idempotency_key.as_str(),
                "reconcile": {
                    "command": "job.lookup",
                    "idempotency_key": idempotency_key.as_str(),
                },
            })
            .as_object()
            .expect("unknown-outcome details are an object")
            .clone();
        }
        if let Some(mismatch) = self.procedure_digest_mismatch {
            return Map::from_iter([
                (
                    "schema".to_owned(),
                    Value::String("podway.procedure-digest-mismatch-details/v1".to_owned()),
                ),
                (
                    "expected_procedure_digest".to_owned(),
                    Value::String(mismatch.expected.into_inner()),
                ),
                (
                    "actual_procedure_digest".to_owned(),
                    Value::String(mismatch.actual.into_inner()),
                ),
                ("admission".to_owned(), json!({"admitted": false})),
            ]);
        }
        if let Some(identity) = self.identity_conflict {
            let mut admission =
                Map::from_iter([("admitted".to_owned(), Value::Bool(self.job_id.is_some()))]);
            if let Some(job_id) = self.job_id {
                admission.insert("job_id".to_owned(), Value::String(job_id.into_inner()));
            }
            if let Some(job_sequence) = self.job_sequence {
                admission.insert("workspace_sequence".to_owned(), Value::from(job_sequence));
            }
            let (schema, expected_key, expected, actual_key, actual) = match *identity {
                IdentityConflictDetailsV1::Workspace { expected, actual } => (
                    "podway.workspace-uuid-mismatch-details/v1",
                    "expected_workspace_uuid",
                    Value::String(expected.into_inner()),
                    "actual_workspace_uuid",
                    Value::String(actual.into_inner()),
                ),
                IdentityConflictDetailsV1::Session { expected, actual } => (
                    "podway.session-id-mismatch-details/v1",
                    "expected_session_id",
                    Value::String(expected.into_inner()),
                    "actual_session_id",
                    actual
                        .map(SessionId::into_inner)
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                ),
            };
            return Map::from_iter([
                ("schema".to_owned(), Value::String(schema.to_owned())),
                (expected_key.to_owned(), expected),
                (actual_key.to_owned(), actual),
                ("admission".to_owned(), Value::Object(admission)),
            ]);
        }
        let admission =
            requires_admission.then(|| admission_value_v1(self.job_id.as_ref(), self.job_sequence));
        let mut details = Map::new();
        if let Some(job_id) = self.job_id {
            details.insert("job_id".to_owned(), Value::String(job_id.into_inner()));
        }
        if let Some(job_sequence) = self.job_sequence {
            details.insert("job_sequence".to_owned(), Value::from(job_sequence));
        }
        if let Some(expected_revision) = self.expected_revision {
            details.insert(
                "expected_revision".to_owned(),
                Value::from(expected_revision.get()),
            );
        }
        if let Some(current_revision) = self.current_revision {
            details.insert(
                "current_revision".to_owned(),
                Value::from(current_revision.get()),
            );
        }
        if let Some(attempt_mismatch) = self.attempt_mismatch {
            details.insert(
                "expected_attempt_id".to_owned(),
                Value::String(attempt_mismatch.expected.into_inner()),
            );
            if let Some(actual_attempt_id) = attempt_mismatch.actual {
                details.insert(
                    "actual_attempt_id".to_owned(),
                    Value::String(actual_attempt_id.into_inner()),
                );
            }
        }
        if let Some(maximum_open_blockers) = self.maximum_open_blockers {
            details.insert(
                "maximum_open_blockers".to_owned(),
                Value::from(u64::try_from(*maximum_open_blockers).expect("usize fits in u64")),
            );
        }
        if let Some(admission) = admission {
            details.insert("admission".to_owned(), admission);
        }
        details
    }
}

fn admission_value_v1(job_id: Option<&JobId>, workspace_sequence: Option<u64>) -> Value {
    match (job_id, workspace_sequence) {
        (Some(job_id), Some(workspace_sequence)) => json!({
            "admitted": true,
            "job_id": job_id,
            "workspace_sequence": workspace_sequence,
        }),
        (None, None) => json!({"admitted": false}),
        _ => unreachable!("admission identity is stored atomically"),
    }
}

fn job_admission_value_v1(job: &JobOutputV1) -> Value {
    admission_value_v1(Some(job.id()), Some(job.sequence()))
}

/// Stable public classifications used to bridge typed protocol, domain, Store, Git, and config
/// failures into the error catalog.
///
/// Infrastructure adapters translate their native error types into one of these classifications at
/// their own boundary. Keeping those native error types out of this module prevents Store protocol
/// coupling and prevents raw diagnostics from crossing the daemon response boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchFailureKindV1 {
    RequestInvalid,
    ProtocolVersionUnsupported,
    NotGitWorktree,
    BareGitRepository,
    WorktreeGone,
    WorkspaceNotInitialized,
    WorkspaceAlreadyInitialized,
    WorkspaceInitConflict,
    WorkspaceIdentityConflict,
    WorkspaceUuidMismatch,
    WorkspaceConfigInvalid,
    WorkspaceStateUnreadable,
    WorkspaceSchemaUnsupported,
    WorkspaceQueueFull,
    WorkspaceMaintenance,
    WorkspacePathUnsafe,
    PathOutsideWorktree,
    MigrationFailed,
    ProcedureNotFound,
    ProcedureInvalid,
    ProcedureSchemaUnsupported,
    ProcedureDigestMismatch,
    PresetNotFound,
    SessionNotFound,
    SessionIdMismatch,
    SessionAlreadyExists,
    SessionNotRunning,
    SessionNotCompleted,
    SessionCancelled,
    SessionRevisionConflict,
    AttemptNotCurrent,
    StageNotFound,
    StageNotSkippable,
    ReturnNotAllowed,
    ReopenNotAllowed,
    RequiredItemsMissing,
    BlockersPresent,
    BlockerLimitReached,
    ItemNotFound,
    ItemTypeMismatch,
    ItemConstraintFailed,
    ItemRevisionConflict,
    ItemAlreadySet,
    ListValueNotFound,
    ListValueDuplicate,
    ArtifactNotFound,
    ArtifactUnreadable,
    ArtifactChanged,
    ArtifactMediaTypeNotAllowed,
    BlockerNotFound,
    BlockerNotCurrent,
    IdempotencyKeyReused,
    MutationOutcomeUnknown,
    JobNotFound,
    JobNotCancellable,
    JobWaitTimeout,
    DaemonUnavailable,
    DaemonShuttingDown,
    Internal,
}

/// A typed, diagnostics-safe dispatcher failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchFailureV1 {
    kind: DispatchFailureKindV1,
    details: DispatchErrorDetailsV1,
}

impl DispatchFailureV1 {
    pub fn new(kind: DispatchFailureKindV1) -> Self {
        Self {
            kind,
            details: DispatchErrorDetailsV1 {
                job_id: None,
                job_sequence: None,
                expected_revision: None,
                current_revision: None,
                attempt_mismatch: None,
                identity_conflict: None,
                procedure_digest_mismatch: None,
                outcome_unknown_key: None,
                maximum_open_blockers: None,
            },
        }
    }

    pub fn with_details(mut self, details: DispatchErrorDetailsV1) -> Self {
        self.details = details;
        self
    }

    pub fn with_job(mut self, job: &JobOutputV1) -> Self {
        self.details = self.details.with_job(job);
        self
    }

    pub fn with_admission_identity(mut self, job_id: JobId, job_sequence: u64) -> Self {
        self.details = self.details.with_admission_identity(job_id, job_sequence);
        self
    }

    pub const fn kind(&self) -> DispatchFailureKindV1 {
        self.kind
    }

    pub fn with_kind(mut self, kind: DispatchFailureKindV1) -> Self {
        self.kind = kind;
        self
    }

    pub(crate) fn into_details(self) -> DispatchErrorDetailsV1 {
        self.details
    }
}

/// The catalog-controlled presentation for a public dispatcher error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchErrorPresentationV1 {
    code: ErrorCodeV1,
    message: &'static str,
    retryable: bool,
    exit_code: ExitCodeV1,
}

impl DispatchErrorPresentationV1 {
    pub fn catalog(kind: DispatchFailureKindV1) -> Self {
        let (code, message, retryable, exit_code) = catalog_error_spec_v1(kind);
        assert!(message.chars().count() <= MAX_PUBLIC_DIAGNOSTIC_SCALARS_V1);
        Self {
            code: ErrorCodeV1::new(code).expect("catalog error codes are valid"),
            message,
            retryable,
            exit_code: ExitCodeV1::new(exit_code).expect("catalog exit codes are valid"),
        }
    }
}

/// Converts typed, diagnostics-safe failures to their stable public catalog presentation.
pub trait DispatchErrorMapperV1: Send + Sync {
    fn map_failure(&self, failure: &DispatchFailureV1) -> DispatchErrorPresentationV1;
}

/// The production catalog mapper for dispatcher-owned failure classifications.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogDispatchErrorMapperV1;

impl DispatchErrorMapperV1 for CatalogDispatchErrorMapperV1 {
    fn map_failure(&self, failure: &DispatchFailureV1) -> DispatchErrorPresentationV1 {
        DispatchErrorPresentationV1::catalog(failure.kind())
    }
}

/// Supplies only the response timestamp required by dispatch responses.
pub trait DispatchResponseMetadataV1: Send + Sync {
    fn generated_at(&self) -> Rfc3339MillisV1;
}

/// A pre-serialized authoritative workspace projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherWorkspaceOutputV1 {
    workspace: WorkspaceOutputV1,
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl DispatcherWorkspaceOutputV1 {
    pub fn new(
        workspace: WorkspaceOutputV1,
        result: Map<String, Value>,
        warnings: Vec<Map<String, Value>>,
    ) -> Self {
        Self {
            workspace,
            result,
            warnings,
        }
    }
}

/// A read-only idempotency reconciliation result.
///
/// Reconciliation must represent a mutation that was never admitted before the workspace had a
/// durable UUID. In that case `workspace` is absent and `result.found` is false.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherReconciliationOutputV1 {
    workspace: Option<WorkspaceOutputV1>,
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl DispatcherReconciliationOutputV1 {
    pub fn new(
        workspace: Option<WorkspaceOutputV1>,
        result: Map<String, Value>,
        warnings: Vec<Map<String, Value>>,
    ) -> Self {
        Self {
            workspace,
            result,
            warnings,
        }
    }
}

/// An opaque workspace-routing authority.
///
/// Implementations must resolve the selector through Store UUID plus Git common-directory and
/// worktree-administration fingerprints. The opaque `Workspace` value lets the implementation
/// retain those validated identity facts without permitting this dispatcher to derive a key from a
/// root path or diagnostic display string.
pub trait WorkspaceRuntimeV1: Send + Sync {
    type Workspace: Send + Sync;

    fn resolve_existing(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1>;
    /// Resolves only through the runtime's non-mutating identity path.
    fn resolve_existing_readonly(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1>;

    fn resolve_bootstrap(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1>;

    fn workspace_output(&self, workspace: &Self::Workspace) -> WorkspaceOutputV1;
    /// Performs read-only workspace diagnostics after validated identity resolution.
    fn doctor(
        &self,
        selector: &WorktreeSelectorWireV1,
        deep: bool,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1>;

    /// Projects one workspace only after validated identity resolution.
    fn show(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1>;

    /// Repairs only daemon registry metadata after validated identity resolution.
    fn repair(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1>;
}

/// The request-derived wait policy for an authoritative read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestReadWaitV1 {
    Immediate,
    IdleUntil { timeout_millis: u64 },
    AfterJobUntil { job_id: JobId, timeout_millis: u64 },
}

impl RequestReadWaitV1 {
    fn from_query_wait(wait: &QueryWaitV1, timeout_millis: u64) -> Self {
        match wait {
            QueryWaitV1::Immediate => Self::Immediate,
            QueryWaitV1::Idle => Self::IdleUntil { timeout_millis },
            QueryWaitV1::AfterJob { job_id } => Self::AfterJobUntil {
                job_id: job_id.clone(),
                timeout_millis,
            },
        }
    }
}

/// Typed dispatcher input for `session.status`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherStatusRequestV1 {
    pub wait: RequestReadWaitV1,
    pub verbose: bool,
    pub expected_session_id: Option<SessionId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherNextRequestV1 {
    pub wait: RequestReadWaitV1,
    pub expected_session_id: Option<SessionId>,
}

/// A pre-serialized authoritative query projection.
///
/// The concrete read adapter owns conversion from the protocol's typed status/next DTOs. Its
/// result must be a protocol-valid JSON object; any invalid shape is converted to a bounded
/// internal error rather than reflected to the client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherReadOutputV1 {
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl DispatcherReadOutputV1 {
    pub fn new(result: Map<String, Value>, warnings: Vec<Map<String, Value>>) -> Self {
        Self { result, warnings }
    }
}
/// A pre-serialized authoritative named-job projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherJobOutputV1 {
    job: JobOutputV1,
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl DispatcherJobOutputV1 {
    pub fn new(
        job: JobOutputV1,
        result: Map<String, Value>,
        warnings: Vec<Map<String, Value>>,
    ) -> Self {
        Self {
            job,
            result,
            warnings,
        }
    }
}

/// Projects authoritative Store state for all daemon query routes. Notifications may wake
/// implementations, but they never establish a result without a Store read.
pub trait DispatcherReadServiceV1<Workspace>: Send + Sync {
    fn status(
        &self,
        workspace: &Workspace,
        input: DispatcherStatusRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1>;

    fn next(
        &self,
        workspace: &Workspace,
        input: DispatcherNextRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1>;

    fn job_list(
        &self,
        workspace: &Workspace,
        state: Option<JobStateV1>,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1>;

    fn job_lookup(
        &self,
        selector: &WorktreeSelectorWireV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<DispatcherReconciliationOutputV1, DispatchFailureV1>;

    fn job_status(
        &self,
        workspace: &Workspace,
        job_id: &JobId,
        wait: RequestReadWaitV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1>;
}

/// Performs a non-durable control operation through its sole workspace authority.
pub trait DispatcherControlServiceV1<Workspace>: Send + Sync {
    fn cancel_job(
        &self,
        workspace: &Workspace,
        job_id: &JobId,
        expected_state: JobStateV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1>;
}
/// Computes a non-durable preview from committed workspace state.
pub trait DispatcherPreviewServiceV1<Workspace>: Send + Sync {
    fn preview(
        &self,
        workspace: &Workspace,
        request: &SliceRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1>;
}

/// The request-derived behavior for a durably admitted mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationWaitV1 {
    Detached,
    UntilTerminal { timeout_millis: u64 },
}

impl MutationWaitV1 {
    fn from_request(request: &RequestEnvelopeV1) -> Self {
        if request.options().detach() {
            Self::Detached
        } else {
            Self::UntilTerminal {
                timeout_millis: request.options().wait_timeout_ms(),
            }
        }
    }
}

/// The immutable, successful terminal payload saved by the mutation worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatcherTerminalOutputV1 {
    session: Option<SessionOutputV1>,
    result: Map<String, Value>,
    warnings: Vec<Map<String, Value>>,
}

impl DispatcherTerminalOutputV1 {
    pub fn new(
        session: Option<SessionOutputV1>,
        result: Map<String, Value>,
        warnings: Vec<Map<String, Value>>,
    ) -> Self {
        Self {
            session,
            result,
            warnings,
        }
    }
}

/// An immutable terminal result for one previously admitted job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatcherTerminalResultV1 {
    Output(DispatcherTerminalOutputV1),
    Error(DispatchFailureV1),
}

/// The worker result after durable admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationDispatchOutcomeV1 {
    /// A durable admission receipt. This is the normal detached response.
    Detached {
        job: JobOutputV1,
        procedure_digest: Option<Sha256Digest>,
    },
    /// A persisted terminal result, which may be returned for synchronous requests or for a
    /// detached replay that discovered a job had already completed.
    Terminal {
        job: JobOutputV1,
        result: DispatcherTerminalResultV1,
        response_context: Option<Box<TerminalResponseContextV1>>,
    },
    /// A synchronous wait elapsed after admission. The worker must not cancel the job.
    TimedOut { job: JobOutputV1 },
}

/// Immutable response metadata captured for a terminal mutation receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalResponseContextV1 {
    request_id: podway_protocol::RequestIdV1,
    command: podway_protocol::CommandNameV1,
    generated_at: Rfc3339MillisV1,
    workspace: WorkspaceOutputV1,
}

impl TerminalResponseContextV1 {
    pub fn new(
        request_id: podway_protocol::RequestIdV1,
        command: podway_protocol::CommandNameV1,
        generated_at: Rfc3339MillisV1,
        workspace: WorkspaceOutputV1,
    ) -> Self {
        Self {
            request_id,
            command,
            generated_at,
            workspace,
        }
    }

    pub fn request_id(&self) -> &podway_protocol::RequestIdV1 {
        &self.request_id
    }
}

/// Reconstructs the canonical terminal wire envelope from an immutable Store receipt.
pub fn terminal_response_envelope_v1(
    context: TerminalResponseContextV1,
    job: JobOutputV1,
    result: DispatcherTerminalResultV1,
    bootstrap: bool,
) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
    terminal_response_envelope_with_mapper_v1(
        context,
        job,
        result,
        bootstrap,
        &CatalogDispatchErrorMapperV1,
    )
}

fn terminal_response_envelope_with_mapper_v1(
    context: TerminalResponseContextV1,
    job: JobOutputV1,
    result: DispatcherTerminalResultV1,
    bootstrap: bool,
    errors: &impl DispatchErrorMapperV1,
) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
    match result {
        DispatcherTerminalResultV1::Output(output) if bootstrap && output.session.is_some() => {
            Err(DispatchFailureV1::new(DispatchFailureKindV1::Internal))
        }
        DispatcherTerminalResultV1::Output(mut output) => {
            output
                .result
                .insert("admission".to_owned(), job_admission_value_v1(&job));
            OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
                request_id: context.request_id,
                command: context.command,
                generated_at: context.generated_at,
                workspace: Some(context.workspace),
                job: Some(job),
                session: output.session,
                result: output.result,
                warnings: output.warnings,
            })
            .map(ResponseEnvelopeV1::Output)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
        }
        DispatcherTerminalResultV1::Error(failure) => {
            let failure = failure.with_job(&job);
            let presentation = errors.map_failure(&failure);
            let workspace = serde_json::to_value(&context.workspace)
                .ok()
                .and_then(|value| value.as_object().cloned());
            ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
                request_id: context.request_id,
                command: context.command,
                generated_at: context.generated_at,
                code: presentation.code,
                message: presentation.message.to_owned(),
                retryable: presentation.retryable,
                exit_code: presentation.exit_code,
                workspace,
                details: failure.into_details().into_json(true),
            })
            .map(ResponseEnvelopeV1::Error)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
        }
    }
}

/// Immutable public correlation captured before durable admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationResponseContextV1 {
    request_id: podway_protocol::RequestIdV1,
    command: podway_protocol::CommandNameV1,
    workspace: WorkspaceOutputV1,
}

impl MutationResponseContextV1 {
    pub fn new(request: &RequestEnvelopeV1, workspace: WorkspaceOutputV1) -> Self {
        Self {
            request_id: request.request_id().clone(),
            command: request.command().clone(),
            workspace,
        }
    }

    pub fn request_id(&self) -> &podway_protocol::RequestIdV1 {
        &self.request_id
    }

    pub fn command(&self) -> &podway_protocol::CommandNameV1 {
        &self.command
    }

    pub fn workspace(&self) -> &WorkspaceOutputV1 {
        &self.workspace
    }
}

/// Admits a mutation durably, wakes any worker, and optionally waits for its immutable terminal
/// Store result. It must honor idempotency itself; a repeated key returns the original job/result.
pub trait MutationAdmissionWorkerV1<Workspace>: Send + Sync {
    fn admit_and_wait(
        &self,
        workspace: &Workspace,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        response_context: &MutationResponseContextV1,
        wait: MutationWaitV1,
    ) -> Result<MutationDispatchOutcomeV1, DispatchFailureV1>;
    /// Completes reset-all through the daemon maintenance state machine. This path receives no
    /// resolved workspace because reset must run before ordinary Store bootstrap, resolution, and
    /// admission.
    fn reset_all(
        &self,
        selector: &WorktreeSelectorWireV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        response_request_id: &podway_protocol::RequestIdV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1>;
}

/// Production adapter for valid G006 requests.
///
/// The server parses and admits [`SliceRequestV1`] before invoking this type. The dispatcher still
/// verifies request/slice command agreement as a defense against incorrect in-process wiring.
pub struct RequestDispatcherV1Adapter<
    Runtime,
    Reads,
    Controls,
    Previews,
    Mutations,
    Metadata,
    Errors,
> {
    runtime: Runtime,
    reads: Reads,
    controls: Controls,
    previews: Previews,
    mutations: Mutations,
    metadata: Metadata,
    errors: Errors,
}

impl<Runtime, Reads, Controls, Previews, Mutations, Metadata, Errors>
    RequestDispatcherV1Adapter<Runtime, Reads, Controls, Previews, Mutations, Metadata, Errors>
{
    pub fn new(
        runtime: Runtime,
        reads: Reads,
        controls: Controls,
        previews: Previews,
        mutations: Mutations,
        metadata: Metadata,
        errors: Errors,
    ) -> Self {
        Self {
            runtime,
            reads,
            controls,
            previews,
            mutations,
            metadata,
            errors,
        }
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn reads(&self) -> &Reads {
        &self.reads
    }

    pub fn controls(&self) -> &Controls {
        &self.controls
    }

    pub fn previews(&self) -> &Previews {
        &self.previews
    }

    pub fn mutations(&self) -> &Mutations {
        &self.mutations
    }
}

impl<Runtime, Reads, Controls, Previews, Mutations, Metadata, Errors>
    RequestDispatcherV1Adapter<Runtime, Reads, Controls, Previews, Mutations, Metadata, Errors>
where
    Runtime: WorkspaceRuntimeV1,
    Reads: DispatcherReadServiceV1<Runtime::Workspace>,
    Controls: DispatcherControlServiceV1<Runtime::Workspace>,
    Previews: DispatcherPreviewServiceV1<Runtime::Workspace>,
    Mutations: MutationAdmissionWorkerV1<Runtime::Workspace>,
    Metadata: DispatchResponseMetadataV1,
    Errors: DispatchErrorMapperV1,
{
    fn dispatch_valid(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        if request.command().as_str() != slice_request.command().command_name() {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::RequestInvalid,
            ));
        }

        match slice_request.command() {
            SliceCommandV1::WorkspaceDoctor(input) => {
                self.dispatch_doctor(request, slice_request, input.deep)
            }
            SliceCommandV1::WorkspaceShow(_) => self.dispatch_show(request, slice_request),
            SliceCommandV1::WorkspaceRepair(_) => self.dispatch_repair(request, slice_request),
            SliceCommandV1::SessionStatus(input) => self.dispatch_status(
                request,
                slice_request,
                &input.wait,
                input.verbose,
                input.compact,
                input.preconditions.expected_session_id.clone(),
            ),
            SliceCommandV1::SessionNext(input) => self.dispatch_next(
                request,
                slice_request,
                &input.wait,
                input.preconditions.expected_session_id.clone(),
            ),
            SliceCommandV1::SessionStart(input) if input.dry_run => {
                self.dispatch_preview(request, slice_request)
            }
            SliceCommandV1::SessionStartReplace(input) if input.start.dry_run => {
                self.dispatch_preview(request, slice_request)
            }
            SliceCommandV1::SessionReturn(input) if input.dry_run => {
                self.dispatch_preview(request, slice_request)
            }
            SliceCommandV1::SessionReopen(input) if input.dry_run => {
                self.dispatch_preview(request, slice_request)
            }
            SliceCommandV1::SessionReset(input) if input.dry_run => {
                self.dispatch_preview(request, slice_request)
            }
            SliceCommandV1::JobList(input) => {
                self.dispatch_job_list(request, slice_request, input.state)
            }
            SliceCommandV1::JobLookup(input) => {
                self.dispatch_job_lookup(request, slice_request, &input.idempotency_key)
            }
            SliceCommandV1::JobStatus(input) => self.dispatch_job_status(
                request,
                slice_request,
                &input.job_id,
                RequestReadWaitV1::Immediate,
            ),
            SliceCommandV1::JobWait(input) => self.dispatch_job_status(
                request,
                slice_request,
                &input.job_id,
                RequestReadWaitV1::AfterJobUntil {
                    job_id: input.job_id.clone(),
                    timeout_millis: request.options().wait_timeout_ms(),
                },
            ),
            SliceCommandV1::JobCancel(input) => self.dispatch_job_cancel(
                request,
                slice_request,
                &input.job_id,
                input.preconditions.expected_job_state,
            ),
            SliceCommandV1::WorkspaceInit(_) => {
                self.dispatch_mutation(request, slice_request, true)
            }
            SliceCommandV1::WorkspaceResetAll(_) => self.dispatch_reset_all(request, slice_request),
            _ => self.dispatch_mutation(request, slice_request, false),
        }
    }

    fn dispatch_doctor(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        deep: bool,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        self.workspace_output_response(
            request,
            self.runtime.doctor(slice_request.selector(), deep)?,
        )
    }

    fn dispatch_show(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        self.workspace_output_response(request, self.runtime.show(slice_request.selector())?)
    }

    fn dispatch_repair(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        self.workspace_output_response(request, self.runtime.repair(slice_request.selector())?)
    }

    fn dispatch_status(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        query_wait: &QueryWaitV1,
        verbose: bool,
        compact: bool,
        expected_session_id: Option<SessionId>,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let workspace = self.runtime.resolve_existing(slice_request.selector())?;
        let output = self.reads.status(
            &workspace,
            DispatcherStatusRequestV1 {
                wait: RequestReadWaitV1::from_query_wait(
                    query_wait,
                    request.options().wait_timeout_ms(),
                ),
                verbose,
                expected_session_id,
            },
        )?;
        let status = StatusResultV1::from_result_map(&output.result)
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
        let mut workspace_output = self.runtime.workspace_output(&workspace);
        let result = if compact {
            workspace_output =
                workspace_at_sequence(workspace_output, status.queue.latest_workspace_sequence)?;
            CompactStatusResultV1::from_status(&status).to_result_map()
        } else {
            status.to_result_map()
        };
        self.output_response(
            request,
            Some(workspace_output),
            None,
            None,
            result,
            output.warnings,
        )
    }

    fn dispatch_next(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        query_wait: &QueryWaitV1,
        expected_session_id: Option<SessionId>,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let workspace = self.runtime.resolve_existing(slice_request.selector())?;
        let output = self.reads.next(
            &workspace,
            DispatcherNextRequestV1 {
                wait: RequestReadWaitV1::from_query_wait(
                    query_wait,
                    request.options().wait_timeout_ms(),
                ),
                expected_session_id,
            },
        )?;
        let result = NextResultV1::from_result_map(&output.result)
            .map(|result| result.to_result_map())
            .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))?;
        self.output_response(
            request,
            Some(self.runtime.workspace_output(&workspace)),
            None,
            None,
            result,
            output.warnings,
        )
    }
    fn dispatch_preview(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let workspace = self
            .runtime
            .resolve_existing_readonly(slice_request.selector())?;
        let output = self.previews.preview(&workspace, slice_request)?;
        self.output_response(
            request,
            Some(self.runtime.workspace_output(&workspace)),
            None,
            None,
            output.result,
            output.warnings,
        )
    }

    fn dispatch_job_list(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        state: Option<JobStateV1>,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let workspace = self.runtime.resolve_existing(slice_request.selector())?;
        let output = self.reads.job_list(&workspace, state)?;
        self.output_response(
            request,
            Some(self.runtime.workspace_output(&workspace)),
            None,
            None,
            output.result,
            output.warnings,
        )
    }

    fn dispatch_job_lookup(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let output = self
            .reads
            .job_lookup(slice_request.selector(), idempotency_key)?;
        self.output_response(
            request,
            output.workspace,
            None,
            None,
            output.result,
            output.warnings,
        )
    }

    fn dispatch_job_status(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        job_id: &JobId,
        wait: RequestReadWaitV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let workspace = self.runtime.resolve_existing(slice_request.selector())?;
        let output = self.reads.job_status(&workspace, job_id, wait)?;
        self.output_response(
            request,
            Some(self.runtime.workspace_output(&workspace)),
            Some(output.job),
            None,
            output.result,
            output.warnings,
        )
    }

    fn dispatch_job_cancel(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        job_id: &JobId,
        expected_state: JobStateV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_query_options(request)?;
        let workspace = self.runtime.resolve_existing(slice_request.selector())?;
        let output = self
            .controls
            .cancel_job(&workspace, job_id, expected_state)?;
        self.output_response(
            request,
            Some(self.runtime.workspace_output(&workspace)),
            Some(output.job),
            None,
            output.result,
            output.warnings,
        )
    }

    fn workspace_output_response(
        &self,
        request: &RequestEnvelopeV1,
        output: DispatcherWorkspaceOutputV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.output_response(
            request,
            Some(output.workspace),
            None,
            None,
            output.result,
            output.warnings,
        )
    }

    fn require_query_options(&self, request: &RequestEnvelopeV1) -> Result<(), DispatchFailureV1> {
        if request.options().detach() {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::RequestInvalid,
            ));
        }
        Ok(())
    }
    fn require_reset_all_options(
        &self,
        request: &RequestEnvelopeV1,
    ) -> Result<(), DispatchFailureV1> {
        if request.options().detach()
            || request.options().wait_timeout_ms() != RESET_ALL_WAIT_TIMEOUT_MILLIS_V1
        {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::RequestInvalid,
            ));
        }
        Ok(())
    }
    fn dispatch_reset_all(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        self.require_reset_all_options(request)?;
        let idempotency_key = request
            .idempotency_key()
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let (workspace, outcome) = self.mutations.reset_all(
            slice_request.selector(),
            slice_request,
            idempotency_key,
            request.request_id(),
        )?;
        match outcome {
            MutationDispatchOutcomeV1::Terminal {
                job,
                result,
                response_context,
            } => self.terminal_response(request, workspace, job, result, response_context, false),
            MutationDispatchOutcomeV1::Detached { .. }
            | MutationDispatchOutcomeV1::TimedOut { .. } => {
                Err(DispatchFailureV1::new(DispatchFailureKindV1::Internal))
            }
        }
    }
    fn dispatch_mutation(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
        bootstrap: bool,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        let idempotency_key = request
            .idempotency_key()
            .ok_or_else(|| DispatchFailureV1::new(DispatchFailureKindV1::RequestInvalid))?;
        let workspace = if bootstrap {
            self.runtime.resolve_bootstrap(slice_request.selector())?
        } else {
            self.runtime.resolve_existing(slice_request.selector())?
        };
        let workspace_output = self.runtime.workspace_output(&workspace);
        let response_context = MutationResponseContextV1::new(request, workspace_output.clone());
        let wait = MutationWaitV1::from_request(request);
        let outcome = self.mutations.admit_and_wait(
            &workspace,
            slice_request,
            idempotency_key,
            &response_context,
            wait,
        )?;

        match (wait, outcome) {
            (
                MutationWaitV1::Detached,
                MutationDispatchOutcomeV1::Detached {
                    job,
                    procedure_digest,
                },
            ) => {
                let workspace_output =
                    workspace_at_least_sequence(workspace_output, job.sequence())?;
                self.detached_response(request, workspace_output, job, procedure_digest.as_ref())
            }
            (
                MutationWaitV1::Detached,
                MutationDispatchOutcomeV1::Terminal {
                    job,
                    result,
                    response_context,
                },
            )
            | (
                MutationWaitV1::UntilTerminal { .. },
                MutationDispatchOutcomeV1::Terminal {
                    job,
                    result,
                    response_context,
                },
            ) => self.terminal_response(
                request,
                workspace_output,
                job,
                result,
                response_context,
                bootstrap,
            ),
            (MutationWaitV1::UntilTerminal { .. }, MutationDispatchOutcomeV1::TimedOut { job }) => {
                Err(DispatchFailureV1::new(DispatchFailureKindV1::JobWaitTimeout).with_job(&job))
            }
            (MutationWaitV1::Detached, MutationDispatchOutcomeV1::TimedOut { .. })
            | (MutationWaitV1::UntilTerminal { .. }, MutationDispatchOutcomeV1::Detached { .. }) => {
                Err(DispatchFailureV1::new(DispatchFailureKindV1::Internal))
            }
        }
    }

    fn detached_response(
        &self,
        request: &RequestEnvelopeV1,
        workspace: WorkspaceOutputV1,
        job: JobOutputV1,
        procedure_digest: Option<&Sha256Digest>,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        let mut result = Map::from_iter([
            ("admission".to_owned(), job_admission_value_v1(&job)),
            ("detached".to_owned(), Value::Bool(true)),
        ]);
        if let Some(procedure_digest) = procedure_digest {
            result.insert("procedure_digest".to_owned(), json!(procedure_digest));
        }
        self.output_response(
            request,
            Some(workspace),
            Some(job),
            None,
            result,
            Vec::new(),
        )
    }

    fn terminal_response(
        &self,
        request: &RequestEnvelopeV1,
        workspace: WorkspaceOutputV1,
        job: JobOutputV1,
        result: DispatcherTerminalResultV1,
        response_context: Option<Box<TerminalResponseContextV1>>,
        bootstrap: bool,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        let context = response_context
            .filter(|context| context.request_id() == request.request_id())
            .map(|context| *context)
            .unwrap_or_else(|| {
                TerminalResponseContextV1::new(
                    request.request_id().clone(),
                    request.command().clone(),
                    self.metadata.generated_at(),
                    workspace,
                )
            });
        terminal_response_envelope_with_mapper_v1(context, job, result, bootstrap, &self.errors)
    }

    fn output_response(
        &self,
        request: &RequestEnvelopeV1,
        workspace: Option<WorkspaceOutputV1>,
        job: Option<JobOutputV1>,
        session: Option<SessionOutputV1>,
        result: Map<String, Value>,
        warnings: Vec<Map<String, Value>>,
    ) -> Result<ResponseEnvelopeV1, DispatchFailureV1> {
        OutputEnvelopeV1::new(OutputEnvelopeInputV1 {
            request_id: request.request_id().clone(),
            command: request.command().clone(),
            generated_at: self.metadata.generated_at(),
            workspace,
            job,
            session,
            result,
            warnings,
        })
        .map(ResponseEnvelopeV1::Output)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
    }

    fn error_response(
        &self,
        request: &RequestEnvelopeV1,
        failure: DispatchFailureV1,
        requires_admission: bool,
    ) -> ResponseEnvelopeV1 {
        let presentation = self.errors.map_failure(&failure);
        let generated_at = self.metadata.generated_at();
        let envelope = ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
            request_id: request.request_id().clone(),
            command: request.command().clone(),
            generated_at: generated_at.clone(),
            code: presentation.code,
            message: presentation.message.to_owned(),
            retryable: presentation.retryable,
            exit_code: presentation.exit_code,
            workspace: None,
            details: failure.into_details().into_json(requires_admission),
        })
        .unwrap_or_else(|_| {
            let internal = DispatchErrorPresentationV1::catalog(DispatchFailureKindV1::Internal);
            ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at,
                code: internal.code,
                message: internal.message.to_owned(),
                retryable: internal.retryable,
                exit_code: internal.exit_code,
                workspace: None,
                details: if requires_admission {
                    Map::from_iter([("admission".to_owned(), json!({"admitted": false}))])
                } else {
                    Map::new()
                },
            })
            .expect("static internal dispatcher error must be protocol-valid")
        });
        ResponseEnvelopeV1::Error(envelope)
    }
}

fn workspace_at_least_sequence(
    workspace: WorkspaceOutputV1,
    sequence: u64,
) -> Result<WorkspaceOutputV1, DispatchFailureV1> {
    let latest = workspace.latest_workspace_sequence().max(sequence);
    WorkspaceOutputV1::new(workspace.uuid().clone(), workspace.root(), latest)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

fn workspace_at_sequence(
    workspace: WorkspaceOutputV1,
    sequence: u64,
) -> Result<WorkspaceOutputV1, DispatchFailureV1> {
    WorkspaceOutputV1::new(workspace.uuid().clone(), workspace.root(), sequence)
        .map_err(|_| DispatchFailureV1::new(DispatchFailureKindV1::Internal))
}

impl<Runtime, Reads, Controls, Previews, Mutations, Metadata, Errors> RequestDispatcherV1
    for RequestDispatcherV1Adapter<Runtime, Reads, Controls, Previews, Mutations, Metadata, Errors>
where
    Runtime: WorkspaceRuntimeV1,
    Reads: DispatcherReadServiceV1<Runtime::Workspace>,
    Controls: DispatcherControlServiceV1<Runtime::Workspace>,
    Previews: DispatcherPreviewServiceV1<Runtime::Workspace>,
    Mutations: MutationAdmissionWorkerV1<Runtime::Workspace>,
    Metadata: DispatchResponseMetadataV1,
    Errors: DispatchErrorMapperV1,
{
    fn dispatch(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> ResponseEnvelopeV1 {
        let requires_admission = matches!(
            slice_request.command().operation(),
            OperationV1::Mutate | OperationV1::Bootstrap
        );
        self.dispatch_valid(request, slice_request)
            .unwrap_or_else(|failure| self.error_response(request, failure, requires_admission))
    }
}

fn catalog_error_spec_v1(kind: DispatchFailureKindV1) -> (&'static str, &'static str, bool, u8) {
    match kind {
        DispatchFailureKindV1::RequestInvalid => (
            "REQUEST_INVALID",
            "Request is malformed or violates schema.",
            false,
            2,
        ),
        DispatchFailureKindV1::ProtocolVersionUnsupported => (
            "PROTOCOL_VERSION_UNSUPPORTED",
            "Requested IPC protocol is unsupported.",
            false,
            3,
        ),
        DispatchFailureKindV1::NotGitWorktree => (
            "NOT_A_GIT_WORKTREE",
            "No valid Git worktree contains the path.",
            false,
            5,
        ),
        DispatchFailureKindV1::BareGitRepository => (
            "BARE_GIT_REPOSITORY",
            "Bare repositories are unsupported.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorktreeGone => {
            ("WORKTREE_GONE", "The worktree no longer exists.", false, 5)
        }
        DispatchFailureKindV1::WorkspaceNotInitialized => (
            "WORKSPACE_NOT_INITIALIZED",
            "Podway workspace state is absent.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorkspaceAlreadyInitialized => (
            "WORKSPACE_ALREADY_INITIALIZED",
            "Workspace is already initialized.",
            false,
            1,
        ),
        DispatchFailureKindV1::WorkspaceInitConflict => (
            "WORKSPACE_INIT_CONFLICT",
            "Existing Podway files conflict with safe initialization.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorkspaceIdentityConflict => (
            "WORKSPACE_ID_CONFLICT",
            "The workspace UUID is active at another root.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorkspaceUuidMismatch => (
            "WORKSPACE_UUID_MISMATCH",
            "The workspace UUID differs from the expected identity.",
            false,
            4,
        ),
        DispatchFailureKindV1::WorkspaceConfigInvalid => (
            "WORKSPACE_CONFIG_INVALID",
            "Workspace configuration is invalid.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorkspaceStateUnreadable => (
            "WORKSPACE_STATE_UNREADABLE",
            "Workspace state is corrupt or inaccessible.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorkspaceSchemaUnsupported => (
            "WORKSPACE_SCHEMA_UNSUPPORTED",
            "Workspace schema is unsupported.",
            false,
            5,
        ),
        DispatchFailureKindV1::WorkspaceQueueFull => (
            "WORKSPACE_QUEUE_FULL",
            "Workspace mutation queue is full.",
            true,
            4,
        ),
        DispatchFailureKindV1::WorkspaceMaintenance => (
            "WORKSPACE_MAINTENANCE",
            "Workspace maintenance temporarily blocks mutation admission.",
            true,
            4,
        ),
        DispatchFailureKindV1::WorkspacePathUnsafe => (
            "WORKSPACE_PATH_UNSAFE",
            "Podway workspace path is unsafe.",
            false,
            5,
        ),
        DispatchFailureKindV1::PathOutsideWorktree => (
            "PATH_OUTSIDE_WORKTREE",
            "A requested path escapes the worktree.",
            false,
            5,
        ),
        DispatchFailureKindV1::MigrationFailed => {
            ("MIGRATION_FAILED", "Workspace migration failed.", false, 5)
        }
        DispatchFailureKindV1::ProcedureNotFound => (
            "PROCEDURE_NOT_FOUND",
            "Procedure cannot be resolved.",
            false,
            1,
        ),
        DispatchFailureKindV1::ProcedureInvalid => {
            ("PROCEDURE_INVALID", "Procedure is invalid.", false, 1)
        }
        DispatchFailureKindV1::ProcedureSchemaUnsupported => (
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "Procedure schema is unsupported.",
            false,
            1,
        ),
        DispatchFailureKindV1::ProcedureDigestMismatch => (
            "PROCEDURE_DIGEST_MISMATCH",
            "The canonical Procedure digest differs from the expected digest.",
            false,
            4,
        ),
        DispatchFailureKindV1::PresetNotFound => (
            "PRESET_NOT_FOUND",
            "Built-in preset does not exist.",
            false,
            1,
        ),
        DispatchFailureKindV1::SessionNotFound => (
            "SESSION_NOT_FOUND",
            "No current task session exists.",
            false,
            1,
        ),
        DispatchFailureKindV1::SessionIdMismatch => (
            "SESSION_ID_MISMATCH",
            "The session ID differs from the expected identity.",
            false,
            4,
        ),
        DispatchFailureKindV1::SessionAlreadyExists => (
            "SESSION_ALREADY_EXISTS",
            "A task session already exists.",
            false,
            1,
        ),
        DispatchFailureKindV1::SessionNotRunning => (
            "SESSION_NOT_RUNNING",
            "The command requires a running session.",
            false,
            1,
        ),
        DispatchFailureKindV1::SessionNotCompleted => (
            "SESSION_NOT_COMPLETED",
            "The command requires a completed session.",
            false,
            1,
        ),
        DispatchFailureKindV1::SessionCancelled => {
            ("SESSION_CANCELLED", "The session was cancelled.", false, 1)
        }
        DispatchFailureKindV1::SessionRevisionConflict => (
            "SESSION_REVISION_CONFLICT",
            "The session changed after it was observed.",
            true,
            4,
        ),
        DispatchFailureKindV1::AttemptNotCurrent => (
            "ATTEMPT_NOT_CURRENT",
            "The target attempt is no longer active.",
            true,
            4,
        ),
        DispatchFailureKindV1::StageNotFound => {
            ("STAGE_NOT_FOUND", "The stage does not exist.", false, 1)
        }
        DispatchFailureKindV1::StageNotSkippable => (
            "STAGE_NOT_SKIPPABLE",
            "The active stage cannot be skipped.",
            false,
            1,
        ),
        DispatchFailureKindV1::ReopenNotAllowed => (
            "REOPEN_NOT_ALLOWED",
            "The session or destination cannot be reopened.",
            false,
            1,
        ),
        DispatchFailureKindV1::ReturnNotAllowed => (
            "RETURN_NOT_ALLOWED",
            "The return destination is not allowed.",
            false,
            1,
        ),
        DispatchFailureKindV1::RequiredItemsMissing => (
            "REQUIRED_ITEMS_MISSING",
            "Required items are missing.",
            false,
            1,
        ),
        DispatchFailureKindV1::BlockersPresent => (
            "BLOCKERS_PRESENT",
            "Open blockers prevent completion.",
            false,
            1,
        ),
        DispatchFailureKindV1::BlockerLimitReached => (
            "BLOCKER_LIMIT_REACHED",
            "The active attempt reached the open blocker limit.",
            false,
            1,
        ),
        DispatchFailureKindV1::ItemNotFound => (
            "ITEM_NOT_FOUND",
            "The item does not exist on the active stage.",
            false,
            1,
        ),
        DispatchFailureKindV1::ItemTypeMismatch => (
            "ITEM_TYPE_MISMATCH",
            "The command is incompatible with the item type.",
            false,
            1,
        ),
        DispatchFailureKindV1::ItemConstraintFailed => (
            "ITEM_CONSTRAINT_FAILED",
            "The item value violates its constraints.",
            false,
            1,
        ),
        DispatchFailureKindV1::ItemRevisionConflict => (
            "ITEM_REVISION_CONFLICT",
            "The item changed after it was observed.",
            true,
            4,
        ),
        DispatchFailureKindV1::ItemAlreadySet => (
            "ITEM_ALREADY_SET",
            "The item was expected to be unset.",
            true,
            4,
        ),
        DispatchFailureKindV1::ListValueNotFound => (
            "LIST_VALUE_NOT_FOUND",
            "The list value does not exist.",
            false,
            1,
        ),
        DispatchFailureKindV1::ListValueDuplicate => (
            "LIST_VALUE_DUPLICATE",
            "The unique list already contains the value.",
            false,
            1,
        ),
        DispatchFailureKindV1::ArtifactNotFound => (
            "ARTIFACT_NOT_FOUND",
            "The local artifact does not exist.",
            false,
            1,
        ),
        DispatchFailureKindV1::ArtifactUnreadable => (
            "ARTIFACT_UNREADABLE",
            "The local artifact cannot be opened or hashed.",
            false,
            5,
        ),
        DispatchFailureKindV1::ArtifactChanged => (
            "ARTIFACT_CHANGED",
            "The local artifact no longer matches stored metadata.",
            true,
            1,
        ),
        DispatchFailureKindV1::ArtifactMediaTypeNotAllowed => (
            "ARTIFACT_MEDIA_TYPE_NOT_ALLOWED",
            "The artifact media type is not allowed.",
            false,
            1,
        ),
        DispatchFailureKindV1::BlockerNotFound => {
            ("BLOCKER_NOT_FOUND", "The blocker does not exist.", false, 1)
        }
        DispatchFailureKindV1::BlockerNotCurrent => (
            "BLOCKER_NOT_CURRENT",
            "The blocker belongs to an old attempt.",
            true,
            4,
        ),
        DispatchFailureKindV1::IdempotencyKeyReused => (
            "IDEMPOTENCY_KEY_REUSED",
            "The idempotency key is bound to another request.",
            false,
            2,
        ),
        DispatchFailureKindV1::MutationOutcomeUnknown => (
            "MUTATION_OUTCOME_UNKNOWN",
            "Mutation outcome is unknown; reconcile by idempotency key.",
            true,
            4,
        ),
        DispatchFailureKindV1::JobNotFound => (
            "JOB_NOT_FOUND",
            "The job does not exist or was pruned.",
            false,
            1,
        ),
        DispatchFailureKindV1::JobNotCancellable => (
            "JOB_NOT_CANCELLABLE",
            "The job is running or terminal.",
            false,
            1,
        ),
        DispatchFailureKindV1::JobWaitTimeout => (
            "JOB_WAIT_TIMEOUT",
            "The wait expired; the admitted job may continue.",
            true,
            4,
        ),
        DispatchFailureKindV1::DaemonUnavailable => (
            "DAEMON_UNAVAILABLE",
            "Daemon socket cannot be reached.",
            true,
            3,
        ),
        DispatchFailureKindV1::DaemonShuttingDown => (
            "DAEMON_SHUTTING_DOWN",
            "Daemon is draining and not accepting work.",
            true,
            3,
        ),
        DispatchFailureKindV1::Internal => (
            "INTERNAL_ERROR",
            "An unexpected internal error occurred.",
            false,
            6,
        ),
    }
}
