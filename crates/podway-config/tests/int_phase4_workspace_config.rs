use podway_config::{
    CanonicalDigest, CanonicalJson, ConfigError, DEFAULT_WORKSPACE_CONFIG_YAML_V1,
    MAX_WORKSPACE_CONFIG_BYTES_V1, WorkspaceConfigParseLimitsV1, WorkspaceConfigV1,
    parse_workspace_config_v1, parse_workspace_config_v1_with_limits, rewrite_workspace_mode_v1,
};
use podway_core::RuntimeModeV1;
use sha2::{Digest as _, Sha256};

fn limits(max_bytes: usize, max_depth: usize, max_nodes: usize) -> WorkspaceConfigParseLimitsV1 {
    WorkspaceConfigParseLimitsV1 {
        max_bytes,
        max_depth,
        max_nodes,
    }
}

#[test]
fn default_workspace_config_bytes_are_explicit_canonical_and_replayable() {
    assert_eq!(
        DEFAULT_WORKSPACE_CONFIG_YAML_V1,
        b"schema: podway.workspace/v1\nprocedure_paths:\n  - .podway/procedures\ndefault_preset: sw-dev-v2\njob_queue:\n  max_pending: 256\nui:\n  show_stage_in_prompt: false\n",
    );
    let first = parse_workspace_config_v1(DEFAULT_WORKSPACE_CONFIG_YAML_V1).unwrap();
    let second = parse_workspace_config_v1(DEFAULT_WORKSPACE_CONFIG_YAML_V1).unwrap();
    let defaults = WorkspaceConfigV1::default();

    assert_eq!(first, defaults);
    assert_eq!(second, defaults);
    assert_eq!(
        first.canonical_json_v1().unwrap().as_str(),
        concat!(
            r#"{"default_preset":"sw-dev-v2","job_queue":{"max_pending":256},"#,
            r#""procedure_paths":[".podway/procedures"],"#,
            r#""schema":"podway.workspace/v1","ui":{"show_stage_in_prompt":false}}"#,
        ),
    );
    assert_eq!(
        first.canonical_json_v1().unwrap(),
        second.canonical_json_v1().unwrap()
    );

    let canonical = first.canonical_json_v1().unwrap();
    let expected_digest = format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()));
    assert_eq!(
        first.canonical_digest_v1().unwrap().as_str(),
        expected_digest
    );
    assert_eq!(
        first.canonical_digest_v1().unwrap(),
        second.canonical_digest_v1().unwrap()
    );
}

#[test]
fn workspace_config_limits_admit_equality_and_reject_first_overflow() {
    let input = std::str::from_utf8(DEFAULT_WORKSPACE_CONFIG_YAML_V1).unwrap();
    let at_limit = format!("{input} ");
    let first_overflow = format!("{at_limit} ");
    let byte_limit = at_limit.len();

    parse_workspace_config_v1_with_limits(&at_limit, limits(byte_limit, 3, 9)).unwrap();
    assert_eq!(
        parse_workspace_config_v1_with_limits(&first_overflow, limits(byte_limit, 3, 9)),
        Err(ConfigError::InputTooLarge {
            maximum: byte_limit,
            actual: byte_limit + 1,
        })
    );

    let depth_overflow = concat!(
        "schema: podway.workspace/v1\n",
        "unknown:\n",
        "  nested:\n",
        "    value: 0\n",
    );
    assert_eq!(
        parse_workspace_config_v1_with_limits(depth_overflow, limits(1_024, 3, 64)),
        Err(ConfigError::InputTooDeep {
            maximum: 3,
            actual: 4,
        })
    );

    let node_overflow = format!("{input}unknown: true\n");
    assert_eq!(
        parse_workspace_config_v1_with_limits(&node_overflow, limits(1_024, 3, 9)),
        Err(ConfigError::InputTooComplex {
            maximum: 9,
            actual: 10,
        })
    );

    let oversized = vec![b' '; MAX_WORKSPACE_CONFIG_BYTES_V1 + 1];
    assert_eq!(
        parse_workspace_config_v1(oversized),
        Err(ConfigError::InputTooLarge {
            maximum: MAX_WORKSPACE_CONFIG_BYTES_V1,
            actual: MAX_WORKSPACE_CONFIG_BYTES_V1 + 1,
        })
    );
}

