//! Authoring-diagnostic emission: source context, `ConfigError` classification, and the bounded,
//! deterministically ordered result every authoring command reports (dossier sections 11.1 and
//! 11.6).
//!
//! `podway-core` owns the diagnostic *value*; this module owns turning a config-layer failure into
//! one. Three pieces:
//!
//! - [`AuthoringContext`] carries the source document and lazily builds its path index, so a
//!   command that emits no diagnostic never pays for the index and a command that emits many builds
//!   it once.
//! - [`config_error_diagnostic`] classifies a [`ConfigError`] into the catalog. Parsing and
//!   validation are single-error stages by design, so this converts exactly one failure.
//! - [`finalize_diagnostics`] merges the stages into one bounded, sorted result.
//!
//! **Location policy, stable and documented.** A diagnostic spans from its anchor's first character
//! to the end of that source line. When the exact path is absent from the source — an omitted
//! `manual_rework`, a shape-only `ConfigError` field — the longest present prefix of the path is
//! used, and the document start `(1, 1, 1, 1)` is the final fallback. Locations degrade; they never
//! fail.

use std::cell::OnceCell;

use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode, AuthoringSeverity, MAX_AUTHORING_DIAGNOSTICS,
    SourceLocation,
};

use crate::procedure_v2_canonical::CANONICAL_PROJECTION_FIELD;
use crate::procedure_v2_source::{FieldPath, SourceIndex, build_source_index};
use crate::{ConfigError, ProcedureDocumentFormat};

/// The number of characters of a `ConfigError`'s own text embedded in a diagnostic message. Leaves
/// room for the leading sentence inside the schema's 512-character message bound.
const MAX_EMBEDDED_DETAIL_CHARS: usize = 320;

/// Message prefixes owned by the wire deserializer and the schema dispatch.
///
/// `serde`'s `de::Error` constructors produce exactly this closed set of shapes, so an
/// `InvalidDocument` carrying one of them is a violation of the closed v2 authoring schema; every
/// other `InvalidDocument` comes from the shared bounded decoder (UTF-8, multi-document, trailing
/// data, JSON syntax, explicit null, YAML binary, non-string mapping keys, JSON-form-as-YAML) and
/// is an unsupported source construct.
const SCHEMA_REASON_PREFIXES: &[&str] = &[
    "unknown field ",
    "unknown variant ",
    "missing field ",
    "duplicate field ",
    "invalid type: ",
    "invalid value: ",
    "invalid length ",
    "procedure schema must be a string",
    "procedure document must declare a schema",
];

/// One source document under authoring analysis.
///
/// Holds no filesystem handle and performs no I/O: `podway-config` never touches the filesystem.
/// `source_path` is a label the caller supplies for the diagnostic's `source_path` field.
pub struct AuthoringContext<'a> {
    source_path: &'a str,
    source: &'a str,
    format: ProcedureDocumentFormat,
    index: OnceCell<SourceIndex>,
}

impl<'a> AuthoringContext<'a> {
    pub fn new(source_path: &'a str, source: &'a str, format: ProcedureDocumentFormat) -> Self {
        Self {
            source_path,
            source,
            format,
            index: OnceCell::new(),
        }
    }

    pub fn source_path(&self) -> &'a str {
        self.source_path
    }

    pub fn source(&self) -> &'a str {
        self.source
    }

    pub const fn format(&self) -> ProcedureDocumentFormat {
        self.format
    }

    pub(crate) fn index(&self) -> &SourceIndex {
        self.index.get_or_init(|| build_source_index(self.source))
    }

    /// The span of a structural path, falling back to its longest present prefix and finally to the
    /// document start.
    pub(crate) fn locate(&self, path: &FieldPath) -> SourceLocation {
        let index = self.index();
        let mut candidate = Some(path.clone());
        while let Some(current) = candidate {
            if let Some((line, column)) = index.position(&current) {
                return self.span(line, column);
            }
            candidate = current.parent();
        }
        SourceLocation::document_start()
    }

    /// The span of a dotted authored field shape such as `graph.nodes.use`, resolved through the
    /// same prefix rule. Array indices are unknown at this level, so the enclosing array anchors it.
    pub(crate) fn locate_field(&self, field: &str) -> SourceLocation {
        let path = field
            .split('.')
            .filter(|segment| !segment.is_empty())
            .fold(FieldPath::root(), |path, segment| path.child_key(segment));
        self.locate(&path)
    }

    /// The span from one source position to the end of its line.
    pub(crate) fn span(&self, line: u32, column: u32) -> SourceLocation {
        let end_column = self.index().line_length(line).saturating_add(1);
        SourceLocation::new(line, column, line, end_column)
    }
}

