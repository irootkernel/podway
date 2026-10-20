//! Surviving persistence guarantees after daemon-era Store retirement.

use podway_core::{UnixMillis, WorkspaceId};
use podway_store::{
    DurableWorktreeIdentityV1, GraphSessionStateV2,
    direct_store::{DIRECT_SCHEMA_GENERATION_V1, DirectStoreV1, ExclusiveMaintenanceLeaseV1},
};
use tempfile::TempDir;

use super::int_v2dfc005_direct_store::{deadline, request, store, success};

pub(super) fn fixture(identity: &DurableWorktreeIdentityV1) -> (TempDir, DirectStoreV1) {
    let root = TempDir::new().unwrap();
    let direct = store(root.path());
    let lease = ExclusiveMaintenanceLeaseV1::acquire(direct.layout(), deadline(), true).unwrap();
    let init = request("initialize-current-fixture");
    direct
        .bootstrap(
            &lease,
            identity.workspace_uuid().clone(),
            "current-generation",
            &init,
            deadline(),
            || UnixMillis::new(1),
            |transaction| {
                Ok(success(
                    &init,
                    serde_json::json!({"initialized":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    drop(lease);
    direct
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.binding().identity(), identity);
            assert_eq!(
                snapshot.binding().schema_version(),
                DIRECT_SCHEMA_GENERATION_V1
            );
            Ok(())
        })
        .unwrap();
    (root, direct)
}

pub(super) fn seed(direct: &DirectStoreV1, state: &GraphSessionStateV2) {
    let seed = request(&format!(
        "seed-current-graph-{}",
        state.trace().session_id().as_str()
    ));
    direct
        .mutate(
            &seed,
            deadline(),
            || UnixMillis::new(100),
            |transaction| {
                transaction.now();
                transaction.create_graph(state)?;
                Ok(success(
                    &seed,
                    serde_json::json!({"seeded":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    assert_eq!(
        direct
            .read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap(),
        Some(state.clone())
    );
    direct.verify_integrity(deadline()).unwrap();
}

pub(super) fn image(direct: &DirectStoreV1) -> Vec<Option<Vec<u8>>> {
    [direct.layout().database(), direct.layout().wal()]
        .iter()
        .map(|path| match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read owned fixture: {error}"),
        })
        .collect()
}

pub(super) fn reject_corruption(direct: &DirectStoreV1, live_projection: bool) {
    let before = image(direct);
    let cold = store(direct.layout().root().to_path_buf().as_path());
    if live_projection {
        assert!(
            cold.read(deadline(), |snapshot| snapshot.view(UnixMillis::new(200)))
                .is_err()
        );
    }
    assert!(cold.verify_integrity(deadline()).is_err());
    assert_eq!(
        image(&cold),
        before,
        "cold corruption checks must not repair persisted bytes"
    );
}

#[test]
fn current_workspace_binding_and_shared_values_preserve_bounds_and_lossless_paths() {
    use podway_store::{
        CanonicalExecutionJsonV1, IdempotencyKeyV1, ValidatedWorkspaceRootV1, WorkspaceBindingV1,
    };
    let identity = DurableWorktreeIdentityV1::new(
        super::int_v2dfc005_direct_store::digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        super::int_v2dfc005_direct_store::digest('b'),
    );
    let root = ValidatedWorkspaceRootV1::from_unix_bytes(b"/tmp/non-utf8-\xff".to_vec()).unwrap();
    assert_eq!(
        ValidatedWorkspaceRootV1::from_encoded(root.as_encoded()).unwrap(),
        root
    );
    assert_eq!(
        WorkspaceBindingV1::new(identity, root).schema_version(),
        DIRECT_SCHEMA_GENERATION_V1
    );
    assert!(IdempotencyKeyV1::new("k".repeat(256)).is_ok());
    assert!(IdempotencyKeyV1::new("k".repeat(257)).is_err());
    assert!(CanonicalExecutionJsonV1::new("{\"a\":1}").is_ok());
    assert!(CanonicalExecutionJsonV1::new("{ \"a\":1}").is_err());
    let at_limit = format!("\"{}\"", "x".repeat(1_048_574));
    assert!(CanonicalExecutionJsonV1::new(at_limit).is_ok());
    assert!(CanonicalExecutionJsonV1::new(format!("\"{}\"", "x".repeat(1_048_575))).is_err());
    let nested = |depth: usize| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
    assert!(CanonicalExecutionJsonV1::new(nested(64)).is_ok());
    assert!(CanonicalExecutionJsonV1::new(nested(65)).is_err());
}

#[test]
fn parse001_canonical_execution_admission_preserves_failure_precedence_and_bytes() {
    use podway_store::{CanonicalExecutionJsonV1, StoreValueErrorV1};

    assert_eq!(
        CanonicalExecutionJsonV1::new(""),
        Err(StoreValueErrorV1::EmptyValue {
            field: "canonical execution JSON",
        })
    );
    assert_eq!(
        CanonicalExecutionJsonV1::new("[".repeat(1_048_577)),
        Err(StoreValueErrorV1::ValueTooLong {
            field: "canonical execution JSON",
            maximum_bytes: 1_048_576,
        })
    );
    let nested = |depth: usize| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
    for input in [nested(65), format!(" {}", nested(65))] {
        assert_eq!(
            CanonicalExecutionJsonV1::new(input),
            Err(StoreValueErrorV1::CanonicalExecutionJsonDepthExceeded { maximum: 64 })
        );
    }
    for input in [
        "{".to_owned(),
        nested(256),
        r#"{"a":1,"a":1}"#.to_owned(),
        r#"{"b":1,"a":2}"#.to_owned(),
        r#"{ "a":1}"#.to_owned(),
        r#"{"a":"\u0061"}"#.to_owned(),
        "1.0".to_owned(),
        "1e0".to_owned(),
        "-0".to_owned(),
    ] {
        assert_eq!(
            CanonicalExecutionJsonV1::new(&input),
            Err(StoreValueErrorV1::InvalidCanonicalExecutionJson),
            "{input}"
        );
    }
    let original = format!(r#"{{"a":[{},{}],"z":"é","é":"\\\""}}"#, i64::MIN, u64::MAX);
    let admitted = CanonicalExecutionJsonV1::new(original.clone()).unwrap();
    assert_eq!(admitted.as_str(), original);
    assert_eq!(admitted.into_inner(), original);
}

pub(super) fn archive(direct: &DirectStoreV1, completed: &GraphSessionStateV2, now: UnixMillis) {
    assert_eq!(
        completed.trace().lifecycle(),
        podway_core::SessionLifecycle::Completed
    );
    let archive = request(&format!(
        "archive-{}",
        completed.trace().session_id().as_str()
    ));
    direct
        .mutate(
            &archive,
            deadline(),
            || now,
            |transaction| {
                transaction.now();
                transaction.record_disposition(
                    &podway_core::TerminalDispositionV2::not_required(
                        completed.trace().session_id().clone(),
                        completed.trace().revision(),
                        "No handoff required",
                        None,
                        now,
                    )
                    .unwrap(),
                )?;
                let summary = transaction
                    .archive_graph(completed.workspace_revision(), completed.trace().revision())?;
                assert_eq!(summary.session_id(), completed.trace().session_id());
                Ok(success(
                    &archive,
                    serde_json::json!({"archived":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    assert!(
        direct
            .read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap()
            .is_none()
    );
}

fn archived_fixture(direct: &DirectStoreV1, number: u64) -> GraphSessionStateV2 {
    let running =
        podway_store::test_support::running_archive_fixture_session_v2(number, number + 100, 10);
    let completed = running
        .complete_active_action_v2(
            running.trace().revision(),
            running.trace().active_attempt().unwrap().attempt_id(),
            None,
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    seed(direct, &completed);
    archive(direct, &completed, UnixMillis::new(30 + number));
    completed
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        super::int_v2dfc005_direct_store::digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        super::int_v2dfc005_direct_store::digest('b'),
    )
}

#[test]
fn terminal_archive_reuses_the_first_unused_slot_after_purge() {
    let (_temporary, direct) = fixture(&identity());
    let first = archived_fixture(&direct, 9400);
    let second = archived_fixture(&direct, 9401);
    let purge = request("purge-first-archive");
    direct
        .mutate(
            &purge,
            deadline(),
            || UnixMillis::new(9432),
            |transaction| {
                transaction.now();
                transaction.purge_archive(first.trace().session_id(), first.trace().revision())?;
                Ok(success(
                    &purge,
                    serde_json::json!({"purged":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let third = archived_fixture(&direct, 9402);
    let page = direct
        .read(deadline(), |snapshot| snapshot.archives(256))
        .unwrap();
    assert_eq!(page.count(), 2);
    assert_eq!(page.sessions()[0].session_id(), third.trace().session_id());
    assert_eq!(page.sessions()[0].archive_slot(), 1);
    assert_eq!(page.sessions()[1].session_id(), second.trace().session_id());
    assert_eq!(page.sessions()[1].archive_slot(), 2);
    direct.verify_integrity(deadline()).unwrap();
}

#[test]
fn unreadable_archive_does_not_block_list_or_purge() {
    let (_temporary, direct) = fixture(&identity());
    let completed = archived_fixture(&direct, 8910);
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_procedure_snapshots SET canonical_json='{}' WHERE snapshot_id=?1",
            [completed.snapshot().snapshot_id().as_str()],
        )
        .unwrap();
    drop(connection);
    let before = image(&direct);
    let cold = store(direct.layout().root().to_path_buf().as_path());
    let page = cold
        .read(deadline(), |snapshot| snapshot.archives(256))
        .unwrap();
    assert_eq!(page.count(), 1);
    assert_eq!(page.sessions().len(), 1);
    assert!(
        cold.read(deadline(), |snapshot| snapshot
            .archived_graph(completed.trace().session_id()))
            .is_err()
    );
    assert!(cold.verify_integrity(deadline()).is_err());
    assert_eq!(image(&cold), before);
    let purge = request("purge-unreadable-archive");
    cold.mutate(
        &purge,
        deadline(),
        || UnixMillis::new(10000),
        |transaction| {
            transaction.now();
            let purged = transaction
                .purge_archive(completed.trace().session_id(), completed.trace().revision())?;
            assert_eq!(purged.session_id(), completed.trace().session_id());
            Ok(success(
                &purge,
                serde_json::json!({"purged":true}),
                transaction.now(),
            ))
        },
    )
    .unwrap();
    let page = cold
        .read(deadline(), |snapshot| snapshot.archives(256))
        .unwrap();
    assert_eq!(page.count(), 0);
    assert!(page.sessions().is_empty());
    assert!(page.oldest().is_none());
    cold.verify_integrity(deadline()).unwrap();
}

#[test]
fn procedure_v2_terminal_reason_accepts_2000_characters_and_cold_read_rejects_2001() {
    use podway_core::{AttemptId, ReasonV2};
    use podway_store::AttemptMetadataV2;
    let (_temporary, direct) = fixture(&identity());
    let running = podway_store::test_support::running_archive_fixture_session_v2(10, 11, 10);
    let old_attempt = running
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let next_attempt = AttemptId::new("00000000-0000-4000-8000-000000000099").unwrap();
    let at_limit = "r".repeat(2000);
    let over_limit = "r".repeat(2001);
    assert!(
        AttemptMetadataV2::new(
            old_attempt.clone(),
            UnixMillis::new(10),
            Some(UnixMillis::new(20)),
            Some(at_limit.clone())
        )
        .is_ok()
    );
    assert!(
        AttemptMetadataV2::new(
            old_attempt.clone(),
            UnixMillis::new(10),
            Some(UnixMillis::new(20)),
            Some(over_limit.clone())
        )
        .is_err()
    );
    seed(&direct, &running);
    let retried = running
        .retry_active_attempt_v2(
            running.trace().revision(),
            &old_attempt,
            next_attempt,
            ReasonV2::new(at_limit.clone()).unwrap(),
            UnixMillis::new(20),
        )
        .unwrap()
        .into_state();
    let retry = request("retry-at-terminal-reason-bound");
    direct
        .mutate(
            &retry,
            deadline(),
            || UnixMillis::new(200),
            |transaction| {
                transaction.now();
                transaction.replace_graph(
                    running.workspace_revision(),
                    running.trace().revision(),
                    &retried,
                )?;
                Ok(success(
                    &retry,
                    serde_json::json!({"retried":true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let cold = store(direct.layout().root().to_path_buf().as_path());
    assert_eq!(
        cold.read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap()
            .unwrap()
            .attempt_metadata()[0]
            .terminal_reason(),
        Some(at_limit.as_str())
    );
    let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
    connection
        .execute(
            "UPDATE v2_attempts SET terminal_reason=?1 WHERE attempt_id=?2",
            [&over_limit, old_attempt.as_str()],
        )
        .unwrap();
    drop(connection);
    reject_corruption(&cold, true);
}
