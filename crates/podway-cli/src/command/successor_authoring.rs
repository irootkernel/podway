//! Offline authoring of the successor Procedure model and its preset pairs.

use std::path::Path;

use podway_config::execution_groups::{
    SuccessorDefinition, SuccessorGraphFormat, ValidatedSuccessorDefinition,
    format_successor_definition, lint_successor_definition, parse_successor_document,
    project_successor_graph, render_successor_graph, validate_successor_definition,
};
use podway_config::{
    AuthoringContext, AuthoringStage, FinalizedDiagnostics, FormatRequest, config_error_diagnostic,
    finalize_diagnostics,
};
use podway_core::AuthoringDiagnostic;
use podway_presets::{PresetMetadata, list_v3, lookup_v3};
use serde_json::{Value, json};

use super::{
    LocalFailure, PresetCommand, ProcedureCommand, RunResult, is_worktree_relative_procedure_path,
    local_result_v2, open_descriptor_relative_procedure, open_offline_procedure, preset_failure,
    procedure_config_failure, procedure_format, render_authoring_diagnostics,
    render_preview_start_command, replace_procedure_document,
};

const AUTHORING_SCHEMA: &str = "podway.successor-authoring-result/v1";
const PRESET_SCHEMA: &str = "podway.successor-preset-result/v1";

pub(super) fn execute_procedure(command: &ProcedureCommand) -> Result<RunResult, LocalFailure> {
    let (path, name) = match command {
        ProcedureCommand::Validate { file, .. } => (file, "procedure.validate"),
        ProcedureCommand::Show { file, .. } => (file, "procedure.show"),
        ProcedureCommand::Format { file, .. } => (file, "procedure.format"),
        ProcedureCommand::Vet { file } => (file, "procedure.vet"),
        ProcedureCommand::Graph { file, .. } => (file, "procedure.graph"),
        ProcedureCommand::Preview { file } => (file, "procedure.preview"),
        ProcedureCommand::Lint { file, .. } => (file, "procedure.lint"),
        ProcedureCommand::Check { file, .. } => (file, "procedure.check"),
        ProcedureCommand::Scaffold { template } => return scaffold(template),
    };
    let opened = if matches!(command, ProcedureCommand::Preview { .. }) {
        if !is_worktree_relative_procedure_path(path) || path.to_str().is_none() {
            return Err(LocalFailure::request_invalid(
                "preview requires a safe UTF-8 worktree-relative path",
            )
            .with_command(name));
        }
        open_descriptor_relative_procedure(
            Path::new("."),
            path,
            LocalFailure::catalog("PATH_OUTSIDE_WORKTREE", "cannot open worktree", name),
        )
    } else {
        open_offline_procedure(path)
    }
    .map_err(|failure| failure.with_command(name))?;
    let source = std::str::from_utf8(opened.bytes()).map_err(|_| {
        LocalFailure::procedure_invalid("procedure file is not UTF-8").with_command(name)
    })?;
    let path_text = path.display().to_string();
    let source_format = procedure_format(path);
    let context = AuthoringContext::new(&path_text, source, source_format);
    let admitted = match parse_successor_document(source.as_bytes(), source_format)
        .and_then(validate_successor_definition)
    {
        Ok(admitted) => admitted,
        Err(error) if matches!(command, ProcedureCommand::Show { .. }) => {
            return Err(procedure_config_failure(error).with_command(name));
        }
        Err(error) => {
            return Ok(diagnostics(
                name,
                &path_text,
                None,
                Some((
                    AuthoringStage::Validate,
                    config_error_diagnostic(&error, &context),
                )),
            ));
        }
    };

    let view = json!({
        "file": path_text,
        "document_schema": admitted.definition().schema(),
        "digest": admitted.digest().as_str(),
    });
    match command {
        ProcedureCommand::Show { canonical, .. } => {
            let mut result = view;
            result["canonical_json"] = json!(admitted.canonical_json().as_str());
            let text = if *canonical {
                admitted.canonical_json().as_str()
            } else {
                source
            };
            Ok(output(AUTHORING_SCHEMA, name, result, text.to_owned(), 0))
        }
        ProcedureCommand::Check {
            warnings_as_errors, ..
        } => {
            let format_finding = match format_successor_definition(FormatRequest {
                source,
                source_path: &path_text,
                format: source_format,
            }) {
                Ok(formatted) => formatted.drift_diagnostic(&context),
                Err(error) => Some(config_error_diagnostic(&error, &context)),
            };
            let mut entries = lint_findings(&admitted, &context);
            entries.extend(format_finding.map(|finding| (AuthoringStage::Format, finding)));
            Ok(report_diagnostics(
                name,
                &path_text,
                Some(&admitted),
                entries,
                *warnings_as_errors,
            ))
        }
        ProcedureCommand::Format { check, write, .. } => {
            let formatted = match format_successor_definition(FormatRequest {
                source,
                source_path: &path_text,
                format: source_format,
            }) {
                Ok(formatted) => formatted,
                Err(error) => {
                    return Ok(diagnostics(
                        name,
                        &path_text,
                        Some(&admitted),
                        Some((
                            AuthoringStage::Format,
                            config_error_diagnostic(&error, &context),
                        )),
                    ));
                }
            };
            if *check && let Some(finding) = formatted.drift_diagnostic(&context) {
                return Ok(diagnostics(
                    name,
                    &path_text,
                    Some(&admitted),
                    Some((AuthoringStage::Format, finding)),
                ));
            }
            let changed = *write && formatted.changed();
            if changed {
                replace_procedure_document(&opened, formatted.document().as_bytes())
                    .map_err(|failure| failure.with_command(name))?;
            }
            let (mode, text) = if changed {
                (
                    "write",
                    format!("{path_text} rewritten in canonical authoring form"),
                )
            } else if *write {
                (
                    "write",
                    format!("{path_text} is in canonical authoring form"),
                )
            } else if *check {
                (
                    "check",
                    format!("{path_text} is in canonical authoring form"),
                )
            } else {
                ("stdout", formatted.document().to_owned())
            };
            let mut result = view;
            result["document"] = json!(formatted.document());
            result["mode"] = json!(mode);
            result["changed"] = json!(changed);
            Ok(output(AUTHORING_SCHEMA, name, result, text, 0))
        }
        ProcedureCommand::Graph { format, .. } => {
            Ok(graph(&admitted, &context, name, view, format))
        }
        ProcedureCommand::Preview { .. } => preview(&admitted, &context, name, view),
        ProcedureCommand::Lint {
            warnings_as_errors, ..
        } => Ok(report_diagnostics(
            name,
            &path_text,
            Some(&admitted),
            lint_findings(&admitted, &context),
            *warnings_as_errors,
        )),
        ProcedureCommand::Validate { .. } | ProcedureCommand::Vet { .. } => {
            Ok(diagnostics(name, &path_text, Some(&admitted), None))
        }
        ProcedureCommand::Scaffold { .. } => unreachable!("scaffold has no input path"),
    }
}

