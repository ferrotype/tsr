"""Integration regressions for pinned generation; all mutations use temporary repos.

Run: python3 -m unittest discover -s scripts/tests -v
"""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest


REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("ledger_init", REPO / "scripts/ledger-init.py")
ledger_init = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ledger_init)


class InventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.build = tempfile.TemporaryDirectory(prefix="ts-rust-inventory-build-")
        cls.binary = Path(cls.build.name) / "go-inventory"
        subprocess.run(["go", "build", "-o", str(cls.binary), str(REPO / "scripts/go-inventory/main.go")], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.build.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ts-rust-inventory-test-")
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.upstream = root / "upstream"
        self.output = root / "output"
        self.upstream.mkdir()
        self.output.mkdir()
        self.git("init", "--quiet")
        self.git("config", "user.name", "Inventory Test")
        self.git("config", "user.email", "inventory-test@example.invalid")
        self.write_source("scanner.go", "package scanner\nfunc Scan() {}\n")
        self.pin = self.commit("initial fixture")

    def git(self, *args):
        env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        return subprocess.check_output(["git", "-C", str(self.upstream), *args], env=env, stderr=subprocess.PIPE).decode().strip()

    def commit(self, message):
        self.git("add", "--all")
        self.git("-c", "commit.gpgsign=false", "commit", "--quiet", "-m", message)
        return self.git("rev-parse", "HEAD")

    def write_source(self, name, source):
        path = self.upstream / "tsc/internal/scanner" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)

    def generate(self, pin=None):
        return ledger_init.generate(self.output, self.upstream, pin, [str(self.binary)])

    def artifacts(self):
        return {name: (self.output / name).read_bytes() for name in
                ["PORTS.toml", "data/go-functions.tsv", "data/upstream.json"]}

    def ledger(self):
        return tomllib.loads((self.output / "PORTS.toml").read_text())

    def test_receiver_ids_init_ordinals_and_full_provenance(self):
        self.write_source("methods.go", """package scanner
type A struct{}
type B[T any] struct{}
type C[T, U any] struct{}
func (*A) Map() {}
func (B[T]) Map() {}
func (*C[T, U]) Map() {}
func init() {}
func init() {}
""")
        pin = self.commit("receiver fixture")
        self.generate(pin[:10])
        artifacts = self.artifacts()
        lines = artifacts["data/go-functions.tsv"].decode().splitlines()
        self.assertEqual(lines[0], "# upstream " + pin)
        rows = [line.split("\t") for line in lines[2:]]
        keys = [row[6] for row in rows]
        prefix = "tsc/internal/scanner/methods.go:"
        self.assertEqual(set(keys), {prefix + x for x in ["A.Map", "B.Map", "C.Map", "init#1", "init#2"]}
                         | {"tsc/internal/scanner/scanner.go:Scan"})
        self.assertEqual(len(keys), len(set(keys)))
        self.assertEqual({row[2] for row in rows if row[3] == "Map"}, {"A", "B", "C"})
        manifest = json.loads(artifacts["data/upstream.json"])
        self.assertEqual(manifest, {
            "schema_version": 2, "pin": pin,
            "ledger_generated_sha256": ledger_init.ledger_generated_sha256(self.ledger()),
            "inventory_sha256": hashlib.sha256(artifacts["data/go-functions.tsv"]).hexdigest(),
        })
        for entry in self.ledger()["file"]:
            self.assertEqual(entry["pin"], pin)
            self.assertEqual(entry["rust"], [])
            self.assertEqual(entry["verify"], [])
            self.assertEqual(entry["source_hash"], hashlib.sha256((self.upstream / entry["go"]).read_bytes()).hexdigest())

    def test_pin_mismatch_leaves_outputs_unchanged(self):
        self.generate()
        before = self.artifacts()
        self.write_source("scanner.go", "package scanner\nfunc Scan() { println(1) }\n")
        self.commit("change")
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.generate(self.pin)
        self.assertEqual(self.artifacts(), before)

    def test_dirty_tracked_input_is_rejected_by_both_generators(self):
        self.generate()
        before = self.artifacts()
        self.write_source("scanner.go", "package scanner\nfunc Changed() {}\n")
        with self.assertRaisesRegex(ValueError, "dirty"):
            self.generate()
        result = subprocess.run([str(self.binary), str(self.upstream / "tsc")], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"dirty", result.stderr)
        self.assertEqual(result.stdout, b"")
        self.assertEqual(self.artifacts(), before)

    def test_untracked_input_is_rejected(self):
        self.write_source("untracked.go", "package scanner\nfunc Untracked() {}\n")
        with self.assertRaisesRegex(ValueError, "dirty"):
            self.generate()
        self.assertFalse((self.output / "PORTS.toml").exists())

    def test_ignored_untracked_source_does_not_enter_denominator(self):
        (self.upstream / ".gitignore").write_text("ignored.go\n")
        self.commit("ignore fixture")
        self.write_source("ignored.go", "this is invalid Go syntax\n")
        self.generate()
        self.assertNotIn(b"ignored.go", self.artifacts()["data/go-functions.tsv"])
        self.assertEqual(len(self.ledger()["file"]), 1)

    def test_pin_bump_preserves_stale_entries_across_regeneration(self):
        self.write_source("unchanged.go", "package scanner\nfunc Same() {}\n")
        first = self.commit("two files")
        self.generate()
        path = self.output / "PORTS.toml"
        text = path.read_text().replace('status = "planned"', 'status = "ported"')
        text = text.replace('rust = []', 'rust = ["crates/tsr_scanner/src/a.rs", "crates/tsr_scanner/src/b.rs"]')
        path.write_text(text.replace('verify = []', 'verify = ["evidence.scanner.passed == 1"]'))
        self.write_source("scanner.go", "package scanner\nfunc Scan() { println(2) }\n")
        second = self.commit("change one source")
        self.generate()
        self.generate()  # Must not mistake the newly stored hash for synchronization.
        entries = {e["go"].rsplit("/", 1)[1]: e for e in self.ledger()["file"]}
        self.assertEqual(entries["scanner.go"]["pin"], first)
        self.assertEqual(entries["unchanged.go"]["pin"], second)
        self.assertEqual(entries["scanner.go"]["source_hash"], hashlib.sha256((self.upstream / "tsc/internal/scanner/scanner.go").read_bytes()).hexdigest())
        for entry in entries.values():
            self.assertEqual(entry["status"], "ported")
            self.assertEqual(len(entry["rust"]), 2)
            self.assertEqual(entry["verify"], ["evidence.scanner.passed == 1"])
        self.write_source("new.go", "package scanner\nfunc New() {}\n")
        third = self.commit("unrelated addition")
        self.generate()
        entries = {e["go"].rsplit("/", 1)[1]: e for e in self.ledger()["file"]}
        self.assertEqual(entries["scanner.go"]["pin"], first)
        self.assertEqual(entries["unchanged.go"]["pin"], third)

    def test_legacy_scalar_and_verified_migration_uses_pinned_hash(self):
        (self.output / "PORTS.toml").write_text(f'''pin = "{self.pin[:10]}"
[[file]]
go = "tsc/internal/scanner/scanner.go"
status = "verified"
rust = "crates/tsr_scanner/src/lib.rs"
pin = "{self.pin[:10]}"
verify = ["evidence.scanner.passed == 1"]
''')
        self.write_source("scanner.go", "package scanner\nfunc Changed() {}\n")
        second = self.commit("changed since legacy verification")
        self.generate()
        ledger = self.ledger()
        entry = ledger["file"][0]
        self.assertEqual(ledger["pin"], second)
        self.assertEqual(entry["pin"], self.pin)
        self.assertEqual(entry["status"], "ported")
        self.assertEqual(entry["rust"], ["crates/tsr_scanner/src/lib.rs"])
        self.assertEqual(entry["verify"], ["evidence.scanner.passed == 1"])

    def test_parse_error_is_fatal_and_does_not_publish_partial_outputs(self):
        self.generate()
        before = self.artifacts()
        self.write_source("zbroken.go", "package scanner\nfunc Broken(\n")
        self.commit("invalid syntax")
        with self.assertRaisesRegex(ValueError, "inventory generation failed"):
            self.generate()
        self.assertEqual(self.artifacts(), before)

    def test_duplicate_canonical_id_is_fatal(self):
        self.write_source("duplicate.go", "package scanner\nfunc Duplicate() {}\nfunc Duplicate() {}\n")
        self.commit("duplicate declaration")
        with self.assertRaisesRegex(ValueError, "duplicate function inventory ID"):
            self.generate()
        self.assertFalse((self.output / "data/upstream.json").exists())


