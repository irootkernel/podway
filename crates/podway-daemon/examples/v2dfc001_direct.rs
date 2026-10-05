//! `V2DFC-001` direct-path example for full-invocation timing.
//!
//! Each measured command does the complete work a daemon-free CLI invocation would do: parse its
//! arguments, resolve the worktree through Git, take the cheap open, prepare and digest the
//! request, commit in one transaction, project the result, validate the `podway.output/v3`
//! envelope with the shipped validator, and print it. Mutation envelopes keep the current
//! generation's admission and job members with synthetic values so that validator can run;
//! `V2DFC-003` retires those members. `tools/v2dfc_gate.py` drives this example, and
//! `V2DFC-007` deletes it with the daemon crate.

use std::{
    env, fs,
    io::{self, Read, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use podway_core::{AttemptId, ItemId, JobId, Revision, SessionId, WorkspaceId};
use podway_daemon::v2_read_service::project_graph_observation_v1;
use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, ItemRecordManyDispositionV1, ItemRecordValueV1,
    JobOutputV1, JobStateV1, OperationV1, OutputEnvelopeInputV3, OutputEnvelopeV3, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1, WorkspaceOutputV1, WorktreeSelectorWireV1,
    canonical_mutation_identity_v1, decode_item_record_many_input_v1, validate_command_result_v2,
};
use podway_store::test_support::v2dfc001::{
    DirectCommitContextV1, DirectDetailV1, DirectMutationV1, DirectOutcomeV1, DirectRequestV1,
    OpenModeV1, OpenStoreV1, StoreBindingV1, WorkspaceLayoutV1, commit_direct_mutation_v1,
    convert_to_prototype_v1, grow_wal_v1, integrity_check_v1, open_store_v1,
    prepare_local_artifacts_v1, quick_check_v1, read_workspace_view_v1,
};
use podway_store::{ActiveItemMutationRequestV2, ActiveItemMutationV2, ValidatedWorkspaceRootV1};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const WRITER_IDENTITY: &str = "v2dfc001-prototype";
const COMMAND_DEADLINE: Duration = Duration::from_secs(5);
const WAIT_TIMEOUT_MS: u64 = 30_000;
const PHASES_PREFIX: &str = "V2DFC001_PHASES ";

fn main() -> ExitCode {
    let started = Instant::now();
    let arguments: Vec<String> = env::args().skip(1).collect();
    match run(started, &arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(io::stderr(), "{}", json!({"error": message}));
            ExitCode::FAILURE
        }
    }
}

/// Per-phase wall-clock durations of one invocation, printed only with `--phase-timings`.
struct PhasesV1 {
    enabled: bool,
    last: Instant,
    phases: Map<String, Value>,
}

impl PhasesV1 {
    fn new(enabled: bool, started: Instant) -> Self {
        Self {
            enabled,
            last: started,
            phases: Map::new(),
        }
    }

    fn mark(&mut self, name: &str) {
        let now = Instant::now();
        let elapsed = u64::try_from((now - self.last).as_micros()).unwrap_or(u64::MAX);
        self.phases.insert(format!("{name}_us"), json!(elapsed));
        self.last = now;
    }

    fn finish(self, started: Instant) {
        if self.enabled {
            let total = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let mut phases = self.phases;
            phases.insert("total_us".to_owned(), json!(total));
            let _ = writeln!(io::stderr(), "{PHASES_PREFIX}{}", Value::Object(phases));
        }
    }
}

/// Parsed global flags and the remaining command words.
struct ArgumentsV1 {
    phase_timings: bool,
    command: Vec<String>,
    flags: Map<String, Value>,
}

fn parse(arguments: &[String]) -> Result<ArgumentsV1, String> {
    let mut parsed = ArgumentsV1 {
        phase_timings: false,
        command: Vec::new(),
        flags: Map::new(),
    };
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        match argument.as_str() {
            "--phase-timings" => parsed.phase_timings = true,
            "--json" | "--stdin" => {
                parsed.flags.insert(argument.clone(), Value::Bool(true));
            }
            flag if flag.starts_with("--") => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                parsed.flags.insert(flag.to_owned(), json!(value));
                index += 1;
            }
            word => parsed.command.push(word.to_owned()),
        }
        index += 1;
    }
    Ok(parsed)
}

