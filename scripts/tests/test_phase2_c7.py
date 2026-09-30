"""C7 starts from the content-mapper ledger move with its bound audit scope (docs/PHASE2-C7-plan.md, C7.8.0)."""
import copy
import importlib.util
from pathlib import Path
import sys
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402

_spec = importlib.util.spec_from_file_location("ledger_init", ROOT / "scripts/ledger-init.py")
ledger_init = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ledger_init)


class LedgerMove(unittest.TestCase):
    def test_the_content_mapper_files_the_rows_execute_are_phase_2(self):
        phases = {entry["go"]: entry["phase"] for entry in tomllib.loads((ROOT / "PORTS.toml").read_text())["file"]}
        self.assertEqual(len(ledger_init.CONTENT_MAPPER_FILES), 22)
        for path in ledger_init.CONTENT_MAPPER_FILES:
            self.assertEqual(phases[path], 2, path)
        # Reached by no row: the process transports, the synchronous conn, the
        # other mappers and the content-mapper baseline the oracle never takes.
        for path, phase in (("tsc/internal/ipc/transport.go", 6), ("tsc/internal/ipc/transport_unix.go", 6),
                            ("tsc/internal/ipc/conn_sync.go", 6),
                            ("tsc/internal/testutil/contentmappertest/manifest.go", 1),
                            ("tsc/internal/testutil/contentmappertest/verbatim.go", 1),
                            ("tsc/internal/testutil/tsbaseline/contentmapper_baseline.go", 1)):
            self.assertEqual(phases[path], phase, path)

    def test_the_c7_audit_scope_is_bound_over_complete_files_and_the_integration(self):
        document = audit.load(ROOT / "data/phase2/c7-audit.json")
        self.assertEqual(document["checkpoint"], "C7")
        self.assertEqual(audit.problems(document, allow_open=True), [])
        self.assertEqual(sum(count for count, _ in audit.C7_REVIEWED_GROUPS.values()), 237)
        known = audit.inventory()
        moved = {path + ":" for path in ledger_init.CONTENT_MAPPER_FILES}
        covered = {identity for group in audit.C7_COMPLETE_FILES for identity in document["groups"][group]}
        self.assertEqual(covered, {identity for identity in known if identity.startswith(tuple(moved))})
        altered = copy.deepcopy(document)
        altered["groups"]["C7.8.1 IPC connection and protocol (ipc)"].pop()
        self.assertTrue(any("complete pinned file" in problem or "reviewed function inventory" in problem
                            for problem in audit.problems(altered, allow_open=True)))
        self.assertFalse(audit.complete(document))


if __name__ == "__main__":
    unittest.main()
