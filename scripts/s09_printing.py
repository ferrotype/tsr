#!/usr/bin/env python3
"""Freeze a bounded pinned-Go decode/print observation; not an E3 producer."""

import argparse
import json
from pathlib import Path
import sys
import tomllib

from s04_common import strict_json_loads
from s08_oracle import ROOT, canonical, digest, run_overlay

CASES = ROOT / "data/s09/printing-cases.json"
OBSERVATIONS = ROOT / "data/s09/printing-observations.json"
SOURCE = ROOT / "tools/s09/printing_test.go"
OPTIONS = ("never_ascii_escape", "preserve_source_newlines", "terminate_unterminated_literals")


def validate_request(request, pin):
    if request["version"] != 1 or request["pin"] != pin:
        raise ValueError("printing requests target a different upstream pin")
    cases = request["cases"]
    names = [case["name"] for case in cases]
    if not names or any(not isinstance(name, str) or not name for name in names):
        raise ValueError("printing requests require nonempty names")
    if len(set(names)) != len(names):
        raise ValueError("duplicate printing request name")
    for case in cases:
        if ("tree" in case) == ("wire_hex" in case):
            raise ValueError("printing request requires exactly one tree or wire payload")
        if case["rust_expected"] not in ("text", "unsupported", "decode_error", "panic"):
            raise ValueError("unrecognized Rust boundary classification")
        for key, value in case["options"].items():
            if key not in OPTIONS or type(value) is not bool:
                raise ValueError("invalid API printing option")
        if (case["rust_expected"] == "unsupported") != ("rust_unsupported" in case):
            raise ValueError("unsupported request requires its exact Rust reason")
        if (case["rust_expected"] == "panic") != ("expected_panic" in case):
            raise ValueError("panic request requires its native contract message")
        if (case["rust_expected"] == "panic") != ("panic_class" in case):
            raise ValueError("panic request requires its narrow class")
        if "panic_class" in case and case["panic_class"] not in ("nil-root", "synthetic-expression"):
            raise ValueError("unknown native panic class")
    return cases


def validate_rows(cases, rows):
    names = [case["name"] for case in cases]
    if [row["name"] for row in rows] != names:
        raise ValueError("missing, extra, duplicate or reordered printing observation")
    for case, row in zip(cases, rows):
        bytes.fromhex(row["encoded_hex"])
        outcomes = [key for key in ("text_hex", "decode_error", "panic") if key in row]
        if len(outcomes) != 1:
            raise ValueError(f"{row['name']} requires exactly one observed outcome")
        allowed = {"name", "encoded_hex", "options", outcomes[0]}
        if "panic_class" in case:
            allowed.add("panic_class")
        if "rust_unsupported" in case:
            allowed.add("rust_unsupported")
        if set(row) != allowed or set(row["options"]) != set(OPTIONS):
            raise ValueError(f"{row['name']} has missing or unknown observation fields")
        if case["rust_expected"] == "panic":
            if row.get("panic") != case["expected_panic"]:
                raise ValueError(f"wrong native panic in {row['name']}: {row.get('panic')}")
            if row.get("panic_class") != case["panic_class"]:
                raise ValueError(f"wrong native panic class in {row['name']}")
        elif "panic" in row:
            raise ValueError(f"unexpected native panic in {row['name']}: {row['panic']}")
        elif case["rust_expected"] == "decode_error":
            if not isinstance(row.get("decode_error"), str) or not row["decode_error"]:
                raise ValueError(f"{row['name']} did not observe the requested decode failure")
        elif "text_hex" not in row:
            raise ValueError(f"{row['name']} did not produce Go text")
        else:
            bytes.fromhex(row["text_hex"])
        if "wire_hex" in case and row["encoded_hex"] != case["wire_hex"]:
            raise ValueError(f"{row['name']} observed another wire payload")
        if row.get("rust_unsupported") != case.get("rust_unsupported"):
            raise ValueError(f"{row['name']} observed another Rust boundary classification")
        if case["rust_expected"] != "panic" and "panic_class" in row:
            raise ValueError(f"unexpected panic class in {row['name']}")
        for key in OPTIONS:
            if row["options"].get(key) is not case["options"].get(key, False):
                raise ValueError(f"{row['name']} observed another printer option")


def input_hashes(root, request):
    return {
        "cases_sha256": digest(canonical(request)),
        "source_sha256": digest((root / "tools/s09/printing_test.go").read_bytes()),
        "capture_script_sha256": digest((root / "scripts/s09_printing.py").read_bytes()),
        "overlay_helper_sha256": digest((root / "scripts/s08_oracle.py").read_bytes()),
        "environment_helper_sha256": digest((root / "scripts/s04.py").read_bytes()),
        "protocol_helper_sha256": digest((root / "scripts/s04_common.py").read_bytes()),
    }


def verify_frozen(root=ROOT):
    """Validate frozen requests, native observations and input closure offline."""
    try:
        return _verify_frozen(Path(root))
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError(f"malformed printing fixture: {error}") from error


def _verify_frozen(root):
    root = Path(root)
    request = strict_json_loads((root / "data/s09/printing-cases.json").read_bytes())
    observation = strict_json_loads((root / "data/s09/printing-observations.json").read_bytes())
    pin = strict_json_loads((root / "data/upstream.json").read_bytes())["pin"]
    cases = validate_request(request, pin)
    if observation["version"] != 1 or observation["pin"] != pin:
        raise ValueError("printing observations target a different upstream pin")
    for key, expected in input_hashes(root, request).items():
        if observation[key] != expected:
            raise ValueError(f"stale printing observation: {key}")
    go = tomllib.loads((root / "data/s04/toolchains.toml").read_text())["go"]
    if observation["go"] != go or observation["toolchain_local"] is not True:
        raise ValueError("printing observations used a different Go toolchain")
    validate_rows(cases, observation["rows"])
    return observation


def capture(directory):
    request = strict_json_loads(CASES.read_bytes())
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    cases = validate_request(request, pin)
    hashes = input_hashes(ROOT, request)
    report = run_overlay(Path(directory) / "go-printing", "printer", SOURCE.read_text(),
                         {"cases": cases}, "TestS09DecodeAndPrint")
    rows = report["rows"]
    validate_rows(cases, rows)
    if hashes != input_hashes(ROOT, strict_json_loads(CASES.read_bytes())):
        raise ValueError("printing inputs changed during native capture")
    provenance = strict_json_loads((Path(directory) / "go-printing/provenance.json").read_bytes())
    observation = {
        "version": 1, "pin": pin,
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"],
        **hashes,
        "toolchain_local": provenance["toolchain_local"],
        "scope": report["scope"],
        "derivation": "python3 scripts/s09_printing.py --output <new directory> [--freeze]; "
                      "access-only go test -overlay in internal/printer with the pinned local Go toolchain",
        "rows": rows,
    }
    (Path(directory) / "printing-observations.candidate.json").write_bytes(canonical(observation) + b"\n")
    return observation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--freeze", action="store_true")
    args = parser.parse_args()
    observation = capture(args.output)
    content = canonical(observation) + b"\n"
    if args.freeze:
        OBSERVATIONS.write_bytes(content)
    elif not OBSERVATIONS.exists() or OBSERVATIONS.read_bytes() != content:
        raise ValueError("printing observations differ from the frozen fixture; inspect the candidate")
    verify_frozen()
    print(json.dumps({"cases": len(observation["rows"]), "frozen": args.freeze}, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
        print(f"S09 printing capture failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
