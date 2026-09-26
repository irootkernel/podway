#!/usr/bin/env python3
"""V2SDG-008 native qualification; the external executor is deterministic, not live."""

from __future__ import annotations

import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid

import dev_runtime as runtime


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
    def __init__(self, cli: Path, daemon: Path, root: Path):
        self.cli, self.daemon, self.root = cli, daemon, root
        self.account = root / "account"
        self.home = root / "dev"
        self.socket = self.home / "run/podwayd.sock"
        self.process = None

    def environment(self, home: Path | None = None) -> dict[str, str]:
        chosen = self.home if home is None else home
        assert chosen.is_absolute() and chosen.is_relative_to(self.root)
        env = runtime.isolation_environment(self.account, chosen)
        assert env["PODWAY_DEV_HOME"] == str(chosen)
        return env

    def start(self) -> None:
        self.environment()
        self.process = runtime.start_isolated_daemon(
            self.daemon, self.account, self.home, cli=self.cli, mode="dev",
        )
        status = self.call(self.account, ["daemon", "status"])["result"]
        assert status["mode"] == "dev", status
        assert status["effective_socket_path"] == str(self.socket), status

    def stop(self, crash: bool = False) -> None:
        if self.process is not None:
            if crash:
                self.process.send_signal(signal.SIGKILL)
                self.process.wait(timeout=10)
            else:
                runtime.stop_process(self.process)
                assert self.process.returncode == 0, self.process.returncode
                assert not self.socket.exists(), "daemon left its socket"
            assert self.process.poll() is not None
            if self.process.stderr:
                self.process.stderr.close()
            self.process = None

    def call(self, worktree: Path, args: list[str], *, payload: dict | None = None,
             error: str | None = None, home: Path | None = None) -> dict:
        local = args[:1] == ["procedure"]
        selector = [] if local or args[:1] == ["daemon"] else ["--worktree", str(worktree)]
        mode = [] if local else ["--mode", "dev"]
        completed = subprocess.run(
            [str(self.cli), *mode, "--json", *selector, *args],
            env=self.environment(home), cwd=worktree,
            input=None if payload is None else json.dumps(payload).encode(),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, check=False,
        )
        envelope = runtime.decode_cli_json(completed, label=" ".join(args), expected_code=None)
        if error:
            assert envelope.get("code") == error, (args, envelope, completed.stderr, error)
            assert completed.returncode != 0
        else:
            assert completed.returncode == 0, (args, envelope, completed.stderr)
            assert envelope["schema"] == "podway.output/v3", envelope
        return envelope

    def workspace(self, name: str, disposition: str | None = "retain", *, goal: bool = False,
                  upstream: str | None = None) -> Path:
        worktree = self.root / name
        worktree.mkdir(mode=0o700)
        subprocess.run(["git", "init", "--quiet", str(worktree)], check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.call(worktree, ["init"])
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
        return self.call(worktree, ["observe", "--wait-for-idle"])

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
        completed = subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "--fixture-ledger", str(worktree / "external.json")],
            input=json.dumps({"operation": operation, **fields}).encode(),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True,
            env={"PATH": "/usr/bin:/bin"},
        )
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
        proxy_home = self.root / ("proxy-" + uuid.uuid4().hex[:6])
        (proxy_home / "run").mkdir(parents=True, mode=0o700)
        proxy = proxy_home / "run/podwayd.sock"
        command = "delegation.accept" if payload is not None else "session.complete"
        relay, outcome = runtime.response_loss_relay(proxy, self.socket, discard_command=command)
        failure = None
        try:
            self.call(worktree, args, payload=payload, home=proxy_home, error="MUTATION_OUTCOME_UNKNOWN")
        except (AssertionError, runtime.DevRuntimeError) as error:
            failure = error
        finally:
            relay.join(timeout=15)
            assert not relay.is_alive(), "response-loss relay survived"
            proxy.unlink(missing_ok=True)
        assert "error" not in outcome, outcome
        response = json.loads(outcome["response"][4:])
        assert failure is None, (str(failure), response)
        return response


