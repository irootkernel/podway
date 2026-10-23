//! Human and JSON output rendering.

use std::io::{self, Write};

use podway_protocol::{CommandNameV1, RequestIdV1};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    LOCAL_CLIENT_EXIT, LOCAL_USAGE_EXIT, LocalEnvelopeClock, LocalFailure, RunResult,
    SystemLocalEnvelopeClock, local_generated_at, retirement,
};

pub(super) fn render_retirement(retired: retirement::RetiredSurface, json_output: bool) -> i32 {
    let guidance = match &retired.replacement {
        Some(replacement) => format!("This command is retired. Use {replacement} instead."),
        None => "This command or flag is retired. No replacement exists.".to_owned(),
    };
    if !json_output {
        return if writeln!(io::stderr().lock(), "error: COMMAND_RETIRED: {guidance}").is_ok() {
            LOCAL_USAGE_EXIT
        } else {
            LOCAL_CLIENT_EXIT
        };
    }
    let generated_at = match local_generated_at(&SystemLocalEnvelopeClock) {
        Ok(timestamp) => timestamp,
        Err(failure) => return render_clock_failure_to(failure, &mut io::stderr().lock()),
    };
    let value = json!({
        "schema": "podway.error/v2",
        "request_id": Uuid::new_v4().to_string(),
        "command": retired.command,
        "generated_at": generated_at.as_str(),
        "code": "COMMAND_RETIRED",
        "message": guidance,
        "retryable": false,
        "exit_code": LOCAL_USAGE_EXIT,
        "details": {
            "schema": "podway.command-retired-details/v1",
            "retired": retired.name,
            "replacement": retired.replacement,
            "guidance": guidance,
        },
    });
    let Ok(error) = podway_protocol::ErrorEnvelopeV2::from_value(value) else {
        return LOCAL_CLIENT_EXIT;
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &error).is_err() || writeln!(stdout).is_err() {
        LOCAL_CLIENT_EXIT
    } else {
        LOCAL_USAGE_EXIT
    }
}

pub(super) fn render_result(result: &RunResult, json_output: bool, quiet: bool) -> i32 {
    render_result_with_clock(result, json_output, quiet, &SystemLocalEnvelopeClock)
}

fn render_result_with_clock(
    result: &RunResult,
    json_output: bool,
    quiet: bool,
    clock: &impl LocalEnvelopeClock,
) -> i32 {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    render_result_with_clock_and_writers(
        result,
        json_output,
        quiet,
        clock,
        &mut stdout,
        &mut stderr,
    )
}

