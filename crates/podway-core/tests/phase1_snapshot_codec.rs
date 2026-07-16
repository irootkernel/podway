use podway_core::{
    CanonicalProcedureJsonV1, CanonicalProcedureSnapshotInputV1, DomainError, ItemCommonV1, ItemId,
    ItemSpecV1, ItemTypeV1, ProcedureSnapshotAssemblyInputV1, ProcedureSnapshotId,
    ProcedureSnapshotInputV1, ProcedureSnapshotV1, ProcedureSourceKindV1, ProcedureSourceLabelV1,
    ProcedureWarningCodeV1, Sha256Digest, SkipPolicyV1, StageId, StageSpecV1, UnixMillis,
};
use sha2::{Digest as _, Sha256};

const SNAPSHOT_ID: &str = "123e4567-e89b-12d3-a456-426614174000";
const VALID_DOCUMENT: &str = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}],"title":"Stage"}],"version":"1"}"#;
const EXPLICIT_EMPTY_MEDIA_TYPES_DOCUMENT: &str = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"allowed_media_types":[],"id":"artifact","prompt":"Artifact","required":true,"type":"artifact"}],"title":"Stage"}],"version":"1"}"#;
const WHITESPACE_CHOICE_DOCUMENT: &str = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"choices":[" "],"id":"choice","prompt":"Choice","required":true,"type":"choice"}],"title":"Stage"}],"version":"1"}"#;
const INVALID_MEDIA_TYPE_START_DOCUMENT: &str = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"allowed_media_types":[".application/json"],"id":"artifact","prompt":"Artifact","required":true,"type":"artifact"}],"title":"Stage"}],"version":"1"}"#;
const INVALID_MEDIA_SUBTYPE_START_DOCUMENT: &str = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"allowed_media_types":["application/.json"],"id":"artifact","prompt":"Artifact","required":true,"type":"artifact"}],"title":"Stage"}],"version":"1"}"#;

fn assert_rejects_self_consistent(canonical_json: &str) {
    assert!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            canonical_json,
            "codec",
            "1",
            "Codec",
        ))
        .is_err()
    );
}
fn assert_rejects_self_consistent_noncanonical_bytes(canonical_json: &str) {
    assert_eq!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            canonical_json,
            "codec",
            "1",
            "Codec",
        ))
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "canonical procedure JSON is not exact Podway Canonical JSON v1",
        },
    );
}

fn snapshot_input(
    canonical_json: &str,
    procedure_id: &str,
    procedure_version: &str,
    name: &str,
) -> CanonicalProcedureSnapshotInputV1 {
    let canonical_json = CanonicalProcedureJsonV1::new(canonical_json).unwrap();
    let digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_str().as_bytes())
    ))
    .unwrap();
    CanonicalProcedureSnapshotInputV1 {
        snapshot_id: ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        schema_id: "podway.procedure/v1".to_owned(),
        procedure_id: procedure_id.to_owned(),
        procedure_version: procedure_version.to_owned(),
        name: name.to_owned(),
        source_label: ProcedureSourceLabelV1::new("preset:codec").unwrap(),
        canonical_json,
        digest,
        created_at: UnixMillis::new(42),
    }
}

fn decode(canonical_json: &str) -> ProcedureSnapshotV1 {
    let input = snapshot_input(canonical_json, "codec", "1", "Codec");
    let expected_canonical_json = input.canonical_json.clone();
    let snapshot = ProcedureSnapshotV1::from_canonical_json(input).unwrap();
    assert_eq!(snapshot.canonical_json(), &expected_canonical_json);
    snapshot
}

fn item_common(id: &str) -> ItemCommonV1 {
    ItemCommonV1::new(ItemId::new(id).unwrap(), "Prompt", None, true).unwrap()
}

fn assembly_input(
    stages: Vec<StageSpecV1>,
    return_policy: podway_core::ReturnPolicyV1,
    accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
) -> ProcedureSnapshotAssemblyInputV1 {
    ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(SNAPSHOT_ID).unwrap(),
        procedure_id: "codec".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Codec".to_owned(),
        description: None,
        stages,
        return_policy,
        source_label: ProcedureSourceLabelV1::preset("codec").unwrap(),
        accepted_warning_codes,
        created_at: UnixMillis::new(42),
    }
}

