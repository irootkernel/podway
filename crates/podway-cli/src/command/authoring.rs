//! Offline Procedure and preset authoring handlers.

use std::{
    ffi::OsStr,
    path::{Component, Path},
};

use podway_config::{
    AuthoringContext, AuthoringStage, FormatFailure, FormatRequest, FormattedProcedureV2,
    ParsedProcedure, ProcedureDocumentFormat, ScaffoldTemplate, check_procedure_v2,
    config_error_diagnostic, finalize_diagnostics, format_procedure_v2, lint_procedure_v2,
    normalize_procedure_v2_graph, parse_procedure_document, preview_procedure_v2,
    project_procedure_v2_dot, project_procedure_v2_graph, project_procedure_v2_mermaid,
    project_procedure_v2_plantuml, scaffold_procedure_v2, validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{
    ItemConditionV2, ItemPredicateOperatorV2, OptionGuardV2, PROCEDURE_SCHEMA_V2,
    PredicateScalarV2, Sha256Digest,
};
use podway_presets::catalog_v2;
use serde_json::{Map, Value, json};

use super::{
    LocalFailure, OpenedProcedure, PresetCommand, ProcedureCommand, RunResult, local_result,
    local_result_v2, open_descriptor_relative_procedure, open_offline_procedure, preset_failure,
    procedure_config_failure, read_offline_procedure, render_authoring_diagnostics,
    render_preview_start_command, render_preview_text_value, replace_procedure_document,
};

pub(super) fn execute_preset(command: &PresetCommand) -> Result<RunResult, LocalFailure> {
    match command {
        PresetCommand::List => {
            let presets: Vec<Value> = catalog_v2()
                .list()
                .iter()
                .map(|preset| preset.metadata)
                .map(|metadata| {
                    json!({ "id": metadata.id, "name": metadata.name, "version": metadata.version, "description": metadata.description })
                })
                .collect();
            let text = presets
                .iter()
                .filter_map(|preset| preset.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(local_result(
                "preset.list",
                json!({ "presets": presets }),
                text,
            ))
        }
        PresetCommand::Show { name } => {
            let preset = catalog_v2().lookup(name).ok_or_else(|| {
                LocalFailure::preset_not_found("unknown preset").with_command("preset.show")
            })?;
            let admitted = preset
                .validate()
                .map_err(|error| preset_failure(error).with_command("preset.show"))?;
            let procedure: Value = serde_json::from_str(admitted.canonical_json().as_str())
                .map_err(|_| {
                    LocalFailure::procedure_invalid(
                        "embedded Procedure v2 canonical JSON is invalid",
                    )
                    .with_command("preset.show")
                })?;
            Ok(local_result(
                "preset.show",
                json!({
                    "preset": preset.metadata.id,
                    "metadata": preset.metadata,
                    "digest": admitted.digest().as_str(),
                    "procedure": procedure,
                    "warnings": [],
                }),
                preset.yaml.to_owned(),
            ))
        }
        PresetCommand::Explain { name } => {
            let preset = catalog_v2().lookup(name).ok_or_else(|| {
                LocalFailure::preset_not_found("unknown preset").with_command("preset.explain")
            })?;
            let admitted = preset
                .validate()
                .map_err(|error| preset_failure(error).with_command("preset.explain"))?;
            let document: Value = serde_json::from_str(admitted.canonical_json().as_str())
                .map_err(|_| {
                    LocalFailure::procedure_invalid(
                        "embedded Procedure v2 canonical JSON is invalid",
                    )
                    .with_command("preset.explain")
                })?;
            let definitions = document["node_definitions"]
                .as_object()
                .expect("validated Procedure v2 has node definitions");
            let nodes: Vec<Value> = document["graph"]["nodes"]
                .as_array()
                .expect("validated Procedure v2 has graph nodes")
                .iter()
                .map(|node| {
                    let definition = node["use"]
                        .as_str()
                        .and_then(|id| definitions.get(id))
                        .expect("validated graph node resolves its definition");
                    json!({
                        "id": node["id"],
                        "title": definition["title"],
                        "type": definition["type"],
                    })
                })
                .collect();
            let text = format!(
                "{}\n{}\nGraph nodes: {}",
                preset.metadata.name,
                preset.metadata.description,
                nodes
                    .iter()
                    .filter_map(|node| node.get("id").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(local_result(
                "preset.explain",
                json!({ "preset": preset.metadata, "nodes": nodes }),
                text,
            ))
        }
    }
}

pub(super) fn execute_procedure(command: &ProcedureCommand) -> Result<RunResult, LocalFailure> {
    let (file, warnings_as_errors, name) = match command {
        ProcedureCommand::Validate {
            file,
            warnings_as_errors,
        } => (file, *warnings_as_errors, PROCEDURE_VALIDATE_COMMAND),
        ProcedureCommand::Show { file, .. } => (file, false, "procedure.show"),
        ProcedureCommand::Format { file, check, write } => {
            return execute_procedure_format(file, *check, *write);
        }
        ProcedureCommand::Vet { file } => {
            return execute_procedure_vet(file);
        }
        ProcedureCommand::Graph { file, format } => {
            return execute_procedure_graph(file, format);
        }
        ProcedureCommand::Preview { file } => {
            return execute_procedure_preview(file);
        }
        ProcedureCommand::Lint {
            file,
            warnings_as_errors,
        } => {
            return execute_procedure_lint(file, *warnings_as_errors);
        }
        ProcedureCommand::Check {
            file,
            warnings_as_errors,
        } => {
            return execute_procedure_check(file, *warnings_as_errors);
        }
        ProcedureCommand::Scaffold { template } => {
            return Ok(execute_procedure_scaffold(template));
        }
    };
    let bytes = read_offline_procedure(file).map_err(|failure| failure.with_command(name))?;
    std::str::from_utf8(&bytes).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(name)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    if matches!(command, ProcedureCommand::Validate { .. }) {
        return Ok(execute_procedure_validate_v2(file, &bytes, format));
    }
    debug_assert!(!warnings_as_errors);
    let ParsedProcedure::V2(parsed) = parse_procedure_document(&bytes, format)
        .map_err(|error| procedure_config_failure(error).with_command(name))?;
    let validated = validate_procedure_v2(parsed)
        .map_err(|error| procedure_config_failure(error).with_command(name))?;
    let result = json!({
        "file": file.display().to_string(),
        "digest": validated.digest().as_str(),
        "canonical_json": validated.canonical_json().as_str(),
        "procedure_schema": PROCEDURE_SCHEMA_V2,
    });
    let text = match command {
        ProcedureCommand::Validate { .. } => format!(
            "{} ({})",
            validated.parsed().name(),
            validated.digest().as_str()
        ),
        ProcedureCommand::Show {
            canonical: true, ..
        } => validated.canonical_json().as_str().to_owned(),
        ProcedureCommand::Show {
            canonical: false, ..
        } => String::from_utf8(bytes).map_err(|_| {
            LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(name)
        })?,
        ProcedureCommand::Format { .. }
        | ProcedureCommand::Vet { .. }
        | ProcedureCommand::Graph { .. }
        | ProcedureCommand::Preview { .. }
        | ProcedureCommand::Lint { .. }
        | ProcedureCommand::Check { .. }
        | ProcedureCommand::Scaffold { .. } => {
            unreachable!("the v2 authoring commands dispatch to their own execution paths")
        }
    };
    Ok(local_result(name, result, text))
}

/// The route every `procedure validate` result reports under, named once so the v2 result builder
/// cannot disagree with the route the v1 path reports under.
const PROCEDURE_VALIDATE_COMMAND: &str = "procedure.validate";

/// Validates a Procedure v2 document and reports one bounded diagnostics result.
///
/// Reached only when the schema sniff positively identified Procedure v2, so this never sees a v1
/// document and never has to choose an error surface for one.
///
/// Validate is a two-stage, single-error pipeline: parsing maps the document into the model and
/// validation resolves its closed reference set, and the first failure of either is the whole
/// report. The result therefore carries zero diagnostics or exactly one, and `valid` is the
/// difference between them. `digest` is present exactly when validation produced one, which is
/// exactly when the document is admissible.
///
/// `--warnings-as-errors` is accepted and does nothing here: every diagnostic validate can emit is
/// catalogued as an error, so there is no warning for the policy to promote. The advisory stages
/// that do emit warnings are `procedure lint` and `procedure check`.
fn execute_procedure_validate_v2(
    file: &Path,
    bytes: &[u8],
    format: ProcedureDocumentFormat,
) -> RunResult {
    let source = std::str::from_utf8(bytes)
        .expect("execute_procedure rejects a non-UTF-8 document before dispatching");
    let source_path = file.display().to_string();
    let context = AuthoringContext::new(&source_path, source, format);

    let admitted = match parse_procedure_document(bytes, format) {
        Ok(ParsedProcedure::V2(parsed)) => validate_procedure_v2(parsed),
        Err(error) => Err(error),
    };

    match admitted {
        Ok(validated) => {
            procedure_validate_v2_diagnostics(&source_path, Some(validated.digest()), Vec::new())
        }
        Err(error) => procedure_validate_v2_diagnostics(
            &source_path,
            None,
            vec![config_error_diagnostic(&error, &context)],
        ),
    }
}

/// The `procedure validate` result for a Procedure v2 document.
///
/// Exit code 0 when the document is admissible and 1 when it is not, which for this command is the
/// same thing as the presence of a diagnostic: validate emits nothing but errors.
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

/// Renders a Procedure v2 source document in canonical authoring form.
///
/// The read is the same hardened descriptor-relative walk `procedure validate` uses, and it runs
/// before the unimplemented-flag rejections so a missing or unsafe path always reports the path
/// failure rather than a capability failure.
///
/// `--check` answers the same question the default mode answers and reports it differently: the
/// pipeline runs identically, and only the last step — comparing the rendering against the source —
/// decides between a source result that says "already canonical" and the single
/// `FORMAT_NOT_CANONICAL` finding. Nothing on either path touches the filesystem after the read, so
/// `--check` is observably non-writing rather than merely intended to be.
///
/// `--write` adds one step after that comparison and nothing before it. The read, the parse, the
/// validation, the construct scan, the emission, and the projection bound all run first, so every
/// way this command can refuse a document is a refusal that has already happened by the time any
/// filesystem mutation is possible: a rejected file is byte-identical afterwards and no temporary
/// file was ever created.
fn execute_procedure_format(
    file: &Path,
    check: bool,
    write: bool,
) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_FORMAT_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;

    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();
    match format_procedure_v2(FormatRequest {
        source,
        source_path: &source_path,
        format,
    }) {
        Ok(formatted) if check => {
            let context = AuthoringContext::new(&source_path, source, format);
            match formatted.drift_diagnostic(&context) {
                Some(diagnostic) => {
                    Ok(procedure_format_diagnostics(&source_path, vec![diagnostic]))
                }
                None => Ok(procedure_format_source(
                    &source_path,
                    &formatted,
                    "check",
                    canonical_form_summary(&source_path),
                )),
            }
        }
        Ok(formatted) if write => procedure_format_write(&source_path, &formatted, &opened),
        Ok(formatted) => {
            let document = formatted.document().to_owned();
            Ok(procedure_format_source(
                &source_path,
                &formatted,
                "stdout",
                document,
            ))
        }
        Err(FormatFailure::NotProcedureV2) => Err(LocalFailure::catalog(
            "PROCEDURE_SCHEMA_UNSUPPORTED",
            "procedure format requires a podway.procedure/v2 document",
            NAME,
        )),
        Err(FormatFailure::Diagnostics(diagnostics)) => {
            Ok(procedure_format_diagnostics(&source_path, diagnostics))
        }
    }
}

/// The route every `procedure format` result reports under, named once so the three result
/// builders below cannot disagree with the route the failure paths use.
pub(super) const PROCEDURE_FORMAT_COMMAND: &str = "procedure.format";

/// The pinned one-line verdict for a source that is already in canonical authoring form.
///
/// `--check` and `--write` share it deliberately: a clean file gets the same answer whether or not
/// the caller was prepared to rewrite it, because in both cases nothing happened.
fn canonical_form_summary(source_path: &str) -> String {
    format!("{source_path} is in canonical authoring form\n")
}

/// Rewrites a drifted procedure file in canonical authoring form, or leaves a clean one alone.
///
/// The no-op is a real no-op: an already-canonical file is not rewritten with identical bytes, it
/// is not opened for writing, and no temporary file is created, so its modification time survives
/// and a build system watching the tree sees no work. `changed` reports which of the two happened.
fn procedure_format_write(
    source_path: &str,
    formatted: &FormattedProcedureV2,
    opened: &OpenedProcedure,
) -> Result<RunResult, LocalFailure> {
    if !formatted.changed() {
        return Ok(procedure_format_source(
            source_path,
            formatted,
            "write",
            canonical_form_summary(source_path),
        ));
    }
    replace_procedure_document(opened, formatted.document().as_bytes())?;
    Ok(procedure_format_source(
        source_path,
        formatted,
        "write",
        format!("{source_path} rewritten in canonical authoring form\n"),
    ))
}

/// The `procedure format` success result: the canonical document plus the mode that produced it.
///
/// `document` is present in every mode because the source result schema requires it, which is the
/// right requirement: a `--check` client that learns the file has drifted with no way to see the
/// canonical form would have to run the command a second time to act on the answer.
fn procedure_format_source(
    source_path: &str,
    formatted: &FormattedProcedureV2,
    mode: &str,
    text: String,
) -> RunResult {
    let Value::Object(result) = json!({
        "schema": "podway.procedure-source-result/v1",
        "operation": "format",
        "target_schema": "podway.procedure/v2",
        "target_digest": formatted.digest().as_str(),
        "document": formatted.document(),
        "file": source_path,
        "mode": mode,
        "changed": formatted.changed(),
    }) else {
        unreachable!("the static source result is a JSON object");
    };
    local_result_v2(PROCEDURE_FORMAT_COMMAND, result, text, 0)
}

/// The `procedure format` findings result, shared by every stage that can produce one.
fn procedure_format_diagnostics(
    source_path: &str,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Format, diagnostic))
            .collect(),
    );
    let text =
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total());
    let Value::Object(result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "format",
        "procedure_schema": "podway.procedure/v2",
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    local_result_v2(PROCEDURE_FORMAT_COMMAND, result, text, 1)
}

