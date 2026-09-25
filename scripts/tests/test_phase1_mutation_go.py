"""Phase 1 mutation witnesses, Go side: digests, coverage decoding, stage rules and frozen bindings."""
import ast
import contextlib
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1_mutation_go as go  # noqa: E402
from s06_protocol import canonical  # noqa: E402
from s06_results import compare_case  # noqa: E402

META_HASH = bytes(range(16))
PARSER = "github.com/microsoft/TypeScript/tsc/internal/parser"
AST = "github.com/microsoft/TypeScript/tsc/internal/ast"


def uleb(value):
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def counter_file(entries, *, meta_hash=META_HASH, flavor=2, segments=1, strings=b"abc", arguments=b"xy"):
    """A counter-data file laid out as internal/coverage/defs.go describes."""
    data = bytearray(b"\x00cwm" + (1).to_bytes(4, "little") + meta_hash + bytes([flavor, 0]) + bytes(6))
    data += len(entries).to_bytes(8, "little") + len(strings).to_bytes(4, "little") + len(arguments).to_bytes(4, "little")
    data += strings + arguments
    while len(data) % 4:
        data.append(0)
    for package, function, counters in entries:
        data += uleb(len(counters)) + uleb(package) + uleb(function)
        for value in counters:
            data += uleb(value)
    data += b"\x00cwm" + bytes(4) + segments.to_bytes(4, "little") + bytes(4)
    return bytes(data)


DUMP = f"""/* header */
Cover mode: atomic

Package path: {AST}
Package name: ast
Module path: github.com/microsoft/TypeScript/tsc

Func: Node.Kind
Srcfile: {AST}/ast.go
Literal: false
0: L10:C2 -- L11:C3 NS=1 = 0

Func: func.L20.C5
Srcfile: {AST}/ast.go
Literal: true
0: L20:C9 -- L21:C3 NS=1 = 0

Package path: {PARSER}
Package name: parser
Module path: github.com/microsoft/TypeScript/tsc

Func: *Parser.parseThing
Srcfile: {PARSER}/parser.go
Literal: false
0: L31:C2 -- L32:C3 NS=2 = 0
1: L36:C3 -- L37:C4 NS=1 = 0
2: L33:C4 -- L34:C5 NS=1 = 0

Func: S07IsMissingList
Srcfile: {PARSER}/s07_bridge.go
Literal: false
0: L6:C2 -- L6:C60 NS=1 = 0
"""

INVENTORY = {
    "tsc/internal/ast/ast.go": [(9, 12, "tsc/internal/ast/ast.go:Node.Kind"), (19, 22, "tsc/internal/ast/ast.go:init#1")],
    "tsc/internal/parser/parser.go": [(30, 40, "tsc/internal/parser/parser.go:Parser.parseThing")],
}
KIND = "tsc/internal/ast/ast.go:Node.Kind"
THING = "tsc/internal/parser/parser.go:Parser.parseThing"


def mapped_dump():
    packages = go.parse_dump(DUMP)
    problems = go.attach_ids(packages, INVENTORY)
    return packages, problems


def segment_literals(oracle):
    """Segment names a build opens: the patches (process oracles) or the driver source (batch)."""
    spec = go.ORACLES[oracle]
    if spec.kind == "process":
        text = "".join(patch["new"] for patch in go.load_patches()["oracles"][oracle])
        names = set(re.findall(r'phase1CoverBegin\("([^"]+)"\)', text))
        if "phase1CoverBegin(name)" in text:
            names |= set(spec.operations)
        if "phase1CoverBegin(stage)" in text:
            names |= {stage for stage in spec.operations if stage.endswith("_graph")}
        return names
    text = (ROOT / spec.source_dir / "main.go").read_text()
    return set(re.findall(r'phase1CoverBegin\("([^"]+)"\)', text)) | set(re.findall(r'stage\(req\.ID, "([^"]+)"', text))


class CoverageDecodingTests(unittest.TestCase):
    def test_uleb_round_trips_multibyte_values(self):
        for value in (0, 1, 127, 128, 300, 2**31 - 1, 2**40):
            self.assertEqual(go.uleb(uleb(value) + b"\xff", 0), (value, len(uleb(value))))

    def test_counter_file_parses_every_entry_after_aligned_preamble(self):
        data = counter_file([(0, 0, [5]), (1, 0, [1, 0, 300])])
        self.assertEqual(list(go.parse_counters(data, META_HASH)), [(0, 0, [5]), (1, 0, [1, 0, 300])])

    def test_counter_file_refuses_foreign_meta_flavor_and_segments(self):
        with self.assertRaisesRegex(ValueError, "different meta-data"):
            list(go.parse_counters(counter_file([]), bytes(16)))
        with self.assertRaisesRegex(ValueError, "ULEB128"):
            list(go.parse_counters(counter_file([], flavor=1), META_HASH))
        with self.assertRaisesRegex(ValueError, "one counter segment"):
            list(go.parse_counters(counter_file([], segments=2), META_HASH))
        with self.assertRaisesRegex(ValueError, "not a coverage"):
            list(go.parse_counters(b"\x00cvm" + counter_file([])[4:], META_HASH))

    def test_dump_keeps_meta_order_units_and_counts(self):
        packages = go.parse_dump(DUMP)
        self.assertEqual([path for path, _ in packages], [AST, PARSER])
        thing = packages[1][1][0]
        self.assertEqual(thing["file"], "tsc/internal/parser/parser.go")
        self.assertEqual(thing["units"][1], (36, 3, 37, 4, 1))
        self.assertEqual(thing["counts"], [0, 0, 0])
        self.assertTrue(packages[0][1][1]["literal"])

    def test_mapping_is_by_line_containment_not_by_name(self):
        packages, problems = mapped_dump()
        self.assertEqual(problems, [])
        functions = {function["name"]: function for _, items in packages for function in items}
        # cmd/cover names the pointer receiver '*Parser'; the id has none.
        self.assertEqual(functions["*Parser.parseThing"]["id"], THING)
        self.assertEqual(functions["Node.Kind"]["id"], KIND)
        self.assertIsNone(functions["func.L20.C5"]["id"], "a package literal is not a FuncDecl operation")
        self.assertIsNone(functions["S07IsMissingList"]["id"], "oracle bridges are outside the inventory")

    def test_ambiguous_or_missing_home_is_a_problem(self):
        inventory = {**INVENTORY, "tsc/internal/ast/ast.go": [(9, 12, "a"), (8, 13, "b")]}
        problems = go.attach_ids(go.parse_dump(DUMP), inventory)
        self.assertEqual([problem["hits"] for problem in problems], [["a", "b"]])
        inventory = {**INVENTORY, "tsc/internal/parser/parser.go": [(33, 40, "late")]}
        self.assertEqual(go.attach_ids(go.parse_dump(DUMP), inventory)[0]["hits"], [])

    def test_entry_block_is_the_first_positioned_unit(self):
        packages, _ = mapped_dump()
        # Only a later block (a closure folded into the parent) counted: not entered.
        folded = counter_file([(1, 0, [0, 0, 4])])
        self.assertEqual(go.entered(packages, folded, META_HASH, "production"), set())
        ran = counter_file([(1, 0, [1, 0, 0])])
        self.assertEqual(go.entered(packages, ran, META_HASH, "production"), {THING})

    def test_observation_segments_count_only_the_named_prefixes(self):
        packages, _ = mapped_dump()
        both = counter_file([(0, 0, [1]), (1, 0, [1, 1, 1]), (1, 1, [7])])
        self.assertEqual(go.entered(packages, both, META_HASH, "production"), {KIND, THING})
        self.assertEqual(go.entered(packages, both, META_HASH, "observe", ("tsc/internal/parser/",)), {THING})
        # The binder, syntax and facts observers count nothing, whatever ran.
        self.assertEqual(go.entered(packages, both, META_HASH, "observe", ()), set())
        with self.assertRaisesRegex(ValueError, "segment rule"):
            go.entered(packages, both, META_HASH, "observer", ())

    def test_counter_unit_length_mismatch_refuses(self):
        packages, _ = mapped_dump()
        with self.assertRaisesRegex(ValueError, "length mismatch"):
            go.entered(packages, counter_file([(1, 0, [1])]), META_HASH, "production")

    def test_stream_records_round_trip_the_go_hook_layout(self):
        def record(row, segment, payload):
            return (len(row).to_bytes(4, "little") + row.encode() + len(segment).to_bytes(2, "little")
                    + segment.encode() + len(payload).to_bytes(4, "little") + payload)
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "stream"
            path.write_bytes(record("a/b#1", "parse", b"xyz") + record("a/b#1", "parse:observe", b""))
            self.assertEqual(list(go.read_stream(path)), [("a/b#1", "parse", b"xyz"), ("a/b#1", "parse:observe", b"")])
            path.write_bytes(record("r", "parse", b"xyz")[:-1])
            with self.assertRaisesRegex(ValueError, "truncated"):
                list(go.read_stream(path))

    def test_decode_shard_applies_each_oracles_observe_rule(self):
        packages_dump = DUMP
        both = counter_file([(0, 0, [1]), (1, 0, [1, 1, 1])])

        def stream(segment):
            return (len("r").to_bytes(4, "little") + b"r" + len(segment).to_bytes(2, "little") + segment.encode()
                    + len(both).to_bytes(4, "little") + both)
        with tempfile.TemporaryDirectory() as scratch, patch.object(go, "load_inventory", return_value=INVENTORY):
            path = Path(scratch) / "s"
            for oracle, segment, expected in (("e1", "node_index_before", {THING}), ("binder", "bound_graph", set()),
                                              ("binder", "bind", {KIND, THING}), ("syntax", "render", set()),
                                              ("facts", "facts:walk", set()), ("facts", "subtree_facts", {KIND, THING})):
                path.write_bytes(stream(segment))
                rows, segments = go._decode_shard((oracle, str(path), packages_dump, META_HASH))
                self.assertEqual(rows, {"r": expected}, (oracle, segment))
                self.assertEqual(segments, {segment: [1, {op: 1 for op in expected}]})
            path.write_bytes(stream("unknown"))
            with self.assertRaisesRegex(ValueError, "unknown coverage segment"):
                go._decode_shard(("binder", str(path), packages_dump, META_HASH))


