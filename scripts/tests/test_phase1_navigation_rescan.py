"""The private Rust rescan unit test consumes measured pinned Go answers."""
import hashlib
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase1_capture as capture

ROOT = Path(__file__).resolve().parents[2]


class NavigationRescanFixtureTests(unittest.TestCase):
    def test_native_fixture_identity_and_rust_test_inputs(self):
        path = ROOT / "tools/phase1/syntax/astnav-rescan"
        requests = json.loads((path / "requests.json").read_bytes())
        observed = json.loads((path / "native-observations.json").read_bytes())
        provenance = json.loads((path / "native-provenance.json").read_bytes())
        self.assertEqual(provenance["pin"], json.loads((ROOT / "data/upstream.json").read_bytes())["pin"])
        self.assertEqual(provenance["source_sha256"], hashlib.sha256((path / "probe_test.go").read_bytes()).hexdigest())
        self.assertEqual(provenance["request_sha256"], capture.digest(capture.canonical(requests) + b"\n"))
        self.assertEqual(provenance["output_sha256"], hashlib.sha256((path / "native-observations.json").read_bytes()).hexdigest())
        self.assertEqual(observed["request_sha256"], provenance["request_sha256"])
        self.assertTrue(provenance["toolchain_local"])
        self.assertEqual([r["case"] for r in requests["requests"]], [r["case"] for r in observed["observations"]])
        lines = ["# input hex\tjsx-child\tbefore\tafter\tstart\tend\tflags"]
        for request, row in zip(requests["requests"], observed["observations"]):
            lines.append("\t".join([request["Text"].encode().hex(), str(request["JSX"]).lower(),
                                    *[str(row[key]) for key in ("before", "after", "start", "end", "flags")]]))
        self.assertEqual((ROOT / "crates/tsr_astnav/src/testdata/navigation-rescan.tsv").read_text(), "\n".join(lines) + "\n")
        self.assertEqual({r["rescan"] for r in observed["observations"]}, {False, True})


if __name__ == "__main__":
    unittest.main()