fn assemble_with_item(item: ItemSpecV1) -> ProcedureSnapshotV1 {
    let stage = StageSpecV1::new(
        StageId::new("stage").unwrap(),
        "Stage",
        Vec::new(),
        vec![item],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    ProcedureSnapshotV1::assemble(assembly_input(
        vec![stage],
        podway_core::ReturnPolicyV1::any_previous(),
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ))
    .unwrap()
}

fn new_input_matching_snapshot(
    snapshot: &ProcedureSnapshotV1,
    canonical_json: &str,
) -> ProcedureSnapshotInputV1 {
    let input = snapshot_input(canonical_json, "codec", "1", "Codec");
    ProcedureSnapshotInputV1 {
        snapshot_id: snapshot.snapshot_id().clone(),
        procedure_id: snapshot.procedure_id().to_owned(),
        procedure_version: snapshot.procedure_version().to_owned(),
        name: snapshot.name().to_owned(),
        description: snapshot.description().map(ToOwned::to_owned),
        stages: snapshot.stages().to_vec(),
        return_policy: snapshot.return_policy().clone(),
        source_label: snapshot.source_label().clone(),
        canonical_json: input.canonical_json,
        digest: input.digest,
        accepted_warning_codes: snapshot.accepted_warning_codes().to_vec(),
        created_at: snapshot.created_at(),
    }
}

#[test]
fn rehydrates_all_item_kinds_and_only_return_policy() {
    let snapshot = decode(
        r#"{"description":"Full procedure","id":"codec","name":"Codec","rework":{"allow_return_to":["stage"]},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":["Read this"],"items":[{"help":"Confirm help","id":"confirm","prompt":"Confirm","required":true,"type":"confirm"},{"help":"Text help","id":"text","max_length":10,"min_length":2,"multiline":true,"prompt":"Text","required":false,"type":"text"},{"choices":["one","two"],"id":"choice","prompt":"Choice","required":false,"type":"choice"},{"id":"integer","maximum":9,"minimum":-2,"prompt":"Integer","required":false,"type":"integer"},{"id":"list","max_item_length":20,"max_items":4,"min_items":1,"prompt":"List","required":false,"type":"list","unique":true},{"allowed_media_types":["application/json"],"id":"artifact","prompt":"Artifact","required":false,"type":"artifact"}],"skip":{"allowed":true,"reason_required":false},"title":"Stage"}],"version":"1"}"#,
    );

    assert_eq!(snapshot.description(), Some("Full procedure"));
    assert!(!snapshot.stages()[0].skip_policy().reason_required());
    assert_eq!(
        snapshot
            .return_policy()
            .destinations()
            .unwrap()
            .iter()
            .map(|stage| stage.as_str())
            .collect::<Vec<_>>(),
        vec!["stage"]
    );
    assert_eq!(
        snapshot.stages()[0]
            .items()
            .iter()
            .map(ItemSpecV1::item_type)
            .collect::<Vec<_>>(),
        vec![
            ItemTypeV1::Confirm,
            ItemTypeV1::Text,
            ItemTypeV1::Choice,
            ItemTypeV1::Integer,
            ItemTypeV1::List,
            ItemTypeV1::Artifact,
        ]
    );

    match &snapshot.stages()[0].items()[1] {
        ItemSpecV1::Text(spec) => {
            assert_eq!(spec.common().help(), Some("Text help"));
            assert_eq!(spec.min_length(), 2);
            assert_eq!(spec.max_length(), 10);
            assert!(spec.multiline());
        }
        _ => panic!("expected text item"),
    }
    match &snapshot.stages()[0].items()[2] {
        ItemSpecV1::Choice(spec) => {
            assert_eq!(spec.choices(), &["one".to_owned(), "two".to_owned()]);
        }
        _ => panic!("expected choice item"),
    }
    match &snapshot.stages()[0].items()[3] {
        ItemSpecV1::Integer(spec) => {
            assert_eq!(spec.minimum(), Some(-2));
            assert_eq!(spec.maximum(), Some(9));
        }
        _ => panic!("expected integer item"),
    }
    match &snapshot.stages()[0].items()[4] {
        ItemSpecV1::List(spec) => {
            assert_eq!(spec.min_items(), 1);
            assert_eq!(spec.max_items(), 4);
            assert_eq!(spec.max_item_length(), 20);
            assert!(spec.unique());
        }
        _ => panic!("expected list item"),
    }
    match &snapshot.stages()[0].items()[5] {
        ItemSpecV1::Artifact(spec) => {
            assert_eq!(spec.allowed_media_types(), &["application/json".to_owned()]);
        }
        _ => panic!("expected artifact item"),
    }
    assert_eq!(
        snapshot.accepted_warning_codes(),
        &[ProcedureWarningCodeV1::FinalStageSkippable]
    );
}

#[test]
fn rehydrates_any_previous_return_policy_and_recomputes_warnings() {
    let snapshot = decode(
        r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}],"title":"Stage"}],"version":"1"}"#,
    );

    assert!(snapshot.return_policy().destinations().is_none());
    assert_eq!(
        snapshot.accepted_warning_codes(),
        &[ProcedureWarningCodeV1::AnyPreviousReturnPolicy]
    );
}

