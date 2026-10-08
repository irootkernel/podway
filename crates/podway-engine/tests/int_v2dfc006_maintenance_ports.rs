//! Retained destructive-maintenance predicates through real native direct fixtures.
use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture},
    int_v2dfc006_fixture_ports::STATE_PROCEDURE,
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::WorkspaceId;
use podway_engine::{
    direct_api::{DirectCommandV1 as D, DirectResponseV1, execute_direct_v1},
    direct_execution::NativeDirectIdSourceV1,
    direct_maintenance::{
        DirectMaintenanceBoundaryV1 as B, DirectMaintenanceRequestV1,
        reset_all_with_boundary_observer_v1,
    },
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::NativeGitResolverV1;
use podway_protocol::RequestIdV1;
use podway_store::IdempotencyKeyV1;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, path::Path};
fn engine() -> Engine {
    let (temporary, selector, store, _) = fixture();
    Engine {
        temporary,
        selector,
        store,
        resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
    }
}
fn uuid(engine: &Engine) -> WorkspaceId {
    engine
        .store
        .read(deadline(), |s| {
            Ok(s.binding().identity().workspace_uuid().clone())
        })
        .unwrap()
}
fn marked_uuid(engine: &Engine) -> WorkspaceId {
    let lease = podway_store::direct_store::ExclusiveMaintenanceLeaseV1::acquire(
        engine.store.layout(),
        deadline(),
        true,
    )
    .unwrap();
    engine
        .store
        .read_under_exclusive_lease(&lease, deadline(), |s| {
            Ok(s.binding().identity().workspace_uuid().clone())
        })
        .unwrap()
}
fn mismatched_selector(engine: &Engine, wrong: WorkspaceId) -> podway_git::WorktreeSelectorV1 {
    let (worktree, _) = engine
        .resolver
        .resolve_candidate(engine.selector.clone())
        .unwrap();
    let identity = worktree.identity();
    let durable = podway_git::DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
        wrong,
        identity.common_directory_fingerprint().clone(),
        identity.worktree_administration_fingerprint().clone(),
        identity.root_directory_fingerprint().unwrap().clone(),
        identity.last_validated_root().clone(),
    );
    podway_git::WorktreeSelectorV1::new(
        engine.selector.version(),
        Some(durable),
        engine.selector.path().clone(),
    )
    .unwrap()
}
fn files(path: &Path) -> BTreeMap<String, String> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                visit(root, &path, out);
            } else {
                let bytes = if kind.is_symlink() {
                    std::fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec()
                } else {
                    std::fs::read(&path).unwrap()
                };
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    format!("{:x}", Sha256::digest(&bytes)),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(path, path, &mut out);
    out
}
fn reject(
    engine: &Engine,
    request: podway_engine::direct_api::DirectRequestV1,
    code: &str,
) -> Value {
    let before = files(&engine.temporary.path().join(".podway"));
    let DirectResponseV1::Failure(error) = engine.execute(request, &Clock) else {
        panic!("expected {code}")
    };
    let error = error.into_value();
    assert_eq!(error["code"], code);
    assert_eq!(files(&engine.temporary.path().join(".podway")), before);
    error
}
fn maintenance(engine: &Engine, key: &str) -> DirectMaintenanceRequestV1 {
    DirectMaintenanceRequestV1 {
        selector: engine.selector.clone(),
        expected_workspace_id: Some(uuid(engine)),
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000098").unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        confirmed: true,
        force: false,
        deadline: deadline(),
    }
}
fn crash(engine: &Engine, action: &str, phase: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "int_v2dfc006_maintenance::maintenance_crash_child",
            "--nocapture",
        ])
        .env("PODWAY_MAINTENANCE_FIXTURE", engine.temporary.path())
        .env("PODWAY_MAINTENANCE_UUID", uuid(engine).as_str())
        .env("PODWAY_MAINTENANCE_ACTION", action)
        .env("PODWAY_MAINTENANCE_PHASE", phase)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(77),
        "owned {action}/{phase} boundary not reached: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn missing_registry_reset_crash_boundaries_converge_on_cold_replay() {
    for phase in [
        "intent",
        "marker",
        "cleared",
        "phase",
        "before_begin",
        "before_outcome",
        "before_publication",
        "after_publication",
    ] {
        let mut engine = engine();
        let old = uuid(&engine);
        let outside = engine.temporary.path().join("preserved.txt");
        std::fs::write(&outside, "retain outside Podway").unwrap();
        crash(&engine, "reset", phase);
        let intent: Value =
            serde_json::from_slice(&std::fs::read(engine.store.layout().intent_marker()).unwrap())
                .unwrap();
        let planned = WorkspaceId::new(intent["workspace_uuid"].as_str().unwrap()).unwrap();
        assert_ne!(planned, old);
        match phase {
            "intent" | "marker" => assert_eq!(marked_uuid(&engine), old),
            "cleared" | "phase" | "before_begin" | "before_outcome" | "before_publication" => {
                assert!(!engine.store.layout().database().exists())
            }
            "after_publication" => assert_eq!(marked_uuid(&engine), planned),
            _ => unreachable!(),
        }
        engine.cold();
        let mut request = engine.request(D::WorkspaceResetAll { confirmed: true }, "reset");
        request.expected_workspace_id = Some(old);
        let output = engine.output(engine.execute(request.clone(), &NoClock));
        assert_eq!(output["workspace"]["uuid"], planned.as_str());
        assert!(!engine.store.layout().reset_marker().exists());
        assert!(!engine.store.layout().intent_marker().exists());
        engine.cold();
        assert_eq!(engine.output(engine.execute(request, &NoClock)), output);
        assert_eq!(uuid(&engine), planned);
        assert_eq!(
            engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"]["healthy"],
            true
        );
        assert_eq!(
            std::fs::read_to_string(outside).unwrap(),
            "retain outside Podway"
        );
        assert!(engine.temporary.path().join(".git").is_dir());
    }
}
#[test]
fn every_removal_crash_boundary_converges_on_cold_replay() {
    for phase in ["intent", "marker", "cleared", "phase", "final"] {
        let mut engine = engine();
        let old = uuid(&engine);
        let outside = engine.temporary.path().join("preserved.txt");
        std::fs::write(&outside, "retain outside Podway").unwrap();
        crash(&engine, "remove", phase);
        engine.cold();
        let mut request = engine.request(
            D::WorkspaceRemove {
                confirmed: true,
                force: false,
            },
            "remove",
        );
        request.expected_workspace_id = Some(old.clone());
        let output = engine.output(engine.execute(request, &NoClock));
        assert_eq!(output["result"]["workspace_uuid"], old.as_str());
        assert_eq!(output["result"]["already_absent"], false);
        assert!(!engine.temporary.path().join(".podway").exists());
        assert!(engine.temporary.path().join(".git").is_dir());
        assert_eq!(
            std::fs::read_to_string(outside).unwrap(),
            "retain outside Podway"
        );
    }
}
#[test]
fn confirmed_reset_all_recovers_a_store_when_the_registry_entry_is_missing() {
    // There is no direct registry; a local authoritative Store is sufficient.
    let mut engine = engine();
    let old = uuid(&engine);
    let mut request = engine.request(D::WorkspaceResetAll { confirmed: true }, "reset");
    request.expected_workspace_id = Some(old.clone());
    let once = engine.output(engine.execute(request.clone(), &Clock));
    let new = WorkspaceId::new(once["workspace"]["uuid"].as_str().unwrap()).unwrap();
    assert_ne!(new, old);
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    assert_eq!(
        engine.success(D::WorkspaceShow, "show")["workspace"]["uuid"],
        new.as_str()
    );
    assert_eq!(
        engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"]["healthy"],
        true
    );
}
#[test]
fn reset_rejects_a_store_bound_to_another_exact_root() {
    let source = engine();
    let target = engine();
    // An actual copied Store in an unrelated native Git root has foreign fingerprints.
    std::fs::copy(
        source.store.layout().database(),
        target.store.layout().database(),
    )
    .unwrap();
    reject(
        &target,
        target.request(D::WorkspaceResetAll { confirmed: true }, "foreign-root"),
        "WORKSPACE_ID_CONFLICT",
    );
    assert!(!target.store.layout().reset_marker().exists());
    assert!(!target.store.layout().intent_marker().exists());
}
#[test]
fn reset_rejects_a_local_store_uuid_registered_at_another_root_before_deletion() {
    // The registry conflict is retired. Preserve the actual local UUID fence and no deletion.
    let engine = engine();
    let mut request = engine.request(D::WorkspaceResetAll { confirmed: true }, "wrong-uuid");
    request.expected_workspace_id =
        Some(WorkspaceId::new("00000000-0000-4000-8000-000000049999").unwrap());
    reject(&engine, request, "WORKSPACE_UUID_MISMATCH");
    assert!(!engine.store.layout().reset_marker().exists());
}
#[test]
fn reset_all_recovers_when_full_store_openability_fails_internal_codec() {
    let mut engine = Engine::new(STATE_PROCEDURE);
    let old = uuid(&engine);
    podway_store::test_support::corrupt_current_session_cursor(&engine.store.layout().database())
        .unwrap();
    assert_eq!(
        engine.success(D::WorkspaceDoctor { deep: true }, "diagnose")["result"]["healthy"],
        false
    );
    let mut request = engine.request(D::WorkspaceResetAll { confirmed: true }, "codec-reset");
    request.expected_workspace_id = Some(old.clone());
    let once = engine.output(engine.execute(request.clone(), &Clock));
    let new = once["workspace"]["uuid"].as_str().unwrap();
    assert_ne!(new, old.as_str());
    engine.cold();
    assert_eq!(
        engine.success(D::WorkspaceShow, "show")["workspace"]["uuid"],
        new
    );
    assert_eq!(
        engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"]["healthy"],
        true
    );
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    assert!(
        engine
            .store
            .read(deadline(), |s| s.current_graph())
            .unwrap()
            .is_none()
    );
    assert!(
        engine
            .store
            .read(deadline(), |s| s
                .outcome(&IdempotencyKeyV1::new("start").unwrap()))
            .unwrap()
            .is_none()
    );
}
#[test]
fn v2cut_legacy_schema_v3_rejects_then_confirmed_reset_all_replaces_and_cold_reopens() {
    use podway_store::test_support::legacy_fixture::{LegacyHeaderV1, write_legacy_store_v1};
    let mut engine = engine();
    let runtime = engine.store.layout().runtime_directory();
    let backup = engine.temporary.path().join("initial-runtime");
    std::fs::rename(&runtime, &backup).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let staging = engine.temporary.path().join("legacy-staging");
    std::fs::create_dir(&staging).unwrap();
    write_legacy_store_v1(&staging, &runtime, 3, LegacyHeaderV1::WalOnly).unwrap();
    let before = files(&runtime);
    let DirectResponseV1::Failure(error) =
        engine.execute(engine.request(D::WorkspaceShow, "legacy-show"), &Clock)
    else {
        panic!("legacy state must reject")
    };
    assert_eq!(
        error.into_value()["code"],
        "LEGACY_RUNTIME_STATE_UNSUPPORTED"
    );
    assert_eq!(files(&runtime), before);
    let request = engine.request(D::WorkspaceResetAll { confirmed: true }, "legacy-reset");
    let once = engine.output(engine.execute(request.clone(), &Clock));
    engine.cold();
    assert_eq!(
        engine.success(D::WorkspaceShow, "show")["workspace"]["uuid"],
        once["workspace"]["uuid"]
    );
    assert_eq!(
        engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"]["healthy"],
        true
    );
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    let lookup = engine.success(
        D::OutcomeLookup {
            key: IdempotencyKeyV1::new("legacy-reset").unwrap(),
        },
        "lookup",
    );
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], once);
}
#[test]
fn confirmed_removal_deletes_only_podway_and_converges_on_replay() {
    let mut engine = engine();
    let old = uuid(&engine);
    let outside = engine.temporary.path().join("preserved.txt");
    std::fs::write(&outside, "preserve the Git worktree\n").unwrap();
    let custom = engine
        .temporary
        .path()
        .join(".podway/custom/procedure.yaml");
    std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
    std::fs::write(&custom, "schema: custom\n").unwrap();
    std::os::unix::fs::symlink(
        &outside,
        engine.temporary.path().join(".podway/outside-link"),
    )
    .unwrap();
    let mut request = engine.request(
        D::WorkspaceRemove {
            confirmed: true,
            force: true,
        },
        "remove",
    );
    request.expected_workspace_id = Some(old.clone());
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["workspace_uuid"], old.as_str());
    assert_eq!(once["result"]["podway_directory_removed"], true);
    assert_eq!(once["result"]["already_absent"], false);
    assert!(!engine.temporary.path().join(".podway").exists());
    assert!(engine.temporary.path().join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "preserve the Git worktree\n"
    );
    engine.cold();
    let absent = engine.output(engine.execute(request, &Clock));
    assert!(absent["result"]["workspace_uuid"].is_null());
    assert_eq!(absent["result"]["podway_directory_removed"], false);
    assert_eq!(absent["result"]["already_absent"], true);
    let new = engine.success(D::WorkspaceInit { repair: false }, "reinitialize");
    assert_ne!(new["workspace"]["uuid"], old.as_str());
}
#[test]
fn uuid_mismatch_rejects_removal_without_mutation() {
    let engine = engine();
    let wrong = WorkspaceId::new("00000000-0000-4000-8000-000000049999").unwrap();
    for selector_fence in [false, true] {
        let mut request = engine.request(
            D::WorkspaceRemove {
                confirmed: true,
                force: true,
            },
            "wrong-remove",
        );
        if selector_fence {
            request.selector = mismatched_selector(&engine, wrong.clone());
        } else {
            request.expected_workspace_id = Some(wrong.clone());
        }
        reject(&engine, request, "WORKSPACE_UUID_MISMATCH");
        assert!(engine.store.layout().database().is_file());
    }
}
#[test]
fn markerless_residual_converges_only_when_descriptor_verified_empty() {
    let engine = engine();
    let old = uuid(&engine);
    engine.success(
        D::WorkspaceRemove {
            confirmed: true,
            force: true,
        },
        "remove-initial",
    );
    let podway = engine.temporary.path().join(".podway");
    std::fs::create_dir_all(podway.join("empty/leaf")).unwrap();
    let mut request = engine.request(
        D::WorkspaceRemove {
            confirmed: true,
            force: true,
        },
        "remove-residual",
    );
    request.expected_workspace_id = Some(old);
    let empty = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(empty["result"]["already_absent"], true);
    assert_eq!(empty["result"]["podway_directory_removed"], true);
    assert!(!podway.exists());
    std::fs::create_dir(&podway).unwrap();
    std::fs::write(podway.join("ambiguous"), "retain without marker").unwrap();
    reject(&engine, request, "WORKSPACE_PATH_UNSAFE");
    assert_eq!(
        std::fs::read_to_string(podway.join("ambiguous")).unwrap(),
        "retain without marker"
    );
}
#[test]
fn reset_marker_resume_accepts_an_exact_root_with_detached_git_identity() {
    let engine = engine();
    let database = engine.store.layout().database();
    let request = maintenance(&engine, "detached-resume");
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reset_all_with_boundary_observer_v1(
            &engine.resolver,
            request.clone(),
            &Clock,
            &NativeDirectIdSourceV1,
            |b| {
                if b == B::MarkerPublished {
                    panic!("owned marker interruption");
                }
            },
        )
    }));
    assert!(interrupted.is_err());
    podway_store::test_support::detach_git_identity(&database).unwrap();
    let mut retry = engine.request(D::WorkspaceResetAll { confirmed: true }, "detached-resume");
    retry.expected_workspace_id = request.expected_workspace_id;
    let output =
        execute_direct_v1(&engine.resolver, retry, &Clock, &NativeDirectIdSourceV1).unwrap();
    let DirectResponseV1::Success(output) = output else {
        panic!("exact reset retry rejected");
    };
    let output = output.as_value();
    assert_eq!(output["result"]["reset"], true);
    assert_eq!(
        engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"]["healthy"],
        true
    );
}
#[test]
fn reset_rechecks_initially_absent_marker_uuids_after_marker_publication() {
    // No registry is involved. Preserve the required post-publication native root identity fence.
    let engine = engine();
    let request = maintenance(&engine, "post-marker");
    let database = engine.store.layout().database();
    let before = std::fs::metadata(&database).unwrap();
    let root = engine.temporary.path().to_path_buf();
    let moved = root.join("original-git");
    let saved_root = root.clone();
    let result = reset_all_with_boundary_observer_v1(
        &engine.resolver,
        request,
        &Clock,
        &NativeDirectIdSourceV1,
        move |b| {
            if b == B::MarkerPublished {
                std::fs::rename(saved_root.join(".git"), &moved).unwrap();
                assert!(
                    std::process::Command::new("git")
                        .args(["init", "--quiet"])
                        .arg(&saved_root)
                        .status()
                        .unwrap()
                        .success()
                );
            }
        },
    );
    assert!(
        result.is_err(),
        "a post-marker foreign Git identity must reject before destroying the predecessor"
    );
    let after = std::fs::metadata(&database).unwrap();
    assert_eq!(after.dev(), before.dev());
    assert_eq!(after.ino(), before.ino());
    assert!(engine.store.layout().reset_marker().exists());
    assert!(root.join("original-git").is_dir());
}
