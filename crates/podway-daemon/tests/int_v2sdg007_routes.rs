//! Native dispatcher coverage for the delegated package, acceptance, and receipt lifecycle.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{Revision, SessionId, canonicalize_json_v1};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const PROCEDURE: &str = r#"schema: podway.procedure/v2
id: delegated-native-routes
version: "1"
name: Delegated native routes
purpose: Complete a delegated attempt through native dispatch.
node_definitions:
  work:
    type: action
    title: Work
    intent: Record final evidence before acceptance.
    items:
      - id: done
        type: confirm
        prompt: Confirm the final result.
        required: true
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
      delegation:
        binding_ref: local-specialist
        role_ref: reviewer
        role_snapshot_digest: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        max_rounds: 3
        after_accept: retain
"#;

const DIRECT_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: direct-native-routes
version: "1"
name: Direct native routes
purpose: Preserve the Direct path without delegation.
node_definitions:
  work:
    type: action
    title: Work
    intent: Complete the ordinary action.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"#;

const EVIDENCE_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: delegated-evidence-routes
version: "1"
name: Delegated evidence routes
purpose: Carry recorded source evidence into a delegated package.
node_definitions:
  source:
    type: action
    title: Source
    intent: Record source evidence.
    items:
      - id: result
        type: text
        prompt: Record the result.
        required: true
  work:
    type: action
    title: Delegated work
    intent: Accept the source evidence.
graph:
  entry: source
  nodes:
    - id: source
      use: source
      next: work
    - id: work
      use: work
      evidence_from:
        - node: source
          required: true
          items: [result]
      terminal: true
      delegation:
        binding_ref: local-specialist
        role_ref: reviewer
        role_snapshot_digest: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        max_rounds: 3
        after_accept: retain
"#;

fn fields(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn error_code(response: ResponseEnvelopeV2) -> String {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("expected a closed error: {response:?}");
    };
    error.code().as_str().to_owned()
}

fn error_code_and_reason(response: ResponseEnvelopeV2) -> (String, String) {
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("expected a closed error: {response:?}");
    };
    (
        error.code().as_str().to_owned(),
        error.details()["reason"].as_str().unwrap().to_owned(),
    )
}

#[test]
fn v2sdg007_direct_attempt_rejects_delegation_reads_without_changing_direct_work() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("direct.yaml"), DIRECT_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(DIRECT_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2sdg007-direct-routes");
    let init = runtime::request(
        772_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2sdg007-direct-init",
        PreconditionsV1::default(),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &init), "workspace.init");
    let binding = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            None,
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = binding
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let selector_path = fs::canonicalize(fixture.main()).unwrap();
    let selector = WorktreeSelectorWireV1::new(
        selector_path.as_os_str().as_encoded_bytes(),
        selector_path.display().to_string(),
        Some(workspace_uuid.clone()),
    )
    .unwrap();
    let start = runtime::request(
        772_002,
        "session.start",
        &selector,
        fields(
            json!({"procedure":"direct.yaml", "expected_procedure_digest":digest,
            "task_title":"Direct work remains available"}),
        ),
        "v2sdg007-direct-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    runtime::begin(
        &dispatcher,
        &selector,
        772_003,
        session_id,
        Map::new(),
        "v2sdg007-direct-begin",
    );
    let status = runtime::status(&dispatcher, &selector, 772_004, session_id);
    let attempt_id = status["current"]["attempt"]["attempt_id"].as_str().unwrap();
    let package = runtime::request(
        772_005,
        "delegation.package",
        &selector,
        Map::new(),
        "unused-package-key",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &package)),
        (
            "DELEGATION_NOT_APPLICABLE".to_owned(),
            "direct_attempt".to_owned()
        )
    );
    let receipt = runtime::request(
        772_006,
        "delegation.receipt",
        &selector,
        fields(json!({"session_id":session_id, "attempt_id":attempt_id})),
        "unused-receipt-key",
        PreconditionsV1::default(),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &receipt)),
        (
            "DELEGATION_NOT_APPLICABLE".to_owned(),
            "direct_attempt".to_owned()
        )
    );
    let direct_accept = runtime::request(
        772_008,
        "delegation.accept",
        &selector,
        fields(json!({"input":{
            "schema":"podway.delegation-accept-input/v1",
            "workspace_uuid":workspace_uuid,
            "session_id":session_id,
            "session_revision":status["session"]["revision"],
            "attempt_id":attempt_id,
            "idempotency_key":"v2sdg007-direct-accept",
            "package_digest":format!("sha256:{}", "a".repeat(64)),
            "execution":{
                "binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1",
                "primary_host":"external", "scope_kind":"test", "scope_ref":"scope-1",
                "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
                "role_ref":"reviewer", "role_snapshot_digest":format!("sha256:{}", "a".repeat(64)),
                "agent_configuration_digest":format!("sha256:{}", "b".repeat(64)),
                "accepted_task_digest":format!("sha256:{}", "c".repeat(64))
            },
            "rounds_used":1,
            "result_digest":format!("sha256:{}", "d".repeat(64)),
            "summary":"Direct attempts cannot accept delegation.",
            "worker_disposition":"idle", "accepted":true
        }})),
        "v2sdg007-direct-accept",
        runtime::session_preconditions(&status),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &direct_accept)),
        (
            "DELEGATION_NOT_APPLICABLE".to_owned(),
            "direct_attempt".to_owned()
        )
    );
    let complete = runtime::request(
        772_007,
        "session.complete",
        &selector,
        Map::new(),
        "v2sdg007-direct-complete",
        runtime::session_preconditions(&status),
    );
    let result = runtime::v2_result(
        runtime::dispatch(&dispatcher, &complete),
        "session.complete",
    );
    assert_eq!(result["schema"], "podway.stage-transition-result/v2");
}