#[test]
fn rejects_missing_defaulted_fields_and_unknown_fields() {
    let missing_defaulted_field = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"text","max_length":10,"min_length":0,"prompt":"Text","required":true,"type":"text"}],"title":"Stage"}],"version":"1"}"#;
    assert!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            missing_defaulted_field,
            "codec",
            "1",
            "Codec",
        ))
        .is_err()
    );

    let unknown_field = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm","unexpected":true}],"title":"Stage"}],"version":"1"}"#;
    assert!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            unknown_field,
            "codec",
            "1",
            "Codec",
        ))
        .is_err()
    );
}

#[test]
fn rejects_row_schema_and_metadata_mismatches() {
    let canonical_json = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}],"title":"Stage"}],"version":"1"}"#;

    let mut row_mismatch = snapshot_input(canonical_json, "codec", "1", "Other name");
    assert!(ProcedureSnapshotV1::from_canonical_json(row_mismatch.clone()).is_err());

    row_mismatch.name = "Codec".to_owned();
    row_mismatch.schema_id = "podway.procedure/v2".to_owned();
    assert!(ProcedureSnapshotV1::from_canonical_json(row_mismatch).is_err());
}

#[test]
fn rejects_invalid_constraints_and_digest_mismatches() {
    let invalid_constraints = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"text","max_length":1,"min_length":2,"multiline":false,"prompt":"Text","required":true,"type":"text"}],"title":"Stage"}],"version":"1"}"#;
    assert!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            invalid_constraints,
            "codec",
            "1",
            "Codec",
        ))
        .is_err()
    );

    let valid_document = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}],"title":"Stage"}],"version":"1"}"#;
    let mut digest_mismatch = snapshot_input(valid_document, "codec", "1", "Codec");
    digest_mismatch.digest = Sha256Digest::new(format!("sha256:{}", "0".repeat(64))).unwrap();
    assert!(ProcedureSnapshotV1::from_canonical_json(digest_mismatch).is_err());
}
#[test]
fn rejects_self_consistent_noncanonical_bytes() {
    let unsorted = VALID_DOCUMENT.replacen(
        r#"{"id":"codec","name":"Codec""#,
        r#"{"name":"Codec","id":"codec""#,
        1,
    );
    let duplicate_key =
        VALID_DOCUMENT.replacen(r#""id":"codec","#, r#""id":"codec","id":"codec","#, 1);
    let nested_unsorted = VALID_DOCUMENT.replacen(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"type":"confirm","id":"confirm","prompt":"Confirm","required":true}"#,
        1,
    );
    let nested_duplicate = VALID_DOCUMENT.replacen(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"id":"confirm","id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        1,
    );
    let internal_whitespace =
        VALID_DOCUMENT.replacen(r#""prompt":"Confirm""#, r#""prompt" : "Confirm""#, 1);
    let alternate_unicode_escape =
        VALID_DOCUMENT.replacen(r#""name":"Codec""#, r#""name":"Co\u0064ec""#, 1);
    let alternate_solidus_escape = VALID_DOCUMENT.replacen(
        r#""schema":"podway.procedure/v1""#,
        r#""schema":"podway.procedure\/v1""#,
        1,
    );
    let negative_zero = VALID_DOCUMENT.replace(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"id":"integer","minimum":-0,"prompt":"Integer","required":true,"type":"integer"}"#,
    );
    let non_integer_number = VALID_DOCUMENT.replace(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"id":"integer","minimum":1.0,"prompt":"Integer","required":true,"type":"integer"}"#,
    );

    for document in [
        format!(" {VALID_DOCUMENT}"),
        format!("{VALID_DOCUMENT} "),
        unsorted,
        duplicate_key,
        nested_unsorted,
        nested_duplicate,
        internal_whitespace,
        alternate_unicode_escape,
        alternate_solidus_escape,
        negative_zero,
        non_integer_number,
        "{".to_owned(),
    ] {
        assert_rejects_self_consistent_noncanonical_bytes(&document);
    }
}
#[test]
fn rejects_noncanonical_semantic_persisted_documents() {
    for document in [
        EXPLICIT_EMPTY_MEDIA_TYPES_DOCUMENT,
        WHITESPACE_CHOICE_DOCUMENT,
        INVALID_MEDIA_TYPE_START_DOCUMENT,
        INVALID_MEDIA_SUBTYPE_START_DOCUMENT,
    ] {
        assert_rejects_self_consistent(document);
    }
}
#[test]
fn rejects_self_consistent_noncanonical_skip_policies() {
    let missing_reason_required = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"skip":{"allowed":true},"title":"Stage"}],"version":"1"}"#;
    assert_eq!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            missing_reason_required,
            "codec",
            "1",
            "Codec",
        ))
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "invalid canonical procedure JSON",
        }
    );

    let redundant_disallowed_skip = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"skip":{"allowed":false,"reason_required":false},"title":"Stage"}],"version":"1"}"#;
    assert_eq!(
        ProcedureSnapshotV1::from_canonical_json(snapshot_input(
            redundant_disallowed_skip,
            "codec",
            "1",
            "Codec",
        ))
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "canonical procedure JSON must omit disallowed skip policies",
        }
    );
}

