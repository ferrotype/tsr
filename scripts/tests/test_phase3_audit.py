"""Phase 3 C6: the function audit over the 73 Phase 3 files and the Emit family
of program.go (docs/PHASE3-plan.md section 5). The committed audit is current;
a duplicate marker, a marker for an unknown id and a removed marker each change
the result and are problems."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_audit as audit  # noqa: E402


class Audit(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.markers = audit.marker_sites()
        cls.document = audit.build_document(markers=cls.markers)

    def rebuilt(self, markers=None, reviewed=None):
        return audit.build_document(markers=self.markers if markers is None else markers, reviewed=reviewed)

    def mapped(self, prefix):
        return next(identity for row in self.document["files"].values() for identity, entry in row["functions"].items()
                    if identity.startswith(prefix) and entry["status"] == "mapped")

    def test_the_committed_audit_is_current_and_valid(self):
        self.assertEqual(self.document["problems"], [])
        self.assertEqual(audit.AUDIT.read_text(), audit.render(self.document))
        self.assertEqual(audit.check(), [])
        # The transpile package is pending until unit C3 lands, so T8's
        # closure check still fails.
        self.assertFalse(self.document["complete"])
        self.assertTrue(audit.check(complete=True))

    def test_the_scope_is_the_ledger_phase_3_files_the_moved_files_and_the_emit_family(self):
        ledger = tomllib.loads((ROOT / "PORTS.toml").read_text())
        phase3 = {entry["go"] for entry in ledger["file"] if entry.get("phase") == 3}
        files, ids, _ = audit.scope()
        self.assertEqual(set(files), phase3 | set(audit.MOVED))
        self.assertEqual((len(files), len(ids)), (73, 1942))
        self.assertEqual(ids[-len(audit.EMIT_FAMILY):], list(audit.EMIT_FAMILY))
        self.assertEqual(len(audit.EMIT_FAMILY), 10)
        scope = self.document["scope"]
        self.assertEqual((scope["files"], scope["functions"]), (73, 1942))
        self.assertEqual(sum(row["counts"]["total"] for row in self.document["groups"].values()), 1942)
        self.assertEqual(set(self.document["groups"]), set(audit.GROUPS))
        self.assertEqual(self.document["files"][audit.PROGRAM]["group"], "T8")
        self.assertEqual(set(self.document["files"][audit.PROGRAM]["functions"]), set(audit.EMIT_FAMILY))
        for go, group in (("tsc/internal/printer/printer.go", "T1"), ("tsc/internal/sourcemap/generator.go", "T2"),
                          ("tsc/internal/transformers/chain.go", "T3"),
                          ("tsc/internal/transformers/inliners/constenum.go", "T4"),
                          ("tsc/internal/transformers/estransforms/using.go", "T5"),
                          ("tsc/internal/transformers/jsxtransforms/jsx.go", "T6"),
                          ("tsc/internal/pseudochecker/lookup.go", "T7"),
                          ("tsc/internal/compiler/emitHost.go", "T8")):
            self.assertEqual(audit.group_of(go), group, go)
        with self.assertRaises(ValueError):
            audit.group_of("tsc/internal/checker/checker.go")

    def test_a_duplicate_marker_changes_the_result(self):
        identity = self.mapped("tsc/internal/printer/printer.go:")
        markers = copy.deepcopy(self.markers)
        markers[identity].append("crates/tsr_printer/src/printer.rs:1")
        document = self.rebuilt(markers)
        go = identity.split(":", 1)[0]
        self.assertEqual(document["files"][go]["functions"][identity]["status"], "duplicate")
        self.assertEqual(document["groups"]["T1"]["counts"]["duplicate"], 1)
        self.assertTrue(any(problem.startswith(identity + ": 2 port markers") for problem in document["problems"]))
        self.assertNotEqual(audit.render(document), audit.render(self.document))

    def test_a_marker_for_an_unknown_id_changes_the_result(self):
        markers = copy.deepcopy(self.markers)
        for typo in ("tsc/internal/printer/printer.go:Printer.emitNothing",
                     "tsc/internal/transformers/estransforms/nosuchfile.go:visit",
                     "tsc/internal/compiler/emitter.go:Emitter.emit"):
            markers[typo] = ["crates/tsr_printer/src/printer.rs:1"]
        # Unknown ids outside the Phase 3 files and packages are not this audit's.
        markers["tsc/internal/checker/checker.go:Checker.noSuchFunction"] = ["crates/tsr_checker/src/lib.rs:1"]
        document = self.rebuilt(markers)
        self.assertEqual(sorted(document["unknown_markers"]), [
            "tsc/internal/compiler/emitter.go:Emitter.emit",
            "tsc/internal/printer/printer.go:Printer.emitNothing",
            "tsc/internal/transformers/estransforms/nosuchfile.go:visit"])
        self.assertEqual(len(document["problems"]), 3)
        self.assertNotEqual(audit.render(document), audit.render(self.document))

    def test_a_removed_marker_changes_the_result(self):
        identity = self.mapped("tsc/internal/transformers/estransforms/classfields.go:")
        markers = {key: value for key, value in self.markers.items() if key != identity}
        document = self.rebuilt(markers)
        go = identity.split(":", 1)[0]
        self.assertEqual(document["files"][go]["functions"][identity]["status"], "gap")
        self.assertEqual(document["groups"]["T5"]["counts"]["gap"], 1)
        self.assertIn(f"{identity}: no port marker and no reviewed disposition", document["problems"])
        self.assertFalse(document["complete"])

    def test_a_reviewed_disposition_needs_its_function_unmarked_and_its_site(self):
        reviewed = copy.deepcopy(audit.REVIEWED)
        marked = self.mapped("tsc/internal/sourcemap/generator.go:")
        reviewed[marked] = {"disposition": "equivalent", "rust": ("crates/tsr_sourcemap/src/lib.rs", "fn"),
                            "reason": "r"}
        first = next(iter(reviewed))
        reviewed[first] = dict(reviewed[first], rust=(reviewed[first]["rust"][0], "no such anchor text"))
        reviewed["tsc/internal/checker/checker.go:Checker.checkSourceFile"] = {
            "disposition": "later", "owner": "Phase 4", "reason": "r"}
        problems = self.rebuilt(reviewed=reviewed)["problems"]
        self.assertIn(f"{marked}: a port marker names it, so its reviewed disposition is stale", problems)
        self.assertTrue(any(problem.startswith(f"{first}: equivalent site") for problem in problems))
        self.assertIn("tsc/internal/checker/checker.go:Checker.checkSourceFile: reviewed, but not a Phase 3 function",
                      problems)

    def test_later_and_gap_need_an_owner(self):
        identity = "tsc/internal/printer/utilities.go:isNotPrologueDirective"
        for kind, owner, accepted in (("later", "Phase 4", True), ("later", "T8", False),
                                      ("gap", "T1", True), ("gap", "Phase 4", False)):
            reviewed = dict(audit.REVIEWED, **{identity: {"disposition": kind, "owner": owner, "reason": "r"}})
            document = self.rebuilt(reviewed=reviewed)
            self.assertEqual(document["files"]["tsc/internal/printer/utilities.go"]["functions"][identity]["status"],
                             kind)
            self.assertEqual(document["problems"] == [], accepted, (kind, owner))

    def test_every_unmarked_function_is_reviewed_or_pending(self):
        for row in self.document["files"].values():
            for identity, entry in row["functions"].items():
                if entry["status"] == "pending_c3":
                    self.assertTrue(identity.startswith("tsc/internal/transpile/"), identity)
                elif entry["status"] == "equivalent":
                    self.assertIn(identity, audit.REVIEWED)
                    self.assertEqual(len(entry["rust"]), 1)
                else:
                    self.assertEqual(entry["status"], "mapped", identity)
                    self.assertEqual(len(entry["rust"]), 1, identity)

    def test_a_stale_committed_audit_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "audit.json"
            stale = json.loads(audit.render(self.document))
            stale["totals"]["mapped"] -= 1
            path.write_text(json.dumps(stale, indent=1, sort_keys=True) + "\n")
            self.assertTrue(any("differs from the rebuilt audit" in problem for problem in audit.check(path=path)))


if __name__ == "__main__":
    unittest.main()
