//! Canonical authoring form for Procedure v2: the YAML and JSON emitters and the `format` pipeline
//! (dossier section 11.1).
//!
//! **The formatter renders the canonical document; it does not rewrite the source.** It parses,
//! validates, and then emits `procedure_v2_document`'s authoring tree — the same tree the digest is
//! computed from. Determinism, idempotence, and digest preservation are therefore properties of
//! construction rather than of normalization rules: there is no edit path along which the emitter
//! could change what the document means, because it never reads the source's shape at all. The only
//! thing carried across from the source is the comment side table.
//!
//! Layout, fixed:
//!
//! - block style only, two spaces per level, LF, exactly one trailing newline, no document markers,
//!   no blank lines, no BOM;
//! - keys in `assets/schemas/procedure-v2.schema.json` `properties` order, authoring maps and
//!   order-bearing arrays in author order;
//! - a sequence's `- ` marker sits at the child indent, and a mapping inside a sequence element puts
//!   its first key on the marker line — the style every shipped fixture and preset is written in.
//!
//! Scalars are plain only when every clause of [`is_plain_safe`] says a YAML reader will read them
//! back unchanged, and double-quoted with JSON escapes otherwise. Four clauses do the real work,
//! and none of them is a hand-maintained list: the resolver oracle `yaml_rust2::Yaml::from_str`
//! decides `true`/`0`/`~`/`1.5`; `parser::reject_noncanonical_yaml_scalar` is literally the rule the
//! document's own preflight applies, so emission and admission cannot drift; the construct
//! scanner's own line lexer runs over the candidate, which makes the emitter a fixpoint of its own
//! supported-construct grammar; and finally `serde_yaml` — the reader that will actually parse the
//! output — must return the identical string. Block scalars are never emitted: a `\n`-bearing
//! string becomes a one-line double-quoted scalar with `\n` escapes, which preserves the model and
//! therefore the digest.
//!
//! A `.json` source formats to canonical JSON *authoring text* — the same key order and the same
//! two-space indent, never YAML. That text is not Canonical JSON v1: the digest's canonical form is
//! single-line, byte-sorted, and reachable through `procedure show --canonical`. JSON has no comment
//! syntax, so the comment pass and the supported-construct scan (whose "one node per line" invariant
//! a flow document cannot satisfy) apply to YAML only.
//!
//! Drift is the same rendering read the other way round:
//! [`FormattedProcedureV2::drift_diagnostic`] compares the source against this document as bytes
//! and, when they differ, reports the one `FORMAT_NOT_CANONICAL` finding that locates the first
//! divergence. A document that cannot be rendered at all never reaches that comparison, so an
//! unformattable source is reported by its own stage and is never also called non-canonical.

use podway_core::{
    AuthoringDiagnostic, AuthoringDiagnosticCode, SOURCE_PROJECTION_MAX_CHARACTERS, Sha256Digest,
    SourceLocation,
};

use crate::procedure_v2_diagnostics::{AuthoringContext, config_error_diagnostic, diagnostic_hint};
use crate::procedure_v2_document::{AuthoringValue, authoring_document_value};
use crate::procedure_v2_source::{
    FieldPath, SourceComments, SourceConstructViolation, field_string,
    plain_scalar_is_lexically_inert, scan_source,
};
use crate::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};

/// Characters that may never open a plain scalar: every YAML indicator, plus the quote characters.
const PLAIN_SCALAR_FORBIDDEN_FIRST: &str = "-?:,[]{}#&*!|>'\"%@`";

/// YAML 1.1 boolean words. `yaml-rust2` and `serde_yaml` resolve YAML 1.2 core, where these stay
/// strings, but quoting them costs one pair of quotes and removes a resolver dependency.
const YAML_ONE_ONE_BOOLEAN_WORDS: [&str; 6] = ["yes", "no", "on", "off", "y", "n"];

/// One request to render a Procedure v2 source document in canonical authoring form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FormatRequest<'a> {
    /// The complete source document text.
    pub source: &'a str,
    /// The label reported as a diagnostic's `source_path`. `podway-config` performs no I/O; the
    /// caller owns reading the file.
    pub source_path: &'a str,
    /// The source encoding, which is also the output encoding: there is no cross-format conversion.
    pub format: ProcedureDocumentFormat,
}

/// A successfully formatted Procedure v2 document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormattedProcedureV2 {
    document: String,
    /// The authoring tree the document was rendered from, kept so a drift diagnostic names its
    /// field exactly the way every other format-stage diagnostic does — with graph identifiers
    /// rather than array offsets.
    document_value: AuthoringValue,
    digest: Sha256Digest,
    changed: bool,
}

