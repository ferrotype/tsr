"""The shared renderer cannot change or replace the captured Rust result."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import phase1_capture as capture


class RendererTests(unittest.TestCase):
    def documents(self):
        request = {"case": "a", "operation": "tsoptions.parseBuildOptionsBaseline", "subject": "commandLineBaseline", "baseline": "a.js"}
        raw = {"case": "a", "operation": request["operation"], "result": "observed", "observation": {"compiler": ["struct", {"Strict": ["int", 0]}]}}
        final = copy.deepcopy(raw)
        final["observation"] = {"baseline": "a.js", "typed": raw["observation"], "rendered": "bytes", "rendered_sha256": capture.digest(b"bytes")}
        return {"version": 1, "requests": [request]}, raw, final

    def verify(self, mutate=None):
        requests, raw, final = self.documents()
        rendered = {"observations": [final]}
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "renderer").mkdir()
            documents = {"rust-raw-observations.json": {"observations": [raw]}, "renderer/requests.json": {**requests, "observations": [raw]}, "renderer/observations.json": rendered}
            # Independent decoded copies, as on replay.
            documents = json.loads(json.dumps(documents))
            if mutate:
                mutate(documents, rendered)
            for name, value in documents.items():
                (root / name).write_text(json.dumps(value))
            with patch.object(capture, "command", side_effect=AssertionError("replay spawned a child")):
                capture.validate_renderer(root, requests, rendered)

    def test_unchanged_rust_state_replays_without_children(self):
        self.verify()

    def test_bridge_cannot_replace_raw_input(self):
        def mutate(documents, _):
            documents["renderer/requests.json"]["observations"][0]["observation"] = {}
        with self.assertRaisesRegex(ValueError, "captured Rust results"):
            self.verify(mutate)

    def test_bridge_cannot_drop_a_typed_field_even_with_matching_bytes(self):
        def mutate(documents, rendered):
            rendered["observations"][0]["observation"]["typed"] = {}
            documents["renderer/observations.json"] = copy.deepcopy(rendered)
        with self.assertRaisesRegex(ValueError, "typed Rust result"):
            self.verify(mutate)

    def test_bridge_cannot_rewrite_rendered_output_afterwards(self):
        def mutate(_, rendered):
            rendered["observations"][0]["observation"]["rendered"] = "replaced"
        with self.assertRaisesRegex(ValueError, "renderer output"):
            self.verify(mutate)
