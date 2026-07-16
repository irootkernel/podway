//! Deliberately bounded command-line surface for the G005 vertical slice.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    time::Duration,
};

use clap::{ArgAction, Parser, Subcommand};
use nix::unistd::geteuid;
use podway_cli::client::{
    DEFAULT_DAEMON_CONNECT_TIMEOUT_V1, DEFAULT_DAEMON_WRITE_TIMEOUT_V1, DaemonClientErrorV1,
    DaemonClientTimeoutsV1, DaemonClientV1,
};
use podway_core::{AttemptId, Revision, WorkspaceId};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, MAX_WAIT_TIMEOUT_MILLIS_V1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV1, WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_service::ServiceRuntimePathsV1;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
const LOCAL_USAGE_EXIT: i32 = 2;
const LOCAL_CLIENT_EXIT: i32 = 6;

#[derive(Debug, Parser)]
#[command(
    name = "podway",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_required = true,
    arg_required_else_help = false
)]
struct Cli {
    /// Emit exactly one versioned JSON envelope.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    json: bool,

    /// Use this local workspace path instead of the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Bound daemon query/job waiting and the local transport exchange.
    #[arg(
        long,
        global = true,
        value_name = "DURATION",
        value_parser = parse_timeout_millis
    )]
    timeout: Option<u64>,

    /// Return after durable mutation admission.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    detach: bool,

    /// Reuse this idempotency key instead of generating a UUID-v4 key.
    #[arg(long, global = true, value_name = "KEY")]
    idempotency_key: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Start {
        #[arg(long, value_name = "PRESET")]
        preset: String,
        #[arg(long, value_name = "TITLE")]
        task: String,
    },
    Status,
    Next,
    Check {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
    },
    Set {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    Add {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    Attach {
        #[arg(value_name = "ITEM_ID")]
        item_id: String,
        #[arg(value_name = "PATH")]
        path: String,
        #[arg(long, value_name = "TYPE")]
        media_type: Option<String>,
    },
    Block {
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    Unblock {
        #[arg(
            value_name = "BLOCKER_ID",
            required_unless_present = "all",
            conflicts_with = "all"
        )]
        blocker_id: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        all: bool,
    },
    Retry {
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    Return {
        #[arg(long, value_name = "STAGE_ID")]
        to: String,
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    Complete,
}

impl Command {
    const fn wire_name(&self) -> &'static str {
        match self {
            Self::Init => "workspace.init",
            Self::Start { .. } => "preset.start",
            Self::Status => "session.status",
            Self::Next => "session.next",
            Self::Check { .. } => "item.check",
            Self::Set { .. } => "item.set",
            Self::Add { .. } => "item.add",
            Self::Attach { .. } => "item.attach_path",
            Self::Block { .. } => "session.block",
            Self::Unblock { .. } => "session.unblock",
            Self::Retry { .. } => "session.retry",
            Self::Return { .. } => "session.return",
            Self::Complete => "session.complete",
        }
    }

    const fn is_mutation(&self) -> bool {
        !matches!(self, Self::Status | Self::Next)
    }

    const fn needs_status_preflight(&self) -> bool {
        self.is_mutation() && !matches!(self, Self::Init | Self::Start { .. })
    }

    fn post_preflight_mutation(&self) -> Option<PostPreflightMutationV1<'_>> {
        match self {
            Self::Check { item_id } => Some(PostPreflightMutationV1::Check { item_id }),
            Self::Set { item_id, value } => Some(PostPreflightMutationV1::Set { item_id, value }),
            Self::Add { item_id, value } => Some(PostPreflightMutationV1::Add { item_id, value }),
            Self::Attach {
                item_id,
                path,
                media_type,
            } => Some(PostPreflightMutationV1::Attach {
                item_id,
                path,
                media_type: media_type.as_deref(),
            }),
            Self::Block { reason } => Some(PostPreflightMutationV1::Block { reason }),
            Self::Unblock { blocker_id, all } => Some(PostPreflightMutationV1::Unblock {
                blocker_id: blocker_id.as_deref(),
                all: *all,
            }),
            Self::Retry { reason } => Some(PostPreflightMutationV1::Retry { reason }),
            Self::Return { to, reason } => Some(PostPreflightMutationV1::Return { to, reason }),
            Self::Complete => Some(PostPreflightMutationV1::Complete),
            Self::Init | Self::Start { .. } | Self::Status | Self::Next => None,
        }
    }
}

