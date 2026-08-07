//! Authoring diagnostics: the bounded value types every Procedure v2 authoring stage emits
//! (dossier sections 11.1 through 11.6).
//!
//! This module owns the *shape* of an authoring diagnostic, not the rules that produce one. It is
//! a pure, dependency-free value layer so that the two consumers — `podway-config`, which emits
//! diagnostics from parsing, validation, formatting, vetting, and linting, and `podway-cli`, which
//! renders them — share one definition instead of two that can drift.
//!
//! Three contracts are fixed here and nowhere else:
//!
//! 1. **The code catalog is closed.** [`AuthoringDiagnosticCode`] enumerates exactly the codes
//!    `assets/specifications/authoring-diagnostics.json` lists, in catalog order, and
//!    [`AuthoringDiagnosticCode::severity`] binds each code to the severity that catalog assigns.
//!    `assets/schemas/authoring-diagnostic-v1.schema.json` expresses the same binding as a `oneOf`
//!    over two code sets, so a diagnostic whose severity disagrees with its code is unrepresentable
//!    rather than merely invalid.
//! 2. **Every bound is enforced by construction.** [`AuthoringDiagnostic::new`] clamps rather than
//!    failing: a diagnostic constructor that can itself fail would need a diagnostic to report the
//!    failure. Locations clamp into `1..=1_048_576`, `message` and `hint` into 512 characters,
//!    `field` and `source_path` into 4,096, and `related_graph_node_ids` into 64 unique entries —
//!    exactly the schema's bounds, so a constructed value always serializes to a valid document.
//! 3. **Serialization order is the schema's field order.** `code`, `severity`, `schema`,
//!    `source_path`, `location`, `field`, `message`, `hint`, then the three optional identity
//!    fields, which are omitted entirely when absent.

use serde::{Serialize, Serializer};

use crate::PROCEDURE_SCHEMA_V2;

/// The maximum number of diagnostics one authoring result carries; the remainder is reported
/// through a truncation flag and a pre-truncation total.
pub const MAX_AUTHORING_DIAGNOSTICS: usize = 256;
/// The maximum number of characters in a diagnostic `message`.
pub const MAX_AUTHORING_DIAGNOSTIC_MESSAGE_CHARS: usize = 512;
/// The maximum number of characters in a diagnostic `hint`.
pub const MAX_AUTHORING_DIAGNOSTIC_HINT_CHARS: usize = 512;
/// The maximum number of characters in a diagnostic `field` path.
pub const MAX_AUTHORING_DIAGNOSTIC_FIELD_CHARS: usize = 4_096;
/// The maximum number of characters in a diagnostic `source_path`.
pub const MAX_AUTHORING_DIAGNOSTIC_SOURCE_PATH_CHARS: usize = 4_096;
/// The maximum number of related graph node identifiers one diagnostic carries.
pub const MAX_AUTHORING_DIAGNOSTIC_RELATED_GRAPH_NODES: usize = 64;
/// The maximum line or column number a diagnostic location may report.
pub const MAX_AUTHORING_SOURCE_POSITION: u32 = 1_048_576;

/// The severity a diagnostic code carries. Severity is a property of the code, never of the
/// invocation: `--warnings-as-errors` changes an exit code, never this value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AuthoringSeverity {
    Error,
    Warning,
}

impl AuthoringSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

impl Serialize for AuthoringSeverity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Declares the closed diagnostic catalog once, so a code's stable string and its severity can
/// never be written in two places and drift apart.
macro_rules! authoring_diagnostic_catalog {
    ($( $variant:ident => ($code:literal, $severity:ident) ),+ $(,)?) => {
        /// Every stable authoring diagnostic code, in the order
        /// `assets/specifications/authoring-diagnostics.json` lists them.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum AuthoringDiagnosticCode {
            $( $variant ),+
        }

        impl AuthoringDiagnosticCode {
            /// The complete catalog in specification order.
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];

            /// The stable wire string automation branches on.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $code ),+
                }
            }

            /// The severity the catalog binds to this code.
            pub const fn severity(self) -> AuthoringSeverity {
                match self {
                    $( Self::$variant => AuthoringSeverity::$severity ),+
                }
            }
        }
    };
}

