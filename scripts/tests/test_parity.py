"""scripts/parity.py over a fake runner: sharding, crash and deadline handling,
check against the expectation file, accept keeping reasons and approvals."""
from __future__ import annotations

import contextlib
import io
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import parity  # noqa: E402

FAKE_RUNNER = r'''#!/usr/bin/env python3
import json, os, sys, time
args = sys.argv[1:]
VARIANTS = ["compiler/a.ts", "compiler/b(target=es2015).ts", "compiler/crash.ts", "compiler/slow.ts",
            "conformance/c.ts"]
if "list" in args:
    print("\n".join(VARIANTS))
    sys.exit(0)
variant = args[args.index("--id") + 1]
if variant == "compiler/crash.ts":
    print(json.dumps({"id": variant + "/error", "state": "pass"}))
    print("boom", file=sys.stderr)
    sys.exit(101)
if variant == "compiler/slow.ts":
    time.sleep(5)
for subtest in ("error", "types", "symbols"):
    failing = (variant, subtest) in {("compiler/a.ts", "types"), ("conformance/c.ts", "error")}
    if os.environ.get("FAKE_FIXED") and variant == "compiler/a.ts":
        failing = False
    row = {"id": f"{variant}/{subtest}", "state": "fail" if failing else "pass"}
    if failing:
        row["reason"] = "baseline differs"
        row["detail"] = "line 1\nline 2"
    print(json.dumps(row))
if variant == "compiler/b(target=es2015).ts":
    print(json.dumps({"id": variant + "/output", "state": "skip", "reason": "nondeterministic"}))
'''


def variants_of(rows):
    return {row["id"].rsplit("/", 1)[0] if row["id"].count("/") > 1 else row["id"] for row in rows}