impl ArgumentsV1 {
    fn text(&self, flag: &str) -> Result<String, String> {
        self.flags
            .get(flag)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{flag} is required"))
    }

    fn number(&self, flag: &str) -> Result<u64, String> {
        self.text(flag)?
            .parse()
            .map_err(|_| format!("{flag} must be a non-negative integer"))
    }

    fn optional_number(&self, flag: &str, default: u64) -> Result<u64, String> {
        if self.flags.contains_key(flag) {
            self.number(flag)
        } else {
            Ok(default)
        }
    }
}

/// The resolved worktree and binding shared by every command.
struct WorktreeV1 {
    root: PathBuf,
    layout: WorkspaceLayoutV1,
    binding: StoreBindingV1,
}

fn resolve_worktree() -> Result<WorktreeV1, String> {
    let root = fs::canonicalize(env::current_dir().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let bytes = root.as_os_str().as_bytes();
    let display = DiagnosticPathDisplayV1::new(root.display().to_string())
        .map_err(|error| format!("{error:?}"))?;
    let lossless =
        LosslessPathV1::from_raw_bytes(bytes, display).map_err(|error| format!("{error:?}"))?;
    let selector = WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless)
        .map_err(|error| format!("{error:?}"))?;
    let worktree = NativeGitResolverV1::new()
        .resolve(selector)
        .map_err(|error| format!("worktree resolution failed: {error:?}"))?;
    let identity = worktree.identity();
    let binding = StoreBindingV1 {
        common_dir_identity: identity.common_directory_fingerprint().clone(),
        worktree_admin_identity: identity.worktree_administration_fingerprint().clone(),
        root: ValidatedWorkspaceRootV1::from_path(&root).map_err(|error| format!("{error:?}"))?,
        writer_identity: WRITER_IDENTITY.to_owned(),
    };
    Ok(WorktreeV1 {
        layout: WorkspaceLayoutV1::new(&root),
        root,
        binding,
    })
}

fn open(worktree: &WorktreeV1, mode: OpenModeV1) -> Result<OpenStoreV1, String> {
    open_store_v1(
        &worktree.layout,
        &worktree.binding,
        mode,
        Instant::now() + COMMAND_DEADLINE,
    )
    .map_err(|error| format!("{}: {error:?}", error.code()))
}

fn run(started: Instant, arguments: &[String]) -> Result<(), String> {
    let arguments = parse(arguments)?;
    let mut phases = PhasesV1::new(arguments.phase_timings, started);
    phases.mark("arguments");
    let words: Vec<&str> = arguments.command.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["observe"] => observe(&mut phases)?,
        ["set", item, value] => mutate(&arguments, &mut phases, Some((item, value)))?,
        ["complete"] | ["record"] => mutate(&arguments, &mut phases, None)?,
        ["convert"] => convert()?,
        ["hot-wal"] => hot_wal(&arguments)?,
        ["open-bench"] => open_bench(&arguments)?,
        ["integrity-bench"] => integrity_bench(&arguments)?,
        _ => return Err(format!("unsupported command {words:?}")),
    }
    phases.finish(started);
    Ok(())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn timestamp(millis: u64) -> Result<Rfc3339MillisV1, String> {
    Rfc3339MillisV1::from_unix_millis(millis).map_err(|error| format!("{error:?}"))
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn print_envelope(output: OutputEnvelopeV3) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &ResponseEnvelopeV2::OutputV2(output))
        .map_err(|error| error.to_string())?;
    writeln!(stdout).map_err(|error| error.to_string())
}