authoring_diagnostic_catalog! {
    AuthoringSchemaInvalid => ("AUTHORING_SCHEMA_INVALID", Error),
    SourceConstructUnsupported => ("SOURCE_CONSTRUCT_UNSUPPORTED", Error),
    SourceProjectionBudgetExceeded => ("SOURCE_PROJECTION_BUDGET_EXCEEDED", Error),
    FormatNotCanonical => ("FORMAT_NOT_CANONICAL", Error),
    EntryNodeInvalid => ("ENTRY_NODE_INVALID", Error),
    GraphDefinitionUnknown => ("GRAPH_DEFINITION_UNKNOWN", Error),
    RouteTargetNotFound => ("ROUTE_TARGET_NOT_FOUND", Error),
    UnreachableGraphNode => ("UNREACHABLE_GRAPH_NODE", Error),
    NoTerminalPath => ("NO_TERMINAL_PATH", Error),
    ActionDispositionInvalid => ("ACTION_DISPOSITION_INVALID", Error),
    DecisionOptionRouteMissing => ("DECISION_OPTION_ROUTE_MISSING", Error),
    DecisionRouteOptionUndefined => ("DECISION_ROUTE_OPTION_UNDEFINED", Error),
    GoalAssessmentOptionUnmapped => ("GOAL_ASSESSMENT_OPTION_UNMAPPED", Error),
    GoalAssessmentOutcomeUnknown => ("GOAL_ASSESSMENT_OUTCOME_UNKNOWN", Error),
    GoalAssessmentOutcomeUnreachable => ("GOAL_ASSESSMENT_OUTCOME_UNREACHABLE", Error),
    GoalAssessmentRequiresGoalTracking => ("GOAL_ASSESSMENT_REQUIRES_GOAL_TRACKING", Error),
    GoalAssessmentNotDominatingTerminal => ("GOAL_ASSESSMENT_NOT_DOMINATING_TERMINAL", Error),
    EvidenceSourceUnknown => ("EVIDENCE_SOURCE_UNKNOWN", Error),
    EvidenceSourceSelfReference => ("EVIDENCE_SOURCE_SELF_REFERENCE", Error),
    EvidenceSourceDoesNotDominateConsumer => ("EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER", Error),
    SkippableEvidenceSource => ("SKIPPABLE_EVIDENCE_SOURCE", Error),
    EvidenceSelectorUnknownItem => ("EVIDENCE_SELECTOR_UNKNOWN_ITEM", Error),
    ReadbackBudgetExceeded => ("READBACK_BUDGET_EXCEEDED", Error),
    NextStaticBudgetExceeded => ("NEXT_STATIC_BUDGET_EXCEEDED", Error),
    DecisionSkipNotAllowed => ("DECISION_SKIP_NOT_ALLOWED", Error),
    GraphCycleInvalid => ("GRAPH_CYCLE_INVALID", Error),
    ReworkTargetNotDominating => ("REWORK_TARGET_NOT_DOMINATING", Error),
    ManualReworkTargetUnknown => ("MANUAL_REWORK_TARGET_UNKNOWN", Error),
    AmbiguousGraphReference => ("AMBIGUOUS_GRAPH_REFERENCE", Error),
    UnusedNodeDefinition => ("UNUSED_NODE_DEFINITION", Warning),
    SingleOptionDecision => ("SINGLE_OPTION_DECISION", Warning),
    IndistinguishableOptionLabels => ("INDISTINGUISHABLE_OPTION_LABELS", Warning),
    IdenticalEffectiveRoutes => ("IDENTICAL_EFFECTIVE_ROUTES", Warning),
    WeakPurposeGuidance => ("WEAK_PURPOSE_GUIDANCE", Warning),
    WeakIntentGuidance => ("WEAK_INTENT_GUIDANCE", Warning),
    WeakObjectiveGuidance => ("WEAK_OBJECTIVE_GUIDANCE", Warning),
    WeakPromptGuidance => ("WEAK_PROMPT_GUIDANCE", Warning),
    WeakCriteriaGuidance => ("WEAK_CRITERIA_GUIDANCE", Warning),
    WeakReasonGuidance => ("WEAK_REASON_GUIDANCE", Warning),
    EvidenceGuidanceMissing => ("EVIDENCE_GUIDANCE_MISSING", Warning),
    OptionalEvidenceUnresolvable => ("OPTIONAL_EVIDENCE_UNRESOLVABLE", Warning),
    GoalClarificationPathMissing => ("GOAL_CLARIFICATION_PATH_MISSING", Warning),
    GoalAssessmentTooEarly => ("GOAL_ASSESSMENT_TOO_EARLY", Warning),
    ManualReworkTargetsBroad => ("MANUAL_REWORK_TARGETS_BROAD", Warning),
    LargeOptionSet => ("LARGE_OPTION_SET", Warning),
    LargeCycle => ("LARGE_CYCLE", Warning),
    DuplicatedNodeDefinition => ("DUPLICATED_NODE_DEFINITION", Warning),
    GraphNodeIdConfusing => ("GRAPH_NODE_ID_CONFUSING", Warning),
    ReworkTopologyConfusing => ("REWORK_TOPOLOGY_CONFUSING", Warning),
    NoReactivationPath => ("NO_REACTIVATION_PATH", Warning),
    GoalRevisionTargetUnsafe => ("GOAL_REVISION_TARGET_UNSAFE", Warning),
    MultipleGoalAssessmentSources => ("MULTIPLE_GOAL_ASSESSMENT_SOURCES", Warning),
}

