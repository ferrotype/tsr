"""Receipt replay is child-free and cannot bless missing sanitizer coverage."""
import copy
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase4_corpus as corpus
import phase4_sanitizer as sanitizer


class Receipt(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        document = corpus.read_document()
        cls.document = copy.deepcopy(document)
        cls.document["scenarios"] = document["scenarios"][:2]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name) / "capture"
        self.directory.mkdir()
        # Use a small inventory and source closure, but the real target roster,
        # Rust source test floor, compiler audit and all validators below.
        self.sources = {"source.rs": "a" * 64}
        for patch in (mock.patch.object(sanitizer, "sources", lambda: self.sources),
                      mock.patch.object(corpus, "read_document", lambda: self.document)):
            patch.start()
            self.addCleanup(patch.stop)
        self.report = {"version": 1, "sources": self.sources.copy(), "source_stable": True,
                       "nightly": sanitizer.nightly(), "source_root": "/original/checkout",
                       "capture_root": "/original/capture", "python": "/usr/bin/python3",
                       "target": "x86_64-unknown-linux-gnu", "host": {"os": "linux", "arch": "x86_64"},
                       "compiler": {"path": "/toolchains/pinned/bin/rustc", "sha256": "b" * 64},
                       "allocator": sanitizer.allocator_policy(), "images": {}, "error": None}
        self.env = sanitizer.instrumentation_environment({"CARGO_HOME": "/existing/cargo", "CARGO_NET_OFFLINE": "true"})
        self.process("rustc-path", ["rustup", "which", "--toolchain", sanitizer.nightly(), "rustc"],
                     self.report["compiler"]["path"].encode() + b"\n")
        self.process("rustc-version", ["rustup", "run", sanitizer.nightly(), "rustc", "-Vv"],
                     b"rustc 1.99.0-nightly\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.0-nightly\n")
        self.process("components", ["rustup", "component", "list", "--toolchain", sanitizer.nightly(), "--installed"], b"rust-src\n")
        self.env.update(RUSTC=self.report["compiler"]["path"], RUSTC_WRAPPER="/original/capture/rustc-wrapper",
                        CARGO_TARGET_DIR="/original/capture/build-target", PHASE4_TSAN_AUDIT="/original/capture/compiler-audit",
                        PHASE4_TSAN_RUSTC=self.report["compiler"]["path"], PHASE4_TSAN_RUSTC_SHA256="b" * 64)
        (self.directory / "rustc-wrapper").write_text(sanitizer.wrapper_text(self.report["python"], self.report["source_root"]))
        (self.directory / "images").mkdir()
        (self.directory / "compiler-audit").mkdir()
        self.audit("std", ["--crate-name", "std", "--target", self.report["target"], sanitizer.FLAGS])
        self.audit("core", ["--crate-name", "core", "--target", self.report["target"], sanitizer.FLAGS])
        for test in (True, False):
            targets = sanitizer.test_targets() if test else [{"package": "phase4_tsctests", "kind": "bin",
                        "name": "phase4_tsctests", "source": "tools/phase4/tsctests/src/main.rs"}]
            events = []
            for target in targets:
                key = sanitizer.target_id(target)
                identity = ("test:" if test else "bin:") + key
                image = "images/" + identity.replace(":", "-")
                payload = identity.encode()
                (self.directory / image).write_bytes(payload)
                path = "/original/capture/build-target/" + self.report["target"] + "/debug/deps/" + identity.replace(":", "-")
                self.report["images"][identity] = {**target, "image": image, "built_path": path,
                                                   "sha256": sanitizer.digest(payload), "test": test}
                events.append({"reason": "compiler-artifact", "target": {"name": target["name"], "kind": [target["kind"]],
                               "src_path": "/original/checkout/" + target["source"]}, "profile": {"test": test},
                               "executable": path, "fresh": False,
                               "features": [sanitizer.SYSTEM_ALLOCATOR_FEATURE] if target["package"] == "tsr" else []})
                args = ["--crate-name", target["name"], target["source"], "--target", self.report["target"], sanitizer.FLAGS]
                if test:
                    args.append("--test")
                if target["package"] == "tsr":
                    args += ["--cfg", 'feature="' + sanitizer.SYSTEM_ALLOCATOR_FEATURE + '"']
                self.audit(identity.replace(":", "-"), args, {path: sanitizer.digest(payload)})
                if test:
                    names = self.names(target)
                    name = identity.replace(":", "-")
                    executable = "/original/capture/" + image
                    listed = "".join(item + ": test\n" for item in names) + f"\n{len(names)} tests, 0 benchmarks\n"
                    output = "".join(f"test {item} ... ok\n" for item in names)
                    output += f"\ntest result: ok. {len(names)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
                    self.process(name + "-list", [executable, "--list", "--format=pretty"], listed.encode())
                    self.process(name + "-run", [executable, "--test-threads=1", "--show-output", "--format=pretty"], output.encode())
            events.append({"reason": "build-finished", "success": True})
            self.process("tests-build" if test else "harness-build", sanitizer.cargo_command(self.report, test),
                         b"\n".join(sanitizer.canonical(event) for event in events) + b"\n")
        output = self.directory / "scenarios"
        output.mkdir()
        rows = []
        for scenario in self.document["scenarios"]:
            text = b"scenario transcript\n"
            baseline = output / "baselines" / scenario["id"]
            baseline.parent.mkdir(parents=True, exist_ok=True)
            baseline.write_bytes(text)
            rows.append(corpus.completed_row(scenario, self.document["provenance"]["scenario_digests"][scenario["id"]], text))
        (output / "rows.jsonl").write_bytes(b"\n".join(sanitizer.canonical(row) for row in rows) + b"\n")
        sanitizer.write(output / "summary.json", {"version": 1, "pin": self.document["provenance"]["pin"],
                        "inventory": {"scenarios": 2, "orphan_references": self.document["orphan_references"]},
                        "selectors": ["all"], "rows": 2, "families": list(corpus.FAMILIES), "states": corpus.states_by_family(rows)})
        self.process("scenarios", sanitizer.scenario_command(self.report, "images/bin-phase4_tsctests-bin-phase4_tsctests"),
                     (output / "summary.json").read_bytes())
        self.bind()

    def names(self, target):
        required = sanitizer.required_test_names(target, self.report["host"])
        names = [f"module_{index}::{name}" for name, count in required.items() for index in range(count)]
        if target["package"] == "tsr_fswatch":
            names += ["watcher::roster::watch_file_create", "linux::tests::fanotify_backend_selection",
                      "linux::tests::fanotify_cross_watcher_same_filesystem", "linux::tests::fanotify_handle_key_round_trip"]
        return sorted(names)

    def audit(self, name, args, outputs=None):
        sanitizer.write(self.directory / "compiler-audit" / (name + ".json"), {
            "compiler": self.report["compiler"]["path"], "compiler_sha256": "b" * 64,
            "arguments": args, "cwd": "/original/checkout", "status": 0, "outputs": outputs or {}})

    def process(self, name, command, stdout, stderr=b""):
        directory = self.directory / "processes" / name
        directory.mkdir(parents=True)
        sanitizer.write(directory / "invocation.json", {"command": command, "cwd": "/original/checkout",
                        "environment": sanitizer.environment_record(self.env), "unset_environment": list(sanitizer.TEST_FILTERS),
                        "timeout_seconds": 7200})
        sanitizer.write(directory / "result.json", {"status": 0, "timeout": False, "error": None, "seconds": 1.0})
        (directory / "stdout").write_bytes(stdout)
        (directory / "stderr").write_bytes(stderr)

    def bind(self):
        self.report["artifacts"] = sanitizer.artifacts(self.directory)
        sanitizer.write(self.directory / "report.json", self.report)

    def verify(self, good=False, pattern=None):
        with mock.patch.object(sanitizer.subprocess, "run", side_effect=AssertionError("replay started a child")):
            try:
                result = sanitizer.verify_witnesses(self.directory, expected_host="linux")
            except (ValueError, OSError) as error:
                self.assertFalse(good, error)
                if pattern:
                    self.assertIn(pattern, str(error))
                return None
        self.assertIs(result["metrics"]["thread_sanitizer"], good, result)
        self.assertEqual(result["identities"]["thread_sanitizer"], sanitizer.digest((self.directory / "report.json").read_bytes()))
        if pattern:
            self.assertIn(pattern, result["details"]["error"])
        return result

    def mutate_json(self, relative, change):
        path = self.directory / relative
        value = sanitizer.load(path)
        change(value)
        sanitizer.write(path, value)
        self.bind()

    def test_full_receipt_is_child_free_and_relocatable(self):
        result = self.verify(good=True)
        self.assertEqual(result["details"]["scenarios"], 2)
        self.assertEqual(set(result["details"]["backends"]), {"inotify", "fanotify", "fanotify-no-rename"})
        moved = self.directory.with_name("moved")
        self.directory.rename(moved)
        self.directory = moved
        self.verify(good=True)

    def test_raw_tampering_is_rejected(self):
        (self.directory / "processes/scenarios/stderr").write_bytes(b"tampered")
        self.verify(pattern="artifacts changed")

    def test_sanitizer_diagnostic_fails_even_at_exit_zero_and_after_rebinding(self):
        (self.directory / "processes/scenarios/stderr").write_bytes(b"WARNING: ThreadSanitizer: data race\n")
        self.bind()
        self.verify(pattern="sanitizer diagnostic")

    def test_failed_child_cannot_be_overridden_by_report_claim(self):
        self.report["pass"] = True
        self.mutate_json("processes/scenarios/result.json", lambda value: value.update(status=66))
        self.verify(pattern="did not finish successfully")

    def test_missing_target_is_not_a_smaller_successful_suite(self):
        name = "test:phase4_tsctests:test:watcher_race"
        del self.report["images"][name]
        self.bind()
        self.verify(pattern="missing executed images")

    def test_cargo_must_report_all_required_targets(self):
        path = self.directory / "processes/tests-build/stdout"
        events = sanitizer.read_events(path)
        events = [event for event in events if event.get("target", {}).get("name") != "ownership"]
        path.write_bytes(b"\n".join(sanitizer.canonical(event) for event in events) + b"\n")
        self.bind()
        self.verify(pattern="missing required Cargo")

    def test_ignored_test_is_not_a_pass(self):
        path = self.directory / "processes/test-phase4_tsctests-test-ownership-run/stdout"
        path.write_bytes(path.read_bytes().replace(b" ... ok", b" ... ignored", 1))
        self.bind()
        self.verify(pattern="executed tests differ")

    def test_missing_execution_is_rejected(self):
        path = self.directory / "processes/test-phase4_tsctests-test-watcher_race-run/stdout"
        path.write_bytes(b"\n".join(path.read_bytes().splitlines()[1:]) + b"\n")
        self.bind()
        self.verify(pattern="executed tests differ")

    def test_shortened_list_cannot_shrink_the_required_source_roster(self):
        path = self.directory / "processes/test-phase4_tsctests-test-ownership-list/stdout"
        names = sanitizer.validate_inventory(path.read_bytes())[1:]
        path.write_text("".join(name + ": test\n" for name in names) + f"\n{len(names)} tests, 0 benchmarks\n")
        self.bind()
        self.verify(pattern="source-declared required tests")

    def test_successful_skip_does_not_claim_fanotify(self):
        path = self.directory / "processes/test-tsr_fswatch-lib-tsr_fswatch-run/stdout"
        path.write_bytes(path.read_bytes() + b"\nskip: fanotify not available\n")
        self.bind()
        result = self.verify(pattern="unavailable test/backend coverage")
        self.assertFalse(result["details"]["backends"]["fanotify"]["available"])
        self.assertFalse(result["details"]["backends"]["fanotify"]["tested"])

    def test_missing_instrumentation_on_actual_compiler_fails(self):
        self.mutate_json("compiler-audit/test-phase4_tsctests-test-ownership.json",
                         lambda value: value["arguments"].remove(sanitizer.FLAGS))
        self.verify(pattern="lacks ThreadSanitizer")

    def test_cli_cargo_artifact_must_select_system_allocator(self):
        path = self.directory / "processes/tests-build/stdout"
        events = sanitizer.read_events(path)
        for event in events:
            if event.get("target", {}).get("name") == "tsrust":
                event["features"] = []
        path.write_bytes(b"\n".join(sanitizer.canonical(event) for event in events) + b"\n")
        self.bind()
        self.verify(pattern="CLI Cargo artifact did not select the system allocator")

    def test_cli_compiler_must_select_system_allocator_even_when_cargo_claims_it(self):
        self.mutate_json("compiler-audit/test-tsr-bin-tsrust.json",
                         lambda value: value["arguments"].remove('feature="' + sanitizer.SYSTEM_ALLOCATOR_FEATURE + '"'))
        self.verify(pattern="CLI compiler invocation did not select the system allocator")

    def test_cli_feature_in_check_cfg_is_not_an_enabled_feature(self):
        self.mutate_json("compiler-audit/test-tsr-bin-tsrust.json",
                         lambda value: value["arguments"].__setitem__(value["arguments"].index("--cfg"), "--check-cfg"))
        self.verify(pattern="CLI compiler invocation did not select the system allocator")

    def test_absent_std_audit_fails_even_if_build_std_was_requested(self):
        (self.directory / "compiler-audit/std.json").unlink()
        self.bind()
        self.verify(pattern="standard library was not rebuilt")

    def test_truncated_compiler_arguments_withhold_instead_of_crashing_the_producer(self):
        self.mutate_json("compiler-audit/core.json", lambda value: value.update(arguments=["--target"]))
        with self.assertRaisesRegex(ValueError, "malformed sanitizer receipt"):
            sanitizer.verify_witnesses(self.directory, expected_host="linux")

    def test_another_actual_compiler_fails(self):
        self.mutate_json("compiler-audit/core.json", lambda value: value.update(compiler="/unrelated/rustc"))
        self.verify(pattern="another compiler")

    def test_replaced_image_fails_after_outer_artifacts_are_rebound(self):
        (self.directory / "images/test-phase4_tsctests-test-ownership").write_bytes(b"uninstrumented")
        self.bind()
        self.verify(pattern="no audited compiler output")

    def test_build_command_and_environment_are_checked(self):
        self.mutate_json("processes/tests-build/invocation.json", lambda value: value["command"].remove("-Zbuild-std"))
        self.verify(pattern="invocation schedule")

    def test_altered_runtime_options_are_checked(self):
        self.mutate_json("processes/scenarios/invocation.json", lambda value: value["environment"].update(TSAN_OPTIONS="exitcode=0"))
        self.verify(pattern="instrumentation environment")

    def test_test_filter_cannot_be_reintroduced_in_a_receipt(self):
        self.mutate_json("processes/test-tsr_incremental-test-buildinfo_witness-run/invocation.json",
                         lambda value: value["environment"].update(PHASE3_INCREMENTAL_CASE="one-case"))
        with self.assertRaisesRegex(ValueError, "test-filter environment"):
            sanitizer.verify_witnesses(self.directory)

    def test_nonfinite_json_cannot_be_authenticated(self):
        path = self.directory / "processes/scenarios/result.json"
        path.write_text('{"status":0,"timeout":false,"error":null,"seconds":NaN}')
        self.bind()
        with self.assertRaisesRegex(ValueError, "non-finite JSON"):
            sanitizer.verify_witnesses(self.directory)

    def test_partial_scenario_inventory_fails(self):
        path = self.directory / "scenarios/rows.jsonl"
        path.write_bytes(path.read_bytes().splitlines()[0] + b"\n")
        self.bind()
        self.verify(pattern="full scenario-harness inventory")

    def test_source_drift_fails(self):
        self.sources["added-test.rs"] = "d" * 64
        with self.assertRaisesRegex(ValueError, "stale sanitizer"):
            sanitizer.verify_witnesses(self.directory)

    def test_another_host_fails(self):
        with self.assertRaisesRegex(ValueError, "another host"):
            sanitizer.verify_witnesses(self.directory, expected_host="macos")


