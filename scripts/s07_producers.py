#!/usr/bin/env python3
"""Scoped S07 evidence. Complete failed measurements remain false gates."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from s04_common import command, strict_json_loads
from s06_protocol import canonical

ROOT = Path(__file__).resolve().parents[1]

# Each name is an executed adversarial observation in s07_binder_tests.run.
# Counts alone would accept a replaced or omitted graph-contract obligation.
GRAPH_CONTRACT_TESTS = {
    "deleted flow edge", "changed graph alias target", "changed declaration identity",
    "changed backing alias", "reordered declarations", "dropped required variant",
    "manufactured matching panic", "bounded identity-number substitution",
    "identity target retained", "literal name exactness", "diagnostic argument exactness",
}


def binder_contract_metrics(documents, report, helpers, protocol):
    from s07_helpers import load_manifest
    manifest, _ = load_manifest()
    resolver = next(group for group in manifest['groups'] if group['name'] == 'resolver')
    observed = [row for row in helpers['results'] if row['group'] == 'resolver']
    resolvers = ([row['test'] for row in observed] == resolver['tests']
                 and all(row['pass'] is True for row in observed))
    primary = documents['binder-cases.json']
    supplemental = documents['binder-supplemental.json']['requests']
    names = protocol['passed']
    protocol_complete = (type(protocol['tests']) is int and protocol['tests'] > 0
                         and protocol['tests'] == len(names) == len(set(names)))
    graph_contracts = (
        bool(primary) and set(report['tests']) == set(primary)
        and all(value is True for value in report['tests'].values())
        and bool(supplemental)
        and report['supplemental']['requests'] == len(supplemental)
        and report['supplemental']['passed'] == len(supplemental)
        and protocol_complete and GRAPH_CONTRACT_TESTS <= set(names))
    return {'resolvers': resolvers, 'graph_contracts': graph_contracts}


def binder():
    from s07_binder import rust_binary
    from s07_binder_corpus import freeze, capture
    from s07_binder_tests import run
    from s07_helpers import measure
    from s07_depth import capture as capture_depth, inputs as depth_inputs
    directory = ROOT / "target/s07-binder-reports"
    directory.mkdir(parents=True, exist_ok=True)
    before = depth_inputs()
    documents, requests, changes, _ = freeze(False)
    if changes:
        raise ValueError("binder obligations changed")
    oracle = ROOT / "target/s07-oracle/go-binder"
    rust = rust_binary()
    helpers = measure()
    protocol = run(oracle, rust, directory / "protocol")
    report = capture(documents, requests, oracle, rust, directory)
    depth = capture_depth(directory / "depth")
    report.update(helpers=helpers, protocol=protocol, depth=depth,
                  production_inputs=before, source_stable=before == depth_inputs())
    (directory / "report.json").write_bytes(canonical(report) + b"\n")
    if report["source_stable"] is not True:
        raise ValueError("binder source changed during capture; diagnostic report retained")
    # The tracker derives primary parity from the frozen primary row IDs.
    # Supplemental probes, protocol rejection and helper coverage cannot inflate it.
    metrics = {"primary_requests": report["primary_requests"], "primary_rows": report["primary_rows"],
               "reached_bind": all(value == report["primary_requests"] for value in report["reached_bind"].values()),
               "supplemental_parity": report["supplemental"]["parity"],
               "supplemental_requests": report["supplemental"]["requests"],
               "protocol": len(protocol["passed"]) == protocol["tests"], "protocol_tests": protocol["tests"],
               "helpers": helpers["pass"], "helper_tests": helpers["tests"],
               "depth": depth["metrics"]["binder_depth"]}
    metrics.update(binder_contract_metrics(documents, report, helpers, protocol))
    print(json.dumps({key: value for key, value in report.items() if key != "tests"}, sort_keys=True), file=sys.stderr)
    return {"metrics": metrics, "tests": {name: "pass" if value else "fail" for name, value in report["tests"].items()}}


def prepare_subset(reuse=False):
    """Rebuild source/config observations for a clean checkout; no stale cache pass."""
    import s07_subset
    directory = ROOT / "target/s07-subset"
    observations = directory / "source-observations.ndjson"
    review = directory / "review"
    if reuse:
        from s07_subset_freeze import verify_loader
        loader = review / "loading-observations.candidate.json"
        try:
            candidate = s07_subset.classify(observations)
            verify_loader(loader, candidate["requests"])
            s07_subset.prepare(observations, review)
            print("Reusing unchanged source observations and verified Go loader closure from the preceding capture", file=sys.stderr)
            return observations, loader, review / "loading-requests.candidate.json"
        except (OSError, ValueError, KeyError):
            # Missing or changed source inputs require executing the oracle;
            # they cannot inherit the cached metric or shorten its denominator.
            pass
    s07_subset.export_observations(observations)
    s07_subset.prepare(observations, review)
    requests = review / "loading-requests.candidate.json"
    loader = review / "loading-observations.candidate.json"
    command([sys.executable, "scripts/s07_program.py", "--requests", str(requests), "--output", str(loader)], cwd=ROOT)
    return observations, loader, requests


def e2():
    from s07_subset_freeze import freeze
    observations, loader, _ = prepare_subset()
    # S07 freezes the source-selected denominator only; no future checker metric.
    return {"metrics": freeze(observations, loader, ROOT / "data/s07/subset-review.json", False)}



def program():
    from s07_subset_freeze import freeze
    from s07_program_compare import check_subset, input_fingerprints, digest
    from s07_program_helpers import measure, preflight
    from s07_verify_compare import capture as capture_verification
    def production_inputs():
        values = input_fingerprints()
        for pattern in ("scripts/s06_utilities.py", "scripts/s07_program*.py", "scripts/s07_verify*.py", "scripts/s07_config*.py",
                        "scripts/s07_subset*.py", "scripts/s07_operations.py", "scripts/s07_operation_validation.py", "scripts/s07_producers.py",
                        "tools/s07/program/*", "tools/s07/verify-options/*", "tools/s07/config/*"):
            values.update((str(path.relative_to(ROOT)), digest(path)) for path in ROOT.glob(pattern) if path.is_file())
        return values
    before = production_inputs()
    directory = ROOT / "target/s07-program-reports"
    prepared_helpers = preflight(directory / "helpers")
    observations, loader, requests = prepare_subset(reuse=True)
    freeze(observations, loader, ROOT / "data/s07/subset-review.json", False)
    config = directory / "config-evidence.json"
    command([sys.executable, "scripts/s07_config.py", "--loading-requests", str(requests),
             "--output", str(config)], cwd=ROOT)
    loader_report = check_subset(requests, loader, directory / "loader.json", config)
    verify_oracle = directory / "verification-go.json"
    command([sys.executable, "scripts/s07_verify_options.py", "--requests", str(requests),
             "--output", str(verify_oracle)], cwd=ROOT)
    verification = capture_verification(requests, verify_oracle, directory / "verification")
    helpers = measure(directory / "helpers", prepared_helpers)
    metrics = {"loader_parity": loader_report["loader_graph_parity"],
               "config_parity": loader_report["config_parity"],
               "option_verification": verification["metrics"]["option_verification"],
               "helpers": helpers["pass"], "helper_tests": helpers["tests"],
               "required_variants": loader_report["required_variants"],
               "passed_loader_variants": loader_report["passed_variants"],
               "passed_verification_variants": verification["passed_variants"]}
    metrics["source_stable"] = before == production_inputs()
    metrics["subset_loads"] = (metrics["source_stable"] and loader_report["metrics"]["subset_loads"]
                               and metrics["option_verification"] and metrics["helpers"])
    report = {"metrics":metrics, "inputs":before, "loader":loader_report, "verification":verification, "helpers":helpers}
    (directory / "report.json").write_bytes(canonical(report)+b"\n")
    print(json.dumps(report, sort_keys=True), file=sys.stderr)
    return {"metrics":metrics}

def bindworkload():
    command([sys.executable, "scripts/s07_benchmark_graph.py", "capture"], cwd=ROOT)
    path = ROOT / "target/s07-bindworkload/report.json"
    report = strict_json_loads(path.read_bytes())
    if report.get("diagnostic_subset") is not False or report.get("source_stable") is not True:
        raise ValueError("graph capture is diagnostic or source-unstable")
    print(json.dumps(report, sort_keys=True), file=sys.stderr)
    return {"metrics": {"parity": report["parity"], "files": report["files"], "worker_modes": len(report["runs"])}}


def performance(operation):
    from s07_benchmark_report import read_capture
    report, rows = read_capture()
    # Preserve every raw sample and numerator/denominator in the ledger, not just
    # a path into target/ which is absent when reviewing archived evidence.
    print(json.dumps({"capture": report, "raw_samples": rows}, sort_keys=True), file=sys.stderr)
    names = ("peak_rss_ratio", "allocated_bytes_ratio") if operation == "e5" else ("one_thread_wall_time_ratio", "eight_threads_wall_time_ratio", "stable")
    metrics = {name: report["metrics"][name] for name in names}
    for workers in ("1", "8"):
        fields = ("peak_rss_bytes", "allocated_bytes") if operation == "e5" else ("wall_time_ns",)
        for field in fields:
            value = report["summaries"][workers][field]
            for statistic in ("go_median", "rust_median", "ratio", "samples_per_runtime", "go_relative_mad", "rust_relative_mad"):
                metrics[f"workers_{workers}_{field}_{statistic}"] = value[statistic]
    if operation == "e5":
        # The checker per-type footprint (data/s08/type-footprint.json) is measured by the
        # S08 checkerbench allocation executable at the retained checkpoint of every
        # acceptance variant; without a current full capture it stays unavailable.
        from s08_checkerbench import footprint_metric
        footprint = footprint_metric()
        if footprint is None:
            print("run.e5.type_footprint_ratio unavailable: no current full S08 checkerbench capture (docs/S08-P7.md)", file=sys.stderr)
        else:
            metrics["type_footprint_ratio"] = footprint
    return {"metrics": metrics}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("binder", "program", "e2", "bindworkload", "e5", "e6"))
    args = parser.parse_args()
    if args.operation in ("e5", "e6"):
        report = performance(args.operation)
    else:
        report = {"binder": binder, "program": program, "e2": e2, "bindworkload": bindworkload}[args.operation]()
    print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"S07 producer failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