class StageRuleTests(unittest.TestCase):
    def test_every_segment_a_build_opens_has_a_rule_and_nothing_else_does(self):
        for oracle, spec in go.ORACLES.items():
            self.assertEqual(set(spec.segments.values()) - {"production", "observe"}, set(), oracle)
            self.assertEqual(segment_literals(oracle), set(spec.segments), oracle)

    def test_per_oracle_production_and_observation_rules(self):
        production = {oracle: {segment for segment, rule in spec.segments.items() if rule == "production"}
                      for oracle, spec in go.ORACLES.items()}
        self.assertEqual(production, {"e1": {"parse"}, "binder": {"parse", "bind", "repeat_bind"},
                                      "syntax": {"load", "syntactic"}, "facts": {"parse", "subtree_facts"}})
        observe = {oracle: spec.observe_counts for oracle, spec in go.ORACLES.items()}
        self.assertEqual(observe, {"e1": ("tsc/internal/parser/",), "binder": (), "syntax": (), "facts": ()})

    def test_stage_rule_digest_moves_with_the_rule(self):
        for oracle in go.ORACLES:
            self.assertEqual(go.stage_rule_digest(oracle), go.sha256(canonical(go.stage_rule(oracle))))
            self.assertEqual(go.stage_rule(oracle)["segments"], dict(sorted(go.ORACLES[oracle].segments.items())))
        self.assertEqual(len({go.stage_rule_digest(oracle) for oracle in go.ORACLES}), len(go.ORACLES))
        changed = dict(go.ORACLES)
        changed["binder"] = go.Oracle(**{**go.ORACLES["binder"].__dict__, "observe_counts": ("tsc/internal/parser/",)})
        before = go.stage_rule_digest("binder")
        with patch.object(go, "ORACLES", changed):
            self.assertNotEqual(go.stage_rule_digest("binder"), before)

    def test_unstable_operations_are_the_pool_and_every_run_dependent_one(self):
        first = {"a": [0, 1, 2], "b": [3], go.POOL_OPERATIONS[0]: [4]}
        second = {"a": [0, 1, 2], "b": [3, 5], go.POOL_OPERATIONS[0]: [4]}
        detail = go.unstable_operations(first, second)
        self.assertNotIn("a", detail)
        self.assertEqual(detail["b"], {"reason": "run_dependent", "rows": [1, 2], "rows_differing": 1})
        self.assertEqual(detail[go.POOL_OPERATIONS[0]]["reason"], "pool", "a pool operation is unstable even when runs agree")
        self.assertTrue(set(go.POOL_OPERATIONS) <= set(detail))
        self.assertEqual(detail["b"], go.unstable_operations(second, first)["b"] | {"rows": [1, 2]})

    def reach_document(self, **changes):
        document = {"version": go.VERSION, "oracle": "binder", "pin": go.pin(), "native_sha256": "n",
                    "row_encoding": go.ROW_ENCODING, "stage_rule": go.stage_rule("binder"),
                    "stage_rule_sha256": go.stage_rule_digest("binder"),
                    "instrumentation_sha256": go.instrumentation_digest("binder"),
                    "go_functions_sha256": go.file_sha256(ROOT / "data/go-functions.tsv"),
                    "unstable_ops": sorted(go.POOL_OPERATIONS), "op_row_gaps": {"x": [0]}}
        document.update(changes)
        return document

    def test_reach_problems_catch_stale_rules_and_indexed_unstable_operations(self):
        self.assertEqual(go.reach_problems("binder", self.reach_document(), "n"), [])
        stale = {**go.stage_rule("binder"), "observe": {"counts": "x", "prefixes": ["tsc/internal/parser/"]}}
        cases = {
            "stage rule": self.reach_document(stage_rule=stale),
            "stage rule digest": self.reach_document(stage_rule_sha256="0" * 64),
            "instrumentation": self.reach_document(instrumentation_sha256="0" * 64),
            "pool": self.reach_document(unstable_ops=sorted(go.POOL_OPERATIONS)[1:]),
            "indexed": self.reach_document(op_row_gaps={go.POOL_OPERATIONS[0]: [0]}),
            "native": self.reach_document(native_sha256="other"),
        }
        for name, document in cases.items():
            self.assertNotEqual(go.reach_problems("binder", document, "n"), [], name)


