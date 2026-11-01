use podway_config::{
    CanonicalJson, ConfigError, IntegrationSettingsSourceV1, WorkspaceConfigParseLimitsV1,
    parse_user_config_v1, parse_user_config_v1_with_limits, parse_workspace_config_v1,
    resolve_integration_settings_v1,
};

#[test]
fn strict_settings_documents_preserve_inheritance_and_boolean_value_precedence() {
    for project in [None, Some(false), Some(true)] {
        for user in [None, Some(false), Some(true)] {
            let resolved = resolve_integration_settings_v1(project, user);
            assert_eq!(resolved.enabled, project.or(user).unwrap_or(false));
            assert_eq!(
                resolved.source,
                if project.is_some() {
                    IntegrationSettingsSourceV1::Project
                } else if user.is_some() {
                    IntegrationSettingsSourceV1::User
                } else {
                    IntegrationSettingsSourceV1::Default
                }
            );
        }
    }
    for nested in ["", "integrations: {}\n", "integrations:\n  dolgorae: {}\n"] {
        let user =
            parse_user_config_v1(format!("schema: podway.user-config/v1\n{nested}")).unwrap();
        assert_eq!(
            user.integrations
                .as_ref()
                .and_then(|value| value.dolgorae_enabled()),
            None
        );
        let project =
            parse_workspace_config_v1(format!("schema: podway.workspace/v3\n{nested}")).unwrap();
        assert_eq!(
            project
                .integrations
                .as_ref()
                .and_then(|value| value.dolgorae_enabled()),
            None
        );
    }
    for boolean in [false, true] {
        let body = format!("integrations:\n  dolgorae:\n    enabled: {boolean}\n");
        let user = parse_user_config_v1(format!("schema: podway.user-config/v1\n{body}")).unwrap();
        assert_eq!(user.integrations.unwrap().dolgorae_enabled(), Some(boolean));
        let project =
            parse_workspace_config_v1(format!("schema: podway.workspace/v3\n{body}")).unwrap();
        assert_eq!(
            project.integrations.as_ref().unwrap().dolgorae_enabled(),
            Some(boolean)
        );
        assert!(
            project
                .canonical_json_v1()
                .unwrap()
                .as_str()
                .contains(&format!("\"enabled\":{boolean}"))
        );
    }
}

#[test]
fn settings_reject_null_types_unknown_fields_duplicates_and_unsafe_yaml() {
    for schema in ["podway.workspace/v3", "podway.user-config/v1"] {
        for body in [
            "integrations: null",
            "integrations: false",
            "integrations: []",
            "integrations: {other: {}}",
            "integrations: {dolgorae: null}",
            "integrations: {dolgorae: {unknown: true}}",
            "unknown: true",
            "integrations: {dolgorae: {enabled: null}}",
            "integrations: {dolgorae: {enabled: 'true'}}",
            "integrations: {dolgorae: {enabled: 1}}",
            "integrations: {dolgorae: {enabled: true, enabled: false}}",
            "integrations: {dolgorae: {}, dolgorae: {}}",
            "integrations: &x {dolgorae: {enabled: true}}",
            "integrations: *x",
            "integrations: !custom {}",
            "integrations: {<<: {}}",
        ] {
            let text = format!("schema: {schema}\n{body}\n");
            let invalid = if schema == "podway.workspace/v3" {
                parse_workspace_config_v1(text).is_err()
            } else {
                parse_user_config_v1(text).is_err()
            };
            assert!(invalid, "accepted {schema}: {body}");
        }
    }
    for schema in ["podway.workspace/v1", "podway.workspace/v2"] {
        assert!(
            parse_workspace_config_v1(format!("schema: {schema}\nintegrations: {{}}\n")).is_err()
        );
    }
    for input in [
        b"".as_slice(),
        b"\xff",
        b"schema: podway.user-config/v2\n",
        b"schema: podway.user-config/v1\n---\nschema: podway.user-config/v1\n",
        b"schema: podway.user-config/v1\nprocedure_paths: []\n",
    ] {
        assert!(parse_user_config_v1(input).is_err());
    }
}

#[test]
fn user_config_limits_admit_exact_boundaries_and_refuse_one_over() {
    let input = "schema: podway.user-config/v1\nintegrations: {dolgorae: {enabled: true}}\n";
    // Values and containers count as nodes, matching workspace YAML admission.
    let mut limits = WorkspaceConfigParseLimitsV1 {
        max_bytes: input.len(),
        max_depth: 4,
        max_nodes: 5,
    };
    parse_user_config_v1_with_limits(input, limits).unwrap();
    limits.max_bytes -= 1;
    assert!(matches!(
        parse_user_config_v1_with_limits(input, limits),
        Err(ConfigError::InputTooLarge { .. })
    ));
    limits.max_bytes += 1;
    limits.max_depth -= 1;
    assert!(matches!(
        parse_user_config_v1_with_limits(input, limits),
        Err(ConfigError::InputTooDeep { .. })
    ));
    limits.max_depth += 1;
    limits.max_nodes -= 1;
    assert!(matches!(
        parse_user_config_v1_with_limits(input, limits),
        Err(ConfigError::InputTooComplex { .. })
    ));
    let exact_bytes = format!(
        "schema: podway.user-config/v1\n#{}",
        "x".repeat(65_536 - 31)
    );
    assert_eq!(exact_bytes.len(), 65_536);
    parse_user_config_v1(&exact_bytes).unwrap();
    assert!(parse_user_config_v1(format!("{exact_bytes}x")).is_err());
}

#[test]
fn default_settings_depth_and_node_ceilings_admit_the_boundary_before_semantic_rejection() {
    for schema in ["podway.workspace/v3", "podway.user-config/v1"] {
        let parse = |document: String| {
            if schema == "podway.workspace/v3" {
                parse_workspace_config_v1(document).map(|_| ())
            } else {
                parse_user_config_v1(document).map(|_| ())
            }
        };
        let mut nested = "true".to_owned();
        for _ in 0..14 {
            nested = format!("{{nested: {nested}}}");
        }
        let boundary = format!("schema: {schema}\nunknown: {nested}\n");
        assert!(matches!(
            parse(boundary),
            Err(ConfigError::InvalidDocument { .. })
        ));
        assert!(matches!(
            parse(format!("schema: {schema}\nunknown: {{nested: {nested}}}\n")),
            Err(ConfigError::InputTooDeep {
                maximum: 16,
                actual: 17
            })
        ));
        let values = vec!["true"; 1021].join(",");
        assert!(matches!(
            parse(format!("schema: {schema}\nunknown: [{values}]\n")),
            Err(ConfigError::InvalidDocument { .. })
        ));
        assert!(matches!(
            parse(format!("schema: {schema}\nunknown: [{values},true]\n")),
            Err(ConfigError::InputTooComplex {
                maximum: 1024,
                actual: 1025
            })
        ));
    }
}