pub(super) fn render_result_with_clock_and_writers(
    result: &RunResult,
    json_output: bool,
    quiet: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match result {
        RunResult::Direct {
            response,
            procedure_check_path,
        } => {
            let value = match &**response {
                podway_engine::direct_api::DirectResponseV1::Success(output) => output.as_value(),
                podway_engine::direct_api::DirectResponseV1::Failure(error) => error.as_value(),
            };
            if json_output {
                if serde_json::to_writer(&mut *stdout, value).is_err() || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            } else {
                let rendered = match &**response {
                    podway_engine::direct_api::DirectResponseV1::Success(output) if !quiet => {
                        render_human_output_direct(output, stdout)
                    }
                    podway_engine::direct_api::DirectResponseV1::Failure(error) => {
                        render_human_error_direct(error, procedure_check_path.as_deref(), stderr)
                    }
                    _ => Ok(()),
                };
                if rendered.is_err() {
                    return LOCAL_CLIENT_EXIT;
                }
            }
            match &**response {
                podway_engine::direct_api::DirectResponseV1::Success(_) => 0,
                podway_engine::direct_api::DirectResponseV1::Failure(error) => {
                    error.as_value()["exit_code"]
                        .as_i64()
                        .unwrap_or(i64::from(LOCAL_CLIENT_EXIT)) as i32
                }
            }
        }
        RunResult::VersionSummary {
            name,
            version,
            text,
        } => {
            if json_output {
                let output = json!({ "name": name, "version": version });
                if serde_json::to_writer(&mut *stdout, &output).is_err()
                    || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            } else if !quiet && writeln!(stdout, "{text}").is_err() {
                return LOCAL_CLIENT_EXIT;
            }
            0
        }
        RunResult::Local {
            command,
            result,
            text,
        } => {
            if json_output {
                let generated_at = match local_generated_at(clock) {
                    Ok(timestamp) => timestamp,
                    Err(failure) => return render_clock_failure_to(failure, stderr),
                };
                let request_id = RequestIdV1::new(Uuid::new_v4().to_string())
                    .expect("UUID-v4 request identifiers satisfy the public protocol");
                let command = CommandNameV1::new(command.clone())
                    .expect("local command names satisfy the public protocol");
                let Ok(output) = podway_protocol::OutputEnvelopeV4::new(
                    podway_protocol::OutputEnvelopeInputV4 {
                        request_id,
                        command,
                        generated_at,
                        workspace: None,
                        session: None,
                        result: result.clone(),
                        warnings: Vec::new(),
                    },
                ) else {
                    return render_local_failure_with_clock_and_writers(
                        LocalFailure::response_invalid(
                            "local output exceeds the protocol bounds or is invalid",
                        ),
                        true,
                        clock,
                        stdout,
                        stderr,
                    );
                };
                let output = output.into_value();
                if serde_json::to_writer(&mut *stdout, &output).is_err()
                    || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            } else if !quiet && !text.is_empty() && writeln!(stdout, "{text}").is_err() {
                return LOCAL_CLIENT_EXIT;
            }
            0
        }
        RunResult::LocalV2 {
            command,
            result,
            text,
            exit_code,
        } => {
            if json_output {
                let generated_at = match local_generated_at(clock) {
                    Ok(timestamp) => timestamp,
                    Err(failure) => return render_clock_failure_to(failure, stderr),
                };
                let request_id = RequestIdV1::new(Uuid::new_v4().to_string())
                    .expect("UUID-v4 request identifiers satisfy the public protocol");
                let command = CommandNameV1::new(command.clone())
                    .expect("local command names satisfy the public protocol");
                let Ok(output) = podway_protocol::OutputEnvelopeV4::new(
                    podway_protocol::OutputEnvelopeInputV4 {
                        request_id,
                        command,
                        generated_at,
                        workspace: None,
                        session: None,
                        result: result.clone(),
                        warnings: Vec::new(),
                    },
                ) else {
                    return render_local_failure_with_clock_and_writers(
                        LocalFailure::response_invalid(
                            "local output exceeds the protocol bounds or is invalid",
                        ),
                        true,
                        clock,
                        stdout,
                        stderr,
                    );
                };
                if serde_json::to_writer(&mut *stdout, &output).is_err()
                    || writeln!(stdout).is_err()
                {
                    return LOCAL_CLIENT_EXIT;
                }
            // The text payload is the exact byte projection — a formatted document already ends in
            // its own newline — so it is written verbatim rather than through `writeln!`.
            } else if !quiet && !text.is_empty() && write!(stdout, "{text}").is_err() {
                return LOCAL_CLIENT_EXIT;
            }
            *exit_code
        }
    }
}

fn render_clock_failure_to(failure: LocalFailure, stderr: &mut dyn Write) -> i32 {
    if writeln!(stderr, "error: {}", failure.message).is_err() {
        LOCAL_CLIENT_EXIT
    } else {
        failure.exit_code
    }
}

