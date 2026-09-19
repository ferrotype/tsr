"""Build provenance regressions, including an actual conflicting Cargo build."""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s07_benchmark as benchmark


class CargoArtifacts(unittest.TestCase):
    def artifact(self):
        manifest = Path("/benchmark/crates/tsr_bench/Cargo.toml")
        executable = "/configured-output/native/release/ts-bench"
        return manifest, {
            "reason": "compiler-artifact", "manifest_path": str(manifest),
            "target": {"name": "ts-bench", "kind": ["bin"], "crate_types": ["bin"]},
            "features": [], "profile": {"opt_level": "3", "debuginfo": 0,
                "debug_assertions": False, "overflow_checks": False, "test": False},
            "filenames": [executable], "executable": executable,
        }

    def decode(self, manifest, rows, allocation=False):
        return benchmark.rust_executable(
            b"\n".join(json.dumps(row).encode() for row in rows), manifest, allocation)

    def test_exact_artifact_selection_and_feature_mode(self):
        manifest, artifact = self.artifact()
        done = {"reason": "build-finished", "success": True}
        unrelated = copy.deepcopy(artifact)
        unrelated["manifest_path"] = "/another/Cargo.toml"
        self.assertEqual(self.decode(manifest, [unrelated, artifact, done]), Path(artifact["executable"]))
        artifact["features"] = ["allocation"]
        self.assertEqual(self.decode(manifest, [artifact, done], True), Path(artifact["executable"]))

    def test_rejects_missing_duplicate_failed_wrong_profile_or_feature_artifact(self):
        manifest, artifact = self.artifact()
        done = {"reason": "build-finished", "success": True}
        invalid = [[], [artifact], [done], [artifact, artifact, done],
                   [artifact, done, done], [done, artifact],
                   [artifact, {**done, "success": False}], [[], artifact, done]]
        for field, value in (("features", ["allocation"]), ("features", ["profile"]),
                             ("executable", "relative/ts-bench"), ("filenames", []),
                             ("filenames", "/configured-output/native/release/ts-bench"),
                             ("target", None), ("profile", None), ("manifest_path", None),
                             ("manifest_path", "/different/Cargo.toml")):
            invalid.append([{**artifact, field: value}, done])
        for field, value in (("opt_level", "0"), ("debuginfo", False),
                             ("debug_assertions", 0), ("overflow_checks", True), ("test", True)):
            invalid.append([{**artifact, "profile": {**artifact["profile"], field: value}}, done])
        for rows in invalid:
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                self.decode(manifest, rows)


