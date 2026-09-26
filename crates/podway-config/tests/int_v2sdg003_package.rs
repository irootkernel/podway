use std::fs;
use std::path::Path;

use podway_config::{
    ConfigError, DelegationEvidenceItemV1, DelegationEvidenceResolutionV1, DelegationEvidenceV1,
    DelegationWorkPackageInputV1, FormatRequest, ParsedProcedure, ProcedureDocumentFormat,
    format_procedure_v2, parse_procedure_document, procedure_placement_budget_v2,
    project_delegation_work_package_v1, validate_procedure_v2,
};
use podway_core::{
    AttemptId, AttemptNumberV2, CriterionId, GoalCriterionV2, GoalDefinitionV2,
    GoalRevisionNumberV2, GoalRevisionReasonV2, GoalRevisionRecordV2, GoalStatementV2, GraphNodeId,
    ItemId, ItemTypeV1, Revision, SessionId, Sha256Digest, TraceSequenceV2, UnixMillis,
    WorkspaceId,
};
use sha2::{Digest as _, Sha256};

fn delegated_source() -> String {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/v2/compatibility/supervised-delegation-direct-control.yaml"),
    )
    .unwrap();
    source.replace(
        "      terminal: true\n",
        concat!(
            "      terminal: true\n",
            "      delegation:\n",
            "        binding_ref: local-specialist\n",
            "        role_ref: reviewer\n",
            "        role_snapshot_digest: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
            "        max_rounds: 3\n",
            "        after_accept: retain\n",
        ),
    )
}

fn admit(source: &str) -> podway_config::ValidatedProcedureV2 {
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap();
    validate_procedure_v2(parsed).unwrap()
}

#[test]
fn v2sdg007_delegated_observation_charges_both_projection_copies() {
    let direct_source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/v2/compatibility/supervised-delegation-direct-control.yaml"),
    )
    .unwrap();
    let direct = admit(&direct_source);
    let delegated = admit(&delegated_source());
    let placement = GraphNodeId::new("work").unwrap();
    let direct_charge = procedure_placement_budget_v2(&direct, &placement).unwrap();
    let delegated_charge = procedure_placement_budget_v2(&delegated, &placement).unwrap();
    assert_eq!(
        delegated_charge.next_static() - direct_charge.next_static(),
        8 * 1_024
    );
    assert_eq!(
        delegated_charge.observation_static_members() - direct_charge.observation_static_members(),
        16 * 1_024
    );
}

#[test]
fn v2sdg003_authoring_projection_preserves_delegation() {
    let source = delegated_source();
    let original = admit(&source);
    let formatted = format_procedure_v2(FormatRequest {
        source: &source,
        source_path: "delegated.yaml",
        format: ProcedureDocumentFormat::Yaml,
    })
    .unwrap();
    for field in [
        "delegation:",
        "binding_ref: local-specialist",
        "role_ref: reviewer",
        "role_snapshot_digest: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "max_rounds: 3",
        "after_accept: retain",
    ] {
        assert!(formatted.document().contains(field), "missing {field}");
    }
    assert_eq!(formatted.digest(), original.digest());
    assert_eq!(admit(formatted.document()).digest(), original.digest());
}

fn optional_upstream_source() -> String {
    delegated_source()
        .replace(
            "      terminal: true\n",
            "      next: upstream\n      evidence_from:\n        - node: upstream\n          required: false\n          items:\n            - result\n",
        )
        .replace(
            "manual_rework:\n",
            "    - id: upstream\n      use: work\n      terminal: true\nmanual_rework:\n",
        )
}