fn observe(phases: &mut PhasesV1) -> Result<(), String> {
    let worktree = resolve_worktree()?;
    phases.mark("resolve");
    let mut store = open(&worktree, OpenModeV1::ReadOnly)?;
    phases.mark("open");
    let view = read_workspace_view_v1(&mut store).map_err(|error| format!("{error:?}"))?;
    drop(store);
    phases.mark("read");
    let result = project_graph_observation_v1(&view).map_err(|error| format!("{error:?}"))?;
    phases.mark("project");
    let output = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
        request_id: RequestIdV1::new(new_uuid()).map_err(|error| format!("{error:?}"))?,
        command: CommandNameV1::new("session.observe").map_err(|error| format!("{error:?}"))?,
        generated_at: timestamp(now_millis())?,
        workspace: Some(
            WorkspaceOutputV1::new(
                view.identity().workspace_uuid().clone(),
                worktree.root.display().to_string(),
                view.latest_workspace_sequence(),
            )
            .map_err(|error| format!("{error:?}"))?,
        ),
        job: None,
        session: None,
        result,
        warnings: Vec::new(),
    })
    .map_err(|error| format!("{error:?}"))?;
    phases.mark("envelope");
    print_envelope(output)?;
    phases.mark("output");
    Ok(())
}

/// The fences and payload of one mutation, gathered from flags or the record document.
struct MutationInputV1 {
    command: &'static str,
    workspace_uuid: WorkspaceId,
    session_id: SessionId,
    attempt_id: AttemptId,
    idempotency_key: String,
    preconditions: PreconditionsV1,
    payload: Map<String, Value>,
    mutation: DirectMutationV1,
}

fn debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

