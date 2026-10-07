#!/usr/bin/env python3
"""Single-binary delegation qualification with a deterministic external executor."""

from __future__ import annotations

import argparse
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import uuid


def bounded_process(argv: list[str], *, cwd: Path | None = None,
                    payload: bytes | None = None, timeout: int = 30) -> subprocess.CompletedProcess:
    child = subprocess.Popen(argv, cwd=cwd, env={"PATH": "/usr/bin:/bin"},
                             stdin=subprocess.PIPE if payload is not None else subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    try:
        stdout, stderr = child.communicate(payload, timeout=timeout)
        assert len(stdout) <= 1024 * 1024 + 2 and len(stderr) <= 65536, "bounded child output"
        return subprocess.CompletedProcess(argv, child.returncode, stdout, stderr)
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
        child.wait(timeout=5)
        for stream in (child.stdin, child.stdout, child.stderr):
            if stream is not None:
                stream.close()


def digest(value: object) -> str:
    return "sha256:" + hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def procedure(disposition: str | None = "retain") -> dict:
    node = {"id": "work", "use": "work", "terminal": True}
    if disposition:
        node["delegation"] = {
            "binding_ref": "fixture-specialist", "role_ref": "reviewer",
            "role_snapshot_digest": digest("reviewer-v1"),
            "max_rounds": 3, "after_accept": disposition,
        }
    return {
        "schema": "podway.procedure/v2", "id": "delegation-qualification",
        "version": "1", "name": "Delegation qualification",
        "purpose": "Qualify native lifecycle without a live executor.",
        "manual_rework": {"allowed_targets": ["work"]},
        "node_definitions": {"work": {
            "type": "action", "title": "Work", "intent": "Record the final result.",
            "items": [
                {"id": "result", "type": "text", "prompt": "Final result", "required": True},
                {"id": "note", "type": "text", "prompt": "Optional note", "required": False},
            ],
        }},
        "graph": {"entry": "work", "nodes": [node]},
    }


def fixture_process(ledger: Path) -> None:
    """One disposable Primary process, resuming the external runtime's durable ledger."""
    request = json.load(sys.stdin)
    state = json.loads(ledger.read_text()) if ledger.exists() else {"assignments": [], "effects": 0}
    if request["operation"] == "assign":
        package = request["package"]
        execution = {
            "binding_ref": "fixture-specialist", "adapter_id": "deterministic-fixture",
            "adapter_version": "1", "primary_host": request["host"],
            "scope_kind": "qualification", "scope_ref": "scope-1",
            "supervisor_ref": "primary-1", "worker_ref": f"worker-{len(state['assignments']) + 1}",
            "task_ref": package["package"]["attempt_id"], "role_ref": "reviewer",
            "role_snapshot_digest": digest("reviewer-v1"),
            "agent_configuration_digest": digest({"model": "fixture", "instructions": "review-v1"}),
            "accepted_task_digest": digest(package),
        }
        state["assignments"].append({"execution": execution, "package": package,
                                     "feedback": ["correct", "accept"], "rounds_used": 3,
                                     "outcome": "unknown" if request.get("uncertain") else "accepted"})
        state["effects"] += 1
        ledger.write_text(json.dumps(state))
    elif request["operation"] == "reconcile":
        assert state["assignments"][-1]["outcome"] == "unknown"
        state["assignments"][-1]["outcome"] = "accepted"
        ledger.write_text(json.dumps(state))
    elif request["operation"] != "inspect":
        raise AssertionError("unsupported fixture operation")
    print(json.dumps(state))


class Fixture:
    def __init__(self, cli: Path, root: Path):
        self.cli, self.root = cli, root

    def call(self, worktree: Path, args: list[str], *, payload: dict | None = None,
             error: str | None = None) -> dict:
        assert worktree.is_relative_to(self.root) and worktree.is_dir()
        workspace = [] if args[0] in {"help", "version", "completions", "procedure", "preset"} else ["--worktree", str(worktree)]
        completed = bounded_process(
            [str(self.cli), "--json", *workspace, *args], cwd=worktree,
            payload=None if payload is None else json.dumps(payload).encode(),
        )
        assert not completed.stderr, (args, completed.stderr)
        envelope = json.loads(completed.stdout)
        if error:
            assert envelope.get("schema") == "podway.error/v2", envelope
            assert envelope.get("code") == error, (args, envelope, error)
            assert completed.returncode == envelope["exit_code"] != 0
        else:
            assert completed.returncode == 0, (args, envelope)
            assert envelope["schema"] == "podway.output/v4", envelope
        return envelope

    def workspace(self, name: str, disposition: str | None = "retain", *, goal: bool = False,
                  upstream: str | None = None) -> Path:
        worktree = self.root / name
        worktree.mkdir(mode=0o700)
        subprocess.run(["git", "init", "--quiet", str(worktree)], check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        self.call(worktree, ["init", "--idempotency-key", "init"])
        document = procedure(disposition)
        if goal:
            document["goal_tracking"] = True
            outcomes = {"achieved": "achieved", "not-achieved": "not_achieved", "superseded": "superseded"}
            document["node_definitions"]["assess"] = {
                "type": "decision", "title": "Assess", "objective": "Assess the goal.",
                "prompt": "What is the goal outcome?", "reason": {"required": True},
                "options": [{"id": key, "label": key} for key in outcomes],
                "assessment": {"target": "session_goal", "outcomes": outcomes},
            }
            document["node_definitions"]["finish"] = {
                "type": "action", "title": "Finish", "intent": "Finish the goal.",
            }
            entry = document["graph"]["nodes"][0]
            del entry["terminal"]
            entry["next"] = "assess"
            document["graph"]["nodes"] += [
                {"id": "assess", "use": "assess", "routes": {
                    key: {"to": "finish", "effect": "advance"} for key in outcomes}},
                {"id": "finish", "use": "finish", "terminal": True},
            ]
        if upstream:
            document["node_definitions"]["source"] = {
                "type": upstream, "title": "Source",
                "items": [{"id": "input", "type": "text", "prompt": "Input", "required": True}],
            }
            source = document["node_definitions"]["source"]
            source_node = {"id": "source", "use": "source"}
            evidence = {"node": "source", "required": True}
            if upstream == "decision":
                source.update(objective="Record upstream input.", prompt="Proceed?",
                              options=[{"id": "go", "label": "Go"}], reason={"required": True})
                source_node["routes"] = {"go": {"to": "work", "effect": "advance"}}
            else:
                source["intent"] = "Record upstream input."
                source_node["next"] = "work"
                evidence["items"] = ["input"]
            document["graph"]["nodes"][0]["evidence_from"] = [
                evidence,
            ]
            document["graph"]["entry"] = "source"
            document["graph"]["nodes"].insert(0, source_node)
        (worktree / "procedure.json").write_text(json.dumps(document))
        preview = self.call(worktree, ["procedure", "preview", "procedure.json"])["result"]
        assert preview["admissible"] is True, preview
        self.call(worktree, ["start", "--procedure", "procedure.json", "--expect-procedure-digest",
                            preview["procedure_digest"], "--task", name,
                            "--idempotency-key", f"start-{name}"])
        begin = ["begin"]
        if goal:
            begin += ["--goal", "Qualify fresh goal attempts", "--criterion", "proof=Native proof"]
        self.mutate(worktree, begin, attempt=False)
        if upstream:
            self.mutate(worktree, ["set", "input", "Immutable source input"], item="input")
            if upstream == "decision":
                self.mutate(worktree, ["decide", "--option", "go", "--reason", "Use the recorded input."])
            else:
                self.mutate(worktree, ["complete"])
        self.mutate(worktree, ["set", "result", "Final output"], item="result")
        return worktree

    def observe(self, worktree: Path) -> dict:
        return self.call(worktree, ["observe"])

    def fences(self, observation: dict, *, attempt: bool = True, item: str | None = None) -> list[str]:
        status = observation["result"]["status"]
        args = ["--if-workspace-uuid", observation["workspace"]["uuid"],
                "--if-session-id", status["session"]["id"]]
        if item is None:
            args += ["--if-session-revision", str(status["session"]["revision"])]
        if attempt:
            args += ["--if-attempt", status["current"]["attempt"]["attempt_id"]]
        if item:
            slot = next(v for v in observation["result"]["active_items"] if v["item_id"] == item)
            args += ["--if-item-revision", str(slot["revision"])]
        return args

    def mutate(self, worktree: Path, args: list[str], *, item: str | None = None,
               attempt: bool = True, error: str | None = None) -> dict:
        return self.call(worktree, [*args, *self.fences(self.observe(worktree), attempt=attempt, item=item),
                                   "--idempotency-key", str(uuid.uuid4())], error=error)

    def external(self, worktree: Path, operation: str, **fields: object) -> dict:
        completed = bounded_process(
            [sys.executable, str(Path(__file__).resolve()), "--fixture-ledger", str(worktree / "external.json")],
            payload=json.dumps({"operation": operation, **fields}).encode(), timeout=10,
        )
        assert completed.returncode == 0 and not completed.stderr, completed
        return json.loads(completed.stdout)

    def acceptance(self, worktree: Path, host: str = "external", disposition: str = "idle",
                   *, uncertain: bool = False) -> tuple[dict, dict]:
        package = self.call(worktree, ["delegation", "package"])["result"]
        state = self.external(worktree, "assign", package=package, host=host, uncertain=uncertain)
        observation = self.observe(worktree)
        status = observation["result"]["status"]
        assignment = state["assignments"][-1]
        return {
            "schema": "podway.delegation-accept-input/v1",
            "workspace_uuid": observation["workspace"]["uuid"],
            "session_id": status["session"]["id"], "session_revision": status["session"]["revision"],
            "attempt_id": status["current"]["attempt"]["attempt_id"],
            "idempotency_key": str(uuid.uuid4()), "package_digest": package["package_digest"],
            "execution": assignment["execution"], "rounds_used": assignment["rounds_used"],
            "result_digest": digest("Final output"), "summary": "Fixture Primary accepted after feedback.",
            "worker_disposition": disposition, "accepted": True,
        }, state

    def accept(self, worktree: Path, payload: dict, error: str | None = None) -> dict:
        return self.call(worktree, ["delegation", "accept", "--stdin"], payload=payload, error=error)

    def receipt(self, worktree: Path, payload: dict) -> dict:
        return self.call(worktree, ["delegation", "receipt", "--session", payload["session_id"],
                                   "--attempt", payload["attempt_id"]])["result"]

    def state(self, worktree: Path, expected: str) -> dict:
        observed = self.observe(worktree)
        assert observed["result"]["status"]["delegation"]["state"] == expected, observed
        return observed

    def lost(self, worktree: Path, args: list[str], payload: dict | None = None) -> dict:
        # Discard the consumer response and recover only through the durable outcome.
        assert worktree.is_relative_to(self.root) and worktree.is_dir()
        completed = bounded_process(
            [str(self.cli), "--json", "--worktree", str(worktree), *args], cwd=worktree,
            payload=None if payload is None else json.dumps(payload).encode(),
        )
        assert completed.returncode == 0 and not completed.stderr, completed
        key = payload["idempotency_key"] if payload is not None else args[args.index("--idempotency-key") + 1]
        lookup = self.call(worktree, ["outcome", "lookup", "--idempotency-key", key])["result"]
        assert lookup["found"] is True, lookup
        return lookup["outcome"]["response"]


def same_replay(original: dict, replay: dict) -> None:
    assert original == replay, (original, replay)


def decision_evidence(f: Fixture) -> None:
    worktree = f.workspace("decision-evidence", upstream="decision")
    package = f.call(worktree, ["delegation", "package"])["result"]
    evidence = package["package"]["evidence"][0]
    assert evidence["resolution"] == "resolved"
    assert evidence["selector"]["node"] == "source"
    assert len(evidence["items"]) == 1
    assert evidence["items"][0]["item_id"] == "input"
    assert evidence["items"][0]["state"] == "present"
    assert evidence["items"][0]["value_type"] == "text"
    payload, external = f.acceptance(worktree, "external")
    assert external["effects"] == 1
    assert payload["package_digest"] == package["package_digest"]
    f.accept(worktree, payload)
    f.mutate(worktree, ["complete"])
    assert f.receipt(worktree, payload)["applicability"] == "historical"


def host_matrix(f: Fixture, host: str) -> None:
    worktree = f.workspace(host, upstream="action")
    payload, external = f.acceptance(worktree, host)
    assert external["effects"] == 1 and len(external["assignments"][-1]["feedback"]) == 2
    for field, value, code in [
        ("role_ref", "incompatible", "DELEGATION_BINDING_MISMATCH"),
        ("role_snapshot_digest", digest("wrong-role"), "DELEGATION_BINDING_MISMATCH"),
    ]:
        invalid = deepcopy(payload)
        invalid["execution"][field] = value
        invalid["idempotency_key"] = str(uuid.uuid4())
        f.accept(worktree, invalid, code)
    over_budget = deepcopy(payload)
    over_budget.update(rounds_used=4, idempotency_key=str(uuid.uuid4()))
    f.accept(worktree, over_budget, "DELEGATION_BINDING_MISMATCH")
    accepted = f.accept(worktree, payload)
    receipt = f.receipt(worktree, payload)
    assert receipt["receipt"]["execution"] == payload["execution"], receipt
    assert receipt["receipt"]["rounds_used"] == 3
    assert receipt["receipt"]["worker_disposition"] == "idle"
    before = f.state(worktree, "accepted")["result"]["status"]
    for args in [["status"], ["next"], ["delegation", "package"],
                 ["evidence", "read", "--source", "source", "--item", "input"]]:
        f.call(worktree, args)
    f.mutate(worktree, ["set", "result", "Final output"], item="result")
    assert f.state(worktree, "accepted")["result"]["status"]["session"] == before["session"]
    # A late failure in an atomic batch must not commit the preceding valid item edit.
    observed = f.observe(worktree)
    failed_batch = {
        "schema": "podway.item-record-many-input/v1", "workspace_uuid": payload["workspace_uuid"],
        "session_id": payload["session_id"], "session_revision": before["session"]["revision"],
        "attempt_id": payload["attempt_id"], "idempotency_key": str(uuid.uuid4()),
        "operations": [
            {"item_id": "note", "expected_item_revision": 0, "record": {"type": "text", "value": "must roll back"}},
            {"item_id": "result", "expected_item_revision": 999, "record": {"type": "text", "value": "invalid fence"}},
        ],
    }
    f.call(worktree, ["record", "--stdin"], payload=failed_batch, error="ITEM_REVISION_CONFLICT")
    assert f.observe(worktree)["result"]["active_items"] == observed["result"]["active_items"]
    f.state(worktree, "accepted")
    f.mutate(worktree, ["block", "--reason", "External fixture temporarily unavailable"])
    f.state(worktree, "accepted")
    f.mutate(worktree, ["complete"], error="BLOCKERS_PRESENT")
    f.mutate(worktree, ["unblock", "--all"])
    f.state(worktree, "accepted")
    assert f.receipt(worktree, payload)["receipt"] == receipt["receipt"]
    # Cosmetic required-item correction and edit/revert both invalidate permanently.
    f.mutate(worktree, ["set", "result", "Final output "], item="result")
    f.state(worktree, "stale_acceptance")
    f.mutate(worktree, ["set", "result", "Final output"], item="result")
    f.state(worktree, "stale_acceptance")
    f.mutate(worktree, ["complete"], error="DELEGATION_RECEIPT_STALE")
    same_replay(accepted, f.accept(worktree, payload))
    lookup = f.call(worktree, ["outcome", "lookup", "--idempotency-key", payload["idempotency_key"]])["result"]
    assert lookup["outcome"]["response"] == accepted
    f.state(worktree, "stale_acceptance")
    changed = deepcopy(payload)
    changed["summary"] = "Changed input cannot repair acceptance"
    f.accept(worktree, changed, "IDEMPOTENCY_KEY_REUSED")
    duplicate = deepcopy(payload)
    duplicate.update(idempotency_key=str(uuid.uuid4()),
                     session_revision=f.observe(worktree)["result"]["status"]["session"]["revision"])
    f.accept(worktree, duplicate, "DELEGATION_ALREADY_ACCEPTED")
    f.mutate(worktree, ["retry", "--reason", "Explicit fresh attempt"])
    fresh = f.state(worktree, "awaiting_acceptance")["result"]["status"]
    assert fresh["current"]["attempt"]["attempt_id"] != payload["attempt_id"]
    assert f.receipt(worktree, payload)["receipt"] == receipt["receipt"]
    # Fresh ordinary fences cannot turn an old package into a new acceptance.
    relabeled = deepcopy(payload)
    relabeled.update(idempotency_key=str(uuid.uuid4()), session_revision=fresh["session"]["revision"],
                     attempt_id=fresh["current"]["attempt"]["attempt_id"])
    f.accept(worktree, relabeled, "DELEGATION_BINDING_MISMATCH")
    assert f.external(worktree, "inspect") == external, "Podway recovery redispatched external work"
    f.mutate(worktree, ["set", "result", "Final output"], item="result")
    second, state = f.acceptance(worktree, host)
    assert state["effects"] == 2
    f.accept(worktree, second)
    f.mutate(worktree, ["set", "note", "Optional correction"], item="note")
    f.state(worktree, "stale_acceptance")
    f.mutate(worktree, ["clear", "note"], item="note")
    f.state(worktree, "stale_acceptance")
    f.mutate(worktree, ["rework", "--to", "work", "--reason", "Explicit rework"])
    f.state(worktree, "awaiting_acceptance")
    assert f.receipt(worktree, second)["receipt"]["attempt_id"] == second["attempt_id"]
    assert f.external(worktree, "inspect") == state

    released = f.workspace(host + "-released", "release")
    release_input, _ = f.acceptance(released, host, "released")
    invalid = deepcopy(release_input)
    invalid.update(worker_disposition="idle", idempotency_key=str(uuid.uuid4()))
    f.accept(released, invalid, "DELEGATION_BINDING_MISMATCH")
    f.accept(released, release_input)
    assert f.receipt(released, release_input)["receipt"]["worker_disposition"] == "released"
    f.mutate(released, ["complete"])
    assert f.observe(released)["result"]["status"]["session"]["lifecycle"] == "completed"


def documented_example(f: Fixture) -> dict:
    """Exercise the published YAML bytes, not a reconstructed example graph."""
    source = Path(__file__).resolve().parent.parent / "docs/examples/procedures/supervised-review-v2.yaml"
    original = source.read_bytes()
    worktree = f.root / "documented-example"
    worktree.mkdir(mode=0o700)
    subprocess.run(["git", "init", "--quiet", str(worktree)], check=True,
                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    f.call(worktree, ["init", "--idempotency-key", "init"])
    relative = ".podway/procedures/supervised-review-v2.yaml"
    target = worktree / relative
    target.parent.mkdir(exist_ok=True)
    target.write_bytes(original)
    formatted = f.call(worktree, ["procedure", "format", relative, "--check"])["result"]
    assert formatted["changed"] is False
    assert formatted["document"].encode() == original
    for operation, flags in (("validate", []), ("vet", []),
                             ("lint", ["--warnings-as-errors"]), ("check", ["--warnings-as-errors"])):
        result = f.call(worktree, ["procedure", operation, relative, *flags])["result"]
        assert result["valid"] is True, result
    preview = f.call(worktree, ["procedure", "preview", relative])["result"]
    assert preview["admissible"] is True, preview
    f.call(worktree, ["start", "--procedure", relative, "--expect-procedure-digest",
                      preview["procedure_digest"], "--task", "Documented supervised review",
                      "--idempotency-key", "start-documented-example"])
    f.mutate(worktree, ["begin"], attempt=False)
    f.mutate(worktree, ["set", "input", "Review the bounded change against its acceptance criteria."], item="input")
    f.mutate(worktree, ["complete"])
    package = f.call(worktree, ["delegation", "package"])["result"]
    assert package["package"]["placement_id"] == "work", package
    evidence = package["package"]["evidence"][0]
    assert evidence["selector"] == {"node": "source", "required": True, "items": ["input"]}, evidence
    readback = f.call(worktree, ["evidence", "read", "--source", "source", "--item", "input"])["result"]
    assert readback["truncated"] is False, readback
    f.mutate(worktree, ["set", "result", "Final output"], item="result")
    f.mutate(worktree, ["complete"], error="DELEGATION_RECEIPT_REQUIRED")
    payload, external = f.acceptance(worktree)
    assert payload["package_digest"] == package["package_digest"]
    for kind in ("role", "package", "rounds"):
        invalid = deepcopy(payload)
        invalid["idempotency_key"] = str(uuid.uuid4())
        if kind == "role":
            invalid["execution"]["role_ref"] = "wrong-role"
        elif kind == "package":
            invalid["package_digest"] = digest("wrong-package")
        else:
            invalid["rounds_used"] = 4
        f.accept(worktree, invalid, "DELEGATION_BINDING_MISMATCH")
        f.state(worktree, "awaiting_acceptance")
    before = f.observe(worktree)["result"]["status"]["current"]
    accepted = f.accept(worktree, payload)
    after = f.state(worktree, "accepted")["result"]["status"]["current"]
    assert after["node"] == before["node"] and after["attempt"] == before["attempt"]
    assert before["readiness"]["can_advance"] is False
    assert after["readiness"]["can_advance"] is True
    f.mutate(worktree, ["set", "note", "A clarification after acceptance"], item="note")
    f.state(worktree, "stale_acceptance")
    f.mutate(worktree, ["complete"], error="DELEGATION_RECEIPT_STALE")
    same_replay(accepted, f.accept(worktree, payload))
    f.state(worktree, "stale_acceptance")
    for transition in (["retry", "--reason", "Revalidate the corrected review"],
                       ["rework", "--to", "work", "--reason", "Revalidate through the declared route"]):
        previous_attempt = f.observe(worktree)["result"]["status"]["current"]["attempt"]["attempt_id"]
        f.mutate(worktree, transition)
        current = f.state(worktree, "awaiting_acceptance")["result"]["status"]
        assert current["current"]["attempt"]["attempt_id"] != previous_attempt
        assert f.external(worktree, "inspect") == external, "Podway redispatched example work"
        f.mutate(worktree, ["set", "result", "Final output"], item="result")
        relabeled = deepcopy(payload)
        relabeled.update(idempotency_key=str(uuid.uuid4()),
                         session_revision=f.observe(worktree)["result"]["status"]["session"]["revision"],
                         attempt_id=current["current"]["attempt"]["attempt_id"])
        f.accept(worktree, relabeled, "DELEGATION_BINDING_MISMATCH")
    fresh, _ = f.acceptance(worktree)
    f.accept(worktree, fresh)
    f.mutate(worktree, ["complete"])
    assert f.receipt(worktree, fresh)["applicability"] == "historical"
    assert [item["item_id"] for item in f.observe(worktree)["result"]["active_items"]] == ["closeout"]
    f.call(worktree, ["evidence", "read", "--source", "work", "--item", "result"])
    f.mutate(worktree, ["set", "closeout", "The fixture review is complete; no live executor was used."], item="closeout")
    f.mutate(worktree, ["complete"])
    assert f.observe(worktree)["result"]["status"]["session"]["lifecycle"] == "completed"
    assert source.read_bytes() == target.read_bytes() == original
    return {"source": source.relative_to(source.parents[3]).as_posix(),
            "source_sha256": hashlib.sha256(original).hexdigest(),
            "procedure_digest": preview["procedure_digest"]}


def recovery(f: Fixture) -> None:
    worktree = f.workspace("recovery", goal=True)
    payload, state = f.acceptance(worktree, uncertain=True)
    # A new Primary process resumes the exact persisted assignment without issuing work.
    assert f.external(worktree, "inspect") == state
    assert state["assignments"][-1]["outcome"] == "unknown"
    f.state(worktree, "awaiting_acceptance")
    f.mutate(worktree, ["complete"], error="DELEGATION_RECEIPT_REQUIRED")
    direct = f.workspace("direct", None)
    f.mutate(direct, ["retry", "--reason", "Direct retry while delegated executor is unknown"])
    f.mutate(direct, ["set", "result", "Final output"], item="result")
    f.mutate(direct, ["complete"])
    assert f.observe(direct)["result"]["status"]["session"]["lifecycle"] == "completed"
    # A fresh Primary process reconciles the same external outcome before submission.
    f.state(worktree, "awaiting_acceptance")
    reconciled = f.external(worktree, "reconcile")
    assert reconciled["effects"] == state["effects"]
    assert reconciled["assignments"][-1]["execution"] == payload["execution"]
    state = reconciled
    original = f.lost(worktree, ["delegation", "accept", "--stdin"], payload)
    assert original["command"] == "delegation.accept" and "result" in original, original
    lookup = f.call(worktree, ["outcome", "lookup", "--idempotency-key", payload["idempotency_key"]])["result"]
    assert lookup["outcome"]["response"] == original
    f.mutate(worktree, ["set", "note", "Correction after lost response"], item="note")
    same_replay(original, f.accept(worktree, payload))
    f.state(worktree, "stale_acceptance")
    observed = f.observe(worktree)
    revision = observed["result"]["status"]["goal_revision"]
    f.mutate(worktree, ["goal", "revise", "--goal", "Revalidate corrected result",
                        "--criterion", "proof=Native proof", "--rework-to", "work",
                        "--reason", "Correction", "--if-goal-revision", str(revision)])
    f.state(worktree, "awaiting_acceptance")
    assert f.receipt(worktree, payload)["receipt"]["attempt_id"] == payload["attempt_id"]
    late = deepcopy(payload)
    late["idempotency_key"] = str(uuid.uuid4())
    f.accept(worktree, late, "SESSION_REVISION_CONFLICT")
    assert f.external(worktree, "inspect") == state
    # Complete-response loss is independent of acceptance loss and must not repeat effects.
    ready = f.workspace("complete-loss")
    current, external = f.acceptance(ready)
    f.accept(ready, current)
    args = ["complete", *f.fences(f.observe(ready)), "--idempotency-key", "complete-loss"]
    f.state(ready, "accepted")
    completed = f.lost(ready, args)
    lookup = f.call(ready, ["outcome", "lookup", "--idempotency-key", "complete-loss"])["result"]
    assert lookup["outcome"]["response"] == completed
    same_replay(completed, f.call(ready, args))
    assert f.observe(ready)["result"]["status"]["session"]["lifecycle"] == "completed"
    assert f.external(ready, "inspect") == external


def terminate_qualification(signum: int, _frame: object) -> None:
    # Unwind bounded child cleanup and the owning temporary directory.
    raise SystemExit(128 + signum)


def main() -> None:
    if not __debug__:
        raise RuntimeError("qualification requires Python assertions to be enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--podway", type=Path)
    parser.add_argument("--fixture-ledger", type=Path)
    parser.add_argument("--scenario", choices=("all", "example", "hosts", "recovery"), default="all")
    args = parser.parse_args()
    if args.fixture_ledger:
        fixture_process(args.fixture_ledger)
        return
    signal.signal(signal.SIGTERM, terminate_qualification)
    assert args.podway, "the native CLI is required"
    supplied = args.podway.absolute()
    assert supplied.is_absolute() and not supplied.is_symlink() and supplied.is_file()
    original_digest = hashlib.sha256(supplied.read_bytes()).hexdigest()
    checks = {}
    example = None
    with tempfile.TemporaryDirectory(prefix="pw-sdg-", dir=tempfile.gettempdir()) as directory:
        root = Path(directory)
        cli = root / "podway"
        shutil.copy2(supplied, cli)
        cli.chmod(0o500)
        assert hashlib.sha256(cli.read_bytes()).hexdigest() == original_digest
        f = Fixture(cli, root)
        identity_output = bounded_process([str(cli), "version", "--json", "--identity"], cwd=root)
        assert identity_output.returncode == 0 and not identity_output.stderr
        identity_envelope = json.loads(identity_output.stdout)
        identity = identity_envelope["result"]
        assert identity_envelope["schema"] == "podway.output/v4", identity_envelope
        assert identity["schema"] == "podway.version-result/v2", identity
        assert identity["contract_manifest_schema"] == "podway.contract-manifest/v2", identity
        assert "supported_ipc_ids" not in identity, identity
        if args.scenario in ("all", "example"):
            example = documented_example(f)
            checks["example"] = True
        if args.scenario in ("all", "hosts"):
            for host in ("external", "managed"):
                host_matrix(f, host)
                checks[host] = True
            decision_evidence(f)
            checks["decision_evidence"] = True
        if args.scenario in ("all", "recovery"):
            recovery(f)
            checks["recovery"] = True
        assert hashlib.sha256(cli.read_bytes()).hexdigest() == original_digest
    assert hashlib.sha256(supplied.read_bytes()).hexdigest() == original_digest
    print(json.dumps({"schema": "podway.direct-delegation-qualification/v2", "checks": checks,
                      "binaries": {"podway": original_digest}, "contract_identity": identity, "example": example,
                      "runtime_cleanup": True, "live_adapter": "not_performed",
                      "live_model": "not_performed"}, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
