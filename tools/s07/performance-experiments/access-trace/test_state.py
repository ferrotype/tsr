import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest

import state


class StateExportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.raw = (state.BASE / state.SCHEMA).read_bytes()
        cls.shapes = state.schema(cls.raw.decode())

    def test_schema_covers_every_physical_payload_field_without_kind_aliases(self):
        self.assertEqual(hashlib.sha256(self.raw).hexdigest(), state.SCHEMA_SHA256)
        self.assertEqual(len(self.shapes), 192)
        fields = [field for shape in self.shapes for field in shape["fields"]]
        self.assertEqual(len(fields), 494)
        self.assertEqual([field["id"] for field in fields], list(range(1, 495)))
        self.assertEqual(self.shapes[1]["name"], "Identifier")
        self.assertEqual(sum(field["type"] == "NodeSlice" for field in fields), 2)
        self.assertEqual(sum(field["type"] == "TextSlice" for field in fields), 4)
        self.assertEqual(sum(field["type"] == "JsString" for field in fields), 15)
        self.assertEqual(state.registry(self.shapes), json.loads((state.HERE / "state-registry.json").read_text()))

    def test_unhandled_fields_and_shape_drift_do_not_silently_shrink_coverage(self):
        text = self.raw.decode()
        for changed in (
            text.replace("pub text: JsString,", "pub text: UnknownType,", 1),
            text.replace("pub text: JsString,", "pub text: JsString,\n    pub text: JsString,", 1),
            text.replace("pub text: JsString,", "private_text: JsString,", 1),
            text.replace("Token(TokenData),", "Token(NotAData),", 1),
        ):
            with self.assertRaises(ValueError):
                state.schema(changed)

    def test_generated_fields_preserve_bytes_and_every_descriptor_component(self):
        generated = state.generated(self.shapes)
        self.assertEqual(generated.count("NodeData::"), 384)
        self.assertEqual(generated.count(".as_bytes()"), 15)
        self.assertEqual(generated.count(".backing_id()"), 6)
        self.assertNotIn("runtime_node_id(", generated)
        self.assertNotIn("from_utf8", generated)
        self.assertNotIn("to_string", generated)
        self.assertNotIn("_ =>", generated)

    def test_staging_is_additive_and_refuses_original_or_duplicate_application(self):
        with self.assertRaisesRegex(ValueError, "production or frozen"):
            state.apply(state.ROOT)
        with self.assertRaisesRegex(ValueError, "production or frozen"):
            state.apply(state.BASE)
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary)
            for name in (*state.ADDITIONS, state.SCHEMA):
                target = stage / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(state.BASE / name, target)
            originals = {name: (stage / name).read_text() for name in state.ADDITIONS}
            changed = state.apply(stage)
            self.assertEqual(len(changed), len(state.ADDITIONS) + 2)
            for name, original in originals.items():
                self.assertTrue((stage / name).read_text().startswith(original))
            self.assertEqual((stage / state.SCHEMA).read_bytes(), self.raw)
            generated = stage / "crates/tsr_ast/src/access_trace_state_generated.rs"
            self.assertEqual(generated.read_text(), state.generated(self.shapes))
            with self.assertRaisesRegex(ValueError, "already applied"):
                state.apply(stage)

    def test_registry_declares_omissions_and_preserves_domain_and_blob_roles(self):
        registry = state.registry(self.shapes)
        events = registry["events"]
        self.assertEqual(len({event["id"] for event in events}), len(events))
        self.assertTrue(all(event["domain"] == 1 and 10 <= event["id"] < 80 for event in events))
        self.assertTrue(registry["coverage"]["omitted"])
        for event in events:
            self.assertEqual(set(event["fields"]), {"a", "b", "c", "d"})
            self.assertEqual(len(event["sites"]), len({site["id"] for site in event["sites"]}))
            if event["blob"]:
                self.assertEqual((event["fields"]["c"], event["fields"]["d"]), ("byte_offset", "logical_blob_bytes"))


if __name__ == "__main__":
    unittest.main()
