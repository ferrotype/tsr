"""Embedding parity routing, Cargo artifact selection and process deadlines.

All executables are fixtures; these tests never invoke Cargo or a compiler.
"""
from __future__ import annotations

import contextlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import parity


def fixture_identity(suite):
    if suite.endswith('-embed'):
        return {'version': 1, 'runtime': 'embed', 'consumer_sha256': '0' * 64}
    family = suite.rsplit('-', 1)[0]
    return {'version': 1, 'runtime': 'wasm', 'family': family,
            'engine': {'version': 'v24.20.0', 'flags': ['--stack-size=4096'], 'node_options': None, 'sha256': '1' * 64},
            'build_sha256': '2' * 64,
            'artifacts': {('compiler' if family == 'compiler' else 'checker') + '_bg.wasm': '3' * 64,
                          'bindings.mjs': '4' * 64},
            'transport': {'wasm-consumer': '5' * 64}}


RUNNER = r'''#!/usr/bin/env python3
import json, subprocess, sys
args = sys.argv[1:]
suite = args[args.index("--suite") + 1]
assert suite in ("transpile", "compiler")
variants = [suite + "/b(sourceMap=true)", suite + "/a"]
if "list" in args:
    assert "--consumer" not in args
    print("\n".join(variants))
else:
    assert args[args.index("--backend") + 1] == "embed"
    consumer = args[args.index("--consumer") + 1]
    result = subprocess.run([consumer], capture_output=True, text=True)
    variant = args[args.index("--id") + 1]
    for kind in (("module", "declaration") if suite == "transpile" else ("error", "type", "output")):
        print(json.dumps({"id": variant + "/" + kind,
            "state": "pass" if result.returncode == 0 else "fail",
            "reason": "baseline differs" if result.returncode else ""}))
'''


class EmbedParityTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="parity-embed-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.runner = self.executable("runner", RUNNER)
        self.consumer = self.executable("consumer", "#!/usr/bin/env python3\n")
        self.parity_patch = patch.object(parity, "PARITY", self.directory / "expectations")
        self.parity_patch.start()
        self.addCleanup(self.parity_patch.stop)

    def executable(self, name, source):
        path = self.directory / name
        if source.startswith("#!/usr/bin/env python3\n"):
            source = source.replace("#!/usr/bin/env python3\n", '#!/usr/bin/env python3\nimport sys, json\nif sys.argv[1:] == ["--identity"]:\n    print(json.dumps({"version": 1, "runtime": "embed"})); sys.exit(0)\n', 1)
        path.write_text(source)
        path.chmod(path.stat().st_mode | stat.S_IEXEC)
        return path

    def arguments(self, *extra, suite="transpile-embed"):
        return ["run", suite, "--runner", str(self.runner),
                "--output", str(self.directory / "results"), "--jobs", "1", *extra]

    def quiet(self, arguments):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            parity.main(arguments)

    def test_embed_roster_is_identical_to_native_transpile_without_a_consumer_build(self):
        with patch.object(parity, "consumer_path", side_effect=AssertionError("consumer build")):
            native = parity.list_variants("transpile", self.runner)
            embedded = parity.list_variants("transpile-embed", self.runner)
            wasm = parity.list_variants("transpile-wasm", self.runner)
            self.quiet(["list", "transpile-embed", "--runner", str(self.runner)])
            self.quiet(["list", "transpile-wasm", "--runner", str(self.runner)])
        self.assertEqual(native, ["transpile/a", "transpile/b(sourceMap=true)"])
        self.assertEqual(embedded, native)
        self.assertEqual(wasm, native)

    def test_run_forwards_explicit_consumer_and_keeps_both_operation_results(self):
        with patch.object(parity.subprocess, "run", wraps=subprocess.run) as run:
            self.quiet(self.arguments("--consumer", str(self.consumer)))
        self.assertFalse(any(command.args[0][0] == "cargo" for command in run.call_args_list))
        rows = [json.loads(row) for row in
                (self.directory / "results/results.ndjson").read_text().splitlines()]
        self.assertEqual(len(rows), 4)
        self.assertTrue(all(row["state"] == "pass" for row in rows))
        self.assertEqual({row["id"].rsplit("/", 1)[-1] for row in rows}, {"module", "declaration"})
        meta = json.loads((self.directory / "results/meta.json").read_text())
        self.assertEqual(meta["suite"], "transpile-embed")
        self.assertEqual(meta["consumer"], str(self.consumer.resolve()))
        self.assertEqual(meta['portable_identity'], {'version': 1, 'runtime': 'embed',
                                                    'consumer_sha256': parity.sha256(self.consumer)})
        self.assertEqual(meta["total"], 2)

    def test_runner_override_still_resolves_or_builds_a_consumer(self):
        with patch.object(parity, "consumer_path", return_value=self.consumer) as resolve:
            self.quiet(self.arguments())
        resolve.assert_called_once_with(None)

    def test_wasm_requires_an_explicit_host_before_any_build(self):
        with patch.object(parity, "consumer_path", side_effect=AssertionError("native consumer build")), \
                patch.object(parity, "runner_path", side_effect=AssertionError("runner build")):
            for suite in ("transpile-wasm", "compiler-wasm"):
                with self.subTest(suite=suite), self.assertRaisesRegex(SystemExit, "requires --consumer"):
                    self.quiet(self.arguments(suite=suite))

    def test_compiler_suites_share_native_roster_and_forward_external_consumer(self):
        native = parity.list_variants("compiler", self.runner)
        for suite in ("compiler-embed", "compiler-wasm"):
            with self.subTest(suite=suite):
                with patch.object(parity, "consumer_path", side_effect=AssertionError("consumer build")):
                    self.assertEqual(parity.list_variants(suite, self.runner), native)
                with patch.object(parity, 'portable_identity', return_value=fixture_identity(suite)):
                    self.quiet(self.arguments("--consumer", str(self.consumer), suite=suite))
                rows = [json.loads(line) for line in
                        (self.directory / "results/results.ndjson").read_text().splitlines()]
                self.assertEqual(len(rows), 6)
                self.assertTrue(all(row["state"] == "pass" for row in rows))
                (self.directory / "results").rename(self.directory / suite)

    def test_wasm_runs_an_explicit_host_without_building_a_native_consumer(self):
        with patch.object(parity.subprocess, "run", wraps=subprocess.run) as run, \
                patch.object(parity, 'portable_identity', return_value=fixture_identity('transpile-wasm')):
            self.quiet(self.arguments("--consumer", str(self.consumer), suite="transpile-wasm"))
        self.assertFalse(any(command.args[0][0] == "cargo" for command in run.call_args_list))
        meta = json.loads((self.directory / "results/meta.json").read_text())
        self.assertEqual(meta["suite"], "transpile-wasm")
        self.assertEqual(meta["counts"], {"pass": 4, "fail": 0, "skip": 0})

    def test_embed_and_wasm_results_and_expectations_are_isolated(self):
        self.quiet(self.arguments("--consumer", str(self.consumer)))
        embedded = self.directory / "embedded"
        (self.directory / "results").rename(embedded)
        with patch.object(parity, 'portable_identity', return_value=fixture_identity('transpile-wasm')):
            self.quiet(self.arguments("--consumer", str(self.consumer), suite="transpile-wasm"))
        wasm = self.directory / "results"
        with patch.object(parity, "consumer_path", side_effect=AssertionError("consumer build")), \
                patch.object(parity, "runner_path", side_effect=AssertionError("runner build")):
            for suite, directory in (("transpile-embed", embedded), ("transpile-wasm", wasm)):
                self.quiet(["accept", suite, "--results", str(directory)])
                self.quiet(["check", suite, "--results", str(directory)])
            with self.assertRaisesRegex(SystemExit, "not transpile-wasm"):
                self.quiet(["check", "transpile-wasm", "--results", str(embedded)])
            with self.assertRaisesRegex(SystemExit, "not transpile-embed"):
                self.quiet(["accept", "transpile-embed", "--results", str(wasm)])
        self.assertTrue((self.directory / "expectations/transpile-embed.json").exists())
        self.assertTrue((self.directory / "expectations/transpile-wasm.json").exists())

    def test_wasm_consumer_cannot_be_graded_as_embed(self):
        identity = fixture_identity('compiler-wasm')
        with patch.object(parity.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps(identity), '')):
            for suite in ('compiler-embed', 'transpile-embed'):
                with self.subTest(suite=suite), self.assertRaisesRegex(SystemExit, 'embed consumer identity'):
                    parity.portable_identity(suite, self.consumer)

    def test_node_options_are_required_and_preserved(self):
        identity = fixture_identity('compiler-wasm')
        consumer = parity.ROOT / 'tools/phase7/portable/wasm-consumer'
        identity['engine']['node_options'] = '--max-old-space-size=4096'
        with patch.object(parity.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps(identity), '')):
            self.assertEqual(parity.portable_identity('compiler-wasm', consumer)['engine']['node_options'], '--max-old-space-size=4096')
        del identity['engine']['node_options']
        with self.assertRaisesRegex(SystemExit, 'invalid verified'):
            parity.validate_portable_identity('compiler-wasm', identity)

    def test_native_consumer_cannot_be_graded_as_wasm(self):
        for suite in ('compiler-wasm', 'transpile-wasm'):
            with self.subTest(suite=suite), self.assertRaisesRegex(SystemExit, 'verified wasm launcher'):
                self.quiet(self.arguments('--consumer', str(self.consumer), suite=suite))
        self.assertFalse((self.directory / 'results/meta.json').exists())

    def test_verified_wasm_identity_requires_pinned_engine_and_artifact_loader_success(self):
        consumer = parity.ROOT / 'tools/phase7/portable/wasm-consumer'
        identity = fixture_identity('compiler-wasm')
        for result, message in (
            (subprocess.CompletedProcess([], 1, '', 'stale artifact'), 'verification failed'),
            (subprocess.CompletedProcess([], 0, '{}', ''), 'invalid verified'),
            (subprocess.CompletedProcess([], 0, json.dumps({**identity, 'runtime': 'embed'}), ''), 'invalid verified'),
            (subprocess.CompletedProcess([], 0, json.dumps({**identity, 'engine': {**identity['engine'], 'version': 'v0'}}), ''), 'pinned Node'),
        ):
            with self.subTest(message=message), patch.object(parity.subprocess, 'run', return_value=result), \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaisesRegex(SystemExit, message):
                parity.portable_identity('compiler-wasm', consumer)
        with patch.object(parity.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps(identity), '')) as probe:
            recorded = parity.portable_identity('compiler-wasm', consumer)
        self.assertEqual(recorded['artifacts'], identity['artifacts'])
        self.assertEqual(recorded['transport']['wasm-consumer'], parity.sha256(consumer))
        self.assertEqual(probe.call_args.args[0], [str(consumer), '--compiler', '--identity'])

    def test_changed_consumer_during_capture_does_not_write_success_metadata(self):
        before = fixture_identity('transpile-embed')
        after = {**before, 'consumer_sha256': '1' * 64}
        with patch.object(parity, 'portable_identity', side_effect=[before, after]), \
                self.assertRaisesRegex(SystemExit, 'changed during capture'):
            self.quiet(self.arguments('--consumer', str(self.consumer)))
        self.assertFalse((self.directory / 'results/meta.json').exists())

    def test_consumer_failure_is_preserved_without_native_fallback(self):
        consumer = self.executable("failing-consumer", "#!/usr/bin/env python3\nraise SystemExit(31)\n")
        self.quiet(self.arguments("--consumer", str(consumer)))
        rows = [json.loads(row) for row in
                (self.directory / "results/results.ndjson").read_text().splitlines()]
        self.assertEqual(len(rows), 4)
        self.assertTrue(all(row["state"] == "fail" for row in rows))
        self.assertTrue(all(row["reason"] == "baseline differs" for row in rows))

    def test_consumer_option_is_rejected_outside_embed_run_before_any_build(self):
        with patch.object(parity, "runner_path", side_effect=AssertionError("runner build")):
            with self.assertRaisesRegex(SystemExit, "only to compiler/transpile"):
                self.quiet(["run", "transpile", "--output", str(self.directory / "results"),
                            "--consumer", str(self.consumer)])
            for command in ("list", "check", "accept"):
                arguments = [command, "transpile-embed", "--consumer", str(self.consumer)]
                if command != "list":
                    arguments.extend(("--results", str(self.directory / "results")))
                with self.subTest(command=command), self.assertRaises(SystemExit):
                    self.quiet(arguments)

    def test_nonexecutable_or_missing_explicit_consumer_fails_without_building(self):
        nonexecutable = self.directory / "text"
        nonexecutable.write_text("not executable")
        with patch.object(parity.subprocess, "run", side_effect=AssertionError("unexpected build")):
            for candidate in (self.directory / "missing", nonexecutable, self.directory):
                with self.subTest(candidate=candidate), self.assertRaisesRegex(SystemExit, "not an executable file"):
                    parity.consumer_path(candidate)

    def artifact(self, executable, **changes):
        artifact = {"reason": "compiler-artifact", "executable": str(executable),
                    "manifest_path": str(parity.ROOT / "tools/s10/rust-consumer/Cargo.toml"),
                    "target": {"name": "s10_rust_consumer", "kind": ["bin"]}}
        return {**artifact, **changes}

    def test_consumer_build_uses_exact_cargo_artifact_with_caller_configuration(self):
        selected = self.executable("s10_rust_consumer", "#!/usr/bin/env python3\n")
        artifacts = [self.artifact(self.directory / "wrong", target={"name": "other", "kind": ["bin"]}),
                     self.artifact(selected), {"reason": "build-finished", "success": True}]
        def build(command, **kwargs):
            self.assertIn("--release", command)
            self.assertIn("--locked", command)
            self.assertIn("--manifest-path", command)
            self.assertIn("--message-format=json-render-diagnostics", command)
            self.assertNotIn("env", kwargs)
            return subprocess.CompletedProcess(command, 0, "\n".join(map(json.dumps, artifacts)), "")
        with patch.object(parity.subprocess, "run", side_effect=build):
            self.assertEqual(parity.consumer_path(), selected.resolve())

    def test_failed_or_ambiguous_cargo_consumer_selection_never_guesses_a_path(self):
        selected = self.executable("s10_rust_consumer", "#!/usr/bin/env python3\n")
        artifact = self.artifact(selected)
        cases = [subprocess.CompletedProcess([], 1, "", "build failed"),
                 subprocess.CompletedProcess([], 0, "", ""),
                 subprocess.CompletedProcess([], 0, json.dumps(self.artifact(selected,
                     manifest_path=str(self.directory / "unrelated/Cargo.toml"))), ""),
                 subprocess.CompletedProcess([], 0, json.dumps(artifact) + "\n" + json.dumps(artifact), "")]
        for completed in cases:
            with self.subTest(completed=completed), patch.object(parity.subprocess, "run", return_value=completed):
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                    parity.consumer_path()

    def test_check_and_accept_never_resolve_or_build_consumers(self):
        self.quiet(self.arguments("--consumer", str(self.consumer)))
        results = ["--results", str(self.directory / "results")]
        with patch.object(parity, "consumer_path", side_effect=AssertionError("consumer build")), \
                patch.object(parity, "runner_path", side_effect=AssertionError("runner build")):
            self.quiet(["accept", "transpile-embed", *results])
            self.quiet(["check", "transpile-embed", *results])
        self.assertTrue((self.directory / "expectations/transpile-embed.json").exists())
        self.assertFalse((self.directory / "expectations/transpile.json").exists())

    @unittest.skipUnless(os.name == "posix", "process-group deadline witness requires POSIX")
    def test_deadline_kills_consumer_descendants_and_records_variant_failure(self):
        heartbeat = self.directory / "heartbeat"
        child = self.executable("child", "#!/usr/bin/env python3\nimport pathlib,time\n"
            f"path = pathlib.Path({str(heartbeat)!r})\n"
            "while True:\n    path.write_text(str(time.monotonic()))\n    time.sleep(0.01)\n")
        runner = self.executable("hanging-runner", "#!/usr/bin/env python3\nimport subprocess,time\n"
            f"subprocess.Popen([{str(child)!r}])\ntime.sleep(60)\n")
        for suite in parity.PORTABLE_SUITES:
            with self.subTest(suite=suite):
                heartbeat.unlink(missing_ok=True)
                rows, _ = parity.run_variant(suite, runner, "transpile/hang",
                                            self.directory, 2, self.consumer)
                self.assertEqual(rows, [{"id": "transpile/hang", "state": "fail", "reason": "deadline: 2 s"}])
                self.assertTrue(heartbeat.exists(), "descendant actually ran before the deadline")
                stopped = heartbeat.read_text()
                time.sleep(0.1)
                self.assertEqual(heartbeat.read_text(), stopped, "descendant survived the runner deadline")


if __name__ == "__main__":
    unittest.main()
