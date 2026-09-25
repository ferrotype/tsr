"""Mutation sites follow the scope's marker rules; spans, keys and the scratch copy are exact."""
import ast
import gzip
import hashlib
import inspect
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import unittest
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_mutation_plan as mutation
import phase1_scope as scope
import phase1_tables

ROOT = Path(__file__).resolve().parents[2]

# Shared with tools/phase1/mutation/splicer/src/source.rs: both sides must agree.
FIXTURE = b"fn a() {}\n// port: tsc/x.go:B\nfn b() -> bool {\n    true\n}\n"
FIXTURE_SPAN_SHA256 = "6e551632f6e6d937576fdb5620d1506633db7d318a675812e26c0eb79045ea82"
FIXTURE_KEY = "bb20205aa0809b53"

SAMPLE = """\
impl Parser {
    /// Parses a thing.
    /// port: tsc/internal/parser/parser.go:Parser.parseThing
    /// port: tsc/internal/parser/parser.go:Parser.parseOther
    #[inline]
    pub(crate) fn parse_thing(&mut self) -> NodeId {
        match self.token {
            // port: tsc/internal/ast/ast.go:Arm.computeSubtreeFacts
            K::A => NONE,
            _ => NONE,
        }
    }
}
// port: tsc/internal/ast/utilities.go:IsEmittableImport
#[allow(
    clippy::match_same_arms,
)]
fn emittable() -> bool { false }
// port: tsc/internal/ast/utilities.go:Trailing (a note)
macro_rules! reads {
    () => {
        /// port: tsc/internal/ast/ast.go:Node.Expression
        $visibility fn expression(&self) -> Option<$node_id> { None }
    };
}
"""


def write(root: Path, files: dict) -> None:
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


class MarkerRuleTests(unittest.TestCase):
    def test_line_prefixes_are_the_scopes_own(self):
        source = inspect.getsource(scope.unmapped_ids)
        found = re.search(r'for prefix in (\("/// port:"[^)]*\))', source)
        if found is None:
            self.assertIn("_MARKER_PREFIXES", source, "phase1_scope.unmapped_ids changed its prefix rule")
            self.assertEqual(scope._MARKER_PREFIXES, mutation.MARKER_PREFIXES)
        else:
            self.assertEqual(ast.literal_eval(found.group(1)), mutation.MARKER_PREFIXES)
        if hasattr(scope, "_MARKER_PREFIXES"):
            self.assertEqual(tuple(scope._MARKER_PREFIXES), mutation.MARKER_PREFIXES)

    def test_function_markers_are_exactly_the_scope_annotations(self):
        found = mutation.markers(SAMPLE)
        functions = [(m["op"], m["function"]) for m in found if m["kind"] == "fn"]
        self.assertEqual(sorted(functions), sorted(scope._PORT_ANNOTATION.findall(SAMPLE)))
        self.assertEqual(found, [
            {"line": 3, "op": "tsc/internal/parser/parser.go:Parser.parseThing", "kind": "fn",
             "function": "parse_thing", "fn_line": 6},
            {"line": 4, "op": "tsc/internal/parser/parser.go:Parser.parseOther", "kind": "fn",
             "function": "parse_thing", "fn_line": 6},
            {"line": 8, "op": "tsc/internal/ast/ast.go:Arm.computeSubtreeFacts", "kind": "statement"},
            # Above a multi-line attribute the scope attributes nothing: a
            # statement marker that the splicer then reports as unresolved.
            {"line": 14, "op": "tsc/internal/ast/utilities.go:IsEmittableImport", "kind": "statement"},
            # xtask maps the whole remainder, so trailing text is not this op.
            {"line": 19, "op": "tsc/internal/ast/utilities.go:Trailing (a note)", "kind": "statement"},
            {"line": 22, "op": "tsc/internal/ast/ast.go:Node.Expression", "kind": "fn",
             "function": "expression", "fn_line": 23},
        ])


