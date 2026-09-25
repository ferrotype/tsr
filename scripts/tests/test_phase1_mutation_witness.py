"""mutation_kill witnesses confer coverage only while every binding holds.

Every test builds a tiny synthetic campaign in a temporary directory: a fake
crate carrying `port:` markers, a manifest with plan homes and a paired
control, two-oracle results over shared row ids, per-oracle native freezes and
Go-reach indices, request inventories and a Go function table. Nothing here
runs a mutant, a driver or Go; the subject is the child-free harness contract
of docs/PHASE1-mutation-witnesses.md.
"""
import copy
import gzip
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1  # noqa: E402
import phase1_coverage as coverage  # noqa: E402
import phase1_integration as integration  # noqa: E402
import phase1_producers as producers  # noqa: E402
import phase1_scope as scope  # noqa: E402
import phase1_tables  # noqa: E402

PIN = "0" * 40
OP_A = "tsc/internal/parser/parser.go:Parser.parseA"
OP_B = "tsc/internal/parser/parser.go:Parser.parseB"
OP_C = "tsc/internal/parser/parser.go:Parser.parseC"
OP_D = "tsc/internal/parser/parser.go:Parser.parseD"
OP_E = "tsc/internal/parser/parser.go:Parser.parseE"
OP_F = "tsc/internal/parser/parser.go:Parser.parseF"
OP_ARM = "tsc/internal/ast/ast.go:ArmNode.computeSubtreeFacts"
OP_X = "tsc/internal/parser/parser.go:Parser.parseX"
OP_POOL = "tsc/internal/scanner/scanner.go:cleared"
FAKE = "crates/tsr_fake/src/lib.rs"
OTHER = "crates/tsr_other/src/lib.rs"
FAKE_SOURCE = f"""//! A fixture crate.

/// port: {OP_A}
fn parse_a(x: u32) -> u32 {{
    x + 1
}}

/// port: {OP_B}
/// port: {OP_C}
fn shared(x: u32) -> u32 {{
    x * 2
}}

fn arms(kind: u32) -> u32 {{
    match kind {{
        // port: {OP_ARM}
        1 => 7,
        _ => 0,
    }}
}}

/// port: {OP_D}
fn d_one(x: u32) -> u32 {{
    x - 1
}}

/// port: {OP_E}
fn e_fn(x: u32) -> u32 {{
    x + 5
}}

/// port: {OP_F}
fn parse_f(x: u32) -> u32 {{
    x + 6
}}
"""
OTHER_SOURCE = f"""/// port: {OP_D}
pub fn d_two(x: u32) -> u32 {{
    x - 2
}}

pub fn helper(x: u32) -> u32 {{
    // port: {OP_E}
    let y = x + 3;
    y
}}
"""
STAGES = {"e1": ("parse", "node_index_before", "node_index_after", "encode_source_file"),
          "binder": ("parsed_graph", "bound_graph", "repeated_graph")}
ORACLES = ("e1", "binder")
ALLOCATING = "wrap_result:self.create_missing_identifier()"
# Every excuse the fixture writes names both traced oracles.
UNREACHED = "no production or observation reach on the e1 and binder campaigns"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical_digest(value) -> str:
    """phase1_mutation_go.stage_rule_digest's rule: sha256 of s06_protocol.canonical."""
    return sha(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode())


def request(row: str) -> str:
    return sha(row.encode())


def digests(oracle: str, row: str) -> dict:
    return {stage: sha(f"{oracle}/{row}/{stage}".encode()) for stage in STAGES[oracle]}


def bindings(oracle: str) -> dict:
    """What the fixture's "current code" states for an oracle's Go artifacts."""
    rule = {"oracle": oracle, "production": ["parse"], "observe": "nothing"}
    return {"oracle_sources": {f"scripts/{oracle}_oracle/main.go": sha(oracle.encode())},
            "instrumentation_sha256": sha(b"instrumentation " + oracle.encode()),
            "stage_rule": rule, "stage_rule_sha256": canonical_digest(rule), "unstable_ops": [OP_POOL]}


def fn_site(text: str, ops: list[str], function: str) -> tuple[int, dict, list[int]]:
    """The plan's shape for a function site: `fn` line, marker lines, span from the topmost marker."""
    lines = text.split("\n")
    sig = next(i for i, line in enumerate(lines, 1) if f"fn {function}(" in line)
    markers = {op: max(i for i, line in enumerate(lines[:sig], 1) if line.endswith(f"port: {op}")) for op in ops}
    end = next(i for i in range(sig, len(lines) + 1) if lines[i - 1] == "}")
    return sig, markers, [min(markers.values()), end]


class Campaign:
    """A synthetic, internally consistent two-oracle campaign on disk."""

    def __init__(self, root: Path):
        self.root = root
        self.rows = ["r0", "r1", "r2", "r3"]
        self.plain_reach = False
        self.homes = True
        self.bindings = {oracle: bindings(oracle) for oracle in ORACLES}
        self.headers = {oracle: {} for oracle in ORACLES}
        # oracle -> extra native-freeze header fields.
        self.native_headers: dict[str, dict] = {oracle: {} for oracle in ORACLES}
        self.reach = {oracle: {OP_A: [0, 1], OP_B: [1], OP_C: [2], OP_D: [0], OP_ARM: [3], OP_E: [2], OP_F: [1],
                               OP_POOL: [0, 1]} for oracle in ORACLES}
        self.write_sources(FAKE_SOURCE, OTHER_SOURCE)
        self.write("data/go-functions.tsv", b"fixture go function table\n")
        self.mutants = [
            self.fn_mutant(1, "kA", [OP_A], FAKE, "parse_a"),
            self.fn_mutant(2, "kS", [OP_B, OP_C], FAKE, "shared"),
            self.fn_mutant(3, "kD1", [OP_D], FAKE, "d_one"),
            self.fn_mutant(4, "kD2", [OP_D], OTHER, "d_two"),
            self.marker_mutant(5, "kR", OP_ARM, FAKE, "arms", "arm"),
            self.fn_mutant(6, "kE", [OP_E], FAKE, "e_fn"),
            self.marker_mutant(7, "kEs", OP_E, OTHER, "helper", "stmt"),
            self.fn_mutant(8, "kF", [OP_F], FAKE, "parse_f", operator=ALLOCATING, control=9),
        ]
        self.mutants.append(dict(self.mutants[-1], id=9, key="kFc", operator="control", control=None, control_of=8,
                                 insert=[{"line": 1, "column": 0, "order": 1,
                                          "text": "if hit(9) { let _ = self.create_missing_identifier(); }"}]))
        # key -> oracle -> (state, kill rows, Rust rows); a missing oracle leaves it unrun there.
        self.runs = {
            "kA": {"e1": ("killed", ["r0"], 2), "binder": ("killed", ["r0"], 2)},
            "kS": {"e1": ("killed", ["r1"], 1), "binder": ("survived", [], 1)},
            "kD1": {"e1": ("killed", ["r0"], 1), "binder": ("killed", ["r0"], 1)},
            "kD2": {"e1": ("not_reached", [], 0), "binder": ("not_reached", [], 0)},
            "kR": {"e1": ("killed", ["r3"], 1), "binder": ("survived", [], 1)},
            "kE": {"e1": ("killed", ["r2"], 1), "binder": ("killed", ["r2"], 1)},
            "kEs": {"e1": ("not_reached", [], 0), "binder": ("not_reached", [], 0)},
            "kF": {"e1": ("killed", ["r1"], 1), "binder": ("not_reached", [], 0)},
        }
        # (key, oracle, row) -> field overrides of that recorded kill.
        self.kill_fields: dict[tuple[str, str, str], dict] = {}
        # (key, oracle) -> rows whose observation stages executed the site.
        self.observe: dict[tuple[str, str], int] = {}
        self.write_all()

    def write(self, relative: str, data: bytes) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def write_specs(self, claims: dict[str, list[str]], extra: dict | None = None) -> dict[str, str]:
        """Table specs with every `claims` column (and its operations) in one group; their digests."""
        specs = {}
        for group in phase1_tables.GROUPS:
            relative = f"{phase1_tables.SPEC_DIRECTORY}/{group}.json"
            columns = [{"id": column, "operations": ops} for column, ops in claims.items()] if group == "class" else []
            data = json.dumps({"columns": columns, **(extra or {})}).encode()
            self.write(relative, data)
            specs[relative] = sha(data)
        return specs

    def write_sources(self, fake: str, other: str) -> None:
        self.write(FAKE, fake.encode())
        self.write(OTHER, other.encode())

    def fn_mutant(self, number: int, key: str, ops: list[str], file: str, function: str, *,
                  operator: str = "return:0", control: int | None = None) -> dict:
        data = (self.root / file).read_bytes()
        sig, markers, span = fn_site(data.decode(), ops, function)
        return {"id": number, "key": key, "op": ops[0], "ops": list(ops), "file": file, "function": function,
                "site_kind": "fn", "site_line": sig, "markers": markers, "span": span,
                "span_sha256": scope.span_digest(data, *span), "operator": operator, "control": control,
                "insert": [{"line": sig, "column": 0, "order": 1, "text": f"if hit({number}) {{ return 0; }}"}]}

    def marker_mutant(self, number: int, key: str, op: str, file: str, function: str, kind: str) -> dict:
        data = (self.root / file).read_bytes()
        marker = next(i for i, line in enumerate(data.decode().split("\n"), 1) if line.endswith(f"port: {op}"))
        return {"id": number, "key": key, "op": op, "ops": [op], "file": file, "function": function,
                "site_kind": kind, "site_line": marker + 1, "markers": {op: marker}, "span": [marker, marker + 1],
                "span_sha256": scope.span_digest(data, marker, marker + 1), "operator": f"{kind}:0", "control": None,
                "insert": [{"line": marker + 1, "column": 0, "order": 1, "text": f"if hit({number}) {{ 0 }}"}]}

    def plan_homes(self) -> dict:
        homes: dict[str, dict] = {}
        for mutant in self.mutants:
            if scope.is_control(mutant):
                continue
            for op in mutant["ops"]:
                site = (mutant["file"], mutant["function"], mutant["site_kind"], mutant["site_line"])
                home = homes.setdefault(op, {}).setdefault(site, {
                    "file": mutant["file"], "function": mutant["function"], "site_kind": mutant["site_kind"],
                    "site_line": mutant["site_line"], "span_sha256": mutant["span_sha256"], "mutants": []})
                home["mutants"].append(mutant["key"])
        return {op: list(sites.values()) for op, sites in sorted(homes.items())}

    def manifest(self) -> dict:
        return {"version": 1, "mutants": self.mutants, **({"homes": self.plan_homes()} if self.homes else {})}

    def kill(self, oracle: str, row: str, key: str) -> dict:
        native = digests(oracle, row)
        first = STAGES[oracle][0]
        mutated = dict(native, **{first: sha(f"mutated {key} {row}".encode())})
        control = dict(native, **{first: sha(f"control {key} {row}".encode())}) if key == "kF" else None
        return {"oracle": oracle, "row": row, "request_sha256": request(row), "stages": [first], "native": native,
                "base": dict(native), "mutant": mutated, "control": control, "ops": [],
                "reach": {"rust": True, "go": []}, **self.kill_fields.get((key, oracle, row), {})}

    def results(self) -> dict:
        """phase1_mutation_run.results' shape, bound to the artifacts on disk."""
        digest = lambda relative: sha((self.root / relative).read_bytes())  # noqa: E731
        records = []
        for mutant in self.mutants:
            if scope.is_control(mutant):
                continue
            runs = self.runs[mutant["key"]]
            kills = [self.kill(oracle, row, mutant["key"]) for oracle, (_state, rows, _rust) in sorted(runs.items())
                     for row in rows]
            states = [state for state, _rows, _rust in runs.values()]
            state = "killed" if kills else next((s for s in ("timeout", "crash", "survived", "build_failed",
                                                             "unsupported", "not_reached") if s in states),
                                                "not_reached")
            records.append({"id": mutant["id"], "key": mutant["key"], "op": mutant["op"], "ops": mutant["ops"],
                            "state": state, "candidates": len(kills), "ran": len(kills), "kills": kills,
                            # [eligible rows, all rows, observation rows] from each oracle's full trace.
                            "reach": {oracle: [run[2], run[2], self.observe.get((mutant["key"], oracle), 0)]
                                      for oracle, run in runs.items()},
                            "oracles": {oracle: {"state": run[0], "rust_rows": run[2], "candidates": len(run[1]),
                                                 "ran": len(run[1]), "crashes": 0} for oracle, run in runs.items()}})
        return {"version": 1, "inputs": {"plan_sha256": digest(scope.MUTATION_MANIFEST), "traced_oracles": list(ORACLES),
                                         "oracles": {oracle: {
                                             "native_sha256": digest(scope.mutation_native_path(oracle)),
                                             "go_reach_sha256": digest(scope.mutation_reach_path(oracle))}
                                             for oracle in ORACLES}},
                "mutants": records, "operations": self.operations(records)}

    def operations(self, records: list[dict]) -> dict:
        """The results' per-operation homes with `excused` as phase1_mutation_run.results decides it."""
        by_key = {record["key"]: record for record in records}
        operations = {}
        for op, homes in (self.plan_homes() if self.homes else {}).items():
            rows = []
            for plan_home in homes:
                keys = [key for key in plan_home["mutants"] if key in by_key]
                reach = [by_key[key]["reach"].get(oracle) for key in keys for oracle in ORACLES]
                excused = (bool(keys) and all(entry == [0, 0, 0] for entry in reach)
                           and not any(by_key[key]["kills"] for key in keys))
                rows.append({**plan_home, "mutants": keys, "excused": excused})
            operations[op] = {"homes": rows}
        return operations

    def native(self, oracle: str) -> dict:
        return {"version": 1, "oracle": oracle, "pin": PIN, "go_version": "go1.27.1", "stages": list(STAGES[oracle]),
                "oracle_sources": self.bindings[oracle]["oracle_sources"], **self.native_headers[oracle],
                "rows": [{"row": row, "request_sha256": request(row),
                          "outcomes": dict.fromkeys(STAGES[oracle], "ok"), "digests": digests(oracle, row)}
                         for row in self.rows]}

    def reach_document(self, oracle: str, native_bytes: bytes, rows: int) -> dict:
        reach = self.reach[oracle]
        stated = self.bindings[oracle]
        return {"version": 1, "oracle": oracle, "pin": PIN, "go_version": "go1.27.1",
                "instrumentation_sha256": stated["instrumentation_sha256"], "stage_rule": stated["stage_rule"],
                "stage_rule_sha256": stated["stage_rule_sha256"], "unstable_ops": list(stated["unstable_ops"]),
                "go_functions_sha256": sha((self.root / "data/go-functions.tsv").read_bytes()),
                "binary_sha256": "2" * 64, "native_sha256": sha(native_bytes), "rows_checked": rows,
                **({"ops": reach} if self.plain_reach else {"row_encoding": "gaps", "op_row_gaps": {
                    op: [index - previous for previous, index in zip([0] + sorted(indices)[:-1], sorted(indices))]
                    for op, indices in reach.items()}}), **self.headers[oracle]}

    def write_all(self, results: dict | None = None, natives: dict | None = None) -> None:
        self.write("data/upstream.json", json.dumps({"pin": PIN}).encode())
        for oracle in ORACLES:
            self.write(f"inventory/{oracle}.json", json.dumps({"version": 1, "requests": [
                {"id": row, "request_sha256": request(row)} for row in self.rows]}).encode())
            native = (natives or {}).get(oracle) or self.native(oracle)
            native_bytes = gzip.compress(json.dumps(native).encode(), mtime=0)
            self.write(scope.mutation_native_path(oracle), native_bytes)
            self.write(scope.mutation_reach_path(oracle), gzip.compress(json.dumps(
                self.reach_document(oracle, native_bytes, len(native["rows"]))).encode(), mtime=0))
        self.write(scope.MUTATION_MANIFEST, json.dumps(self.manifest()).encode())
        self.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(results or self.results()).encode(), mtime=0))
        self.write("data/phase1/scope.json", json.dumps({"operations": [
            {"id": op} for op in (OP_A, OP_B, OP_C, OP_D, OP_E, OP_F, OP_ARM, OP_X)]}).encode())

    def rebind(self) -> None:
        """Rewrite the results as a campaign run against the artifacts now on disk."""
        self.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(self.results()).encode(), mtime=0))

    def rewrite_reach(self, name: str, /, **fields) -> None:
        path = self.root / scope.mutation_reach_path(name)
        document = {**json.loads(gzip.decompress(path.read_bytes())), **fields}
        self.write(scope.mutation_reach_path(name), gzip.compress(json.dumps(
            {key: value for key, value in document.items() if value is not None}).encode(), mtime=0))
        self.rebind()

    def declaration(self, operations: list[str], keys: list[str], not_claimed: dict | None = None,
                    oracle: str = "e1") -> dict:
        return {"id": f"mutation/{oracle}", "kind": "mutation_kill", "oracle": oracle,
                "artifact": scope.MUTATION_RESULTS, "operations": operations,
                "mutation_gate": "python3 scripts/phase1_mutation_run.py confirm",
                "mutants": [{field: mutant.get(field) for field in scope.MUTANT_FIELDS} for mutant in self.mutants
                            if mutant["key"] in keys],
                "not_claimed": not_claimed or {}, "witnesses": "synthetic campaign"}

    def forge(self, witness: dict, kills: list[tuple[str, str, str]]) -> dict:
        """Evidence that matches the current artifacts, bypassing record's refusals."""
        oracle = witness["oracle"]
        digests_now = {}
        for field, relative in (("manifest_sha256", scope.MUTATION_MANIFEST),
                                ("results_sha256", scope.MUTATION_RESULTS),
                                ("native_sha256", scope.mutation_native_path(oracle)),
                                ("go_reach_sha256", scope.mutation_reach_path(oracle))):
            digests_now[field] = sha((self.root / relative).read_bytes())
        witness = copy.deepcopy(witness)
        witness["mutation_evidence"] = {
            "claims_sha256": scope.witness_claims_digest(witness), **digests_now,
            "kills": [{"op": op, "key": key, "row": row, "request_sha256": request(row)} for op, key, row in kills],
            "result": "killed"}
        return witness

    def reforged(self, witness: dict) -> dict:
        """The same recorded kills, with digests re-bound to the rewritten artifacts."""
        return self.forge(witness, [(kill["op"], kill["key"], kill["row"])
                                    for kill in witness["mutation_evidence"]["kills"]])

    def record(self, witness: dict) -> dict:
        witness = copy.deepcopy(witness)
        witness["mutation_evidence"] = scope.mutation_evidence(witness, self.root)
        return witness

    def state(self, witness: dict, committed_scope: dict | None = None) -> dict:
        return scope.recorded_mutations({"witnesses": [witness]}, self.root,
                                        committed_scope=committed_scope)[witness["id"]]


