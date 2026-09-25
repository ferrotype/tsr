"""Phase 1 operation tables: specs, registries, selection, materialization (docs section 9).

Child-free except the Go driver test at the end, which builds the table driver
from a git-archive export of the pin (needs go on PATH) and checks that its
canonical encoding and the Python digest rule agree on every synthetic row.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1_mutation_go as go  # noqa: E402
import phase1_tables as tables  # noqa: E402
from s06_protocol import canonical  # noqa: E402


def sha(value) -> str:
    return hashlib.sha256(canonical(value)).hexdigest()


class SpecTests(unittest.TestCase):
    def setUp(self):
        self.specs = tables.load_specs()

    def problems(self, group, change):
        specs = copy.deepcopy(self.specs)
        change(specs[group])
        return tables.spec_problems(specs)

    def test_committed_specs_and_both_registries_agree(self):
        self.assertEqual(tables.spec_problems(), [])
        self.assertEqual(list(self.specs), list(tables.GROUPS))
        for group in tables.GROUPS:
            self.assertTrue((ROOT / tables.GO_DIRECTORY / f"{group}_columns.go").is_file(), group)
            self.assertTrue((ROOT / tables.RUST_DIRECTORY / f"{group}.rs").is_file(), group)
        registered, problems = tables.go_registry()
        self.assertEqual(problems, [])
        declared, problems = tables.rust_registry()
        self.assertEqual(problems, [])
        spec_columns = {column["id"]: (group, column["input"], column["survey"])
                        for group, column in tables.columns(self.specs)}
        self.assertEqual(registered, spec_columns)
        self.assertEqual(declared, {column: group for column, (group, _, _) in spec_columns.items()})

    def test_the_smoke_columns_claim_their_operations(self):
        self.assertLessEqual(set([
            "tsc/internal/ast/ast.go:Node.Decorators", "tsc/internal/ast/utilities.go:IsDeclarationName",
            "tsc/internal/binder/binder.go:FindUseStrictPrologue",
            "tsc/internal/binder/binder.go:isUseStrictPrologueDirective", "tsc/internal/core/core.go:Splice",
            "tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.PossiblyMatchesDirectoryName"]),
            set(tables.claimed_operations()))

    def test_column_rules(self):
        # The smoke column of each group these cases edit.
        smoke = {"positions": "ast.IsDeclarationName", "core": "core.Splice", "runtime": "runtime.walk"}

        def column(spec):
            return next(column for column in spec["columns"] if column["id"] == smoke[spec["group"]])
        cases = [
            ("positions", lambda spec: column(spec).pop("rust"), "must carry exactly"),
            ("positions", lambda spec: column(spec).update(extra=1), "must carry exactly"),
            ("positions", lambda spec: column(spec)["operations"].append("tsc/internal/ast/utilities.go:Nope"),
             "is not in data/go-functions.tsv"),
            ("positions", lambda spec: column(spec).update(operations=[]), "only a runtime column may claim no"),
            ("runtime", lambda spec: column(spec).update(operations=["tsc/internal/core/core.go:Splice"]),
             "a runtime column claims no operation"),
            ("core", lambda spec: column(spec).update(survey=True), "true only for"),
            ("core", lambda spec: column(spec).update(input="text"), "input must be one of"),
            ("core", lambda spec: column(spec).update(synthetic=[]), "needs survey rows or synthetic rows"),
            ("core", lambda spec: column(spec)["synthetic"].append(dict(column(spec)["synthetic"][0])),
             "distinct names"),
            ("core", lambda spec: column(spec)["synthetic"].append({"name": "a@b", "input": {}}), "distinct names"),
            ("core", lambda spec: column(spec).update(panic_contract="nil_dereference"), "panic_contract"),
            ("positions", lambda spec: spec["columns"].append(copy.deepcopy(next(
                column for column in self.specs["core"]["columns"] if column["id"] == "core.Splice"))),
             "also a column of positions"),
            ("positions", lambda spec: column(spec).update(id="ast.Renamed"), "is not registered by"),
            ("positions", lambda spec: column(spec).update(id="ast.Renamed"), "Go registers column ast.IsDeclarationName"),
            ("positions", lambda spec: column(spec).update(id="ast.Renamed"), "Rust declares column ast.IsDeclarationName"),
            ("positions", lambda spec: column(spec).update(survey=False, synthetic=[{"name": "x", "input": {}}]),
             "the Go registry says group positions, input source, surveyed True"),
            ("positions", lambda spec: spec.update(group="class"), "not a version 1 spec of group positions"),
        ]
        for group, change, text in cases:
            with self.subTest(text=text):
                problems = self.problems(group, change)
                self.assertTrue(any(text in problem for problem in problems), problems)
        partial = {group: spec for group, spec in self.specs.items() if group != "targets"}
        self.assertIn("missing table specs: ['targets']", tables.spec_problems(partial))
        self.assertEqual(tables.spec_problems(partial, complete=False), [])

    def test_registries_are_read_from_the_sources(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            go_dir, rust_dir = root / tables.GO_DIRECTORY, root / tables.RUST_DIRECTORY
            go_dir.mkdir(parents=True)
            rust_dir.mkdir(parents=True)
            (go_dir / "core_columns.go").write_text(
                'func init() {\n\tRegister("core",\n\t\tColumn{ID: "a", Input: "values", Build: nil},\n'
                '\t\tColumn{ID: "b", Input: "source", Build: nil, Survey: nil},\n\t)\n}\n')
            (go_dir / "class_columns.go").write_text('func init() {\n\tRegister("core")\n}\n')
            registered, problems = tables.go_registry(root)
            self.assertEqual(registered, {"a": ("core", "values", False), "b": ("core", "source", True)})
            self.assertEqual(problems, ["class_columns.go must call Register exactly once, for its own group"])
            for group in tables.GROUPS:
                (rust_dir / f"{group}.rs").write_text("pub const COLUMNS: &[&str] = &[];\n")
            (rust_dir / "core.rs").write_text('pub const COLUMNS: &[&str] = &[\n    "a",\n    "b",\n];\n')
            (rust_dir / "class.rs").write_text('pub const COLUMNS: &[&str] = &["a"];\n')
            declared, problems = tables.rust_registry(root)
            self.assertEqual(declared, {"a": "core", "b": "core"})
            self.assertEqual(problems, ["column a is declared twice in Rust"])


class SelectionTests(unittest.TestCase):
    def test_greedy_cover_takes_the_largest_gain_then_the_smaller_file_then_the_row(self):
        rows = [("a", frozenset({1, 2}), 50), ("b", frozenset({1, 2}), 10), ("c", frozenset({3}), 5),
                ("d", frozenset({2, 3, 4}), 100), ("e", frozenset(), 1)]
        chosen, universe = tables.greedy(rows)
        self.assertEqual((chosen, universe), (["d", "b"], 4))
        self.assertEqual(tables.greedy([("b", frozenset({1}), 1), ("a", frozenset({1}), 1)]), (["a"], 1))
        self.assertEqual(tables.greedy([]), ([], 0))
        # Lazy re-evaluation: a stale gain is pushed back, never taken.
        rows = [("x", frozenset({1, 2, 3}), 1), ("y", frozenset({1, 2, 4}), 1), ("z", frozenset({4, 5}), 1)]
        self.assertEqual(tables.greedy(rows)[0], ["x", "z"])

    def test_synthetic_source_inputs_take_text_and_default_path_and_flags(self):
        value = tables.source_input({"filename": "/a.ts", "script_kind": 3, "source": "é;"})
        self.assertEqual(value, {"filename": "/a.ts", "path": "/a.ts", "jsx": False, "force": False,
                                 "script_kind": 3, "source_hex": "é;".encode().hex()})
        with self.assertRaisesRegex(ValueError, "both source and source_hex"):
            tables.source_input({"filename": "/a.ts", "script_kind": 3, "source": "", "source_hex": ""})
        with self.assertRaisesRegex(ValueError, "needs exactly"):
            tables.source_input({"filename": "/a.ts", "source": ""})

    def fake_s06(self):
        return {f"f{index}": {"id": f"f{index}", "filename": f"/f{index}.ts", "path": f"/f{index}.ts", "jsx": False,
                              "force": False, "script_kind": 3, "source_hex": f"{index:02x}", "op": "parse"}
                for index in range(4)}

    def select(self, s06):
        found = {"ast.IsDeclarationName": [("f0", frozenset({"a"}), 1), ("f1", frozenset({"a", "b"}), 1)]}

        def survey(columns, requests_file, *, binary=None, out=None):
            if not columns:
                return {}, None
            return {column: found.get(column, [("f3", frozenset({"k"}), 1)]) for column in columns}, \
                "digest-" + ",".join(columns)
        with patch.object(tables, "survey", side_effect=survey), patch.object(tables, "build_go_driver",
                                                                              return_value=("drv", "d")):
            return tables.select(["positions", "core"], s06=s06)

    def test_selection_orders_rows_and_binds_every_request(self):
        s06 = self.fake_s06()
        document = self.select(s06)
        rows = document["requests"]
        specs = tables.load_specs(["positions", "core"])
        splice = next(column for column in specs["core"]["columns"] if column["id"] == "core.Splice")
        self.assertEqual([row["id"] for row in rows if row["column"] == "ast.IsDeclarationName"],
                         ["ast.IsDeclarationName@s06:f1"], "the cover takes f1, which holds every class")
        self.assertEqual([row["id"] for row in rows if row["column"] == "core.Splice"],
                         [f"core.Splice@{synthetic['name']}" for synthetic in splice["synthetic"]])
        groups = [row["group"] for row in rows]
        self.assertEqual(groups, sorted(groups, key=["positions", "core"].index), "rows are ordered by group")
        self.assertEqual(rows[0], {"id": "ast.IsDeclarationName@s06:f1", "column": "ast.IsDeclarationName",
                                   "group": "positions", "s06": "f1", "request_sha256": sha({
                                       "id": "ast.IsDeclarationName@s06:f1", "column": "ast.IsDeclarationName",
                                       "op": "table", "input": {field: s06["f1"][field]
                                                                for field in tables.SOURCE_FIELDS}})})
        self.assertEqual(set(document["specs"]), {"data/phase1/tables/positions.json", "data/phase1/tables/core.json"})
        self.assertEqual(document["selection"]["groups"]["positions"]["columns"]["ast.IsDeclarationName"],
                         {"classes": 2, "survey_rows": 1, "synthetic_rows": 0})
        self.assertEqual(document["selection"]["groups"]["positions"]["survey_sha256"], "digest-ast.IsDeclarationName")
        self.assertEqual(document["selection"]["groups"]["core"]["survey_sha256"], None)
        self.assertEqual(self.select(s06), document, "selection is deterministic")
        self.assertEqual(tables.inventory_bytes(document), tables.inventory_bytes(self.select(s06)))
        # Materialization rebuilds every request from its source and checks it.
        requests = tables.materialize_requests(document, s06=s06)
        self.assertEqual([sha(request) for request in requests], [row["request_sha256"] for row in rows])
        self.assertEqual(requests[0]["input"]["source_hex"], "01")
        self.assertEqual(requests[-1]["input"], rows[-1]["input"])
        drifted = {**s06, "f1": {**s06["f1"], "source_hex": "ff"}}
        with self.assertRaisesRegex(ValueError, "differ from their request_sha256"):
            tables.materialize_requests(document, s06=drifted)
        with self.assertRaisesRegex(ValueError, "not in the S06 inventory"):
            tables.materialize_requests(document, s06={"f0": s06["f0"]})

    def test_selection_refuses_invalid_specs(self):
        specs = tables.load_specs(["core"])
        next(column for column in specs["core"]["columns"] if column["id"] == "core.Splice")["synthetic"] = []
        with self.assertRaisesRegex(ValueError, "table specs are invalid"):
            tables.select(["core"], specs=specs, s06={})


class InventoryTests(unittest.TestCase):
    """The committed inventory: child-free structure and freshness."""

    def setUp(self):
        self.document = json.loads((ROOT / tables.INVENTORY).read_text())

    def test_committed_inventory_rows_are_spec_columns_with_bound_requests(self):
        problems, stale = tables.inventory_problems(self.document)
        self.assertEqual(problems, [])
        self.assertEqual(self.document["pin"], go.pin())
        self.assertEqual(self.document["selection_sha256"], sha({
            "selection": self.document["selection"],
            "rows": [[row["id"], row["request_sha256"]] for row in self.document["requests"]]}))
        s06 = {row["id"] for row in json.loads((ROOT / "data/s06/requests.json").read_text())["requests"]}
        self.assertLessEqual({row["s06"] for row in self.document["requests"] if "s06" in row}, s06)
        self.assertEqual(self.document["selection"]["corpus"], tables.s06_binding())

    def test_inventory_problems_catch_drift(self):
        document = copy.deepcopy(self.document)
        row = next(row for row in document["requests"] if "input" in row)
        row["input"] = {**row["input"], "drift": 1}
        document["requests"].append(dict(document["requests"][0]))
        document["requests"].append({**document["requests"][0], "id": "x", "column": "no.such"})
        document["specs"] = {**document["specs"], "data/phase1/tables/core.json": "0" * 64}
        problems, stale = tables.inventory_problems(document)
        self.assertTrue(any("differs from its request_sha256" in problem for problem in problems))
        self.assertTrue(any("occurs twice" in problem for problem in problems))
        self.assertTrue(any("which no spec declares" in problem for problem in problems))
        self.assertEqual(stale, ["data/phase1/tables/core.json"])

    def test_check_fails_while_the_inventory_is_stale(self):
        """A spec edit after selection stales the table witness, so check fails until re-selection."""
        with patch.object(tables, "spec_problems", return_value=[]), \
                patch.object(tables, "inventory_problems", return_value=([], ["data/phase1/tables/core.json"])), \
                patch("sys.stdout"):
            self.assertEqual(tables.main(["check"]), 1)
            self.assertEqual(tables.main(["check", "--allow-stale"]), 0)
        with patch.object(tables, "spec_problems", return_value=[]), \
                patch.object(tables, "inventory_problems", return_value=([], [])), patch("sys.stdout"):
            self.assertEqual(tables.main(["check"]), 0)

    def test_the_oracle_reads_the_committed_inventory(self):
        self.assertEqual(go.ORACLES["table"].inventory, tables.INVENTORY)
        self.assertEqual(go.inventory_rows("table"), [(row["id"], row["request_sha256"])
                                                       for row in self.document["requests"]])


class DriverCacheTests(unittest.TestCase):
    """The cached survey driver is named by everything it is built from."""

    def test_a_pin_bump_or_another_toolchain_builds_a_new_driver(self):
        built = []

        def build_driver(oracle, files, flags, destination):
            built.append((oracle, list(flags)))
            Path(destination).parent.mkdir(parents=True, exist_ok=True)
            Path(destination).write_bytes(b"driver")
            return destination, "go", {}
        state = {"pin": "a" * 40, "go": "go1.27.1"}
        with tempfile.TemporaryDirectory() as scratch, \
                patch.object(tables, "TARGET", Path(scratch)), \
                patch.object(tables, "pin", side_effect=lambda: state["pin"]), \
                patch.object(go, "go_version", side_effect=lambda env: state["go"]), \
                patch("s04.go_environment", return_value={}), \
                patch.object(go, "build_driver", side_effect=build_driver):
            first, _ = tables.build_go_driver()
            self.assertEqual(tables.build_go_driver()[0], first, "the same inputs reuse the cached binary")
            self.assertEqual(len(built), 1)
            state["pin"] = "b" * 40
            bumped, _ = tables.build_go_driver()
            state["go"] = "go1.27.2"
            toolchain, _ = tables.build_go_driver()
            self.assertEqual(len({first, bumped, toolchain}), 3)
            self.assertEqual(built, [("table", ["-trimpath", "-mod=readonly"])] * 3)
            self.assertTrue(all(path.is_file() for path in (first, bumped, toolchain)))


@unittest.skipUnless(shutil.which("go"), "go is not on PATH")
class DriverTests(unittest.TestCase):
    """The Go driver: canonical values digest as Python digests them, panics and setups refuse."""

    def test_synthetic_rows_run_and_their_raw_values_digest_in_python(self):
        document = json.loads((ROOT / tables.INVENTORY).read_text())
        synthetic = [row for row in document["requests"] if "input" in row]
        requests = tables.materialize_requests({"requests": synthetic}, s06={})
        binary, _ = tables.build_go_driver()
        with tempfile.TemporaryDirectory() as scratch:
            source, out = Path(scratch) / "requests.ndjson", Path(scratch) / "rows.ndjson"
            broken = {"id": "core.Splice@broken", "column": "core.Splice", "op": "table", "input": {"s": "x"}}
            source.write_bytes(b"".join(canonical(request) + b"\n" for request in requests + [broken]))
            subprocess.run([str(binary), str(source), str(out)], check=True, env=dict(os.environ, PHASE1_TABLE_RAW="1"))
            rows = [json.loads(line) for line in out.read_bytes().splitlines()]
        self.assertEqual([row["row"] for row in rows], [request["id"] for request in requests] + [broken["id"]])
        for row in rows[:-1]:
            with self.subTest(row=row["row"]):
                self.assertEqual(row["outcomes"], {"setup": "ok", "column": "ok"})
                self.assertEqual(row["digests"]["column"], sha(row["value"]))
                self.assertEqual(go.table_row(row)["digests"], row["digests"])
        values = {row["row"]: row["value"] for row in rows[:-1]}
        self.assertEqual(values["runtime.values@objects"],
                         next(row["input"]["value"] for row in synthetic if row["id"] == "runtime.values@objects"))
        self.assertEqual(rows[-1]["outcomes"], {"setup": "panic", "column": "not_run"})
        self.assertIn("input:", bytes.fromhex(rows[-1]["messages"]["setup"]).decode())
        self.assertNotIn("digests", rows[-1])

    # Test-only columns built into a separate driver: the reference of every
    # JSDoc comment each node lists, under the plain walk and the JSDoc walk.
    PROBE = """package main

