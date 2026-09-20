"""Drift checks plus executable tests of the exact staged helper bodies.

The tiny Rust fixture checks observation ordering/single evaluation, not real
AST ownership or whole-binder parity; the staged driver must check those too.
"""
import hashlib
import importlib.util
import json
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
FROZEN = ROOT / "target/s07-bis/cp1-node-read-candidate/source"
spec = importlib.util.spec_from_file_location("access_trace_hooks", HERE / "hooks.py")
hooks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hooks)


def function(source, name):
    """Extract one full named function, balancing braces in these fixed bodies."""
    matches = list(re.finditer(r"^    pub(?:\(crate\))? fn " + name + r"\(", source, re.M))
    if len(matches) != 1:
        raise ValueError(f"expected one function: {name}")
    start = matches[0].start()
    opening = source.index("{", start)
    depth = 1
    for index in range(opening + 1, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if not depth:
                return source[start:index + 1]
    raise ValueError(f"unterminated function: {name}")


class HookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="s07-hook-test-")
        self.addCleanup(self.temp.cleanup)
        self.stage = Path(self.temp.name) / "stage"
        self.original = {}
        for name in hooks.SOURCE_SHA256:
            data = (FROZEN / name).read_bytes()
            self.original[name] = data
            path = self.stage / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)

    def test_exact_source_and_registry_match_all_emitted_sites(self):
        unrelated = self.stage / "unchanged.rs"
        unrelated.write_text("retain this file\n")
        changed = hooks.apply(self.stage)
        self.assertEqual(changed, sorted(hooks.SOURCE_SHA256))
        self.assertEqual(unrelated.read_text(), "retain this file\n")
        emitted = []
        for name in changed:
            text = (self.stage / name).read_text()
            emitted.extend((int(op), int(site)) for op, site in re.findall(
                r"tsr_ast::access_trace::event\(\s*(\d+),\s*(\d+),", text))
        registry = json.loads((HERE / "hooks-registry.json").read_text())
        declared = [(event["id"], site["id"]) for event in registry["events"]
                    for site in event["sites"]]
        self.assertEqual(sorted(emitted), sorted(declared))
        self.assertEqual(len(set(emitted)), 11)
        for event in registry["events"]:
            self.assertEqual(event["domain"], 2)
            self.assertFalse(event["blob"])
            self.assertEqual(set(event["fields"]), set("abcd"))

    def test_all_original_getters_and_mutations_still_occur_once(self):
        hooks.apply(self.stage)
        # Comparing call spellings across whole files catches hidden enrichment
        # lookups as well as accidentally duplicated original evaluations.
        access = re.compile(
            r"(?:self\.n|\.flags|\.kind|\.set_node_flags|\.set_node_flow|"
            r"\.node_slice|\.list|\.parsed_view|\.node)\s*\(")
        for name, original in self.original.items():
            with self.subTest(file=name):
                self.assertEqual(access.findall(original.decode()),
                                 access.findall((self.stage / name).read_text()))

    def test_last_file_drift_rejects_before_any_write(self):
        name = list(hooks.SOURCE_SHA256)[-1]
        (self.stage / name).write_bytes(self.original[name] + b"// drift\n")
        before = {name: (self.stage / name).read_bytes() for name in hooks.SOURCE_SHA256}
        with self.assertRaisesRegex(ValueError, "source changed"):
            hooks.apply(self.stage)
        self.assertEqual(before, {name: (self.stage / name).read_bytes()
                                  for name in hooks.SOURCE_SHA256})

    def test_duplicate_exact_match_rejects_even_if_hash_was_updated(self):
        name = list(hooks.SOURCE_SHA256)[-1]
        altered = self.original[name] + hooks.REPLACEMENTS[name][0][0].encode()
        (self.stage / name).write_bytes(altered)
        hashes = dict(hooks.SOURCE_SHA256, **{name: hashlib.sha256(altered).hexdigest()})
        with patch.object(hooks, "SOURCE_SHA256", hashes):
            with self.assertRaisesRegex(ValueError, "one exact source match"):
                hooks.apply(self.stage)
        for previous in list(hooks.SOURCE_SHA256)[:-1]:
            self.assertEqual((self.stage / previous).read_bytes(), self.original[previous])

    def test_reapplication_is_rejected(self):
        hooks.apply(self.stage)
        before = {name: (self.stage / name).read_bytes() for name in hooks.SOURCE_SHA256}
        with self.assertRaisesRegex(ValueError, "source changed"):
            hooks.apply(self.stage)
        self.assertEqual(before, {name: (self.stage / name).read_bytes()
                                  for name in hooks.SOURCE_SHA256})

    def test_production_frozen_and_symlink_inputs_are_rejected(self):
        for path in [ROOT, FROZEN]:
            with self.subTest(path=path):
                with self.assertRaisesRegex(ValueError, "isolated staging|frozen source"):
                    hooks.apply(path)
        name = list(hooks.SOURCE_SHA256)[-1]
        (self.stage / name).unlink()
        (self.stage / name).symlink_to(FROZEN / name)
        with self.assertRaisesRegex(ValueError, "symlinked"):
            hooks.apply(self.stage)
        self.assertEqual((FROZEN / name).read_bytes(), self.original[name])
        self.assertEqual((self.stage / list(hooks.SOURCE_SHA256)[0]).read_bytes(),
                         self.original[list(hooks.SOURCE_SHA256)[0]])

    def test_exact_helper_bodies_preserve_values_evaluations_and_order(self):
        hooks.apply(self.stage)
        names = {
            "state.rs": ["n", "set_flags"],
            "containers.rs": ["syntax_slice", "syntax_node", "syntax_nodes", "set_flow_node"],
            "dispatch.rs": ["bind_node_error"],
        }
        fixture = (HERE / "hooks_fixture.rs").read_text()
        for variant, directory in [("original", FROZEN), ("traced", self.stage)]:
            methods = []
            for file, selected in names.items():
                source = (directory / "crates/tsr_binder/src" / file).read_text()
                methods += [function(source, name) for name in selected]
            fixture = fixture.replace(f"// {variant.upper()}_METHODS", "\n".join(methods))
        rustc = shutil.which("rustc")
        self.assertIsNotNone(rustc, "rustc is required for the executable hook contracts")
        source = Path(self.temp.name) / "fixture.rs"
        source.write_text(fixture)
        for mode, flags in [("debug", []), ("release", ["-O"])]:
            with self.subTest(mode=mode):
                binary = Path(self.temp.name) / f"fixture-{mode}"
                result = subprocess.run([rustc, "--edition=2021", "--test", "-Dwarnings",
                                         *flags, str(source), "-o", str(binary)],
                                        capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