fn graph(
    definition: &ValidatedSuccessorDefinition,
    context: &AuthoringContext<'_>,
    name: &str,
    mut result: Value,
    format: &str,
) -> RunResult {
    let rendered = match format {
        "json" => project_successor_graph(definition)
            .map(|graph| (graph.projection().to_owned(), graph.digest().clone())),
        "mermaid" => render_successor_graph(definition, SuccessorGraphFormat::Mermaid),
        "dot" => render_successor_graph(definition, SuccessorGraphFormat::Dot),
        "puml" => render_successor_graph(definition, SuccessorGraphFormat::PlantUml),
        _ => unreachable!("Clap checks graph format"),
    };
    let (projection, digest) = match rendered {
        Ok(value) => value,
        Err(error) => {
            return diagnostics(
                name,
                context.source_path(),
                Some(definition),
                Some((
                    AuthoringStage::Vet,
                    config_error_diagnostic(&error, context),
                )),
            );
        }
    };
    result["format"] = json!(format);
    result["projection"] = json!(projection);
    result["projection_digest"] = json!(digest.as_str());
    output(AUTHORING_SCHEMA, name, result, projection, 0)
}

fn preview(
    definition: &ValidatedSuccessorDefinition,
    context: &AuthoringContext<'_>,
    name: &str,
    mut result: Value,
) -> Result<RunResult, LocalFailure> {
    let header = definition.definition().header();
    let (graph, mermaid) = match project_successor_graph(definition).and_then(|graph| {
        render_successor_graph(definition, SuccessorGraphFormat::Mermaid)
            .map(|(mermaid, _)| (graph, mermaid))
    }) {
        Ok(value) => value,
        Err(error) => {
            return Ok(diagnostics(
                name,
                context.source_path(),
                Some(definition),
                Some((
                    AuthoringStage::Vet,
                    config_error_diagnostic(&error, context),
                )),
            ));
        }
    };
    result["document_id"] = json!(header.id().as_str());
    result["document_version"] = json!(header.version());
    result["purpose"] = json!(header.purpose());
    result["graph"] = json!(graph.projection());
    result["mermaid"] = json!(mermaid);
    let report = finalize_successor_diagnostics(lint_findings(definition, context));
    result["admissible"] = json!(true);
    result["checks"] = json!({"validate": true, "vet": true, "lint": report.total() == 0});
    add_report_diagnostics(&mut result, &report);
    let mut text = format!(
        "{} ({})\n{}\n\n{mermaid}",
        header.name(),
        definition.digest().as_str(),
        header.purpose()
    );
    if matches!(definition.definition(), SuccessorDefinition::Base(_)) {
        let argv = [
            "podway",
            "start",
            "--procedure",
            context.source_path(),
            "--expect-procedure-digest",
            definition.digest().as_str(),
            "--task",
            "<task>",
        ]
        .map(str::to_owned);
        result["start_argv"] = json!(argv);
        text.push_str("\nStart: ");
        text.push_str(&render_preview_start_command(&argv));
    }
    if report.total() != 0 {
        text.push_str("\n\n");
        text.push_str(&render_authoring_diagnostics(
            report.diagnostics(),
            report.truncated(),
            report.total(),
        ));
    }
    Ok(output(AUTHORING_SCHEMA, name, result, text, 0))
}

