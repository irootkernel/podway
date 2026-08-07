//! Source-text analysis for Procedure v2 authoring: the path/span index, the supported-construct
//! grammar, and the comment side table (dossier sections 11.1 and 11.6).
//!
//! Everything here reads the *source document*, which the canonical pipeline deliberately does not.
//! Three products come out of it:
//!
//! 1. **A path index.** [`build_source_index`] walks `yaml_rust2`'s parser events and records, for
//!    every node, the line and column where it starts. Because the walk is shape-driven it indexes
//!    JSON with the same code — JSON is YAML flow. `Marker::line()` is already 1-indexed;
//!    `Marker::col()` is 0-indexed, so a column is `col() + 1`.
//! 2. **A supported-construct verdict.** [`scan_source`] rejects the constructs that would make
//!    comment attribution ambiguous or byte-preserving rewriting impossible. The invariant it
//!    enforces is one sentence: *every YAML node begins and ends on one source line.*
//! 3. **A comment side table.** Full-line comments only. A run of consecutive full-line comment
//!    lines is one block; a block attaches to the next content line, and re-emits immediately above
//!    that line's anchor. Blank lines are not preserved — the formatter owns vertical whitespace.
//!
//! **Anchoring rule.** Several paths can start on one line (`- id: perform` starts both the
//! sequence element and its `id` entry). A comment attaches to the *shortest* path anchored on that
//! line, so it re-emits above the `- ` marker rather than between the marker and its first key. The
//! document root is excluded from anchoring, which is what makes a leading comment block attach to
//! the first root key — the key the emitter always writes first — with no special case.
//!
//! **Nothing is ever dropped.** The emitter visits every path the model carries, and the model
//! carries every path the source could have anchored a comment to (unknown fields are rejected at
//! parse, explicitly empty optional collections are rejected by the parser, and neither arrays nor
//! authoring maps are reordered). As a defensive floor for anything that argument misses, a block
//! whose anchor the emitter never visits is appended to the trailing block instead of discarded.

use std::collections::BTreeMap;

use yaml_rust2::parser::{Event, Parser};

use crate::procedure_v2_document::AuthoringValue;

/// A hard ceiling on parser events, so a pathological document cannot spin the walk forever. The
/// bounded decoder already caps a document at 100,000 nodes; each node contributes a small,
/// constant number of events.
const MAX_SOURCE_INDEX_EVENTS: usize = 1_000_000;

/// The authoring maps whose keys the author chooses. Their keys render in brackets in a diagnostic
/// `field` path (`node_definitions[work]`), matching the identifier-indexed rendering of
/// order-bearing arrays; fixed schema field names render with dots.
const AUTHORING_MAP_KEYS: [&str; 3] = ["node_definitions", "routes", "outcomes"];

/// Sequence keys whose elements are bare identifier strings that still name something stable, so a
/// diagnostic `field` reports the identifier rather than a positional index.
const IDENTIFIER_SEQUENCE_KEYS: [&str; 2] = ["allowed_targets", "items"];

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------

/// One step of a structural path through the document.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum PathSegment {
    Key(String),
    Index(usize),
}

/// A structural path: the index-based address used for source lookup and comment attachment.
///
/// This is deliberately *not* the diagnostic `field` string. [`field_string`] renders that,
/// substituting identifiers for positional indices where the element carries one.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FieldPath(Vec<PathSegment>);

impl FieldPath {
    pub(crate) const fn root() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn child_key(&self, key: impl Into<String>) -> Self {
        let mut segments = self.0.clone();
        segments.push(PathSegment::Key(key.into()));
        Self(segments)
    }

    pub(crate) fn child_index(&self, index: usize) -> Self {
        let mut segments = self.0.clone();
        segments.push(PathSegment::Index(index));
        Self(segments)
    }

    pub(crate) fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    pub(crate) fn depth(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path with its last segment removed, or `None` at the root.
    pub(crate) fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            return None;
        }
        let mut segments = self.0.clone();
        segments.pop();
        Some(Self(segments))
    }
}