fn large_delegated_source(help_chars: usize) -> String {
    let items = (0..128)
        .map(|index| {
            serde_json::json!({
                "id": format!("item-{index}"),
                "type": "text",
                "prompt": "Record the result.",
                "help": "😀".repeat(help_chars),
                "required": false,
            })
        })
        .collect::<Vec<_>>();
    let mut nodes = (0..8)
        .map(|index| {
            serde_json::json!({
                "id": format!("source-{index}"),
                "use": "work",
                "next": if index == 7 { "delegated".to_owned() } else { format!("source-{}", index + 1) },
            })
        })
        .collect::<Vec<_>>();
    nodes.push(serde_json::json!({
        "id": "delegated",
        "use": "work",
        "terminal": true,
        "evidence_from": (0..8).map(|index| serde_json::json!({
            "node": format!("source-{index}")
        })).collect::<Vec<_>>(),
        "delegation": {
            "binding_ref": "local-specialist",
            "role_ref": "reviewer",
            "role_snapshot_digest": format!("sha256:{}", "a".repeat(64)),
            "max_rounds": 3,
            "after_accept": "retain",
        }
    }));
    serde_json::json!({
        "schema": "podway.procedure/v2",
        "id": "delegated-budget-control",
        "version": "1",
        "name": "Delegation package budget control",
        "purpose": "Exercise the static delegated package budget.",
        "node_definitions": {
            "work": {
                "type": "action",
                "title": "Record work",
                "intent": "Record a result.",
                "items": items,
            }
        },
        "graph": {"entry": "source-0", "nodes": nodes},
    })
    .to_string()
}

fn decision_sources(source: &str) -> String {
    let mut document: serde_json::Value = serde_json::from_str(source).unwrap();
    let items = document["node_definitions"]["work"]["items"].clone();
    document["node_definitions"]["source"] = serde_json::json!({
        "type": "decision", "title": "Choose input", "objective": "Record source evidence.",
        "prompt": "Proceed?", "items": items,
        "options": [{"id": "go", "label": "Go"}], "reason": {"required": true},
    });
    for node in document["graph"]["nodes"].as_array_mut().unwrap() {
        if let Some(next) = node.as_object_mut().unwrap().remove("next") {
            node["use"] = serde_json::json!("source");
            node["routes"] = serde_json::json!({"go": {"to": next, "effect": "advance"}});
        }
    }
    document.to_string()
}

#[test]
fn v2sdg003_static_package_budget_rejects_before_canonical_admission() {
    let below = large_delegated_source(800);
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(below.as_bytes(), ProcedureDocumentFormat::Json).unwrap();
    assert!(validate_procedure_v2(parsed).is_ok());

    let over = large_delegated_source(900);
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(over.as_bytes(), ProcedureDocumentFormat::Json).unwrap();
    assert!(matches!(
        validate_procedure_v2(parsed),
        Err(ConfigError::OutOfBounds {
            field: "delegation work package worst-case JSON bytes",
            ..
        })
    ));
}

#[test]
fn v2sdg003_decision_source_items_count_toward_static_package_budget() {
    let below = decision_sources(&large_delegated_source(800));
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(below.as_bytes(), ProcedureDocumentFormat::Json).unwrap();
    assert!(validate_procedure_v2(parsed).is_ok());

    let over = decision_sources(&large_delegated_source(900));
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(over.as_bytes(), ProcedureDocumentFormat::Json).unwrap();
    assert!(matches!(
        validate_procedure_v2(parsed),
        Err(ConfigError::OutOfBounds {
            field: "delegation work package worst-case JSON bytes",
            ..
        })
    ));
}

