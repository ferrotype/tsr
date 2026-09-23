"""Host applicability must come from authenticated native identity, not prose."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_capture as capture
import phase1_hosts as hosts
from test_phase1 import SyntheticCapture


class HostContractTests(unittest.TestCase):
    def test_structured_host_contract(self):
        self.assertEqual(hosts.required_goos({"hosts": ["any"]}), ("linux", "darwin"))
        self.assertEqual(hosts.required_goos({"hosts": ["posix"]}), ("linux", "darwin"))
        self.assertTrue(hosts.applies({"hosts": ["darwin"]}, "darwin"))
        self.assertFalse(hosts.applies({"hosts": ["linux"]}, "darwin"))
        for tags in (None, [], "linux", ["Linux"], ["any", "linux"], ["posix", "darwin"], ["linux", "linux"]):
            with self.subTest(tags=tags), self.assertRaises(ValueError):
                hosts.request_hosts({"hosts": tags})
        with self.assertRaisesRegex(ValueError, "unsupported"):
            hosts.applies({"hosts": ["any"]}, "freebsd")

    def test_every_filesystem_request_has_reviewed_hosts_not_runtime_prose(self):
        requests = capture.load_requests(capture.FAMILIES["filesystem"])["requests"]
        for request in requests:
            self.assertIn("hosts", request, request["case"])
            self.assertNotIn("host_applicability", request, request["case"])
            self.assertTrue(request["host_note"], request["case"])
            hosts.request_hosts(request)
        os_rows = {r["case"].rsplit("/", 1)[1]: r for r in requests if r["case"].startswith("filesystem/osvfs/")}
        self.assertEqual(os_rows["nativepath-realpath-eval-symlinks"]["hosts"], ["darwin"])
        self.assertEqual(os_rows["nativepath-realpath-linux-procfs"]["hosts"], ["linux"])

    def test_filesystem_loader_refuses_missing_or_invalid_host_metadata(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "requests.json"
            for fields in ({}, {"hosts": ["guess from note"]}):
                path.write_text(json.dumps({"family": "filesystem", "requests": [{"case": "x", **fields}]}))
                with self.assertRaises(ValueError):
                    capture.load_requests({"requests": str(path)})

    def test_case_claims_must_agree_with_request_hosts(self):
        request = {"case": "x", "hosts": ["linux"]}
        hosts.validate_case(request, {"id": "x", "hosts": ["linux"]})
        for case in ({"id": "x"}, {"id": "x", "hosts": ["darwin"]}, {"id": "x", "hosts": []}):
            with self.subTest(case=case), self.assertRaises(ValueError):
                hosts.validate_case(request, case)

    def test_declaring_the_wrong_document_family_cannot_bypass_hosts(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "requests.json"
            path.write_text(json.dumps({"family": "pilot", "requests": [{"case": "x"}]}))
            with self.assertRaisesRegex(ValueError, "structured hosts"):
                capture.load_requests({"requests": str(path), "rust_package": "phase1_filesystem"})


class HostComparisonTests(unittest.TestCase):
    def setUp(self):
        self.requests = [{"case": name, "hosts": targets} for name, targets in (
            ("linux", ["linux"]), ("darwin", ["darwin"]), ("any", ["any"]), ("posix", ["posix"]))]
        self.provenance = {"family": "filesystem", "pin": "pin", "partial": False,
                           "host": {"goos": "darwin"}, "requests_sha256": "request-hash",
                           "case_claims": {r["case"]: "claims-hash" for r in self.requests}}
        self.native = {r["case"]: {"case": r["case"], "result": "observed", "observation": []} for r in self.requests}
        self.rust = copy.deepcopy(self.native)
        self.native["linux"] = {"case": "linux", "result": "native_unavailable", "reason": "Linux-only native implementation"}

    def compare(self, require_parity=False):
        with patch.object(capture, "validate_capture", return_value=(self.provenance, self.requests, self.native, self.rust)), \
                patch.object(capture, "load_requests", return_value={"requests": self.requests}), \
                patch.object(capture, "sha_file", return_value="capture-hash"):
            return capture.compare(Path("unused"), require_parity)

    def test_excluded_unavailable_keeps_raw_reason_and_leaves_denominator(self):
        report = self.compare(require_parity=True)
        self.assertEqual(report["parity"], 1.0)
        self.assertEqual(report["applicable_cases"], 3)
        self.assertEqual(report["counts"]["not_applicable"], 1)
        row = report["rows"][0]
        self.assertEqual(row["native_result"], "native_unavailable")
        self.assertEqual(row["reason"], self.native["linux"]["reason"])
        self.assertEqual(report["host"], self.provenance["host"])
        self.assertEqual(self.native["linux"]["result"], "native_unavailable")

    def test_applicable_unavailable_is_still_a_required_failure(self):
        for identity in ("darwin", "any", "posix"):
            with self.subTest(identity=identity):
                self.native[identity] = {"result": "native_unavailable", "reason": "probe broke"}
                report = self.compare()
                self.assertGreater(report["required_non_match"], 0)
                self.assertEqual(next(r for r in report["rows"] if r["case"] == identity)["result"], "native_unavailable")
                with self.assertRaisesRegex(ValueError, "parity required"):
                    self.compare(require_parity=True)

    def test_exclusion_does_not_hide_a_harness_failure(self):
        self.rust["linux"] = {"result": "harness_failed", "error": "bad input"}
        with self.assertRaisesRegex(ValueError, "failed in the harness"):
            self.compare()

    def test_observed_excluded_case_is_not_silently_discarded(self):
        self.native["linux"] = {"result": "observed", "observation": ["different"]}
        self.assertEqual(self.compare()["counts"]["different"], 1)

    def test_join_refuses_cross_host_row_folding(self):
        darwin = self.compare()
        linux = copy.deepcopy(darwin)
        linux["host"]["goos"] = "linux"
        with self.assertRaisesRegex(ValueError, "retain separate host captures"):
            capture.join([darwin, linux])

    def test_partial_host_selection_has_local_denominator_but_stays_partial(self):
        inventory = copy.deepcopy(self.requests)
        selected = [r for r in self.requests if r["case"] != "linux"]
        self.provenance["partial"] = True
        with patch.object(capture, "validate_capture", return_value=(self.provenance, selected, self.native, self.rust)), \
                patch.object(capture, "load_requests", return_value={"requests": inventory}), \
                patch.object(capture, "sha_file", return_value="capture-hash"):
            report = capture.compare(Path("unused"))
            self.assertEqual(report["applicable_cases"], 3)
            self.assertEqual(report["parity"], 1.0)
            self.assertEqual(report["counts"]["not_run"], 1)
            with self.assertRaisesRegex(ValueError, "unrun"):
                capture.compare(Path("unused"), require_parity=True)


class HostAuthenticationTests(unittest.TestCase):
    def test_legacy_filesystem_host_is_stale_but_tampered_bytes_are_invalid(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            SyntheticCapture(directory, [{"case": "unknown", "native": [],
                "rust": {"result": "observed", "observation": []}}], closure={})
            path = directory / "provenance.json"
            provenance = json.loads(path.read_text())
            provenance["family"] = "filesystem"
            provenance["host"] = {"platform": "macOS-26-arm64", "python": "3.12"}
            path.write_bytes(capture.canonical(provenance))
            with self.assertRaisesRegex(capture.StaleCapture, "predates authenticated host"):
                capture.authenticate(directory)
            (directory / "rust-observations.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "artifact.*recorded hash") as error:
                capture.authenticate(directory)
            self.assertNotIsInstance(error.exception, capture.StaleCapture)

    def test_host_label_cannot_disagree_with_authenticated_native_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            SyntheticCapture(directory, [{"case": "unknown", "native": [],
                "rust": {"result": "observed", "observation": []}}], closure={})
            path = directory / "native/vfsmatch/observations.json"
            raw = json.loads(path.read_text())
            raw.update(goos="darwin", goarch="arm64")
            path.write_bytes(capture.canonical(raw))
            prov_path = directory / "provenance.json"
            provenance = json.loads(prov_path.read_text())
            provenance["host"] = {"goos": "darwin", "goarch": "arm64"}
            provenance["native_probes"]["vfsmatch"]["observations_sha256"] = capture.sha_file(path)
            prov_path.write_bytes(capture.canonical(provenance))
            with patch.object(capture, "source_closure", return_value={}):
                self.assertEqual(capture.authenticate(directory)["host"]["goos"], "darwin")
                provenance["host"]["goos"] = "linux"
                prov_path.write_bytes(capture.canonical(provenance))
                with self.assertRaisesRegex(ValueError, "host GOOS disagrees"):
                    capture.authenticate(directory)
                provenance["host"]["goos"] = "darwin"
                provenance["native_probes"]["vfsmatch"]["goos"] = "linux"
                prov_path.write_bytes(capture.canonical(provenance))
                with self.assertRaisesRegex(ValueError, "host GOOS disagrees"):
                    capture.authenticate(directory)