/// Renders the diagnostic `field` string for a structural path.
///
/// Order-bearing arrays whose elements carry a stable identifier report that identifier
/// (`graph.nodes[confirm-closeout].evidence_from[finish-not-achieved]`, exactly the dossier's own
/// section 11.6 example); arrays of free-form strings report a 0-based index. Author-chosen map
/// keys render in brackets, fixed schema field names with dots. The root renders as `$`, satisfying
/// the diagnostic schema's `minLength: 1`.
///
/// `document` is optional because a parse failure has no model to resolve identifiers against; in
/// that case every index renders positionally.
pub(crate) fn field_string(path: &FieldPath, document: Option<&AuthoringValue>) -> String {
    if path.is_root() {
        return "$".to_owned();
    }

    let mut rendered = String::new();
    let mut cursor = document;
    let mut parent_key: Option<&str> = None;
    for segment in path.segments() {
        match segment {
            PathSegment::Key(key) => {
                if rendered.is_empty() {
                    rendered.push_str(key);
                } else if parent_key.is_some_and(|parent| AUTHORING_MAP_KEYS.contains(&parent)) {
                    rendered.push('[');
                    rendered.push_str(key);
                    rendered.push(']');
                } else {
                    rendered.push('.');
                    rendered.push_str(key);
                }
                cursor = cursor.and_then(|value| match value {
                    AuthoringValue::Map(entries) => entries
                        .iter()
                        .find(|(candidate, _)| candidate == key)
                        .map(|(_, value)| value),
                    _ => None,
                });
                parent_key = Some(key);
            }
            PathSegment::Index(index) => {
                let element = cursor.and_then(|value| match value {
                    AuthoringValue::Seq(values) => values.get(*index),
                    _ => None,
                });
                let token = element
                    .and_then(|value| element_identifier(value, parent_key))
                    .unwrap_or_else(|| index.to_string());
                rendered.push('[');
                rendered.push_str(&token);
                rendered.push(']');
                cursor = element;
            }
        }
    }
    rendered
}

fn element_identifier(value: &AuthoringValue, parent_key: Option<&str>) -> Option<String> {
    match value {
        AuthoringValue::Map(entries) => entries
            .iter()
            .find(|(key, _)| key == "id" || key == "node")
            .and_then(|(_, value)| match value {
                AuthoringValue::Text(text) => Some(text.clone()),
                _ => None,
            }),
        AuthoringValue::Text(text)
            if parent_key.is_some_and(|parent| IDENTIFIER_SEQUENCE_KEYS.contains(&parent)) =>
        {
            Some(text.clone())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Source index
// ---------------------------------------------------------------------------------------------

/// One indexed node: where it starts, and its scalar text when it is a leaf.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceIndexEntry {
    line: u32,
    column: u32,
    path: FieldPath,
    scalar: Option<String>,
}

/// Path-to-span index over one source document.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceIndex {
    entries: Vec<SourceIndexEntry>,
    by_path: BTreeMap<FieldPath, usize>,
    anchor_by_line: BTreeMap<u32, usize>,
    line_lengths: Vec<u32>,
}

impl SourceIndex {
    /// The one-based `(line, column)` where `path` starts, when the source declares it.
    pub(crate) fn position(&self, path: &FieldPath) -> Option<(u32, u32)> {
        self.by_path
            .get(path)
            .map(|index| (self.entries[*index].line, self.entries[*index].column))
    }

    /// The shortest non-root path anchored on `line`: the node a comment on the preceding line
    /// belongs to, and the node a line-scoped diagnostic names.
    pub(crate) fn anchor_at_line(&self, line: u32) -> Option<&FieldPath> {
        self.anchor_by_line
            .get(&line)
            .map(|index| &self.entries[*index].path)
    }

    /// The character length of a one-based source line, or 0 when the line does not exist.
    pub(crate) fn line_length(&self, line: u32) -> u32 {
        line.checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.line_lengths.get(index))
            .copied()
            .unwrap_or(0)
    }

    /// The first leaf whose path ends in `field`'s last dotted segment and whose scalar text is
    /// `value`.
    ///
    /// `ConfigError` variants name a *shape* (`graph.nodes.use`) plus the offending value but carry
    /// no index, so this recovers a precise span from the pair. Source order makes the answer
    /// deterministic; the caller falls back to the longest-present-prefix rule when it misses.
    pub(crate) fn find_scalar(&self, field: &str, value: &str) -> Option<(u32, u32)> {
        let leaf = field.rsplit('.').next()?;
        self.entries
            .iter()
            .find(|entry| {
                entry.scalar.as_deref() == Some(value)
                    && matches!(
                        entry.path.segments().last(),
                        Some(PathSegment::Key(key)) if key == leaf
                    )
            })
            .map(|entry| (entry.line, entry.column))
    }
}