def same_replay(original: dict, replay: dict) -> None:
    assert {k: v for k, v in original.items() if k != "request_id"} == {
        k: v for k, v in replay.items() if k != "request_id"
    }, (original, replay)


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
    lookup = f.call(worktree, ["job", "lookup", "--idempotency-key", payload["idempotency_key"]])["result"]
    assert lookup["job"]["terminal_response"] == accepted
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
    # Crash before receipt submission, then reconcile the same external outcome.
    f.stop(crash=True)
    f.start()
    f.state(worktree, "awaiting_acceptance")
    reconciled = f.external(worktree, "reconcile")
    assert reconciled["effects"] == state["effects"]
    assert reconciled["assignments"][-1]["execution"] == payload["execution"]
    state = reconciled
    original = f.lost(worktree, ["delegation", "accept", "--stdin"], payload)
    assert original["command"] == "delegation.accept" and "result" in original, original
    f.stop(crash=True)
    f.start()
    lookup = f.call(worktree, ["job", "lookup", "--idempotency-key", payload["idempotency_key"]])["result"]
    assert lookup["job"]["terminal_response"] == original
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
    f.stop(crash=True)
    f.start()
    f.state(ready, "accepted")
    completed = f.lost(ready, args)
    f.stop(crash=True)
    f.start()
    lookup = f.call(ready, ["job", "lookup", "--idempotency-key", "complete-loss"])["result"]
    assert lookup["job"]["terminal_response"] == completed
    same_replay(completed, f.call(ready, args))
    assert f.observe(ready)["result"]["status"]["session"]["lifecycle"] == "completed"
    assert f.external(ready, "inspect") == external

    admitted = f.workspace("admitted-crash")
    receipt_input, external = f.acceptance(admitted)
    unchanged = f.observe(admitted)["result"]["status"]["session"]
    admission = f.call(admitted, ["delegation", "accept", "--stdin", "--detach"],
                       payload=receipt_input)["result"]["admission"]
    # Kill after durable admission: execution may be pending or already committed.
    f.stop(crash=True)
    f.start()
    f.call(admitted, ["job", "wait", admission["job_id"]])
    recovered = f.state(admitted, "accepted")["result"]["status"]["session"]
    assert recovered["revision"] == unchanged["revision"] + 1
    lookup = f.call(admitted, ["job", "lookup", "--idempotency-key", receipt_input["idempotency_key"]])["result"]
    assert lookup["job"]["state"] == "succeeded"
    assert lookup["job"]["id"] == admission["job_id"]
    same_replay(lookup["job"]["terminal_response"], f.accept(admitted, receipt_input))
    assert f.external(admitted, "inspect") == external
    f.mutate(admitted, ["complete"])


def graph_rows(connection: sqlite3.Connection) -> dict:
    tables = [row[0] for row in connection.execute(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name GLOB 'v2_*' "
        "AND name != 'v2_delegation_receipts' ORDER BY name"
    )]
    return {table: sorted(connection.execute(f'SELECT * FROM "{table}"').fetchall(), key=repr)
            for table in tables}


def terminate_qualification(signum: int, _frame: object) -> None:
    # Unwind the owning fixture's finally block when a runner terminates this helper.
    raise SystemExit(128 + signum)


