"""Exercise the production guard against Cargo's actual feature unification."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_production_features as guard


class ProductionFeaturesTests(unittest.TestCase):
    def setUp(self):
        # Inherit this repository's pinned toolchain, including on CI.
        target = guard.ROOT / "target"
        target.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix="production-features-", dir=target)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.write("Cargo.toml", '[workspace]\nresolver = "2"\nmembers = ["crates/*", "tools/leaf"]\n')
        self.write("crates/core/Cargo.toml", '''[package]
name = "tsr_core"
version = "0.0.0"
edition = "2021"
[features]
default = []
go-slice-compat = []
''')
        self.write("crates/core/src/lib.rs", '''#[cfg(feature = "go-slice-compat")]
pub fn compatibility() {}
''')
        self.consumer_manifest = '''[package]
name = "consumer"
version = "0.0.0"
edition = "2021"
[dependencies]
tsr_core = { path = "../core" }
'''
        self.write("crates/consumer/Cargo.toml", self.consumer_manifest)
        self.write("crates/consumer/src/lib.rs", "pub fn production() {}\n")
        self.write("tools/leaf/Cargo.toml", '''[package]
name = "leaf"
version = "0.0.0"
edition = "2021"
[dependencies]
tsr_core = { path = "../../crates/core", features = ["go-slice-compat"] }
''')
        self.write("tools/leaf/src/lib.rs", "pub fn probe() { tsr_core::compatibility(); }\n")
        subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=self.root, check=True)

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def workspace_check(self):
        result = subprocess.run(
            ["cargo", "check", "--workspace", "--locked", "--message-format=json"],
            cwd=self.root, stdout=subprocess.PIPE, text=True, check=True,
        )
        core_artifacts = [event for line in result.stdout.splitlines()
                          if (event := json.loads(line)).get("reason") == "compiler-artifact"
                          and event["target"]["name"] == "tsr_core"]
        self.assertTrue(core_artifacts)
        self.assertTrue(all(guard.FORBIDDEN_FEATURE in event["features"] for event in core_artifacts))

    def test_harness_opt_in_is_excluded_but_production_opt_in_is_rejected(self):
        self.workspace_check()
        self.assertEqual(guard.check(self.root), 2)
        self.write("crates/consumer/Cargo.toml", self.consumer_manifest.replace(
            'path = "../core"', 'path = "../core", features = ["go-slice-compat"]'))
        # Compilation succeeds; the artifact feature check must still reject it.
        with self.assertRaisesRegex(ValueError, "production build enabled tsr_core/go-slice-compat"):
            guard.check(self.root)

    def test_workspace_unification_cannot_hide_accidental_api_use(self):
        self.write("crates/consumer/src/lib.rs", "pub fn production() { tsr_core::compatibility(); }\n")
        self.workspace_check()
        with self.assertRaises(subprocess.CalledProcessError):
            guard.check(self.root)