impl FormattedProcedureV2 {
    /// The canonical authoring text, ending in exactly one newline.
    pub fn document(&self) -> &str {
        &self.document
    }

    /// The canonical semantic digest, which formatting never changes.
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    /// Whether the source differs from the canonical form, compared as bytes — so a missing or
    /// doubled trailing newline, CRLF line endings, and a byte order mark all count as drift with
    /// no special case.
    pub const fn changed(&self) -> bool {
        self.changed
    }

    /// The single `FORMAT_NOT_CANONICAL` diagnostic describing a source that is not in canonical
    /// authoring form, or `None` when it already is.
    ///
    /// `format --check` and section 11.5's check pipeline both report drift through this
    /// constructor, so the two can never disagree about whether a document has drifted or about
    /// where. Exactly one diagnostic: drift is a property of the document, not a list of edits, and
    /// the canonical rendering the caller already holds is the complete remedy.
    ///
    /// `context` must wrap the very source this document was formatted from. The comparison reads
    /// [`AuthoringContext::source`] rather than the recorded [`Self::changed`] flag so the verdict
    /// and the reported position always describe the same text; for the intended context the two
    /// agree by construction.
    pub fn drift_diagnostic(&self, context: &AuthoringContext<'_>) -> Option<AuthoringDiagnostic> {
        let source = context.source();
        if source.as_bytes() == self.document.as_bytes() {
            return None;
        }

        let (line, column) = drift_position(source, &self.document);
        let field = context.index().anchor_at_line(line).map_or_else(
            || "$".to_owned(),
            |path| field_string(path, Some(&self.document_value)),
        );
        Some(AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::FormatNotCanonical,
            context.source_path(),
            context.span(line, column),
            field,
            "The source is not in canonical authoring form at this line.",
            diagnostic_hint(AuthoringDiagnosticCode::FormatNotCanonical),
        ))
    }
}

/// The one-based `(line, column)` where a source first diverges from its canonical rendering.
///
/// Both texts are split on `\n` — not on the source-line rule, which folds CRLF away — because the
/// comparison is over bytes: a source whose only defect is its line endings must report the column
/// where the first `\r` sits, not "no difference". The line is clamped to the source's own
/// newline-split count so the position never runs off the end of the text it describes, and a
/// divergence past the end of either side reports column 1, since the whole line is the difference.
fn drift_position(source: &str, formatted: &str) -> (u32, u32) {
    let source_lines: Vec<&str> = source.split('\n').collect();
    let formatted_lines: Vec<&str> = formatted.split('\n').collect();
    let shared = source_lines.len().min(formatted_lines.len());
    let offset = (0..shared)
        .find(|offset| source_lines[*offset] != formatted_lines[*offset])
        .unwrap_or(shared);

    let source_line_count = u32::try_from(source_lines.len()).unwrap_or(u32::MAX);
    let line = u32::try_from(offset.saturating_add(1))
        .unwrap_or(u32::MAX)
        .min(source_line_count)
        .max(1);
    let column = match (source_lines.get(offset), formatted_lines.get(offset)) {
        (Some(source_line), Some(formatted_line)) => {
            common_prefix_characters(source_line, formatted_line).saturating_add(1)
        }
        _ => 1,
    };
    (line, column)
}

/// The number of leading characters — not bytes; columns are character positions — the two lines
/// share.
fn common_prefix_characters(left: &str, right: &str) -> u32 {
    let shared = left
        .chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count();
    u32::try_from(shared).unwrap_or(u32::MAX)
}

/// Why a format request produced no document.
///
/// The two variants map to different result families, which is why they are distinct rather than
/// both being diagnostics: a v1 document cannot be *described* by the authoring diagnostic schema,
/// whose `schema` field is `const "podway.procedure/v2"`, so it is a command-level failure rather
/// than a finding about a v2 procedure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatFailure {
    /// The document declares a schema other than `podway.procedure/v2`.
    NotProcedureV2,
    /// The document is Procedure v2 and has findings, in `(line, column)` order.
    Diagnostics(Vec<AuthoringDiagnostic>),
}