# The decoder digest of each DECODER_VERSION. A change to any decoder
# definition fails test_decoder_version_is_bumped_with_any_decoder_change until
# DECODER_VERSION is bumped and its digest recorded here; every reach file
# decoded by the old code then stops binding (stage_rule changes).
DECODER_DIGESTS = {2: "995507c32f653c2c34a852a9b2bf9d2e08d766ea6d3b48c55a90b7a80e54824a"}


def definition_lines(text):
    """{module-level name: (first line, last line)} of every function and single-name assignment."""
    spans = {}
    for node in ast.parse(text).body:
        if isinstance(node, ast.FunctionDef):
            spans[node.name] = (node.lineno, node.end_lineno)
        elif isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            spans[node.targets[0].id] = (node.lineno, node.end_lineno)
    return spans


def edit_line(text, line, suffix="  # edited"):
    lines = text.splitlines(keepends=True)
    lines[line - 1] = lines[line - 1].rstrip("\n") + suffix + "\n"
    return "".join(lines)


class DecoderBindingTests(unittest.TestCase):
    def test_decoder_version_is_bumped_with_any_decoder_change(self):
        self.assertEqual(go.decoder_digest(), DECODER_DIGESTS.get(go.DECODER_VERSION),
                         "the decoder source changed: bump DECODER_VERSION and record its digest in DECODER_DIGESTS")

    def test_stage_rule_binds_the_decoder_version_and_source(self):
        for oracle in go.ORACLES:
            self.assertEqual(go.stage_rule(oracle)["decoder"],
                             {"version": go.DECODER_VERSION,
                              "definitions": list(go.DECODER_CONSTANTS + go.DECODER_FUNCTIONS),
                              "source_sha256": go.decoder_digest()}, oracle)
        before = {oracle: go.stage_rule_digest(oracle) for oracle in go.ORACLES}
        with patch.object(go, "DECODER_VERSION", go.DECODER_VERSION + 1):
            for oracle in go.ORACLES:
                self.assertNotEqual(go.stage_rule_digest(oracle), before[oracle], oracle)
        with patch.object(go, "decoder_digest", return_value="0" * 64):
            for oracle in go.ORACLES:
                self.assertNotEqual(go.stage_rule_digest(oracle), before[oracle], oracle)

    def test_decoder_digest_moves_with_each_decoder_definition_and_nothing_else(self):
        text = Path(go.__file__).read_text(encoding="utf-8")
        base = go.decoder_digest(text)
        self.assertEqual(base, go.decoder_digest())
        spans = definition_lines(text)
        for name in go.DECODER_CONSTANTS + go.DECODER_FUNCTIONS:
            first, last = spans[name]
            for line in {first, last}:
                self.assertNotEqual(go.decoder_digest(edit_line(text, line)), base, f"{name} line {line}")
        # A semantic change inside the entry rule of `entered`.
        entry_test = 'counters[item["entry"]] == 0'
        self.assertEqual(text.count(entry_test), 1)
        self.assertNotEqual(go.decoder_digest(text.replace(entry_test, 'counters[item["entry"]] < 0')), base)
        # Edits to definitions outside the decoder, or between definitions, do not move it.
        for name in ("reach_document", "reach", "verify_reach", "covdata_check", "stage_rule", "native"):
            self.assertEqual(go.decoder_digest(edit_line(text, spans[name][1])), base, name)
        self.assertEqual(go.decoder_digest(text + "\n# trailing comment\n"), base)

    def test_decoder_source_refuses_missing_or_repeated_definitions(self):
        text = Path(go.__file__).read_text(encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "missing"):
            go.decoder_source(text.replace("def merge_shards(", "def merged_shards(", 1))
        with self.assertRaisesRegex(ValueError, "missing"):
            go.decoder_source(text.replace("\nUNIT = ", "\nUNITS = ", 1))
        with self.assertRaisesRegex(ValueError, "twice"):
            go.decoder_source(text + "\n\ndef uleb(data, at):\n    return 0, at\n")

    def test_stage_rule_binds_the_run_environment(self):
        self.assertEqual(go.stage_rule("syntax")["environment"], {"PHASE1_SYNTAX_MEMO": "0"})
        self.assertEqual(go.stage_rule("e1")["environment"], {})
        before = go.stage_rule_digest("syntax")
        with patch.object(go, "reach_environment", return_value={}):
            self.assertNotEqual(go.stage_rule_digest("syntax"), before)


def stream_bytes(records):
    """A PHASE1_COVER_STREAM file: (row, segment, counter entries) per snapshot."""
    data = b""
    for row, segment, entries in records:
        payload = counter_file(entries)
        data += (len(row).to_bytes(4, "little") + row.encode() + len(segment).to_bytes(2, "little")
                 + segment.encode() + len(payload).to_bytes(4, "little") + payload)
    return data


# Counter entries of the DUMP fixture: Node.Kind (package 0, function 0) and
# Parser.parseThing (package 1, function 0) with its entry block counted.
ENTER = {KIND: (0, 0, [1]), THING: (1, 0, [1, 0, 0])}
ROWS = [(f"r{index}", f"d{index}", {"id": f"r{index}"}) for index in range(4)]
PLANNED = frozenset({KIND, THING, "tsc/internal/parser/parser.go:notEntered"})


