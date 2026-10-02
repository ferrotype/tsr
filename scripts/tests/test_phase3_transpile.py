"""Phase 3 C3: the transpile comparison (`scripts/phase3_transpile.py`): the
native capture's verification against tampering, the grading of every
baseline, the Rust run's binding, and the mutation check, run for real
against the crate and the harness."""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_transpile as transpile  # noqa: E402
from s08_oracle import canonical, digest  # noqa: E402


def recorded():
    return json.loads(transpile.RECORDED.read_bytes())


def write_document(directory, document):
    path = Path(directory) / "transpile-native.json"
    path.write_bytes(json.dumps(document).encode())
    return path


def rust_rows(native_rows):
    """The rows a harness that reproduces the pin would write."""
    return [{"id": row["id"], "state": "executed", "configured_name": row["configured_name"],
             "runs": [{"declaration": run["declaration"], "baseline": copy.deepcopy(run["baseline"]),
                       "units": copy.deepcopy(run["units"])} for run in row["runs"]]} for row in native_rows]


def write_rust(directory, native_rows, rows):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    raw = b"".join(canonical(row) + b"\n" for row in rows)
    (directory / "rows.ndjson").write_bytes(raw)
    report = {"version": 1, "native": {"rows_sha256": digest(canonical(native_rows))},
              "harness": {"binary_sha256": "0" * 64, "sources": {}}, "rows_sha256": digest(raw),
              "rows": len(rows)}
    (directory / "report.json").write_bytes(json.dumps(report).encode())
    return directory


