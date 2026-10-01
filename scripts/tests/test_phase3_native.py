"""Phase 3 T0: the native emit capture's validation, against four real native rows."""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_inventory as inventory  # noqa: E402
import phase3_native as native  # noqa: E402
from s08_oracle import canonical, digest  # noqa: E402

FIXTURE = ROOT / "scripts/tests/fixtures/phase3/native-rows.json"
CONTENT, NO_CONTENT, DISABLED, SOURCEMAP = range(4)


class NativeRows(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.observed = json.loads(FIXTURE.read_bytes())
        document = inventory.read()
        cls.owed = {row["id"]: row for row in document["rows"]}
        requests = {row["id"]: row for row in inventory.requests(document)}
        cls.requests = [requests[row["id"]] for row in cls.observed]

    def rows(self):
        return copy.deepcopy(self.observed)

    def test_real_rows_validate_and_agree_with_the_inventory(self):
        native.validate(self.requests, self.observed)
        for row in self.observed:
            self.assertEqual(native.against_inventory(self.owed[row["id"]], row), [], row["id"])
        self.assertEqual([row["output"]["state"] for row in self.observed],
                         ["content", "no_content", "disabled", "content"])
        self.assertEqual(self.observed[SOURCEMAP]["sourcemap"]["state"], "content")

    def test_harness_failures_are_rejected_and_native_failures_need_a_stage(self):
        rows = self.rows()
        rows[CONTENT]["state"] = "harness_failed"
        with self.assertRaisesRegex(ValueError, "is harness_failed"):
            native.validate(self.requests, rows)
        rows = self.rows()
        rows[CONTENT] = {"id": rows[CONTENT]["id"], "state": "upstream_failed",
                         "failure": {"stage": "harness_input", "message": "x"}}
        with self.assertRaisesRegex(ValueError, "explicit stage"):
            native.validate(self.requests, rows)
        rows[CONTENT]["failure"]["stage"] = "native_output"
        native.validate(self.requests, rows)

    def test_a_row_must_have_read_the_requested_input(self):
        rows = self.rows()
        rows[CONTENT]["loaded_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "different input"):
            native.validate(self.requests, rows)

    def test_domain_outcomes_are_a_closed_vocabulary(self):
        rows = self.rows()
        rows[CONTENT]["output"]["state"] = "skipped"
        with self.assertRaisesRegex(ValueError, "unknown output outcome"):
            native.validate(self.requests, rows)
        rows = self.rows()
        rows[NO_CONTENT]["output"]["text_hex"] = ""
        with self.assertRaisesRegex(ValueError, "carries text"):
            native.validate(self.requests, rows)
        rows = self.rows()
        del rows[DISABLED]["output"]["reason"]
        with self.assertRaisesRegex(ValueError, "without a reason"):
            native.validate(self.requests, rows)
        rows = self.rows()
        rows[DISABLED]["output"] = {"state": "no_content", "name": "x"}
        with self.assertRaisesRegex(ValueError, "without a non-declaration input"):
            native.validate(self.requests, rows)
        rows = self.rows()
        rows[CONTENT]["sourcemap"] = {"state": "disabled", "reason": "x"}
        with self.assertRaisesRegex(ValueError, "never disables"):
            native.validate(self.requests, rows)

    def test_inventory_disagreements_name_the_field(self):
        def fields(index, change):
            row = copy.deepcopy(self.observed[index])
            change(row)
            return [entry["field"] for entry in native.against_inventory(self.owed[row["id"]], row)]

        self.assertEqual(fields(CONTENT, lambda row: row["output"].update(text_hex="00")), ["output_reference"])
        self.assertEqual(fields(CONTENT, lambda row: row.update(output={"state": "no_content", "name": "x"})), ["output"])
        self.assertEqual(fields(NO_CONTENT, lambda row: row.update(has_non_dts_files=False)), ["has_non_dts_files"])
        self.assertEqual(fields(DISABLED, lambda row: row["output"].update(reason="another")), ["output"])
        self.assertEqual(fields(SOURCEMAP, lambda row: row.update(sourcemap={"state": "not_baselined"})), ["sourcemap"])
        self.assertEqual(fields(CONTENT, lambda row: row.update(
            sourcemap_record=dict(self.observed[SOURCEMAP]["sourcemap_record"]))),
            ["sourcemap_record", "sourcemap_record_reference"])

    def test_row_digest_covers_every_observed_field(self):
        row = self.rows()[CONTENT]
        before = native.contract_digest(row)
        row["reprint"][0]["no_comments"]["sha256"] = "0" * 64
        self.assertNotEqual(before, native.contract_digest(row))


class Selection(unittest.TestCase):
    ROWS = [{"id": str(index)} for index in range(10)]

    def test_shards_partition_the_rows(self):
        for scheme in ("contiguous", "interleaved"):
            groups = native.shard_indices(10, 3, scheme)
            self.assertEqual(sorted(index for group in groups for index in group), list(range(10)))
        self.assertEqual(native.shard_indices(10, 3, "interleaved")[0], [0, 3, 6, 9])
        with self.assertRaisesRegex(ValueError, "at least one shard"):
            native.shard_indices(10, 0, "contiguous")

    def test_development_subsets(self):
        self.assertEqual(native.select(self.ROWS, None, 1, ()), self.ROWS)
        self.assertEqual([row["id"] for row in native.select(self.ROWS, 2, 4, ())], ["0", "4"])
        self.assertEqual([row["id"] for row in native.select(self.ROWS, None, 1, ["7", "2"])], ["2", "7"])
        with self.assertRaisesRegex(ValueError, "unknown --case"):
            native.select(self.ROWS, None, 1, ["11"])


class Overlay(unittest.TestCase):
    def test_overlay_adds_hooks_at_exact_anchors_and_replaces_no_test(self):
        upstream = native.pinned_upstream()
        sources = native.overlay_sources(upstream)
        self.assertEqual(set(sources), {"testutil/harnessutil/harnessutil.go",
                                        "testutil/harnessutil/s08_diagnostics_observer.go",
                                        "testutil/baseline/baseline.go", native.OBSERVER, native.DRIVER})
        pinned = (upstream / "tsc/internal/testutil/baseline/baseline.go").read_text()
        hook = "\tif Phase3Observe != nil && Phase3Observe(fileName, actual, opts) {\n\t\treturn\n\t}\n"
        self.assertEqual(sources["testutil/baseline/baseline.go"].replace(hook, ""), pinned)
        for name in (native.OBSERVER, native.DRIVER):
            self.assertFalse((upstream / "tsc/internal" / name).exists())
        with self.assertRaisesRegex(ValueError, "occurs 0 times"):
            native.replace_exact(pinned, "func Run(t *testing.T, another string) {\n", "")


class CaptureDirectory(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="phase3-native-"))
        self.addCleanup(shutil.rmtree, self.directory)
        observed = json.loads(FIXTURE.read_bytes())
        raw = b"".join(canonical(row) + b"\n" for row in observed)
        (self.directory / "observations.ndjson").write_bytes(raw)
        self.report = {"partial": True, "observation_sha256": digest(raw),
                       "row_sha256": [native.contract_digest(row) for row in observed]}
        self.write()

    def write(self):
        (self.directory / "report.json").write_bytes(canonical(self.report) + b"\n")

    def test_a_partial_capture_never_stands_for_the_denominator(self):
        with self.assertRaisesRegex(ValueError, "partial capture"):
            native.load_capture(self.directory)
        self.assertEqual(len(native.load_capture(self.directory, partial=True)[2]), 4)
        with self.assertRaisesRegex(ValueError, "never recorded"):
            native.review(self.directory, True, partial=True)

    def test_tampered_observations_are_rejected(self):
        path = self.directory / "observations.ndjson"
        path.write_bytes(path.read_bytes().replace(b'"emit_skipped":false', b'"emit_skipped":true', 1))
        with self.assertRaisesRegex(ValueError, "changed since its report"):
            native.load_capture(self.directory, partial=True)
        self.report["observation_sha256"] = digest(path.read_bytes())
        self.write()
        with self.assertRaisesRegex(ValueError, "differ from their recorded digests"):
            native.load_capture(self.directory, partial=True)


if __name__ == "__main__":
    unittest.main()
