use std::{env, io, time::SystemTime};

use nix::unistd::geteuid;
use podway_cli::client::{DaemonClientErrorV1, DaemonClientIoOperationV1, DaemonClientV1};
use podway_protocol::{
    CommandNameV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, validate_command_result_v2,
};
use podway_service::{
    PodwayHomeV1, RuntimeResetErrorV1, RuntimeResetInspectorV1, RuntimeResetPathV1,
    RuntimeResetPeerV1, RuntimeResetPlanV1, RuntimeResetPlannerV1, RuntimeResetProcessV1,
    RuntimeResetReasonV1, RuntimeResetSelectionV1, ServiceClockV1, ServiceRuntimePathsV1,
};
use serde::Serialize;
use serde_json::json;

use super::{
    Cli, LocalFailure, RunResult, build_daemon_status_request, local_result_v2,
    system_launchctl_runner, system_service_clock, validated_live_daemon_status,
};

pub(super) fn execute_plan(cli: &Cli, all_modes: bool) -> Result<RunResult, LocalFailure> {
    let explicit_mode = cli.mode.is_some() || cli.dev;
    if all_modes == explicit_mode || env::var_os("PODWAY_DEV_HOME").is_some() {
        return Err(LocalFailure::request_invalid(
            "runtime reset requires exactly one explicit --mode, --dev, or --all-modes selector and no custom runtime root",
        ));
    }
    let selection = if all_modes {
        RuntimeResetSelectionV1::AllModes
    } else {
        RuntimeResetSelectionV1::Mode {
            mode: cli.runtime_mode()?,
        }
    };
    let home = account_home()?;
    let mut planner = RuntimeResetPlannerV1::new(home, system_launchctl_runner(), PeerInspector);
    let plan = planner
        .plan(
            selection,
            system_service_clock(SystemTime::now(), "runtime.reset.plan")?.now(),
        )
        .map_err(map_reset_error)?;
    let public_plan = PlanResult::new(&plan)?;
    let result = serde_json::to_value(&public_plan)
        .map_err(|_| LocalFailure::response_invalid("runtime reset plan could not be encoded"))?
        .as_object()
        .cloned()
        .ok_or_else(|| LocalFailure::response_invalid("runtime reset plan must be an object"))?;
    validate_command_result_v2("runtime.reset.plan", &result).map_err(|_| {
        LocalFailure::response_invalid("runtime reset plan violated its result contract")
    })?;
    let mut lines = vec![format!(
        "Runtime reset plan: {}",
        public_enum_label(&plan.status)
    )];
    for target in &plan.targets {
        lines.push(format!(
            "{}: {} ({})",
            target.mode,
            target.root.display,
            public_enum_label(&target.state)
        ));
        for resource in &target.resources {
            lines.push(format!(
                "  remove {}: {}",
                public_enum_label(&resource.class),
                resource.path.display
            ));
        }
        if let Some(reason) = &target.reason {
            lines.push(format!("  blocked: {}", public_enum_label(reason)));
        }
    }
    for resource in &plan.preserved {
        lines.push(format!(
            "preserve {}: {}",
            public_enum_label(&resource.class),
            resource.path.display
        ));
    }
    for excluded in &plan.excluded {
        lines.push(format!(
            "exclude: {}",
            excluded
                .path
                .as_ref()
                .map(|path| path.display.as_str())
                .unwrap_or("external managed runtimes")
        ));
    }
    if let Some(token) = &plan.plan_token {
        lines.push(format!(
            "Expires: {}",
            public_plan
                .expires_at
                .as_ref()
                .expect("token expiry")
                .as_str()
        ));
        lines.push(format!("Plan token: {token}"));
    }
    if let Some(operation) = &plan.recovery_operation {
        lines.push(format!("Recovery required: {}", operation.operation_id));
    }
    Ok(local_result_v2(
        "runtime.reset.plan",
        result,
        lines.join("\n"),
        0,
    ))
}