/// The authoring stage a diagnostic came from.
///
/// Carried out-of-band because it is not a schema field: it exists only to order the report so it
/// reads as the section 11.5 pipeline (`format --check`, then validate, then vet, then lint) even
/// though execution order differs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthoringStage {
    Format,
    Validate,
    Vet,
    Lint,
}

impl AuthoringStage {
    const fn rank(self) -> u8 {
        match self {
            Self::Format => 0,
            Self::Validate => 1,
            Self::Vet => 2,
            Self::Lint => 3,
        }
    }
}

/// A bounded, sorted diagnostic report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FinalizedDiagnostics {
    diagnostics: Vec<AuthoringDiagnostic>,
    total: u32,
    truncated: bool,
}

impl FinalizedDiagnostics {
    /// The retained diagnostics, at most [`MAX_AUTHORING_DIAGNOSTICS`].
    pub fn diagnostics(&self) -> &[AuthoringDiagnostic] {
        &self.diagnostics
    }

    /// The count before truncation.
    pub const fn total(&self) -> u32 {
        self.total
    }

    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// True when no diagnostic has error severity. Describes the procedure, not the invocation's
    /// warning policy.
    pub fn valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity() == AuthoringSeverity::Error)
    }

    pub fn into_diagnostics(self) -> Vec<AuthoringDiagnostic> {
        self.diagnostics
    }
}

/// Merges every stage's diagnostics into one bounded report.
///
/// Sorted by `(stage, line, column, code, field)` with a stable sort, so diagnostics a rule emits
/// in a meaningful order keep it on a full tie. `total` counts before truncation, so a client can
/// always tell that it is seeing a prefix.
pub fn finalize_diagnostics(
    entries: Vec<(AuthoringStage, AuthoringDiagnostic)>,
) -> FinalizedDiagnostics {
    let total = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    let truncated = entries.len() > MAX_AUTHORING_DIAGNOSTICS;

    let mut entries = entries;
    entries.sort_by(|(left_stage, left), (right_stage, right)| {
        (
            left_stage.rank(),
            left.location().line(),
            left.location().column(),
            left.code().as_str(),
            left.field(),
        )
            .cmp(&(
                right_stage.rank(),
                right.location().line(),
                right.location().column(),
                right.code().as_str(),
                right.field(),
            ))
    });
    entries.truncate(MAX_AUTHORING_DIAGNOSTICS);

    FinalizedDiagnostics {
        diagnostics: entries
            .into_iter()
            .map(|(_, diagnostic)| diagnostic)
            .collect(),
        total,
        truncated,
    }
}

/// Converts one `ConfigError` into its authoring diagnostic.
///
/// The base classification: everything the closed v2 authoring schema rejects becomes
/// `AUTHORING_SCHEMA_INVALID`, everything the shared bounded decoder rejects becomes
/// `SOURCE_CONSTRUCT_UNSUPPORTED`, and the canonical projection bound becomes
/// `SOURCE_PROJECTION_BUDGET_EXCEEDED`. V2AUT-008 refines the closed-reference variants into their
/// specific graph codes; until then they classify as schema violations, which is a true statement
/// about a document whose references do not close, never a wrong one.
///
/// Never panics and never falls through silently: an unreachable variant classifies defensively as
/// a schema violation rather than being dropped.
pub fn config_error_diagnostic(
    error: &ConfigError,
    context: &AuthoringContext<'_>,
) -> AuthoringDiagnostic {
    let classification = classify(error);
    let field = classification.field.unwrap_or("$");
    let location = classification
        .value
        .and_then(|value| context.index().find_scalar(field, value))
        .map_or_else(
            || match classification.field {
                Some(field) => context.locate_field(field),
                None => SourceLocation::document_start(),
            },
            |(line, column)| context.span(line, column),
        );

    AuthoringDiagnostic::new(
        classification.code,
        context.source_path(),
        location,
        field,
        format!(
            "{} {}.",
            lead_sentence(classification.code),
            bounded_detail(&error.to_string())
        ),
        diagnostic_hint(classification.code),
    )
}

struct Classification<'a> {
    code: AuthoringDiagnosticCode,
    /// The authored field shape the error names, when it names one.
    field: Option<&'a str>,
    /// The offending scalar text, used to recover a precise span from a shape-only field.
    value: Option<&'a str>,
}