def migration(f: Fixture) -> None:
    worktree = f.workspace("migration", None)
    direct = f.workspace("migration-control", None)
    before = f.observe(worktree)["result"]["status"]
    f.stop()
    database = worktree / ".podway/runtime/state.sqlite3"
    assert database.is_file(), database
    with sqlite3.connect(database) as connection:
        snapshot = graph_rows(connection)
        connection.executescript("""
            DROP TRIGGER v2_delegation_receipts_immutable;
            DROP TABLE v2_delegation_receipts;
            DELETE FROM schema_migrations WHERE version IN (12, 13);
            PRAGMA user_version = 11;
        """)
        backup = f.root / "v11-backup.sqlite3"
        with sqlite3.connect(backup) as target:
            connection.backup(target)
    # Hold the writer boundary so native startup cannot finish the pending upgrade.
    # Interior transaction rollback is checked separately by the injected SQL failure.
    with sqlite3.connect(database) as lock:
        lock.execute("BEGIN IMMEDIATE")
        f.process = subprocess.Popen(
            [str(f.daemon), "--mode", "dev"], cwd=f.account, env=f.environment(),
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        identity = runtime.daemon_contract_identity(f.cli, f.account, f.home)
        deadline = time.monotonic() + 10
        recovering = False
        while time.monotonic() < deadline:
            if runtime.endpoint_is_live(f.socket):
                recovering = runtime.probe_daemon_readiness(f.socket, identity, expected_mode="dev") == "recovering"
                if recovering:
                    break
            time.sleep(0.01)
        assert recovering, "daemon did not expose recovery while the migration writer was held"
        f.stop(crash=True)
        lock.rollback()
        assert lock.execute("PRAGMA user_version").fetchone()[0] == 11
        assert graph_rows(lock) == snapshot
    with sqlite3.connect(database) as connection:
        # Fail the migration transaction after v12 DDL, before recording v13.
        connection.executescript("""
            CREATE TRIGGER qualification_migration_failure BEFORE INSERT ON schema_migrations
            WHEN NEW.version = 13 BEGIN SELECT RAISE(ABORT, 'qualification migration failure'); END;
        """)
    f.start()
    f.call(worktree, ["status"], error="WORKSPACE_STATE_UNREADABLE")
    f.mutate(direct, ["retry", "--reason", "Progress while another Store cannot migrate"])
    f.mutate(direct, ["set", "result", "Unaffected Direct work"], item="result")
    f.mutate(direct, ["complete"])
    f.stop()
    with sqlite3.connect(database) as connection:
        assert connection.execute("PRAGMA user_version").fetchone()[0] == 11
        assert connection.execute("SELECT count(*) FROM sqlite_schema WHERE name='v2_delegation_receipts'").fetchone()[0] == 0
        assert graph_rows(connection) == snapshot
        connection.execute("DROP TRIGGER qualification_migration_failure")
    f.start()
    after = f.observe(worktree)["result"]["status"]
    assert after["session"] == before["session"] and after["current"] == before["current"]
    f.stop()
    with sqlite3.connect(database) as connection:
        assert connection.execute("PRAGMA user_version").fetchone()[0] == 13
        assert graph_rows(connection) == snapshot
        connection.execute("PRAGMA user_version = 14")
    f.start()
    f.call(worktree, ["status"], error="WORKSPACE_SCHEMA_UNSUPPORTED")
    f.stop()
    with sqlite3.connect(database) as connection:
        assert connection.execute("PRAGMA user_version").fetchone()[0] == 14
    # Restore only this stopped, helper-owned fixture from the populated v11 backup.
    with sqlite3.connect(backup) as source, sqlite3.connect(database) as target:
        source.backup(target)
    f.start()
    assert f.observe(worktree)["result"]["status"]["session"] == before["session"]
    f.mutate(worktree, ["complete"])
    assert f.observe(worktree)["result"]["status"]["session"]["lifecycle"] == "completed"


def main() -> None:
    if not __debug__:
        raise RuntimeError("qualification requires Python assertions to be enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--podway", type=Path)
    parser.add_argument("--podwayd", type=Path)
    parser.add_argument("--fixture-ledger", type=Path)
    parser.add_argument("--scenario", choices=("all", "hosts", "recovery", "migration"), default="all")
    args = parser.parse_args()
    if args.fixture_ledger:
        fixture_process(args.fixture_ledger)
        return
    signal.signal(signal.SIGTERM, terminate_qualification)
    assert args.podway and args.podwayd, "both native binaries are required"
    cli = runtime.require_qualification_binary(str(args.podway.resolve()), label="podway")
    daemon = runtime.require_qualification_binary(str(args.podwayd.resolve()), label="podwayd")
    for binary in (cli, daemon):
        assert runtime.release_archive.test_isolation_capability(binary) is \
            runtime.release_archive.TestIsolationCapability.ENABLED, "test isolation is required before launch"
    binaries = {name: runtime.release_archive.sha256_file(path)
                for name, path in (("podway", cli), ("podwayd", daemon))}
    checks = {}
    with tempfile.TemporaryDirectory(prefix="pw-sdg-", dir="/private/tmp") as directory:
        f = Fixture(cli, daemon, Path(directory))
        try:
            f.start()
            identity = runtime.daemon_contract_identity(cli, f.account, f.home)
            if args.scenario in ("all", "hosts"):
                for host in ("external", "managed"):
                    host_matrix(f, host)
                    checks[host] = True
                decision_evidence(f)
                checks["decision_evidence"] = True
            if args.scenario in ("all", "recovery"):
                recovery(f)
                checks["recovery"] = True
            if args.scenario in ("all", "migration"):
                migration(f)
                checks["migration"] = True
        finally:
            f.stop()
        assert not f.socket.exists()
    assert binaries == {name: runtime.release_archive.sha256_file(path)
                        for name, path in (("podway", cli), ("podwayd", daemon))}
    print(json.dumps({"schema": "podway.v2sdg008-qualification/v1", "checks": checks,
                      "binaries": binaries, "contract_identity": identity,
                      "runtime_cleanup": True, "live_adapter": "not_performed",
                      "live_model": "not_performed"}, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