/// The value slot a node is about to occupy.
enum ValueSlot {
    /// The node is a mapping value; its key entry already anchors the path at this index.
    MapValue(usize),
    /// The node is a sequence element or the document root; it needs its own entry.
    Fresh(FieldPath),
}

enum Frame {
    Map { base: FieldPath, key: Option<usize> },
    Seq { base: FieldPath, index: usize },
}

/// Indexes a source document by walking `yaml_rust2` parser events.
///
/// Never fails: a scan error truncates the index rather than aborting the caller, because a
/// diagnostic without a precise location is still a diagnostic while a panic is not. Aliases are
/// unreachable (the shared preflight rejects them before any caller reaches this) and abandon the
/// walk if they somehow appear.
///
/// Marker semantics, verified against `yaml_rust2` 0.11: a `Scalar` event marks its own token
/// start; a `SequenceStart` marks the `-` of the first entry; a `MappingStart` marks the `:` of its
/// first key. Only the first is directly useful, so two normalizations apply. A mapping entry is
/// anchored at its *key* scalar, which is where a reader looks and where a comment above it
/// belongs. A sequence element is anchored at its `- ` marker rather than at whatever token the
/// event happens to carry — matching the dossier's own section 11.6 example, whose column points at
/// the `- ` of an `evidence_from` entry.
pub(crate) fn build_source_index(source: &str) -> SourceIndex {
    let lines = source_lines(source);
    let mut parser = Parser::new_from_str(source);
    let mut stack: Vec<Frame> = Vec::new();
    let mut entries: Vec<SourceIndexEntry> = Vec::new();

    for _ in 0..MAX_SOURCE_INDEX_EVENTS {
        let Ok((event, marker)) = parser.next_token() else {
            break;
        };
        let line = u32::try_from(marker.line()).unwrap_or(u32::MAX);
        // `Marker::col()` is 0-indexed; diagnostic columns are 1-indexed.
        let column = u32::try_from(marker.col())
            .unwrap_or(u32::MAX)
            .saturating_add(1);

        match event {
            Event::StreamEnd => break,
            Event::Nothing | Event::StreamStart | Event::DocumentStart | Event::DocumentEnd => {}
            Event::Alias(_) => break,
            Event::Scalar(value, ..) => {
                if matches!(stack.last(), Some(Frame::Map { key: None, .. })) {
                    let Some(Frame::Map { base, key }) = stack.last_mut() else {
                        break;
                    };
                    let path = base.child_key(value);
                    *key = Some(entries.len());
                    entries.push(SourceIndexEntry {
                        line,
                        column,
                        path,
                        scalar: None,
                    });
                } else {
                    match value_slot(&mut stack) {
                        ValueSlot::MapValue(index) => entries[index].scalar = Some(value),
                        ValueSlot::Fresh(path) => entries.push(SourceIndexEntry {
                            line,
                            column: element_column(&lines, &path, line, column),
                            path,
                            scalar: Some(value),
                        }),
                    }
                }
            }
            Event::SequenceStart(..) => {
                let base = container_path(&mut stack, &mut entries, &lines, line, column);
                stack.push(Frame::Seq { base, index: 0 });
            }
            Event::MappingStart(..) => {
                let base = container_path(&mut stack, &mut entries, &lines, line, column);
                stack.push(Frame::Map { base, key: None });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                stack.pop();
            }
        }
    }

    finish_index(entries, source)
}

/// Normalizes a freshly anchored node's column: the document root anchors at the document start,
/// and a block-sequence element anchors at its `- ` marker.
fn element_column(lines: &[&str], path: &FieldPath, line: u32, column: u32) -> u32 {
    if path.is_root() {
        return 1;
    }
    let Some(text) = line
        .checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| lines.get(index))
    else {
        return column;
    };
    let characters = text.chars().collect::<Vec<_>>();
    let limit = usize::try_from(column.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(characters.len());
    let Some(first) = characters[..limit]
        .iter()
        .position(|character| !character.is_whitespace())
    else {
        return column;
    };
    let is_block_entry = characters[first] == '-'
        && characters
            .get(first + 1)
            .is_none_or(|character| character.is_whitespace());
    if is_block_entry {
        column_of(first)
    } else {
        column
    }
}