pub(super) fn render_human_error_direct(
    error: &podway_protocol::ErrorEnvelopeV2,
    procedure_check_path: Option<&str>,
    stderr: &mut dyn Write,
) -> Result<(), LocalFailure> {
    let error = error.as_value();
    write_text_line(
        stderr,
        format_args!(
            "error: {}: {}",
            error["code"].as_str().unwrap_or_default(),
            error["message"].as_str().unwrap_or_default()
        ),
    )?;
    if error["code"] == "WORKSPACE_ID_CONFLICT" {
        write_text_line(
            stderr,
            format_args!(
                "hint: Use the original worktree, or retire other writers and explicitly confirm an unfenced reset --all with a new idempotency key to discard the copied or rebound runtime. Unfenced removal requires a human terminal and the exact worktree path."
            ),
        )?;
    }
    if error["code"] == "WORKSPACE_SCHEMA_UNSUPPORTED"
        && let Some(guidance) = error["details"].get("guidance").and_then(Value::as_str)
    {
        write_text_line(stderr, format_args!("hint: {guidance}"))?;
    }
    if error["code"] == "WORKSPACE_CONTRACT_MISMATCH" {
        for line in workspace_contract_mismatch_hint_lines(&error["details"]) {
            write_text_line(stderr, format_args!("hint: {line}"))?;
        }
    }
    if error["code"] == "CRITERION_CITATION_INVALID" {
        for line in criterion_citation_hint_lines(&error["details"]) {
            write_text_line(stderr, format_args!("hint: {line}"))?;
        }
    }
    if let Some(reason) = error["details"]["recovery"]
        .get("reason")
        .and_then(Value::as_str)
    {
        write_text_line(stderr, format_args!("reason: {reason}"))?;
    }
    if error["code"]
        .as_str()
        .unwrap_or_default()
        .starts_with("DELEGATION_")
        && let Some(reason) = error["details"].get("reason").and_then(Value::as_str)
    {
        write_text_line(stderr, format_args!("reason: {reason}"))?;
    }
    if error["code"].as_str().unwrap_or_default() != "PROCEDURE_V2_SCHEMA_INVALID" {
        return Ok(());
    }

    if let Some(codes) = error["details"]
        .get("diagnostic_codes")
        .and_then(Value::as_array)
    {
        let rendered = codes
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if !rendered.is_empty() {
            write_text_line(stderr, format_args!("diagnostics: {rendered}"))?;
        }
    }
    if let Some(path) = procedure_check_path {
        let hint = render_preview_start_command(&[
            "podway".to_owned(),
            "procedure".to_owned(),
            "check".to_owned(),
            "--".to_owned(),
            path.to_owned(),
        ]);
        write_text_line(
            stderr,
            format_args!("hint: {hint} for field-level diagnostics and remediation guidance"),
        )?;
    }
    Ok(())
}

/// Human recovery hints for a stored writer contract this binary does not
/// support. The stored writer release line appears only when the details
/// carry the generation-2 `actual_product_version` field, so generation-1
/// stderr stays byte-identical.
fn workspace_contract_mismatch_hint_lines(details: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(stored) = details
        .get("actual_contract_manifest_digest")
        .and_then(Value::as_str)
    {
        lines.push(format!("stored writer manifest digest: {stored}"));
    }
    if let Some(current) = details
        .get("expected_contract_manifest_digest")
        .and_then(Value::as_str)
    {
        lines.push(format!("this binary manifest digest: {current}"));
    }
    match details.get("actual_product_version") {
        Some(Value::String(release)) => lines.push(format!("stored writer release: {release}")),
        Some(Value::Null) => lines.push("stored writer release: unknown".to_owned()),
        _ => {}
    }
    if !lines.is_empty() {
        lines.push(
            "continue with the writer whose version --identity reports the stored digest, or explicitly confirm an unfenced reset --all"
                .to_owned(),
        );
    }
    lines
}