#[test]
fn v2sdg007_delegated_package_uses_recorded_source_evidence() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("evidence.yaml"), EVIDENCE_PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(EVIDENCE_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let initial_selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2sdg007-evidence-routes");
    let init = runtime::request(
        773_001,
        "workspace.init",
        &initial_selector,
        Map::new(),
        "v2sdg007-evidence-init",
        PreconditionsV1::default(),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &init), "workspace.init");
    let binding = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            None,
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = binding
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let selector_path = fs::canonicalize(fixture.main()).unwrap();
    let selector = WorktreeSelectorWireV1::new(
        selector_path.as_os_str().as_encoded_bytes(),
        selector_path.display().to_string(),
        Some(workspace_uuid.clone()),
    )
    .unwrap();
    let start = runtime::request(
        773_002,
        "session.start",
        &selector,
        fields(
            json!({"procedure":"evidence.yaml", "expected_procedure_digest":procedure_digest,
            "task_title":"Carry source evidence into delegation"}),
        ),
        "v2sdg007-evidence-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    let prepared_package = runtime::request(
        773_003,
        "delegation.package",
        &selector,
        Map::new(),
        "unused-prepared-package",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        error_code(runtime::dispatch(&dispatcher, &prepared_package)),
        "SESSION_NOT_RUNNING"
    );
    runtime::begin(
        &dispatcher,
        &selector,
        773_004,
        session_id,
        Map::new(),
        "v2sdg007-evidence-begin",
    );
    let source = runtime::status(&dispatcher, &selector, 773_005, session_id);
    let source_attempt_id = source["current"]["attempt"]["attempt_id"].as_str().unwrap();
    let set = runtime::request(
        773_006,
        "item.set",
        &selector,
        fields(json!({"item_id":"result", "value":"source proof"})),
        "v2sdg007-evidence-set",
        runtime::item_preconditions(&source, "result"),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &set), "item.set");
    let source_ready = runtime::status(&dispatcher, &selector, 773_007, session_id);
    let advance = runtime::request(
        773_008,
        "session.complete",
        &selector,
        Map::new(),
        "v2sdg007-evidence-complete-source",
        runtime::session_preconditions(&source_ready),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &advance), "session.complete");
    let target = runtime::status(&dispatcher, &selector, 773_009, session_id);
    assert_eq!(target["current"]["node"]["graph_node_id"], "work");
    let package_request = runtime::request(
        773_010,
        "delegation.package",
        &selector,
        Map::new(),
        "unused-evidence-package",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let package = runtime::v2_result(
        runtime::dispatch(&dispatcher, &package_request),
        "delegation.package",
    );
    let evidence = &package["package"]["evidence"][0];
    assert_eq!(evidence["resolution"], "resolved");
    assert_eq!(evidence["source_attempt_id"], source_attempt_id);
    assert_eq!(evidence["source_attempt_number"], 1);
    assert_eq!(evidence["selector"]["node"], "source");
    let value = json!({"kind":"text", "value":"source proof"});
    let canonical_value = canonicalize_json_v1(&value).unwrap();
    assert_eq!(evidence["items"][0]["state"], "present");
    assert_eq!(evidence["items"][0]["item_id"], "result");
    assert_eq!(evidence["items"][0]["item_revision"], 1);
    assert_eq!(evidence["items"][0]["value_type"], "text");
    assert_eq!(evidence["items"][0]["size_bytes"], canonical_value.len());
    assert_eq!(
        evidence["items"][0]["value_digest"],
        format!("sha256:{:x}", Sha256::digest(canonical_value.as_bytes()))
    );
    let canonical_package = canonicalize_json_v1(&package["package"]).unwrap();
    let mut package_hasher = Sha256::new();
    package_hasher.update(b"podway.delegation-work-package/v1\n");
    package_hasher.update(canonical_package.as_bytes());
    assert_eq!(
        package["package_digest"],
        format!("sha256:{:x}", package_hasher.finalize())
    );
    assert_eq!(
        runtime::v2_result(
            runtime::dispatch(&dispatcher, &package_request),
            "delegation.package"
        )["package_digest"],
        package["package_digest"]
    );
    let target_attempt_id = target["current"]["attempt"]["attempt_id"].as_str().unwrap();
    let accept = runtime::request(
        773_011,
        "delegation.accept",
        &selector,
        fields(json!({"input":{
            "schema":"podway.delegation-accept-input/v1",
            "workspace_uuid":workspace_uuid,
            "session_id":session_id,
            "session_revision":target["session"]["revision"],
            "attempt_id":target_attempt_id,
            "idempotency_key":"v2sdg007-evidence-accept",
            "package_digest":package["package_digest"],
            "execution":{
                "binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1",
                "primary_host":"external", "scope_kind":"test", "scope_ref":"scope-1",
                "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
                "role_ref":"reviewer", "role_snapshot_digest":format!("sha256:{}", "a".repeat(64)),
                "agent_configuration_digest":format!("sha256:{}", "b".repeat(64)),
                "accepted_task_digest":format!("sha256:{}", "c".repeat(64))
            },
            "rounds_used":1,
            "result_digest":format!("sha256:{}", "d".repeat(64)),
            "summary":"Source evidence was accepted.",
            "worker_disposition":"idle", "accepted":true
        }})),
        "v2sdg007-evidence-accept",
        runtime::session_preconditions(&target),
    );
    let accepted = runtime::v2_result(runtime::dispatch(&dispatcher, &accept), "delegation.accept");
    assert_eq!(accepted["schema"], "podway.delegation-accept-result/v1");
    let read = runtime::request(
        773_012,
        "delegation.receipt",
        &selector,
        fields(json!({"session_id":session_id, "attempt_id":target_attempt_id})),
        "unused-evidence-receipt",
        PreconditionsV1::default(),
    );
    let receipt = runtime::v2_result(runtime::dispatch(&dispatcher, &read), "delegation.receipt");
    assert_eq!(
        receipt["receipt"]["package_digest"],
        package["package_digest"]
    );
}