/// The route every `procedure vet` result reports under.
const PROCEDURE_VET_COMMAND: &str = "procedure.vet";

/// Runs mandatory graph-wide analysis over a validated Procedure v2 document.
///
/// Vet is unconditionally read-only. Parsing or closed-reference validation stops the pipeline
/// with that single diagnostic; a validated model always carries its digest, including when graph
/// analysis rejects it. Every vet finding is an error, so no warning policy is part of this route.
fn execute_procedure_vet(file: &Path) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_VET_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Err(error) => {
            return Ok(procedure_vet_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };
    let validated = match validate_procedure_v2(parsed) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(procedure_vet_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };

    let findings = vet_procedure_v2(&validated, &context);
    Ok(procedure_vet_diagnostics(
        &source_path,
        Some(validated.digest()),
        AuthoringStage::Vet,
        findings,
    ))
}

fn procedure_vet_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    stage: AuthoringStage,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (stage, diagnostic))
            .collect(),
    );
    let text = if report.diagnostics().is_empty() {
        format!("{source_path}: graph vetting passed\n")
    } else {
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
    };
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "vet",
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
    let exit_code = i32::from(!report.valid());
    local_result_v2(PROCEDURE_VET_COMMAND, result, text, exit_code)
}

/// The route every `procedure graph` result reports under.
const PROCEDURE_GRAPH_COMMAND: &str = "procedure.graph";

