"""F5a integration cannot turn fixture existence into executed parity."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import phase1_generation as generation
import phase1_integration as integration


class IntegrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = integration.load(ROOT, integration.MANIFEST)

    def test_manifest_is_prepared_without_claiming_measurement(self):
        result = integration.check()
        self.assertEqual(result["problems"], [])
        self.assertTrue(result["prepared"])
        self.assertFalse(result["complete"])
        self.assertTrue(all(row["state"] == "pending" for row in result["witnesses"]))
        self.assertEqual(result["transport"]["cases"], 89)
        self.assertEqual(len(result["transport"]["mapper_recordings"]), 5)
        self.assertEqual(result["transport"]["state"], "pending")

    def test_removed_required_witness_is_not_prepared(self):
        document = copy.deepcopy(self.document)
        document["witnesses"].pop()
        self.assertIn("integration witness inventory missing, extra or duplicated", integration.check(document=document)["problems"])

    def test_duplicate_witness_is_rejected(self):
        document = copy.deepcopy(self.document)
        document["witnesses"].append(document["witnesses"][0])
        self.assertFalse(integration.check(document=document)["prepared"])

    def test_unknown_witness_kind_is_not_prepared(self):
        document = copy.deepcopy(self.document)
        document["witnesses"][0]["kind"] = "not-really-a-driver"
        self.assertTrue(any("unknown integration witness kind" in p for p in integration.check(document=document)["problems"]))

    def test_unknown_case_cannot_be_a_witness(self):
        document = copy.deepcopy(self.document)
        document["witnesses"][1]["references"][0] = "config/invented"
        self.assertTrue(any("unknown case" in p for p in integration.check(document=document)["problems"]))

    def test_missing_or_renamed_named_test_is_rejected(self):
        document = copy.deepcopy(self.document)
        document["witnesses"][3]["test"] = "invented"
        self.assertTrue(any("missing named test" in p for p in integration.check(document=document)["problems"]))

    def test_s11_case_removal_and_duplicate_do_not_shrink_contract(self):
        for duplicate in (False, True):
            document = copy.deepcopy(self.document)
            document["transport"]["cases"].pop(0)
            if duplicate:
                document["transport"]["cases"].append(document["transport"]["cases"][0])
            self.assertTrue(any("all 89" in p for p in integration.check(document=document)["problems"]))

    def test_mapper_and_wire_boundaries_are_not_omitted(self):
        document = copy.deepcopy(self.document)
        document["transport"]["mapper_recordings"].pop()
        document["boundaries"].remove("strict-Unicode-filesystem-wire")
        problems = integration.check(document=document)["problems"]
        self.assertTrue(any("five actual" in p for p in problems))
        self.assertTrue(any("boundary inventory" in p for p in problems))

    def test_native_authority_is_not_invented_for_contract_case(self):
        document = copy.deepcopy(self.document)
        document["transport"]["cases"][-1]["authority"] = "pinned-Go-stream"
        self.assertTrue(any("wrong native/contract authority" in p for p in integration.check(document=document)["problems"]))

    def test_deleted_locale_output_remains_a_named_gap(self):
        original = integration.load
        def load(root, path):
            result = original(root, path)
            if str(path) == "data/phase1/locale-tables-manifest.json":
                result["outputs"].pop("crates/tsr_locale/src/tables_generated.rs")
            return result
        with patch.object(integration, "load", side_effect=load):
            self.assertIn("locale generation output inventory incomplete", integration.check()["problems"])

    def test_changed_generated_bytes_are_not_accepted_by_presence(self):
        original = integration.sha
        def changed(path):
            return "0" * 64 if path.name == "locales_generated.rs" else original(path)
        with patch.object(integration, "sha", side_effect=changed):
            self.assertTrue(any("generated output differs" in p for p in integration.check()["problems"]))

    def test_static_closure_has_no_nonexistent_paths_or_documentation(self):
        paths = integration.input_paths()
        self.assertEqual([p for p in paths if not (ROOT / p).is_file()], [])
        self.assertNotIn("docs/PHASE1-progress.md", paths)
        self.assertIn("crates/tsr_compiler/examples/phase1_integration.rs", paths)
        self.assertIn("tools/s11/mapper_test.go", paths)
        self.assertIn("tools/phase1/locale/internal_export_test.go", paths)
        self.assertIn("tools/s10/toolchains.json", paths)
        self.assertIn("LICENSE", paths)
        for package in integration.load(ROOT, "tools/packaging/packages.json")["packages"]:
            if package["publish"]:
                directory = Path(package["manifest"]).parent
                for name in ("Cargo.toml", "README.md", "LICENSE", "NOTICE"):
                    self.assertIn(str(directory / name), paths)

    def test_public_package_manifest_change_stales_receipt_inputs(self):
        import phase1_producers as producers
        manifest = ROOT / "crates/tsr_wasm/Cargo.toml"
        before = producers.source_closure("foundations")
        original = Path.read_bytes

        def changed(path):
            data = original(path)
            return data + b"\n# changed archive manifest\n" if path == manifest else data

        with patch.object(Path, "read_bytes", changed):
            after = producers.source_closure("foundations")
        self.assertNotEqual(before[str(manifest.relative_to(ROOT))], after[str(manifest.relative_to(ROOT))])
        self.assertEqual({name for name in before if before[name] != after[name]},
                         {str(manifest.relative_to(ROOT))})

    def test_localized_config_mismatch_stays_visible(self):
        observed = {"id": "localized-config-diagnostics", "locale": "de-DE", "code": 5023,
                    "leaf_localized": 'Unbekannte Compileroption "notAnOption".',
                    "writer_localized": "Unknown compiler option 'notAnOption'."}
        result = integration.localized_result(ROOT, observed)
        self.assertEqual(result["status"], "different")
        observed["writer_localized"] = observed["leaf_localized"]
        self.assertEqual(integration.localized_result(ROOT, observed)["status"], "match")
        observed["leaf_localized"] = "wrong leaf translation"
        with self.assertRaisesRegex(ValueError, "pinned Go catalog"):
            integration.localized_result(ROOT, observed)

    def test_installed_consumer_cannot_pass_without_locale_assets(self):
        observed = {"encoded_bytes": 64, "libraries": 108, "locale": "de-DE",
                    "diagnostic_code": 2322,
                    "localized_message": 'Der Typ "number" kann dem Typ "string" nicht zugewiesen werden.',
                    "owners_returned_to_baseline": True}
        integration.validate_installed_observation(ROOT, observed)
        for key, wrong in (("encoded_bytes", 0), ("libraries", 107), ("locale", "en"),
                           ("localized_message", "Type 'number' is not assignable to type 'string'."),
                           ("diagnostic_code", True), ("owners_returned_to_baseline", 1)):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "observations differ"):
                integration.validate_installed_observation(ROOT, {**observed, key: wrong})
        with self.assertRaisesRegex(ValueError, "omits"):
            integration.validate_installed_observation(ROOT, None)


class IntegrationEvaluationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.prepared = integration.check()
        cls.inputs = {"fixture": "a" * 64}

    def receipt(self, identity, stdout):
        document = integration.load(ROOT, integration.MANIFEST)
        if identity == "generation":
            command = document["generation"]["command"]
        elif identity == "transport":
            command = ["python3", "scripts/s11.py", "capture"]
        else:
            command = next(row["command"] for row in document["witnesses"] if row["id"] == identity)
        return integration.receipt(identity, command, self.inputs, stdout)

    def evaluate(self, reports=(), receipts=()):
        return integration.evaluate(self.prepared, reports, receipts, source_inputs=self.inputs)

    def test_existing_cases_resolve_only_from_supplied_comparisons(self):
        report = {"rows": [{"case": ref, "result": "match"} for ref in self.prepared["witnesses"][1]["references"]]}
        result = self.evaluate([report])
        self.assertEqual(result["witnesses"][1]["state"], "match")
        self.assertEqual(result["witnesses"][0]["state"], "pending")
        self.assertEqual(result["witnesses"][2]["additional_test_observations"], {"paths_keep_source_order_on_equal_prefixes_but_exact_matches_win": "pending"})
        self.assertFalse(result["complete"])

    def test_duplicate_contributing_case_is_not_silently_folded(self):
        row = {"case": "config/case", "result": "match"}
        result = self.evaluate([{"rows": [row, row]}])
        self.assertTrue(any("duplicate contributing" in p for p in result["problems"]))

    def test_actual_named_test_and_stale_or_tampered_receipt(self):
        receipt = self.receipt("retained-program-snapshot", "test tests::retained_snapshot_edit_reuses_only_equal_parse_inputs ... ok\n")
        result = self.evaluate(receipts=[receipt])
        self.assertEqual(result["witnesses"][3]["state"], "match")
        altered = copy.deepcopy(receipt)
        altered["stdout"] += "tampered"
        self.assertTrue(any("output digest" in p for p in self.evaluate(receipts=[altered])["problems"]))
        altered = copy.deepcopy(receipt)
        altered["source_inputs"]["nested/changed.rs"] = "b" * 64
        self.assertTrue(any("stale or incomplete" in p for p in self.evaluate(receipts=[altered])["problems"]))

    def test_zero_or_duplicate_test_execution_is_invalid(self):
        for output in ("0 tests\n", "test tests::retained_snapshot_edit_reuses_only_equal_parse_inputs ... ok\n" * 2):
            receipt = self.receipt("retained-program-snapshot", output)
            self.assertTrue(any("exactly once" in p for p in self.evaluate(receipts=[receipt])["problems"]))

    def test_s11_receipt_requires_all_89_cases(self):
        output = {"metrics": {"controls": True}, "tests": {case: "pass" for case in integration.load(ROOT, "data/s11/cases.json")}}
        receipt = self.receipt("transport", json.dumps(output))
        self.assertEqual(self.evaluate(receipts=[receipt])["transport"]["state"], "match")
        output["tests"].pop(next(iter(output["tests"])))
        receipt = self.receipt("transport", json.dumps(output))
        self.assertTrue(any("all 89" in p for p in self.evaluate(receipts=[receipt])["problems"]))

    def test_generation_requires_locale_and_untouched_client_results(self):
        output = {"metrics": {"ast_schema": True, "patches_apply": True, "client_identical": True, "drift": False, "locale_complete": True}}
        receipt = self.receipt("generation", json.dumps(output))
        self.assertEqual(self.evaluate(receipts=[receipt])["generation"]["state"], "match")
        del output["metrics"]["locale_complete"]
        receipt = self.receipt("generation", json.dumps(output))
        self.assertTrue(any("required measurement" in p for p in self.evaluate(receipts=[receipt])["problems"]))

    def test_complete_requires_every_measured_integration_boundary(self):
        reports = [{"rows": [{"case": ref, "result": "match"}
                             for row in self.prepared["witnesses"] if row["kind"] == "cases"
                             for ref in row["references"]]}]
        observed = {"id": "localized-config-diagnostics", "locale": "de-DE", "code": 5023,
                    "leaf_localized": 'Unbekannte Compileroption "notAnOption".',
                    "writer_localized": 'Unbekannte Compileroption "notAnOption".'}
        from test_phase1_localized import fixture
        localized = integration.localized_integration_result(ROOT, observed, fixture())
        receipts = [self.receipt("localized-config-diagnostics", json.dumps(localized)),
                    self.receipt("retained-program-snapshot", "test tests::retained_snapshot_edit_reuses_only_equal_parse_inputs ... ok\n"),
                    self.receipt("transport", json.dumps({"metrics": {"controls": True}, "tests": {case: "pass" for case in integration.load(ROOT, "data/s11/cases.json")}})),
                    self.receipt("generation", json.dumps({"metrics": {"ast_schema": True, "patches_apply": True, "client_identical": True, "drift": False, "locale_complete": True}}))]
        for row in self.prepared["witnesses"]:
            for test in row.get("additional_tests", []):
                receipts.append(integration.receipt(row["id"] + "/" + test["test"], test["command"], self.inputs,
                                                    "test " + test["test"] + " ... ok\n"))
        installed = next(row for row in self.prepared["witnesses"] if row["id"] == "installed-generated-assets")
        packages = integration.load(ROOT, "tools/packaging/packages.json")["packages"]
        artifact = {"state": "pass", "archives": {row["name"]: {} for row in packages if row["publish"]},
                    "commands": [{"log": "consumer.log", "command": ["/isolated/package_consumer"]}],
                    "consumer_observation": {"encoded_bytes": 64, "libraries": 108, "locale": "de-DE",
                        "diagnostic_code": 2322,
                        "localized_message": 'Der Typ "number" kann dem Typ "string" nicht zugewiesen werden.',
                        "owners_returned_to_baseline": True}}
        receipts.append(integration.receipt(installed["id"], installed["command"], self.inputs, "", artifact=artifact))
        result = self.evaluate(reports, receipts)
        self.assertEqual(result["problems"], [])
        self.assertTrue(result["complete"])
        self.assertFalse(self.evaluate(reports, receipts[:-1])["complete"])

    def test_duplicate_unknown_and_changed_command_receipts_are_invalid(self):
        receipt = self.receipt("retained-program-snapshot", "test tests::retained_snapshot_edit_reuses_only_equal_parse_inputs ... ok\n")
        self.assertTrue(any("duplicate integration" in p for p in self.evaluate(receipts=[receipt, receipt])["problems"]))
        receipt["command"] = ["true"]
        self.assertTrue(any("command differs" in p for p in self.evaluate(receipts=[receipt])["problems"]))
        receipt["id"] = "invented"
        self.assertTrue(any("unknown integration" in p for p in self.evaluate(receipts=[receipt])["problems"]))


class GenerationTests(unittest.TestCase):
    def run_generation(self, locale_returncode=0, first=None):
        calls = []
        def run(args, **kwargs):
            calls.append((args, kwargs))
            if args[:2] == ["cargo", "xtask"]:
                return subprocess.CompletedProcess(args, 0, json.dumps(first or {"metrics": {"client_identical": True, "drift": False}}).encode())
            return subprocess.CompletedProcess(args, locale_returncode)
        return generation.capture(run=run), calls

    def test_exact_locale_generation_is_measured_separately(self):
        result, calls = self.run_generation()
        self.assertTrue(result["metrics"]["locale_complete"])
        self.assertEqual(calls[0][0], ["cargo", "xtask", "gen", "--verify"])
        self.assertEqual(calls[1][0][1:], ["scripts/generate_locale_tables.py", "--check"])
        self.assertIs(calls[1][1]["stdout"], sys.stderr)

    def test_locale_failure_is_not_borrowed_s03_success(self):
        result, _ = self.run_generation(locale_returncode=1)
        self.assertTrue(result["metrics"]["client_identical"])
        self.assertFalse(result["metrics"]["locale_complete"])

    def test_original_failed_metrics_survive_wrapper(self):
        result, _ = self.run_generation(first={"metrics": {"ast_schema": False}})
        self.assertFalse(result["metrics"]["ast_schema"])
        self.assertNotIn("client_identical", result["metrics"])

    def test_invalid_original_metrics_are_not_success(self):
        with self.assertRaisesRegex(ValueError, "did not emit metrics"):
            self.run_generation(first={"empty": True})

    def test_locale_metric_cannot_shadow_another_producer(self):
        with self.assertRaisesRegex(ValueError, "another producer"):
            self.run_generation(first={"metrics": {"locale_complete": True}})


if __name__ == "__main__":
    unittest.main()