/// Renders a Procedure v2 source document in canonical authoring form.
///
/// Stages, in order: dispatch and parse, validate, scan source constructs and collect comments
/// (YAML only), emit, then bound the emitted document. Nothing is emitted until every stage passes,
/// so a caller that writes files can treat a `Result` as "safe to write" without re-deriving the
/// ordering.
pub fn format_procedure_v2(
    request: FormatRequest<'_>,
) -> Result<FormattedProcedureV2, FormatFailure> {
    let context = AuthoringContext::new(request.source_path, request.source, request.format);

    // A document that *declares* the v1 schema is refused before the dispatching parser runs, so
    // a malformed v1 document is a wrong-schema command failure, never a v2 authoring finding
    // about a document that does not claim to be v2. The `V1` arm below stays as the total-match
    // backstop, but the sniff and the dispatcher read the same decoded `schema` field, so a
    // declared-v1 document cannot reach it.
    if crate::parser::sniff_procedure_schema(request.source.as_bytes(), request.format)
        == Some(crate::PROCEDURE_SCHEMA_V1)
    {
        return Err(FormatFailure::NotProcedureV2);
    }

    let parsed = match parse_procedure_document(request.source.as_bytes(), request.format) {
        Ok(ParsedProcedure::V2(parsed)) => parsed,
        Ok(ParsedProcedure::V1(_)) => return Err(FormatFailure::NotProcedureV2),
        Err(error) => {
            return Err(FormatFailure::Diagnostics(vec![config_error_diagnostic(
                &error, &context,
            )]));
        }
    };
    let validated = validate_procedure_v2(parsed).map_err(|error| {
        FormatFailure::Diagnostics(vec![config_error_diagnostic(&error, &context)])
    })?;

    let authoring = authoring_document_value(validated.parsed());
    let rendered = match request.format {
        ProcedureDocumentFormat::Yaml => {
            let comments = scan_source(request.source, context.index()).map_err(|violations| {
                FormatFailure::Diagnostics(construct_diagnostics(&violations, &context, &authoring))
            })?;
            emit_yaml(&authoring, comments)
        }
        ProcedureDocumentFormat::Json => emit_json(&authoring),
    };

    let characters = rendered.chars().count();
    if characters > SOURCE_PROJECTION_MAX_CHARACTERS {
        return Err(FormatFailure::Diagnostics(vec![
            projection_budget_diagnostic(&context, characters),
        ]));
    }

    Ok(FormattedProcedureV2 {
        changed: request.source.as_bytes() != rendered.as_bytes(),
        document: rendered,
        document_value: authoring,
        digest: validated.digest().clone(),
    })
}

fn construct_diagnostics(
    violations: &[SourceConstructViolation],
    context: &AuthoringContext<'_>,
    document: &AuthoringValue,
) -> Vec<AuthoringDiagnostic> {
    violations
        .iter()
        .map(|violation| {
            let field = context
                .index()
                .anchor_at_line(violation.line)
                .map_or_else(|| "$".to_owned(), |path| field_string(path, Some(document)));
            AuthoringDiagnostic::new(
                AuthoringDiagnosticCode::SourceConstructUnsupported,
                context.source_path(),
                context.span(violation.line, violation.column),
                field,
                format!(
                    "This source uses {}, which canonical authoring form cannot represent.",
                    violation.kind.description()
                ),
                diagnostic_hint(AuthoringDiagnosticCode::SourceConstructUnsupported),
            )
        })
        .collect()
}

fn projection_budget_diagnostic(
    context: &AuthoringContext<'_>,
    characters: usize,
) -> AuthoringDiagnostic {
    AuthoringDiagnostic::new(
        AuthoringDiagnosticCode::SourceProjectionBudgetExceeded,
        context.source_path(),
        SourceLocation::document_start(),
        "$",
        format!(
            "Canonical authoring form of this Procedure is {characters} characters, over the \
             {SOURCE_PROJECTION_MAX_CHARACTERS}-character source projection budget."
        ),
        diagnostic_hint(AuthoringDiagnosticCode::SourceProjectionBudgetExceeded),
    )
}

// ---------------------------------------------------------------------------------------------
// YAML emitter
// ---------------------------------------------------------------------------------------------

