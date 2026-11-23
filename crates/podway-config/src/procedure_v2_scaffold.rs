//! The historical Procedure v2 scaffold and the active Procedure v3 starting point.
//!
//! The Procedure v2 literal follows the constraints from dossier section 11.6 below.
//!
//! A scaffold is a *literal*, not a generator. Each template is a `&'static str` checked into this
//! file, so the document a user receives is the document a reviewer read: there is no rendering
//! step that could interpolate a hostname, a timestamp, or a counter, and determinism is therefore
//! not a property that has to be tested for — it is the type.
//!
//! Each template is also a **fixpoint of the canonical emitter**: `format_procedure_v2(T) == T`
//! byte for byte, which is why `podway procedure scaffold > new.yaml && podway procedure check
//! new.yaml` is quiet. That is an obligation on the literal rather than on any code here, so
//! `int_v2_procedure_scaffold.rs` asserts it directly; a template edited into a form the emitter
//! would rewrite fails that test rather than shipping a document the toolchain immediately
//! complains about.
//!
//! What a template may contain is deliberately narrow. It demonstrates the *shape* of a Procedure —
//! definitions, a graph, items, rework — and nothing about any domain: every guidance string tells
//! the author what belongs in that slot, so the file is honest about being unfinished without
//! containing a workflow nobody asked for. Two consequences follow, and both are load-bearing:
//!
//! - **No `<angle-bracket>` spans.** Section 11.4's weak-guidance rules treat a value that is
//!   entirely a bracketed span as a placeholder, so the obvious `<describe your goal>` idiom would
//!   make the scaffold lint against itself. Templates use imperative sentences instead.
//! - **No decision node.** A decision needs options, per-option criteria, and a reason prompt, and
//!   every one of those would be invented content. Two action nodes demonstrate `next` chaining,
//!   `terminal`, both item kinds the scaffold needs, and `manual_rework` without inventing a single
//!   branch.

/// Every authoring starting point `podway procedure scaffold` can emit.
///
/// Closed on purpose: `--template` is a fixed vocabulary the shell completion, the CLI parser, and
/// this crate all read from [`ScaffoldTemplate::NAMES`], so a template cannot exist in one of those
/// places and not the others.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScaffoldTemplate {
    /// The smallest procedure that still demonstrates the whole authoring surface.
    Minimal,
}

impl ScaffoldTemplate {
    /// Every template name, in the order the CLI offers them.
    ///
    /// An array rather than a slice because Clap's `value_parser` consumes it directly, which is
    /// what keeps the parser's accepted vocabulary and this enum from drifting apart.
    pub const NAMES: [&'static str; 1] = [Self::Minimal.name()];

    /// The `--template` value that selects this template.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
        }
    }

    /// The template a `--template` value names, or `None` when no template has that name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "minimal" => Some(Self::Minimal),
            _ => None,
        }
    }
}

/// The authoring text of one scaffold template.
///
/// The returned document is already in canonical authoring form and already valid, so a caller
/// needs no formatting pass before writing it and no validation pass before trusting it. A caller
/// that wants the digest parses and validates the returned text through the ordinary pipeline
/// rather than being handed a precomputed constant, which keeps one derivation of a digest in the
/// crate instead of two.
pub const fn scaffold_procedure_v2(template: ScaffoldTemplate) -> &'static str {
    match template {
        ScaffoldTemplate::Minimal => SCAFFOLD_TEMPLATE_MINIMAL,
    }
}

/// The `minimal` template: two action nodes, one of each item kind the scaffold needs, and rework.
///
/// Read it as the answer to "what is the least a Procedure v2 document can say and still be a
/// complete example". `perform` does the work and collects a multiline text summary; `confirm` is
/// terminal and collects the single confirmation that ends the session; `manual_rework` names
/// `perform` so a completed session can be sent back. Four full-line comment blocks explain the
/// four regions of the file, and canonical authoring form reattaches all four, so an author who
/// runs `podway procedure format --write` on their edited copy keeps them.
///
/// Every literal here is either structural or an instruction to replace it. The values that lint
/// inspects — `purpose` and both `intent`s — are full sentences of at least two words, so the
/// document that ships is a document `podway procedure check --warnings-as-errors` accepts.
pub const SCAFFOLD_TEMPLATE_MINIMAL: &str = r#"# Podway Procedure v2 scaffold. Replace every identifier and every guidance
# string below with your own; full-line comments survive `podway procedure format`.
schema: podway.procedure/v2
id: scaffold
version: "1"
name: Scaffold
purpose: Describe what this procedure exists to accomplish.
# Node definitions are reusable contracts keyed by a stable identifier. The
# items of a node are the fields an operator fills in while that node is active.
node_definitions:
  perform-work:
    type: action
    title: Perform the work
    intent: Describe the outcome this node must produce.
    instructions:
      - Replace this instruction with the work to perform.
    items:
      - id: summary
        type: text
        prompt: Summarize what was done.
        required: true
        min_length: 0
        max_length: 4000
        multiline: true
  confirm-completion:
    type: action
    title: Confirm completion
    intent: Confirm the work is complete before finishing.
    items:
      - id: complete
        type: confirm
        prompt: The work is complete.
        required: true
# The graph places definitions as nodes and wires the transitions between them.
# One placement is the entry, and every path has to reach a terminal placement.
graph:
  entry: perform
  nodes:
    - id: perform
      use: perform-work
      next: confirm
    - id: confirm
      use: confirm-completion
      terminal: true
# Manual rework lists the placements a completed session may be sent back to.
manual_rework:
  allowed_targets:
    - perform
"#;

