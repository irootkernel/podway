//! Authoring advisory rules common to Procedures and execution groups.

use podway_core::{ActionDefinitionV2, ItemSpecV2, MAX_ATTEMPT_CONTENT_SCALARS_V2};

/// Minimum characters before the primary guidance fields — `purpose`, action `intent`, decision
/// `objective`, decision `prompt` — count as saying anything.
pub(crate) const WEAK_GUIDANCE_MIN_CHARS_PRIMARY: usize = 12;
/// Minimum characters for the secondary guidance fields — option `criteria`, `reason.prompt` —
/// which qualify an already-stated decision rather than introducing one.
pub(crate) const WEAK_GUIDANCE_MIN_CHARS_SECONDARY: usize = 8;
/// Minimum alphanumeric-bearing words in any guidance field. One word is a label, not guidance.
const WEAK_GUIDANCE_MIN_WORDS: usize = 2;
/// The fewest characters two graph node identifiers must both have before a one-edit difference
/// between them is confusing rather than simply short.
const CONFUSABLE_ID_MIN_CHARS: usize = 4;
/// The per-rule finding cap for the pairwise (quadratic) rules.
pub(crate) const PAIRWISE_RULE_MAX_FINDINGS: usize = 8;

/// Prefixes that mark a field as an unfinished note when followed by punctuation.
///
/// There is deliberately no single-token placeholder table: any one-word value — `placeholder`,
/// `n/a`, `...` — is already weak under the two-word minimum, so a table there would be dead code
/// pretending to carry weight.
const WEAK_GUIDANCE_PREFIXES: &[&str] = &["todo", "tbd", "fixme", "xxx"];

/// Advisory for the shared per-attempt aggregate over declared text and list maxima.
pub(crate) fn attempt_content_budget_warning(action: &ActionDefinitionV2) -> Option<String> {
    let id = action.id().as_str();
    let mut charges: Vec<(u64, &str)> = action
        .items()
        .iter()
        .filter_map(|item| match item {
            ItemSpecV2::Text(text) => Some((u64::from(text.max_length()), item.id().as_str())),
            ItemSpecV2::List(list) => {
                Some((u64::from(list.effective_total_length()), item.id().as_str()))
            }
            _ => None,
        })
        .collect();
    let total: u64 = charges.iter().map(|(charge, _)| *charge).sum();
    if total <= MAX_ATTEMPT_CONTENT_SCALARS_V2 {
        return None;
    }
    charges.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let contributors = charges
        .iter()
        .take(3)
        .map(|(charge, item)| format!("`{item}` at {charge}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "The action definition `{id}` declares {total} scalars of worst-case recorded content, over the {MAX_ATTEMPT_CONTENT_SCALARS_V2}-scalar per-attempt aggregate. The largest contributors are {contributors}."
    ))
}

/// Whether a guidance field says nothing useful.
///
/// Five independent clauses, any of which is enough. An absent field is weak: for `criteria` and
/// `reason.prompt` the absence and an empty gesture are the same failure from the reader's side.
pub(crate) fn is_weak_guidance(value: Option<&str>, minimum_chars: usize) -> bool {
    let Some(value) = value else {
        return true;
    };
    let trimmed = value.trim();
    if trimmed.chars().count() < minimum_chars {
        return true;
    }
    let words = trimmed
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count();
    if words < WEAK_GUIDANCE_MIN_WORDS {
        return true;
    }
    let normalized = normalize_text(trimmed);
    if WEAK_GUIDANCE_PREFIXES.iter().any(|prefix| {
        normalized
            .strip_prefix(prefix)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|next| !next.is_alphanumeric())
    }) {
        return true;
    }
    is_unfilled_placeholder_value(trimmed)
}

/// Whether the whole value is one `<…>` span: the shape of an authoring template nobody filled in.
///
/// The rule is deliberately whole-value, not substring: `Verify the page renders <h1> first.` and
/// `Confirm 0 < latency and p99 > 1s.` are ordinary prose that a substring scan would misreport,
/// while a field whose entire content is `<describe the outcome>` carries no guidance at all.
fn is_unfilled_placeholder_value(value: &str) -> bool {
    let mut characters = value.chars();
    if characters.next() != Some('<') {
        return false;
    }
    let Some(last) = value.chars().next_back() else {
        return false;
    };
    if last != '>' {
        return false;
    }
    // Exactly one span: no interior delimiter may close or reopen it.
    let interior: Vec<char> = value.chars().skip(1).collect();
    let interior = &interior[..interior.len().saturating_sub(1)];
    !interior
        .iter()
        .any(|character| *character == '<' || *character == '>')
}

/// The comparison form of an authored text: lowercase, whitespace-collapsed, trimmed, and stripped
/// of trailing sentence punctuation. Two labels that differ only in those respects read the same.
pub(crate) fn normalize_text(value: &str) -> String {
    let collapsed = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed
        .trim_end_matches(['.', '!', '?'])
        .trim()
        .to_owned()
}

/// Whether two graph node identifiers are easy to confuse.
pub(crate) fn are_confusable(left: &str, right: &str) -> bool {
    if left.replace('-', "").to_lowercase() == right.replace('-', "").to_lowercase() {
        return true;
    }
    let left_characters: Vec<char> = left.chars().collect();
    let right_characters: Vec<char> = right.chars().collect();
    if left_characters.len() < CONFUSABLE_ID_MIN_CHARS
        || right_characters.len() < CONFUSABLE_ID_MIN_CHARS
    {
        return false;
    }
    if left_characters.len().abs_diff(right_characters.len()) > 1 {
        return false;
    }
    edit_distance(&left_characters, &right_characters) <= 1
}

/// Levenshtein distance over two character slices, computed with one rolling row.
///
/// Hand-rolled rather than pulled in: `podway-config`'s dependency set is a contract surface, and a
/// 64-identifier comparison does not justify widening it.
fn edit_distance(left: &[char], right: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len().saturating_add(1)];
    for (row, left_character) in left.iter().enumerate() {
        current[0] = row.saturating_add(1);
        for (column, right_character) in right.iter().enumerate() {
            let substitution = usize::from(left_character != right_character);
            current[column.saturating_add(1)] = previous[column]
                .saturating_add(substitution)
                .min(previous[column.saturating_add(1)].saturating_add(1))
                .min(current[column].saturating_add(1));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}