enum PostPreflightMutationV1<'a> {
    Check {
        item_id: &'a str,
    },
    Set {
        item_id: &'a str,
        value: &'a str,
    },
    Add {
        item_id: &'a str,
        value: &'a str,
    },
    Attach {
        item_id: &'a str,
        path: &'a str,
        media_type: Option<&'a str>,
    },
    Block {
        reason: &'a str,
    },
    Unblock {
        blocker_id: Option<&'a str>,
        all: bool,
    },
    Retry {
        reason: &'a str,
    },
    Return {
        to: &'a str,
        reason: &'a str,
    },
    Complete,
}

impl PostPreflightMutationV1<'_> {
    const fn wire_name(&self) -> &'static str {
        match self {
            Self::Check { .. } => "item.check",
            Self::Set { .. } => "item.set",
            Self::Add { .. } => "item.add",
            Self::Attach { .. } => "item.attach_path",
            Self::Block { .. } => "session.block",
            Self::Unblock { .. } => "session.unblock",
            Self::Retry { .. } => "session.retry",
            Self::Return { .. } => "session.return",
            Self::Complete => "session.complete",
        }
    }

    const fn item_id(&self) -> Option<&str> {
        match self {
            Self::Check { item_id }
            | Self::Set { item_id, .. }
            | Self::Add { item_id, .. }
            | Self::Attach { item_id, .. } => Some(item_id),
            Self::Block { .. }
            | Self::Unblock { .. }
            | Self::Retry { .. }
            | Self::Return { .. }
            | Self::Complete => None,
        }
    }
}

#[derive(Clone, Debug)]
struct WorkspaceTarget {
    path_bytes: Vec<u8>,
    display: String,
}

impl WorkspaceTarget {
    fn selector(
        &self,
        expected_uuid: Option<WorkspaceId>,
    ) -> Result<WorktreeSelectorWireV1, LocalFailure> {
        WorktreeSelectorWireV1::new(&self.path_bytes, self.display.clone(), expected_uuid)
            .map_err(|_| LocalFailure::request_invalid("workspace selector is invalid"))
    }

    fn context(
        &self,
        expected_uuid: Option<WorkspaceId>,
    ) -> Result<WorkspaceContextV1, LocalFailure> {
        WorkspaceContextV1::new(self.display.clone(), expected_uuid)
            .map_err(|_| LocalFailure::request_invalid("workspace path is invalid"))
    }
}

#[derive(Clone, Debug)]
struct StatusFacts {
    workspace_id: WorkspaceId,
    session_revision: Option<Revision>,
    attempt_id: Option<AttemptId>,
    item_revisions: Vec<(String, Revision)>,
}

