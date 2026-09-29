"""C6 starts from the concurrent native capture, the moved checker-pool ledger entry, the bound audit scope and
the C6-start baseline; the two native modes differ only where the pin's own union-ordering walk counts checkers."""
import copy
import json
from pathlib import Path
import sys
import tomllib
import unittest
import unittest.mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402
import phase2_blockers as blockers  # noqa: E402
import phase2_native_concurrent as concurrent  # noqa: E402
import phase2_producers as producers  # noqa: E402


class Wiring(unittest.TestCase):
    def test_c6_authorities_are_checker_inputs_and_the_audit_scope_is_bound(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in producers.CHECKPOINT_AUTHORITIES["C6"].values():
            self.assertIn(str(path.relative_to(ROOT)), spec["inputs"])
        self.assertIn("data/phase2/native-provenance-concurrent.json", spec["inputs"])
        self.assertIn("C6", blockers.CHECKPOINT_CLAIMS)
        self.assertEqual(producers.CHECKPOINTS[-1], "C6")
        document = audit.load(ROOT / "data/phase2/c6-audit.json")
        self.assertEqual(document["checkpoint"], "C6")
        self.assertEqual(audit.problems(document, allow_open=True), [])
        for group, (count, _) in audit.C6_REVIEWED_GROUPS.items():
            self.assertEqual(len(document["groups"][group]), count)
        self.assertEqual(sum(count for count, _ in audit.C6_REVIEWED_GROUPS.values()), 89)
        self.assertEqual(len(audit.C6_COMPLETE_FILES), 3)
        altered = copy.deepcopy(document)
        altered["groups"]["C6.3 compiler checker pool (checkerpool.go)"].pop()
        self.assertTrue(audit.problems(altered, allow_open=True))

    def test_the_checker_pool_ledger_entry_is_phase_2(self):
        ledger = tomllib.loads((ROOT / "PORTS.toml").read_text())
        phases = {entry["go"]: entry["phase"] for entry in ledger["file"]}
        self.assertEqual(phases["tsc/internal/compiler/checkerpool.go"], 2)
        self.assertEqual(phases["tsc/internal/compiler/program.go"], 4)
        self.assertEqual(phases["tsc/internal/project/checkerpool.go"], 5)


class NativeModes(unittest.TestCase):
    def test_the_concurrent_capture_is_bound_to_its_mode_and_the_mode_differences_are_recorded(self):
        provenance = json.loads((ROOT / "data/phase2/native-provenance-concurrent.json").read_text())
        self.assertEqual((provenance["mode"], provenance["single_threaded"]), ("concurrent", False))
        self.assertEqual(provenance["reference_disagreements"], [])
        single = json.loads((ROOT / "data/phase2/native-provenance.json").read_text())
        self.assertIs(single["single_threaded"], True)
        self.assertEqual((provenance["go"], provenance["goos"], provenance["goarch"]),
                         (single["go"], single["goos"], single["goarch"]))
        self.assertEqual(provenance["oracle"], single["oracle"])
        claims = json.loads((ROOT / "data/phase2/c6-claims.json").read_text())
        modes = claims["native_modes"]
        self.assertEqual(modes["differences"], [])
        self.assertEqual(modes["concurrent"]["capture_observation_sha256"], provenance["observation_sha256"])
        self.assertEqual(modes["single"]["capture_observation_sha256"], single["observation_sha256"])
        self.assertEqual(claims["rows"], [])

    def test_a_single_threaded_report_is_not_a_concurrent_capture(self):
        report = {"mode": "single", "single_threaded": True, "partial": False}
        with self.assertRaisesRegex(ValueError, "not a concurrent-mode native capture"):
            with unittest.mock.patch.object(concurrent.native, "load_capture",
                                            return_value=(ROOT, report, [])):
                concurrent.load_capture(ROOT)


if __name__ == "__main__":
    unittest.main()
