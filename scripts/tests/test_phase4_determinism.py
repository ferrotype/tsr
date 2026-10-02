"""Bounded X7 verifier tests using explicitly mocked build/process fixtures.

No Rust executable is built or invoked. The three-scenario inventory, native
image, Cargo proof, toolchain and source fingerprints below are fixture mocks;
write_capture's synthetic executable is tested separately and rejected.
"""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_corpus as corpus  # noqa: E402
import phase4_determinism as witness  # noqa: E402
from s08_oracle import canonical, digest  # noqa: E402

IDS = ["tsc/fixture.js", "tsbuild/fixture.js", "tscWatch/fixture.js"]
DOCUMENT = {"scenarios": [{"id": identity, "family": identity.split("/")[0], "edits": []}
                           for identity in IDS],
            "provenance": {"pin": "fixture pin", "scenario_digests": {name: digest(name.encode()) for name in IDS}},
            "orphan_references": []}
MOCK_SOURCES = {"fixture/mock-source.rs": "a" * 64}
MOCK_WITNESS_SOURCES = {**MOCK_SOURCES, "scripts/phase4_determinism.py": "b" * 64}
PIN = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
MOCK_TOOLCHAIN = {"host": "Linux fixture host", "system": "Linux", "machine": "x86_64",
                  "cargo": {"command": ["cargo", "--version"], "exit_status": 0,
                            "stdout": f"cargo {PIN} (fixture mock)\n", "stderr": ""},
                  "rustc": {"command": ["rustc", "--version", "--verbose"], "exit_status": 0,
                            "stdout": f"rustc {PIN} (fixture mock)\nhost: x86_64-unknown-linux-gnu\nrelease: {PIN}\n",
                            "stderr": ""}}
