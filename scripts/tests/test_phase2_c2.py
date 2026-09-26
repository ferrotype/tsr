"""C2 exits require current domain-bound ownership and independent evidence."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_blockers as blockers
import phase2_audit as audit
import phase2_compare as compare
import phase2_corpus as corpus
import phase2_producers as producers
import s08_checkerbench as benchmark
import phase2_order_trace as order_trace
from test_phase2_c1 import row, comparison
from phase2_fixtures import build_capture, CONTROL


class MetricsFixture:
    def setUp(self):
        self.inventory = [{"id": "owned", "checkpoint": "C2"}, {"id": "prior", "checkpoint": "C2"},
                          {"id": "other", "checkpoint": "C3"}]
        self.addCleanup(patch.stopall)
        patch.object(compare.phase2_inventory, "executed", return_value=self.inventory).start()
        self.baseline = comparison([row("owned", errors="different"), row("prior"), row("other", checkpoint="C3")])
        self.current = comparison([row("owned"), row("prior"), row("other", checkpoint="C3")])
        self.claims = {"version": 1, "pin": self.baseline["pin"],
                       "inventory_sha256": self.baseline["inventory_sha256"],
                       "rust_capture_sha256": self.baseline["rust_capture_sha256"],
                       "baseline_sha256": "f" * 64, "rows": [{"id": "owned", "status": "open"}]}
        self.prerequisites = {name: True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                                     "result_recorded", "blockers_named")}

    def metrics(self, **kwargs):
        arguments = dict(checkpoint="C2", comparison=self.current, claims=self.claims, audit_ok=True,
                         baseline=self.baseline, contracts_ok=True, regression_parity=1,
                         blockers={"version": 2, "entries": []}, measured_ok=True,
                         baseline_sha256="f" * 64, prerequisites=self.prerequisites, inventory=self.inventory)
        return producers.checkpoint_metrics(**(arguments | kwargs))


class ExitMetrics(MetricsFixture, unittest.TestCase):
    def test_current_outcomes_override_open_and_closed_labels(self):
        self.assertTrue(self.metrics()["c2_complete"])
        self.current = comparison([row("owned", errors="different"), row("prior"), row("other", checkpoint="C3")])
        self.assertEqual(self.metrics()["c2_open"], 1)
        self.assertFalse(self.metrics()["c2_complete"])
        self.claims["rows"][0].update(status="closed", commit="abcdef1234")
        self.assertEqual(self.metrics()["c2_open"], 1)

    def test_every_evidence_prerequisite_and_authority_is_required(self):
        for key in self.prerequisites:
            with self.subTest(key=key):
                self.assertFalse(self.metrics(prerequisites=self.prerequisites | {key: False})["c2_complete"])
        for key in ("claims", "audit_ok", "baseline", "contracts_ok", "measured_ok", "blockers"):
            with self.subTest(key=key):
                self.assertFalse(self.metrics(**{key: None})["c2_complete"])

    def test_unknown_duplicate_missing_claims_and_bad_bindings_reject(self):
        for mutate in (lambda c: c["rows"].clear(), lambda c: c["rows"].append(c["rows"][0]),
                       lambda c: c["rows"][0].update(id="unknown"),
                       lambda c: c["rows"][0].update(status="candidate"),
                       lambda c: c["rows"][0].update(status="closed"),
                       lambda c: c.update(pin="wrong"), lambda c: c.update(baseline_sha256="e" * 64),
                       lambda c: c.update(inventory_sha256="bad"), lambda c: c.update(rust_capture_sha256="bad")):
            claims = copy.deepcopy(self.claims)
            mutate(claims)
            with self.assertRaises(ValueError):
                self.metrics(claims=claims)

    def test_global_regression_and_unattributed_failure_cannot_escape(self):
        self.current = comparison([row("owned"), row("prior", display="different"), row("other", checkpoint="C3")])
        self.assertEqual(self.metrics()["c2_regressions"], 1)
        self.assertEqual(self.metrics()["c2_open"], 1)
        self.current = comparison([row("owned"), row("prior"), row("other", checkpoint="C3", errors="failed")])
        self.current["rows"][2]["bucket"] = "failed: walker_error"
        self.assertEqual(self.metrics()["c2_failures"], 1)
        self.assertFalse(self.metrics()["c2_complete"])

    def test_unresolved_c2_share_of_mixed_owner_blocker_counts(self):
        register = {"entries": [{"ownership": [{"effective_owner": "C5"}, {"effective_owner": "C2"}]}]}
        self.assertEqual(self.metrics(blockers=register)["c2_blockers_open"], 1)
        self.assertFalse(self.metrics(blockers=register)["c2_complete"])


class Handoffs(MetricsFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "trace.json").write_text('{"cause":"checked native call stack"}\n')
        self.current = comparison([row("owned", errors="different"), row("prior"), row("other", checkpoint="C3")],
                                  rust_capture_sha256="a" * 64)
        self.context = {"owned": {"request_sha256": "b" * 64, "raw_observation_sha256": "c" * 64}}
        self.handoff = {"owner": "C5", "go": "tsc/internal/checker/checker.go:Checker.externalCause",
                        "reproduce": ["python3", "scripts/phase2_corpus.py", "run", "--case", "owned"],
                        "capture_sha256": "a" * 64, **self.context["owned"], "domains": ["errors"],
                        "cause": "trace names declaration accessibility", "trace": {
                            "path": "trace.json", "sha256": producers.digest((self.root / "trace.json").read_bytes())}}
        self.claims["rows"][0].update(status="handed", handoff=self.handoff)

    def validated(self):
        return blockers.validated_handoffs("C2", self.claims, self.current, self.context,
                                          root=self.root, known={self.handoff["go"]})

    def test_current_handoff_is_counted_and_exempts_only_covered_row(self):
        metrics = self.metrics(handoffs=self.validated())
        self.assertEqual((metrics["c2_open"], metrics["c2_handoffs"]), (0, 1))
        self.assertTrue(metrics["c2_complete"])

    def test_hand_label_without_trace_is_rejected(self):
        self.claims["rows"][0].pop("handoff")
        with self.assertRaisesRegex(ValueError, "trace-bound"):
            self.validated()

    def test_stale_capture_request_raw_failure_site_or_trace_reopens(self):
        for key in ("capture_sha256", "request_sha256", "raw_observation_sha256"):
            original = self.handoff[key]
            self.handoff[key] = "d" * 64
            self.assertEqual(self.validated(), {})
            self.assertEqual(self.metrics(handoffs=self.validated())["c2_open"], 1)
            self.handoff[key] = original
        (self.root / "trace.json").write_text("a different trace")
        self.assertEqual(self.validated(), {})
        raw = {"fatal": {"class": "panic"}, "panic_location": "crates/tsr_checker/src/a.rs:1"}
        before = producers.digest(producers.canonical(raw))
        raw["panic_location"] = "crates/tsr_checker/src/b.rs:2"
        self.assertNotEqual(before, producers.digest(producers.canonical(raw)))

    def test_errors_includes_declaration_but_cannot_hide_types(self):
        self.current["rows"][0]["outcomes"]["types"] = "different"
        self.assertEqual(self.validated(), {})
        self.handoff["domains"] = ["declaration"]
        with self.assertRaisesRegex(ValueError, "valid domains"):
            self.validated()

    def test_unknown_function_own_function_owner_and_wrong_repro_reject(self):
        for mutation in ({"owner": "C2"}, {"owner": "elsewhere"}, {"reproduce": ["tool", "--case", "other"]},
                         {"domains": ["errors", "errors"]}, {"trace": {"path": "../trace", "sha256": "b" * 64}}):
            saved = copy.deepcopy(self.handoff)
            self.handoff.update(mutation)
            with self.assertRaises(ValueError):
                self.validated()
            self.handoff.clear(); self.handoff.update(saved)
        with self.assertRaises(ValueError):
            blockers.validated_handoffs("C2", self.claims, self.current, self.context,
                                       root=self.root, known=set())
        with self.assertRaises(ValueError):
            blockers.validated_handoffs("C2", self.claims, self.current, self.context,
                                       root=self.root, known={self.handoff["go"]}, owned_functions={self.handoff["go"]})

    def test_blocker_transfer_is_exact_cause_variant_and_domain(self):
        self.current["rows"][0]["outcomes"]["errors"] = "unsupported"
        self.current["rows"][0]["details"] = {"errors": {"operation": "missing display"}}
        self.handoff["blocker"] = {"kind": "unsupported", "operation": "missing display"}
        transferred = self.validated()
        first = blockers.effective_ownership("unsupported", "missing display", "owned", ["errors"], "C2", transferred)
        second = blockers.effective_ownership("unsupported", "missing display", "prior", ["errors"], "C2", transferred)
        self.assertEqual(blockers.owner_of("missing display", [], first + second), "C2/C5")
        self.assertEqual(first[0]["inventory_owner"], "C2")
        self.assertEqual(first[0]["effective_owner"], "C5")
        self.handoff["blocker"]["operation"] = "different cause"
        self.assertEqual(self.validated(), {})

    def test_registered_blocker_required_and_b_number_irrelevant(self):
        self.current["rows"][0]["outcomes"]["errors"] = "unsupported"
        self.current["rows"][0]["details"] = {"errors": {"operation": "missing display"}}
        self.claims["rows"][0]["status"] = "blocked"
        self.handoff["blocker"] = {"kind": "unsupported", "operation": "missing display"}
        transfers = self.validated()
        self.assertEqual(self.metrics(handoffs=transfers)["c2_open"], 1)
        register = {"entries": [{"id": "B77", "kind": "unsupported", "operation": "missing display",
                                 "variants": ["owned"], "domains": {"errors": 1},
                                 "ownership": [{"variant": "owned", "domain": "errors", "effective_owner": "C5"}]}]}
        self.assertEqual(self.metrics(handoffs=transfers, blockers=register)["c2_open"], 0)
        register["entries"][0]["domains"]["types"] = 1
        register["entries"][0]["ownership"].append({"variant": "prior", "domain": "types", "effective_owner": "C5"})
        self.assertIsNone(blockers.covering_blocker(register, "owned", {
            "kind": "unsupported", "operation": "missing display", "domains": ["types"]}))
        identity = blockers.stable_blocker({"blocker": "B09", "bucket": "emit order: native pre/post-emit sets differ"})
        emit = {"entries": [{"id": "B42", "kind": "emit_order", "variants": ["owned"], "domains": {"errors": 1}}]}
        self.assertIsNotNone(blockers.covering_blocker(emit, "owned", identity))

    def test_failed_row_in_another_crate_requires_current_trace(self):
        self.current["rows"][0]["outcomes"]["errors"] = "failed"
        self.current["rows"][0]["bucket"] = "panic: tsr_ast::symbols"
        self.assertEqual(self.metrics()["c2_failures"], 1)
        self.assertEqual(self.metrics(handoffs=self.validated())["c2_failures"], 0)
        self.handoff["raw_observation_sha256"] = "d" * 64
        self.assertEqual(self.metrics(handoffs=self.validated())["c2_failures"], 1)

    def test_incoming_handoff_must_bind_the_current_site_and_owned_function(self):
        self.current["rows"][2]["outcomes"]["display"] = "different"
        self.current["all_domains_match"] -= 1
        self.claims["rows"][0] = {"id": "owned", "status": "open"}
        received = copy.deepcopy(self.handoff)
        received.update(owner="C2", **{"from": "C3"}, domains=["display"])
        received["reproduce"][-1] = "other"
        self.claims["rows"].append({"id": "other", "status": "open", "incoming": received})
        self.context["other"] = self.context["owned"]
        def validate():
            return blockers.validated_handoffs("C2", self.claims, self.current, self.context, root=self.root,
                known={received["go"]}, owned_functions={received["go"]}, incoming=True, inventory=self.inventory)
        incoming = validate()
        self.assertEqual(self.metrics(incoming=incoming)["c2_open"], 2)
        self.assertEqual(blockers.effective_ownership("failed", "panic", "other", ["display"], "C3", {}, incoming)[0]
                         ["effective_owner"], "C2")
        received["raw_observation_sha256"] = "d" * 64
        self.assertEqual(validate(), {})
        with self.assertRaisesRegex(ValueError, "incoming handoff"):
            self.metrics(incoming=validate())


class BlockerCoverage(unittest.TestCase):
    def test_capture_context_authenticates_and_hashes_full_panic_observation(self):
        request = {"id": "owned"}
        raw = {"fatal": {"class": "panic"}, "panic_location": "crates/tsr_ast/src/a.rs:1"}
        replayed = {"summary": {"partial": False, "harness_errors": 0}, "source_stable": True,
                    "capture_sha256": "a" * 64}
        comparison_ = {"rust_capture_sha256": "a" * 64}
        with patch.object(compare, "load_rust", return_value=(replayed, [request], [raw], None, None)):
            first = blockers.capture_context(Path("fixture"), comparison_)
            raw["panic_location"] = "crates/tsr_ast/src/b.rs:2"
            second = blockers.capture_context(Path("fixture"), comparison_)
            self.assertNotEqual(first["owned"]["raw_observation_sha256"], second["owned"]["raw_observation_sha256"])
            self.assertEqual(first["owned"]["request_sha256"], second["owned"]["request_sha256"])
            replayed["source_stable"] = False
            with self.assertRaisesRegex(ValueError, "current authenticated"):
                blockers.capture_context(Path("fixture"), comparison_)

    def test_kind_operation_variant_and_domain_are_all_required(self):
        for kind, field in (("unsupported", "operation"), ("failed", "reason")):
            observed = row("owned", errors=kind)
            observed["details"] = {"errors": {field: "cause"}}
            entry = {"kind": kind, "operation": "cause", "evidence": ["raw"],
                     "ownership": [{"variant": "owned", "domain": "errors"}]}
            self.assertTrue(blockers.complete({"entries": [entry]}, comparison([observed])))
            for mutation in ({"variant": "other"}, {"domain": "types"}):
                changed = copy.deepcopy(entry); changed["ownership"][0].update(mutation)
                self.assertFalse(blockers.complete({"entries": [changed]}, comparison([observed])))
            changed = dict(entry, operation="other")
            self.assertFalse(blockers.complete({"entries": [changed]}, comparison([observed])))


class Measurement(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name); self.bench = self.root / "bench"; self.rust = self.root / "rust"
        self.bench.mkdir(); self.rust.mkdir()
        self.required = {"crates/checker/src/lib.rs": "source", "Cargo.lock": "lock"}
        self.build = {"sources": self.required.copy(), "binaries": {"rust-normal": {"sha256": "benchmark-binary"}}}
        self.capture = {"pin": "pin", "build_sha256": self.write(self.bench / "build.json", self.build)}
        self.write(self.bench / "capture.json", self.capture)
        self.corpus = {"build": {"sources": self.required.copy(), "binary_sha256": "different-corpus-binary"}}
        self.write(self.rust / "capture.json", self.corpus)
        self.comparison = {"pin": "pin", "rust_capture_sha256": "a" * 64}
        self.report = {"smoke": None, "source_stable": True, "pin": "pin",
                       "metrics": {"elapsed_ratio": 1.2, "retained_bytes_ratio": 0.8, "type_footprint_ratio": 0.9}}
        self.addCleanup(patch.stopall)
        self.replayed = patch.object(benchmark, "report", return_value=self.report).start()
        patch.object(producers, "source_inputs", return_value=self.required).start()
        patch.object(corpus, "replay", return_value={"summary": {"partial": False, "harness_errors": 0},
                    "source_stable": True, "capture_sha256": "a" * 64}).start()

    def write(self, path, value):
        raw = producers.canonical(value); path.write_bytes(raw); return producers.digest(raw)

    def identity(self):
        return producers.measurement_identity(self.bench, self.comparison, self.rust)

    def test_independent_binary_builds_join_by_complete_sources(self):
        identity = self.identity()
        self.assertEqual(identity["production_sources_sha256"], producers.digest(producers.canonical(self.required)))
        self.replayed.assert_called_once_with(self.bench)
        receipt = self.root / "measurement.json"; self.write(receipt, identity)
        self.assertTrue(producers.measurement_current(self.comparison, self.rust, receipt))
        identity["capture_sha256"] = "changed"; self.write(receipt, identity)
        self.assertFalse(producers.measurement_current(self.comparison, self.rust, receipt))

    def test_missing_stale_and_smoke_measurements_do_not_pass(self):
        self.assertIsNone(producers.measurement_current(self.comparison, self.rust, self.root / "missing"))
        for field, value in (("smoke", 1), ("pin", "wrong"), ("source_stable", False)):
            old = self.report[field]; self.report[field] = value
            with self.assertRaises(ValueError): self.identity()
            self.report[field] = old
        self.report["metrics"].pop("type_footprint_ratio")
        with self.assertRaises(ValueError): self.identity()

    def test_each_build_needs_every_shared_input_and_its_current_hash(self):
        for path in self.required:
            for current in (None, "changed"):
                altered = copy.deepcopy(self.corpus)
                if current is None: altered["build"]["sources"].pop(path)
                else: altered["build"]["sources"][path] = current
                self.write(self.rust / "capture.json", altered)
                with self.assertRaisesRegex(ValueError, "corpus capture lacks"): self.identity()
                self.write(self.rust / "capture.json", self.corpus)
                altered = copy.deepcopy(self.build)
                if current is None: altered["sources"].pop(path)
                else: altered["sources"][path] = current
                self.capture["build_sha256"] = self.write(self.bench / "build.json", altered)
                self.write(self.bench / "capture.json", self.capture)
                with self.assertRaisesRegex(ValueError, "measurement capture lacks"): self.identity()
                self.capture["build_sha256"] = self.write(self.bench / "build.json", self.build)
                self.write(self.bench / "capture.json", self.capture)


class Registration(unittest.TestCase):
    def test_c2_authorities_and_complete_closures_invalidate_tracker(self):
        spec = tomllib.loads((ROOT / "status/runs.toml").read_text())["checker"]
        for path in producers.CHECKPOINT_AUTHORITIES["C2"].values():
            self.assertIn(str(path.relative_to(ROOT)), spec["inputs"])
        self.assertIn("data/phase2/receipts/c2-contracts.json", spec["inputs"])
        command = ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--"]
        command.extend(":(top,glob)" + pattern for pattern in spec["sources"])
        registered = set(subprocess.check_output(command, cwd=ROOT).decode().split("\0")) | set(spec["inputs"])
        required = producers.source_inputs(producers.WITNESSES["c2-contracts"]["sources"])
        self.assertEqual(set(required) - registered, set())
        self.assertEqual(set(benchmark.sources()) - registered, set())
        bench = tomllib.loads((ROOT / "status/runs.toml").read_text())["checkerbench"]
        command = command[:7] + [":(top,glob)" + pattern for pattern in bench["sources"]]
        registered = set(subprocess.check_output(command, cwd=ROOT).decode().split("\0")) | set(bench["inputs"])
        self.assertEqual(set(benchmark.sources()) - registered, set())

    def test_capture_closures_cover_assets_workspace_and_generators(self):
        required = producers.source_inputs(producers.PRODUCTION_PATTERNS)
        for actual in (corpus.sources(), benchmark.sources()):
            self.assertEqual({path: actual.get(path) for path in required}, required)
        for path in ("tools/s10/corpus-adapter/Cargo.toml", "crates/tsr_bundled/bundled/libs/lib.d.ts",
                     "xtask/src/gen/diagnostics.rs", "scripts/generate_locale_tables.py"):
            self.assertIn(path, required)

    def test_c2_witness_has_exact_features_profiles_and_minimum_inventory(self):
        spec = producers.WITNESSES["c2-contracts"]
        self.assertEqual(spec["minimum_tests"], 9)
        self.assertEqual(spec["commands"][0], ["cargo", "test", "-p", "tsr_compiler", "--features",
            "recursion-probe,creation-trace", "--test", "c2_contracts", "--locked"])
        self.assertEqual(spec["commands"][1], spec["commands"][0] + ["--release"])
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "tests.rs"
            source.write_text("#[test]\nfn unrelated() {}\n")
            with patch.object(producers, "ROOT", Path(directory)):
                with self.assertRaises(ValueError):
                    producers.witness_tests(dict(spec, test_source="tests.rs"))

    def test_c2_contract_modules_have_exact_paths_and_qualified_results(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "tests.rs"
            module = root / "contracts.rs"
            source.write_text('#[test]\nfn direct() {}\n#[path = "contracts.rs"]\nmod contracts;\n')
            module.write_text("#[test]\nfn native() {}\n")
            spec = {"test_source": "tests.rs", "minimum_tests": 2,
                    "test_modules": {"contracts": "contracts.rs"}}
            with patch.object(producers, "ROOT", root):
                expected = producers.witness_tests(spec)
                self.assertEqual(expected, ["contracts::native", "direct"])
                output = "test contracts::native ... ok\ntest direct ... ok\n" + (
                    "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n")
                self.assertTrue(producers.successful_test_output(output, expected))
                self.assertFalse(producers.successful_test_output(output.replace("contracts::", ""), expected))
                for old, new in (("contracts.rs", "other.rs"), ("mod contracts;", "mod changed;")):
                    original = source.read_text()
                    source.write_text(original.replace(old, new))
                    with self.assertRaisesRegex(ValueError, "module binding"):
                        producers.witness_tests(spec)
                    source.write_text(original)
                source.write_text(source.read_text() + '#[path = "contracts.rs"]\nmod contracts;\n')
                with self.assertRaisesRegex(ValueError, "module binding"):
                    producers.witness_tests(spec)

    def test_order_inputs_stale_c2_receipt_without_entering_measurement_closure(self):
        spec = producers.WITNESSES["c2-contracts"]
        inputs = producers.source_inputs(spec["sources"])
        diagnostic = {**order_trace.native_inputs(), "data/phase2/c2-order-traces.json":
                      producers.digest((ROOT / "data/phase2/c2-order-traces.json").read_bytes())}
        self.assertEqual({path: inputs.get(path) for path in diagnostic}, diagnostic)
        production = producers.source_inputs(producers.PRODUCTION_PATTERNS)
        self.assertFalse(set(diagnostic) & set(production))
        tests = producers.witness_tests(spec)
        self.assertIn("order_contract::source_creation_order_matches_native_without_changing_ordinary_outputs", tests)
        self.assertIn("variance_limits::all_seven_variance_flags_are_measured", tests)
        self.assertIn("cross_product_limits::template_limits_match_native_limits_and_recovery", tests)
        stdout = "".join(f"test {name} ... ok\n" for name in tests) + (
            f"test result: ok. {len(tests)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n")
        record = {"version": 2, "state": "observed", "witness": "c2-contracts", "tests": tests,
                  "source_inputs": inputs, "runs": [
                      {"command": command, "exit_code": 0, "stdout": stdout,
                       "stdout_sha256": producers.digest(stdout.encode())} for command in spec["commands"]]}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.json"
            path.write_bytes(producers.canonical(record))
            with patch.object(producers, "source_inputs", return_value=inputs):
                self.assertTrue(producers.receipt_current("c2-contracts", path))
            for changed in diagnostic:
                with self.subTest(changed=changed), patch.object(
                        producers, "source_inputs", return_value=inputs | {changed: "modified"}):
                    self.assertFalse(producers.receipt_current("c2-contracts", path))

    def test_capture_missing_new_source_hash_is_stale_without_upgrading_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            metadata = build_capture(path, [CONTROL], selection=corpus.selection())
            before = (path / "capture.json").read_bytes()
            with patch.object(corpus, "sources", return_value=metadata["build"]["sources"]):
                self.assertTrue(corpus.replay(path)["source_stable"])
            for added in ("tools/s10/corpus-adapter/Cargo.toml", "xtask/src/gen/diagnostics.rs",
                          "scripts/generate_locale_tables.py"):
                with patch.object(corpus, "sources", return_value=metadata["build"]["sources"] | {added: "new"}):
                    self.assertFalse(corpus.replay(path)["source_stable"])
                self.assertEqual((path / "capture.json").read_bytes(), before)


class AuditScope(unittest.TestCase):
    def setUp(self):
        self.document = audit.load(ROOT / "data/phase2/c2-audit.json")

    def test_c2_scope_is_exact_and_open_reviews_cannot_complete(self):
        self.assertEqual(sum(map(len, self.document["groups"].values())), 231)
        self.assertEqual(audit.problems(self.document, allow_open=True), [])
        changed = copy.deepcopy(self.document)
        identity = next(identity for identity, entry in changed["dispositions"].items()
                        if entry["disposition"] == "equivalent")
        changed["dispositions"][identity] = {
            "disposition": "gap", "item": "C2.3", "reason": "Source review remains unresolved."}
        self.assertEqual(audit.problems(changed, allow_open=True), [])
        self.assertFalse(audit.complete(changed))
        known = audit.inventory()
        for group, prefix in audit.C2_COMPLETE_FILES.items():
            self.assertEqual(set(self.document["groups"][group]), {i for i in known if i.startswith(prefix)})

    def test_discriminator_cannot_swap_or_remove_reviewed_scope(self):
        for key, value in (("checkpoint", "C1"), ("checkpoint", "C3"), ("groups", {}), ("pin", "other")):
            document = dict(self.document, **{key: value})
            self.assertTrue(audit.problems(document, allow_open=True))
        for group in self.document["groups"]:
            for remove_group in (True, False):
                document = copy.deepcopy(self.document)
                if remove_group: document["groups"].pop(group)
                else: document["groups"][group].pop()
                self.assertTrue(any("reviewed" in p for p in audit.problems(document, allow_open=True)))
        c1 = audit.load()
        self.assertEqual(audit.problems(c1, allow_open=True), [])
        self.assertTrue(audit.problems(dict(c1, checkpoint="C2"), allow_open=True))

    def test_ownership_binds_existing_core_operators_without_disposing_gaps(self):
        core = "tsc/internal/checker/checker.go:Checker.getConditionalType"
        self.assertIn(core, blockers.audit_owned_functions(self.document, "C2"))
        self.assertNotIn(core, {i for members in self.document["groups"].values() for i in members})
        changed = copy.deepcopy(self.document)
        changed["ownership"]["functions"].remove(core)
        self.assertTrue(any("ownership" in p for p in audit.problems(changed, allow_open=True)))
        changed = copy.deepcopy(self.document)
        changed["ownership"]["modules"].pop()
        self.assertTrue(any("ownership" in p for p in audit.problems(changed, allow_open=True)))
        self.assertFalse(audit.complete(changed))

    def test_parenthetical_and_historical_contextual_expansions_are_fixed(self):
        groups = self.document["groups"]
        all_ids = {i for members in groups.values() for i in members}
        prefix = "tsc/internal/checker/checker.go:Checker."
        for a, b in (("getTypeArgumentsFromNode", "getTypeArgumentsFromNodes"),
                     ("getIndexTypeOfType", "getIndexTypeOfTypeEx"),
                     ("getApplicableIndexInfo", "getApplicableIndexInfos"),
                     ("getReturnTypeOfSingleNonGenericSignature", "getReturnTypeOfSingleNonGenericSignatureOfCallChain")):
            self.assertTrue({prefix + a, prefix + b} <= all_ids)
        contextual = {i for i in groups["C2.9 contextual typing"] if i.startswith(prefix + "getContextualTypeFor")}
        self.assertEqual(len(contextual), 11)
        self.assertNotIn(prefix + "getContextualTypeForElementExpression", contextual)
        self.assertIn("tsc/internal/checker/checker.go:isGenericTupleType", all_ids)

    def test_review_issues_gate_marked_functions_and_validate_inventory(self):
        identity = "tsc/internal/checker/inference.go:Checker.inferFromTypes"
        issue = {"id": "reviewed-branch", "go": identity, "item": "C2.4",
                 "reason": "A marked function still has a source-reviewed semantic gap."}
        document = copy.deepcopy(self.document)
        document["open_issues"] = [issue]
        self.assertEqual(audit.problems(document, allow_open=True), [])
        self.assertTrue(any("reviewed-branch" in p for p in audit.problems(document)))
        for mutate in (lambda d: d.update(open_issues={}),
                       lambda d: d["open_issues"].append(copy.deepcopy(issue)),
                       lambda d: d["open_issues"][0].update(go="unknown"),
                       lambda d: d["open_issues"][0].update(item="C1.4"),
                       lambda d: d["open_issues"][0].update(reason="")):
            changed = copy.deepcopy(document)
            mutate(changed)
            self.assertTrue(audit.problems(changed, allow_open=True))


if __name__ == "__main__":
    unittest.main()
