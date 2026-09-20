"""Phase 1 capture, comparison and reporting.

`capture` runs the native and Rust children and stores their exact bytes with a
provenance record. `compare` re-reads a stored capture and never runs a child,
so a replay is read-only. Neither step may change the reviewed inventory.

The result vocabulary is the plan's: match, different, not_implemented,
native_unavailable, harness_failed and not_run. Only `match` is feature parity;
`harness_failed` invalidates a capture rather than counting as a non-match.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04_common import command, strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest, run_overlay  # noqa: E402

RESULTS = ("match", "different", "not_implemented", "native_unavailable", "harness_failed", "not_run")
PARITY_RESULTS = ("match",)

FAMILIES = {
    "pilot": {
        "requests": "data/phase1/requests/pilot.json",
        "native_package": "vfs/vfsmatch",
        "native_probe": "tools/phase1/pilot/vfsmatch_probe_test.go",
        "native_test": "TestPhase1PilotReadDirectory",
        "rust_package": "tsr_tsoptions",
        "rust_example": "phase1_pilot",
        "rust_driver": "tools/phase1/pilot/rust_observation.rs",
    },
}
# The six command families the plan names. Only `pilot` is wired at F0; the
# rest are registered so `inventory --check` can report them as unprepared
# rather than silently omitting them.
DECLARED_FAMILIES = ("leaves", "filesystem", "config", "syntax", "utilities", "integration")


def sha_file(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_closure(family: str) -> dict[str, str]:
    """Every tracked input whose change invalidates this family's capture.

    Collected recursively so a file under a nested adapter directory is not
    silently excluded by a shallow glob.
    """
    spec = FAMILIES[family]
    paths = [Path(spec["requests"]), Path(spec["native_probe"]), Path(spec["rust_driver"])]
    for directory in (Path("tools/phase1") / family,):
        if (ROOT / directory).is_dir():
            paths.extend(
                p.relative_to(ROOT) for p in sorted((ROOT / directory).rglob("*")) if p.is_file()
            )
    paths.append(Path("scripts/phase1_capture.py"))
    paths.append(Path(".cargo/config.toml"))
    closure = {}
    for rel in sorted({str(p) for p in paths}):
        path = ROOT / rel
        if path.is_file():
            closure[rel] = sha_file(path)
    return closure


def build_rust(family: str) -> Path:
    spec = FAMILIES[family]
    messages = command(
        [
            "cargo", "build", "--locked", "--offline", "-p", spec["rust_package"],
            "--example", spec["rust_example"], "--message-format=json",
        ],
        cwd=ROOT,
    )
    executable = None
    for line in messages.splitlines():
        if not line.strip():
            continue
        record = json.loads(line)
        if (
            record.get("reason") == "compiler-artifact"
            and record.get("target", {}).get("name") == spec["rust_example"]
            and record.get("executable")
        ):
            executable = record["executable"]
    if executable is None:
        raise ValueError(f"cargo reported no executable for the {family} driver")
    return Path(executable)


def capture(family: str, output: Path, cases: list[str] | None = None) -> dict:
    if family not in FAMILIES:
        raise ValueError(
            f"family {family!r} has no adapter yet; declared families are "
            + ", ".join(DECLARED_FAMILIES)
        )
    spec = FAMILIES[family]
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)

    before = source_closure(family)
    document = strict_json_loads((ROOT / spec["requests"]).read_bytes())
    selected = document["requests"]
    partial = False
    if cases:
        wanted = set(cases)
        unknown = sorted(wanted - {r["case"] for r in selected})
        if unknown:
            raise ValueError("unknown case ids: " + ", ".join(unknown))
        selected = [r for r in selected if r["case"] in wanted]
        partial = len(selected) != len(document["requests"])

    # The children read exactly these bytes; hash the serialized request, not an
    # earlier in-memory object.
    request_document = {
        "version": document["version"],
        "family": family,
        "requests": selected,
    }
    request_bytes = canonical(request_document) + b"\n"
    request_path = output / "requests.json"
    request_path.write_bytes(request_bytes)

    native = run_overlay(
        output / "native",
        spec["native_package"],
        (ROOT / spec["native_probe"]).read_text(),
        request_document,
        spec["native_test"],
    )

    executable = build_rust(family)
    rust_path = output / "rust-observations.json"
    command([str(executable), str(request_path), str(rust_path)], cwd=ROOT)

    after = source_closure(family)
    if before != after:
        raise ValueError("a source input changed while capturing; the capture is invalid")

    provenance = {
        "version": 1,
        "family": family,
        "pin": strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"],
        "partial": partial,
        "selected_cases": sorted(r["case"] for r in selected),
        "requests_sha256": digest(request_bytes),
        "native_observations_sha256": sha_file(output / "native/observations.json"),
        "rust_observations_sha256": sha_file(rust_path),
        "rust_binary_sha256": sha_file(executable),
        "source_closure": before,
        "host": {"platform": platform.platform(), "python": platform.python_version()},
        "go": native.get("go"),
        "goos": native.get("goos"),
        "goarch": native.get("goarch"),
    }
    (output / "provenance.json").write_bytes(canonical(provenance) + b"\n")
    return provenance


def _authenticate(directory: Path) -> dict:
    provenance = strict_json_loads((directory / "provenance.json").read_bytes())
    checks = {
        "requests.json": provenance["requests_sha256"],
        "native/observations.json": provenance["native_observations_sha256"],
        "rust-observations.json": provenance["rust_observations_sha256"],
    }
    for name, expected in checks.items():
        actual = sha_file(directory / name)
        if actual != expected:
            raise ValueError(f"capture artifact {name} does not match its recorded hash")
    for rel, expected in provenance["source_closure"].items():
        path = ROOT / rel
        if not path.is_file():
            raise ValueError(f"capture input {rel} no longer exists")
        if sha_file(path) != expected:
            raise ValueError(f"capture input {rel} changed after the capture")
    return provenance


def compare(directory: Path, require_parity: bool = False) -> dict:
    """Compare stored outputs. Runs no child process."""
    directory = Path(directory).resolve()
    provenance = _authenticate(directory)
    requests = strict_json_loads((directory / "requests.json").read_bytes())["requests"]
    native_rows = {
        row["case"]: row
        for row in strict_json_loads((directory / "native/observations.json").read_bytes())[
            "observations"
        ]
    }
    rust_rows = {
        row["case"]: row
        for row in strict_json_loads((directory / "rust-observations.json").read_bytes())[
            "observations"
        ]
    }
    all_cases = [r["case"] for r in strict_json_loads((ROOT / FAMILIES[provenance["family"]]["requests"]).read_bytes())["requests"]]
    selected = {r["case"] for r in requests}

    rows = []
    for case in all_cases:
        if case not in selected:
            rows.append({"case": case, "result": "not_run", "reason": "outside the captured selection"})
            continue
        native = native_rows.get(case)
        rust = rust_rows.get(case)
        if native is None or rust is None:
            rows.append({"case": case, "result": "harness_failed",
                         "reason": "a child produced no row for a selected case"})
            continue
        if rust["result"] == "harness_failed":
            rows.append({"case": case, "result": "harness_failed", "reason": rust.get("error", "")})
            continue
        if rust["result"] == "not_implemented":
            rows.append({"case": case, "result": "not_implemented",
                         "missing_operation": rust.get("missing_operation"),
                         "native_result": native.get("result"),
                         "native_observation": native.get("observation")})
            continue
        if native["result"] == "native_unavailable":
            rows.append({"case": case, "result": "native_unavailable", "reason": native.get("reason", "")})
            continue
        if native["result"] != "observed":
            rows.append({"case": case, "result": "harness_failed",
                         "reason": f"unexpected native result {native['result']!r}"})
            continue
        same = canonical(native.get("observation")) == canonical(rust.get("observation"))
        rows.append({
            "case": case,
            "result": "match" if same else "different",
            **({} if same else {"native": native.get("observation"), "rust": rust.get("observation")}),
        })

    counts = {result: 0 for result in RESULTS}
    for row in rows:
        counts[row["result"]] += 1
    report = {
        "version": 1,
        "family": provenance["family"],
        "pin": provenance["pin"],
        "partial": provenance["partial"],
        "capture": str(directory),
        "requests_sha256": provenance["requests_sha256"],
        "counts": counts,
        "rows": rows,
    }
    required_non_match = counts["different"] + counts["not_implemented"] + counts["native_unavailable"]
    report["parity"] = counts["match"] / len(rows) if rows else 0.0
    report["required_non_match"] = required_non_match
    if counts["harness_failed"]:
        raise ValueError(
            f"{counts['harness_failed']} case(s) failed in the harness; the capture is invalid"
        )
    if require_parity and (required_non_match or counts["not_run"]):
        raise ValueError(
            "parity required but the report has "
            f"{required_non_match} non-matching and {counts['not_run']} unrun case(s)"
        )
    return report


def join(reports: list[dict], selection_required: bool = False) -> dict:
    """Join verified reports by case id, rejecting duplicates and conflicts."""
    rows: dict[str, dict] = {}
    families: dict[str, str] = {}
    for report in reports:
        family = report["family"]
        if family in families and families[family] != report["requests_sha256"]:
            raise ValueError(f"reports for family {family} use different request schedules")
        families[family] = report["requests_sha256"]
        for row in report["rows"]:
            case = row["case"]
            if case in rows and rows[case]["result"] != row["result"]:
                if rows[case]["result"] == "not_run":
                    rows[case] = row
                    continue
                if row["result"] == "not_run":
                    continue
                raise ValueError(f"case {case} reported twice with different results")
            rows.setdefault(case, row)
    counts = {result: 0 for result in RESULTS}
    for row in rows.values():
        counts[row["result"]] += 1
    if selection_required and counts["not_run"]:
        raise ValueError(f"full-family acceptance rejects {counts['not_run']} unrun case(s)")
    return {
        "version": 1,
        "families": sorted(families),
        "counts": counts,
        "total": len(rows),
        "rows": [rows[case] for case in sorted(rows)],
    }
