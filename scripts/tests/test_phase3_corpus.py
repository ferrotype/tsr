"""Phase 3: the Rust emit harness's row contract, capture replay and
comparison, against four real rows (native capture and the C2 Rust run,
single mode): a printed match whose output sub-test runs its noCheck repeat, a
declaration-only root whose output sub-test the pin disables, a `noEmit` file
the pinned printer panics on and the port refuses, and a content-mapped
program with two files whose emitted declaration differs from the pin's."""
import copy
import hashlib
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
UNSUPPORTED = {"state": "failed", "class": "unsupported", "reason": "a production refusal"}
EMIT_DOMAINS = ("output", "sourcemap", "sourcemap_record", "emit_diagnostics", "declaration")


def fail_emit(rust, failure):
    """A row whose first compilation failed: every sub-test carries it."""
    for name in corpus.EMIT_DOMAINS:
        rust[name] = copy.deepcopy(failure)
    rust["compilations"] = []


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
        built = corpus.build_request({"id": request["id"], "output": request["output"],
                                      "configured_name": request["configured_name"], "suite": request["suite"]},
                                     native, request["loading"], "single")
        self.assertEqual(built, request)
        self.assertEqual(sorted(built["harness_options"]), sorted(corpus.HARNESS_OPTIONS))
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
        del rust["compilations"]
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

    def test_reprinted_files_carry_their_script_kind_and_language_variant(self):
        for key in ("script_kind", "language_variant"):
            request, rust, _ = self.triple(PRINTED)
            del rust["reprint"]["files"][0][key]
            with self.assertRaisesRegex(ValueError, "malformed reprint file"):
                corpus.validate_row(request, rust)
            for value in (True, -1, "3", None):
                request, rust, _ = self.triple(PRINTED)
                rust["reprint"]["files"][0][key] = value
                with self.assertRaisesRegex(ValueError, "malformed reprint file kind"):
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
        rust["output"] = {"state": "no_content", "name": "compiler/missingRequiredDeclare.js"}
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
        fail_emit(rust, {"state": "failed", "class": "unsupported", "reason": ""})
        with self.assertRaisesRegex(ValueError, "malformed failure: emit"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        request["emit"] = False
        with self.assertRaisesRegex(ValueError, "unrequested emit domain"):
            corpus.validate_row(request, rust)

    def test_an_executed_emit_is_complete_and_consistent(self):
        request, rust, _ = self.triple(PRINTED)
        del rust["emit"]["outputs"]
        with self.assertRaisesRegex(ValueError, "malformed executed emit"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(MAPPED)
        rust["emit"]["diagnostics"] += 1
        with self.assertRaisesRegex(ValueError, "not the harness's"):
            corpus.validate_row(request, rust)
        # A count mismatch: the shorter count and the mismatch diagnostic.
        request, rust, _ = self.triple(MAPPED)
        rust["emit"].update(pre_diagnostics=3, post_diagnostics=1, diagnostics=2)
        with self.assertRaisesRegex(ValueError, "first compilation's counts differ"):
            corpus.validate_row(request, rust)
        rust["compilations"][0].update(pre_diagnostics=3, post_diagnostics=1)
        corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["emit"]["outputs"]["js"][0]["text_hex"] = "00"
        with self.assertRaisesRegex(ValueError, "malformed emitted file"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(MAPPED)
        rust["emit"]["outputs"]["dts"][1]["name_hex"] = rust["emit"]["outputs"]["dts"][0]["name_hex"]
        with self.assertRaisesRegex(ValueError, "listed twice"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["emit"]["result"]["emit_skipped"] = "no"
        with self.assertRaisesRegex(ValueError, "malformed emit result"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["emit"]["result"]["diagnostics"] = [{"code": 1}]
        with self.assertRaisesRegex(ValueError, "malformed emit diagnostic"):
            corpus.validate_row(request, rust)
        # A nil emit result is the pin's absent one.
        request, rust, _ = self.triple(PRINTED)
        rust["emit"]["result"] = None
        corpus.validate_row(request, rust)

    def test_compilations_follow_the_pins_order(self):
        request, rust, _ = self.triple(PRINTED)
        self.assertEqual([c["compilation"] for c in rust["compilations"]], ["first", "repeat"])
        rust["compilations"].insert(1, dict(rust["compilations"][1], compilation="declaration"))
        corpus.validate_row(request, rust)
        rust["compilations"][1:] = reversed(rust["compilations"][1:])
        with self.assertRaisesRegex(ValueError, "out of the pin's order"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["compilations"].pop(0)
        with self.assertRaisesRegex(ValueError, "out of the pin's order"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["compilations"][1]["compilation"] = "second"
        with self.assertRaisesRegex(ValueError, "malformed compilation"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(DISABLED)
        rust["compilations"].append(dict(rust["compilations"][0], compilation="repeat"))
        with self.assertRaisesRegex(ValueError, "disabled output sub-test ran its compilations"):
            corpus.validate_row(request, rust)

    def test_composed_baselines_are_a_closed_vocabulary(self):
        request, rust, _ = self.triple(PRINTED)
        rust["output"] = {"state": "not_baselined"}
        with self.assertRaisesRegex(ValueError, "only the sourcemap sub-test"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["output"]["name"] = "conformance/2dArrays.js"
        with self.assertRaisesRegex(ValueError, "outside the request's suite"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["output"]["text_hex"] = "00"
        with self.assertRaisesRegex(ValueError, "malformed composed baseline"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["sourcemap_record"]["state"] = "empty"
        with self.assertRaisesRegex(ValueError, "unknown sourcemap_record state"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        rust["output"] = {"state": "failed", "class": "assertion",
                          "reason": "Expected at least one js file to be emitted or at least one error to be created."}
        corpus.validate_row(request, rust)
        rust["output"]["class"] = "surprise"
        with self.assertRaisesRegex(ValueError, "malformed failure: output"):
            corpus.validate_row(request, rust)

    def test_a_failed_compilation_fails_every_sub_test(self):
        request, rust, _ = self.triple(PRINTED)
        fail_emit(rust, UNSUPPORTED)
        corpus.validate_row(request, rust)
        rust["sourcemap"] = {"state": "not_baselined"}
        with self.assertRaisesRegex(ValueError, "differs from its failure"):
            corpus.validate_row(request, rust)
        request, rust, _ = self.triple(PRINTED)
        fail_emit(rust, UNSUPPORTED)
        rust["compilations"] = [{"compilation": "first", "pre_diagnostics": 0, "post_diagnostics": 0}]
        with self.assertRaisesRegex(ValueError, "without an executed emit"):
            corpus.validate_row(request, rust)
        # A panic names its location; nothing else does.
        request, rust, _ = self.triple(DISABLED)
        fail_emit(rust, {"state": "failed", "class": "panic", "reason": "boom"})
        rust["output"] = {"state": "disabled", "reason": request["output"]["reason"]}
        with self.assertRaisesRegex(ValueError, "malformed failure: emit"):
            corpus.validate_row(request, rust)
        fail_emit(rust, {"state": "failed", "class": "panic", "reason": "boom",
                         "location": "crates/tsr_printer/src/printer.rs:1"})
        rust["output"] = {"state": "disabled", "reason": request["output"]["reason"]}
        corpus.validate_row(request, rust)
        rust["emit"]["location"] = 7
        with self.assertRaisesRegex(ValueError, "malformed failure location: emit"):
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

    def test_harness_failures_in_the_emit_domains_are_harness_errors(self):
        request, rust, _ = self.triple(PRINTED)
        fail_emit(rust, {"state": "failed", "class": "harness", "reason": "a malformed request"})
        self.assertEqual(corpus.completed_problems(rust), ["harness failure in emit: a malformed request"])
        fail_emit(rust, {"state": "failed", "class": "panic", "reason": "boom",
                         "location": "tools/phase3/harness/emit.rs:10"})
        self.assertEqual(corpus.completed_problems(rust), ["emit panic at tools/phase3/harness/emit.rs:10"])
        fail_emit(rust, {"state": "failed", "class": "panic", "reason": "boom",
                         "location": "crates/tsr_compiler/src/emitter.rs:10"})
        self.assertEqual(corpus.completed_problems(rust), [])
        request, rust, _ = self.triple(PRINTED)
        rust["sourcemap"] = {"state": "failed", "class": "panic", "reason": "boom", "location": None}
        self.assertEqual(corpus.completed_problems(rust), ["sourcemap panic at unknown location"])
        rust["sourcemap"] = {"state": "failed", "class": "harness", "reason": "no repeat outputs"}
        self.assertEqual(corpus.completed_problems(rust), ["harness failure in sourcemap: no repeat outputs"])
        rust["sourcemap"] = {"state": "failed", "class": "runtime", "reason": "json.Unmarshal of a source map"}
        self.assertEqual(corpus.completed_problems(rust), [])


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
                self.assertEqual(result[domain], {"category": "match"})
        self.assertEqual(self.outcomes(DISABLED)["output"],
                         {"category": "disabled", "reason": "no input file other than declaration files"})
        for index in (PRINTED, DISABLED, REFUSED):
            self.assertEqual({d: v["category"] for d, v in self.outcomes(index).items() if d != "output"},
                             dict.fromkeys(set(compare.DOMAINS) - {"output"}, "match"))
        self.assertEqual(self.outcomes(PRINTED)["output"], {"category": "match"})
        self.assertEqual(self.outcomes(REFUSED)["output"], {"category": "match"})
        # The content-mapped row's emitted declaration differs from the pin's
        # (the supplemental module's reference): the .js baseline differs at it.
        result = self.outcomes(MAPPED)
        self.assertEqual(result["declaration"], {"category": "different", "kind": "dts", "file_difference": "text",
                                                 "file": "/component.d.vue.ts"})
        self.assertEqual(result["output"], {"category": "different", "difference": "text", "kind": "dts",
                                            "file_difference": "text", "file": "/component.d.vue.ts",
                                            "native_state": "content", "rust_state": "content"})

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

    def test_a_refusal_matches_only_the_pinned_panic_message(self):
        """Mutation check: a refusal with another reason where the pin panics
        must not match."""
        request, rust, native = self.triple(REFUSED)
        pinned = native["reprint"][0]["comments"]
        self.assertEqual(pinned, {"state": "panic", "message": "unhandled statement: KindJSImportDeclaration"})
        self.assertTrue(compare.print_agrees(pinned, rust["reprint"]["files"][0]["comments"]))

        def other_reason(rust, native):
            rust["reprint"]["files"][0]["comments"]["reason"] = "unhandled statement: KindImportDeclaration"
        result = self.outcomes(REFUSED, other_reason)["reprint"]
        self.assertEqual((result["category"], result["kind"], result["mode"]), ("different", "refusal_reason", "comments"))
        self.assertEqual(result["native"], pinned)

        def other_panic(rust, native):
            native["reprint"][0]["no_comments"]["message"] = "runtime error: index out of range"
        result = self.outcomes(REFUSED, other_panic)["reprint"]
        self.assertEqual((result["category"], result["kind"], result["mode"]), ("different", "refusal_reason", "no_comments"))

        request, rust, native = self.triple(REFUSED)
        other_reason(rust, native)
        self.assertEqual([(reason, confirmed) for reason, confirmed, _, _ in compare.refusals(native, rust)],
                         [("unhandled statement: KindImportDeclaration", False),
                          ("unhandled statement: KindJSImportDeclaration", True)])

    def test_file_kinds_must_be_the_pins(self):
        self.assertEqual(self.triple(REFUSED)[2]["reprint"][0]["script_kind"], 1)
        for key, value in (("script_kind", 3), ("language_variant", 0)):
            def kind(rust, native, key=key, value=value):
                rust["reprint"]["files"][0][key] = value
            result = self.outcomes(REFUSED, kind)["reprint"]
            self.assertEqual((result["category"], result["kind"], result["file"]), ("different", "file_kind", "/foo.js"))
            self.assertEqual(result["native"], {"script_kind": 1, "language_variant": 1})

        def second_file(rust, native):
            native["reprint"][1]["language_variant"] = 1
        result = self.outcomes(MAPPED, second_file)["reprint"]
        self.assertEqual((result["kind"], result["extension"]), ("file_kind", ".vue"))

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

        def refused(rust, native):
            fail_emit(rust, UNSUPPORTED)
        result = self.outcomes(PRINTED, refused)
        self.assertEqual({d: result[d] for d in EMIT_DOMAINS},
                         dict.fromkeys(EMIT_DOMAINS, {"category": "unsupported", "operation": "a production refusal"}))
        self.assertEqual(result["reprint"]["category"], "match")

        def emit_panic(rust, native):
            fail_emit(rust, {"state": "failed", "class": "panic", "reason": "boom",
                             "location": "crates/tsr_compiler/src/emitter.rs:9"})
            rust["output"] = {"state": "disabled", "reason": "no input file other than declaration files"}
        result = self.outcomes(DISABLED, emit_panic)
        self.assertEqual(result["output"]["category"], "disabled")
        self.assertEqual(result["emit_diagnostics"],
                         {"category": "failed", "reason": "panic: boom at crates/tsr_compiler/src/emitter.rs:9"})

    def test_baselines_compare_state_name_digest_and_size(self):
        """Mutation-checked: each of the four parts decides a match."""
        request, rust, native = self.triple(PRINTED)
        self.assertEqual(compare.compare_baseline("output", native, rust), {"category": "match"})
        for key, value in (("sha256", "f" * 64), ("bytes", 1), ("name", "compiler/other.js")):
            request, rust, native = self.triple(PRINTED)
            rust["output"][key] = value
            result = compare.compare_baseline("output", native, rust)
            self.assertEqual((result["category"], result["difference"], result["kind"]),
                             ("different", "name" if key == "name" else "text", "baseline"))
        request, rust, native = self.triple(PRINTED)
        rust["output"] = {"state": "no_content", "name": rust["output"]["name"]}
        result = compare.compare_baseline("output", native, rust)
        self.assertEqual((result["difference"], result["native_state"], result["rust_state"]),
                         ("state", "content", "no_content"))
        request, rust, native = self.triple(PRINTED)
        rust["sourcemap_record"]["name"] = "compiler/other.sourcemap.txt"
        self.assertEqual(compare.compare_baseline("sourcemap_record", native, rust)["difference"], "name")
        request, rust, native = self.triple(PRINTED)
        rust["sourcemap"] = {"state": "no_content", "name": "compiler/2dArrays.js.map"}
        self.assertEqual(compare.compare_baseline("sourcemap", native, rust)["difference"], "state")

    def test_a_difference_names_the_first_emitted_file_that_differs(self):
        request, rust, native = self.triple(PRINTED)
        rust["output"]["sha256"] = "f" * 64
        rust["emit"]["outputs"]["js"][0]["sha256"] = "f" * 64
        result = compare.compare_baseline("output", native, rust)
        self.assertEqual((result["kind"], result["file_difference"], result["file"]), ("js", "text", "/.src/2dArrays.js"))
        # A file the pin did not write, and one Rust did not write.
        request, rust, native = self.triple(PRINTED)
        rust["output"]["bytes"] += 1
        rust["emit"]["outputs"]["dts"] = [{"name_hex": "2f2e7372632f32644172726179732e642e7473", "sha256": "0" * 64,
                                            "bytes": 0}]
        result = compare.compare_baseline("output", native, rust)
        self.assertEqual((result["kind"], result["file_difference"], result["file"]), ("dts", "extra", "/.src/2dArrays.d.ts"))
        request, rust, native = self.triple(MAPPED)
        rust["emit"]["outputs"]["dts"].pop(0)
        result = compare.compare_baseline("output", native, rust)
        self.assertEqual((result["kind"], result["file_difference"], result["file"]),
                         ("dts", "missing", "/component.vue.0.d.ts"))
        # The source-map baselines look at the maps first; the record also at
        # the declarations, the .js baseline never at the maps.
        request, rust, native = self.triple(PRINTED)
        rust["sourcemap_record"] = {"state": "content", "name": "compiler/2dArrays.sourcemap.txt",
                                    "sha256": "0" * 64, "bytes": 1}
        rust["emit"]["outputs"]["maps"] = [{"name_hex": "2f2e7372632f32644172726179732e6a732e6d6170",
                                             "sha256": "0" * 64, "bytes": 1}]
        rust["emit"]["outputs"]["js"][0]["sha256"] = "f" * 64
        self.assertEqual(compare.compare_baseline("sourcemap_record", native, rust)["kind"], "map")
        rust["output"]["sha256"] = "f" * 64
        self.assertEqual(compare.compare_baseline("output", native, rust)["kind"], "js")

    def test_a_writer_fault_on_the_rust_outputs_is_a_difference(self):
        request, rust, native = self.triple(PRINTED)
        rust["output"] = {"state": "failed", "class": "assertion",
                          "reason": "Expected at least one js file to be emitted or at least one error to be created."}
        rust["emit"]["outputs"]["js"] = []
        result = compare.compare_baseline("output", native, rust)
        self.assertEqual((result["category"], result["difference"], result["kind"], result["file_difference"]),
                         ("different", "assertion", "js", "missing"))
        rust["output"]["class"] = "error_baseline"
        self.assertEqual(compare.compare_baseline("output", native, rust)["category"], "failed")

    def test_emit_diagnostics_compare_the_result_then_the_counts(self):
        def outcome(index, mutate):
            request, rust, native = self.triple(index)
            mutate(rust, native)
            corpus.validate_row(request, rust)
            return compare.compare_emit_diagnostics(native, rust)

        def skipped(rust, native):
            rust["emit"]["result"]["emit_skipped"] = True
        self.assertEqual(outcome(PRINTED, skipped),
                         {"category": "different", "kind": "emit_skipped", "native": False, "rust": True})

        def emitted(rust, native):
            rust["emit"]["result"]["emitted_files_hex"].pop()
        result = outcome(MAPPED, emitted)
        self.assertEqual((result["kind"], result["native"], result["rust"]),
                         ("emitted_files", ["/component.vue.0.d.ts", "/component.d.vue.ts"], ["/component.vue.0.d.ts"]))

        def reordered(rust, native):
            rust["emit"]["result"]["emitted_files_hex"].reverse()
        self.assertEqual(outcome(MAPPED, reordered)["kind"], "emitted_files")

        def diagnostic(rust, native):
            native["emit"]["diagnostics"] = [{"file_hex": None, "pos": 0, "end": 0, "code": 5033, "category": 1,
                                              "key_hex": "", "text_hex": "", "args_hex": [], "chain": [], "related": []}]
            rust["emit"]["result"]["diagnostics"] = [dict(native["emit"]["diagnostics"][0])]
        self.assertEqual(outcome(PRINTED, diagnostic), {"category": "match"})

        def diagnostic_position(rust, native):
            diagnostic(rust, native)
            rust["emit"]["result"]["diagnostics"][0]["pos"] = 1
        self.assertEqual(outcome(PRINTED, diagnostic_position),
                         {"category": "different", "kind": "diagnostics", "native_codes": [5033], "rust_codes": [5033]})

        def source_maps(rust, native):
            rust["emit"]["result"]["source_maps"] = 1
        self.assertEqual(outcome(PRINTED, source_maps), {"category": "different", "kind": "source_maps"})

        def absent(rust, native):
            rust["emit"]["result"] = None
        self.assertEqual(outcome(PRINTED, absent), {"category": "different", "kind": "result",
                                                    "native_state": "executed", "rust_state": "absent"})

        def count(rust, native):
            rust["emit"].update(pre_diagnostics=1, post_diagnostics=1, diagnostics=1)
            rust["compilations"][0].update(pre_diagnostics=1, post_diagnostics=1)
        self.assertEqual(outcome(DISABLED, count),
                         {"category": "different", "kind": "counts", "compilation": "first", "native": [2, 2, 2],
                          "rust": [1, 1, 1]})

    def test_the_counts_are_the_last_compilations(self):
        """The native oracle's hook records each compilation of the row, so
        its pre- and post-emit counts are the last one's: the content-mapped
        row's first compilation has 1 and 1, its repeat 0 and 0, the native
        row 0 and 0 with len(result.Diagnostics) 1."""
        request, rust, native = self.triple(MAPPED)
        self.assertEqual([(c["compilation"], c["pre_diagnostics"], c["post_diagnostics"]) for c in rust["compilations"]],
                         [("first", 1, 1), ("repeat", 0, 0)])
        self.assertEqual((native["pre_diagnostics"], native["post_diagnostics"], native["diagnostics"]), (0, 0, 1))
        self.assertEqual(compare.compare_emit_diagnostics(native, rust), {"category": "match"})
        rust["compilations"][-1]["post_diagnostics"] = 1
        self.assertEqual(compare.compare_emit_diagnostics(native, rust),
                         {"category": "different", "kind": "counts", "compilation": "repeat", "native": [0, 0, 1],
                          "rust": [0, 1, 1]})
        request, rust, native = self.triple(MAPPED)
        rust["compilations"].pop()
        self.assertEqual(compare.compare_emit_diagnostics(native, rust)["kind"], "counts")
        request, rust, native = self.triple(MAPPED)
        rust["emit"]["diagnostics"] = 2
        self.assertEqual(compare.compare_emit_diagnostics(native, rust)["kind"], "counts")

    def test_declarations_compare_the_emitted_declaration_files(self):
        request, rust, native = self.triple(MAPPED)
        self.assertTrue(compare.declaration_required(native, compare.compare_declaration(native, rust)))
        rust["emit"]["outputs"]["dts"] = copy.deepcopy(native["outputs"]["dts"])
        self.assertEqual(compare.compare_declaration(native, rust), {"category": "match"})
        self.assertTrue(compare.declaration_required(native, {"category": "match"}))
        rust["emit"]["outputs"]["dts"][0]["bytes"] += 1
        self.assertEqual(compare.compare_declaration(native, rust),
                         {"category": "different", "kind": "dts", "file_difference": "text",
                          "file": "/component.vue.0.d.ts"})
        rust["emit"]["outputs"]["dts"].reverse()
        self.assertEqual(compare.compare_declaration(native, rust)["file_difference"], "order")
        # No declaration on either side: it matches and is not required; a
        # Rust-only declaration differs and is.
        request, rust, native = self.triple(PRINTED)
        result = compare.compare_declaration(native, rust)
        self.assertEqual((result, compare.declaration_required(native, result)), ({"category": "match"}, False))
        rust["emit"]["outputs"]["dts"] = [{"name_hex": "2f2e7372632f32644172726179732e642e7473", "sha256": "0" * 64,
                                            "bytes": 0}]
        result = compare.compare_declaration(native, rust)
        self.assertEqual((result["category"], result["file_difference"], compare.declaration_required(native, result)),
                         ("different", "extra", True))

    def test_executed_emit_domains_compare_their_text(self):
        request, rust, native = self.triple(MAPPED)
        self.assertEqual(compare.compare_baseline("output", native, rust)["category"], "different")
        text = bytes.fromhex(native["output"]["text_hex"])
        rust["output"].update(sha256=hashlib.sha256(text).hexdigest(), bytes=len(text))
        self.assertEqual(compare.compare_baseline("output", native, rust)["category"], "match")
        native["output"]["text_hex"] += "0a"
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