impl StatusFacts {
    fn from_status(response: &podway_protocol::OutputEnvelopeV1) -> Result<Self, LocalFailure> {
        let workspace_id = response
            .workspace()
            .map(|workspace| workspace.uuid().clone())
            .ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted workspace identity")
            })?;
        let result = response.result();
        let session_revision = result
            .get("session")
            .and_then(Value::as_object)
            .and_then(|session| session.get("revision"))
            .and_then(Value::as_u64)
            .map(Revision::new);
        let attempt_id = result
            .get("current")
            .and_then(Value::as_object)
            .and_then(|current| current.get("attempt_id"))
            .and_then(Value::as_str)
            .map(|value| {
                AttemptId::new(value.to_owned()).map_err(|_| {
                    LocalFailure::response_invalid("status response has an invalid attempt")
                })
            })
            .transpose()?;
        let mut item_revisions = Vec::new();
        if let Some(items) = result.get("items").and_then(Value::as_array) {
            for item in items {
                let Some(item) = item.as_object() else {
                    return Err(LocalFailure::response_invalid(
                        "status response has an invalid item",
                    ));
                };
                let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                    return Err(LocalFailure::response_invalid(
                        "status response omitted an item id",
                    ));
                };
                let Some(revision) = item.get("revision").and_then(Value::as_u64) else {
                    return Err(LocalFailure::response_invalid(
                        "status response omitted an item revision",
                    ));
                };
                item_revisions.push((item_id.to_owned(), Revision::new(revision)));
            }
        }
        Ok(Self {
            workspace_id,
            session_revision,
            attempt_id,
            item_revisions,
        })
    }

    fn session_preconditions(&self) -> Result<PreconditionsV1, LocalFailure> {
        let session_revision = self.session_revision.ok_or_else(|| {
            LocalFailure::response_invalid("status response omitted the active session revision")
        })?;
        let attempt_id = self.attempt_id.clone().ok_or_else(|| {
            LocalFailure::response_invalid("status response omitted the active attempt")
        })?;
        PreconditionsV1::new(
            None,
            Some(session_revision),
            Some(attempt_id),
            None,
            None,
            None,
        )
        .map_err(|_| LocalFailure::response_invalid("status preconditions are invalid"))
    }

    fn item_preconditions(&self, item_id: &str) -> Result<PreconditionsV1, LocalFailure> {
        let attempt_id = self.attempt_id.clone().ok_or_else(|| {
            LocalFailure::response_invalid("status response omitted the active attempt")
        })?;
        let item_revision = self
            .item_revisions
            .iter()
            .find_map(|(known_id, revision)| (known_id == item_id).then_some(*revision))
            .ok_or_else(|| {
                LocalFailure::response_invalid("status response omitted the requested item")
            })?;
        PreconditionsV1::new(
            None,
            None,
            Some(attempt_id),
            Some(item_revision),
            None,
            None,
        )
        .map_err(|_| LocalFailure::response_invalid("status preconditions are invalid"))
    }
}

#[derive(Clone, Copy, Debug)]
struct LocalFailure {
    code: &'static str,
    message: &'static str,
    exit_code: i32,
}

impl LocalFailure {
    const fn request_invalid(message: &'static str) -> Self {
        Self {
            code: "REQUEST_INVALID",
            message,
            exit_code: LOCAL_USAGE_EXIT,
        }
    }

    const fn client(message: &'static str) -> Self {
        Self {
            code: "DAEMON_UNAVAILABLE",
            message,
            exit_code: LOCAL_CLIENT_EXIT,
        }
    }

    const fn response_invalid(message: &'static str) -> Self {
        Self {
            code: "DAEMON_RESPONSE_INVALID",
            message,
            exit_code: LOCAL_CLIENT_EXIT,
        }
    }
}

/// Runs the bounded CLI and returns its process exit code.
pub fn run() -> i32 {
    let arguments: Vec<OsString> = env::args_os().collect();
    let json_requested = arguments
        .iter()
        .any(|argument| argument.as_os_str() == OsStr::new("--json"));
    match Cli::try_parse_from(&arguments) {
        Ok(cli) => {
            let json_output = cli.json;
            match execute(cli) {
                Ok(response) => render_response(&response, json_output),
                Err(failure) => render_local_failure(failure, json_output),
            }
        }
        Err(_) => render_local_failure(
            LocalFailure::request_invalid("invalid command syntax"),
            json_requested,
        ),
    }
}

struct RequestSpec {
    operation: OperationV1,
    expected_uuid: Option<WorkspaceId>,
    idempotency_key: Option<IdempotencyKeyV1>,
    preconditions: PreconditionsV1,
    detach: bool,
    wait_timeout_ms: u64,
    payload: Map<String, Value>,
}