#[test]
fn v2sdg007_release_declaration_requires_released_worker() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let source = PROCEDURE.replace("after_accept: retain", "after_accept: release");
    fs::write(fixture.main().join("release.yaml"), &source).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2sdg007-release-routes");
    let initial_selector = runtime::selector(fixture.main());
    let init = runtime::request(
        774_001,
        "workspace.init",
        &initial_selector,
        Map::new(),
        "v2sdg007-release-init",
        PreconditionsV1::default(),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &init), "workspace.init");
    let binding = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            None,
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = binding
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let selector_path = fs::canonicalize(fixture.main()).unwrap();
    let selector = WorktreeSelectorWireV1::new(
        selector_path.as_os_str().as_encoded_bytes(),
        selector_path.display().to_string(),
        Some(workspace_uuid.clone()),
    )
    .unwrap();
    let start = runtime::request(
        774_002,
        "session.start",
        &selector,
        fields(
            json!({"procedure":"release.yaml", "expected_procedure_digest":procedure_digest,
            "task_title":"Release declaration acceptance"}),
        ),
        "v2sdg007-release-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    runtime::begin(
        &dispatcher,
        &selector,
        774_003,
        session_id,
        Map::new(),
        "v2sdg007-release-begin",
    );
    let before = runtime::status(&dispatcher, &selector, 774_004, session_id);
    let check = runtime::request(
        774_005,
        "item.check",
        &selector,
        fields(json!({"item_id":"done"})),
        "v2sdg007-release-check",
        runtime::item_preconditions(&before, "done"),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &check), "item.check");
    let ready = runtime::status(&dispatcher, &selector, 774_006, session_id);
    let package_request = runtime::request(
        774_007,
        "delegation.package",
        &selector,
        Map::new(),
        "unused-release-package",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let package = runtime::v2_result(
        runtime::dispatch(&dispatcher, &package_request),
        "delegation.package",
    );
    let attempt_id = ready["current"]["attempt"]["attempt_id"].as_str().unwrap();
    let input = json!({
        "schema":"podway.delegation-accept-input/v1",
        "workspace_uuid":workspace_uuid,
        "session_id":session_id,
        "session_revision":ready["session"]["revision"],
        "attempt_id":attempt_id,
        "idempotency_key":"v2sdg007-release-accept",
        "package_digest":package["package_digest"],
        "execution":{
            "binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1",
            "primary_host":"external", "scope_kind":"test", "scope_ref":"scope-1",
            "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
            "role_ref":"reviewer", "role_snapshot_digest":format!("sha256:{}", "a".repeat(64)),
            "agent_configuration_digest":format!("sha256:{}", "b".repeat(64)),
            "accepted_task_digest":format!("sha256:{}", "c".repeat(64))
        },
        "rounds_used":3,
        "result_digest":format!("sha256:{}", "d".repeat(64)),
        "summary":"The worker was released after final evidence.",
        "worker_disposition":"released", "accepted":true
    });
    let mut wrong_disposition = input.clone();
    wrong_disposition["idempotency_key"] = json!("v2sdg007-release-wrong-disposition");
    wrong_disposition["worker_disposition"] = json!("idle");
    let rejected = runtime::request(
        774_008,
        "delegation.accept",
        &selector,
        fields(json!({"input":wrong_disposition})),
        "v2sdg007-release-wrong-disposition",
        runtime::session_preconditions(&ready),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &rejected)),
        (
            "DELEGATION_BINDING_MISMATCH".to_owned(),
            "disposition_mismatch".to_owned()
        )
    );
    let accept = runtime::request(
        774_009,
        "delegation.accept",
        &selector,
        fields(json!({"input":input})),
        "v2sdg007-release-accept",
        runtime::session_preconditions(&ready),
    );
    let accepted = runtime::v2_result(runtime::dispatch(&dispatcher, &accept), "delegation.accept");
    assert_eq!(accepted["schema"], "podway.delegation-accept-result/v1");
    let receipt = runtime::request(
        774_010,
        "delegation.receipt",
        &selector,
        fields(json!({"session_id":session_id, "attempt_id":attempt_id})),
        "unused-release-receipt",
        PreconditionsV1::default(),
    );
    let read = runtime::v2_result(
        runtime::dispatch(&dispatcher, &receipt),
        "delegation.receipt",
    );
    assert_eq!(read["receipt"]["worker_disposition"], "released");
    assert_eq!(read["receipt"]["rounds_used"], 3);
}