fn classify(error: &ConfigError) -> Classification<'_> {
    match error {
        ConfigError::InvalidSchema { .. } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: Some("schema"),
            value: None,
        },
        ConfigError::InvalidIdentifier { field, value } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: authored_field(field),
            value: Some(value.as_str()),
        },
        ConfigError::InvalidValue { field, .. } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: authored_field(field),
            value: None,
        },
        // The canonical projection bound is a document-level budget, not an authored field, so it
        // reports the document root. `CANONICAL_PROJECTION_FIELD` is shared with the bound check
        // itself so the two cannot drift.
        ConfigError::OutOfBounds { field, .. } if *field == CANONICAL_PROJECTION_FIELD => {
            Classification {
                code: AuthoringDiagnosticCode::SourceProjectionBudgetExceeded,
                field: None,
                value: None,
            }
        }
        ConfigError::OutOfBounds { field, .. } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: authored_field(field),
            value: None,
        },
        ConfigError::DuplicateValue { field, value } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: authored_field(field),
            value: Some(value.as_str()),
        },
        ConfigError::UnknownV2Reference { field, value } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: authored_field(field),
            value: Some(value.as_str()),
        },
        ConfigError::V2ShapeMismatch { field, .. } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: authored_field(field),
            value: None,
        },
        ConfigError::DuplicateKey { key } => Classification {
            code: AuthoringDiagnosticCode::SourceConstructUnsupported,
            field: None,
            value: Some(key.as_str()),
        },
        ConfigError::UnsupportedYamlFeature { .. }
        | ConfigError::NonCanonicalNumber
        | ConfigError::InputTooLarge { .. }
        | ConfigError::InputTooDeep { .. }
        | ConfigError::InputTooComplex { .. } => Classification {
            code: AuthoringDiagnosticCode::SourceConstructUnsupported,
            field: None,
            value: None,
        },
        ConfigError::InvalidDocument { reason } => Classification {
            code: if SCHEMA_REASON_PREFIXES
                .iter()
                .any(|prefix| reason.starts_with(prefix))
            {
                AuthoringDiagnosticCode::AuthoringSchemaInvalid
            } else {
                AuthoringDiagnosticCode::SourceConstructUnsupported
            },
            field: None,
            value: None,
        },
        // v1-only variants and canonicalization failures are unreachable from the v2 stages. They
        // classify defensively rather than panicking: a production diagnostic path must not be able
        // to abort the process.
        ConfigError::Serialization(_)
        | ConfigError::InvalidDigest
        | ConfigError::UnknownReturnTarget { .. }
        | ConfigError::WarningsAsErrors { .. }
        | ConfigError::CoreAdmission { .. } => Classification {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field: None,
            value: None,
        },
    }
}

/// Drops the `"procedure v2"` sentinel `map_domain_error` uses when a core constructor rejects a
/// value without naming an authored path; there is no field to point at, so the document root is
/// the honest answer.
fn authored_field(field: &'static str) -> Option<&'static str> {
    (field != "procedure v2").then_some(field)
}

fn lead_sentence(code: AuthoringDiagnosticCode) -> &'static str {
    match code {
        AuthoringDiagnosticCode::SourceConstructUnsupported => {
            "The source uses an unsupported YAML or JSON construct:"
        }
        AuthoringDiagnosticCode::SourceProjectionBudgetExceeded => {
            "The canonical source projection exceeds its budget:"
        }
        _ => "The Procedure source violates the closed v2 authoring schema:",
    }
}

fn bounded_detail(detail: &str) -> String {
    let detail = detail.trim_end_matches('.');
    if detail.chars().count() > MAX_EMBEDDED_DETAIL_CHARS {
        detail.chars().take(MAX_EMBEDDED_DETAIL_CHARS).collect()
    } else {
        detail.to_owned()
    }
}