#[test]
fn v2sdg003_decision_evidence_preserves_selection_and_declared_types() {
    for selected in [false, true] {
        let mut document: serde_json::Value = serde_yaml::from_str(&delegated_source()).unwrap();
        document["node_definitions"]["source"] = serde_json::json!({
            "type": "decision", "title": "Choose input", "objective": "Record source evidence.",
            "prompt": "Proceed?", "options": [{"id": "go", "label": "Go"}],
            "reason": {"required": true},
            "items": [
                {"id": "input", "type": "text", "prompt": "Input", "required": false},
                {"id": "count", "type": "integer", "prompt": "Count", "required": false},
            ],
        });
        document["graph"]["entry"] = serde_json::json!("source");
        let mut selector = serde_json::json!({"node": "source", "required": false});
        if selected {
            selector["items"] = serde_json::json!(["input"]);
        }
        document["graph"]["nodes"][0]["evidence_from"] = serde_json::json!([selector]);
        document["graph"]["nodes"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({
                "id": "source", "use": "source",
                "routes": {"go": {"to": "work", "effect": "advance"}},
            }),
        );
        let procedure = admit(&serde_yaml::to_string(&document).unwrap());
        let podway_core::GraphPlacementV2::Action(action) =
            &procedure.parsed().graph().placements()[1]
        else {
            panic!("expected action")
        };
        let selector = action.evidence_from().unwrap().entries()[0].clone();
        let digest = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
        for resolved in [false, true] {
            let items = [("input", ItemTypeV1::Text), ("count", ItemTypeV1::Integer)]
                .into_iter()
                .take(if selected { 1 } else { 2 })
                .map(|(id, value_type)| {
                    let item_id = ItemId::new(id).unwrap();
                    if resolved {
                        DelegationEvidenceItemV1::Present {
                            item_id,
                            item_revision: Revision::new(1),
                            value_digest: digest.clone(),
                            value_type,
                            size_bytes: 5,
                        }
                    } else {
                        DelegationEvidenceItemV1::Missing { item_id }
                    }
                })
                .collect();
            let mut evidence = [DelegationEvidenceV1 {
                selector: selector.clone(),
                resolution: if resolved {
                    DelegationEvidenceResolutionV1::Resolved {
                        source_attempt_id: AttemptId::new("55555555-5555-4555-8555-555555555555")
                            .unwrap(),
                        source_attempt_number: AttemptNumberV2::FIRST,
                        source_items_digest: digest.clone(),
                    }
                } else {
                    DelegationEvidenceResolutionV1::Unresolved
                },
                items,
            }];
            let project = |evidence: &[DelegationEvidenceV1]| {
                project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
                    procedure: &procedure,
                    workspace_uuid: &WorkspaceId::new("11111111-1111-4111-8111-111111111111")
                        .unwrap(),
                    session_id: &SessionId::new("22222222-2222-4222-8222-222222222222").unwrap(),
                    placement_id: &GraphNodeId::new("work").unwrap(),
                    attempt_id: &AttemptId::new("33333333-3333-4333-8333-333333333333").unwrap(),
                    goal: None,
                    evidence,
                })
            };
            let package = project(&evidence).unwrap();
            let projected = package.package()["evidence"][0]["items"]
                .as_array()
                .unwrap();
            assert_eq!(projected.len(), if selected { 1 } else { 2 });
            assert_eq!(projected[0]["item_id"], "input");
            if !selected {
                assert_eq!(projected[1]["item_id"], "count");
            }
            if resolved {
                assert_eq!(projected[0]["value_type"], "text");
                if !selected {
                    assert_eq!(projected[1]["value_type"], "integer");
                }
                let mut wrong_type = evidence.clone();
                if let DelegationEvidenceItemV1::Present { value_type, .. } =
                    &mut wrong_type[0].items[0]
                {
                    *value_type = ItemTypeV1::Confirm;
                }
                assert!(project(&wrong_type).is_err());
            } else {
                assert!(projected.iter().all(|item| item["state"] == "missing"));
            }
            evidence[0].items.pop();
            assert!(project(&evidence).is_err());
        }
    }
}

#[test]
fn v2sdg003_package_digest_is_domain_separated_and_attempt_bound() {
    let procedure = admit(&delegated_source());
    let workspace = WorkspaceId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let session = SessionId::new("22222222-2222-4222-8222-222222222222").unwrap();
    let placement = GraphNodeId::new("work").unwrap();
    let attempt = AttemptId::new("33333333-3333-4333-8333-333333333333").unwrap();
    let package = project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
        procedure: &procedure,
        workspace_uuid: &workspace,
        session_id: &session,
        placement_id: &placement,
        attempt_id: &attempt,
        goal: None,
        evidence: &[],
    })
    .unwrap();
    assert_eq!(package.package()["goal"], serde_json::Value::Null);
    assert_eq!(package.package()["evidence"], serde_json::json!([]));
    assert_eq!(package.package()["action"]["title"], "Record work");
    assert!(package.package()["action"].get("type").is_none());
    let domain = format!(
        "podway.delegation-work-package/v1\n{}",
        package.canonical_json().as_str()
    );
    assert_eq!(
        package.digest().as_str(),
        format!("sha256:{:x}", Sha256::digest(domain.as_bytes()))
    );
    assert_ne!(package.digest(), procedure.digest());

    // Mutable output values and session revisions are deliberately absent from the input. A
    // repeated projection is the same assignment even after a caller records output elsewhere.
    let repeated = project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
        procedure: &procedure,
        workspace_uuid: &workspace,
        session_id: &session,
        placement_id: &placement,
        attempt_id: &attempt,
        goal: None,
        evidence: &[],
    })
    .unwrap();
    assert_eq!(package, repeated);

    let retry = AttemptId::new("44444444-4444-4444-8444-444444444444").unwrap();
    let retried = project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
        procedure: &procedure,
        workspace_uuid: &workspace,
        session_id: &session,
        placement_id: &placement,
        attempt_id: &retry,
        goal: None,
        evidence: &[],
    })
    .unwrap();
    assert_ne!(package.digest(), retried.digest());
}

