//! Procedure v2 session-goal tracking opt-in, statement, criteria, definition, and bounded
//! reason values.

use std::collections::BTreeSet;
use std::fmt;

use crate::{CriterionId, DomainError, validate_text};

use super::invalid;

const MAX_GOAL_STATEMENT_CHARS: usize = 1_000;
const MAX_CRITERION_STATEMENT_CHARS: usize = 300;
const MAX_GOAL_REVISION_REASON_CHARS: usize = 1_000;
const MAX_CRITERION_ASSESSMENT_REASON_CHARS: usize = 2_000;
const MIN_GOAL_CRITERIA: usize = 1;
const MAX_GOAL_CRITERIA: usize = 16;

/// The procedure-level session-goal tracking opt-in.
///
/// Session goal tracking is enabled only when a procedure declares exactly `goal_tracking: true`.
/// The only accepted value is `true`; absence of the key disables tracking. `false`, and non-boolean
/// authored values, are rejected. Non-boolean values are refused by the parser before reaching the
/// domain; the domain value enforces the boolean boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GoalTrackingOptIn {
    enabled: bool,
}

impl GoalTrackingOptIn {
    pub fn from_bool(value: bool) -> Result<Self, DomainError> {
        if !value {
            return Err(invalid(
                "goal_tracking accepts only true; omit the key to disable",
            ));
        }
        Ok(Self { enabled: true })
    }

    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// A task-specific session goal statement (dossier section 4.5).
///
/// The bounded value is non-empty and holds at most 1,000 Unicode characters. Whether a statement
/// is required is a goal-revision record concern (V2MOD-003); this value enforces only the bound
/// that applies when a statement is present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalStatementV2(String);

impl GoalStatementV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text("goal statement", &value, 1, MAX_GOAL_STATEMENT_CHARS, true)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for GoalStatementV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for GoalStatementV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One stable-ID success criterion of a session goal (dossier section 4.5). The statement is
/// non-empty and holds at most 300 Unicode characters; the identifier bound is owned by
/// `CriterionId`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalCriterionV2 {
    id: CriterionId,
    statement: String,
}

impl GoalCriterionV2 {
    pub fn new(id: CriterionId, statement: impl Into<String>) -> Result<Self, DomainError> {
        let statement = statement.into();
        validate_text(
            "criterion statement",
            &statement,
            1,
            MAX_CRITERION_STATEMENT_CHARS,
            true,
        )?;
        Ok(Self { id, statement })
    }

    pub fn id(&self) -> &CriterionId {
        &self.id
    }

    pub fn statement(&self) -> &str {
        &self.statement
    }
}

/// The ordered set of success criteria that define a session goal (dossier section 4.5). A
/// definition holds one to sixteen criteria with unique `CriterionId` values and preserves author
/// order. Revision metadata is owned by the goal-revision record (V2MOD-003).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalDefinitionV2 {
    criteria: Vec<GoalCriterionV2>,
}

impl GoalDefinitionV2 {
    pub fn new(criteria: Vec<GoalCriterionV2>) -> Result<Self, DomainError> {
        if criteria.len() < MIN_GOAL_CRITERIA || criteria.len() > MAX_GOAL_CRITERIA {
            return Err(invalid("goal criterion count must be between one and 16"));
        }
        let mut seen = BTreeSet::new();
        for criterion in &criteria {
            if !seen.insert(criterion.id()) {
                return Err(invalid("goal criterion identifiers must be unique"));
            }
        }
        Ok(Self { criteria })
    }

    pub fn criteria(&self) -> &[GoalCriterionV2] {
        &self.criteria
    }
}

/// A bounded goal revision reason (dossier section 4.5). Non-empty, at most 1,000 Unicode
/// characters. A revision reason is supplied for revisions after the first; whether one is present
/// is a goal-revision record concern (V2MOD-003), distinct from this bounded value used when
/// present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalRevisionReasonV2(String);

impl GoalRevisionReasonV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "goal revision reason",
            &value,
            1,
            MAX_GOAL_REVISION_REASON_CHARS,
            true,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for GoalRevisionReasonV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for GoalRevisionReasonV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A bounded criterion assessment reason (dossier section 4.5). Non-empty, at most 2,000 Unicode
/// characters. The reason is always required for every criterion assessment status; this value
/// enforces the bound used when a reason is recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CriterionAssessmentReasonV2(String);

impl CriterionAssessmentReasonV2 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_text(
            "criterion assessment reason",
            &value,
            1,
            MAX_CRITERION_ASSESSMENT_REASON_CHARS,
            true,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for CriterionAssessmentReasonV2 {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for CriterionAssessmentReasonV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
