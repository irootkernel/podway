//! Real Git, SQLite, and Unix-socket coverage for the production daemon runtime.

#![forbid(unsafe_code)]

mod support_phase4_workspace;

use std::{
    fs,
    net::Shutdown,
    num::NonZeroUsize,
    os::unix::{
        ffi::OsStrExt,
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use podway_core::UnixMillis;
use podway_daemon::{
    dispatch::{DispatchFailureKindV1, WorkspaceRuntimeV1},
    production::{NativeProductionClockV1, ProductionWorkspaceRuntimeV1},
    runtime::{
        ProductionDaemonRuntimeConfigV1, ProductionDaemonRuntimeV1, WorkspaceRecoveryEntryV1,
        WorkspaceRecoveryUnavailableReasonV1,
    },
    runtime_workspace::{WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1},
};
use podway_git::{GitResolverContractV1, NativeGitResolverV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1 as ProtocolIdempotencyKeyV1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV1, Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1,
    WorktreeSelectorWireV1, decode_response_payload_v1, encode_request_payload_v1,
    read_single_frame_v1, write_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, SqliteStoreV1, ValidatedWorkspaceRootV1, WorkerIdV1};
use support_phase4_workspace::{
    TemporaryDirectoryV1, copy_tree, git_worktrees, non_utf8_child_path, read_file,
    selector as git_selector,
};

struct RuntimePathsFixtureV1 {
    paths: ServiceRuntimePathsV1,
    runtime_directory: PathBuf,
}

impl RuntimePathsFixtureV1 {
    fn paths(&self) -> &ServiceRuntimePathsV1 {
        &self.paths
    }
}

impl Drop for RuntimePathsFixtureV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.runtime_directory);
    }
}

fn runtime_paths(root: &Path) -> RuntimePathsFixtureV1 {
    let application_support = root.join("Application Support/Podway");
    fs::create_dir_all(&application_support).expect("registry parent must be created");
    fs::set_permissions(&application_support, fs::Permissions::from_mode(0o700))
        .expect("registry parent must be private");
    let fixture_name = root
        .file_name()
        .expect("fixture root must have a final component")
        .to_string_lossy();
    let runtime_directory = PathBuf::from("/tmp").join(format!("pdr-{fixture_name}"));
    let _ = fs::remove_dir_all(&runtime_directory);
    let paths = ServiceRuntimePathsV1::from_directories(
        root.join("LaunchAgents"),
        application_support,
        root.join("Logs/Podway"),
        &runtime_directory,
    )
    .expect("service paths must be valid");
    RuntimePathsFixtureV1 {
        paths,
        runtime_directory,
    }
}

fn make_workspace_runtime_private(root: &Path) {
    fs::set_permissions(
        root.join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("workspace runtime directory must be private");
}

fn configuration(worker: &str) -> ProductionDaemonRuntimeConfigV1 {
    ProductionDaemonRuntimeConfigV1::new(
        WorkerIdV1::new(worker).expect("worker ID must be valid"),
        NonZeroUsize::new(4).expect("four is nonzero"),
        Default::default(),
    )
}
fn observation() -> WorkspaceRuntimeObservationV1 {
    WorkspaceRuntimeObservationV1::new(
        UnixMillis::new(1_700_000_000_123),
        Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z")
            .expect("fixture registry timestamp must be valid"),
    )
}

fn selector(path: &Path) -> WorktreeSelectorWireV1 {
    use std::os::unix::ffi::OsStrExt;

    let canonical = fs::canonicalize(path).expect("fixture worktree must be canonical");
    WorktreeSelectorWireV1::new(
        canonical.as_os_str().as_bytes(),
        canonical.display().to_string(),
        None,
    )
    .expect("fixture selector must be valid")
}

fn request(
    request_number: u64,
    command: &str,
    operation: OperationV1,
    selector: &WorktreeSelectorWireV1,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    let payload = serde_json::json!({"selector": selector})
        .as_object()
        .expect("fixture payload must be an object")
        .clone();
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .expect("fixture request ID must be valid"),
        client: ClientInfoV1::new("daemon-runtime-test", "1", 1)
            .expect("fixture client must be valid"),
        operation,
        command: CommandNameV1::new(command).expect("fixture command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new(selector.display(), None)
                .expect("fixture workspace context must be valid"),
        ),
        idempotency_key: (command != "session.status").then(|| {
            ProtocolIdempotencyKeyV1::new(format!("daemon-runtime-{request_number}"))
                .expect("fixture idempotency key must be valid")
        }),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(operation != OperationV1::Query, 0)
            .expect("fixture request options must be valid"),
        payload,
    })
    .expect("fixture request must be valid");
    let slice = SliceRequestV1::from_envelope(&envelope).expect("fixture request must be routed");
    (envelope, slice)
}

