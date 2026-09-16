//! V2DOG-003 runtime binding for independently pinned embedded Procedure v2 presets.

use podway_core::{AttemptId, ProcedureSnapshotId, SessionId, UnixMillis};
use podway_daemon::execution::{
    EmbeddedPresetProcedureProviderV1, ProcedureProviderV1, prepare_preset_procedure_v2_start,
};
use podway_daemon::{
    native_execution::NativeProcedureProviderV1, workspace::SqliteWorkspaceBindingInspectorV1,
};
use podway_presets::catalog_v2;
use podway_store::SqliteStoreOptionsV1;

#[test]
fn embedded_provider_prepares_all_v2_presets_from_their_shipped_digest() {
    let provider = EmbeddedPresetProcedureProviderV1;
    for (index, preset) in catalog_v2().list().iter().enumerate() {
        let suffix = index + 1;
        let state = prepare_preset_procedure_v2_start(
            &provider,
            preset.metadata.id,
            "Exercise the shipped preset",
            SessionId::new(format!("00000000-0000-4000-8000-{suffix:012}")).unwrap(),
            AttemptId::new(format!("00000000-0000-4000-8001-{suffix:012}")).unwrap(),
            ProcedureSnapshotId::new(format!("00000000-0000-4000-8002-{suffix:012}")).unwrap(),
            UnixMillis::new(1_700_000_000_000 + suffix as u64),
        )
        .expect("shipped preset preparation must succeed");
        let snapshot = state.snapshot();

        assert_eq!(snapshot.procedure_id(), preset.metadata.id);
        assert_eq!(snapshot.procedure_version(), preset.metadata.version);
        assert_eq!(snapshot.digest().as_str(), preset.shipped_digest);
        assert_eq!(
            snapshot.source().as_str(),
            format!("preset:{}", preset.metadata.id)
        );
        assert_eq!(
            state.trace().active_attempt().unwrap().graph_node_id(),
            snapshot.entry_graph_node_id()
        );
    }
}

#[test]
fn embedded_v2_provider_rejects_removed_and_unknown_preset_names() {
    let provider = EmbeddedPresetProcedureProviderV1;
    for name in ["analysis", "bug-fix", "docs-only", "sw-dev", "missing"] {
        assert!(
            provider
                .load_preset_snapshot_v2(
                    name,
                    ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000001").unwrap(),
                    UnixMillis::new(1),
                )
                .expect("catalog miss is not a source-admission failure")
                .is_none(),
            "{name} must not be reinterpreted as a v2 preset"
        );
    }
}

#[test]
fn unknown_v2_preset_preparation_has_a_distinct_error() {
    let result = prepare_preset_procedure_v2_start(
        &EmbeddedPresetProcedureProviderV1,
        "aquarium-goal-v2",
        "Reject an unknown built-in preset",
        SessionId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        AttemptId::new("00000000-0000-4000-8001-000000000001").unwrap(),
        ProcedureSnapshotId::new("00000000-0000-4000-8002-000000000001").unwrap(),
        UnixMillis::new(1),
    );
    assert!(matches!(
        result,
        Err(podway_daemon::execution::ProcedureV2StartPreparationErrorV1::PresetNotFound)
    ));
}

#[test]
fn native_production_provider_delegates_v2_presets_to_the_embedded_catalog() {
    let provider = NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(
        SqliteStoreOptionsV1::new(8).unwrap(),
    ));
    for preset in catalog_v2().list() {
        let (snapshot, pinned) = provider
            .load_preset_snapshot_v2(
                preset.metadata.id,
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000010").unwrap(),
                UnixMillis::new(10),
            )
            .expect("native provider must preserve embedded admission errors")
            .expect("native provider must expose every embedded v2 preset");
        assert_eq!(snapshot.digest(), &pinned);
        assert_eq!(pinned.as_str(), preset.shipped_digest);
    }
}
