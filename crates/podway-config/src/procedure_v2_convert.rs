//! Procedure v1 → v2 conversion: the deterministic, review-required candidate `podway procedure
//! convert` emits (roadmap V2AUT-007).
//!
//! **A conversion is a candidate, not a migration.** v1 is a linear stage list; v2 is a graph with
//! decisions, evidence wiring, and session goals. Nothing in a v1 document says where a branch
//! belongs, so this module never invents one: every stage becomes one action definition placed once
//! in a linear chain, and the two values v2 requires that v1 has no field for — the procedure
//! `purpose` and each action `intent` — are synthesized from a fixed template and marked with a
//! full-line review comment. The output is a starting point an author edits, and it says so in its
//! own text.
//!
//! **Determinism is structural.** The input is `parse_procedure_v1` — the byte-locked v1 admission
//! path — the mapping is a pure function of the validated v1 model, and the output goes through the
//! very emitter `procedure format` uses. There is no clock, no counter, no hash-map iteration, and
//! no second copy of the canonical shape rules, so the same v1 document converts to the same bytes
//! forever, and the result is a fixpoint of `procedure format --check` by construction rather than
//! by inspection.
//!
//! **The shared pipeline is reused end to end.** The mapping builds the v2 *wire* document and
//! hands it to [`map_document`], the same function the YAML and JSON front ends call, so every
//! identifier rule, every text bound, and every collection bound is enforced by the code that
//! enforces them for an authored document. Then `validate_procedure_v2` resolves the closed
//! references and computes the digest, and the emitter renders it. A conversion cannot produce a
//! document the rest of the toolchain would reject, because it is admitted by the same toolchain.
//!
//! **Overflow is reported, never truncated.** v2 tightened many v1 bounds (a 500-character v1 item
//! prompt against v2's 300, a 4,000-character v1 help against v2's 1,000, and so on). A value that
//! does not fit is an `AUTHORING_SCHEMA_INVALID` diagnostic naming the *v1* source path and the *v1*
//! source position, so the author edits the file they still own; silently shortening the text would
//! change what the procedure asks for. Unlike parsing and validation, which stop at the first
//! failure because there is no model to keep walking, this scan walks the whole v1 model and reports
//! every overflow at once — the v1 model is already complete, so there is nothing to lose by
//! continuing.

use podway_core::{AuthoringDiagnostic, AuthoringDiagnosticCode, Sha256Digest};

use crate::procedure_v2_diagnostics::{AuthoringContext, config_error_diagnostic};
use crate::procedure_v2_format::emit_procedure_v2_yaml;
use crate::procedure_v2_parse::map_document;
use crate::procedure_v2_source::{FieldPath, SourceComments};
use crate::procedure_v2_wire::{
    GraphPlacementWire, GraphWire, ItemWire, ManualReworkWire, NodeDefinitionWire, OrderedMap,
    PROCEDURE_SCHEMA_V2, ProcedureV2DocumentWire, SkipPolicyWire,
};
use crate::{
    ItemDefinitionV1, ProcedureDefinitionV1, ReturnTargetsV1, StageDefinitionV1,
    ValidatedProcedureV1, validate_procedure_v2,
};

// -------------------------------------------------------------------------------------------
// The Procedure v2 bounds a legal Procedure v1 value can exceed
// -------------------------------------------------------------------------------------------
//
// These mirror private constants of `podway_core::procedure_v2::{items, definitions}` and of
// `procedure_v2_parse`. They are duplicated rather than exported because a bound is an
// implementation detail of the constructor that enforces it, and widening `podway-core`'s public
// surface to let one caller pre-check it would make every future tightening a breaking change.
// The duplication is pinned instead: `int_v2_procedure_convert.rs` asserts, for every constant
// below, that the real v2 constructor accepts the bound and rejects one past it, so a tightened v2
// bound fails a test here rather than silently producing a conversion that cannot validate.

