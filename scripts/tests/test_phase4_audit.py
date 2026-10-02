"""Phase 4 X0: the function audit over the Phase 4 files left after the owner's
decisions (docs/PHASE4-plan.md sections 2, 4 and 5). The committed audit is
current and valid but not complete; a duplicate marker, a marker for an unknown
id and a removed marker each change the result and are problems."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase4_audit as audit  # noqa: E402

PROGRAM = audit.PROGRAM
WRITER = "tsc/internal/diagnosticwriter/diagnosticwriter.go"
SEEDED = (PROGRAM, WRITER) + tuple(f"tsc/internal/compiler/{name}.go" for name in (
    "host", "fileInclude", "fileloader", "filesparser", "includeprocessor", "processingDiagnostic",
    "projectreferencefilemapper", "projectreferenceparser", "pkg"))


class Audit(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.markers = audit.marker_sites()
        cls.document = audit.build_document(markers=cls.markers)

    def rebuilt(self, markers=None, reviewed=None):
        return audit.build_document(markers=self.markers if markers is None else markers, reviewed=reviewed)

    def entry(self, document, identity):
        return document["files"][identity.split(":", 1)[0]]["functions"][identity]

    def mapped(self, prefix, sites=1):
        return next(identity for row in self.document["files"].values() for identity, entry in row["functions"].items()
                    if identity.startswith(prefix) and entry["status"] == "mapped" and len(entry["rust"]) == sites)

    def test_the_committed_audit_is_current_valid_and_not_complete(self):
        self.assertEqual(self.document["problems"], [])
        self.assertEqual(audit.AUDIT.read_text(), audit.render(self.document))
        self.assertEqual(audit.check(), [])
        # Unported checkpoints keep functions pending: X7's closure check fails
        # and names the open functions per owning checkpoint.
        self.assertFalse(self.document["complete"])
        found = audit.check(complete=True)
        self.assertEqual(len(found), 1)
        self.assertTrue(found[0].startswith("the audit is not complete: "))
        for checkpoint in ("X1", "X3", "X4", "X5", "X6"):
            self.assertIn(f"pending {checkpoint}", found[0])
        self.assertEqual(self.document["totals"]["gap"], 0)
        self.assertEqual(self.document["totals"]["duplicate"], 0)

    def test_the_scope_is_the_ledger_phase_4_files_less_the_decision_4_moves(self):
        ledger = tomllib.loads((ROOT / "PORTS.toml").read_text())
        phase4 = {entry["go"] for entry in ledger["file"]
                  if entry.get("phase") == 4 and entry.get("kind") in ("source", "harness")}
        files, ids, _ = audit.scope()
        self.assertEqual(set(files), phase4 - set(audit.MOVED_OUT))
        self.assertEqual(set(audit.MOVED_OUT) & set(files), set())
        self.assertEqual((len(files), len(ids)), (67, 1007))
        scope = self.document["scope"]
        self.assertEqual((scope["files"], scope["functions"]), (67, 1007))
        # The plan's pre-Phase-3 denominator is the scope before the moves.
        before = scope["ledger"]["before_decisions"]
        self.assertEqual((before["files"], before["source"], before["harness"], before["functions"],
                          before["lines"]), (70, 65, 5, 1055, 22479))
        self.assertEqual((scope["ledger"]["ledger_phase4_files"], scope["ledger"]["ledger_out_of_scope"]), (80, 10))
        self.assertEqual(sum(row["counts"]["total"] for row in self.document["groups"].values()), 1007)
        self.assertEqual(set(self.document["groups"]), set(audit.GROUPS))
        for go, group in (("tsc/internal/execute/tsctests/runner.go", "X0"), ("tsc/cmd/tsc/sys.go", "X1"),
                          (PROGRAM, "X1"), (WRITER, "X1"), ("tsc/internal/execute/tsc/help.go", "X1"),
                          ("tsc/internal/execute/incremental/snapshot.go", "X2"),
                          ("tsc/internal/execute/build/buildtask.go", "X3"), ("tsc/internal/fswatch/watcher.go", "X4"),
                          ("tsc/internal/execute/watcher.go", "X5"),
                          ("tsc/internal/execute/watchmanager/watchmanager.go", "X5"),
                          ("tsc/internal/tracing/tracing.go", "X6"),
                          ("tsc/internal/execute/tsc/statistics.go", "X6")):
            self.assertEqual(audit.group_of(go), group, go)
        with self.assertRaises(ValueError):
            audit.group_of("tsc/internal/checker/checker.go")

    def test_the_seeded_files_have_every_unmarked_function_reviewed(self):
        for go in SEEDED:
            for identity, entry in self.document["files"][go]["functions"].items():
                if entry["status"] != "mapped":
                    self.assertIn(identity, audit.REVIEWED, identity)
        # 96 marked and 48 reviewed at X0; a port landing on a reviewed pending
        # function moves it to mapped.
        program = self.document["files"][PROGRAM]["counts"]
        self.assertEqual((program["total"], program["equivalent"], program["later"]), (144, 12, 19))
        self.assertEqual(program["mapped"] + program["pending"], 96 + 17)
        self.assertEqual(sum(review["disposition"] == "pending" for identity, review in audit.REVIEWED.items()
                             if identity.startswith(PROGRAM + ":")), 17)
        writer = self.document["files"][WRITER]["counts"]
        self.assertEqual((writer["total"], writer["mapped"], writer["equivalent"]), (40, 25, 15))
        other = [self.document["files"][go]["counts"] for go in SEEDED[2:]]
        self.assertEqual(sum(counts["total"] - counts["mapped"] for counts in other), 23)
        # Every function of incremental is marked (Phase 3's tsr_incremental).
        incremental = [row for go, row in self.document["files"].items()
                       if go.startswith("tsc/internal/execute/incremental/")]
        self.assertEqual(sum(row["counts"]["mapped"] for row in incremental), 175)
        self.assertEqual(sum(row["counts"]["total"] for row in incremental), 175)

    def test_dispositions_carry_their_owners_and_sites(self):
        later = {identity: entry for row in self.document["files"].values()
                 for identity, entry in row["functions"].items() if entry["status"] == "later"}
        self.assertEqual(self.document["later_by_owner"], {"Phase 5": 25, "Phase 6": 2, "Phase 7": 0})
        self.assertEqual(later["tsc/cmd/tsc/api.go:runAPI"]["owner"], "Phase 6")
        self.assertEqual(later["tsc/cmd/tsc/lsp.go:runLSP"]["owner"], "Phase 5")
        for row in self.document["files"].values():
            for identity, entry in row["functions"].items():
                if entry["status"] == "equivalent":
                    self.assertEqual(len(entry["rust"]), 1, identity)
                    self.assertTrue(entry["reason"].strip())
                if entry["status"] == "pending":
                    self.assertIn(entry["owner"], audit.CHECKPOINTS)
        for identity, owner in (*audit.OWNER_OVERRIDES.items(), (PROGRAM + ":Program.ReuseProgram", "X5"),
                                (PROGRAM + ":Program.LineCount", "X6"), (PROGRAM + ":Program.IsMissingPath", "X2"),
                                (PROGRAM + ":Program.ExplainFiles", "X1")):
            entry = self.entry(self.document, identity)
            self.assertIn(entry["status"], ("pending", "mapped"), identity)
            if entry["status"] == "pending":
                self.assertEqual(entry["owner"], owner, identity)
            elif identity in audit.REVIEWED:
                self.assertIn(identity, self.document["pending_reviews_superseded"])
        self.assertIn(WRITER + ":diagnosticPrefix", self.document["markers_to_add"])
        self.assertTrue(all(self.entry(self.document, identity)["marker_to_add"]
                            for identity in self.document["markers_to_add"]))
        pending = sum(self.document["pending_by_checkpoint"].values())
        self.assertEqual(pending, self.document["totals"]["pending"])

    def test_the_harness_group_closes_with_its_decoders_recorded_without_a_caller(self):
        # X0 ports the harness; the readable build info's four UnmarshalJSON
        # methods have no caller at the pin and are recorded as equivalent at
        # the encode-only Rust type.
        self.assertEqual(self.document["pending_by_checkpoint"]["X0"], 0)
        counts = self.document["groups"]["X0"]["counts"]
        self.assertEqual((counts["total"], counts["mapped"], counts["equivalent"], counts["pending"]), (96, 92, 4, 0))
        decoders = {identity: entry for identity, entry in
                    self.document["files"]["tsc/internal/execute/tsctests/readablebuildinfo.go"]["functions"].items()
                    if identity.endswith(".UnmarshalJSON")}
        self.assertEqual(len(decoders), 4)
        for identity, entry in decoders.items():
            self.assertEqual(entry["status"], "equivalent", identity)
            self.assertTrue(entry["rust"][0].startswith("tools/phase4/tsctests/src/readablebuildinfo.rs:"), identity)
            self.assertFalse(entry["marker_to_add"])
            self.assertIn("No caller at the pin", entry["reason"])

    def test_a_duplicate_marker_changes_the_result(self):
        identity = self.mapped(PROGRAM + ":")
        markers = copy.deepcopy(self.markers)
        markers[identity].append("crates/tsr_compiler/src/loader.rs:1")
        document = self.rebuilt(markers)
        self.assertEqual(self.entry(document, identity)["status"], "duplicate")
        self.assertEqual(document["groups"]["X1"]["counts"]["duplicate"], 1)
        self.assertTrue(any(problem.startswith(identity + ": 2 port markers") for problem in document["problems"]))
        self.assertNotEqual(audit.render(document), audit.render(self.document))
        # A reviewed multi-site marker gaining a site is a duplicate too, and
        # one losing a site is stale.
        multi = PROGRAM + ":Program.BindSourceFiles"
        self.assertEqual(self.entry(self.document, multi)["status"], "mapped")
        markers = copy.deepcopy(self.markers)
        markers[multi].append("crates/tsr_compiler/src/loader.rs:1")
        document = self.rebuilt(markers)
        self.assertEqual(self.entry(document, multi)["status"], "duplicate")
        markers[multi] = markers[multi][:1]
        self.assertIn(f"{multi}: reviewed as 2 marker sites, 1 found", self.rebuilt(markers)["problems"])

    def test_a_marker_for_an_unknown_id_changes_the_result(self):
        markers = copy.deepcopy(self.markers)
        for typo in (PROGRAM + ":Program.NoSuchMethod", "tsc/internal/fswatch/nosuchfile.go:watch",
                     "tsc/internal/execute/tsc/help.go:printNothing", "tsc/cmd/tsc/main.go:mian"):
            markers[typo] = ["crates/tsr_compiler/src/loader.rs:1"]
        # Unknown ids outside the Phase 4 files and packages are not this
        # audit's, and test-file ids are reported apart.
        markers["tsc/internal/checker/checker.go:Checker.noSuchFunction"] = ["crates/tsr_checker/src/lib.rs:1"]
        markers["tsc/internal/compiler/checkerpool.go:noSuchFunction"] = ["crates/tsr_compiler/src/lib.rs:1"]
        markers["tsc/internal/execute/tsctests/tscwatch_test.go:newTscEdit"] = ["tools/phase4/x.rs:1"]
        document = self.rebuilt(markers)
        self.assertEqual(sorted(document["unknown_markers"]), [
            "tsc/cmd/tsc/main.go:mian", PROGRAM + ":Program.NoSuchMethod",
            "tsc/internal/execute/tsc/help.go:printNothing", "tsc/internal/fswatch/nosuchfile.go:watch"])
        self.assertEqual(list(document["test_file_markers"]),
                         ["tsc/internal/execute/tsctests/tscwatch_test.go:newTscEdit"])
        self.assertEqual(len(document["problems"]), 4)
        self.assertNotEqual(audit.render(document), audit.render(self.document))

    def test_a_removed_marker_changes_the_result(self):
        # In a ported file the function becomes a gap; in an unported one, pending.
        identity = self.mapped("tsc/internal/execute/incremental/snapshot.go:")
        markers = {key: value for key, value in self.markers.items() if key != identity}
        document = self.rebuilt(markers)
        self.assertEqual(self.entry(document, identity)["status"], "gap")
        self.assertEqual(document["groups"]["X2"]["counts"]["gap"], 1)
        self.assertIn(f"{identity}: no port marker and no reviewed disposition", document["problems"])
        unported = "tsc/internal/execute/build/orchestrator.go:Orchestrator.Order"
        markers = dict(self.markers, **{unported: ["crates/tsr_compiler/src/loader.rs:1"]})
        self.assertEqual(self.entry(self.rebuilt(markers), unported)["status"], "mapped")
        markers.pop(unported)
        self.assertEqual(self.entry(self.rebuilt(markers), unported),
                         {"status": "pending", "owner": "X3", "reason": "not yet ported"})

    def test_a_reviewed_disposition_needs_its_function_unmarked_and_its_site(self):
        reviewed = copy.deepcopy(audit.REVIEWED)
        marked = self.mapped(WRITER + ":")
        reviewed[marked] = {"disposition": "equivalent", "rust": ("crates/tsr_compiler/src/lib.rs", "fn"),
                            "reason": "r"}
        anchored = WRITER + ":diagnosticPrefix"
        reviewed[anchored] = dict(reviewed[anchored], rust=(reviewed[anchored]["rust"][0], "no such anchor text"))
        reviewed["tsc/internal/checker/checker.go:Checker.checkSourceFile"] = {
            "disposition": "later", "owner": "Phase 5", "reason": "r"}
        problems = self.rebuilt(reviewed=reviewed)["problems"]
        self.assertIn(f"{marked}: a port marker names it, so its reviewed disposition is stale", problems)
        self.assertTrue(any(problem.startswith(f"{anchored}: equivalent site") for problem in problems))
        self.assertIn("tsc/internal/checker/checker.go:Checker.checkSourceFile: reviewed, but not a Phase 4 function",
                      problems)

    def test_a_marker_supersedes_a_pending_review_and_stales_any_other(self):
        pending, equivalent = PROGRAM + ":Program.ExplainFiles", WRITER + ":diagnosticPrefix"
        markers = dict(self.markers, **{pending: ["tools/phase4/x.rs:1"], equivalent: ["crates/x.rs:1"]})
        document = self.rebuilt(markers)
        self.assertEqual(self.entry(document, pending), {"status": "mapped", "rust": ["tools/phase4/x.rs:1"]})
        self.assertIn(pending, document["pending_reviews_superseded"])
        self.assertEqual(document["problems"],
                         [f"{equivalent}: a port marker names it, so its reviewed disposition is stale"])

    def test_later_and_pending_need_an_owner(self):
        identity = PROGRAM + ":Program.HasTSFile"
        for kind, owner, accepted in (("later", "Phase 5", True), ("later", "Phase 7", True),
                                      ("later", "Phase 4", False), ("later", "X5", False),
                                      ("pending", "X5", True), ("pending", "Phase 5", False),
                                      ("gap", "X5", False)):
            reviewed = dict(audit.REVIEWED, **{identity: {"disposition": kind, "owner": owner, "reason": "r"}})
            document = self.rebuilt(reviewed=reviewed)
            self.assertEqual(self.entry(document, identity)["status"], kind)
            self.assertEqual(document["problems"] == [], accepted, (kind, owner))

    def test_a_stale_committed_audit_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "x-audit.json"
            stale = json.loads(audit.render(self.document))
            stale["totals"]["mapped"] -= 1
            path.write_text(json.dumps(stale, indent=1, sort_keys=True) + "\n")
            self.assertTrue(any("differs from the rebuilt audit" in problem for problem in audit.check(path=path)))


if __name__ == "__main__":
    unittest.main()