fn public_enum_label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("runtime reset enums serialize")
        .as_str()
        .expect("runtime reset enums use string labels")
        .to_owned()
}

// The CLI owns the public result envelope; the service plan remains timestamp-neutral.
#[derive(Serialize)]
struct PlanResult<'a> {
    schema: &'static str,
    #[serde(flatten)]
    plan: &'a RuntimeResetPlanV1,
    expires_at: Option<Rfc3339MillisV1>,
}

impl<'a> PlanResult<'a> {
    fn new(plan: &'a RuntimeResetPlanV1) -> Result<Self, LocalFailure> {
        let expires_at = plan
            .expires_at_ms
            .map(Rfc3339MillisV1::from_unix_millis)
            .transpose()
            .map_err(|_| LocalFailure::response_invalid("runtime reset expiry is out of range"))?;
        Ok(Self {
            schema: "podway.runtime-reset-plan-result/v1",
            plan,
            expires_at,
        })
    }
}

fn account_home() -> Result<PodwayHomeV1, LocalFailure> {
    #[cfg(debug_assertions)]
    if let Some(path) = env::var_os("PODWAY_TEST_ACCOUNT_ROOT") {
        return PodwayHomeV1::from_account_home(path, geteuid().as_raw()).map_err(|_| {
            map_reset_error(RuntimeResetErrorV1::unsafe_reason(
                RuntimeResetReasonV1::UnsafePath,
            ))
        });
    }
    PodwayHomeV1::for_effective_user().map_err(|_| {
        map_reset_error(RuntimeResetErrorV1::unsafe_reason(
            RuntimeResetReasonV1::UnsafePath,
        ))
    })
}

fn map_reset_error(error: RuntimeResetErrorV1) -> LocalFailure {
    let mut failure = LocalFailure::catalog(
        error.code(),
        "runtime reset planning failed",
        "runtime.reset.plan",
    );
    failure.details = json!({
        "schema": "podway.runtime-reset-error-details/v1",
        "mode": error.mode,
        "reason": error.reason,
        "result": null,
        "retry": null,
    })
    .as_object()
    .expect("reset error details are an object")
    .clone();
    failure
}