impl Serialize for AuthoringDiagnosticCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A bounded one-based source span. `end_line`/`end_column` never precede `line`/`column`, and
/// every component sits inside `1..=1_048_576`, so the value always satisfies the diagnostic
/// schema's `location` bounds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SourceLocation {
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

impl SourceLocation {
    /// The first character of the document: the stable fallback when no more precise anchor exists.
    pub const fn document_start() -> Self {
        Self {
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 1,
        }
    }

    /// Builds a span, clamping every component into the schema's range and forcing the end of the
    /// span to be at or after its start.
    pub const fn new(line: u32, column: u32, end_line: u32, end_column: u32) -> Self {
        let line = clamp_position(line);
        let column = clamp_position(column);
        let end_line = at_least(clamp_position(end_line), line);
        let end_column = if end_line == line {
            at_least(clamp_position(end_column), column)
        } else {
            clamp_position(end_column)
        };
        Self {
            line,
            column,
            end_line,
            end_column,
        }
    }

    pub const fn line(self) -> u32 {
        self.line
    }

    pub const fn column(self) -> u32 {
        self.column
    }

    pub const fn end_line(self) -> u32 {
        self.end_line
    }

    pub const fn end_column(self) -> u32 {
        self.end_column
    }
}

const fn clamp_position(value: u32) -> u32 {
    if value == 0 {
        1
    } else if value > MAX_AUTHORING_SOURCE_POSITION {
        MAX_AUTHORING_SOURCE_POSITION
    } else {
        value
    }
}

const fn at_least(value: u32, floor: u32) -> u32 {
    if value < floor { floor } else { value }
}

/// One authoring diagnostic, serialized in `assets/schemas/authoring-diagnostic-v1.schema.json`
/// field order with the three optional identity fields omitted when absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthoringDiagnostic {
    code: AuthoringDiagnosticCode,
    severity: AuthoringSeverity,
    schema: &'static str,
    source_path: String,
    location: SourceLocation,
    field: String,
    message: String,
    hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_definition_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph_node_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related_graph_node_ids: Vec<String>,
}