/// `procedure.purpose`, which rule P-a copies a short v1 description into.
const V2_MAX_PURPOSE_CHARS: usize = 500;
/// `procedure.description`, which rule P-b carries a long v1 description into verbatim.
const V2_MAX_DESCRIPTION_CHARS: usize = 1_000;
/// Instructions per action definition. v1 allows 32.
const V2_MAX_INSTRUCTIONS: usize = 16;
/// Characters per instruction. v1 allows 2,000.
const V2_MAX_INSTRUCTION_CHARS: usize = 1_000;
/// Items per definition. v1 allows 128 per stage.
const V2_MAX_ITEMS: usize = 64;
/// Characters in an item prompt. v1 allows 500.
const V2_MAX_ITEM_PROMPT_CHARS: usize = 300;
/// Characters in item help. v1 allows 4,000.
const V2_MAX_ITEM_HELP_CHARS: usize = 1_000;
/// Both text-item length constraints. v1 allows 65,536 and defaults to 8,000.
const V2_MAX_TEXT_LENGTH: u32 = 16_384;
/// Values in a choice item. v1 allows 64.
const V2_MAX_CHOICES: usize = 32;
/// Both list-item count constraints. v1 allows 1,000 and defaults to 100.
const V2_MAX_LIST_ITEMS: u16 = 200;
/// Characters in one list entry. v1 allows 4,000 and defaults to 500.
const V2_MAX_LIST_ITEM_LENGTH: u16 = 1_000;

// -------------------------------------------------------------------------------------------
// Synthesis
// -------------------------------------------------------------------------------------------

/// The review comment written above a synthesized `purpose` (rules P-b and P-c).
const SYNTHESIZED_PURPOSE_COMMENT: &str =
    "# Synthesized from the v1 procedure name; review and replace.";

/// The review comment written above every synthesized `intent`.
const SYNTHESIZED_INTENT_COMMENT: &str =
    "# Synthesized from the v1 stage title; review and replace.";

/// The review comment written above `manual_rework.allowed_targets` when v1 said `any_previous`.
///
/// Three fixed lines: what happened, why the list is total, and the runtime rule that makes the
/// total list faithful rather than permissive. The second and third exist because a reviewer who
/// sees every node listed will otherwise read the conversion as having widened rework, and dossier
/// section 9.5 is the reason it has not.
const ANY_PREVIOUS_REWORK_COMMENT: [&str; 3] = [
    "# Converted from v1 rework.allow_return_to: any_previous; narrow this list.",
    "# A v2 target list is static, so cursor-relative previous cannot be authored and every",
    "# node is listed; section 9.5 still requires the target to have a valid prior attempt.",
];

/// The remediation every overflow diagnostic carries.
///
/// One string rather than one per code: the remedy is the same for all twelve overflow classes, and
/// it is not the remedy the shared per-code hint gives. `AUTHORING_SCHEMA_INVALID` normally says to
/// correct the field against the v2 schema, which would send an author to a file that does not
/// exist yet — the document they can still edit is the v1 source.
const OVERFLOW_HINT: &str = "Reduce the reported value in the Procedure v1 source, then convert \
                             again.";

/// The `purpose` template rules P-b and P-c synthesize from the v1 procedure name.
///
/// A v1 `name` is at most 120 characters, so the result is at most 143 — well inside v2's 500 — and
/// it is 3 words of ordinary prose, so it never trips the weak-guidance lint rules.
fn synthesized_purpose(name: &str) -> String {
    format!("Complete the {name} workflow.")
}

/// The `intent` template every converted action definition carries.
///
/// A v1 stage title is at most 120 characters, so the result is at most 140 — well inside v2's 300.
fn synthesized_intent(title: &str) -> String {
    format!("Complete the {title} stage.")
}

