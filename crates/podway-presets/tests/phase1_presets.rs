//! Phase 1 conformance for built-in preset admission.

use podway_config::{ConfigError, ProcedureFormatV1, ProcedureWarningPolicyV1, parse_procedure_v1};
use podway_core::{
    AddItemV1, ArtifactValueV1, AttachItemV1, AttemptId, CheckItemV1, CommandContextV1,
    CompleteSessionV1, ItemMutationPreconditionsV1, ItemSpecV1, ItemValueV1, ProcedureSnapshotId,
    ProcedureWarningCodeV1, RetrySessionV1, ReturnSessionV1, Revision, SessionAggregateV1,
    SessionCommandV1, SessionId, SessionLifecycle, SetItemV1, Sha256Digest, UnixMillis,
    apply_transition_v1,
};
use podway_presets::{PresetError, catalog_v1, list};

#[test]
fn built_in_yaml_matches_catalog_sources_and_public_config_admission() {
    let expected_sources: [(&str, &[u8]); 4] = [
        ("analysis", include_bytes!("../../../presets/analysis.yaml")),
        ("bug-fix", include_bytes!("../../../presets/bug-fix.yaml")),
        (
            "docs-only",
            include_bytes!("../../../presets/docs-only.yaml"),
        ),
        ("sw-dev", include_bytes!("../../../presets/sw-dev.yaml")),
    ];
    let catalog = catalog_v1();

    assert_eq!(catalog.list(), list());
    assert_eq!(catalog.lookup("missing"), None);

    for (preset, (expected_id, source)) in catalog.list().iter().zip(expected_sources) {
        assert_eq!(preset.metadata.id, expected_id);
        assert_eq!(preset.source_bytes(), source);
        assert_eq!(preset.yaml.as_bytes(), source);
        assert_eq!(catalog.lookup(expected_id), Some(*preset));

        let parsed = parse_procedure_v1(source, ProcedureFormatV1::Yaml)
            .expect("root preset source must pass public config admission");
        let validated = preset
            .validate()
            .expect("catalog preset must pass public config admission");

        assert_eq!(validated.definition(), parsed.definition());
        assert_eq!(validated.canonical_json(), parsed.canonical_json());
        assert_eq!(validated.digest(), parsed.digest());
        assert_eq!(
            validated.metadata().schema,
            validated.definition().schema.as_str()
        );
        assert_eq!(validated.metadata().id, validated.definition().id.as_str());
        assert_eq!(
            validated.metadata().version,
            validated.definition().version.as_str()
        );
        assert_eq!(
            validated.metadata().name,
            validated.definition().name.as_str()
        );
        assert_eq!(
            validated.metadata().description,
            validated
                .definition()
                .description
                .as_deref()
                .expect("all built-in presets have descriptions"),
        );

        assert!(
            validated
                .clone()
                .admit(ProcedureWarningPolicyV1::Accept)
                .is_ok()
        );
    }
}

#[test]
fn built_in_presets_have_deterministic_canonical_digests_and_core_snapshots() {
    let snapshot_ids = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        "00000000-0000-4000-8000-000000000003",
        "00000000-0000-4000-8000-000000000004",
    ];

    for ((index, preset), snapshot_id) in catalog_v1().list().iter().enumerate().zip(snapshot_ids) {
        let first = preset
            .validate()
            .expect("catalog preset must validate deterministically");
        let second = preset
            .validate()
            .expect("catalog preset must validate deterministically");

        assert_eq!(
            first.canonical_json().as_bytes(),
            second.canonical_json().as_bytes()
        );
        assert_eq!(first.digest(), second.digest());

        assert_eq!(
            first
                .warnings()
                .iter()
                .map(|warning| warning.code())
                .collect::<Vec<_>>(),
            vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
        );

        let snapshot = first
            .into_snapshot_v1(
                ProcedureSnapshotId::new(snapshot_id).expect("fixed snapshot ID must be valid"),
                UnixMillis::new(index as u64),
                ProcedureWarningPolicyV1::Accept,
            )
            .expect("validated built-in preset must convert to a core snapshot");

        assert_eq!(snapshot.procedure_id(), preset.metadata.id);
        assert_eq!(snapshot.procedure_version(), preset.metadata.version);
        assert_eq!(snapshot.name(), preset.metadata.name);
        assert_eq!(
            snapshot.canonical_json().as_str().as_bytes(),
            second.canonical_json().as_bytes()
        );
        assert_eq!(snapshot.digest(), second.digest());
        assert_eq!(
            snapshot.accepted_warning_codes(),
            &[ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
        );
        assert_eq!(
            snapshot.source_label().as_str(),
            format!("preset:{}", preset.metadata.id),
        );
    }
}
#[test]
fn reject_warning_policy_blocks_all_built_in_preset_snapshot_conversions() {
    for (index, preset) in catalog_v1().list().iter().enumerate() {
        let error = preset
            .validate()
            .expect("catalog preset must validate before policy admission")
            .into_snapshot_v1(
                ProcedureSnapshotId::new(format!(
                    "00000000-0000-4000-8000-0000000001{:02}",
                    index + 1
                ))
                .expect("fixed snapshot ID must be valid"),
                UnixMillis::new(index as u64),
                ProcedureWarningPolicyV1::Reject,
            )
            .expect_err("any_previous preset must be rejected when warnings are rejected");

        assert_eq!(
            error,
            PresetError::Admission {
                preset_id: preset.metadata.id,
                error: ConfigError::WarningsAsErrors {
                    warnings: vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
                },
            },
        );
    }
}
fn next_uuid(counter: &mut u64) -> String {
    *counter += 1;
    format!("00000000-0000-4000-8000-{counter:012}")
}