class CargoConfiguration(unittest.TestCase):
    def test_compiler_configuration_fails_closed_and_legacy_shadow_is_respected(self):
        with tempfile.TemporaryDirectory(prefix="s07-cargo-config-") as temporary:
            root = Path(temporary)
            (root / ".cargo").mkdir()
            (root / "Cargo.toml").write_text('[workspace]\n')
            env = {"CARGO_HOME": str(root / "cargo-home")}
            for contents in ('[build]\nrustflags=["-Cpanic=abort"]\n',
                             '[target.\'cfg(unix)\']\nrustflags=["-Copt-level=0"]\n',
                             '[env]\nCARGO_PROFILE_RELEASE_PANIC="abort"\n'):
                (root / ".cargo/config.toml").write_text(contents)
                with self.subTest(contents=contents), self.assertRaises(ValueError):
                    benchmark.release_configuration(env, root)
            # Cargo chooses the extensionless file when both exist. Keep the
            # ignored file fingerprinted, but do not interpret it as effective.
            (root / ".cargo/config").write_text('[net]\noffline=true\n')
            benchmark.release_configuration(env, root)
            self.assertIn(root / ".cargo/config.toml", benchmark.cargo_configuration_paths(env, root))

    def test_actual_cargo_output_and_profile_override_stale_default_binary(self):
        toolchain = (benchmark.ROOT / "rust-toolchain.toml").read_bytes()
        with tempfile.TemporaryDirectory(prefix="s07-cargo-native-") as temporary:
            root = Path(temporary)
            crate = root / "crates/tsr_bench"
            (crate / "src").mkdir(parents=True)
            (root / ".cargo").mkdir()
            (root / "cargo-home").mkdir()
            (root / "rust-toolchain.toml").write_bytes(toolchain)
            (root / "Cargo.toml").write_text('''[workspace]
members=["crates/tsr_bench"]
resolver="2"
[profile.release]
panic="unwind"
lto="fat"
codegen-units=1
''')
            (crate / "Cargo.toml").write_text('''[package]
name="tsr_bench"
version="0.1.0"
edition="2021"
[features]
allocation=[]
[[bin]]
name="ts-bench"
path="src/main.rs"
''')
            (crate / "src/main.rs").write_text('''fn main() {
    println!("current {} {} {}", cfg!(feature="allocation"), cfg!(debug_assertions), cfg!(panic="unwind"));
}
''')
            (crate / "examples").mkdir()
            (crate / "examples/allocation_probe.rs").write_text('fn main() { println!("probe {} {}", cfg!(debug_assertions), cfg!(panic="unwind")); }')
            config = root / ".cargo/config.toml"
            config.write_text('''[build]
target-dir="configured-output"
build-dir="configured-intermediate"
target="intentionally-not-a-real-target"
[profile.release]
panic="abort"
lto=false
codegen-units=16
[profile.release.package.tsr_bench]
opt-level=0
debug=true
debug-assertions=true
overflow-checks=true
codegen-units=32
[profile.dev]
opt-level=3
panic="abort"
debug=false
debug-assertions=false
overflow-checks=false
[target.'cfg(unix)']
runner="this-runner-must-never-execute"
''')
            home_config = root / "cargo-home/config.toml"
            home_config.write_text('''[net]
offline=true
[registries.preserved]
index="https://example.invalid/preserved-registry"
[profile.release.package."*"]
opt-level=1
codegen-units=64
''')
            originals = {path: path.read_bytes() for path in (config, home_config)}
            stale = root / "target/release/ts-bench"
            stale.parent.mkdir(parents=True)
            stale.write_text("stale executable from before target-dir changed")
            invocations = []

            def execute(args, *, cwd, env, **kwargs):
                # Verbose rustc commands independently establish the effective
                # options Cargo's JSON profile does not report (LTO/CG units).
                actual = args + (["-vv"] if args[0] == "cargo" else [])
                result = subprocess.run(actual, cwd=cwd, env=env, capture_output=True, check=True)
                invocations.append((args, result.stderr.decode()))
                return result.stdout

            with patch.object(benchmark, "ROOT", root), patch.object(benchmark, "CACHE", root / "cache"), \
                 patch.dict(os.environ, {"CARGO_HOME": str(root / "cargo-home")}), \
                 patch.object(benchmark, "command", execute):
                os.environ.pop("CARGO_BUILD_BUILD_DIR", None)
                os.environ.pop("CARGO_TARGET_DIR", None)
                env = benchmark.native_environment()
                stable, host = benchmark.rust_native_toolchain(env)
                subprocess.run(["cargo", "+"+stable, "generate-lockfile", "--offline"],
                               cwd=root, env=env, capture_output=True, check=True)
                # A real diagnostic crate with the same package/bin identities
                # occupies the configured cache. Native builds must not reuse
                # or mutate it, regardless of Cargo's source-root freshness.
                diagnostic = root / "diagnostic-source"
                diagnostic_crate = diagnostic / "crates/tsr_bench"
                (diagnostic_crate / "src").mkdir(parents=True)
                for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
                    (diagnostic / name).write_bytes((root / name).read_bytes())
                (diagnostic_crate / "Cargo.toml").write_bytes((crate / "Cargo.toml").read_bytes())
                (diagnostic_crate / "src/main.rs").write_text('fn main() { println!("diagnostic copy"); }')
                configured = root / "configured-output"
                messages = execute(["cargo", "+"+stable, "build", "--release", "--locked",
                                    "--offline", "--package", "tsr_bench", "--bin", "ts-bench",
                                    "--target", host, "--target-dir", str(configured),
                                    "--message-format=json-render-diagnostics",
                                    *benchmark.release_configuration(env)], cwd=diagnostic, env=env)
                diagnostic_binary = benchmark.rust_executable(messages, diagnostic_crate / "Cargo.toml", False)
                self.assertEqual(subprocess.check_output([str(diagnostic_binary)]).strip(), b"diagnostic copy")
                environment_target = root / "environment-output"
                environment_build = root / "environment-intermediate"
                for directory in (environment_target, environment_build):
                    directory.mkdir()
                    (directory / "preserved").write_bytes(b"caller cache")
                cache_directories = (configured, root / "configured-intermediate", environment_target, environment_build)
                self.assertTrue((root / "configured-intermediate").is_dir())
                cached_files = {path.relative_to(root): hashlib.sha256(path.read_bytes()).hexdigest()
                                for directory in cache_directories for path in directory.rglob("*") if path.is_file()}
                build_directories = []
                for allocation in (False, True):
                    if allocation:
                        os.environ["CARGO_TARGET_DIR"] = str(environment_target)
                        os.environ["CARGO_BUILD_BUILD_DIR"] = str(environment_build)
                    binary, runtime_env = benchmark.build_rust(allocation)
                    output = subprocess.check_output([str(binary)], env=runtime_env).decode().strip()
                    self.assertEqual(output, f"current {str(allocation).lower()} false true")
                    args, compiler = invocations[-1]
                    self.assertEqual(args[args.index("--target")+1], host)
                    self.assertIn("-C lto=fat", compiler)
                    self.assertIn("-C codegen-units=1", compiler)
                    self.assertNotIn("-C panic=abort", compiler)
                    build_directory = Path(args[args.index("--target-dir")+1])
                    build_directories.append(build_directory)
                    self.assertEqual(build_directory.parent, (root / "target").resolve())
                    self.assertIn("build.build-dir=" + json.dumps(str(build_directory)), args)
                    self.assertFalse(build_directory.exists(), "temporary target survives copied artifact")
                    self.assertEqual(cached_files, {
                        path.relative_to(root): hashlib.sha256(path.read_bytes()).hexdigest()
                        for directory in cache_directories for path in directory.rglob("*") if path.is_file()})
                self.assertNotEqual(*build_directories)
                for mode in ("debug", "release"):
                    binary, runtime_env = benchmark.build_allocation_probe(mode, stable)
                    self.assertEqual(subprocess.check_output([str(binary)], env=runtime_env).decode().strip(),
                                     f"probe {str(mode == 'debug').lower()} true")
                    args, _ = invocations[-1]
                    self.assertEqual(args[2], "build")
                    self.assertEqual(args[args.index("--target")+1], host)
                self.assertEqual(runtime_env["CARGO_HOME"], str(root / "cargo-home"))
            for path, original in originals.items():
                self.assertEqual(path.read_bytes(), original)
            self.assertEqual(stale.read_text(), "stale executable from before target-dir changed")

    def test_escaped_artifact_is_rejected_and_temporary_target_is_removed(self):
        with tempfile.TemporaryDirectory(prefix="s07-cargo-escape-") as temporary:
            root = Path(temporary)
            external = root / "configured-output/ts-bench"
            external.parent.mkdir()
            external.write_bytes(b"unrelated configured executable")
            build_directories = []

            def escaped_artifact(args, **kwargs):
                build_directories.append(Path(args[args.index("--target-dir")+1]))
                self.assertTrue(build_directories[-1].is_dir())
                _, artifact = CargoArtifacts().artifact()
                artifact.update(manifest_path=str(root / "crates/tsr_bench/Cargo.toml"),
                                executable=str(external), filenames=[str(external)])
                return b"\n".join(json.dumps(row).encode() for row in
                                  (artifact, {"reason": "build-finished", "success": True}))

            with patch.object(benchmark, "ROOT", root), patch.object(benchmark, "CACHE", root / "cache"), \
                 patch.object(benchmark, "native_environment", return_value={}), \
                 patch.object(benchmark, "rust_native_toolchain", return_value=("1.97.1", "native")), \
                 patch.object(benchmark, "release_configuration", return_value=[]), \
                 patch.object(benchmark, "command", escaped_artifact):
                with self.assertRaisesRegex(ValueError, "escaped its isolated build directory"):
                    benchmark.build_rust()
            self.assertEqual(len(build_directories), 1)
            self.assertFalse(build_directories[0].exists())
            self.assertEqual(external.read_bytes(), b"unrelated configured executable")
            self.assertFalse((root / "cache/s07-benchmark/rust-benchmark").exists())

    def test_persisted_go_target_and_experiments_cannot_change_native_baseline(self):
        with tempfile.TemporaryDirectory(prefix="s07-go-native-") as temporary:
            settings = Path(temporary) / "go.env"
            settings.write_text("GOOS=linux\nGOARCH=386\nGOEXPERIMENT=nogreenteagc\nGOPROXY=https://example.invalid/preserved\nGONOSUMDB=private.example.invalid\n")
            env = benchmark.go_native_environment({**os.environ, "GOENV": str(settings),
                                                   "GOAMD64": "v3", "MIMALLOC_PURGE_DELAY": "-1",
                                                   "mimalloc_eager_commit": "0"})
            actual = json.loads(subprocess.check_output(["go", "env", "-json", "GOOS", "GOARCH", "GOHOSTOS", "GOHOSTARCH", "GOEXPERIMENT", "GOPROXY", "GONOSUMDB"], env=env))
            self.assertEqual(actual["GOOS"], actual["GOHOSTOS"])
            self.assertEqual(actual["GOARCH"], actual["GOHOSTARCH"])
            self.assertEqual(actual["GOEXPERIMENT"], "")
            self.assertEqual(actual["GOPROXY"], "https://example.invalid/preserved")
            self.assertEqual(actual["GONOSUMDB"], "private.example.invalid")
            self.assertEqual(env["GOENV"], "off")
            self.assertNotIn("GOAMD64", env)
            self.assertFalse(any(key.lower().startswith("mimalloc_") for key in env))


if __name__ == "__main__":
    unittest.main()