/// How the v1 `description` became the v2 `purpose` and (sometimes) the v2 `description`.
///
/// v2 requires a `purpose` and v1 has no such field, so one of three rules always applies. The
/// split point is v2's own 500-character `purpose` bound, and the fallback is the 1,000-character
/// `description` bound underneath it:
///
/// - **P-a** — the trimmed v1 description fits in `purpose`: adopt it, and write no `description`.
///   The author's own sentence becomes the procedure's stated purpose, which is the closest thing
///   v1 has to one.
/// - **P-b** — it does not fit in `purpose` but the untrimmed text fits in `description`: keep the
///   description verbatim and synthesize a purpose above it. Nothing is lost and nothing is cut.
/// - **P-c** — there is no description, or it is only whitespace: synthesize a purpose and write no
///   description.
struct PurposeMapping {
    purpose: String,
    description: Option<String>,
    /// True for P-b and P-c, which is exactly when the purpose needs a review comment.
    synthesized: bool,
}

impl PurposeMapping {
    fn of(definition: &ProcedureDefinitionV1) -> Self {
        let synthesized = |description: Option<String>| Self {
            purpose: synthesized_purpose(&definition.name),
            description,
            synthesized: true,
        };
        let Some(description) = definition.description.as_deref() else {
            return synthesized(None); // P-c
        };
        let trimmed = description.trim();
        if trimmed.is_empty() {
            return synthesized(None); // P-c
        }
        if trimmed.chars().count() <= V2_MAX_PURPOSE_CHARS {
            // P-a. The trimmed form is adopted because `purpose` rejects surrounding whitespace as
            // guidance; the digest of the v1 document is unaffected either way, and `description`
            // is not also written, which would duplicate the same sentence twice in one document.
            return Self {
                purpose: trimmed.to_owned(),
                description: None,
                synthesized: false,
            };
        }
        synthesized(Some(description.to_owned())) // P-b
    }
}

// -------------------------------------------------------------------------------------------
// Public surface
// -------------------------------------------------------------------------------------------

/// A Procedure v1 document rendered as an equivalent Procedure v2 authoring candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConvertedProcedureV2 {
    document: String,
    digest: Sha256Digest,
    source_digest: Sha256Digest,
}

impl ConvertedProcedureV2 {
    /// The candidate in canonical authoring YAML, ending in exactly one newline.
    ///
    /// Always YAML, whatever the v1 source was encoded as: the review comments this conversion
    /// attaches are the point of it, and JSON cannot carry them. A JSON v1 document and its YAML
    /// twin therefore convert to identical bytes.
    pub fn document(&self) -> &str {
        &self.document
    }

    /// The canonical semantic digest of the converted Procedure v2 candidate.
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    /// The canonical semantic digest of the Procedure v1 document this came from.
    ///
    /// Reported beside the target digest so a reviewer can pin both ends of the conversion: the
    /// same value `podway procedure validate` reports for the same file.
    pub fn source_digest(&self) -> &Sha256Digest {
        &self.source_digest
    }
}

/// Converts one validated Procedure v1 document into a Procedure v2 authoring candidate.
///
/// `context` must wrap the very source `v1` was parsed from. Nothing is read out of it except
/// diagnostic geography: an overflow reports the v1 source path and the position of the offending
/// v1 value, which is the only position that helps — the v2 document the author would otherwise be
/// pointed at does not exist, precisely because the conversion failed.
///
/// Either the whole candidate is produced or none of it is. Every diagnostic returned is an error,
/// and the vector is in v1 document order; the caller merges it through `finalize_diagnostics`,
/// which applies the shared sort and the 256 bound exactly as it does for every other authoring
/// command.
pub fn convert_procedure_v1_to_v2(
    v1: &ValidatedProcedureV1,
    context: &AuthoringContext<'_>,
) -> Result<ConvertedProcedureV2, Vec<AuthoringDiagnostic>> {
    let definition = v1.definition();

    let overflows = OverflowScan::run(definition, context);
    if !overflows.is_empty() {
        return Err(overflows);
    }

    let purpose = PurposeMapping::of(definition);
    let wire = document_wire(definition, &purpose)
        .ok_or_else(|| vec![empty_stage_list_diagnostic(context)])?;

    // Past this point every failure is defensive. The overflow scan has already checked every bound
    // v2 tightened, and every other v1 rule is at least as strict as its v2 counterpart, so a
    // rejection here means the two bound tables have drifted. It is reported rather than panicked,
    // through the same classifier every other config failure goes through: a conversion must never
    // abort the process, and a wrong-but-honest location beats no diagnostic at all.
    let parsed =
        map_document(wire).map_err(|error| vec![config_error_diagnostic(&error, context)])?;
    let validated = validate_procedure_v2(parsed)
        .map_err(|error| vec![config_error_diagnostic(&error, context)])?;

    let document =
        emit_procedure_v2_yaml(&validated, review_comments(definition, &purpose), context)?;

    Ok(ConvertedProcedureV2 {
        document,
        digest: validated.digest().clone(),
        source_digest: v1.digest().clone(),
    })
}

