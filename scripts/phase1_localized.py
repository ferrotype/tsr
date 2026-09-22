"""Bounded production config -> localized writer -> pinned test-envelope check."""
from pathlib import Path
import tempfile

import phase1_capture as capture
from s04_common import command, strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
REQUESTS = "tools/phase1/config/localized-requests.json"


def schedule(root=ROOT):
    return strict_json_loads((root / REQUESTS).read_bytes())


def compare(observation, root=ROOT):
    """Replay all six observations without executing a compiler or renderer."""
    requests = schedule(root)
    if observation.get("requests_sha256") != capture.digest(capture.canonical(requests) + b"\n"):
        raise ValueError("localized envelope request schedule differs")
    if observation.get("pin") != strict_json_loads((root / "data/upstream.json").read_bytes())["pin"]:
        raise ValueError("localized envelope pin differs")
    if observation["native"].get("request_sha256") != observation["requests_sha256"]:
        raise ValueError("native localized output observed a different request")
    bridge = {**requests, "observations": observation["raw"]["observations"]}
    if observation["rust"].get("request_sha256") != capture.digest(capture.canonical(bridge) + b"\n"):
        raise ValueError("localized renderer observed a different Rust request")
    native = capture.validate_response(observation["native"], requests["requests"], "native")
    capture.validate_rendered_rows(requests, observation["raw"], observation["rust"])
    rust = capture.validate_response(observation["rust"], requests["requests"], "rust")
    rows = []
    for expected, actual in zip(native, rust):
        if expected["result"] != "observed" or actual["result"] != "observed":
            raise ValueError("localized envelope did not execute: " + expected["case"])
        same = capture.canonical(expected["observation"]) == capture.canonical(actual["observation"])
        rows.append({"case": expected["case"], "result": "match" if same else "different"})
    return {"rows": rows, "status": "match" if rows and all(row["result"] == "match" for row in rows) else "different",
            "authority": "pinned native config parser and diagnostic writer; shared Go test-envelope renderer"}


def observe(root=ROOT):
    requests = schedule(root)
    target = root / "target/phase1-localized"
    target.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="capture-", dir=target))
    probe = next(p for p in capture.FAMILIES["config"]["native_probes"] if p["name"] == "tsconfigparsing")
    native = capture.run_probe(output / "native", probe["package"], (root / probe["probe"]).read_text(),
                               requests, "TestPhase1LocalizedConfig", False, (root / probe["helper"]).read_text(),
                               {name: (root / path).read_text() for name, path in probe["extra_sources"].items()})
    capture.validate_response(native, requests["requests"], "native")
    request_path = output / "requests.json"
    request_path.write_bytes(capture.canonical(requests) + b"\n")
    binary = capture.build_rust("config")
    raw_path = output / "rust-raw-observations.json"
    command([str(binary), str(request_path), str(raw_path)], cwd=root)
    raw = strict_json_loads(raw_path.read_bytes())
    capture.validate_response(raw, requests["requests"], "rust")
    renderer = capture.FAMILIES["config"]["renderer"]
    rust = capture.run_probe(output / "renderer", renderer["package"], (root / renderer["probe"]).read_text(),
                             {**requests, "observations": raw["observations"]}, renderer["test"], False,
                             (root / renderer["helper"]).read_text(),
                             {name: (root / path).read_text() for name, path in renderer["extra_sources"].items()})
    capture.validate_renderer(output, requests, rust)
    observation = {"pin": capture.pin(), "requests_sha256": capture.digest(request_path.read_bytes()),
                   "native": native, "raw": raw, "rust": rust}
    result = {**compare(observation, root), "observation": observation}
    (output / "comparison.json").write_bytes(capture.canonical(result) + b"\n")
    return result
