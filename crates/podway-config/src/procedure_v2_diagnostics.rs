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
//!   validation are single-error stages by design, so this converts exactly one failure. It is the
//!   production mapping: no other module decides what code a parse or validate rejection reports.
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
use crate::procedure_v2_graph_projection::GRAPH_PROJECTION_FIELD;
use crate::procedure_v2_parse::{
    ACTION_OUTCOME_ABSENT_REASON, ACTION_OUTCOME_BOTH_REASON, DECISION_SKIP_REASON,
    DOMAIN_SENTINEL_FIELD, PLACEMENT_FIELD,
};
use crate::procedure_v2_source::{FieldPath, SHAPE_WILDCARD, SourceIndex, build_source_index};
use crate::procedure_v2_validate::{
    ACTION_PLACEMENT_KIND_REASON, ASSESSMENT_FIELD, ASSESSMENT_OUTCOME_UNDECLARED_OPTION_REASON,
    ASSESSMENT_OUTCOMES_FIELD, ASSESSMENT_REQUIRES_GOAL_TRACKING_REASON,
    DECISION_PLACEMENT_KIND_REASON, EVIDENCE_SELECTOR_FIELD, EVIDENCE_SELF_REFERENCE_REASON,
    EVIDENCE_SOURCE_FIELD, MANUAL_REWORK_TARGETS_FIELD, OPTION_ROUTE_MISSING_REASON,
    PLACEMENT_NEXT_FIELD, PLACEMENT_ROUTE_TARGET_FIELD, PLACEMENT_ROUTES_FIELD,
    PLACEMENT_USE_FIELD, ROUTE_OPTION_UNDECLARED_REASON,
};
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

    /// The span of the first leaf matching a structural `shape` whose scalar text is `value`.
    ///
    /// A `ConfigError` that names a shape carries no index, so the offending value is what
    /// identifies which of the shape's occurrences the author must edit. `None` when the source
    /// does not spell the value out at that shape at all — a converted document reporting a v1
    /// source path, say — leaving the caller to fall back to the prefix rule.
    pub(crate) fn locate_shape(&self, shape: &[&str], value: &str) -> Option<SourceLocation> {
        self.index()
            .locate_shape(shape, value)
            .map(|(line, column)| self.span(line, column))
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

// ---------------------------------------------------------------------------------------------
// `ConfigError` classification
// ---------------------------------------------------------------------------------------------

/// The authored field an entry-node rejection reports.
///
/// The rejection reaches this module from `ProcedureGraphV2::new` as a `DomainError::InvalidState`,
/// which `map_domain_error` funnels under the [`DOMAIN_SENTINEL_FIELD`] sentinel — a marker, not a
/// path. `graph.entry` is the one authored position the reason can be about, so the diagnostic names
/// and locates it there instead of at the document root.
const GRAPH_ENTRY_FIELD: &str = "graph.entry";

/// Structural shapes used to recover a precise span from a shape-plus-value `ConfigError`.
///
/// Each mirrors the dotted field constant its `ConfigError` carries, with a wildcard wherever the
/// authored document has an array index or an author-chosen map key. Matching the whole path means
/// a value can only answer for the position the error is actually about.
const SHAPE_PLACEMENT_USE: &[&str] = &["graph", "nodes", SHAPE_WILDCARD, "use"];
const SHAPE_PLACEMENT_NEXT: &[&str] = &["graph", "nodes", SHAPE_WILDCARD, "next"];
const SHAPE_ROUTE_TARGET: &[&str] = &[
    "graph",
    "nodes",
    SHAPE_WILDCARD,
    "routes",
    SHAPE_WILDCARD,
    "to",
];
const SHAPE_EVIDENCE_SOURCE: &[&str] = &[
    "graph",
    "nodes",
    SHAPE_WILDCARD,
    "evidence_from",
    SHAPE_WILDCARD,
    "node",
];
const SHAPE_MANUAL_REWORK_TARGET: &[&str] = &["manual_rework", "allowed_targets", SHAPE_WILDCARD];

/// Converts one `ConfigError` into its authoring diagnostic.
///
/// This is the production `ConfigError` → catalog mapping: every rejection the v2 parse and
/// validate stages can raise lands on the one code in
/// `assets/specifications/authoring-diagnostics.json` that describes it, or — where no code fits —
/// on `AUTHORING_SCHEMA_INVALID`, which is a true statement about any document those stages refuse.
///
/// **Nothing here matches a free-typed string.** A refinement keys on the same `field` or `reason`
/// constant its raise site reads, so a check cannot change what it reports without moving the
/// constant this switches on. The constants live next to their raise sites in
/// `procedure_v2_validate`, `procedure_v2_parse`, `procedure_v2_canonical`,
/// `procedure_v2_graph_projection`, and `podway_core`.
///
/// Never panics and never falls through silently: the match over `ConfigError` is exhaustive, and
/// the variants unreachable from v2 classify defensively rather than aborting a diagnostic path.
pub fn config_error_diagnostic(
    error: &ConfigError,
    context: &AuthoringContext<'_>,
) -> AuthoringDiagnostic {
    let classification = classify(error);
    let field = classification.field.unwrap_or("$");
    let location = locate_classification(&classification, field, context);

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

/// Resolves a classification to a span, most precise rule first.
///
/// A declared shape plus the offending value pins the exact leaf — the `to:` naming a missing graph
/// node, not the route table holding it. Without a shape, the value still narrows a dotted field by
/// its last segment. Without either, the field's longest present prefix answers, and the document
/// start is the final fallback. Every step degrades; none can fail.
fn locate_classification(
    classification: &Classification<'_>,
    field: &str,
    context: &AuthoringContext<'_>,
) -> SourceLocation {
    if let Some(value) = classification.value
        && let Some(location) = classification
            .shape
            .and_then(|shape| context.locate_shape(shape, value))
            .or_else(|| {
                classification
                    .field
                    .and_then(|field| context.index().find_scalar(field, value))
                    .map(|(line, column)| context.span(line, column))
            })
    {
        return location;
    }
    match classification.field {
        Some(_) => context.locate_field(field),
        None => SourceLocation::document_start(),
    }
}

struct Classification<'a> {
    code: AuthoringDiagnosticCode,
    /// The authored field shape the diagnostic reports, when there is one to report.
    field: Option<&'a str>,
    /// The structural shape whose leaves can carry `value`, when the field names one.
    shape: Option<&'static [&'static str]>,
    /// The offending scalar text, used to recover a precise span from a shape-only field.
    value: Option<&'a str>,
}