fn execute(cli: Cli) -> Result<ResponseEnvelopeV1, LocalFailure> {
    if !cli.command.is_mutation() && (cli.detach || cli.idempotency_key.is_some()) {
        return Err(LocalFailure::request_invalid(
            "detach and idempotency-key apply only to mutations",
        ));
    }

    let target = workspace_target(cli.workspace)?;
    let wait_timeout_ms = cli.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    let client = daemon_client(wait_timeout_ms)?;

    if matches!(&cli.command, Command::Init) {
        let request = build_request(
            "workspace.init",
            &target,
            RequestSpec {
                operation: OperationV1::Bootstrap,
                expected_uuid: None,
                idempotency_key: Some(mutation_key(cli.idempotency_key)?),
                preconditions: PreconditionsV1::default(),
                detach: cli.detach,
                wait_timeout_ms,
                payload: Map::new(),
            },
        )?;
        return request_daemon(&client, &request);
    }
    if let Command::Start { preset, task } = &cli.command {
        let request = build_request(
            "preset.start",
            &target,
            RequestSpec {
                operation: OperationV1::Mutate,
                expected_uuid: None,
                idempotency_key: Some(mutation_key(cli.idempotency_key)?),
                preconditions: PreconditionsV1::default(),
                detach: cli.detach,
                wait_timeout_ms,
                payload: Map::from_iter([
                    ("preset".to_owned(), Value::String(preset.clone())),
                    ("task_title".to_owned(), Value::String(task.clone())),
                ]),
            },
        )?;
        return request_daemon(&client, &request);
    }

    if !cli.command.needs_status_preflight() {
        let request = build_request(
            cli.command.wire_name(),
            &target,
            RequestSpec {
                operation: OperationV1::Query,
                expected_uuid: None,
                idempotency_key: None,
                preconditions: PreconditionsV1::default(),
                detach: false,
                wait_timeout_ms,
                payload: Map::new(),
            },
        )?;
        return request_daemon(&client, &request);
    }

    let mutation = cli
        .command
        .post_preflight_mutation()
        .ok_or_else(|| LocalFailure::request_invalid("unsupported mutation command"))?;
    let status_request = build_request(
        "session.status",
        &target,
        RequestSpec {
            operation: OperationV1::Query,
            expected_uuid: None,
            idempotency_key: None,
            preconditions: PreconditionsV1::default(),
            detach: false,
            wait_timeout_ms,
            payload: Map::new(),
        },
    )?;
    let status_response = request_daemon(&client, &status_request)?;
    let status = match status_response {
        ResponseEnvelopeV1::Output(status) => status,
        ResponseEnvelopeV1::Error(error) => {
            return re_correlate_preflight_error(&error, mutation.wire_name());
        }
    };
    let facts = StatusFacts::from_status(&status)?;
    let request = build_mutation_request(
        mutation,
        &target,
        &facts,
        mutation_key(cli.idempotency_key)?,
        cli.detach,
        wait_timeout_ms,
    )?;
    request_daemon(&client, &request)
}

fn workspace_target(workspace: Option<PathBuf>) -> Result<WorkspaceTarget, LocalFailure> {
    let path = workspace.unwrap_or_else(|| PathBuf::from("."));
    let absolute = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|_| LocalFailure::client("cannot determine the current directory"))?
            .join(path)
    };
    let canonical = match fs::canonicalize(&absolute) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == io::ErrorKind::NotFound => absolute,
        Err(_) => {
            return Err(LocalFailure::request_invalid(
                "workspace path cannot be resolved",
            ));
        }
    };
    let path_bytes = canonical.as_os_str().as_bytes().to_vec();
    if path_bytes.is_empty() {
        return Err(LocalFailure::request_invalid("workspace path is empty"));
    }
    Ok(WorkspaceTarget {
        path_bytes,
        display: canonical.display().to_string(),
    })
}

fn daemon_client(wait_timeout_ms: u64) -> Result<DaemonClientV1, LocalFailure> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| LocalFailure::client("cannot determine the local user home"))?;
    let temporary = env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let paths = ServiceRuntimePathsV1::for_user(home, temporary, geteuid().as_raw())
        .map_err(|_| LocalFailure::client("cannot determine the daemon runtime path"))?;

    let read_timeout = Duration::from_millis(wait_timeout_ms.saturating_add(1_000))
        .max(DEFAULT_DAEMON_CONNECT_TIMEOUT_V1);
    let timeouts = DaemonClientTimeoutsV1::new(
        DEFAULT_DAEMON_CONNECT_TIMEOUT_V1,
        read_timeout,
        DEFAULT_DAEMON_WRITE_TIMEOUT_V1,
    )
    .map_err(|_| LocalFailure::client("invalid local daemon timeout"))?;
    Ok(DaemonClientV1::with_timeouts(paths, timeouts))
}