fn value_slot(stack: &mut [Frame]) -> ValueSlot {
    match stack.last_mut() {
        Some(Frame::Map { key, .. }) => match key.take() {
            Some(index) => ValueSlot::MapValue(index),
            None => ValueSlot::Fresh(FieldPath::root()),
        },
        Some(Frame::Seq { base, index }) => {
            let path = base.child_index(*index);
            *index += 1;
            ValueSlot::Fresh(path)
        }
        None => ValueSlot::Fresh(FieldPath::root()),
    }
}

/// The path of a container that is starting, recording a fresh entry only when the container is not
/// already anchored by its mapping key.
fn container_path(
    stack: &mut [Frame],
    entries: &mut Vec<SourceIndexEntry>,
    lines: &[&str],
    line: u32,
    column: u32,
) -> FieldPath {
    match value_slot(stack) {
        ValueSlot::MapValue(index) => entries[index].path.clone(),
        ValueSlot::Fresh(path) => {
            entries.push(SourceIndexEntry {
                line,
                column: element_column(lines, &path, line, column),
                path: path.clone(),
                scalar: None,
            });
            path
        }
    }
}

fn finish_index(entries: Vec<SourceIndexEntry>, source: &str) -> SourceIndex {
    let mut by_path: BTreeMap<FieldPath, usize> = BTreeMap::new();
    let mut anchor_by_line: BTreeMap<u32, usize> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        by_path.entry(entry.path.clone()).or_insert(index);
        if entry.path.is_root() {
            continue;
        }
        let replaces = match anchor_by_line.get(&entry.line) {
            Some(existing) => {
                let existing = &entries[*existing];
                (entry.path.depth(), entry.column, &entry.path)
                    < (existing.path.depth(), existing.column, &existing.path)
            }
            None => true,
        };
        if replaces {
            anchor_by_line.insert(entry.line, index);
        }
    }

    SourceIndex {
        entries,
        by_path,
        anchor_by_line,
        line_lengths: source_lines(source)
            .into_iter()
            .map(|line| u32::try_from(line.chars().count()).unwrap_or(u32::MAX))
            .collect(),
    }
}

/// Splits a document into lines, dropping the `\r` of a CRLF pair but keeping every other `\r`
/// (which [`scan_source`] rejects as a lone carriage return).
fn source_lines(source: &str) -> Vec<&str> {
    let segments = source.split('\n').collect::<Vec<_>>();
    let last = segments.len().saturating_sub(1);
    segments
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            if index == last {
                segment
            } else {
                segment.strip_suffix('\r').unwrap_or(segment)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Supported-construct scan
// ---------------------------------------------------------------------------------------------

/// A source construct the authoring toolchain refuses to rewrite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceConstructKind {
    ByteOrderMark,
    LoneCarriageReturn,
    UnicodeLineBreak,
    InlineComment,
    BlockScalar,
    MultiLineQuotedScalar,
    MultiLineFlowCollection,
}

impl SourceConstructKind {
    /// A short noun phrase naming the construct, embedded in the diagnostic message.
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::ByteOrderMark => "a leading byte order mark",
            Self::LoneCarriageReturn => "a carriage return that is not part of a CRLF line break",
            Self::UnicodeLineBreak => {
                "a Unicode line-break character (U+0085, U+2028, or U+2029) written literally"
            }
            Self::InlineComment => "an inline trailing comment",
            Self::BlockScalar => "a block scalar",
            Self::MultiLineQuotedScalar => "a quoted scalar that spans more than one line",
            Self::MultiLineFlowCollection => "a flow collection that spans more than one line",
        }
    }
}

/// One rejected construct, located at its first character.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceConstructViolation {
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) kind: SourceConstructKind,
}

/// The comment side table extracted from one source document.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceComments {
    attached: BTreeMap<FieldPath, Vec<String>>,
    trailing: Vec<String>,
}

impl SourceComments {
    /// Removes and returns the block attached to `path`, if any. The emitter consumes blocks as it
    /// visits paths so that whatever is left over can be recovered rather than lost.
    pub(crate) fn take(&mut self, path: &FieldPath) -> Option<Vec<String>> {
        self.attached.remove(path)
    }

    /// The trailing block, plus every block whose anchor the emitter never visited.
    pub(crate) fn into_trailing(self) -> Vec<String> {
        let mut lines = Vec::new();
        for (_, block) in self.attached {
            lines.extend(block);
        }
        lines.extend(self.trailing);
        lines
    }
}

