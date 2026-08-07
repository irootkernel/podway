//! The aggregate authoring gate for Procedure v2 (dossier section 11.5).
//!
//! Section 11.5 describes check as one pipeline — `format --check` → validate → vet → lint —
//! returning "one bounded diagnostic result suitable for humans, CI, and AI clients". That
//! description is the *reported* order. The order the stages must actually run in is different, and
//! the difference is the whole design of this module:
//!
//! | step | stage | on findings |
//! |---|---|---|
//! | S0 | parse | one diagnostic, stop — no model exists |
//! | S1 | validate | one diagnostic, stop — no model exists |
//! | S2 | supported-construct scan | n diagnostics, skip S3 only |
//! | S3 | drift against the canonical rendering | 0 or 1 diagnostic, never stops |
//! | S4 | vet | never stops |
//! | S5 | lint | — |
//!
//! Only the absence of a *model* stops the pipeline. Everything downstream of validation reads the
//! validated model, so a stale format must not hide a graph error: `finalize_diagnostics` sorts the
//! merged vector by `(stage, line, column, code, field)`, and the report therefore reads as section
//! 11.5's pipeline while execution keeps the order it needs.
//!
//! **Stage assignment, decided once and documented because the schema does not carry it.**
//!
//! - *Parse and validate failures carry [`AuthoringStage::Validate`].* Reported after formatting
//!   findings, which is the right place for them: a parse failure is a statement that this document
//!   is not an admissible Procedure, and admissibility is validation's question. The alternative —
//!   calling a parse failure a format-stage finding because `format --check` is the first pipeline
//!   step — would be false in the one way that matters: `format --check` cannot run at all on a
//!   document that has no model, so it did not find anything. `procedure lint` already assigns the
//!   same stage to the same two failures, so the two commands report an inadmissible document
//!   identically.
//! - *Construct violations and the projection bound carry [`AuthoringStage::Format`].* Both are
//!   findings of the rendering stage — the source uses a construct canonical authoring form cannot
//!   represent, or the canonical form of an admissible model does not fit its budget. Section 6 of
//!   the design tables the first and is silent on the second; they are treated identically because
//!   they are the same event to every caller: **this model has no canonical authoring text**, so
//!   S3 has nothing to compare against and is skipped, while S4 and S5 still run on the model.
//! - *Drift carries [`AuthoringStage::Format`]* and is produced by
//!   [`crate::FormattedProcedureV2::drift_diagnostic`] — the same constructor `format --check`
//!   calls, so the two commands can never disagree about whether a document has drifted or about
//!   where.
//!
//! **Validity is derived, never asserted.** `FORMAT_NOT_CANONICAL` is an *error* in
//! `assets/specifications/authoring-diagnostics.json`, so a merely drifted document reports
//! `valid: false` even though every graph rule passed. That follows from the catalog binding
//! severity to the code; this module does not decide it and must not contradict it.
//!
//! **The digest is present exactly when S1 succeeded** — including when drift, a construct
//! violation, the projection bound, or a lint warning was reported afterwards. It answers "which
//! procedure are these findings about", which is a question a document has an answer to as soon as
//! it is admissible, and no answer to before.

use podway_core::{AuthoringDiagnostic, Sha256Digest};

use crate::ConfigError;
use crate::procedure_v2_diagnostics::{
    AuthoringContext, AuthoringStage, FinalizedDiagnostics, config_error_diagnostic,
    finalize_diagnostics,
};
use crate::procedure_v2_format::{
    FormatFailure, FormatRequest, admit_procedure_v2, render_procedure_v2,
};
use crate::procedure_v2_lint::lint_procedure_v2;
use crate::procedure_v2_vet::vet_procedure_v2;

/// Everything `podway procedure check` learned about one source document.
///
/// Accessors mirror [`FinalizedDiagnostics`]: `total` counts before truncation and `truncated` says
/// whether the retained `diagnostics` are a prefix, so a client can always tell it is seeing part
/// of the answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureCheckReport {
    digest: Option<Sha256Digest>,
    findings: FinalizedDiagnostics,
}

