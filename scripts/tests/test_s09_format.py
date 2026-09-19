import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from s09_format import (ENTRY_VARIANTS, GLOBAL_OPS, FORMAT_VARIANTS, INDENT_VARIANTS, INSERT_VARIANTS, OPS, differences, frozen_form, validate_observation, validate_stream,
                        walk_streams)

SHA = "0" * 64


def observation(ops=OPS):
    value = {"id": "case/unit/0/config/0", "parse": {"end": 1, "node_count": 2, "language_variant": 0, "lines": 1}}
    if "nav" in ops:
        value["nav"] = {"rows": 3, "failures": 0, "sha256": SHA}
    if "indent" in ops:
        value["indent"] = {name: {"rows": 2, "failures": 0, "sha256": SHA} for name in INDENT_VARIANTS}
    if "position" in ops:
        value["position"] = {"rows": 5, "failures": 0, "sha256": SHA}
    if "scan" in ops:
        value["scan"] = {"rows": 7, "failures": 0, "sha256": SHA}
    if "rules" in ops:
        value["rules"] = {name: {"rows": 6, "failures": 0, "sha256": SHA} for name in FORMAT_VARIANTS}
    if "insert" in ops:
        value["insert"] = {name: {"rows": 4, "failures": 1, "sha256": SHA} for name in INSERT_VARIANTS}
    if "entry" in ops:
        value["entry"] = {name: {"rows": 8, "failures": 0, "sha256": SHA} for name in ENTRY_VARIANTS}
    if "format" in ops:
        value["format"] = {name: {"rows": 1, "failures": 0, "sha256": SHA, "text_sha256": SHA} for name in FORMAT_VARIANTS}
    return value


class FormatObservationContract(unittest.TestCase):
    def setUp(self):
        self.request = {"id": "case/unit/0/config/0"}

    def test_a_complete_observation_is_accepted_for_every_operation_subset(self):
        for ops in (OPS, ("nav",), ("indent", "format"), ("format",)):
            with self.subTest(ops=ops):
                validate_observation(self.request, observation(ops), ops)

    def test_another_identity_an_oracle_error_or_a_missing_or_extra_operation_is_rejected(self):
        for change in ({"id": "other"}, {"error": "unknown operation"}, {"extra": {}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_observation(self.request, {**observation(), **change}, OPS)
        for op in OPS:
            value = observation()
            del value[op]
            with self.subTest(missing=op), self.assertRaises(ValueError):
                validate_observation(self.request, value, OPS)
        with self.assertRaises(ValueError):
            validate_observation(self.request, observation(), ("nav",))

    def test_the_variant_sets_are_part_of_the_contract(self):
        for op, names in (("indent", INDENT_VARIANTS), ("format", FORMAT_VARIANTS), ("insert", INSERT_VARIANTS),
                          ("entry", ENTRY_VARIANTS)):
            value = observation()
            del value[op][names[-1]]
            with self.subTest(op=op, change="missing"), self.assertRaises(ValueError):
                validate_observation(self.request, value, OPS)
            value = observation()
            value[op]["invented"] = copy.deepcopy(value[op][names[0]])
            with self.subTest(op=op, change="extra"), self.assertRaises(ValueError):
                validate_observation(self.request, value, OPS)

    def test_a_stream_is_a_count_and_a_digest_or_a_native_panic(self):
        ok = {"rows": 4, "failures": 1, "sha256": SHA}
        validate_stream("nav", {"rows": 0, "failures": 0, "sha256": SHA})
        validate_stream("nav", {**ok, "detail": "T|0|-\n"})
        validate_stream("nav", {"panic": "native assertion"})
        for value in ({**ok, "rows": -1}, {**ok, "rows": True}, {**ok, "rows": "1"}, {**ok, "failures": 5},
                      {**ok, "failures": -1}, {**ok, "failures": None}, {**ok, "sha256": "short"},
                      {"rows": 1, "failures": 0}, {"sha256": SHA, "failures": 0}, {"rows": 1, "sha256": SHA},
                      {**ok, "text_sha256": SHA}, {**ok, "invented": 1},
                      {"panic": 7}, {"panic": "x", "rows": 1}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_stream("nav", value)

    def test_a_format_stream_records_the_applied_text_or_the_native_failure_to_apply_never_both(self):
        ok = {"rows": 2, "failures": 0, "sha256": SHA}
        validate_stream("format", {**ok, "text_sha256": SHA})
        validate_stream("format", {**ok, "text_panic": "slice bounds out of range"})
        for value in (ok, {**ok, "text_sha256": SHA, "text_panic": "both"}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_stream("format", value)

    def test_every_stream_of_the_selected_operations_is_walked_once(self):
        self.assertEqual(len(list(walk_streams(observation(), OPS))),
                         3 + len(INDENT_VARIANTS) + 2 * len(FORMAT_VARIANTS) + len(INSERT_VARIANTS)
                         + len(ENTRY_VARIANTS))
        self.assertEqual(len(list(walk_streams(observation(("nav",)), ("nav",)))), 1)
        failed = {**observation(("nav",)), "parse": {"panic": "parser"}}
        self.assertEqual(len(list(walk_streams(failed, ("nav",)))), 2)

    def test_the_frozen_form_drops_what_a_second_run_cannot_reproduce(self):
        summary = {"version": 1, "pin": "p", "diagnostic_subset": False,
                   "native": {"requests": 2, "rows": 9, "seconds": 245.6, "stream_sha256": SHA}}
        frozen = frozen_form(summary)
        self.assertEqual(frozen, {"version": 1, "pin": "p",
                                  "native": {"requests": 2, "rows": 9, "stream_sha256": SHA}})
        self.assertEqual(frozen, frozen_form({**summary, "native": {**summary["native"], "seconds": 1.0}}))
        self.assertIn("seconds", summary["native"], "the run summary keeps its timing")

    def test_differences_name_the_operation_and_variant_that_disagree(self):
        native = observation()
        self.assertEqual(differences(native, copy.deepcopy(native), OPS), [])
        changed = copy.deepcopy(native)
        changed["nav"]["sha256"] = "1" * 64
        changed["format"]["terse"]["rows"] += 1
        changed["indent"]["tabs"]["failures"] = 1
        del changed["insert"]["default"]
        changed["parse"]["node_count"] += 1
        self.assertEqual(sorted(differences(native, changed, OPS)),
                         sorted(["parse", "nav", "indent/tabs", "format/terse", "insert/default"]))
        # Only the requested operations are compared, and a missing one differs.
        self.assertEqual(differences(native, changed, ("position",)), ["parse"])
        del changed["position"]
        self.assertIn("position", differences(native, changed, ("position",)))

    def test_a_global_operation_is_asked_once_and_is_not_a_per_input_operation(self):
        from s09_format import SINGLE, global_request
        self.assertEqual(GLOBAL_OPS, ("rulesmap",))
        self.assertFalse(set(GLOBAL_OPS) & set(OPS))
        self.assertTrue(set(GLOBAL_OPS) <= set(SINGLE), "a global answer is one stream")
        carrier = global_request([{"id": "first", "source_hex": "00", "version": 1}, {"id": "second"}])
        self.assertEqual(carrier, {"id": "global", "source_hex": "00", "version": 1,
                                   "ops": ["rulesmap"], "detail": False})


if __name__ == "__main__":
    unittest.main()