// -------------------------------------------------------------------------------------------
// Mapping
// -------------------------------------------------------------------------------------------

/// Builds the v2 wire document, or `None` when the v1 model declares no stage.
///
/// `None` is unreachable: `ValidatedProcedureV1` has already enforced `1..=64` stages. It exists so
/// the entry node can be named without indexing.
fn document_wire(
    definition: &ProcedureDefinitionV1,
    purpose: &PurposeMapping,
) -> Option<ProcedureV2DocumentWire> {
    let entry = definition.stages.first()?.id.clone();
    let node_definitions = OrderedMap::from_entries(
        definition
            .stages
            .iter()
            .map(|stage| (stage.id.clone(), action_definition_wire(stage)))
            .collect(),
    );
    let nodes = definition
        .stages
        .iter()
        .enumerate()
        .map(|(index, stage)| placement_wire(&definition.stages, index, stage))
        .collect();

    Some(ProcedureV2DocumentWire {
        schema: PROCEDURE_SCHEMA_V2.to_owned(),
        id: definition.id.clone(),
        version: definition.version.clone(),
        name: definition.name.clone(),
        purpose: purpose.purpose.clone(),
        description: purpose.description.clone(),
        // v1 has no session-goal concept, and opting a converted procedure into goal tracking would
        // invent a commitment the author never made — and would immediately lint, because a
        // goal-tracked procedure with no assessment decision is incomplete.
        goal_tracking: None,
        node_definitions,
        graph: GraphWire { entry, nodes },
        manual_rework: Some(ManualReworkWire {
            allowed_targets: rework_targets(definition),
        }),
    })
}

/// One stage's reusable action definition. The definition identifier is the stage identifier: v1
/// stage identifiers are already unique, so no synthesis and no collision handling is needed.
fn action_definition_wire(stage: &StageDefinitionV1) -> NodeDefinitionWire {
    NodeDefinitionWire::Action {
        title: stage.title.clone(),
        intent: synthesized_intent(&stage.title),
        // v1 has no per-stage description. The stage's guidance lives in `instructions`, which maps
        // across unchanged.
        description: None,
        // An optional v2 collection is omitted when empty, never written as `[]`: the schema gives
        // each one `minItems: 1` and the parser rejects an explicitly empty one.
        instructions: (!stage.instructions.is_empty()).then(|| stage.instructions.clone()),
        items: (!stage.items.is_empty())
            .then(|| stage.items.iter().map(item_wire).collect::<Vec<_>>()),
    }
}

/// One stage's graph placement: the linear chain v1 execution already is.
///
/// The placement identifier is the stage identifier, so `manual_rework` targets and the graph agree
/// with the v1 document's own vocabulary and a reviewer reads familiar names.
fn placement_wire(
    stages: &[StageDefinitionV1],
    index: usize,
    stage: &StageDefinitionV1,
) -> GraphPlacementWire {
    let next = stages.get(index + 1).map(|stage| stage.id.clone());
    GraphPlacementWire {
        id: stage.id.clone(),
        use_: stage.id.clone(),
        // v1 has no evidence wiring: a stage reads nothing back from an earlier stage.
        evidence_from: None,
        // v2 has no representation for "skipping is explicitly disallowed" — an absent skip policy
        // *is* not skippable — so a v1 `skip: {allowed: false}` maps to omission with no loss.
        // `ValidatedProcedureV1` has already normalized that case to `None` and has already
        // materialized `reason_required`, so the `filter` and the `unwrap_or` are belt and braces
        // over a model that cannot reach them.
        skip: stage
            .skip
            .as_ref()
            .filter(|skip| skip.allowed)
            .map(|skip| SkipPolicyWire {
                allowed: true,
                reason_required: skip.reason_required.unwrap_or(true),
            }),
        terminal: next.is_none().then_some(true),
        next,
        routes: None,
    }
}