/// Classifies every source line, rejects unsupported constructs, and builds the comment table.
///
/// Every violation in the document is collected and reported in `(line, column)` order: this stage
/// is naturally multi-diagnostic (it is a lexical sweep, not a fail-fast parse), and an author
/// fixing an unsupported construct benefits from seeing all of them at once.
pub(crate) fn scan_source(
    source: &str,
    index: &SourceIndex,
) -> Result<SourceComments, Vec<SourceConstructViolation>> {
    let mut violations = Vec::new();
    if source.starts_with('\u{feff}') {
        violations.push(SourceConstructViolation {
            line: 1,
            column: 1,
            kind: SourceConstructKind::ByteOrderMark,
        });
    }

    let lines = source_lines(source);
    let mut classes = Vec::with_capacity(lines.len());
    for (offset, line) in lines.iter().enumerate() {
        let number = u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(1);
        for (column, character) in line.chars().enumerate() {
            // Characters a YAML reader breaks lines on but `split('\n')` does not. Left in place they
            // would silently fold to a space inside a quoted scalar (verified for U+0085 and a lone
            // `\r` against `serde_yaml` 0.9) and would put every location this module reports one or
            // more lines out, so the "one node per source line" invariant rejects them outright.
            let kind = match character {
                '\r' => SourceConstructKind::LoneCarriageReturn,
                '\u{85}' | '\u{2028}' | '\u{2029}' => SourceConstructKind::UnicodeLineBreak,
                _ => continue,
            };
            violations.push(SourceConstructViolation {
                line: number,
                column: u32::try_from(column).unwrap_or(u32::MAX).saturating_add(1),
                kind,
            });
        }
        classes.push(classify_line(line, number, &mut violations));
    }

    violations.sort_by_key(|violation| (violation.line, violation.column));
    if !violations.is_empty() {
        return Err(violations);
    }

    Ok(attach_comments(&classes, index))
}

/// What one source line contributes to the comment table.
enum LineClass {
    Blank,
    Comment(String),
    Content,
}

fn classify_line(
    line: &str,
    number: u32,
    violations: &mut Vec<SourceConstructViolation>,
) -> LineClass {
    let outcome = lex_line(line);
    for (column, kind) in outcome.violations {
        violations.push(SourceConstructViolation {
            line: number,
            column,
            kind,
        });
    }
    if outcome.is_comment_line {
        LineClass::Comment(line.trim().to_owned())
    } else if line.trim().is_empty() {
        LineClass::Blank
    } else {
        LineClass::Content
    }
}

fn attach_comments(classes: &[LineClass], index: &SourceIndex) -> SourceComments {
    let mut comments = SourceComments::default();
    let mut block: Vec<String> = Vec::new();
    for (offset, class) in classes.iter().enumerate() {
        match class {
            LineClass::Comment(text) => block.push(text.clone()),
            LineClass::Blank => {}
            LineClass::Content => {
                if block.is_empty() {
                    continue;
                }
                let number = u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(1);
                match index.anchor_at_line(number) {
                    Some(path) => comments
                        .attached
                        .entry(path.clone())
                        .or_default()
                        .append(&mut block),
                    None => comments.trailing.append(&mut block),
                }
            }
        }
    }
    comments.trailing.append(&mut block);
    comments
}

// ---------------------------------------------------------------------------------------------
// Line lexer
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Eq, PartialEq)]
enum LexState {
    Out,
    Single,
    Double,
}

struct LineOutcome {
    is_comment_line: bool,
    violations: Vec<(u32, SourceConstructKind)>,
}