fn build_mutation_request(
    command: PostPreflightMutationV1<'_>,
    target: &WorkspaceTarget,
    facts: &StatusFacts,
    idempotency_key: IdempotencyKeyV1,
    detach: bool,
    wait_timeout_ms: u64,
) -> Result<RequestEnvelopeV1, LocalFailure> {
    let wire_name = command.wire_name();
    let preconditions = match command.item_id() {
        Some(item_id) => facts.item_preconditions(item_id)?,
        None => facts.session_preconditions()?,
    };
    let mut payload = Map::new();

    match command {
        PostPreflightMutationV1::Check { item_id } => {
            payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
        }
        PostPreflightMutationV1::Set { item_id, value }
        | PostPreflightMutationV1::Add { item_id, value } => {
            payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
            payload.insert("value".to_owned(), Value::String(value.to_owned()));
        }
        PostPreflightMutationV1::Attach {
            item_id,
            path,
            media_type,
        } => {
            payload.insert("item_id".to_owned(), Value::String(item_id.to_owned()));
            payload.insert("path".to_owned(), Value::String(path.to_owned()));
            if let Some(media_type) = media_type {
                payload.insert(
                    "media_type".to_owned(),
                    Value::String(media_type.to_owned()),
                );
            }
        }
        PostPreflightMutationV1::Block { reason } | PostPreflightMutationV1::Retry { reason } => {
            payload.insert("reason".to_owned(), Value::String(reason.to_owned()));
        }
        PostPreflightMutationV1::Unblock { blocker_id, all } => {
            if let Some(blocker_id) = blocker_id {
                payload.insert(
                    "blocker_id".to_owned(),
                    Value::String(blocker_id.to_owned()),
                );
            }
            payload.insert("all".to_owned(), Value::Bool(all));
        }
        PostPreflightMutationV1::Return { to, reason } => {
            payload.insert(
                "destination_stage_id".to_owned(),
                Value::String(to.to_owned()),
            );
            payload.insert("reason".to_owned(), Value::String(reason.to_owned()));
        }
        PostPreflightMutationV1::Complete => {}
    }

    build_request(
        wire_name,
        target,
        RequestSpec {
            operation: OperationV1::Mutate,
            expected_uuid: Some(facts.workspace_id.clone()),
            idempotency_key: Some(idempotency_key),
            preconditions,
            detach,
            wait_timeout_ms,
            payload,
        },
    )
}

fn build_request(
    command: &str,
    target: &WorkspaceTarget,
    spec: RequestSpec,
) -> Result<RequestEnvelopeV1, LocalFailure> {
    let RequestSpec {
        operation,
        expected_uuid,
        idempotency_key,
        preconditions,
        detach,
        wait_timeout_ms,
        mut payload,
    } = spec;
    let selector = target.selector(expected_uuid.clone())?;
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector)
            .map_err(|_| LocalFailure::request_invalid("cannot encode workspace selector"))?,
    );
    let request_id = RequestIdV1::new(Uuid::new_v4().to_string())
        .map_err(|_| LocalFailure::client("cannot generate a request identifier"))?;
    let client = ClientInfoV1::new("podway", env!("CARGO_PKG_VERSION"), std::process::id())
        .map_err(|_| LocalFailure::client("cannot construct client metadata"))?;
    let command = CommandNameV1::new(command.to_owned())
        .map_err(|_| LocalFailure::request_invalid("invalid command"))?;
    let workspace = target.context(expected_uuid)?;
    let options = RequestOptionsV1::new(detach, wait_timeout_ms)
        .map_err(|_| LocalFailure::request_invalid("invalid timeout"))?;
    RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id,
        client,
        operation,
        command,
        workspace: Some(workspace),
        idempotency_key,
        preconditions,
        options,
        payload,
    })
    .map_err(|_| LocalFailure::request_invalid("invalid request"))
}

fn mutation_key(value: Option<String>) -> Result<IdempotencyKeyV1, LocalFailure> {
    IdempotencyKeyV1::new(value.unwrap_or_else(|| Uuid::new_v4().to_string()))
        .map_err(|_| LocalFailure::request_invalid("invalid idempotency key"))
}