/// Canonical minimal Procedure v3 starting point with its own authoring identity.
pub const SCAFFOLD_TEMPLATE_MINIMAL_V3: &str = r#"schema: podway.procedure/v3
id: custom-procedure-v3
version: "1"
name: Custom Procedure
purpose: Replace this purpose with the outcome your Procedure delivers.
description: A minimal Procedure v3 starting point derived from small-change-v3.
node_definitions:
  inspect-change:
    type: action
    title: Inspect the change boundary
    intent: Establish the exact scope and constraints before editing.
    instructions:
      - "Accountable role: Coordinator. Use the current selected evidence and record the declared outputs against that basis."
      - Inspect the relevant implementation, tests, and authoritative contracts.
      - Record the bounded scope and constraints in English.
      - Report actual negative or incomplete results in the declared output fields. If a required result cannot be supplied, record a blocker with the missing facts.
      - Select a validated group proposal only after recording the planning basis. Activate it explicitly; when revising a plan, reconcile retained work and current evidence before activating a fresh group.
    items:
      - id: scope-summary
        type: text
        prompt: Summarize the change scope and constraints in English.
        required: true
        min_length: 1
        max_length: 4000
        multiline: true
  closeout-change:
    type: action
    title: Record the closeout
    intent: Record the terminal outcome of the small change.
    instructions:
      - "Accountable role: Coordinator. Use the current selected evidence and record the declared outputs against that basis."
      - Report actual negative or incomplete results in the declared output fields. If a required result cannot be supplied, record a blocker with the missing facts.
    items:
      - id: closeout-note
        type: text
        prompt: Record the final closeout in English.
        required: true
        min_length: 1
        max_length: 2000
        multiline: true
graph:
  entry: inspect
  nodes:
    - id: inspect
      use: inspect-change
      next: closeout
    - id: closeout
      use: closeout-change
      evidence_from:
        - scope: execution_result
          required: true
          items:
            - implementation-summary
            - verification-command
            - verification-exit-status
      terminal: true
execution_group:
  anchor: inspect
  success_return: closeout
  plan_inputs:
    - scope-summary
  result_items:
    - id: implementation-summary
      type: text
      prompt: Summarize the implemented change in English.
      required: true
      min_length: 1
      max_length: 4000
      multiline: true
    - id: verification-command
      type: text
      prompt: Record the caller-asserted verification command.
      help: Podway records this assertion and does not execute or attest the command. Use a check_result item when structural operation binding is required.
      required: true
      min_length: 1
      max_length: 2000
      multiline: true
    - id: verification-exit-status
      type: integer
      prompt: Record the caller-asserted verification exit status.
      help: Podway records this assertion and does not determine whether the command ran or the status is semantically acceptable.
      required: true
  replan_decisions: []
"#;

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::SCAFFOLD_TEMPLATE_MINIMAL_V3;
    use crate::{
        AuthoringContext, FormatRequest, ParsedProcedure, ProcedureDocumentFormat,
        execution_groups::{
            SuccessorDefinition, format_successor_definition, lint_successor_definition,
            parse_successor_document, project_successor_graph, validate_successor_definition,
        },
        parse_procedure_document, validate_procedure_v2,
    };

    #[test]
    fn minimal_v3_scaffold_is_canonical_admissible_and_distinct_from_presets() {
        let source = SCAFFOLD_TEMPLATE_MINIMAL_V3;
        let format = ProcedureDocumentFormat::Yaml;
        let file = "scaffold.yaml";
        let context = AuthoringContext::new(file, source, format);
        let definition = validate_successor_definition(
            parse_successor_document(source.as_bytes(), format).unwrap(),
        )
        .unwrap();
        let SuccessorDefinition::Base(base) = definition.definition() else {
            panic!("the minimal scaffold must be a base Procedure");
        };
        assert_eq!(base.header().id().as_str(), "custom-procedure-v3");
        let formatted = format_successor_definition(FormatRequest {
            source,
            source_path: file,
            format,
        })
        .unwrap();
        assert_eq!(formatted.document(), source);
        assert_eq!(formatted.digest(), definition.digest());
        assert!(!formatted.changed());
        assert!(lint_successor_definition(&definition, &context).is_empty());
        project_successor_graph(&definition).unwrap();

        let preset_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets");
        let small_change = fs::read_to_string(preset_root.join("small-change-v3.yaml")).unwrap();
        let expected = small_change
            .replace("id: small-change-v3\n", "id: custom-procedure-v3\n")
            .replace("name: Small Change v3\n", "name: Custom Procedure\n")
            .replace(
                "purpose: Deliver a small reviewed change with fresh caller-asserted verification evidence.\n",
                "purpose: Replace this purpose with the outcome your Procedure delivers.\n",
            )
            .replace(
                "description: The lightweight assertion-only reference Procedure without goal tracking or typed external results.\n",
                "description: A minimal Procedure v3 starting point derived from small-change-v3.\n",
            );
        assert_eq!(source, expected);

        for entry in fs::read_dir(preset_root).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let bytes = fs::read(&path).unwrap();
            let digest = if path.file_stem().unwrap().to_string_lossy().ends_with("-v2") {
                let ParsedProcedure::V2(parsed) = parse_procedure_document(&bytes, format).unwrap();
                validate_procedure_v2(parsed)
                    .unwrap()
                    .digest()
                    .as_str()
                    .to_owned()
            } else {
                validate_successor_definition(parse_successor_document(&bytes, format).unwrap())
                    .unwrap()
                    .digest()
                    .as_str()
                    .to_owned()
            };
            assert_ne!(definition.digest().as_str(), digest, "{path:?}");
        }
    }
}