class EnvironmentAndFailure(unittest.TestCase):
    def test_only_cli_test_build_selects_system_allocator_feature(self):
        report = {"nightly": sanitizer.nightly(), "target": "x86_64-unknown-linux-gnu",
                  "compiler": {"path": "/rustc"}, "capture_root": "/capture"}
        test = sanitizer.cargo_command(report, True)
        self.assertEqual(test[test.index("--features") + 1], "tsr/system-allocator")
        self.assertNotIn("--features", sanitizer.cargo_command(report, False))

    def test_environment_keeps_homes_offline_and_registry_config(self):
        base = {"CARGO_HOME": "/existing/cargo", "RUSTUP_HOME": "/existing/rustup", "CARGO_NET_OFFLINE": "true",
                "CARGO_REGISTRIES_CORPORATE_INDEX": "https://mirror.example/index", "RUSTFLAGS": "-Zsanitizer=address",
                "RUSTC_WRAPPER": "skip-compiler", "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER": "true",
                "CARGO_ENCODED_RUSTFLAGS": "--cfg=fake", "TSAN_OPTIONS": "exitcode=0",
                "PHASE3_INCREMENTAL_CASE": "one-case-only", "CARGO_BUILD_RUSTC": "other-rustc", "RUSTUP_TOOLCHAIN": "stable", "LD_PRELOAD": "override.so"}
        result = sanitizer.instrumentation_environment(base)
        for key in ("CARGO_HOME", "RUSTUP_HOME", "CARGO_NET_OFFLINE", "CARGO_REGISTRIES_CORPORATE_INDEX"):
            self.assertEqual(result[key], base[key])
        for key in ("RUSTC_WRAPPER", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTC", "RUSTUP_TOOLCHAIN", "LD_PRELOAD",
                    "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER", "PHASE3_INCREMENTAL_CASE"):
            self.assertNotIn(key, result)
        self.assertEqual(result["RUSTFLAGS"], "-Zsanitizer=thread")
        self.assertEqual(result["TSAN_OPTIONS"], sanitizer.TSAN_OPTIONS)

    def test_absent_capture_withholds_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(FileNotFoundError):
                sanitizer.verify_witnesses(Path(directory) / "missing")

    def test_missing_toolchain_failure_is_retained_without_installing(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(sanitizer, "sources", return_value={}):
            with mock.patch.object(sanitizer.subprocess, "run", side_effect=FileNotFoundError("rustup unavailable")) as child:
                output = Path(directory) / "failed"
                result = sanitizer.run(output)
            self.assertFalse(result["metrics"]["thread_sanitizer"])
            self.assertIn("rustc-path failed", sanitizer.load(output / "report.json")["error"])
            self.assertEqual(child.call_count, 1)
            self.assertTrue((output / "processes/rustc-path/stderr").is_file())
            self.assertEqual(child.call_args.args[0][:2], ["rustup", "which"])

    def test_source_fingerprint_includes_tests_and_helpers(self):
        names = sanitizer.sources()
        for name in ("tools/phase4/tsctests/tests/watcher_race.rs", "tools/phase4/tsctests/tests/ownership.rs",
                     "crates/tsr_incremental/tests/buildinfo_witness.rs", "scripts/phase4_sanitizer.py",
                     "scripts/phase4_corpus.py", "data/s04/toolchains.toml", ".cargo/config.toml",
                     "tools/s08/p4/executor.rs", "tools/s07/program/rust_observation.rs", "tools/phase3/harness/incremental.rs"):
            self.assertIn(name, names)


if __name__ == "__main__":
    unittest.main()