impl<'a> Classification<'a> {
    const fn schema_invalid(field: Option<&'a str>) -> Self {
        Self {
            code: AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            field,
            shape: None,
            value: None,
        }
    }

    const fn at(code: AuthoringDiagnosticCode, field: &'a str) -> Self {
        Self {
            code,
            field: Some(field),
            shape: None,
            value: None,
        }
    }

    const fn with_value(mut self, value: &'a str) -> Self {
        self.value = Some(value);
        self
    }

    const fn with_shape(mut self, shape: &'static [&'static str]) -> Self {
        self.shape = Some(shape);
        self
    }
}

fn classify(error: &ConfigError) -> Classification<'_> {
    use AuthoringDiagnosticCode as Code;

    match error {
        ConfigError::InvalidSchema { .. } => Classification::schema_invalid(Some("schema")),
        ConfigError::InvalidIdentifier { field, value } => {
            Classification::schema_invalid(authored_field(field)).with_value(value.as_str())
        }
        ConfigError::InvalidValue { field, reason } => classify_invalid_value(field, reason),
        // The canonical projection bound is a document-level budget, not an authored field, so it
        // reports the document root. `CANONICAL_PROJECTION_FIELD` is shared with the bound check
        // itself so the two cannot drift.
        ConfigError::OutOfBounds { field, .. } if *field == CANONICAL_PROJECTION_FIELD => {
            Classification {
                code: Code::SourceProjectionBudgetExceeded,
                field: None,
                shape: None,
                value: None,
            }
        }
        // The generated graph text is a separate complete-document projection from canonical
        // source, so its identical numeric cap has a distinct stable diagnostic code.
        ConfigError::OutOfBounds { field, .. } if *field == GRAPH_PROJECTION_FIELD => {
            Classification {
                code: Code::GraphProjectionBudgetExceeded,
                field: None,
                shape: None,
                value: None,
            }
        }
        ConfigError::OutOfBounds { field, .. } => {
            Classification::schema_invalid(authored_field(field))
        }
        // Verified unreachable from the v2 stages: every `DuplicateValue` raise site is in the v1
        // and workspace validators (`procedure_paths`, `stage.id`, `item.id`,
        // `rework.allow_return_to`, `item.choice.choices`, `item.artifact.allowed_media_types`).
        // v2 uniqueness is enforced by the core constructors, which report `InvalidState`, and
        // duplicate authoring-map keys are refused by the bounded decoder as `DuplicateKey`. No
        // `field` here can pin a v2 meaning, so it stays generic rather than guessing one.
        ConfigError::DuplicateValue { field, value } => {
            Classification::schema_invalid(authored_field(field)).with_value(value.as_str())
        }
        ConfigError::UnknownV2Reference { field, value } => {
            classify_unknown_reference(field, value)
        }
        ConfigError::V2ShapeMismatch { field, reason } => classify_shape_mismatch(field, reason),
        ConfigError::DuplicateKey { key } => Classification {
            code: Code::SourceConstructUnsupported,
            field: None,
            shape: None,
            value: Some(key.as_str()),
        },
        ConfigError::UnsupportedYamlFeature { .. }
        | ConfigError::NonCanonicalNumber
        | ConfigError::InputTooLarge { .. }
        | ConfigError::InputTooDeep { .. }
        | ConfigError::InputTooComplex { .. } => Classification {
            code: Code::SourceConstructUnsupported,
            field: None,
            shape: None,
            value: None,
        },
        ConfigError::InvalidDocument { reason } => Classification {
            code: if SCHEMA_REASON_PREFIXES
                .iter()
                .any(|prefix| reason.starts_with(prefix))
            {
                Code::AuthoringSchemaInvalid
            } else {
                Code::SourceConstructUnsupported
            },
            field: None,
            shape: None,
            value: None,
        },
        // Canonicalization failures are unreachable from the v2 authoring stages. They classify
        // defensively rather than panicking: a production diagnostic path must not abort.
        ConfigError::Serialization(_)
        | ConfigError::InvalidDigest
        | ConfigError::CoreAdmission { .. } => Classification::schema_invalid(None),
    }
}

