"""F5a integration cannot turn fixture existence into executed parity."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
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
        self.assertNotIn("crates/tsr_jsnum/SLICE.md", paths)
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

    def test_build_and_generation_transitive_inputs_are_bound(self):
        paths = set(integration.input_paths())
        required = {"xtask/src/main.rs", ".gitmodules", "data/s03/api-special-codecs.json",
                    "scripts/s05_tables.py", "crates/tsr_testhost/Cargo.toml", "data/s07/program-requests.json"}
        self.assertLessEqual(required, paths)

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
        import phase1_producers as producers
        cls.prepared = integration.check()
        # The real producer snapshot: each receipt records its own closure from it.
        cls.inputs = producers.source_closure("foundations")

    def closure(self, identity):
        return integration.receipt_inputs(identity, self.inputs)

    def receipt(self, identity, stdout):
        document = integration.load(ROOT, integration.MANIFEST)
        command = integration.receipt_specs(document)[identity]["command"]
        return integration.receipt(identity, command, self.closure(identity), stdout)

    def evaluate(self, reports=(), receipts=()):
        return integration.evaluate(self.prepared, reports, receipts, source_inputs=self.inputs)

    def test_existing_cases_resolve_only_from_supplied_comparisons(self):
        report = {"rows": [{"case": ref, "result": "match"} for ref in self.prepared["witnesses"][1]["references"]]}
        result = self.evaluate([report])
        self.assertEqual(result["witnesses"][1]["state"], "match")
        self.assertEqual(result["witnesses"][0]["state"], "pending")
        self.assertEqual(result["witnesses"][2]["additional_test_observations"], {"paths_keep_source_order_on_equal_prefixes_but_exact_matches_win": "pending"})
        self.assertFalse(result["complete"])

    def test_an_empty_case_witness_never_passes_vacuously(self):
        preparation = copy.deepcopy(self.prepared)
        row = next(row for row in preparation["witnesses"] if row["kind"] == "cases")
        row["references"] = []
        result = integration.evaluate(preparation, [], source_inputs=self.inputs)
        observed = next(item for item in result["witnesses"] if item["id"] == row["id"])
        self.assertEqual(observed["state"], "pending")
        self.assertFalse(result["complete"])

    def test_duplicate_contributing_case_is_not_silently_folded(self):
        row = {"case": "config/case", "result": "match"}
        result = self.evaluate([{"rows": [row, row]}])
        self.assertTrue(any("duplicate contributing" in p for p in result["problems"]))

    def test_excluded_host_case_is_valid_but_cannot_certify_integration(self):
        witness = next(row for row in self.prepared["witnesses"] if row["kind"] == "cases")
        report = {"rows": [{"case": ref, "result": "not_applicable"} for ref in witness["references"]]}
        result = self.evaluate([report])
        self.assertEqual(result["problems"], [])
        observed = next(row for row in result["witnesses"] if row["id"] == witness["id"])
        self.assertNotEqual(observed["state"], "match")

    def test_actual_named_test_and_stale_or_tampered_receipt(self):
        receipt = self.receipt("retained-program-snapshot", "test tests::live_filesystem_snapshots_preserve_retained_program_files ... ok\n")
        result = self.evaluate(receipts=[receipt])
        self.assertEqual(result["witnesses"][3]["state"], "match")
        altered = copy.deepcopy(receipt)
        altered["stdout"] += "tampered"
        self.assertTrue(any("output digest" in p for p in self.evaluate(receipts=[altered])["problems"]))
        altered = copy.deepcopy(receipt)
        altered["source_inputs"]["nested/changed.rs"] = "b" * 64
        stale = self.evaluate(receipts=[altered])
        self.assertEqual(stale["problems"], [])
        self.assertEqual(stale["witnesses"][3]["state"], "unavailable")
        self.assertEqual(stale["unavailable"][receipt["id"]]["changed_inputs"], ["nested/changed.rs"])
        self.assertFalse(stale["complete"])
        altered["stdout"] += "tampered"
        self.assertTrue(any("output digest" in p for p in self.evaluate(receipts=[altered])["problems"]))

    def test_stale_receipt_does_not_discard_independent_current_result(self):
        stale = self.receipt("retained-program-snapshot", "test tests::live_filesystem_snapshots_preserve_retained_program_files ... ok\n")
        stale["source_inputs"] = {"old": "c" * 64}
        current = self.receipt("generation", json.dumps({"metrics": {"ast_schema": True, "patches_apply": True,
            "client_identical": True, "drift": False, "locale_complete": True}}))
        result = self.evaluate(receipts=[stale, current])
        self.assertEqual(result["problems"], [])
        self.assertEqual(result["generation"]["state"], "match")
        self.assertEqual(result["witnesses"][3]["state"], "unavailable")
        self.assertFalse(result["complete"])

    def test_malformed_receipt_input_map_is_not_staleness(self):
        receipt = self.receipt("retained-program-snapshot", "test tests::live_filesystem_snapshots_preserve_retained_program_files ... ok\n")
        for bad in (None, {}, [], {"source": "not-a-digest"}):
            with self.subTest(inputs=bad):
                result = self.evaluate(receipts=[{**receipt, "source_inputs": bad}])
                self.assertTrue(any("malformed" in p for p in result["problems"]))

    def test_zero_or_duplicate_test_execution_is_invalid(self):
        for output in ("0 tests\n", "test tests::live_filesystem_snapshots_preserve_retained_program_files ... ok\n" * 2):
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
                    self.receipt("retained-program-snapshot", "test tests::live_filesystem_snapshots_preserve_retained_program_files ... ok\n"),
                    self.receipt("transport", json.dumps({"metrics": {"controls": True}, "tests": {case: "pass" for case in integration.load(ROOT, "data/s11/cases.json")}})),
                    self.receipt("generation", json.dumps({"metrics": {"ast_schema": True, "patches_apply": True, "client_identical": True, "drift": False, "locale_complete": True}}))]
        for row in self.prepared["witnesses"]:
            for test in row.get("additional_tests", []):
                identity = row["id"] + "/" + test["test"]
                receipts.append(integration.receipt(identity, test["command"], self.closure(identity),
                                                    "test " + test["test"] + " ... ok\n"))
        installed = next(row for row in self.prepared["witnesses"] if row["id"] == "installed-generated-assets")
        packages = integration.load(ROOT, "tools/packaging/packages.json")["packages"]
        artifact = {"state": "pass", "archives": {row["name"]: {} for row in packages if row["publish"]},
                    "commands": [{"log": "consumer.log", "command": ["/isolated/package_consumer"]}],
                    "consumer_observation": {"encoded_bytes": 64, "libraries": 108, "locale": "de-DE",
                        "diagnostic_code": 2322,
                        "localized_message": 'Der Typ "number" kann dem Typ "string" nicht zugewiesen werden.',
                        "owners_returned_to_baseline": True}}
        receipts.append(integration.receipt(installed["id"], installed["command"], self.closure(installed["id"]), "",
                                            artifact=artifact))
        observations = [{"id": identity, "command": command, "exit_code": 0,
                         "stdout": "".join("test " + name + " ... ok\n" for name in tests)}
                        for identity, (command, tests) in integration.RUST_WITNESS_TESTS.items()]
        receipts.append(self.receipt("rust-witnesses", json.dumps(observations)))
        result = self.evaluate(reports, receipts)
        self.assertEqual(result["problems"], [])
        self.assertTrue(result["complete"])
        self.assertFalse(self.evaluate(reports, receipts[:-1])["complete"])

    def test_rust_witness_metric_requires_exact_executed_inventory(self):
        observations = [{"id": identity, "command": command, "exit_code": 0,
                         "stdout": "".join("test " + name + " ... ok\n" for name in tests)}
                        for identity, (command, tests) in integration.RUST_WITNESS_TESTS.items()]
        self.assertTrue(all(value == "match" for value in integration.rust_witness_result(observations).values()))
        for mutate in (lambda rows: rows.pop(), lambda rows: rows.append(rows[0]),
                       lambda rows: rows[0].update(stdout="running 0 tests\n"),
                       lambda rows: rows[0].update(exit_code=101)):
            changed = copy.deepcopy(observations)
            mutate(changed)
            with self.assertRaises(ValueError):
                integration.rust_witness_result(changed)

    def test_binder_witness_uses_rust_module_identity_not_source_filename(self):
        command, names = integration.RUST_WITNESS_TESTS["witness/binder-container-flags-source-contract"]
        self.assertEqual(command, ["cargo", "test", "--locked", "-p", "tsr_binder", "--lib",
                                  "container_classification::tests::", "--", "--test-threads=1"])
        self.assertEqual(names, [
            "container_classification::tests::fixed_container_rules_do_not_inspect_payload_or_parent",
            "container_classification::tests::method_rules_read_only_the_selected_parent_kind",
            "container_classification::tests::block_rules_include_signature_and_static_block_parents",
            "container_classification::tests::property_rules_inspect_initializer_without_requiring_a_parent",
            "container_classification::tests::local_dynamic_rules_preserve_checked_contract_failures",
        ])

    def test_duplicate_unknown_and_changed_command_receipts_are_invalid(self):
        receipt = self.receipt("retained-program-snapshot", "test tests::live_filesystem_snapshots_preserve_retained_program_files ... ok\n")
        self.assertTrue(any("duplicate integration" in p for p in self.evaluate(receipts=[receipt, receipt])["problems"]))
        receipt["command"] = ["true"]
        self.assertTrue(any("command differs" in p for p in self.evaluate(receipts=[receipt])["problems"]))
        receipt["id"] = "invented"
        self.assertTrue(any("unknown integration" in p for p in self.evaluate(receipts=[receipt])["problems"]))


class ReceiptClosureTests(unittest.TestCase):
    """Each receipt is bound to what its own command reads, and to nothing else.

    A change only another receipt depends on (or a coverage record) must not
    make it unavailable, and every file its command reads must.
    """

    # One input each receipt's command reads, chosen away from the shared build
    # inputs, so the test can show that only the receipts reading it go stale.
    REPRESENTATIVE = {
        "transport": "crates/tsr_testhost/src/session.rs",
        "generation": "scripts/s05_tables.py",
        "rust-witnesses": "crates/tsr_binder/src/container_classification.rs",
        integration.MUTATION_RECEIPT: "tools/phase1/mutation/go/patches.json",
        "localized-config-diagnostics": "tools/phase1/config/localized-requests.json",
        "retained-program-snapshot": "crates/tsr_compiler/src/tests.rs",
        "installed-generated-assets": "tools/packaging/consumer.rs",
        "ordered-config-resolution/paths_keep_source_order_on_equal_prefixes_but_exact_matches_win":
            "crates/tsr_module/tests/relative_paths.rs",
    }

    @classmethod
    def setUpClass(cls):
        import phase1_producers as producers
        cls.producers = producers
        cls.prepared = integration.check()
        cls.document = integration.load(ROOT, integration.MANIFEST)
        cls.specs = integration.receipt_specs(cls.document)
        cls.inputs = producers.source_closure("foundations")
        cls.paths = {identity: set(integration.receipt_input_paths(identity)) for identity in cls.specs}
        cls.recorded = [integration.receipt(identity, spec["command"], integration.receipt_inputs(identity, cls.inputs), "")
                        for identity, spec in cls.specs.items()]

    def stale(self, changed):
        """Receipts recorded on the committed snapshot, evaluated on a changed one."""
        return integration.evaluate(self.prepared, [], self.recorded, source_inputs=changed)["unavailable"]

    def test_every_receipt_kind_has_a_representative_input(self):
        self.assertEqual(set(self.REPRESENTATIVE), set(self.specs))

    def test_every_receipt_closure_is_inside_the_producer_closure_and_its_ledger(self):
        from test_phase1_tracker import runs, selected
        ledger = selected(ROOT, runs()["foundations"])
        for identity, paths in self.paths.items():
            with self.subTest(receipt=identity):
                self.assertGreater(len(paths), len(integration.RECEIPT_BUILD_INPUTS))
                self.assertEqual(sorted(path for path in paths if not (ROOT / path).is_file()), [])
                self.assertEqual(sorted(paths - self.inputs.keys()), [])
                self.assertEqual(sorted({"upstream" if path.startswith("upstream/") else path
                                         for path in paths} - ledger), [])

    def test_a_receipt_that_recorded_more_than_its_closure_stays_as_strict(self):
        # An older receipt recorded the whole producer closure: every input it
        # recorded still counts, so an unrelated edit keeps it unavailable.
        identity = "generation"
        receipt = integration.receipt(identity, self.specs[identity]["command"], dict(self.inputs), "")
        unrelated = "data/phase1/cases.json"
        self.assertNotIn(unrelated, self.paths[identity])
        result = integration.evaluate(self.prepared, [], [receipt], source_inputs={**self.inputs, unrelated: "0" * 64})
        self.assertEqual(result["unavailable"][identity]["changed_inputs"], [unrelated])

    def test_a_receipt_missing_part_of_its_closure_is_unavailable(self):
        identity = "transport"
        inputs = integration.receipt_inputs(identity, self.inputs)
        inputs.pop("scripts/s11_tunnel.py")
        receipt = integration.receipt(identity, self.specs[identity]["command"], inputs, "")
        result = integration.evaluate(self.prepared, [], [receipt], source_inputs=self.inputs)
        self.assertEqual(result["unavailable"][identity]["changed_inputs"], ["scripts/s11_tunnel.py"])

    def test_a_change_stales_exactly_the_receipts_that_read_it(self):
        for identity, path in self.REPRESENTATIVE.items():
            with self.subTest(receipt=identity, path=path):
                self.assertIn(path, self.paths[identity])
                self.assertIn(path, self.inputs)
                changed = {**self.inputs, path: "0" * 64}
                unavailable = self.stale(changed)
                self.assertEqual(set(unavailable), {name for name, paths in self.paths.items() if path in paths})
                self.assertIn(identity, unavailable)
                self.assertLess(len(unavailable), len(self.specs), "a representative input stales every receipt")
                for row in unavailable.values():
                    self.assertEqual(row["changed_inputs"], [path])

    def test_coverage_records_and_the_testhost_do_not_touch_generation(self):
        # The review's own witness: re-recording cases.json stales no receipt;
        # a testhost edit stales transport and leaves generation current.
        self.assertEqual(self.stale({**self.inputs, "data/phase1/cases.json": "0" * 64}), {})
        unavailable = self.stale({**self.inputs, "crates/tsr_testhost/src/session.rs": "0" * 64})
        self.assertIn("transport", unavailable)
        self.assertNotIn("generation", unavailable)

    def edited_digest(self, relative):
        """The producer snapshot's digest of `relative` after an in-memory edit (None: not an input)."""
        target, original = ROOT / relative, Path.read_bytes
        self.assertTrue(target.is_file(), relative)
        with patch.object(Path, "read_bytes", lambda path: original(path) + b"\nedited\n" if path == target
                          else original(path)):
            return self.producers.source_closure("foundations").get(relative)

    def test_a_real_file_edit_reaches_the_receipt_through_the_producer_snapshot(self):
        # Only the edited digest is applied, so concurrent edits elsewhere cannot blur the result.
        relative = "scripts/s11_tunnel.py"
        digest = self.edited_digest(relative)
        self.assertNotEqual(digest, self.inputs[relative])
        unavailable = self.stale({**self.inputs, relative: digest})
        self.assertEqual(unavailable["transport"]["changed_inputs"], [relative])

    def test_entry_scripts_are_followed_through_their_imports(self):
        for identity, path in (("transport", "scripts/s11_followups.py"), ("transport", "scripts/s11_tunnel.py"),
                               ("transport", "scripts/tracking-bootstrap.py"),
                               ("generation", "scripts/s05_tables.py"), ("generation", "scripts/s03.py"),
                               ("installed-generated-assets", "scripts/package_assets.py"),
                               ("localized-config-diagnostics", "scripts/phase1_localized.py"),
                               (integration.MUTATION_RECEIPT, "scripts/phase1_mutation_go.py"),
                               # s07_binder imports it through sys.path to validate binder graphs.
                               (integration.MUTATION_RECEIPT, "tools/s07/binder/generate_syntax.py")):
            with self.subTest(receipt=identity, path=path):
                self.assertIn(path, self.paths[identity])

    def test_transport_binds_the_whole_testhost_package(self):
        package = {str(path.relative_to(ROOT)) for path in (ROOT / "crates/tsr_testhost").rglob("*")
                   if path.is_file() and path.name != ".DS_Store"}
        self.assertIn("crates/tsr_testhost/src/session.rs", package)
        self.assertLessEqual(package, self.paths["transport"])

    def test_transport_and_generation_cover_their_ledger_runs(self):
        """transport is the [testhost] run and generation the [gen] run, less only what the command never reads.

        Prose is no input; `upstream` is bound as the pin (data/upstream.json,
        .gitmodules); xtask only launches the testhost run and the S11 unit
        tests are not what `s11.py capture` executes.
        """
        from test_phase1_tracker import runs, selected
        ledger = runs()
        for identity, section, unread in (
                ("transport", "testhost", lambda path: path.startswith(("xtask/", "scripts/tests/"))),
                ("generation", "gen", lambda path: False)):
            with self.subTest(receipt=identity):
                expected = selected(ROOT, ledger[section]) | set(ledger[section]["inputs"])
                missing = sorted(path for path in expected - self.paths[identity]
                                 if path != "upstream" and not path.endswith(".md") and not unread(path))
                self.assertEqual(missing, [])
                self.assertIn("scripts/s05_tables.py", selected(ROOT, ledger["gen"]))

    def test_mutation_receipt_binds_every_spliced_file(self):
        import phase1_scope
        manifest = integration.load(ROOT, phase1_scope.MUTATION_MANIFEST)
        files = {mutant["file"] for mutant in manifest["mutants"]}
        self.assertTrue(files)
        self.assertLessEqual(files, self.paths[integration.MUTATION_RECEIPT])

    def test_mutation_receipt_binds_every_oracle_binary_confirm_builds(self):
        """confirm builds each traced oracle's package; the syntax oracle is phase1_syntax, not the driver."""
        import gzip
        import phase1_capture as capture
        import phase1_mutation_go as go
        import phase1_mutation_run as run
        import phase1_scope
        results = json.loads(gzip.decompress((ROOT / phase1_scope.MUTATION_RESULTS).read_bytes()))
        traced = set(results["inputs"].get("traced_oracles") or results["inputs"]["oracles"])
        self.assertIn("syntax", traced)
        self.assertLessEqual(traced, set(run.PACKAGES))
        self.assertEqual(set(go.ORACLES), set(run.PACKAGES))
        names = self.producers.workspace_packages()
        packages = self.producers.rust_package_closure([names[name] for name in set(run.PACKAGES.values())])
        self.assertEqual(packages, self.producers.mutation_oracle_packages())
        for directory in ("tools/phase1/syntax", "tools/phase1/harness", "crates/tsr_checker", "crates/tsr_compiler"):
            self.assertIn(directory, packages)
        files = {str(path.resolve().relative_to(ROOT.resolve())) for directory in packages
                 for path in capture.package_input_files(ROOT / directory)}
        mutation = self.paths[integration.MUTATION_RECEIPT]
        self.assertEqual(sorted(files - mutation), [])
        mutated = {mutant["file"] for mutant in integration.load(ROOT, phase1_scope.MUTATION_MANIFEST)["mutants"]}
        unmutated = sorted(path for path in files if path.startswith("crates/tsr_checker/src/")
                           and path.endswith(".rs") and path not in mutated)[0]
        for path in ("tools/phase1/syntax/src/mutation.rs", "tools/phase1/syntax/src/main.rs",
                     "tools/phase1/harness/src/lib.rs", unmutated, "tools/s07/binder/generate_syntax.py"):
            with self.subTest(path=path):
                unavailable = self.stale({**self.inputs, path: "0" * 64})
                self.assertIn(integration.MUTATION_RECEIPT, unavailable)
                self.assertEqual(unavailable[integration.MUTATION_RECEIPT]["changed_inputs"], [path])

    def test_generation_binds_the_pin_xtask_reads_before_it_dispatches_gen(self):
        # `cargo xtask gen --verify` passes read_ledger(PORTS.toml).pin to s03
        # and writes it into the generated outputs.
        main = (ROOT / "xtask/src/main.rs").read_text()
        self.assertIn('root.join("PORTS.toml")', main)
        self.assertIn('gen::run(&root, &args[1..], &read_ledger(&root).pin)', main)
        self.assertIn("PORTS.toml", self.paths["generation"])
        from test_phase1_tracker import runs, selected
        self.assertIn("PORTS.toml", selected(ROOT, runs()["gen"]))
        unavailable = self.stale({**self.inputs, "PORTS.toml": "0" * 64})
        self.assertIn("generation", unavailable)
        self.assertEqual(unavailable["generation"]["changed_inputs"], ["PORTS.toml"])

    def test_packaged_files_honor_include_and_readme(self):
        files = {str(path.relative_to(ROOT)) for path in integration.packaged_files(ROOT / "crates/tsr_jsnum")}
        self.assertTrue((ROOT / "crates/tsr_jsnum/SLICE.md").is_file())
        self.assertIn("crates/tsr_jsnum/README.md", files)
        self.assertIn("crates/tsr_jsnum/src/lib.rs", files)
        self.assertIn("crates/tsr_jsnum/Cargo.toml", files)
        self.assertNotIn("crates/tsr_jsnum/SLICE.md", files)
        self.assertFalse(any("/tests/" in name or "/examples/" in name for name in files))
        with tempfile.TemporaryDirectory() as temporary:
            package = Path(temporary)
            (package / "src").mkdir()
            for name in ("src/lib.rs", "NOTES.md", "README.md", "docs.txt"):
                (package / name).write_text("x")
            (package / "Cargo.toml").write_text('[package]\nname = "p"\nreadme = "README.md"\ninclude = ["/src/**"]\n')
            self.assertEqual({path.name for path in integration.packaged_files(package)},
                             {"lib.rs", "README.md", "Cargo.toml"})
            (package / "Cargo.toml").write_text('[package]\nname = "p"\n')
            self.assertEqual({path.name for path in integration.packaged_files(package)},
                             {"lib.rs", "NOTES.md", "README.md", "docs.txt", "Cargo.toml"})

    def test_unpackaged_prose_never_stales_receipts_but_a_packaged_readme_does(self):
        from test_phase1_tracker import runs, selected
        ledger = selected(ROOT, runs()["foundations"])
        for relative, packaged in (("crates/tsr_jsnum/SLICE.md", False), ("crates/tsr_jsstring/SLICE.md", False),
                                   ("crates/tsr_jsnum/README.md", True)):
            with self.subTest(path=relative):
                digest = self.edited_digest(relative)
                if packaged:
                    self.assertNotEqual(digest, self.inputs[relative])
                    unavailable = self.stale({**self.inputs, relative: digest})
                    self.assertIn("installed-generated-assets", unavailable)
                    self.assertNotIn("transport", unavailable)
                    self.assertIn(relative, ledger)
                else:
                    self.assertIsNone(digest, "unpackaged prose is a producer input")
                    self.assertFalse(any(relative in paths for paths in self.paths.values()))
                    self.assertNotIn(relative, ledger)


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
