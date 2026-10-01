"""Phase 3 T0: the Rust emit harness's row contract, capture replay and
comparison, against four real rows (native capture and Rust run, single mode):
a printed match whose output sub-test runs, a declaration-only root whose
output sub-test the pin disables, a file the pinned printer panics on and the
port refuses, and a content-mapped program with two files."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_compare as compare  # noqa: E402
import phase3_corpus as corpus  # noqa: E402
import s08_p4 as p4  # noqa: E402
from s08_oracle import digest  # noqa: E402

FIXTURE = ROOT / "scripts/tests/fixtures/phase3/corpus-rows.json"
PRINTED, DISABLED, REFUSED, MAPPED = range(4)
UNSUPPORTED = {"state": "failed", "class": "unsupported", "reason": "Program.Emit is Phase 3 T8"}


class Rows(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = json.loads(FIXTURE.read_bytes())["rows"]

    def triple(self, index):
        row = copy.deepcopy(self.fixture[index])
        return row["request"], row["rust"], row["native"]


class RowContract(Rows):
    def test_real_rows_validate(self):
        for index in range(len(self.fixture)):
            request, rust, _ = self.triple(index)
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(DISABLED)
        self.assertEqual(rust["output"]["state"], "disabled")
        self.assertEqual(rust["output"]["reason"], request["output"]["reason"])

    def test_requests_carry_the_runner_inputs_in_its_order(self):
        request, _, native = self.triple(MAPPED)
        native["baseline_inputs"] = request["baseline_inputs"]
        built = corpus.build_request({"id": request["id"], "output": request["output"]}, native,
                                     request["loading"], "single")
        self.assertEqual(built, request)
        inputs = request["baseline_inputs"]
        self.assertEqual(built["error_inputs"],
                         inputs["ts_config_files"] + inputs["to_be_compiled"] + inputs["other_files"])
        self.assertTrue(built["error_inputs"], "the content-mapped row's config parse needs its inputs")

    def test_rows_with_missing_or_extra_fields_are_rejected(self):
        request, rust, _ = self.triple(PRINTED)
        rust["extra"] = 1
        with self.assertRaisesRegex(ValueError, "missing or extra row field"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        del rust["sourcemap"]
        with self.assertRaisesRegex(ValueError, "missing or extra row field"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["id"] = "other"
        with self.assertRaisesRegex(ValueError, "missing/extra/reordered"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["mode"] = "concurrent"
        with self.assertRaisesRegex(ValueError, "mode differs"):
            corpus.validate_row(request, rust)

    def test_a_loaded_program_cannot_drop_its_reprint(self):
        request, rust, _ = self.triple(PRINTED)
        rust["reprint"] = {"state": "not_requested"}
        with self.assertRaisesRegex(ValueError, "silently dropped"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["reprint"] = {"state": "not_reached", "reason": "the program did not load"}
        with self.assertRaisesRegex(ValueError, "silently dropped"):
            corpus.validate_row(request, rust)
        # A program that did not load reports why, and the reprint inherits it.
        rust["load"] = {"state": "failed", "class": "config_parse", "reason": "bad config"}
        corpus.validate_row(request, rust)
        rust["reprint"] = {"state": "executed", "files": []}
        with self.assertRaisesRegex(ValueError, "without a loaded program"):
            corpus.validate_row(request, rust)

    def test_reprint_results_are_a_closed_vocabulary(self):
        request, rust, _ = self.triple(PRINTED)
        rust["reprint"]["files"][0]["comments"]["text_hex"] = "00"
        with self.assertRaisesRegex(ValueError, "malformed printed"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["reprint"]["files"][0]["comments"] = {"state": "panic", "message": "x"}
        with self.assertRaisesRegex(ValueError, "unknown reprint state"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(REFUSED)
        rust["reprint"]["files"][0]["comments"]["reason"] = ""
        with self.assertRaisesRegex(ValueError, "malformed refused"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["reprint"]["files"][0]["comments"] = {"state": "failed", "class": "panic", "reason": "boom"}
        with self.assertRaisesRegex(ValueError, "malformed failed"):
            corpus.validate_row(request, rust)
        rust["reprint"]["files"][0]["comments"]["location"] = "crates/tsr_printer/src/printer.rs:1:1"
        corpus.validate_row(request, rust)

    def test_reprint_lists_each_non_library_file_once(self):
        request, rust, _ = self.triple(MAPPED)
        rust["reprint"]["files"][1] = copy.deepcopy(rust["reprint"]["files"][0])
        with self.assertRaisesRegex(ValueError, "listed twice"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["load"]["libraries"] = rust["load"]["files"]
        with self.assertRaisesRegex(ValueError, "more files than"):
            corpus.validate_row(request, rust)

    def test_emit_domains_follow_the_request(self):
        request, rust, _ = self.triple(DISABLED)
        rust["output"] = dict(UNSUPPORTED)
        with self.assertRaisesRegex(ValueError, "disabled output sub-test ran"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(DISABLED)
        rust["output"]["reason"] = "another reason"
        with self.assertRaisesRegex(ValueError, "disabled output sub-test ran"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["output"] = {"state": "disabled", "reason": "x"}
        with self.assertRaisesRegex(ValueError, "disabled a sub-test the runner runs"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["emit"] = {"state": "failed", "class": "unsupported", "reason": ""}
        with self.assertRaisesRegex(ValueError, "malformed failure: emit"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        request["emit"] = False
        with self.assertRaisesRegex(ValueError, "unrequested emit domain"):
            corpus.validate_row(request, rust)

    def test_fatal_rows_carry_nothing_else_and_panics_name_a_location(self):
        request, _, _ = self.triple(PRINTED)
        fatal = {"version": 1, "id": request["id"], "acceptance_tier": "executed",
                 "fatal": {"state": "failed", "class": "panic", "reason": "boom"}}
        with self.assertRaisesRegex(ValueError, "panic location"):
            corpus.validate_row(request, fatal)
        fatal["panic_location"] = "crates/tsr_printer/src/printer.rs:10:5"
        corpus.validate_row(request, fatal)
        fatal["load"] = {"state": "executed", "files": 1, "libraries": 0}
        with self.assertRaisesRegex(ValueError, "fabricated partial success"):
            corpus.validate_row(request, fatal)
        timeout = p4.fatal(request, "timeout", "variant exceeded 60 seconds")
        corpus.validate_row(request, timeout)
        timeout["panic_location"] = None
        with self.assertRaisesRegex(ValueError, "fabricated partial success"):
            corpus.validate_row(request, timeout)

    def test_adapter_panics_are_harness_errors(self):
        request, rust, _ = self.triple(PRINTED)
        self.assertEqual(corpus.completed_problems(rust), [])
        rust["reprint"]["files"][0]["comments"] = {"state": "failed", "class": "panic", "reason": "boom",
                                                   "location": "tools/phase3/harness/reprint.rs:40:9"}
        self.assertEqual(corpus.completed_problems(rust), ["reprint panic at tools/phase3/harness/reprint.rs:40:9"])
        fatal = {"version": 1, "id": request["id"], "acceptance_tier": "executed", "panic_location":
                 "crates/tsr_compiler/examples/phase3_emit.rs:9:1",
                 "fatal": {"state": "failed", "class": "panic", "reason": "boom"}}
        self.assertEqual(corpus.attribute(fatal, b"")[0], "harness")
        fatal["panic_location"] = "crates/tsr_printer/src/printer.rs:9:1"
        self.assertEqual(corpus.attribute(fatal, b""), ("production", "panic at crates/tsr_printer/src/printer.rs:9:1"))


class Comparison(Rows):
    def outcomes(self, index, mutate=None):
        request, rust, native = self.triple(index)
        if mutate:
            mutate(rust, native)
            if "fatal" not in rust:
                corpus.validate_row(request, rust)
        return compare.compare_row(native, rust)

    def test_real_rows_compare(self):
        for index in range(len(self.fixture)):
            result = self.outcomes(index)
            self.assertEqual(set(result), set(compare.DOMAINS))
            self.assertEqual(result["reprint"]["category"], "match")
            for domain in ("sourcemap", "sourcemap_record", "emit_diagnostics"):
                self.assertEqual(result[domain], {"category": "unsupported", "operation": "Program.Emit is Phase 3 T8"})
        self.assertEqual(self.outcomes(DISABLED)["output"],
                         {"category": "disabled", "reason": "no input file other than declaration files"})
        self.assertEqual(self.outcomes(PRINTED)["output"]["category"], "unsupported")

    def test_refusal_matches_only_a_pinned_panic(self):
        def printed(rust, native):
            rust["reprint"]["files"][0]["comments"] = {"state": "printed", "sha256": "0" * 64, "bytes": 3}
        result = self.outcomes(REFUSED, printed)["reprint"]
        self.assertEqual((result["category"], result["kind"], result["extension"], result["mode"]),
                         ("different", "native_panic", ".js", "comments"))

        def refused(rust, native):
            rust["reprint"]["files"][0]["no_comments"] = {"state": "refused", "reason": "printer does not support X yet"}
        result = self.outcomes(PRINTED, refused)["reprint"]
        self.assertEqual((result["category"], result["kind"], result["mode"]), ("different", "rust_refused", "no_comments"))

    def test_text_differences_name_the_first_differing_file(self):
        def text(rust, native):
            rust["reprint"]["files"][1]["no_comments"]["bytes"] += 1
        result = self.outcomes(MAPPED, text)["reprint"]
        self.assertEqual((result["category"], result["kind"], result["file"], result["extension"], result["mode"]),
                         ("different", "text", "/component.vue", ".vue", "no_comments"))

        def digest_only(rust, native):
            rust["reprint"]["files"][0]["comments"]["sha256"] = "f" * 64
        self.assertEqual(self.outcomes(PRINTED, digest_only)["reprint"]["extension"], ".ts")

    def test_file_lists_must_agree_in_name_digest_and_order(self):
        def reorder(rust, native):
            rust["reprint"]["files"].reverse()
        self.assertEqual(self.outcomes(MAPPED, reorder)["reprint"]["kind"], "file_list")

        def missing(rust, native):
            rust["reprint"]["files"].pop()
        result = self.outcomes(MAPPED, missing)["reprint"]
        self.assertEqual((result["kind"], result["native_files"], result["rust_files"]), ("file_list", 2, 1))

        def source(rust, native):
            native["reprint"][0]["source_sha256"] = "0" * 64
        self.assertEqual(self.outcomes(PRINTED, source)["reprint"]["kind"], "file_list")

    def test_failures_and_unexecuted_rows_are_never_blank(self):
        def panic(rust, native):
            rust["reprint"]["files"][0]["comments"] = {"state": "failed", "class": "panic", "reason": "boom",
                                                       "location": "crates/tsr_printer/src/printer.rs:1:1"}
        result = self.outcomes(PRINTED, panic)["reprint"]
        self.assertEqual(result["category"], "failed")
        self.assertIn("at crates/tsr_printer/src/printer.rs:1:1", result["reason"])

        def unloaded(rust, native):
            rust["load"] = {"state": "failed", "class": "unsupported", "reason": "a loader gap"}
            rust["reprint"] = {"state": "not_reached", "reason": "the program did not load"}
        self.assertEqual(self.outcomes(PRINTED, unloaded)["reprint"], {"category": "unsupported", "operation": "a loader gap"})

        def fatal(rust, native):
            for key in list(rust):
                if key not in ("version", "id", "acceptance_tier"):
                    del rust[key]
            rust["fatal"] = {"state": "failed", "class": "timeout", "reason": "variant exceeded 60 seconds"}
        result = self.outcomes(DISABLED, fatal)
        self.assertEqual(result["output"]["category"], "disabled")
        self.assertEqual({result[d]["category"] for d in compare.DOMAINS if d != "output"}, {"failed"})

        def native_failed(rust, native):
            native["state"] = "upstream_failed"
        self.assertEqual({v["category"] for v in self.outcomes(PRINTED, native_failed).values()}, {"unexecuted"})

    def test_executed_emit_domains_compare_their_text(self):
        def content(rust, native):
            rust["output"] = {key: native["output"][key] for key in ("state", "name", "text_hex")}
        request, rust, native = self.triple(PRINTED)
        content(rust, native)
        self.assertEqual(compare.compare_baseline("output", native, rust)["category"], "match")
        rust["output"]["text_hex"] += "0a"
        self.assertEqual(compare.compare_baseline("output", native, rust)["category"], "different")
        rust["output"] = {"state": "no_content", "name": native["output"]["name"]}
        self.assertEqual(compare.compare_baseline("output", native, rust)["category"], "different")


class Captures(Rows):
    """Completion records are bound to their requests, capture and raw
    artifacts; an incomplete capture is never replayed or compared."""

    def make(self, directory, complete=True, partial=True):
        requests = [self.triple(PRINTED)[0], self.triple(DISABLED)[0]]
        rows = [self.triple(PRINTED)[1], self.triple(DISABLED)[1]]
        (directory / "cases").mkdir()
        (directory / "source-snapshot").mkdir()
        (directory / "executable").write_bytes(b"binary")
        selection = {"sample": False, "cases": [r["id"] for r in requests], "limit": None}
        metadata = {"version": 1, "requests_sha256": digest(p4.canonical(requests) + b"\n"),
                    "build": {"sources": {}, "binary_sha256": digest(b"binary")}, "timeout_seconds": 60,
                    "native": {}, "partial": partial, "selection": selection, "mode": "single"}
        p4.write_new(directory / "requests.json", requests)
        p4.write_new(directory / "capture.json", metadata)
        for index, (request, row) in enumerate(zip(requests, rows)):
            if index and not complete:
                break
            case = directory / "cases" / f"{index:05d}"
            p4.begin_case(case, request)
            (case / "stdout").write_bytes(b"")
            (case / "stderr").write_bytes(b"")
            (case / "observation.json").write_bytes(json.dumps(row).encode())
            p4.complete_case(case, request, metadata, row)
        return requests, rows

    def test_a_complete_capture_loads(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            requests, rows = self.make(directory)
            _, loaded_requests, loaded_rows, _ = corpus.load_capture(directory)
            self.assertEqual((loaded_requests, loaded_rows), (requests, rows))

    def test_an_incomplete_capture_is_never_replayed(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make(directory, complete=False)
            with self.assertRaisesRegex(ValueError, "capture incomplete at"):
                corpus.load_capture(directory)

    def test_a_partial_selection_must_be_flagged(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make(directory, partial=False)
            with self.assertRaisesRegex(ValueError, "disagrees with partial flag"):
                corpus.load_capture(directory)

    def test_changed_artifacts_and_foreign_completions_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make(directory)
            (directory / "cases/00001/stderr").write_bytes(b"later")
            with self.assertRaisesRegex(ValueError, "raw case artifact changed"):
                corpus.load_capture(directory)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make(directory)
            (directory / "cases/00002").mkdir()
            with self.assertRaisesRegex(ValueError, "outside the request inventory"):
                corpus.load_capture(directory)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.make(directory)
            (directory / "executable").write_bytes(b"rebuilt")
            with self.assertRaisesRegex(ValueError, "captured executable changed"):
                corpus.load_capture(directory)

    def test_selection_flags(self):
        self.assertEqual(corpus.phase2_corpus.selection(), {"sample": False, "cases": [], "limit": None})
        with self.assertRaisesRegex(ValueError, "--limit"):
            corpus.phase2_corpus.selection(sample=True, limit=3)


if __name__ == "__main__":
    unittest.main()
