//! Historical validation fixtures for shared output and renderer regression tests.

use podway_config::{AuthoringStage, finalize_diagnostics};
use podway_core::{PROCEDURE_SCHEMA_V2, Sha256Digest};
use serde_json::{Value, json};

use super::{RunResult, local_result_v2, render_authoring_diagnostics};

const PROCEDURE_VALIDATE_COMMAND: &str = "procedure.validate";

pub(super) fn procedure_validate_v2_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Validate, diagnostic))
            .collect(),
    );
    let text = match digest {
        Some(digest) if report.diagnostics().is_empty() => {
            format!("{source_path}: valid ({})\n", digest.as_str())
        }
        _ => render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total()),
    };
    let exit_code = i32::from(!report.valid());
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "validate",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    if let Some(digest) = digest {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    local_result_v2(PROCEDURE_VALIDATE_COMMAND, result, text, exit_code)
}