impl ProcedureCheckReport {
    /// The canonical semantic digest, present exactly when the document is admissible.
    pub const fn digest(&self) -> Option<&Sha256Digest> {
        self.digest.as_ref()
    }

    /// True when no reported diagnostic has error severity.
    ///
    /// Describes the procedure, not the invocation: `--warnings-as-errors` is a policy about an
    /// exit code and never moves this flag.
    pub fn valid(&self) -> bool {
        self.findings.valid()
    }

    /// The retained diagnostics, at most `MAX_AUTHORING_DIAGNOSTICS`, in reported order.
    pub fn diagnostics(&self) -> &[AuthoringDiagnostic] {
        self.findings.diagnostics()
    }

    /// The number of findings before truncation.
    pub const fn total(&self) -> u32 {
        self.findings.total()
    }

    pub const fn truncated(&self) -> bool {
        self.findings.truncated()
    }
}

/// Runs every authoring stage over one source document and reports the merged result.
///
/// The pipeline never fails: an inadmissible document is a report carrying exactly one error and no
/// digest, which is a complete answer rather than an absent one.
pub fn check_procedure_v2(request: FormatRequest<'_>) -> ProcedureCheckReport {
    let context = AuthoringContext::new(request.source_path, request.source, request.format);

    // S0 and S1. A document with no model has nothing left to report, so the one diagnostic that
    // says why is the whole report.
    let validated = match admit_procedure_v2(&context) {
        Ok(validated) => validated,
        Err(failure) => return stopped(failure, &context),
    };

    let mut entries: Vec<(AuthoringStage, AuthoringDiagnostic)> = Vec::new();

    // S2 and S3. Rendering is one step for the caller and two findings sources: either the model
    // has canonical authoring text and the source is compared against it, or it has none and the
    // reasons are reported in its place. Both are format-stage findings, and neither stops the run.
    match render_procedure_v2(&validated, &context) {
        Ok(formatted) => entries.extend(
            formatted
                .drift_diagnostic(&context)
                .map(|diagnostic| (AuthoringStage::Format, diagnostic)),
        ),
        Err(diagnostics) => entries.extend(
            diagnostics
                .into_iter()
                .map(|diagnostic| (AuthoringStage::Format, diagnostic)),
        ),
    }

    // S4. Empty until V2GRF-001 and V2GRF-002; wired now so the aggregate gate needs no change on
    // the day it stops being empty.
    entries.extend(
        vet_procedure_v2(&validated, &context)
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Vet, diagnostic)),
    );

    // S5.
    entries.extend(
        lint_procedure_v2(&validated, &context)
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Lint, diagnostic)),
    );

    ProcedureCheckReport {
        digest: Some(validated.digest().clone()),
        findings: finalize_diagnostics(entries),
    }
}

/// The report for a document that produced no model.
fn stopped(failure: FormatFailure, context: &AuthoringContext<'_>) -> ProcedureCheckReport {
    let diagnostics = match failure {
        // A caller that reaches this arm handed check a document declaring another schema. The CLI
        // cannot: it sniffs the schema first and refuses a v1 file as a command-level failure,
        // because the diagnostics result pins `procedure_schema` to `podway.procedure/v2` and
        // therefore cannot describe a v1 document. This report is the library-level answer to the
        // same question, and it is a true one — the source does not declare the v2 authoring
        // schema — reached through the classifier every other schema violation goes through.
        FormatFailure::NotProcedureV2 => vec![config_error_diagnostic(
            &ConfigError::InvalidSchema {
                expected: podway_core::PROCEDURE_SCHEMA_V2,
                actual: crate::PROCEDURE_SCHEMA_V1.to_owned(),
            },
            context,
        )],
        FormatFailure::Diagnostics(diagnostics) => diagnostics,
    };
    ProcedureCheckReport {
        digest: None,
        findings: finalize_diagnostics(
            diagnostics
                .into_iter()
                .map(|diagnostic| (AuthoringStage::Validate, diagnostic))
                .collect(),
        ),
    }
}
