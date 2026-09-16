#!/usr/bin/env python3
"""S08 P7 census validation (data/s08/type-footprint.json): the Go and Rust structural
census adapters over small fixture programs that exercise the accounting rules, and a
bounded check over acceptance variants before the full checkerbench capture.

  fixtures   build both runners, census every fixture program on both runtimes, validate
             the per-runtime invariants and report the paired counts
  workload   run the allocation checkerbench children on the first N acceptance variants
             and aggregate their census results on both runtimes
"""
import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

from s04 import go_environment, verified_upstream
from s04_common import command, strict_json_loads
from s08_oracle import ROOT, canonical, digest
import s08_checkerbench
import s08_measurement as measurement
from s08_census_runtime import runtime_overlay

DEFAULT = ROOT / "target/s08/census"
FIXTURES = ROOT / "tools/s08/p7/census-fixtures.json"
FAMILIES_DIR = ROOT / "tools/s08/oracle/families"
TYPE_FAMILIES = ("type_records", "intrinsic", "literal", "unique_es_symbol", "anonymous", "evolving_arrays", "reference",
                 "interface", "tuple", "union", "intersection", "type_parameter", "template_literal", "mapped",
                 "reverse_mapped", "instantiation_expression", "index", "indexed_access", "string_mapping",
                 "substitution", "conditional", "alias", "type_lists", "type_caches")


def requests_from_fixtures():
    fixtures = strict_json_loads(FIXTURES.read_bytes())
    requests = []
    for case in fixtures["cases"]:
        requests.append({"id": case["id"], "source_hex": case["source"].encode().hex(),
                         "files": {path: text.encode().hex() for path, text in case.get("files", {}).items()},
                         "allow_js": bool(case.get("allow_js", False)), "roots": case["roots"], "rule": case["rule"]})
    return fixtures, requests


def build_rust(directory):
    env = s08_checkerbench.native_environment()
    manifest = ROOT / "crates/ts_compiler/Cargo.toml"
    messages = command(["cargo", "build", "--release", "--locked", "--example", "p7_census", "--features", "s08-allocation",
                        "--message-format=json", "--manifest-path", str(manifest)], cwd=ROOT, env=env).decode()
    binary = s08_checkerbench.cargo_executable(messages, manifest, "p7_census", "example", ["s08-allocation"])
    target = directory / "bin" / "p7_census"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, target)
    return target


def run_go_fixtures(directory, requests_path, output):
    upstream = verified_upstream()
    env = go_environment()
    overlay = {}
    for name in ("bridge.go", "census_v2.go", "census_allocations.go", "census_v2_test.go", "program_census_test.go"):
        source = FAMILIES_DIR / name
        virtual = upstream / "tsc/internal/checker" / ("s08_families_" + name)
        if virtual.exists():
            raise ValueError(f"overlay would replace a source file: {virtual}")
        path = directory / "overlay" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(source.read_bytes())
        overlay[str(virtual)] = str(path)
    runtime_overlay(directory, overlay, upstream, env)
    (directory / "overlay.json").write_bytes(canonical({"Replace": overlay}) + b"\n")
    env.update(S08_REQUESTS=str(requests_path), S08_OUTPUT=str(output))
    args = ["go", "test", "-mod=readonly", "-trimpath", "-overlay", str(directory / "overlay.json"), "./internal/checker",
            "-run", "^TestS08ProgramCensus$|^TestS08MapBytes|^TestS08Arena|^TestS08Census", "-v", "-count=1", "-timeout=10m"]
    (directory / "go-command.json").write_bytes(canonical(args) + b"\n")
    with (directory / "go.stdout").open("wb") as stdout, (directory / "go.stderr").open("wb") as stderr:
        completed = subprocess.run(args, cwd=upstream / "tsc", env=env, stdout=stdout, stderr=stderr, timeout=900, check=False)
    if completed.returncode != 0 or not output.exists():
        raise ValueError(f"Go census fixtures failed (exit {completed.returncode}); see {directory / 'go.stdout'}")
    return strict_json_loads(output.read_bytes())