/// The `manual_rework.allowed_targets` list.
///
/// `Only(ids)` maps across in v1 order, already validated against the stage set.
///
/// `any_previous` becomes **every** graph node in stage order. v2's target list is static, so
/// "previous" — a property of where the cursor is — cannot be expressed at authoring time at all.
/// Listing every node is the faithful static over-approximation rather than a widening, because
/// dossier section 9.5 makes "the target graph node has a valid attempt on the current valid
/// execution trace" a *runtime* precondition of manual rework, and "has already been attempted" is
/// exactly what v1 meant by "previous". The static list says which nodes may ever be returned to;
/// the runtime still decides which of them may be returned to now.
fn rework_targets(definition: &ProcedureDefinitionV1) -> Vec<String> {
    match &definition.rework.allow_return_to {
        ReturnTargetsV1::Only(stage_ids) => stage_ids.clone(),
        ReturnTargetsV1::AnyPrevious => definition
            .stages
            .iter()
            .map(|stage| stage.id.clone())
            .collect(),
    }
}

/// One item, field for field.
///
/// Every field is written explicitly from the v1 value, including the ones v1 itself defaulted:
/// `serde` materialized those at deserialization, so the struct already holds the v1-effective
/// value, and that value is the v1 semantics whether or not the author typed it. Writing it
/// explicitly matters because the v2 defaults differ — an omitted v1 text `max_length` means 8,000
/// while an omitted v2 one means 4,000 — so carrying the field across is what makes the conversion
/// meaning-preserving rather than merely shape-preserving.
fn item_wire(item: &ItemDefinitionV1) -> ItemWire {
    match item {
        ItemDefinitionV1::Confirm {
            id,
            prompt,
            help,
            required,
        } => ItemWire::Confirm {
            id: id.clone(),
            prompt: prompt.clone(),
            help: help.clone(),
            required: *required,
        },
        ItemDefinitionV1::Text {
            id,
            prompt,
            help,
            required,
            min_length,
            max_length,
            multiline,
        } => ItemWire::Text {
            id: id.clone(),
            prompt: prompt.clone(),
            help: help.clone(),
            required: *required,
            min_length: *min_length,
            max_length: *max_length,
            multiline: *multiline,
        },
        ItemDefinitionV1::Choice {
            id,
            prompt,
            help,
            required,
            choices,
        } => ItemWire::Choice {
            id: id.clone(),
            prompt: prompt.clone(),
            help: help.clone(),
            required: *required,
            choices: choices.clone(),
        },
        ItemDefinitionV1::Integer {
            id,
            prompt,
            help,
            required,
            minimum,
            maximum,
        } => ItemWire::Integer {
            id: id.clone(),
            prompt: prompt.clone(),
            help: help.clone(),
            required: *required,
            minimum: *minimum,
            maximum: *maximum,
        },
        ItemDefinitionV1::List {
            id,
            prompt,
            help,
            required,
            min_items,
            max_items,
            max_item_length,
            unique,
        } => ItemWire::List {
            id: id.clone(),
            prompt: prompt.clone(),
            help: help.clone(),
            required: *required,
            min_items: *min_items,
            max_items: *max_items,
            max_item_length: *max_item_length,
            unique: *unique,
        },
        ItemDefinitionV1::Artifact {
            id,
            prompt,
            help,
            required,
            allowed_media_types,
        } => ItemWire::Artifact {
            id: id.clone(),
            prompt: prompt.clone(),
            help: help.clone(),
            required: *required,
            // An empty v1 list and an absent one mean the same thing — no restriction — and v2 has
            // only the absent form. `ValidatedProcedureV1` already normalized the empty case away.
            allowed_media_types: allowed_media_types
                .as_ref()
                .filter(|media_types| !media_types.is_empty())
                .cloned(),
        },
    }
}