#[test]
fn v2sdg003_package_binds_attempt_goal_revision_and_criteria() {
    let procedure = admit(&delegated_source().replace(
        "node_definitions:\n",
        "goal_tracking: true\nnode_definitions:\n",
    ));
    let workspace = WorkspaceId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let session = SessionId::new("22222222-2222-4222-8222-222222222222").unwrap();
    let placement = GraphNodeId::new("work").unwrap();
    let attempt = AttemptId::new("33333333-3333-4333-8333-333333333333").unwrap();
    let criteria = GoalDefinitionV2::new(vec![
        GoalCriterionV2::new(
            CriterionId::new("quality").unwrap(),
            "The result is reviewed.",
        )
        .unwrap(),
        GoalCriterionV2::new(
            CriterionId::new("delivery").unwrap(),
            "The work is handed off.",
        )
        .unwrap(),
    ])
    .unwrap();
    let first_goal = GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::FIRST,
        None,
        GoalStatementV2::new("Deliver the result.").unwrap(),
        criteria.clone(),
        None,
        None,
        false,
        None,
        TraceSequenceV2::FIRST,
        UnixMillis::new(1),
    )
    .unwrap();
    let untracked = admit(&delegated_source());
    assert!(matches!(
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &untracked,
            workspace_uuid: &workspace,
            session_id: &session,
            placement_id: &placement,
            attempt_id: &attempt,
            goal: Some(&first_goal),
            evidence: &[],
        }),
        Err(ConfigError::InvalidValue {
            field: "delegation work package goal",
            ..
        })
    ));
    let revised_goal = GoalRevisionRecordV2::new(
        GoalRevisionNumberV2::new(2),
        Some(GoalRevisionNumberV2::FIRST),
        GoalStatementV2::new("Deliver the result.").unwrap(),
        criteria,
        Some(GoalRevisionReasonV2::new("Clarify the target.").unwrap()),
        Some(placement.clone()),
        false,
        None,
        TraceSequenceV2::new(2),
        UnixMillis::new(2),
    )
    .unwrap();
    let project = |goal: &GoalRevisionRecordV2| {
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &procedure,
            workspace_uuid: &workspace,
            session_id: &session,
            placement_id: &placement,
            attempt_id: &attempt,
            goal: Some(goal),
            evidence: &[],
        })
        .unwrap()
    };
    let first = project(&first_goal);
    assert_eq!(first.package()["goal"]["revision"], 1);
    assert_eq!(first.package()["goal"]["statement"], "Deliver the result.");
    assert_eq!(
        first.package()["goal"]["criteria"][0]["criterion_id"],
        "quality"
    );
    assert_eq!(
        first.package()["goal"]["criteria"][1]["statement"],
        "The work is handed off."
    );
    let revised = project(&revised_goal);
    assert_eq!(revised.package()["goal"]["revision"], 2);
    assert_ne!(first.digest(), revised.digest());
}