fn next_attempt_id(counter: &mut u64) -> AttemptId {
    AttemptId::new(next_uuid(counter)).expect("fixed attempt ID must be valid")
}

fn apply(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    now: &mut u64,
) -> SessionAggregateV1 {
    *now += 1;
    apply_transition_v1(
        Some(session),
        &command,
        CommandContextV1 {
            expected_revision: session.revision(),
            now: UnixMillis::new(*now),
        },
    )
    .expect("built-in preset lifecycle transition must succeed")
    .next_aggregate()
    .expect("state-changing lifecycle transition must retain an aggregate")
    .clone()
}

fn active_item_preconditions(
    session: &SessionAggregateV1,
    item_id: &podway_core::ItemId,
) -> ItemMutationPreconditionsV1 {
    let attempt = session
        .attempts()
        .iter()
        .find(|attempt| Some(attempt.attempt_id()) == session.active_attempt_id())
        .expect("running session must have an active attempt");
    let slot = attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == item_id)
        .expect("active stage must have a slot for every item");

    ItemMutationPreconditionsV1 {
        expected_attempt_id: attempt.attempt_id().clone(),
        expected_item_revision: slot.revision(),
    }
}

fn satisfy_required_items(mut session: SessionAggregateV1, now: &mut u64) -> SessionAggregateV1 {
    let stage = session
        .snapshot()
        .stages()
        .iter()
        .find(|stage| Some(stage.id()) == session.active_stage_id())
        .expect("running session must have an active stage")
        .clone();

    for item in stage.items().iter().filter(|item| item.common().required()) {
        let preconditions = active_item_preconditions(&session, item.id());
        session = match item {
            ItemSpecV1::Confirm(_) => apply(
                &session,
                SessionCommandV1::Check(CheckItemV1 {
                    item_id: item.id().clone(),
                    preconditions,
                }),
                now,
            ),
            ItemSpecV1::Text(_) => apply(
                &session,
                SessionCommandV1::Set(SetItemV1 {
                    item_id: item.id().clone(),
                    value: ItemValueV1::text("completed"),
                    preconditions,
                }),
                now,
            ),
            ItemSpecV1::Choice(specification) => apply(
                &session,
                SessionCommandV1::Set(SetItemV1 {
                    item_id: item.id().clone(),
                    value: ItemValueV1::choice(
                        specification
                            .choices()
                            .first()
                            .expect("validated choice item must have a choice")
                            .clone(),
                    )
                    .expect("shipped choice must produce a valid value"),
                    preconditions,
                }),
                now,
            ),
            ItemSpecV1::Integer(specification) => apply(
                &session,
                SessionCommandV1::Set(SetItemV1 {
                    item_id: item.id().clone(),
                    value: ItemValueV1::integer(specification.minimum().unwrap_or(0)),
                    preconditions,
                }),
                now,
            ),
            ItemSpecV1::List(specification) => {
                let mut next = session.clone();
                for index in 0..specification.min_items().max(1) {
                    let preconditions = active_item_preconditions(&next, item.id());
                    next = apply(
                        &next,
                        SessionCommandV1::Add(AddItemV1 {
                            item_id: item.id().clone(),
                            value: format!("entry-{index}"),
                            preconditions,
                        }),
                        now,
                    );
                }
                next
            }
            ItemSpecV1::Artifact(_) => apply(
                &session,
                SessionCommandV1::Attach(AttachItemV1 {
                    item_id: item.id().clone(),
                    value: ArtifactValueV1::external_reference(
                        "artifact:conformance",
                        Sha256Digest::new(format!("sha256:{}", "a".repeat(64)))
                            .expect("fixed digest must be valid"),
                        1,
                        "text/plain",
                    )
                    .expect("fixed external artifact must be valid"),
                    preconditions,
                }),
                now,
            ),
        };
    }

    session
}

fn complete_active_stage(
    session: SessionAggregateV1,
    ids: &mut u64,
    now: &mut u64,
) -> SessionAggregateV1 {
    let session = satisfy_required_items(session, now);
    let final_stage =
        session.snapshot().stages().last().map(|stage| stage.id()) == session.active_stage_id();

    apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session
                .active_attempt_id()
                .expect("running session must have an active attempt")
                .clone(),
            next_attempt_id: (!final_stage).then(|| next_attempt_id(ids)),
            local_artifact_verifications: Vec::new(),
        }),
        now,
    )
}