/// Refines `InvalidValue`, whose `field` is often the [`DOMAIN_SENTINEL_FIELD`] marker.
///
/// A core constructor's rejection arrives with no authored path and one static reason, so the reason
/// is the only thing that can distinguish it. Three of those reasons name a condition the catalog
/// has an exact code for, and one parse-level pair does; each is matched as the `(field, reason)`
/// tuple its raise site produces, against the very constants that raise site reads.
///
/// `graph.nodes.terminal` / `"terminal must be true"` deliberately stays generic: the field pins a
/// scalar whose authored value violates the schema's `const: true`, which is a value violation
/// rather than a claim about which disposition the placement declared.
fn classify_invalid_value(field: &'static str, reason: &'static str) -> Classification<'static> {
    use AuthoringDiagnosticCode as Code;

    match (field, reason) {
        (DOMAIN_SENTINEL_FIELD, podway_core::GRAPH_ENTRY_ABSENT_REASON) => {
            Classification::at(Code::EntryNodeInvalid, GRAPH_ENTRY_FIELD)
        }
        // Two placements sharing an identifier make every reference naming it resolve to both.
        // The core constructor reports the collision without naming the identifier, so the
        // diagnostic anchors the placement array rather than inventing a target.
        (DOMAIN_SENTINEL_FIELD, podway_core::GRAPH_NODE_ID_NOT_UNIQUE_REASON) => {
            Classification::at(Code::AmbiguousGraphReference, PLACEMENT_FIELD)
        }
        // `GoalOutcome::from_str` is reached from exactly one authored position in the v2 parse
        // pipeline — an `assessment.outcomes` mapping value — so the reason pins what the sentinel
        // field cannot.
        (DOMAIN_SENTINEL_FIELD, podway_core::UNKNOWN_GOAL_OUTCOME_REASON) => Classification::at(
            Code::GoalAssessmentOutcomeUnknown,
            ASSESSMENT_OUTCOMES_FIELD,
        ),
        (PLACEMENT_FIELD, ACTION_OUTCOME_BOTH_REASON | ACTION_OUTCOME_ABSENT_REASON) => {
            Classification::at(Code::ActionDispositionInvalid, PLACEMENT_FIELD)
        }
        (PLACEMENT_FIELD, DECISION_SKIP_REASON) => {
            Classification::at(Code::DecisionSkipNotAllowed, PLACEMENT_FIELD)
        }
        _ => Classification::schema_invalid(authored_field(field)),
    }
}