// -------------------------------------------------------------------------------------------
// Review comments
// -------------------------------------------------------------------------------------------

/// The comment side table the emitter weaves into the converted document.
///
/// Each block anchors to the structural path of the field it annotates, which is how a comment
/// carried across from a real source anchors too — so the converted document is a fixpoint of
/// `procedure format` for its comments as well as for its layout: re-scanning the output attaches
/// the same blocks to the same anchors and re-emits them byte for byte.
fn review_comments(definition: &ProcedureDefinitionV1, purpose: &PurposeMapping) -> SourceComments {
    let root = FieldPath::root();
    let mut attached = Vec::new();

    if purpose.synthesized {
        attached.push((
            root.child_key("purpose"),
            vec![SYNTHESIZED_PURPOSE_COMMENT.to_owned()],
        ));
    }

    let definitions = root.child_key("node_definitions");
    for stage in &definition.stages {
        attached.push((
            definitions.child_key(&stage.id).child_key("intent"),
            vec![SYNTHESIZED_INTENT_COMMENT.to_owned()],
        ));
    }

    if matches!(
        definition.rework.allow_return_to,
        ReturnTargetsV1::AnyPrevious
    ) {
        attached.push((
            root.child_key("manual_rework").child_key("allowed_targets"),
            ANY_PREVIOUS_REWORK_COMMENT
                .iter()
                .map(|line| (*line).to_owned())
                .collect(),
        ));
    }

    SourceComments::from_attached(attached)
}

// -------------------------------------------------------------------------------------------
// Overflow scan
// -------------------------------------------------------------------------------------------

/// Walks the whole v1 model and collects every value v2 will not accept.
///
/// The walk is over the v1 model rather than over a partially built v2 document because the v2
/// constructors stop at the first rejection: an author with six oversized prompts would otherwise
/// have to convert six times. Every diagnostic names a v1 path and a v1 position, so the report
/// reads as a list of edits to make in the file the author has.
struct OverflowScan<'a> {
    context: &'a AuthoringContext<'a>,
    diagnostics: Vec<AuthoringDiagnostic>,
}

impl<'a> OverflowScan<'a> {
    fn run(
        definition: &ProcedureDefinitionV1,
        context: &'a AuthoringContext<'a>,
    ) -> Vec<AuthoringDiagnostic> {
        let mut scan = Self {
            context,
            diagnostics: Vec::new(),
        };
        scan.document(definition);
        scan.diagnostics
    }

    /// Records one overflow. `path` is the *structural* v1 path used to find the source position;
    /// `field` is the identifier-indexed v1 path a reader edits by.
    fn report(&mut self, path: &FieldPath, field: String, message: String) {
        self.diagnostics.push(AuthoringDiagnostic::new(
            AuthoringDiagnosticCode::AuthoringSchemaInvalid,
            self.context.source_path(),
            self.context.locate(path),
            field,
            message,
            OVERFLOW_HINT,
        ));
    }

    fn document(&mut self, definition: &ProcedureDefinitionV1) {
        self.description(definition);
        let stages = FieldPath::root().child_key("stages");
        for (index, stage) in definition.stages.iter().enumerate() {
            self.stage(&stages.child_index(index), stage);
        }
    }