def invariants(row, runtime, case):
    """Per-runtime rules every fixture census must satisfy."""
    problems = []
    if row.get("state", "executed") != "executed":
        return [f"{runtime}: not executed: {row.get('panic') or row.get('state')}"]
    census = row["census"]
    if census.get("unavailable"):
        problems.append(f"{runtime}: unavailable families {census['unavailable']}")
    types = census["types"]
    if types["reachable"] > types["created"]:
        problems.append(f"{runtime}: reachable {types['reachable']} exceeds created {types['created']}")
    if types["unreachable_occupied"] != types["created"] - types["reachable"]:
        problems.append(f"{runtime}: unreachable_occupied {types['unreachable_occupied']} != created - reachable")
    if row["roots"] != len(case["roots"]):
        problems.append(f"{runtime}: {row['roots']} roots resolved, {len(case['roots'])} requested")
    families = census["families"]
    for name in TYPE_FAMILIES:
        if name not in families:
            problems.append(f"{runtime}: type family {name} missing")
    negative = [name for name, family in families.items() if family["bytes"] < 0 or family["count"] < 0]
    if negative:
        problems.append(f"{runtime}: negative families {negative}")
    if census["type_storage_bytes"] != sum(families[name]["bytes"] for name in TYPE_FAMILIES if name in families):
        problems.append(f"{runtime}: type_storage_bytes is not the type-family sum")
    if census["checker_bytes"] != sum(family["bytes"] for family in families.values()):
        problems.append(f"{runtime}: checker_bytes is not the family sum")
    bound = census["bound_inputs"]
    if bound["backings"] < 1 or bound["bytes"] <= 0:
        problems.append(f"{runtime}: no bound-input backings recorded")
    if bound["referenced_backings"] > bound["backings"] or (bound["references"] == 0) != (bound["referenced_backings"] == 0):
        problems.append(f"{runtime}: inconsistent bound-input references {bound}")
    if case["id"] == "bound-inputs" and runtime == "go" and bound["references"] < 1:
        # Go literal values and symbol names alias the source text; Rust copies literal
        # texts into checker-owned allocations, which the literal family charges instead.
        problems.append(f"{runtime}: the bound-inputs fixture referenced no source text")
    return problems


def fixtures(directory):
    directory.mkdir(parents=True, exist_ok=True)
    fixture_file, requests = requests_from_fixtures()
    requests_path = directory / "requests.json"
    requests_path.write_bytes(canonical(requests) + b"\n")
    rust_binary = build_rust(directory)
    rust_output = directory / "rust.json"
    command([str(rust_binary), str(requests_path), str(rust_output)], cwd=ROOT, env=s08_checkerbench.native_environment())
    rust = strict_json_loads(rust_output.read_bytes())
    go = run_go_fixtures(directory, requests_path, directory / "go.json")
    if [r["id"] for r in rust["rows"]] != [c["id"] for c in fixture_file["cases"]] or [r["id"] for r in go["rows"]] != [c["id"] for c in fixture_file["cases"]]:
        raise ValueError("fixture inventory drift")
    cases = []
    all_problems = []
    for case, rust_row, go_row in zip(fixture_file["cases"], rust["rows"], go["rows"], strict=True):
        problems = invariants(rust_row, "rust", case) + invariants(go_row, "go", case)
        problems.extend(paired_inventory(rust_row["census"], go_row["census"]))
        all_problems.extend(f"{case['id']}: {p}" for p in problems)
        counts = {}
        for name in TYPE_FAMILIES:
            r = rust_row["census"]["families"].get(name, {"count": None, "bytes": None})
            g = go_row["census"]["families"].get(name, {"count": None, "bytes": None})
            counts[name] = {"rust": r, "go": g, "count_equal": r["count"] == g["count"]}
        cases.append({"id": case["id"], "rule": case["rule"], "problems": problems,
                      "types": {"rust": rust_row["census"]["types"], "go": go_row["census"]["types"]},
                      "created": {"rust": {"types": rust_row["types_created"], "symbols": rust_row["symbols_created"], "signatures": rust_row["signatures_created"]},
                                  "go": {"types": go_row["types_created"], "symbols": go_row["symbols_created"], "signatures": go_row["signatures_created"]}},
                      "diagnostics": {"rust": rust_row["diagnostics"], "go": go_row["diagnostics"]},
                      "bound_inputs": {"rust": rust_row["census"]["bound_inputs"], "go": go_row["census"]["bound_inputs"]},
                      "type_storage_bytes": {"rust": rust_row["census"]["type_storage_bytes"], "go": go_row["census"]["type_storage_bytes"]},
                      "checker_bytes": {"rust": rust_row["census"]["checker_bytes"], "go": go_row["census"]["checker_bytes"]},
                      "unavailable": {"rust": rust_row["census"]["unavailable"], "go": go_row["census"]["unavailable"]},
                      "type_families": counts,
                      "families": {"rust": rust_row["census"]["families"], "go": go_row["census"]["families"]}})
    rust_families = set(rust["rows"][0]["census"]["families"])
    go_families = set(go["rows"][0]["census"]["families"])
    report = {"version": 1, "fixtures_sha256": digest(FIXTURES.read_bytes()), "requests_sha256": digest(requests_path.read_bytes()),
              "rust_binary_sha256": digest(rust_binary.read_bytes()), "sources_sha256": s08_checkerbench.fingerprint(s08_checkerbench.sources()),
              "pass": not all_problems, "problems": all_problems, "cases": cases,
              "family_inventory": {"rust_only": sorted(rust_families - go_families), "go_only": sorted(go_families - rust_families),
                                   "shared": len(rust_families & go_families)}}
    (directory / "report.json").write_bytes(canonical(report) + b"\n")
    return report


def paired_inventory(rust, go):
    rust_families, go_families = set(rust["families"]), set(go["families"])
    if rust_families != go_families:
        return [f"family inventory differs: Rust-only {sorted(rust_families - go_families)}, Go-only {sorted(go_families - rust_families)}"]
    return []


