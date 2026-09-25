"""Phase 2 C0.4: every domain lands in exactly one category, from evidence only."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare as compare  # noqa: E402


def native(**overrides):
    value = {"state": "executed", "types": {"state": "content", "text_hex": "61"},
             "symbols": {"state": "content", "text_hex": "62"}, "queries": [],
             "trace": {"state": "disabled"}, "errors": {"state": "no_content"},
             "error_pre_diagnostics": [], "error_post_diagnostics": [], "error_diagnostics": [],
             "error_render_inputs": [], "error_pretty": False,
             "public_type_strings": {"state": "executed", "queries": []},
             "union_ordering": {"state": "executed", "checkers": 1, "unions": 3, "inconsistent": 0},
             "parent_pointers": {"state": "executed", "files": 1, "nodes": 9, "failure": None}}
    value.update(overrides)
    return value


def rust(**overrides):
    value = {"id": "v", "load": {"state": "executed"},
             "phases": {name: {"state": "executed", "diagnostics": []} for name in ("config", "program", "syntactic", "global")}
             | {"semantic": {"state": "executed", "files": []}},
             "error_baseline": {"state": "executed", "diagnostics": [], "baseline": {"state": "no_content"},
                                "emit": "not_executed", "pretty": False, "inputs": []},
             "type_symbol_baselines": {"state": "executed", "types": {"state": "content", "text_hex": "61"},
                                       "symbols": {"state": "content", "text_hex": "62"}, "queries": [],
                                       "public_type_strings": {"state": "executed", "queries": []}},
             "phase2": {"trace": {"state": "disabled"},
                        "union_ordering": {"state": "executed", "checkers": 1, "unions": 5, "inconsistent": 0},
                        "parent_pointers": {"state": "executed", "files": 1, "nodes": 9, "failure": None}}}
    value.update(overrides)
    return value


def categories(result):
    return {domain: value["category"] for domain, value in result.items()}


class Categorization(unittest.TestCase):
    def test_categories_are_closed(self):
        with self.assertRaises(ValueError):
            compare.outcome("partly")

    def test_matching_row(self):
        result = compare.compare_row(native(), rust(), None)
        self.assertEqual(set(categories(result).values()), {"match", "disabled"})
        self.assertEqual(result["trace"]["category"], "disabled")

    def test_union_count_is_not_graded_but_the_verdict_is(self):
        result = compare.compare_row(native(), rust(), None)
        self.assertEqual(result["union_ordering"]["category"], "match")
        broken = rust()
        broken["phase2"]["union_ordering"]["inconsistent"] = 2
        broken["phase2"]["parent_pointers"]["failure"] = "parent node does not exist"
        result = compare.compare_row(native(), broken, None)
        self.assertEqual(result["union_ordering"]["category"], "different")
        self.assertEqual(result["parent_pointers"]["category"], "different")

    def test_native_disabled_domains_stay_disabled_when_rust_fails(self):
        disabled = native(types={"state": "disabled"}, symbols={"state": "disabled"})
        fatal = {"id": "v", "fatal": {"state": "failed", "class": "panic", "reason": "boom"},
                 "panic_location": "crates/tsr_checker/src/x.rs:1"}
        result = compare.compare_row(disabled, fatal, None, "panic at crates/tsr_checker/src/x.rs:1")
        self.assertEqual(categories(result), {"errors": "failed", "types": "disabled", "symbols": "disabled",
                                              "display": "disabled", "trace": "disabled",
                                              "union_ordering": "failed", "parent_pointers": "failed"})

    def test_load_refusal_names_its_operation_everywhere_it_withholds(self):
        refused = rust(load={"state": "failed", "class": "unsupported", "reason": "content-mapper execution"},
                       error_baseline={"state": "not_implemented", "reason": "diagnostic aggregation not reached"},
                       type_symbol_baselines={"state": "not_implemented",
                                              "reason": "P5 native baseline walker/display schedule"},
                       phase2={"state": "not_reached"})
        result = compare.compare_row(native(), refused, None)
        for domain in ("errors", "types", "symbols", "display", "union_ordering", "parent_pointers"):
            self.assertEqual(result[domain], {"category": "unsupported", "operation": "content-mapper execution"}, domain)

    def test_wrapped_checker_refusal_is_unsupported(self):
        walked = rust(type_symbol_baselines={"state": "failed", "class": "walker_error",
                                             "reason": "unsupported checker operation: typeReferenceToTypeNode"})
        result = compare.compare_row(native(), walked, None)
        self.assertEqual(result["types"], {"category": "unsupported", "operation": "typeReferenceToTypeNode"})

    def test_production_error_is_failed_not_unsupported(self):
        walked = rust(type_symbol_baselines={"state": "failed", "class": "walker_error",
                                             "reason": "lazy roots must belong to the current transaction"})
        self.assertEqual(compare.compare_row(native(), walked, None)["types"]["category"], "failed")

    def test_pre_post_emit_difference_is_a_visible_difference(self):
        diagnostic = {"code": 2313, "pos": 1, "end": 2}
        emitted = native(error_pre_diagnostics=[diagnostic], error_post_diagnostics=[dict(diagnostic, pos=5)],
                         error_diagnostics=[dict(diagnostic, pos=5)])
        result = compare.compare_row(emitted, rust(), None)
        self.assertEqual(result["errors"]["category"], "different")
        self.assertIn("native_pre_post", result["errors"]["differences"])
        self.assertEqual(compare.area(result, rust()), "emit order: native pre/post-emit sets differ")

    def test_first_differing_code(self):
        self.assertEqual(compare.first_code([{"code": 1}, {"code": 2}], [{"code": 1}, {"code": 3}]), 2)
        self.assertEqual(compare.first_code([{"code": 1}], [{"code": 1}, {"code": 7}]), 7)
        self.assertIsNone(compare.first_code([{"code": 1}], [{"code": 1}]))

    def test_owner_follows_evidence_then_the_inventory(self):
        row = {"checkpoint": "C2"}
        self.assertEqual(compare.owner(row, {}, "unsupported: content-mapper execution"), "Phase 5 (content mapper)")
        self.assertEqual(compare.owner(row, {}, "emit order: native pre/post-emit sets differ"), "C5 / Phase 3 (emit order)")
        self.assertEqual(compare.owner(row, {}, "diagnostics: TS2322"), "C2")
        self.assertEqual(compare.owner({"checkpoint": "regression"}, {}, "diagnostics: TS2322"), "C1")




if __name__ == "__main__":
    unittest.main()