fn mutation_input(
    arguments: &ArgumentsV1,
    set: Option<(&&str, &&str)>,
) -> Result<MutationInputV1, String> {
    if let Some((item, value)) = set {
        let session_id = SessionId::new(arguments.text("--if-session-id")?).map_err(debug)?;
        let attempt_id = AttemptId::new(arguments.text("--if-attempt")?).map_err(debug)?;
        let item_revision = Revision::new(arguments.number("--if-item-revision")?);
        let mut payload = Map::new();
        payload.insert("item_id".to_owned(), json!(item));
        payload.insert("value".to_owned(), json!(value));
        return Ok(MutationInputV1 {
            command: "item.set",
            workspace_uuid: WorkspaceId::new(arguments.text("--if-workspace-uuid")?)
                .map_err(debug)?,
            preconditions: PreconditionsV1::new(
                Some(session_id.clone()),
                None,
                Some(attempt_id.clone()),
                Some(item_revision),
                None,
                None,
            )
            .map_err(|error| format!("{error:?}"))?,
            session_id,
            attempt_id,
            idempotency_key: arguments.text("--idempotency-key")?,
            payload,
            mutation: DirectMutationV1::Set {
                item_id: ItemId::new(*item).map_err(debug)?,
                expected_item_revision: item_revision,
                value: (*value).to_owned(),
            },
        });
    }
    if arguments.command[0] == "complete" {
        let session_id = SessionId::new(arguments.text("--if-session-id")?).map_err(debug)?;
        let attempt_id = AttemptId::new(arguments.text("--if-attempt")?).map_err(debug)?;
        let session_revision = Revision::new(arguments.number("--if-session-revision")?);
        return Ok(MutationInputV1 {
            command: "session.complete",
            workspace_uuid: WorkspaceId::new(arguments.text("--if-workspace-uuid")?)
                .map_err(debug)?,
            preconditions: PreconditionsV1::new(
                Some(session_id.clone()),
                Some(session_revision),
                Some(attempt_id.clone()),
                None,
                None,
                None,
            )
            .map_err(|error| format!("{error:?}"))?,
            session_id,
            attempt_id,
            idempotency_key: arguments.text("--idempotency-key")?,
            payload: Map::new(),
            mutation: DirectMutationV1::Complete {
                expected_session_revision: session_revision,
                fresh_attempt_id: AttemptId::new(new_uuid()).map_err(debug)?,
                prepared_artifacts: Vec::new(),
            },
        });
    }
    if arguments.flags.get("--stdin") != Some(&Value::Bool(true)) {
        return Err("record requires --stdin".to_owned());
    }
    let mut bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let input = decode_item_record_many_input_v1(&bytes).map_err(|error| format!("{error:?}"))?;
    let operations = input
        .operations
        .iter()
        .map(|operation| {
            let mutation = match &operation.disposition {
                ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Text { value },
                } => ActiveItemMutationV2::Set {
                    value: value.clone(),
                },
                ItemRecordManyDispositionV1::Clear { clear: true } => ActiveItemMutationV2::Clear,
                _ => return Err("the prototype records text values and clears only".to_owned()),
            };
            Ok(ActiveItemMutationRequestV2::new(
                operation.item_id.clone(),
                operation.expected_item_revision,
                mutation,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut payload = Map::new();
    payload.insert(
        "operations".to_owned(),
        serde_json::to_value(&input.operations).map_err(|error| error.to_string())?,
    );
    Ok(MutationInputV1 {
        command: "item.record_many",
        workspace_uuid: input.workspace_uuid.clone(),
        preconditions: PreconditionsV1::new(
            Some(input.session_id.clone()),
            Some(input.session_revision),
            Some(input.attempt_id.clone()),
            None,
            None,
            None,
        )
        .map_err(|error| format!("{error:?}"))?,
        session_id: input.session_id.clone(),
        attempt_id: input.attempt_id.clone(),
        idempotency_key: input.idempotency_key.as_str().to_owned(),
        payload,
        mutation: DirectMutationV1::Record {
            expected_session_revision: input.session_revision,
            operations,
        },
    })
}

/// Builds the request envelope the CLI would send and derives its canonical identity digest.
fn request_identity(
    worktree: &WorktreeV1,
    input: &MutationInputV1,
) -> Result<(String, String), String> {
    let selector = WorktreeSelectorWireV1::new(
        worktree.root.as_os_str().as_bytes(),
        worktree.root.display().to_string(),
        Some(input.workspace_uuid.clone()),
    )
    .map_err(|error| format!("{error:?}"))?;
    let mut payload = input.payload.clone();
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&selector).map_err(|error| error.to_string())?,
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(new_uuid()).map_err(|error| format!("{error:?}"))?,
        client: ClientInfoV1::new("podway-v2dfc001-direct", "1", std::process::id())
            .map_err(|error| format!("{error:?}"))?,
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(input.command).map_err(|error| format!("{error:?}"))?,
        workspace: Some(
            WorkspaceContextV1::new(
                worktree.root.display().to_string(),
                Some(input.workspace_uuid.clone()),
            )
            .map_err(|error| format!("{error:?}"))?,
        ),
        idempotency_key: Some(
            IdempotencyKeyV1::new(input.idempotency_key.clone())
                .map_err(|error| format!("{error:?}"))?,
        ),
        preconditions: input.preconditions.clone(),
        options: RequestOptionsV1::new(false, WAIT_TIMEOUT_MS)
            .map_err(|error| format!("{error:?}"))?,
        payload,
    })
    .map_err(|error| format!("{error:?}"))?;
    let slice = SliceRequestV1::from_envelope(&envelope).map_err(|error| format!("{error:?}"))?;
    let canonical = canonical_mutation_identity_v1(&slice, &input.workspace_uuid)
        .map_err(|error| format!("{error:?}"))?;
    let digest = format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()));
    Ok((canonical, digest))
}

fn admission(job_id: &str, sequence: u64) -> Value {
    json!({"admitted": true, "job_id": job_id, "workspace_sequence": sequence})
}

