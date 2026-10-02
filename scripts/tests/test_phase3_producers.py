"""Phase 3: the comparison over whole captures, the recorded result, the
blocker register, the two-mode comparison and the `emit` producer's harness
validity and parity metrics, over captures built from the four real rows of
fixtures/phase3/corpus-rows.json (a native capture of those rows and a Rust
capture of each mode). A tampered, partial or stale capture is rejected."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase3_blockers as blockers  # noqa: E402
import phase3_compare as compare  # noqa: E402
import phase3_corpus as corpus  # noqa: E402
import phase3_native as native  # noqa: E402
import phase3_producers as producers  # noqa: E402
import phase3_report as report  # noqa: E402
import phase3_residuals as residuals  # noqa: E402
import s08_p4 as p4  # noqa: E402
from s08_oracle import canonical, digest  # noqa: E402

FIXTURE = ROOT / "scripts/tests/fixtures/phase3/corpus-rows.json"
SOURCE = "tools/phase3/harness/reprint.rs"
SOURCES = {SOURCE: digest((ROOT / SOURCE).read_bytes())}
REFUSAL = "unsupported checker operation: GetReferencedImportDeclaration"
FULL = {"sample": False, "cases": [], "limit": None}


def write_native(directory, rows, mode):
    """A native capture of `rows` in `mode`, bound as phase3_native writes one."""
    directory.mkdir(parents=True)
    raw = b"".join(canonical(row) + b"\n" for row in rows)
    (directory / "observations.ndjson").write_bytes(raw)
    report = {"version": 1, "pin": "1f70213d4922b434345f639b441681e470c7cfc1", "mode": mode, "partial": False,
              "observation_sha256": digest(raw), "row_sha256": [native.contract_digest(row) for row in rows]}
    (directory / "report.json").write_bytes(canonical(report) + b"\n")
    return report


def write_rust(directory, native_dir, requests, rows, mode, *, partial=False, complete=True):
    """A Rust capture of `rows`, bound to `native_dir` and built from SOURCES."""
    (directory / "cases").mkdir(parents=True)
    snapshot = directory / "source-snapshot" / SOURCE
    snapshot.parent.mkdir(parents=True)
    snapshot.write_bytes((ROOT / SOURCE).read_bytes())
    (directory / "executable").write_bytes(b"binary")
    report = json.loads((native_dir / "report.json").read_bytes())
    selection = {"sample": False, "cases": [r["id"] for r in requests], "limit": None} if partial else FULL
    metadata = {"version": 1, "requests_sha256": digest(p4.canonical(requests) + b"\n"),
                "build": {"sources": dict(SOURCES), "binary_sha256": digest(b"binary")}, "timeout_seconds": 60,
                "native": {"directory": str(native_dir), "report_sha256": digest((native_dir / "report.json").read_bytes()),
                           "observation_sha256": report["observation_sha256"], "mode": mode},
                "partial": partial, "selection": selection, "mode": mode}
    p4.write_new(directory / "requests.json", requests)
    p4.write_new(directory / "capture.json", metadata)
    for index, (request, row) in enumerate(zip(requests, rows)):
        if index and not complete:
            break
        case = directory / "cases" / f"{index:05d}"
        p4.begin_case(case, request)
        (case / "stdout").write_bytes(b"")
        (case / "stderr").write_bytes(b"")
        (case / "observation.json").write_bytes(json.dumps(row).encode())
        p4.complete_case(case, request, metadata, row)
    return metadata


def with_mode(request, row, mode):
    request, row = copy.deepcopy(request), copy.deepcopy(row)
    request["mode"] = row["mode"] = mode
    return request, row


class Fixture(unittest.TestCase):
    """Patches the source fingerprint to SOURCES and the native capture's
    input currency (a pin-and-overlay check the native tests cover)."""

    @classmethod
    def setUpClass(cls):
        cls.fixture = json.loads(FIXTURE.read_bytes())["rows"]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        patches = [mock.patch.object(corpus, "sources", lambda: dict(SOURCES)),
                   mock.patch.object(native, "current", lambda report: None)]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        self.addCleanup(self.temporary.cleanup)

    def captures(self, mode="single", mutate=None, **options):
        """(native_dir, rust_dir) over the four fixture rows; `mutate(rows)`
        edits the Rust rows before they are written."""
        native_dir, rust_dir = self.root / f"native-{mode}", self.root / f"rust-{mode}"
        if not native_dir.exists():
            write_native(native_dir, [row["native"] for row in self.fixture], mode)
        pairs = [with_mode(row["request"], row["rust"], mode) for row in self.fixture]
        requests, rows = [p[0] for p in pairs], [p[1] for p in pairs]
        if mutate:
            mutate(rows)
        write_rust(rust_dir, native_dir, requests, rows, mode, **options)
        return native_dir, rust_dir


class Comparison(Fixture):
    def test_a_complete_capture_compares_and_records(self):
        native_dir, rust_dir = self.captures()
        result = compare.report(native_dir, rust_dir)
        summary = result["summary"]
        self.assertEqual((summary["rows"], summary["valid"], summary["partial"], summary["all_domains_met"],
                          summary["unsupported_rows"], summary["declaration_required"]), (4, True, False, 3, 0, 1))
        self.assertEqual(summary["domains"]["reprint"]["match"], 4)
        self.assertEqual(summary["domains"]["output"], {"match": 2, "different": 1, "failed": 0, "unsupported": 0,
                                                        "disabled": 1, "unexecuted": 0})
        for domain in ("sourcemap", "sourcemap_record", "emit_diagnostics"):
            self.assertEqual(summary["domains"][domain]["match"], 4)
        self.assertEqual(summary["domains"]["declaration"]["different"], 1)
        self.assertEqual([(item["domain"], item["kind"], item["rows"]) for item in result["buckets"]["different"]],
                         [("declaration", "dts", 1), ("output", "dts", 1)])
        self.assertTrue(result["rust"]["source_stable"])
        self.assertEqual(result["buckets"]["reprint_refusals_matching_a_pinned_panic"]
                         ["unhandled statement: KindJSImportDeclaration"]["rows"], 1)
        target = self.root / "first-comparison.json"
        compare.record(result, target)
        self.assertEqual(json.loads(target.read_bytes()), compare.acceptance_summary(result))
        self.assertNotIn("rows", json.loads(target.read_bytes()))

    def test_a_tampered_capture_is_rejected(self):
        native_dir, rust_dir = self.captures()
        envelope = json.loads((rust_dir / "cases/00000/result.json").read_bytes())
        envelope["row"]["reprint"]["files"][0]["comments"]["bytes"] += 1
        (rust_dir / "cases/00000/result.json").write_bytes(json.dumps(envelope).encode())
        with self.assertRaisesRegex(ValueError, "completion differs from its raw observation"):
            compare.report(native_dir, rust_dir)
        native_dir, rust_dir = self.root / "native-single", self.root / "other"
        write_rust(rust_dir, native_dir, [row["request"] for row in self.fixture],
                   [row["rust"] for row in self.fixture], "single")
        (rust_dir / "requests.json").write_bytes((rust_dir / "requests.json").read_bytes() + b" ")
        with self.assertRaisesRegex(ValueError, "capture request inventory changed"):
            compare.report(native_dir, rust_dir)

    def test_a_capture_against_another_native_capture_is_rejected(self):
        native_dir, rust_dir = self.captures()
        observations = native_dir / "observations.ndjson"
        rows = [json.loads(line) for line in observations.read_bytes().splitlines()]
        rows[0]["diagnostics"] += 1
        write_native(self.root / "native-other", rows, "single")
        with self.assertRaisesRegex(ValueError, "another native capture"):
            compare.report(self.root / "native-other", rust_dir)
        observations.write_bytes(observations.read_bytes() + b"\n")
        with self.assertRaisesRegex(ValueError, "changed since its report"):
            compare.report(native_dir, rust_dir)

    def test_a_partial_capture_is_informational(self):
        native_dir, rust_dir = self.captures(partial=True)
        result = compare.report(native_dir, rust_dir)
        self.assertTrue(result["summary"]["partial"])
        with self.assertRaisesRegex(ValueError, "partial"):
            compare.record(result, self.root / "first-comparison.json")
        with self.assertRaisesRegex(ValueError, "full comparison"):
            blockers.build(native_dir, rust_dir)
        native_dir, rust_dir = self.root / "native-single", self.root / "incomplete"
        write_rust(rust_dir, native_dir, [row["request"] for row in self.fixture],
                   [row["rust"] for row in self.fixture], "single", complete=False)
        with self.assertRaisesRegex(ValueError, "capture incomplete"):
            compare.report(native_dir, rust_dir)

    def test_a_stale_source_is_never_recorded(self):
        native_dir, rust_dir = self.captures()
        with mock.patch.object(corpus, "sources", lambda: {SOURCE: "0" * 64}):
            result = compare.report(native_dir, rust_dir)
            self.assertFalse(result["rust"]["source_stable"])
            self.assertFalse(corpus.replay(rust_dir, write=False)["source_stable"])
            with self.assertRaisesRegex(ValueError, "current sources"):
                compare.record(result, self.root / "first-comparison.json")
        concurrent = compare.report(*self.captures("concurrent"))
        with self.assertRaisesRegex(ValueError, "single-threaded"):
            compare.record(concurrent, self.root / "first-comparison.json")


class HarnessValidity(Fixture):
    def state(self, mode="single", rust_dir=None, native_dir=None, requests_sha256=None, executed=4):
        if rust_dir is None:
            native_dir, rust_dir = self.captures(mode)
        loaded = compare.load_native(native_dir)
        capture = corpus.load_capture(rust_dir)
        replayed = corpus.replay(rust_dir, write=False, capture=capture)
        comparison = compare.report(native_dir, rust_dir, native=loaded, capture=capture)
        return producers.capture_valid(mode, capture[0], replayed, comparison, loaded[1],
                                       digest((native_dir / "report.json").read_bytes()),
                                       requests_sha256 or capture[0]["requests_sha256"], executed)

    def test_a_complete_current_capture_is_valid_in_its_mode(self):
        self.assertTrue(self.state())
        self.assertTrue(self.state("concurrent"))
        native_dir, rust_dir = self.root / "native-single", self.root / "rust-single"
        self.assertFalse(self.state("concurrent", rust_dir, native_dir))

    def test_a_partial_capture_is_not_valid(self):
        native_dir, rust_dir = self.captures(partial=True)
        self.assertFalse(self.state(rust_dir=rust_dir, native_dir=native_dir))

    def test_a_capture_missing_rows_of_the_denominator_is_not_valid(self):
        self.assertFalse(self.state(executed=5))

    def test_a_stale_source_or_request_is_not_valid(self):
        native_dir, rust_dir = self.captures()
        with mock.patch.object(corpus, "sources", lambda: {SOURCE: "0" * 64}):
            self.assertFalse(self.state(rust_dir=rust_dir, native_dir=native_dir))
        self.assertFalse(self.state(rust_dir=rust_dir, native_dir=native_dir, requests_sha256="0" * 64))
        self.assertTrue(self.state(rust_dir=rust_dir, native_dir=native_dir))

    def test_a_harness_error_is_not_valid(self):
        def adapter_panic(rows):
            rows[0]["reprint"]["files"][0]["comments"] = {"state": "failed", "class": "panic", "reason": "boom",
                                                          "location": "tools/phase3/harness/reprint.rs:40:9"}
        native_dir, rust_dir = self.captures(mutate=adapter_panic)
        self.assertFalse(self.state(rust_dir=rust_dir, native_dir=native_dir))
        self.assertEqual(compare.report(native_dir, rust_dir)["summary"]["harness_errors"], 1)

    def test_the_recorded_result_is_the_acceptance_summary(self):
        native_dir, rust_dir = self.captures()
        result = compare.report(native_dir, rust_dir)
        target = self.root / "first-comparison.json"
        compare.record(result, target)
        again = compare.report(native_dir, rust_dir)
        self.assertEqual(json.loads(target.read_bytes()), compare.acceptance_summary(again))
        again["summary"]["domains"]["reprint"]["match"] -= 1
        self.assertNotEqual(json.loads(target.read_bytes()), compare.acceptance_summary(again))


class Parity(Fixture):
    def test_parity_is_the_matched_fraction_of_each_domain(self):
        comparison = compare.report(*self.captures())
        metrics = producers.parity(comparison["rows"])
        self.assertEqual(metrics, {"output_parity": 0.75, "sourcemap_parity": 1.0, "sourcemap_record_parity": 1.0,
                                   "emit_diagnostics_parity": 1.0, "declaration_parity": 0.0})
        self.assertEqual(producers.matched_ratio(comparison["rows"], "reprint"), 1.0)

    def test_declaration_parity_counts_the_rows_that_require_it(self):
        """The pin's declaration rows, and any row where Rust differs: a
        Rust-only declaration counts against it."""
        def extra(rows):
            rows[0]["emit"]["outputs"]["dts"] = [{"name_hex": "2f2e7372632f32644172726179732e642e7473",
                                                  "sha256": "0" * 64, "bytes": 0}]
        comparison = compare.report(*self.captures(mutate=extra))
        self.assertEqual([row["declaration_required"] for row in comparison["rows"]], [True, False, False, True])
        self.assertEqual(producers.parity(comparison["rows"])["declaration_parity"], 0.0)
        self.root = self.root / "matching"
        self.root.mkdir()

        def matching(rows):
            rows[3]["emit"]["outputs"]["dts"] = copy.deepcopy(self.fixture[3]["native"]["outputs"]["dts"])
        comparison = compare.report(*self.captures(mutate=matching))
        self.assertEqual(producers.parity(comparison["rows"])["declaration_parity"], 1.0)

    def test_failures_count_against_every_domain(self):
        def refused(rows):
            for name in corpus.EMIT_DOMAINS:
                rows[0][name] = {"state": "failed", "class": "unsupported", "reason": REFUSAL}
            rows[0]["compilations"] = []
        comparison = compare.report(*self.captures(mutate=refused))
        metrics = producers.parity(comparison["rows"])
        self.assertEqual((metrics["output_parity"], metrics["sourcemap_parity"], metrics["emit_diagnostics_parity"]),
                         (0.5, 0.75, 0.75))
        self.assertEqual(comparison["summary"]["unsupported_rows"], 1)


class Modes(Fixture):
    def test_the_two_modes_agree(self):
        single = self.captures("single")
        concurrent = self.captures("concurrent")
        summary = compare.modes(*single, *concurrent, write=False)
        self.assertEqual((summary["rows"], summary["outcome_differences"], summary["rust_observation_differences"]),
                         (4, 0, 0))
        compare.modes(*single, *concurrent)
        self.assertEqual(json.loads((concurrent[1] / "modes.json").read_bytes())["outcome_differences"], 0)

    def test_an_outcome_difference_between_the_modes_is_counted(self):
        def other_reason(rows):
            refused = next(row for row in rows if row["id"].startswith("conformance/jsdoc/importTag11"))
            refused["reprint"]["files"][0]["comments"]["reason"] = "unhandled statement: KindImportDeclaration"
        single = self.captures("single")
        concurrent = self.captures("concurrent", mutate=other_reason)
        summary = compare.modes(*single, *concurrent, write=False)
        self.assertEqual(summary["outcome_differences"], 1)
        self.assertEqual(summary["differences"][0]["domain"], "reprint")
        self.assertEqual((summary["differences"][0]["single"], summary["differences"][0]["concurrent"]),
                         ("match", "different"))

    def test_each_run_compares_with_its_own_mode(self):
        single = self.captures("single")
        concurrent = self.captures("concurrent")
        with self.assertRaisesRegex(ValueError, "single run and its native capture"):
            compare.modes(concurrent[0], single[1], *concurrent, write=False)
        with self.assertRaisesRegex(ValueError, "concurrent run and its native capture"):
            compare.modes(*single, *single, write=False)


class Register(Fixture):
    def test_the_register_names_every_row_that_cannot_pass(self):
        native_dir, rust_dir = self.captures()
        register = blockers.build(native_dir, rust_dir)
        self.assertEqual([(e["kind"], e["cause"], e["owner"], e["variants"], e["domains"]) for e in register["entries"]],
                         [("different", "declaration differs: dts", "T7", 1, {"declaration": 1}),
                          ("different", "output differs: dts", "T8", 1, {"output": 1})])
        self.assertEqual(register["by_owner"], {"T7": 1, "T8": 1})
        comparison = compare.report(native_dir, rust_dir)
        self.assertTrue(blockers.complete(register, comparison))
        self.assertEqual({item["dependency"]: item["state"] for item in register["cross_phase"]},
                         {"bounded work group": "landed", "emit resolver queries the transforms need": "not observed"})

    def test_a_new_bucket_makes_the_register_incomplete(self):
        native_dir, rust_dir = self.captures()
        register = blockers.build(native_dir, rust_dir)

        def different(rows):
            rows[0]["reprint"]["files"][0]["no_comments"]["sha256"] = "f" * 64
        native_dir, rust_dir = self.root / "native-single", self.root / "rust-different"
        pairs = [with_mode(row["request"], row["rust"], "single") for row in self.fixture]
        rows = [p[1] for p in pairs]
        different(rows)
        write_rust(rust_dir, native_dir, [p[0] for p in pairs], rows, "single")
        comparison = compare.report(native_dir, rust_dir)
        self.assertFalse(blockers.complete(register, comparison))
        rebuilt = blockers.build(native_dir, rust_dir, comparison=comparison)
        self.assertTrue(blockers.complete(rebuilt, comparison))
        reprint = [e for e in rebuilt["entries"] if e["domains"] == {"reprint": 1}]
        self.assertEqual([(e["kind"], e["cause"], e["owner"]) for e in reprint],
                         [("different", "reprint differs: text", "T1")])
        rebuilt["entries"][0]["variants_sha256"] = "0" * 64
        self.assertFalse(blockers.complete(rebuilt, comparison))

    def test_owners_follow_the_cause_and_domains(self):
        self.assertEqual(blockers.owner_of("unsupported", "Program.Emit is Phase 3 T8", {"output": 1}), ("T8", False))
        self.assertEqual(blockers.owner_of("different", "declaration differs: dts", {"declaration": 1}),
                         ("T7", False))
        self.assertEqual(blockers.owner_of("unsupported", "unsupported checker operation: GetConstantValue",
                                           {"output": 1}), (blockers.RESOLVER_OWNER, True))
        self.assertEqual(blockers.owner_of("failed", "panic: boom", {"reprint": 1}), ("T1", False))
        self.assertEqual(blockers.owner_of("different", "sourcemap differs", {"sourcemap": 1, "output": 1}),
                         ("T2/T8", False))

    def test_a_resolver_refusal_is_a_cross_phase_entry(self):
        def resolver(rows):
            for name in corpus.EMIT_DOMAINS:
                rows[0][name] = {"state": "failed", "class": "unsupported", "reason": REFUSAL}
            rows[0]["compilations"] = []
        native_dir, rust_dir = self.captures(mutate=resolver)
        register = blockers.build(native_dir, rust_dir)
        entry = next(e for e in register["entries"] if e["cross_phase"])
        self.assertEqual((entry["owner"], entry["variants"], entry["domains"]),
                         (blockers.RESOLVER_OWNER, 1, {"declaration": 1, "emit_diagnostics": 1, "output": 1,
                                                       "sourcemap": 1, "sourcemap_record": 1}))
        self.assertEqual(register["cross_phase"][1]["state"], "observed")
        self.assertTrue(blockers.complete(register, compare.report(native_dir, rust_dir)))


def closure_comparison(mode, rows):
    return {"native": {"observation_sha256": mode + "-native"}, "rust": {"capture_sha256": mode + "-rust"},
            "rows": rows}


class Closure(unittest.TestCase):
    """T8's closure: the residual list and the checkpoints' recorded facts."""

    METRICS = {"reprint_parity": 1, "output_parity": 1, "declaration_parity": 1, "sourcemap_parity": 1,
               "sourcemap_record_parity": 1, "emit_diagnostics_parity": 1, "transpile_parity": 1,
               "harness_valid": True, "harness_valid_concurrent": True, "mode_parity": True,
               "unsupported_required": 0, "residuals": 0, "dispositions": True, "report": True,
               "evidence_current": True}

    @staticmethod
    def audit(**open_counts):
        groups = {f"T{n}": {"counts": {"gap": 0, "pending_c3": 0, "duplicate": 0}} for n in range(1, 9)}
        for checkpoint, kind in open_counts.items():
            groups[checkpoint]["counts"][kind] = 1
        return {"groups": groups}

    RECEIPTS = {f"t{n}-contracts": True for n in range(1, 9)}

    def test_a_residual_is_a_domain_that_does_not_match(self):
        row = {"id": "a", "declaration_required": False,
               "outcomes": {"output": "match", "sourcemap": "disabled", "declaration": "different", "reprint": "match"}}
        self.assertEqual(residuals.residual_domains(row), {})
        row["declaration_required"] = True
        self.assertEqual(residuals.residual_domains(row), {"declaration": "different"})
        row["outcomes"]["output"] = "failed"
        self.assertEqual(residuals.residual_domains(row), {"declaration": "different", "output": "failed"})

    def test_the_list_names_each_row_once_with_both_modes(self):
        good = {"id": "a", "declaration_required": False, "outcomes": {"output": "match"}}
        bad = {"id": "b", "declaration_required": False, "outcomes": {"output": "different"}}
        failed = dict(bad, outcomes={"output": "failed"})
        document = residuals.document({"single": closure_comparison("single", [good, bad]),
                                       "concurrent": closure_comparison("concurrent", [good, failed])})
        self.assertEqual(document["count"], 1)
        self.assertEqual(document["rows"], [{"id": "b", "modes": {"concurrent": {"output": "failed"},
                                                                  "single": {"output": "different"}}}])
        self.assertEqual(document["comparisons"]["single"], {"native": "single-native", "rust": "single-rust"})
        with self.assertRaisesRegex(ValueError, "both modes"):
            residuals.document({"single": closure_comparison("single", [good])})

    def test_every_checkpoint_closes_on_complete_evidence(self):
        done = producers.checkpoint_metrics(self.METRICS, self.audit(), self.RECEIPTS)
        self.assertEqual(done, {f"t{n}_complete": True for n in range(1, 9)})

    def test_an_open_function_or_stale_receipt_holds_only_its_checkpoint(self):
        done = producers.checkpoint_metrics(self.METRICS, self.audit(T3="gap", T6="pending_c3"),
                                            dict(self.RECEIPTS, **{"t5-contracts": False, "t2-contracts": None}))
        self.assertEqual({name for name, value in done.items() if not value},
                         {"t2_complete", "t3_complete", "t5_complete", "t6_complete"})
        self.assertEqual(producers.checkpoint_metrics(self.METRICS, None, self.RECEIPTS),
                         {f"t{n}_complete": False for n in range(1, 9)})

    def test_each_checkpoint_needs_its_own_parity_and_t8_the_p3b_exit(self):
        cases = {"reprint_parity": {"t1_complete", "t8_complete"},
                 "output_parity": {"t4_complete", "t5_complete", "t6_complete", "t8_complete"},
                 "declaration_parity": {"t7_complete", "t8_complete"},
                 "transpile_parity": {"t8_complete"}, "sourcemap_parity": {"t8_complete"}}
        for name, failing in cases.items():
            with self.subTest(metric=name):
                done = producers.checkpoint_metrics(dict(self.METRICS, **{name: 0.99}), self.audit(), self.RECEIPTS)
                self.assertEqual({key for key, value in done.items() if not value}, failing)
        for name, value in (("residuals", 1), ("mode_parity", False), ("dispositions", False), ("report", False),
                            ("evidence_current", False), ("unsupported_required", 2), ("harness_valid_concurrent", False)):
            with self.subTest(metric=name):
                done = producers.checkpoint_metrics(dict(self.METRICS, **{name: value}), self.audit(), self.RECEIPTS)
                self.assertEqual({key for key, value in done.items() if not value}, {"t8_complete"})


class Report(unittest.TestCase):
    """T8's record: the per-area dashboard and its staleness."""

    @staticmethod
    def entry(identity, suite, sourcemap="absent", **settings):
        return {"id": identity, "suite": suite, "sourcemap": sourcemap, "settings": settings}

    def test_rows_group_by_area_and_folded_settings(self):
        conformance = self.entry("conformance/es6/classes/a.ts#configuration=0", "conformance", Target="ES6",
                                 module="CommonJS", declaration="true")
        self.assertEqual(report.groups(conformance),
                         {"suite": "conformance", "area": "es6", "target": "es2015", "module": "commonjs",
                          "jsx": "unset", "declaration": "true", "sourcemap": "absent"})
        compiler = self.entry("compiler/b.ts#configuration=1", "compiler", "reference", jsx="Preserve")
        self.assertEqual({key: report.groups(compiler)[key] for key in ("area", "target", "jsx", "sourcemap")},
                         {"area": "(compiler)", "target": "unset", "jsx": "preserve", "sourcemap": "reference"})

    def test_a_row_counts_as_matched_only_without_a_residual_domain(self):
        inventory = {"a": self.entry("a", "compiler", target="es5"), "b": self.entry("b", "compiler", target="es5"),
                     "c": self.entry("c", "compiler", target="es2015")}
        rows = [{"id": "a", "declaration_required": False, "outcomes": {"output": "match", "declaration": "different"}},
                {"id": "b", "declaration_required": True, "outcomes": {"output": "match", "declaration": "different"}},
                {"id": "c", "declaration_required": False, "outcomes": {"output": "disabled"}}]
        summary = {"rows": 3, "all_domains_met": 2, "declaration_required": 1, "domains": {}}
        board = report.dashboard({"rows": rows, "summary": summary}, inventory)
        self.assertEqual(board["rates"]["target"], {"es2015": {"rows": 1, "matched": 1},
                                                    "es5": {"rows": 2, "matched": 1}})
        self.assertEqual(board["rates"]["suite"], {"compiler": {"rows": 3, "matched": 2}})

    def test_the_committed_record_and_page_must_equal_the_rebuild(self):
        record = {"captured": 1}
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(report, "RECORD", Path(directory) / "report.json"), \
                mock.patch.object(report, "PAGE", Path(directory) / "report.md"), \
                mock.patch.object(report, "build_record", return_value=record), \
                mock.patch.object(report, "render", side_effect=lambda value: json.dumps(value) + "\n"):
            self.assertFalse(report.current({}, {}))
            report.RECORD.write_text(report.text(record))
            report.PAGE.write_text(json.dumps(record) + "\n")
            self.assertTrue(report.current({}, {}))
            report.PAGE.write_text(json.dumps(record) + "\nedited\n")
            self.assertFalse(report.current({}, {}))

    def test_the_record_needs_both_modes_and_a_transpile_run(self):
        with self.assertRaisesRegex(ValueError, "both modes"):
            report.build_record({"single": {}}, {"matched": 1})
        with self.assertRaisesRegex(ValueError, "transpile"):
            report.build_record({"single": {}, "concurrent": {}}, None)


if __name__ == "__main__":
    unittest.main()