#[test]
fn v2sdg003_unresolved_optional_evidence_keeps_selector_without_source_attempt() {
    let procedure = admit(&optional_upstream_source());
    let selector = match &procedure.parsed().graph().placements()[0] {
        podway_core::GraphPlacementV2::Action(action) => {
            action.evidence_from().unwrap().entries()[0].clone()
        }
        _ => panic!("expected action"),
    };
    let evidence = [DelegationEvidenceV1 {
        selector,
        resolution: DelegationEvidenceResolutionV1::Unresolved,
        items: vec![DelegationEvidenceItemV1::Missing {
            item_id: podway_core::ItemId::new("result").unwrap(),
        }],
    }];
    let package = project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
        procedure: &procedure,
        workspace_uuid: &WorkspaceId::new("11111111-1111-4111-8111-111111111111").unwrap(),
        session_id: &SessionId::new("22222222-2222-4222-8222-222222222222").unwrap(),
        placement_id: &GraphNodeId::new("work").unwrap(),
        attempt_id: &AttemptId::new("33333333-3333-4333-8333-333333333333").unwrap(),
        goal: None,
        evidence: &evidence,
    })
    .unwrap();
    assert_eq!(package.package()["evidence"][0]["resolution"], "unresolved");
    assert!(
        package.package()["evidence"][0]
            .get("source_attempt_id")
            .is_none()
    );
    assert_eq!(
        package.package()["evidence"][0]["items"][0]["state"],
        "missing"
    );
}

#[test]
fn v2sdg003_omitted_selector_projects_every_source_item_in_order() {
    let source = optional_upstream_source()
        .replace("          items:\n            - result\n", "")
        .replace(
            "graph:\n",
            "      - id: note\n        type: text\n        prompt: Record a note.\n        required: false\n        min_length: 0\n        max_length: 256\n        multiline: true\ngraph:\n",
        );
    let procedure = admit(&source);
    let selector = match &procedure.parsed().graph().placements()[0] {
        podway_core::GraphPlacementV2::Action(action) => {
            action.evidence_from().unwrap().entries()[0].clone()
        }
        _ => panic!("expected action"),
    };
    assert!(selector.selected_items().is_none());
    let mut evidence = [DelegationEvidenceV1 {
        selector,
        resolution: DelegationEvidenceResolutionV1::Unresolved,
        items: vec![
            DelegationEvidenceItemV1::Missing {
                item_id: ItemId::new("result").unwrap(),
            },
            DelegationEvidenceItemV1::Missing {
                item_id: ItemId::new("note").unwrap(),
            },
        ],
    }];
    let project = |evidence: &[DelegationEvidenceV1]| {
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &procedure,
            workspace_uuid: &WorkspaceId::new("11111111-1111-4111-8111-111111111111").unwrap(),
            session_id: &SessionId::new("22222222-2222-4222-8222-222222222222").unwrap(),
            placement_id: &GraphNodeId::new("work").unwrap(),
            attempt_id: &AttemptId::new("33333333-3333-4333-8333-333333333333").unwrap(),
            goal: None,
            evidence,
        })
    };
    let package = project(&evidence).unwrap();
    assert_eq!(
        package.package()["evidence"][0]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut wrong_selector = evidence.clone();
    wrong_selector[0].selector =
        podway_core::EvidenceReferenceV2::new(GraphNodeId::new("upstream").unwrap(), true, None)
            .unwrap();
    assert!(project(&wrong_selector).is_err());
    let mut oversized = evidence.clone();
    oversized[0].items = vec![
        DelegationEvidenceItemV1::Missing {
            item_id: ItemId::new("result").unwrap(),
        };
        129
    ];
    assert!(project(&oversized).is_err());
    evidence[0].items.swap(0, 1);
    assert!(project(&evidence).is_err());
    evidence[0].items.pop();
    assert!(project(&evidence).is_err());
    assert!(project(&[]).is_err());
}