    /// **O1.** A v1 description over 500 characters is not an error — rule P-b keeps it as the v2
    /// `description` — so the overflow is only the case where it does not fit there either. v1
    /// allows 4,000 characters; v2 allows 1,000.
    fn description(&mut self, definition: &ProcedureDefinitionV1) {
        let Some(description) = definition.description.as_deref() else {
            return;
        };
        let trimmed = description.trim().chars().count();
        let length = description.chars().count();
        if trimmed <= V2_MAX_PURPOSE_CHARS || length <= V2_MAX_DESCRIPTION_CHARS {
            return;
        }
        self.report(
            &FieldPath::root().child_key("description"),
            "description".to_owned(),
            format!(
                "The v1 procedure description is {length} characters, over the \
                 {V2_MAX_DESCRIPTION_CHARS}-character Procedure v2 description bound."
            ),
        );
    }

    fn stage(&mut self, path: &FieldPath, stage: &StageDefinitionV1) {
        // O2: v1 allows 32 instructions per stage, v2 allows 16 per definition.
        let instructions = path.child_key("instructions");
        if stage.instructions.len() > V2_MAX_INSTRUCTIONS {
            self.report(
                &instructions,
                format!("stages[{}].instructions", stage.id),
                format!(
                    "The v1 stage declares {} instructions, over the Procedure v2 maximum of \
                     {V2_MAX_INSTRUCTIONS}.",
                    stage.instructions.len()
                ),
            );
        }
        // O3: v1 allows 2,000 characters per instruction, v2 allows 1,000.
        for (index, instruction) in stage.instructions.iter().enumerate() {
            let length = instruction.chars().count();
            if length > V2_MAX_INSTRUCTION_CHARS {
                self.report(
                    &instructions.child_index(index),
                    format!("stages[{}].instructions[{index}]", stage.id),
                    format!(
                        "The v1 instruction is {length} characters, over the \
                         {V2_MAX_INSTRUCTION_CHARS}-character Procedure v2 bound."
                    ),
                );
            }
        }

        // O4: v1 allows 128 items per stage, v2 allows 64 per definition.
        let items = path.child_key("items");
        if stage.items.len() > V2_MAX_ITEMS {
            self.report(
                &items,
                format!("stages[{}].items", stage.id),
                format!(
                    "The v1 stage declares {} items, over the Procedure v2 maximum of \
                     {V2_MAX_ITEMS}.",
                    stage.items.len()
                ),
            );
        }
        for (index, item) in stage.items.iter().enumerate() {
            self.item(&items.child_index(index), &stage.id, item);
        }
    }