#[test]
fn structured_and_verified_construction_share_canonical_parity_rules() {
    assert!(ItemSpecV1::choice(item_common("choice"), vec![" ".to_owned()]).is_err());
    for media_type in [".application/json", "application/.json"] {
        assert!(
            ItemSpecV1::artifact(item_common("artifact"), vec![media_type.to_owned()]).is_err()
        );
    }

    let restricted = assemble_with_item(
        ItemSpecV1::artifact(item_common("artifact"), vec!["1/2".to_owned()]).unwrap(),
    );
    assert!(
        restricted
            .canonical_json()
            .as_str()
            .contains(r#""allowed_media_types":["1/2"]"#)
    );
    assert_eq!(
        ProcedureSnapshotV1::new(new_input_matching_snapshot(
            &restricted,
            restricted.canonical_json().as_str(),
        ))
        .unwrap(),
        restricted
    );

    let unrestricted =
        assemble_with_item(ItemSpecV1::artifact(item_common("artifact"), Vec::new()).unwrap());
    assert!(
        !unrestricted
            .canonical_json()
            .as_str()
            .contains(r#""allowed_media_types""#)
    );
    assert_eq!(
        ProcedureSnapshotV1::new(new_input_matching_snapshot(
            &unrestricted,
            unrestricted.canonical_json().as_str(),
        ))
        .unwrap(),
        unrestricted
    );

    for (document, reason) in [
        (
            EXPLICIT_EMPTY_MEDIA_TYPES_DOCUMENT,
            "canonical procedure JSON must omit empty allowed media types",
        ),
        (WHITESPACE_CHOICE_DOCUMENT, "choice"),
        (
            INVALID_MEDIA_TYPE_START_DOCUMENT,
            "media type must be lowercase ASCII without parameters",
        ),
        (
            INVALID_MEDIA_SUBTYPE_START_DOCUMENT,
            "media type must be lowercase ASCII without parameters",
        ),
    ] {
        assert_eq!(
            ProcedureSnapshotV1::from_canonical_json(snapshot_input(
                document, "codec", "1", "Codec"
            ))
            .unwrap_err(),
            DomainError::InvalidState { reason }
        );
    }
}

#[test]
fn rejects_generic_structured_canonical_mismatch() {
    let snapshot = decode(VALID_DOCUMENT);
    let different_but_valid_document =
        VALID_DOCUMENT.replace(r#""prompt":"Confirm""#, r#""prompt":"Confirm differently""#);

    assert_eq!(
        ProcedureSnapshotV1::new(new_input_matching_snapshot(
            &snapshot,
            &different_but_valid_document,
        ))
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "caller-supplied snapshot fields do not match canonical procedure JSON",
        }
    );
}

#[test]
fn rejects_explicit_null_at_every_optional_procedure_path() {
    let description_null =
        VALID_DOCUMENT.replacen(r#"{"id":"codec""#, r#"{"description":null,"id":"codec""#, 1);
    let help_null =
        VALID_DOCUMENT.replacen(r#"{"id":"confirm""#, r#"{"help":null,"id":"confirm""#, 1);
    let minimum_null = VALID_DOCUMENT.replace(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"id":"integer","minimum":null,"prompt":"Integer","required":true,"type":"integer"}"#,
    );
    let maximum_null = VALID_DOCUMENT.replace(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"id":"integer","maximum":null,"prompt":"Integer","required":true,"type":"integer"}"#,
    );
    let media_types_null = VALID_DOCUMENT.replace(
        r#"{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}"#,
        r#"{"allowed_media_types":null,"id":"artifact","prompt":"Artifact","required":true,"type":"artifact"}"#,
    );
    let skip_null = VALID_DOCUMENT.replacen(
        r#","title":"Stage"}],"version":"1"}"#,
        r#","skip":null,"title":"Stage"}],"version":"1"}"#,
        1,
    );

    for document in [
        description_null,
        help_null,
        minimum_null,
        maximum_null,
        media_types_null,
        skip_null,
    ] {
        assert_rejects_self_consistent(&document);
    }
}

#[test]
fn rejects_missing_default_expanded_fields() {
    let text_without_minimum = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"text","max_length":10,"multiline":false,"prompt":"Text","required":true,"type":"text"}],"title":"Stage"}],"version":"1"}"#;
    let text_without_maximum = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"text","min_length":0,"multiline":false,"prompt":"Text","required":true,"type":"text"}],"title":"Stage"}],"version":"1"}"#;
    let text_without_multiline = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"text","max_length":10,"min_length":0,"prompt":"Text","required":true,"type":"text"}],"title":"Stage"}],"version":"1"}"#;
    let list_document = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"list","max_item_length":1,"max_items":1,"min_items":0,"prompt":"List","required":true,"type":"list","unique":true}],"title":"Stage"}],"version":"1"}"#;
    let list_without_maximum_item_length = list_document.replacen(r#""max_item_length":1,"#, "", 1);
    let list_without_maximum_items = list_document.replacen(r#""max_items":1,"#, "", 1);
    let list_without_minimum_items = list_document.replacen(r#""min_items":0,"#, "", 1);
    let list_without_unique = list_document.replacen(r#","unique":true"#, "", 1);
    let stage_without_instructions = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","items":[{"id":"confirm","prompt":"Confirm","required":true,"type":"confirm"}],"title":"Stage"}],"version":"1"}"#;
    let stage_without_items = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"title":"Stage"}],"version":"1"}"#;
    for document in [
        text_without_minimum,
        text_without_maximum,
        text_without_multiline,
        list_without_maximum_item_length.as_str(),
        list_without_maximum_items.as_str(),
        list_without_minimum_items.as_str(),
        list_without_unique.as_str(),
        stage_without_instructions,
        stage_without_items,
    ] {
        assert_rejects_self_consistent(document);
    }
}