fn request_daemon(
    client: &DaemonClientV1,
    request: &RequestEnvelopeV1,
) -> Result<ResponseEnvelopeV1, LocalFailure> {
    client.request(request).map_err(map_client_error)
}
fn re_correlate_preflight_error(
    error: &podway_protocol::ErrorEnvelopeV1,
    command: &str,
) -> Result<ResponseEnvelopeV1, LocalFailure> {
    let mut envelope = serde_json::to_value(error)
        .map_err(|_| LocalFailure::response_invalid("status preflight error cannot be read"))?;
    let fields = envelope
        .as_object_mut()
        .ok_or_else(|| LocalFailure::response_invalid("status preflight error is invalid"))?;
    fields.insert("command".to_owned(), Value::String(command.to_owned()));
    let error = serde_json::from_value(envelope)
        .map_err(|_| LocalFailure::response_invalid("status preflight error is invalid"))?;
    Ok(ResponseEnvelopeV1::Error(error))
}

fn map_client_error(error: DaemonClientErrorV1) -> LocalFailure {
    match error {
        DaemonClientErrorV1::RequestAdmission { .. }
        | DaemonClientErrorV1::RequestEncoding { .. } => {
            LocalFailure::request_invalid("the request cannot be admitted")
        }
        DaemonClientErrorV1::ResponseDecoding { .. }
        | DaemonClientErrorV1::Framing { .. }
        | DaemonClientErrorV1::MissingResponse
        | DaemonClientErrorV1::ResponseMismatch { .. } => {
            LocalFailure::response_invalid("the daemon returned an invalid response")
        }
        DaemonClientErrorV1::InvalidTimeout { .. }
        | DaemonClientErrorV1::Connection { .. }
        | DaemonClientErrorV1::SocketConfiguration { .. }
        | DaemonClientErrorV1::Timeout { .. } => {
            LocalFailure::client("the local daemon is unavailable")
        }
    }
}

fn parse_timeout_millis(value: &str) -> Result<u64, String> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000)
    } else {
        return Err("duration must use ms, s, or m".to_owned());
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("duration must be an unsigned integer followed by ms, s, or m".to_owned());
    }
    let millis = number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| "duration is out of range".to_owned())?;
    if millis > MAX_WAIT_TIMEOUT_MILLIS_V1 {
        return Err(format!(
            "duration exceeds the maximum of {MAX_WAIT_TIMEOUT_MILLIS_V1}ms"
        ));
    }
    Ok(millis)
}

fn render_response(response: &ResponseEnvelopeV1, json_output: bool) -> i32 {
    if json_output {
        let mut stdout = io::stdout().lock();
        if serde_json::to_writer(&mut stdout, response).is_err() || writeln!(stdout).is_err() {
            return LOCAL_CLIENT_EXIT;
        }
    } else {
        render_human_response(response);
    }
    match response {
        ResponseEnvelopeV1::Output(_) => 0,
        ResponseEnvelopeV1::Error(error) => i32::from(error.exit_code().get()),
    }
}

fn render_local_failure(failure: LocalFailure, json_output: bool) -> i32 {
    if json_output {
        let output = json!({
            "schema": "podway.error/v1",
            "request_id": Uuid::new_v4().to_string(),
            "command": "cli",
            "generated_at": "1970-01-01T00:00:00.000Z",
            "code": failure.code,
            "message": failure.message,
            "retryable": false,
            "exit_code": failure.exit_code,
            "details": {},
        });
        let mut stdout = io::stdout().lock();
        let _ = serde_json::to_writer(&mut stdout, &output);
        let _ = writeln!(stdout);
    } else {
        let _ = writeln!(io::stderr().lock(), "error: {}", failure.message);
    }
    failure.exit_code
}

