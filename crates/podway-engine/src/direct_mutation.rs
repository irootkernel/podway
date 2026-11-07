//! Shared direct mutation fences and complete, bounded success serialization.

use podway_core::{AttemptId, DomainError, Revision, SessionId, canonicalize_json_v1};
use podway_protocol::{
    CommandNameV1, DirectSessionOutputV1, MUTATION_OBSERVATION_RESULT_SCHEMA_V1,
    OutputEnvelopeInputV4, OutputEnvelopeV4, RequestIdV1, Rfc3339MillisV1, WorkspaceOutputV1,
    validate_mutation_observation_output_envelope_v1,
};
use podway_store::{
    CanonicalExecutionJsonV1, GraphMutationErrorV2, GraphSessionStateV2,
    direct_store::{DirectStoreErrorV1, DirectTransactionV1},
};

use crate::{
    v2_read_service::{SuccessorObservationContextV1, project_successor_observation_v1},
    workspace::ResolvedWorkspaceV1,
};

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
enum ObservationFaultV1 {
    InconsistentState,
    ExceedLookupHeadroom,
    ExceedLookupDepth,
}

#[cfg(test)]
std::thread_local! {
    static NEXT_OBSERVATION_FAULT_V1: std::cell::Cell<Option<ObservationFaultV1>> =
        const { std::cell::Cell::new(None) };
    static LAST_OBSERVATION_PROTOCOL_ERROR_V1: std::cell::RefCell<Option<podway_protocol::ProtocolError>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn take_observation_fault_v1() -> Option<ObservationFaultV1> {
    NEXT_OBSERVATION_FAULT_V1.with(|slot| slot.take())
}

#[cfg(test)]
fn record_last_observation_protocol_error_v1(error: podway_protocol::ProtocolError) {
    LAST_OBSERVATION_PROTOCOL_ERROR_V1.with(|slot| {
        *slot.borrow_mut() = Some(error);
    });
}

#[cfg(test)]
fn take_last_observation_protocol_error_v1() -> Option<podway_protocol::ProtocolError> {
    LAST_OBSERVATION_PROTOCOL_ERROR_V1.with(|slot| slot.borrow_mut().take())
}

pub(crate) fn validate_disposition_input(
    input: &podway_protocol::TerminalDispositionInputV1,
) -> Result<(), DirectStoreErrorV1> {
    let fields: &[&str] = match input {
        podway_protocol::TerminalDispositionInputV1::HandedOff { summary, reference } => {
            &[summary, reference]
        }
        podway_protocol::TerminalDispositionInputV1::NotRequired { reason } => &[reason],
    };
    if fields.iter().any(|value| {
        value.trim().is_empty()
            || value.chars().count() > podway_core::MAX_TERMINAL_DISPOSITION_CHARACTERS_V2
    }) {
        return Err(DirectStoreErrorV1::InvalidInput {
            reason: "invalid terminal disposition text",
        });
    }
    Ok(())
}

pub(crate) fn check_session_fences(
    graph: &GraphSessionStateV2,
    session: &SessionId,
    revision: Revision,
) -> Result<(), DirectStoreErrorV1> {
    if graph.trace().session_id() != session {
        return Err(
            GraphMutationErrorV2::Domain(DomainError::SessionIdentityMismatch {
                expected: session.clone(),
                actual: Some(graph.trace().session_id().clone()),
            })
            .into(),
        );
    }
    if graph.trace().revision() != revision {
        return Err(GraphMutationErrorV2::SessionRevisionConflict {
            expected: revision,
            actual: graph.trace().revision(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn check_attempt_fence(
    graph: &GraphSessionStateV2,
    attempt: &AttemptId,
) -> Result<(), DirectStoreErrorV1> {
    let actual = graph
        .trace()
        .active_attempt()
        .map(|active| active.attempt_id());
    if actual != Some(attempt) {
        return Err(GraphMutationErrorV2::AttemptNotCurrent {
            expected: attempt.clone(),
            actual: actual.cloned(),
        }
        .into());
    }
    Ok(())
}

pub(crate) fn graph_success<Clock: FnOnce() -> podway_core::UnixMillis>(
    transaction: &mut DirectTransactionV1<'_, '_, Clock>,
    resolved: &ResolvedWorkspaceV1,
    request_id: RequestIdV1,
    command: &str,
    prior: &GraphSessionStateV2,
    next: &GraphSessionStateV2,
    result: serde_json::Map<String, serde_json::Value>,
) -> Result<CanonicalExecutionJsonV1, DirectStoreErrorV1> {
    graph_success_with_observation(
        transaction,
        resolved,
        request_id,
        command,
        Some(prior),
        next,
        result,
        false,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared transaction-bound envelope builder takes explicit prior/next graph and response-mode inputs"
)]
pub(crate) fn graph_success_with_observation<Clock: FnOnce() -> podway_core::UnixMillis>(
    transaction: &mut DirectTransactionV1<'_, '_, Clock>,
    resolved: &ResolvedWorkspaceV1,
    request_id: RequestIdV1,
    command: &str,
    prior: Option<&GraphSessionStateV2>,
    next: &GraphSessionStateV2,
    result: serde_json::Map<String, serde_json::Value>,
    return_observation: bool,
) -> Result<CanonicalExecutionJsonV1, DirectStoreErrorV1> {
    let now = transaction.now();
    let generated_at = Rfc3339MillisV1::from_unix_millis(now.get()).map_err(|_| {
        DirectStoreErrorV1::InvalidInput {
            reason: "clock exceeds the public timestamp range",
        }
    })?;
    #[cfg(test)]
    let fault = if return_observation {
        take_observation_fault_v1()
    } else {
        None
    };
    let (workspace_sequence, result) = if return_observation {
        if !matches!(
            command,
            "session.start"
                | "session.begin"
                | "item.record_many"
                | "session.complete"
                | "session.decide"
                | "goal.assess_criteria"
        ) {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "observation return is not supported for this command",
            });
        }
        let context = SuccessorObservationContextV1::from_transaction(transaction, next)?;
        let workspace_sequence = context.workspace_sequence();
        #[cfg(test)]
        if matches!(fault, Some(ObservationFaultV1::InconsistentState)) {
            return Err(Result::<(), crate::v2_read_service::GraphViewErrorV2>::Err(
                crate::v2_read_service::GraphViewErrorV2::InconsistentState(
                    "injected fault for rollback verification",
                ),
            )
            .map_err(|_| DirectStoreErrorV1::Unreadable)
            .unwrap_err());
        }
        #[cfg_attr(not(test), expect(unused_mut))]
        let mut observation = project_successor_observation_v1(&context)
            .map_err(|_| DirectStoreErrorV1::Unreadable)?;
        #[cfg(test)]
        if matches!(fault, Some(ObservationFaultV1::ExceedLookupHeadroom))
            && let Some(argv) = observation
                .get_mut("mutation_templates")
                .and_then(serde_json::Value::as_array_mut)
                .and_then(|templates| templates.first_mut())
                .and_then(|template| template.get_mut("argv"))
                .and_then(serde_json::Value::as_array_mut)
        {
            let block = serde_json::Value::String("\u{001f}".repeat(4000));
            for _ in 0..42 {
                argv.insert(2, block.clone());
            }
            argv.insert(2, serde_json::Value::String("\u{001f}".repeat(3000)));
        }
        let mut wrapped = serde_json::Map::with_capacity(4);
        wrapped.insert(
            "schema".into(),
            serde_json::json!(MUTATION_OBSERVATION_RESULT_SCHEMA_V1),
        );
        wrapped.insert(
            "workspace_sequence".into(),
            serde_json::json!(workspace_sequence),
        );
        wrapped.insert("mutation_result".into(), serde_json::Value::Object(result));
        wrapped.insert("observation".into(), serde_json::Value::Object(observation));
        (workspace_sequence, wrapped)
    } else {
        (transaction.next_workspace_sequence()?, result)
    };
    let output = OutputEnvelopeV4::new(OutputEnvelopeInputV4 {
        request_id,
        command: CommandNameV1::new(command).map_err(|_| DirectStoreErrorV1::InvalidInput {
            reason: "invalid direct command",
        })?,
        generated_at,
        workspace: Some(
            WorkspaceOutputV1::new(
                transaction.binding().identity().workspace_uuid().clone(),
                resolved
                    .worktree()
                    .roots()
                    .worktree_root()
                    .display()
                    .as_str(),
                workspace_sequence,
            )
            .map_err(|_| DirectStoreErrorV1::BindingMismatch)?,
        ),
        session: Some(DirectSessionOutputV1 {
            id: next.trace().session_id().clone(),
            title: next.task_title().into(),
            lifecycle: next.trace().lifecycle(),
            revision_before: prior.map_or(Revision::new(0), |prior| prior.trace().revision()),
            revision_after: next.trace().revision(),
        }),
        result,
        warnings: Vec::new(),
    })
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "direct success violates the canonical envelope",
    })?;
    if return_observation {
        #[cfg(test)]
        let mutated_depth_envelope = if matches!(fault, Some(ObservationFaultV1::ExceedLookupDepth))
        {
            let mut deep = serde_json::Value::Null;
            for _ in 0..63 {
                deep = serde_json::json!({ "a": deep });
            }
            let mut mutated = output.as_value().clone();
            mutated["result"]["extra_depth"] = deep;
            Some(mutated)
        } else {
            None
        };
        #[cfg(test)]
        let envelope_to_validate = mutated_depth_envelope
            .as_ref()
            .unwrap_or_else(|| output.as_value());
        #[cfg(not(test))]
        let envelope_to_validate = output.as_value();
        validate_mutation_observation_output_envelope_v1(envelope_to_validate).map_err(|err| {
            #[cfg(test)]
            record_last_observation_protocol_error_v1(err.clone());
            let _ = err;
            DirectStoreErrorV1::InvalidInput {
                reason: "mutation observation envelope exceeds the direct or lookup output bound",
            }
        })?;
    }
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(output.as_value()).map_err(|_| DirectStoreErrorV1::Unreadable)?,
    )
    .map_err(|_| DirectStoreErrorV1::InvalidInput {
        reason: "direct success exceeds the outcome bound",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::ffi::OsStrExt,
        path::Path,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use podway_core::{
        AttemptId, BlockerId, CriterionId, DelegationReceiptIdV1, GraphNodeId, ItemId, OptionId,
        ProcedureSnapshotId, Sha256Digest, UnixMillis, WorkspaceId,
    };
    use podway_git::{
        DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
        WorktreeSelectorV1,
    };
    use podway_protocol::{
        ContractGenerationV1, CriterionAssessmentManyEntryWireV1, GoalAssessCriteriaV2,
        GoalCriterionWireV2, InitialGoalWireV2, ItemRecordManyDispositionV1,
        ItemRecordManyOperationV1, ItemRecordManyV1, ItemRecordValueV1,
        ProcedureV2MutationCommandV1, SessionBeginV1, SessionCompleteV1, SessionDecideV2,
        SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1,
        SessionStartCommandV1, SessionStartSourceV1, SessionStartV1, SessionTerminalDispositionV1,
        TerminalDispositionInputV1,
    };
    use podway_store::IdempotencyKeyV1;
    use tempfile::TempDir;

    use crate::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::{DirectCursorCommandV1, DirectIdSourceV1},
        direct_start::DirectStartCommandV1,
        execution::{ExecutionClockV1, prepare_workspace_procedure_source_v2},
        workspace::DirectWorkspaceResolverV1,
    };

    const EQUIVALENT_PROCEDURE: &[u8] =
        include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.json");

    struct TestClock {
        millis: AtomicU64,
    }

    impl ExecutionClockV1 for TestClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(self.millis.fetch_add(1_000, Ordering::SeqCst))
        }
    }

    struct TestIds {
        next: AtomicU64,
    }

    impl TestIds {
        fn uuid(&self, prefix: u16) -> String {
            let value = self.next.fetch_add(1, Ordering::SeqCst);
            format!("{prefix:04x}0000-0000-4000-8000-{value:012x}")
        }
    }

    impl DirectIdSourceV1 for TestIds {
        fn next_workspace_id(&self) -> WorkspaceId {
            WorkspaceId::new(self.uuid(1)).unwrap()
        }
        fn next_session_id(&self) -> SessionId {
            SessionId::new(self.uuid(2)).unwrap()
        }
        fn next_attempt_id(&self) -> AttemptId {
            AttemptId::new(self.uuid(3)).unwrap()
        }
        fn next_blocker_id(&self) -> BlockerId {
            BlockerId::new(self.uuid(4)).unwrap()
        }
        fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
            ProcedureSnapshotId::new(self.uuid(5)).unwrap()
        }
        fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
            DelegationReceiptIdV1::new(self.uuid(6)).unwrap()
        }
    }

    struct FaultFixture {
        _temp: TempDir,
        root: std::path::PathBuf,
        selector: WorktreeSelectorV1,
        resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
        clock: TestClock,
        ids: TestIds,
        requests: AtomicU64,
        procedure_digest: Sha256Digest,
    }

    impl FaultFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            assert!(
                Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(temp.path())
                    .status()
                    .unwrap()
                    .success()
            );
            let root = fs::canonicalize(temp.path()).unwrap();
            fs::write(root.join("procedure.json"), EQUIVALENT_PROCEDURE).unwrap();
            let admitted =
                prepare_workspace_procedure_source_v2("procedure.json", EQUIVALENT_PROCEDURE)
                    .unwrap();
            let selector = WorktreeSelectorV1::new(
                WORKTREE_SELECTOR_VERSION_V1,
                None,
                LosslessPathV1::from_raw_bytes(
                    root.as_os_str().as_bytes(),
                    DiagnosticPathDisplayV1::new(root.to_string_lossy()).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            let manifest_digest = Sha256Digest::new(
                podway_protocol::build_identity_v1()
                    .contract_manifest_digest()
                    .to_owned(),
            )
            .unwrap();
            Self {
                _temp: temp,
                root,
                selector,
                resolver: DirectWorkspaceResolverV1::new(
                    NativeGitResolverV1::new(),
                    manifest_digest,
                ),
                clock: TestClock {
                    millis: AtomicU64::new(1_760_000_000_000),
                },
                ids: TestIds {
                    next: AtomicU64::new(1),
                },
                requests: AtomicU64::new(1),
                procedure_digest: admitted.digest().clone(),
            }
        }

        fn execute(
            &self,
            command: DirectCommandV1,
            key: Option<&str>,
            return_observation: bool,
        ) -> DirectResponseV1 {
            let req = self.requests.fetch_add(1, Ordering::SeqCst);
            execute_direct_v1(
                &self.resolver,
                DirectRequestV1 {
                    request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{req:012x}"))
                        .unwrap(),
                    selector: self.selector.clone(),
                    expected_workspace_id: None,
                    idempotency_key: key.map(|value| IdempotencyKeyV1::new(value).unwrap()),
                    command,
                    deadline: Instant::now() + Duration::from_secs(5),
                    observed_at: Rfc3339MillisV1::from_unix_millis(1_760_000_000_000).unwrap(),
                    timeout_ms: 5_000,
                    generation: ContractGenerationV1::Generation2,
                    return_observation,
                },
                &self.clock,
                &self.ids,
            )
            .unwrap()
        }
    }

    fn snapshot_sqlite_state(db_path: &Path) -> Vec<(String, Vec<String>)> {
        let conn = rusqlite::Connection::open(db_path).expect("open sqlite db");
        let mut table_stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap();
        let tables = table_stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>();
        tables
            .iter()
            .map(|table| {
                let sql = format!("SELECT quote(rowid), * FROM {table} ORDER BY rowid");
                let mut stmt = conn.prepare(&sql).unwrap();
                let col_count = stmt.column_count();
                let rows = stmt
                    .query_map([], |row| {
                        let mut parts = Vec::with_capacity(col_count);
                        for idx in 0..col_count {
                            let val: rusqlite::types::Value = row.get(idx)?;
                            parts.push(format!("{val:?}"));
                        }
                        Ok(parts.join("|"))
                    })
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect::<Vec<_>>();
                (table.clone(), rows)
            })
            .collect()
    }

    fn assert_faults_roll_back_and_key_reusable(
        fixture: &FaultFixture,
        db_path: &Path,
        command: DirectCommandV1,
        key: &str,
    ) -> OutputEnvelopeV4 {
        let before = snapshot_sqlite_state(db_path);

        // (a) GraphViewErrorV2::InconsistentState -> WORKSPACE_STATE_UNREADABLE (exit 5)
        NEXT_OBSERVATION_FAULT_V1.with(|slot| {
            slot.set(Some(ObservationFaultV1::InconsistentState));
        });
        match fixture.execute(command.clone(), Some(key), true) {
            DirectResponseV1::Failure(err) => {
                assert_eq!(err.as_value()["code"], "WORKSPACE_STATE_UNREADABLE");
                assert_eq!(err.as_value()["exit_code"], serde_json::json!(5));
            }
            DirectResponseV1::Success(out) => {
                panic!("expected WORKSPACE_STATE_UNREADABLE, got success: {out:?}");
            }
        }
        assert_eq!(
            snapshot_sqlite_state(db_path),
            before,
            "SQLite state must roll back completely on GraphViewErrorV2::InconsistentState for key {key}"
        );

        // (b1) validate_mutation_observation_output_envelope_v1 over-lookup-headroom -> REQUEST_INVALID (exit 2)
        let _ = take_last_observation_protocol_error_v1();
        NEXT_OBSERVATION_FAULT_V1.with(|slot| {
            slot.set(Some(ObservationFaultV1::ExceedLookupHeadroom));
        });
        match fixture.execute(command.clone(), Some(key), true) {
            DirectResponseV1::Failure(err) => {
                assert_eq!(err.as_value()["code"], "REQUEST_INVALID");
                assert_eq!(err.as_value()["exit_code"], serde_json::json!(2));
            }
            DirectResponseV1::Success(out) => {
                panic!(
                    "expected REQUEST_INVALID on lookup headroom overflow, got success: {out:?}"
                );
            }
        }
        assert!(
            matches!(
                take_last_observation_protocol_error_v1(),
                Some(podway_protocol::ProtocolError::ValueTooLong {
                    field: "outcome.lookup response",
                    maximum: podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1,
                    actual,
                }) if actual > podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1
            ),
            "headroom fault must fail with ProtocolError::ValueTooLong on outcome.lookup response before public mapping for key {key}"
        );
        assert_eq!(
            snapshot_sqlite_state(db_path),
            before,
            "SQLite state must roll back completely on lookup headroom overflow for key {key}"
        );

        // (b2) validate_mutation_observation_output_envelope_v1 over-depth -> REQUEST_INVALID (exit 2)
        let _ = take_last_observation_protocol_error_v1();
        NEXT_OBSERVATION_FAULT_V1.with(|slot| {
            slot.set(Some(ObservationFaultV1::ExceedLookupDepth));
        });
        match fixture.execute(command.clone(), Some(key), true) {
            DirectResponseV1::Failure(err) => {
                assert_eq!(err.as_value()["code"], "REQUEST_INVALID");
                assert_eq!(err.as_value()["exit_code"], serde_json::json!(2));
            }
            DirectResponseV1::Success(out) => {
                panic!("expected REQUEST_INVALID on depth overflow, got success: {out:?}");
            }
        }
        assert_eq!(
            take_last_observation_protocol_error_v1(),
            Some(podway_protocol::ProtocolError::JsonDepthExceeded {
                maximum: podway_protocol::MAX_JSON_DEPTH_V1,
            }),
            "depth fault must fail with depth-specific ProtocolError::JsonDepthExceeded before public mapping for key {key}"
        );
        assert_eq!(
            snapshot_sqlite_state(db_path),
            before,
            "SQLite state must roll back completely on depth overflow for key {key}"
        );

        // Key remains unbound and succeeds when retried without fault.
        match fixture.execute(command, Some(key), true) {
            DirectResponseV1::Success(out) => out,
            DirectResponseV1::Failure(err) => {
                panic!(
                    "expected retry with same key {key} to succeed after rollback, got: {err:?}"
                );
            }
        }
    }

    #[test]
    fn agent005_observation_faults_and_bounds_roll_back_and_preserve_idempotency_keys_across_all_routes()
     {
        let fixture = FaultFixture::new();
        match fixture.execute(
            DirectCommandV1::WorkspaceInit { repair: false },
            Some("init-1"),
            false,
        ) {
            DirectResponseV1::Success(_) => {}
            DirectResponseV1::Failure(err) => panic!("init failed: {err:?}"),
        }
        let db_path = fixture.root.join(".podway/runtime/state.sqlite3");

        // 1. Plain session.start (initial)
        let start_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
                preconditions: None,
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "procedure.json".to_owned(),
                    },
                    expected_procedure_digest: Some(fixture.procedure_digest.clone()),
                    task_title: "Initial task".to_owned(),
                    dry_run: false,
                },
            })),
            "start-key-1",
        );
        let session_id = SessionId::new(
            start_out.as_value()["result"]["mutation_result"]["session_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();

        // 2. session.begin
        let begin_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: session_id.clone(),
                        expected_session_revision: Revision::new(0),
                    },
                    initial_goal: Some(InitialGoalWireV2 {
                        goal: "Verify rollback on all routes.".to_owned(),
                        criteria: vec![GoalCriterionWireV2 {
                            criterion_id: CriterionId::new("g1").unwrap(),
                            statement: "All routes roll back cleanly.".to_owned(),
                        }],
                        actor: Some("tester".to_owned()),
                    }),
                },
            )),
            "begin-key-1",
        );
        let perform_attempt_id = AttemptId::new(
            begin_out.as_value()["result"]["mutation_result"]["attempt"]["attempt_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();

        // 3. item.record_many (at perform)
        let record_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::RecordMany(ItemRecordManyV1 {
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(1),
                    expected_attempt_id: perform_attempt_id.clone(),
                },
                operations: vec![ItemRecordManyOperationV1 {
                    item_id: ItemId::new("result").unwrap(),
                    expected_item_revision: Revision::new(0),
                    disposition: ItemRecordManyDispositionV1::Record {
                        record: ItemRecordValueV1::Text {
                            value: "recorded result".to_owned(),
                        },
                    },
                }],
            }),
            "record-many-key-1",
        );
        assert_eq!(
            record_out.as_value()["result"]["mutation_result"]["revision"],
            serde_json::json!(2)
        );

        // 4. session.complete (perform -> decide)
        let complete_perform_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(2),
                    expected_attempt_id: perform_attempt_id,
                },
            })),
            "complete-perform-key-1",
        );
        let decide_attempt_id = AttemptId::new(
            complete_perform_out.as_value()["result"]["mutation_result"]["to_attempt_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();

        // 5. goal.assess_criteria (at decide)
        let assess_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriteria(
                GoalAssessCriteriaV2 {
                    preconditions: SessionMutationPreconditionsWireV1 {
                        expected_session_id: session_id.clone(),
                        expected_session_revision: Revision::new(3),
                        expected_attempt_id: decide_attempt_id.clone(),
                    },
                    expected_goal_revision: 1,
                    actor: Some("reviewer".to_owned()),
                    assessments: vec![CriterionAssessmentManyEntryWireV1 {
                        criterion_id: CriterionId::new("g1").unwrap(),
                        status: "satisfied".to_owned(),
                        reason: "Verified rollback.".to_owned(),
                        evidence: vec![GraphNodeId::new("perform").unwrap()],
                        items: vec![],
                    }],
                },
            )),
            "assess-criteria-key-1",
        );
        assert_eq!(
            assess_out.as_value()["result"]["mutation_result"]["revision"],
            serde_json::json!(4)
        );

        // 6. session.decide (decide -> finish)
        let decide_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
                SessionDecideV2 {
                    preconditions: SessionMutationPreconditionsWireV1 {
                        expected_session_id: session_id.clone(),
                        expected_session_revision: Revision::new(4),
                        expected_attempt_id: decide_attempt_id,
                    },
                    expected_goal_revision: Some(1),
                    option_id: OptionId::new("achieved").unwrap(),
                    reason: "Proceed to finish.".to_owned(),
                    actor: Some("reviewer".to_owned()),
                },
            )),
            "decide-key-1",
        );
        let finish_attempt_id = AttemptId::new(
            decide_out.as_value()["result"]["mutation_result"]["target_attempt_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();

        // Satisfy finish node's required item before completing finish.
        match fixture.execute(
            DirectCommandV1::RecordMany(ItemRecordManyV1 {
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(5),
                    expected_attempt_id: finish_attempt_id.clone(),
                },
                operations: vec![ItemRecordManyOperationV1 {
                    item_id: ItemId::new("result").unwrap(),
                    expected_item_revision: Revision::new(0),
                    disposition: ItemRecordManyDispositionV1::Record {
                        record: ItemRecordValueV1::Text {
                            value: "final result".to_owned(),
                        },
                    },
                }],
            }),
            Some("record-finish-key-1"),
            false,
        ) {
            DirectResponseV1::Success(_) => {}
            DirectResponseV1::Failure(err) => panic!("record finish item failed: {err:?}"),
        }

        // 7. session.complete (finish -> completed terminal state)
        let complete_finish_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: SessionMutationPreconditionsWireV1 {
                    expected_session_id: session_id.clone(),
                    expected_session_revision: Revision::new(6),
                    expected_attempt_id: finish_attempt_id,
                },
            })),
            "complete-finish-key-1",
        );
        assert_eq!(
            complete_finish_out.as_value()["result"]["mutation_result"]["session_state"],
            serde_json::json!("completed")
        );

        // Record terminal disposition so plain session.start exercises the disposed-terminal archive_graph path.
        match fixture.execute(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                SessionTerminalDispositionV1 {
                    preconditions: SessionIdentityPreconditionsWireV1 {
                        expected_session_id: session_id.clone(),
                        expected_session_revision: Revision::new(7),
                    },
                    disposition: TerminalDispositionInputV1::NotRequired {
                        reason: "Completed fault test.".to_owned(),
                    },
                    actor: Some("tester".to_owned()),
                },
            )),
            Some("dispose-key-1"),
            false,
        ) {
            DirectResponseV1::Success(_) => {}
            DirectResponseV1::Failure(err) => panic!("disposition failed: {err:?}"),
        }

        // 8. Disposed-terminal plain session.start (admits before archive_graph / create_graph / scope_created_session_outcome)
        let restart_out = assert_faults_roll_back_and_key_reusable(
            &fixture,
            &db_path,
            DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
                preconditions: Some(SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session_id,
                    expected_session_revision: Revision::new(7),
                }),
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "procedure.json".to_owned(),
                    },
                    expected_procedure_digest: Some(fixture.procedure_digest.clone()),
                    task_title: "Second task after disposed terminal".to_owned(),
                    dry_run: false,
                },
            })),
            "start-disposed-terminal-key-1",
        );
        assert_eq!(
            restart_out.as_value()["session"]["revision_before"],
            serde_json::json!(7)
        );
        assert_eq!(
            restart_out.as_value()["session"]["revision_after"],
            serde_json::json!(0)
        );
    }
}