/// Renders the success result exactly as the daemon's terminal renderer shapes it.
fn mutation_result(
    command: &str,
    context: &DirectCommitContextV1<'_>,
    job_id: &str,
) -> Result<Map<String, Value>, String> {
    let active = context
        .previous
        .trace()
        .active_attempt()
        .ok_or("the mutated session has no active attempt")?;
    let revision = context.next.trace().revision().get();
    let admission = admission(job_id, context.workspace_sequence);
    let mut result = match (command, context.detail) {
        ("item.set", DirectDetailV1::Item(item)) => {
            let mut result = json!({
                "schema": "podway.item-mutation-result/v2",
                "changed": item.changed,
                "graph_node_id": active.graph_node_id(),
                "attempt_id": active.attempt_id(),
                "attempt_number": active.number().get(),
                "item_id": item.item_id,
                "revision": revision,
                "admission": admission,
            });
            if let Some(digest) = &item.value_digest {
                result["value_digest"] = json!(digest);
            }
            result
        }
        ("item.record_many", DirectDetailV1::Items { changed, items }) => {
            let items: Vec<Value> = items
                .iter()
                .map(|item| {
                    let mut entry = json!({
                        "item_id": item.item_id,
                        "expected_item_revision": item.expected_item_revision,
                        "changed": item.changed,
                        "item_revision": item.item_revision,
                    });
                    if let Some(digest) = &item.value_digest {
                        entry["value_digest"] = json!(digest);
                    }
                    entry
                })
                .collect();
            json!({
                "schema": "podway.item-record-many-result/v1",
                "changed": changed,
                "graph_node_id": active.graph_node_id(),
                "attempt_id": active.attempt_id(),
                "attempt_number": active.number().get(),
                "revision": revision,
                "items": items,
                "admission": admission,
            })
        }
        (
            "session.complete",
            DirectDetailV1::Complete {
                from_graph_node_id,
                from_attempt_id,
                to_graph_node_id,
                to_attempt_id,
            },
        ) => {
            let mut result = json!({
                "schema": "podway.stage-transition-result/v2",
                "transition": "complete",
                "from_graph_node_id": from_graph_node_id,
                "from_attempt_id": from_attempt_id,
                "revision": revision,
                "session_state": if to_graph_node_id.is_some() { "running" } else { "completed" },
                "admission": admission,
            });
            if let (Some(node), Some(attempt)) = (to_graph_node_id, to_attempt_id) {
                result["to_graph_node_id"] = json!(node);
                result["to_attempt_id"] = json!(attempt);
            }
            result
        }
        _ => return Err("the commit detail does not match the command".to_owned()),
    };
    Ok(result
        .as_object_mut()
        .ok_or("result is not an object")?
        .clone())
}

/// Compiles the shipped result validators before the write lock by validating a result of the
/// command's own shape, so the transaction validates the real envelope with warm validators.
fn warm_validators(command: &str) {
    let placeholder = "00000000-0000-4000-8000-000000000000";
    let admission = admission(placeholder, 1);
    let result = match command {
        "item.set" => json!({
            "schema": "podway.item-mutation-result/v2", "changed": true,
            "graph_node_id": "n0001", "attempt_id": placeholder, "attempt_number": 1,
            "item_id": "i01", "revision": 1, "admission": admission,
        }),
        "item.record_many" => json!({
            "schema": "podway.item-record-many-result/v1", "changed": true,
            "graph_node_id": "n0001", "attempt_id": placeholder, "attempt_number": 1,
            "revision": 1, "items": [], "admission": admission,
        }),
        _ => json!({
            "schema": "podway.stage-transition-result/v2", "transition": "complete",
            "from_graph_node_id": "n0001", "from_attempt_id": placeholder, "revision": 1,
            "session_state": "completed", "admission": admission,
        }),
    };
    if let Some(result) = result.as_object() {
        let _ = validate_command_result_v2(command, result);
    }
}