class TestOnlyCodeTests(unittest.TestCase):
    def test_mask_blanks_comments_strings_and_characters_only(self):
        cases = {
            "a // x {\nb": "a       \nb",
            'let s = "a{\\"}"; z': "let s = " + " " * 7 + "; z",
            "let c = '{'; fn f<'a>(x: &'a u8) {}": "let c =    ; fn f<'a>(x: &'a u8) {}",
            'r#"a"{"# b': "         b",
            'br"{" c': "      c",
            "b'{' '\\u{7b}' '\\'' d": " " * 19 + "d",
        }
        for text, expected in cases.items():
            self.assertEqual(mutation.code_mask(text), expected, text)
        nested = "x /* a /* b */ { */ y"
        self.assertEqual(mutation.code_mask(nested), "x " + " " * (len(nested) - 4) + " y")

    def test_predicates(self):
        self.assertTrue(mutation.test_only("test"))
        self.assertTrue(mutation.test_only("all(test, not(miri))"))
        self.assertFalse(mutation.test_only('any(test, feature = "harness")'))
        self.assertFalse(mutation.test_only("not(test)"))

    def test_regions_cover_test_items_and_nothing_else(self):
        text = "\n".join([
            "fn keep() {}",                          # 1
            "#[cfg(test)]",                          # 2
            "mod tests {",                           # 3
            "    fn t() { let s = \"}\"; }",         # 4
            "}",                                     # 5
            '#[cfg(any(test, feature = "x"))]',      # 6
            "fn shared() {}",                        # 7
            "#[cfg(all(test, unix))]",               # 8
            "#[inline]",                             # 9
            "fn unix_test() {",                      # 10
            "}",                                     # 11
            "#[test]",                               # 12
            "fn a_test() {}",                        # 13
        ]) + "\n"
        self.assertEqual(mutation.test_regions(text), [(2, 5), (8, 11), (12, 13)])
        self.assertEqual(mutation.test_regions("#![cfg(test)]\nfn a() {}\n"), [(1, 2)])

    def test_module_files_follow_declarations_and_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, {
                "crates/a/src/lib.rs": '#[cfg(test)]\nmod tests;\nmod real;\n#[cfg(test)]\n#[path = "x_tests.rs"]\n'
                                       'mod x;\n#[cfg(any(test, feature = "h"))]\nmod shared;\n',
                "crates/a/src/tests.rs": "mod helper;\n",
                "crates/a/src/tests/helper.rs": "",
                "crates/a/src/x_tests.rs": "",
                "crates/a/src/real.rs": "#[cfg(test)]\nmod inner;\n",
                "crates/a/src/real/inner.rs": "",
                "crates/a/src/shared.rs": "",
            })
            self.assertEqual(mutation.test_module_files(root), {
                "crates/a/src/tests.rs", "crates/a/src/tests/helper.rs", "crates/a/src/x_tests.rs",
                "crates/a/src/real/inner.rs"})


class SiteTests(unittest.TestCase):
    def test_sites_cover_src_only_and_explain_every_missing_operation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write(root, {
                "crates/a/src/lib.rs": SAMPLE + "#[cfg(test)]\nmod tests {\n    /// port: tsc/t.go:TestOnly\n"
                                                "    fn t() -> bool { true }\n}\n",
                "crates/a/tests/it.rs": "/// port: tsc/t.go:Elsewhere\nfn e() {}\n",
                "crates/a/examples/ex.rs": "/// port: tsc/t.go:Elsewhere\nfn e() {}\n",
            })
            ops = ["tsc/internal/parser/parser.go:Parser.parseThing", "tsc/internal/ast/ast.go:Arm.computeSubtreeFacts",
                   "tsc/t.go:TestOnly", "tsc/t.go:Elsewhere", "tsc/internal/ast/utilities.go:Trailing"]
            found, unsited = mutation._resolve(root, ops)
            self.assertEqual(found, [
                {"op": "tsc/internal/parser/parser.go:Parser.parseThing", "file": "crates/a/src/lib.rs",
                 "marker_line": 3, "marker_kind": "fn", "function": "parse_thing", "fn_line": 6},
                {"op": "tsc/internal/ast/ast.go:Arm.computeSubtreeFacts", "file": "crates/a/src/lib.rs",
                 "marker_line": 8, "marker_kind": "statement"},
            ])
            self.assertEqual(found, mutation.sites(root, ops))
            self.assertEqual([(entry["op"], entry["reason"].split(":")[0]) for entry in unsited], [
                ("tsc/internal/ast/utilities.go:Trailing", "no_marker"),
                ("tsc/t.go:Elsewhere", "no_marker"),
                ("tsc/t.go:TestOnly", "cfg_test_only"),
            ])
            self.assertIn("crates/a/src/lib.rs:28", unsited[2]["reason"])

    def test_every_default_operation_is_sited_or_explained(self):
        ops = mutation.default_ops()
        report = json.loads(gzip.decompress((ROOT / mutation.COVERAGE_REPORT).read_bytes()))
        claimed = {op for witness in report.get("witnesses", []) if witness.get("kind") == "mutation_kill"
                   for op in witness["claimed_operations"]}
        claimed |= set(phase1_tables.claimed_operations(phase1_tables.load_specs(root=ROOT)))
        self.assertEqual(ops, sorted({row["id"] for row in report["operations"]
                                      if row.get("root_cause") in ("operation_witness_missing",
                                                                   "mutation_witness_stale")} | claimed))
        # Recording takes witnessed operations out of the witness-missing set;
        # they stay planned, so re-planning reproduces the committed manifest's
        # operations.
        manifest = ROOT / "data/phase1/mutation/manifest.json"
        if manifest.is_file():
            self.assertEqual(ops, sorted(json.loads(manifest.read_bytes())["homes"]))
        found, unsited = mutation._resolve(ROOT, ops)
        self.assertEqual({site["op"] for site in found} | {entry["op"] for entry in unsited}, set(ops))
        self.assertFalse({site["op"] for site in found} & {entry["op"] for entry in unsited})
        texts = {}
        for site in found:
            lines = texts.setdefault(site["file"], (ROOT / site["file"]).read_text().split("\n"))
            self.assertTrue(site["file"].startswith("crates/") and "/src/" in site["file"], site)
            self.assertIn(f"port: {site['op']}", lines[site["marker_line"] - 1], site)
            if site["marker_kind"] == "fn":
                self.assertRegex(lines[site["fn_line"] - 1], rf"\bfn\s+{site['function']}\b", site)


