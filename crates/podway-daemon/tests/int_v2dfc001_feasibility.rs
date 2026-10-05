//! `V2DFC-001` feasibility gate harness for the daemon-free direct-write model.
//!
//! Every check is ignored. `tools/v2dfc_gate.py` runs the checks in the release profile, reads
//! the raw samples each one writes under `V2DFC001_OUTPUT_DIR`, and owns the percentile
//! verdicts. Each check asserts its own correctness oracle and re-executes this test binary for
//! its child processes. `V2DFC-007` deletes this harness with the daemon crate.

use crate::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{BufRead, BufReader, Lines, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, ItemId, Revision, SessionId, canonicalize_json_v1};
use podway_git::{GitResolverContractV1, NativeGitResolverV1};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2};
use podway_store::test_support::v2dfc001::{
    DirectBoundaryV1, DirectCommitContextV1, DirectMutationV1, DirectOutcomeV1, DirectRequestV1,
    LEGACY_PODWAY_MARKER_V1, LEGACY_RUNTIME_MARKERS_V1, LegacyHeaderV1, MaintenanceBoundaryV1,
    MaintenanceOutcomeV1, MaintenanceRequestV1, OpenModeV1, OpenStoreV1, PreparedArtifactV1,
    PrototypeErrorV1, RetiredGenerationV1, StoreBindingV1, StoreClassificationV1, StoreSeedV1,
    WorkspaceLayoutV1, apply_direct_mutation_v1, classify_store_v1, commit_direct_mutation_v1,
    convert_to_prototype_v1, create_workspace_v1, hold_write_lock_v1, integrity_check_v1,
    list_outcomes_v1, lookup_outcome_v1, open_store_v1, prepare_local_artifacts_v1,
    read_workspace_view_v1, remove_workspace_v1, reset_all_v1, wal_size_bytes_v1,
    write_legacy_store_v1,
};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, GraphSessionStateV2,
    ValidatedWorkspaceRootV1,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const OUTPUT_ENV: &str = "V2DFC001_OUTPUT_DIR";
const PROFILE_ENV: &str = "V2DFC001_PROFILE";
const CHILD_ENV: &str = "V2DFC001_CHILD";
const CHILD_TEST: &str = "int_v2dfc001_feasibility::v2dfc001_child";
const PROTOCOL_PREFIX: &str = "V2DFC001 ";
const WRITER_IDENTITY: &str = "v2dfc001-prototype";
const ITEM_COUNT: u64 = 16;
/// Chain length of the common gate fixture, shared with the G7 latency fixtures.
const GATE_NODES: usize = 300;
/// ADR-0037 maximum, measured only as information for later Store work.
const SCALE_NODES: usize = 1_024;
const CONTENDER_RETRY_PAUSE: Duration = Duration::from_millis(1);
const COMMAND_DEADLINE: Duration = Duration::from_secs(5);
const MAINTENANCE_DEADLINE: Duration = Duration::from_secs(30);
const G3_DEADLINE: Duration = Duration::from_millis(500);
const G9_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
const G9_DRAIN_INTERVAL: Duration = Duration::from_millis(50);
const G9_DRAIN_WINDOW: Duration = Duration::from_secs(5);
/// Each G9 writer owes a complete every period-th attempt, at most this many times, so eight
/// writers stay inside the 300-node chain.
const G9_COMPLETE_PERIOD: u64 = 192;
const G9_COMPLETES_PER_WRITER: u64 = 36;
const STORE_CLOSE_WAIT: Duration = Duration::from_secs(30);
const SIGKILL: i32 = 9;
const ARTIFACT_PATH: &str = "gate-evidence.txt";
const ARTIFACT_ORIGINAL: &[u8] = b"V2DFC-001 attached evidence\n";
const ARTIFACT_CHANGED: &[u8] = b"V2DFC-001 changed after preparation\n";

/// Pre-registered parameters. Only `evidence` runs count as gate evidence.
#[derive(Clone, Copy, Debug)]
struct ProfileV1 {
    name: &'static str,
    g1_writers: u64,
    g1_successes: u64,
    g1_observers: u64,
    scale_successes: u64,
    g3_warmup: u64,
    g3_measured: u64,
    g4_per_boundary: u64,
    g4_random: u64,
    g4_calibration: u64,
    g5_cycles: u64,
    g5_writers: u64,
    g5_readers: u64,
    g6_state: u64,
    g6_artifact: u64,
    g9_seconds: u64,
    g9_writers: u64,
    g9_readers: u64,
}

const EVIDENCE: ProfileV1 = ProfileV1 {
    name: "evidence",
    g1_writers: 8,
    g1_successes: 1_000,
    g1_observers: 2,
    scale_successes: 200,
    g3_warmup: 20,
    g3_measured: 200,
    g4_per_boundary: 20,
    g4_random: 200,
    g4_calibration: 20,
    g5_cycles: 100,
    g5_writers: 4,
    g5_readers: 4,
    g6_state: 500,
    g6_artifact: 500,
    g9_seconds: 600,
    g9_writers: 8,
    g9_readers: 4,
};

const QUICK: ProfileV1 = ProfileV1 {
    name: "quick",
    g1_writers: 8,
    g1_successes: 100,
    g1_observers: 2,
    scale_successes: 20,
    g3_warmup: 2,
    g3_measured: 10,
    g4_per_boundary: 2,
    g4_random: 10,
    g4_calibration: 5,
    g5_cycles: 3,
    g5_writers: 4,
    g5_readers: 4,
    g6_state: 30,
    g6_artifact: 30,
    g9_seconds: 20,
    g9_writers: 8,
    g9_readers: 4,
};

fn profile() -> ProfileV1 {
    match env::var(PROFILE_ENV).as_deref() {
        Ok("evidence") => EVIDENCE,
        _ => QUICK,
    }
}