/// Projects a validated and vetted Procedure v2 graph in the selected deterministic format.
///
/// This route is unconditionally read-only. It deliberately repeats vetting against the exact
/// bytes opened for this invocation: a previous successful `procedure vet` result cannot admit a
/// file that changed afterwards. Parsing or closed-reference validation has no canonical digest;
/// every later rejection does.
fn execute_procedure_graph(file: &Path, format_name: &str) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_GRAPH_COMMAND;

    debug_assert!(
        matches!(format_name, "json" | "mermaid" | "puml" | "dot"),
        "Clap admits only implemented graph formats"
    );
    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Err(error) => {
            return Ok(procedure_graph_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };
    let validated = match validate_procedure_v2(parsed) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(procedure_graph_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };

    let findings = vet_procedure_v2(&validated, &context);
    if !findings.is_empty() {
        return Ok(procedure_graph_diagnostics(
            &source_path,
            Some(validated.digest()),
            AuthoringStage::Vet,
            findings,
        ));
    }

    let (result_format, projection) = match format_name {
        "json" => (
            "json",
            project_procedure_v2_graph(&validated).map(|projection| {
                (
                    projection.projection().to_owned(),
                    projection.projection_digest().clone(),
                )
            }),
        ),
        "mermaid" => (
            "mermaid",
            normalize_procedure_v2_graph(&validated)
                .and_then(|graph| project_procedure_v2_mermaid(&graph))
                .map(|projection| {
                    (
                        projection.projection().to_owned(),
                        projection.projection_digest().clone(),
                    )
                }),
        ),
        "puml" => (
            "plantuml",
            normalize_procedure_v2_graph(&validated)
                .and_then(|graph| project_procedure_v2_plantuml(&graph))
                .map(|projection| {
                    (
                        projection.projection().to_owned(),
                        projection.projection_digest().clone(),
                    )
                }),
        ),
        "dot" => (
            "dot",
            normalize_procedure_v2_graph(&validated)
                .and_then(|graph| project_procedure_v2_dot(&graph))
                .map(|projection| {
                    (
                        projection.projection().to_owned(),
                        projection.projection_digest().clone(),
                    )
                }),
        ),
        _ => unreachable!("Clap admits only implemented graph formats"),
    };
    let (projection, projection_digest) = match projection {
        Ok(projection) => projection,
        Err(error) => {
            return Ok(procedure_graph_diagnostics(
                &source_path,
                Some(validated.digest()),
                AuthoringStage::Vet,
                vec![config_error_diagnostic(&error, &context)],
            ));
        }
    };
    let text = format!("{projection}\n");
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-graph-result/v1",
        "procedure_schema": PROCEDURE_SCHEMA_V2,
        "procedure_digest": validated.digest().as_str(),
        "format": result_format,
        "projection_digest": projection_digest.as_str(),
        "projection": projection,
    }) else {
        unreachable!("the static graph result is a JSON object");
    };
    let delegated = validated.parsed().delegated_placements();
    if !delegated.is_empty() {
        result.insert(
            "schema".to_owned(),
            json!("podway.procedure-graph-result/v2"),
        );
        result.insert(
            "delegated_placement_count".to_owned(),
            json!(delegated.len()),
        );
        result.insert("delegated_placements".to_owned(), json!(delegated));
    }
    Ok(local_result_v2(PROCEDURE_GRAPH_COMMAND, result, text, 0))
}