#[test]
fn startup_recovery_serves_a_real_workspace_without_public_store_mutation_access() {
    let fixture = git_worktrees();
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let options = SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid");
    let manager = Arc::new(WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        options.clone(),
    ));
    let workspace_runtime = ProductionWorkspaceRuntimeV1::new(
        Arc::clone(&manager),
        Arc::new(NativeProductionClockV1::default()),
    );
    let workspace_selector = selector(fixture.main());
    let workspace = workspace_runtime
        .resolve_bootstrap(&workspace_selector)
        .expect("real workspace must bootstrap");
    let _context = workspace.scheduler().context_snapshot();
    drop(_context);
    drop(workspace);
    drop(workspace_runtime);
    drop(manager);

    let runtime =
        ProductionDaemonRuntimeV1::bind(paths.paths(), options, configuration("recovery-worker"))
            .expect("startup recovery must bind the endpoint and recover the workspace");
    assert!(matches!(
        runtime.recovery_report().workspaces(),
        [WorkspaceRecoveryEntryV1::Recovered(report)]
            if report.requeued_job_count() == 0 && report.drained_terminal_job_count() == 0
    ));

    let socket_path = runtime.socket_path().to_path_buf();
    let shutdown = runtime.shutdown_handle();
    let server = thread::spawn(move || runtime.run());
    let status = request(2, "session.status", OperationV1::Query, &workspace_selector);
    let payload = encode_request_payload_v1(&status.0).expect("status request must encode");
    let mut client =
        UnixStream::connect(&socket_path).expect("client must connect to bound daemon");
    write_frame_v1(&mut client, &payload).expect("client must write framed status request");
    client
        .shutdown(Shutdown::Write)
        .expect("client must half-close status request");
    let response_payload = read_single_frame_v1(&mut client)
        .expect("daemon must return one framed response")
        .expect("daemon must return a response frame");
    let response = decode_response_payload_v1(&response_payload)
        .expect("daemon response must satisfy the public protocol");
    match response {
        ResponseEnvelopeV1::Error(error) => assert_eq!(error.code().as_str(), "SESSION_NOT_FOUND"),
        ResponseEnvelopeV1::Output(_) => {
            panic!("uninitialized session status must be a typed error")
        }
    }

    shutdown.request_shutdown();
    assert!(
        server
            .join()
            .expect("daemon thread must not panic")
            .expect("daemon shutdown must succeed")
            .recovered_workspace_count()
            == 1
    );
    assert!(
        !socket_path.exists(),
        "owned socket must be removed at shutdown"
    );
}

