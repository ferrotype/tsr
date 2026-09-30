#!/usr/bin/env python3
"""Phase 2 C7.5: the dashboard and the C7 record (docs/PHASE2-C7-plan.md, C7.5).

`build` writes `data/phase2/c7-report.json` and renders `docs/PHASE2-C7.md`
from the final comparisons of both modes. It holds the pass-rate dashboard
(rates by checkpoint, suite, family and configuration dimension per mode),
the final counts per domain, the residual and divergence lists, the
disposition summary, what later phases can consume and the measured
performance risks Phase 7 owns. The record binds both captures by digest.

`current(comparison, concurrent)` is the producer's `c7_report`: the
committed record and page equal a rebuild over the comparisons the producer
read.
"""
from __future__ import annotations
import argparse
from collections import defaultdict
from datetime import datetime, timezone
import json
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare  # noqa: E402
import phase2_inventory  # noqa: E402

RECORD = ROOT / "data/phase2/c7-report.json"
PAGE = ROOT / "docs/PHASE2-C7.md"
MODES = ("single", "concurrent")
GROUPS = ("checkpoint", "suite", "family", "module", "target", "jsx", "strict")

CONSUMABLE = (
    ("Phase 3", "the emit resolver's JavaScript half and the declaration-diagnostics path, with the harness's "
                "post-emit schedule"),
    ("Phase 4 and 5", "tsr_compiler::CheckedProgram, the program's checker driving through the compiler pool or a "
                      "supplied one, with CheckerRequest carrying the checker lifetime and the cancellation"),
    ("Phase 4", "CompilerCheckerPool, with its count rule and the per-architecture FENNEL partition"),
    ("Phase 4 and 5", "tsr_core::CancellationToken and the poisoned-checker contract"),
    ("Phase 4", "the TraceSink seam with its in-memory and JSON-lines sinks; the file writer is Phase 4's"),
    ("Phase 4", "the bounded tsr_core::workgroup::WorkGroup"),
    ("Phase 4", "the checkers option and the program's own single-threaded setting"),
    ("Phase 5 and 6", "the services operations, the recorded fourslash replay and hover expansion"),
    ("Phase 5", "the content-mapper host, the IPC and JSON-RPC layers and span maps, ported for the harness's "
                "in-process mappers; the project system's child-process spawner completes them"),
    ("Phase 5", "the project pool's CheckerPool implementation and its disposal of a canceled checker"),
    ("Phase 6", "the public API"),
    ("diagnostics", "the creation-trace mode and the assignment witnesses"),
    ("Phase 4", "the E3 contracts through the compiler pool"),
)


def dashboard(comparison, inventory):
    """Rows and matched rows per group value, the domain categories and totals."""
    counts = {group: defaultdict(lambda: [0, 0]) for group in GROUPS}
    for row in comparison["rows"]:
        if "outcomes" not in row:
            continue
        entry = inventory[row["id"]]
        matched = int(all(outcome in phase2_compare.MATCHED for outcome in row["outcomes"].values()))
        values = {"checkpoint": [entry["checkpoint"]], "suite": [entry["suite"]],
                  "family": entry["families"] or ["(none)"]}
        values.update({key: [str(value)] for key, value in phase2_compare.configuration(entry).items()})
        for group in GROUPS:
            for value in values[group]:
                counts[group][value][0] += 1
                counts[group][value][1] += matched
    return {"executed": comparison["executed"], "all_domains_match": comparison["all_domains_match"],
            "categories": comparison["categories"],
            "rates": {group: {value: {"rows": rows, "matched": matched}
                              for value, (rows, matched) in sorted(counts[group].items())} for group in GROUPS}}


def recorded_run(run):
    """A recorded run's metrics, host and date from the committed evidence."""
    status = json.loads((ROOT / "status/status.json").read_text())
    path = status["evidence_artifacts"].get(run)
    if not path:
        return None
    evidence = json.loads((ROOT / path).read_text())
    return {"metrics": json.loads(evidence["stdout"])["metrics"], "host": evidence["host"],
            "date": datetime.fromtimestamp(evidence["recorded_at"], timezone.utc).date().isoformat()}