class LedgerProjectionTests(unittest.TestCase):
    def setUp(self):
        self.ledger = {
            "pin": "a" * 40,
            "file": [{
                "go": "tsc/internal/é.go", "package": "internal", "crate": "tsr_core",
                "phase": 0, "kind": "source", "pin": "a" * 40,
                "source_hash": "b" * 64, "loc": 2,
                "status": "planned", "rust": [], "verify": [],
            }],
        }

    def digest(self, ledger=None):
        return ledger_init.ledger_generated_sha256(self.ledger if ledger is None else ledger)

    def test_canonical_json_contract(self):
        # Fixed serialized vector also documents the bytes Rust must hash.
        canonical = ('{"file":[{"crate":"tsr_core","go":"tsc/internal/é.go",'
                     '"kind":"source","loc":2,"package":"internal","phase":0,'
                     '"pin":"' + "a" * 40 + '","source_hash":"' + "b" * 64
                     + '"}],"pin":"' + "a" * 40 + '"}')
        self.assertEqual(self.digest(), hashlib.sha256(canonical.encode("utf-8")).hexdigest())

    def test_editable_fields_do_not_change_projection_hash(self):
        before = self.digest()
        for field, value in {
            "status": "ported",
            "rust": ["crates/tsr_core/src/a.rs", "crates/tsr_core/src/b.rs"],
            "verify": ["evidence.core.passed == 1"],
        }.items():
            with self.subTest(field=field):
                edited = copy.deepcopy(self.ledger)
                edited["file"][0][field] = value
                self.assertEqual(self.digest(edited), before)
        # The projection is usable without Go, a checkout, or another generator run.
        for field in ("status", "rust", "verify"):
            del self.ledger["file"][0][field]
        self.assertEqual(self.digest(), before)

    def test_comments_formatting_and_table_field_order_are_irrelevant(self):
        lines = ["# editable formatting", 'pin = "' + "a" * 40 + '"', "", "[[file]]"]
        for key, value in reversed(list(self.ledger["file"][0].items())):
            lines.append(f"{key}={json.dumps(value, ensure_ascii=False)} # comment")
        self.assertEqual(self.digest(tomllib.loads("\n".join(lines))), self.digest())
        other = copy.deepcopy(self.ledger["file"][0])
        other["go"] = "tsc/internal/a.go"
        self.ledger["file"].append(other)
        before = self.digest()
        self.ledger["file"].reverse()
        self.assertEqual(self.digest(), before)

    def test_every_generated_field_changes_projection_hash(self):
        before = self.digest()
        for field, value in {
            "go": "tsc/internal/other.go", "package": "internal/other", "crate": "ts_other",
            "phase": 1, "kind": "generated", "pin": "c" * 40, "source_hash": "d" * 64, "loc": 3,
        }.items():
            with self.subTest(field=field):
                edited = copy.deepcopy(self.ledger)
                edited["file"][0][field] = value
                self.assertNotEqual(self.digest(edited), before)
        self.ledger["pin"] = "e" * 40
        self.assertNotEqual(self.digest(), before)

    def test_missing_generated_fields_are_rejected(self):
        for field in ledger_init.GENERATED_FILE_FIELDS:
            with self.subTest(field=field):
                edited = copy.deepcopy(self.ledger)
                del edited["file"][0][field]
                with self.assertRaisesRegex(ValueError, "missing generated field"):
                    self.digest(edited)
        for field in ("pin", "file"):
            with self.subTest(field=field):
                edited = copy.deepcopy(self.ledger)
                del edited[field]
                with self.assertRaises(ValueError):
                    self.digest(edited)

    def test_invalid_generated_field_values_are_rejected(self):
        for field, values in {
            "go": ["", 1], "package": ["", []], "crate": ["", {}],
            "phase": [-1, True, 0.5, "0", 2**63], "loc": [-1, False, 2.5, "2", 2**63],
            "kind": ["", "planned", 0], "pin": ["a" * 10, "A" * 40, 0],
            "source_hash": ["b" * 40, "B" * 64, None],
        }.items():
            for value in values:
                with self.subTest(field=field, value=value):
                    edited = copy.deepcopy(self.ledger)
                    edited["file"][0][field] = value
                    with self.assertRaises(ValueError):
                        self.digest(edited)
        for edited in [None, {"pin": "a" * 10, "file": []}, {"pin": "a" * 40, "file": {}},
                       {"pin": "a" * 40, "file": [None]}]:
            with self.subTest(ledger=edited):
                with self.assertRaises(ValueError):
                    ledger_init.ledger_generated_sha256(edited)

    def test_duplicate_go_names_are_rejected(self):
        self.ledger["file"].append(copy.deepcopy(self.ledger["file"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate ledger source path"):
            self.digest()


if __name__ == "__main__":
    unittest.main()