#[test]
fn readonly_resolution_reuses_only_the_exact_active_context_without_registry_refresh() {
    let fixture = git_worktrees();
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let manager = WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
    );
    let scheduler = manager
        .bootstrap(git_selector(fixture.main()), observation())
        .expect("workspace bootstrap must succeed");
    let context = scheduler.context_snapshot();
    let workspace_id = context.binding().identity().workspace_uuid().clone();
    let before = manager
        .registry()
        .lookup(&workspace_id)
        .expect("registry must be readable")
        .expect("bootstrap must publish registry metadata");
    let resolution = manager
        .resolve_existing_readonly(git_selector(fixture.main()), Some(&workspace_id))
        .expect("read-only resolution must validate the existing workspace");
    let active = resolution
        .active_scheduler()
        .expect("the exact active context must be reusable");
    assert!(Arc::ptr_eq(active, &scheduler));
    assert_eq!(resolution.binding(), context.binding());
    let after = manager
        .registry()
        .lookup(&workspace_id)
        .expect("registry must remain readable")
        .expect("read-only resolution must retain registry metadata");
    assert_eq!(after.last_known_root(), before.last_known_root());
    assert_eq!(after.last_seen_at(), before.last_seen_at());
}
#[test]
fn shutdown_leaves_a_replacement_socket_untouched() {
    let temporary = TemporaryDirectoryV1::new("podway-daemon-runtime-endpoint");
    let paths = runtime_paths(temporary.path());
    let runtime = ProductionDaemonRuntimeV1::bind(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
        configuration("replacement-worker"),
    )
    .expect("daemon must bind an empty registry");
    let socket_path = runtime.socket_path().to_path_buf();
    fs::remove_file(&socket_path).expect("test must replace the owned socket path");
    let replacement = UnixListener::bind(&socket_path).expect("replacement socket must bind");

    let shutdown = runtime.shutdown_handle();
    shutdown.request_shutdown();
    runtime.run().expect("runtime must stop cleanly");
    assert!(
        socket_path.exists(),
        "endpoint shutdown must not unlink a replacement socket"
    );
    drop(replacement);
    fs::remove_file(socket_path).expect("test replacement socket must be removed");
}