impl AuthoringDiagnostic {
    /// Builds a diagnostic, binding `severity` to `code` and clamping every bounded field.
    ///
    /// Clamping, not rejection: a diagnostic is already the report of a failure, so a constructor
    /// that could fail would have nothing to report the second failure with. Callers keep messages
    /// short by construction; the clamp is the floor under that discipline, not the plan.
    pub fn new(
        code: AuthoringDiagnosticCode,
        source_path: impl Into<String>,
        location: SourceLocation,
        field: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: code.severity(),
            schema: PROCEDURE_SCHEMA_V2,
            source_path: bounded(
                source_path.into(),
                MAX_AUTHORING_DIAGNOSTIC_SOURCE_PATH_CHARS,
                "-",
            ),
            location,
            field: bounded(field.into(), MAX_AUTHORING_DIAGNOSTIC_FIELD_CHARS, "$"),
            message: bounded(
                message.into(),
                MAX_AUTHORING_DIAGNOSTIC_MESSAGE_CHARS,
                code.as_str(),
            ),
            hint: bounded(
                hint.into(),
                MAX_AUTHORING_DIAGNOSTIC_HINT_CHARS,
                "Review the Procedure v2 authoring contract.",
            ),
            node_definition_id: None,
            graph_node_id: None,
            related_graph_node_ids: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_node_definition_id(mut self, node_definition_id: impl Into<String>) -> Self {
        self.node_definition_id = Some(node_definition_id.into());
        self
    }

    #[must_use]
    pub fn with_graph_node_id(mut self, graph_node_id: impl Into<String>) -> Self {
        self.graph_node_id = Some(graph_node_id.into());
        self
    }

    /// Attaches related graph node identifiers, de-duplicated in first-occurrence order and capped
    /// at the schema's 64 entries (`uniqueItems`, `maxItems`).
    #[must_use]
    pub fn with_related_graph_node_ids(
        mut self,
        related_graph_node_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut unique: Vec<String> = Vec::new();
        for candidate in related_graph_node_ids {
            if unique.len() == MAX_AUTHORING_DIAGNOSTIC_RELATED_GRAPH_NODES {
                break;
            }
            if !unique.contains(&candidate) {
                unique.push(candidate);
            }
        }
        self.related_graph_node_ids = unique;
        self
    }

    pub const fn code(&self) -> AuthoringDiagnosticCode {
        self.code
    }

    pub const fn severity(&self) -> AuthoringSeverity {
        self.severity
    }

    pub const fn schema(&self) -> &'static str {
        self.schema
    }

    pub fn source_path(&self) -> &str {
        &self.source_path
    }

    pub const fn location(&self) -> SourceLocation {
        self.location
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint(&self) -> &str {
        &self.hint
    }

    pub fn node_definition_id(&self) -> Option<&str> {
        self.node_definition_id.as_deref()
    }

    pub fn graph_node_id(&self) -> Option<&str> {
        self.graph_node_id.as_deref()
    }

    pub fn related_graph_node_ids(&self) -> &[String] {
        &self.related_graph_node_ids
    }
}

/// Truncates on a character boundary and substitutes a non-empty fallback, so the result always
/// satisfies both the schema's `minLength: 1` and its maximum.
fn bounded(value: String, maximum: usize, fallback: &str) -> String {
    let truncated = if value.chars().count() > maximum {
        value.chars().take(maximum).collect()
    } else {
        value
    };
    if truncated.is_empty() {
        fallback.to_owned()
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic() -> AuthoringDiagnostic {
        AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            "workflow.yaml",
            SourceLocation::new(3, 5, 3, 20),
            "graph.nodes[perform].use",
            "The Procedure source violates the closed v2 authoring schema.",
            "Correct the field against assets/schemas/procedure-v2.schema.json.",
        )
    }

    #[test]
    fn every_catalog_code_is_unique_and_binds_one_severity() {
        let mut codes = std::collections::BTreeSet::new();
        for code in AuthoringDiagnosticCode::ALL {
            assert!(
                codes.insert(code.as_str()),
                "duplicate code {}",
                code.as_str()
            );
            assert_eq!(code.severity(), (*code).severity());
        }
        assert_eq!(codes.len(), 52);
        assert_eq!(AuthoringDiagnosticCode::ALL.len(), 52);
    }

    #[test]
    fn severity_partitions_the_catalog_exactly_as_the_schema_one_of_does() {
        let errors = AuthoringDiagnosticCode::ALL
            .iter()
            .filter(|code| code.severity() == AuthoringSeverity::Error)
            .count();
        let warnings = AuthoringDiagnosticCode::ALL
            .iter()
            .filter(|code| code.severity() == AuthoringSeverity::Warning)
            .count();
        assert_eq!(errors, 29);
        assert_eq!(warnings, 23);
    }

    #[test]
    fn source_location_clamps_zero_overflow_and_inverted_spans() {
        assert_eq!(
            SourceLocation::new(0, 0, 0, 0),
            SourceLocation::document_start()
        );

        let clamped = SourceLocation::new(u32::MAX, u32::MAX, u32::MAX, u32::MAX);
        assert_eq!(clamped.line(), MAX_AUTHORING_SOURCE_POSITION);
        assert_eq!(clamped.end_column(), MAX_AUTHORING_SOURCE_POSITION);

        let inverted = SourceLocation::new(10, 40, 4, 2);
        assert_eq!(inverted.line(), 10);
        assert_eq!(inverted.column(), 40);
        assert_eq!(inverted.end_line(), 10);
        assert_eq!(inverted.end_column(), 40);

        let multi_line = SourceLocation::new(10, 40, 12, 2);
        assert_eq!(multi_line.end_line(), 12);
        assert_eq!(multi_line.end_column(), 2);
    }

    #[test]
    fn constructor_clamps_message_hint_and_field_on_character_boundaries() {
        let long = "é".repeat(4_100);
        let diagnostic = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::NoReactivationPath,
            String::new(),
            SourceLocation::document_start(),
            long.clone(),
            long.clone(),
            long,
        );

        assert_eq!(diagnostic.source_path(), "-");
        assert_eq!(diagnostic.field().chars().count(), 4_096);
        assert_eq!(diagnostic.message().chars().count(), 512);
        assert_eq!(diagnostic.hint().chars().count(), 512);
        assert_eq!(diagnostic.severity(), AuthoringSeverity::Warning);
    }