class ParityTests(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="parity-"))
        self.runner = self.directory / "runner.py"
        self.runner.write_text(FAKE_RUNNER)
        self.runner.chmod(self.runner.stat().st_mode | stat.S_IEXEC)
        self.saved = parity.PARITY, parity.pin, parity.SUITES["compiler"]
        parity.PARITY = self.directory / "parity"
        parity.pin = lambda: "deadbeef"
        parity.SUITES["compiler"] = parity.Suite("x", "x", ("--suite", "compiler"), 1)
        self.addCleanup(self.restore)

    def restore(self):
        parity.PARITY, parity.pin, parity.SUITES["compiler"] = self.saved
        os.environ.pop("FAKE_FIXED", None)

    def run_shard(self, name, *extra):
        output = self.directory / name
        parity.main(["run", "compiler", "--runner", str(self.runner), "--output", str(output), "--jobs", "2",
                     "--timeout", "1", *extra])
        return [json.loads(line) for line in (output / "results.ndjson").read_text().splitlines()]

    def results(self, *names):
        return ["--results", *(str(self.directory / name) for name in names)]

    def expectation(self):
        return json.loads((self.directory / "parity/compiler.json").read_text())

    def capture(self, arguments):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = 0
            try:
                parity.main(arguments)
            except SystemExit as exit:
                code = exit.code
        return code, out.getvalue()

    def test_run_records_every_subtest_and_fails_crashes_and_deadlines_as_the_variant(self):
        rows = self.run_shard("out")
        by_id = {row["id"]: row for row in rows}
        self.assertEqual(by_id["compiler/a.ts/types"]["state"], "fail")
        self.assertEqual(by_id["compiler/b(target=es2015).ts/output"]["state"], "skip")
        self.assertEqual(by_id["compiler/crash.ts"],
                         {"id": "compiler/crash.ts", "state": "fail", "reason": "exit 101", "detail": "boom"})
        self.assertEqual(by_id["compiler/slow.ts"]["reason"], "deadline: 1 s")
        meta = json.loads((self.directory / "out/meta.json").read_text())
        self.assertEqual((meta["total"], meta["selected"], meta["shard"], meta["partial"]), (5, 5, [1, 1], False))
        self.assertEqual(meta["counts"]["fail"], 4)
        self.assertIn("compiler/slow.ts", {entry["variant"] for entry in meta["slowest"]})

    def test_shards_partition_the_sorted_variants(self):
        first = self.run_shard("one", "--shard", "1/2")
        second = self.run_shard("two", "--shard", "2/2")
        self.assertEqual(variants_of(first), {"compiler/a.ts", "compiler/crash.ts", "conformance/c.ts"})
        self.assertEqual(variants_of(second), {"compiler/b(target=es2015).ts", "compiler/slow.ts"})

    def test_check_requires_every_shard_once_and_refuses_partial_runs(self):
        self.run_shard("one", "--shard", "1/2")
        with self.assertRaisesRegex(SystemExit, "every shard"):
            parity.main(["check", "compiler", *self.results("one")])
        self.run_shard("dev", "--id", "compiler/a.ts")
        with self.assertRaisesRegex(SystemExit, "development check"):
            parity.main(["accept", "compiler", *self.results("dev")])

    def test_accept_then_check_passes_and_keeps_reasons_and_approvals(self):
        self.run_shard("one", "--shard", "1/2")
        self.run_shard("two", "--shard", "2/2")
        results = self.results("one", "two")
        parity.main(["accept", "compiler", *results])
        document = self.expectation()
        self.assertEqual((document["pin"], document["total"]), ("deadbeef", 5))
        self.assertEqual(list(document["failing"]), sorted(document["failing"]))
        self.assertEqual(document["failing"]["compiler/a.ts/types"], {"reason": "baseline differs", "detail": "line 1"})
        self.assertEqual(document["failing"]["compiler/crash.ts"]["reason"], "exit 101")
        # The owner annotates an entry; accept keeps the annotation.
        document["failing"]["conformance/c.ts/error"] = {"reason": "known TS6059 ordering", "approved": "owner 2026-10-03"}
        (self.directory / "parity/compiler.json").write_text(json.dumps(document))
        code, out = self.capture(["check", "compiler", *results])
        self.assertEqual(code, 0)
        self.assertIn("fail 4 (1 approved)", out)
        self.assertNotIn("new failure", out)
        parity.main(["accept", "compiler", *results])
        self.assertEqual(self.expectation()["failing"]["conformance/c.ts/error"]["approved"], "owner 2026-10-03")
        # A test that starts passing fails the check until accepted.
        os.environ["FAKE_FIXED"] = "1"
        self.run_shard("one", "--shard", "1/2")
        code, out = self.capture(["check", "compiler", *results])
        self.assertEqual(code, 1)
        self.assertIn("1 named failure(s) now pass", out)
        self.assertIn("compiler/a.ts/types", out)
        parity.main(["accept", "compiler", *results])
        self.assertNotIn("compiler/a.ts/types", self.expectation()["failing"])

    def test_check_reports_new_failures_pin_and_denominator_drift(self):
        self.run_shard("out")
        parity.main(["accept", "compiler", *self.results("out")])
        document = self.expectation()
        document["pin"], document["total"] = "cafe", 4
        del document["failing"]["compiler/crash.ts"]
        (self.directory / "parity/compiler.json").write_text(json.dumps(document))
        code, out = self.capture(["check", "compiler", *self.results("out")])
        self.assertEqual(code, 1)
        self.assertIn("pin deadbeef differs", out)
        self.assertIn("5 variants enumerated; the expectation file records 4", out)
        self.assertIn("1 new failure(s)", out)
        self.assertIn("compiler/crash.ts: exit 101", out)

    def test_step_summary_is_appended_when_github_sets_it(self):
        self.run_shard("out")
        parity.main(["accept", "compiler", *self.results("out")])
        summary = self.directory / "summary.md"
        os.environ["GITHUB_STEP_SUMMARY"] = str(summary)
        self.addCleanup(os.environ.pop, "GITHUB_STEP_SUMMARY", None)
        self.capture(["check", "compiler", *self.results("out")])
        self.assertTrue(summary.read_text().startswith("### parity: compiler"))


if __name__ == "__main__":
    unittest.main()
