import copy
import json
import unittest

import audit


class InventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schema = json.loads((audit.ROOT / "data/s03/schema/ast.json").read_text())
        cls.go = "\n".join((audit.ROOT / path).read_text() for path in [
            "upstream/tsc/internal/ast/ast_generated.go", "upstream/tsc/internal/ast/ast.go"])
        cls.data = (audit.ROOT / "crates/tsr_ast/src/data_generated.rs").read_text()

    def test_transitive_partition_is_not_total_base_count(self):
        observed = audit.base_inventory(self.schema, self.go)
        self.assertEqual(observed["shape_count"], 192)
        self.assertEqual(observed["partition_counts"], {
            "other": 109, "flow_without_declaration_or_locals": 23,
            "declaration_without_flow_or_locals": 24,
            "flow_and_declaration_without_locals": 9, "locals_or_function": 27})
        self.assertEqual(observed["overlapping_base_totals"]["FlowNodeBase"], 44)
        self.assertEqual(observed["overlapping_base_totals"]["DeclarationBase"], 54)
        self.assertEqual(observed["binding_fields_outside_three_bases"], {
            "CaseOrDefaultClause": ["FallthroughFlowNode"]})
        self.assertEqual(observed["shapes_with_binding_fields"], 84)

    def test_missing_transitive_base_cannot_silently_change_group(self):
        schema = copy.deepcopy(self.schema)
        schema["nodes"][1]["baseTypes"].remove("FlowNodeBase")
        with self.assertRaisesRegex(ValueError, "transitive base mismatch"):
            audit.base_inventory(schema, self.go)

    def test_physical_go_embedding_must_agree(self):
        changed = self.go.replace("type Identifier struct {\n\tPrimaryExpressionBase\n\tFlowNodeBase",
                                  "type Identifier struct {\n\tPrimaryExpressionBase", 1)
        self.assertNotEqual(changed, self.go)
        with self.assertRaisesRegex(ValueError, "direct embeddings differ: Identifier"):
            audit.base_inventory(self.schema, changed)

    def test_multiple_inheritance_paths_are_rejected(self):
        bases = {"A": {"extends": []}, "B": {"extends": ["A"]}}
        with self.assertRaisesRegex(ValueError, "duplicate embedded base"):
            audit.closure(["A", "B"], bases)

    def test_direct_field_omission_is_rejected(self):
        changed = self.go.replace("\tFallthroughFlowNode *FlowNode\n", "", 1)
        self.assertNotEqual(changed, self.go)
        with self.assertRaisesRegex(ValueError, "binding fields differ: CaseOrDefaultClause"):
            audit.base_inventory(self.schema, changed)

    def test_generator_boxing_rule_is_checked(self):
        variants = audit.generated_inventory(self.data)
        self.assertEqual(len(variants), 192)
        changed = self.data.replace("Identifier(IdentifierData),", "Identifier(Box<IdentifierData>),", 1)
        with self.assertRaisesRegex(ValueError, "boxing budget differs: Identifier"):
            audit.generated_inventory(changed)

    def test_unrecognized_enum_variant_fails(self):
        changed = self.data.replace("Token(TokenData),", "Token { value: TokenData },", 1)
        with self.assertRaisesRegex(ValueError, "unknown enum entry"):
            audit.generated_inventory(changed)


if __name__ == "__main__":
    unittest.main()
