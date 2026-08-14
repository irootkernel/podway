//! Canonical execution identity regressions for the V2GOL epic.

use super::*;

const GOAL_PROCEDURE: &[u8] = br#"schema: podway.procedure/v2
id: v2gol-epic-integrity
version: "2"
name: V2GOL execution integrity
purpose: Exercise durable goal execution identity checks.
goal_tracking: true
node_definitions:
  assess:
    type: decision
    title: Assess goal
    objective: Select the recorded goal outcome.
    prompt: Which outcome applies?
    options:
      - id: achieved
        label: Achieved
      - id: not-achieved
        label: Not achieved
      - id: superseded
        label: Superseded
    reason:
      required: true
    assessment:
      target: session_goal
      outcomes:
        achieved: achieved
        not-achieved: not_achieved
        superseded: superseded
  finish:
    type: action
    title: Finish
    intent: Finish after assessment.
graph:
  entry: assess
  nodes:
    - id: assess
      use: assess
      routes:
        achieved:
          to: finish
          effect: advance
        not-achieved:
          to: finish
          effect: advance
        superseded:
          to: finish
          effect: advance
    - id: finish
      use: finish
      terminal: true
manual_rework:
  allowed_targets:
    - assess
"#;

fn selector() -> WorktreeSelectorWireV1 {
    WorktreeSelectorWireV1::new(b"/tmp/worktree", "/tmp/worktree", None).unwrap()
}

fn workspace(suffix: u64) -> WorkspaceId {
    WorkspaceId::new(format!("00000000-0000-4000-8000-{suffix:012x}")).unwrap()
}

fn session(suffix: u64) -> SessionId {
    SessionId::new(format!("00000000-0000-4000-8000-{suffix:012x}")).unwrap()
}

fn attempt(suffix: u64) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{suffix:012x}")).unwrap()
}

fn criterion() -> GoalCriterionWireV2 {
    GoalCriterionWireV2 {
        criterion_id: podway_core::CriterionId::new("verified").unwrap(),
        statement: "The durable mutation preserves its admitted identity.".to_owned(),
    }
}

fn goal_define(workspace_id: WorkspaceId) -> AdmittedProcedureV2GoalMutationV1 {
    AdmittedProcedureV2GoalMutationV1::Define {
        selector: selector(),
        workspace_id,
        command: GoalDefineV2 {
            goal: "Preserve durable goal identity.".to_owned(),
            criteria: vec![criterion()],
            actor: Some("integrity-test".to_owned()),
            preconditions: podway_protocol::SessionIdentityPreconditionsWireV1 {
                expected_session_id: session(1),
                expected_session_revision: Revision::new(7),
            },
        },
    }
}

fn goal_revise(workspace_id: WorkspaceId) -> AdmittedProcedureV2GoalMutationV1 {
    AdmittedProcedureV2GoalMutationV1::Revise {
        selector: selector(),
        workspace_id,
        command: GoalReviseV2 {
            goal: "Preserve revised durable goal identity.".to_owned(),
            criteria: vec![criterion()],
            target_graph_node_id: podway_core::GraphNodeId::new("assess").unwrap(),
            reason: "Exercise the canonical revision fence.".to_owned(),
            actor: Some("integrity-test".to_owned()),
            reactivate: false,
            preconditions: podway_protocol::GoalRevisionPreconditionsWireV2 {
                expected_session_id: session(1),
                expected_session_revision: Revision::new(7),
                expected_attempt_id: Some(attempt(2)),
                expected_goal_revision: 1,
            },
        },
        fresh_attempt_id: attempt(3),
    }
}

#[test]
fn v2gol_epic_v11_goal_claim_rejects_foreign_workspace_for_define_and_revise() {
    let claimed_workspace = workspace(10);
    let foreign_workspace = workspace(11);

    for admitted in [
        goal_define(foreign_workspace.clone()),
        goal_revise(foreign_workspace.clone()),
    ] {
        let canonical = procedure_v2_goal_execution_document_v1(&admitted).unwrap();
        let decoded = decode_procedure_v2_goal_execution_v1(canonical.as_str()).unwrap();
        assert!(matches!(
            validate_goal_claim_workspace_v1(&decoded, &claimed_workspace),
            Err(ExecutionErrorV1::InvalidPersistedExecution { .. })
        ));
    }

    let admitted = goal_define(claimed_workspace.clone());
    let canonical = procedure_v2_goal_execution_document_v1(&admitted).unwrap();
    let decoded = decode_procedure_v2_goal_execution_v1(canonical.as_str()).unwrap();
    validate_goal_claim_workspace_v1(&decoded, &claimed_workspace).unwrap();
}