fn scaffold(template: &str) -> Result<RunResult, LocalFailure> {
    let name = "procedure.scaffold";
    let preset = lookup_v3("small-change-v3").expect("built-in minimal base");
    let definition = preset
        .validate()
        .map_err(|error| preset_failure(error).with_command(name))?;
    let result = json!({
        "template": template,
        "document_schema": definition.base.definition().schema(),
        "digest": definition.base.digest().as_str(),
        "document": preset.base_yaml,
    });
    Ok(output(
        AUTHORING_SCHEMA,
        name,
        result,
        preset.base_yaml.to_owned(),
        0,
    ))
}

pub(super) fn execute_preset(command: &PresetCommand) -> Result<RunResult, LocalFailure> {
    let (id, name) = match command {
        PresetCommand::List => {
            let rows: Vec<_> = list_v3()
                .iter()
                .map(|preset| metadata(preset.metadata))
                .collect();
            let text = list_v3()
                .iter()
                .map(|preset| preset.metadata.id)
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(output(
                PRESET_SCHEMA,
                "preset.list",
                json!({"presets": rows}),
                text,
                0,
            ));
        }
        PresetCommand::Show { name } => (name, "preset.show"),
        PresetCommand::Explain { name } => (name, "preset.explain"),
    };
    let preset = lookup_v3(id)
        .ok_or_else(|| LocalFailure::preset_not_found("unknown preset").with_command(name))?;
    let admitted = preset
        .validate()
        .map_err(|error| preset_failure(error).with_command(name))?;
    let mut result = json!({
        "preset": metadata(preset.metadata),
        "base_digest": admitted.base.digest().as_str(),
        "group_digest": admitted.default_group.digest().as_str(),
    });
    let text = if matches!(command, PresetCommand::Show { .. }) {
        for (key, document) in [
            ("procedure", &admitted.base),
            ("default_group", &admitted.default_group),
        ] {
            result[key] =
                serde_json::from_str(document.canonical_json().as_str()).map_err(|_| {
                    LocalFailure::procedure_invalid("invalid embedded canonical JSON")
                        .with_command(name)
                })?;
        }
        preset.base_yaml.to_owned()
    } else {
        let SuccessorDefinition::Base(base) = admitted.base.definition() else {
            unreachable!("preset admission requires a base")
        };
        result["goal_tracking"] = json!(base.goal_tracking().is_some());
        for (key, document) in [
            ("base_graph", &admitted.base),
            ("default_group_graph", &admitted.default_group),
        ] {
            let projection = project_successor_graph(document)
                .map_err(|error| procedure_config_failure(error).with_command(name))?;
            result[key] = json!(projection.projection());
        }
        let goal = if base.goal_tracking().is_some() {
            "enabled"
        } else {
            "disabled"
        };
        format!(
            "{}\n{}\nGoal tracking: {goal}\nBase digest: {}\nDefault group digest: {}",
            preset.metadata.name,
            preset.metadata.description,
            admitted.base.digest().as_str(),
            admitted.default_group.digest().as_str()
        )
    };
    Ok(output(PRESET_SCHEMA, name, result, text, 0))
}