def home(file: str, function: str, kind: str = "fn", line: int | None = None) -> str:
    return scope.home_identity({"file": file, "function": function, "site_kind": kind, "site_line": line})


def excuses(campaign: Campaign) -> dict:
    stmt = next(mutant for mutant in campaign.mutants if mutant["key"] == "kEs")
    return {home(OTHER, "d_two"): UNREACHED, home(OTHER, "helper", "stmt", stmt["site_line"]): UNREACHED}


ALL_OPS = (OP_A, OP_B, OP_D, OP_ARM, OP_E, OP_F)
ALL_KEYS = ("kA", "kS", "kD1", "kR", "kE", "kF")


class MutationFixture(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        oracles = patch.dict(scope.MUTATION_ORACLES, {oracle: {
            "inventory": f"inventory/{oracle}.json", "rows": "requests", "request": "request_sha256",
            "select": None} for oracle in ORACLES}, clear=True)
        oracles.start()
        self.addCleanup(oracles.stop)
        self.campaign = Campaign(self.root)
        stated = patch.object(scope, "mutation_go_bindings",
                              lambda oracle: (copy.deepcopy(self.campaign.bindings[oracle]), None))
        stated.start()
        self.addCleanup(stated.stop)

    def bound(self, operations=ALL_OPS, keys=ALL_KEYS, not_claimed=None, oracle="e1"):
        not_claimed = excuses(self.campaign) if not_claimed is None else not_claimed
        witness = self.campaign.record(self.campaign.declaration(list(operations), list(keys), not_claimed, oracle))
        state = self.campaign.state(witness)
        self.assertEqual(state["state"], "bound", state)
        self.assertEqual(state["operations"], sorted(operations), state)
        return witness

    def assert_stale(self, witness: dict, operation: str, text: str):
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(state["state"], "bound", state)
        self.assertNotIn(operation, state["operations"])
        self.assertIn(text, state["stale_operations"][operation])

    def assert_whole_witness_stale(self, witness: dict, text: str):
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(state["state"], "stale", state)
        self.assertEqual(state["operations"], [])
        self.assertIn(text, state["reason"])


class BindingTests(MutationFixture):
    def test_declared_but_unrecorded_witness_confers_nothing(self):
        witness = self.campaign.declaration([OP_A], ["kA"])
        state = self.campaign.state(witness)
        self.assertEqual(state["state"], "unrecorded")
        self.assertEqual(state["operations"], [])
        self.assertIn("never recorded", state["stale_operations"][OP_A])
        document = {"witnesses": [witness]}
        self.assertEqual(scope.covering_witness_operations(document, self.root), [("mutation/e1", [])])

    def test_recorded_kills_bind_and_confer_exactly_their_operations(self):
        witness = self.bound()
        self.assertEqual(scope.covering_witness_operations({"witnesses": [witness]}, self.root),
                         [("mutation/e1", sorted(ALL_OPS))])
        pairs = {(kill["op"], kill["key"], kill["row"]) for kill in witness["mutation_evidence"]["kills"]}
        self.assertEqual(pairs, {(OP_A, "kA", "r0"), (OP_B, "kS", "r1"), (OP_D, "kD1", "r0"), (OP_ARM, "kR", "r3"),
                                 (OP_E, "kE", "r2"), (OP_F, "kF", "r1")})

    def test_each_oracle_binds_its_own_witness_over_shared_rows(self):
        declared, unclaimed = scope.declare_mutation_witness("binder", scope.MUTATION_RESULTS, self.root)
        self.assertEqual(declared["operations"], sorted([OP_A, OP_D, OP_E]))
        self.assertIn("no recorded binder kill", unclaimed[OP_B])
        recorded = self.campaign.record(declared)
        self.assertEqual({(kill["key"], kill["row"]) for kill in recorded["mutation_evidence"]["kills"]},
                         {("kA", "r0"), ("kD1", "r0"), ("kE", "r2")})
        self.assertEqual(self.campaign.state(recorded)["operations"], sorted([OP_A, OP_D, OP_E]))

    def test_declaration_and_artifact_changes_stale_the_whole_witness(self):
        witness = self.bound()
        changed = copy.deepcopy(witness)
        changed["witnesses"] = "edited description"
        self.assertEqual(self.campaign.state(changed)["state"], "stale")
        rewrites = {
            "manifest": lambda: (self.campaign.write(
                scope.MUTATION_MANIFEST, json.dumps({**self.campaign.manifest(), "x": 1}).encode()),
                self.campaign.rebind()),
            "results": lambda: self.campaign.write_all(results={**self.campaign.results(), "note": "rerun"}),
            "native": lambda: self.campaign.write_all(natives={"e1": {**self.campaign.native("e1"),
                                                                      "go_version": "go1.27.2"}}),
            "go reach": lambda: self.campaign.rewrite_reach("e1", binary_sha256="3" * 64),
        }
        for name, rewrite in rewrites.items():
            with self.subTest(artifact=name):
                self.campaign.write_all()
                self.assertEqual(self.campaign.state(witness)["state"], "bound")
                rewrite()
                state = self.campaign.state(witness)
                self.assertEqual(state["state"], "stale", name)
                self.assertEqual(state["operations"], [])
                self.assertIn("changed after the evidence was recorded", state["reason"])
                self.assertEqual(set(state["stale_operations"]), set(ALL_OPS))

    def test_every_part_of_the_declaration_is_bound(self):
        witness = self.bound()
        edits = {
            "operations": lambda w: w["operations"].remove(OP_F),
            "mutants": lambda w: w["mutants"].pop(),
            "mutant field": lambda w: w["mutants"][0].update(operator="return:1"),
            "not_claimed": lambda w: w["not_claimed"].update({home(OTHER, "d_two"): UNREACHED + "."}),
            "oracle": lambda w: w.update(oracle="binder"),
            "artifact": lambda w: w.update(artifact=scope.MUTATION_DIRECTORY + "/other.json.gz"),
            "gate": lambda w: w.update(mutation_gate="true"),
            "description": lambda w: w.update(witnesses="edited"),
        }
        for name, edit in edits.items():
            with self.subTest(field=name):
                changed = copy.deepcopy(witness)
                edit(changed)
                state = self.campaign.state(changed)
                self.assertEqual(state["state"], "stale")
                self.assertIn("declaration changed", state["reason"])

    def test_claimed_mutant_must_equal_the_manifest(self):
        # The claims digest is recomputable by anyone, so only the manifest
        # stops a declaration from pointing its span digest at an edited span.
        witness = self.bound()
        self.campaign.write_sources(FAKE_SOURCE.replace("x + 1", "x + 2"), OTHER_SOURCE)
        self.assertNotIn(OP_A, self.campaign.state(witness)["operations"])
        edited = copy.deepcopy(witness)
        data = (self.root / FAKE).read_bytes()
        mutant = next(row for row in edited["mutants"] if row["key"] == "kA")
        mutant["span_sha256"] = scope.span_digest(data, *mutant["span"])
        state = self.campaign.state(self.campaign.reforged(edited))
        self.assertEqual(state["state"], "stale")
        self.assertIn("kA differs from the manifest", state["reason"])

    def test_changed_request_row_stales_what_rests_on_it(self):
        witness = self.bound()
        inventory = {"version": 1, "requests": [{"id": row, "request_sha256": request(row) if row != "r0" else "f" * 64}
                                                for row in self.campaign.rows]}
        self.campaign.write("inventory/e1.json", json.dumps(inventory).encode())
        state = self.campaign.state(witness)
        self.assertEqual(state["state"], "stale")
        self.assertIn("request inventory", state["reason"])

    def test_span_edit_returns_only_its_operation_to_pending(self):
        witness = self.bound()
        self.campaign.write_sources(FAKE_SOURCE.replace("x + 1", "x + 2"), OTHER_SOURCE)
        state = self.campaign.state(witness)
        self.assertEqual(state["state"], "bound")
        self.assertEqual(state["operations"], sorted(set(ALL_OPS) - {OP_A}))
        self.assertIn("changed since the plan", state["stale_operations"][OP_A])

    def test_arm_span_edit_stales_the_arm_operation(self):
        witness = self.bound()
        self.campaign.write_sources(FAKE_SOURCE.replace("1 => 7,", "1 => 8,"), OTHER_SOURCE)
        state = self.campaign.state(witness)
        self.assertNotIn(OP_ARM, state["operations"])
        self.assertIn(OP_A, state["operations"])

    def test_unrelated_edit_that_moves_the_span_does_not_stale(self):
        witness = self.bound()
        moved = FAKE_SOURCE.replace("//! A fixture crate.\n", "//! A fixture crate.\n\nconst UNRELATED: u32 = 3;\n\n")
        self.campaign.write_sources(moved, "// a new header line\n" + OTHER_SOURCE)
        state = self.campaign.state(witness)
        self.assertEqual(state["operations"], sorted(ALL_OPS), state)

    def test_claimed_mutant_without_a_recorded_kill_stales_the_witness(self):
        declaration = self.campaign.declaration([OP_A], ["kA", "kD1"])
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_A, "kA", "r0")]))
        self.assertEqual(state["state"], "stale")
        self.assertIn("kD1 has no recorded kill", state["reason"])

    def test_removed_marker_stales_the_operation(self):
        witness = self.bound()
        self.campaign.write_sources(FAKE_SOURCE.replace(f"/// port: {OP_A}\n", "/// A helper.\n"), OTHER_SOURCE)
        state = self.campaign.state(witness)
        self.assertIn("no longer carries", state["stale_operations"][OP_A])

    def test_go_reach_checked_against_another_native_freeze_is_rejected(self):
        witness = self.bound()
        self.assertEqual(self.campaign.state(self.campaign.reforged(witness))["state"], "bound")
        for field, value, text in (("native_sha256", "e" * 64, "different native freeze"),
                                   ("pin", "1" * 40, "records pin"), ("oracle", "binder", "version 1 e1")):
            with self.subTest(field=field):
                self.campaign.write_all()
                self.campaign.rewrite_reach("e1", **{field: value})
                self.assert_whole_witness_stale(witness, text)

    def test_go_reach_gap_encoding_matches_the_go_tool(self):
        import phase1_mutation_go
        for indices in ([], [0], [0, 1, 5], [3, 4, 10, 11]):
            with self.subTest(indices=indices):
                gaps = phase1_mutation_go.encode_gaps(indices)
                self.assertEqual(scope.reach_indices(gaps, True), frozenset(indices))
        self.assertEqual(scope.reach_indices([2, 0], True), frozenset())
        self.assertEqual(scope.reach_indices([-1], True), frozenset())
        for malformed in ([1.5], ["0"], [True], None):
            self.assertEqual(scope.reach_indices(malformed, True), frozenset())
            self.assertEqual(scope.reach_indices(malformed, False), frozenset())
        self.assertEqual(scope.reach_indices([0, 3], False), frozenset({0, 3}))

    def test_plain_operation_index_is_read_too(self):
        self.campaign.plain_reach = True
        self.campaign.write_all()
        self.bound()

    def test_go_reach_must_check_every_native_row(self):
        witness = self.bound()
        self.campaign.rewrite_reach("e1", rows_checked=3)
        self.assert_whole_witness_stale(witness, "checked 3 rows")

    def test_results_of_another_plan_freeze_or_reach_index_are_rejected(self):
        witness = self.bound()
        results = self.campaign.results()
        for name, change, text in (
                ("plan", lambda r: r["inputs"].update(plan_sha256="f" * 64), "plan other than"),
                ("native", lambda r: r["inputs"]["oracles"]["e1"].update(native_sha256="f" * 64), "another"),
                ("reach", lambda r: r["inputs"]["oracles"]["e1"].update(go_reach_sha256="f" * 64), "another"),
                ("campaign", lambda r: r["inputs"]["oracles"].pop("e1"), "no e1 campaign")):
            with self.subTest(name=name):
                changed = copy.deepcopy(results)
                change(changed)
                self.campaign.write_all(results=changed)
                self.assert_whole_witness_stale(witness, text)

    def test_native_freeze_must_cover_the_inventory_exactly(self):
        witness = self.bound()
        native = self.campaign.native("e1")
        for mutate, text in ((lambda rows: rows.pop(), "request inventory"),
                             (lambda rows: rows.reverse(), "request inventory"),
                             (lambda rows: rows.append(dict(rows[0])), "duplicated row")):
            with self.subTest(text=text):
                changed = copy.deepcopy(native)
                mutate(changed["rows"])
                self.campaign.write_all(natives={"e1": changed})
                self.campaign.rewrite_reach("e1")
                self.assert_whole_witness_stale(witness, text)

    def test_native_freeze_covers_only_the_selected_inventory_rows(self):
        # The syntax oracle freezes the schedule rows that load; the others
        # are not rows of the freeze, and a freeze that adds or drops one does
        # not match.
        witness = self.bound()
        inventory = {"version": 1, "requests": [
            {"id": row, "request_sha256": request(row), "load": "loaded"} for row in self.campaign.rows]
            + [{"id": "skipped", "request_sha256": request("skipped"), "load": "not_loaded"}]}
        self.campaign.write("inventory/e1.json", json.dumps(inventory).encode())
        self.assertEqual(self.campaign.state(witness)["state"], "stale")
        with patch.dict(scope.MUTATION_ORACLES["e1"], select=("load", "loaded")):
            self.assertEqual(self.campaign.state(witness)["operations"], sorted(ALL_OPS))
            inventory["requests"][0]["load"] = "not_loaded"
            self.campaign.write("inventory/e1.json", json.dumps(inventory).encode())
            self.assertIn("request inventory", self.campaign.state(witness)["reason"])


