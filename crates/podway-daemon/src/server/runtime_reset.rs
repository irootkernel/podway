use std::{
    io,
    os::unix::net::UnixStream,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use podway_core::UnixMillis;
use podway_protocol::{
    ErrorCodeV1, ErrorEnvelopeInputV1, ErrorEnvelopeV1, ExitCodeV1, FrameIoPhaseV1, OperationV1,
    OutputEnvelopeInputV3, OutputEnvelopeV3, RequestEnvelopeV1, ResponseEnvelopeV2,
    build_identity_v1, decode_request_payload_v1,
};
use podway_service::{
    PodwayHomeV1, RuntimeResetLockV1, RuntimeResetOperationV1, RuntimeResetPathV1,
    RuntimeResetReasonV1, RuntimeResetRecordViewV1, ServiceRuntimePathsV1,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    DaemonProcessIdentityV1, DaemonReadinessStateV1, DaemonReadinessV1, PeerCredentialSourceV1,
    RequestContextV1, RequestDispatcherV1, ResponseMetadataSourceV1, ServerConnectionErrorV1,
    ShutdownAdmissionV1, TransportErrorKindV1, UnixServerTransportV1, classify_frame_error,
    classify_payload_error, recover_request_context,
};
use crate::runtime_workspace::{RuntimeResetBackgroundFenceV1, WorkspaceRuntimeManagerV1};

#[derive(Debug)]
pub(super) struct ReservationV1 {
    operation: RuntimeResetOperationV1,
    id: String,
    connected: bool,
    _background: RuntimeResetBackgroundFenceV1,
}

pub(crate) struct RuntimeResetControllerV1 {
    home: Option<PodwayHomeV1>,
    root: Value,
    process: DaemonProcessIdentityV1,
    manager: Arc<WorkspaceRuntimeManagerV1>,
    admission: ShutdownAdmissionV1,
    readiness: DaemonReadinessV1,
}

pub(super) struct ControlOutcomeV1 {
    pub result: Map<String, Value>,
    pub retain_connection: bool,
    pub shutdown: bool,
}

pub(super) struct ControlFailureV1 {
    pub code: &'static str,
    pub reason: RuntimeResetReasonV1,
}

impl ControlFailureV1 {
    fn unsafe_state(reason: RuntimeResetReasonV1) -> Self {
        Self {
            code: "RUNTIME_RESET_UNSAFE",
            reason,
        }
    }

    fn lost() -> Self {
        Self::unsafe_state(RuntimeResetReasonV1::ReservationLost)
    }
}

impl RuntimeResetControllerV1 {
    pub(crate) fn new(
        paths: &ServiceRuntimePathsV1,
        process: DaemonProcessIdentityV1,
        manager: Arc<WorkspaceRuntimeManagerV1>,
        admission: ShutdownAdmissionV1,
        readiness: DaemonReadinessV1,
    ) -> Result<Self, podway_service::RuntimeResetErrorV1> {
        let home = paths.ordinary_account_home().map_err(|_| {
            podway_service::RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::UnsafePath)
        })?;
        let root = serde_json::to_value(RuntimeResetPathV1::new(
            paths
                .podway_home()
                .unwrap_or(paths.runtime_directory())
                .as_path(),
        )?)
        .map_err(|_| {
            podway_service::RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::Io)
        })?;
        Ok(Self {
            home,
            root,
            process,
            manager,
            admission,
            readiness,
        })
    }

    pub(super) fn normal_admission_open(&self) -> bool {
        self.admission
            .lock_state()
            .is_ok_and(|state| state.reservation.is_none())
    }

    fn record(&self) -> Result<Option<RuntimeResetRecordViewV1>, ControlFailureV1> {
        RuntimeResetRecordViewV1::read(
            self.home.as_ref().ok_or_else(|| {
                ControlFailureV1::unsafe_state(RuntimeResetReasonV1::ManagedRuntime)
            })?,
        )
        .map_err(|error| ControlFailureV1::unsafe_state(error.reason))
    }

    fn committed(&self, reservation: &ReservationV1) -> Result<Option<bool>, ControlFailureV1> {
        let Some(record) = self.record()? else {
            return Ok(None);
        };
        let Some(participant) = record.participant(self.process.mode()) else {
            return Ok(None);
        };
        if record.operation != reservation.operation
            || participant.reservation_id.as_deref() != Some(reservation.id.as_str())
            || serde_json::to_value(&participant.root).ok().as_ref() != Some(&self.root)
            || !participant.process.as_ref().is_some_and(|process| {
                process.process_id == self.process.process_id().as_str()
                    && process.pid == self.process.pid()
            })
        {
            return Err(ControlFailureV1::unsafe_state(
                RuntimeResetReasonV1::IdentityChanged,
            ));
        }
        Ok(Some(participant.stop_intended))
    }

    fn idle_fence(&self) -> Result<RuntimeResetBackgroundFenceV1, RuntimeResetReasonV1> {
        let readiness = self.readiness.snapshot();
        if readiness.state() != DaemonReadinessStateV1::Ready || readiness.worktrees().failed() != 0
        {
            return Err(RuntimeResetReasonV1::Recovery);
        }
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .map(UnixMillis::new)
            .ok_or(RuntimeResetReasonV1::UnknownActivity)?;
        self.manager.reserve_runtime_reset(millis)
    }

    fn result(
        &self,
        state: &str,
        reservation: Option<&ReservationV1>,
        reason: Option<RuntimeResetReasonV1>,
        retain_connection: bool,
    ) -> ControlOutcomeV1 {
        ControlOutcomeV1 {
            result: serde_json::json!({
                "schema": "podway.runtime-reset-control-result/v1",
                "state": state,
                "mode": self.process.mode(),
                "namespace_root": self.root,
                "process_id": self.process.process_id(),
                "operation_id": reservation.map(|value| &value.operation.operation_id),
                "token_sha256": reservation.map(|value| &value.operation.token_sha256),
                "reservation_id": reservation.map(|value| &value.id),
                "reason": reason,
            })
            .as_object()
            .expect("control result is an object")
            .clone(),
            retain_connection,
            shutdown: state == "shutting_down",
        }
    }

    pub(super) fn execute(
        &self,
        request: &RequestEnvelopeV1,
        connection_reservation: Option<&str>,
    ) -> Result<ControlOutcomeV1, ControlFailureV1> {
        let input = request.payload();
        if input.get("namespace_root") != Some(&self.root)
            || input.get("mode").and_then(Value::as_str) != Some(self.process.mode().as_str())
            || input.get("expected_process_id").and_then(Value::as_str)
                != Some(self.process.process_id().as_str())
        {
            return Err(ControlFailureV1::unsafe_state(
                RuntimeResetReasonV1::IdentityChanged,
            ));
        }
        if self.home.is_none() {
            return Ok(self.result(
                "unsupported",
                None,
                Some(RuntimeResetReasonV1::ManagedRuntime),
                false,
            ));
        }
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(ControlFailureV1::lost)?;
        // Release and shutdown serialize with the coordinator's durable commit. Snapshot and
        // reserve never retake this lock: the coordinator may hold it while awaiting snapshots.
        let _commit = if matches!(action, "release" | "shutdown") {
            Some(
                RuntimeResetLockV1::acquire_commit(self.home.as_ref().expect("ordinary runtime"))
                    .map_err(|error| ControlFailureV1::unsafe_state(error.reason))?,
            )
        } else {
            None
        };
        let mut state = self
            .admission
            .lock_state()
            .map_err(|_| ControlFailureV1::unsafe_state(RuntimeResetReasonV1::UnknownActivity))?;
        if matches!(action, "inspect" | "reserve") {
            if connection_reservation.is_some() {
                return Err(ControlFailureV1::lost());
            }
            if !state.accepting || state.in_flight != 1 {
                return Ok(self.result("busy", None, Some(RuntimeResetReasonV1::Activity), false));
            }
            if action == "inspect" {
                if state.reservation.is_some() {
                    return Ok(self.result(
                        "busy",
                        None,
                        Some(RuntimeResetReasonV1::OperationInProgress),
                        false,
                    ));
                }
                return Ok(match self.idle_fence() {
                    Ok(_fence) => self.result("idle", None, None, false),
                    Err(reason) => self.result("busy", None, Some(reason), false),
                });
            }
            let operation = RuntimeResetOperationV1 {
                operation_id: input["operation_id"]
                    .as_str()
                    .ok_or_else(ControlFailureV1::lost)?
                    .to_owned(),
                token_sha256: input["token_sha256"]
                    .as_str()
                    .ok_or_else(ControlFailureV1::lost)?
                    .to_owned(),
            };
            if let Some(reservation) = state.reservation.as_mut() {
                if reservation.connected
                    || reservation.operation != operation
                    || self.committed(reservation)?.is_none()
                {
                    return Err(ControlFailureV1::lost());
                }
                reservation.connected = true;
                return Ok(self.result("committed", Some(reservation), None, true));
            }
            if self
                .record()?
                .is_some_and(|record| record.participant(self.process.mode()).is_some())
            {
                return Err(ControlFailureV1::unsafe_state(
                    RuntimeResetReasonV1::IdentityChanged,
                ));
            }
            let background = match self.idle_fence() {
                Ok(fence) => fence,
                Err(reason) => return Ok(self.result("busy", None, Some(reason), false)),
            };
            let reservation = ReservationV1 {
                operation,
                id: Uuid::new_v4().to_string(),
                connected: true,
                _background: background,
            };
            let result = self.result("reserved", Some(&reservation), None, true);
            state.reservation = Some(reservation);
            return Ok(result);
        }
        let reservation = state
            .reservation
            .as_mut()
            .ok_or_else(ControlFailureV1::lost)?;
        if !reservation.connected
            || connection_reservation != Some(reservation.id.as_str())
            || input["reservation_id"].as_str() != Some(reservation.id.as_str())
            || input["operation_id"].as_str() != Some(reservation.operation.operation_id.as_str())
            || input["token_sha256"].as_str() != Some(reservation.operation.token_sha256.as_str())
        {
            return Err(ControlFailureV1::lost());
        }
        let committed = self.committed(reservation)?;
        match action {
            "snapshot" => Ok(self.result(
                if committed.is_some() {
                    "committed"
                } else {
                    "reserved"
                },
                Some(reservation),
                None,
                true,
            )),
            "release" => {
                let result = self.result(
                    if committed.is_some() {
                        "committed"
                    } else {
                        "released"
                    },
                    Some(reservation),
                    None,
                    false,
                );
                if committed.is_none() {
                    state.reservation = None;
                } else {
                    reservation.connected = false;
                }
                Ok(result)
            }
            "shutdown" if committed == Some(true) => {
                let result = self.result("shutting_down", Some(reservation), None, false);
                reservation.connected = false;
                Ok(result)
            }
            "shutdown" => Err(ControlFailureV1::unsafe_state(
                RuntimeResetReasonV1::MissingIntent,
            )),
            _ => Err(ControlFailureV1::lost()),
        }
    }

    fn disconnect(&self, reservation_id: &str) {
        let commit = RuntimeResetLockV1::acquire_commit(
            self.home.as_ref().expect("reserved ordinary runtime"),
        );
        let Ok(mut state) = self.admission.lock_state() else {
            return;
        };
        let Some(reservation) = state.reservation.as_mut() else {
            return;
        };
        if reservation.id != reservation_id {
            return;
        }
        reservation.connected = false;
        // An unreadable record or unavailable interlock cannot authorize reopening admission.
        if commit.is_ok() && matches!(self.committed(reservation), Ok(None)) {
            state.reservation = None;
        }
    }
}