class DigestTests(unittest.TestCase):
    def test_span_digest_and_key_match_the_splicer(self):
        self.assertEqual(mutation.span_bytes(FIXTURE, 2, 5), b"// port: tsc/x.go:B\nfn b() -> bool {\n    true\n}\n")
        self.assertEqual(mutation.span_sha256(FIXTURE, 2, 5), FIXTURE_SPAN_SHA256)
        self.assertEqual(mutation.mutant_key("tsc/x.go:B", "crates/x/src/lib.rs", "b", 3, "return:true",
                                             FIXTURE_SPAN_SHA256), FIXTURE_KEY)
        if hasattr(scope, "span_digest"):  # the coverage binding's own reading of a span
            self.assertEqual(scope.span_digest(FIXTURE, 2, 5), FIXTURE_SPAN_SHA256)
            self.assertEqual(scope.span_digest(b"a\nb", 2, 2), mutation.span_sha256(b"a\nb", 2, 2))
        self.assertIsNone(mutation.span_bytes(FIXTURE, 0, 1))
        self.assertIsNone(mutation.span_bytes(FIXTURE, 3, 6))
        self.assertEqual(mutation.span_bytes(b"a\nb", 2, 2), b"b")

    def test_span_is_found_again_after_unrelated_edits_but_not_after_its_own(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "crates/x/src/lib.rs"
            path.parent.mkdir(parents=True)
            path.write_bytes(FIXTURE)
            mutant = {"file": "crates/x/src/lib.rs", "span": [2, 5], "span_sha256": FIXTURE_SPAN_SHA256,
                      "markers": {"tsc/x.go:B": 2}}
            self.assertTrue(mutation.span_intact(root, mutant))
            path.write_bytes(b"// new\n// lines\n" + FIXTURE + b"fn c() {}\n")
            self.assertTrue(mutation.span_intact(root, mutant), "moved, not edited")
            path.write_bytes(FIXTURE.replace(b"true", b"false"))
            self.assertFalse(mutation.span_intact(root, mutant))
            path.write_bytes(FIXTURE.replace(b"tsc/x.go:B", b"tsc/x.go:C"))
            self.assertFalse(mutation.span_intact(root, mutant), "the marker is gone")
            path.unlink()
            self.assertFalse(mutation.span_intact(root, mutant))


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout


def repository(root: Path) -> None:
    git(root, "init", "-q")
    git(root, "config", "user.email", "test@example.com")
    git(root, "config", "user.name", "Test")
    write(root, {
        "Cargo.toml": "[workspace]\n", "crates/a/src/lib.rs": "fn a() {}\n", "data/big.json": "{}\n",
        ".gitignore": "/target\n", "tools/run.sh": "#!/bin/sh\n",
    })
    os.chmod(root / "tools/run.sh", 0o755)
    os.symlink("lib.rs", root / "crates/a/src/link.rs")
    git(root, "add", "-A")
    git(root, "update-index", "--add", "--cacheinfo", "160000,1f70213d4922b434345f639b441681e470c7cfc1,upstream")
    (root / "upstream").mkdir()
    git(root, "commit", "-q", "-m", "fixture")


class TreeTests(unittest.TestCase):
    def test_tree_id_equals_git_for_a_clean_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repository(root)
            self.assertEqual(mutation.source_tree_sha(root), git(root, "rev-parse", "HEAD^{tree}").strip())
            write(root, {"target/debug/out": "ignored", "notes.md": "untracked"})
            self.assertIn("notes.md", mutation.source_files(root))
            self.assertNotIn("target/debug/out", mutation.source_files(root))
            self.assertNotIn("upstream", mutation.source_files(root))
            self.assertNotEqual(mutation.source_tree_sha(root), git(root, "rev-parse", "HEAD^{tree}").strip())

    def test_sync_mirrors_sources_links_data_and_keeps_the_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "src"
            ws = Path(directory) / "ws"
            root.mkdir()
            repository(root)
            write(ws, {"stale.rs": "x", "crates/gone/src/lib.rs": "x", "target/keep": "build"})
            mutation._sync(root, ws)
            self.assertEqual((ws / "crates/a/src/lib.rs").read_text(), "fn a() {}\n")
            self.assertEqual(os.readlink(ws / "crates/a/src/link.rs"), "lib.rs")
            self.assertTrue(os.access(ws / "tools/run.sh", os.X_OK))
            self.assertEqual(Path(os.readlink(ws / "data")), root / "data")
            self.assertEqual(Path(os.readlink(ws / "upstream")), root / "upstream")
            self.assertFalse((ws / "stale.rs").exists())
            self.assertFalse((ws / "crates/gone").exists())
            self.assertEqual((ws / "target/keep").read_text(), "build")
            # Unchanged files keep their mtime; a spliced file gets the original back, fresh.
            old = time.time() - 1000
            os.utime(ws / "Cargo.toml", (old, old))
            (ws / "crates/a/src/lib.rs").write_text("fn a() {if ::phase1_mutants::hit(1) { return; } }\n")
            os.utime(ws / "crates/a/src/lib.rs", (old, old))
            mutation._sync(root, ws)
            self.assertEqual(os.stat(ws / "Cargo.toml").st_mtime, old)
            self.assertEqual((ws / "crates/a/src/lib.rs").read_text(), "fn a() {}\n")
            self.assertGreater(os.stat(ws / "crates/a/src/lib.rs").st_mtime, old + 500)

    def test_schemata_refuses_a_directory_that_is_not_a_mutation_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "src"
            root.mkdir()
            repository(root)
            other = Path(directory) / "other"
            write(other, {"precious.txt": "keep"})
            with self.assertRaisesRegex(ValueError, "not a mutation workspace"):
                mutation.schemata(root, root / "plan.json", other)
            with self.assertRaisesRegex(ValueError, "scratch workspace"):
                mutation.schemata(root, root / "plan.json", root)
            self.assertEqual((other / "precious.txt").read_text(), "keep")


SPLICER = ROOT / "target/debug" / mutation.SPLICER


@unittest.skipUnless(SPLICER.is_file(), "build the splicer: cargo build -p phase1_mutation_splicer")
class EndToEndTests(unittest.TestCase):
    def test_plan_and_splice_a_synthetic_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "src"
            root.mkdir()
            repository(root)
            write(root, {
                "crates/a/Cargo.toml": '[package]\nname = "a"\n\n[dependencies]\n',
                "crates/a/src/lib.rs": "/// port: tsc/a.go:Flag\npub fn flag(x: u32) -> bool {\n    x > 1\n}\n",
                "tools/phase1/mutation/switch/Cargo.toml": '[package]\nname = "phase1_mutants"\n',
            })
            git(root, "add", "-A")
            git(root, "commit", "-q", "-m", "crate")
            ops = ["tsc/a.go:Flag", "tsc/a.go:Missing"]
            out = Path(directory) / "plan" / "plan.json"
            document = mutation.plan(root, ops, out, splicer=SPLICER)
            self.assertEqual(document["root_tree"], git(root, "rev-parse", "HEAD^{tree}").strip())
            self.assertEqual([m["operator"] for m in document["mutants"]], ["return:true", "return:false"])
            self.assertEqual([m["id"] for m in document["mutants"]], [1, 2])
            self.assertEqual([m["control"] for m in document["mutants"]], [None, None], "nothing allocates")
            self.assertEqual(document["unsupported"][0]["op"], "tsc/a.go:Missing")
            self.assertEqual(document["homes"]["tsc/a.go:Missing"], [], "no marker, no home")
            [home] = document["homes"]["tsc/a.go:Flag"]
            self.assertEqual({key: home[key] for key in ("file", "function", "site_kind", "site_line", "marker_line",
                                                         "span", "reason")},
                             {"file": "crates/a/src/lib.rs", "function": "flag", "site_kind": "fn", "site_line": 2,
                              "marker_line": 1, "span": [1, 4], "reason": None})
            self.assertEqual(home["mutants"], [m["key"] for m in document["mutants"]])
            self.assertEqual(home["span_sha256"], document["mutants"][0]["span_sha256"])
            sources = scope.mutation_sources(root) if hasattr(scope, "mutation_site_problem") else None
            for mutant in document["mutants"]:
                self.assertTrue(mutation.span_intact(root, mutant))
                if sources is not None:  # the coverage binding accepts every planned site
                    self.assertIsNone(scope.mutation_site_problem(mutant, mutant["op"], sources))
                self.assertEqual(mutant["key"], mutation.mutant_key(
                    mutant["op"], mutant["file"], mutant["function"], mutant["site_line"], mutant["operator"],
                    mutant["span_sha256"]))
            ws = Path(directory) / "ws"
            self.assertEqual(mutation.schemata(root, out, ws, splicer=SPLICER), ws.resolve())
            spliced = (ws / "crates/a/src/lib.rs").read_text()
            self.assertEqual(spliced.split("\n")[1], "pub fn flag(x: u32) -> bool {if ::phase1_mutants::hit(1) "
                             "{ return true; } if ::phase1_mutants::hit(2) { return false; } ")
            self.assertIn('phase1_mutants = { path = "../../tools/phase1/mutation/switch" }',
                          (ws / "crates/a/Cargo.toml").read_text())
            marker = json.loads((ws / mutation.WS_MARKER).read_text())
            self.assertEqual(marker["state"], "spliced")
            self.assertEqual(marker["plan_sha256"], hashlib.sha256(out.read_bytes()).hexdigest())
            self.assertEqual((root / "crates/a/src/lib.rs").read_text().count("phase1_mutants"), 0)
            # Re-splicing the same plan restores the originals first.
            mutation.schemata(root, out, ws, splicer=SPLICER)
            self.assertEqual((ws / "crates/a/src/lib.rs").read_text(), spliced)
            # A source edit inside a planned span refuses the splice.
            (root / "crates/a/src/lib.rs").write_text((root / "crates/a/src/lib.rs").read_text().replace("> 1", ">= 1"))
            with self.assertRaises(subprocess.CalledProcessError):
                mutation.schemata(root, out, ws, splicer=SPLICER)

    def test_real_plan_pairs_controls_and_matches_the_scopes_homes(self):
        """The plan of the witness-missing operations over the real tree: every
        allocating replacement has its control, and the homes are exactly the
        marker sites the coverage binding reads from the live sources."""
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "plan.json"
            ops = mutation.default_ops()
            document = mutation.plan(ROOT, ops, out, splicer=SPLICER)  # runs check_plan
            self.assertEqual(set(document["homes"]), set(ops))
            entries = document["mutants"]
            controls = [m for m in entries if mutation.is_control(m)]
            self.assertEqual(document["counts"]["controls"], len(controls))
            self.assertEqual(document["counts"]["mutants"], len(entries) - len(controls))
            self.assertTrue(controls, "the parser's missing-node wrappers allocate")
            for mutant in controls:
                self.assertEqual(mutant["key"], mutation.mutant_key(
                    mutant["op"], mutant["file"], mutant["function"], mutant["site_line"], "control",
                    mutant["span_sha256"]))
                texts = "".join(insert["text"] for insert in mutant["insert"])
                self.assertIn("{ let _ = ", texts, "a control computes the replacement")
                self.assertNotIn("{ return ", texts, "and returns the real result")
            if hasattr(scope, "allocates"):
                self.assertEqual(scope._ALLOCATING.pattern, mutation._ALLOCATING.pattern)
                for mutant in entries:
                    if not scope.is_control(mutant):
                        self.assertEqual(scope.allocates(mutant), mutant["control"] is not None, mutant["id"])
            if hasattr(scope, "marker_homes"):
                sources = scope.mutation_sources(ROOT)
                for op, homes in document["homes"].items():
                    functions, markers = scope.marker_homes(sources, op)
                    self.assertEqual({(h["file"], h["function"]) for h in homes
                                      if h["site_kind"] in ("fn", "macro_fn")}, functions, op)
                    self.assertEqual(Counter(h["file"] for h in homes if h["site_kind"] not in ("fn", "macro_fn")),
                                     markers, op)
                    if hasattr(scope, "home_identity"):
                        identities = [scope.home_identity(h) for h in homes]
                        self.assertEqual(len(set(identities)), len(identities), op)
            for mutant in entries:
                self.assertTrue(mutation.span_intact(ROOT, mutant), mutant["id"])
            # tsr_parser ports of internal/ast utilities (NodeIsMissing,
            # GetNodeAtPosition, ...) are production-only sites.
            parser_hit = {m["function"] for m in entries
                          if m["file"].startswith(mutation.PARSER_SOURCES) and m["hit_fn"] == "hit"}
            self.assertIn("node_is_missing", parser_hit)
            self.assertTrue(all(op.startswith(mutation.PARSER_OPERATIONS)
                                for m in entries if m["hit_fn"] == "hit_parser" for op in m["ops"]))


A = "tsc/internal/parser/parser.go:Parser.a"
B = "tsc/internal/parser/parser.go:Parser.b"


def plan_document(*changes):
    """A two-operation plan: an allocating wrapper with its control and a guard
    at one tsr_parser site of A, and B without a marker. `changes` are (path,
    value) pairs."""
    alloc = {"id": 1, "key": "k1", "op": A, "ops": [A], "file": "crates/tsr_parser/src/lib.rs",
             "function": "a", "site_kind": "fn", "site_line": 3, "markers": {A: 2}, "span": [2, 5],
             "span_sha256": "0" * 64, "crate": "tsr_parser", "hit_fn": "hit_parser", "return_category": "node_id",
             "operator": "wrap_result:self.create_missing_identifier()", "control": 2,
             "insert": [{"line": 3, "column": 40, "order": 1,
                         "text": "if ::phase1_mutants::hit_parser(1) { return self.create_missing_identifier(); }"}]}
    control = dict(alloc, id=2, key="k2", operator="control", control=None, control_of=1, insert=[
        {"line": 3, "column": 40, "order": 2,
         "text": "if ::phase1_mutants::hit_parser(2) { let _ = self.create_missing_identifier(); }"}])
    guard = dict(alloc, id=3, key="k3", operator="return:true", control=None, return_category="bool", insert=[
        {"line": 3, "column": 40, "order": 3, "text": "if ::phase1_mutants::hit_parser(3) { return true; }"}])
    home = {"file": "crates/tsr_parser/src/lib.rs", "function": "a", "site_kind": "fn", "site_line": 3,
            "span": [2, 5], "span_sha256": "0" * 64, "marker_line": 2, "mutants": ["k1", "k3"], "reason": None}
    document = {"mutants": [alloc, control, guard], "homes": {A: [home], B: []},
                "unsupported": [{"op": B, "file": None, "function": None, "reason": "no_marker"}]}
    for path, value in changes:
        node = document
        for part in path[:-1]:
            node = node[part]
        node[path[-1]] = value
    return document


class CheckPlanTests(unittest.TestCase):
    OPS = [A, B]

    def test_a_consistent_plan_passes(self):
        mutation.check_plan(plan_document(), self.OPS)

    def test_each_invariant_can_fail(self):
        home = plan_document()["homes"][A][0]
        cases = {
            "allocation and control disagree": [(("mutants", 2, "operator"), "wrap_result:self.new_identifier(x)")],
            "not paired with a mutant at its site": [(("mutants", 1, "span"), [2, 6])],
            "control 2 is not paired": [(("mutants", 0, "control"), 3)],
            "names k2, which is not a planned mutant": [(("homes", A, 0, "mutants"), ["k1", "k2", "k3"])],
            "not in exactly one home": [(("homes", A, 0, "mutants"), ["k1"])],
            "no mutant and no reason": [(("homes", A), [dict(home, mutants=[])]),
                                        (("homes", B), [])],
            "have no home list": [(("homes",), {A: [home]})],
            "malformed home": [(("homes", B), [dict(home, site_kind="item", mutants=[], reason="x")])],
            "ids are not": [(("mutants", 2, "id"), 7)],
            "unaccounted": [(("unsupported",), [])],
            # hit_parser stays live while observing, where Go counts only
            # internal/parser entries: not for a site shared with an
            # internal/ast operation, nor outside tsr_parser.
            "3 calls hit_parser, but its site allows only hit": [
                (("mutants", 2, "ops"), [A, "tsc/internal/ast/utilities.go:NodeIsMissing"])],
            "calls hit_parser, but its site allows only hit": [(("mutants", 2, "file"), "crates/tsr_ast/src/lib.rs")],
            "calls hit, but its site allows only hit_parser": [(("mutants", 2, "hit_fn"), "hit")],
            r"switch calls \[\('hit', '3'\)\] are not hit_parser\(3\)": [
                (("mutants", 2, "insert", 0, "text"), "if ::phase1_mutants::hit(3) { return true; }")],
            r"switch calls \[\('hit_parser', '1'\)\] are not hit_parser\(3\)": [
                (("mutants", 2, "insert", 0, "text"), "if ::phase1_mutants::hit_parser(1) { return true; }")],
            r"switch calls \[\] are not": [(("mutants", 2, "insert", 0, "text"), "return true;")],
            r"switch calls \[\('hit_parser', '3'\), \('active', None\)\] are not": [
                (("mutants", 2, "insert", 0, "text"),
                 "if ::phase1_mutants::hit_parser(3) { return ::phase1_mutants::active() == 3; }")],
        }
        for message, changes in cases.items():
            with self.subTest(message), self.assertRaisesRegex(ValueError, message):
                mutation.check_plan(plan_document(*changes), self.OPS)

    def test_only_parser_sites_of_parser_operations_call_hit_parser(self):
        ast_op = "tsc/internal/ast/utilities.go:NodeIsMissing"
        cases = [
            ("crates/tsr_parser/src/lib.rs", [A], "hit_parser"),
            ("crates/tsr_parser/src/lib.rs", [A, B], "hit_parser"),
            ("crates/tsr_parser/src/references.rs", [ast_op], "hit"),
            ("crates/tsr_parser/src/lib.rs", [A, ast_op], "hit"),
            ("crates/tsr_ast/src/lib.rs", [A], "hit"),
            ("crates/tsr_parser/src/lib.rs", [], "hit"),
        ]
        for file, ops, expected in cases:
            with self.subTest(file=file, ops=ops):
                self.assertEqual(mutation.hit_function({"file": file, "ops": ops}), expected)

    def test_parser_rule_is_the_go_observation_rule(self):
        """hit_parser keeps a mutant live while observing; that is sound only
        for operations the Go reach counts in observation segments."""
        import phase1_mutation_go as go
        observing = {name: spec.observe_counts for name, spec in go.ORACLES.items() if spec.observe_counts}
        self.assertEqual(sorted(observing), ["e1"])
        self.assertTrue(mutation.PARSER_OPERATIONS.startswith(tuple(observing["e1"])))
        self.assertTrue((ROOT / "upstream/tsc" / mutation.PARSER_OPERATIONS.removeprefix("tsc/")).is_dir())
        self.assertTrue((ROOT / mutation.PARSER_SOURCES).is_dir())

    def test_allocation_rule_matches_the_scope(self):
        for text, expected in [("self.create_missing_identifier()", True), ("f.new_identifier (x)", True),
                               ("Vec::new()", False), ("renew_x()", False), ("self.create_missing_list", False)]:
            self.assertEqual(bool(mutation._ALLOCATING.search(text)), expected, text)
        if hasattr(scope, "_ALLOCATING"):
            self.assertEqual(scope._ALLOCATING.pattern, mutation._ALLOCATING.pattern)


if __name__ == "__main__":
    unittest.main()