#[test]
fn v2sdg007_native_delegation_round_trip_and_closed_failures() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    fs::write(fixture.main().join("delegated.yaml"), PROCEDURE).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml).unwrap()
    else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let initial_selector = runtime::selector(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2sdg007-native-routes");

    let initialized = runtime::request(
        771_001,
        "workspace.init",
        &initial_selector,
        Map::new(),
        "v2sdg007-init",
        PreconditionsV1::default(),
    );
    let _ = runtime::v2_result(
        runtime::dispatch(&dispatcher, &initialized),
        "workspace.init",
    );
    let binding = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            None,
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = binding
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let selector_path = fs::canonicalize(fixture.main()).unwrap();
    let selector = WorktreeSelectorWireV1::new(
        selector_path.as_os_str().as_encoded_bytes(),
        selector_path.display().to_string(),
        Some(workspace_uuid.clone()),
    )
    .unwrap();

    let start = runtime::request(
        771_002,
        "session.start",
        &selector,
        fields(
            json!({"procedure":"delegated.yaml", "expected_procedure_digest":procedure_digest,
            "task_title":"V2SDG-007 native route"}),
        ),
        "v2sdg007-start",
        PreconditionsV1::default(),
    );
    let started = runtime::v2_result(runtime::dispatch(&dispatcher, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    runtime::begin(
        &dispatcher,
        &selector,
        771_003,
        &session_id,
        Map::new(),
        "v2sdg007-begin",
    );

    let before = runtime::status(&dispatcher, &selector, 771_004, &session_id);
    let attempt_id = before["current"]["attempt"]["attempt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let receipt = runtime::request(
        771_012,
        "delegation.receipt",
        &selector,
        fields(json!({"session_id":session_id, "attempt_id":attempt_id})),
        "unused-receipt-key",
        PreconditionsV1::default(),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &receipt)),
        (
            "DELEGATION_RECEIPT_REQUIRED".to_owned(),
            "receipt_missing".to_owned()
        )
    );
    let package = runtime::request(
        771_005,
        "delegation.package",
        &selector,
        Map::new(),
        "unused-package-key",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let package_result = runtime::v2_result(
        runtime::dispatch(&dispatcher, &package),
        "delegation.package",
    );
    assert_eq!(
        package_result["schema"],
        "podway.delegation-package-result/v1"
    );
    assert_eq!(package_result["package"]["attempt_id"], attempt_id);
    let package_digest = package_result["package_digest"]
        .as_str()
        .unwrap()
        .to_owned();

    let confirm = runtime::request(
        771_006,
        "item.check",
        &selector,
        fields(json!({"item_id":"done"})),
        "v2sdg007-check",
        runtime::item_preconditions(&before, "done"),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &confirm), "item.check");
    let ready = runtime::status(&dispatcher, &selector, 771_007, &session_id);
    let complete_without_receipt = runtime::request(
        771_008,
        "session.complete",
        &selector,
        Map::new(),
        "v2sdg007-missing-receipt",
        runtime::session_preconditions(&ready),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &complete_without_receipt)),
        (
            "DELEGATION_RECEIPT_REQUIRED".to_owned(),
            "receipt_missing".to_owned()
        )
    );

    let execution = json!({
        "binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1",
        "primary_host":"external", "scope_kind":"test", "scope_ref":"scope-1",
        "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
        "role_ref":"reviewer", "role_snapshot_digest":format!("sha256:{}", "a".repeat(64)),
        "agent_configuration_digest":format!("sha256:{}", "b".repeat(64)),
        "accepted_task_digest":format!("sha256:{}", "c".repeat(64)),
    });
    let acceptance_input = json!({
        "schema":"podway.delegation-accept-input/v1", "workspace_uuid":workspace_uuid,
        "session_id":session_id, "session_revision":ready["session"]["revision"],
        "attempt_id":attempt_id, "idempotency_key":"v2sdg007-accept",
        "package_digest":package_digest, "execution":execution, "rounds_used":2,
        "result_digest":format!("sha256:{}", "d".repeat(64)),
        "summary":"The final result was reviewed.", "worker_disposition":"idle", "accepted":true,
    });
    let mut stale_revision_input = acceptance_input.clone();
    stale_revision_input["session_revision"] = before["session"]["revision"].clone();
    stale_revision_input["idempotency_key"] = json!("v2sdg007-stale-revision");
    let stale_revision = runtime::request(
        771_035,
        "delegation.accept",
        &selector,
        fields(json!({"input":stale_revision_input})),
        "v2sdg007-stale-revision",
        runtime::session_preconditions(&before),
    );
    assert_eq!(
        error_code(runtime::dispatch(&dispatcher, &stale_revision)),
        "SESSION_REVISION_CONFLICT"
    );
    let mut wrong_binding = acceptance_input.clone();
    wrong_binding["execution"]["binding_ref"] = json!("other-binding");
    wrong_binding["idempotency_key"] = json!("v2sdg007-wrong-binding");
    let mismatch = runtime::request(
        771_009,
        "delegation.accept",
        &selector,
        fields(json!({"input":wrong_binding})),
        "v2sdg007-wrong-binding",
        runtime::session_preconditions(&ready),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &mismatch)),
        (
            "DELEGATION_BINDING_MISMATCH".to_owned(),
            "binding_mismatch".to_owned()
        )
    );
    for (number, key, field, value, reason) in [
        (
            771_026,
            "v2sdg007-wrong-package",
            "package_digest",
            json!(format!("sha256:{}", "e".repeat(64))),
            "binding_mismatch",
        ),
        (
            771_027,
            "v2sdg007-wrong-role",
            "role_ref",
            json!("other-role"),
            "role_mismatch",
        ),
        (
            771_038,
            "v2sdg007-wrong-role-digest",
            "role_snapshot_digest",
            json!(format!("sha256:{}", "e".repeat(64))),
            "role_mismatch",
        ),
        (
            771_028,
            "v2sdg007-too-many-rounds",
            "rounds_used",
            json!(4),
            "round_limit",
        ),
        (
            771_029,
            "v2sdg007-wrong-disposition",
            "worker_disposition",
            json!("released"),
            "disposition_mismatch",
        ),
    ] {
        let mut invalid = acceptance_input.clone();
        invalid["idempotency_key"] = json!(key);
        if field == "role_ref" || field == "role_snapshot_digest" {
            invalid["execution"][field] = value;
        } else {
            invalid[field] = value;
        }
        let request = runtime::request(
            number,
            "delegation.accept",
            &selector,
            fields(json!({"input":invalid})),
            key,
            runtime::session_preconditions(&ready),
        );
        assert_eq!(
            error_code_and_reason(runtime::dispatch(&dispatcher, &request)),
            ("DELEGATION_BINDING_MISMATCH".to_owned(), reason.to_owned()),
            "{field}"
        );
    }

    let mut acceptance_input = acceptance_input;
    acceptance_input["rounds_used"] = json!(3);
    let accept = runtime::request(
        771_010,
        "delegation.accept",
        &selector,
        fields(json!({"input":acceptance_input})),
        "v2sdg007-accept",
        runtime::session_preconditions(&ready),
    );
    let accepted = runtime::v2_result(runtime::dispatch(&dispatcher, &accept), "delegation.accept");
    assert_eq!(accepted["schema"], "podway.delegation-accept-result/v1");
    let replay = runtime::v2_result(runtime::dispatch(&dispatcher, &accept), "delegation.accept");
    assert_eq!(replay, accepted);
    let lookup = runtime::request(
        771_030,
        "job.lookup",
        &selector,
        fields(json!({"idempotency_key":"v2sdg007-accept"})),
        "unused-accept-lookup",
        PreconditionsV1::default(),
    );
    let found = runtime::v2_result(runtime::dispatch(&dispatcher, &lookup), "job.lookup");
    assert_eq!(found["found"], true);
    assert_eq!(found["schema"], "podway.job-lookup-result/v5");
    assert_eq!(found["job"]["command"], "delegation.accept");
    assert_eq!(found["job"]["state"], "succeeded");
    assert_eq!(
        found["job"]["terminal_response"]["result"],
        Value::Object(accepted.clone())
    );
    let wait = runtime::request(
        771_031,
        "job.wait",
        &selector,
        fields(json!({"job_id":found["job"]["id"]})),
        "unused-accept-wait",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(waited) = runtime::dispatch(&dispatcher, &wait) else {
        panic!("terminal delegation.accept job.wait must return podway.output/v3");
    };
    assert_eq!(waited.result()["schema"], "podway.job-result/v5");
    assert_eq!(waited.result()["job"]["command"], "delegation.accept");
    let after_accept = runtime::status(&dispatcher, &selector, 771_011, &session_id);
    assert_eq!(after_accept["delegation"]["state"], "accepted");

    let read = runtime::v2_result(
        runtime::dispatch(&dispatcher, &receipt),
        "delegation.receipt",
    );
    assert_eq!(read["schema"], "podway.delegation-receipt-result/v1");
    assert_eq!(read["applicability"], "accepted");
    let unknown_receipt = runtime::request(
        771_036,
        "delegation.receipt",
        &selector,
        fields(json!({"session_id":session_id,
            "attempt_id":"00000000-0000-4000-8000-000000000099"})),
        "unused-unknown-receipt",
        PreconditionsV1::default(),
    );
    assert_eq!(
        error_code(runtime::dispatch(&dispatcher, &unknown_receipt)),
        "ATTEMPT_NOT_FOUND"
    );
    let unknown_session = runtime::request(
        771_039,
        "delegation.receipt",
        &selector,
        fields(json!({"session_id":"00000000-0000-4000-8000-000000000099",
            "attempt_id":attempt_id})),
        "unused-unknown-session-receipt",
        PreconditionsV1::default(),
    );
    assert_eq!(
        error_code(runtime::dispatch(&dispatcher, &unknown_session)),
        "SESSION_NOT_FOUND"
    );

    let mut second_input = acceptance_input.clone();
    second_input["session_revision"] = after_accept["session"]["revision"].clone();
    second_input["idempotency_key"] = json!("v2sdg007-second-accept");
    let second = runtime::request(
        771_013,
        "delegation.accept",
        &selector,
        fields(json!({"input":second_input})),
        "v2sdg007-second-accept",
        runtime::session_preconditions(&after_accept),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &second)),
        (
            "DELEGATION_ALREADY_ACCEPTED".to_owned(),
            "duplicate_acceptance".to_owned()
        )
    );

    let uncheck = runtime::request(
        771_014,
        "item.uncheck",
        &selector,
        fields(json!({"item_id":"done"})),
        "v2sdg007-uncheck",
        runtime::item_preconditions(&after_accept, "done"),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &uncheck), "item.uncheck");
    let unchecked = runtime::status(&dispatcher, &selector, 771_015, &session_id);
    let recheck = runtime::request(
        771_016,
        "item.check",
        &selector,
        fields(json!({"item_id":"done"})),
        "v2sdg007-recheck",
        runtime::item_preconditions(&unchecked, "done"),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &recheck), "item.check");
    let stale = runtime::status(&dispatcher, &selector, 771_017, &session_id);
    assert_eq!(stale["delegation"]["state"], "stale_acceptance");
    let stale_read = runtime::v2_result(
        runtime::dispatch(&dispatcher, &receipt),
        "delegation.receipt",
    );
    assert_eq!(stale_read["applicability"], "stale_acceptance");
    let stale_complete = runtime::request(
        771_018,
        "session.complete",
        &selector,
        Map::new(),
        "v2sdg007-stale-complete",
        runtime::session_preconditions(&stale),
    );
    assert_eq!(
        error_code_and_reason(runtime::dispatch(&dispatcher, &stale_complete)),
        (
            "DELEGATION_RECEIPT_STALE".to_owned(),
            "items_changed".to_owned()
        )
    );
    let stable_package = runtime::v2_result(
        runtime::dispatch(&dispatcher, &package),
        "delegation.package",
    );
    assert_eq!(stable_package["package_digest"], package_digest);

    let retry = runtime::request(
        771_019,
        "session.retry",
        &selector,
        fields(json!({"reason":"Revalidate the edited result."})),
        "v2sdg007-retry",
        runtime::session_preconditions(&stale),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &retry), "session.retry");
    let fresh = runtime::status(&dispatcher, &selector, 771_020, &session_id);
    let fresh_attempt_id = fresh["current"]["attempt"]["attempt_id"].as_str().unwrap();
    assert_ne!(fresh_attempt_id, attempt_id);
    let fresh_package = runtime::v2_result(
        runtime::dispatch(&dispatcher, &package),
        "delegation.package",
    );
    assert_ne!(fresh_package["package_digest"], package_digest);
    let fresh_confirm = runtime::request(
        771_021,
        "item.check",
        &selector,
        fields(json!({"item_id":"done"})),
        "v2sdg007-fresh-check",
        runtime::item_preconditions(&fresh, "done"),
    );
    runtime::v2_result(runtime::dispatch(&dispatcher, &fresh_confirm), "item.check");
    let fresh_ready = runtime::status(&dispatcher, &selector, 771_022, &session_id);
    let mut fresh_input = acceptance_input.clone();
    fresh_input["session_revision"] = fresh_ready["session"]["revision"].clone();
    fresh_input["attempt_id"] = json!(fresh_attempt_id);
    fresh_input["idempotency_key"] = json!("v2sdg007-fresh-accept");
    fresh_input["package_digest"] = fresh_package["package_digest"].clone();
    let fresh_accept = runtime::request(
        771_023,
        "delegation.accept",
        &selector,
        fields(json!({"input":fresh_input})),
        "v2sdg007-fresh-accept",
        runtime::session_preconditions(&fresh_ready),
    );
    runtime::v2_result(
        runtime::dispatch(&dispatcher, &fresh_accept),
        "delegation.accept",
    );
    let fresh_accepted = runtime::status(&dispatcher, &selector, 771_024, &session_id);

    let complete = runtime::request(
        771_025,
        "session.complete",
        &selector,
        Map::new(),
        "v2sdg007-complete",
        runtime::session_preconditions(&fresh_accepted),
    );
    let completed = runtime::v2_result(
        runtime::dispatch(&dispatcher, &complete),
        "session.complete",
    );
    assert_eq!(completed["schema"], "podway.stage-transition-result/v2");
    let historical = runtime::v2_result(
        runtime::dispatch(&dispatcher, &receipt),
        "delegation.receipt",
    );
    assert_eq!(historical["applicability"], "historical");
    let completed_status = runtime::status(&dispatcher, &selector, 771_032, &session_id);
    let mut terminal_input = acceptance_input.clone();
    terminal_input["session_revision"] = completed_status["session"]["revision"].clone();
    terminal_input["attempt_id"] = json!(fresh_attempt_id);
    terminal_input["idempotency_key"] = json!("v2sdg007-terminal-accept");
    terminal_input["package_digest"] = fresh_package["package_digest"].clone();
    let terminal_revision =
        Revision::new(completed_status["session"]["revision"].as_u64().unwrap());
    let terminal_accept = runtime::request(
        771_037,
        "delegation.accept",
        &selector,
        fields(json!({"input":terminal_input})),
        "v2sdg007-terminal-accept",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            Some(terminal_revision),
            Some(podway_core::AttemptId::new(fresh_attempt_id).unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        error_code(runtime::dispatch(&dispatcher, &terminal_accept)),
        "ATTEMPT_NOT_CURRENT"
    );
    let terminal_fence = PreconditionsV1::new(
        Some(SessionId::new(&session_id).unwrap()),
        Some(terminal_revision),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let disposition = runtime::request(
        771_033,
        "session.terminal_disposition",
        &selector,
        fields(json!({"kind":"not_required", "reason":"No handoff is required."})),
        "v2sdg007-terminal-disposition",
        terminal_fence.clone(),
    );
    runtime::v2_result(
        runtime::dispatch(&dispatcher, &disposition),
        "session.terminal_disposition",
    );
    let archive = runtime::request(
        771_034,
        "session.archive",
        &selector,
        Map::new(),
        "v2sdg007-archive",
        terminal_fence,
    );
    let archived = runtime::v2_result(runtime::dispatch(&dispatcher, &archive), "session.archive");
    assert_eq!(archived["activity"], "inactive");
    let archived_receipt = runtime::v2_result(
        runtime::dispatch(&dispatcher, &receipt),
        "delegation.receipt",
    );
    assert_eq!(archived_receipt, historical);
}