class Native(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())

    def tearDown(self):
        shutil.rmtree(self.temp)

    def test_the_recorded_capture_verifies(self):
        provenance, rows = transpile.load_native(transpile.RECORDED)
        self.assertEqual((provenance["configurations"], provenance["baselines"]), (28, 41))
        self.assertEqual(len(transpile.requests(rows)), 28)
        request = transpile.requests(rows)[14]
        self.assertEqual(request["id"], "transpile/declarationSingleFileHasErrorsReported.ts#")
        self.assertTrue(request["report_diagnostics"])
        self.assertEqual(set(request), {"id", "file", "configuration_name", "units", "options",
                                        "report_diagnostics"})

    @unittest.skipUnless((transpile.DEFAULT_NATIVE / "transpile.json").is_file(), "no native capture directory")
    def test_the_capture_directory_verifies_against_the_record(self):
        provenance, _ = transpile.load_native(transpile.DEFAULT_NATIVE)
        self.assertEqual(provenance["rows_sha256"], transpile.load_native(transpile.RECORDED)[0]["rows_sha256"])

    def test_a_tampered_baseline_is_rejected(self):
        document = recorded()
        baseline = document["rows"][3]["runs"][0]["baseline"]
        text = bytearray(bytes.fromhex(baseline["text_hex"]))
        text[-2] ^= 1
        baseline["text_hex"] = bytes(text).hex()
        with self.assertRaisesRegex(ValueError, "differs from the committed reference"):
            transpile.load_native(write_document(self.temp, document))

    def test_a_tampered_source_digest_is_rejected(self):
        document = recorded()
        document["rows"][0]["source_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "source digest"):
            transpile.load_native(write_document(self.temp, document))

    def test_a_partial_or_duplicated_capture_is_rejected(self):
        document = recorded()
        dropped = document["rows"].pop(5)
        document["configurations"] -= 1
        document["baselines"] -= len(dropped["runs"])
        with self.assertRaisesRegex(ValueError, "not the committed transpile references"):
            transpile.load_native(write_document(self.temp, document))
        document = recorded()
        document["rows"].append(document["rows"][0])
        with self.assertRaisesRegex(ValueError, "missing or duplicated"):
            transpile.load_native(write_document(self.temp, document))

    def test_a_failed_or_reshaped_native_row_is_rejected(self):
        document = recorded()
        document["rows"][2]["state"] = "upstream_failed"
        with self.assertRaisesRegex(ValueError, "did not execute"):
            transpile.load_native(write_document(self.temp, document))
        document = recorded()
        document["rows"][0]["runs"].reverse()
        with self.assertRaisesRegex(ValueError, "not the runner's"):
            transpile.load_native(write_document(self.temp, document))

    def test_a_capture_directory_must_agree_with_the_record(self):
        document = recorded()
        unit = document["rows"][0]["runs"][0]["units"][0]
        unit["output_hex"] = unit["output_hex"][:-2]
        (self.temp / "capture").mkdir()
        (self.temp / "capture" / "transpile.json").write_bytes(canonical(document))
        with self.assertRaisesRegex(ValueError, "differ from the recorded document"):
            transpile.load_native(self.temp / "capture")


class Comparison(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.provenance, cls.native = transpile.load_native(transpile.RECORDED)

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())

    def tearDown(self):
        shutil.rmtree(self.temp)

    def compare(self, rows):
        directory = write_rust(self.temp / "rust", self.native, rows)
        return transpile.compare(transpile.RECORDED, directory, self.temp / "comparison.json")

    def test_the_pins_rows_match_every_baseline(self):
        document = self.compare(rust_rows(self.native))
        self.assertEqual(document["outcomes"], {"match": 41, "different": 0, "failed": 0})
        self.assertEqual((document["transpile_parity"], document["all_match"]), (1.0, True))
        self.assertEqual(len(document["results"]), 41)
        self.assertTrue(all(item["units"]["fields"]["output_hex"] == item["units"]["of"]
                            for item in document["results"]))
        self.assertEqual(json.loads((self.temp / "comparison.json").read_bytes())["matched"], 41)

    def test_a_changed_baseline_is_different_with_both_texts(self):
        rows = rust_rows(self.native)
        run = rows[14]["runs"][1]
        text = bytes.fromhex(run["baseline"]["text_hex"]).replace(b"TS9010", b"TS9011")
        run["baseline"]["text_hex"] = text.hex()
        run["units"][0]["diagnostics"][0]["code"] = 9011
        document = self.compare(rows)
        self.assertEqual(document["outcomes"], {"match": 40, "different": 1, "failed": 0})
        self.assertEqual(document["transpile_parity"], 40 / 41)
        self.assertFalse(document["all_match"])
        item = next(item for item in document["results"] if item["outcome"] == "different")
        self.assertEqual(item["name"], "transpile/declarationSingleFileHasErrorsReported.d.ts")
        self.assertIn("TS9010", item["native_text"])
        self.assertIn("TS9011", item["rust_text"])
        self.assertGreater(item["first_difference"]["line"], 1)
        self.assertEqual(item["differing_units"], [{"unit": "declarationSingleFileHasErrorsReported.ts",
                                                    "fields": ["diagnostics"]}])

    def test_a_failed_configuration_fails_each_of_its_baselines(self):
        rows = rust_rows(self.native)
        rows[1] = {"id": rows[1]["id"], "state": "failed", "class": "panic", "reason": "boom", "location": "x.rs:1"}
        document = self.compare(rows)
        self.assertEqual(document["outcomes"], {"match": 39, "different": 0, "failed": 2})
        failed = [item for item in document["results"] if item["outcome"] == "failed"]
        self.assertEqual({item["reason"] for item in failed}, {"boom"})

    def test_missing_renamed_and_extra_runs_do_not_match(self):
        rows = rust_rows(self.native)
        rows[0]["runs"].pop()
        rows[1]["runs"][0]["baseline"]["name"] = "transpile/other.js"
        extra = copy.deepcopy(rows[4]["runs"][0])
        extra["declaration"] = True
        rows[22]["runs"].append(extra)
        document = self.compare(rows)
        self.assertEqual(document["outcomes"], {"match": 39, "different": 2, "failed": 0})
        self.assertEqual(document["extra_runs"], [{"id": rows[22]["id"], "declaration": True}])
        self.assertFalse(document["all_match"])
        missing = next(item for item in document["results"] if item.get("reason"))
        self.assertEqual(missing["reason"], "the harness made no such run")

    def test_the_rust_run_is_bound_to_its_rows_and_to_the_native_rows(self):
        directory = write_rust(self.temp / "rust", self.native, rust_rows(self.native))
        raw = (directory / "rows.ndjson").read_bytes()
        (directory / "rows.ndjson").write_bytes(raw.replace(b'"executed"', b'"executed" ', 1))
        with self.assertRaisesRegex(ValueError, "differ from their run report"):
            transpile.compare(transpile.RECORDED, directory, self.temp / "c.json")
        rows = rust_rows(self.native)
        rows[0], rows[1] = rows[1], rows[0]
        directory = write_rust(self.temp / "reordered", self.native, rows)
        with self.assertRaisesRegex(ValueError, "missing, extra or reordered"):
            transpile.compare(transpile.RECORDED, directory, self.temp / "c.json")
        directory = write_rust(self.temp / "other", self.native[1:], rust_rows(self.native))
        with self.assertRaisesRegex(ValueError, "other native rows"):
            transpile.compare(transpile.RECORDED, directory, self.temp / "c.json")

    def test_the_row_contract(self):
        row = rust_rows(self.native)[0]
        transpile.validate_row(row)
        transpile.validate_row({"id": "x", "state": "failed", "class": "harness", "reason": "r"})
        with self.assertRaisesRegex(ValueError, "class and reason"):
            transpile.validate_row({"id": "x", "state": "failed", "reason": "r"})
        broken = copy.deepcopy(row)
        broken["runs"][0]["baseline"]["state"] = "unsupported"
        with self.assertRaisesRegex(ValueError, "malformed harness baseline"):
            transpile.validate_row(broken)
        broken = copy.deepcopy(row)
        del broken["runs"][0]["units"][0]["source_map_hex"]
        with self.assertRaisesRegex(ValueError, "malformed harness unit"):
            transpile.validate_row(broken)
        with self.assertRaisesRegex(ValueError, "malformed harness row"):
            transpile.validate_row({"id": "x", "state": "unexecuted"})


@unittest.skipUnless(shutil.which("cargo"), "cargo is not available")
class ForReal(unittest.TestCase):
    """The harness built and run, unmutated and with every mutant."""

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp())

    def tearDown(self):
        shutil.rmtree(self.temp)

    def test_each_mutant_anchor_occurs_once(self):
        self.assertGreaterEqual(len(transpile.MUTANTS), 5)
        self.assertEqual({path for _, path, _, _ in transpile.MUTANTS},
                         {"crates/tsr_transpile/src/lib.rs", "tools/phase3/harness/transpile.rs"})
        for name, path, before, after in transpile.MUTANTS:
            self.assertEqual((ROOT / path).read_text().count(before), 1, name)
            self.assertNotEqual(before, after, name)

    def test_the_harness_matches_every_baseline(self):
        transpile.run(transpile.RECORDED, self.temp / "rust")
        document = transpile.compare(transpile.RECORDED, self.temp / "rust")
        self.assertEqual(document["outcomes"], {"match": 41, "different": 0, "failed": 0})
        self.assertTrue(document["all_match"])

    def test_every_mutant_is_caught(self):
        sources = {path: (ROOT / path).read_bytes() for _, path, _, _ in transpile.MUTANTS}
        summary = transpile.mutate(transpile.RECORDED, self.temp / "mutation")
        self.assertEqual([item["mutant"] for item in summary["mutants"]], [m[0] for m in transpile.MUTANTS])
        self.assertTrue(summary["all_killed"])
        for item in summary["mutants"]:
            self.assertLess(item["outcomes"]["match"], 41, item["mutant"])
        for path, original in sources.items():
            self.assertEqual((ROOT / path).read_bytes(), original, path)


if __name__ == "__main__":
    unittest.main()