/// Refines `UnknownV2Reference` by the authored field its raise site names.
///
/// Each field is one closed-reference check, so the field alone pins the meaning; the offending
/// value plus the field's structural shape then pins the exact line.
fn classify_unknown_reference<'a>(field: &'static str, value: &'a str) -> Classification<'a> {
    use AuthoringDiagnosticCode as Code;

    let (code, shape) = match field {
        PLACEMENT_USE_FIELD => (Code::GraphDefinitionUnknown, Some(SHAPE_PLACEMENT_USE)),
        PLACEMENT_NEXT_FIELD => (Code::RouteTargetNotFound, Some(SHAPE_PLACEMENT_NEXT)),
        PLACEMENT_ROUTE_TARGET_FIELD => (Code::RouteTargetNotFound, Some(SHAPE_ROUTE_TARGET)),
        EVIDENCE_SOURCE_FIELD => (Code::EvidenceSourceUnknown, Some(SHAPE_EVIDENCE_SOURCE)),
        // No shape for the selector: its legality is relative to the *source placement's*
        // definition, so the same item identifier can be legal in one `evidence_from` entry and
        // illegal in another, and a first-source-order shape match could point at the legal one.
        // The longest-prefix fallback is coarse but never points at a line that is not at fault.
        EVIDENCE_SELECTOR_FIELD => (Code::EvidenceSelectorUnknownItem, None),
        MANUAL_REWORK_TARGETS_FIELD => (
            Code::ManualReworkTargetUnknown,
            Some(SHAPE_MANUAL_REWORK_TARGET),
        ),
        // Defensive: the five closed-reference checks above are the only raise sites of this
        // variant. A sixth would be a new check, and a schema violation is true of it until it is
        // bound here.
        _ => return Classification::schema_invalid(authored_field(field)).with_value(value),
    };
    let classification = Classification::at(code, field).with_value(value);
    match shape {
        Some(shape) => classification.with_shape(shape),
        None => classification,
    }
}

/// Refines `V2ShapeMismatch` by the `(field, reason)` pair its raise site names.
fn classify_shape_mismatch(field: &'static str, reason: &'static str) -> Classification<'static> {
    use AuthoringDiagnosticCode as Code;

    match (field, reason) {
        (PLACEMENT_ROUTES_FIELD, ROUTE_OPTION_UNDECLARED_REASON) => {
            Classification::at(Code::DecisionRouteOptionUndefined, field)
        }
        (PLACEMENT_ROUTES_FIELD, OPTION_ROUTE_MISSING_REASON) => {
            Classification::at(Code::DecisionOptionRouteMissing, field)
        }
        (EVIDENCE_SOURCE_FIELD, EVIDENCE_SELF_REFERENCE_REASON) => {
            Classification::at(Code::EvidenceSourceSelfReference, field)
        }
        (ASSESSMENT_FIELD, ASSESSMENT_REQUIRES_GOAL_TRACKING_REASON) => {
            Classification::at(Code::GoalAssessmentRequiresGoalTracking, field)
        }
        // No catalog code fits an outcome mapping that names an *option* the definition does not
        // declare. `GOAL_ASSESSMENT_OUTCOME_UNKNOWN` is the unknown-*outcome* case, which
        // `GoalOutcome::from_str` already rejected at parse; `GOAL_ASSESSMENT_OPTION_UNMAPPED` is
        // the opposite direction — a declared option with no mapping — and belongs to vet. The
        // catalog is closed, so this stays the honest generic code.
        (ASSESSMENT_OUTCOMES_FIELD, ASSESSMENT_OUTCOME_UNDECLARED_OPTION_REASON) => {
            Classification::schema_invalid(Some(field))
        }
        // A placement/definition kind disagreement is a closed-shape violation: the catalog names
        // an unknown definition and an invalid disposition, but not a definition of the wrong kind.
        (PLACEMENT_USE_FIELD, ACTION_PLACEMENT_KIND_REASON | DECISION_PLACEMENT_KIND_REASON) => {
            Classification::schema_invalid(Some(field))
        }
        // Defensive: a shape mismatch this module does not yet bind is still a schema violation.
        _ => Classification::schema_invalid(authored_field(field)),
    }
}