import "encoding/json"

func init() {
	Register("probe",
		Column{ID: "probe.plain", Input: "source", Build: probeRefs(ParseSource)},
		Column{ID: "probe.jsdoc", Input: "source_jsdoc", Build: probeRefs(ParseSourceJSDoc)},
	)
}

func probeRefs(setup func(json.RawMessage) (*Parsed, error)) func(json.RawMessage) (func() any, error) {
	return func(raw json.RawMessage) (func() any, error) {
		p, err := setup(raw)
		if err != nil {
			return nil, err
		}
		return func() any {
			out := []any{}
			for _, node := range p.Nodes {
				for _, comment := range node.JSDoc(p.File) {
					out = append(out, []any{p.Ref(node), p.Ref(comment), p.Ref(comment.Parent)})
				}
			}
			return out
		}, nil
	}
}
"""

    def test_a_node_outside_the_walk_stops_the_driver_and_the_jsdoc_walk_gives_it_one_reference(self):
        files = go.driver_sources("table", cover=False)
        files["tsc/internal/phase1table/zz_probe_columns.go"] = self.PROBE.encode()
        text = "/** @typedef {{a: string}} T */\n/** @type {T} */\nvar x;\n"
        source = tables.source_input({"filename": "/t.js", "script_kind": 1, "source": text})
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            binary, _, _ = go.build_driver("table", files, list(tables.DRIVER_FLAGS), scratch / "probe")

            def run(*columns):
                requests = scratch / "requests.ndjson"
                requests.write_bytes(b"".join(canonical({"id": column, "column": column, "op": "table",
                                                         "input": source}) + b"\n" for column in columns))
                completed = subprocess.run([str(binary), str(requests), str(scratch / "rows.ndjson")],
                                           capture_output=True, env=dict(os.environ, PHASE1_TABLE_RAW="1"))
                rows = [json.loads(line) for line in (scratch / "rows.ndjson").read_bytes().splitlines()]
                return completed, {row["row"]: row for row in rows}
            # A JSDoc comment has no reference in the plain walk: the driver
            # stops instead of writing a placeholder both sides could share.
            completed, rows = run("probe.plain")
            self.assertEqual(completed.returncode, 2)
            self.assertIn(b"is outside the Walk", completed.stderr)
            self.assertEqual(rows, {})
            completed, rows = run("probe.jsdoc", "runtime.walk_jsdoc", "runtime.walk")
            self.assertEqual(completed.returncode, 0, completed.stderr)
        for row in rows.values():
            self.assertEqual(row["outcomes"], {"setup": "ok", "column": "ok"})
        refs, walk, plain = (rows[column]["value"] for column in ("probe.jsdoc", "runtime.walk_jsdoc", "runtime.walk"))
        # The reparsed @typedef declaration lists the statement's comment as
        # its own; the comment has one reference, under its parent.
        self.assertEqual(len(refs), 3)
        self.assertTrue(all(type(value) is int for triple in refs for value in triple))
        shared = [triple for triple in refs if triple[1] == refs[0][1]]
        self.assertEqual(len(shared), 2)
        statement = shared[0][2]
        self.assertEqual({triple[2] for triple in refs}, {statement})
        self.assertEqual({triple[0] for triple in shared}, {statement, 1})
        jsdoc_kind = walk[refs[0][1]][0]
        self.assertEqual([at for at, row in enumerate(walk) if row[0] == jsdoc_kind], sorted({t[1] for t in refs}))
        self.assertEqual(refs[0][1], statement + 1, "a node's comments come before its children")
        self.assertTrue(all(walk[t[1]][3] == statement for t in refs))
        # The plain walk is the JSDoc walk without the comment subtrees.
        inside = set()
        for at, row in enumerate(walk):
            if row[0] == jsdoc_kind or row[3] in inside:
                inside.add(at)
        self.assertEqual([row[:3] for at, row in enumerate(walk) if at not in inside], [row[:3] for row in plain])


if __name__ == "__main__":
    unittest.main()