struct PeerInspector;
impl RuntimeResetInspectorV1 for PeerInspector {
    fn inspect(
        &mut self,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<RuntimeResetPeerV1, RuntimeResetErrorV1> {
        let request = build_daemon_status_request()
            .map_err(|_| RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::Io))?;
        let client = DaemonClientV1::new(paths.clone());
        let output = match client.daemon_status(&request) {
            Ok(ResponseEnvelopeV2::OutputV2(output)) => output,
            Ok(_) => return Ok(RuntimeResetPeerV1::Unsupported),
            Err(error) => return classify_peer_error(error),
        };
        let status = match validated_live_daemon_status(&output, None, paths, None) {
            Ok(status) => status,
            Err(_) => return Ok(RuntimeResetPeerV1::Unsupported),
        };
        let invalid = || RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::UnknownActivity);
        let field = |name: &str| {
            status
                .get(name)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid)
        };
        let process = RuntimeResetProcessV1 {
            pid: status["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or_else(invalid)?,
            process_id: field("process_id")?.to_owned(),
            executable: RuntimeResetPathV1::new(std::path::Path::new(field("executable_path")?))?,
            started_at_ms: Rfc3339MillisV1::new(field("started_at")?)
                .ok()
                .and_then(|timestamp| timestamp.to_unix_millis())
                .ok_or_else(invalid)?,
        };
        let root = RuntimeResetPathV1::new(paths.podway_home().ok_or_else(invalid)?.as_path())?;
        let payload = json!({
            "schema": "podway.runtime-reset-control-input/v1",
            "action": "inspect",
            "mode": paths.mode(),
            "namespace_root": root,
            "expected_process_id": process.process_id,
            "operation_id": null,
            "token_sha256": null,
            "reservation_id": null,
        })
        .as_object()
        .expect("control payload is an object")
        .clone();
        let inspection = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
            request_id: RequestIdV1::new(uuid::Uuid::new_v4().to_string())
                .map_err(|_| invalid())?,
            client: request.client().clone(),
            operation: request.operation(),
            command: CommandNameV1::new("daemon.runtime_reset").map_err(|_| invalid())?,
            workspace: None,
            idempotency_key: None,
            preconditions: request.preconditions().clone(),
            options: request.options(),
            payload,
        })
        .map_err(|_| invalid())?;
        let output = match client.runtime_reset_inspect(&inspection) {
            Ok(ResponseEnvelopeV2::OutputV2(output)) => output,
            Ok(ResponseEnvelopeV2::Error(error))
                if matches!(
                    error.code().as_str(),
                    "RUNTIME_RESET_UNSUPPORTED" | "REQUEST_INVALID"
                ) =>
            {
                return Ok(RuntimeResetPeerV1::Unsupported);
            }
            Ok(ResponseEnvelopeV2::Error(error))
                if error.code().as_str() == "RUNTIME_RESET_UNSAFE" =>
            {
                let reason = serde_json::from_value(error.details()["reason"].clone())
                    .map_err(|_| invalid())?;
                return Err(RuntimeResetErrorV1::unsafe_reason(reason));
            }
            _ => return Err(invalid()),
        };
        let result = output.result();
        if result.get("namespace_root") != Some(&json!(root))
            || result.get("mode") != Some(&json!(paths.mode()))
            || result.get("process_id").and_then(serde_json::Value::as_str)
                != Some(process.process_id.as_str())
        {
            return Err(RuntimeResetErrorV1::unsafe_reason(
                RuntimeResetReasonV1::IdentityChanged,
            ));
        }
        let busy_reason = match result.get("state").and_then(serde_json::Value::as_str) {
            Some("unsupported") => return Ok(RuntimeResetPeerV1::Unsupported),
            Some("idle") if result["reason"].is_null() => None,
            Some("busy") => {
                Some(serde_json::from_value(result["reason"].clone()).map_err(|_| invalid())?)
            }
            _ => return Err(invalid()),
        };
        Ok(RuntimeResetPeerV1::Live {
            process,
            busy_reason,
        })
    }
}

fn classify_peer_error(
    error: DaemonClientErrorV1,
) -> Result<RuntimeResetPeerV1, RuntimeResetErrorV1> {
    match error {
        DaemonClientErrorV1::Connection {
            operation: DaemonClientIoOperationV1::Connect,
            source,
        } if matches!(
            source.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        ) =>
        {
            Ok(RuntimeResetPeerV1::Offline)
        }
        DaemonClientErrorV1::PeerIdentity { .. } | DaemonClientErrorV1::EndpointSecurity { .. } => {
            Err(RuntimeResetErrorV1::unsafe_reason(
                RuntimeResetReasonV1::UnsafePath,
            ))
        }
        _ => Err(RuntimeResetErrorV1::unsafe_reason(RuntimeResetReasonV1::Io)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_security_failures_never_prove_offline() {
        for error in [
            DaemonClientErrorV1::PeerIdentity {
                expected_uid: 501,
                actual_uid: 502,
            },
            DaemonClientErrorV1::EndpointSecurity {
                message: "unsafe endpoint".to_owned(),
            },
        ] {
            let error = classify_peer_error(error).unwrap_err();
            assert_eq!(error.code(), "RUNTIME_RESET_UNSAFE");
            assert_eq!(error.reason, RuntimeResetReasonV1::UnsafePath);
        }
        // The same OS error after connecting cannot establish process absence.
        let error = classify_peer_error(DaemonClientErrorV1::Connection {
            operation: DaemonClientIoOperationV1::Read,
            source: io::Error::from(io::ErrorKind::ConnectionRefused),
        })
        .unwrap_err();
        assert_eq!(error.reason, RuntimeResetReasonV1::Io);
    }
}
