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
            "task_ref": (package["package"]["target"]["attempt"]
                         if "target" in package["package"] else package["package"]["attempt_id"]),
            "role_ref": "reviewer",
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

    def human(self, worktree: Path, args: list[str], expected: list[str],
              *, payload: dict | None = None) -> None:
        assert worktree.is_relative_to(self.root) and worktree.is_dir()
        completed = bounded_process(
            [str(self.cli), "--worktree", str(worktree), *args], cwd=worktree,
            payload=None if payload is None else json.dumps(payload).encode(),
        )
        assert completed.returncode == 0 and not completed.stderr, completed
        text = completed.stdout.decode()
        assert "podway.error" not in text and "error:" not in text.lower(), text
        for value in expected:
            assert value in text, (args, value, text)

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


class ScopedFixture(Fixture):
    """Exercise the scoped CLI in a disposable worktree."""

    def workspace(self, name: str, disposition: str | None = "retain", *,
                  goal: bool = False, upstream: str | None = None,
                  example: Path | None = None) -> Path:
        root = self.root / name
        root.mkdir(mode=0o700)
        checked = bounded_process(["/usr/bin/git", "init", "--quiet", str(root)])
        assert checked.returncode == 0, checked
        self.call(root, ["init", "--idempotency-key", "init"])
        source = root / "procedure.json"
        if example:
            source = root / "procedure.yaml"
            source.write_bytes(example.read_bytes())
        else:
            document = procedure(disposition)
            document["schema"] = "podway.procedure/v3"
            node = document["graph"]["nodes"][0]
            del node["terminal"]
            node["next"] = "plan"
            document["node_definitions"].update({
                "plan": {"type": "action", "title": "Plan", "intent": "Record the design anchor."},
                "finish": {"type": "action", "title": "Finish", "intent": "Finish the fixture."},
            })
            document["graph"]["nodes"].extend([
                {"id": "plan", "use": "plan", "next": "finish"},
                {"id": "finish", "use": "finish", "terminal": True},
            ])
            document["execution_group"] = {
                "anchor": "plan", "success_return": "finish", "plan_inputs": [],
                "result_items": [], "replan_decisions": [],
            }
            if goal:
                document["goal_tracking"] = True
                outcomes = {"achieved":"achieved", "not-achieved":"not_achieved", "superseded":"superseded"}
                document["node_definitions"]["assess"] = {
                    "type":"decision", "title":"Assess", "objective":"Assess the current goal.",
                    "prompt":"Which outcome applies?", "reason":{"required":True},
                    "options":[{"id":option,"label":option} for option in outcomes],
                    "assessment":{"target":"session_goal","outcomes":outcomes},
                }
                plan = document["graph"]["nodes"][1]
                plan["next"] = "assess"
                document["graph"]["nodes"].insert(2,{
                    "id":"assess","use":"assess",
                    "routes":{option:{"to":"finish","effect":"advance"} for option in outcomes},
                })
                document["execution_group"]["success_return"] = "assess"
            if upstream:
                upstream_item = {"id": "input", "type": "text", "prompt": "Input", "required": True}
                source_def = {"type": upstream, "title": "Source", "items": [upstream_item]}
                source_node = {"id": "source", "use": "source"}
                if upstream == "decision":
                    source_def.update(objective="Settle the source.", prompt="Proceed?",
                                      options=[{"id": "go", "label": "Go"}], reason={"required": True})
                    source_node["routes"] = {"go": {"to": "work", "effect": "advance"}}
                else:
                    source_def["intent"] = "Record the source."
                    source_node["next"] = "work"
                document["node_definitions"]["source"] = source_def
                document["graph"]["entry"] = "source"
                document["graph"]["nodes"].insert(0, source_node)
                node["evidence_from"] = [{"scope":"local", "node":"source", "required":True, "items":["input"]}]
            source.write_text(json.dumps(document))
        preview = self.call(root, ["procedure", "preview", source.name])["result"]
        self.call(root, ["start", "--procedure", source.name, "--expect-procedure-digest",
                         preview["digest"], "--task", name, "--idempotency-key", "start"])
        begin = ["begin"]
        if goal:
            begin += ["--goal", "Qualify fresh goal attempts", "--criterion", "proof=Native proof"]
        self.mutate(root, begin)
        if upstream or example:
            self.mutate(root, ["set", "input", "Immutable source input"], item="input")
            if upstream == "decision":
                self.mutate(root, ["decide", "--option", "go", "--reason", "Use the recorded input."])
            else:
                self.mutate(root, ["complete"])
        self.mutate(root, ["set", "result", "Final output"], item="result")
        return root

    @staticmethod
    def context(observation: dict, key: str | None = None) -> dict:
        cursor = dict(observation["result"]["cursor"])
        cursor.pop("session")
        return {
            "workspace_uuid": observation["workspace"]["uuid"],
            "idempotency_key": key or str(uuid.uuid4()), "cursor": cursor,
        }

    def fences(self, observation: dict, *, attempt: bool = True,
               item: str | None = None) -> list[str]:
        cursor = observation["result"]["cursor"]
        assert not attempt or cursor["attempt"] is not None
        args = ["--if-workspace-uuid", observation["workspace"]["uuid"],
                "--if-cursor", json.dumps(cursor)]
        if item:
            slot = next(entry for entry in observation["result"]["items"] if entry["item"] == item)
            args += ["--if-item-revision", str(slot["revision"])]
        return args

    def mutate(self, worktree: Path, args: list[str], *, item: str | None = None,
               attempt: bool = True, error: str | None = None) -> dict:
        return super().mutate(worktree, args, item=item, attempt=attempt and args[0] != "begin", error=error)

    def acceptance(self, worktree: Path, host: str = "external", disposition: str = "idle",
                   *, uncertain: bool = False) -> tuple[dict, dict]:
        package = self.call(worktree, ["delegation", "package"])["result"]
        state = self.external(worktree, "assign", package=package, host=host, uncertain=uncertain)
        assignment = state["assignments"][-1]
        return {
            "schema": "podway.delegation-accept-input/v2",
            "context": self.context(self.observe(worktree)),
            "package_digest": package["package_digest"], "execution": assignment["execution"],
            "rounds_used": assignment["rounds_used"], "result_digest": digest("Final output"),
            "summary": "Fixture Primary accepted after feedback.",
            "worker_disposition": disposition, "accepted": True,
        }, state

    def receipt(self, worktree: Path, payload: dict) -> dict:
        target = payload["context"]["cursor"]["attempt"]
        return self.call(worktree, ["delegation", "receipt", "--session", target["session"],
                                   "--attempt", target["attempt"]])["result"]


