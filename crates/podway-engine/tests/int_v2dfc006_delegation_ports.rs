//! Retained delegation routes through real typed Engine execution.

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::{AttemptId, ItemId, SessionId, canonicalize_json_v1};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1},
    direct_items::DirectItemCommandV1,
    direct_sessions::DirectSessionCommandV1,
};
use podway_protocol::{
    DelegationAcceptV1, ItemCheckV1, ItemMutationPreconditionsWireV1, ItemUncheckV1,
    ProcedureV2MutationCommandV1, SessionArchiveV1, SessionIdentityPreconditionsWireV1,
    SessionTerminalDispositionV1, TerminalDispositionInputV1,
};
use serde_json::{Value, json};
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

fn identity(engine: &Engine) -> SessionIdentityPreconditionsWireV1 {
    let graph = engine.graph();
    SessionIdentityPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
    }
}
fn read(engine: &Engine, command: DirectReadCommandV1) -> Value {
    engine.success(
        DirectCommandV1::Read {
            command,
            expected_session_id: None,
        },
        "unused-read",
    )["result"]
        .clone()
}
fn reject(
    engine: &Engine,
    command: DirectCommandV1,
    key: &str,
    code: &str,
    reason: Option<&str>,
) -> Value {
    let before = engine
        .store
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap();
    let sequence = engine.sequence();
    let request = engine.request(command, key);
    let response = if matches!(&request.command, DirectCommandV1::Read { .. }) {
        engine.execute(request, &Clock)
    } else {
        engine.execute(request, &NoClock)
    };
    let DirectResponseV1::Failure(error) = response else {
        panic!("expected {code}")
    };
    let value = error.into_value();
    assert_eq!(value["code"], code);
    if let Some(reason) = reason {
        assert_eq!(value["details"]["reason"], reason);
    }
    assert_eq!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap(),
        before
    );
    assert_eq!(engine.sequence(), sequence);
    assert!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot
                .outcome(&podway_store::IdempotencyKeyV1::new(key).unwrap()))
            .unwrap()
            .is_none()
    );
    value
}
fn check(engine: &Engine, checked: bool, key: &str) {
    let graph = engine.graph();
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    let revision = graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap()
        .item_slots()
        .iter()
        .find(|slot| slot.item_id().as_str() == "done")
        .unwrap()
        .revision();
    let fences = ItemMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_attempt_id: attempt.clone(),
        expected_item_revision: revision,
    };
    let command = if checked {
        DirectItemCommandV1::Check(ItemCheckV1 {
            item_id: ItemId::new("done").unwrap(),
            preconditions: fences,
        })
    } else {
        DirectItemCommandV1::Uncheck(ItemUncheckV1 {
            item_id: ItemId::new("done").unwrap(),
            preconditions: fences,
        })
    };
    let request = engine.request(DirectCommandV1::Item(command), key);
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
}
fn input(engine: &Engine, package: &Value, key: &str, disposition: &str, rounds: u64) -> Value {
    let graph = engine.graph();
    let digest = |letter: char| format!("sha256:{}", letter.to_string().repeat(64));
    json!({"schema":"podway.delegation-accept-input/v1",
        "workspace_uuid":engine.store.read(deadline(),|snapshot|Ok(snapshot.binding().identity().workspace_uuid().clone())).unwrap(),
        "session_id":graph.trace().session_id(),"session_revision":graph.trace().revision(),
        "attempt_id":graph.trace().active_attempt().unwrap().attempt_id(),"idempotency_key":key,
        "package_digest":package["package_digest"],"execution":{"binding_ref":"local-specialist",
            "adapter_id":"fixture","adapter_version":"1","primary_host":"external","scope_kind":"test",
            "scope_ref":"scope-1","supervisor_ref":"supervisor-1","worker_ref":"worker-1","task_ref":"task-1",
            "role_ref":"reviewer","role_snapshot_digest":digest('a'),"agent_configuration_digest":digest('b'),
            "accepted_task_digest":digest('c')},"rounds_used":rounds,"result_digest":digest('d'),
        "summary":"The final result was reviewed.","worker_disposition":disposition,"accepted":true})
}
fn accept(engine: &Engine, input: Value) -> DirectCommandV1 {
    DirectCommandV1::DelegationAccept(DelegationAcceptV1 {
        input,
        preconditions: engine.fences(),
    })
}
fn archive(engine: &Engine) {
    engine.success(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
            SessionTerminalDispositionV1 {
                disposition: TerminalDispositionInputV1::NotRequired {
                    reason: "No external handoff.".into(),
                },
                actor: None,
                preconditions: identity(engine),
            },
        )),
        "disposition",
    );
    let output = engine.success(
        DirectCommandV1::Session(DirectSessionCommandV1::Archive(SessionArchiveV1 {
            preconditions: identity(engine),
        })),
        "archive",
    );
    assert_eq!(output["result"]["activity"], "inactive");
}