fn procedure_graph_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    stage: AuthoringStage,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (stage, diagnostic))
            .collect(),
    );
    let text =
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total());
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "graph",
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
    local_result_v2(PROCEDURE_GRAPH_COMMAND, result, text, 1)
}

/// The route every `procedure preview` result reports under.
const PROCEDURE_PREVIEW_COMMAND: &str = "procedure.preview";

/// Aggregates the complete local admission preview without touching runtime state.
fn execute_procedure_preview(file: &Path) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_PREVIEW_COMMAND;

    if !is_worktree_relative_procedure_path(file) {
        return Err(
            LocalFailure::request_invalid("procedure must be worktree-relative").with_command(NAME),
        );
    }
    let Some(source_path) = file.to_str() else {
        return Err(
            LocalFailure::request_invalid("procedure path must be UTF-8").with_command(NAME),
        );
    };
    let opened = open_descriptor_relative_procedure(
        Path::new("."),
        file,
        LocalFailure::catalog("PATH_OUTSIDE_WORKTREE", "cannot open worktree", "procedure"),
    )
    .map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let report = preview_procedure_v2(FormatRequest {
        source,
        source_path,
        format,
    });
    let checks = report.checks();
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-preview-result/v1",
        "file": source_path,
        "admissible": report.admissible(),
        "checks": {
            "validate": checks.validate(),
            "vet": checks.vet(),
            "lint": checks.lint(),
        },
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.diagnostics_truncated(),
        "diagnostics_total": report.diagnostics_total(),
    }) else {
        unreachable!("the static preview result is a JSON object");
    };
    let delegated = report.delegated_placements();
    if !delegated.is_empty() {
        result.insert(
            "schema".to_owned(),
            json!("podway.procedure-preview-result/v2"),
        );
        result.insert(
            "delegated_placement_count".to_owned(),
            json!(delegated.len()),
        );
        result.insert("delegated_placements".to_owned(), json!(delegated));
    }

    let text = if let Some(details) = report.details() {
        let summary = details.summary();
        let graph = details.graph();
        let nodes = graph
            .nodes()
            .iter()
            .map(|node| {
                json!({
                    "graph_node_id": node.graph_node_id(),
                    "node_definition_id": node.node_definition_id(),
                    "node_type": node.node_type().as_str(),
                    "terminal": node.terminal(),
                    "skippable": node.skippable(),
                    "items": node.items().iter().map(|item| {
                        let mut value = Map::new();
                        value.insert("item_id".to_owned(), json!(item.item_id()));
                        value.insert("required".to_owned(), json!(item.required()));
                        if let Some(condition) = item.required_when() {
                            value.insert("required_when".to_owned(), condition_json(condition));
                        }
                        Value::Object(value)
                    }).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let edges = graph
            .edges()
            .iter()
            .map(|edge| {
                let mut edge_value = json!({
                    "from_graph_node_id": edge.from_graph_node_id(),
                    "to_graph_node_id": edge.to_graph_node_id(),
                    "effect": edge.effect(),
                });
                if let Some(option_id) = edge.option_id() {
                    edge_value["option_id"] = Value::String(option_id.to_owned());
                }
                if let Some(guards) = edge.guards() {
                    edge_value["guards"] = guards_json(guards);
                }
                edge_value
            })
            .collect::<Vec<_>>();
        let start_suggestion = details.start_suggestion();
        result.extend(
            json!({
                "procedure_schema": details.procedure_schema(),
                "procedure_id": details.procedure_id(),
                "procedure_version": details.procedure_version(),
                "purpose": details.purpose(),
                "procedure_digest": details.procedure_digest().as_str(),
                "goal_tracking": details.goal_tracking(),
                "goal_assessment_graph_node_ids": details.goal_assessment_graph_node_ids(),
                "summary": {
                    "definition_count": summary.definition_count(),
                    "graph_node_count": summary.graph_node_count(),
                    "action_node_count": summary.action_node_count(),
                    "decision_node_count": summary.decision_node_count(),
                    "route_count": summary.route_count(),
                    "cycle_count": summary.cycle_count(),
                    "evidence_reference_count": summary.evidence_reference_count(),
                    "skippable_node_count": summary.skippable_node_count(),
                    "manual_rework_target_count": summary.manual_rework_target_count(),
                },
                "graph": {
                    "entry_graph_node_id": graph.entry_graph_node_id(),
                    "terminal_graph_node_ids": graph.terminal_graph_node_ids(),
                    "nodes": nodes,
                    "edges": edges,
                },
                "mermaid": details.mermaid(),
                "start_suggestion": {
                    "command": start_suggestion.command(),
                    "argv": start_suggestion.argv(),
                },
            })
            .as_object()
            .expect("the static preview details are a JSON object")
            .clone(),
        );

        let mut text = render_authoring_diagnostics(
            report.diagnostics(),
            report.diagnostics_truncated(),
            report.diagnostics_total(),
        );
        let assessments = if details.goal_assessment_graph_node_ids().is_empty() {
            "none".to_owned()
        } else {
            details.goal_assessment_graph_node_ids().join(",")
        };
        let terminals = graph.terminal_graph_node_ids().join(",");
        text.push_str(&format!(
            concat!(
                "{}: Procedure v2 preview admissible\n",
                "procedure: {} {}@{}\n",
                "purpose: {}\n",
                "procedure-digest: {}\n",
                "checks: validate={}, vet={}, lint={}\n",
                "goal-tracking: {}, goal-assessments: {}\n",
                "summary: definitions={}, graph-nodes={}, actions={}, decisions={}, ",
                "decision-routes={}, cyclic-regions={}, evidence-references={}, ",
                "skippable-nodes={}, manual-rework-targets={}\n",
                "graph: entry={}, terminals={}\n",
                "graph-nodes:\n"
            ),
            file.display(),
            details.procedure_schema(),
            details.procedure_id(),
            render_preview_text_value(details.procedure_version()),
            render_preview_text_value(details.purpose()),
            details.procedure_digest().as_str(),
            checks.validate(),
            checks.vet(),
            checks.lint(),
            details.goal_tracking(),
            assessments,
            summary.definition_count(),
            summary.graph_node_count(),
            summary.action_node_count(),
            summary.decision_node_count(),
            summary.route_count(),
            summary.cycle_count(),
            summary.evidence_reference_count(),
            summary.skippable_node_count(),
            summary.manual_rework_target_count(),
            graph.entry_graph_node_id(),
            terminals,
        ));
        for node in graph.nodes() {
            text.push_str(&format!(
                "  {}: definition={}, type={}, terminal={}, skippable={}\n",
                node.graph_node_id(),
                node.node_definition_id(),
                node.node_type().as_str(),
                node.terminal(),
                node.skippable(),
            ));
        }
        text.push_str("graph-edges:\n");
        for edge in graph.edges() {
            text.push_str(&format!(
                "  {} -> {}: {}{}\n",
                edge.from_graph_node_id(),
                edge.to_graph_node_id(),
                edge.option_id()
                    .map_or_else(String::new, |option| format!("{option} · ")),
                edge.effect(),
            ));
        }
        text.push_str(&format!(
            "\n{}\n\nstart: {}\n",
            details.mermaid(),
            render_preview_start_command(start_suggestion.argv()),
        ));
        text
    } else {
        render_authoring_diagnostics(
            report.diagnostics(),
            report.diagnostics_truncated(),
            report.diagnostics_total(),
        )
    };
    let exit_code = i32::from(!report.admissible());
    Ok(local_result_v2(NAME, result, text, exit_code))
}

fn condition_json(condition: &ItemConditionV2) -> Value {
    Value::Array(
        condition
            .predicates()
            .iter()
            .map(|predicate| {
                let mut value = Map::new();
                value.insert("item".to_owned(), json!(predicate.item().as_str()));
                if predicate.field_outcome() {
                    value.insert("field".to_owned(), json!("outcome"));
                }
                match predicate.operator() {
                    ItemPredicateOperatorV2::Equals(expected) => {
                        value.insert("equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::NotEquals(expected) => {
                        value.insert("not_equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::Empty => {
                        value.insert("empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::NonEmpty => {
                        value.insert("non_empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::AtLeast(expected) => {
                        value.insert("at_least".to_owned(), json!(expected));
                    }
                    ItemPredicateOperatorV2::AtMost(expected) => {
                        value.insert("at_most".to_owned(), json!(expected));
                    }
                }
                Value::Object(value)
            })
            .collect(),
    )
}

fn predicate_scalar_json(value: &PredicateScalarV2) -> Value {
    match value {
        PredicateScalarV2::Boolean(value) => json!(value),
        PredicateScalarV2::Integer(value) => json!(value),
        PredicateScalarV2::Text(value) => json!(value),
    }
}

fn guards_json(guards: &OptionGuardV2) -> Value {
    Value::Array(
        guards
            .predicates()
            .iter()
            .map(|predicate| {
                let mut value = Map::new();
                value.insert(
                    "evidence".to_owned(),
                    json!({
                        "node": predicate.source_node().as_str(),
                        "item": predicate.item().as_str(),
                    }),
                );
                if predicate.field_outcome() {
                    value.insert("field".to_owned(), json!("outcome"));
                }
                match predicate.operator() {
                    ItemPredicateOperatorV2::Equals(expected) => {
                        value.insert("equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::NotEquals(expected) => {
                        value.insert("not_equals".to_owned(), predicate_scalar_json(expected));
                    }
                    ItemPredicateOperatorV2::Empty => {
                        value.insert("empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::NonEmpty => {
                        value.insert("non_empty".to_owned(), json!(true));
                    }
                    ItemPredicateOperatorV2::AtLeast(expected) => {
                        value.insert("at_least".to_owned(), json!(expected));
                    }
                    ItemPredicateOperatorV2::AtMost(expected) => {
                        value.insert("at_most".to_owned(), json!(expected));
                    }
                }
                Value::Object(value)
            })
            .collect(),
    )
}

pub(super) fn is_worktree_relative_procedure_path(path: &Path) -> bool {
    !path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// The route every `procedure lint` result reports under, named once so the two result builders
/// below cannot disagree with the route the failure paths use.
const PROCEDURE_LINT_COMMAND: &str = "procedure.lint";

/// Reports the advisory authoring findings of a Procedure v2 document.
///
/// Lint is a read: the hardened descriptor-relative walk that `procedure format` uses opens the
/// file, and nothing afterwards touches the filesystem at all.
///
/// The pipeline is deliberately shorter than the formatter's. Parsing and closed-reference
/// validation run, and then the rules run on the validated model; the supported-construct scan and
/// the comment pass do not, because lint never re-emits the document, so a source construct the
/// formatter could not reproduce is still a source lint can read. A document that fails parsing or
/// validation is reported through that failure and is *not* linted: every rule reads a resolved
/// model, and advisory findings about a document Podway is already rejecting would bury the
/// rejection.
///
/// `--warnings-as-errors` is a policy about the invocation, not a statement about the document. It
/// moves the exit code from 0 to 1 and changes nothing else: the result body, every `severity`, and
/// `valid` are byte-identical with and without it.
fn execute_procedure_lint(
    file: &Path,
    warnings_as_errors: bool,
) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_LINT_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let context = AuthoringContext::new(&source_path, source, format);
    let parsed = match parse_procedure_document(source.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Err(error) => {
            return Ok(procedure_lint_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
                1,
            ));
        }
    };
    let validated = match validate_procedure_v2(parsed) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(procedure_lint_diagnostics(
                &source_path,
                None,
                AuthoringStage::Validate,
                vec![config_error_diagnostic(&error, &context)],
                1,
            ));
        }
    };

    let findings = lint_procedure_v2(&validated, &context);
    let exit_code = i32::from(warnings_as_errors && !findings.is_empty());
    Ok(procedure_lint_diagnostics(
        &source_path,
        Some(validated.digest()),
        AuthoringStage::Lint,
        findings,
        exit_code,
    ))
}

/// The pinned one-line verdict for a document with no advisory findings.
fn no_lint_findings_summary(source_path: &str) -> String {
    format!("{source_path}: no lint findings\n")
}

/// The `procedure lint` result, shared by the advisory report and by the two document-level
/// failures that stop the pipeline before the rules run.
///
/// `digest` is present exactly when validation produced one, which is exactly when the findings are
/// advisory: a client can therefore tell "these warnings describe procedure `sha256:…`" from "this
/// document has no digest because it is not admissible" without inspecting severities.
fn procedure_lint_diagnostics(
    source_path: &str,
    digest: Option<&Sha256Digest>,
    stage: AuthoringStage,
    diagnostics: Vec<podway_core::AuthoringDiagnostic>,
    exit_code: i32,
) -> RunResult {
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (stage, diagnostic))
            .collect(),
    );
    let text = if report.diagnostics().is_empty() {
        no_lint_findings_summary(source_path)
    } else {
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
    };
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "lint",
        "procedure_schema": "podway.procedure/v2",
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
    local_result_v2(PROCEDURE_LINT_COMMAND, result, text, exit_code)
}

/// The route every `procedure check` result reports under, named once so the result builder below
/// cannot disagree with the route the failure paths use.
const PROCEDURE_CHECK_COMMAND: &str = "procedure.check";

/// Runs every authoring stage over a Procedure v2 document and reports one merged verdict.
///
/// Check is the aggregate gate of section 11.5, not a new analysis: it runs the same parse,
/// validation, canonical rendering, vet, and lint stages the individual commands run, and its value
/// is that a caller gets all of their findings in one bounded, deterministically ordered result
/// instead of running four commands and merging them by hand. The drift finding is produced by the
/// same constructor `format --check` uses, so the two commands can never disagree about whether a
/// file has drifted or about where.
///
/// The vet stage is shared with `procedure vet`: it includes the structural graph rules and
/// wire-budget proofs, so the aggregate and standalone commands cannot disagree.
///
/// Only the absence of a *model* stops the pipeline. A document that parses and validates is
/// vetted and linted even when it has drifted or cannot be rendered at all, because a stale format
/// must not hide a graph finding.
///
/// Exit behaviour: 0 when nothing was found, 1 when any finding carries error severity — a drifted
/// file included, since `FORMAT_NOT_CANONICAL` is catalogued as an error — and 1 under
/// `--warnings-as-errors` when any finding at all was reported. As with lint, the flag moves the
/// exit code and nothing else: the result body, every `severity`, and `valid` are byte-identical
/// with and without it.
fn execute_procedure_check(
    file: &Path,
    warnings_as_errors: bool,
) -> Result<RunResult, LocalFailure> {
    const NAME: &str = PROCEDURE_CHECK_COMMAND;

    let opened = open_offline_procedure(file).map_err(|failure| failure.with_command(NAME))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(NAME)
    })?;
    let format = if file.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    };
    let source_path = file.display().to_string();

    let report = check_procedure_v2(FormatRequest {
        source,
        source_path: &source_path,
        format,
    });
    let exit_code =
        i32::from(!report.valid() || (warnings_as_errors && !report.diagnostics().is_empty()));
    let text = if report.diagnostics().is_empty() {
        all_checks_passed_summary(&source_path)
    } else {
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
    };
    let Value::Object(mut result) = json!({
        "schema": "podway.procedure-diagnostics-result/v1",
        "operation": "check",
        "procedure_schema": "podway.procedure/v2",
        "file": source_path,
        "valid": report.valid(),
        "diagnostics": report.diagnostics(),
        "diagnostics_truncated": report.truncated(),
        "diagnostics_total": report.total(),
    }) else {
        unreachable!("the static diagnostics result is a JSON object");
    };
    // Present exactly when the document is admissible, which is exactly when the findings describe
    // a procedure rather than explain why there is none.
    if let Some(digest) = report.digest() {
        result.insert(
            "digest".to_owned(),
            Value::String(digest.as_str().to_owned()),
        );
    }
    Ok(local_result_v2(
        PROCEDURE_CHECK_COMMAND,
        result,
        text,
        exit_code,
    ))
}

/// The pinned one-line verdict for a document every authoring stage accepted.
fn all_checks_passed_summary(source_path: &str) -> String {
    format!("{source_path}: all authoring checks passed\n")
}

/// The route every `procedure scaffold` result reports under.
const PROCEDURE_SCAFFOLD_COMMAND: &str = "procedure.scaffold";

/// Emits one authoring starting point.
///
/// This is the only local procedure command that reads nothing: there is no file argument, so there
/// is no path to resolve, no descriptor to open, and no way for the command to fail. It always
/// exits 0.
///
/// `--template` is closed at the parser, so `template` is always a name
/// [`ScaffoldTemplate::from_name`] resolves; an unknown value never reaches here, it is a usage
/// failure Clap reports with exit code 2.
///
/// The digest is derived from the emitted text through the same parse-and-validate path every other
/// Procedure v2 digest comes from rather than being carried beside the template as a second
/// constant. That makes the command self-checking — a template edited into something inadmissible
/// fails here instead of shipping a document whose advertised digest describes nothing — and it
/// keeps one derivation of the digest in the build.
fn execute_procedure_scaffold(template: &str) -> RunResult {
    let template =
        ScaffoldTemplate::from_name(template).expect("the parser closes --template to known names");
    let document = scaffold_procedure_v2(template);
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(document.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap_or_else(|error| unreachable!("a scaffold template must parse: {error}"));
    let validated =
        validate_procedure_v2(parsed).expect("a scaffold template is a valid Procedure v2 model");
    let Value::Object(result) = json!({
        "schema": "podway.procedure-source-result/v1",
        "operation": "scaffold",
        "target_schema": "podway.procedure/v2",
        "target_digest": validated.digest().as_str(),
        "document": document,
        "template": template.name(),
    }) else {
        unreachable!("the static source result is a JSON object");
    };
    local_result_v2(PROCEDURE_SCAFFOLD_COMMAND, result, document.to_owned(), 0)
}