MOCK_IMAGE = b"\x7fELFfixture mock, never executed\n"
REAL_WITNESS_SOURCES = witness.sources


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        for target, value in (("read_document", DOCUMENT), ("sources", MOCK_SOURCES)):
            patcher = mock.patch.object(corpus, target, return_value=copy.deepcopy(value))
            patcher.start()
            self.addCleanup(patcher.stop)
        for target, value in (("sources", MOCK_WITNESS_SOURCES), ("toolchain", MOCK_TOOLCHAIN)):
            patcher = mock.patch.object(witness, target, return_value=copy.deepcopy(value))
            patcher.start()
            self.addCleanup(patcher.stop)
        self.texts = {identity: (identity + "\n").encode() for identity in IDS}

    def mock_build(self, directory):
        """Mock Cargo proof/image with the same shape as an actual corpus build."""
        binary = self.root / "fixture-binary"
        binary.write_bytes(MOCK_IMAGE)
        record = {"version": 1, "command": witness.BUILD_COMMAND, "environment": {},
                  "binary": str(binary), "binary_sha256": digest(MOCK_IMAGE), "sources": MOCK_SOURCES}
        events = [{"reason": "compiler-artifact", "executable": str(binary),
                   "manifest_path": str(ROOT / "tools/phase4/tsctests/Cargo.toml"),
                   "target": {"name": corpus.PACKAGE, "kind": ["bin"],
                              "src_path": str(ROOT / "tools/phase4/tsctests/src/main.rs")},
                   "filenames": [str(binary)], "features": [],
                   "profile": {"opt_level": "0", "debug_assertions": True, "test": False}},
                  {"reason": "build-finished", "success": True}]
        (directory / "build.stdout").write_bytes(b"".join(canonical(event) + b"\n" for event in events))
        (directory / "build.stderr").write_bytes(b"")
        witness.write_json(directory / "build.json", record)
        (directory / "executable").write_bytes(MOCK_IMAGE)
        return record

    def rebind_capture(self, directory):
        metadata = witness.read_json(directory / "capture.json")
        metadata["artifacts"] = {name: digest((directory / name).read_bytes()) for name in corpus.ARTIFACTS}
        witness.write_json(directory / "capture.json", metadata)
        return digest((directory / "capture.json").read_bytes())

    def rebind(self, directory, report=None):
        report = report or witness.read_json(directory / "report.json")
        for entry in report["runs"]:
            if entry.get("capture_sha256") is not None:
                entry["capture_sha256"] = digest((directory / entry["path"] / "capture.json").read_bytes())
        report["artifacts"] = witness.artifact_hashes(directory)
        witness.write_json(directory / "report.json", report)
        return report

    def fixture(self, *, selectors=("all",)):
        directory = self.root / "witness"
        directory.mkdir()
        build_dir = directory / "build"
        build_dir.mkdir()
        self.mock_build(build_dir)
        witness.write_json(directory / "toolchain.json", MOCK_TOOLCHAIN)
        report = {"version": 1, "sources": MOCK_WITNESS_SOURCES, "toolchain": MOCK_TOOLCHAIN,
                  "roots": {"repository": str(ROOT), "output": str(directory)}, "timeout_seconds": 600,
                  "inventory": corpus.inventory_binding(DOCUMENT),
                  "build": witness.authenticate_build(build_dir), "environment": {"LANG": "C"},
                  "jobs": 3, "runs": []}
        for number in range(1, witness.RUN_COUNT + 1):
            run = directory / f"runs/{number}"
            corpus.write_capture(run, DOCUMENT, selectors, self.texts)
            witness.copy_build(build_dir, run)
            self.rebind_capture(run)
            invocation = {"version": 1, "id": f"{number:032x}",
                          "command": witness.invocation_command(run, 3, selectors), "cwd": str(ROOT),
                          "environment": report["environment"], "jobs": 3, "exit_status": 0, "error": None,
                          "timeout_seconds": 600, "timed_out": False}
            witness.write_json(run / "invocation.json", invocation)
            report["runs"].append({"path": f"runs/{number}",
                                   "capture_sha256": digest((run / "capture.json").read_bytes())})
        self.rebind(directory, report)
        return directory

    def change_rows(self, directory, number, mutate, texts=None):
        run = directory / f"runs/{number}"
        rows = [json.loads(line) for line in (run / "rows.jsonl").read_bytes().splitlines()]
        mutate(rows)
        (run / "rows.jsonl").write_bytes(b"".join(canonical(row) + b"\n" for row in rows))
        if texts:
            for identity, text in texts.items():
                (run / "baselines" / identity).write_bytes(text)
        summary = witness.read_json(run / "summary.json")
        summary["states"] = corpus.states_by_family(rows)
        for name in ("summary.json", "stdout"):
            witness.write_json(run / name, summary)
        self.rebind_capture(run)
        self.rebind(directory)

    def metric(self, directory):
        return witness.verify_witnesses(directory)["metrics"]["determinism"]