#[test]
fn v2sdg007_delegated_package_uses_recorded_source_evidence() {
    let mut engine = Engine::new(EVIDENCE_PROCEDURE);
    let source = engine.fences().expected_attempt_id;
    engine.set("result", "source proof", "source-result");
    engine.complete("source");
    let package = read(&engine, DirectReadCommandV1::DelegationPackage);
    let evidence = &package["package"]["evidence"][0];
    assert_eq!(evidence["resolution"], "resolved");
    assert_eq!(evidence["source_attempt_id"], json!(source));
    assert_eq!(evidence["source_attempt_number"], 1);
    assert_eq!(evidence["selector"]["node"], "source");
    let value = canonicalize_json_v1(&json!({"kind":"text","value":"source proof"})).unwrap();
    assert_eq!(evidence["items"][0]["state"], "present");
    assert_eq!(evidence["items"][0]["item_id"], "result");
    assert_eq!(evidence["items"][0]["item_revision"], 1);
    assert_eq!(evidence["items"][0]["value_type"], "text");
    assert_eq!(evidence["items"][0]["size_bytes"], value.len());
    assert_eq!(
        evidence["items"][0]["value_digest"],
        format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
    );
    let mut hash = Sha256::new();
    hash.update(b"podway.delegation-work-package/v1\n");
    hash.update(
        canonicalize_json_v1(&package["package"])
            .unwrap()
            .as_bytes(),
    );
    assert_eq!(
        package["package_digest"],
        format!("sha256:{:x}", hash.finalize())
    );
    engine.cold();
    assert_eq!(
        read(&engine, DirectReadCommandV1::DelegationPackage),
        package
    );
    let request = engine.request(
        accept(&engine, input(&engine, &package, "accept", "idle", 1)),
        "accept",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    engine.complete("complete");
    assert_eq!(
        read(
            &engine,
            DirectReadCommandV1::DelegationReceipt {
                session_id: identity(&engine).expected_session_id,
                attempt_id: engine
                    .graph()
                    .trace()
                    .attempts()
                    .last()
                    .unwrap()
                    .attempt_id()
                    .clone()
            }
        )["applicability"],
        "historical"
    );
}

fn receipt_command(session_id: SessionId, attempt_id: AttemptId) -> DirectCommandV1 {
    DirectCommandV1::Read {
        command: DirectReadCommandV1::DelegationReceipt {
            session_id,
            attempt_id,
        },
        expected_session_id: None,
    }
}

fn complete_command(engine: &Engine) -> DirectCommandV1 {
    DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(
        podway_protocol::SessionCompleteV1 {
            preconditions: engine.fences(),
        },
    ))
}

#[test]
fn v2sdg007_direct_attempt_rejects_delegation_reads_without_changing_direct_work() {
    let mut engine = Engine::new(DIRECT_PROCEDURE);
    let fences = engine.fences();
    let package_command = DirectCommandV1::Read {
        command: DirectReadCommandV1::DelegationPackage,
        expected_session_id: None,
    };
    reject(
        &engine,
        package_command,
        "direct-package",
        "DELEGATION_NOT_APPLICABLE",
        Some("direct_attempt"),
    );
    let receipt = receipt_command(
        fences.expected_session_id.clone(),
        fences.expected_attempt_id.clone(),
    );
    reject(
        &engine,
        receipt.clone(),
        "direct-receipt",
        "DELEGATION_NOT_APPLICABLE",
        Some("direct_attempt"),
    );
    let package = json!({"package_digest":format!("sha256:{}","a".repeat(64))});
    reject(
        &engine,
        accept(
            &engine,
            input(&engine, &package, "direct-accept", "idle", 1),
        ),
        "direct-accept",
        "DELEGATION_NOT_APPLICABLE",
        Some("direct_attempt"),
    );
    engine.complete("direct-complete");
    archive(&engine);
    engine.cold();
    reject(
        &engine,
        receipt,
        "archived-direct-receipt",
        "DELEGATION_NOT_APPLICABLE",
        Some("direct_attempt"),
    );
}