/// Lexes one source line under the "every node starts and ends on one line" invariant.
///
/// Three states plus a flow-collection depth counter. A quote opens a quoted scalar only at a token
/// start (line start, or after whitespace or an indicator), so `don't` stays plain text; a `#` is a
/// comment only as the first non-whitespace character of the line or after whitespace, so `a#b`
/// stays plain text. Both rules exist to avoid false positives on ordinary authored prose.
fn lex_line(line: &str) -> LineOutcome {
    let characters = line.chars().collect::<Vec<_>>();
    let mut state = LexState::Out;
    let mut depth = 0usize;
    let mut seen_content = false;
    let mut violations = Vec::new();
    let mut is_comment_line = false;
    let mut cursor = 0usize;

    while cursor < characters.len() {
        let character = characters[cursor];
        match state {
            LexState::Out => match character {
                '\'' | '"' if opens_quoted_scalar(&characters, cursor) => {
                    state = if character == '\'' {
                        LexState::Single
                    } else {
                        LexState::Double
                    };
                }
                '{' | '[' => depth += 1,
                '}' | ']' => depth = depth.saturating_sub(1),
                '#' => {
                    if !seen_content {
                        is_comment_line = true;
                        break;
                    }
                    if matches!(
                        cursor
                            .checked_sub(1)
                            .and_then(|index| characters.get(index)),
                        Some(' ' | '\t')
                    ) {
                        violations.push((column_of(cursor), SourceConstructKind::InlineComment));
                        break;
                    }
                }
                '|' | '>' if is_block_scalar_header(&characters, cursor) => {
                    violations.push((column_of(cursor), SourceConstructKind::BlockScalar));
                }
                _ => {}
            },
            LexState::Single => {
                if character == '\'' {
                    if characters.get(cursor + 1) == Some(&'\'') {
                        cursor += 1;
                    } else {
                        state = LexState::Out;
                    }
                }
            }
            LexState::Double => {
                if character == '\\' {
                    cursor += 1;
                } else if character == '"' {
                    state = LexState::Out;
                }
            }
        }
        if !character.is_whitespace() {
            seen_content = true;
        }
        cursor += 1;
    }

    if !is_comment_line {
        let end = column_of(characters.len());
        if state != LexState::Out {
            violations.push((end, SourceConstructKind::MultiLineQuotedScalar));
        } else if depth != 0 {
            violations.push((end, SourceConstructKind::MultiLineFlowCollection));
        }
    }

    LineOutcome {
        is_comment_line,
        violations,
    }
}

fn column_of(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(1)
}

/// A quote opens a quoted scalar only where a token can start: line start, after whitespace, or
/// immediately after a YAML indicator.
fn opens_quoted_scalar(characters: &[char], cursor: usize) -> bool {
    match cursor
        .checked_sub(1)
        .and_then(|index| characters.get(index))
    {
        None => true,
        Some(' ' | '\t' | '-' | ':' | ',' | '[' | '{' | '?') => true,
        Some(_) => false,
    }
}

/// A `|`/`>` is a block scalar header only when it opens a value: it must follow whitespace (or
/// start the line), the text before it must be empty or end with a `:` or `-` indicator, and only
/// chomping/indentation indicators may follow it before end of line.
///
/// Both halves matter. Without the "follows whitespace" half, `a-|` inside a plain scalar would be
/// rejected; without the indicator half, `a - >` would be.
fn is_block_scalar_header(characters: &[char], cursor: usize) -> bool {
    match cursor
        .checked_sub(1)
        .and_then(|index| characters.get(index))
    {
        None => {}
        Some(' ' | '\t') => {}
        Some(_) => return false,
    }
    let prefix = characters[..cursor].iter().collect::<String>();
    let prefix = prefix.trim_end();
    if !(prefix.is_empty() || prefix.ends_with(':') || prefix.ends_with('-')) {
        return false;
    }

    let mut tail = cursor + 1;
    let mut indicators = 0;
    while indicators < 2 && matches!(characters.get(tail), Some('+' | '-' | '1'..='9')) {
        tail += 1;
        indicators += 1;
    }
    characters[tail..]
        .iter()
        .all(|character| character.is_whitespace())
}