def performance():
    benchmark = json.loads((ROOT / "data/phase2/c2-benchmark.json").read_text())
    result = {"checkerbench": {"source": "data/phase2/c2-benchmark.json", "host_busy": benchmark["host_busy"],
                               **{name: benchmark["metrics"][name] for name in (
                                   "elapsed_ratio", "allocated_bytes_ratio", "retained_bytes_ratio",
                                   "type_footprint_ratio")}}}
    experiments = tomllib.loads((ROOT / "status/experiments.toml").read_text())
    for run, experiment in (("e5", "E5"), ("e6", "E6")):
        recorded = recorded_run(run)
        criteria = experiments[experiment]["criteria"]
        result[run] = None if recorded is None else {
            "host": recorded["host"], "date": recorded["date"],
            "criteria": [{"metric": criterion["metric"], "op": criterion["op"], "threshold": criterion["threshold"],
                          "value": recorded["metrics"].get(criterion["metric"].rsplit(".", 1)[-1])}
                         for criterion in criteria]}
    return result


def build_record(comparison, concurrent):
    import phase2_dispositions
    import phase2_divergences
    import phase2_residuals
    if comparison is None or concurrent is None:
        raise ValueError("the C7 record needs both modes' comparisons")
    inventory = {row["id"]: row for row in phase2_inventory.executed()}
    residual_count = phase2_residuals.verified_count(comparison, concurrent)
    residuals = json.loads(phase2_residuals.RESIDUALS.read_text())
    dispositions = json.loads(phase2_dispositions.DISPOSITIONS.read_text())
    return {
        "version": 1, "pin": comparison["pin"],
        "captures": {mode: {"rust_capture_sha256": value["rust_capture_sha256"],
                            "native_observation_sha256": value["native_observation_sha256"]}
                     for mode, value in zip(MODES, (comparison, concurrent), strict=True)},
        "dashboard": {mode: dashboard(value, inventory) for mode, value in zip(MODES, (comparison, concurrent))},
        "residuals": {"count": residual_count, "rows": residuals["rows"]},
        "divergences": {"approved": len(phase2_divergences.approvals()),
                        "proposals": len(phase2_divergences.proposed_variants())},
        "dispositions": {"files": dispositions["files"], "functions": dispositions["functions"],
                         "counts": dispositions["counts"], "rejected": len(dispositions["rejected"]),
                         "later_phase_handoffs": len(dispositions["handoffs"])},
        "consumable": [{"phase": phase, "item": item} for phase, item in CONSUMABLE],
        "performance": performance(),
    }


def percent(matched, rows):
    return f"{100 * matched / rows:.2f}%" if rows else "n/a"