#[test]
fn rejects_duplicate_identifiers_values_and_return_destinations() {
    let duplicate_stage_ids = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"title":"First"},{"id":"stage","instructions":[],"items":[],"title":"Second"}],"version":"1"}"#;
    let duplicate_item_ids = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"item","prompt":"One","required":true,"type":"confirm"},{"id":"item","prompt":"Two","required":true,"type":"confirm"}],"title":"Stage"}],"version":"1"}"#;
    let duplicate_choices = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"choices":["one","one"],"id":"choice","prompt":"Choice","required":true,"type":"choice"}],"title":"Stage"}],"version":"1"}"#;
    let duplicate_media_types = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"allowed_media_types":["application/json","application/json"],"id":"artifact","prompt":"Artifact","required":true,"type":"artifact"}],"title":"Stage"}],"version":"1"}"#;
    let duplicate_return_targets = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":["stage","stage"]},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"title":"Stage"}],"version":"1"}"#;

    for document in [
        duplicate_stage_ids,
        duplicate_item_ids,
        duplicate_choices,
        duplicate_media_types,
        duplicate_return_targets,
    ] {
        assert_rejects_self_consistent(document);
    }
}

#[test]
fn rejects_hard_limit_and_invalid_constraint_representatives() {
    let text_over_hard_limit = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"text","max_length":65537,"min_length":0,"multiline":false,"prompt":"Text","required":true,"type":"text"}],"title":"Stage"}],"version":"1"}"#;
    let list_over_hard_limit = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"list","max_item_length":1,"max_items":1001,"min_items":0,"prompt":"List","required":true,"type":"list","unique":true}],"title":"Stage"}],"version":"1"}"#;
    let zero_capacity_list =
        list_over_hard_limit.replacen(r#""max_items":1001"#, r#""max_items":0"#, 1);
    let unsatisfiable_integer = r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[{"id":"integer","maximum":1,"minimum":2,"prompt":"Integer","required":true,"type":"integer"}],"title":"Stage"}],"version":"1"}"#;

    for document in [
        text_over_hard_limit,
        list_over_hard_limit,
        zero_capacity_list.as_str(),
        unsatisfiable_integer,
    ] {
        assert_rejects_self_consistent(document);
    }
}