/// Human hint for a rejected goal-criterion citation. The citable-source
/// lines appear only when the details carry the generation-2 lists, so
/// generation-1 stderr stays the single admission-rule line.
fn criterion_citation_hint_lines(details: &Value) -> Vec<String> {
    let mut lines = vec![
        "cite only resolved guidance.references[] sources and recorded items of the decision attempt, each once, at most four citations"
            .to_owned(),
    ];
    for (field, label) in [
        (
            "citable_evidence_source_nodes",
            "citable evidence source nodes",
        ),
        ("citable_item_ids", "citable item ids"),
    ] {
        if let Some(values) = details.get(field).and_then(Value::as_array) {
            let joined = if values.is_empty() {
                "(none)".to_owned()
            } else {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            lines.push(format!("{label}: {joined}"));
        }
    }
    lines
}

fn render_human_output_direct(
    output: &podway_protocol::OutputEnvelopeV4,
    stdout: &mut dyn Write,
) -> Result<(), LocalFailure> {
    let value = output.as_value();
    if let Some(workspace) = value.get("workspace") {
        write_text_line(
            stdout,
            format_args!(
                "workspace: {} {} sequence={}",
                workspace["uuid"].as_str().unwrap_or_default(),
                workspace["root"].as_str().unwrap_or_default(),
                workspace["latest_workspace_sequence"]
            ),
        )?;
    }
    if let Some(session) = value.get("session") {
        write_text_line(
            stdout,
            format_args!(
                "session: {} {} {} revision={}",
                session["id"].as_str().unwrap_or_default(),
                session["title"].as_str().unwrap_or_default(),
                session["lifecycle"].as_str().unwrap_or_default(),
                session["revision_after"]
            ),
        )?;
    }
    let result = &value["result"];
    if contains_check_result_projection_v1(result) {
        write_text_line(
            stdout,
            format_args!(
                "note: check_result is a caller-supplied, structurally bound external check result; Podway does not verify or attest it"
            ),
        )?;
    }
    for line in unsatisfied_check_result_lines_v1(result) {
        write_text_line(stdout, format_args!("{line}"))?;
    }
    if result["schema"].as_str() == Some("podway.workspace-removal-result/v2") {
        write_text_line(
            stdout,
            format_args!(
                "Podway workspace state was removed from {}. The Git worktree was preserved; tracked .podway paths may now appear deleted.",
                result["worktree_root"]
                    .as_str()
                    .unwrap_or("the selected worktree")
            ),
        )?;
    }
    let rendered = serde_json::to_string_pretty(result)
        .map_err(|_| LocalFailure::response_invalid("cannot render Procedure v2 result"))?;
    write_text_line(stdout, format_args!("result: {rendered}"))?;
    for warning in value["warnings"].as_array().into_iter().flatten() {
        write_text_line(stdout, format_args!("warning: {warning}"))?;
    }
    Ok(())
}

pub(super) fn contains_check_result_projection_v1(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) == Some("check_result")
                || ([
                    "operation_id",
                    "operation_digest",
                    "input_basis",
                    "outcome",
                    "output_digest",
                ]
                .into_iter()
                .all(|field| object.contains_key(field)))
                || object.values().any(contains_check_result_projection_v1)
        }
        Value::Array(values) => values.iter().any(contains_check_result_projection_v1),
        _ => false,
    }
}

/// Human-readable lines naming why each stored check result in one result stays unsatisfied.
///
/// The machine `unsatisfied_reason` field stays authoritative; these lines only place the declared
/// and recorded identity side by side so a caller sees the exact mismatch without comparing
/// digests by eye.
pub(super) fn unsatisfied_check_result_lines_v1(value: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    collect_unsatisfied_check_result_lines_v1(value, &mut lines);
    lines
}

fn collect_unsatisfied_check_result_lines_v1(value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("check_result")
                && let Some(reason) = object.get("unsatisfied_reason").and_then(Value::as_str)
            {
                lines.push(unsatisfied_check_result_line_v1(object, reason));
            }
            object
                .values()
                .for_each(|nested| collect_unsatisfied_check_result_lines_v1(nested, lines));
        }
        Value::Array(values) => values
            .iter()
            .for_each(|nested| collect_unsatisfied_check_result_lines_v1(nested, lines)),
        _ => {}
    }
}

fn unsatisfied_check_result_line_v1(item: &Map<String, Value>, reason: &str) -> String {
    let item_id = item
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or("<item>");
    let text = |source: &str, field: &str| -> String {
        item.get(source)
            .and_then(|value| value.get(field))
            .map(|value| match value {
                Value::String(text) => text.clone(),
                Value::Array(values) => values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
                other => other.to_string(),
            })
            .unwrap_or_else(|| "<unknown>".to_owned())
    };
    let detail = match reason {
        "operation_id_mismatch" => format!(
            "declared operation_id {}; recorded {}",
            text("constraints", "operation_id"),
            text("value", "operation_id")
        ),
        "operation_digest_mismatch" => format!(
            "declared operation_digest {}; recorded {}",
            text("constraints", "operation_digest"),
            text("value", "operation_digest")
        ),
        "outcome_not_accepted" => format!(
            "accepted outcomes {}; recorded {}",
            text("constraints", "accepted_outcomes"),
            text("value", "outcome")
        ),
        _ => "see the machine result".to_owned(),
    };
    format!("unsatisfied check_result {item_id}: {reason}; {detail}")
}

