from __future__ import annotations

import importlib.util
import io
import json
import os
import plistlib
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


producer = load("aquarium_dev_producer", ROOT / "tools/aquarium_dev_producer.py")
controller = load("aquarium_dev_service", ROOT / "tools/aquarium_dev_service.py")


class AquariumDevProducerTests(unittest.TestCase):
    def test_description_is_exact_managed_service_v2(self):
        self.assertEqual(
            producer.description(),
            {
                "schema": "aquarium-dev-producer-description/v2",
                "project_id": "podway",
                "next_version": "v0.2.8",
                "artifact_kind": "managed-service",
                "artifact_path": "bundle",
                "command_path": "bin/podway",
                "controller_path": "libexec/aquarium-dev-service",
            },
        )

    def test_output_must_be_absolute_empty_and_not_a_symlink(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            relative = Path("relative")
            with self.assertRaises(producer.ProducerError):
                producer._validate_output(os.fspath(relative))
            (root / "occupied").mkdir()
            (root / "occupied/file").write_text("x", encoding="utf-8")
            with self.assertRaises(producer.ProducerError):
                producer._validate_output(os.fspath(root / "occupied"))
            (root / "empty").mkdir()
            (root / "link").symlink_to(root / "empty")
            with self.assertRaises(producer.ProducerError):
                producer._validate_output(os.fspath(root / "link"))

    def test_source_validation_rejects_non_main_and_dirty_checkouts(self):
        common = [os.fspath(ROOT)]
        with mock.patch.object(producer, "_run", side_effect=common + ["feature"]):
            with self.assertRaisesRegex(producer.ProducerError, "local main"):
                producer._validate_source()

        head = "a" * 40
        responses = [os.fspath(ROOT), "main", head, head, "dirty.txt"]
        with mock.patch.object(producer, "_run", side_effect=responses):
            with self.assertRaisesRegex(producer.ProducerError, "clean worktree"):
                producer._validate_source()

    def test_tree_digest_uses_canonical_path_and_raw_digest_records(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "b").write_bytes(b"two")
            (root / "a").write_bytes(b"one")
            first = producer.tree_digest(root)
            (root / "a").write_bytes(b"changed")
            self.assertNotEqual(first, producer.tree_digest(root))
            changed = producer.tree_digest(root)
            (root / "a").rename(root / "c")
            self.assertNotEqual(changed, producer.tree_digest(root))

    def test_producer_and_standalone_controller_share_exact_digest_semantics(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            (root / "nested").mkdir()
            (root / "nested/file").write_bytes(b"bundle bytes")
            self.assertEqual(producer.tree_digest(root), controller._tree_digest(root))
            (root / "link").symlink_to(root / "nested/file")
            with self.assertRaises(producer.ProducerError):
                producer.tree_digest(root)
            with self.assertRaises(controller.ControllerError):
                controller._tree_digest(root)

    def test_failed_build_removes_only_owned_partial_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary).resolve() / "output"
            output.mkdir()

            def fail_build(*_arguments, environment=None):
                build_root = Path(environment["CARGO_TARGET_DIR"])
                build_root.mkdir()
                (build_root / "partial").write_text("partial", encoding="utf-8")
                raise producer.ProducerError("build failed")

            with mock.patch.object(producer, "_validate_source", return_value="a" * 40), mock.patch.object(
                producer, "_run", side_effect=fail_build
            ):
                with self.assertRaisesRegex(producer.ProducerError, "build failed"):
                    producer.build(os.fspath(output))
            self.assertEqual(list(output.iterdir()), [])

    def test_build_emits_sealed_exact_identity_bundle(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary).resolve() / "output"
            output.mkdir()
            git_sha = "a" * 40

            def fake_run(*arguments, environment=None):
                self.assertEqual(arguments[:3], ("cargo", "build", "--release"))
                build_root = Path(environment["CARGO_TARGET_DIR"])
                release = build_root / "release"
                release.mkdir(parents=True)
                for name in ("podway", "podwayd"):
                    (release / name).write_text(f"#!/bin/sh\n# {name}\n", encoding="utf-8")
                return ""

            with mock.patch.object(producer, "_validate_source", return_value=git_sha), mock.patch.object(
                producer, "_run", side_effect=fake_run
            ):
                manifest = producer.build(os.fspath(output))
            self.assertEqual(manifest["git_sha"], git_sha)
            self.assertEqual(manifest["sha256"], producer.tree_digest(output / "bundle"))
            self.assertFalse((output / ".build").exists())
            launcher = (output / "bundle/bin/podway").read_text(encoding="utf-8")
            self.assertIn('"$here/../libexec/podway" --dev', launcher)
            self.assertNotIn("command -v", launcher)
            self.assertNotIn("exec podway", launcher)


class AquariumDevControllerTests(unittest.TestCase):
    def _generation(self, root: Path, sha: str) -> Path:
        generation = root / sha
        bundle = generation / "bundle"
        for relative in (
            "bin/podway",
            "libexec/podway",
            "libexec/podwayd",
            "libexec/aquarium-dev-service",
        ):
            path = bundle / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            path.chmod(0o755)
        executables = {}
        for role, relative in {
            "launcher": "bin/podway",
            "cli": "libexec/podway",
            "daemon": "libexec/podwayd",
            "controller": "libexec/aquarium-dev-service",
        }.items():
            executables[role] = {"path": relative, "sha256": controller._file_digest(bundle / relative)}
        (bundle / "manifest.json").write_text(json.dumps({
            "schema": "podway.aquarium-dev-bundle/v1",
            "project_id": "podway",
            "git_sha": sha,
            "development_version": f"v0.2.8-dev.{sha[:12]}",
            "payload_sha256": controller._tree_digest(bundle),
            "controller_protocol": "aquarium-dev-service/v1",
            "runtime_protocol": "podway.managed-runtime/v3",
            "executables": executables,
        }), encoding="utf-8")
        (generation / ".aquarium-manifest.json").write_text(json.dumps({
            "schema": "aquarium-dev-artifact-manifest/v2",
            "project_id": "podway",
            "git_sha": sha,
            "development_version": f"v0.2.8-dev.{sha[:12]}",
            "artifact_kind": "managed-service",
            "artifact_path": "bundle",
            "command_path": "bin/podway",
            "controller_path": "libexec/aquarium-dev-service",
            "sha256": controller._tree_digest(bundle),
        }), encoding="utf-8")
        return generation

    def test_absent_status_and_install_plan_are_read_only(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            generation = self._generation(root, "1" * 40)
            self.assertEqual(controller.status_document(runtime)["state"], "absent")
            plan = controller.plan_document(runtime, generation)
            self.assertEqual(plan["action"], "install")
            self.assertRegex(plan["plan_token"], r"^sha256:[0-9a-f]{64}$")
            self.assertFalse(runtime.exists())

    def test_controller_documents_are_bounded_before_json_decode(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = Path(temporary).resolve() / "oversized.json"
            document.write_bytes(b"{" + b" " * controller.MAX_JSON_BYTES)
            with self.assertRaisesRegex(controller.ControllerError, "unsafe controller file"):
                controller._read_json(document)

    def test_public_status_cli_emits_the_closed_json_document(self):
        with tempfile.TemporaryDirectory() as temporary:
            runtime = Path(temporary).resolve() / "runtime"
            output = io.StringIO()
            with redirect_stdout(output):
                exit_code = controller.main(
                    ["status", "--json", "--runtime-root", os.fspath(runtime)]
                )
            self.assertEqual(exit_code, 0)
            self.assertEqual(json.loads(output.getvalue()), controller.status_document(runtime))

    def test_busy_replacement_defers_without_token(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = self._generation(root, "2" * 40)
            with mock.patch.object(controller, "observe", return_value={
                "state": "busy", "active": "1" * 40, "busy": True, "recovery": False
            }):
                plan = controller.plan_document(root / "runtime", target)
            self.assertEqual(plan["action"], "defer")
            self.assertIsNone(plan["plan_token"])

    def test_busy_active_target_also_defers_with_matching_status(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = self._generation(root, "2" * 40)
            observed = {
                "state": "busy", "active": target.name, "busy": True, "recovery": False
            }
            with mock.patch.object(controller, "observe", return_value=observed):
                plan = controller.plan_document(root / "runtime", target)
            self.assertEqual(plan["action"], "defer")
            self.assertTrue(plan["busy"])
            self.assertIsNone(plan["plan_token"])

    def test_unknown_daemon_activity_fails_closed_as_busy(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            generation = self._generation(root, "1" * 40)
            paths = controller._runtime_paths(runtime)
            controller._prepare_root(paths)
            value = controller._generation(generation)
            metadata = controller._runtime_metadata(paths, value)
            plist = controller._plist(paths, value)
            controller._atomic_write(paths["runtime_metadata"], metadata)
            controller._atomic_write(paths["plist"], plist)
            controller._atomic_write(paths["state"], controller._json_bytes({
                "schema": controller.STATE_SCHEMA,
                "active_git_sha": generation.name,
                "generation_root": os.fspath(generation),
                "bundle_sha256": value["manifest"]["sha256"],
                "plist_sha256": controller._digest_bytes(plist),
            }))
            daemon = {
                "mode": "dev",
                "executable_path": os.fspath(value["daemon"]),
                "source_commit": generation.name,
                "readiness_state": "ready",
                "queued_job_count": None,
                "running_job_count": 0,
                "in_flight_client_count": 0,
                "maintenance_operation_count": 0,
                "worktree_recovery": {"failed": 0},
            }
            with mock.patch.object(controller, "_daemon_status", return_value=daemon):
                observed = controller.observe(runtime)
            self.assertEqual(observed["state"], "busy")
            self.assertTrue(observed["busy"])

            daemon["queued_job_count"] = 0
            daemon["worktree_recovery"]["failed"] = 1
            with mock.patch.object(controller, "_daemon_status", return_value=daemon):
                recovered = controller.observe(runtime)
            self.assertEqual(recovered["state"], "busy")
            self.assertTrue(recovered["busy"])

    def test_unreachable_cli_projection_means_no_live_daemon(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            generation = controller._generation(self._generation(root, "1" * 40))
            result = subprocess.CompletedProcess(
                [],
                0,
                json.dumps({"result": {"reachable": False, "readiness_state": "not_running"}}),
                "",
            )
            with mock.patch.object(controller, "_run", return_value=result):
                self.assertIsNone(
                    controller._daemon_status(controller._runtime_paths(root / "runtime"), generation)
                )

    def test_wrong_generation_daemon_still_defers_when_work_is_active(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            generation = self._generation(root, "1" * 40)
            paths = controller._runtime_paths(runtime)
            controller._prepare_root(paths)
            value = controller._generation(generation)
            metadata = controller._runtime_metadata(paths, value)
            plist = controller._plist(paths, value)
            controller._atomic_write(paths["runtime_metadata"], metadata)
            controller._atomic_write(paths["plist"], plist)
            controller._atomic_write(paths["state"], controller._json_bytes({
                "schema": controller.STATE_SCHEMA,
                "active_git_sha": generation.name,
                "generation_root": os.fspath(generation),
                "bundle_sha256": value["manifest"]["sha256"],
                "plist_sha256": controller._digest_bytes(plist),
            }))
            daemon = {
                "mode": "dev",
                "executable_path": "/different/podwayd",
                "source_commit": "2" * 40,
                "readiness_state": "ready",
                "queued_job_count": 1,
                "running_job_count": 0,
                "in_flight_client_count": 0,
                "maintenance_operation_count": 0,
                "worktree_recovery": {"failed": 0},
            }
            with mock.patch.object(controller, "_daemon_status", return_value=daemon):
                status = controller.status_document(runtime)
                plan = controller.plan_document(runtime, generation)
            self.assertEqual(status["state"], "busy")
            self.assertTrue(status["busy"])
            self.assertEqual(plan["action"], "defer")

    def test_launchctl_targets_only_the_aquarium_development_label(self):
        completed = subprocess.CompletedProcess([], 0, "", "")
        plist = Path("/tmp/dev.aquarium.podwayd.plist")
        with mock.patch.object(controller, "_launchctl", return_value=completed) as launchctl:
            controller._bootout()
            controller._bootstrap(plist)
        self.assertEqual(
            launchctl.call_args_list[0].args,
            ("bootout", f"gui/{os.geteuid()}/{controller.SERVICE_LABEL}"),
        )
        self.assertEqual(
            launchctl.call_args_list[1].args,
            ("bootstrap", f"gui/{os.geteuid()}", os.fspath(plist)),
        )

    def test_generation_manifest_tampering_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            generation = self._generation(root, "1" * 40)
            manifest_path = generation / "bundle/manifest.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["runtime_protocol"] = "podway.managed-runtime/v2"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            external_path = generation / ".aquarium-manifest.json"
            external = json.loads(external_path.read_text(encoding="utf-8"))
            external["sha256"] = controller._tree_digest(generation / "bundle")
            external_path.write_text(json.dumps(external), encoding="utf-8")
            with self.assertRaisesRegex(controller.ControllerError, "manifest identity"):
                controller._generation(generation)

    def test_stale_token_rejects_before_launchctl(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            target = self._generation(root, "2" * 40)
            with mock.patch.object(controller, "_launchctl") as launchctl:
                with self.assertRaises(controller.ControllerError):
                    controller.apply(runtime, target, "sha256:" + "0" * 64)
            launchctl.assert_not_called()
            self.assertFalse(runtime.exists())

    def test_locked_replan_rejects_a_token_after_activity_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            target = self._generation(root, "2" * 40)
            idle = {"state": "absent", "active": None, "busy": False, "recovery": False}
            busy = {"state": "busy", "active": "1" * 40, "busy": True, "recovery": False}
            with mock.patch.object(controller, "observe", side_effect=[idle, idle, busy]), mock.patch.object(
                controller, "_bootout"
            ) as bootout, mock.patch.object(controller, "_install") as install:
                plan = controller.plan_document(runtime, target)
                with self.assertRaisesRegex(controller.ControllerError, "absent or stale"):
                    controller.apply(runtime, target, plan["plan_token"])
            bootout.assert_not_called()
            install.assert_not_called()
            self.assertFalse(controller._runtime_paths(runtime)["recovery"].exists())

    def test_initial_install_uses_exact_generation_service_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            target = self._generation(root, "2" * 40)
            target_value = controller._generation(target)
            paths = controller._runtime_paths(runtime)
            plist = plistlib.loads(controller._plist(paths, target_value))
            self.assertEqual(plist["Label"], controller.SERVICE_LABEL)
            self.assertEqual(
                plist["ProgramArguments"], [os.fspath(target_value["daemon"]), "--dev"]
            )
            self.assertEqual(plist["EnvironmentVariables"], {"PODWAY_DEV_HOME": os.fspath(runtime)})

            plan = controller.plan_document(runtime, target)
            unreachable = subprocess.CompletedProcess(
                [], 0, json.dumps({"result": {"reachable": False}}), ""
            )
            with mock.patch.object(controller, "_run", return_value=unreachable), mock.patch.object(
                controller, "_install"
            ) as install:
                result = controller.apply(runtime, target, plan["plan_token"])
            self.assertEqual(result["status"], "activated")
            self.assertEqual(install.call_args.args[1]["git_sha"], target.name)
            self.assertFalse(paths["recovery"].exists())

    def test_no_change_apply_never_restarts_the_active_generation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            target = self._generation(root, "2" * 40)
            observed = {
                "state": "ready", "active": target.name, "busy": False, "recovery": False
            }
            with mock.patch.object(controller, "observe", return_value=observed), mock.patch.object(
                controller, "_bootout"
            ) as bootout, mock.patch.object(controller, "_install") as install:
                plan = controller.plan_document(runtime, target)
                self.assertEqual(plan["action"], "no-change")
                result = controller.apply(runtime, target, plan["plan_token"])
            self.assertEqual(result["status"], "no-change")
            bootout.assert_not_called()
            install.assert_not_called()

    def test_failed_activation_restores_exact_prior_generation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            prior = self._generation(root, "1" * 40)
            target = self._generation(root, "2" * 40)
            paths = controller._runtime_paths(runtime)
            controller._prepare_root(paths)
            prior_value = controller._generation(prior)
            metadata = controller._runtime_metadata(paths, prior_value)
            plist = controller._plist(paths, prior_value)
            controller._atomic_write(paths["runtime_metadata"], metadata)
            controller._atomic_write(paths["plist"], plist)
            controller._atomic_write(paths["state"], controller._json_bytes({
                "schema": controller.STATE_SCHEMA,
                "active_git_sha": prior.name,
                "generation_root": os.fspath(prior),
                "bundle_sha256": prior_value["manifest"]["sha256"],
                "plist_sha256": controller._digest_bytes(plist),
            }))
            observations = [
                {"state": "ready", "active": prior.name, "busy": False, "recovery": False},
                {"state": "ready", "active": prior.name, "busy": False, "recovery": False},
                {"state": "ready", "active": prior.name, "busy": False, "recovery": False},
                {"state": "ready", "active": prior.name, "busy": False, "recovery": False},
            ]
            with mock.patch.object(controller, "observe", side_effect=observations), mock.patch.object(
                controller, "_bootout"
            ), mock.patch.object(controller, "_install", side_effect=[controller.ControllerError("target failed"), None]) as install:
                plan = controller.plan_document(runtime, target)
                with self.assertRaises(controller.ControllerError) as failure:
                    controller.apply(runtime, target, plan["plan_token"])
            self.assertIn("restoration was proven", str(failure.exception))
            self.assertEqual(install.call_args_list[-1].args[1]["git_sha"], prior.name)
            self.assertFalse(paths["recovery"].exists())

    def test_incomplete_rollback_preserves_original_prior_for_repair(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            prior = self._generation(root, "1" * 40)
            target = self._generation(root, "2" * 40)
            paths = controller._runtime_paths(runtime)
            controller._prepare_root(paths)
            prior_value = controller._generation(prior)
            metadata = controller._runtime_metadata(paths, prior_value)
            plist = controller._plist(paths, prior_value)
            controller._atomic_write(paths["runtime_metadata"], metadata)
            controller._atomic_write(paths["plist"], plist)
            prior_state = {
                "schema": controller.STATE_SCHEMA,
                "active_git_sha": prior.name,
                "generation_root": os.fspath(prior),
                "bundle_sha256": prior_value["manifest"]["sha256"],
                "plist_sha256": controller._digest_bytes(plist),
            }
            controller._atomic_write(paths["state"], controller._json_bytes(prior_state))

            def observed(_root):
                recovering = paths["recovery"].exists()
                return {
                    "state": "broken" if recovering else "ready",
                    "active": prior.name,
                    "busy": False,
                    "recovery": recovering,
                }

            with mock.patch.object(controller, "observe", side_effect=observed), mock.patch.object(
                controller, "_bootout"
            ), mock.patch.object(
                controller,
                "_install",
                side_effect=[controller.ControllerError("target failed"), controller.ControllerError("prior failed")],
            ):
                plan = controller.plan_document(runtime, target)
                with self.assertRaisesRegex(controller.ControllerError, "not proven"):
                    controller.apply(runtime, target, plan["plan_token"])

            recovery = controller._recovery(paths)
            self.assertEqual(recovery["previous"], prior_state)
            self.assertEqual(
                recovery["debt"],
                ["target-activation-failed", "prior-restoration-unproven"],
            )

            prior.rename(root / "removed-prior-generation")
            with mock.patch.object(controller, "_bootout"), mock.patch.object(
                controller, "_install"
            ) as install:
                repair = controller.plan_document(runtime, target)
                self.assertEqual(repair["action"], "repair")
                result = controller.apply(runtime, target, repair["plan_token"])
            self.assertEqual(result["status"], "repaired")
            self.assertEqual(install.call_args.args[1]["git_sha"], target.name)
            self.assertFalse(paths["recovery"].exists())

    def test_missing_failed_target_can_be_rebound_to_a_new_repair_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runtime = root / "runtime"
            failed = self._generation(root, "2" * 40)
            replacement = self._generation(root, "3" * 40)
            paths = controller._runtime_paths(runtime)
            controller._prepare_root(paths)
            controller._atomic_write(paths["recovery"], controller._json_bytes({
                "schema": controller.RECOVERY_SCHEMA,
                "target_git_sha": failed.name,
                "target_generation_root": os.fspath(failed),
                "previous": None,
                "phase": "recovery-required",
                "debt": ["target-activation-failed"],
            }))
            failed.rename(root / "removed-failed-target")

            plan = controller.plan_document(runtime, replacement)
            self.assertEqual(plan["action"], "repair")
            with mock.patch.object(controller, "_install") as install, mock.patch.object(
                controller, "_bootout"
            ):
                result = controller.apply(runtime, replacement, plan["plan_token"])
            self.assertEqual(result["status"], "repaired")
            self.assertEqual(install.call_args.args[1]["git_sha"], replacement.name)
            self.assertFalse(paths["recovery"].exists())


if __name__ == "__main__":
    unittest.main()