    fn item(&mut self, path: &FieldPath, stage_id: &str, item: &ItemDefinitionV1) {
        let field = |leaf: &str| format!("stages[{stage_id}].items[{}].{leaf}", item.id());

        // O5: v1 allows a 500-character prompt, v2 allows 300.
        let (prompt, help) = item_text(item);
        let length = prompt.chars().count();
        if length > V2_MAX_ITEM_PROMPT_CHARS {
            self.report(
                &path.child_key("prompt"),
                field("prompt"),
                format!(
                    "The v1 item prompt is {length} characters, over the \
                     {V2_MAX_ITEM_PROMPT_CHARS}-character Procedure v2 bound."
                ),
            );
        }
        // O6: v1 allows 4,000 characters of help, v2 allows 1,000.
        if let Some(help) = help {
            let length = help.chars().count();
            if length > V2_MAX_ITEM_HELP_CHARS {
                self.report(
                    &path.child_key("help"),
                    field("help"),
                    format!(
                        "The v1 item help is {length} characters, over the \
                         {V2_MAX_ITEM_HELP_CHARS}-character Procedure v2 bound."
                    ),
                );
            }
        }

        match item {
            ItemDefinitionV1::Text {
                min_length,
                max_length,
                ..
            } => {
                // O7 and O8. v2 bounds `max_length` at 16,384 and requires `min_length` not to
                // exceed it, so an oversized `min_length` always brings an oversized `max_length`
                // with it — O7 cannot be provoked on its own. Both are still reported, because both
                // are values the author has to change.
                if *min_length > V2_MAX_TEXT_LENGTH {
                    self.report(
                        &path.child_key("min_length"),
                        field("min_length"),
                        format!(
                            "The v1 text item requires at least {min_length} characters, over the \
                             Procedure v2 maximum of {V2_MAX_TEXT_LENGTH}."
                        ),
                    );
                }
                if *max_length > V2_MAX_TEXT_LENGTH {
                    self.report(
                        &path.child_key("max_length"),
                        field("max_length"),
                        format!(
                            "The v1 text item allows up to {max_length} characters, over the \
                             Procedure v2 maximum of {V2_MAX_TEXT_LENGTH}."
                        ),
                    );
                }
            }
            // O9: v1 allows 64 choices, v2 allows 32.
            ItemDefinitionV1::Choice { choices, .. } if choices.len() > V2_MAX_CHOICES => {
                self.report(
                    &path.child_key("choices"),
                    field("choices"),
                    format!(
                        "The v1 choice item declares {} values, over the Procedure v2 maximum of \
                         {V2_MAX_CHOICES}.",
                        choices.len()
                    ),
                );
            }
            ItemDefinitionV1::List {
                min_items,
                max_items,
                max_item_length,
                ..
            } => {
                // O10 and O11, with the same relationship O7 has to O8: v2 bounds `max_items` at
                // 200 and requires `min_items` not to exceed it, so O10 never fires alone.
                if *min_items > V2_MAX_LIST_ITEMS {
                    self.report(
                        &path.child_key("min_items"),
                        field("min_items"),
                        format!(
                            "The v1 list item requires at least {min_items} entries, over the \
                             Procedure v2 maximum of {V2_MAX_LIST_ITEMS}."
                        ),
                    );
                }
                if *max_items > V2_MAX_LIST_ITEMS {
                    self.report(
                        &path.child_key("max_items"),
                        field("max_items"),
                        format!(
                            "The v1 list item allows up to {max_items} entries, over the Procedure \
                             v2 maximum of {V2_MAX_LIST_ITEMS}."
                        ),
                    );
                }
                // O12: v1 allows a 4,000-character entry, v2 allows 1,000.
                if *max_item_length > V2_MAX_LIST_ITEM_LENGTH {
                    self.report(
                        &path.child_key("max_item_length"),
                        field("max_item_length"),
                        format!(
                            "The v1 list item allows {max_item_length}-character entries, over the \
                             Procedure v2 maximum of {V2_MAX_LIST_ITEM_LENGTH}."
                        ),
                    );
                }
            }
            // Confirm, integer, and artifact items carry nothing v2 bounds more tightly than v1:
            // the integer bounds are `i64` on both sides, and both cap artifact media types at 64.
            ItemDefinitionV1::Confirm { .. }
            | ItemDefinitionV1::Choice { .. }
            | ItemDefinitionV1::Integer { .. }
            | ItemDefinitionV1::Artifact { .. } => {}
        }
    }
}

/// The `prompt` and `help` every v1 item variant shares.
fn item_text(item: &ItemDefinitionV1) -> (&str, Option<&str>) {
    match item {
        ItemDefinitionV1::Confirm { prompt, help, .. }
        | ItemDefinitionV1::Text { prompt, help, .. }
        | ItemDefinitionV1::Choice { prompt, help, .. }
        | ItemDefinitionV1::Integer { prompt, help, .. }
        | ItemDefinitionV1::List { prompt, help, .. }
        | ItemDefinitionV1::Artifact { prompt, help, .. } => (prompt, help.as_deref()),
    }
}

/// The defensive diagnostic for a v1 model with no stage, which `ValidatedProcedureV1` cannot
/// produce. Reported rather than panicked, for the same reason every other unreachable failure in
/// this module is.
fn empty_stage_list_diagnostic(context: &AuthoringContext<'_>) -> AuthoringDiagnostic {
    AuthoringDiagnostic::new(
        AuthoringDiagnosticCode::AuthoringSchemaInvalid,
        context.source_path(),
        context.locate(&FieldPath::root().child_key("stages")),
        "stages",
        "The Procedure v1 document declares no stage, so it has no Procedure v2 entry node.",
        OVERFLOW_HINT,
    )
}