fn output_directory(check: &str) -> PathBuf {
    let root = env::var_os(OUTPUT_ENV).map_or_else(
        || env::temp_dir().join(format!("v2dfc001-{}", std::process::id())),
        PathBuf::from,
    );
    let directory = root.join(check);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn write_summary(directory: &Path, check: &str, profile: ProfileV1, mut summary: Value) {
    let object = summary.as_object_mut().unwrap();
    object.insert("check".to_owned(), json!(check));
    object.insert("profile".to_owned(), json!(profile.name));
    fs::write(
        directory.join("summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
}

fn append_jsonl(file: &mut fs::File, value: &Value) {
    serde_json::to_writer(&mut *file, value).unwrap();
    file.write_all(b"\n").unwrap();
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn write_samples(directory: &Path, samples: &[Value]) {
    let mut file = fs::File::create(directory.join("samples.jsonl")).unwrap();
    for sample in samples {
        append_jsonl(&mut file, sample);
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Small deterministic generator for mutation choices and kill delays.
struct RandomV1(u64);

impl RandomV1 {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn item_id(index: u64) -> String {
    format!("i{:02}", index + 1)
}

fn payload(tag: &str, length: usize) -> String {
    let mut value = String::with_capacity(length);
    value.push_str(tag);
    value.push('-');
    while value.len() < length {
        value.push(char::from(b'a' + u8::try_from(value.len() % 26).unwrap()));
    }
    value.truncate(length);
    value
}

fn chain_procedure(nodes: usize, artifact: bool) -> String {
    let mut items: String = (0..ITEM_COUNT)
        .map(|index| {
            format!(
                "      - id: {}\n        type: text\n        prompt: Record payload {}.\n        required: false\n        max_length: 65536\n",
                item_id(index),
                index + 1
            )
        })
        .collect();
    if artifact {
        items.push_str(
            "      - id: evidence\n        type: artifact\n        prompt: Attach the evidence.\n        required: false\n        allowed_media_types:\n          - text/plain\n",
        );
    }
    let placements: String = (1..=nodes)
        .map(|node| {
            let tail = if node == nodes {
                "      terminal: true\n".to_owned()
            } else {
                format!("      next: n{:04}\n", node + 1)
            };
            format!("    - id: n{node:04}\n      use: work\n{tail}")
        })
        .collect();
    format!(
        "schema: podway.procedure/v2\nid: v2dfc001-gate\nversion: \"1\"\nname: V2DFC-001 gate\npurpose: Measure the direct-write model.\nnode_definitions:\n  work:\n    type: action\n    title: Record payloads\n    intent: Record gate payloads.\n    items:\n{items}graph:\n  entry: n0001\n  nodes:\n{placements}"
    )
}

/// One converted prototype worktree and the state it started from.
struct GateFixtureV1 {
    _worktrees: support_phase4_workspace::GitWorktreeFixtureV1,
    layout: WorkspaceLayoutV1,
    binding: StoreBindingV1,
    initial: GraphSessionStateV2,
}

/// Starts a session through the in-process daemon, closes it, and converts the Store.
fn gate_fixture(nodes: usize, artifact: bool) -> GateFixtureV1 {
    let worktrees = support_phase4_workspace::git_worktrees();
    let root = fs::canonicalize(worktrees.main()).unwrap();
    daemon_session(&root, worktrees.temporary_path(), nodes, artifact);
    let layout = WorkspaceLayoutV1::new(&root);
    let binding = resolved_binding(&root);
    convert_to_prototype_v1(&layout, &binding).unwrap();
    let initial = read_state(&layout, &binding);
    GateFixtureV1 {
        _worktrees: worktrees,
        layout,
        binding,
        initial,
    }
}

/// Creates a generation-13 Store with a running session and waits until every handle closed.
fn daemon_session(root: &Path, service_root: &Path, nodes: usize, artifact: bool) {
    runtime::make_runtime_private(root);
    let procedure = chain_procedure(nodes, artifact);
    fs::write(root.join("gate.yaml"), &procedure).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(procedure.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("the gate fixture must be Procedure v2")
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    {
        let manager = Arc::new(runtime::manager(service_root));
        let dispatcher = runtime::dispatcher(manager, "v2dfc001-fixture");
        let selector = runtime::selector(root);
        let initialize = runtime::request(
            1,
            "workspace.init",
            &selector,
            Map::new(),
            "v2dfc001-initialize",
            PreconditionsV1::default(),
        );
        assert!(matches!(
            runtime::dispatch(&dispatcher, &initialize),
            ResponseEnvelopeV2::OutputV2(_)
        ));
        let start = runtime::request(
            2,
            "session.start",
            &selector,
            json!({
                "procedure": "gate.yaml",
                "expected_procedure_digest": digest,
                "task_title": "V2DFC-001 gate",
            })
            .as_object()
            .unwrap()
            .clone(),
            "v2dfc001-start",
            PreconditionsV1::default(),
        );
        let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
        let session_id = started["session_id"].as_str().unwrap().to_owned();
        runtime::begin(
            &dispatcher,
            &selector,
            3,
            &session_id,
            Map::new(),
            "v2dfc001-begin",
        );
        if artifact {
            fs::write(root.join(ARTIFACT_PATH), ARTIFACT_ORIGINAL).unwrap();
            runtime::mutate_item(
                &dispatcher,
                &selector,
                4,
                &session_id,
                "item.attach",
                "evidence",
                json!({"path": ARTIFACT_PATH, "media_type": "text/plain"})
                    .as_object()
                    .unwrap()
                    .clone(),
                "v2dfc001-attach",
            );
        }
    }
    let wal = WorkspaceLayoutV1::new(root).wal();
    let deadline = Instant::now() + STORE_CLOSE_WAIT;
    while wal.exists() {
        assert!(
            Instant::now() < deadline,
            "the in-process daemon must close the gate Store"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn resolved_binding(root: &Path) -> StoreBindingV1 {
    let worktree = NativeGitResolverV1::new()
        .resolve(support_phase4_workspace::selector(root))
        .expect("the gate worktree must resolve");
    let identity = worktree.identity();
    StoreBindingV1 {
        common_dir_identity: identity.common_directory_fingerprint().clone(),
        worktree_admin_identity: identity.worktree_administration_fingerprint().clone(),
        root: ValidatedWorkspaceRootV1::from_path(root).unwrap(),
        writer_identity: WRITER_IDENTITY.to_owned(),
    }
}

fn open(layout: &WorkspaceLayoutV1, binding: &StoreBindingV1, mode: OpenModeV1) -> OpenStoreV1 {
    open_store_v1(layout, binding, mode, Instant::now() + COMMAND_DEADLINE).unwrap()
}

fn read_state(layout: &WorkspaceLayoutV1, binding: &StoreBindingV1) -> GraphSessionStateV2 {
    let mut store = open(layout, binding, OpenModeV1::ReadOnly);
    read_workspace_view_v1(&mut store)
        .unwrap()
        .graph_state()
        .expect("the gate session must exist")
        .clone()
}

fn read_current_state(store: &mut OpenStoreV1) -> Result<GraphSessionStateV2, PrototypeErrorV1> {
    read_workspace_view_v1(store)?
        .graph_state()
        .cloned()
        .ok_or_else(|| PrototypeErrorV1::Rejected {
            reason: "no current session".to_owned(),
        })
}

fn item_revision(state: &GraphSessionStateV2, item: &str) -> Revision {
    let Some(active) = state.trace().active_attempt() else {
        return Revision::ZERO;
    };
    state
        .workflow_memory()
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == active.attempt_id())
        .and_then(|attempt| {
            attempt
                .item_slots()
                .iter()
                .find(|slot| slot.item_id().as_str() == item)
        })
        .map_or(Revision::ZERO, |slot| slot.revision())
}

fn active_attempt_id(state: &GraphSessionStateV2) -> String {
    state
        .trace()
        .active_attempt()
        .map_or_else(String::new, |active| {
            active.attempt_id().as_str().to_owned()
        })
}

fn canonical_set(state: &GraphSessionStateV2, item: &str, value: String) -> Value {
    json!({
        "command": "item.set",
        "session_id": state.trace().session_id().as_str(),
        "attempt_id": active_attempt_id(state),
        "item_id": item,
        "expected_item_revision": item_revision(state, item).get(),
        "value": value,
    })
}

fn canonical_record(state: &GraphSessionStateV2, items: &[(String, String)]) -> Value {
    let operations: Vec<Value> = items
        .iter()
        .map(|(item, value)| {
            json!({
                "item_id": item,
                "expected_item_revision": item_revision(state, item).get(),
                "value": value,
            })
        })
        .collect();
    json!({
        "command": "item.record_many",
        "session_id": state.trace().session_id().as_str(),
        "attempt_id": active_attempt_id(state),
        "expected_session_revision": state.trace().revision().get(),
        "operations": operations,
    })
}

fn canonical_complete(state: &GraphSessionStateV2) -> Value {
    json!({
        "command": "session.complete",
        "session_id": state.trace().session_id().as_str(),
        "attempt_id": active_attempt_id(state),
        "expected_session_revision": state.trace().revision().get(),
        "fresh_attempt_id": uuid::Uuid::new_v4().to_string(),
    })
}

fn canonical_text(canonical: &Value) -> String {
    canonicalize_json_v1(canonical).unwrap()
}

/// Rebuilds the exact request from its canonical text, which also seeds the request digest.
fn direct_request(
    key: &str,
    canonical: &str,
    prepared_artifacts: Vec<PreparedArtifactV1>,
) -> DirectRequestV1 {
    let value: Value = serde_json::from_str(canonical).unwrap();
    let text = |name: &str| value[name].as_str().unwrap().to_owned();
    let revision = |field: &Value| Revision::new(field.as_u64().unwrap());
    let mutation = match value["command"].as_str().unwrap() {
        "item.set" => DirectMutationV1::Set {
            item_id: ItemId::new(text("item_id")).unwrap(),
            expected_item_revision: revision(&value["expected_item_revision"]),
            value: text("value"),
        },
        "item.record_many" => DirectMutationV1::Record {
            expected_session_revision: revision(&value["expected_session_revision"]),
            operations: value["operations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|operation| {
                    ActiveItemMutationRequestV2::new(
                        ItemId::new(operation["item_id"].as_str().unwrap()).unwrap(),
                        revision(&operation["expected_item_revision"]),
                        ActiveItemMutationV2::Set {
                            value: operation["value"].as_str().unwrap().to_owned(),
                        },
                    )
                })
                .collect(),
        },
        "session.complete" => DirectMutationV1::Complete {
            expected_session_revision: revision(&value["expected_session_revision"]),
            fresh_attempt_id: AttemptId::new(text("fresh_attempt_id")).unwrap(),
            prepared_artifacts,
        },
        command => panic!("unsupported gate command {command}"),
    };
    DirectRequestV1 {
        idempotency_key: key.to_owned(),
        request_digest: format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())),
        canonical_request: canonical.to_owned(),
        expected_session_id: SessionId::new(text("session_id")).unwrap(),
        expected_attempt_id: AttemptId::new(text("attempt_id")).unwrap(),
        mutation,
    }
}

fn compact_response(context: &DirectCommitContextV1<'_>) -> Result<String, String> {
    Ok(json!({
        "workspace_sequence": context.workspace_sequence,
        "revision": context.next.trace().revision().get(),
    })
    .to_string())
}

fn commit(
    store: &mut OpenStoreV1,
    request: &DirectRequestV1,
    deadline: Instant,
) -> Result<DirectOutcomeV1, PrototypeErrorV1> {
    commit_direct_mutation_v1(store, request, deadline, &mut |_| {}, &mut compact_response)
}

/// Commits and times the IMMEDIATE transaction from its hooks, so replays and rejections count
/// toward G2 as commits do. The wait ends when BEGIN returns; the hold ends at COMMIT or, for a
/// replay or rejection, when the rolled-back attempt returns.
fn timed_commit(
    store: &mut OpenStoreV1,
    request: &DirectRequestV1,
    deadline: Instant,
) -> (
    Result<DirectOutcomeV1, PrototypeErrorV1>,
    Option<(Duration, Duration)>,
) {
    let (mut begin, mut acquired, mut committed) = (None, None, None);
    let result = commit_direct_mutation_v1(
        store,
        request,
        deadline,
        &mut |boundary| match boundary {
            DirectBoundaryV1::BeforeBegin => begin = Some(Instant::now()),
            DirectBoundaryV1::AfterBegin => acquired = Some(Instant::now()),
            DirectBoundaryV1::AfterStateWrite => {}
            DirectBoundaryV1::AfterCommit => committed = Some(Instant::now()),
        },
        &mut compact_response,
    );
    let ended = committed.unwrap_or_else(Instant::now);
    let timing = begin.map(|begin| match acquired {
        Some(acquired) => (acquired - begin, ended - acquired),
        None => (ended - begin, Duration::ZERO),
    });
    (result, timing)
}

fn log_transaction(log: &mut fs::File, outcome: &str, timing: Option<(Duration, Duration)>) {
    if let Some((wait, hold)) = timing {
        append_jsonl(
            log,
            &json!({
                "kind": "transaction",
                "outcome": outcome,
                "wait_us": micros(wait),
                "hold_us": micros(hold),
            }),
        );
    }
}

fn maintenance_request(key: &str) -> MaintenanceRequestV1 {
    MaintenanceRequestV1 {
        idempotency_key: key.to_owned(),
        request_digest: format!("sha256:{:x}", Sha256::digest(key.as_bytes())),
    }
}

/// Copies the converted Store and its `.podway` tree as the seed for maintenance.
fn prepare_seed(fixture: &GateFixtureV1, directory: &Path) -> StoreSeedV1 {
    let seed = directory.join("seed");
    if seed.exists() {
        fs::remove_dir_all(&seed).unwrap();
    }
    fs::create_dir_all(&seed).unwrap();
    let database = seed.join("state.sqlite3");
    fs::copy(fixture.layout.database(), &database).unwrap();
    let tree = seed.join("podway");
    copy_tree_without_runtime(&fixture.layout.podway_directory(), &tree);
    StoreSeedV1 {
        database,
        podway_tree: tree,
    }
}

fn copy_tree_without_runtime(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == "runtime" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree_without_runtime(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn boundary_name(boundary: DirectBoundaryV1) -> &'static str {
    match boundary {
        DirectBoundaryV1::BeforeBegin => "before_begin",
        DirectBoundaryV1::AfterBegin => "after_begin",
        DirectBoundaryV1::AfterStateWrite => "after_state_write",
        DirectBoundaryV1::AfterCommit => "after_commit",
    }
}

fn parse_boundary(name: &str) -> DirectBoundaryV1 {
    match name {
        "before_begin" => DirectBoundaryV1::BeforeBegin,
        "after_begin" => DirectBoundaryV1::AfterBegin,
        "after_state_write" => DirectBoundaryV1::AfterStateWrite,
        "after_commit" => DirectBoundaryV1::AfterCommit,
        other => panic!("unknown commit boundary {other}"),
    }
}

fn maintenance_boundary_name(boundary: MaintenanceBoundaryV1) -> &'static str {
    match boundary {
        MaintenanceBoundaryV1::ExclusiveAcquired => "exclusive_acquired",
        MaintenanceBoundaryV1::MarkerPublished => "marker_published",
        MaintenanceBoundaryV1::StateUnlinked => "state_unlinked",
        MaintenanceBoundaryV1::StoreSeeded => "store_seeded",
        MaintenanceBoundaryV1::MarkerRemoved => "marker_removed",
        MaintenanceBoundaryV1::ContentsRemoved => "contents_removed",
        MaintenanceBoundaryV1::MarkerUnlinked => "marker_unlinked",
        MaintenanceBoundaryV1::DirectoriesRemoved => "directories_removed",
    }
}

const RESET_KILL_STEPS: [MaintenanceBoundaryV1; 4] = [
    MaintenanceBoundaryV1::MarkerPublished,
    MaintenanceBoundaryV1::StateUnlinked,
    MaintenanceBoundaryV1::StoreSeeded,
    MaintenanceBoundaryV1::MarkerRemoved,
];
const REMOVAL_KILL_STEPS: [MaintenanceBoundaryV1; 4] = [
    MaintenanceBoundaryV1::MarkerPublished,
    MaintenanceBoundaryV1::ContentsRemoved,
    MaintenanceBoundaryV1::MarkerUnlinked,
    MaintenanceBoundaryV1::DirectoriesRemoved,
];

fn outcome_value(outcome: &MaintenanceOutcomeV1) -> Value {
    json!({
        "acquisition_us": micros(outcome.acquisition),
        "replayed": outcome.replayed,
        "resumed": outcome.resumed,
        "retired_epoch": outcome.retired_epoch,
        "epoch": outcome.epoch,
    })
}

/// One re-executed child of this test binary and its line protocol.
struct ChildV1 {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl ChildV1 {
    fn spawn(spec: &Value) -> Self {
        let mut child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD_TEST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
                "--quiet",
            ])
            .env(CHILD_ENV, spec.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn message(&mut self) -> Value {
        loop {
            let line = self
                .lines
                .next()
                .expect("the gate child closed its output early")
                .unwrap();
            if let Some(message) = line.strip_prefix(PROTOCOL_PREFIX) {
                return serde_json::from_str(message).unwrap();
            }
        }
    }

    fn expect(&mut self, event: &str) -> Value {
        let message = self.message();
        assert_eq!(message["event"], event, "unexpected gate child message");
        message
    }

    /// Sends `SIGKILL` and reports whether it ended the child rather than finding it exited.
    fn kill(mut self) -> bool {
        self.child.kill().unwrap();
        self.child.wait().unwrap().signal() == Some(SIGKILL)
    }

    fn finish(mut self) -> Value {
        let message = self.expect("done");
        let status = self.child.wait().unwrap();
        assert!(status.success(), "a gate child failed: {message}");
        message
    }
}

fn announce(message: Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{PROTOCOL_PREFIX}{message}").unwrap();
    stdout.flush().unwrap();
}

fn wait_for_line(expected: &str) {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line.trim_end(), expected);
}

fn park_forever() -> ! {
    loop {
        thread::sleep(Duration::from_secs(3_600));
    }
}

fn spec_path(spec: &Value, field: &str) -> PathBuf {
    PathBuf::from(spec[field].as_str().unwrap())
}

#[test]
#[ignore = "child process of the V2DFC-001 feasibility gate"]
fn v2dfc001_child() {
    let Some(spec) = env::var_os(CHILD_ENV) else {
        return;
    };
    let spec: Value = serde_json::from_str(spec.to_str().unwrap()).unwrap();
    match spec["role"].as_str().unwrap() {
        "g1_writer" => g1_writer(&spec),
        "observer" => observer(&spec),
        "g3_controller" => g3_controller(&spec),
        "g4_writer" => g4_writer(&spec),
        "maintenance" => maintenance_child(&spec),
        "g5_contender" => g5_contender(&spec),
        "g9_contender" => g9_contender(&spec),
        role => panic!("unknown gate child role {role}"),
    }
}

fn g1_writer(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let binding = resolved_binding(layout.root());
    let writer = spec["writer"].as_u64().unwrap();
    let target = spec["successes"].as_u64().unwrap();
    let mut log = fs::File::create(spec_path(spec, "log")).unwrap();
    let mut random = RandomV1::new(writer + 1);
    announce(json!({"event": "ready"}));
    wait_for_line("go");
    let started = Instant::now();
    let mut counts = BTreeMap::<&str, u64>::new();
    let mut successes = 0;
    let mut attempts = 0;
    while successes < target {
        attempts += 1;
        let key = format!("g1-w{writer}-{attempts}");
        let deadline = Instant::now() + COMMAND_DEADLINE;
        let choice = random.below(100);
        let mut timing = None;
        let mut attempt =
            || -> Result<(DirectRequestV1, DirectOutcomeV1, String), PrototypeErrorV1> {
                let mut store = open_store_v1(&layout, &binding, OpenModeV1::Mutation, deadline)?;
                let state = read_current_state(&mut store)?;
                let tag = format!("w{writer}a{attempts}");
                let canonical = if choice < 78 {
                    canonical_set(
                        &state,
                        &item_id(random.below(ITEM_COUNT)),
                        payload(&tag, 64),
                    )
                } else if choice < 98 {
                    let first = random.below(ITEM_COUNT);
                    let items: Vec<(String, String)> = (0..4)
                        .map(|offset| {
                            (
                                item_id((first + offset) % ITEM_COUNT),
                                payload(&format!("{tag}o{offset}"), 64),
                            )
                        })
                        .collect();
                    canonical_record(&state, &items)
                } else {
                    canonical_complete(&state)
                };
                let request = direct_request(&key, &canonical_text(&canonical), Vec::new());
                let (outcome, measured) = timed_commit(&mut store, &request, deadline);
                timing = measured;
                Ok((request, outcome?, store.epoch().to_owned()))
            };
        match attempt() {
            Ok((
                request,
                DirectOutcomeV1::Committed {
                    response,
                    workspace_sequence,
                    ..
                },
                epoch,
            )) => {
                successes += 1;
                let (wait, hold) = timing.expect("a committed attempt began a transaction");
                append_jsonl(
                    &mut log,
                    &json!({
                        "kind": "success",
                        "key": key,
                        "epoch": epoch,
                        "workspace_sequence": workspace_sequence,
                        "wait_us": micros(wait),
                        "hold_us": micros(hold),
                    }),
                );
                let replay = open_store_v1(
                    &layout,
                    &binding,
                    OpenModeV1::Mutation,
                    Instant::now() + COMMAND_DEADLINE,
                )
                .and_then(|mut store| {
                    let (replay, measured) =
                        timed_commit(&mut store, &request, Instant::now() + COMMAND_DEADLINE);
                    log_transaction(&mut log, "replay", measured);
                    replay
                });
                match replay {
                    Ok(DirectOutcomeV1::Replayed {
                        response: replayed, ..
                    }) if replayed == response => *counts.entry("replayed").or_default() += 1,
                    Ok(_) => *counts.entry("replay_mismatch").or_default() += 1,
                    Err(PrototypeErrorV1::WorkspaceBusy) => {
                        *counts.entry("busy").or_default() += 1;
                    }
                    Err(_) => *counts.entry("replay_error").or_default() += 1,
                }
            }
            Ok((_, DirectOutcomeV1::Replayed { .. }, _)) => {
                *counts.entry("unexpected_replay").or_default() += 1;
            }
            Err(PrototypeErrorV1::Rejected { .. }) => {
                *counts.entry("rejected").or_default() += 1;
                log_transaction(&mut log, "rejected", timing);
            }
            Err(PrototypeErrorV1::WorkspaceBusy) => {
                *counts.entry("busy").or_default() += 1;
                log_transaction(&mut log, "busy", timing);
            }
            Err(error) => {
                *counts.entry("other").or_default() += 1;
                log_transaction(&mut log, "other", timing);
                append_jsonl(
                    &mut log,
                    &json!({"kind": "error", "code": error.code(), "detail": format!("{error:?}")}),
                );
            }
        }
    }
    announce(json!({
        "event": "done",
        "writer": writer,
        "successes": successes,
        "attempts": attempts,
        "counts": counts,
        "elapsed_us": micros(started.elapsed()),
    }));
}

/// Reads committed snapshots until the stop file appears and checks what every read decodes.
fn observer(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let binding = resolved_binding(layout.root());
    let stop = spec_path(spec, "stop");
    let mut counts = BTreeMap::<String, u64>::new();
    let mut last_revision = 0;
    announce(json!({"event": "ready"}));
    wait_for_line("go");
    while !stop.exists() {
        let deadline = Instant::now() + COMMAND_DEADLINE;
        let read = open_store_v1(&layout, &binding, OpenModeV1::ReadOnly, deadline)
            .and_then(|mut store| read_workspace_view_v1(&mut store));
        let class = match read {
            Ok(view) => match view.graph_state() {
                Some(state) if state.trace().revision().get() >= last_revision => {
                    last_revision = state.trace().revision().get();
                    "read".to_owned()
                }
                Some(_) => "revision_regressed".to_owned(),
                None => "session_missing".to_owned(),
            },
            Err(error) => error.code().to_owned(),
        };
        *counts.entry(class).or_default() += 1;
    }
    announce(json!({"event": "done", "counts": counts}));
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g1_g2_multi_process_correctness() {
    multi_process_run("G1-G2", "g1-g2", GATE_NODES, profile().g1_successes);
}

/// Repeats the G1 workload on a maximum-size Procedure; its timings carry no verdict.
#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g2_scale_information() {
    multi_process_run(
        "G2-scale",
        "g2-scale",
        SCALE_NODES,
        profile().scale_successes,
    );
}

fn multi_process_run(check: &str, name: &str, nodes: usize, successes: u64) {
    let profile = profile();
    let directory = output_directory(name);
    let fixture = gate_fixture(nodes, false);
    let stop = directory.join("observers.stop");
    let _ = fs::remove_file(&stop);
    let root = fixture.layout.root().display().to_string();
    let mut writers: Vec<ChildV1> = (0..profile.g1_writers)
        .map(|writer| {
            ChildV1::spawn(&json!({
                "role": "g1_writer",
                "root": root,
                "writer": writer,
                "successes": successes,
                "log": directory.join(format!("writer-{writer}.jsonl")).display().to_string(),
            }))
        })
        .collect();
    let mut observers: Vec<ChildV1> = (0..profile.g1_observers)
        .map(|_| {
            ChildV1::spawn(&json!({
                "role": "observer",
                "root": root,
                "stop": stop.display().to_string(),
            }))
        })
        .collect();
    for child in writers.iter_mut().chain(observers.iter_mut()) {
        child.expect("ready");
    }
    for child in writers.iter_mut().chain(observers.iter_mut()) {
        child.send("go");
    }
    let writer_reports: Vec<Value> = writers.into_iter().map(ChildV1::finish).collect();
    fs::write(&stop, b"stop").unwrap();
    let observer_reports: Vec<Value> = observers.into_iter().map(ChildV1::finish).collect();

    let mut acknowledged = BTreeSet::new();
    let mut samples = Vec::new();
    let mut timed = BTreeMap::<String, u64>::new();
    let mut writer_errors = 0;
    for writer in 0..profile.g1_writers {
        for line in read_jsonl(&directory.join(format!("writer-{writer}.jsonl"))) {
            let transaction = match line["kind"].as_str().unwrap() {
                "success" => {
                    acknowledged.insert(line["key"].as_str().unwrap().to_owned());
                    "commit"
                }
                "transaction" => line["outcome"].as_str().unwrap(),
                _ => {
                    writer_errors += 1;
                    continue;
                }
            };
            *timed.entry(transaction.to_owned()).or_default() += 1;
            samples.push(json!({
                "writer": writer,
                "transaction": transaction,
                "wait_us": line["wait_us"],
                "hold_us": line["hold_us"],
            }));
        }
    }
    let count = |reports: &[Value], name: &str| -> u64 {
        reports
            .iter()
            .map(|report| report["counts"][name].as_u64().unwrap_or(0))
            .sum()
    };
    let busy = count(&writer_reports, "busy");
    let other = count(&writer_reports, "other") + count(&writer_reports, "unexpected_replay");
    let replay_mismatch =
        count(&writer_reports, "replay_mismatch") + count(&writer_reports, "replay_error");
    let observer_reads = count(&observer_reports, "read");
    let observer_violations: u64 = observer_reports
        .iter()
        .flat_map(|report| report["counts"].as_object().unwrap().iter())
        .filter(|(class, _)| class.as_str() != "read")
        .map(|(_, value)| value.as_u64().unwrap())
        .sum();

    let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
    let outcomes = list_outcomes_v1(&mut store).unwrap();
    let final_state = read_current_state(&mut store).unwrap();
    drop(store);
    let recorded: BTreeSet<String> = outcomes
        .iter()
        .map(|outcome| outcome.idempotency_key.clone())
        .collect();
    let lost = acknowledged.difference(&recorded).count();
    let unacknowledged = recorded.difference(&acknowledged).count();
    let duplicated = outcomes.len() - recorded.len();
    let mut replayed = fixture.initial.clone();
    let mut stale_fence_commits = 0;
    let mut revision_mismatches = 0;
    for outcome in &outcomes {
        let request = direct_request(
            &outcome.idempotency_key,
            &outcome.canonical_request,
            Vec::new(),
        );
        match apply_direct_mutation_v1(&replayed, &request, outcome.recorded_at) {
            Ok(next) => {
                if next.trace().revision().get() != outcome.session_revision_after {
                    revision_mismatches += 1;
                }
                replayed = next;
            }
            Err(_) => stale_fence_commits += 1,
        }
    }
    let replay_matches = replayed == final_state;
    let writer_elapsed: Vec<u64> = writer_reports
        .iter()
        .map(|report| report["elapsed_us"].as_u64().unwrap())
        .collect();
    write_samples(&directory, &samples);
    write_summary(
        &directory,
        check,
        profile,
        json!({
            "procedure_nodes": nodes,
            "writers": profile.g1_writers,
            "successes_per_writer": successes,
            "observers": profile.g1_observers,
            "acknowledged": acknowledged.len(),
            "recorded": recorded.len(),
            "lost": lost,
            "unacknowledged": unacknowledged,
            "duplicated": duplicated,
            "stale_fence_commits": stale_fence_commits,
            "revision_mismatches": revision_mismatches,
            "replay_matches_final_state": replay_matches,
            "busy_reaching_caller": busy,
            "other_errors": other + writer_errors,
            "same_key_replay_mismatches": replay_mismatch,
            "fence_rejections": count(&writer_reports, "rejected"),
            "timed_transactions": timed,
            "observer_reads": observer_reads,
            "observer_violations": observer_violations,
            "writer_elapsed_us": writer_elapsed,
            "writer_reports": writer_reports,
            "observer_reports": observer_reports,
        }),
    );
    assert_eq!(
        lost + unacknowledged + duplicated,
        0,
        "G1 lost or duplicated effects"
    );
    assert_eq!(
        stale_fence_commits + revision_mismatches,
        0,
        "G1 stale-fence commits"
    );
    assert!(
        replay_matches,
        "G1 serial replay must equal the final state"
    );
    assert_eq!(busy, 0, "G1 busy errors reached a caller");
    assert_eq!(
        other + writer_errors + replay_mismatch,
        0,
        "G1 unexpected errors"
    );
    assert_eq!(
        observer_violations, 0,
        "G1 observers decoded an invariant violation"
    );
}

fn g3_controller(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let binding = resolved_binding(layout.root());
    let mut store = open_store_v1(
        &layout,
        &binding,
        OpenModeV1::Mutation,
        Instant::now() + MAINTENANCE_DEADLINE,
    )
    .unwrap();
    announce(json!({"event": "ready"}));
    loop {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        match line.trim_end() {
            "hold" => {
                hold_write_lock_v1(
                    &mut store,
                    Instant::now() + MAINTENANCE_DEADLINE,
                    &mut || {
                        announce(json!({"event": "held"}));
                        wait_for_line("release");
                    },
                )
                .unwrap();
                announce(json!({"event": "released"}));
            }
            "exit" => break,
            other => panic!("unknown controller command {other}"),
        }
    }
    announce(json!({"event": "done"}));
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g3_over_deadline() {
    let profile = profile();
    let directory = output_directory("g3");
    let fixture = gate_fixture(8, false);
    let mut controller = ChildV1::spawn(&json!({
        "role": "g3_controller",
        "root": fixture.layout.root().display().to_string(),
    }));
    controller.expect("ready");
    let state = fixture.initial.clone();
    let mut samples = Vec::new();
    let mut violations = 0;
    for trial in 0..profile.g3_warmup + profile.g3_measured {
        controller.send("hold");
        controller.expect("held");
        let key = format!("g3-{trial}");
        let canonical = canonical_text(&canonical_set(&state, "i01", payload(&key, 64)));
        let request = direct_request(&key, &canonical, Vec::new());
        let started = Instant::now();
        let deadline = started + G3_DEADLINE;
        let result = open_store_v1(
            &fixture.layout,
            &fixture.binding,
            OpenModeV1::Mutation,
            deadline,
        )
        .and_then(|mut store| commit(&mut store, &request, deadline));
        let elapsed = started.elapsed();
        let code = match &result {
            Ok(_) => "COMMITTED",
            Err(error) => error.code(),
        };
        let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::ReadOnly);
        let recorded = lookup_outcome_v1(&mut store, &key).unwrap().is_some();
        let revision = read_current_state(&mut store).unwrap().trace().revision();
        drop(store);
        controller.send("release");
        controller.expect("released");
        let unchanged = revision == state.trace().revision();
        if code != "WORKSPACE_BUSY" || recorded || !unchanged {
            violations += 1;
        }
        samples.push(json!({
            "trial": trial,
            "warmup": trial < profile.g3_warmup,
            "code": code,
            "elapsed_us": micros(elapsed),
            "outcome_recorded": recorded,
            "state_unchanged": unchanged,
        }));
    }
    controller.send("exit");
    controller.finish();
    write_samples(&directory, &samples);
    write_summary(
        &directory,
        "G3",
        profile,
        json!({
            "deadline_ms": G3_DEADLINE.as_millis(),
            "warmup": profile.g3_warmup,
            "measured": profile.g3_measured,
            "violations": violations,
        }),
    );
    assert_eq!(violations, 0, "G3 over-deadline attempts must fail cleanly");
}

fn g4_writer(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let binding = resolved_binding(layout.root());
    let request = direct_request(
        spec["key"].as_str().unwrap(),
        spec["canonical"].as_str().unwrap(),
        Vec::new(),
    );
    let pause = spec["pause_at"].as_str().map(parse_boundary);
    let deadline = Instant::now() + COMMAND_DEADLINE;
    let mut store = open_store_v1(&layout, &binding, OpenModeV1::Mutation, deadline).unwrap();
    if spec["announce_start"].as_bool() == Some(true) {
        announce(json!({"event": "starting"}));
    }
    let result = commit_direct_mutation_v1(
        &mut store,
        &request,
        deadline,
        &mut |boundary| {
            if Some(boundary) == pause {
                announce(json!({"event": "paused", "boundary": boundary_name(boundary)}));
                park_forever();
            }
        },
        &mut compact_response,
    );
    announce(json!({
        "event": "done",
        "code": result.as_ref().map_or_else(|error| error.code(), |_| "COMMITTED"),
    }));
}

fn maintenance_child(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let seed = StoreSeedV1 {
        database: spec_path(spec, "seed_database"),
        podway_tree: spec_path(spec, "seed_tree"),
    };
    let request = maintenance_request(spec["key"].as_str().unwrap());
    let pause = spec["pause_at"].as_str();
    let retire_log = spec_path(spec, "retire_log");
    let operation = spec["operation"].as_str().unwrap();
    let started = Instant::now();
    let mut hook = |boundary: MaintenanceBoundaryV1| {
        if boundary == MaintenanceBoundaryV1::ExclusiveAcquired {
            announce(json!({"event": "acquired", "acquisition_us": micros(started.elapsed())}));
        }
        if pause == Some(maintenance_boundary_name(boundary)) {
            announce(json!({"event": "paused", "boundary": maintenance_boundary_name(boundary)}));
            park_forever();
        }
    };
    let mut retire = |generation: &RetiredGenerationV1| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&retire_log)
            .unwrap();
        append_jsonl(
            &mut file,
            &json!({
                "operation": operation,
                "key": request.idempotency_key,
                "epoch": generation.epoch,
                "keys": generation.outcome_keys,
            }),
        );
        file.sync_all().unwrap();
    };
    let deadline = Instant::now() + MAINTENANCE_DEADLINE;
    let result = match operation {
        "reset" => reset_all_v1(&layout, &seed, &request, deadline, &mut hook, &mut retire),
        "remove" => remove_workspace_v1(&layout, &request, deadline, &mut hook, &mut retire),
        other => panic!("unknown maintenance operation {other}"),
    };
    match result {
        Ok(outcome) => announce(json!({"event": "done", "outcome": outcome_value(&outcome)})),
        Err(error) => announce(json!({"event": "done", "error": error.code()})),
    }
}

fn spawn_maintenance(
    fixture_root: &str,
    seed: &StoreSeedV1,
    retire_log: &Path,
    operation: &str,
    key: &str,
    pause_at: Option<MaintenanceBoundaryV1>,
) -> ChildV1 {
    ChildV1::spawn(&json!({
        "role": "maintenance",
        "root": fixture_root,
        "seed_database": seed.database.display().to_string(),
        "seed_tree": seed.podway_tree.display().to_string(),
        "retire_log": retire_log.display().to_string(),
        "operation": operation,
        "key": key,
        "pause_at": pause_at.map(maintenance_boundary_name),
    }))
}

/// Runs one child commit, kills it at a boundary or after a delay, and checks the key.
fn g4_mutation_trial(
    fixture: &GateFixtureV1,
    trial: u64,
    boundary: Option<DirectBoundaryV1>,
    delay: Option<Duration>,
) -> Value {
    let before = read_state(&fixture.layout, &fixture.binding);
    let key = format!("g4-{trial}");
    let canonical = canonical_text(&canonical_set(&before, "i01", payload(&key, 256)));
    let mut child = ChildV1::spawn(&json!({
        "role": "g4_writer",
        "root": fixture.layout.root().display().to_string(),
        "key": key,
        "canonical": canonical,
        "pause_at": boundary.map(boundary_name),
        "announce_start": delay.is_some(),
    }));
    match (boundary, delay) {
        (Some(_), _) => {
            child.expect("paused");
        }
        (None, Some(delay)) => {
            child.expect("starting");
            thread::sleep(delay);
        }
        (None, None) => unreachable!("a G4 trial needs a boundary or a delay"),
    }
    let killed = child.kill();
    let revision_before = before.trace().revision().get();
    let request = direct_request(&key, &canonical, Vec::new());
    let open_ok = open_store_v1(
        &fixture.layout,
        &fixture.binding,
        OpenModeV1::Mutation,
        Instant::now() + COMMAND_DEADLINE,
    )
    .is_ok();
    let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
    let recorded = lookup_outcome_v1(&mut store, &key).unwrap();
    let revision_after_kill = read_current_state(&mut store)
        .unwrap()
        .trace()
        .revision()
        .get();
    let deadline = Instant::now() + COMMAND_DEADLINE;
    let (effect_ok, retry_ok) = match &recorded {
        Some(outcome) => {
            let effect = revision_after_kill == revision_before + 1
                && outcome.session_revision_after == revision_after_kill;
            let replay = matches!(
                commit(&mut store, &request, deadline),
                Ok(DirectOutcomeV1::Replayed { ref response, .. }) if *response == outcome.response
            );
            let unchanged = read_current_state(&mut store)
                .unwrap()
                .trace()
                .revision()
                .get()
                == revision_after_kill;
            (effect, replay && unchanged)
        }
        None => {
            let none = revision_after_kill == revision_before;
            let applied = matches!(
                commit(&mut store, &request, deadline),
                Ok(DirectOutcomeV1::Committed { .. })
            );
            let once = read_current_state(&mut store)
                .unwrap()
                .trace()
                .revision()
                .get()
                == revision_before + 1;
            let replay = matches!(
                commit(&mut store, &request, deadline),
                Ok(DirectOutcomeV1::Replayed { .. })
            );
            (none, applied && once && replay)
        }
    };
    let expected_recorded = boundary.map(|boundary| boundary == DirectBoundaryV1::AfterCommit);
    let boundary_ok =
        expected_recorded.is_none_or(|expected| killed && expected == recorded.is_some());
    json!({
        "trial": trial,
        "kind": if boundary.is_some() { "boundary" } else { "random" },
        "boundary": boundary.map(boundary_name),
        "delay_us": delay.map(micros),
        "killed": killed,
        "open_after_kill": open_ok,
        "recorded": recorded.is_some(),
        "effect_ok": effect_ok,
        "retry_ok": retry_ok,
        "boundary_expectation_ok": boundary_ok,
        "ok": open_ok && effect_ok && retry_ok && boundary_ok,
    })
}

/// Kills a reset at one marker step, then proves the exact retry completes it exactly once.
fn g4_maintenance_trial(
    fixture: &GateFixtureV1,
    seed: &StoreSeedV1,
    retire_log: &Path,
    trial: u64,
    boundary: MaintenanceBoundaryV1,
) -> Value {
    let before_epoch = open(&fixture.layout, &fixture.binding, OpenModeV1::ReadOnly)
        .epoch()
        .to_owned();
    let key = format!("g4-reset-{trial}");
    let root = fixture.layout.root().display().to_string();
    let mut child = spawn_maintenance(&root, seed, retire_log, "reset", &key, Some(boundary));
    loop {
        let message = child.message();
        if message["event"] == "paused" {
            break;
        }
        assert_eq!(message["event"], "acquired");
    }
    let killed = child.kill();
    let open_after_kill = match open_store_v1(
        &fixture.layout,
        &fixture.binding,
        OpenModeV1::Mutation,
        Instant::now() + COMMAND_DEADLINE,
    ) {
        Ok(_) => "OPENED",
        Err(error) => error.code(),
    };
    let request = maintenance_request(&key);
    let retry = reset_all_v1(
        &fixture.layout,
        seed,
        &request,
        Instant::now() + MAINTENANCE_DEADLINE,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    let again = reset_all_v1(
        &fixture.layout,
        seed,
        &request,
        Instant::now() + MAINTENANCE_DEADLINE,
        &mut |_| {},
        &mut |_| {},
    )
    .unwrap();
    let after = open_store_v1(
        &fixture.layout,
        &fixture.binding,
        OpenModeV1::Mutation,
        Instant::now() + COMMAND_DEADLINE,
    );
    let after_epoch = after.as_ref().map(|store| store.epoch().to_owned()).ok();
    let exactly_once = after_epoch
        .as_deref()
        .is_some_and(|epoch| epoch != before_epoch)
        && again.replayed
        && again.epoch == after_epoch
        && retry.epoch == after_epoch;
    json!({
        "trial": trial,
        "kind": "maintenance",
        "boundary": maintenance_boundary_name(boundary),
        "killed": killed,
        "open_after_kill": open_after_kill,
        "retry": outcome_value(&retry),
        "again": outcome_value(&again),
        "open_after_retry": after.is_ok(),
        "exactly_once": exactly_once,
        "ok": killed
            && after.is_ok()
            && exactly_once
            && matches!(open_after_kill, "OPENED" | "WORKSPACE_MAINTENANCE"),
    })
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g4_crash_boundaries() {
    let profile = profile();
    let directory = output_directory("g4");
    let fixture = gate_fixture(8, false);
    let seed = prepare_seed(&fixture, &directory);
    let retire_log = directory.join("retired.jsonl");
    let mut records = Vec::new();
    let mut trial = 0;
    for boundary in [
        DirectBoundaryV1::BeforeBegin,
        DirectBoundaryV1::AfterBegin,
        DirectBoundaryV1::AfterStateWrite,
        DirectBoundaryV1::AfterCommit,
    ] {
        for _ in 0..profile.g4_per_boundary {
            trial += 1;
            records.push(g4_mutation_trial(&fixture, trial, Some(boundary), None));
        }
    }
    let mut durations = Vec::new();
    for _ in 0..profile.g4_calibration {
        trial += 1;
        let before = read_state(&fixture.layout, &fixture.binding);
        let key = format!("g4-calibration-{trial}");
        let canonical = canonical_text(&canonical_set(&before, "i01", payload(&key, 256)));
        let mut child = ChildV1::spawn(&json!({
            "role": "g4_writer",
            "root": fixture.layout.root().display().to_string(),
            "key": key,
            "canonical": canonical,
            "pause_at": null,
            "announce_start": true,
        }));
        child.expect("starting");
        let started = Instant::now();
        let done = child.finish();
        assert_eq!(done["code"], "COMMITTED");
        durations.push(started.elapsed());
    }
    durations.sort();
    // Delays span the median writer lifetime; a kill that finds the writer already exited is
    // kept as a trial but does not count, so the check reaches its quota of real kills.
    let window = durations[durations.len() / 2];
    let mut random = RandomV1::new(0x0004);
    let mut random_kills = 0;
    let mut random_attempts = 0;
    while random_kills < profile.g4_random {
        assert!(
            random_attempts < profile.g4_random * 10,
            "G4 random kills keep missing the writer"
        );
        random_attempts += 1;
        trial += 1;
        let delay =
            Duration::from_nanos(random.below(u64::try_from(window.as_nanos()).unwrap() + 1));
        let record = g4_mutation_trial(&fixture, trial, None, Some(delay));
        if record["killed"] == true {
            random_kills += 1;
        }
        records.push(record);
    }
    let kills_before_commit = records
        .iter()
        .filter(|record| {
            record["kind"] == "random" && record["killed"] == true && record["recorded"] == false
        })
        .count();
    let maintenance_steps = [
        MaintenanceBoundaryV1::ExclusiveAcquired,
        MaintenanceBoundaryV1::MarkerPublished,
        MaintenanceBoundaryV1::StateUnlinked,
        MaintenanceBoundaryV1::StoreSeeded,
        MaintenanceBoundaryV1::MarkerRemoved,
    ];
    for index in 0..profile.g4_per_boundary {
        trial += 1;
        let boundary = maintenance_steps[usize::try_from(index).unwrap() % maintenance_steps.len()];
        records.push(g4_maintenance_trial(
            &fixture,
            &seed,
            &retire_log,
            trial,
            boundary,
        ));
    }
    let integrity = integrity_check_v1(&mut open(
        &fixture.layout,
        &fixture.binding,
        OpenModeV1::Mutation,
    ))
    .unwrap();
    let failures = records.iter().filter(|record| record["ok"] != true).count();
    write_samples(&directory, &records);
    write_summary(
        &directory,
        "G4",
        profile,
        json!({
            "boundary_trials_per_boundary": profile.g4_per_boundary,
            "random_kills": random_kills,
            "random_kills_before_commit": kills_before_commit,
            "random_attempts": random_attempts,
            "random_window_us": micros(window),
            "maintenance_trials": profile.g4_per_boundary,
            "trials": records.len(),
            "failures": failures,
            "integrity_check": integrity,
        }),
    );
    assert_eq!(failures, 0, "G4 crash trials must recover exactly once");
    assert_eq!(integrity, "ok", "G4 final integrity_check must pass");
}

fn g5_contender(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let binding = resolved_binding(layout.root());
    let writer = spec["writer"].as_u64().unwrap();
    let reader = spec["reader"].as_bool() == Some(true);
    let stop = spec_path(spec, "stop");
    let mut log = fs::File::create(spec_path(spec, "log")).unwrap();
    let mut counts = BTreeMap::<String, u64>::new();
    let mut attempts = 0;
    announce(json!({"event": "ready"}));
    wait_for_line("go");
    while !stop.exists() {
        attempts += 1;
        let deadline = Instant::now() + COMMAND_DEADLINE;
        let class = if reader {
            match open_store_v1(&layout, &binding, OpenModeV1::ReadOnly, deadline)
                .and_then(|mut store| read_current_state(&mut store))
            {
                Ok(_) => "read".to_owned(),
                Err(error) => error.code().to_owned(),
            }
        } else {
            let key = format!("g5-w{writer}-{attempts}");
            let attempt = || -> Result<String, PrototypeErrorV1> {
                let mut store = open_store_v1(&layout, &binding, OpenModeV1::Mutation, deadline)?;
                let state = read_current_state(&mut store)?;
                let canonical = canonical_text(&canonical_set(
                    &state,
                    &item_id(writer % ITEM_COUNT),
                    payload(&key, 64),
                ));
                let request = direct_request(&key, &canonical, Vec::new());
                match commit(&mut store, &request, deadline)? {
                    DirectOutcomeV1::Committed { .. } => Ok(store.epoch().to_owned()),
                    DirectOutcomeV1::Replayed { .. } => Err(PrototypeErrorV1::Storage {
                        reason: "fresh key replayed".to_owned(),
                    }),
                }
            };
            match attempt() {
                Ok(epoch) => {
                    append_jsonl(&mut log, &json!({"key": key, "epoch": epoch}));
                    "success".to_owned()
                }
                Err(error) => error.code().to_owned(),
            }
        };
        if !matches!(class.as_str(), "success" | "read") {
            thread::sleep(CONTENDER_RETRY_PAUSE);
        }
        *counts.entry(class).or_default() += 1;
    }
    announce(json!({"event": "done", "counts": counts}));
}

/// Runs one maintenance operation, killing it once at each step, then replays it.
#[allow(clippy::too_many_arguments)]
fn g5_operation(
    root: &str,
    seed: &StoreSeedV1,
    retire_log: &Path,
    operation: &str,
    key: &str,
    steps: &[MaintenanceBoundaryV1],
    acquisitions: &mut Vec<Value>,
    attempts: &mut Vec<Value>,
) -> Value {
    for step in steps {
        let mut child = spawn_maintenance(root, seed, retire_log, operation, key, Some(*step));
        loop {
            let message = child.message();
            match message["event"].as_str().unwrap() {
                "acquired" => acquisitions.push(json!({
                    "operation": operation,
                    "acquisition_us": message["acquisition_us"],
                })),
                "paused" => {
                    let killed = child.kill();
                    attempts.push(json!({"key": key, "step": maintenance_boundary_name(*step), "killed": killed}));
                    break;
                }
                "done" => {
                    child.child.wait().unwrap();
                    attempts.push(json!({"key": key, "step": maintenance_boundary_name(*step), "killed": false, "result": message}));
                    break;
                }
                event => panic!("unexpected maintenance event {event}"),
            }
        }
    }
    let mut child = spawn_maintenance(root, seed, retire_log, operation, key, None);
    loop {
        let message = child.message();
        match message["event"].as_str().unwrap() {
            "acquired" => acquisitions.push(json!({
                "operation": operation,
                "acquisition_us": message["acquisition_us"],
            })),
            "done" => {
                assert!(child.child.wait().unwrap().success());
                return message;
            }
            event => panic!("unexpected maintenance event {event}"),
        }
    }
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g5_maintenance_exclusion() {
    let profile = profile();
    let directory = output_directory("g5");
    let fixture = gate_fixture(GATE_NODES, false);
    let seed = prepare_seed(&fixture, &directory);
    let retire_log = directory.join("retired.jsonl");
    let _ = fs::remove_file(&retire_log);
    let stop = directory.join("contenders.stop");
    let _ = fs::remove_file(&stop);
    let root = fixture.layout.root().display().to_string();
    let initial_epoch = open(&fixture.layout, &fixture.binding, OpenModeV1::ReadOnly)
        .epoch()
        .to_owned();
    let mut contenders: Vec<ChildV1> = (0..profile.g5_writers + profile.g5_readers)
        .map(|index| {
            ChildV1::spawn(&json!({
                "role": "g5_contender",
                "root": root,
                "writer": index,
                "reader": index >= profile.g5_writers,
                "stop": stop.display().to_string(),
                "log": directory.join(format!("contender-{index}.jsonl")).display().to_string(),
            }))
        })
        .collect();
    for child in &mut contenders {
        child.expect("ready");
    }
    for child in &mut contenders {
        child.send("go");
    }
    let mut acquisitions = Vec::new();
    let mut attempts = Vec::new();
    let mut operations = Vec::new();
    for cycle in 0..profile.g5_cycles {
        for (operation, steps) in [("reset", RESET_KILL_STEPS), ("remove", REMOVAL_KILL_STEPS)] {
            let key = format!("g5-{operation}-{cycle}");
            let last = g5_operation(
                &root,
                &seed,
                &retire_log,
                operation,
                &key,
                &steps,
                &mut acquisitions,
                &mut attempts,
            );
            operations.push(json!({"operation": operation, "key": key, "final": last}));
        }
        let key = format!("g5-create-{cycle}");
        let created = create_workspace_v1(
            &fixture.layout,
            &seed,
            &maintenance_request(&key),
            Instant::now() + MAINTENANCE_DEADLINE,
        )
        .unwrap();
        acquisitions
            .push(json!({"operation": "create", "acquisition_us": micros(created.acquisition)}));
        operations.push(json!({"operation": "create", "key": key, "final": {"outcome": outcome_value(&created)}}));
    }
    fs::write(&stop, b"stop").unwrap();
    let reports: Vec<Value> = contenders.into_iter().map(ChildV1::finish).collect();

    let retired = read_jsonl(&retire_log);
    let mut retired_keys = BTreeMap::<String, BTreeSet<String>>::new();
    let mut retirements_by_operation = BTreeMap::<String, BTreeSet<String>>::new();
    for record in &retired {
        let epoch = record["epoch"].as_str().unwrap().to_owned();
        let keys: BTreeSet<String> = record["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap().to_owned())
            .collect();
        retired_keys.insert(epoch.clone(), keys);
        retirements_by_operation
            .entry(record["key"].as_str().unwrap().to_owned())
            .or_default()
            .insert(epoch);
    }
    let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
    let final_epoch = store.epoch().to_owned();
    let final_keys: BTreeSet<String> = list_outcomes_v1(&mut store)
        .unwrap()
        .into_iter()
        .map(|outcome| outcome.idempotency_key)
        .collect();
    drop(store);

    let mut lost_commits = 0;
    let mut unknown_epochs = 0;
    let mut acknowledged = 0;
    for index in 0..profile.g5_writers {
        for line in read_jsonl(&directory.join(format!("contender-{index}.jsonl"))) {
            acknowledged += 1;
            let key = line["key"].as_str().unwrap();
            let epoch = line["epoch"].as_str().unwrap();
            let present = if epoch == final_epoch {
                final_keys.contains(key)
            } else if let Some(keys) = retired_keys.get(epoch) {
                keys.contains(key)
            } else {
                unknown_epochs += 1;
                false
            };
            if !present {
                lost_commits += 1;
            }
        }
    }

    let mut chain_violations = 0;
    let mut live = Some(initial_epoch.clone());
    for operation in &operations {
        let key = operation["key"].as_str().unwrap();
        let kind = operation["operation"].as_str().unwrap();
        let outcome = &operation["final"]["outcome"];
        let retirements = retirements_by_operation
            .get(key)
            .cloned()
            .unwrap_or_default();
        match kind {
            "reset" | "remove" => {
                let expected: BTreeSet<String> = live.iter().cloned().collect();
                if retirements != expected || outcome["replayed"] != true {
                    chain_violations += 1;
                }
                live = outcome["epoch"].as_str().map(str::to_owned);
                if kind == "reset" && live.as_deref() == expected.iter().next().map(String::as_str)
                {
                    chain_violations += 1;
                }
            }
            _ => live = outcome["epoch"].as_str().map(str::to_owned),
        }
    }
    if live.as_deref() != Some(final_epoch.as_str()) {
        chain_violations += 1;
    }
    let mut classes = BTreeMap::<String, u64>::new();
    for report in &reports {
        for (class, value) in report["counts"].as_object().unwrap() {
            *classes.entry(class.clone()).or_default() += value.as_u64().unwrap();
        }
    }
    let permitted = [
        "success",
        "read",
        "WORKSPACE_MAINTENANCE",
        "WORKSPACE_NOT_INITIALIZED",
        "PRECONDITION_FAILED",
    ];
    let unexpected_outcomes: u64 = classes
        .iter()
        .filter(|(class, _)| !permitted.contains(&class.as_str()))
        .map(|(_, count)| *count)
        .sum();
    write_samples(&directory, &acquisitions);
    write_summary(
        &directory,
        "G5",
        profile,
        json!({
            "cycles": profile.g5_cycles,
            "writers": profile.g5_writers,
            "readers": profile.g5_readers,
            "maintenance_attempts": attempts.len() + operations.len(),
            "killed_attempts": attempts.iter().filter(|attempt| attempt["killed"] == true).count(),
            "acknowledged_commits": acknowledged,
            "lost_commits": lost_commits,
            "unknown_epochs": unknown_epochs,
            "chain_violations": chain_violations,
            "contender_outcomes": classes,
            "unexpected_contender_outcomes": unexpected_outcomes,
            "operations": operations.len(),
        }),
    );
    let mut attempts_file = fs::File::create(directory.join("attempts.jsonl")).unwrap();
    for attempt in attempts.iter().chain(operations.iter()) {
        append_jsonl(&mut attempts_file, attempt);
    }
    assert_eq!(
        lost_commits, 0,
        "G5 acknowledged commits must survive maintenance"
    );
    assert_eq!(
        chain_violations, 0,
        "G5 interrupted maintenance must complete exactly once"
    );
    assert_eq!(
        unexpected_outcomes, 0,
        "G5 contenders saw an unexpected outcome"
    );
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g6_preparation_races() {
    let profile = profile();
    let directory = output_directory("g6");
    let fixture = gate_fixture(3, true);
    let artifact = fixture.layout.root().join(ARTIFACT_PATH);
    let mut samples = Vec::new();
    let mut committed = 0;
    for trial in 0..profile.g6_state {
        let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
        let state = read_current_state(&mut store).unwrap();
        let key = format!("g6-state-{trial}");
        let (kind, canonical) = match trial % 3 {
            0 => ("set", canonical_set(&state, "i01", payload(&key, 64))),
            1 => (
                "record",
                canonical_record(&state, &[("i02".to_owned(), payload(&key, 64))]),
            ),
            _ => ("complete", canonical_complete(&state)),
        };
        let competing_item = if kind == "set" { "i01" } else { "i16" };
        let prepared = if kind == "complete" {
            prepare_local_artifacts_v1(&fixture.layout, &state).unwrap()
        } else {
            Vec::new()
        };
        let request = direct_request(&key, &canonical_text(&canonical), prepared);
        let mut race = |boundary: DirectBoundaryV1| {
            if boundary != DirectBoundaryV1::BeforeBegin {
                return;
            }
            let mut other = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
            let current = read_current_state(&mut other).unwrap();
            let competing_key = format!("{key}-competing");
            let competing = direct_request(
                &competing_key,
                &canonical_text(&canonical_set(
                    &current,
                    competing_item,
                    payload(&competing_key, 64),
                )),
                Vec::new(),
            );
            assert!(matches!(
                commit(&mut other, &competing, Instant::now() + COMMAND_DEADLINE),
                Ok(DirectOutcomeV1::Committed { .. })
            ));
        };
        let result = commit_direct_mutation_v1(
            &mut store,
            &request,
            Instant::now() + COMMAND_DEADLINE,
            &mut race,
            &mut compact_response,
        );
        let recorded = lookup_outcome_v1(&mut store, &key).unwrap().is_some();
        if result.is_ok() || recorded {
            committed += 1;
        }
        samples.push(json!({
            "trial": trial,
            "race": "state",
            "kind": kind,
            "code": result.as_ref().map_or_else(|error| error.code(), |_| "COMMITTED"),
            "outcome_recorded": recorded,
        }));
    }
    for trial in 0..profile.g6_artifact {
        fs::write(&artifact, ARTIFACT_ORIGINAL).unwrap();
        let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
        let state = read_current_state(&mut store).unwrap();
        let prepared = prepare_local_artifacts_v1(&fixture.layout, &state).unwrap();
        assert_eq!(
            prepared.len(),
            1,
            "the G6 node must hold one attached artifact"
        );
        let key = format!("g6-artifact-{trial}");
        let request = direct_request(&key, &canonical_text(&canonical_complete(&state)), prepared);
        let revision_before = state.trace().revision();
        let result = commit_direct_mutation_v1(
            &mut store,
            &request,
            Instant::now() + COMMAND_DEADLINE,
            &mut |boundary| {
                if boundary == DirectBoundaryV1::BeforeBegin {
                    fs::write(&artifact, ARTIFACT_CHANGED).unwrap();
                }
            },
            &mut compact_response,
        );
        let recorded = lookup_outcome_v1(&mut store, &key).unwrap().is_some();
        let unchanged =
            read_current_state(&mut store).unwrap().trace().revision() == revision_before;
        if result.is_ok() || recorded || !unchanged {
            committed += 1;
        }
        samples.push(json!({
            "trial": trial,
            "race": "artifact",
            "kind": "complete",
            "code": result.as_ref().map_or_else(|error| error.code(), |_| "COMMITTED"),
            "outcome_recorded": recorded,
            "state_unchanged": unchanged,
        }));
    }
    fs::write(&artifact, ARTIFACT_ORIGINAL).unwrap();
    let mut store = open(&fixture.layout, &fixture.binding, OpenModeV1::Mutation);
    let state = read_current_state(&mut store).unwrap();
    let control_set = direct_request(
        "g6-control-set",
        &canonical_text(&canonical_set(&state, "i01", payload("control", 64))),
        Vec::new(),
    );
    let control_set_committed = matches!(
        commit(&mut store, &control_set, Instant::now() + COMMAND_DEADLINE),
        Ok(DirectOutcomeV1::Committed { .. })
    );
    let state = read_current_state(&mut store).unwrap();
    let prepared = prepare_local_artifacts_v1(&fixture.layout, &state).unwrap();
    let control_complete = direct_request(
        "g6-control-complete",
        &canonical_text(&canonical_complete(&state)),
        prepared,
    );
    let control_complete_committed = matches!(
        commit(
            &mut store,
            &control_complete,
            Instant::now() + COMMAND_DEADLINE
        ),
        Ok(DirectOutcomeV1::Committed { .. })
    );
    write_samples(&directory, &samples);
    write_summary(
        &directory,
        "G6",
        profile,
        json!({
            "state_trials": profile.g6_state,
            "artifact_trials": profile.g6_artifact,
            "rejected": samples.len() - committed,
            "stale_commits": committed,
            "control_set_committed": control_set_committed,
            "control_complete_committed": control_complete_committed,
        }),
    );
    assert_eq!(committed, 0, "G6 races must all be rejected");
    assert!(
        control_set_committed && control_complete_committed,
        "G6 controls must commit"
    );
}

fn g9_contender(spec: &Value) {
    let layout = WorkspaceLayoutV1::new(spec_path(spec, "root"));
    let binding = resolved_binding(layout.root());
    let writer = spec["writer"].as_u64().unwrap();
    let reader = spec["reader"].as_bool() == Some(true);
    let stop = spec_path(spec, "stop");
    let mut random = RandomV1::new(0x0900 + writer);
    let mut counts = BTreeMap::<String, u64>::new();
    let mut attempts = 0;
    let mut completes = 0;
    let mut complete_due = false;
    announce(json!({"event": "ready"}));
    wait_for_line("go");
    while !stop.exists() {
        attempts += 1;
        let deadline = Instant::now() + COMMAND_DEADLINE;
        let class = if reader {
            match open_store_v1(&layout, &binding, OpenModeV1::ReadOnly, deadline)
                .and_then(|mut store| read_current_state(&mut store))
            {
                Ok(_) => "read".to_owned(),
                Err(error) => error.code().to_owned(),
            }
        } else {
            let key = format!("g9-w{writer}-{attempts}");
            let choice = random.below(100);
            let first = random.below(ITEM_COUNT);
            // A due complete is retried until it lands, because concurrent writers keep moving
            // its session-revision fence.
            complete_due |=
                attempts % G9_COMPLETE_PERIOD == 0 && completes < G9_COMPLETES_PER_WRITER;
            let complete = complete_due;
            let attempt = || -> Result<(), PrototypeErrorV1> {
                let mut store = open_store_v1(&layout, &binding, OpenModeV1::Mutation, deadline)?;
                let state = read_current_state(&mut store)?;
                let canonical = if complete {
                    canonical_complete(&state)
                } else if choice < 85 {
                    canonical_set(&state, &item_id(first), payload(&key, 4_096))
                } else {
                    let items: Vec<(String, String)> = (0..4)
                        .map(|offset| {
                            (
                                item_id((first + offset) % ITEM_COUNT),
                                payload(&format!("{key}o{offset}"), 4_096),
                            )
                        })
                        .collect();
                    canonical_record(&state, &items)
                };
                let request = direct_request(&key, &canonical_text(&canonical), Vec::new());
                commit(&mut store, &request, deadline).map(|_| ())
            };
            match attempt() {
                Ok(()) if complete => {
                    completes += 1;
                    complete_due = false;
                    "complete".to_owned()
                }
                Ok(()) => "success".to_owned(),
                Err(error) => error.code().to_owned(),
            }
        };
        *counts.entry(class).or_default() += 1;
    }
    announce(json!({"event": "done", "counts": counts}));
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g9_wal_bound() {
    let profile = profile();
    let directory = output_directory("g9");
    let fixture = gate_fixture(GATE_NODES, false);
    let stop = directory.join("load.stop");
    let _ = fs::remove_file(&stop);
    let root = fixture.layout.root().display().to_string();
    let idle_holder = open(&fixture.layout, &fixture.binding, OpenModeV1::ReadOnly);
    let mut children: Vec<ChildV1> = (0..profile.g9_writers + profile.g9_readers)
        .map(|index| {
            ChildV1::spawn(&json!({
                "role": "g9_contender",
                "root": root,
                "writer": index,
                "reader": index >= profile.g9_writers,
                "stop": stop.display().to_string(),
            }))
        })
        .collect();
    for child in &mut children {
        child.expect("ready");
    }
    for child in &mut children {
        child.send("go");
    }
    let started = Instant::now();
    let mut samples = Vec::new();
    let load = Duration::from_secs(profile.g9_seconds);
    while started.elapsed() < load {
        samples.push(json!({
            "phase": "load",
            "elapsed_ms": started.elapsed().as_millis(),
            "wal_bytes": wal_size_bytes_v1(&fixture.layout),
        }));
        thread::sleep(G9_SAMPLE_INTERVAL);
    }
    fs::write(&stop, b"stop").unwrap();
    let stopped = Instant::now();
    let reports: Vec<Value> = children.into_iter().map(ChildV1::finish).collect();
    let children_exited_ms = stopped.elapsed().as_millis();
    loop {
        let elapsed = stopped.elapsed();
        samples.push(json!({
            "phase": "drain",
            "elapsed_ms": elapsed.as_millis(),
            "wal_bytes": wal_size_bytes_v1(&fixture.layout),
        }));
        if elapsed >= G9_DRAIN_WINDOW {
            break;
        }
        thread::sleep(G9_DRAIN_INTERVAL);
    }
    drop(idle_holder);
    let mut classes = BTreeMap::<String, u64>::new();
    for report in &reports {
        for (class, value) in report["counts"].as_object().unwrap() {
            *classes.entry(class.clone()).or_default() += value.as_u64().unwrap();
        }
    }
    write_samples(&directory, &samples);
    write_summary(
        &directory,
        "G9",
        profile,
        json!({
            "load_seconds": profile.g9_seconds,
            "writers": profile.g9_writers,
            "readers": profile.g9_readers,
            "sample_interval_ms": G9_SAMPLE_INTERVAL.as_millis(),
            "drain_interval_ms": G9_DRAIN_INTERVAL.as_millis(),
            "drain_window_ms": G9_DRAIN_WINDOW.as_millis(),
            "children_exited_ms": children_exited_ms,
            "idle_connection_held": true,
            "outcomes": classes,
        }),
    );
    let unexpected: u64 = classes
        .iter()
        .filter(|(class, _)| {
            !matches!(
                class.as_str(),
                "success" | "complete" | "read" | "PRECONDITION_FAILED"
            )
        })
        .map(|(_, count)| *count)
        .sum();
    assert_eq!(unexpected, 0, "G9 load must not fail unexpectedly");
}

fn file_digests(root: &Path) -> BTreeMap<String, String> {
    fn walk(directory: &Path, root: &Path, digests: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            if path.is_dir() {
                digests.insert(format!("{relative}/"), "directory".to_owned());
                walk(&path, root, digests);
            } else {
                digests.insert(
                    relative,
                    format!("{:x}", Sha256::digest(fs::read(&path).unwrap())),
                );
            }
        }
    }
    let mut digests = BTreeMap::new();
    walk(root, root, &mut digests);
    digests
}

#[test]
#[ignore = "run with tools/v2dfc_gate.py; V2DFC-001 feasibility gate"]
fn v2dfc001_g10_legacy_classification() {
    let profile = profile();
    let directory = output_directory("g10");
    let worktrees = support_phase4_workspace::git_worktrees();
    let base_root = fs::canonicalize(worktrees.main()).unwrap();
    daemon_session(&base_root, worktrees.temporary_path(), 3, false);
    let base_layout = WorkspaceLayoutV1::new(&base_root);
    let base_binding = resolved_binding(&base_root);
    let fixtures = directory.join("fixtures");
    if fixtures.exists() {
        fs::remove_dir_all(&fixtures).unwrap();
    }
    let staging = directory.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let daemon_generation_13 = staging.join("daemon-generation-13.sqlite3");
    fs::copy(base_layout.database(), &daemon_generation_13).unwrap();
    convert_to_prototype_v1(&base_layout, &base_binding).unwrap();
    let current = staging.join("prototype-current.sqlite3");
    fs::copy(base_layout.database(), &current).unwrap();

    let mut cases = Vec::new();
    for generation in [1, 4, 10, 13] {
        for (label, header) in [
            ("checkpointed", LegacyHeaderV1::Checkpointed),
            ("wal-only", LegacyHeaderV1::WalOnly),
        ] {
            let case = fixtures.join(format!("generation-{generation}-{label}"));
            let base = (generation == 13 && header == LegacyHeaderV1::Checkpointed)
                .then_some(daemon_generation_13.as_path());
            write_legacy_store_v1(
                &staging,
                &case.join(".podway/runtime"),
                generation,
                header,
                base,
            )
            .unwrap();
            cases.push((case, Some(generation), None::<&str>));
        }
    }
    for marker in LEGACY_RUNTIME_MARKERS_V1 {
        let case = fixtures.join(format!("marker-{marker}"));
        fs::create_dir_all(case.join(".podway/runtime")).unwrap();
        fs::copy(&current, case.join(".podway/runtime/state.sqlite3")).unwrap();
        fs::write(
            case.join(".podway/runtime").join(marker),
            b"{\"schema\":\"legacy\"}",
        )
        .unwrap();
        cases.push((case, None, Some(marker)));
    }
    let mode_switch = fixtures.join("marker-mode-switch");
    fs::create_dir_all(mode_switch.join(".podway/runtime")).unwrap();
    fs::copy(&current, mode_switch.join(".podway/runtime/state.sqlite3")).unwrap();
    fs::write(
        mode_switch.join(".podway").join(LEGACY_PODWAY_MARKER_V1),
        b"{\"schema\":\"legacy\"}",
    )
    .unwrap();
    cases.push((mode_switch, None, Some(LEGACY_PODWAY_MARKER_V1)));
    let combined = fixtures.join("generation-13-all-markers");
    write_legacy_store_v1(
        &staging,
        &combined.join(".podway/runtime"),
        13,
        LegacyHeaderV1::WalOnly,
        None,
    )
    .unwrap();
    for marker in LEGACY_RUNTIME_MARKERS_V1 {
        fs::write(
            combined.join(".podway/runtime").join(marker),
            b"{\"schema\":\"legacy\"}",
        )
        .unwrap();
    }
    fs::write(
        combined.join(".podway").join(LEGACY_PODWAY_MARKER_V1),
        b"{\"schema\":\"legacy\"}",
    )
    .unwrap();
    cases.push((combined, Some(13), Some("all")));

    let mut records = Vec::new();
    let mut failures = 0;
    for (case, generation, marker) in &cases {
        let before = file_digests(case);
        let layout = WorkspaceLayoutV1::new(case);
        let classification = classify_store_v1(&layout).unwrap();
        let opened = open_store_v1(
            &layout,
            &base_binding,
            OpenModeV1::Mutation,
            Instant::now() + COMMAND_DEADLINE,
        );
        let after = file_digests(case);
        let wal_present = layout.wal().exists();
        let shm_present = layout.shm().exists();
        let legacy = matches!(classification, StoreClassificationV1::Legacy { .. });
        let generation_ok = match (marker, classification) {
            (
                None,
                StoreClassificationV1::Legacy {
                    generation: found, ..
                },
            ) => found == *generation,
            (Some(_), StoreClassificationV1::Legacy { marker: found, .. }) => found.is_some(),
            _ => false,
        };
        let open_error = opened.as_ref().err().cloned();
        let open_legacy = matches!(
            open_error,
            Some(PrototypeErrorV1::LegacyRuntimeState { .. })
        );
        let unchanged = before == after;
        let ok = legacy && generation_ok && open_legacy && unchanged && !shm_present;
        if !ok {
            failures += 1;
        }
        records.push(json!({
            "case": case.file_name().unwrap().to_string_lossy(),
            "classification": format!("{classification:?}"),
            "open_code": open_error.as_ref().map(PrototypeErrorV1::code),
            "recipe": open_error.as_ref().and_then(PrototypeErrorV1::recipe),
            "hot_wal_present": wal_present,
            "shm_created": shm_present,
            "files": before.len(),
            "unchanged": unchanged,
            "ok": ok,
        }));
    }
    write_samples(&directory, &records);
    write_summary(
        &directory,
        "G10",
        profile,
        json!({
            "cases": records.len(),
            "failures": failures,
        }),
    );
    assert_eq!(failures, 0, "G10 legacy state must classify without writes");
}