fn emit_yaml(document: &AuthoringValue, comments: SourceComments) -> String {
    let mut out = String::new();
    let mut comments = comments;
    let root = FieldPath::root();
    match document {
        AuthoringValue::Map(entries) => emit_yaml_map(&mut out, entries, 0, &root, &mut comments),
        AuthoringValue::Seq(items) => emit_yaml_sequence(&mut out, items, 0, &root, &mut comments),
        scalar => {
            out.push_str(&scalar_token(scalar));
            out.push('\n');
        }
    }
    for line in comments.into_trailing() {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn emit_yaml_map(
    out: &mut String,
    entries: &[(String, AuthoringValue)],
    indent: usize,
    base: &FieldPath,
    comments: &mut SourceComments,
) {
    for (key, value) in entries {
        let path = base.child_key(key);
        emit_comment_block(out, indent, comments.take(&path));
        push_indent(out, indent);
        emit_yaml_entry(out, key, value, indent, &path, comments);
    }
}

/// Writes `key:` plus its value, assuming the caller already wrote the leading indentation or the
/// `- ` marker.
fn emit_yaml_entry(
    out: &mut String,
    key: &str,
    value: &AuthoringValue,
    indent: usize,
    path: &FieldPath,
    comments: &mut SourceComments,
) {
    out.push_str(&text_token(key));
    out.push(':');
    match value {
        AuthoringValue::Seq(items) if items.is_empty() => out.push_str(" []\n"),
        AuthoringValue::Seq(items) => {
            out.push('\n');
            emit_yaml_sequence(out, items, indent + 2, path, comments);
        }
        AuthoringValue::Map(entries) if entries.is_empty() => out.push_str(" {}\n"),
        AuthoringValue::Map(entries) => {
            out.push('\n');
            emit_yaml_map(out, entries, indent + 2, path, comments);
        }
        scalar => {
            out.push(' ');
            out.push_str(&scalar_token(scalar));
            out.push('\n');
        }
    }
}

fn emit_yaml_sequence(
    out: &mut String,
    items: &[AuthoringValue],
    indent: usize,
    base: &FieldPath,
    comments: &mut SourceComments,
) {
    for (index, item) in items.iter().enumerate() {
        let path = base.child_index(index);
        emit_comment_block(out, indent, comments.take(&path));
        match item {
            AuthoringValue::Map(entries) if !entries.is_empty() => {
                // The element's first key shares the `- ` line, so a comment anchored to either the
                // element or that key renders above the marker. Re-scanning the output attributes
                // the merged block to the element, which renders identically — the property that
                // makes comment placement idempotent.
                let first = path.child_key(&entries[0].0);
                emit_comment_block(out, indent, comments.take(&first));
                for (offset, (key, value)) in entries.iter().enumerate() {
                    let child = path.child_key(key);
                    if offset == 0 {
                        push_indent(out, indent);
                        out.push_str("- ");
                    } else {
                        emit_comment_block(out, indent + 2, comments.take(&child));
                        push_indent(out, indent + 2);
                    }
                    emit_yaml_entry(out, key, value, indent + 2, &child, comments);
                }
            }
            AuthoringValue::Map(_) => {
                push_indent(out, indent);
                out.push_str("- {}\n");
            }
            AuthoringValue::Seq(nested) if nested.is_empty() => {
                push_indent(out, indent);
                out.push_str("- []\n");
            }
            AuthoringValue::Seq(nested) => {
                push_indent(out, indent);
                out.push_str("-\n");
                emit_yaml_sequence(out, nested, indent + 2, &path, comments);
            }
            scalar => {
                push_indent(out, indent);
                out.push_str("- ");
                out.push_str(&scalar_token(scalar));
                out.push('\n');
            }
        }
    }
}

fn emit_comment_block(out: &mut String, indent: usize, block: Option<Vec<String>>) {
    for line in block.into_iter().flatten() {
        push_indent(out, indent);
        out.push_str(&line);
        out.push('\n');
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn scalar_token(value: &AuthoringValue) -> String {
    match value {
        AuthoringValue::Text(text) => text_token(text),
        AuthoringValue::Flag(flag) => flag.to_string(),
        AuthoringValue::Integer(number) => number.to_string(),
        // Containers are written by their own emitters; a container never reaches this path.
        AuthoringValue::Seq(_) => "[]".to_owned(),
        AuthoringValue::Map(_) => "{}".to_owned(),
    }
}

fn text_token(value: &str) -> String {
    if is_plain_safe(value) {
        value.to_owned()
    } else {
        double_quoted(value)
    }
}

/// True when `value` may be written as a plain YAML scalar and read back byte-identically.
///
/// Every clause is a rejection, so the predicate is conservative by construction: over-quoting
/// costs two characters, under-quoting corrupts the document.
fn is_plain_safe(value: &str) -> bool {
    // 1. A plain scalar cannot be empty.
    let Some(first) = value.chars().next() else {
        return false;
    };
    // 2. No control character and no character a YAML reader may treat as a line break or as
    //    invisible. This is also what forces every multi-line string to be quoted.
    if value.chars().any(|character| {
        character.is_control() || matches!(character, '\u{2028}' | '\u{2029}' | '\u{feff}')
    }) {
        return false;
    }
    // 3. No leading indicator, no trailing `:`, no leading or trailing space. Authored whitespace is
    //    semantic — the digest depends on it — so it is preserved by quoting, never trimmed.
    if PLAIN_SCALAR_FORBIDDEN_FIRST.contains(first)
        || value.ends_with(':')
        || value.starts_with(' ')
        || value.ends_with(' ')
    {
        return false;
    }
    // 4. No embedded mapping separator and no embedded comment introducer.
    if value.contains(": ") || value.contains(" #") {
        return false;
    }
    // 5. The resolver oracle: the scalar must resolve back to a string, not to a boolean, a null, or
    //    a number. One call replaces a hand-maintained list of `true`/`~`/`0`/`1.5`/`.inf`/`.nan`.
    if !matches!(
        yaml_rust2::Yaml::from_str(value),
        yaml_rust2::Yaml::String(_)
    ) {
        return false;
    }
    // 6. YAML 1.1 boolean words, quoted although today's readers resolve YAML 1.2 core.
    if YAML_ONE_ONE_BOOLEAN_WORDS
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word))
    {
        return false;
    }
    // 7. The document's own preflight rule. Emission consults exactly what admission enforces.
    if crate::parser::reject_noncanonical_yaml_scalar(value).is_err() {
        return false;
    }
    // 8. The supported-construct scanner's own lexer. Without this the emitter could write a plain
    //    scalar its own `format --check` would then reject, and idempotence would not hold.
    if !plain_scalar_is_lexically_inert(value) {
        return false;
    }
    // 9. The reader's own verdict, and the last word. `serde_yaml` is the parser that will actually
    //    read this document back, and clause 5's resolver only approximates it: verified against
    //    `yaml-rust2` 0.11, `Yaml::from_str` resolves `null` but not `Null` or `NULL`, both of which
    //    `serde_yaml` reads as null. Round-tripping the candidate through the real reader closes
    //    that gap and every future one like it.
    survives_yaml_round_trip(value)
}

/// True when `serde_yaml` reads `value`, written plain as a mapping value, back byte-identically.
///
/// The target type is `serde_json::Value` because that is exactly what `decode_procedure_document`
/// deserializes into: `serde_yaml` is type-directed, so asking it for a `String` would hand back the
/// raw scalar text and hide the very resolution this clause exists to detect (`x: Null` really does
/// deserialize to the string "Null" when a `String` is requested, and to null when a value is).
fn survives_yaml_round_trip(value: &str) -> bool {
    let document = format!("x: {value}\n");
    matches!(
        serde_yaml::from_str::<serde_json::Value>(&document),
        Ok(serde_json::Value::Object(entries))
            if entries.get("x") == Some(&serde_json::Value::String(value.to_owned()))
    )
}

/// Writes a YAML double-quoted scalar using the JSON escape set, which YAML 1.2 accepts verbatim.
///
/// Non-ASCII printable characters pass through as UTF-8, matching Canonical JSON v1's own string
/// output, so a formatted document and its canonical projection agree on what a string contains.
fn double_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            character
                if character.is_control()
                    || matches!(character, '\u{2028}' | '\u{2029}' | '\u{feff}') =>
            {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------------------------
// JSON emitter
// ---------------------------------------------------------------------------------------------

fn emit_json(document: &AuthoringValue) -> String {
    let mut out = String::new();
    emit_json_value(&mut out, document, 0);
    out.push('\n');
    out
}

fn emit_json_value(out: &mut String, value: &AuthoringValue, indent: usize) {
    match value {
        AuthoringValue::Text(text) => out.push_str(&json_string(text)),
        AuthoringValue::Flag(flag) => out.push_str(if *flag { "true" } else { "false" }),
        AuthoringValue::Integer(number) => out.push_str(&number.to_string()),
        AuthoringValue::Seq(items) if items.is_empty() => out.push_str("[]"),
        AuthoringValue::Seq(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    out.push_str(",\n");
                }
                push_indent(out, indent + 2);
                emit_json_value(out, item, indent + 2);
            }
            out.push('\n');
            push_indent(out, indent);
            out.push(']');
        }
        AuthoringValue::Map(entries) if entries.is_empty() => out.push_str("{}"),
        AuthoringValue::Map(entries) => {
            out.push_str("{\n");
            for (index, (key, value)) in entries.iter().enumerate() {
                if index != 0 {
                    out.push_str(",\n");
                }
                push_indent(out, indent + 2);
                out.push_str(&json_string(key));
                out.push_str(": ");
                emit_json_value(out, value, indent + 2);
            }
            out.push('\n');
            push_indent(out, indent);
            out.push('}');
        }
    }
}

/// A JSON string literal with `serde_json`'s escape set, so authoring JSON and Canonical JSON v1
/// escape identically. The fallback keeps the emitter total: `serde_json` cannot fail serializing a
/// `str` into a `String`, and a formatter must not panic on a path a user can reach.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    })
}