class Replay(Fixture):
    def test_five_complete_mock_captures_recompute_true_and_bind_report(self):
        directory = self.fixture()
        result = witness.verify_witnesses(directory)
        self.assertTrue(result["metrics"]["determinism"])
        self.assertEqual(result["identities"]["determinism"], digest((directory / "report.json").read_bytes()))
        self.assertEqual(len(result["details"]["runs"]), 5)

    def test_linux_archive_replays_after_checkout_and_output_relocation_without_children(self):
        directory = self.fixture()
        captured_root = Path("/fixture/original-checkout")
        report = witness.read_json(directory / "report.json")
        report["roots"]["repository"] = str(captured_root)
        events_path = directory / "build/build.stdout"
        events = [json.loads(line) for line in events_path.read_bytes().splitlines()]
        events[0]["manifest_path"] = str(captured_root / "tools/phase4/tsctests/Cargo.toml")
        events[0]["target"]["src_path"] = str(captured_root / "tools/phase4/tsctests/src/main.rs")
        events_path.write_bytes(b"".join(canonical(event) + b"\n" for event in events))
        report["build"] = witness.authenticate_build(directory / "build")
        for entry in report["runs"]:
            run = directory / entry["path"]
            witness.copy_build(directory / "build", run)
            self.rebind_capture(run)
            invocation = witness.read_json(run / "invocation.json")
            invocation["cwd"] = str(captured_root)
            invocation["command"] = witness.invocation_command(run, 3, ["all"], captured_root)
            witness.write_json(run / "invocation.json", invocation)
        self.rebind(directory, report)
        moved = self.root / "relocated-archive"
        directory.rename(moved)
        with mock.patch.object(witness.subprocess, "run", side_effect=AssertionError("replay spawned a child")), \
                mock.patch.object(witness, "toolchain", side_effect=AssertionError("replay probed this host")):
            self.assertTrue(self.metric(moved))

    def test_recorded_compiler_must_match_the_pin_and_recorded_host(self):
        directory = self.fixture()
        original = copy.deepcopy(MOCK_TOOLCHAIN)
        for key, value in (("stdout", "rustc 0.0.0 (fixture wrong version)\n"),
                           ("stdout", original["rustc"]["stdout"].replace("linux-gnu", "apple-darwin")),
                           ("exit_status", 1)):
            versions = copy.deepcopy(original)
            versions["rustc"][key] = value
            witness.write_json(directory / "toolchain.json", versions)
            report = witness.read_json(directory / "report.json")
            report["toolchain"] = versions
            self.rebind(directory, report)
            with self.assertRaises(ValueError):
                self.metric(directory)

    def test_byte_difference_is_false_even_if_cached_pass_is_true(self):
        directory = self.fixture()
        text = b"changed rendered byte\n"
        def change(rows):
            rows[0] = corpus.completed_row(DOCUMENT["scenarios"][0],
                                            DOCUMENT["provenance"]["scenario_digests"][IDS[0]], text)
        self.change_rows(directory, 5, change, {IDS[0]: text})
        report = witness.read_json(directory / "report.json")
        report["pass"] = True
        report["summary"] = {"determinism": True}
        witness.write_json(directory / "report.json", report)
        witness.write_json(directory / "replayed.json", {"metrics": {"determinism": True}})
        self.assertFalse(self.metric(directory))

    def test_same_bytes_with_incomplete_state_is_false(self):
        directory = self.fixture()
        def refuse(rows):
            row = rows[0]
            for key in corpus.STATE_FIELDS["completed"]:
                del row[key]
            row.update(state="unsupported", operation="fixture missing command",
                       progress={"stage": "initial", "commands": 0, "edits_completed": 0, "edits": 0})
        for number in range(1, 6):
            self.change_rows(directory, number, refuse)
        self.assertFalse(self.metric(directory))

    def test_completed_state_difference_is_false(self):
        directory = self.fixture()
        self.change_rows(directory, 3, lambda rows: rows[0].update(unexpected_diff="fixture difference"))
        self.assertFalse(self.metric(directory))

    def test_partial_family_captures_are_withheld(self):
        with self.assertRaisesRegex(ValueError, "partial determinism capture"):
            self.metric(self.fixture(selectors=("tsbuild",)))

    def test_failed_process_is_retained_and_false(self):
        directory = self.fixture()
        run = directory / "runs/4"
        invocation = witness.read_json(run / "invocation.json")
        invocation["exit_status"] = 101
        witness.write_json(run / "invocation.json", invocation)
        (run / "stderr").write_bytes(b"fixture process panic\n")
        report = witness.read_json(directory / "report.json")
        report["runs"][3]["capture_sha256"] = None
        self.rebind(directory, report)
        self.assertFalse(self.metric(directory))
        self.assertEqual((run / "stderr").read_bytes(), b"fixture process panic\n")

    def test_synthetic_capture_marker_cannot_authenticate_even_when_rebound(self):
        directory = self.fixture()
        for path in [directory / "build", *[directory / f"runs/{i}" for i in range(1, 6)]]:
            (path / "executable").write_bytes(corpus.SYNTHETIC)
            record = witness.read_json(path / "build.json")
            record["binary_sha256"] = digest(corpus.SYNTHETIC)
            witness.write_json(path / "build.json", record)
            if (path / "capture.json").exists():
                self.rebind_capture(path)
        self.rebind(directory)
        with self.assertRaisesRegex(ValueError, "native harness"):
            self.metric(directory)

    def test_stale_sources_toolchain_inventory_and_build_raise(self):
        directory = self.fixture()
        original = witness.read_json(directory / "report.json")
        for key in ("sources", "toolchain", "inventory"):
            report = copy.deepcopy(original)
            report[key] = {}
            witness.write_json(directory / "report.json", report)
            with self.assertRaisesRegex(ValueError, "stale"):
                self.metric(directory)
        witness.write_json(directory / "report.json", original)
        with mock.patch.object(witness, "sources", return_value={**MOCK_WITNESS_SOURCES,
                                                                "scripts/phase4_determinism.py": "c" * 64}):
            with self.assertRaisesRegex(ValueError, "stale"):
                self.metric(directory)
        with mock.patch.object(corpus, "sources", return_value={"other.rs": "d" * 64}):
            with self.assertRaisesRegex(ValueError, "stale"):
                self.metric(directory)

    def test_missing_or_changed_raw_artifact_raises(self):
        directory = self.fixture()
        path = directory / "runs/1/baselines" / IDS[0]
        path.write_bytes(b"tampered without rebinding")
        with self.assertRaisesRegex(ValueError, "changed determinism artifact"):
            self.metric(directory)
        self.rebind(directory)
        with self.assertRaisesRegex(ValueError, "transcript"):
            self.metric(directory)
        path.unlink()
        self.rebind(directory)
        with self.assertRaises(OSError):
            self.metric(directory)

    def test_malformed_nested_cargo_artifact_raises_a_withholding_error(self):
        directory = self.fixture()
        path = directory / "build/build.stdout"
        events = [json.loads(line) for line in path.read_bytes().splitlines()]
        events[0]["target"] = []
        path.write_bytes(b"".join(canonical(event) + b"\n" for event in events))
        self.rebind(directory)
        with self.assertRaisesRegex(ValueError, "malformed determinism evidence"):
            self.metric(directory)

    def test_missing_build_proof_raises_even_with_rebound_index(self):
        directory = self.fixture()
        (directory / "build/build.stdout").unlink()
        self.rebind(directory)
        with self.assertRaises(OSError):
            self.metric(directory)

    def test_malformed_reordered_and_missing_rows_raise(self):
        for mutation in (lambda rows: rows.reverse(), lambda rows: rows.pop(),
                         lambda rows: rows[0].update(state="invented")):
            with self.subTest(mutation=mutation):
                directory = self.fixture()
                self.change_rows(directory, 2, mutation)
                with self.assertRaises(ValueError):
                    self.metric(directory)
                shutil.rmtree(directory)

    def test_exact_run_count_order_and_distinct_references_required(self):
        directory = self.fixture()
        original = witness.read_json(directory / "report.json")
        for mutate in (lambda rows: rows.pop(), lambda rows: rows.append(rows[0]),
                       lambda rows: rows.reverse(), lambda rows: rows.__setitem__(1, rows[0])):
            report = copy.deepcopy(original)
            mutate(report["runs"])
            witness.write_json(directory / "report.json", report)
            with self.assertRaisesRegex(ValueError, "five distinct, ordered"):
                self.metric(directory)

    def test_duplicate_invocations_or_borrowed_run_paths_raise(self):
        directory = self.fixture()
        path = directory / "runs/2/invocation.json"
        original = witness.read_json(path)
        first = witness.read_json(directory / "runs/1/invocation.json")
        for key in ("id", "command", "jobs", "environment"):
            invocation = copy.deepcopy(original)
            invocation[key] = (first[key] if key in ("id", "command") else 99 if key == "jobs" else {})
            witness.write_json(path, invocation)
            self.rebind(directory)
            with self.assertRaises(ValueError):
                self.metric(directory)

    def test_one_different_image_or_symlink_cannot_share_build(self):
        directory = self.fixture()
        (directory / "runs/2/executable").write_bytes(MOCK_IMAGE + b"different")
        self.rebind(directory)
        with self.assertRaisesRegex(ValueError, "one harness build"):
            self.metric(directory)
        (directory / "runs/2/executable").unlink()
        (directory / "runs/2/executable").symlink_to(directory / "runs/1/executable")
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.metric(directory)