def check_scoped_acceptance(fixture: ScopedFixture, root: Path, host: str) -> dict:
    package = fixture.call(root, ["delegation", "package"])["result"]
    payload, external = fixture.acceptance(root, host)
    target = payload["context"]["cursor"]["attempt"]
    key = payload["context"]["idempotency_key"]
    assert package["package"]["target"] == target
    assert external["effects"] == 1 and external["assignments"][-1]["feedback"] == ["correct", "accept"]
    assert fixture.receipt(root, payload)["acceptance_state"] == "missing"
    fixture.mutate(root, ["complete"], error="DELEGATION_RECEIPT_REQUIRED")
    for field, value in [("role_ref", "wrong-role"), ("role_snapshot_digest", digest("wrong"))]:
        invalid = deepcopy(payload)
        invalid["execution"][field] = value
        invalid["context"]["idempotency_key"] = str(uuid.uuid4())
        fixture.accept(root, invalid, "DELEGATION_BINDING_MISMATCH")
    invalid = deepcopy(payload)
    invalid["rounds_used"] = 4
    invalid["context"]["idempotency_key"] = str(uuid.uuid4())
    fixture.accept(root, invalid, "DELEGATION_BINDING_MISMATCH")
    original = fixture.accept(root, payload)
    same_replay(original, fixture.accept(root, payload))
    lookup = fixture.call(root, ["outcome", "lookup", "--idempotency-key", key])["result"]
    assert lookup["found"] is True and lookup["outcome"]["response"] == original
    receipt = fixture.receipt(root, payload)
    assert receipt["acceptance_state"] == "accepted" and receipt["target"] == target
    assert receipt["receipt"]["receipt_id"] == original["result"]["receipt_id"]
    assert receipt["receipt"]["result"]["execution"] == payload["execution"]
    observed = fixture.observe(root)
    fixture.human(root, ["delegation", "package"], ["podway.delegation-package-result/v2", payload["package_digest"]])
    fixture.human(root, ["delegation", "receipt", "--session", target["session"], "--attempt", target["attempt"]],
                  ["podway.delegation-receipt-result/v2", receipt["receipt"]["receipt_id"]])
    failed_batch = {
        "schema":"podway.scoped-item-record-many-input/v1", "context":fixture.context(observed),
        "operations":[
            {"item_id":"note","expected_item_revision":0,"record":{"type":"text","value":"must roll back"}},
            {"item_id":"result","expected_item_revision":999,"record":{"type":"text","value":"invalid fence"}},
        ],
    }
    fixture.call(root, ["record", "--stdin"], payload=failed_batch, error="ITEM_REVISION_CONFLICT")
    assert fixture.observe(root)["result"]["items"] == observed["result"]["items"]
    fixture.mutate(root, ["block", "--reason", "The owned fixture is blocked."])
    fixture.mutate(root, ["complete"], error="BLOCKERS_PRESENT")
    fixture.mutate(root, ["unblock", "--all"])
    fixture.mutate(root, ["complete"])
    historical = fixture.receipt(root, payload)
    assert historical["applicability"] == "historical" and historical["receipt"] == receipt["receipt"]
    same_replay(original, fixture.accept(root, payload))
    return package


