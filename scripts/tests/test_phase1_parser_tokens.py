"""The keyword/punctuation Rust table is the pinned Go answer, byte for byte.

witness/parser-keyword-or-punctuation is rust_gated: `cargo test -p tsr_parser`
checks tokens.rs against crates/tsr_parser/src/testdata/keyword_or_punctuation.rs.
This test ties that table to the frozen pinned-Go observations and their
provenance, so neither can be edited (or regenerated from a Rust answer)
without a red test.
"""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_capture as capture
import phase1_integration as integration
from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tools/phase1/parser-tokens"
WITNESS = "witness/parser-keyword-or-punctuation"


def load_observe():
    spec = importlib.util.spec_from_file_location("phase1_parser_tokens_observe", FIXTURE / "observe.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ParserTokensFixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.observe = load_observe()
        cls.requests = strict_json_loads((FIXTURE / "requests.json").read_bytes())
        cls.observed = strict_json_loads((FIXTURE / "native-observations.json").read_bytes())
        cls.provenance = strict_json_loads((FIXTURE / "native-provenance.json").read_bytes())

    def test_provenance_authenticates_the_frozen_go_observations(self):
        provenance = self.provenance
        self.assertEqual(provenance["pin"], json.loads((ROOT / "data/upstream.json").read_bytes())["pin"])
        self.assertEqual((provenance["package"], provenance["test"]), ("parser", self.observe.TEST))
        self.assertEqual(provenance["source_sha256"],
                         hashlib.sha256((FIXTURE / "export_test.go").read_bytes()).hexdigest())
        self.assertIsNone(provenance["helper_sha256"])
        self.assertEqual(provenance["extra_sources_sha256"], {})
        self.assertEqual(provenance["request_sha256"], capture.digest(capture.request_bytes(self.requests)))
        self.assertEqual(provenance["output_sha256"],
                         hashlib.sha256((FIXTURE / "native-observations.json").read_bytes()).hexdigest())
        # The Go overlay hashes the request file it read.
        self.assertEqual(self.observed["request_sha256"], provenance["request_sha256"])
        self.assertTrue(provenance["toolchain_local"])
        self.assertEqual((self.observed["go"], self.observed["goos"], self.observed["goarch"]),
                         (provenance["go"], provenance["goos"], provenance["goarch"]))

    def test_rows_cover_every_kind_and_both_answers(self):
        count = self.observed["kind_count"]
        kinds = [row["kind"] for row in self.observed["observations"]]
        self.assertEqual(self.requests, {"first": -1, "beyond_count": 1})
        self.assertEqual(kinds, list(range(-1, count + 2)))
        self.assertEqual({row["keyword_or_punctuation"] for row in self.observed["observations"]}, {False, True})

    def test_rust_table_is_generated_from_the_frozen_rows_byte_for_byte(self):
        self.assertEqual(self.observe.FIXTURE, FIXTURE)
        table = ROOT / "crates/tsr_parser/src/testdata/keyword_or_punctuation.rs"
        self.assertEqual(self.observe.TABLE, table)
        self.assertEqual(table.read_text(), self.observe.table(self.observed))

    def test_the_rust_gate_reads_the_table_and_its_receipt_binds_the_fixture(self):
        tests = (ROOT / "crates/tsr_parser/src/tokens_tests.rs").read_text()
        self.assertIn('#[path = "testdata/keyword_or_punctuation.rs"]', tests)
        command, expected = integration.RUST_WITNESS_TESTS[WITNESS]
        self.assertEqual(command[command.index("-p") + 1], "tsr_parser")
        self.assertEqual(expected, ["tokens::tests::keyword_or_punctuation_matches_the_pinned_kind_table"])
        self.assertIn("tools/phase1/parser-tokens", integration.RECEIPT_INPUTS["rust-witnesses"]["directories"])
        paths = set(integration.receipt_input_paths("rust-witnesses"))
        for name in ("export_test.go", "requests.json", "native-observations.json", "native-provenance.json",
                     "observe.py"):
            self.assertIn(f"tools/phase1/parser-tokens/{name}", paths)
        self.assertIn("crates/tsr_parser/src/testdata/keyword_or_punctuation.rs", paths)


if __name__ == "__main__":
    unittest.main()
