"""The generated AST probe dispatches production APIs, never models answers."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / "tools/phase1/syntax/ast-generated"
SPEC = importlib.util.spec_from_file_location("generated_ast_fixture", BASE / "generate.py")
GEN = importlib.util.module_from_spec(SPEC)
# The adapter source closure must not acquire interpreter cache artifacts.
exec(compile((BASE / "generate.py").read_text(), str(BASE / "generate.py"), "exec"), GEN.__dict__)


class GeneratedAstFixtureTests(unittest.TestCase):
    def test_outputs_are_regenerated_without_expected_values(self):
        for name, source in GEN.outputs().items():
            self.assertEqual((BASE / name).read_text(), source, name)
        document = GEN.document()
        self.assertEqual(len(document["requests"]), 1035)
        self.assertTrue(all("expected" not in row for row in document["requests"]))

    def test_every_pinned_predicate_has_an_exact_native_and_rust_dispatch(self):
        dispatch = GEN.predicate_dispatch()
        self.assertEqual(len(dispatch), 232)
        self.assertEqual(len({name for name, _, _ in dispatch}), len(dispatch))
        requests = [r for r in GEN.document()["requests"] if r["subject"] == "generatedPredicate"]
        self.assertEqual({r["predicate"] for r in requests}, {name for name, _, _ in dispatch})
        schema = json.loads((ROOT / "data/s03/schema/ast.json").read_text())
        for row in requests:
            self.assertEqual(row["first_kind"], -1)
            self.assertEqual(row["last_kind"], max(k["value"] for k in schema["kinds"]) + 1)
            self.assertEqual(row["extra_kinds"], [-32768, 32767])
            self.assertEqual(row["operations"], [row["operation"]])

    def test_shape_claims_are_bound_to_individual_action_calls(self):
        requests = [r for r in GEN.document()["requests"] if r["subject"] == "generatedAst"]
        self.assertEqual(len(requests), 26)
        self.assertEqual({r["shape"] for r in requests}, set(GEN.SHAPES))
        for row in requests:
            actions = row["operation_actions"]
            self.assertEqual(set(actions), {action["op"] for action in row["actions"]})
            self.assertEqual(actions["counts"], ["tsc/internal/ast/ast.go:NodeFactory.NodeCount",
                                                 "tsc/internal/ast/ast.go:NodeFactory.TextCount"])
            self.assertEqual(set(row["operations"]), {op for ops in actions.values() for op in ops})
            # Only a changed update runs updateNode.
            self.assertEqual(actions["update-same"] + ["tsc/internal/ast/ast.go:updateNode"],
                             actions["update-changed"])
            self.assertEqual(actions["visit-same"], actions["visit-replace"])
            self.assertEqual("facts" in actions, row["shape"] in ("Block", "QualifiedName"))
        dynamic = [r for r in requests if r["shape"] == "JSDocParameterOrPropertyTag"]
        self.assertEqual({r["name_first"] for r in dynamic}, {False, True})
        self.assertEqual({r["list"] for r in dynamic}, {"nil", "empty", "nodes", "nil-element"})

    def test_every_generated_shape_binds_each_changed_field_to_its_update(self):
        shapes = GEN._shape_module["inventory"]()
        self.assertEqual(len(shapes), 190)
        requests = [r for r in GEN.document()["requests"] if r["subject"] == "generatedShape"]
        self.assertEqual(len(requests), 760)
        self.assertEqual({r["mode"] for r in requests}, {"nil", "empty", "nodes", "nil-element"})
        for shape in shapes:
            rows = [r for r in requests if r["shape"] == shape["name"]]
            self.assertEqual(len(rows), 4)
            for row in rows:
                links = row["operation_actions"]
                self.assertEqual(set(links), {action["op"] for action in row["actions"]})
                self.assertEqual(set(row["operations"]), {op for ops in links.values() for op in ops})
                for member in shape["update_members"]:
                    self.assertEqual(links["update-" + member["name"]],
                                     links["update-same"] + ["tsc/internal/ast/ast.go:updateNode"])
        self.assertEqual(set(GEN._shape_module["SKIPPED"]), {"SourceFile", "SyntheticExpression"})

    def test_lift_coverage_requires_syntax_list_results(self):
        rows = GEN.document()["requests"]
        operation = "tsc/internal/ast/visitor.go:NodeVisitor.liftToBlock"
        witnesses = [row for row in rows if operation in row["operations"]]
        self.assertEqual({row["shape"] for row in witnesses}, {"EmbeddedStatement"})
        self.assertEqual({row["mode"] for row in witnesses}, {"empty", "one", "many"})
        for row in witnesses:
            self.assertIn(operation, row["operation_actions"]["visit"])
        boundaries = [row for row in rows if row.get("shape") == "EmbeddedStatement"]
        self.assertEqual({row["mode"] for row in boundaries},
                         {"absent", "removed", "unchanged", "empty", "one", "many"})

    def test_missing_rust_dispatch_is_not_silently_omitted(self):
        original = Path.read_text
        def read(path, *args, **kwargs):
            text = original(path, *args, **kwargs)
            if path == ROOT / "crates/tsr_ast/src/runtime_generated.rs":
                text = text.replace("ast_generated.go:IsAwaitExpression", "ast_generated.go:RemovedAwaitExpression")
            return text
        with patch.object(Path, "read_text", read), self.assertRaisesRegex(ValueError, "lacks production Rust dispatch"):
            GEN.predicate_dispatch()

    def test_unknown_native_signature_fails_instead_of_shrinking_inventory(self):
        original = Path.read_text
        def read(path, *args, **kwargs):
            text = original(path, *args, **kwargs)
            if path == ROOT / "upstream/tsc/internal/ast/ast_generated.go":
                text = text.replace("func IsAwaitExpression(node *Node)", "func IsAwaitExpression(other *Node)")
            return text
        with patch.object(Path, "read_text", read), self.assertRaisesRegex(ValueError, "signature inventory changed"):
            GEN.predicate_dispatch()


if __name__ == "__main__":
    unittest.main()