#[test]
fn active_move_rebind_recovers_an_exact_store_registry_split_idempotently() {
    let fixture = git_worktrees();
    make_workspace_runtime_private(fixture.main());
    let paths = runtime_paths(fixture.temporary_path());
    let manager = WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid"),
    );
    let initial = manager
        .bootstrap(git_selector(fixture.main()), observation())
        .expect("main worktree must bootstrap");
    let initial_context = initial.context_snapshot();
    let workspace_uuid = initial_context
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let initial_key = initial.key().clone();
    let initial_generation = initial.generation();
    let original_root = initial_context.workspace_root().to_path_buf();
    let relocated = fixture.temporary_path().join("relocated-main");
    fs::rename(fixture.main(), &relocated).expect("real Git worktree must move atomically");
    let canonical_relocated =
        fs::canonicalize(&relocated).expect("relocated worktree must canonicalize");
    let relocated_root = ValidatedWorkspaceRootV1::from_path(&canonical_relocated)
        .expect("canonical relocated root must be Store-valid");
    let relocated_database = relocated.join(".podway/runtime/state.sqlite3");

    // Simulate the durable half of a prior move activation that completed before its metadata
    // publication. The retry below must only repair metadata from the validated Store/Git state.
    let partial_store = SqliteStoreV1::open(
        &relocated_database,
        &relocated_root,
        initial_context.binding().identity().clone(),
        initial_context.store_options().clone(),
        observation().store_now(),
    )
    .expect("Store root update must establish the deterministic partial move state");
    drop(partial_store);
    let binding_after_partial = SqliteStoreV1::inspect_workspace_binding(
        &relocated_database,
        initial_context.store_options(),
    )
    .expect("partial Store binding remains inspectable")
    .expect("partial Store binding remains present");
    assert_eq!(binding_after_partial.last_validated_root(), &relocated_root);
    assert_eq!(
        manager
            .registry()
            .lookup(&workspace_uuid)
            .expect("registry remains readable")
            .expect("bootstrap metadata remains registered")
            .last_known_root()
            .to_path_buf(),
        original_root,
        "the deterministic partial state leaves only registry metadata at the prior root"
    );

    let rebound = manager
        .resolve_existing(
            git_selector(&relocated),
            Some(&workspace_uuid),
            observation(),
        )
        .expect("validated move retry must reconcile only the stale registry metadata");
    let rebound_context = rebound.context_snapshot();
    assert!(Arc::ptr_eq(&initial, &rebound));
    assert_eq!(rebound.key(), &initial_key);
    assert_eq!(rebound.generation(), initial_generation);
    assert_eq!(
        rebound_context.workspace_root().to_path_buf(),
        canonical_relocated
    );
    assert_eq!(
        manager
            .registry()
            .lookup(&workspace_uuid)
            .expect("reconciled registry remains readable")
            .expect("reconciled metadata remains registered")
            .last_known_root()
            .to_path_buf(),
        canonical_relocated
    );

    let retried = manager
        .resolve_existing(
            git_selector(&relocated),
            Some(&workspace_uuid),
            observation(),
        )
        .expect("the recovered stationary binding must remain idempotently available");
    assert!(Arc::ptr_eq(&rebound, &retried));
    assert_eq!(retried.generation(), initial_generation);
}
#[test]
fn deleted_registered_worktree_is_unavailable_without_recovering_as_another_workspace() {
    let fixture = git_worktrees();
    let direct = NativeGitResolverV1::new()
        .resolve(git_selector(fixture.main()))
        .expect("fixture main worktree must resolve natively");
    assert_eq!(
        direct
            .roots()
            .worktree_root()
            .decode_path_bytes()
            .expect("native root bytes must decode"),
        fs::canonicalize(fixture.main())
            .expect("fixture main path must canonicalize")
            .as_os_str()
            .as_bytes()
    );
    assert!(
        non_utf8_child_path(fixture.temporary_path())
            .as_os_str()
            .as_bytes()
            .ends_with(&[0xff]),
        "lossless fixture must retain the non-UTF-8 path byte"
    );
    make_workspace_runtime_private(fixture.main());
    make_workspace_runtime_private(fixture.linked());
    let paths = runtime_paths(fixture.temporary_path());
    let options = SqliteStoreOptionsV1::new(8).expect("SQLite options must be valid");
    let manager = Arc::new(WorkspaceRuntimeManagerV1::new(
        paths.paths(),
        options.clone(),
    ));
    let workspace_runtime = ProductionWorkspaceRuntimeV1::new(
        Arc::clone(&manager),
        Arc::new(NativeProductionClockV1::default()),
    );
    let main = workspace_runtime
        .resolve_bootstrap(&selector(fixture.main()))
        .expect("main worktree must bootstrap");
    let linked = workspace_runtime
        .resolve_bootstrap(&selector(fixture.linked()))
        .expect("linked worktree must bootstrap");
    assert!(
        read_file(&fixture.main().join(".podway/config.yaml")).starts_with(b"schema:"),
        "bootstrap must publish the admitted workspace configuration"
    );
    let copied = fixture.temporary_path().join("copied-main-worktree");
    copy_tree(fixture.main(), &copied);
    let copied_error = match workspace_runtime.resolve_existing(&selector(&copied)) {
        Ok(_) => panic!("a copied worktree must not adopt the registered durable identity"),
        Err(error) => error,
    };
    assert_eq!(
        copied_error.kind(),
        DispatchFailureKindV1::WorkspaceIdentityConflict,
        "a copied worktree must preserve the public identity-conflict classification: {copied_error:?}"
    );
    let main_uuid = main
        .scheduler()
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let linked_uuid = linked
        .scheduler()
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    drop(linked);
    drop(main);
    drop(workspace_runtime);
    drop(manager);
    fs::remove_dir_all(fixture.linked()).expect("linked worktree must be deleted before recovery");

    let runtime =
        ProductionDaemonRuntimeV1::bind(paths.paths(), options, configuration("isolation-worker"))
            .expect("a deleted registry entry must not block recovery of the valid worktree");
    assert_eq!(runtime.recovery_report().recovered_workspace_count(), 1);
    assert_eq!(runtime.recovery_report().unavailable_workspace_count(), 1);
    assert!(runtime.recovery_report().workspaces().iter().any(|entry| {
        matches!(
            entry,
            WorkspaceRecoveryEntryV1::Recovered(report)
                if report.workspace_uuid() == &main_uuid
        )
    }));
    assert!(runtime.recovery_report().workspaces().iter().any(|entry| {
        matches!(
            entry,
            WorkspaceRecoveryEntryV1::Unavailable(report)
                if report.workspace_uuid() == &linked_uuid
                    && report.reason() == WorkspaceRecoveryUnavailableReasonV1::WorktreeGone
        )
    }));

    let shutdown = runtime.shutdown_handle();
    shutdown.request_shutdown();
    runtime.run().expect("runtime cleanup must succeed");
}
