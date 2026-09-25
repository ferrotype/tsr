"""The grouped tracker adapters must not turn incomplete captures into parity."""
import contextlib
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_producers as p


class AggregationTests(unittest.TestCase):
    def setUp(self):
        self.qualifier = patch.object(p, "qualifications", return_value={})
        self.qualifier.start()
        self.addCleanup(self.qualifier.stop)
        self.health = {"healthy": True, "preparations": {family: {"complete": True}
                       for family in ("leaves", "filesystem", "syntax", "config")},
                       "integration": {"prepared": True, "complete": False}, "coverage": {"preparation_complete": False}}

    def test_comparator_controls_are_executed_not_assumed(self):
        self.assertTrue(all(p.comparator_controls().values()))
        with patch.object(p, "require_rows", return_value={"one": {}, "two": {}}):
            controls = p.comparator_controls()
        self.assertFalse(controls["removed"])
        self.assertFalse(controls["duplicate"])

    def test_partial_duplicate_extra_and_old_order_never_pass(self):
        good = [{"case": name, "result": "match"} for name in ("one", "two")]
        for rows in (good[:1], [good[0], good[0]], good + [good[0]], good[::-1]):
            with self.subTest(rows=rows), self.assertRaisesRegex(ValueError, "exactly once"):
                p.require_rows(rows, ["one", "two"])
        for result in ("not_run", "harness_failed", "approved", "invented"):
            with self.subTest(result=result), self.assertRaises(ValueError):
                p.require_rows([{"case": "one", "result": result}], ["one"])

    def test_unavailable_and_missing_behavior_cannot_be_green(self):
        for result in ("native_unavailable", "not_implemented", "different"):
            report = {"rows": [{"case": "one", "result": result}]}
            with patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}):
                out = p.aggregate("foundations", {"leaves": report}, self.health)
            self.assertFalse(out["metrics"]["leaves_complete"])
            self.assertTrue(out["metrics"]["leaves_prepared"])
            self.assertFalse(out["metrics"]["filesystem_complete"])
            self.assertFalse(out["metrics"]["integration_complete"])

    def test_empty_report_is_not_vacuous_success(self):
        out = p.aggregate("foundations", {}, self.health)
        for metric in ("leaves_complete", "filesystem_complete", "utilities_complete", "leaves_prepared"):
            self.assertFalse(out["metrics"][metric])
        with self.assertRaises(ValueError):
            p.require_rows([], [])

    def test_config_output_owners_are_exactly_309_with_matchfiles_once(self):
        owners = p.baseline_owners()
        self.assertEqual(len(owners), 309)
        self.assertEqual(sum(family == "filesystem" for family, _ in owners.values()), 142)
        self.assertEqual(len(set(owners.values())), 309)
        self.assertEqual(p.case_manifest_problems(), [])

    def test_removed_required_case_is_detected_before_aggregation(self):
        original = p.capture.load_requests
        def removed(spec):
            value = copy.deepcopy(original(spec))
            if spec == p.capture.FAMILIES["filesystem"]:
                value["requests"] = [r for r in value["requests"] if not r.get("baseline")][1:]
            return value
        with patch.object(p.capture, "load_requests", side_effect=removed), self.assertRaises(ValueError):
            p.baseline_owners()

    def test_config_baselines_do_not_close_supplementary_direct_cases(self):
        with patch.object(p, "baseline_owners", return_value={"out": ("config", "base")}), \
             patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "base"}, {"case": "direct"}]}):
            result = p.aggregate("config", {"config": {"rows": [
                {"case": "base", "result": "match"}, {"case": "direct", "result": "different"}]}}, self.health)
        self.assertEqual(result["tests"], {"out": "pass"})
        self.assertFalse(result["metrics"]["direct_complete"])
        self.assertNotIn("parity", result["metrics"])  # xtask derives this.

    def test_manifest_failure_blocks_every_completion_metric(self):
        self.health["healthy"] = False
        with patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}):
            result = p.aggregate("foundations", {"leaves": {"rows": [{"case": "one", "result": "match"}]}}, self.health)
        self.assertFalse(any(result["metrics"].values()))

    def test_authenticated_replay_rejects_tampering_and_partial_capture(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps({"family": "leaves", "source_closure": {"source.rs": "old"}}))
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={"source.rs": "new"}), \
                 patch.object(p.capture, "compare", return_value={"family": "leaves", "partial": False}) as compare:
                with self.assertRaisesRegex(ValueError, "inputs changed"):
                    p.replay_family("leaves", root)
                compare.assert_called_once()  # Artifact integrity precedes staleness.
            (root / "provenance.json").write_text(json.dumps({"family": "leaves", "source_closure": {}}))
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={}), \
                 patch.object(p.capture, "compare", side_effect=ValueError("capture artifact changed")):
                with self.assertRaisesRegex(ValueError, "artifact changed"):
                    p.replay_family("leaves", root)
            with patch.object(p, "rust_packages", return_value=[]), \
                 patch.object(p.capture, "source_closure", return_value={}), \
                 patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}), \
                 patch.object(p.capture, "compare", return_value={"family": "leaves", "partial": True}):
                with self.assertRaisesRegex(ValueError, "complete capture"):
                    p.replay_family("leaves", root)

    def test_wrong_family_is_invalid_even_when_its_sources_are_stale(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "provenance.json").write_text(json.dumps({"family": "config", "source_closure": {"old": "old"}}))
            with patch.object(p.capture, "compare", side_effect=p.capture.StaleCapture("changed source")) as compare:
                with self.assertRaisesRegex(ValueError, "another family") as error:
                    p.replay_family("leaves", root)
                self.assertNotIsInstance(error.exception, p.capture.StaleCapture)
                compare.assert_not_called()
            (root / "provenance.json").write_text(json.dumps({"family": "leaves", "partial": True,
                                                              "source_closure": {"old": "old"}}))
            with patch.object(p.capture, "compare", side_effect=p.capture.StaleCapture("changed source")) as compare:
                with self.assertRaisesRegex(ValueError, "complete capture") as error:
                    p.replay_family("leaves", root)
                self.assertNotIsInstance(error.exception, p.capture.StaleCapture)
                compare.assert_not_called()

    def test_program_smoke_or_stale_full_never_passes(self):
        for mode, current in (("smoke", True), ("full", False)):
            with patch.object(p.syntax, "schedule_problems", return_value=[]), \
                 patch.object(p.syntax, "replay", return_value={"selection": mode, "rust_sources_current": current}), \
                 self.assertRaisesRegex(ValueError, "current full"):
                p.replay_program(Path("unused"))

    def test_stale_family_is_unavailable_without_discarding_other_family(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            captures = {family: directory / family for family in ("config", "filesystem")}
            for path in captures.values():
                path.mkdir()
            def replay(family, _):
                if family == "config":
                    raise p.capture.StaleCapture("config production input changed")
                return {"capture_identity": "f" * 64, "host": {"goos": "darwin"},
                        "rows": [{"case": "one", "result": "match"}]}
            with patch.object(p, "source_closure", return_value={"input": "a" * 64}), \
                 patch.object(p, "harness_check", return_value=self.health), \
                 patch.object(p, "replay_family", side_effect=replay), \
                 patch.object(p, "baseline_owners", return_value={"fs-output": ("filesystem", "one")}), \
                 patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "one"}]}):
                result = p.produce("config", captures, directory / "reports", platform_captures=[])
            self.assertEqual(result["tests"], {"fs-output": "pass"})
            self.assertFalse(result["metrics"]["direct_complete"])
            detail = p.read(next((directory / "reports").glob("config-*.json")))
            self.assertEqual(detail["unavailable"], {"config": "config production input changed"})
            self.assertEqual(set(detail["reports"]), {"filesystem"})

    def test_invalid_capture_is_not_downgraded_to_stale(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(p, "source_closure", return_value={"input": "a" * 64}), \
                 patch.object(p, "harness_check", return_value=self.health), \
                 patch.object(p, "replay_family", side_effect=ValueError("changed artifact")), \
                 self.assertRaisesRegex(ValueError, "changed artifact"):
                p.produce("config", {"config": Path(tmp)}, Path(tmp) / "reports")

    def test_explicit_host_with_stale_base_retains_current_independent_family(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            captures = {family: directory / family for family in ("config", "filesystem")}
            for path in captures.values():
                path.mkdir()
            host = {"capture_identity": "linux-archive", "host": {"goos": "linux"},
                    "report": {"rows": [{"case": "filesystem/one", "result": "match"}]}}
            def replay(family, _):
                if family == "filesystem":
                    raise p.capture.StaleCapture("filesystem production input changed")
                return {"capture_identity": "config-archive", "rows": [{"case": "config/one", "result": "match"}]}
            with patch.object(p, "source_closure", return_value={"input": "a" * 64}), \
                 patch.object(p, "harness_check", return_value=self.health), \
                 patch.object(p, "replay_family", side_effect=replay), \
                 patch.object(p, "replay_host_capture", return_value=host), \
                 patch.object(p, "baseline_owners", return_value={"config-output": ("config", "config/one")}), \
                 patch.object(p.capture, "load_requests", return_value={"requests": [{"case": "config/one"}]}):
                result = p.produce("config", captures, directory / "reports", platform_captures=[directory / "linux"])
            self.assertEqual(result["tests"], {"config-output": "pass"})
            self.assertFalse(result["metrics"]["prepared"])
            detail = p.read(next((directory / "reports").glob("config-*.json")))
            self.assertEqual(detail["unavailable"], {"filesystem": "filesystem production input changed"})
            self.assertEqual(set(detail["reports"]), {"config"})
            self.assertEqual(detail["unattached_host_captures"], {"linux": host})

    def test_scoped_health_does_not_consume_integration_or_live_source_classification(self):
        import phase1_integration
        for producer in ("config", "syntax"):
            with self.subTest(producer=producer), \
                 patch.object(phase1_integration, "check", side_effect=AssertionError("unfingerprinted integration")), \
                 patch.object(p.scope, "build", side_effect=AssertionError("unrelated Rust source audit")):
                health = p.harness_check(producer)
                self.assertEqual(health["integration"], {"prepared": False, "complete": False, "problems": []})


@contextlib.contextmanager
def traced_reads():
    """Record every repository file opened for reading; refuse writes and children."""
    import builtins
    import io
    import pathlib
    root = p.ROOT.resolve()
    reads: set[str] = set()
    original_open, original_path_open = builtins.open, pathlib.Path.open

    def note(target):
        try:
            reads.add(str(Path(target).resolve().relative_to(root)))
        except (TypeError, ValueError, OSError):
            pass

    def opened(file, mode="r", *args, **kwargs):
        if isinstance(file, (str, os.PathLike)):
            if any(flag in mode for flag in "wax+"):
                raise AssertionError(f"harness check wrote {file}")
            note(file)
        return original_open(file, mode, *args, **kwargs)

    def path_opened(self, mode="r", *args, **kwargs):
        if any(flag in mode for flag in "wax+"):
            raise AssertionError(f"harness check wrote {self}")
        note(self)
        return original_path_open(self, mode, *args, **kwargs)

    def refused(*args, **kwargs):
        raise AssertionError("harness check started a child process")

    with patch.object(builtins, "open", opened), patch.object(io, "open", opened), \
         patch.object(pathlib.Path, "open", path_opened), \
         patch.object(subprocess, "run", refused), patch.object(subprocess, "Popen", refused), \
         patch.object(subprocess, "check_output", refused):
        yield reads


class ImportClosureTests(unittest.TestCase):
    def test_a_module_imported_through_an_inserted_sys_path_directory_is_followed(self):
        # s07_binder.syntax_schema inserts tools/s07/binder and imports
        # generate_syntax; the binder oracle validates graphs through it.
        module = "tools/s07/binder/generate_syntax.py"
        self.assertIn("from generate_syntax import category", (p.ROOT / "scripts/s07_binder.py").read_text())
        self.assertIn(module, p.python_import_closure(["scripts/s07_binder.py"]))
        self.assertIn(module, p.python_import_closure(p.MUTATION_COMMANDS))
        self.assertIn(module, p.mutation_inputs())
        self.assertIn(module, p.source_closure("foundations"))

    def test_inserted_directories_resolve_or_are_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "scripts").mkdir()
            (root / "tools/extra").mkdir(parents=True)
            (root / "scripts/entry.py").write_text(
                "import sys\nfrom pathlib import Path\nsys.path.insert(0, str(Path(__file__).resolve().parent))\n"
                "def later():\n    sys.path.insert(0, str(ROOT / 'tools/extra'))\n    import helper\n")
            (root / "tools/extra/helper.py").write_text("import shared\n")
            (root / "scripts/shared.py").write_text("import json\n")
            (root / "scripts/other.py").write_text("import late\n")
            (root / "tools/extra/late.py").write_text("")
            with patch.object(p, "ROOT", root):
                self.assertEqual(p.python_import_closure(["scripts/entry.py"]),
                                 {"scripts/entry.py", "tools/extra/helper.py", "scripts/shared.py"})
                # sys.path is process-wide: once any followed module inserts the
                # directory, an import in a module scanned earlier resolves
                # through it too.
                self.assertEqual(p.python_import_closure(["scripts/other.py"]), {"scripts/other.py"})
                self.assertEqual(p.python_import_closure(["scripts/other.py", "scripts/entry.py"]),
                                 {"scripts/other.py", "tools/extra/late.py", "scripts/entry.py",
                                  "tools/extra/helper.py", "scripts/shared.py"})
                (root / "scripts/opaque.py").write_text("import sys\nsys.path.insert(0, somewhere())\n")
                with self.assertRaisesRegex(ValueError, "cannot resolve the sys.path entry"):
                    p.python_import_closure(["scripts/opaque.py"])


class HarnessInputTests(unittest.TestCase):
    """What harness health reads must be what its producer binds (review item I13)."""

    def test_harness_reads_stay_inside_closure_and_ledger(self):
        from test_phase1_tracker import runs, selected
        ledger = runs()
        for producer in ("config", "syntax", "foundations"):
            with self.subTest(producer=producer):
                closure = p.source_closure(producer)
                with traced_reads() as reads:
                    health = p.harness_check(producer, current_classification=False)
                reads = {name for name in reads if not name.startswith(("upstream/", "target/"))}
                # The trace must see the coverage join and the mutation binding.
                self.assertLessEqual({"data/phase1/cases.json", "data/phase1/mutation/results.json.gz",
                                      "tools/phase1/mutation/go/patches.json", "scripts/s07_oracle/binder.go"}, reads)
                self.assertTrue(health["healthy"] or health["problems"])
                self.assertEqual(sorted(reads - closure.keys()), [], f"{producer} reads outside its closure")
                self.assertEqual(sorted(reads - selected(ROOT, ledger[producer])), [],
                                 f"{producer} reads outside its ledger globs")

    def test_the_harness_and_the_go_oracles_name_the_same_inventories(self):
        import phase1_mutation_go as go
        self.assertEqual({oracle: spec["inventory"] for oracle, spec in p.scope.MUTATION_ORACLES.items()},
                         {oracle: definition.inventory for oracle, definition in go.ORACLES.items()})

    def test_mutation_binding_changes_stale_every_harness_producer(self):
        # Each changes the config coverage report and the syntax preparation
        # (the review's in-memory probe); none may leave a closure unchanged.
        original = Path.read_bytes
        before = {producer: p.source_closure(producer) for producer in ("config", "syntax", "foundations")}
        for relative in ("tools/phase1/mutation/go/patches.json", "scripts/s07_oracle/binder.go",
                         "tools/phase1/syntax/program_probe_test.go", "data/s07/binder-requests.json",
                         "data/phase1/mutation/native-e1.json.gz"):
            target = ROOT / relative
            self.assertTrue(target.is_file(), relative)
            for producer, closure in before.items():
                with self.subTest(path=relative, producer=producer):
                    with patch.object(Path, "read_bytes",
                                      lambda path: original(path) + b"\n" if path == target else original(path)):
                        after = p.source_closure(producer)
                    self.assertIn(relative, closure)
                    self.assertNotEqual(closure[relative], after[relative])


class QualificationTests(unittest.TestCase):
    def test_approved_differences_keep_raw_result_and_reject_additional_drift(self):
        approved = p.qualifications()
        self.assertEqual(len(approved), 5)
        for identity, item in approved.items():
            row = {"case": identity, "result": "different", "native": item["native"], "rust": item["rust"]}
            with self.subTest(case=identity):
                self.assertTrue(p.accepted(row, approved))
                self.assertEqual(row["result"], "different")
                changed = copy.deepcopy(row)
                changed["rust"]["unapproved_extra_field"] = True
                self.assertFalse(p.accepted(changed, approved))
                self.assertFalse(p.accepted({**row, "result": "not_implemented"}, approved))
        package = next(k for k in approved if k.startswith("config/"))
        changed = copy.deepcopy(approved[package]["rust"])
        # The exact-pair policy cannot waive even a harmless-looking addition,
        # let alone losing the separately observed shared contents identity.
        changed["new"] = False
        self.assertFalse(p.accepted({"case": package, "result": "different",
                                    "native": approved[package]["native"], "rust": changed}, approved))

    def test_changed_approval_request_fails_closed(self):
        original = p.capture.load_requests
        def changed(spec):
            value = copy.deepcopy(original(spec))
            for row in value["requests"]:
                row["unreviewed_action"] = True
            return value
        with patch.object(p.capture, "load_requests", side_effect=changed), self.assertRaisesRegex(ValueError, "request changed"):
            p.qualifications()


if __name__ == '__main__':
    unittest.main()