#[test]
fn rejects_every_row_metadata_mismatch() {
    let mut procedure_id = snapshot_input(VALID_DOCUMENT, "other", "1", "Codec");
    assert!(ProcedureSnapshotV1::from_canonical_json(procedure_id.clone()).is_err());

    procedure_id.procedure_id = "codec".to_owned();
    procedure_id.procedure_version = "2".to_owned();
    assert!(ProcedureSnapshotV1::from_canonical_json(procedure_id.clone()).is_err());

    procedure_id.procedure_version = "1".to_owned();
    procedure_id.name = "Other".to_owned();
    assert!(ProcedureSnapshotV1::from_canonical_json(procedure_id.clone()).is_err());

    procedure_id.name = "Codec".to_owned();
    procedure_id.schema_id = "podway.procedure/v2".to_owned();
    assert!(ProcedureSnapshotV1::from_canonical_json(procedure_id).is_err());
}

#[test]
fn source_labels_canonicalize_and_round_trip_through_authoritative_rows() {
    assert_eq!(
        ProcedureSourceLabelV1::new("fixture").unwrap(),
        ProcedureSourceLabelV1::file("fixture").unwrap()
    );

    for (source, kind, raw_label, display_label) in [
        (
            ProcedureSourceLabelV1::new("fixture").unwrap(),
            ProcedureSourceKindV1::File,
            "fixture",
            "procedure:fixture",
        ),
        (
            ProcedureSourceLabelV1::new("preset:codec").unwrap(),
            ProcedureSourceKindV1::Preset,
            "codec",
            "preset:codec",
        ),
        (
            ProcedureSourceLabelV1::new("procedure:workflows/codec.yaml").unwrap(),
            ProcedureSourceKindV1::File,
            "workflows/codec.yaml",
            "procedure:workflows/codec.yaml",
        ),
        (
            ProcedureSourceLabelV1::preset("direct-preset").unwrap(),
            ProcedureSourceKindV1::Preset,
            "direct-preset",
            "preset:direct-preset",
        ),
        (
            ProcedureSourceLabelV1::file("direct-file").unwrap(),
            ProcedureSourceKindV1::File,
            "direct-file",
            "procedure:direct-file",
        ),
        (
            ProcedureSourceLabelV1::from_row(ProcedureSourceKindV1::Preset, "row-preset").unwrap(),
            ProcedureSourceKindV1::Preset,
            "row-preset",
            "preset:row-preset",
        ),
        (
            ProcedureSourceLabelV1::from_row(ProcedureSourceKindV1::File, "row-file").unwrap(),
            ProcedureSourceKindV1::File,
            "row-file",
            "procedure:row-file",
        ),
    ] {
        assert_eq!(source.kind(), kind);
        assert_eq!(source.label(), raw_label);
        assert_eq!(source.display_label(), display_label);
        assert_eq!(
            ProcedureSourceLabelV1::from_row(kind, raw_label).unwrap(),
            source
        );
    }

    for raw_label in ["preset:codec", "procedure:codec"] {
        let expected = DomainError::InvalidState {
            reason: "procedure source raw label must not contain a display prefix",
        };
        assert_eq!(
            ProcedureSourceLabelV1::preset(raw_label).unwrap_err(),
            expected
        );
        assert_eq!(
            ProcedureSourceLabelV1::file(raw_label).unwrap_err(),
            expected
        );
        assert_eq!(
            ProcedureSourceLabelV1::from_row(ProcedureSourceKindV1::Preset, raw_label).unwrap_err(),
            expected
        );
        assert_eq!(
            ProcedureSourceLabelV1::from_row(ProcedureSourceKindV1::File, raw_label).unwrap_err(),
            expected
        );
    }
    for legacy_display in [
        "preset:preset:codec",
        "preset:procedure:codec",
        "procedure:preset:codec",
        "procedure:procedure:codec",
    ] {
        assert_eq!(
            ProcedureSourceLabelV1::new(legacy_display).unwrap_err(),
            DomainError::InvalidState {
                reason: "procedure source raw label must not contain a display prefix",
            }
        );
    }

    for (kind, raw_label, expected_display) in [
        (
            ProcedureSourceKindV1::Preset,
            "presets:codec",
            "preset:presets:codec",
        ),
        (
            ProcedureSourceKindV1::File,
            "procedures:codec",
            "procedure:procedures:codec",
        ),
    ] {
        assert_eq!(
            ProcedureSourceLabelV1::from_row(kind, raw_label)
                .unwrap()
                .display_label(),
            expected_display
        );
    }
    assert_eq!(
        ProcedureSourceKindV1::from_row_value("preset").unwrap(),
        ProcedureSourceKindV1::Preset
    );
    assert!(ProcedureSourceKindV1::from_row_value("remote").is_err());
}

