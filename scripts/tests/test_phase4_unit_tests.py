"""Phase 4 X0: the unit-test rosters (docs/PHASE4-plan.md section 4). The
committed roster is current and valid; a changed pinned test file, a new test
function, a second Rust carrier and a Rust comment naming an unknown test each
change the result."""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_unit_tests as roster  # noqa: E402

FSWATCH = "tsc/internal/fswatch/"
TSCTESTS = "tsc/internal/execute/tsctests/"


class Roster(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ported = roster.ported_tests()
        cls.document = roster.build_document(ported=cls.ported)
        cls.by_id = {entry["id"]: entry for entry in cls.document["tests"]}

    def copied_upstream(self, directory):
        """The pinned test files of the Phase 4 packages under a scratch upstream root."""
        upstream = Path(directory) / "upstream"
        for relative in roster.test_files(roster.UPSTREAM, roster.packages()):
            target = upstream / "tsc" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(roster.UPSTREAM / "tsc" / relative, target)
        return upstream

    def test_the_committed_roster_is_current_and_valid(self):
        self.assertEqual(self.document["problems"], [])
        self.assertEqual(roster.ROSTER.read_text(), roster.render(self.document))
        self.assertEqual(roster.check(), [])

    def test_the_counts(self):
        counts = self.document["counts"]
        self.assertEqual((counts["unit"], counts["direct"], counts["baseline"], counts["neither"]), (150, 24, 51, 1))
        self.assertEqual(counts["tsctests_functions"], 76)
        self.assertEqual(counts["unit_by_package"], {
            "cmd/tsc": 1, "internal/execute/build": 2, "internal/execute/incremental": 5, "internal/execute/tsc": 4,
            "internal/execute/watchmanager": 7, "internal/fswatch": 124, "internal/nativepath": 5,
            "internal/tracing": 2})
        self.assertEqual(counts["direct_by_file"], {"contentmapper_watch_test.go": 12, "watcher_race_test.go": 12})
        self.assertEqual(counts["by_checkpoint"], {"X1": 11, "X2": 5, "X3": 5, "X4": 124, "X5": 27, "X6": 2})
        self.assertEqual(counts["by_host"], {"any": 148, "linux": 8, "macos": 14, "windows": 4})
        # Five are ported at X0 (tsr_incremental); later ports move pending entries.
        self.assertEqual(counts["by_status"]["not_applicable"], 5)
        self.assertEqual(counts["by_status"].get("pending", 0) + counts["by_status"]["ported"], 169)
        self.assertGreaterEqual(counts["by_status"]["ported"], 5)
        self.assertEqual(self.document["plan_differences"], [
            "unit: plan 146, pin 150", "direct: plan 14, pin 24", "baseline: plan 61, pin 51",
            "unit tests of internal/execute/tsc: plan 0, pin 4",
            "direct tests of watcher_race_test.go: plan 2, pin 12"])
        self.assertNotIn("internal/compiler", self.document["scope"]["packages"])
        self.assertNotIn("internal/pprof", self.document["scope"]["packages"])

    def test_checkpoints_hosts_and_applicability(self):
        direct = [entry for entry in self.document["tests"] if entry["kind"] == "direct"]
        self.assertEqual({checkpoint: sum(entry["checkpoint"] == checkpoint for entry in direct)
                          for checkpoint in ("X1", "X3", "X5")}, {"X1": 1, "X3": 3, "X5": 20})
        self.assertEqual(self.by_id[TSCTESTS + "contentmapper_watch_test.go:"
                                    "TestContentMapperSupplementalDiagnosticUsesOriginalFileName"]["checkpoint"], "X1")
        for identity, host in ((FSWATCH + "fanotify_linux_test.go:TestLinuxFanotifyBackendSelection", "linux"),
                               (FSWATCH + "fsevents_darwin_shared_test.go:TestFSEventsOverflowMatchesWatch", "macos"),
                               ("tsc/internal/nativepath/realpath_darwin_test.go:TestRealpathHardlinkedFile", "macos"),
                               ("tsc/cmd/tsc/sys_unix_test.go:TestChildProcessCloseDoesNotWaitForLauncherDescendants",
                                "any"),
                               (FSWATCH + "watcher_test.go:TestNonRecursiveWithDeniedSubdir", "any")):
            self.assertEqual(self.by_id[identity]["host"], host, identity)
        trampoline = self.by_id[FSWATCH + "fsevents_darwin_ffi_arm64_test.go:TestCallbackASMTouchesOnlySafeRegisters"]
        self.assertEqual((trampoline["status"], trampoline["host"], trampoline["arch"]),
                         ("not_applicable", "macos", ["arm64"]))
        windows = [entry for entry in self.document["tests"] if entry["host"] == "windows"]
        self.assertEqual(len(windows), 4)
        self.assertTrue(all(entry["status"] == "not_applicable" and entry["package"] == "internal/nativepath"
                            for entry in windows))
        self.assertEqual(sum(entry["per_backend"] for entry in self.document["tests"]), 68)
        self.assertEqual(self.by_id["tsc/internal/execute/tsc/extendedconfigcache_test.go:"
                                    "TestExtendedConfigCacheExtendsCircularity"]["subtests"],
                         ["self-referencing extends", "mutual extends cycle",
                          "case-insensitive self-referencing extends"])

    def test_ported_tests_name_their_rust_test(self):
        incremental = [entry for entry in self.document["tests"] if entry["package"] == "internal/execute/incremental"]
        self.assertEqual(len(incremental), 5)
        for entry in incremental:
            self.assertEqual(entry["status"], "ported")
            self.assertTrue(entry["rust"][0]["test"].startswith("crates/tsr_incremental/src/tests.rs::"))
        for entry in self.document["tests"]:
            if entry["status"] == "ported":
                self.assertTrue(all(carrier["test"] for carrier in entry["rust"]), entry["id"])

    def test_a_changed_test_file_digest_changes_the_result(self):
        with tempfile.TemporaryDirectory() as directory:
            upstream = self.copied_upstream(directory)
            changed = upstream / "tsc/internal/tracing/tracing_test.go"
            changed.write_text(changed.read_text() + "\n// changed\n")
            document = roster.build_document(upstream=upstream, ported=self.ported)
            self.assertIn(f"the test files differ from the reviewed {roster.REVIEWED_TEST_FILES[0]}",
                          document["problems"])
            self.assertNotEqual(document["scope"]["test_files"]["tsc/internal/tracing/tracing_test.go"],
                                self.document["scope"]["test_files"]["tsc/internal/tracing/tracing_test.go"])
            self.assertTrue(any("differs from the rebuilt roster" in problem or "test files differ" in problem
                                for problem in roster.check(upstream=upstream)))

    def test_a_new_test_function_changes_the_result(self):
        with tempfile.TemporaryDirectory() as directory:
            upstream = self.copied_upstream(directory)
            changed = upstream / "tsc/internal/execute/watchmanager/watchmanager_test.go"
            changed.write_text(changed.read_text() + "\nfunc TestAddedLater(t *testing.T) {\n\tt.Parallel()\n}\n")
            document = roster.build_document(upstream=upstream, ported=self.ported)
            self.assertIn(f"the test functions differ from the reviewed {roster.REVIEWED_TEST_FUNCTIONS[0]}",
                          document["problems"])
            added = next(entry for entry in document["tests"] if entry["function"] == "TestAddedLater")
            self.assertEqual((added["checkpoint"], added["status"]), ("X5", "pending"))

    def test_a_second_carrier_and_an_unknown_test_change_the_result(self):
        identity = ("tsc/internal/execute/incremental/external_diagnostic_test.go:"
                    "TestExternalDiagnosticBuildInfoRoundTrip")
        ported = copy.deepcopy(self.ported)
        ported[identity].append({"site": "tools/phase4/x.rs:1", "test": "tools/phase4/x.rs::again"})
        document = roster.build_document(ported=ported)
        entry = next(entry for entry in document["tests"] if entry["id"] == identity)
        self.assertEqual(len(entry["rust"]), 2)
        self.assertNotEqual(roster.render(document), roster.render(self.document))
        ported = copy.deepcopy(self.ported)
        ported["tsc/internal/tracing/tracing_test.go:TestNoSuchTest"] = [
            {"site": "crates/tsr_tracing/src/tests.rs:1", "test": "crates/tsr_tracing/src/tests.rs::t"}]
        # A comment naming a test outside the Phase 4 packages is not this roster's.
        ported["tsc/internal/checker/checker_test.go:TestNoSuchTest"] = [
            {"site": "crates/tsr_checker/src/tests.rs:1", "test": "crates/tsr_checker/src/tests.rs::t"}]
        document = roster.build_document(ported=ported)
        self.assertEqual(list(document["unknown_ported"]), ["tsc/internal/tracing/tracing_test.go:TestNoSuchTest"])
        self.assertEqual(len(document["problems"]), 1)
        pending = "tsc/internal/tracing/tracing_test.go:TestThreadIDsAreStableAcrossFirstSeenOrder"
        ported[pending] = [{"site": "crates/tsr_tracing/src/tests.rs:9", "test": "crates/tsr_tracing/src/tests.rs::s"}]
        entry = next(entry for entry in roster.build_document(ported=ported)["tests"] if entry["id"] == pending)
        self.assertEqual(entry["status"], "ported")

    def test_build_constraints_and_skips(self):
        self.assertEqual(roster.file_constraint("fsevents_darwin_ffi_arm64_test.go"), (None, "arm64"))
        self.assertEqual(roster.file_constraint("realpath_darwin_test.go"), ("darwin", None))
        self.assertEqual(roster.file_constraint("symlink_windows_test.go"), ("windows", None))
        self.assertEqual(roster.file_constraint("sys_unix_test.go"), (None, None))
        self.assertTrue(roster.evaluate("darwin && (amd64 || arm64)", "darwin", "arm64"))
        self.assertFalse(roster.evaluate("darwin && (amd64 || arm64)", "linux", "arm64"))
        self.assertTrue(roster.evaluate("unix", "linux", "amd64"))
        self.assertFalse(roster.evaluate("!linux", "linux", "amd64"))
        pairs = {pair for pairs in roster.HOSTS.values() for pair in pairs}
        self.assertEqual(roster.host_of(pairs, 'if runtime.GOOS != "linux" {\n\tt.Skip("x")\n}'), "linux")
        self.assertEqual(roster.host_of(pairs, 'if runtime.GOOS == "darwin" {\n\tt.Skip("x")\n}'), "linux")
        self.assertEqual(roster.host_of(pairs, 'if runtime.GOOS == "windows" {\n\tt.Skip("x")\n}'), "any")

    def test_a_stale_committed_roster_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "unit-tests.json"
            stale = json.loads(roster.render(self.document))
            stale["counts"]["unit"] -= 1
            path.write_text(json.dumps(stale, indent=1, sort_keys=True) + "\n")
            self.assertTrue(any("differs from the rebuilt roster" in problem for problem in roster.check(path=path)))


if __name__ == "__main__":
    unittest.main()