pub(super) struct ControlConnectionV1<'a> {
    pub controller: &'a RuntimeResetControllerV1,
    pub reservation_id: Option<String>,
}

impl Drop for ControlConnectionV1<'_> {
    fn drop(&mut self) {
        if let Some(id) = &self.reservation_id {
            self.controller.disconnect(id);
        }
    }
}

impl<Source, Dispatcher, Metadata> UnixServerTransportV1<Source, Dispatcher, Metadata>
where
    Source: PeerCredentialSourceV1,
    Dispatcher: RequestDispatcherV1,
    Metadata: ResponseMetadataSourceV1,
{
    pub(super) fn runtime_reset_error_response(
        &self,
        request: &RequestEnvelopeV1,
        code: &str,
        reason: RuntimeResetReasonV1,
    ) -> Result<ResponseEnvelopeV2, ServerConnectionErrorV1> {
        let code = ErrorCodeV1::new(code)
            .map_err(|_| ServerConnectionErrorV1::InvalidDispatcherResponse)?;
        let mut details = serde_json::json!({
            "schema": "podway.runtime-reset-error-details/v1",
            "mode": self.process_identity.as_ref().map(DaemonProcessIdentityV1::mode),
            "reason": reason,
            "result": null,
            "retry": null,
        })
        .as_object()
        .expect("reset details are an object")
        .clone();
        let in_progress = code.as_str() == "RUNTIME_RESET_IN_PROGRESS";
        let admission_closed = code.as_str() == "DAEMON_SHUTTING_DOWN";
        if admission_closed {
            details = serde_json::json!({ "schema": "podway.endpoint-error-details/v1" })
                .as_object()
                .unwrap()
                .clone();
            if matches!(
                request.operation(),
                OperationV1::Mutate | OperationV1::Bootstrap
            ) {
                details.insert(
                    "admission".to_owned(),
                    serde_json::json!({ "admitted": false }),
                );
            }
        }
        let exit_code = if in_progress {
            4
        } else if admission_closed || code.as_str() == "RUNTIME_RESET_UNSUPPORTED" {
            3
        } else {
            5
        };
        ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
            request_id: request.request_id().clone(),
            command: request.command().clone(),
            generated_at: self
                .metadata
                .try_generated_at()
                .map_err(ServerConnectionErrorV1::ResponseMetadata)?,
            code,
            message: if admission_closed {
                "Daemon is draining and not accepting work."
            } else if reason == RuntimeResetReasonV1::MissingIntent {
                "Runtime reset shutdown requires durable stop intent."
            } else {
                "Runtime reset control could not be admitted."
            }
            .to_owned(),
            retryable: in_progress || admission_closed,
            exit_code: ExitCodeV1::new(exit_code).expect("catalog exit code"),
            workspace: None,
            details,
        })
        .map(ResponseEnvelopeV2::Error)
        .map_err(|_| ServerConnectionErrorV1::InvalidDispatcherResponse)
    }

    pub(super) fn handle_runtime_reset(
        &self,
        mut connection: UnixStream,
        mut request: RequestEnvelopeV1,
    ) -> Result<(), ServerConnectionErrorV1> {
        let Some(controller) = &self.runtime_reset else {
            let response = self.runtime_reset_error_response(
                &request,
                "RUNTIME_RESET_UNSUPPORTED",
                RuntimeResetReasonV1::UnsupportedPeer,
            )?;
            return self.write_response(&mut connection, &response);
        };
        connection
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(ServerConnectionErrorV1::ConfigureReadTimeout)?;
        connection
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(ServerConnectionErrorV1::ConfigureWriteTimeout)?;
        let mut guard = ControlConnectionV1 {
            controller,
            reservation_id: None,
        };
        for pair in 0..8 {
            let identity = build_identity_v1();
            if request.client().product() != identity.product()
                || request.client().contract_manifest_digest()
                    != identity.contract_manifest_digest()
            {
                let response = self.contract_mismatch_response(&request)?;
                return self.write_response(&mut connection, &response);
            }
            if !podway_protocol::validate_runtime_reset_control_request_v1(&request)
                || has_pipelined_bytes(&connection)?
            {
                return self.write_transport_error(
                    &mut connection,
                    Some(RequestContextV1::from_request(&request)),
                    TransportErrorKindV1::InvalidRequest,
                );
            }
            let outcome = match controller.execute(&request, guard.reservation_id.as_deref()) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let response =
                        self.runtime_reset_error_response(&request, error.code, error.reason)?;
                    return self.write_response(&mut connection, &response);
                }
            };
            if outcome.retain_connection {
                guard.reservation_id = outcome.result["reservation_id"].as_str().map(str::to_owned);
            }
            let response = OutputEnvelopeV3::new(OutputEnvelopeInputV3 {
                request_id: request.request_id().clone(),
                command: request.command().clone(),
                generated_at: self
                    .metadata
                    .try_generated_at()
                    .map_err(ServerConnectionErrorV1::ResponseMetadata)?,
                workspace: None,
                job: None,
                session: None,
                result: outcome.result,
                warnings: Vec::new(),
            })
            .map(ResponseEnvelopeV2::OutputV2)
            .map_err(|_| ServerConnectionErrorV1::InvalidDispatcherResponse)?;
            let written = self.write_response(&mut connection, &response);
            if outcome.shutdown {
                controller.admission.request_shutdown();
            }
            written?;
            if !outcome.retain_connection || pair == 7 {
                return Ok(());
            }
            let payload = match podway_protocol::read_frame_v1(&mut connection) {
                Ok(Some(payload)) => payload,
                Ok(None) => return Ok(()),
                Err(error) => {
                    self.write_transport_error(
                        &mut connection,
                        None,
                        classify_frame_error(&error),
                    )?;
                    return Ok(());
                }
            };
            request = match decode_request_payload_v1(&payload) {
                Ok(request) => request,
                Err(error) => {
                    return self.write_transport_error(
                        &mut connection,
                        recover_request_context(&payload),
                        classify_payload_error(&error),
                    );
                }
            };
        }
        Ok(())
    }
}

fn has_pipelined_bytes(connection: &UnixStream) -> Result<bool, ServerConnectionErrorV1> {
    use nix::{
        errno::Errno,
        sys::socket::{MsgFlags, recv},
    };
    use std::os::fd::AsRawFd;
    match recv(
        connection.as_raw_fd(),
        &mut [0_u8; 1],
        MsgFlags::MSG_PEEK | MsgFlags::MSG_DONTWAIT,
    ) {
        Ok(length) => Ok(length != 0),
        Err(Errno::EAGAIN) => Ok(false),
        Err(error) => Err(ServerConnectionErrorV1::RequestFrameIo {
            phase: FrameIoPhaseV1::Payload,
            kind: io::Error::from_raw_os_error(error as i32).kind(),
        }),
    }
}

#[cfg(test)]
mod tests;