fn mutate(
    arguments: &ArgumentsV1,
    phases: &mut PhasesV1,
    set: Option<(&&str, &&str)>,
) -> Result<(), String> {
    let mut input = mutation_input(arguments, set)?;
    phases.mark("input");
    let worktree = resolve_worktree()?;
    phases.mark("resolve");
    let mut store = open(&worktree, OpenModeV1::Mutation)?;
    if store.identity().workspace_uuid() != &input.workspace_uuid {
        return Err("PRECONDITION_FAILED: workspace UUID fence failed".to_owned());
    }
    phases.mark("open");
    if let DirectMutationV1::Complete {
        prepared_artifacts, ..
    } = &mut input.mutation
    {
        let view = read_workspace_view_v1(&mut store).map_err(|error| format!("{error:?}"))?;
        if let Some(state) = view.graph_state() {
            *prepared_artifacts = prepare_local_artifacts_v1(&worktree.layout, state)
                .map_err(|error| format!("{}: {error:?}", error.code()))?;
        }
    }
    let (canonical, digest) = request_identity(&worktree, &input)?;
    let request = DirectRequestV1 {
        idempotency_key: input.idempotency_key.clone(),
        request_digest: digest,
        canonical_request: canonical,
        expected_session_id: input.session_id.clone(),
        expected_attempt_id: input.attempt_id.clone(),
        mutation: input.mutation.clone(),
    };
    phases.mark("prepare");
    warm_validators(input.command);
    phases.mark("validator_compile");
    let workspace_uuid = store.identity().workspace_uuid().clone();
    let root = worktree.root.display().to_string();
    let submitted = now_millis();
    let command = input.command;
    let mut respond = |context: &DirectCommitContextV1<'_>| -> Result<String, String> {
        let job_id = new_uuid();
        let finished = timestamp(context.committed_at.get())?;
        let output = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
            request_id: RequestIdV1::new(new_uuid()).map_err(|error| format!("{error:?}"))?,
            command: CommandNameV1::new(command).map_err(|error| format!("{error:?}"))?,
            generated_at: finished.clone(),
            workspace: Some(
                WorkspaceOutputV1::new(
                    workspace_uuid.clone(),
                    root.clone(),
                    context.workspace_sequence,
                )
                .map_err(|error| format!("{error:?}"))?,
            ),
            job: Some(
                JobOutputV1::new(
                    JobId::new(job_id.clone()).map_err(debug)?,
                    context.workspace_sequence,
                    JobStateV1::Succeeded,
                    timestamp(submitted)?,
                    Some(timestamp(submitted)?),
                    Some(finished),
                )
                .map_err(|error| format!("{error:?}"))?,
            ),
            session: None,
            result: mutation_result(command, context, &job_id)?,
            warnings: Vec::new(),
        })
        .map_err(|error| format!("{error:?}"))?;
        serde_json::to_string(&ResponseEnvelopeV2::OutputV2(output))
            .map_err(|error| error.to_string())
    };
    let outcome = commit_direct_mutation_v1(
        &mut store,
        &request,
        Instant::now() + COMMAND_DEADLINE,
        &mut |_| {},
        &mut respond,
    )
    .map_err(|error| format!("{}: {error:?}", error.code()))?;
    drop(store);
    phases.mark("transaction");
    let response = match outcome {
        DirectOutcomeV1::Committed { response, .. }
        | DirectOutcomeV1::Replayed { response, .. } => response,
    };
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(response.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .map_err(|error| error.to_string())?;
    phases.mark("output");
    Ok(())
}

fn convert() -> Result<(), String> {
    let worktree = resolve_worktree()?;
    let epoch = convert_to_prototype_v1(&worktree.layout, &worktree.binding)
        .map_err(|error| format!("{error:?}"))?;
    println!("{}", json!({"converted": true, "epoch": epoch}));
    Ok(())
}

fn snapshot_paths(directory: &Path) -> (PathBuf, PathBuf) {
    (
        directory.join("state.sqlite3"),
        directory.join("state.sqlite3-wal"),
    )
}

/// Grows a hot WAL with automatic checkpoints disabled and copies the database and WAL, without
/// a shared-memory file, while the writing connection remains open.
fn hot_wal(arguments: &ArgumentsV1) -> Result<(), String> {
    let worktree = resolve_worktree()?;
    let directory = PathBuf::from(arguments.text("--snapshot")?);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let minimum = arguments.number("--min-wal-bytes")?;
    let mut store = open(&worktree, OpenModeV1::Mutation)?;
    store
        .disable_auto_checkpoint()
        .map_err(|error| format!("{error:?}"))?;
    let size = grow_wal_v1(&mut store, minimum).map_err(|error| format!("{error:?}"))?;
    let (database, wal) = snapshot_paths(&directory);
    fs::copy(worktree.layout.database(), &database).map_err(|error| error.to_string())?;
    fs::copy(worktree.layout.wal(), &wal).map_err(|error| error.to_string())?;
    drop(store);
    println!(
        "{}",
        json!({"wal_bytes": size, "database_bytes": fs::metadata(&database).map_err(|error| error.to_string())?.len()})
    );
    Ok(())
}