# The real oracle table, captured before any fixture patches it.
TABLE = copy.deepcopy(scope.MUTATION_ORACLES)


class OracleTableTests(unittest.TestCase):
    def test_every_oracle_names_a_committed_inventory_of_unique_rows(self):
        self.assertEqual(sorted(TABLE), ["binder", "e1", "facts", "syntax", "table"])
        for oracle, spec in TABLE.items():
            with self.subTest(oracle=oracle):
                inventory = json.loads((ROOT / spec["inventory"]).read_text())
                rows = [row for row in inventory[spec["rows"]] if spec["select"] is None
                        or row.get(spec["select"][0]) == spec["select"][1]]
                self.assertTrue(rows)
                self.assertTrue(all(isinstance(row.get(spec["request"]), str) for row in rows))
                self.assertEqual(len({row["id"] for row in rows}), len(rows))
        # The syntax oracle's rows are the schedule rows that load, in order:
        # the complete program inventory the syntax producer replays.
        schedule = json.loads((ROOT / "data/phase1/syntax-schedule.json").read_text())
        self.assertEqual([row["id"] for row in schedule["rows"] if row["load"] == "loaded"],
                         json.loads((ROOT / "data/phase1/syntax-cases.json").read_text()))
        self.assertEqual(TABLE["facts"], TABLE["e1"])
        # The table inventory names each row's column; the column rules read it.
        self.assertEqual(TABLE["table"]["column"], "column")
        inventory = json.loads((ROOT / TABLE["table"]["inventory"]).read_text())
        self.assertTrue(all(isinstance(row.get("column"), str) for row in inventory["requests"]))
        self.assertEqual(scope.MUTATION_COLUMN_ORACLES, ("table",))
        self.assertFalse(hasattr(scope, "MUTATION_ONE_HOME_ORACLES"), "rule 7 follows the operation, not the oracle")


class TableRuleTests(MutationFixture):
    """The operation-table rules, with the fixture's e1 oracle standing in for the table oracle.

    Column parity: a kill on a row of column C credits only while every
    inventory row of C matched native in the campaign's base trace, as the
    results record it. One home: no home of a claimed operation is excused.
    """

    COLUMNS = {"r0": "c0", "r1": "c1", "r2": "c2", "r3": "c3"}

    def setUp(self):
        super().setUp()
        for patcher in (patch.dict(scope.MUTATION_ORACLES["e1"], column="column"),
                        patch.object(scope, "MUTATION_COLUMN_ORACLES", ("e1",))):
            patcher.start()
            self.addCleanup(patcher.stop)

    def columns(self, parity=None, inventory=None, claims=None):
        """A campaign of the columns: specs whose columns claim `claims` (default: nothing), the column
        inventory and native freeze selected from them, and results carrying `parity` (default: every
        column at parity) with the claims of the inventory's columns as their one-home operations."""
        column_of = self.COLUMNS if inventory is None else inventory
        names = sorted(set(column_of.values()) - {None})
        claims = {column: [] for column in names} if claims is None else claims
        specs = self.campaign.write_specs(claims)
        self.campaign.native_headers["e1"] = {"request_inventory": {"specs": specs}}
        self.campaign.write_all()
        self.select(specs, column_of)
        if parity is None:
            parity = {column: {"rows": 1, "base_match": 1, "mismatched": []} for column in names}
        results = self.campaign.results()
        results["columns"] = {"e1": parity}
        results["one_home_operations"] = sorted({op for column in names for op in claims.get(column, ())})
        self.campaign.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(results).encode(), mtime=0))

    def select(self, specs, column_of=None):
        """The column inventory, as selected from `specs`."""
        column_of = self.COLUMNS if column_of is None else column_of
        self.campaign.write("inventory/e1.json", json.dumps({"version": 1, "specs": specs, "requests": [
            {"id": row, "request_sha256": request(row), "column": column_of[row]} for row in self.campaign.rows]}).encode())

    def test_a_kill_credits_only_on_a_column_at_parity(self):
        self.columns()
        witness = self.bound()
        broken = {column: {"rows": 1, "base_match": 1, "mismatched": []} for column in ("c0", "c2", "c3")}
        broken["c1"] = {"rows": 1, "base_match": 0, "mismatched": ["r1"]}
        self.columns(broken)
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(state["state"], "bound", state)
        # kS (OP_B) and kF (OP_F) were killed on r1, the only row of c1.
        self.assertEqual(sorted(state["stale_operations"]), sorted([OP_B, OP_F]))
        self.assertIn("column parity", state["stale_operations"][OP_B])
        self.assertEqual(state["operations"], sorted({OP_A, OP_D, OP_ARM, OP_E}))
        with self.assertRaisesRegex(ValueError, "column parity"):
            self.campaign.record(self.campaign.declaration([OP_B], ["kS"]))

    def test_parity_must_cover_the_inventory_rows_of_the_column(self):
        self.columns()
        witness = self.bound()
        # Two inventory rows of c1, the results count one: the parity is of another inventory.
        self.columns(inventory={**self.COLUMNS, "r2": "c1"})
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertIn("the results record 1 rows of column c1, the inventory has 2",
                      state["stale_operations"][OP_B])
        # No recorded parity at all: nothing on the column credits.
        self.columns({})
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(state["operations"], [])
        self.assertIn("record no parity of column c0", state["stale_operations"][OP_A])

    def test_a_row_without_a_column_never_credits(self):
        self.columns(inventory={**self.COLUMNS, "r0": None})
        with self.assertRaisesRegex(ValueError, "names no column"):
            self.campaign.record(self.campaign.declaration([OP_A], ["kA"]))

    def one_home(self, operations, columns=True):
        """Rewrite the results with `operations` as the table columns' claims."""
        results = self.campaign.results()
        if columns:
            results["columns"] = {"e1": {column: {"rows": 1, "base_match": 1, "mismatched": []}
                                         for column in sorted(set(self.COLUMNS.values()))}}
        if operations is not None:
            results["one_home_operations"] = operations
        self.campaign.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(results).encode(), mtime=0))

    def test_one_home_follows_the_column_claims_not_the_oracle(self):
        """Rule 7 binds exactly the operations the results say a column claims.

        OP_D's second home (d_two) and OP_E's statement home are unreached
        copies. Only OP_D is column-claimed: its copy is never excused, while
        OP_E, which this column oracle's witness credits through a callee,
        keeps the ordinary excusal (the results do the same).
        """
        self.columns()
        witness = self.bound()
        self.columns(claims={"c0": [OP_D], "c1": [], "c2": [], "c3": []})
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(sorted(state["stale_operations"]), [OP_D])
        self.assertIn("has one home", state["stale_operations"][OP_D])
        self.assertEqual(state["operations"], sorted({OP_A, OP_B, OP_ARM, OP_E, OP_F}))
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_D, declared["operations"])
        self.assertIn(OP_E, declared["operations"])
        self.assertIn("has one home", unclaimed[OP_D])
        self.assertEqual(set(declared["not_claimed"]), {home for home in excuses(self.campaign) if "helper" in home})

    def campaign_from(self, frozen, selected, one_home):
        """Rewrite the native freeze as taken from `frozen` specs, the inventory as selected from
        `selected` ones, and results over them whose rule-7 set is `one_home`."""
        self.campaign.native_headers["e1"] = {"request_inventory": {"specs": frozen}}
        self.campaign.write_all()
        self.select(selected)
        results = self.campaign.results()
        results["columns"] = {"e1": {column: {"rows": 1, "base_match": 1, "mismatched": []}
                                     for column in sorted(set(self.COLUMNS.values()))}}
        results["one_home_operations"] = one_home
        self.campaign.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(results).encode(), mtime=0))

    def test_a_spec_edit_stales_the_column_witness_until_the_campaign_reruns(self):
        """The column witness is bound to the specs its inventory was selected from."""
        self.columns()
        witness = self.bound()
        # A column now claims OP_D, whose d_two copy the recorded campaign excused.
        claims = {"c0": [OP_D], "c1": [], "c2": [], "c3": []}
        edited = self.campaign.write_specs(claims)
        self.assert_whole_witness_stale(witness, "selected from other table specs")
        # Re-selected and re-frozen, but rule 7 was applied to the claims before the edit.
        self.campaign_from(edited, edited, [])
        self.assert_whole_witness_stale(witness, "one-home operations are not what the current table specs claim")
        # A freeze taken from another selection than the committed inventory's.
        self.campaign_from({}, edited, [OP_D])
        self.assert_whole_witness_stale(witness, "frozen from another selection")
        # The rerun campaign binds again, with OP_D's copy no longer excused.
        self.campaign_from(edited, edited, [OP_D])
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(state["state"], "bound", state)
        self.assertIn("has one home", state["stale_operations"][OP_D])
        # A spec edit that leaves every claim alone still stales it.
        self.campaign.write_specs(claims, {"notes": "edited"})
        self.assert_whole_witness_stale(witness, "selected from other table specs")

    def test_one_home_results_are_well_formed(self):
        self.columns()
        witness = self.bound()
        for value, text in ((["b", "a"], "one-home operations malformed"), ([1], "one-home operations malformed"),
                            (None, "column parity but no one-home operations")):
            with self.subTest(value=value):
                self.one_home(value)
                state = self.campaign.state(self.campaign.reforged(witness))
                self.assertEqual(state["state"], "stale", state)
                self.assertIn(text, state["reason"])


class OneHomeAcrossWitnessesTests(MutationFixture):
    """A column-claimed operation has one home in every witness that claims it."""

    def test_another_oracles_witness_cannot_excuse_a_copy_of_a_column_claimed_operation(self):
        witness = self.bound()
        results = self.campaign.results()
        results["one_home_operations"] = [OP_D]
        self.campaign.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(results).encode(), mtime=0))
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(sorted(state["stale_operations"]), [OP_D])
        self.assertIn("a table column claims this operation", state["stale_operations"][OP_D])
        self.assertIn(OP_E, state["operations"])
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_D, declared["operations"])
        self.assertIn("has one home", unclaimed[OP_D])
        # Without a table campaign in the results no operation has one home.
        del results["one_home_operations"]
        self.campaign.write(scope.MUTATION_RESULTS, gzip.compress(json.dumps(results).encode(), mtime=0))
        self.assertEqual(self.campaign.state(self.campaign.reforged(witness))["operations"], sorted(ALL_OPS))

    def test_a_current_spec_claim_denies_the_excuse_whatever_the_results_say(self):
        """Rule 7 reads the current specs: a column that starts claiming an operation after the
        campaign stales the other witnesses' excused copies of it at once."""
        witness = self.bound()
        self.assertNotIn("one_home_operations", self.campaign.results())
        specs = self.campaign.write_specs({"c0": [OP_D]})
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(sorted(state["stale_operations"]), [OP_D])
        self.assertIn("a table column claims this operation", state["stale_operations"][OP_D])
        self.assertEqual(state["inputs"].keys() & specs.keys(), specs.keys(), "the witness binds the specs it read")
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_D, declared["operations"])
        self.assertIn("has one home", unclaimed[OP_D])
        # Specs that cannot be read leave rule 7 undecidable: nothing is credited.
        self.campaign.write(f"{phase1_tables.SPEC_DIRECTORY}/core.json", b"{")
        self.assert_whole_witness_stale(witness, "table spec data/phase1/tables/core.json is unreadable")
        self.campaign.write(f"{phase1_tables.SPEC_DIRECTORY}/core.json",
                            json.dumps({"columns": [{"id": "c0", "operations": []}]}).encode())
        self.assert_whole_witness_stale(witness, "malformed or duplicated column")
        (self.root / phase1_tables.SPEC_DIRECTORY / "core.json").unlink()
        self.assert_whole_witness_stale(witness, "core.json is unreadable")


class GoBindingTests(MutationFixture):
    def test_current_oracle_code_is_recomputed(self):
        witness = self.bound()
        for name, change, text in (
                ("oracle sources", lambda b: b["oracle_sources"].update({"scripts/e1_oracle/main.go": "0" * 64}),
                 "other e1 oracle sources"),
                ("instrumentation", lambda b: b.update(instrumentation_sha256="0" * 64), "other instrumentation"),
                ("stage rule", lambda b: b.update(stage_rule={"oracle": "e1", "observe": "internal/parser"},
                                                   stage_rule_sha256=canonical_digest(
                                                       {"oracle": "e1", "observe": "internal/parser"})),
                 "another stage rule")):
            with self.subTest(binding=name):
                self.campaign.bindings = {oracle: bindings(oracle) for oracle in ORACLES}
                self.assertEqual(self.campaign.state(witness)["state"], "bound")
                change(self.campaign.bindings["e1"])
                self.assert_whole_witness_stale(witness, text)

    def test_stage_rule_digest_and_function_table_are_bound(self):
        witness = self.bound()
        self.campaign.rewrite_reach("e1", stage_rule_sha256="0" * 64)
        self.assert_whole_witness_stale(witness, "another stage rule")
        # A header whose rule disagrees with its own (current) digest.
        self.campaign.write_all()
        self.campaign.rewrite_reach("e1", stage_rule={"oracle": "e1", "observe": "internal/parser"})
        self.assert_whole_witness_stale(witness, "another stage rule")
        self.campaign.write_all()
        self.campaign.write("data/go-functions.tsv", b"an edited go function table\n")
        self.assert_whole_witness_stale(witness, "go-functions.tsv")

    def test_code_that_cannot_state_its_bindings_binds_nothing(self):
        witness = self.bound()
        with patch.object(scope, "mutation_go_bindings", lambda oracle: (None, "no stage rule")):
            state = self.campaign.state(witness)
        self.assertEqual((state["state"], state["reason"]), ("stale", "no stage rule"))

    def test_unstable_operations_never_witness(self):
        witness = self.bound()
        self.campaign.headers["e1"] = {"unstable_ops": [OP_POOL, OP_A]}
        self.campaign.write_all()
        self.assert_stale(witness, OP_A, "pool or memo state")
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_A, declared["operations"])
        self.assertIn("pool or memo", unclaimed[OP_A])
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            scope.mutation_evidence(self.campaign.declaration([OP_A], ["kA"]), self.root)

    def test_reach_index_must_list_the_unstable_operations(self):
        witness = self.bound()
        for header, text in (({"unstable_ops": None}, "unstable"), ({"unstable_ops": []}, "omits unstable"),
                             ({"unstable_ops": "x"}, "unstable")):
            with self.subTest(header=header):
                self.campaign.write_all()
                self.campaign.rewrite_reach("e1", **header)
                self.assert_whole_witness_stale(witness, text)

    def test_real_code_states_the_bindings_of_every_oracle(self):
        # The go-reach header records stage_rule(oracle) and its digest, the
        # native freeze oracle_sources(oracle); the harness recomputes both
        # from phase1_mutation_go, the same functions that wrote them.
        import phase1_mutation_go as go
        with patch.object(scope, "mutation_go_bindings", TRUE_BINDINGS):
            for oracle in sorted(TABLE):
                with self.subTest(oracle=oracle):
                    stated, reason = scope.mutation_go_bindings(oracle)
                    self.assertIsNone(reason)
                    self.assertEqual(stated["stage_rule"], go.stage_rule(oracle))
                    self.assertEqual(stated["stage_rule_sha256"], canonical_digest(stated["stage_rule"]))
                    self.assertEqual(stated["oracle_sources"], go.oracle_sources(oracle))
                    self.assertEqual(stated["instrumentation_sha256"], go.instrumentation_digest(oracle))
                    self.assertIn("tsc/internal/scanner/scanner.go:cleared", stated["unstable_ops"])
                    self.assertEqual(go.oracle_spec(oracle).inventory, TABLE[oracle]["inventory"])