/// The stable remediation hint for a diagnostic code.
///
/// One hint per code, exhaustively: adding a code to the catalog forces a hint for it here rather
/// than silently reaching a default.
pub(crate) const fn diagnostic_hint(code: AuthoringDiagnosticCode) -> &'static str {
    use AuthoringDiagnosticCode as Code;
    match code {
        Code::AuthoringSchemaInvalid => {
            "Correct the reported field against assets/schemas/procedure-v2.schema.json."
        }
        Code::SourceConstructUnsupported => {
            "Rewrite the construct as a single-line value with full-line comments only."
        }
        Code::SourceProjectionBudgetExceeded => {
            "Split the procedure or shorten its longest values to fit the source projection budget."
        }
        Code::FormatNotCanonical => {
            "Run `podway procedure format <file> --write` to rewrite the file in canonical form."
        }
        Code::EntryNodeInvalid => "Set graph.entry to the identifier of a declared graph node.",
        Code::GraphDefinitionUnknown => {
            "Point the placement's `use` at a declared node_definitions key."
        }
        Code::RouteTargetNotFound => "Point the transition at a declared graph node identifier.",
        Code::UnreachableGraphNode => "Add a transition that reaches the node, or remove the node.",
        Code::NoTerminalPath => "Give the region a path to a terminal action.",
        Code::ActionDispositionInvalid => {
            "Declare exactly one of `next` or `terminal: true` on the action placement."
        }
        Code::DecisionOptionRouteMissing => "Add one route for every declared decision option.",
        Code::DecisionRouteOptionUndefined => {
            "Remove the route, or declare the option on the decision definition."
        }
        Code::GoalAssessmentOptionUnmapped => {
            "Map every declared option to one of achieved, not_achieved, or superseded."
        }
        Code::GoalAssessmentOutcomeUnknown => {
            "Use one of achieved, not_achieved, or superseded as the mapped outcome."
        }
        Code::GoalAssessmentOutcomeUnreachable => {
            "Add an option that maps to the unreachable goal outcome."
        }
        Code::GoalAssessmentRequiresGoalTracking => {
            "Declare `goal_tracking: true` at the procedure root, or remove the assessment."
        }
        Code::GoalAssessmentNotDominatingTerminal => {
            "Place a session-goal assessment on every path to the terminal action."
        }
        Code::EvidenceSourceUnknown => {
            "Point the evidence reference at a declared graph node identifier."
        }
        Code::EvidenceSourceSelfReference => {
            "Reference a different placement; a node cannot read back its own evidence."
        }
        Code::EvidenceSourceDoesNotDominateConsumer => {
            "Reference a placement that dominates this node, or mark the reference optional."
        }
        Code::SkippableEvidenceSource => {
            "Remove the source's skip policy, or mark the evidence reference optional."
        }
        Code::EvidenceSelectorUnknownItem => {
            "Select an item the source placement's definition declares."
        }
        Code::ReadbackBudgetExceeded => {
            "Select fewer evidence items, or shorten the recorded items they read back."
        }
        Code::NextStaticBudgetExceeded => {
            "Shorten the node's instructions, prompts, and help text."
        }
        Code::DecisionSkipNotAllowed => "Remove the skip policy from the decision placement.",
        Code::GraphCycleInvalid => {
            "Reduce the region to the single rework cycle the v2 graph rule allows."
        }
        Code::ReworkTargetNotDominating => {
            "Point the rework route at a node that dominates the decision."
        }
        Code::ManualReworkTargetUnknown => {
            "List only declared graph node identifiers in manual_rework.allowed_targets."
        }
        Code::AmbiguousGraphReference => "Rename one of the colliding identifiers.",
        Code::UnusedNodeDefinition => {
            "Place the definition in the graph, or delete the definition."
        }
        Code::SingleOptionDecision => {
            "Add a second option, or replace the decision with an action."
        }
        Code::IndistinguishableOptionLabels => {
            "Rewrite the labels so a reader can tell the options apart."
        }
        Code::IdenticalEffectiveRoutes => {
            "Give the options different targets or effects, or merge them."
        }
        Code::WeakPurposeGuidance => "State what the procedure exists to accomplish.",
        Code::WeakIntentGuidance => "State the outcome this node must produce.",
        Code::WeakObjectiveGuidance => "State what the decision must determine.",
        Code::WeakPromptGuidance => "Ask the decision as a concrete question.",
        Code::WeakCriteriaGuidance => "State when this option applies.",
        Code::WeakReasonGuidance => "Ask for the specific rationale the reason must record.",
        Code::EvidenceGuidanceMissing => {
            "Add evidence_from references, or describe the evidence in evidence_guidance."
        }
        Code::OptionalEvidenceUnresolvable => {
            "Remove the reference, or add a path from the source to this node."
        }
        Code::GoalClarificationPathMissing => {
            "Add an early node that records or decides the session goal."
        }
        Code::GoalAssessmentTooEarly => "Move the assessment closer to the terminal actions.",
        Code::ManualReworkTargetsBroad => {
            "Narrow manual_rework.allowed_targets to the nodes rework should reach."
        }
        Code::LargeOptionSet => "Split the decision, or group the options into fewer choices.",
        Code::LargeCycle => "Split the rework region into smaller loops.",
        Code::DuplicatedNodeDefinition => {
            "Reuse one definition from both placements instead of duplicating it."
        }
        Code::GraphNodeIdConfusing => "Rename one identifier so the two read differently.",
        Code::ReworkTopologyConfusing => {
            "Route the decision's rework options to one target, and never to itself."
        }
        Code::NoReactivationPath => {
            "Add manual_rework.allowed_targets so a completed session can be reopened."
        }
        Code::GoalRevisionTargetUnsafe => {
            "Choose a target whose every path to a terminal passes a goal assessment."
        }
        Code::MultipleGoalAssessmentSources => {
            "Reference one goal-assessment source so the read-back has one authority."
        }
    }
}