#[test]
fn verified_and_structured_snapshot_construction_cannot_bypass_canonical_state() {
    let snapshot = decode(VALID_DOCUMENT);
    let caller_supplied = ProcedureSnapshotInputV1 {
        snapshot_id: snapshot.snapshot_id().clone(),
        procedure_id: snapshot.procedure_id().to_owned(),
        procedure_version: snapshot.procedure_version().to_owned(),
        name: snapshot.name().to_owned(),
        description: snapshot.description().map(ToOwned::to_owned),
        stages: snapshot.stages().to_vec(),
        return_policy: snapshot.return_policy().clone(),
        source_label: snapshot.source_label().clone(),
        canonical_json: snapshot.canonical_json().clone(),
        digest: snapshot.digest().clone(),
        accepted_warning_codes: Vec::new(),
        created_at: snapshot.created_at(),
    };
    assert!(ProcedureSnapshotV1::new(caller_supplied).is_err());

    let assembled = ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: snapshot.snapshot_id().clone(),
        procedure_id: snapshot.procedure_id().to_owned(),
        procedure_version: snapshot.procedure_version().to_owned(),
        name: snapshot.name().to_owned(),
        description: snapshot.description().map(ToOwned::to_owned),
        stages: snapshot.stages().to_vec(),
        return_policy: snapshot.return_policy().clone(),
        source_label: snapshot.source_label().clone(),
        accepted_warning_codes: snapshot.accepted_warning_codes().to_vec(),
        created_at: snapshot.created_at(),
    })
    .unwrap();
    assert_eq!(assembled.canonical_json(), snapshot.canonical_json());
    assert_eq!(assembled.digest(), snapshot.digest());
    assert_eq!(
        assembled.accepted_warning_codes(),
        snapshot.accepted_warning_codes()
    );
}
#[test]
fn structured_snapshot_assembly_requires_exact_warning_acceptance() {
    let warning_free_stage_id = StageId::new("warning-free").unwrap();
    let warning_free_stage = StageSpecV1::new(
        warning_free_stage_id.clone(),
        "Warning-free",
        Vec::new(),
        vec![ItemSpecV1::confirm(item_common("confirm"))],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let warning_free = ProcedureSnapshotV1::assemble(assembly_input(
        vec![warning_free_stage],
        podway_core::ReturnPolicyV1::only(vec![warning_free_stage_id]).unwrap(),
        Vec::new(),
    ))
    .unwrap();
    assert!(warning_free.accepted_warning_codes().is_empty());

    let warning_stage = StageSpecV1::new(
        StageId::new("warnings").unwrap(),
        "Warnings",
        Vec::new(),
        vec![ItemSpecV1::confirm(item_common("confirm"))],
        SkipPolicyV1::new(true, false).unwrap(),
    )
    .unwrap();
    let expected = vec![
        ProcedureWarningCodeV1::FinalStageSkippable,
        ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
    ];
    let admitted = ProcedureSnapshotV1::assemble(assembly_input(
        vec![warning_stage.clone()],
        podway_core::ReturnPolicyV1::any_previous(),
        expected.clone(),
    ))
    .unwrap();
    assert_eq!(admitted.accepted_warning_codes(), expected.as_slice());

    for proof in [
        Vec::new(),
        vec![
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ProcedureWarningCodeV1::StageNearHardLimits,
        ],
        vec![
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
        vec![
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ProcedureWarningCodeV1::FinalStageSkippable,
        ],
    ] {
        assert!(
            ProcedureSnapshotV1::assemble(assembly_input(
                vec![warning_stage.clone()],
                podway_core::ReturnPolicyV1::any_previous(),
                proof,
            ))
            .is_err()
        );
    }
}

#[test]
fn warning_codes_have_a_stable_v1_order() {
    let stages = (0..58)
        .map(|index| {
            if index == 0 {
                r#"{"id":"stage-0","instructions":[],"items":[{"id":"first","prompt":"Must provide result","required":false,"type":"confirm"},{"id":"second","prompt":"Must provide result","required":false,"type":"confirm"}],"title":"Stage 0"}"#.to_owned()
            } else if index == 57 {
                r#"{"id":"stage-57","instructions":[],"items":[],"skip":{"allowed":true,"reason_required":false},"title":"Stage 57"}"#.to_owned()
            } else {
                format!(
                    r#"{{"id":"stage-{index}","instructions":[],"items":[],"title":"Stage {index}"}}"#
                )
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let document = format!(
        r#"{{"id":"codec","name":"Codec","rework":{{"allow_return_to":"any_previous"}},"schema":"podway.procedure/v1","stages":[{stages}],"version":"1"}}"#
    );
    let snapshot = decode(&document);

    assert_eq!(
        snapshot.accepted_warning_codes(),
        &[
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::StageNearHardLimits,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ProcedureWarningCodeV1::RepeatedPrompt,
            ProcedureWarningCodeV1::OptionalItemAppearsRequired,
        ]
    );
}