TRUE_BINDINGS = scope.mutation_go_bindings


class DecoderBindingTests(MutationFixture):
    def test_a_decoder_change_stales_the_reach_index_through_the_real_binding(self):
        # GR2: the stage rule the harness recomputes from phase1_mutation_go
        # carries DECODER_VERSION and the digest of the decoder's own source,
        # so a decoder fix without a reach rerun stales the old index.
        import phase1_mutation_go as go
        with patch.object(scope, "mutation_go_bindings", TRUE_BINDINGS):
            stated, reason = scope.mutation_go_bindings("e1")
        self.assertIsNone(reason)
        rule = json.dumps(stated["stage_rule"], sort_keys=True)
        self.assertIn(go.decoder_digest(), rule)
        self.assertIn(f'"version": {go.DECODER_VERSION}', rule)
        # The fixture's e1 freeze and index as the real current code states them.
        self.campaign.bindings["e1"] = copy.deepcopy(stated)
        self.campaign.write_all()
        with patch.object(scope, "mutation_go_bindings", TRUE_BINDINGS):
            witness = self.bound()
        text = Path(go.__file__).read_text(encoding="utf-8")
        self.assertEqual(text.count("\ndef attach_ids("), 1)
        edited = text.replace("\ndef attach_ids(", "\ndef attach_ids(  ", 1)
        self.assertNotEqual(go._decoder_digest(edited), go.decoder_digest())
        edits = {"version": patch.object(go, "DECODER_VERSION", go.DECODER_VERSION + 1),
                 "source": patch.object(go, "decoder_digest", lambda text=None: go._decoder_digest(edited))}
        for name, edit in edits.items():
            with self.subTest(decoder=name), edit, patch.object(scope, "mutation_go_bindings", TRUE_BINDINGS):
                self.assert_whole_witness_stale(witness, "another stage rule")
        with patch.object(scope, "mutation_go_bindings", TRUE_BINDINGS):
            self.assertEqual(self.campaign.state(witness)["operations"], sorted(ALL_OPS))


class KillRuleTests(MutationFixture):
    def set_kill(self, key: str, oracle: str = "e1", row: str | None = None, **fields):
        row = row or self.campaign.runs[key][oracle][1][0]
        self.campaign.kill_fields[(key, oracle, row)] = fields
        self.campaign.write_all()

    def test_survived_crash_timeout_and_build_failed_never_link(self):
        witness = self.bound()
        for state in ("survived", "crash", "timeout", "build_failed", "not_reached", "unsupported"):
            with self.subTest(state=state):
                self.campaign.runs["kA"] = {oracle: (state, [], 1) for oracle in ORACLES}
                self.campaign.write_all()
                with self.assertRaisesRegex(ValueError, "only a killed"):
                    scope.mutation_evidence(self.campaign.declaration([OP_A], ["kA"]), self.root)
                self.assert_stale(witness, OP_A, "not killed")

    def test_row_outside_go_reach_is_rejected(self):
        witness = self.bound()
        self.campaign.reach["e1"][OP_A] = [1]
        self.campaign.write_all()
        self.assert_stale(witness, OP_A, "was not entered")

    def test_row_outside_rust_reach_is_rejected(self):
        witness = self.bound()
        self.set_kill("kA", reach={"rust": False, "go": [OP_A]})
        self.assert_stale(witness, OP_A, "mutated site executed")

    def test_base_that_does_not_match_native_is_rejected(self):
        witness = self.bound()
        self.set_kill("kA", base=dict(digests("e1", "r0"), parse="0" * 64))
        self.assert_stale(witness, OP_A, "does not match native")

    def test_crash_and_no_difference_are_not_kills(self):
        witness = self.bound()
        partial = {stage: value for stage, value in self.campaign.kill("e1", "r0", "kA")["mutant"].items()
                   if stage == "parse"}
        for fields, text in (({"mutant": partial}, "did not complete"),
                             ({"mutant": dict(digests("e1", "r0"), parse=None)}, "did not complete"),
                             ({"mutant": digests("e1", "r0"), "stages": []}, "no compared stage differs"),
                             ({"stages": ["encode_source_file"]}, "disagree")):
            with self.subTest(text=text):
                self.set_kill("kA", **fields)
                self.assert_stale(witness, OP_A, text)

    def test_native_row_that_did_not_complete_cannot_witness(self):
        witness = self.bound()
        native = self.campaign.native("e1")
        native["rows"][0]["outcomes"]["encode_source_file"] = "panic"
        self.campaign.write_all(natives={"e1": native})
        self.assert_stale(witness, OP_A, "did not complete every stage")

    def test_mutant_outside_an_annotated_home_is_rejected(self):
        self.campaign.reach["e1"][OP_X] = [0]
        self.campaign.write_all()
        declaration = self.campaign.declaration([OP_X], ["kA"])
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            scope.mutation_evidence(declaration, self.root)
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_X, "kA", "r0")]))
        self.assertIn("not an annotated Rust home", state["stale_operations"][OP_X])
        self.assertEqual(state["operations"], [])
        committed = self.campaign.state(self.campaign.forge(declaration, [(OP_X, "kA", "r0")]),
                                        committed_scope={"operations": [{"id": OP_X, "cases": ["mutation/e1"]}]})
        self.assertIn("not an annotated Rust home", committed["stale_operations"][OP_X])

    def test_shared_function_credits_only_operations_entered_on_the_kill_row(self):
        witness, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertIn(OP_B, witness["operations"])
        self.assertNotIn(OP_C, witness["operations"])
        self.assertIn("not entered", unclaimed[OP_C])
        both = self.campaign.declaration([OP_B, OP_C], ["kS"])
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            scope.mutation_evidence(both, self.root)
        state = self.campaign.state(self.campaign.forge(both, [(OP_B, "kS", "r1"), (OP_C, "kS", "r1")]))
        self.assertEqual(state["operations"], [OP_B])
        self.assertIn("was not entered", state["stale_operations"][OP_C])
        self.campaign.reach["e1"][OP_C] = [1, 2]
        self.campaign.write_all()
        recorded = self.campaign.record(both)
        self.assertEqual(self.campaign.state(recorded)["operations"], [OP_B, OP_C])

    def test_arm_site_credits_only_the_operation_its_marker_names(self):
        self.campaign.reach["e1"][OP_A] = [0, 1, 3]
        self.campaign.write_all()
        declaration = self.campaign.declaration([OP_A], ["kR"])
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_A, "kR", "r3")]))
        self.assertIn("not an annotated Rust home", state["stale_operations"][OP_A])

    def test_an_operation_with_no_recorded_kill_is_never_credited(self):
        # HR1: every home of OP_D is provably unreached and excused, so the
        # home rule alone would pass it; only the kill requirement refuses a
        # hand-edited claim of an operation no kill witnesses.
        self.campaign.runs["kD1"] = {oracle: ("not_reached", [], 0) for oracle in ORACLES}
        self.campaign.write_all()
        excused = {home(FAKE, "d_one"): UNREACHED, home(OTHER, "d_two"): UNREACHED}
        declaration = self.campaign.declaration([OP_A, OP_D], ["kA"], excused)
        self.assertEqual(scope.mutation_declaration_problems(declaration), [])
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_A, "kA", "r0")]))
        self.assertEqual(state["operations"], [OP_A], state)
        self.assertEqual(state["stale_operations"][OP_D], "no recorded kill")
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            scope.mutation_evidence(declaration, self.root)

    def test_recorded_request_digest_must_equal_the_results_kill(self):
        # HR2: the committed row and the evidence agree, but the results say
        # the kill ran another request; only this comparison links them.
        witness = self.bound(operations=(OP_A,), keys=("kA",), not_claimed={})
        self.set_kill("kA", request_sha256="f" * 64)
        self.assert_stale(witness, OP_A, "recorded request digest differs from the results")

    def test_a_kill_row_recorded_twice_is_not_a_kill(self):
        # HR3: phase1_integration refuses the duplicate in a receipt; the
        # binding refuses it on its own.
        witness = self.bound(operations=(OP_A,), keys=("kA",), not_claimed={})
        results = self.campaign.results()
        record = next(row for row in results["mutants"] if row["key"] == "kA")
        record["kills"].append(copy.deepcopy(next(kill for kill in record["kills"] if kill["oracle"] == "e1")))
        self.campaign.write_all(results=results)
        self.assert_stale(witness, OP_A, "not exactly one recorded e1 kill")

    def test_whole_program_oracle_never_credits_a_multi_operation_site(self):
        # KS4: on a whole-program row Go enters nearly every operation, so the
        # shared-function rule is vacuous there. The fixture plays the syntax
        # oracle's part with binder.
        self.assertEqual(scope.MUTATION_SINGLE_OPERATION_ORACLES, ("syntax",))
        self.campaign.runs["kS"] = {"e1": ("killed", ["r1"], 1), "binder": ("killed", ["r1"], 1)}
        self.campaign.write_all()
        declaration = self.campaign.declaration([OP_B], ["kS"], oracle="binder")
        credited = self.campaign.record(declaration)
        self.assertEqual(self.campaign.state(credited)["operations"], [OP_B])
        with patch.object(scope, "MUTATION_SINGLE_OPERATION_ORACLES", ("binder",)):
            state = self.campaign.state(credited)
            self.assertEqual(state["operations"], [])
            self.assertIn("not_credited_multi_op", state["stale_operations"][OP_B])
            with self.assertRaisesRegex(ValueError, "not_credited_multi_op"):
                scope.mutation_evidence(declaration, self.root)
            declared, unclaimed = scope.declare_mutation_witness("binder", scope.MUTATION_RESULTS, self.root)
            self.assertNotIn(OP_B, declared["operations"])
            self.assertIn("not_credited_multi_op", unclaimed[OP_B])
            # A single-operation site of the same oracle still credits, and
            # another oracle's kill of the shared site is unaffected.
            self.assertIn(OP_A, declared["operations"])
            self.assertIn(OP_B, scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)[0]["operations"])

    def test_a_kill_counts_only_for_its_own_oracle(self):
        # Row ids are shared between oracles. A binder kill on r0 is not an e1
        # kill on r0, and a row killed under both oracles is still exactly one
        # kill of each.
        witness = self.bound(operations=(OP_A,), keys=("kA",), not_claimed={})
        self.campaign.runs["kA"] = {"e1": ("killed", ["r1"], 2), "binder": ("killed", ["r0"], 2)}
        self.campaign.write_all()
        self.assert_stale(witness, OP_A, "not exactly one recorded e1 kill")
        self.campaign.runs["kA"] = {"e1": ("killed", ["r0"], 2), "binder": ("killed", ["r0"], 2)}
        self.campaign.write_all()
        self.assertEqual(self.campaign.state(self.campaign.reforged(witness))["operations"], [OP_A])


class ControlTests(MutationFixture):
    def set_kill(self, **fields):
        self.campaign.kill_fields[("kF", "e1", "r1")] = fields
        self.campaign.write_all()

    def replace_mutant(self, key: str, **fields):
        index = next(i for i, mutant in enumerate(self.campaign.mutants) if mutant["key"] == key)
        self.campaign.mutants[index] = {**self.campaign.mutants[index], **fields}
        self.campaign.write_all()

    def test_allocating_mutant_is_credited_when_it_differs_from_its_control(self):
        witness = self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
        self.assertEqual(witness["mutants"][0]["control"], 9)
        self.assertEqual(self.campaign.state(witness)["operations"], [OP_F])

    def test_mutant_equal_to_its_control_is_not_a_kill(self):
        # K1: the allocated missing node moves the counters on its own; a
        # mutant that differs from native only that way is not a kill.
        witness = self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
        mutated = self.campaign.kill("e1", "r1", "kF")["mutant"]
        self.set_kill(control=dict(mutated))
        self.assert_stale(witness, OP_F, "equals its control")
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            scope.mutation_evidence(self.campaign.declaration([OP_F], ["kF"]), self.root)
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_F, declared["operations"])
        self.assertIn("equals its control", unclaimed[OP_F])

    def test_the_control_must_differ_where_the_mutant_differs_from_native(self):
        # KS2: the mutant differs from native only in parse, where it equals
        # its control; the control also moved node_index_before, where the
        # mutant equals native. The mutant differs from the control, but only
        # through the control's own side effect: not a kill.
        witness = self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
        native = digests("e1", "r1")
        mutated = dict(native, parse=sha(b"allocated"))
        self.set_kill(mutant=mutated, stages=["parse"],
                      control=dict(native, parse=sha(b"allocated"), node_index_before=sha(b"control only")))
        self.assert_stale(witness, OP_F, "equals its control in every stage where it differs from native")
        # A difference from the control in a stage that also differs from
        # native is a kill, whatever the control does elsewhere.
        both = dict(native, parse=sha(b"allocated"), encode_source_file=sha(b"replaced value"))
        self.set_kill(mutant=both, stages=["encode_source_file", "parse"],
                      control=dict(native, parse=sha(b"allocated"), node_index_before=sha(b"control only")))
        self.assertEqual(self.campaign.state(self.campaign.reforged(witness))["operations"], [OP_F])

    def test_kill_without_a_completed_control_run_is_not_a_kill(self):
        witness = self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
        partial = {"parse": sha(b"control only parse")}
        for control in (None, partial, dict(digests("e1", "r1"), parse=None)):
            with self.subTest(control=control):
                self.set_kill(control=control)
                self.assert_stale(witness, OP_F, "no completed run of the paired control")

    def test_allocating_mutant_without_a_planned_control_is_never_credited(self):
        self.replace_mutant("kF", control=None)
        self.campaign.mutants = [mutant for mutant in self.campaign.mutants if mutant["key"] != "kFc"]
        self.campaign.write_all()
        declaration = self.campaign.declaration([OP_F], ["kF"])
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            scope.mutation_evidence(declaration, self.root)
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_F, "kF", "r1")]))
        self.assertIn("pairs it with no control", state["stale_operations"][OP_F])

    def test_control_must_be_planned_at_the_same_site_for_this_mutant(self):
        for fields, text in (({"control_of": 1}, "not a control planned at the same site"),
                             ({"site_line": 2}, "not a control planned at the same site"),
                             ({"span_sha256": "0" * 64}, "not a control planned at the same site")):
            with self.subTest(fields=fields):
                self.campaign = Campaign(self.root)
                witness = self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
                self.replace_mutant("kFc", **fields)
                self.assert_stale(witness, OP_F, text)
        self.campaign = Campaign(self.root)
        witness = self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
        self.replace_mutant("kF", control=5)
        self.assert_whole_witness_stale(witness, "kF differs from the manifest")

    def test_controls_are_never_claimed_and_never_make_a_campaign_incomplete(self):
        declaration = self.campaign.declaration([OP_F], ["kFc"])
        self.assertTrue(any("is a control" in problem for problem in scope.mutation_declaration_problems(declaration)))
        # A campaign whose results omit the control, or record it with a state
        # of its own, is complete.
        self.bound(operations=(OP_F,), keys=("kF",), not_claimed={})
        results = self.campaign.results()
        results["mutants"].append({"id": 9, "key": "kFc", "state": "control", "kills": [], "oracles": {}})
        self.campaign.write_all(results=results)
        self.assertEqual(scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)[0]["id"],
                         "mutation/e1")
        witness = self.campaign.record(self.campaign.declaration([OP_F], ["kF"]))
        self.assertEqual(self.campaign.state(witness)["operations"], [OP_F])

    def test_a_control_recorded_as_killed_still_confers_nothing(self):
        # Even if a campaign recorded a control as a killed mutant with a kill
        # row, the kill rule refuses it before looking at the row.
        results = self.campaign.results()
        kill = self.campaign.kill("e1", "r1", "kF")
        results["mutants"].append({"id": 9, "key": "kFc", "op": OP_F, "ops": [OP_F], "state": "killed",
                                   "kills": [kill], "oracles": {"e1": {"state": "killed", "rust_rows": 1}}})
        self.campaign.write_all(results=results)
        context, reason = scope.mutation_artifacts("e1", scope.MUTATION_RESULTS, self.root)
        self.assertIsNone(reason)
        control = {field: context["manifest"]["kFc"].get(field) for field in scope.MUTANT_FIELDS}
        problem = scope.mutation_kill_problem(context, None, OP_F, dict(control, operator="control"),
                                              {"row": "r1", "request_sha256": request("r1")})
        self.assertIn("controls are never credited", problem)

    def test_allocation_is_read_from_the_operator_and_the_inserted_text(self):
        self.assertTrue(scope.allocates({"operator": ALLOCATING}))
        self.assertTrue(scope.allocates({"operator": "wrap_result:Some(self.create_missing_list())"}))
        self.assertTrue(scope.allocates({"operator": "return:x", "insert": [{"text": "return self.new_identifier(k);"}]}))
        for operator in ("return:None", "return:true", "wrap_result:SyntaxKind::Unknown", "return:Vec::new()",
                         "skip_body", "param:list=None"):
            with self.subTest(operator=operator):
                self.assertFalse(scope.allocates({"operator": operator, "insert": [{"text": "if hit(1) { return; }"}]}))