/// Drops the [`DOMAIN_SENTINEL_FIELD`] marker `map_domain_error` uses when a core constructor
/// rejects a value without naming an authored path; there is no field to point at, so the document
/// root is the honest answer.
fn authored_field(field: &'static str) -> Option<&'static str> {
    (field != DOMAIN_SENTINEL_FIELD).then_some(field)
}

/// The opening sentence of a diagnostic message, one per code this module can classify into.
///
/// The `ConfigError`'s own `Display` follows it through [`bounded_detail`], and that text carries
/// the offending value in backticks — so the message names both the rule and the value that broke
/// it without this table having to re-render either.
///
/// The final arm is the one documented wildcard: it covers the catalog's lint and vet codes, which
/// this module never produces because no `ConfigError` describes an advisory finding. Its sentence
/// is true of every document the parse and validate stages refuse, so reaching it degrades
/// precision and never truthfulness.
fn lead_sentence(code: AuthoringDiagnosticCode) -> &'static str {
    use AuthoringDiagnosticCode as Code;

    match code {
        Code::SourceConstructUnsupported => {
            "The source uses an unsupported YAML or JSON construct:"
        }
        Code::SourceProjectionBudgetExceeded => {
            "The canonical source projection exceeds its budget:"
        }
        Code::GraphProjectionBudgetExceeded => {
            "The generated graph text projection exceeds its budget:"
        }
        Code::EntryNodeInvalid => "The graph entry does not name a declared graph node:",
        Code::GraphDefinitionUnknown => {
            "A graph placement uses a node definition the procedure does not declare:"
        }
        Code::RouteTargetNotFound => {
            "A transition names a graph node the procedure does not declare:"
        }
        Code::ActionDispositionInvalid => {
            "An action placement does not declare exactly one of next or terminal:"
        }
        Code::DecisionSkipNotAllowed => "A decision placement declares a skip policy:",
        Code::DecisionOptionRouteMissing => "A declared decision option has no route:",
        Code::DecisionRouteOptionUndefined => {
            "A route names an option the decision definition does not declare:"
        }
        Code::GoalAssessmentOutcomeUnknown => {
            "A session-goal assessment maps an option to an unknown outcome:"
        }
        Code::GoalAssessmentRequiresGoalTracking => {
            "A session-goal assessment is declared without procedure-level goal tracking:"
        }
        Code::EvidenceSourceUnknown => {
            "An evidence reference names a graph node the procedure does not declare:"
        }
        Code::EvidenceSourceSelfReference => {
            "An evidence reference names its own consuming placement:"
        }
        Code::EvidenceSelectorUnknownItem => {
            "An evidence selector names an item its source definition does not declare:"
        }
        Code::ManualReworkTargetUnknown => {
            "A manual rework target names a graph node the procedure does not declare:"
        }
        Code::AmbiguousGraphReference => {
            "Two graph placements share an identifier, so every reference to it is ambiguous:"
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
        Code::GraphProjectionBudgetExceeded => {
            "Split the graph or shorten its generated text to fit the graph projection budget."
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
