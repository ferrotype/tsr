"""Phase 3 contract witnesses and their receipts (scripts/phase3_receipts.py)."""
from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import phase2_producers as phase2  # noqa: E402
import phase3_receipts as receipts  # noqa: E402


def passing(tests):
    return ("".join(f"test {name} ... ok\n" for name in tests)
            + f"\ntest result: ok. {len(tests)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; "
              "finished in 0.01s\n")


class Witnesses(unittest.TestCase):
    def test_every_checkpoint_has_a_witness_over_its_suite_in_debug_and_release(self):
        self.assertEqual(sorted(receipts.WITNESSES), [f"t{n}-contracts" for n in range(1, 9)])
        for identity, spec in receipts.WITNESSES.items():
            suite = identity.replace("-", "_")
            self.assertEqual(spec["test_source"], f"crates/tsr_compiler/tests/{suite}.rs")
            self.assertEqual(spec["commands"], [
                ["cargo", "test", "-p", "tsr_compiler", "--test", suite, "--locked"],
                ["cargo", "test", "-p", "tsr_compiler", "--test", suite, "--locked", "--release"]])
            for source in ("crates", "Cargo.lock", "Cargo.toml", "scripts/phase2_producers.py",
                           "scripts/phase3_receipts.py"):
                self.assertIn(source, spec["sources"], identity)
            # The inventory is the suite's top-level tests, at least the minimum.
            tests = phase2.witness_tests(spec)
            self.assertGreaterEqual(len(tests), spec["minimum_tests"], identity)

    def test_the_bound_sources_cover_the_suites_their_support_and_fixtures(self):
        inputs = phase2.source_inputs(receipts.WITNESSES["t3-contracts"]["sources"])
        for path in ("crates/tsr_compiler/tests/t3_contracts.rs",
                     "crates/tsr_compiler/tests/support/phase3_contracts.rs",
                     "crates/tsr_compiler/src/program_emit.rs", "crates/tsr_printer/src/printer.rs",
                     "crates/tsr_transformers/src/transformer.rs", receipts.STRESS):
            self.assertIn(path, inputs)

    def test_registration_is_scoped_to_one_call(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertIsNone(receipts.receipt_current("t1-contracts", Path(directory) / "absent.json"))
        self.assertNotIn("t1-contracts", phase2.WITNESSES)
        with self.assertRaisesRegex(ValueError, "unknown witness"):
            receipts.receipt_current("c1-contracts")
        with self.assertRaises(RuntimeError), receipts.registered("t2-contracts"):
            self.assertIn("t2-contracts", phase2.WITNESSES)
            raise RuntimeError
        self.assertNotIn("t2-contracts", phase2.WITNESSES)


class Receipt(unittest.TestCase):
    """A probe witness under a temporary root, observed through the real
    machinery with the suite's process replaced."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="phase3-receipt-"))
        self.addCleanup(shutil.rmtree, self.root)
        self.addCleanup(patch.stopall)
        suite = self.root / "tests/probe_contracts.rs"
        suite.parent.mkdir(parents=True)
        suite.write_text("#[test]\nfn first() {}\n#[test]\nfn second() {}\n")
        self.bound = self.root / "src/lib.rs"
        self.bound.parent.mkdir(parents=True)
        self.bound.write_text("pub fn production() {}\n")
        self.spec = {"commands": [["cargo", "test"], ["cargo", "test", "--release"]],
                     "test_source": "tests/probe_contracts.rs", "minimum_tests": 2, "test_modules": {},
                     "sources": ["src", "tests"]}
        patch.object(phase2, "ROOT", self.root).start()
        patch.dict(receipts.WITNESSES, {"probe": self.spec}).start()
        self.output = self.root / "receipts"

    def observe(self, stdout, code=0):
        completed = subprocess.CompletedProcess([], code, stdout.encode(), b"")
        with patch.object(phase2.subprocess, "run", return_value=completed):
            return receipts.observe("probe", self.output)

    def current(self):
        return receipts.receipt_current("probe", self.output / "probe.json")

    def test_a_passing_suite_yields_a_current_receipt_in_phase2s_format(self):
        record = self.observe(passing(["first", "second"]))
        self.assertEqual((record["version"], record["witness"], record["state"]), (2, "probe", "observed"))
        self.assertEqual(record["tests"], ["first", "second"])
        self.assertEqual([run["command"] for run in record["runs"]], self.spec["commands"])
        self.assertEqual(json.loads((self.output / "probe.json").read_text()), record)
        self.assertTrue(self.current())

    def test_a_receipt_goes_stale_when_a_bound_source_changes(self):
        self.observe(passing(["first", "second"]))
        self.bound.write_text("pub fn production() { let _changed = 1; }\n")
        self.assertFalse(self.current())
        self.bound.write_text("pub fn production() {}\n")
        self.assertTrue(self.current())
        added = self.bound.parent / "added.rs"
        added.write_text("pub fn added() {}\n")
        self.assertFalse(self.current())
        added.unlink()
        self.assertTrue(self.current())
        self.bound.unlink()
        self.assertFalse(self.current())

    def test_a_changed_test_inventory_stales_the_receipt(self):
        self.observe(passing(["first", "second"]))
        suite = self.root / "tests/probe_contracts.rs"
        suite.write_text(suite.read_text() + "#[test]\nfn third() {}\n")
        self.assertFalse(self.current())

    def test_a_failing_suite_never_yields_a_current_receipt(self):
        tests = ["first", "second"]
        failures = [
            (passing(tests).replace("test second ... ok", "test second ... FAILED"), 101),
            (passing(tests), 101),
            (passing(tests).replace("test second ... ok", "test second ... FAILED"), 0),
            (passing(["first"]), 0),
            (passing(tests).replace("test second ... ok", "test second ... ignored")
             .replace("0 ignored", "1 ignored"), 0),
            (passing(tests).replace("0 filtered out", "1 filtered out"), 0),
            (passing(["first", "other"]), 0),
        ]
        for stdout, code in failures:
            with self.assertRaisesRegex(ValueError, "probe failed"):
                self.observe(stdout, code)
            # The receipt is kept for diagnosis, and it is not current.
            self.assertTrue((self.output / "probe.json").is_file())
            self.assertFalse(self.current(), (stdout, code))
        self.observe(passing(tests))
        self.assertTrue(self.current())

    def test_a_missing_or_foreign_receipt_is_not_current(self):
        self.assertIsNone(self.current())
        self.observe(passing(["first", "second"]))
        record = json.loads((self.output / "probe.json").read_text())
        for field, value in (("witness", "t1-contracts"), ("version", 1), ("state", "unobserved")):
            (self.output / "probe.json").write_text(json.dumps(dict(record, **{field: value})))
            self.assertFalse(self.current(), field)


if __name__ == "__main__":
    unittest.main()