def validate_workload(rows, ids):
    """A census comparison still needs identical successful semantic work."""
    for runtime in ("rust", "go"):
        totals = measurement.checker_rows(rows[runtime], ids, "alloc")
        measurement.check_totals(rows[runtime + "_totals"], totals)
    for rust, go in zip(rows["rust"], rows["go"], strict=True):
        for field in ("output_sha256", "actions"):
            if rust[field] != go[field]:
                raise ValueError(f"{rust['id']}: cross-runtime {field} differs; footprint is not comparable")
        r, g = rust["checkpoint"]["census"], go["checkpoint"]["census"]
        if r.get("state") != "failed" and g.get("state") != "failed":
            problems = paired_inventory(r, g)
            if problems:
                raise ValueError(f"{rust['id']}: {problems[0]}")


def workload(directory, variants):
    """The allocation executables of both runtimes over the first `variants` acceptance variants."""
    directory.mkdir(parents=True, exist_ok=True)
    build_report = s08_checkerbench.build(directory, modes=("alloc",))
    requests = s08_checkerbench.prepare_requests(directory, variants)
    rows = {}
    for runtime in ("rust", "go"):
        sample = s08_checkerbench.sample(directory, build_report, runtime, "alloc", "workload")
        rows[runtime] = s08_checkerbench.rows_of(directory, sample)
        rows[runtime + "_totals"] = sample["totals"]
    ids = [row["id"] for row in strict_json_loads((directory / "rust-requests.json").read_bytes())]
    validate_workload(rows, ids)
    per_variant = []
    unavailable = {"rust": 0, "go": 0}
    failed = {"rust": 0, "go": 0}
    sums = {runtime: {"type_storage_bytes": 0, "checker_bytes": 0, "reachable": 0, "created": 0, "bound_referenced_bytes": 0} for runtime in ("rust", "go")}
    family_sums = {runtime: {} for runtime in ("rust", "go")}
    for rust_row, go_row in zip(rows["rust"], rows["go"], strict=True):
        entry = {"id": rust_row["id"]}
        for runtime, row in (("rust", rust_row), ("go", go_row)):
            census = row["checkpoint"]["census"]
            if census.get("state") == "failed":
                failed[runtime] += 1
                entry[runtime] = {"failed": census.get("reason")}
                continue
            if census["unavailable"]:
                unavailable[runtime] += 1
            entry[runtime] = {"types": census["types"], "type_storage_bytes": census["type_storage_bytes"],
                              "checker_bytes": census["checker_bytes"], "unavailable": census["unavailable"],
                              "bound_inputs": census["bound_inputs"]}
            sums[runtime]["type_storage_bytes"] += census["type_storage_bytes"]
            sums[runtime]["checker_bytes"] += census["checker_bytes"]
            sums[runtime]["reachable"] += census["types"]["reachable"]
            sums[runtime]["created"] += census["types"]["created"]
            sums[runtime]["bound_referenced_bytes"] += census["bound_inputs"]["referenced_bytes"]
            for name, family in census["families"].items():
                slot = family_sums[runtime].setdefault(name, {"count": 0, "bytes": 0})
                slot["count"] += family["count"]
                slot["bytes"] += family["bytes"]
        entry["reachable_equal"] = ("types" in entry.get("rust", {}) and "types" in entry.get("go", {})
                                    and entry["rust"]["types"]["reachable"] == entry["go"]["types"]["reachable"])
        per_variant.append(entry)
    valid = not failed["rust"] and not failed["go"] and not unavailable["rust"] and not unavailable["go"]
    means = {runtime: (sums[runtime]["type_storage_bytes"] / sums[runtime]["reachable"]) if sums[runtime]["reachable"] else None for runtime in ("rust", "go")}
    ratio = (means["rust"] / means["go"]) if valid and means["go"] and means["rust"] is not None else None
    report = {"version": 1, "variants": len(ids), "valid": valid, "failed": failed, "unavailable_variants": unavailable,
              "sums": sums, "mean_bytes_per_reachable_type": means, "type_footprint_ratio": ratio,
              "reachable_equal_variants": sum(1 for e in per_variant if e["reachable_equal"]),
              "family_sums": family_sums, "per_variant": per_variant,
              "scope": "bounded workload check of the census adapters; not the frozen measurement"}
    (directory / "workload-report.json").write_bytes(canonical(report) + b"\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("fixtures", "workload"))
    parser.add_argument("--output", type=Path, default=DEFAULT)
    parser.add_argument("--variants", type=int, default=100)
    args = parser.parse_args()
    directory = args.output.resolve()
    if args.command == "fixtures":
        report = fixtures(directory / "fixtures")
        print(json.dumps({"pass": report["pass"], "problems": report["problems"], "family_inventory": report["family_inventory"],
                          "cases": [{"id": c["id"], "types": c["types"], "type_storage_bytes": c["type_storage_bytes"]} for c in report["cases"]]}, indent=1))
    else:
        report = workload(directory / "workload", args.variants)
        print(json.dumps({k: report[k] for k in ("variants", "valid", "failed", "unavailable_variants", "sums", "mean_bytes_per_reachable_type", "type_footprint_ratio", "reachable_equal_variants")}, indent=1))
    return 0 if report.get("pass", report.get("valid", False)) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
