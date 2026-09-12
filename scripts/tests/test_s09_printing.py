"""Offline counterexamples for the native printing observation contract."""

import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s09_printing as printing


class PrintingProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for relative in (
            "data/upstream.json",
            "data/s04/toolchains.toml",
            "data/s09/printing-cases.json",
            "data/s09/printing-observations.json",
            "tools/s09/printing_test.go",
            "scripts/s09_printing.py",
            "scripts/s08_oracle.py",
            "scripts/s04.py",
            "scripts/s04_common.py",
        ):
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(printing.ROOT / relative, destination)

    def change_json(self, relative, change):
        path = self.root / relative
        value = json.loads(path.read_text())
        change(value)
        path.write_text(json.dumps(value))

    def mutate_observation(self, change):
        self.change_json("data/s09/printing-observations.json", change)

    def test_frozen_native_observation_verifies_without_go(self):
        observation = printing.verify_frozen(self.root)
        self.assertEqual(len(observation["rows"]), 12)

    def test_requested_text_change_requires_a_new_native_capture(self):
        self.change_json("data/s09/printing-cases.json",
                         lambda value: value["cases"][0].update(text="different"))
        with self.assertRaisesRegex(ValueError, "cases_sha256"):
            printing.verify_frozen(self.root)

    def test_oracle_and_capture_dependencies_cannot_change_silently(self):
        for relative, field in (
            ("tools/s09/printing_test.go", "source_sha256"),
            ("scripts/s09_printing.py", "capture_script_sha256"),
            ("scripts/s08_oracle.py", "overlay_helper_sha256"),
            ("scripts/s04.py", "environment_helper_sha256"),
            ("scripts/s04_common.py", "protocol_helper_sha256"),
        ):
            with self.subTest(relative=relative):
                path = self.root / relative
                original = path.read_bytes()
                path.write_bytes(original + b"\n")
                with self.assertRaisesRegex(ValueError, field):
                    printing.verify_frozen(self.root)
                path.write_bytes(original)

    def test_duplicate_row_is_not_a_complete_inventory(self):
        self.mutate_observation(lambda value: value["rows"].append(value["rows"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate or reordered"):
            printing.verify_frozen(self.root)

    def test_changed_outcome_does_not_pass_as_native_text(self):
        def change(value):
            row = value["rows"][0]
            del row["text_hex"]
            row["decode_error"] = "unexpected decoder failure"
        self.mutate_observation(change)
        with self.assertRaisesRegex(ValueError, "did not produce Go text"):
            printing.verify_frozen(self.root)

    def test_printer_options_must_match_the_request(self):
        self.mutate_observation(lambda value: value["rows"][0]["options"].update(never_ascii_escape=True))
        with self.assertRaisesRegex(ValueError, "another printer option"):
            printing.verify_frozen(self.root)

    def test_panic_classes_cannot_be_interchanged(self):
        def change(value):
            row = next(row for row in value["rows"] if row["name"] == "nil-root")
            row["panic_class"] = "synthetic-expression"
        self.mutate_observation(change)
        with self.assertRaisesRegex(ValueError, "wrong native panic class"):
            printing.verify_frozen(self.root)

    def test_unrelated_panic_does_not_match_the_contract(self):
        def change(value):
            row = next(row for row in value["rows"] if row["name"] == "synthetic-expression-panic")
            row["panic"] = "assertion failed in another decoder path"
        self.mutate_observation(change)
        with self.assertRaisesRegex(ValueError, "wrong native panic"):
            printing.verify_frozen(self.root)

    def test_rust_boundary_reason_must_remain_explicit(self):
        def change(value):
            row = next(row for row in value["rows"] if row["name"] == "statement-boundary")
            row["rust_unsupported"] = "another boundary"
        self.mutate_observation(change)
        with self.assertRaisesRegex(ValueError, "another Rust boundary"):
            printing.verify_frozen(self.root)

    def test_supplied_malformed_wire_is_the_wire_observed(self):
        def change(value):
            row = next(row for row in value["rows"] if row["name"] == "malformed-short")
            row["encoded_hex"] = "aabbcc"
        self.mutate_observation(change)
        with self.assertRaisesRegex(ValueError, "another wire payload"):
            printing.verify_frozen(self.root)

    def test_missing_envelope_fields_fail_with_a_protocol_error(self):
        self.mutate_observation(lambda value: value.pop("source_sha256"))
        with self.assertRaisesRegex(ValueError, "malformed printing fixture"):
            printing.verify_frozen(self.root)


if __name__ == "__main__":
    unittest.main()