def render(record):
    """The C7 record as a page."""
    single, concurrent = (record["dashboard"][mode] for mode in MODES)
    lines = ["# Phase 2 C7: the phase record", "",
             "Generated by `scripts/phase2_report.py build` from `data/phase2/c7-report.json`; do not edit.",
             "", "## Captures", "", "| Mode | Rust capture | Native observation |", "| --- | --- | --- |"]
    for mode in MODES:
        capture = record["captures"][mode]
        lines.append(f"| {mode} | `{capture['rust_capture_sha256'][:16]}` | "
                     f"`{capture['native_observation_sha256'][:16]}` |")
    lines += ["", "## Final counts", "",
              f"Executed rows: {single['executed']}. Rows matching in every domain: "
              f"{single['all_domains_match']} single-threaded, {concurrent['all_domains_match']} concurrent.", "",
              "| Domain | " + " | ".join(f"{mode} {category}" for mode in MODES for category in ("match", "other")) + " |",
              "| --- |" + " ---: |" * 4]
    for domain in phase2_compare.DOMAINS:
        cells = []
        for mode in MODES:
            categories = record["dashboard"][mode]["categories"][domain]
            matched = sum(count for category, count in categories.items() if category in phase2_compare.MATCHED)
            other = {category: count for category, count in categories.items()
                     if category not in phase2_compare.MATCHED}
            cells += [str(matched), ", ".join(f"{count} {category}" for category, count in sorted(other.items()))
                      or "0"]
        lines.append(f"| {domain} | " + " | ".join(cells) + " |")
    lines += ["", "Matched counts include rows the native runner disables for a domain.", "",
              "## Pass rates", "", "Rows matching in every domain, per group value and mode.", ""]
    for group in GROUPS:
        lines += [f"### By {group}", "", f"| {group} | Rows | Single-threaded | Concurrent |",
                  "| --- | ---: | ---: | ---: |"]
        for value, counts in single["rates"][group].items():
            other = concurrent["rates"][group].get(value, {"rows": 0, "matched": 0})
            lines.append(f"| {value} | {counts['rows']} | {percent(counts['matched'], counts['rows'])} | "
                         f"{percent(other['matched'], other['rows'])} |")
        lines.append("")
    residuals = record["residuals"]
    lines += ["## Residuals", "", f"{residuals['count']} residual rows."]
    for row in residuals["rows"]:
        lines.append(f"- `{row['id']}`: {row['owner']} ({row['resolution']}), single "
                     f"{', '.join(row['domains']['single']) or '-'}, concurrent "
                     f"{', '.join(row['domains']['concurrent']) or '-'}")
    divergences = record["divergences"]
    dispositions = record["dispositions"]
    counts = dispositions["counts"]
    lines += ["", "## Divergences", "",
              f"{divergences['approved']} approved witnesses, {divergences['proposals']} proposals awaiting the owner.",
              "", "## Dispositions", "",
              f"{dispositions['functions']} functions in {dispositions['files']} Phase 2 files: {counts['mapped']} "
              f"mapped, {counts['equivalent']} equivalent and {counts['later']} later; {dispositions['rejected']} "
              f"rejected. {dispositions['later_phase_handoffs']} checkpoint handoffs name a later phase.",
              "", "## What later phases can consume", ""]
    lines += [f"- {entry['phase']}: {entry['item']}." for entry in record["consumable"]]
    performance = record["performance"]
    bench = performance["checkerbench"]
    lines += ["", "## Measured performance risks Phase 7 owns", "",
              f"- The C2 checkerbench capture (`{bench['source']}`), Rust over Go: elapsed "
              f"{bench['elapsed_ratio']:.3f}, allocated bytes {bench['allocated_bytes_ratio']:.3f}, retained bytes "
              f"{bench['retained_bytes_ratio']:.3f}, type footprint {bench['type_footprint_ratio']:.3f}"
              + (" (taken on a busy host)." if bench["host_busy"] else ".")]
    for run in ("e5", "e6"):
        recorded = performance[run]
        if recorded is None:
            lines.append(f"- {run.upper()}: no recorded run.")
            continue
        figures = "; ".join(f"`{c['metric']}` {c['value']:.3f} against {c['op']} {c['threshold']}"
                            for c in recorded["criteria"] if c["value"] is not None)
        lines.append(f"- {run.upper()}, recorded {recorded['date']} on {recorded['host']}: {figures}.")
    lines += ["", "Full checking remains extrapolated at these gates, and the multi-checker speedup is unmeasured "
                  "until Phase 7's benchmarking scenarios. The corpus runs start a process per variant and measure "
                  "no speedup. Elapsed time, retained bytes and peak RSS are not converted into one another.", ""]
    return "\n".join(lines)


def text(record):
    return json.dumps(record, indent=1, sort_keys=True) + "\n"


def current(comparison, concurrent):
    record = build_record(comparison, concurrent)
    return (RECORD.is_file() and RECORD.read_text() == text(record)
            and PAGE.is_file() and PAGE.read_text() == render(record))


def build(native, rust, native_concurrent, rust_concurrent, *, check=False):
    comparison = phase2_compare.report(native, rust, write=False)
    concurrent = phase2_compare.report(native_concurrent, rust_concurrent, write=False)
    record = build_record(comparison, concurrent)
    if check:
        if not current(comparison, concurrent):
            raise ValueError("the committed C7 record differs from the rebuilt one")
    else:
        RECORD.write_text(text(record))
        PAGE.write_text(render(record))
    return {"residuals": record["residuals"]["count"],
            "all_domains_match": {mode: record["dashboard"][mode]["all_domains_match"] for mode in MODES}}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build",))
    parser.add_argument("--native", type=Path, default=ROOT / "target/phase2/native")
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument("--native-concurrent", type=Path, default=ROOT / "target/phase2/native-concurrent")
    parser.add_argument("--rust-concurrent", type=Path, required=True)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    print(json.dumps(build(args.native, args.rust, args.native_concurrent, args.rust_concurrent, check=args.check),
                     indent=1))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 report failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
