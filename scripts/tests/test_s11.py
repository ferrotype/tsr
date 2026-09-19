"""Adversarial checks on the S11 evidence denominator, not mirrored Rust tests."""
import copy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s11
from s04 import same_json_value
from s04_common import strict_json_loads


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.cases = ["fs/one", "control"]
        self.rows = [{"id":"fs/one", "category":"go-filesystem", "pass":True},
                     {"id":"control", "category":"controls", "pass":True}]

    def test_failure_is_measured_without_disappearing_from_denominator(self):
        rows = copy.deepcopy(self.rows)
        rows[0]["pass"] = False
        report = s11.summarize(rows, self.cases)
        self.assertEqual(report["tests"], {"fs/one":"fail", "control":"pass"})
        self.assertNotIn("parity", report["metrics"])

    def test_control_failure_cannot_inherit_success(self):
        self.rows[1]["pass"] = False
        self.assertIs(s11.summarize(self.rows, self.cases)["metrics"]["controls"], False)

    def test_incomplete_duplicate_extra_reordered_and_empty_inventory_fail(self):
        for rows in (self.rows[:1], self.rows + self.rows[:1], self.rows[::-1], [],
                     self.rows + [{"id":"extra", "category":"controls", "pass":True}]):
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                s11.summarize(rows, self.cases)

    def test_numeric_truth_cannot_pass(self):
        self.rows[0]["pass"] = 1
        with self.assertRaises(ValueError):
            s11.summarize(self.rows, self.cases)
        self.assertFalse(same_json_value(True, 1))
        self.assertFalse(same_json_value({"content":None}, {"content":""}))

    def test_no_controls_cannot_pass(self):
        with self.assertRaises(ValueError):
            s11.summarize(self.rows[:1], self.cases[:1])

    def test_oracle_reordering_missing_extra_or_duplicate_is_rejected(self):
        fixtures = [{"id":"one"}, {"id":"two"}]
        for rows in (fixtures[::-1], fixtures[:1], fixtures + fixtures[:1], []):
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                s11.validate_observations(fixtures, rows, "test")

    def test_changed_observation_is_not_a_valid_go_comparison(self):
        original = {"id":"read", "result":{"content":None}, "callbacks":[]}
        changed = copy.deepcopy(original)
        changed["result"]["content"] = ""
        self.assertFalse(same_json_value(original, changed))
        changed = copy.deepcopy(original)
        changed["callbacks"].append({"method":"readFile", "params":"/file"})
        self.assertFalse(same_json_value(original, changed))

    def test_null_result_requires_its_exact_response_envelope(self):
        s11.check_response({"jsonrpc":"2.0", "id":1, "result":None}, 1, {"result":None})
        for actual in ({"jsonrpc":"2.0", "id":1},
                       {"jsonrpc":"2.0", "id":1, "result":None, "extra":True},
                       {"jsonrpc":"2.0", "id":1, "result":None, "error":{}},
                       {"jsonrpc":"2.0", "id":True, "result":None}):
            with self.subTest(actual=actual), self.assertRaises(ValueError):
                s11.check_response(actual, 1, {"result":None})

    def test_duplicate_json_key_rejected_before_projection(self):
        with self.assertRaises(ValueError):
            strict_json_loads('{"result":{"content":null,"content":""}}')

    def test_internal_hook_evidence_requires_each_named_test_to_execute(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data/s11").mkdir(parents=True)
            (root / "data/s11/hook-cases.json").write_text(json.dumps([{"id":"internal/one","test":"session::tests::one"}]))
            for output in (b"running 0 tests\n", b"test session::tests::other ... ok\n",
                           b"test session::tests::one ... ok\ntest session::tests::one ... ok\n"):
                with patch.object(s11, "ROOT", root), patch.object(s11, "REPORTS", root), patch.object(s11, "command", return_value=output), self.assertRaises(ValueError):
                    s11.hook_rows()
            with patch.object(s11, "ROOT", root), patch.object(s11, "REPORTS", root), patch.object(s11, "command", return_value=b"test session::tests::one ... ignored\n"):
                self.assertFalse(s11.hook_rows()[0]["pass"])

    def test_mapper_byte_capture_rejects_dropped_changed_and_truncated_streams(self):
        import base64
        from s11_contracts import encode
        fixture = {"id":"fixture", "requests":[{"method":"initialize","params":{}}]}
        request = {"jsonrpc":"2.0","id":"fixture:0","method":"initialize","params":{}}
        response = {"jsonrpc":"2.0","id":"fixture:0","result":{"ok":True}}
        observation = {"responses":[{"result":{"ok":True}}],
                       "server_bytes":base64.b64encode(encode(request)).decode(),
                       "plugin_bytes":base64.b64encode(encode(response)).decode()}
        s11.validate_mapper_bytes(fixture, observation)
        for key in ("server_bytes", "plugin_bytes"):
            for data in (b"", b"Content-Length: 99\r\n\r\n{}", encode({**response,"id":"wrong"})):
                altered = {**observation,key:base64.b64encode(data).decode()}
                with self.subTest(key=key,data=data), self.assertRaises(ValueError):
                    s11.validate_mapper_bytes(fixture, altered)

    def test_frozen_full_inventory_is_composition_of_all_four_groups(self):
        root = s11.ROOT / "data/s11"
        fs = json.loads((root / "fs-fixtures.json").read_text())
        mapper = json.loads((root / "mapper-fixtures.json").read_text())
        transport = json.loads((root / "transport-cases.json").read_text())
        hooks = json.loads((root / "hook-cases.json").read_text())
        combined = ["fs/" + f["id"] for f in fs] + ["mapper/" + f["id"] for f in mapper] + [f["id"] for f in transport] + [f["id"] for f in hooks]
        self.assertEqual(json.loads((root / "cases.json").read_text()), combined)
        self.assertEqual(len(set(combined)), len(combined))


if __name__ == "__main__":
    unittest.main()