    #[test]
    fn empty_strings_become_the_schema_satisfying_fallbacks() {
        let diagnostic = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::FormatNotCanonical,
            "",
            SourceLocation::document_start(),
            "",
            "",
            "",
        );

        assert_eq!(diagnostic.source_path(), "-");
        assert_eq!(diagnostic.field(), "$");
        assert_eq!(diagnostic.message(), "FORMAT_NOT_CANONICAL");
        assert!(!diagnostic.hint().is_empty());
    }

    #[test]
    fn related_graph_node_ids_are_deduplicated_and_capped() {
        let ids = (0..80)
            .map(|index| format!("n{index}"))
            .chain(["n0".to_owned()])
            .collect::<Vec<_>>();
        let diagnostic = diagnostic().with_related_graph_node_ids(ids);

        assert_eq!(diagnostic.related_graph_node_ids().len(), 64);
        assert_eq!(diagnostic.related_graph_node_ids()[0], "n0");

        let deduplicated = AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::LargeCycle,
            "workflow.yaml",
            SourceLocation::document_start(),
            "$",
            "message",
            "hint",
        )
        .with_related_graph_node_ids(["a".to_owned(), "a".to_owned(), "b".to_owned()]);
        assert_eq!(deduplicated.related_graph_node_ids(), ["a", "b"]);
    }

    #[test]
    fn serialization_uses_schema_field_order_and_omits_absent_identity_fields() {
        let json = serde_json::to_string(&diagnostic()).expect("diagnostics serialize");
        assert_eq!(
            json,
            concat!(
                r#"{"code":"AUTHORING_SCHEMA_INVALID","severity":"error","#,
                r#""schema":"podway.procedure/v2","source_path":"workflow.yaml","#,
                r#""location":{"line":3,"column":5,"end_line":3,"end_column":20},"#,
                r#""field":"graph.nodes[perform].use","#,
                r#""message":"The Procedure source violates the closed v2 authoring schema.","#,
                r#""hint":"Correct the field against assets/schemas/procedure-v2.schema.json."}"#,
            )
        );
    }

    #[test]
    fn serialization_carries_every_present_identity_field() {
        let json = serde_json::to_value(
            diagnostic()
                .with_node_definition_id("work")
                .with_graph_node_id("perform")
                .with_related_graph_node_ids(["decide".to_owned()]),
        )
        .expect("diagnostics serialize");

        for required in [
            "code",
            "severity",
            "schema",
            "source_path",
            "location",
            "field",
            "message",
            "hint",
        ] {
            assert!(json.get(required).is_some(), "missing {required}");
        }
        assert_eq!(json["node_definition_id"], "work");
        assert_eq!(json["graph_node_id"], "perform");
        assert_eq!(
            json["related_graph_node_ids"],
            serde_json::json!(["decide"])
        );
        assert_eq!(json["schema"], PROCEDURE_SCHEMA_V2);
    }
}