#[test]
fn v2gol_epic_v11_goal_revision_rejects_zero_revision_in_canonical_execution() {
    let canonical = procedure_v2_goal_execution_document_v1(&goal_revise(workspace(20))).unwrap();
    let mut document: Value = serde_json::from_str(canonical.as_str()).unwrap();
    document["preconditions"]["goal_revision"] = json!(0);

    assert!(matches!(
        decode_procedure_v2_goal_execution_v1(&document.to_string()),
        Err(ExecutionErrorV1::InvalidPersistedExecution { .. })
    ));
}

fn v12_start_execution(workspace_id: WorkspaceId) -> CanonicalExecutionJsonV1 {
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "integrity.yaml",
        GOAL_PROCEDURE,
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000101").unwrap(),
        UnixMillis::new(10),
    )
    .unwrap();
    let state = graph_session_state_from_procedure_v2_snapshot(
        snapshot,
        "V2GOL replay integrity",
        session(102),
        attempt(103),
        UnixMillis::new(10),
    )
    .unwrap();
    let admitted = AdmittedProcedureV2StartV1 {
        selector: selector(),
        workspace_id,
        replace: false,
        expected_current: GraphStartCurrentTaskV2::Absent,
        state,
    };
    let command = SliceCommandV1::SessionStart(SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "integrity.yaml".to_owned(),
        },
        task_title: "V2GOL replay integrity".to_owned(),
        expected_procedure_digest: Some(admitted.state.snapshot().digest().clone()),
        dry_run: false,
    });
    let v6 = procedure_v2_start_execution_document_v1(&admitted, &command).unwrap();
    let mut document: Value = serde_json::from_str(v6.as_str()).unwrap();
    document["execution_version"] = json!(EXECUTION_DOCUMENT_VERSION_V12);
    document.as_object_mut().unwrap().insert(
        "initial_goal".to_owned(),
        json!({
            "actor": "integrity-test",
            "criteria": [criterion()],
            "goal": "Preserve goal-bearing start identity."
        }),
    );
    CanonicalExecutionJsonV1::new(canonicalize_json_v1(&document).unwrap()).unwrap()
}

#[test]
fn v2gol_epic_v12_replay_requires_exact_version_and_workspace() {
    let expected_workspace = workspace(30);
    let canonical = v12_start_execution(expected_workspace.clone());
    assert!(
        decode_typed_start_replay_execution_v1(&canonical, &expected_workspace)
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        decode_typed_start_replay_execution_v1(&canonical, &workspace(31)),
        Err(ExecutionErrorV1::InvalidPersistedExecution { .. })
    ));

    let mut omitted_goal: Value = serde_json::from_str(canonical.as_str()).unwrap();
    omitted_goal["initial_goal"] = Value::Null;
    let omitted_goal =
        CanonicalExecutionJsonV1::new(canonicalize_json_v1(&omitted_goal).unwrap()).unwrap();
    assert!(
        decode_typed_start_replay_execution_v1(&omitted_goal, &expected_workspace)
            .unwrap()
            .is_some()
    );

    let mut wrong_version: Value = serde_json::from_str(canonical.as_str()).unwrap();
    wrong_version["execution_version"] = json!(EXECUTION_DOCUMENT_VERSION_V6);
    wrong_version
        .as_object_mut()
        .unwrap()
        .remove("initial_goal");
    let wrong_version =
        CanonicalExecutionJsonV1::new(canonicalize_json_v1(&wrong_version).unwrap()).unwrap();
    assert!(
        decode_typed_start_replay_execution_v1(&wrong_version, &expected_workspace)
            .unwrap()
            .is_none()
    );
}