fn metadata(metadata: PresetMetadata) -> Value {
    json!({"id":metadata.id,"name":metadata.name,"version":metadata.version,"description":metadata.description})
}

fn diagnostics(
    name: &str,
    path: &str,
    definition: Option<&ValidatedSuccessorDefinition>,
    finding: Option<(AuthoringStage, AuthoringDiagnostic)>,
) -> RunResult {
    report_diagnostics(name, path, definition, finding.into_iter().collect(), false)
}

fn lint_findings(
    definition: &ValidatedSuccessorDefinition,
    context: &AuthoringContext<'_>,
) -> Vec<(AuthoringStage, AuthoringDiagnostic)> {
    lint_successor_definition(definition, context)
        .into_iter()
        .map(|finding| (AuthoringStage::Lint, finding))
        .collect()
}

fn finalize_successor_diagnostics(
    entries: Vec<(AuthoringStage, AuthoringDiagnostic)>,
) -> FinalizedDiagnostics {
    finalize_diagnostics(
        entries
            .into_iter()
            .map(|(stage, finding)| (stage, finding.for_successor_authoring()))
            .collect(),
    )
}

fn add_report_diagnostics(result: &mut Value, report: &FinalizedDiagnostics) {
    result["diagnostics"] = json!(report.diagnostics());
    result["diagnostics_total"] = json!(report.total());
    result["diagnostics_truncated"] = json!(report.truncated());
}

fn report_diagnostics(
    name: &str,
    path: &str,
    definition: Option<&ValidatedSuccessorDefinition>,
    entries: Vec<(AuthoringStage, AuthoringDiagnostic)>,
    warnings_as_errors: bool,
) -> RunResult {
    let report = finalize_successor_diagnostics(entries);
    let text = if report.total() == 0 {
        format!(
            "{path}: valid ({})",
            definition.expect("valid definition").digest().as_str()
        )
    } else {
        render_authoring_diagnostics(report.diagnostics(), report.truncated(), report.total())
    };
    let mut result = json!({"file": path, "valid": report.valid()});
    add_report_diagnostics(&mut result, &report);
    if let Some(definition) = definition {
        result["document_schema"] = json!(definition.definition().schema());
        result["digest"] = json!(definition.digest().as_str());
    }
    let exit_code = i32::from(!report.valid() || (warnings_as_errors && report.total() != 0));
    output(AUTHORING_SCHEMA, name, result, text, exit_code)
}

fn output(schema: &str, name: &str, mut result: Value, text: String, exit: i32) -> RunResult {
    result["schema"] = json!(schema);
    result["operation"] = json!(name);
    let Value::Object(result) = result else {
        unreachable!("authoring result is an object");
    };
    local_result_v2(name, result, text, exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn successor_authoring_routes_return_closed_results() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/presets/small-change-v3.yaml");
        let group = base.with_file_name("small-change-default-v3.yaml");
        for command in [
            ProcedureCommand::Validate {
                file: base.clone(),
                warnings_as_errors: true,
            },
            ProcedureCommand::Show {
                file: base,
                canonical: true,
            },
            ProcedureCommand::Graph {
                file: group.clone(),
                format: "mermaid".to_owned(),
            },
            ProcedureCommand::Check {
                file: group,
                warnings_as_errors: true,
            },
            ProcedureCommand::Scaffold {
                template: "minimal".to_owned(),
            },
        ] {
            let RunResult::LocalV2 {
                result, exit_code, ..
            } = execute_procedure(&command).unwrap()
            else {
                panic!("expected a structured authoring result");
            };
            assert_eq!(exit_code, 0);
            assert_eq!(result["schema"], AUTHORING_SCHEMA);
        }
        for command in [
            PresetCommand::List,
            PresetCommand::Show {
                name: "sw-dev-v3".to_owned(),
            },
            PresetCommand::Explain {
                name: "analysis-v3".to_owned(),
            },
        ] {
            let RunResult::LocalV2 {
                result, exit_code, ..
            } = execute_preset(&command).unwrap()
            else {
                panic!("expected a structured preset result");
            };
            assert_eq!(exit_code, 0);
            assert_eq!(result["schema"], PRESET_SCHEMA);
        }
        assert!(
            execute_preset(&PresetCommand::Show {
                name: "sw-dev-v2".to_owned()
            })
            .is_err()
        );
    }
}