class HomeRuleTests(MutationFixture):
    def stmt_home(self) -> str:
        stmt = next(mutant for mutant in self.campaign.mutants if mutant["key"] == "kEs")
        return home(OTHER, "helper", "stmt", stmt["site_line"])

    def test_every_reached_home_must_be_killed(self):
        self.bound(operations=(OP_D,), keys=("kD1",), not_claimed={home(OTHER, "d_two"): UNREACHED})
        declaration = self.campaign.declaration([OP_D], ["kD1"], {})
        with self.assertRaisesRegex(ValueError, "neither killed nor listed"):
            scope.mutation_evidence(declaration, self.root)
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED})
        for state, rust_rows in (("survived", 3), ("not_reached", 3)):
            # Rust reach on rows where Go never entered the operation still
            # reaches the home: it is not excused as unreached.
            with self.subTest(state=state):
                self.campaign.runs["kD2"] = {"e1": (state, [], rust_rows), "binder": ("not_reached", [], 0)}
                self.campaign.write_all()
                with self.assertRaisesRegex(ValueError, "reached by a driver but not killed"):
                    scope.mutation_evidence(declaration, self.root)

    def test_statement_and_arm_homes_are_homes(self):
        # K2: a statement home of OP_E exists beside its function home.
        declaration = self.campaign.declaration([OP_E], ["kE"], {})
        with self.assertRaisesRegex(ValueError, "neither killed nor listed"):
            scope.mutation_evidence(declaration, self.root)
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_E, "kE", "r2")]))
        self.assertIn(self.stmt_home(), state["stale_operations"][OP_E])
        self.bound(operations=(OP_E,), keys=("kE",), not_claimed={self.stmt_home(): UNREACHED})
        self.campaign.runs["kEs"] = {"e1": ("not_reached", [], 0), "binder": ("survived", [], 2)}
        self.campaign.write_all()
        with self.assertRaisesRegex(ValueError, "reached by a driver"):
            scope.mutation_evidence(self.campaign.declaration([OP_E], ["kE"], {self.stmt_home(): UNREACHED}), self.root)

    def test_home_without_a_mutant_cannot_be_excused(self):
        # H1: a reviewer's reason cannot excuse a home whose reach nobody measured.
        self.campaign.mutants = [mutant for mutant in self.campaign.mutants if mutant["key"] != "kD2"]
        del self.campaign.runs["kD2"]
        manifest = self.campaign.manifest()
        manifest["homes"][OP_D].append({"file": OTHER, "function": "d_two", "site_kind": "fn", "site_line": 2,
                                        "span_sha256": "0" * 64, "mutants": []})
        self.campaign.write(scope.MUTATION_MANIFEST, json.dumps(manifest).encode())
        self.campaign.rebind()
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED})
        with self.assertRaisesRegex(ValueError, "has no mutant"):
            scope.mutation_evidence(declaration, self.root)
        state = self.campaign.state(self.campaign.forge(declaration, [(OP_D, "kD1", "r0")]))
        self.assertIn("has no mutant", state["stale_operations"][OP_D])
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_D, declared["operations"])
        self.assertIn("has no mutant", unclaimed[OP_D])

    def test_reach_measured_by_only_some_oracles_is_unknown(self):
        # H6: proof of non-reach needs every traced oracle. kD2 ran only in
        # the e1 campaign although the results also trace binder: the
        # campaign is partial. With the binder run recorded but its reach
        # table entry missing, the home's reach is unknown.
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED})
        self.campaign.runs["kD2"] = {"e1": ("not_reached", [], 0)}
        self.campaign.write_all()
        with self.assertRaisesRegex(ValueError, "not run to a final state in every traced oracle"):
            scope.mutation_evidence(declaration, self.root)
        self.campaign.runs["kD2"] = {oracle: ("not_reached", [], 0) for oracle in ORACLES}
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kD2")["reach"].pop("binder")
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "not measured in every traced oracle"):
            scope.mutation_evidence(declaration, self.root)
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertIn("not measured in every traced oracle", unclaimed[OP_D])

    def test_only_a_full_reach_table_proves_non_reach(self):
        # H3/H6: eligible-row counts are not proof; the full-trace reach table
        # of every traced oracle is, and a positive count anywhere is reach.
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED})
        for change, text in ((lambda record: record["reach"].pop("binder"), "not measured in every traced oracle"),
                             (lambda record: record.pop("reach"), "not measured in every traced oracle"),
                             (lambda record: record["reach"].update(binder=[0]), "not measured in every traced oracle"),
                             (lambda record: record["reach"].update(binder=[0, 2]), "reached by a driver"),
                             (lambda record: record["oracles"]["e1"].update(rust_rows=1), "reached by a driver")):
            with self.subTest(text=text):
                results = self.campaign.results()
                change(next(row for row in results["mutants"] if row["key"] == "kD2"))
                self.campaign.write_all(results=results)
                with self.assertRaisesRegex(ValueError, text):
                    scope.mutation_evidence(declaration, self.root)
        # An oracle the results say was traced but that ran nothing leaves
        # every unkilled mutant unrun there: the campaign is partial.
        results = self.campaign.results()
        results["inputs"]["traced_oracles"] = ["binder", "e1", "facts"]
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "not run to a final state in every traced oracle"):
            scope.mutation_evidence(declaration, self.root)

    def test_observation_reach_is_reach(self):
        # KS3: a site executed only while observing (the e1 encoder, a graph
        # dump) is reached; its home cannot be excused as unreached.
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED})
        witness = self.campaign.record(declaration)
        self.campaign.observe[("kD2", "binder")] = 5
        self.campaign.write_all()
        with self.assertRaisesRegex(ValueError, "reached by a driver but not killed"):
            scope.mutation_evidence(declaration, self.root)
        self.assert_stale(witness, OP_D, "reached by a driver but not killed")
        declared, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_D, declared["operations"])
        self.assertIn("reached by a driver", unclaimed[OP_D])
        # The per-oracle run record's observation count is reach too.
        self.campaign.observe.clear()
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kD2")["oracles"]["e1"]["observe_rows"] = 2
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "reached by a driver but not killed"):
            scope.mutation_evidence(declaration, self.root)

    def test_a_state_that_ran_candidate_rows_is_reach_whatever_the_table_says(self):
        # A mutant recorded as not_credited_multi_op or budget had candidate
        # rows, so its site executed; a reach table that says otherwise does
        # not excuse its home.
        witness = self.campaign.record(self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED}))
        for state in ("not_credited_multi_op", "budget", "survived"):
            with self.subTest(state=state):
                results = self.campaign.results()
                next(row for row in results["mutants"] if row["key"] == "kD2")["oracles"]["binder"]["state"] = state
                self.campaign.write_all(results=results)
                self.assert_stale(witness, OP_D, "reached by a driver but not killed")

    def test_production_only_reach_never_proves_a_home_unreached(self):
        # KS3: a two-count entry (round two's [eligible, all]) measured
        # production stages only; it shows reach but cannot prove its absence.
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): UNREACHED})
        scope.mutation_evidence(declaration, self.root)
        for oracle in ORACLES:
            with self.subTest(oracle=oracle):
                results = self.campaign.results()
                next(row for row in results["mutants"] if row["key"] == "kD2")["reach"][oracle] = [0, 0]
                self.campaign.write_all(results=results)
                with self.assertRaisesRegex(ValueError, "not measured in every traced oracle"):
                    scope.mutation_evidence(declaration, self.root)
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kD2")["reach"]["binder"] = [0, 1]
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "reached by a driver"):
            scope.mutation_evidence(declaration, self.root)

    def test_excuse_must_say_no_production_or_observation_reach(self):
        # KS3: round two's "no Rust reach" text overclaimed; the excuse names
        # both kinds of stage and every traced oracle.
        for reason in ("no Rust reach in the e1 and binder campaigns",
                       "no traced Phase 1 oracle (e1, binder) executes this Rust home"):
            with self.subTest(reason=reason):
                declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): reason})
                with self.assertRaisesRegex(ValueError, "does not say 'no production or observation reach on'"):
                    scope.mutation_evidence(declaration, self.root)
        self.assertIn(scope.MUTATION_UNREACHED_PHRASE + " binder, e1:",
                      scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)[0]["not_claimed"][
                          home(OTHER, "d_two")])

    def test_excuse_must_name_every_traced_oracle(self):
        declaration = self.campaign.declaration([OP_D], ["kD1"], {home(OTHER, "d_two"): "no e1 row reaches it"})
        with self.assertRaisesRegex(ValueError, "does not name every traced oracle"):
            scope.mutation_evidence(declaration, self.root)

    def test_declaration_lists_only_proven_unreached_homes(self):
        witness, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertEqual(witness["operations"], sorted(ALL_OPS))
        self.assertEqual(set(witness["not_claimed"]), {home(OTHER, "d_two"), self.stmt_home()})
        for reason in witness["not_claimed"].values():
            self.assertTrue(reason.startswith("no production or observation reach on binder, e1:"), reason)
        self.assertEqual([mutant["key"] for mutant in witness["mutants"]], sorted(ALL_KEYS))
        self.assertIn(OP_C, unclaimed)
        self.campaign.runs["kD2"] = {"e1": ("build_failed", [], 0)}
        self.campaign.write_all()
        with self.assertRaisesRegex(ValueError, "not run to a final state in every traced oracle"):
            scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.campaign.runs["kD2"] = {oracle: ("build_failed", [], 0) for oracle in ORACLES}
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kD2")["reach"]["binder"] = None
        self.campaign.write_all(results=results)
        witness, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertNotIn(OP_D, witness["operations"])
        self.assertIn("not measured in every traced oracle", unclaimed[OP_D])
        self.campaign.runs["kD2"] = {"e1": ("not_reached", [], 0), "binder": ("not_reached", [], 4)}
        self.campaign.write_all()
        witness, unclaimed = scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)
        self.assertIn("reached by a driver", unclaimed[OP_D])

    def test_a_new_home_returns_the_operation_to_pending(self):
        witness = self.bound()
        third = OTHER_SOURCE + f"\n/// port: {OP_A}\npub fn a_again() -> u32 {{\n    0\n}}\n"
        self.campaign.write_sources(FAKE_SOURCE, third)
        state = self.campaign.state(witness)
        self.assertNotIn(OP_A, state["operations"])
        self.assertIn("a_again", state["stale_operations"][OP_A])
        statement = OTHER_SOURCE + f"\npub fn b_again(x: u32) -> u32 {{\n    // port: {OP_B}\n    x\n}}\n"
        self.campaign.write_sources(FAKE_SOURCE, statement)
        state = self.campaign.state(witness)
        self.assertNotIn(OP_B, state["operations"])
        self.assertIn("arm or statement marker", state["stale_operations"][OP_B])
        self.assertIn(OP_A, state["operations"])

    def test_excused_home_edit_returns_the_operation_to_pending(self):
        witness = self.bound()
        self.campaign.write_sources(FAKE_SOURCE, OTHER_SOURCE.replace("x - 2", "x - 3"))
        state = self.campaign.state(witness)
        self.assertNotIn(OP_D, state["operations"])
        self.assertIn("excused Rust home", state["stale_operations"][OP_D])
        self.campaign.write_sources(FAKE_SOURCE, OTHER_SOURCE.replace("let y = x + 3;", "let y = x + 4;"))
        self.assertIn("excused Rust home", self.campaign.state(witness)["stale_operations"][OP_E])

    def test_manifest_without_homes_credits_nothing(self):
        witness = self.bound()
        self.campaign.homes = False
        self.campaign.write_all()
        state = self.campaign.state(self.campaign.reforged(witness))
        self.assertEqual(state["operations"], [])
        self.assertIn("records no Rust homes", state["stale_operations"][OP_A])
        with self.assertRaisesRegex(ValueError, "records no Rust homes"):
            scope.declare_mutation_witness("e1", scope.MUTATION_RESULTS, self.root)

    def test_two_homes_under_one_identity_keep_the_operation_pending(self):
        witness = self.bound()
        manifest = self.campaign.manifest()
        twin = dict(manifest["homes"][OP_D][0], site_line=manifest["homes"][OP_D][0]["site_line"] + 100, mutants=[])
        manifest["homes"][OP_D].append(twin)
        self.campaign.write(scope.MUTATION_MANIFEST, json.dumps(manifest).encode())
        self.campaign.rebind()
        self.assert_stale(witness, OP_D, "under one identity")

    def test_malformed_homes_stale_the_witness(self):
        witness = self.bound()
        for change, text in ((lambda homes: homes[OP_A][0]["mutants"].append("kZ"), "not a planned mutant"),
                             (lambda homes: homes[OP_A][0]["mutants"].append("kFc"), "not a planned mutant"),
                             (lambda homes: homes[OP_A][0].pop("span_sha256"), "malformed home"),
                             (lambda homes: homes[OP_A].append(dict(homes[OP_A][0])), "two homes")):
            with self.subTest(text=text):
                manifest = self.campaign.manifest()
                change(manifest["homes"])
                self.campaign.write(scope.MUTATION_MANIFEST, json.dumps(manifest).encode())
                self.campaign.rebind()
                self.assert_whole_witness_stale(witness, text)