pub(super) fn render_local_failure(failure: LocalFailure, json_output: bool) -> i32 {
    render_local_failure_with_clock(failure, json_output, &SystemLocalEnvelopeClock)
}

fn render_local_failure_with_clock(
    failure: LocalFailure,
    json_output: bool,
    clock: &impl LocalEnvelopeClock,
) -> i32 {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    render_local_failure_with_clock_and_writers(
        failure,
        json_output,
        clock,
        &mut stdout,
        &mut stderr,
    )
}

pub(super) fn render_local_failure_with_clock_and_writers(
    mut failure: LocalFailure,
    json_output: bool,
    clock: &impl LocalEnvelopeClock,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if json_output {
        let generated_at = match local_generated_at(clock) {
            Ok(timestamp) => timestamp,
            Err(clock_failure) => return render_clock_failure_to(clock_failure, stderr),
        };
        let request_id = failure
            .request_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let output = json!({ "schema": "podway.error/v2", "request_id": request_id, "command": failure.command, "generated_at": generated_at.as_str(), "code": failure.code, "message": failure.message, "retryable": failure.retryable, "exit_code": failure.exit_code, "details": failure.details });
        let output = match podway_protocol::ErrorEnvelopeV2::from_value(output) {
            Ok(output) => output,
            Err(_) => {
                failure = LocalFailure::response_invalid(
                    "the local client could not construct a valid error response",
                );
                let fallback = json!({
                    "schema": "podway.error/v2",
                    "request_id": Uuid::new_v4().to_string(),
                    "command": "cli",
                    "generated_at": generated_at.as_str(),
                    "code": failure.code,
                    "message": failure.message,
                    "retryable": failure.retryable,
                    "exit_code": failure.exit_code,
                    "details": failure.details
                });
                match podway_protocol::ErrorEnvelopeV2::from_value(fallback) {
                    Ok(output) => output,
                    Err(_) => return render_clock_failure_to(failure, stderr),
                }
            }
        };
        if serde_json::to_writer(&mut *stdout, &output).is_err() || writeln!(stdout).is_err() {
            return LOCAL_CLIENT_EXIT;
        }
    } else if writeln!(stderr, "error: {}", failure.message).is_err() {
        return LOCAL_CLIENT_EXIT;
    }
    failure.exit_code
}

pub(super) fn render_write_failure() -> LocalFailure {
    LocalFailure::response_invalid("cannot write command output")
}

pub(super) fn write_text_line(
    writer: &mut dyn Write,
    text: std::fmt::Arguments<'_>,
) -> Result<(), LocalFailure> {
    writer.write_fmt(text).map_err(|_| render_write_failure())?;
    writer.write_all(b"\n").map_err(|_| render_write_failure())
}

/// The stable one-line-per-finding authoring report.
///
/// The format is `<source_path>:<line>:<column> <severity> <code> <message>`, mirroring the
/// position-first convention every editor's error parser already understands. The machine-readable
/// form — locations, hints, and graph identities included — is the JSON result.
pub(super) fn render_authoring_diagnostics(
    diagnostics: &[podway_core::AuthoringDiagnostic],
    truncated: bool,
    total: u32,
) -> String {
    let mut text = String::new();
    for diagnostic in diagnostics {
        text.push_str(&format!(
            "{}:{}:{} {} {} {}\n",
            diagnostic.source_path(),
            diagnostic.location().line(),
            diagnostic.location().column(),
            diagnostic.severity().as_str(),
            diagnostic.code().as_str(),
            diagnostic.message(),
        ));
    }
    if truncated {
        text.push_str(&format!(
            "diagnostics truncated: showing {} of {total}\n",
            diagnostics.len(),
        ));
    }
    text
}

pub(super) fn render_preview_text_value(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            rendered.extend(character.escape_default());
        } else {
            rendered.push(character);
        }
    }
    rendered
}

/// Renders a structured preview suggestion as copyable POSIX shell source for human output.
pub(super) fn render_preview_start_command(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| {
            if !argument.is_empty()
                && argument
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"@%_+=:,./-".contains(&byte))
            {
                argument.clone()
            } else {
                format!("'{}'", argument.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