/// True when a plain scalar, written as the value of a block mapping entry, is inert under
/// [`lex_line`].
///
/// This is the clause that makes the formatter a fixpoint of its own construct scan: a scalar the
/// scanner would read as an inline comment, a block scalar header, an unterminated quote, or an
/// unbalanced flow collection is quoted instead of emitted plain. Checking by executing the lexer
/// is stronger than enumerating hazards, and it cannot drift from the scanner it protects against.
pub(crate) fn plain_scalar_is_lexically_inert(value: &str) -> bool {
    let outcome = lex_line(&format!("x: {value}"));
    !outcome.is_comment_line && outcome.violations.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        "schema: podway.procedure/v2\n",
        "id: sample\n",
        "node_definitions:\n",
        "  work:\n",
        "    type: action\n",
        "graph:\n",
        "  entry: perform\n",
        "  nodes:\n",
        "    - id: perform\n",
        "      use: work\n",
        "      terminal: true\n",
    );

    fn path(segments: &[&str]) -> FieldPath {
        segments
            .iter()
            .fold(FieldPath::root(), |path, segment| path.child_key(*segment))
    }

    #[test]
    fn index_anchors_every_path_with_one_based_lines_and_columns() {
        let index = build_source_index(SAMPLE);

        assert_eq!(index.position(&FieldPath::root()), Some((1, 1)));
        assert_eq!(index.position(&path(&["schema"])), Some((1, 1)));
        assert_eq!(
            index.position(&path(&["node_definitions", "work"])),
            Some((4, 3))
        );
        assert_eq!(
            index.position(&path(&["node_definitions", "work", "type"])),
            Some((5, 5))
        );
        // A key anchors at its own line even when its value opens on the next one.
        assert_eq!(index.position(&path(&["graph", "nodes"])), Some((8, 3)));
        // A sequence element anchors at its `- ` marker, not at the token the event carries.
        assert_eq!(
            index.position(&path(&["graph"]).child_key("nodes").child_index(0)),
            Some((9, 5))
        );
    }

    #[test]
    fn the_shortest_path_wins_a_shared_anchor_line_and_the_root_never_anchors() {
        let index = build_source_index(SAMPLE);

        // Line 1 carries both the document root and `schema`; the root is excluded so a leading
        // comment lands on the first key the emitter writes.
        assert_eq!(index.anchor_at_line(1), Some(&path(&["schema"])));
        // Line 9 carries the sequence element and its `id` entry; the element wins.
        assert_eq!(
            index.anchor_at_line(9),
            Some(
                &FieldPath::root()
                    .child_key("graph")
                    .child_key("nodes")
                    .child_index(0)
            )
        );
        assert_eq!(index.line_length(1), 27);
        assert_eq!(index.line_length(999), 0);
    }

    #[test]
    fn find_scalar_recovers_a_span_from_a_shape_and_a_value() {
        let index = build_source_index(SAMPLE);
        assert_eq!(index.find_scalar("graph.nodes.use", "work"), Some((10, 7)));
        assert_eq!(index.find_scalar("graph.nodes.use", "absent"), None);
    }

    #[test]
    fn field_string_uses_identifiers_for_id_bearing_arrays_and_authoring_maps() {
        let document = AuthoringValue::Map(vec![
            (
                "node_definitions".to_owned(),
                AuthoringValue::Map(vec![(
                    "work".to_owned(),
                    AuthoringValue::Map(vec![(
                        "instructions".to_owned(),
                        AuthoringValue::Seq(vec![AuthoringValue::Text("Read.".to_owned())]),
                    )]),
                )]),
            ),
            (
                "graph".to_owned(),
                AuthoringValue::Map(vec![(
                    "nodes".to_owned(),
                    AuthoringValue::Seq(vec![AuthoringValue::Map(vec![
                        ("id".to_owned(), AuthoringValue::Text("perform".to_owned())),
                        (
                            "evidence_from".to_owned(),
                            AuthoringValue::Seq(vec![AuthoringValue::Map(vec![(
                                "node".to_owned(),
                                AuthoringValue::Text("start".to_owned()),
                            )])]),
                        ),
                    ])]),
                )]),
            ),
            (
                "manual_rework".to_owned(),
                AuthoringValue::Map(vec![(
                    "allowed_targets".to_owned(),
                    AuthoringValue::Seq(vec![AuthoringValue::Text("perform".to_owned())]),
                )]),
            ),
        ]);

        assert_eq!(field_string(&FieldPath::root(), Some(&document)), "$");
        assert_eq!(
            field_string(&path(&["node_definitions", "work"]), Some(&document)),
            "node_definitions[work]"
        );
        assert_eq!(
            field_string(
                &path(&["node_definitions", "work", "instructions"]).child_index(0),
                Some(&document)
            ),
            "node_definitions[work].instructions[0]"
        );
        assert_eq!(
            field_string(
                &path(&["graph", "nodes"])
                    .child_index(0)
                    .child_key("evidence_from")
                    .child_index(0),
                Some(&document)
            ),
            "graph.nodes[perform].evidence_from[start]"
        );
        assert_eq!(
            field_string(
                &path(&["manual_rework", "allowed_targets"]).child_index(0),
                Some(&document)
            ),
            "manual_rework.allowed_targets[perform]"
        );
        // With no model, every index renders positionally.
        assert_eq!(
            field_string(&path(&["graph", "nodes"]).child_index(0), None),
            "graph.nodes[0]"
        );
    }

    #[test]
    fn parent_and_depth_walk_the_structural_path() {
        let leaf = path(&["graph", "nodes"]).child_index(2).child_key("use");
        assert_eq!(leaf.depth(), 4);
        let parent = leaf.parent().expect("a leaf has a parent");
        assert_eq!(parent, path(&["graph", "nodes"]).child_index(2));
        assert!(FieldPath::root().parent().is_none());
        assert!(FieldPath::root().is_root());
    }

    fn violations(source: &str) -> Vec<SourceConstructViolation> {
        let index = build_source_index(source);
        scan_source(source, &index).expect_err("source must be rejected")
    }

    #[test]
    fn a_clean_document_yields_no_violations_and_no_comments() {
        let index = build_source_index(SAMPLE);
        let mut comments = scan_source(SAMPLE, &index).expect("clean source is accepted");
        assert!(comments.take(&path(&["schema"])).is_none());
        assert!(comments.into_trailing().is_empty());
    }

    #[test]
    fn every_unsupported_construct_is_rejected_at_its_first_character() {
        assert_eq!(
            violations("id: sample # trailing\n")[0],
            SourceConstructViolation {
                line: 1,
                column: 12,
                kind: SourceConstructKind::InlineComment,
            }
        );
        assert_eq!(
            violations("intent: |\n  text\n")[0],
            SourceConstructViolation {
                line: 1,
                column: 9,
                kind: SourceConstructKind::BlockScalar,
            }
        );
        assert_eq!(
            violations("intent: \"open\n")[0].kind,
            SourceConstructKind::MultiLineQuotedScalar
        );
        assert_eq!(
            violations("choices: [a,\n  b]\n")[0].kind,
            SourceConstructKind::MultiLineFlowCollection
        );
        assert_eq!(
            violations("id: a\rb\n")[0],
            SourceConstructViolation {
                line: 1,
                column: 6,
                kind: SourceConstructKind::LoneCarriageReturn,
            }
        );
        assert_eq!(
            violations("\u{feff}id: sample\n")[0],
            SourceConstructViolation {
                line: 1,
                column: 1,
                kind: SourceConstructKind::ByteOrderMark,
            }
        );
    }

    #[test]
    fn ordinary_prose_is_never_mistaken_for_an_unsupported_construct() {
        for line in [
            "intent: don't stop",
            "intent: a#b",
            "intent: say \"hi\" now",
            "intent: a-|",
            "intent: a > b",
            "intent: choose [a] or [b]",
            "intent: 'quoted value'",
        ] {
            let outcome = lex_line(line);
            assert!(
                outcome.violations.is_empty() && !outcome.is_comment_line,
                "{line:?} must lex clean"
            );
        }
        // CRLF is drift, never an unsupported construct.
        let crlf = SAMPLE.replace('\n', "\r\n");
        let crlf_index = build_source_index(&crlf);
        assert!(scan_source(&crlf, &crlf_index).is_ok());
    }

    #[test]
    fn plain_scalar_inertness_rejects_exactly_the_scanner_hazards() {
        for inert in [
            "Record the result.",
            "a#b",
            "don't",
            "it's",
            "a-|",
            "a > b",
            "[a] b",
        ] {
            assert!(plain_scalar_is_lexically_inert(inert), "{inert:?}");
        }
        for hazard in ["x #comment", "a- |", "say \"hi", "a [b", "quote \" open"] {
            assert!(!plain_scalar_is_lexically_inert(hazard), "{hazard:?}");
        }
    }

    #[test]
    fn comment_blocks_attach_to_the_next_content_lines_shortest_anchor() {
        let source = concat!(
            "# leading one\n",
            "\n",
            "# leading two\n",
            "schema: podway.procedure/v2\n",
            "node_definitions:\n",
            "  # about work\n",
            "  work:\n",
            "    type: action\n",
            "graph:\n",
            "  entry: perform\n",
            "  nodes:\n",
            "    # about the first placement\n",
            "    - id: perform\n",
            "      use: work\n",
            "# trailing\n",
        );
        let index = build_source_index(source);
        let mut comments = scan_source(source, &index).expect("comment source is supported");

        assert_eq!(
            comments.take(&path(&["schema"])),
            Some(vec!["# leading one".to_owned(), "# leading two".to_owned()])
        );
        assert_eq!(
            comments.take(&path(&["node_definitions", "work"])),
            Some(vec!["# about work".to_owned()])
        );
        assert_eq!(
            comments.take(&path(&["graph", "nodes"]).child_index(0)),
            Some(vec!["# about the first placement".to_owned()])
        );
        assert_eq!(comments.into_trailing(), vec!["# trailing".to_owned()]);
    }
}