class CommittedViewTests(MutationFixture):
    def linked(self, operations, witness="mutation/e1"):
        return {"operations": [{"id": op, "cases": [witness] if op in operations else []}
                               for op in (*ALL_OPS, OP_C, OP_X)]}

    def test_committed_view_reads_no_rust_source(self):
        witness = self.bound()
        with patch.object(scope, "mutation_sources", side_effect=AssertionError("live source read")):
            state = self.campaign.state(witness, committed_scope=self.linked(ALL_OPS))
        self.assertEqual(state["operations"], sorted(ALL_OPS))

    def test_committed_view_credits_only_what_the_committed_scope_links(self):
        witness = self.bound()
        state = self.campaign.state(witness, committed_scope=self.linked((OP_A, OP_B)))
        self.assertEqual(state["operations"], [OP_A, OP_B])
        self.assertIn("committed scope.json does not link it", state["stale_operations"][OP_D])

    def test_span_edit_changes_only_the_live_view(self):
        witness = self.bound()
        self.campaign.write_sources(FAKE_SOURCE.replace("x + 1", "x + 2"), OTHER_SOURCE)
        self.assertNotIn(OP_A, self.campaign.state(witness)["operations"])
        self.assertIn(OP_A, self.campaign.state(witness, committed_scope=self.linked(ALL_OPS))["operations"])

    def test_artifact_bindings_hold_in_both_views(self):
        witness = self.bound()
        self.campaign.write_all(results={**self.campaign.results(), "note": "rerun"})
        self.assertEqual(self.campaign.state(witness, committed_scope=self.linked(ALL_OPS))["state"], "stale")

    def test_rosters_and_preparation_use_the_committed_view(self):
        witness = self.bound()
        cases = {"cases": [], "witnesses": [witness]}
        real = scope.recorded_mutations
        with patch.object(scope, "recorded_mutations",
                          lambda cases, root=None, **kwargs: real(cases, self.root, **kwargs)), \
                patch.object(scope, "mutation_sources", side_effect=AssertionError("live source read")):
            links = scope.prepared_links(cases, "syntax", self.linked((OP_A,)))
        self.assertEqual(links, {OP_A: ["mutation/e1"]})


class DeclarationTests(MutationFixture):
    def test_malformed_declarations_are_manifest_problems(self):
        base = self.campaign.declaration([OP_A], ["kA"])
        cases = [
            (lambda w: w.update(operations=[]), "nonempty operation list"),
            (lambda w: w.update(oracle="invented"), "unknown mutation oracle"),
            (lambda w: w.update(mutation_gate=""), "mutation gate"),
            (lambda w: w.update(artifact="docs/results.json.gz"), "committed"),
            (lambda w: w["mutants"][0].update(file="crates/tsr_fake/tests/lib.rs"), "production source"),
            (lambda w: w["mutants"].append(dict(w["mutants"][0])), "duplicated claimed mutant"),
            (lambda w: w["mutants"][0].pop("span_sha256"), "exactly"),
            (lambda w: w["mutants"][0].pop("control"), "exactly"),
            (lambda w: w["mutants"][0].update(control="9"), "malformed claimed mutant"),
            (lambda w: w["mutants"][0].update(operator="control"), "is a control"),
            (lambda w: w["mutants"][0].update(markers={OP_B: 3}), "malformed claimed mutant"),
            (lambda w: w["mutants"][0].update(ops=[OP_B, OP_A]), "malformed claimed mutant"),
            (lambda w: w.update(not_claimed={"x": ""}), "not_claimed"),
            (lambda w: w.update(operations=[OP_A, "tsc/internal/core/core.go:Invented"]), "not an operation"),
        ]
        known = set(ALL_OPS) | {OP_C}
        self.assertEqual(scope.mutation_declaration_problems(base, known), [])
        for mutate, text in cases:
            with self.subTest(text=text):
                witness = copy.deepcopy(base)
                mutate(witness)
                self.assertTrue(any(text in p for p in scope.mutation_declaration_problems(witness, known)),
                                scope.mutation_declaration_problems(witness, known))

    def test_duplicate_recorded_kill_is_malformed(self):
        witness = self.bound(operations=(OP_A,), keys=("kA",), not_claimed={})
        witness["mutation_evidence"]["kills"].append(dict(witness["mutation_evidence"]["kills"][0]))
        self.assertTrue(any("duplicated kill" in p for p in scope.mutation_declaration_problems(witness)))
        self.assertEqual(self.campaign.state(witness)["state"], "stale")

    def test_span_digest_splits_on_newline_only(self):
        data = b"a\r\nb\rc\nlast"
        self.assertEqual(scope.span_digest(data, 1, 1), sha(b"a\r\n"))
        self.assertEqual(scope.span_digest(data, 2, 3), sha(b"b\rc\nlast"))
        self.assertIsNone(scope.span_digest(data, 3, 4))
        self.assertIsNone(scope.span_digest(data, 0, 1))
        self.assertEqual(scope.span_digest(b"x\n", 1, 1), sha(b"x\n"))
        self.assertIsNone(scope.span_digest(b"x\n", 2, 2))

    def test_witness_problems_check_mutation_declarations(self):
        # H6: the mutation branch of witness_problems_in, on the real manifest.
        real = json.loads((ROOT / "data/phase1/scope.json").read_text())["operations"][0]["id"]
        witness = self.campaign.declaration([real], ["kA"])
        self.assertEqual(scope.witness_problems_in({"witnesses": [witness]}), [])
        for change, text in ((lambda w: w.update(oracle="invented"), "unknown mutation oracle"),
                             (lambda w: w.update(operations=[OP_A]), "not an operation in the frozen scope"),
                             (lambda w: w.update(mutants=[]), "list its claimed mutants")):
            with self.subTest(text=text):
                changed = copy.deepcopy(witness)
                change(changed)
                self.assertTrue(any(text in problem for problem in scope.witness_problems_in({"witnesses": [changed]})))


class RecordTests(MutationFixture):
    def setUp(self):
        super().setUp()
        self.cases = self.root / "data/phase1/cases.json"
        self.cases.write_text(json.dumps({"version": 1, "cases": [], "witnesses": []}))
        for target, name, value in ((phase1, "ROOT", self.root), (phase1, "CASES", self.cases),
                                    (scope, "ROOT", self.root)):
            patcher = patch.object(target, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_record(self, **kwargs):
        return phase1.record_mutations(self.root / scope.MUTATION_RESULTS, True, **kwargs)

    def test_declare_and_record_writes_bound_evidence(self):
        result = self.run_record(declare="e1")
        self.assertEqual(result["witnesses"]["mutation/e1"]["operations"], len(ALL_OPS))
        document = json.loads(self.cases.read_text())
        self.assertEqual(scope.witness_problems_in(document), [])
        state = scope.recorded_mutations(document, self.root)["mutation/e1"]
        self.assertEqual(state["operations"], sorted(ALL_OPS))
        self.assertIn(OP_C, result["unclaimed"])
        again = self.run_record()
        self.assertEqual(again["witnesses"], result["witnesses"])
        self.assertEqual(json.loads(self.cases.read_text()), document)
        self.run_record(declare="binder")
        document = json.loads(self.cases.read_text())
        records = scope.recorded_mutations(document, self.root)
        self.assertEqual(sorted(records), ["mutation/binder", "mutation/e1"])
        self.assertEqual(records["mutation/binder"]["operations"], sorted([OP_A, OP_D, OP_E]))
        self.assertEqual(records["mutation/e1"]["operations"], sorted(ALL_OPS))

    def test_refuses_partial_campaign(self):
        self.campaign.write_all(results={**self.campaign.results(), "partial": True})
        with self.assertRaisesRegex(ValueError, "partial"):
            self.run_record(declare="e1")

    def test_evidence_itself_refuses_an_incomplete_campaign(self):
        # H6: mutation_evidence checks completeness on its own, not only
        # through --declare.
        declaration = self.campaign.declaration(list(ALL_OPS), list(ALL_KEYS), excuses(self.campaign))
        scope.mutation_evidence(declaration, self.root)
        self.campaign.write_all(results={**self.campaign.results(), "partial": True})
        with self.assertRaisesRegex(ValueError, "partial"):
            scope.mutation_evidence(declaration, self.root)
        results = self.campaign.results()
        results["mutants"] = [row for row in results["mutants"] if row["key"] != "kS"]
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "have no result"):
            scope.mutation_evidence(self.campaign.declaration([OP_A], ["kA"]), self.root)

    def test_refuses_unrun_mutants(self):
        results = self.campaign.results()
        results["mutants"].pop()
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "have no result"):
            self.run_record(declare="e1")
        results = self.campaign.results()
        results["mutants"][0]["state"] = "pending"
        self.campaign.write_all(results=results)
        with self.assertRaisesRegex(ValueError, "final state"):
            self.run_record(declare="e1")
        # A planned mutant no oracle campaign ran (no campaigns) is un-run, not evidence.
        self.campaign.runs["kD2"] = {}
        self.campaign.write_all()
        with self.assertRaisesRegex(ValueError, "final state"):
            self.run_record(declare="e1")

    def test_a_budget_or_not_run_mutant_makes_the_campaign_partial(self):
        # Item 8: `budget` (candidate rows left when --max-rows ran out) is not
        # a final state, merged or in any one oracle, whatever `partial` says.
        for name, key, change in (
                ("merged budget", "kA", lambda record: record.update(state="budget", kills=[])),
                ("merged not_run", "kA", lambda record: record.update(state="not_run", kills=[])),
                ("one oracle's budget", "kD2", lambda record: record["oracles"]["binder"].update(state="budget")),
                ("one oracle's not_run", "kD2", lambda record: record["oracles"]["binder"].update(state="not_run")),
                ("one oracle never ran it", "kD2", lambda record: record["oracles"].pop("binder"))):
            with self.subTest(name=name):
                results = self.campaign.results()
                self.assertNotIn("partial", results)
                change(next(row for row in results["mutants"] if row["key"] == key))
                self.campaign.write_all(results=results)
                before = self.cases.read_bytes()
                with self.assertRaisesRegex(ValueError, "is partial"):
                    self.run_record(declare="e1")
                with self.assertRaisesRegex(ValueError, "is partial"):
                    scope.mutation_evidence(self.campaign.declaration([OP_E], ["kE"]), self.root)
                self.assertEqual(self.cases.read_bytes(), before)
        # A killed mutant is final wherever else the budget stopped it.
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kA")["oracles"]["binder"].update(state="budget")
        self.campaign.write_all(results=results)
        self.assertIn("mutation/e1", self.run_record(declare="e1")["witnesses"])
        # A site credited nowhere by the whole-program rule is a final state.
        self.campaign.write_all()
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kS").update(state="not_credited_multi_op", kills=[])
        self.campaign.write_all(results=results)
        self.run_record(declare="e1")
        credited = scope.recorded_mutations(json.loads(self.cases.read_text()), self.root)["mutation/e1"]["operations"]
        self.assertIn(OP_A, credited)
        self.assertNotIn(OP_B, credited)

    def test_a_skip_is_final_only_for_a_mutant_killed_elsewhere(self):
        # Item 8: --skip-killed leaves a mutant out of one oracle's campaign
        # and the results carry `skipped_by`. A kill elsewhere justifies it;
        # a skipped mutant that is not killed was never measured there.
        self.campaign.runs["kE"] = {"e1": ("killed", ["r2"], 1)}
        self.campaign.write_all()
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kE")["skipped_by"] = {"binder": "a" * 64}
        self.campaign.write_all(results=results)
        self.run_record(declare="e1")
        self.assertIn(OP_E, json.loads(self.cases.read_text())["witnesses"][0]["operations"])
        results = self.campaign.results()
        next(row for row in results["mutants"] if row["key"] == "kD2")["skipped_by"] = {"binder": "a" * 64}
        self.campaign.write_all(results=results)
        before = self.cases.read_bytes()
        with self.assertRaisesRegex(ValueError, "unjustified skipped mutant is partial"):
            self.run_record(declare="e1")
        self.assertEqual(self.cases.read_bytes(), before)

    def test_a_mistyped_witness_name_is_an_error(self):
        self.run_record(declare="e1")
        before = self.cases.read_bytes()
        for names in (["mutation/e1", "mutation/e1-typo"], ["mutation/el"]):
            with self.subTest(names=names):
                with self.assertRaisesRegex(ValueError, names[-1]):
                    self.run_record(witnesses=names)
                self.assertEqual(self.cases.read_bytes(), before)

    def test_declare_never_replaces_another_kind_of_witness(self):
        gated = {"id": "mutation/e1", "kind": "rust_gated", "artifact": "data/phase1/cases.json", "operations": [],
                 "witnesses": "a gated witness that happens to share the id", "gate": "true"}
        self.cases.write_text(json.dumps({"version": 1, "cases": [], "witnesses": [gated]}))
        before = self.cases.read_bytes()
        with self.assertRaisesRegex(ValueError, "already names a rust_gated witness"):
            self.run_record(declare="e1")
        self.assertEqual(self.cases.read_bytes(), before)

    def test_refuses_an_unsupported_claim_and_writes_nothing(self):
        document = {"version": 1, "cases": [], "witnesses": [self.campaign.declaration([OP_A, OP_C], ["kA", "kS"])]}
        self.cases.write_text(json.dumps(document))
        with self.assertRaisesRegex(ValueError, "not witnessed"):
            self.run_record()
        self.assertEqual(json.loads(self.cases.read_text()), document)

    def test_refuses_a_results_file_outside_the_committed_directory(self):
        with self.assertRaisesRegex(ValueError, "committed artifact"):
            phase1.record_mutations(self.root / "inventory/e1.json", False, declare="e1")


def receipt_output(expectation, lost=(), base_lost=(), hit=(), stale_excused=(), changes=None, retraced=True):
    """phase1_mutation_run.confirm's receipt for the expectation.

    `lost` pairs (key, oracle, row) replay other mutant digests (state
    `changed`), `base_lost` rows no longer equal native, `hit` excused mutants
    were reached and `stale_excused` ones could not be spliced.
    """
    base = sorted({(kill["oracle"], kill["row"]) for kill in expectation["kills"]})
    pairs, pair_failures = [], []
    for kill in expectation["kills"]:
        identity = (kill["key"], kill["oracle"], kill["row"])
        mutant = dict(kill["mutant"], **({"parse": "0" * 64} if identity in lost else {}))
        pairs.append({"key": kill["key"], "oracle": kill["oracle"], "row": kill["row"],
                      "request_sha256": kill["request_sha256"], "state": "changed" if identity in lost else "reproduced",
                      "stages": list(kill["stages"]), "mutant": mutant, "control": kill["control"],
                      **(changes or {}).get(identity, {})})
        if identity in lost:
            pair_failures.append({"oracle": kill["oracle"], "row": kill["row"], "key": kill["key"], "status": "changed",
                                  "failure": "the recorded kill pair does not reproduce"})
    failures = ([{"oracle": oracle, "row": row, "mutant": 0, "status": "done",
                  "failure": "base row no longer equals native"} for oracle, row in base if (oracle, row) in base_lost]
                + pair_failures
                + [{"oracle": "e1", "key": key, "rows": 3, "failure": "an excused home is reached"} for key in hit]
                + [{"oracle": "e1", "key": key, "rows": None, "failure": "the excused home's span changed"}
                   for key in stale_excused])
    rows = len(base)
    passed = not failures and retraced
    return {"version": 1, "kind": "mutation-witnesses", "results_sha256": expectation["results_sha256"],
            "plan_sha256": expectation["manifest_sha256"], "drivers": {"phase1_mutation_driver": "d" * 64}, "ws": {},
            "native_sha256": expectation["native_sha256"], "oracles": list(expectation["campaigns"]),
            "killed_mutants": expectation["killed_mutants"], "stale": [], "pairs_total": len(pairs),
            "confirmed": len(pairs) - len(lost), "pairs": pairs, "base_rows": rows, "base_native": rows - len(base_lost),
            "excused_mutants": len(expectation["results_excused"]), "excused_stale": sorted(stale_excused),
            "retraced": {oracle: {"rows": 4, "mutants_reached": 3} for oracle in expectation["campaigns"]}
            if retraced else None,
            "failures": failures, "result": "pass" if passed else "fail"}


