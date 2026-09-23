"""Recorded coverage belongs to the request and claims actually observed."""
import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1
import phase1_capture as capture
import phase1_scope as scope


class RequestBindingTests(unittest.TestCase):
    def setUp(self):
        self.request = {"case": "sample", "operation": "resolve", "options": {
            "paths": {"z*": ["first/*"], "a*": ["second/*"]}}}
        self.case = {"id": "sample", "family": "config", "operations": ["resolve"], "last_result": "match"}
        self.report = {"family": "config", "capture_sha256": "a" * 64, "rows": [{
            "case": "sample", "result": "match", "request_sha256": capture.digest(capture.request_bytes(self.request)),
            "claims_sha256": scope.case_claims_digest(self.case)}]}
        self.loader = patch.object(capture, "load_requests", side_effect=lambda _: {"requests": [self.request]})
        self.loader.start()
        self.addCleanup(self.loader.stop)

    def record(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "cases.json"
            path.write_text(json.dumps({"cases": [self.case]}))
            with patch.object(phase1, "CASES", path), patch.object(capture, "compare", return_value=self.report):
                phase1.record_results(Path(temporary), True)
            return json.loads(path.read_text())

    def test_exact_record_is_used_but_changed_request_is_not(self):
        document = self.record()
        self.assertEqual(scope.recorded_results(document), {"sample": "match"})
        self.request["options"]["paths"] = dict(reversed(list(self.request["options"]["paths"].items())))
        self.assertEqual(scope.recorded_results(document), {"sample": "not_run"})

    def test_claim_or_result_changes_cannot_inherit_a_match(self):
        document = self.record()
        for key, value in (("operations", ["other"]), ("coverage_operations", ["other"]), ("last_result", "different")):
            changed = copy.deepcopy(document)
            changed["cases"][0][key] = value
            self.assertEqual(scope.recorded_results(changed), {"sample": "not_run"})

    def test_unbound_records_do_not_count(self):
        self.assertEqual(scope.recorded_results({"cases": [self.case]}), {"sample": "not_run"})

    def test_record_refuses_previous_capture_after_declaration_edit(self):
        self.case = self.record()["cases"][0]
        self.case["operations"].append("unrelated")
        self.assertEqual(scope.recorded_results({"cases": [self.case]}), {"sample": "not_run"})
        with self.assertRaisesRegex(ValueError, "not bound to the current case declaration"):
            self.record()

    def test_action_reassignment_cannot_inherit_a_match_with_same_operation_union(self):
        self.case["operations"] = ["resolve", "read"]
        self.case["operation_actions"] = {"first": ["resolve"], "second": ["read"]}
        self.report["rows"][0]["claims_sha256"] = scope.case_claims_digest(self.case)
        document = self.record()
        self.assertEqual(scope.recorded_results(document), {"sample": "match"})
        document["cases"][0]["operation_actions"] = {"first": ["read"], "second": ["resolve"]}
        self.assertEqual(scope.recorded_results(document), {"sample": "not_run"})

    def test_changed_missing_operation_cannot_inherit_a_recorded_gap(self):
        self.case["operations"] = ["resolve", "read"]
        self.report["rows"][0].update(result="not_implemented", missing_operation={"operation": "resolve"},
                                      claims_sha256=scope.case_claims_digest(self.case))
        document = self.record()
        self.assertEqual(scope.recorded_results(document), {"sample": "not_implemented"})
        self.assertEqual(document["cases"][0]["result_evidence"]["missing_operations"], ["resolve"])
        document["cases"][0]["missing_operations"] = ["read"]
        self.assertEqual(scope.recorded_results(document), {"sample": "not_run"})

    def test_record_refuses_old_request_even_if_case_identity_matches(self):
        self.request["options"]["paths"]["new/*"] = ["different/*"]
        with self.assertRaisesRegex(ValueError, "not bound to the current request"):
            self.record()

    def test_serialization_preserves_paths_order_at_every_depth(self):
        encoded = capture.request_bytes({"requests": [self.request]})
        decoded = json.loads(encoded)
        self.assertEqual(list(decoded["requests"][0]["options"]["paths"]), ["z*", "a*"])
        sorted_request = json.loads(capture.canonical(self.request))
        self.assertNotEqual(capture.request_bytes(self.request), capture.request_bytes(sorted_request))


class PackageInputPolicyTests(unittest.TestCase):
    def test_whitespace_before_include_macro_bang_keeps_embedded_asset(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "src").mkdir()
            asset = root / "table.md"
            asset.write_text("runtime data")
            for macro in ("include_str", "include_bytes"):
                with self.subTest(macro=macro):
                    (root / "src/lib.rs").write_text(f'{macro} \n ! ("../table.md")')
                    self.assertIn(asset, capture.package_input_files(root))

    def test_prose_does_not_stale_capture_but_embedded_markdown_does(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            package = root / "crates/leaf"
            (package / "src").mkdir(parents=True)
            (package / "README.md").write_text("prose")
            (package / "src/lib.rs").write_text('pub const ASSET: &str = include_str!("../table.md");')
            (package / "table.md").write_text("runtime data")
            with patch.object(capture, "ROOT", root):
                before = capture.source_closure("leaves", ["crates/leaf"])
                self.assertNotIn("crates/leaf/README.md", before)
                self.assertIn("crates/leaf/table.md", before)
                (package / "README.md").write_text("changed prose")
                self.assertEqual(before, capture.source_closure("leaves", ["crates/leaf"]))
                (package / "table.md").write_text("changed runtime data")
                self.assertNotEqual(before, capture.source_closure("leaves", ["crates/leaf"]))

    def test_cross_package_embedded_markdown_remains_an_input(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            a, b = root / "crates/a", root / "crates/b"
            (a / "src").mkdir(parents=True)
            b.mkdir(parents=True)
            (a / "src/lib.rs").write_text('include_str!("../../b/table.md")')
            asset = b / "table.md"
            asset.write_text("observed")
            with patch.object(capture, "ROOT", root):
                before = capture.source_closure("leaves", ["crates/a", "crates/b"])
                self.assertIn("crates/b/table.md", before)
                asset.write_text("changed")
                self.assertNotEqual(before, capture.source_closure("leaves", ["crates/a", "crates/b"]))

    def test_computed_include_and_build_scripts_keep_arbitrary_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "README.md").write_text("asset")
            (root / "lib.rs").write_text('include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))')
            self.assertIn(root / "README.md", capture.package_input_files(root))
            (root / "lib.rs").write_text("")
            (root / "build.rs").write_text("fn main() {}")
            self.assertIn(root / "README.md", capture.package_input_files(root))
