"""Native acceptance replays raw observations, commands and build provenance."""
import io
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase4_native as native
import phase4_live as live


class NativeFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="phase4-native-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.build = self.root / "build"
        self.build.mkdir()
        self.capture = self.root / "capture"
        self.capture.mkdir()
        self.source_map = {"source.rs": native.digest(b"source")}
        self.addCleanup(patch.stopall)
        patch.object(native, "sources", return_value=self.source_map).start()
        patch.object(native.corpus, "read_document", return_value={"library_text": "declare interface Array<T> {}\n"}).start()
        self.export_files = {"tsc/go.mod": b"module example\n", "tsc/testdata/fixtures/compiler/tsconfig.json": b"{}"}
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w") as stream:
            for name, raw in self.export_files.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(raw)
                stream.addfile(entry, io.BytesIO(raw))
        self.archive = archive.getvalue()
        patch.object(native, "pinned_export", return_value=self.archive).start()
        self.recorded_repo = Path("/recorded/source")
        self.recorded_build = Path("/recorded/build")
        self.recorded_capture = Path("/recorded/capture")
        self.host = {"os": "linux", "arch": "x86_64"}
        self.make_build()

    def save(self, path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        native.write(path, value)

    def process(self, path, command, cwd, stdout=b"", status=0, stderr=b""):
        path.mkdir(parents=True, exist_ok=True)
        self.save(path / "invocation.json", {"command": list(map(str, command)), "cwd": str(cwd), "timeout_seconds": 600})
        result = {"status": status, "timeout": False, "seconds": 0.1}
        self.save(path / "result.json", result)
        (path / "stdout").write_bytes(stdout)
        (path / "stderr").write_bytes(stderr)
        return result

    def make_build(self):
        for runtime in ("go", "rust"):
            image = self.build / (runtime + "-executable")
            image.write_bytes((runtime + " executable").encode())
            image.chmod(0o755)
        (self.build / "lib").mkdir()
        shutil.copy2(self.build / "rust-executable", self.build / "lib/tsc")
        for name, raw in self.export_files.items():
            path = self.build / "export" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        self.process(self.build / "go-build", ["go", "build", "-mod=readonly", "-o", self.recorded_build / "go-executable", "./cmd/tsc"], self.recorded_build / "export/tsc")
        cargo = [{"reason": "compiler-artifact", "target": {"name": "tsrust", "kind": ["bin"],
                  "src_path": str(self.recorded_repo / "crates/tsr/src/main.rs")},
                  "profile": {"opt_level": "3", "debug_assertions": False, "test": False},
                  "executable": str(self.recorded_repo / "target/release/tsrust")},
                 {"reason": "build-finished", "success": True}]
        self.process(self.build / "rust-build", native.rust_command(self.recorded_repo), self.recorded_repo,
                     b"\n".join(native.canonical(event) for event in cargo) + b"\n")
        rust_pin = tomllib.loads((native.ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        go_pin = tomllib.loads((native.ROOT / "data/s04/toolchains.toml").read_text())["go"]
        versions = {"go": f"go version {go_pin} linux/amd64\n", "cargo": f"cargo {rust_pin} (test)\n",
                    "rustc": f"rustc {rust_pin} (test)\nhost: x86_64-unknown-linux-gnu\nrelease: {rust_pin}\n"}
        for name, command in (("go", ["go", "version"]), ("rustc", ["rustc", "-Vv"]), ("cargo", ["cargo", "--version"])):
            self.process(self.build / (name + "-version"), command, self.recorded_repo, versions[name].encode())
        metadata = (f"{self.recorded_build / 'go-executable'}: {go_pin}\n"
                    "\tpath\tgithub.com/microsoft/TypeScript/tsc/cmd/tsc\n\tbuild\tGOOS=linux\n"
                    "\tbuild\tGOARCH=amd64\n\tbuild\tCGO_ENABLED=1\n")
        self.process(self.build / "go-image", ["go", "version", "-m", self.recorded_build / "go-executable"],
                     self.recorded_repo, metadata.encode())
        self.build_report = {"version": 2, "pin": native.read_json(native.ROOT / "data/upstream.json")["pin"],
            "sources": self.source_map, "host": self.host, "repo_root": str(self.recorded_repo),
            "directory": str(self.recorded_build), "go_export_sha256": native.digest(self.archive),
            "go_environment": {"CGO_ENABLED": "1", "GOTOOLCHAIN": "local", "GOWORK": "off", "GOFLAGS": ""},
            "rust_environment": {}, "tools": versions, "rust_binary": str(self.recorded_repo / "target/release/tsrust"),
            "images": {runtime: native.digest((self.build / (runtime + "-executable")).read_bytes()) for runtime in ("go", "rust")}}
        self.bind(self.build, self.build_report)

    def bind(self, directory, report):
        report["artifacts"] = native.artifacts(directory)
        self.save(directory / "report.json", report)

    def report(self, witnesses):
        return {"version": 2, "host": self.host, "sources": self.source_map, "source_stable": True,
                "repo_root": str(self.recorded_repo), "build_root": str(self.recorded_build),
                "directory": str(self.recorded_capture), "images": self.build_report["images"],
                "build_sha256": native.digest((self.build / "report.json").read_bytes()),
                "witnesses": witnesses, "pass": False}

    def smoke(self, *, rust_status=0):
        group = {}
        for mode, flags in (("single", ["--singleThreaded"]), ("parallel", [])):
            observations = {}
            for runtime in ("go", "rust"):
                raw = b""
                command = [self.recorded_build / (runtime + "-executable"), "-p",
                           self.recorded_build / "export/tsc/testdata/fixtures/compiler", "--noEmit", *flags]
                execution = self.process(self.capture / "smoke" / mode / runtime, command, self.recorded_capture,
                                         raw, status=rust_status if runtime == "rust" else 0)
                observations[runtime] = {**execution, "stdout": raw.hex(), "stderr": ""}
            group[mode] = {"pass": True, "observations": observations}
        report = self.report({"smoke": group})
        self.bind(self.capture, report)
        return report

    def interop(self):
        group = {}
        for family in native.FAMILIES:
            observations = {}
            for mode, order in native.ORDERS.items():
                base = self.capture / "interop" / family / mode
                root = base / "project"
                original = self.recorded_capture / "interop" / family / mode / "project"
                native.interop_project(root, family)
                observations[mode] = []
                for index, runtime in enumerate(order):
                    native.interop_edit(root, index, family)
                    live.put(root, "out/state.tsbuildinfo", f"build state {index}\n")
                    args = ["-b"] if family == "projectReferences" else ["-p", "."]
                    self.process(base / f"step-{index}", [self.recorded_build / (runtime + "-executable"), *args,
                                 "--pretty", "false"], original)
                    shutil.copytree(root, base / f"step-{index}/project")
                    observations[mode].append({"status": 0, "timeout": False, "stdout": "", "stderr": "", "files": live.outputs(root)})
            group[family] = {"pass": False, "observations": observations}
        report = self.report({"interop": group})
        self.bind(self.capture, report)
        return report


class NativeBuildReplay(NativeFixture):
    def test_relocated_build_uses_recorded_roots_without_following_them(self):
        self.assertEqual(native.verify_build(self.build, expected_host="linux")["host"], self.host)
        moved = self.root / "moved"
        shutil.move(self.build, moved)
        self.assertEqual(native.verify_build(moved)["images"], self.build_report["images"])

    def test_toolchain_host_cannot_be_relabelled(self):
        self.build_report["host"] = {"os": "macos", "arch": "x86_64"}
        self.bind(self.build, self.build_report)
        with self.assertRaisesRegex(ValueError, "toolchain pin or build host"):
            native.verify_build(self.build, expected_host="macos")

    def test_modified_export_is_rejected_even_when_rehashed(self):
        (self.build / "export/tsc/go.mod").write_text("changed")
        self.bind(self.build, self.build_report)
        with self.assertRaisesRegex(ValueError, "authenticated pin"):
            native.verify_build(self.build)

    def test_arbitrary_cargo_executable_is_rejected_even_when_rehashed(self):
        path = self.build / "rust-build/stdout"
        events = [native.strict_json_loads(line) for line in path.read_bytes().splitlines()]
        events[0]["executable"] = "/elsewhere/tsrust"
        path.write_bytes(b"\n".join(native.canonical(event) for event in events) + b"\n")
        self.bind(self.build, self.build_report)
        with self.assertRaisesRegex(ValueError, "executable provenance"):
            native.verify_build(self.build)

    def test_build_command_staging_and_duplicate_json_are_checked(self):
        path = self.build / "go-build/invocation.json"
        value = native.read_json(path)
        value["command"].append("--unexpected")
        self.save(path, value)
        self.bind(self.build, self.build_report)
        with self.assertRaisesRegex(ValueError, "invocation"):
            native.verify_build(self.build)
        (self.build / "report.json").write_bytes(b'{"version":2,"version":2}')
        with self.assertRaisesRegex(ValueError, "duplicate JSON"):
            native.verify_build(self.build)

    def test_nested_malformed_events_and_escaping_roots_raise_value_error(self):
        path = self.build / "rust-build/stdout"
        original = path.read_bytes()
        path.write_bytes(b'{"reason":"compiler-artifact","target":null}\n')
        self.bind(self.build, self.build_report)
        with self.assertRaisesRegex(ValueError, "malformed"):
            native.verify_build(self.build)
        path.write_bytes(original)
        self.build_report["directory"] = "/recorded/../escape"
        self.bind(self.build, self.build_report)
        with self.assertRaisesRegex(ValueError, "artifact root"):
            native.verify_build(self.build)


class NativeWitnessReplay(NativeFixture):
    def test_absent_groups_withhold_metrics(self):
        self.bind(self.capture, self.report({}))
        self.assertEqual(native.verify_witnesses(self.build, self.capture)["metrics"], {})

    def test_smoke_ignores_pass_flags_and_survives_relocation(self):
        self.smoke(rust_status=2)
        self.assertEqual(native.verify_witnesses(self.build, self.capture)["metrics"], {"smoke": False})
        moved = self.root / "moved-capture"
        shutil.move(self.capture, moved)
        self.assertEqual(native.verify_witnesses(self.build, moved)["metrics"], {"smoke": False})

    def test_partial_modes_and_summary_only_forgery_are_rejected(self):
        report = self.smoke()
        report["witnesses"]["smoke"].pop("parallel")
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, "mode inventory"):
            native.verify_witnesses(self.build, self.capture)
        report = self.smoke()
        report["witnesses"]["smoke"]["single"]["observations"]["go"]["status"] = 5
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, "raw outcomes"):
            native.verify_witnesses(self.build, self.capture)

    def test_full_interop_replays_all_six_families_and_handoffs(self):
        self.interop()
        self.assertEqual(native.verify_witnesses(self.build, self.capture)["metrics"], {"buildinfo_interop": True})

    def test_missing_interop_family_is_not_vacuously_true(self):
        report = self.interop()
        report["witnesses"]["interop"].pop("noCheck")
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, "family inventory"):
            native.verify_witnesses(self.build, self.capture)

    def test_wrong_interop_schedule_and_snapshot_are_rejected(self):
        report = self.interop()
        path = self.capture / "interop/incremental/go-rust/step-2/invocation.json"
        invocation = native.read_json(path)
        invocation["command"][0] = str(self.recorded_build / "go-executable")
        self.save(path, invocation)
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, "invocation"):
            native.verify_witnesses(self.build, self.capture)

    def test_stale_source_symlinks_and_unexpected_extra_steps_are_rejected(self):
        report = self.smoke()
        report["sources"] = {"different": native.digest(b"different")}
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, "stale"):
            native.verify_witnesses(self.build, self.capture)
        report["sources"] = self.source_map
        self.save(self.capture / "smoke/extra.json", {})
        self.bind(self.capture, report)
        with self.assertRaisesRegex(ValueError, "unexpected artifact"):
            native.verify_witnesses(self.build, self.capture)
        (self.capture / "escape").symlink_to(self.root)
        with self.assertRaisesRegex(ValueError, "symlink"):
            native.verify_witnesses(self.build, self.capture)
