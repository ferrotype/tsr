"""Phase 1 producer closures and independently required tracker consumers.

These check the real ledger declarations and Git pathspec semantics. They do
not execute a corpus or make unfinished feature parity a harness-health gate.
"""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))


def runs():
    return tomllib.loads((ROOT / "status/runs.toml").read_text())


def selected(root, specification):
    """Use the same recursive Git-glob selection as xtask::evidence."""
    patterns = [f":(top,glob){pattern}" for pattern in specification["sources"]]
    output = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", *patterns],
        cwd=root,
    )
    excluded = {"STATUS.md", "docs/status.html", "status/status.json", "status/history.jsonl",
                "status/unmapped-functions.json"}
    return {name.decode() for name in output.split(b"\0") if name
            and name.decode() not in excluded
            and not name.decode().startswith(("status/evidence/", "target/"))}


def fingerprint(root, specification):
    return {
        name: hashlib.sha256((root / name).read_bytes()).hexdigest()
        for name in selected(root, specification)
        if (root / name).is_file()
    }


class ProducerClosureTests(unittest.TestCase):
    def test_grouped_ledger_covers_each_computed_capture_and_report_input(self):
        import phase1_producers

        ledger = runs()
        for producer in ("foundations", "config", "syntax"):
            with self.subTest(producer=producer):
                closure = phase1_producers.source_closure(producer)
                self.assertTrue(closure, "an empty closure is not evidence")
                self.assertFalse(
                    {name if not name.startswith("upstream/") else "upstream" for name in closure}
                    - selected(ROOT, ledger[producer]),
                    f"{producer} ledger omits capture/report inputs",
                )
                self.assertEqual(
                    ledger[producer]["command"],
                    ["python3", "scripts/phase1_producers.py", producer],
                )

    def test_nested_inputs_and_build_configuration_change_the_ledger_fingerprint(self):
        ledger = runs()
        # Nested additions are the dangerous case for `**` interpreted as a
        # directory rather than recursive files. Use actual Git selection.
        examples = {
            "foundations": (
                "crates/tsr_core/src/nested/contract.rs",
                "tools/phase1/filesystem/nested/oracle.go",
                "data/phase1/requests/nested/required.json",
                "scripts/phase1_integration.py",
            ),
            "config": (
                "crates/tsr_tsoptions/src/nested/options.rs",
                "tools/phase1/config/nested/oracle.go",
                "data/phase1/native/config/nested/observations.json",
                "scripts/phase1_producers.py",
            ),
            "syntax": (
                "crates/tsr_ast/src/nested/children.rs",
                "tools/phase1/syntax/nested/oracle.go",
                "data/phase1/requests/nested/required.json",
                "scripts/phase1_syntax.py",
            ),
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            for producer, paths in examples.items():
                for relative in (*paths, ".cargo/nested/config.toml", "Cargo.lock"):
                    with self.subTest(producer=producer, path=relative):
                        path = root / relative
                        path.parent.mkdir(parents=True, exist_ok=True)
                        before = fingerprint(root, ledger[producer])
                        path.write_bytes(b"first observation\n")
                        added = fingerprint(root, ledger[producer])
                        self.assertNotEqual(before, added, relative)
                        self.assertIn(relative, added)
                        path.write_bytes(b"changed observation\n")
                        changed = fingerprint(root, ledger[producer])
                        self.assertNotEqual(added, changed, relative)
                        path.unlink()
                        self.assertEqual(before, fingerprint(root, ledger[producer]))

    def test_unrelated_documentation_does_not_stale_code_captures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            document = root / "docs/unrelated-phase1-note.md"
            document.parent.mkdir()
            for producer in ("foundations", "config", "syntax"):
                specification = runs()[producer]
                before = fingerprint(root, specification)
                document.write_text("An editorial change, not a producer input.\n")
                self.assertEqual(before, fingerprint(root, specification), producer)
                document.unlink()

    def test_real_package_readme_is_excluded_from_replay_but_is_an_archive_input(self):
        import phase1_producers
        package_readme = ROOT / "crates/tsr_core/README.md"
        self.assertTrue(package_readme.is_file())
        name = str(package_readme.relative_to(ROOT))
        ledger = runs()
        for producer in ("config", "syntax"):
            with self.subTest(producer=producer):
                self.assertNotIn(name, selected(ROOT, ledger[producer]))
                self.assertNotIn(name, phase1_producers.source_closure(producer))
        self.assertIn(name, selected(ROOT, ledger["foundations"]))
        self.assertIn(name, phase1_producers.source_closure("foundations"))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            path = root / name
            path.parent.mkdir(parents=True)
            path.write_text("original published README")
            before = {producer: fingerprint(root, ledger[producer]) for producer in ("foundations", "config", "syntax")}
            path.write_text("changed published README")
            for producer in ("config", "syntax"):
                self.assertEqual(before[producer], fingerprint(root, ledger[producer]))
            self.assertNotEqual(before["foundations"], fingerprint(root, ledger["foundations"]))

    def test_generation_tracks_locale_output_and_its_actual_native_builder(self):
        specification = runs()["gen"]
        self.assertEqual(specification["command"], ["python3", "scripts/phase1_generation.py"])
        paths = selected(ROOT, specification)
        for name in (
            "scripts/phase1_generation.py", "scripts/generate_locale_tables.py",
            "scripts/s04.py", "scripts/tracking-bootstrap.py",
            "data/phase1/locale-tables-manifest.json",
        ):
            self.assertIn(name, paths)
        self.assertTrue(any(name.startswith("tools/phase1/locale/") for name in paths))


class SprintConsumersTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.preparation = tomllib.loads((ROOT / "sprints/P1A.toml").read_text())
        cls.production = tomllib.loads((ROOT / "sprints/P1B.toml").read_text())

    def require_in_item_and_exit(self, document, item_id, expressions):
        item = next(item for item in document["item"] if item["id"] == item_id)
        self.assertTrue(item["required"])
        for expression in expressions:
            with self.subTest(item=item_id, expression=expression):
                # A direct `== true` clause makes both false and missing
                # metrics fail/pending under the tracker's existing evaluator.
                # Requiring it in both places prevents a family being masked by
                # another metric or omitted from the complete sprint exit.
                self.assertIn(expression, item["done_when"])
                self.assertIn(expression, document["exit"])

    def test_each_preparation_boolean_is_independently_required(self):
        groups = {
            "F0": ("foundations.inventory_complete", "foundations.harness_pass"),
            "F1a": ("foundations.leaves_prepared",),
            "F2a": ("foundations.filesystem_prepared",),
            "F3a": ("config.prepared",),
            "F4a": ("syntax.prepared", "foundations.utilities_prepared"),
            "F5a": ("foundations.integration_prepared",),
        }
        for item, metrics in groups.items():
            self.require_in_item_and_exit(
                self.preparation, f"P1A-{item}", [f"run.{name} == true" for name in metrics]
            )
        for expression in self.preparation["exit"]:
            self.assertNotIn(".parity", expression)
            self.assertNotIn(".leaves_complete", expression)
            self.assertNotIn(".filesystem_complete", expression)

    def test_each_production_family_cannot_be_closed_by_another_family(self):
        groups = {
            "F1b": ["run.foundations.leaves_complete == true"],
            "F2b": ["run.foundations.filesystem_complete == true"],
            "F3b": ["run.config.inventory_complete == true", "run.config.tests_total == 309",
                    "run.config.parity == 1", "run.config.direct_complete == true"],
            "F4b": ["run.foundations.utilities_complete == true", "run.syntax.inventory_complete == true",
                    "run.syntax.parity == 1", "run.e1.parity == 1", "run.e1.frozen_denominator == true",
                    "run.binder.parity == 1", "run.binder.reached_bind == true",
                    "run.binder.supplemental_parity == 1", "run.binder.graph_contracts == true"],
            "F5b": ["run.foundations.inventory_complete == true", "run.foundations.integration_complete == true",
                    "run.gen.locale_complete == true", "run.gen.client_identical == true",
                    "run.testhost.parity == 1", "run.testhost.controls == true"],
        }
        for item, expressions in groups.items():
            self.require_in_item_and_exit(self.production, f"P1B-{item}", expressions)
        self.assertIn("sprint.P1A.done == 1", self.production["exit"])

    def test_phase1_does_not_borrow_e1_threshold_or_reopen_timing_gates(self):
        expressions = self.production["exit"] + [
            expression for item in self.production["item"] for expression in item["done_when"]
        ]
        self.assertIn("run.e1.parity == 1", expressions)
        for expression in expressions:
            self.assertNotIn("exp.E1.", expression)
            for timing in ("run.e5.", "run.e6.", "run.checkerbench.", "run.relater.", "exp.E5.", "exp.E6."):
                self.assertNotIn(timing, expression)

    def test_config_and_syntax_have_runner_owned_case_denominators(self):
        ledger = runs()
        self.assertEqual(ledger["config"]["cases"], "data/phase1/config-cases.json")
        self.assertEqual(ledger["syntax"]["cases"], "data/phase1/syntax-cases.json")
        index = json.loads((ROOT / "data/phase1/config-baselines.json").read_text())
        expected = {row["output"].removeprefix("tsc/testdata/baselines/reference/")
                    for group in index["groups"].values() for row in group["outputs"]}
        observed = json.loads((ROOT / ledger["config"]["cases"]).read_text())
        self.assertEqual(len(observed), 309)
        self.assertEqual(len(set(observed)), 309)
        self.assertEqual(set(observed), expected)

    def test_ci_gates_harness_health_without_requiring_unfinished_parity(self):
        workflow = (ROOT / ".github/workflows/status.yml").read_text()
        self.assertIn("run: python3 scripts/phase1_producers.py check", workflow)
        self.assertNotIn("cargo xtask check P1A", workflow)
        self.assertNotIn("cargo xtask check P1B", workflow)
        self.assertNotIn("cargo xtask run foundations", workflow)
        self.assertNotIn("cargo xtask run config", workflow)
        self.assertNotIn("cargo xtask run syntax", workflow)


if __name__ == "__main__":
    unittest.main()