#[test]
fn workspace_config_parser_rejects_unsafe_yaml_and_invalid_values() {
    assert_eq!(
        parse_workspace_config_v1("schema: podway.workspace/v1\nschema: podway.workspace/v1\n"),
        Err(ConfigError::DuplicateKey {
            key: "schema".to_owned(),
        })
    );
    assert!(matches!(
        parse_workspace_config_v1("schema: podway.workspace/v1\nunexpected: true\n"),
        Err(ConfigError::InvalidDocument { .. })
    ));
    assert_eq!(
        parse_workspace_config_v1("schema: null\n"),
        Err(ConfigError::InvalidDocument {
            reason: "explicit null is not allowed by workspace config v1".to_owned(),
        })
    );
    assert_eq!(
        parse_workspace_config_v1("schema: &schema podway.workspace/v1\n"),
        Err(ConfigError::UnsupportedYamlFeature { feature: "anchor" })
    );
    assert_eq!(
        parse_workspace_config_v1("schema: *unknown\n"),
        Err(ConfigError::UnsupportedYamlFeature { feature: "alias" })
    );
    assert_eq!(
        parse_workspace_config_v1("schema: !remote podway.workspace/v1\n"),
        Err(ConfigError::UnsupportedYamlFeature { feature: "tag" })
    );
    assert_eq!(
        parse_workspace_config_v1("%YAML 1.2\n---\nschema: podway.workspace/v1\n"),
        Err(ConfigError::UnsupportedYamlFeature { feature: "tag" })
    );
    assert_eq!(
        parse_workspace_config_v1("? schema\n: podway.workspace/v1\n"),
        Err(ConfigError::UnsupportedYamlFeature {
            feature: "explicit mapping key",
        })
    );
    assert_eq!(
        parse_workspace_config_v1("<<: { schema: podway.workspace/v1 }\n"),
        Err(ConfigError::UnsupportedYamlFeature {
            feature: "merge key",
        })
    );
    assert_eq!(
        parse_workspace_config_v1("job_queue:\n  max_pending: 1.0\n"),
        Err(ConfigError::NonCanonicalNumber)
    );
    assert_eq!(
        parse_workspace_config_v1("schema: podway.workspace/v1\nprocedure_paths: [../outside]\n"),
        Err(ConfigError::InvalidValue {
            field: "procedure_paths",
            reason: "must be a normalized relative path",
        })
    );
    assert_eq!(
        parse_workspace_config_v1("schema: podway.workspace/v1\njob_queue:\n  max_pending: 0\n"),
        Err(ConfigError::OutOfBounds {
            field: "job_queue.max_pending",
            min: 1,
            max: 4_096,
            actual: 0,
        })
    );
}

#[test]
fn workspace_config_requires_utf8_and_exactly_one_document() {
    assert_eq!(
        parse_workspace_config_v1([0xff_u8]),
        Err(ConfigError::InvalidDocument {
            reason: "input must be valid UTF-8".to_owned(),
        })
    );
    assert_eq!(
        parse_workspace_config_v1(
            "schema: podway.workspace/v1\n---\nschema: podway.workspace/v1\n"
        ),
        Err(ConfigError::InvalidDocument {
            reason: "workspace config document must contain exactly one YAML document".to_owned(),
        })
    );
}

#[test]
fn v2dvc004_workspace_v2_mode_is_admitted_with_v1_production_compatibility() {
    let legacy = parse_workspace_config_v1("schema: podway.workspace/v1\n").unwrap();
    assert_eq!(legacy.effective_mode(), RuntimeModeV1::production());

    let omitted = parse_workspace_config_v1("schema: podway.workspace/v2\n").unwrap();
    assert_eq!(omitted.effective_mode(), RuntimeModeV1::production());

    let named = parse_workspace_config_v1("schema: podway.workspace/v2\nmode: dev\n").unwrap();
    assert_eq!(named.effective_mode(), RuntimeModeV1::development());

    assert_eq!(
        parse_workspace_config_v1("schema: podway.workspace/v1\nmode: dev\n"),
        Err(ConfigError::InvalidValue {
            field: "mode",
            reason: "requires workspace schema v2",
        })
    );
}

#[test]
fn v2dvc004_mode_rewrite_preserves_unrelated_values_and_comments() {
    let source = concat!(
        "# workspace owner\n",
        "schema: podway.workspace/v1 # schema comment\n",
        "procedure_paths:\n",
        "  - custom/procedures # keep me\n",
        "default_preset: custom-preset\n",
        "job_queue:\n",
        "  max_pending: 17\n",
        "ui:\n",
        "  show_stage_in_prompt: true\n",
    );
    let dev = rewrite_workspace_mode_v1(source, &RuntimeModeV1::development()).unwrap();
    assert_eq!(
        String::from_utf8(dev.clone()).unwrap(),
        source.replace(
            "schema: podway.workspace/v1 # schema comment\n",
            "schema: podway.workspace/v2 # schema comment\nmode: dev\n",
        )
    );
    let prod = rewrite_workspace_mode_v1(&dev, &RuntimeModeV1::production()).unwrap();
    assert_eq!(String::from_utf8(prod).unwrap(), source);

    assert_eq!(
        rewrite_workspace_mode_v1("schema: podway.workspace/v1", &RuntimeModeV1::development())
            .unwrap(),
        b"schema: podway.workspace/v2\nmode: dev"
    );
    assert_eq!(
        rewrite_workspace_mode_v1(
            "schema: podway.workspace/v2\r\nmode: dev # retained\r\n",
            &RuntimeModeV1::new("qa").unwrap(),
        )
        .unwrap(),
        b"schema: podway.workspace/v2\r\nmode: qa # retained\r\n"
    );
}
