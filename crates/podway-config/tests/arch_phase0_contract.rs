use podway_config::{
    CanonicalDigest, CanonicalJson, ConfigError, ItemDefinitionV1, ProcedureDefinitionV1,
    ReworkPolicyV1, SkipPolicyV1, StageDefinitionV1, WORKSPACE_SCHEMA_V1, WorkspaceConfigV1,
};

fn empty_stage(id: &str) -> StageDefinitionV1 {
    StageDefinitionV1 {
        id: id.to_owned(),
        title: "Release preparation".to_owned(),
        instructions: Vec::new(),
        items: Vec::new(),
        skip: None,
    }
}

fn procedure_with_skip_reason(reason_required: Option<bool>) -> ProcedureDefinitionV1 {
    ProcedureDefinitionV1::new(
        "release",
        "1.0.0",
        "Release",
        vec![StageDefinitionV1 {
            id: "prepare".to_owned(),
            title: "Prepare release".to_owned(),
            instructions: Vec::new(),
            items: Vec::new(),
            skip: Some(SkipPolicyV1 {
                allowed: true,
                reason_required,
            }),
        }],
        ReworkPolicyV1::any_previous(),
    )
    .unwrap()
}

// DOM-006: Equivalent authored skip-policy data has one canonical byte and digest representation.
#[test]
fn dom_006_equivalent_authored_procedures_canonicalize_to_identical_bytes_and_digest() {
    let implicit_reason_requirement = procedure_with_skip_reason(None);
    let explicit_reason_requirement = procedure_with_skip_reason(Some(true));

    let implicit_json = implicit_reason_requirement.canonical_json_v1().unwrap();
    let explicit_json = explicit_reason_requirement.canonical_json_v1().unwrap();
    assert_eq!(implicit_json.as_bytes(), explicit_json.as_bytes());
    assert_eq!(
        implicit_reason_requirement.canonical_digest_v1().unwrap(),
        explicit_reason_requirement.canonical_digest_v1().unwrap()
    );
}