class ReachDocumentTests(unittest.TestCase):
    """The reach file holds only what a rerun reproduces (the GR1 idempotence)."""

    def decode(self, shards, *, covdata=()):
        """One decoded run from per-shard snapshot lists, through _decode_shard and merge_shards."""
        decoded = []
        with tempfile.TemporaryDirectory() as scratch, patch.object(go, "load_inventory", return_value=INVENTORY):
            for number, records in enumerate(shards):
                path = Path(scratch) / f"shard-{number}.cover"
                path.write_bytes(stream_bytes([(row, segment, [ENTER[op] for op in ops])
                                               for row, segment, ops in records]))
                decoded.append(go._decode_shard(("binder", str(path), DUMP, META_HASH)))
        row_ops, segments = go.merge_shards(decoded)
        return {"row_ops": row_ops, "segments": segments, "instrumented": ["internal/ast", "internal/parser"],
                "processes": len(shards), "rows_checked": len(ROWS),
                "covdata": [{"row": row, "segment": segment, "live_functions": live, "ids": set(ids)}
                            for row, segment, live, ids in covdata]}

    def document(self, runs, planned=PLANNED):
        with patch.object(go, "POOL_OPERATIONS", (KIND,)):
            return go.reach_document("binder", ROWS, runs, native_document={"requests_sha256": "q"},
                                     native_sha256="n", go_version="go1.27.1", binary_sha256="b", planned=planned)

    def file_bytes(self, document):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "reach.json.gz"
            go.write_ops_document(path, document)
            return path.read_bytes()

    def attempt(self, pool_first, pool_second, live):
        """Two runs (2 and 1 processes) where only the pool operation KIND and covdata's live count vary."""
        def with_pool(records, pool_rows):
            return [(row, segment, ops | ({KIND} if (row, segment) in pool_rows else set()))
                    for row, segment, ops in records]
        first = [[("r0", "parse", {THING}), ("r0", "bound_graph", {THING}), ("r2", "parse", {THING})],
                 [("r1", "parse", set()), ("r3", "bind", {THING})]]
        second = [[("r0", "parse", {THING}), ("r0", "bound_graph", {THING}), ("r1", "parse", set()),
                   ("r2", "parse", {THING}), ("r3", "bind", {THING})]]
        runs = [self.decode([with_pool(shard, pool_first) for shard in first],
                            covdata=[("r0", "parse", live, {THING} | ({KIND} if ("r0", "parse") in pool_first else set()))]),
                self.decode([with_pool(shard, pool_second) for shard in second])]
        return self.document(runs)

    def test_pool_and_scheduling_noise_leaves_the_file_byte_identical(self):
        one, one_stats = self.attempt({("r0", "parse"), ("r1", "parse")}, {("r2", "parse")}, 7)
        two, two_stats = self.attempt({("r2", "parse"), ("r3", "bind"), ("r0", "bound_graph")},
                                      {("r0", "parse"), ("r1", "parse"), ("r3", "bind")}, 9)
        self.assertEqual(self.file_bytes(one), self.file_bytes(two))
        self.assertEqual(self.file_bytes(one), self.file_bytes(self.attempt(
            {("r0", "parse"), ("r1", "parse")}, {("r2", "parse")}, 7)[0]), "same inputs, same bytes")
        # The noise is real and kept, but only in the unbound statistics.
        self.assertNotEqual(one_stats["unstable_detail"], two_stats["unstable_detail"])
        self.assertNotEqual(one_stats["runs"][0]["segments"], two_stats["runs"][0]["segments"])
        self.assertNotEqual(one_stats["covdata_check"], two_stats["covdata_check"])
        self.assertEqual(one_stats["unstable_detail"][KIND], {"reason": "pool", "rows": [2, 1], "rows_differing": 3})

    def test_every_count_in_the_file_is_over_stable_operations(self):
        document, statistics = self.attempt({("r0", "parse"), ("r1", "parse")}, {("r2", "parse")}, 7)
        self.assertEqual(document["op_row_gaps"], {THING: [0, 2, 1]})
        self.assertEqual(document["unstable_ops"], [KIND])
        self.assertEqual(document["unstable_detail"], {KIND: {"reason": "pool"}})
        # The observe segment counts nothing; parse enters THING on r0 and r2, bind on r3.
        self.assertEqual(document["segments"], {"bind": {"snapshots": 1, "stable_entered": 1},
                                                "bound_graph": {"snapshots": 1, "stable_entered": 0},
                                                "parse": {"snapshots": 3, "stable_entered": 2}})
        self.assertEqual(statistics["runs"][0]["segments"]["parse"], {"snapshots": 3, "entered": 4, "stable_entered": 2})
        self.assertEqual(document["covdata_check"], [{"row": "r0", "segment": "parse", "stable_entered": 1}])
        self.assertEqual(document["runs"], [{"processes": 2, "rows_checked": 4, "equal_to_native": True},
                                            {"processes": 1, "rows_checked": 4, "equal_to_native": True}])
        self.assertTrue(statistics["stable_segments_equal_between_runs"])

    def test_summary_is_over_the_plan_homes_not_the_coverage_report(self):
        document, _ = self.attempt({("r0", "parse")}, set(), 7)
        self.assertEqual(document["summary"], {
            "operations_entered": 1, "unstable_operations": 1, "index_entries": 3,
            "planned": {"source": f"homes of {go.MANIFEST}", "operations": 3,
                        "operations_sha256": go.sha256(canonical(sorted(PLANNED))), "entered": 1, "unstable": [KIND]}})
        self.assertNotIn(b"coverage_report", self.file_bytes(document))
        with patch.object(go, "POOL_OPERATIONS", (KIND,)):
            other = go.reach_summary({THING: [0]}, [KIND], frozenset({THING}))
        self.assertEqual(other["planned"]["entered"], 1)
        self.assertEqual(other["planned"]["unstable"], [])

    def test_run_dependent_operation_is_unstable_with_its_reason_only(self):
        rest = [("r2", "parse", set()), ("r3", "parse", set())]
        first = self.decode([[("r0", "parse", {THING}), ("r1", "parse", {THING}), *rest]])
        second = self.decode([[("r0", "parse", {THING}), ("r1", "parse", set()), *rest]])
        document, statistics = self.document([first, second])
        self.assertEqual(document["op_row_gaps"], {})
        self.assertEqual(document["unstable_detail"], {KIND: {"reason": "pool"}, THING: {"reason": "run_dependent"}})
        self.assertEqual(statistics["unstable_detail"][THING], {"reason": "run_dependent", "rows": [2, 1],
                                                                  "rows_differing": 1})
        with self.assertRaisesRegex(ValueError, "two instrumented runs"):
            self.document([first])

    def test_merge_refuses_a_row_decoded_by_two_processes(self):
        with self.assertRaisesRegex(ValueError, "two processes"):
            self.decode([[("r0", "parse", {THING})], [("r0", "bind", {THING})]])

    def test_planned_operations_are_the_manifest_homes(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "manifest.json"
            path.write_text(json.dumps({"homes": {THING: [], KIND: []}, "mutants": []}))
            self.assertEqual(go.planned_operations(path), frozenset({THING, KIND}))
            for broken in ({"mutants": []}, {"homes": {}}, {"homes": [THING]}, []):
                path.write_text(json.dumps(broken))
                with self.assertRaisesRegex(ValueError, "no plan homes"):
                    go.planned_operations(path)
            with self.assertRaisesRegex(ValueError, "unreadable"):
                go.planned_operations(Path(scratch) / "absent.json")


LIVE_DUMP = f"""Package path: {AST}
Func: Node.Kind
Srcfile: {AST}/ast.go
Literal: false
0: L10:C2 -- L11:C3 NS=1 = 1

Package path: {PARSER}
Func: *Parser.parseThing
Srcfile: {PARSER}/parser.go
Literal: false
0: L31:C2 -- L32:C3 NS=2 = 1
1: L36:C3 -- L37:C4 NS=1 = 0
2: L33:C4 -- L34:C5 NS=1 = 0
"""


class CovdataCheckTests(unittest.TestCase):
    def check(self, segment, live=LIVE_DUMP, oracle="binder"):
        packages, _ = mapped_dump()
        with tempfile.TemporaryDirectory() as scratch:
            meta = Path(scratch) / "meta"
            meta.mkdir()
            (meta / f"covmeta.{META_HASH.hex()}").write_bytes(b"\x00cvm" + bytes(20) + META_HASH + bytes(8))
            stream = Path(scratch) / "shard-0.cover"
            stream.write_bytes(stream_bytes([("r0", segment, [ENTER[KIND], ENTER[THING]])]))
            with patch.object(go, "TARGET", Path(scratch)), patch.object(go, "debugdump", return_value=live):
                return go.covdata_check(meta, stream, {}, {0}, packages, go.ORACLES[oracle])

    def test_samples_carry_the_operations_their_segment_rule_enters(self):
        self.assertEqual(self.check("bind"), [{"row": "r0", "segment": "bind", "live_functions": 2, "ids": {KIND, THING}}])
        self.assertEqual(self.check("bound_graph")[0]["ids"], set(), "a binder observe segment enters nothing")
        self.assertEqual(self.check("node_index_before", oracle="e1")[0]["ids"], {THING})

    def test_unknown_segment_or_disagreement_with_covdata_refuses(self):
        with self.assertRaisesRegex(ValueError, "unknown coverage segment"):
            self.check("render")
        with self.assertRaisesRegex(ValueError, "disagrees with covdata"):
            self.check("bind", live=LIVE_DUMP.split("\nPackage path: " + PARSER)[0])


class VerifyReachTests(unittest.TestCase):
    DOCUMENT = {"version": 1, "oracle": "facts", "runs": [{"processes": 8}, {"processes": 5}],
                "covdata_check": [{"row": "r0", "segment": "parse", "stable_entered": 1}] * 3,
                "segments": {"parse": {"snapshots": 4, "stable_entered": 2}},
                "op_row_gaps": {THING: [0, 2], KIND: [1]}}

    def verify(self, fresh):
        calls = []
        with tempfile.TemporaryDirectory() as scratch:
            committed = Path(scratch) / "go-reach-facts.json.gz"
            go.write_ops_document(committed, self.DOCUMENT)

            def fake_reach(oracle, **arguments):
                calls.append((oracle, arguments))
                go.write_ops_document(arguments["out"], fresh)
                return {"file_sha256": go.file_sha256(arguments["out"]), "stats": str(arguments["stats"]), "seconds": 0}
            with patch.object(go, "TARGET", Path(scratch)), patch.object(go, "reach_path", return_value=committed), \
                    patch.object(go, "native_path", return_value=committed), patch.object(go, "reach", fake_reach):
                report = go.verify_reach("facts")
                left = sorted(path.name for path in Path(scratch).iterdir())
        return report, calls, left

    def test_identical_rerun_passes_with_the_recorded_process_counts(self):
        report, calls, left = self.verify(self.DOCUMENT)
        self.assertTrue(report["identical"])
        self.assertEqual(report["committed_sha256"], report["fresh_sha256"])
        self.assertEqual(report["processes"], [8, 5])
        self.assertEqual([(oracle, arguments["jobs"], arguments["second_jobs"], arguments["covdata_samples"])
                          for oracle, arguments in calls], [("facts", 8, 5, 3)])
        self.assertEqual(left, ["go-reach-facts.json.gz"], "an identical fresh file is removed")

    def test_any_difference_fails_and_names_where(self):
        for changes, expected in (
                ({"segments": {"parse": {"snapshots": 4, "stable_entered": 3}}}, {"header": ["segments"]}),
                ({"op_row_gaps": {THING: [0, 3], KIND: [1]}}, {"rows_differ": [THING]}),
                ({"op_row_gaps": {THING: [0, 2]}}, {"only_committed": [KIND]})):
            with self.subTest(changes=changes):
                report, _, left = self.verify({**self.DOCUMENT, **changes})
                self.assertFalse(report["identical"])
                self.assertNotEqual(report["committed_sha256"], report["fresh_sha256"])
                for key, value in expected.items():
                    self.assertEqual(report["differences"][key], value)
                self.assertIn("verify-go-reach-facts.json.gz", left, "a differing fresh file is kept")

    def test_refuses_a_file_without_two_recorded_runs(self):
        with tempfile.TemporaryDirectory() as scratch:
            committed = Path(scratch) / "go-reach-facts.json.gz"
            go.write_ops_document(committed, {**self.DOCUMENT, "runs": [{"processes": 8}]})
            with patch.object(go, "reach_path", return_value=committed):
                with self.assertRaisesRegex(ValueError, "two runs"):
                    go.verify_reach("facts")

    def test_command_exits_nonzero_unless_every_oracle_is_identical(self):
        results = {"e1": True, "facts": False}

        def fake(oracle, **_):
            return {"oracle": oracle, "identical": results[oracle]}
        with patch.object(go, "verify_reach", fake), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(go.main(["verify-reach", "--oracle", "e1"]), 0)
            self.assertEqual(go.main(["verify-reach", "--oracle", "facts", "--oracle", "e1"]), 1)
            self.assertEqual(go.main(["verify-reach", "--oracle", "e1", "--oracle", "facts"]), 1)


def s06_stream(request_id, stages):
    frames, seq = [{"tag": "begin", "id": request_id}], 0
    for stage, observations, outcome in stages:
        for kind, value in observations:
            frames.append({"tag": "observation", "seq": seq, "stage": stage, "kind": kind, "value": value})
            seq += 1
        frames.append({"tag": "stage", "stage": stage, "outcome": outcome, "message_hex": "" if outcome == "ok" else "6f6f7073"})
    frames.append({"tag": "end", "observations": seq, "stages": len(stages)})
    return frames


class DigestTests(unittest.TestCase):
    def test_e1_digest_is_the_evidence_comparator_sha256(self):
        request = {"id": "x/unit/0/config/0", "op": "parse"}
        stream = s06_stream(request["id"], [
            ("parse", [("source_file", {"kind": 308, "pos": 0, "end": 3}), ("diagnostic", {"collection": "parse", "index": 0})], "ok"),
            ("node_index_before", [("identity", {"cache_reused": True}), ("table", {"length": 1})], "ok"),
            ("encode_source_file", [("bytes", {"length": 2}), ("chunk", {"offset": 0, "hex": "abcd"})], "ok"),
            ("node_index_after", [("table", {"length": 1})], "ok"),
        ])
        row = go.e1_row(stream)
        compared = compare_case(request, stream, stream, lambda *args: None, {"oracle": set(), "rust": set()})
        for stage in go.ORACLES["e1"].compared:
            self.assertEqual(row["digests"][stage], compared["stages"][stage]["sha256"][0])
        self.assertEqual(set(row["outcomes"].values()), {"ok"})
        self.assertNotIn("messages", row)

    def test_e1_digest_ignores_framing_and_localizes_a_count_change(self):
        base = s06_stream("r", [("parse", [("diagnostic", {"i": 0})], "ok"), ("node_index_before", [("table", {"length": 1})], "ok"),
                                ("encode_source_file", [], "ok"), ("node_index_after", [], "ok")])
        more = s06_stream("r", [("parse", [("diagnostic", {"i": 0}), ("diagnostic", {"i": 1})], "ok"),
                                ("node_index_before", [("table", {"length": 1})], "ok"),
                                ("encode_source_file", [], "ok"), ("node_index_after", [], "ok")])
        left, right = go.e1_row(base), go.e1_row(more)
        # The later stage's seq moved; its digest did not.
        self.assertEqual(left["digests"]["node_index_before"], right["digests"]["node_index_before"])
        self.assertNotEqual(left["digests"]["parse"], right["digests"]["parse"])

    def test_failed_stage_records_message_and_later_stages_not_run(self):
        stream = s06_stream("r", [("parse", [], "ok"), ("node_index_before", [], "panic")])
        row = go.e1_row(stream)
        self.assertEqual(row["outcomes"], {"parse": "ok", "node_index_before": "panic",
                                           "encode_source_file": "not_run", "node_index_after": "not_run"})
        self.assertEqual(row["messages"], {"node_index_before": "6f6f7073"})
        self.assertEqual(set(row["digests"]), {"parse", "node_index_before"})

    def test_out_of_order_stages_refuse(self):
        stream = s06_stream("r", [("node_index_before", [], "ok"), ("parse", [], "ok")])
        with self.assertRaisesRegex(ValueError, "protocol order"):
            go.e1_row(stream)

    def binder_stream(self, records, *, fragment=None):
        stages = []
        for stage in go.ORACLES["binder"].operations:
            observations = records if stage.endswith("_graph") else []
            stages.append((stage, observations, "ok"))
        frames = s06_stream("b", stages)
        if fragment is not None:
            expanded = []
            for frame in frames:
                if frame["tag"] == "observation" and frame["kind"] == "node" and frame["stage"] == "bound_graph":
                    raw = json.dumps(frame["value"]).encode()
                    parts = [raw[index:index + fragment] for index in range(0, len(raw), fragment)]
                    for part, chunk in enumerate(parts):
                        expanded.append({**frame, "kind": "fragment", "value": {
                            "record_kind": "node", "part": part, "parts": len(parts), "payload_hex": chunk.hex()}})
                else:
                    expanded.append(frame)
            frames = expanded
        return frames

    def test_binder_fragments_reassemble_to_the_same_digest(self):
        records = [("source", {"root": 1}), ("node", {"id": 1, "payload": "x" * 50}), ("counts", {"node": 1})]
        whole = go.binder_row(self.binder_stream(records))
        split = go.binder_row(self.binder_stream(records, fragment=16))
        self.assertEqual(whole, split)
        self.assertEqual(set(whole["digests"]), {"parsed_graph", "bound_graph", "repeated_graph"})
        expected = hashlib.sha256(b"".join(canonical([kind, value]) + b"\n" for kind, value in records)).hexdigest()
        self.assertEqual(whole["digests"]["parsed_graph"], expected)

    def test_binder_digest_applies_only_the_comparable_normalization(self):
        def graph(raw):
            return [("symbol", {"name": {"raw_hex": raw, "identity": {"kind": "node", "ref": 3, "prefix_hex": "", "suffix_hex": ""}}})]
        self.assertEqual(go.binder_row(self.binder_stream(graph("aa")))["digests"],
                         go.binder_row(self.binder_stream(graph("bb")))["digests"])
        plain = [("symbol", {"name": {"raw_hex": "aa", "identity": None}})]
        other = [("symbol", {"name": {"raw_hex": "bb", "identity": None}})]
        self.assertNotEqual(go.binder_row(self.binder_stream(plain))["digests"],
                            go.binder_row(self.binder_stream(other))["digests"])

    def test_interleaved_fragment_refuses(self):
        frames = self.binder_stream([("node", {"id": 1})])
        frames.insert(1, {"tag": "observation", "stage": "parsed_graph", "kind": "fragment",
                          "value": {"record_kind": "node", "part": 1, "parts": 2, "payload_hex": "00"}})
        with self.assertRaisesRegex(ValueError, "fragment"):
            go.binder_row(frames)

    SYNTAX_VALUES = {"files": 2, "file_names_sha256": "ab" * 32, "plain_hex": "4142", "pretty_hex": "",
                     "syntactic": [{"File": "/a.ts", "Pos": 1, "End": 2, "Code": 1005, "Category": 1, "Key": "k",
                                    "Args": None, "Text": "té", "Chain": None, "Related": None}]}

    def test_syntax_compared_fields_are_the_schedule_comparators(self):
        import phase1_syntax
        self.assertEqual(go.SYNTAX_COMPARED, phase1_syntax.COMPARED)
        self.assertEqual(go.ORACLES["syntax"].compared, phase1_syntax.COMPARED)

    def test_syntax_digests_agree_for_driver_native_and_rust_rows(self):
        values = self.SYNTAX_VALUES
        driver = go.syntax_row({"row": "r", "outcome": "ok", "values": values, "micros": 3})
        native = go.syntax_row({"id": "r", **values})
        rust = go.syntax_row({"id": "r", "state": "observed", **values})
        self.assertEqual(driver, native)
        self.assertEqual(driver, rust)
        self.assertEqual(driver["outcomes"], {"program": "ok"})
        self.assertEqual(driver["digests"]["syntactic"], go.sha256(canonical(values["syntactic"])))
        self.assertEqual(driver["digests"]["files"], go.sha256(b"2"))
        changed = go.syntax_row({"id": "r", "state": "observed", **values, "pretty_hex": "00"})
        self.assertEqual({field for field in driver["digests"] if driver["digests"][field] != changed["digests"][field]},
                         {"pretty_hex"})

    def test_syntax_failed_rows_carry_their_state_and_no_digest(self):
        self.assertEqual(go.syntax_row({"id": "r", "state": "not_implemented", "operation": "x"}),
                         {"outcomes": {"program": "not_implemented"}, "digests": {}, "messages": {"program": b"x".hex()}})
        self.assertEqual(go.syntax_row({"row": "r", "outcome": "panic", "message": "boom"})["outcomes"], {"program": "panic"})
        with self.assertRaisesRegex(ValueError, "compared field"):
            go.syntax_row({"row": "r", "outcome": "ok", "values": {"files": 1}})

    def test_facts_digest_is_the_driver_byte_format(self):
        pairs = [[307, 5244929], [79, 4194304], [1, 0]]
        # tools/phase1/mutation/go/facts writes '[' + '[k,f]' joined by ',' + ']'.
        driver_bytes = ("[" + ",".join(f"[{kind},{facts}]" for kind, facts in pairs) + "]").encode()
        self.assertEqual(canonical(pairs), driver_bytes)
        self.assertEqual(go.facts_digest(pairs), hashlib.sha256(driver_bytes).hexdigest())

    def test_facts_row_recomputes_raw_lists_and_refuses_disagreement(self):
        pairs = [[307, 1], [79, 4194304]]
        ok = {"row": "r", "outcomes": {"parse": "ok", "subtree_facts": "ok"}, "digest": go.facts_digest(pairs), "nodes": 2}
        self.assertEqual(go.facts_row(ok), {"outcomes": {"parse": "ok", "subtree_facts": "ok"},
                                            "digests": {"subtree_facts": go.facts_digest(pairs)}})
        self.assertEqual(go.facts_row({**ok, "list": pairs}), go.facts_row(ok))
        self.assertEqual(go.facts_row({"row": "r", "outcomes": ok["outcomes"], "list": pairs}), go.facts_row(ok))
        with self.assertRaisesRegex(ValueError, "own node list"):
            go.facts_row({**ok, "list": pairs[:1]})
        parse_panic = {"row": "r", "outcomes": {"parse": "panic", "subtree_facts": "not_run"}, "messages": {"parse": "00"}}
        self.assertEqual(go.facts_row(parse_panic)["digests"], {})
        with self.assertRaisesRegex(ValueError, "digest exactly when"):
            go.facts_row({**parse_panic, "digest": "x"})
        with self.assertRaisesRegex(ValueError, "digest exactly when"):
            go.facts_row({"row": "r", "outcomes": ok["outcomes"]})
        with self.assertRaisesRegex(ValueError, "malformed outcomes"):
            go.facts_row({"row": "r", "outcomes": {"parse": "ok"}, "digest": "x"})

    def test_row_digests_dispatches_every_oracle(self):
        self.assertEqual(set(go.ORACLES), {"e1", "binder", "syntax", "facts"})
        self.assertEqual(go.row_digests("syntax", {"id": "r", **self.SYNTAX_VALUES})["outcomes"], {"program": "ok"})


class RequestAndDocumentTests(unittest.TestCase):
    def inventory(self, requests):
        recipes = [{"id": request["id"], "request_sha256": go.sha256(canonical(request))} for request in requests]
        return recipes, {"request_sha256": go.sha256(canonical(requests)), "parser_expansion_sha256": "e"}

    def test_request_verification_refuses_row_and_sequence_drift(self):
        requests = [{"id": "a", "x": 1}, {"id": "b", "x": 2}]
        with patch.object(go, "committed_inventory", return_value=self.inventory(requests)):
            self.assertEqual(len(go.verify_requests("e1", requests)), 2)
            self.assertEqual(len(go.verify_requests("facts", requests)), 2)
            with self.assertRaisesRegex(ValueError, "differ from committed request_sha256"):
                go.verify_requests("e1", [requests[0], {"id": "b", "x": 3}])
            with self.assertRaisesRegex(ValueError, "requests, committed"):
                go.verify_requests("e1", requests[:1])
        recipes, probes = self.inventory(requests)
        with patch.object(go, "committed_inventory", return_value=(recipes, {**probes, "request_sha256": "0"})):
            with self.assertRaisesRegex(ValueError, "sequence"):
                go.verify_requests("e1", requests)

    def syntax_probe(self, identity, paths):
        return {"guard": True, "id": identity, "load": True, "path": f"tsc/testdata/tests/cases/compiler/{identity.split('#')[0].split('/')[-1]}",
                "request": {"case_sensitive": True, "cwd": "/.src", "files": {}, "id": identity,
                            "options": {"paths": paths}, "roots": [], "skip_module_resolution": False, "symlinks": {}}}

    def test_syntax_requests_keep_ordered_option_maps_and_refuse_drift(self):
        from s07_subset import json_bytes
        probe = self.syntax_probe("compiler/a.ts#configuration=0", {"z/*": ["1"], "a/*": ["2"]})
        row = {"id": probe["id"], "primary": "compiler/a.ts", "loading_request_sha256": go.sha256(json_bytes(probe["request"]))}
        with patch.object(go, "syntax_inventory", return_value=[row]):
            self.assertEqual(go.verify_requests("syntax", [probe]), [row["loading_request_sha256"]])
            line = go.request_line("syntax", probe, row["loading_request_sha256"])
            self.assertLess(line.index(b"z/*"), line.index(b"a/*"), "paths keeps its source order")
            read = go.strict_json_loads(line)
            read.pop("request_sha256")
            self.assertEqual(go.sha256(json_bytes(read["request"])), row["loading_request_sha256"])
            for broken in ({**probe, "load": False}, {**probe, "guard": False},
                           {**probe, "path": "tsc/testdata/tests/cases/compiler/b.ts"},
                           {**probe, "request": {**probe["request"], "options": {"paths": {"a/*": ["2"], "z/*": ["1"]}}}},
                           {**probe, "extra": 1}):
                with self.assertRaisesRegex(ValueError, "syntax"):
                    go.verify_requests("syntax", [broken])
            with self.assertRaisesRegex(ValueError, "schedule loads"):
                go.verify_requests("syntax", [])

    def test_gap_encoding_round_trips_and_refuses_unsorted(self):
        for indices in ([], [0], [3, 4, 9, 22342], list(range(100))):
            self.assertEqual(go.decode_gaps(go.encode_gaps(indices)), indices)
        with self.assertRaises(ValueError):
            go.encode_gaps([3, 3])
        with self.assertRaises(ValueError):
            go.decode_gaps([2, 0])

    def test_row_documents_are_deterministic_gzip(self):
        document = {"version": 1, "oracle": "e1", "rows": [{"row": "a", "digests": {"parse": "00"}}, {"row": "b"}]}
        with tempfile.TemporaryDirectory() as scratch:
            first, second = Path(scratch) / "1.json.gz", Path(scratch) / "2.json.gz"
            go.write_rows_document(first, document)
            go.write_rows_document(second, document)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            self.assertEqual(go.read_document(first), document)
            self.assertEqual(gzip.decompress(first.read_bytes()).count(b"\n"), 4)

    def test_splice_refuses_missing_or_repeated_anchor(self):
        patch_ = {"old": "anchor\n", "new": "anchor\nhook()\n"}
        self.assertEqual(go.splice("a\nanchor\nb", patch_, "f"), "a\nanchor\nhook()\nb")
        with self.assertRaisesRegex(ValueError, "0 times"):
            go.splice("nothing", patch_, "f")
        with self.assertRaisesRegex(ValueError, "2 times"):
            go.splice("anchor\nanchor\n", patch_, "f")

    def test_instrumented_sources_splice_each_process_oracle_once_and_add_the_hook(self):
        for oracle in ("e1", "binder"):
            spec = go.ORACLES[oracle]
            files = go.instrumented_sources(oracle)
            package = f"tsc/{spec.package}"
            self.assertIn(f"{package}/phase1_cover.go", files)
            stage = files[f"{package}/protocol.go"].decode()
            self.assertIn("phase1CoverBegin(name)\n\toutcome, message := capture(action)\n\tphase1CoverEnd(s.id)", stage)
            for source, target in spec.bridges:
                self.assertEqual(files[f"tsc/{target}"], (ROOT / spec.source_dir / source).read_bytes())
            self.assertIn("-coverpkg=github.com/microsoft/TypeScript/tsc/" + spec.package + ",", go.cover_flags(oracle)[-1])
            self.assertEqual(go.instrumentation_digest(oracle), go.instrumentation_digest(oracle))
        self.assertIn('phase1CoverBegin("parse:observe")', go.instrumented_sources("e1")["tsc/internal/s06oracle/behavior.go"].decode())
        self.assertIn("defer phase1CoverEnd(s.id)", go.instrumented_sources("binder")["tsc/internal/s07binder/graph_stream.go"].decode())

    def test_driver_builds_carry_the_hook_only_when_instrumented(self):
        for oracle in ("syntax", "facts"):
            package = f"tsc/{go.ORACLES[oracle].package}"
            plain, cover = go.driver_sources(oracle, cover=False), go.instrumented_sources(oracle)
            self.assertEqual(set(plain), {f"{package}/main.go", f"{package}/phase1_cover_off.go"})
            self.assertEqual(set(cover), {f"{package}/main.go", f"{package}/phase1_cover.go"})
            self.assertNotIn(b"runtime/coverage", plain[f"{package}/phase1_cover_off.go"])
            self.assertIn("tools/phase1/mutation/go/phase1_cover_off.go", go.oracle_sources(oracle))
        self.assertEqual(go.reach_environment("syntax"), {"PHASE1_SYNTAX_MEMO": "0"})
        self.assertEqual(go.reach_environment("facts"), {})
        self.assertIn(go.SYNTAX_NATIVE, go.oracle_sources("syntax"))
        self.assertIn(go.SYNTAX_PROBE, go.oracle_sources("syntax"))

    def test_syntax_driver_copies_the_probe_load_path_verbatim(self):
        probe = (ROOT / go.SYNTAX_PROBE).read_text()
        driver = (ROOT / "tools/phase1/mutation/go/syntax/main.go").read_text()
        statements = [
            "fs := bundled.WrapFS(vfstest.FromMap(files, req.Request.CaseSensitive))",
            "host := &phase1MemoHost{compiler.NewCompilerHost(req.Request.Cwd, fs, bundled.LibPath(), nil, nil, nil), memo}",
            "options := req.Request.Options",
            "config := tsoptions.NewParsedCommandLine(&options, req.Request.Roots, nil, compare)",
            "program := compiler.NewProgram(compiler.ProgramOptions{Host: host, Config: config, "
            "SkipModuleResolution: req.Request.SkipModuleResolution, SingleThreaded: core.TSTrue})",
            "diagnostics := program.GetSyntacticDiagnostics(ctx, nil)",
            "wrapped := diagnosticwriter.ToDiagnostics(diagnosticwriter.WrapASTDiagnostics(diagnostics))",
            'format := &diagnosticwriter.FormattingOptions{NewLine: "\\r\\n", ComparePathsOptions: compare}',
            "diagnosticwriter.WriteFormatDiagnostics(&plain, wrapped, format)",
            "diagnosticwriter.FormatDiagnosticsWithColorAndContext(&pretty, wrapped, format)",
            'hash := sha256.Sum256([]byte(strings.Join(names, "\\n")))',
        ]
        for statement in statements:
            self.assertIn(statement, probe)
            self.assertIn(statement, driver)
        for block in ("func (h *phase1MemoHost) GetSourceFile", "func phase1SyntaxDiag(d *ast.Diagnostic)",
                      "type phase1SyntaxDiagnostic struct"):
            start = probe.index(block)
            body = probe[start:probe.index("\n}\n", start) + 3]
            self.assertIn(body, driver, block)


@unittest.skipUnless(all(go.native_path(oracle).exists() and go.reach_path(oracle).exists() for oracle in go.ORACLES),
                     "frozen mutation artifacts are not present")
class FrozenArtifactTests(unittest.TestCase):
    """Child-free: the committed files still bind the current inputs."""

    EVERY_ROW = {"e1": "tsc/internal/parser/parser.go:ParseSourceFile",
                 "binder": "tsc/internal/binder/binder.go:BindSourceFile",
                 "syntax": "tsc/internal/compiler/program.go:NewProgram",
                 "facts": "tsc/internal/ast/ast.go:Node.SubtreeFacts"}

    def test_no_subprocess_is_started(self):
        with patch("subprocess.run", side_effect=AssertionError("child")), patch("subprocess.Popen", side_effect=AssertionError("child")):
            for oracle in go.ORACLES:
                go.load_native(oracle)
                go.load_reach(oracle)

    def test_native_files_bind_inventory_sources_and_fresh_run(self):
        for oracle, spec in go.ORACLES.items():
            document = go.load_native(oracle)
            self.assertEqual(document["oracle_sources"], go.oracle_sources(oracle), f"{oracle} oracle sources changed")
            self.assertEqual(document["fresh_run"]["equal"], True)
            self.assertEqual(document["digest_rule"], spec.digest_rule)
            self.assertEqual(len(document["rows"]), len(go.inventory_rows(oracle)))
            for row in document["rows"]:
                self.assertEqual(set(row["outcomes"]), set(spec.operations))
                if set(row["outcomes"].values()) == {"ok"}:
                    self.assertEqual(sorted(row["digests"]), sorted(spec.compared))

    def test_syntax_native_is_the_committed_native_values(self):
        document = go.load_native("syntax")
        rows = [(row["row"], row["request_sha256"], None) for row in document["rows"]]
        frozen, _ = go.syntax_native_rows(rows)
        self.assertEqual(frozen, document["rows"])
        self.assertTrue(document["probe_check"]["equal"])
        self.assertEqual(document["values_source"]["file_sha256"], go.file_sha256(ROOT / go.SYNTAX_NATIVE))

    def test_facts_native_recorded_its_digest_and_parse_checks(self):
        document = go.load_native("facts")
        self.assertTrue(document["digest_check"]["equal"])
        self.assertTrue(document["parse_outcomes_equal_e1"])

    def test_reach_files_bind_native_rules_instrumentation_and_every_row(self):
        for oracle in go.ORACLES:
            document = go.read_document(go.reach_path(oracle))
            native = go.load_native(oracle)
            self.assertEqual(go.reach_problems(oracle, document, go.file_sha256(go.native_path(oracle))), [], oracle)
            self.assertEqual(document["stage_rule"], go.stage_rule(oracle))
            self.assertEqual(document["stage_rule_sha256"], go.stage_rule_digest(oracle))
            self.assertEqual(document["requests_sha256"], native["requests_sha256"])
            self.assertEqual(document["rows_checked"], len(native["rows"]))
            self.assertEqual([run["equal_to_native"] for run in document["runs"]], [True, True])
            self.assertEqual(len({run["processes"] for run in document["runs"]}), 2, "two different shardings")
            self.assertGreaterEqual(len(document["covdata_check"]), 1)
            self.assertEqual(document["instrumentation"]["environment"], go.reach_environment(oracle))
            reach = go.load_reach(oracle)
            self.assertEqual(len(reach), document["summary"]["operations_entered"])
            self.assertFalse(set(reach) & set(document["unstable_ops"]))
            self.assertLessEqual(set(go.POOL_OPERATIONS), set(document["unstable_ops"]))
            self.assertEqual(len(reach[self.EVERY_ROW[oracle]]), len(native["rows"]), f"{oracle}: every row enters it")

    def test_reach_files_hold_only_reproducible_fields(self):
        # GR1: nothing in the header may vary with pool or scheduling state, and
        # the summary is over the plan's homes set, never the coverage report.
        planned = go.planned_operations()
        for oracle in go.ORACLES:
            document = go.read_document(go.reach_path(oracle))
            index = {op: go.decode_gaps(gaps) for op, gaps in document["op_row_gaps"].items()}
            self.assertEqual(document["summary"], go.reach_summary(index, document["unstable_ops"], planned), oracle)
            self.assertEqual(document["unstable_detail"],
                             {op: {"reason": "pool" if op in go.POOL_OPERATIONS else "run_dependent"}
                              for op in document["unstable_ops"]}, oracle)
            self.assertEqual([set(run) for run in document["runs"]],
                             [{"processes", "rows_checked", "equal_to_native"}] * 2, oracle)
            self.assertEqual({key for item in document["covdata_check"] for key in item},
                             {"row", "segment", "stable_entered"}, oracle)
            self.assertEqual({key for item in document["segments"].values() for key in item},
                             {"snapshots", "stable_entered"}, oracle)
            header = go.canonical({key: value for key, value in document.items() if key != "op_row_gaps"})
            self.assertNotIn(b"coverage_report", header, oracle)
            self.assertNotIn(b"witness_missing", header, oracle)

    def test_observation_segments_counted_only_what_the_rule_allows(self):
        for oracle, spec in go.ORACLES.items():
            segments = go.read_document(go.reach_path(oracle))["segments"]
            self.assertEqual(set(segments), set(spec.segments))
            for segment, rule in spec.segments.items():
                if rule == "observe" and not spec.observe_counts:
                    self.assertEqual(segments[segment]["stable_entered"], 0, f"{oracle} {segment}")
        # The binder's graph observers reach the parser through a bridge; that is not Go reach.
        binder, e1 = go.load_reach("binder"), go.load_reach("e1")
        missing = "tsc/internal/parser/parser.go:isMissingNodeList"
        self.assertEqual(binder.get(missing, frozenset()), e1.get(missing, frozenset()))


if __name__ == "__main__":
    unittest.main()