#[test]
fn v2sdg007_release_declaration_requires_released_worker() {
    let mut engine =
        Engine::new(&PROCEDURE.replace("after_accept: retain", "after_accept: release"));
    check(&engine, true, "confirm");
    let package = read(&engine, DirectReadCommandV1::DelegationPackage);
    reject(
        &engine,
        accept(
            &engine,
            input(&engine, &package, "wrong-disposition", "idle", 3),
        ),
        "wrong-disposition",
        "DELEGATION_BINDING_MISMATCH",
        Some("disposition_mismatch"),
    );
    let fences = engine.fences();
    let request = engine.request(
        accept(&engine, input(&engine, &package, "released", "released", 3)),
        "released",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    let receipt = read(
        &engine,
        DirectReadCommandV1::DelegationReceipt {
            session_id: fences.expected_session_id,
            attempt_id: fences.expected_attempt_id,
        },
    );
    assert_eq!(receipt["receipt"]["worker_disposition"], "released");
    assert_eq!(receipt["receipt"]["rounds_used"], 3);
}

#[test]
fn v2sdg007_native_delegation_round_trip_and_closed_failures() {
    let mut engine = Engine::new(PROCEDURE);
    let original = engine.fences();
    let receipt = receipt_command(
        original.expected_session_id.clone(),
        original.expected_attempt_id.clone(),
    );
    reject(
        &engine,
        receipt.clone(),
        "missing-receipt",
        "DELEGATION_RECEIPT_REQUIRED",
        Some("receipt_missing"),
    );
    let package = read(&engine, DirectReadCommandV1::DelegationPackage);
    assert_eq!(package["schema"], "podway.delegation-package-result/v1");
    assert_eq!(
        package["package"]["attempt_id"],
        json!(original.expected_attempt_id)
    );
    check(&engine, true, "confirm");
    reject(
        &engine,
        complete_command(&engine),
        "missing-completion-receipt",
        "DELEGATION_RECEIPT_REQUIRED",
        Some("receipt_missing"),
    );
    let accepted_input = input(&engine, &package, "accept", "idle", 3);
    let mut stale = accepted_input.clone();
    stale["idempotency_key"] = json!("stale-accept");
    stale["session_revision"] = json!(original.expected_session_revision);
    let command = DirectCommandV1::DelegationAccept(DelegationAcceptV1 {
        input: stale,
        preconditions: original.clone(),
    });
    reject(
        &engine,
        command,
        "stale-accept",
        "SESSION_REVISION_CONFLICT",
        None,
    );
    for (key, field, value, reason) in [
        (
            "wrong-binding",
            "binding_ref",
            json!("other-binding"),
            "binding_mismatch",
        ),
        (
            "wrong-package",
            "package_digest",
            json!(format!("sha256:{}", "e".repeat(64))),
            "binding_mismatch",
        ),
        (
            "wrong-role",
            "role_ref",
            json!("other-role"),
            "role_mismatch",
        ),
        (
            "wrong-role-digest",
            "role_snapshot_digest",
            json!(format!("sha256:{}", "e".repeat(64))),
            "role_mismatch",
        ),
        ("too-many-rounds", "rounds_used", json!(4), "round_limit"),
        (
            "wrong-disposition",
            "worker_disposition",
            json!("released"),
            "disposition_mismatch",
        ),
    ] {
        let mut invalid = accepted_input.clone();
        invalid["idempotency_key"] = json!(key);
        if matches!(field, "binding_ref" | "role_ref" | "role_snapshot_digest") {
            invalid["execution"][field] = value;
        } else {
            invalid[field] = value;
        }
        reject(
            &engine,
            accept(&engine, invalid),
            key,
            "DELEGATION_BINDING_MISMATCH",
            Some(reason),
        );
    }
    let accept_request = engine.request(accept(&engine, accepted_input), "accept");
    let once = engine.output(engine.execute(accept_request.clone(), &Clock));
    assert_eq!(
        once["result"]["schema"],
        "podway.delegation-accept-result/v2"
    );
    engine.cold();
    assert_eq!(
        engine.output(engine.execute(accept_request, &NoClock)),
        once
    );
    let lookup = engine.success(
        DirectCommandV1::OutcomeLookup {
            key: podway_store::IdempotencyKeyV1::new("accept").unwrap(),
        },
        "unused-lookup",
    );
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], once);
    let after = engine.read(DirectReadCommandV1::Status {
        tier: podway_engine::v2_read_service::GraphStatusTierV2::Standard,
        history_before: None,
    });
    assert_eq!(after["delegation"]["state"], "accepted");
    assert_eq!(
        engine.success(receipt.clone(), "unused-receipt")["result"]["applicability"],
        "accepted"
    );
    let unknown = "00000000-0000-4000-8000-000000000099";
    reject(
        &engine,
        receipt_command(
            original.expected_session_id.clone(),
            AttemptId::new(unknown).unwrap(),
        ),
        "unknown-attempt",
        "ATTEMPT_NOT_FOUND",
        None,
    );
    reject(
        &engine,
        receipt_command(
            SessionId::new(unknown).unwrap(),
            original.expected_attempt_id.clone(),
        ),
        "unknown-session",
        "SESSION_NOT_FOUND",
        None,
    );
    reject(
        &engine,
        accept(&engine, input(&engine, &package, "duplicate", "idle", 3)),
        "duplicate",
        "DELEGATION_ALREADY_ACCEPTED",
        Some("duplicate_acceptance"),
    );
    check(&engine, false, "uncheck");
    check(&engine, true, "recheck");
    let stale = engine.read(DirectReadCommandV1::Status {
        tier: podway_engine::v2_read_service::GraphStatusTierV2::Standard,
        history_before: None,
    });
    assert_eq!(stale["delegation"]["state"], "stale_acceptance");
    assert_eq!(
        engine.success(receipt.clone(), "unused-stale-receipt")["result"]["applicability"],
        "stale_acceptance"
    );
    reject(
        &engine,
        complete_command(&engine),
        "stale-complete",
        "DELEGATION_RECEIPT_STALE",
        Some("items_changed"),
    );
    assert_eq!(
        read(&engine, DirectReadCommandV1::DelegationPackage)["package_digest"],
        package["package_digest"]
    );
    engine.success(
        engine.retry("Revalidate the edited result.".into()),
        "retry",
    );
    let fresh = engine.fences();
    assert_ne!(fresh.expected_attempt_id, original.expected_attempt_id);
    let fresh_package = read(&engine, DirectReadCommandV1::DelegationPackage);
    assert_ne!(fresh_package["package_digest"], package["package_digest"]);
    check(&engine, true, "fresh-confirm");
    let request = engine.request(
        accept(
            &engine,
            input(&engine, &fresh_package, "fresh-accept", "idle", 3),
        ),
        "fresh-accept",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    let mut terminal_input = input(&engine, &fresh_package, "terminal-accept", "idle", 3);
    engine.complete("complete");
    let terminal_graph = engine.graph();
    terminal_input["session_revision"] = json!(terminal_graph.trace().revision());
    let terminal_fences = podway_protocol::SessionMutationPreconditionsWireV1 {
        expected_session_id: fresh.expected_session_id,
        expected_session_revision: terminal_graph.trace().revision(),
        expected_attempt_id: fresh.expected_attempt_id,
    };
    reject(
        &engine,
        DirectCommandV1::Read {
            command: DirectReadCommandV1::DelegationPackage,
            expected_session_id: None,
        },
        "terminal-package",
        "SESSION_NOT_RUNNING",
        None,
    );
    let historical = engine.success(receipt.clone(), "unused-historical")["result"].clone();
    assert_eq!(historical["applicability"], "historical");
    reject(
        &engine,
        DirectCommandV1::DelegationAccept(DelegationAcceptV1 {
            input: terminal_input,
            preconditions: terminal_fences,
        }),
        "terminal-accept",
        "ATTEMPT_NOT_CURRENT",
        None,
    );
    archive(&engine);
    engine.cold();
    assert_eq!(
        engine.success(receipt, "unused-archived")["result"],
        historical
    );
}