// DOM-001: A procedure's ordered stage list cannot contain duplicate stage identifiers.
#[test]
fn dom_001_duplicate_stage_ids_are_rejected() {
    let error = ProcedureDefinitionV1::new(
        "release",
        "1.0.0",
        "Release",
        vec![empty_stage("prepare"), empty_stage("prepare")],
        ReworkPolicyV1::any_previous(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        ConfigError::DuplicateValue {
            field: "stage.id",
            value: "prepare".to_owned(),
        }
    );
}

// DOM-004: Stage item keys are unique within a stage.
#[test]
fn dom_004_duplicate_item_ids_are_rejected() {
    let duplicate_item = || ItemDefinitionV1::Confirm {
        id: "approval".to_owned(),
        prompt: "Approved?".to_owned(),
        help: None,
        required: true,
    };
    let stage = StageDefinitionV1 {
        id: "prepare".to_owned(),
        title: "Prepare release".to_owned(),
        instructions: Vec::new(),
        items: vec![duplicate_item(), duplicate_item()],
        skip: None,
    };

    assert_eq!(
        stage.validate(),
        Err(ConfigError::DuplicateValue {
            field: "item.id",
            value: "approval".to_owned(),
        })
    );
}

// DOM-004: Item length, count, and numeric bounds reject inverted limits.
#[test]
fn dom_004_invalid_item_bounds_are_rejected() {
    let text = ItemDefinitionV1::Text {
        id: "summary".to_owned(),
        prompt: "Summary".to_owned(),
        help: None,
        required: true,
        min_length: 2,
        max_length: 1,
        multiline: true,
    };
    assert_eq!(
        text.validate(),
        Err(ConfigError::InvalidValue {
            field: "item.text.length",
            reason: "minimum cannot exceed maximum",
        })
    );

    let list = ItemDefinitionV1::List {
        id: "reviewers".to_owned(),
        prompt: "Reviewers".to_owned(),
        help: None,
        required: true,
        min_items: 2,
        max_items: 1,
        max_item_length: 100,
        unique: true,
    };
    assert_eq!(
        list.validate(),
        Err(ConfigError::InvalidValue {
            field: "item.list.item_count",
            reason: "minimum cannot exceed maximum",
        })
    );

    let integer = ItemDefinitionV1::Integer {
        id: "priority".to_owned(),
        prompt: "Priority".to_owned(),
        help: None,
        required: true,
        minimum: Some(2),
        maximum: Some(1),
    };
    assert_eq!(
        integer.validate(),
        Err(ConfigError::InvalidValue {
            field: "item.integer.bounds",
            reason: "minimum cannot exceed maximum",
        })
    );
}

// DOM-001: Workspace configuration defaults and deserialization produce one validated contract.
#[test]
fn dom_001_workspace_config_defaults_are_stable_and_valid() {
    let defaults = WorkspaceConfigV1::default();
    assert_eq!(defaults.schema, WORKSPACE_SCHEMA_V1);
    assert_eq!(defaults.procedure_paths, vec![".podway/procedures"]);
    assert_eq!(defaults.default_preset, "sw-dev");
    assert_eq!(defaults.job_queue.max_pending, 256);
    assert!(!defaults.ui.show_stage_in_prompt);
    assert_eq!(defaults.validate(), Ok(()));

    let deserialized: WorkspaceConfigV1 =
        serde_json::from_str(r#"{"schema":"podway.workspace/v1"}"#).unwrap();
    assert_eq!(deserialized, defaults);
}
#[test]
fn g002_text_bounds_include_leading_and_trailing_authored_whitespace() {
    let stage = StageDefinitionV1 {
        id: "prepare".to_owned(),
        title: format!(" {} ", "x".repeat(120)),
        instructions: Vec::new(),
        items: Vec::new(),
        skip: None,
    };

    assert_eq!(
        stage.validate(),
        Err(ConfigError::OutOfBounds {
            field: "stage.title",
            min: 1,
            max: 120,
            actual: 122,
        })
    );
}

#[test]
fn g002_exact_authored_scalar_boundaries_are_accepted() {
    let definition = ProcedureDefinitionV1 {
        schema: "podway.procedure/v1".to_owned(),
        id: "r".repeat(64),
        version: "v".repeat(64),
        name: "n".repeat(120),
        description: Some("d".repeat(4_000)),
        stages: vec![StageDefinitionV1 {
            id: "s".repeat(64),
            title: "é".repeat(120),
            instructions: vec!["i".repeat(2_000)],
            items: vec![
                ItemDefinitionV1::Confirm {
                    id: "c".repeat(64),
                    prompt: "p".repeat(500),
                    help: Some("h".repeat(4_000)),
                    required: true,
                },
                ItemDefinitionV1::Choice {
                    id: "o".repeat(64),
                    prompt: "Choose".to_owned(),
                    help: None,
                    required: true,
                    choices: vec!["o".repeat(120)],
                },
            ],
            skip: None,
        }],
        rework: ReworkPolicyV1::any_previous(),
    };

    assert_eq!(definition.validate(), Ok(()));
    assert_eq!(
        WorkspaceConfigV1::new(
            vec!["p".repeat(1_024)],
            "sw-dev",
            Default::default(),
            Default::default(),
        ),
        Ok(WorkspaceConfigV1 {
            schema: WORKSPACE_SCHEMA_V1.to_owned(),
            procedure_paths: vec!["p".repeat(1_024)],
            default_preset: "sw-dev".to_owned(),
            job_queue: Default::default(),
            ui: Default::default(),
        })
    );
}

#[test]
fn g002_whitespace_only_authored_text_is_empty() {
    let stage = StageDefinitionV1 {
        id: "prepare".to_owned(),
        title: " \t\n".to_owned(),
        instructions: Vec::new(),
        items: Vec::new(),
        skip: None,
    };

    assert_eq!(
        stage.validate(),
        Err(ConfigError::OutOfBounds {
            field: "stage.title",
            min: 1,
            max: 120,
            actual: 0,
        })
    );
}

#[test]
fn g002_canonical_json_and_digest_preserve_authored_whitespace() {
    let authored = ProcedureDefinitionV1::new(
        "release",
        "1.0.0",
        " Release ",
        vec![StageDefinitionV1 {
            id: "prepare".to_owned(),
            title: " Prepare release ".to_owned(),
            instructions: Vec::new(),
            items: Vec::new(),
            skip: None,
        }],
        ReworkPolicyV1::any_previous(),
    )
    .unwrap();
    let unpadded = ProcedureDefinitionV1::new(
        "release",
        "1.0.0",
        "Release",
        vec![empty_stage("prepare")],
        ReworkPolicyV1::any_previous(),
    )
    .unwrap();

    let authored_json = authored.canonical_json_v1().unwrap();
    let authored_digest = authored.canonical_digest_v1().unwrap();

    assert_eq!(
        authored_json.as_str(),
        r#"{"id":"release","name":" Release ","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"prepare","instructions":[],"items":[],"title":" Prepare release "}],"version":"1.0.0"}"#
    );
    assert_eq!(authored_json, authored.canonical_json_v1().unwrap());
    assert_eq!(authored_digest, authored.canonical_digest_v1().unwrap());
    assert_ne!(authored_json, unpadded.canonical_json_v1().unwrap());
    assert_ne!(authored_digest, unpadded.canonical_digest_v1().unwrap());
}