class Runner(Fixture):
    def mock_process(self, command, **kwargs):
        """Mock process output only; run() still records and replays real files."""
        run_dir = Path(command[command.index("--output") + 1])
        temp = self.root / "process-fixture"
        corpus.write_capture(temp, DOCUMENT, ["all"], self.texts)
        for name in ("rows.jsonl", "summary.json", "stdout", "stderr"):
            shutil.copy2(temp / name, run_dir / name)
        shutil.copytree(temp / "baselines", run_dir / "baselines")
        shutil.rmtree(temp)
        return mock.Mock(returncode=0)

    def test_runner_builds_once_and_executes_five_distinct_processes(self):
        directory = self.root / "new"
        with mock.patch.object(corpus, "build", side_effect=self.mock_build) as build, \
                mock.patch.object(witness.subprocess, "run", side_effect=self.mock_process) as execute:
            result = witness.run(directory, jobs=3)
        self.assertTrue(result["metrics"]["determinism"])
        self.assertEqual(build.call_count, 1)
        self.assertEqual(execute.call_count, 5)
        commands = [call.args[0] for call in execute.call_args_list]
        self.assertEqual(len({command[0] for command in commands}), 5)
        self.assertTrue(all(command[-1] == "all" and command[-2] == "3" for command in commands))

    def test_runner_reuses_seed_build_without_rebuilding(self):
        seed = self.fixture() / "runs/1"
        with mock.patch.object(corpus, "build") as build, \
                mock.patch.object(witness.subprocess, "run", side_effect=self.mock_process) as execute:
            result = witness.run(self.root / "reuse", capture=seed, jobs=2)
        self.assertTrue(result["metrics"]["determinism"])
        build.assert_not_called()
        self.assertEqual(execute.call_count, 5)

    def test_runner_retains_process_failures_and_completes_remaining_runs(self):
        def fail(command, **kwargs):
            kwargs["stderr"].write(b"fixture launch failure\n")
            return mock.Mock(returncode=17)
        with mock.patch.object(corpus, "build", side_effect=self.mock_build), \
                mock.patch.object(witness.subprocess, "run", side_effect=fail) as execute:
            result = witness.run(self.root / "failed", jobs=3)
        self.assertFalse(result["metrics"]["determinism"])
        self.assertEqual(execute.call_count, 5)
        self.assertEqual((self.root / "failed/runs/5/stderr").read_bytes(), b"fixture launch failure\n")

    def test_timeout_retains_diagnostics_and_runs_the_remaining_processes(self):
        def timeout(command, **kwargs):
            kwargs["stderr"].write(b"fixture output before timeout\n")
            raise witness.subprocess.TimeoutExpired(command, kwargs["timeout"])
        with mock.patch.object(corpus, "build", side_effect=self.mock_build), \
                mock.patch.object(witness.subprocess, "run", side_effect=timeout) as execute:
            result = witness.run(self.root / "timeout", jobs=3, timeout=0.5)
        self.assertFalse(result["metrics"]["determinism"])
        self.assertEqual(execute.call_count, 5)
        self.assertTrue(all(row["timed_out"] for row in result["details"]["runs"]))
        invocation = witness.read_json(self.root / "timeout/runs/1/invocation.json")
        self.assertEqual(invocation["timeout_seconds"], 0.5)
        self.assertEqual((self.root / "timeout/runs/5/stderr").read_bytes(), b"fixture output before timeout\n")

    def test_source_closure_includes_the_witness_and_corpus_helpers(self):
        bound = REAL_WITNESS_SOURCES()
        self.assertEqual(bound["scripts/phase4_determinism.py"],
                         digest((ROOT / "scripts/phase4_determinism.py").read_bytes()))
        self.assertIn("scripts/phase4_corpus.py", bound)
        self.assertIn("scripts/phase4_scenarios.py", bound)


if __name__ == "__main__":
    unittest.main()
