"""C7 starts from the content-mapper ledger move with its bound audit scope, and from fingerprints that leave the
crates' test-only suites to the contract receipts (docs/PHASE2-C7-plan.md, C7.8.0 and decision 8)."""
import copy
import importlib.util
from pathlib import Path
import sys
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit as audit  # noqa: E402
import phase2_corpus as corpus  # noqa: E402
import phase2_order_trace as order_trace  # noqa: E402
import phase2_producers as producers  # noqa: E402

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


class Fingerprints(unittest.TestCase):
    def test_test_only_suites_leave_the_capture_and_checker_fingerprints(self):
        self.assertTrue(corpus.test_only("crates/tsr_compiler/tests/fixtures/c2/contextual_audit.json"))
        self.assertFalse(corpus.test_only("crates/tsr_compiler/src/program.rs"))
        self.assertFalse(corpus.test_only("crates/tsr_compiler/examples/phase2_checker.rs"))
        sources = corpus.sources()
        self.assertIn("crates/tsr_compiler/src/lib.rs", sources)
        self.assertFalse([path for path in sources if corpus.test_only(path)])
        self.assertFalse([path for path in producers.production_inputs() if corpus.test_only(path)])
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        self.assertIn("crates/**", spec["sources"])
        self.assertEqual(spec["exclude"], ["crates/*/tests/**"])
        # The contract receipts still bind the suites they run.
        receipt = producers.source_inputs(producers.WITNESSES["c2-contracts"]["sources"])
        self.assertIn("crates/tsr_compiler/tests/fixtures/c2/contextual_audit.json", receipt)

    def test_the_order_trace_example_keeps_the_test_support_it_includes(self):
        program = "crates/tsr_compiler/tests/support/c2_order_program.rs"
        self.assertIn(f'#[path = "../tests/support/{Path(program).name}"]',
                      (ROOT / "crates/tsr_compiler/examples/c2_order_trace.rs").read_text())
        self.assertIn(program, order_trace.rust_inputs())


if __name__ == "__main__":
    unittest.main()