def expectation_fixture(**changes):
    results = {"version": 1, "mutants": [
        {"id": 1, "key": "kA", "state": "killed", "kills": [
            {"oracle": "e1", "row": "r0", "request_sha256": request("r0"), "stages": ["parse"],
             "mutant": {"parse": "1" * 64}, "control": None},
            {"oracle": "e1", "row": "r1", "request_sha256": request("r1"), "stages": ["parse"],
             "mutant": {"parse": "2" * 64}, "control": None}]},
        {"id": 2, "key": "kS", "state": "survived", "kills": []},
        {"id": 3, "key": "kB", "state": "killed", "kills": [
            {"oracle": "e1", "row": "r0", "request_sha256": request("r0"), "stages": ["parse"],
             "mutant": {"parse": "3" * 64}, "control": {"parse": "4" * 64}}]}]}
    return {"results_sha256": "a" * 64, "manifest_sha256": "b" * 64,
            "native_sha256": {"binder": "e" * 64, "e1": "c" * 64},
            "kills": integration.mutation_recorded_kills(results), "pairs": integration.mutation_confirm_pairs(results),
            "killed_mutants": 2, "declared": ["mutation/e1"], "bound": ["mutation/e1"],
            "recorded": [("kA", "e1", "r0")], "excused": ["kD2"], "results_excused": ["kD2", "kEs"],
            "campaigns": ["binder", "e1"], **changes}


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.expectation = expectation_fixture()

    def result(self, output, exit_code=None, expectation=None):
        exit_code = (0 if output["result"] == "pass" else 1) if exit_code is None else exit_code
        return integration.mutation_witness_result(output, exit_code, expectation or self.expectation)

    def test_confirm_pairs_follow_the_artifact_and_skip_unkilled_mutants(self):
        self.assertEqual(self.expectation["pairs"], [(1, "kA", "e1", "r0"), (1, "kA", "e1", "r1"), (3, "kB", "e1", "r0")])
        with self.assertRaisesRegex(ValueError, "twice"):
            integration.mutation_recorded_kills({"mutants": [{"key": "k", "state": "killed", "kills": [
                {"oracle": "e1", "row": "r0"}, {"oracle": "e1", "row": "r0"}]}]})
        self.assertEqual(integration.mutation_results_excused({"operations": {
            "a": {"homes": [{"excused": True, "mutants": ["k2", "k1"]}, {"excused": False, "mutants": ["k3"]}]},
            "b": {"homes": [{"excused": "yes", "mutants": ["k4"]}]}}}), ["k1", "k2"])

    def test_complete_replay_matches(self):
        observed = self.result(receipt_output(self.expectation))
        self.assertEqual(observed["state"], "match")
        self.assertEqual((observed["pairs"], observed["base_rows"]), (3, 2))

    def test_every_failure_is_a_measured_difference_with_exit_one(self):
        for name, output in (("lost other", receipt_output(self.expectation, lost={("kB", "e1", "r0")})),
                             ("lost recorded", receipt_output(self.expectation, lost={("kA", "e1", "r0")})),
                             ("base", receipt_output(self.expectation, base_lost={("e1", "r1")})),
                             ("excused hit", receipt_output(self.expectation, hit=["kD2"])),
                             ("excused stale", receipt_output(self.expectation, stale_excused=["kEs"])),
                             ("no retrace", receipt_output(self.expectation, retraced=False))):
            with self.subTest(name=name):
                observed = self.result(output, 1)
                self.assertEqual(observed["state"], "different")
                with self.assertRaisesRegex(ValueError, "exit code"):
                    self.result(output, 0)
        lost = self.result(receipt_output(self.expectation, lost={("kA", "e1", "r0")}), 1)
        self.assertEqual(lost["recorded_pairs_lost"], [("kA", "e1", "r0")])
        self.assertEqual(self.result(receipt_output(self.expectation, hit=["kD2"]), 1)["excused_hit"], ["kD2"])

    def test_a_reproduced_pair_must_carry_its_recorded_stages_and_digests(self):
        # K4: a pair that still differs from native, but not as recorded, is lost.
        changes = {("kB", "e1", "r0"): {"control": {"parse": "5" * 64}},
                   ("kA", "e1", "r1"): {"stages": ["parse", "encode_source_file"]},
                   ("kA", "e1", "r0"): {"mutant": {"parse": "9" * 64}}}
        for identity, change in changes.items():
            with self.subTest(pair=identity):
                output = receipt_output(self.expectation, changes={identity: change})
                with self.assertRaisesRegex(ValueError, "reproduced but not as recorded"):
                    self.result(output, 0)
                row = next(row for row in output["pairs"] if (row["key"], row["oracle"], row["row"]) == identity)
                row["state"] = "changed"
                output["failures"].append({"oracle": identity[1], "row": identity[2], "key": identity[0],
                                           "status": "changed", "failure": "x"})
                output.update(confirmed=2, result="fail")
                self.assertEqual(self.result(output, 1)["state"], "different")

    def test_a_reproduced_pair_must_differ_from_its_control_where_it_differs_from_native(self):
        # KS2 in the validator: the recorded kB kill differs from native in
        # parse only, where it equals its control; its control differs from
        # the mutant only elsewhere. confirm reports such a pair not_a_kill.
        results_kills = copy.deepcopy(self.expectation["kills"])
        kB = next(kill for kill in results_kills if kill["key"] == "kB")
        kB.update(mutant={"parse": "3" * 64, "node_index_before": "6" * 64},
                  control={"parse": "3" * 64, "node_index_before": "7" * 64}, stages=["parse"])
        expectation = expectation_fixture(kills=results_kills)
        with self.assertRaisesRegex(ValueError, "equals its control in every stage where it differs from native"):
            self.result(receipt_output(expectation), 0, expectation)
        output = receipt_output(expectation)
        row = next(row for row in output["pairs"] if row["key"] == "kB")
        row["state"] = "not_a_kill"
        output["failures"].append({"oracle": "e1", "row": "r0", "key": "kB", "status": "not_a_kill", "failure": "x"})
        output.update(confirmed=2, result="fail")
        self.assertEqual(self.result(output, 1, expectation)["state"], "different")

    def test_a_pair_reported_lost_is_lost_whatever_its_digests(self):
        output = receipt_output(self.expectation)
        output["pairs"][0]["state"] = "control_failed"
        output["failures"].append({"oracle": "e1", "row": "r0", "key": "kA", "status": "control_failed", "failure": "x"})
        output.update(confirmed=2, result="fail")
        self.assertEqual(self.result(output, 1)["state"], "different")

    def test_unbound_or_untraced_excuses_are_not_complete(self):
        expectation = expectation_fixture(declared=["mutation/binder", "mutation/e1"])
        observed = self.result(receipt_output(expectation), expectation=expectation)
        self.assertEqual(observed["state"], "different")
        self.assertEqual(observed["unbound_witnesses"], ["mutation/binder"])
        # A bound witness excuses a home the results (so the re-trace) did not.
        expectation = expectation_fixture(excused=["kD2", "kQ"])
        observed = self.result(receipt_output(expectation), expectation=expectation)
        self.assertEqual((observed["state"], observed["untraced_excused"]), ("different", ["kQ"]))

    def test_empty_duplicated_missing_or_tampered_receipts_are_invalid(self):
        good = receipt_output(self.expectation)
        empty = expectation_fixture(kills=[], pairs=[], recorded=[], killed_mutants=0)
        base_failing = receipt_output(self.expectation, base_lost={("e1", "r1")})
        lost = receipt_output(self.expectation, lost={("kB", "e1", "r0")})
        hit = receipt_output(self.expectation, hit=["kD2"])

        def edit(output, change):
            output = copy.deepcopy(output)
            change(output)
            return output

        broken = {
            "empty inventory": (empty, receipt_output(empty)),
            "missing pair": (self.expectation, edit(good, lambda o: (o["pairs"].pop(),
                                                                     o.update(pairs_total=2, confirmed=2)))),
            "duplicated pair": (self.expectation, edit(good, lambda o: (o["pairs"].append(dict(o["pairs"][0])),
                                                                        o.update(pairs_total=4, confirmed=4)))),
            "duplicated pair, counts kept": (self.expectation, edit(good, lambda o: o["pairs"].append(
                dict(o["pairs"][0])))),
            "unrecorded pair": (self.expectation, edit(good, lambda o: o["pairs"][0].update(row="r9"))),
            "other request": (self.expectation, edit(good, lambda o: o["pairs"][0].update(request_sha256="f" * 64))),
            "reproduced but different": (self.expectation, edit(good, lambda o: o["pairs"][0].update(
                mutant={"parse": "8" * 64}))),
            "reproduced, control dropped": (self.expectation, edit(good, lambda o: o["pairs"][2].update(control=None))),
            "stateless pair": (self.expectation, edit(good, lambda o: o["pairs"][0].update(state=None))),
            "pair field missing": (self.expectation, edit(good, lambda o: o["pairs"][0].pop("control"))),
            "no pair list": (self.expectation, edit(good, lambda o: o.pop("pairs"))),
            "lost pair without its failure": (self.expectation, edit(lost, lambda o: o.update(failures=[]))),
            "failure with another state": (self.expectation, edit(lost, lambda o: o["failures"][0].update(
                status="crash"))),
            "failure of a reproduced pair": (self.expectation, edit(lost, lambda o: o["failures"].append(
                {"oracle": "e1", "row": "r1", "key": "kA", "status": "changed", "failure": "x"}))),
            "failure of no known kind": (self.expectation, edit(lost, lambda o: o["failures"].append({"note": "x"}))),
            "missing base row": (self.expectation, {**good, "base_rows": 1, "base_native": 1}),
            "killed count": (self.expectation, {**good, "killed_mutants": 3}),
            "pair count": (self.expectation, {**good, "pairs_total": 4}),
            "confirmed count": (self.expectation, {**lost, "confirmed": 3}),
            "excused count": (self.expectation, {**good, "excused_mutants": 1}),
            "other results": (self.expectation, {**good, "results_sha256": "f" * 64}),
            "other plan": (self.expectation, {**good, "plan_sha256": "f" * 64}),
            "other native": (self.expectation, {**good, "native_sha256": {"binder": "e" * 64, "e1": "f" * 64}}),
            "other oracles": (self.expectation, {**good, "oracles": ["e1"]}),
            "negative base": (self.expectation, {**good, "base_native": -1}),
            "missing count": (self.expectation, {key: value for key, value in good.items() if key != "base_native"}),
            "no failure list": (self.expectation, {**good, "failures": None}),
            "unknown base row": (self.expectation, {**base_failing, "failures": [
                dict(base_failing["failures"][0], row="r9")]}),
            "duplicated base row": (self.expectation, {**base_failing, "base_native": 0,
                                                       "failures": base_failing["failures"] * 2}),
            "uncounted base failure": (self.expectation, {**base_failing, "base_native": 2}),
            "hit outside the excused homes": (self.expectation, edit(hit, lambda o: o["failures"][0].update(key="kQ"))),
            "hit with no rows": (self.expectation, edit(hit, lambda o: o["failures"][0].update(rows=0))),
            "stale list disagrees": (self.expectation, edit(hit, lambda o: o.update(excused_stale=["kD2"]))),
            "retrace misses an oracle": (self.expectation, edit(good, lambda o: o["retraced"].pop("binder"))),
            "retrace of no rows": (self.expectation, edit(good, lambda o: o["retraced"]["e1"].update(rows=0))),
            "result disagrees": (self.expectation, {**lost, "result": "pass"}),
            "wrong kind": (self.expectation, {**good, "kind": "rust-witnesses"}),
            "not a receipt": (self.expectation, [good]),
        }
        for name, (expectation, output) in broken.items():
            # Both exit codes: neither may turn a malformed receipt into a
            # measurement, whatever the outcome it would imply.
            for exit_code in (0, 1):
                with self.subTest(name=name, exit_code=exit_code):
                    with self.assertRaises(ValueError):
                        integration.mutation_witness_result(output, exit_code, expectation)

    def test_recorded_kill_missing_from_the_replay_is_invalid(self):
        expectation = expectation_fixture(recorded=[("kZ", "e1", "r9")])
        with self.assertRaisesRegex(ValueError, "absent"):
            self.result(receipt_output(expectation), 0, expectation)


class ExpectationTests(MutationFixture):
    """mutation_expectation on the fixture root: committed data, committed view."""

    def setUp(self):
        super().setUp()
        self.witness = self.bound()
        self.write_cases([self.witness])

    def write_cases(self, witnesses, linked=ALL_OPS):
        self.campaign.write("data/phase1/cases.json", json.dumps({"witnesses": witnesses}).encode())
        self.campaign.write("data/phase1/scope.json", json.dumps({"operations": [
            {"id": op, "cases": [w["id"] for w in witnesses if op in linked]} for op in (*ALL_OPS, OP_C, OP_X)]}).encode())

    def test_expectation_names_every_recorded_kill_and_excused_mutant(self):
        expectation = integration.mutation_expectation(self.root)
        self.assertEqual(expectation["bound"], ["mutation/e1"])
        self.assertEqual(expectation["campaigns"], ["binder", "e1"])
        self.assertEqual(expectation["excused"], ["kD2", "kEs"])
        self.assertEqual(set(expectation["recorded"]), {(kill["key"], "e1", kill["row"])
                                                         for kill in self.witness["mutation_evidence"]["kills"]})
        self.assertEqual(len(expectation["kills"]), len(expectation["pairs"]))
        kF = next(kill for kill in expectation["kills"] if kill["key"] == "kF")
        self.assertIsNotNone(kF["control"])
        observed = integration.mutation_witness_result(receipt_output(expectation), 0, expectation)
        self.assertEqual(observed["state"], "match")

    def test_expectation_binds_the_native_freeze_of_every_traced_oracle(self):
        # confirm reports every traced oracle's freeze, kills or not.
        for key, runs in self.campaign.runs.items():
            runs["binder"] = ("survived", [], runs["binder"][2]) if "binder" in runs else runs.get("binder")
        self.campaign.runs = {key: {oracle: run for oracle, run in runs.items() if run} for key, runs in
                              self.campaign.runs.items()}
        self.campaign.write_all()
        expectation = integration.mutation_expectation(self.root)
        self.assertEqual({kill["oracle"] for kill in expectation["kills"]}, {"e1"})
        self.assertEqual(sorted(expectation["native_sha256"]), ["binder", "e1"])

    def test_expectation_reads_the_committed_scope(self):
        with patch.object(scope, "mutation_sources", side_effect=AssertionError("live source read")):
            integration.mutation_expectation(self.root)
        self.write_cases([self.witness], linked=())
        self.assertEqual(integration.mutation_expectation(self.root)["bound"], [])

    def test_bound_witness_on_another_results_artifact_is_refused(self):
        forged = copy.deepcopy(self.witness)
        with patch.object(scope, "recorded_mutations", return_value={"mutation/e1": {
                "state": "bound", "operations": [OP_A], "stale_operations": {}, "reason": None, "inputs": {}}}):
            forged["mutation_evidence"]["results_sha256"] = "f" * 64
            self.write_cases([forged])
            with self.assertRaisesRegex(ValueError, "results artifact other than the committed one"):
                integration.mutation_expectation(self.root)


class ObserveTests(unittest.TestCase):
    def test_observe_runs_the_confirmation_for_the_mutation_receipt(self):
        # HR: without its `observe` entry the receipt, and so the P1B metric,
        # could never be produced. Nothing runs here: the command is captured.
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        output = Path(directory.name) / "receipts"
        calls = []

        def run(command, **kwargs):
            calls.append((command, kwargs.get("cwd")))
            return producers.subprocess.CompletedProcess(command, 1, stdout=b"{}", stderr=b"")

        with patch.object(producers, "source_closure", return_value={"fixture": "a" * 64}), \
                patch.object(producers, "DEFAULT", Path(directory.name)), \
                patch.object(producers.subprocess, "run", run):
            result = producers.observe(integration.MUTATION_RECEIPT, output)
        self.assertEqual(calls, [(scope.MUTATION_CONFIRM_COMMAND, producers.ROOT)])
        receipt = json.loads(Path(result["receipt"]).read_text())
        self.assertEqual((receipt["id"], receipt["command"]), (integration.MUTATION_RECEIPT, scope.MUTATION_CONFIRM_COMMAND))
        registry = json.loads((Path(directory.name) / "integration-receipts.json").read_text())
        self.assertEqual(set(registry), {integration.MUTATION_RECEIPT})
        with self.assertRaisesRegex(ValueError, "unknown executable witness"):
            producers.observe("mutation-witness", output)


class ReceiptEvaluationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.prepared = integration.check()
        cls.inputs = {"fixture": "a" * 64}
        cls.expectation = expectation_fixture()

    def receipt(self, output, exit_code=0, inputs=None):
        stdout = output if isinstance(output, str) else json.dumps(output, sort_keys=True)
        return integration.receipt(integration.MUTATION_RECEIPT, scope.MUTATION_CONFIRM_COMMAND,
                                   inputs or self.inputs, stdout, exit_code=exit_code)

    def evaluate(self, receipts):
        with patch.object(integration, "mutation_expectation", return_value=self.expectation):
            return integration.evaluate(self.prepared, [], receipts, source_inputs=self.inputs)

    def test_measured_receipt_sets_the_mutation_state_only(self):
        result = self.evaluate([self.receipt(receipt_output(self.expectation))])
        self.assertEqual(result["problems"], [])
        self.assertEqual(result["mutation_witnesses"], {"state": "match"})
        self.assertFalse(result["complete"])
        failing = self.evaluate([self.receipt(receipt_output(self.expectation, lost={("kA", "e1", "r1")}), 1)])
        self.assertEqual(failing["mutation_witnesses"], {"state": "different"})
        self.assertEqual(failing["problems"], [])

    def test_confirmation_that_printed_no_receipt_failed(self):
        result = self.evaluate([self.receipt("Traceback (most recent call last):\n", exit_code=1)])
        self.assertEqual(result["problems"], [])
        self.assertEqual(result["metric_observations"]["receipts"][integration.MUTATION_RECEIPT], "failed")
        self.assertEqual(result["mutation_witnesses"], {"state": "failed"})

    def test_absent_receipt_is_pending(self):
        self.assertEqual(self.evaluate([])["mutation_witnesses"], {"state": "pending"})

    def test_tampered_stdout_or_mismatched_exit_is_invalid(self):
        receipt = self.receipt(receipt_output(self.expectation))
        receipt["stdout"] = receipt["stdout"].replace('"confirmed": 3', '"confirmed": 2')
        self.assertTrue(any("output digest differs" in p for p in self.evaluate([receipt])["problems"]))
        mismatched = self.receipt(receipt_output(self.expectation), exit_code=1)
        self.assertTrue(any("exit code disagrees" in p for p in self.evaluate([mismatched])["problems"]))
        changed = self.receipt(receipt_output(self.expectation))
        changed["command"] = [*scope.MUTATION_CONFIRM_COMMAND, "--ws", "target/elsewhere"]
        self.assertTrue(any("command differs" in p for p in self.evaluate([changed])["problems"]))

    def test_stale_receipt_is_unavailable_and_keeps_independent_results(self):
        stale = self.receipt(receipt_output(self.expectation), inputs={"fixture": "b" * 64})
        observations = [{"id": identity, "command": command, "exit_code": 0,
                         "stdout": "".join("test " + name + " ... ok\n" for name in tests)}
                        for identity, (command, tests) in integration.RUST_WITNESS_TESTS.items()]
        rust = integration.receipt("rust-witnesses",
                                   ["python3", "scripts/phase1_integration.py", "observe-rust-witnesses"],
                                   self.inputs, json.dumps(observations))
        result = self.evaluate([stale, rust])
        self.assertEqual(result["problems"], [])
        self.assertEqual(result["mutation_witnesses"], {"state": "unavailable"})
        self.assertEqual(result["metric_observations"]["receipts"][integration.MUTATION_RECEIPT], "unavailable")
        self.assertIn(integration.MUTATION_RECEIPT, result["unavailable"])
        self.assertEqual(result["rust_witnesses"], {"state": "match"})

    def test_aggregate_emits_the_metric_only_from_a_matching_receipt(self):
        health = {"healthy": True, "integration": {"mutation_witnesses": {"state": "match"}, "prepared": True,
                                                   "complete": False}, "coverage": {"preparation_complete": False},
                  "preparations": {step: {"complete": False} for step in scope.STEP_PACKAGES}}
        metrics = producers.aggregate("foundations", {}, health)["metrics"]
        self.assertIs(metrics["mutation_witnesses_complete"], True)
        for state in ("different", "pending", "unavailable", "failed", "invalid"):
            health["integration"]["mutation_witnesses"]["state"] = state
            self.assertIs(producers.aggregate("foundations", {}, health)["metrics"]["mutation_witnesses_complete"], False)
        health["integration"]["mutation_witnesses"]["state"] = "match"
        health["healthy"] = False
        self.assertIs(producers.aggregate("foundations", {}, health)["metrics"]["mutation_witnesses_complete"], False)


class HarnessJoinTests(MutationFixture):
    """The real scope/coverage join, with the fixture campaign as the only mutation input."""

    def join(self, witness, linked=True, roster=None):
        """coverage.build over the real data plus `witness`, and a committed scope that links it when `linked`.

        A linked operation's roster state becomes `prepared`, as scope.build
        writes it, unless `roster` names the state to keep.
        """
        cases_path = ROOT / "data/phase1/cases.json"
        scope_path = ROOT / "data/phase1/scope.json"
        document = json.loads(cases_path.read_bytes())
        # A committed mutation witness binds the real campaign, never the
        # fixture root, so here it is stale and credits nothing; one may share
        # the fixture witness's identity, so it is renamed, with its scope
        # links, to keep the fixture campaign the only crediting input.
        renamed = f"{witness['id']}-committed"
        for row in document["witnesses"]:
            if row.get("id") == witness["id"]:
                row["id"] = renamed
        document["witnesses"].append(witness)
        replacement = json.dumps(document).encode()
        committed = json.loads(scope_path.read_bytes())
        for row in committed["operations"]:
            if witness["id"] in row["cases"]:
                row["cases"] = sorted({*row["cases"], renamed} - {witness["id"]})
        if linked:
            for row in committed["operations"]:
                if row["id"] in witness["operations"]:
                    # What scope.build writes for an operation its link covers.
                    committed["counts"][row["disposition"]] -= 1
                    committed["counts"]["covered"] += 1
                    row.update(cases=sorted({*row["cases"], witness["id"]}), disposition="covered",
                               basis=f"witnessed by 1 exact link(s): {witness['id']}",
                               roster=dict(row["roster"], state=roster or "prepared"))
        scope_bytes = json.dumps(committed).encode()
        read_bytes, is_file = Path.read_bytes, Path.is_file
        artifact = ROOT / witness["artifact"]
        real = scope.recorded_mutations
        replaced = {cases_path: replacement, scope_path: scope_bytes}
        with patch.object(Path, "read_bytes", lambda path: replaced.get(path) or read_bytes(path)), \
                patch.object(Path, "is_file", lambda path: True if path == artifact else is_file(path)), \
                patch.object(scope, "recorded_mutations",
                             lambda cases, root=None, **kwargs: real(cases, self.root, **kwargs)), \
                patch.object(scope, "mutation_sources", side_effect=AssertionError("live source read")), \
                patch("subprocess.run", side_effect=AssertionError("child")), \
                patch("subprocess.Popen", side_effect=AssertionError("child")):
            return coverage.build()

    def real_pending_operation(self):
        report = coverage.build()
        return next(row["id"] for row in report["gaps"] if row["root_cause"] == "operation_witness_missing"
                    and row["id"].startswith("tsc/internal/parser/parser.go:"))

    def retarget(self, operation):
        """Point the fixture's OP_A home at a real pending scope operation."""
        self.campaign.write_sources(FAKE_SOURCE.replace(OP_A, operation), OTHER_SOURCE)
        for oracle in ORACLES:
            self.campaign.reach[oracle][operation] = self.campaign.reach[oracle].pop(OP_A)
        self.campaign.mutants[0] = self.campaign.fn_mutant(1, "kA", [operation], FAKE, "parse_a")
        self.campaign.write_all()

    def test_bound_witness_links_its_operation_without_a_child_process(self):
        operation = self.real_pending_operation()
        self.retarget(operation)
        witness = self.campaign.record(self.campaign.declaration([operation], ["kA"]))
        report = self.join(witness)
        row = next(row for row in report["operations"] if row["id"] == operation)
        self.assertIn("mutation/e1", row["links"])
        self.assertIsNone(row["root_cause"])
        joined = next(row for row in report["witnesses"] if row["id"] == "mutation/e1")
        self.assertEqual(joined["producer_metrics"], [scope.MUTATION_METRIC])
        self.assertEqual(joined["operations"], [operation])
        self.assertIn("mutation/e1", report["metric_contributors"][scope.MUTATION_METRIC])
        self.assertIn(scope.MUTATION_RESULTS, report["input_sha256"])
        self.assertIn("scripts/e1_oracle/main.go", report["input_sha256"])
        self.assertEqual(report["problems"], [])

    def test_scope_that_predates_the_recording_links_nothing(self):
        operation = self.real_pending_operation()
        self.retarget(operation)
        witness = self.campaign.record(self.campaign.declaration([operation], ["kA"]))
        report = self.join(witness, linked=False)
        row = next(row for row in report["gaps"] if row["id"] == operation)
        self.assertEqual(row["root_cause"], "mutation_witness_stale")
        self.assertIn("committed scope.json does not link it", row["mutation_witnesses"]["mutation/e1"])
        self.assertEqual(report["problems"], [])

    def test_link_that_stopped_binding_is_a_gap_not_a_problem(self):
        # H2: the committed scope still links the operation, but an artifact
        # binding broke. The operation is a named gap and coverage stays
        # healthy, even though every mutation witness is now stale.
        operation = self.real_pending_operation()
        self.retarget(operation)
        witness = self.campaign.record(self.campaign.declaration([operation], ["kA"]))
        self.campaign.write_all(results={**self.campaign.results(), "note": "rerun"})
        report = self.join(witness)
        row = next(row for row in report["gaps"] if row["id"] == operation)
        self.assertEqual(row["root_cause"], "mutation_witness_stale")
        self.assertIn("changed after the evidence was recorded", row["mutation_witnesses"]["mutation/e1"])
        self.assertEqual(report["problems"], [])
        self.assertTrue(report["healthy"])
        joined = next(row for row in report["witnesses"] if row["id"] == "mutation/e1")
        self.assertEqual((joined["state"], joined["producer_metrics"]), ("stale", []))

    def test_unrecorded_witness_is_a_named_gap_and_healthy(self):
        operation = self.real_pending_operation()
        self.retarget(operation)
        report = self.join(self.campaign.declaration([operation], ["kA"]), linked=False)
        row = next(row for row in report["gaps"] if row["id"] == operation)
        self.assertEqual(row["root_cause"], "mutation_witness_stale")
        self.assertIn("never recorded", row["mutation_witnesses"]["mutation/e1"])
        self.assertTrue(report["healthy"], report["problems"])

    def test_metric_route_is_required_only_while_a_witness_binds(self):
        report = coverage.build()
        row = {"id": "mutation/e1", "kind": "mutation_kill", "state": "bound", "operations": [OP_A],
               "producer_metrics": []}
        with self.assertRaisesRegex(ValueError, scope.MUTATION_METRIC):
            coverage.metric_contributors(report["cases"], report["witnesses"] + [row], report["external_inventories"])
        routed = dict(row, producer_metrics=[scope.MUTATION_METRIC])
        contributors = coverage.metric_contributors(report["cases"], report["witnesses"] + [routed],
                                                    report["external_inventories"])
        self.assertEqual(contributors[scope.MUTATION_METRIC], ["mutation/e1"])
        for stale in (dict(row, state="stale", operations=[]), dict(row, state="unrecorded", operations=[])):
            coverage.metric_contributors(report["cases"], report["witnesses"] + [stale], report["external_inventories"])

    def test_a_bound_mutation_witness_resolves_a_later_step_operation(self):
        # HR3: a mutation witness is a gated link like a rust_gated one, so a
        # later-step operation it covers exactly is no later_step_unresolved gap.
        operation = next(row["id"] for row in coverage.build()["gaps"]
                         if row["root_cause"] == "later_step_unresolved")
        self.retarget(operation)
        witness = self.campaign.record(self.campaign.declaration([operation], ["kA"]))
        report = self.join(witness, roster="exempt:later_step")
        row = next(row for row in report["operations"] if row["id"] == operation)
        self.assertEqual(row["roster_state"], "exempt:later_step")
        self.assertIn("mutation/e1", row["links"])
        self.assertIsNone(row["root_cause"])
        self.assertNotIn(operation, {gap["id"] for gap in report["gaps"]})
        # The same link, stale, leaves the later-step gap open.
        self.campaign.write_all(results={**self.campaign.results(), "note": "rerun"})
        stale = self.join(witness, roster="exempt:later_step")
        self.assertIn(operation, {gap["id"] for gap in stale["gaps"]})

    def test_exempt_operation_claimed_by_a_mutation_witness_fails_the_roster(self):
        document = json.loads((ROOT / "data/phase1/scope.json").read_text())
        roster = json.loads((ROOT / "data/phase1/syntax-roster.json").read_text())
        exempt = roster["exemptions"][0]["operation"]
        cases = json.loads((ROOT / "data/phase1/cases.json").read_text())
        witness = self.campaign.declaration([exempt], ["kA"])
        cases["witnesses"].append(witness)
        self.assertEqual(scope.roster_problems(document, cases, "syntax"), [])
        bound = {"mutation/e1": {"state": "bound", "operations": [exempt], "stale_operations": {}, "reason": None,
                                 "inputs": {}}}
        with patch.object(scope, "recorded_mutations", return_value=bound):
            problems = scope.roster_problems(document, cases, "syntax")
        self.assertTrue(any(exempt in p and "exempted but also prepared by mutation/e1" in p for p in problems))

    def test_foundations_closure_binds_the_mutation_tools_and_artifacts(self):
        # HR: the switch crate reaches the closure through the syntax harness
        # anyway; the driver's kill rule, the splicer's operators, the Go
        # instrumentation and the committed artifacts only through the
        # mutation inputs. Each change must stale the confirmation receipt.
        closure = producers.source_closure("foundations")
        tools = [name for name in closure if name.startswith(producers.MUTATION_TOOLS + "/")]
        for required in ("switch/src/lib.rs", "driver/src/jobs.rs", "splicer/src/operators.rs", "go/phase1_cover.go"):
            self.assertIn(f"{producers.MUTATION_TOOLS}/{required}", closure, required)
        self.assertTrue(any(name.startswith(scope.MUTATION_DIRECTORY + "/") and name.endswith(".json.gz")
                            for name in closure))
        self.assertIn(scope.MUTATION_MANIFEST, closure)
        self.assertFalse(any("/target/" in name for name in tools))
        self.assertLessEqual(producers.python_import_closure(producers.MUTATION_COMMANDS), set(closure))

    def test_receipt_closure_includes_every_module_the_mutation_commands_import(self):
        # H5: `comparable` (s07_binder), the protocol and its codec fixtures are
        # imported lazily or transitively; a change to any of them stales the receipt.
        imported = producers.python_import_closure(producers.MUTATION_COMMANDS)
        with patch.object(producers, "MUTATION_TOOLS", "tools/phase1/absent-for-this-test"):
            inputs = producers.mutation_inputs()
        for module in ("scripts/s07_binder.py", "scripts/s06_protocol.py", "scripts/s06_codec_fixtures.py",
                       "scripts/s08_oracle.py", "scripts/s04_common.py", *producers.MUTATION_COMMANDS):
            self.assertIn(module, imported)
        self.assertLessEqual(imported, inputs)
        self.assertNotIn("scripts/json.py", imported)


if __name__ == "__main__":
    unittest.main()