fn render_human_response(response: &ResponseEnvelopeV1) {
    let mut stdout = io::stdout().lock();
    match response {
        ResponseEnvelopeV1::Output(output) => {
            if let Some(task) = output
                .result()
                .get("task")
                .and_then(Value::as_object)
                .and_then(|task| task.get("title"))
                .and_then(Value::as_str)
                .or_else(|| output.session().map(|session| session.title()))
            {
                let _ = writeln!(stdout, "task: {task}");
            }
            if let Some(stage) = output
                .result()
                .get("current")
                .and_then(Value::as_object)
                .and_then(|stage| stage.get("title"))
                .and_then(Value::as_str)
                .or_else(|| {
                    output
                        .result()
                        .get("stage")
                        .and_then(Value::as_object)
                        .and_then(|stage| stage.get("title"))
                        .and_then(Value::as_str)
                })
            {
                let _ = writeln!(stdout, "stage: {stage}");
            }
            if let Some(next) = output
                .result()
                .get("next_stage_after_completion")
                .and_then(Value::as_object)
                .and_then(|stage| stage.get("title").or_else(|| stage.get("id")))
                .and_then(Value::as_str)
            {
                let _ = writeln!(stdout, "next: {next}");
            }
            if let Some(job) = output.job() {
                let _ = writeln!(stdout, "job: {} ({:?})", job.id(), job.state());
            }
            if output.session().is_none()
                && output.job().is_none()
                && output.result().get("task").is_none()
                && output.result().get("stage").is_none()
                && output.result().get("current").is_none()
            {
                let _ = writeln!(stdout, "command: {}", output.command().as_str());
            }
        }
        ResponseEnvelopeV1::Error(error) => {
            let _ = writeln!(
                stdout,
                "error: {}: {}",
                error.code().as_str(),
                error.message()
            );
            if let Some(job_id) = error.details().get("job_id").and_then(Value::as_str) {
                let _ = writeln!(stdout, "job: {job_id}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, parse_timeout_millis};
    use clap::Parser;

    #[test]
    fn parser_accepts_each_bounded_command_shape() {
        let start = Cli::try_parse_from([
            "podway",
            "start",
            "--preset",
            "sw-dev",
            "--task",
            "bounded task",
        ])
        .expect("start command must parse");
        assert!(matches!(&start.command, Command::Start { .. }));
        assert!(
            !start.command.needs_status_preflight(),
            "preset start must be admitted before any session exists"
        );

        let attach = Cli::try_parse_from([
            "podway",
            "attach",
            "artifact",
            "report.txt",
            "--media-type",
            "text/plain",
        ])
        .expect("attach command must parse");
        assert!(matches!(attach.command, Command::Attach { .. }));

        let unblock =
            Cli::try_parse_from(["podway", "unblock", "--all"]).expect("unblock --all must parse");
        assert!(matches!(
            unblock.command,
            Command::Unblock { all: true, .. }
        ));
        for argv in [
            &["podway", "init"][..],
            &["podway", "status"][..],
            &["podway", "next"][..],
            &["podway", "check", "confirmed"][..],
            &["podway", "set", "note", "text"][..],
            &["podway", "add", "labels", "alpha"][..],
            &["podway", "block", "--reason", "waiting"][..],
            &["podway", "retry", "--reason", "try again"][..],
            &["podway", "return", "--to", "implement", "--reason", "redo"][..],
            &["podway", "complete"][..],
        ] {
            assert!(
                Cli::try_parse_from(argv).is_ok(),
                "bounded command must parse: {argv:?}"
            );
        }
    }

    #[test]
    fn parser_rejects_ambiguous_or_incomplete_forms() {
        assert!(Cli::try_parse_from(["podway", "start", "--preset", "sw-dev"]).is_err());
        assert!(Cli::try_parse_from(["podway", "unblock", "id", "--all"]).is_err());
        assert!(Cli::try_parse_from(["podway", "complete", "--unknown"]).is_err());
        assert!(Cli::try_parse_from(["podway", "daemon", "status"]).is_err());
    }

    #[test]
    fn timeout_parser_accepts_only_bounded_documented_units() {
        assert_eq!(parse_timeout_millis("500ms"), Ok(500));
        assert_eq!(parse_timeout_millis("30s"), Ok(30_000));
        assert_eq!(parse_timeout_millis("2m"), Ok(120_000));
        assert!(parse_timeout_millis("30").is_err());
        assert!(parse_timeout_millis("1h").is_err());
        assert!(parse_timeout_millis("61m").is_err());
    }
}