#[test]
fn pac_068_071_built_in_preset_table_conformance() {
    let cases = [
        (
            "sw-dev",
            include_bytes!("../../../presets/sw-dev.yaml").as_slice(),
        ),
        (
            "bug-fix",
            include_bytes!("../../../presets/bug-fix.yaml").as_slice(),
        ),
        (
            "docs-only",
            include_bytes!("../../../presets/docs-only.yaml").as_slice(),
        ),
        (
            "analysis",
            include_bytes!("../../../presets/analysis.yaml").as_slice(),
        ),
    ];

    for (case_index, (preset_id, source)) in cases.into_iter().enumerate() {
        let preset = catalog_v1()
            .lookup(preset_id)
            .expect("every table row must name a built-in preset");
        let parsed = parse_procedure_v1(source, ProcedureFormatV1::Yaml)
            .expect("shipped preset source must pass schema and semantic validation");
        let validated = preset
            .validate()
            .expect("embedded preset loader must validate its shipped definition");
        assert_eq!(validated.definition(), parsed.definition());
        assert!(
            !validated
                .definition()
                .description
                .as_deref()
                .expect("shipped preset must have help text")
                .trim()
                .is_empty()
        );

        let snapshot = validated
            .into_snapshot_v1(
                ProcedureSnapshotId::new(format!("00000000-0000-4000-8000-{:012}", case_index + 1))
                    .expect("fixed snapshot ID must be valid"),
                UnixMillis::new(1),
                ProcedureWarningPolicyV1::Accept,
            )
            .expect("validated shipped preset must admit to the core engine");

        assert!(
            snapshot.stages().len() >= 2,
            "{preset_id} must have enough stages to prove return semantics"
        );
        for stage in snapshot.stages() {
            assert!(!stage.title().trim().is_empty(), "{preset_id} stage title");
            assert!(
                !stage.instructions().is_empty()
                    && stage
                        .instructions()
                        .iter()
                        .all(|instruction| !instruction.trim().is_empty()),
                "{preset_id} stage instructions"
            );
            for item in stage.items() {
                assert!(
                    !item.common().prompt().trim().is_empty(),
                    "{preset_id} item prompt"
                );
                assert!(
                    item.common()
                        .help()
                        .is_none_or(|help| !help.trim().is_empty()),
                    "{preset_id} item help"
                );
            }
        }

        let mut ids = (case_index as u64) * 100;
        let mut now = 1;
        let mut session = apply_transition_v1(
            None,
            &SessionCommandV1::Start(podway_core::StartSessionV1 {
                task_title: format!("{preset_id} conformance"),
                snapshot,
                session_id: SessionId::new(next_uuid(&mut ids))
                    .expect("fixed session ID must be valid"),
                first_attempt_id: next_attempt_id(&mut ids),
            }),
            CommandContextV1 {
                expected_revision: Revision::ZERO,
                now: UnixMillis::new(now),
            },
        )
        .expect("built-in preset must start in the real core transition engine")
        .next_aggregate()
        .expect("start must create a session")
        .clone();

        let first_attempt = session
            .active_attempt_id()
            .expect("started session must have an active attempt")
            .clone();
        session = apply(
            &session,
            SessionCommandV1::Retry(RetrySessionV1 {
                expected_attempt_id: first_attempt,
                reason: "exercise retry semantics".to_owned(),
                next_attempt_id: next_attempt_id(&mut ids),
            }),
            &mut now,
        );
        assert_eq!(
            session.attempts().len(),
            2,
            "{preset_id} retry creates an attempt"
        );

        session = complete_active_stage(session, &mut ids, &mut now);
        let second_stage = session
            .active_stage_id()
            .expect("completing the first stage must advance")
            .clone();
        assert_eq!(second_stage, *session.snapshot().stages()[1].id());

        session = satisfy_required_items(session, &mut now);
        session = apply(
            &session,
            SessionCommandV1::Return(ReturnSessionV1 {
                expected_attempt_id: session
                    .active_attempt_id()
                    .expect("second stage must be active")
                    .clone(),
                destination_stage_id: session.snapshot().stages()[0].id().clone(),
                reason: "exercise return semantics".to_owned(),
                destination_attempt_id: next_attempt_id(&mut ids),
            }),
            &mut now,
        );
        assert_eq!(
            session.active_stage_id(),
            Some(session.snapshot().stages()[0].id()),
            "{preset_id} return reactivates the earlier stage"
        );

        while session.lifecycle() == SessionLifecycle::Running {
            session = complete_active_stage(session, &mut ids, &mut now);
        }
        assert_eq!(
            session.lifecycle(),
            SessionLifecycle::Completed,
            "{preset_id} lifecycle completes after retry and return"
        );
    }
}