#[test]
fn v2sdg003_frozen_source_and_complete_value_digests_change_package_identity() {
    let procedure = admit(&optional_upstream_source());
    let selector = match &procedure.parsed().graph().placements()[0] {
        podway_core::GraphPlacementV2::Action(action) => {
            action.evidence_from().unwrap().entries()[0].clone()
        }
        _ => panic!("expected action"),
    };
    let digest_a = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
    let digest_b = Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap();
    let mut evidence = [DelegationEvidenceV1 {
        selector,
        resolution: DelegationEvidenceResolutionV1::Resolved {
            source_attempt_id: AttemptId::new("55555555-5555-4555-8555-555555555555").unwrap(),
            source_attempt_number: AttemptNumberV2::new(1),
            source_items_digest: digest_a.clone(),
        },
        items: vec![DelegationEvidenceItemV1::Present {
            item_id: ItemId::new("result").unwrap(),
            item_revision: Revision::new(1),
            value_digest: digest_a.clone(),
            value_type: ItemTypeV1::Text,
            size_bytes: 5,
        }],
    }];
    let workspace = WorkspaceId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let session = SessionId::new("22222222-2222-4222-8222-222222222222").unwrap();
    let placement = GraphNodeId::new("work").unwrap();
    let attempt = AttemptId::new("33333333-3333-4333-8333-333333333333").unwrap();
    let project = |evidence: &[DelegationEvidenceV1]| {
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &procedure,
            workspace_uuid: &workspace,
            session_id: &session,
            placement_id: &placement,
            attempt_id: &attempt,
            goal: None,
            evidence,
        })
        .unwrap()
    };
    let first = project(&evidence);
    assert_eq!(first.package()["evidence"][0]["source_attempt_number"], 1);
    assert_eq!(
        first.package()["evidence"][0]["source_items_digest"],
        digest_a.as_str()
    );
    let mut invalid = evidence.clone();
    if let DelegationEvidenceResolutionV1::Resolved {
        source_attempt_number,
        ..
    } = &mut invalid[0].resolution
    {
        *source_attempt_number = AttemptNumberV2::ZERO;
    }
    assert!(
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &procedure,
            workspace_uuid: &workspace,
            session_id: &session,
            placement_id: &placement,
            attempt_id: &attempt,
            goal: None,
            evidence: &invalid,
        })
        .is_err()
    );
    let mut invalid = evidence.clone();
    invalid[0].resolution = DelegationEvidenceResolutionV1::Skipped {
        source_attempt_id: AttemptId::new("55555555-5555-4555-8555-555555555555").unwrap(),
        source_attempt_number: AttemptNumberV2::new(1),
        source_items_digest: digest_a.clone(),
    };
    assert!(
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &procedure,
            workspace_uuid: &workspace,
            session_id: &session,
            placement_id: &placement,
            attempt_id: &attempt,
            goal: None,
            evidence: &invalid,
        })
        .is_err()
    );
    invalid[0].resolution = DelegationEvidenceResolutionV1::Resolved {
        source_attempt_id: AttemptId::new("55555555-5555-4555-8555-555555555555").unwrap(),
        source_attempt_number: AttemptNumberV2::new(1),
        source_items_digest: digest_a.clone(),
    };
    invalid[0].items[0] = DelegationEvidenceItemV1::Present {
        item_id: ItemId::new("result").unwrap(),
        item_revision: Revision::new(1),
        value_digest: digest_a.clone(),
        value_type: ItemTypeV1::Integer,
        size_bytes: 5,
    };
    assert!(
        project_delegation_work_package_v1(DelegationWorkPackageInputV1 {
            procedure: &procedure,
            workspace_uuid: &workspace,
            session_id: &session,
            placement_id: &placement,
            attempt_id: &attempt,
            goal: None,
            evidence: &invalid,
        })
        .is_err()
    );
    if let DelegationEvidenceResolutionV1::Resolved {
        source_items_digest,
        ..
    } = &mut evidence[0].resolution
    {
        *source_items_digest = digest_b.clone();
    }
    let changed_source = project(&evidence);
    assert_ne!(first.digest(), changed_source.digest());
    if let DelegationEvidenceResolutionV1::Resolved {
        source_items_digest,
        ..
    } = &mut evidence[0].resolution
    {
        *source_items_digest = digest_a;
    }
    evidence[0].items[0] = DelegationEvidenceItemV1::Present {
        item_id: ItemId::new("result").unwrap(),
        item_revision: Revision::new(1),
        value_digest: digest_b,
        value_type: ItemTypeV1::Text,
        size_bytes: 5,
    };
    assert_ne!(first.digest(), project(&evidence).digest());
}
