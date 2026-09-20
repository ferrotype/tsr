"""Phase 1 dispatcher.

A thin front end over the Phase 1 manifests and family adapters. It owns no
comparison algorithm of its own: baseline enumeration lives in
phase1_baselines.py, the operation inventory in phase1_scope.py, and capture /
comparison in phase1_capture.py, which reuses the existing Go overlay and
subprocess helpers rather than re-implementing them.

    python3 scripts/phase1.py inventory --check
    python3 scripts/phase1.py prepare --family FAMILY --output DIRECTORY
    python3 scripts/phase1.py freeze --from DIRECTORY
    python3 scripts/phase1.py map-baselines --output DIRECTORY [--write]
    python3 scripts/phase1.py capture --family FAMILY --output DIRECTORY [--case ID ...]
    python3 scripts/phase1.py compare --capture DIRECTORY [--require-parity]
    python3 scripts/phase1.py report --captures DIRECTORY ... --output FILE

Diagnostics go to stderr; stdout carries one JSON object so the producer
protocol stays machine readable.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import phase1_baselines as baselines  # noqa: E402
import phase1_capture as capture_module  # noqa: E402
import phase1_invocations as invocations  # noqa: E402
import phase1_scope as scope_module  # noqa: E402
from s08_oracle import ROOT  # noqa: E402

SCOPE = ROOT / "data/phase1/scope.json"
CASES = ROOT / "data/phase1/cases.json"
BASELINES = ROOT / "data/phase1/config-baselines.json"


def load(path: Path) -> dict:
    if not path.is_file():
        raise ValueError(f"missing Phase 1 manifest {path.relative_to(ROOT)}")
    return json.loads(path.read_text())


def preflight() -> list[str]:
    """Actionable environment checks. Never downloads or changes a toolchain."""
    problems = []
    if not (ROOT / "upstream/tsc").is_dir():
        problems.append(
            "the upstream submodule is not initialized; run `git submodule update --init upstream`"
        )
    if shutil.which("go") is None:
        problems.append(
            "go is not on PATH; export it from mise, for example "
            'export PATH="$(mise where go)/bin:$PATH"'
        )
    if shutil.which("cargo") is None:
        problems.append("cargo is not on PATH")
    return problems


def inventory_check() -> dict:
    problems = list(preflight())
    scope = load(SCOPE)
    cases = load(CASES)
    index = load(BASELINES)

    problems += scope_module.verify(scope)
    problems += baselines.verify(index)
    problems += baselines.verify_written_subfolders()

    pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
    for name, document in (("scope", scope), ("cases", cases), ("config-baselines", index)):
        if document.get("pin") != pin:
            problems.append(f"{name}.json records pin {document.get('pin')!r}, not {pin!r}")

    # Every case must name operations that exist in the frozen scope, and every
    # case id must be unique across families.
    known = {row["id"] for row in scope["operations"]}
    seen: set[str] = set()
    for case in cases.get("cases", []):
        if case["id"] in seen:
            problems.append(f"duplicate case id {case['id']}")
        seen.add(case["id"])
        for operation in case.get("operations", []):
            if operation not in known and not operation.startswith("phase1/"):
                problems.append(f"case {case['id']} covers unknown operation {operation}")

    blocked = [
        group
        for group, value in index["groups"].items()
        if value["authority"]["status"] != "established"
    ]
    mapping = index.get("invocation_mapping", {})
    verified = mapping.get("verified_outputs", 0)
    # F0's checklist requires a verified invocation mapping for *all* 309
    # outputs. Reporting readiness from the manifests alone would let a blocked
    # group pass unnoticed, so completion is computed and stated, never assumed.
    outstanding = []
    if problems:
        outstanding.append(f"{len(problems)} manifest problem(s)")
    if verified != index["total_outputs"]:
        outstanding.append(
            f"{index['total_outputs'] - verified} of {index['total_outputs']} reference outputs "
            "have no verified invocation mapping"
        )
    for group in blocked:
        outstanding.append(
            f"group {group} is blocked on {index['groups'][group]['authority'].get('blocker')}"
        )
    # Operations whose source file carries file-level producer metrics but which
    # have no operation-level witness. F0 requires exact case/artifact links for
    # anything it calls `covered`, so connecting the existing S04-S11 evidence to
    # operation ids is named here rather than assumed from the ledger.
    unlinked = sum(
        1
        for row in scope["operations"]
        if row["disposition"] == "implemented_untested" and row.get("ledger_verification")
    )
    if unlinked:
        outstanding.append(
            f"{unlinked} operations carry file-level producer metrics but no exact "
            "operation-level coverage link"
        )
    return {
        "pin": pin,
        "operations": scope["total_operations"],
        "dispositions": scope["counts"],
        "baseline_outputs": index["total_outputs"],
        "baseline_groups_blocked": blocked,
        "cases": len(cases.get("cases", [])),
        "families_declared": list(capture_module.DECLARED_FAMILIES),
        "families_prepared": sorted(capture_module.FAMILIES),
        "baseline_outputs_verified": verified,
        "problems": problems,
        "ok": not problems,
        "f0_complete": not outstanding,
        "f0_outstanding": outstanding,
    }


def prepare(family: str, output: Path) -> dict:
    """Export native inputs and observations into a staging directory."""
    provenance = capture_module.capture(family, output)
    return {"prepared": family, "output": str(output), "requests_sha256": provenance["requests_sha256"]}


def freeze(source: Path) -> dict:
    """Install reviewed native observations from a staging directory.

    Never blesses Rust output as expected truth: only the native observation and
    its provenance are installed.
    """
    source = Path(source).resolve()
    # Validate contents, not just bytes. Authentication alone would accept a
    # correctly hashed capture containing a `harness_failed` row, which means
    # the observation never happened. Rust parity is not required:
    # `not_implemented` is the expected preparation-time result.
    provenance, _requests, _native, _rust = capture_module.validate_capture(source)
    if provenance["partial"]:
        raise ValueError("a partial capture cannot be frozen as a family inventory")
    family = provenance["family"]
    destination = ROOT / "data/phase1/native" / family

    # Stage the whole tree, then swap. A rejected or failed freeze must leave
    # the existing frozen inventory exactly as it was.
    staging = destination.with_name(destination.name + ".incoming")
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir(parents=True)

    # One directory per native probe, mirroring the capture layout. A family
    # may be served by several probes, so a single observations.json is not the
    # shape on disk.
    installed = {}
    try:
        for package, probe in sorted(provenance["native_probes"].items()):
            probe_source = source / probe["directory"]
            probe_destination = staging / probe["directory"].removeprefix("native/")
            probe_destination.mkdir(parents=True, exist_ok=True)
            for name in ("observations.json", "provenance.json"):
                shutil.copyfile(probe_source / name, probe_destination / name)
            installed[package] = str(probe_destination.relative_to(staging))
        (staging / "capture-provenance.json").write_text(
            json.dumps(provenance, indent=2, sort_keys=True) + "\n"
        )
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise

    retired = destination.with_name(destination.name + ".outgoing")
    if retired.exists():
        shutil.rmtree(retired)
    if destination.exists():
        destination.rename(retired)
    staging.rename(destination)
    shutil.rmtree(retired, ignore_errors=True)
    return {
        "frozen": family,
        "destination": str(destination.relative_to(ROOT)),
        "probes": installed,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    check = commands.add_parser("inventory")
    check.add_argument("--check", action="store_true", required=True)

    prepare_parser = commands.add_parser("prepare")
    prepare_parser.add_argument("--family", required=True)
    prepare_parser.add_argument("--output", type=Path, required=True)

    freeze_parser = commands.add_parser("freeze")
    freeze_parser.add_argument("--from", dest="source", type=Path, required=True)

    capture_parser = commands.add_parser("capture")
    capture_parser.add_argument("--family", required=True)
    capture_parser.add_argument("--output", type=Path, required=True)
    capture_parser.add_argument("--case", action="append", default=[])

    compare_parser = commands.add_parser("compare")
    compare_parser.add_argument("--capture", type=Path, required=True)
    compare_parser.add_argument("--require-parity", action="store_true")

    map_parser = commands.add_parser("map-baselines")
    map_parser.add_argument("--output", type=Path, required=True,
                            help="staging directory for the instrumented native run")
    map_parser.add_argument("--write", action="store_true",
                            help="install the attributed index into data/phase1/")

    report_parser = commands.add_parser("report")
    report_parser.add_argument("--captures", type=Path, nargs="+", required=True)
    report_parser.add_argument("--output", type=Path, required=True)
    report_parser.add_argument("--require-complete", action="store_true")

    args = parser.parse_args()
    if args.command == "inventory":
        result = inventory_check()
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0 if result["ok"] else 1
    if args.command == "prepare":
        result = prepare(args.family, args.output)
    elif args.command == "freeze":
        result = freeze(args.source)
    elif args.command == "capture":
        result = capture_module.capture(args.family, args.output, args.case or None)
    elif args.command == "map-baselines":
        rows = invocations.record(args.output)
        index = invocations.attribute(rows, baselines.build())
        if args.write:
            BASELINES.write_text(json.dumps(index, indent=2, sort_keys=True) + "\n")
        result = index["invocation_mapping"]
    elif args.command == "compare":
        result = capture_module.compare(args.capture, args.require_parity)
    else:
        reports = [capture_module.compare(directory) for directory in args.captures]
        result = capture_module.join(reports, args.require_complete)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
        print(f"phase1 failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