def documented_example(fixture: ScopedFixture) -> dict:
    source = Path(__file__).resolve().parents[1] / "docs/examples/procedures/supervised-review-v3.yaml"
    root = fixture.workspace("example", example=source)
    package = check_scoped_acceptance(fixture, root, "external")
    return {
        "source": "docs/examples/procedures/supervised-review-v3.yaml",
        "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "procedure_digest": package["package"]["target"]["base_digest"],
        "package_digest": package["package_digest"],
    }


def host_matrix(fixture: ScopedFixture, host: str) -> None:
    root = fixture.workspace(host, upstream="action")
    package = check_scoped_acceptance(fixture, root, host)
    assert package["package"]["evidence"][0]["selector"]["node"] == "source"


def decision_evidence(fixture: ScopedFixture) -> None:
    root = fixture.workspace("decision-evidence", upstream="decision")
    package = check_scoped_acceptance(fixture, root, "external")
    evidence = package["package"]["evidence"][0]
    assert evidence["resolution"] == "resolved"
    assert evidence["source"]["scope"] == {"kind": "base"}
    assert evidence["source"]["node"] == "source"
    assert evidence["items"][0]["item"] == "input" and evidence["items"][0]["type"] == "text"


def recovery(fixture: ScopedFixture) -> None:
    root = fixture.workspace("recovery", goal=True)
    payload, external = fixture.acceptance(root, uncertain=True)
    assert external["assignments"][-1]["outcome"] == "unknown"
    fixture.mutate(root, ["complete"], error="DELEGATION_RECEIPT_REQUIRED")
    settled = fixture.external(root, "reconcile")
    assert settled["effects"] == external["effects"]
    assert settled["assignments"][-1]["execution"] == payload["execution"]
    original = fixture.accept(root, payload)
    key = payload["context"]["idempotency_key"]
    recovered = fixture.call(root, ["outcome", "lookup", "--idempotency-key", key])["result"]
    assert recovered["outcome"]["response"] == original
    fixture.mutate(root, ["set", "note", "Retain a changed output."], item="note")
    assert fixture.receipt(root, payload)["acceptance_state"] == "stale"
    fixture.mutate(root, ["complete"], error="DELEGATION_RECEIPT_STALE")
    same_replay(original, fixture.accept(root, payload))
    fixture.mutate(root, ["goal", "revise", "--goal", "Revalidate corrected result",
                           "--criterion", "proof=Native proof", "--rework-to", "work",
                           "--reason", "The accepted result is stale.", "--basis-description", "Owned fixture",
                           "--basis-digest", digest("changed"), "--feedback", "Request a fresh result."])
    retained = fixture.receipt(root, payload)
    assert retained["applicability"] == "historical"
    late = deepcopy(payload)
    late["context"]["idempotency_key"] = str(uuid.uuid4())
    refusal = fixture.accept(root, late, "ATTEMPT_NOT_CURRENT")
    assert refusal["details"]["expected_attempt_id"] == late["context"]["cursor"]["attempt"]["attempt"]
    current = fixture.observe(root)["result"]["cursor"]["attempt"]["attempt"]
    assert refusal["details"]["actual_attempt_id"] == current
    rejected = fixture.call(root, ["outcome", "lookup", "--idempotency-key", late["context"]["idempotency_key"]])
    assert rejected["result"]["found"] is False
    assert fixture.external(root, "inspect") == settled

    ready = fixture.workspace("complete-loss")
    current, external = fixture.acceptance(ready)
    fixture.accept(ready, current)
    args = ["complete", *fixture.fences(fixture.observe(ready)), "--idempotency-key", "complete-loss"]
    completed = fixture.lost(ready, args)
    same_replay(completed, fixture.call(ready, args))
    assert fixture.receipt(ready, current)["applicability"] == "historical"
    assert fixture.external(ready, "inspect") == external

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
        f = ScopedFixture(cli, root)
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