fn restore_cold(worktree: &WorktreeV1, directory: &Path) -> Result<(), String> {
    let (database, wal) = snapshot_paths(directory);
    let _ = fs::remove_file(worktree.layout.shm());
    fs::copy(&database, worktree.layout.database()).map_err(|error| error.to_string())?;
    fs::copy(&wal, worktree.layout.wal()).map_err(|error| error.to_string())?;
    Ok(())
}

/// Times the cheap open alone. Warm opens run while another connection keeps the WAL and
/// shared memory alive; cold opens start from a restored hot WAL without shared memory.
fn open_bench(arguments: &ArgumentsV1) -> Result<(), String> {
    let worktree = resolve_worktree()?;
    let mode = match arguments.text("--mode")?.as_str() {
        "ro" => OpenModeV1::ReadOnly,
        "rw" => OpenModeV1::Mutation,
        other => return Err(format!("unknown open mode {other}")),
    };
    let cold = match arguments.text("--state")?.as_str() {
        "warm" => false,
        "cold" => true,
        other => return Err(format!("unknown open state {other}")),
    };
    let warmup = arguments.optional_number("--warmup", 20)?;
    let runs = arguments.optional_number("--runs", 220)?;
    let snapshot = cold
        .then(|| arguments.text("--snapshot").map(PathBuf::from))
        .transpose()?;
    let holder = if cold {
        None
    } else {
        Some(open(&worktree, OpenModeV1::ReadOnly)?)
    };
    let mut samples = Vec::new();
    for run in 0..warmup + runs {
        if let Some(snapshot) = &snapshot {
            restore_cold(&worktree, snapshot)?;
        }
        let started = Instant::now();
        let store = open(&worktree, mode)?;
        let elapsed = started.elapsed();
        drop(store);
        if run >= warmup {
            samples.push(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
        }
    }
    drop(holder);
    println!(
        "{}",
        json!({
            "mode": arguments.text("--mode")?,
            "state": arguments.text("--state")?,
            "warmup": warmup,
            "runs": runs,
            "samples_us": samples,
        })
    );
    Ok(())
}

/// Times `quick_check` and `integrity_check` on the worktree's Store.
fn integrity_bench(arguments: &ArgumentsV1) -> Result<(), String> {
    let worktree = resolve_worktree()?;
    let warmup = arguments.optional_number("--warmup", 3)?;
    let runs = arguments.optional_number("--runs", 20)?;
    let mut store = open(&worktree, OpenModeV1::ReadOnly)?;
    let mut quick = Vec::new();
    let mut full = Vec::new();
    for run in 0..warmup + runs {
        let started = Instant::now();
        let quick_result = quick_check_v1(&mut store).map_err(|error| format!("{error:?}"))?;
        let quick_elapsed = started.elapsed();
        let started = Instant::now();
        let full_result = integrity_check_v1(&mut store).map_err(|error| format!("{error:?}"))?;
        let full_elapsed = started.elapsed();
        if quick_result != "ok" || full_result != "ok" {
            return Err(format!("integrity failed: {quick_result} {full_result}"));
        }
        if run >= warmup {
            quick.push(u64::try_from(quick_elapsed.as_micros()).unwrap_or(u64::MAX));
            full.push(u64::try_from(full_elapsed.as_micros()).unwrap_or(u64::MAX));
        }
    }
    println!(
        "{}",
        json!({
            "warmup": warmup,
            "runs": runs,
            "database_bytes": fs::metadata(worktree.layout.database()).map_err(|error| error.to_string())?.len(),
            "quick_check_us": quick,
            "integrity_check_us": full,
        })
    );
    Ok(())
}
