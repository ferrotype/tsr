"""Counterexamples for S05 framing, independent observations and frozen claims."""

import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s05
import s05_cases as cases
import s05_protocol as protocol
import s05_tables as tables
from s04_common import strict_json_loads


def token(kind, text=b"", diagnostics=(), action=0, ordinal=0):
    return {"event": "observation", "id": "test", "action": action, "ordinal": ordinal,
            "status": "ok", "diagnostics": list(diagnostics),
            "value": {"kind": kind, "token": kind, "full_start": 0, "start": 0, "end": len(text),
                      "text_hex": text.hex(), "value_hex": "", "flags": 0, "range": [0, len(text)],
                      "predicates": [False] * 7, "directives": []}}


def diagnostic(code=1490, args=()):
    return {"code": code, "category": 1, "key": "known_key", "start": 0, "length": 1, "args": list(args)}


def validated(item, observations, runtime="oracle"):
    state = protocol.StreamCase(item["request"], runtime)
    state.begin({"event": "begin", "id": "test", "version": 1})
    for observation in observations:
        state.observation(observation)
        yield observation
    state.end({"event": "end", "id": "test", "observations": len(observations), "completed_actions": state.action})


class RequestTests(unittest.TestCase):
    def test_unknown_missing_extra_fields_and_wrong_types_fail(self):
        valid = cases.case("test")["request"]
        for key, bad in (("version", True), ("source_hex", "FF"), ("source_hex", "0"),
                         ("decode_source", 0), ("target", 13), ("variant", True), ("id", ""),
                         ("skip_trivia", None), ("actions", [])):
            with self.subTest(key=key, value=bad):
                request = copy.deepcopy(valid); request[key] = bad
                with self.assertRaises(ValueError):
                    cases.validate_request(request)
        for key in valid:
            request = copy.deepcopy(valid); del request[key]
            with self.assertRaises(ValueError):
                cases.validate_request(request)
        request = copy.deepcopy(valid); request["extra"] = None
        with self.assertRaises(ValueError):
            cases.validate_request(request)

    def test_action_arguments_and_lifo_are_protocol_contracts(self):
        with self.assertRaises(ValueError):
            cases.case("test", actions=[])
        bad_actions = [cases.action("scan", flag=True), cases.action("missing"),
                       cases.action("reset_pos", pos=True), cases.action("reset_pos", pos=2**63),
                       cases.action("identifier_point", point=2**31),
                       cases.action("identifier_block", first=0x10ffff, count=2),
                       cases.action("rescan_slash", report_errors=True),
                       cases.action("rescan_slash", report_errors="maybe"),
                       cases.action("observe", getter="secret"), cases.action("rewind"),
                       cases.action("mark"), cases.action("number_format", bits="00")]
        for action in bad_actions:
            with self.subTest(action=action), self.assertRaises(ValueError):
                cases.case("test", actions=[action])
        cases.case("test", actions=[cases.action("mark"), cases.action("mark"), cases.action("commit"), cases.action("rewind")])

    def test_json_duplicate_fields_and_nonfinite_numbers_fail(self):
        for raw in ('{"id":"a","id":"b"}', '{"actions":[{"op":"scan","op":"reset"}]}',
                    '{"x":NaN}', '{"x":Infinity}', '{} {}'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                strict_json_loads(raw)


class StreamTests(unittest.TestCase):
    def test_valid_differing_token_counts_are_measured_failure_and_both_drain(self):
        item = cases.case("test", b"ab")
        left = [token(80, b"ab"), token(1, ordinal=1)]
        right = [token(80, b"a"), token(80, b"b", ordinal=1), token(1, ordinal=2)]
        result = protocol.compare_case(item, validated(item, left), validated(item, right, "rust"))
        self.assertFalse(result["pass"])
        self.assertFalse(result["values"])
        self.assertEqual(result["observations"], [2, 3])

    def test_missing_eof_and_reordered_actions_fail_capture(self):
        item = cases.case("test", b"a")
        for observations in ([token(80, b"a")], [token(1, ordinal=1)], [token(1, action=1)]):
            with self.subTest(observations=observations), self.assertRaises(ValueError):
                list(validated(item, observations))
        item = cases.case("test", actions=[cases.action("scan"), cases.action("scan")])
        with self.assertRaises(ValueError):
            list(validated(item, [token(1)]))

    def test_token_bound_and_extra_records_fail_capture(self):
        item = cases.case("test")
        with self.assertRaisesRegex(ValueError, "bound"):
            list(validated(item, [token(80), token(80, ordinal=1), token(1, ordinal=2)]))
        with self.assertRaises(ValueError):
            list(validated(item, [token(1), token(1, ordinal=1)]))

    def test_mutations_change_diagnostic_and_value_metrics_independently(self):
        item = cases.case("test", actions=[cases.action("scan")])
        left = token(80, b"x", [diagnostic(args=[{"kind": "string", "hex": "ff"}])])
        changed = copy.deepcopy(left); changed["diagnostics"][0]["args"][0]["hex"] = "fe"
        result = protocol.compare_case(item, iter([left]), iter([changed]))
        self.assertFalse(result["pass"]); self.assertFalse(result["diagnostics"]); self.assertTrue(result["values"])
        changed = copy.deepcopy(left); changed["value"]["value_hex"] = "ff"
        result = protocol.compare_case(item, iter([left]), iter([changed]))
        self.assertFalse(result["pass"]); self.assertTrue(result["diagnostics"]); self.assertFalse(result["values"])

    def test_both_disabled_callbacks_cannot_pass_required_witness(self):
        item = cases.case("test", actions=[cases.action("scan")]); item["witness"] = {"action": 0, "codes": [1490]}
        result = protocol.compare_case(item, iter([token(1)]), iter([token(1)]))
        self.assertFalse(result["pass"]); self.assertFalse(result["diagnostics"])
        self.assertEqual(result["failure"]["codes"], [[], []])

    def test_decoded_input_drives_panic_identity_and_eof_bound(self):
        item = cases.case("test", b"\xff\xfe" + "0b2n".encode("utf-16-le"), [cases.action("pseudo_bigint")], decode_source=True)
        item["expected_panic"] = {"action": 0, "class": "invalid_bigint", "input_hex": "306232"}
        left = {"event": "observation", "id": "test", "action": 0, "ordinal": 0, "status": "panic",
                "value": {"message": 'Failed to parse big int: "0b2"', "class": "invalid_bigint", "input_hex": "306232"}, "diagnostics": []}
        right = copy.deepcopy(left); right["value"]["message"] = "Failed to parse big int (hex): 306232"
        self.assertTrue(protocol.compare_case(item, validated(item, [left]), validated(item, [right], "rust"))["pass"])
        request = cases.case("test", b"\xff\xfea\x00", decode_source=True)["request"]
        state = protocol.StreamCase(request, "oracle")
        self.assertEqual(state.source_hex, "61")
        self.assertEqual(state.source_bound, 3)

    def test_bom_bookkeeping_preserves_raw_bytes_and_truncates_odd_utf16(self):
        for raw, expected in ((b"\xff", b"\xff"), (b"\xef\xbb\xbf\xff", b"\xff"),
                              (b"\xff\xfea\x00Z", b"a"), (b"\xfe\xff\x00aZ", b"a"),
                              (b"\xff\xfe\x00\xd8", "�".encode()),
                              (b"\xff\xfe\x3e\xd8\x80\xdd", "🦀".encode())):
            with self.subTest(raw=raw):
                self.assertEqual(protocol.decoded_request_source(raw), expected)

    def test_wrong_output_types_and_bitset_padding_are_rejected(self):
        examples = [(cases.action("identifier_point", point=1), [True, False, 1]),
                    (cases.action("identifier_block", first=0, count=1), {"start_hex": "01", "part_hex": "81", "jsx_hex": "00"}),
                    (cases.action("number_format", bits="0000000000000000"), {"number_class": "finite", "bits": "7ff0000000000000", "text_hex": ""}),
                    (cases.action("number_format", bits="7ff8000000000000"), {"number_class": "nan", "bits": "7ff8000000000000", "text_hex": ""}),
                    (cases.action("scan"), {**token(1)["value"], "kind": True})]
        for action, value in examples:
            with self.subTest(action=action), self.assertRaises(ValueError):
                protocol.validate_value(action, value)

    def test_only_frozen_expected_panic_can_pass(self):
        item = cases.case("test", b"0xzn", [cases.action("pseudo_bigint")])
        item["expected_panic"] = {"action": 0, "class": "invalid_bigint", "input_hex": "30787a"}
        def panic(message):
            return {"event": "observation", "id": "test", "action": 0, "ordinal": 0, "status": "panic",
                    "value": {"message": message, "class": "invalid_bigint", "input_hex": "30787a"}, "diagnostics": []}
        left = panic('Failed to parse big int: "0xz"'); right = panic('Failed to parse big int (hex): 30787a')
        self.assertTrue(protocol.compare_case(item, validated(item, [left]), validated(item, [right], "rust"))["pass"])
        item["expected_panic"] = None
        self.assertFalse(protocol.compare_case(item, validated(item, [left]), validated(item, [right], "rust"))["pass"])
        right["value"]["message"] = "attempt to add with overflow"
        with self.assertRaisesRegex(ValueError, "classification"):
            list(validated(item, [right], "rust"))

    def test_frozen_bounds_use_independently_classified_runtime_payloads(self):
        item = cases.case("test", b"abc", [cases.action("reset_pos", pos=4), cases.action("observe", getter="token_text")])
        item["expected_panic"] = {"action": 1, "class": "bounds"}
        reset = {"event": "observation", "id": "test", "action": 0, "ordinal": 0, "status": "ok", "value": None, "diagnostics": []}
        def bound(message):
            return {"event": "observation", "id": "test", "action": 1, "ordinal": 1, "status": "panic",
                    "value": {"message": message, "class": "bounds", "input_hex": None}, "diagnostics": []}
        go = bound("runtime error: slice bounds out of range [:4] with length 3")
        rust = bound("range start index 4 out of range for slice of length 3")
        self.assertTrue(protocol.compare_case(item, validated(item, [reset, go]), validated(item, [reset, rust], "rust"))["pass"])
        for runtime, wrong in (("oracle", "runtime error: slice bounds out of range [:4] with length 3 extra"),
                               ("oracle", "runtime error: integer overflow"),
                               ("rust", "range start index 4 out of range for slice of length 3 extra"),
                               ("rust", "range start index x out of range for slice of length 3"),
                               ("rust", "assertion failed: bounds"),
                               ("rust", "attempt to add with overflow")):
            with self.subTest(runtime=runtime, wrong=wrong), self.assertRaisesRegex(ValueError, "classification"):
                list(validated(item, [reset, bound(wrong)], runtime))
        wrong = bound("assertion failed: bounds"); wrong["value"]["class"] = "unexpected"
        self.assertFalse(protocol.compare_case(item, validated(item, [reset, go]), validated(item, [reset, wrong], "rust"))["pass"])
        item["expected_panic"] = None
        self.assertFalse(protocol.compare_case(item, validated(item, [reset, go]), validated(item, [reset, rust], "rust"))["pass"])

    def test_unknown_panic_is_measured_failure_even_when_identical(self):
        item = cases.case("test", actions=[cases.action("scan")])
        record = {"event": "observation", "id": "test", "action": 0, "ordinal": 0, "status": "panic",
                  "value": {"message": "unexpected invariant", "class": "unexpected", "input_hex": None}, "diagnostics": []}
        result = protocol.compare_case(item, validated(item, [record]), validated(item, [record], "rust"))
        self.assertFalse(result["pass"]); self.assertFalse(result["diagnostics"]); self.assertFalse(result["values"])


class PanicPayloadTests(unittest.TestCase):
    def test_go_quote_decodes_raw_bytes_unicode_and_controls(self):
        self.assertEqual(protocol.decode_go_quoted(r'"\xff\000\a\b\f\n\r\t\v\\\"\u0130\U0001f980"'),
                         b'\xff\x00\a\b\f\n\r\t\v\\"' + "İ🦀".encode())
        self.assertEqual(protocol.decode_go_quoted('"K"'), "K".encode())

    def test_wrong_quoted_bigint_input_is_not_a_recognized_panic(self):
        action = cases.action("pseudo_bigint")
        self.assertEqual(protocol.classify_panic(action, 'Failed to parse big int: "wrong"', "oracle", "30787a6e"), ("unexpected", None))
        self.assertEqual(protocol.classify_panic(action, r'Failed to parse big int: "0x\xff"', "oracle", "3078ff6e"), ("invalid_bigint", "3078ff"))
        self.assertEqual(protocol.classify_panic(action, 'Failed to parse big int (hex): 30787a6e', "rust", "30787a6e"), ("unexpected", None))

    def test_malformed_go_quotes_are_rejected(self):
        for quoted in ('unquoted', '"unclosed', r'"\x0"', r'"\Xff"', r'"\400"', r'"\1"', r'"\uD800"', r'"\U00110000"', '"a"b"', '"a\nb"', r'"\q"'):
            with self.subTest(quoted=quoted), self.assertRaises(ValueError):
                protocol.decode_go_quoted(quoted)


class ProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)

    def process(self, source, **kwargs):
        process = protocol.Process([sys.executable, "-u", "-c", source], self.path / "stderr", **kwargs)
        self.addCleanup(process.close)
        return process

    def test_partial_byte_trickle_cannot_extend_record_deadline(self):
        process = self.process("import os,time\nfor i in range(100):\n os.write(1,b'x'); time.sleep(.02)", deadline=.15)
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            process.read()
        self.assertLess(time.monotonic() - started, 1)

    def test_partial_input_consumption_cannot_extend_send_deadline(self):
        process = self.process("import os,time\nwhile True:\n os.read(0,1024); time.sleep(.01)", deadline=.15)
        request = cases.case("test", b"a" * 1024 * 1024)["request"]
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            process.send(request)
        self.assertLess(time.monotonic() - started, 1)

    def test_stderr_cannot_fill_a_pipe_and_block_stdout(self):
        process = self.process("import os\nos.write(2,b'e'*2000000)\nos.write(1,b'{}\\n')")
        self.assertEqual(process.read(), {})
        process.finish()
        self.assertEqual((self.path / "stderr").stat().st_size, 2000000)

    def test_truncation_oversize_invalid_utf8_duplicates_and_extra_records(self):
        for raw in (b'{"x":1}', b'x'*100+b'\n', b'{"x":"\xff"}\n', b'{"x":1,"x":2}\n'):
            with self.subTest(raw=raw):
                process = self.process(f"import os\nos.write(1,{raw!r})", max_record=64)
                with self.assertRaises(ValueError):
                    process.read()
                process.close()
        process = self.process("import os\nos.write(1,b'{}\\n{}\\n')")
        self.assertEqual(process.read(), {})
        with self.assertRaises(ValueError):
            process.finish()

    def test_producer_lock_prevents_concurrent_shared_path_writes(self):
        with patch.object(s05, "ROOT", self.path), s05.producer_lock():
            with self.assertRaisesRegex(RuntimeError, "another S05 producer"):
                with s05.producer_lock():
                    self.fail("second capture acquired shared output paths")
        with patch.object(s05, "ROOT", self.path), s05.producer_lock():
            pass


class TableTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.valid = strict_json_loads((Path(__file__).resolve().parents[2] / "data/s05/tables.json").read_bytes())

    def test_authoritative_table_inventory_is_valid(self):
        tables.validate_tables(self.valid)

    def test_ranges_fold_cycles_and_property_domains_fail_closed(self):
        mutations = [lambda value: value["identifier"]["start"].append(value["identifier"]["start"][0]),
                     lambda value: value["identifier"]["start"][0].__setitem__(2, 0),
                     lambda value: value["simple_fold"].pop(),
                     lambda value: value["scanner"]["non_binary"].__setitem__("sc", "missing"),
                     lambda value: value["scanner"]["binary"].append(value["scanner"]["binary"][0]),
                     lambda value: value["scanner"]["keywords"].__setitem__("let", True),
                     lambda value: value["scanner"]["values"].pop("Script_Extensions")]
        for mutation in mutations:
            value = copy.deepcopy(self.valid); mutation(value)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                tables.validate_tables(value)

    def test_formatter_uses_workspace_edition(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); (root / "Cargo.toml").write_text('[workspace.package]\nedition = "2024"\n')
            with patch.object(tables, "ROOT", root), patch.object(tables, "command", return_value=b"rendered") as command:
                self.assertEqual(tables.render(self.valid, "0"*40), b"rendered")
                self.assertEqual(command.call_args.args[0], ["rustfmt", "--edition", "2024"])

    def test_keyword_emitter_preserves_utf8_and_escaped_bytes_in_rust(self):
        exported = copy.deepcopy(self.valid)
        fixtures = [('quote"', 41), ("\\", 42), ("tab\n\0\b\f", 43), ("é😀", 44)]
        for text, kind in fixtures:
            exported["scanner"]["keywords"][text] = kind
            exported["scanner"]["tokens"][text] = kind
        generated = tables.render(exported, "0" * 40)
        assertions = []
        for text, kind in fixtures:
            byte_array = "&[" + ", ".join(str(byte) for byte in text.encode("utf-8")) + "]"
            assertions.append(f"assert_eq!(keyword_kind({byte_array}), Some({kind}));")
            assertions.append(f"assert!(KEYWORDS.iter().any(|(text, kind)| text.as_bytes() == {byte_array} && *kind == {kind}));")
        assertions.append("assert_eq!(keyword_kind(&[255]), None);")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, executable = root / "keywords.rs", root / "keywords"
            source.write_bytes(b"#![allow(dead_code)]\n" + generated +
                               ("fn main() {\n" + "\n".join(assertions) + "\n}\n").encode())
            tables.command(["rustc", "--edition", "2021", str(source), "-o", str(executable)], cwd=tables.ROOT)
            tables.command([str(executable)], cwd=root)


if __name__ == "__main__":
    unittest.main()
