#!/usr/bin/env python3
"""Phase 3 T8: the per-area dashboard and the Phase 3 report (docs/PHASE3-plan.md,
section 4, T8's closure).

`build` writes `data/phase3/report.json` and renders `docs/PHASE3-report.md`
from both modes' final comparisons and the transpile comparison. It holds the
pass-rate dashboard (rows matching in every domain by suite, conformance area,
target, module, JSX setting, declaration setting and source-map reference, per
mode), the final counts per domain, the transpile result, the residual list,
the function dispositions, what Phases 4 to 7 can consume, and the emit timing
capture Phase 7 owns. The record binds every capture it reads by digest.

`current(comparisons, transpile)` is the `emit` producer's `report`: the
committed record and page equal a rebuild over the comparisons the producer
read.

    build [--native-single DIR] [--rust-single DIR] [--native-concurrent DIR]
          [--rust-concurrent DIR] [--transpile DIR] [--check]
"""
from __future__ import annotations

import argparse
import contextlib
from collections import defaultdict
import io
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT  # noqa: E402
import phase3_audit  # noqa: E402
import phase3_compare  # noqa: E402
import phase3_inventory  # noqa: E402
import phase3_native  # noqa: E402
import phase3_residuals  # noqa: E402
import phase3_transpile  # noqa: E402

RECORD = ROOT / "data/phase3/report.json"
PAGE = ROOT / "docs/PHASE3-report.md"
TIMING = ROOT / "data/phase3/timing.json"
MODES = phase3_native.MODES
GROUPS = ("suite", "area", "target", "module", "jsx", "declaration", "sourcemap")
# Test settings spell one option value several ways.
ALIASES = {"es6": "es2015"}

CONSUMABLE = (
    ("Phase 4", "CheckedProgram::emit with EmitOnly and ForceEmit, the emitter, the emit host and outputpaths, "
                "for the CLI's emit gating (execute/tsc/emit.go) and --listEmittedFiles"),
    ("Phase 4", "tsr_incremental, the pinned execute/incremental package (the incremental program, its affected-file "
                "and emit handlers, build-info read and write), ported for the harness's incremental rows; the CLI's "
                "incremental, build and watch modes consume it"),
    ("Phase 4", "the emit phase's trace pushes through the TraceSink seam; the trace file writer is Phase 4's"),
    ("Phase 5", "tsr_sourcemap's decoder and document position mapper, for the language service's mapped-position "
                "queries over .d.ts.map files"),
    ("Phase 5 and 6", "tsr_printer (the printer, the emit context and factory, the name generator and the text "
                      "writers) for the change tracker and API printing"),
    ("Phase 6", "tsr_transpile's TranspileModule and TranspileDeclaration behind the API method"),
    ("Phase 7", "emit on tsr_embed::Session and on the WebAssembly checker session with an in-memory write "
                "callback, exposed without an acceptance claim"),
    ("Phase 7", "the bounded emit timing capture and the phase timers, for the budgets and benchmark scenarios"),
)


def quietly(function, *args, **kwargs):
    with contextlib.redirect_stdout(io.StringIO()):
        return function(*args, **kwargs)


def setting(entry, name):
    """One test setting's value, lowercased and with its aliases folded."""
    value = next((value for key, value in entry["settings"].items() if key.lower() == name), None)
    if value is None:
        return "unset"
    value = str(value).lower()
    return ALIASES.get(value, value)


def groups(entry):
    """The dashboard's group values of one inventory row."""
    parts = entry["id"].split("#", 1)[0].split("/")
    area = parts[1] if entry["suite"] == "conformance" and len(parts) > 2 else f"({entry['suite']})"
    return {"suite": entry["suite"], "area": area, "target": setting(entry, "target"),
            "module": setting(entry, "module"), "jsx": setting(entry, "jsx"),
            "declaration": setting(entry, "declaration"), "sourcemap": entry["sourcemap"]}


def dashboard(comparison, inventory):
    """Rows and rows matching in every domain per group value, with the final
    counts per domain. A row matches as in the residual list: every domain
    matches or is disabled, the declaration domain where the row requires it."""
    counts = {group: defaultdict(lambda: [0, 0]) for group in GROUPS}
    for row in comparison["rows"]:
        if "outcomes" not in row:
            continue
        matched = int(not phase3_residuals.residual_domains(row))
        for group, value in groups(inventory[row["id"]]).items():
            counts[group][value][0] += 1
            counts[group][value][1] += matched
    summary = comparison["summary"]
    return {"rows": summary["rows"], "all_domains_met": summary["all_domains_met"],
            "declaration_required": summary["declaration_required"], "domains": summary["domains"],
            "rates": {group: {value: {"rows": rows, "matched": matched}
                              for value, (rows, matched) in sorted(counts[group].items())} for group in GROUPS}}


def transpile_comparison(rust):
    """The transpile comparison of the Rust run at `rust`, or None when the run
    is missing or stale."""
    rust = Path(rust)
    if not (rust / "report.json").is_file() or not phase3_transpile.current(rust):
        return None
    return quietly(phase3_transpile.compare, phase3_transpile.RECORDED, rust, rust / "comparison.json")


def timing():
    if not TIMING.is_file():
        return None
    document = strict_json_loads(TIMING.read_bytes())
    return {key: document[key] for key in ("host", "rows", "go_seconds", "rust_seconds", "rust_over_go", "scope")}


def build_record(comparisons, transpile):
    if set(comparisons) != set(MODES) or transpile is None:
        raise ValueError("the Phase 3 report needs both modes' comparisons and a current transpile run")
    inventory = {row["id"]: row for row in phase3_inventory.read()["rows"]}
    residuals = phase3_residuals.committed()
    if residuals != phase3_residuals.document(comparisons):
        raise ValueError("data/phase3/residuals.json is missing or differs from the rebuilt list")
    audit = strict_json_loads(phase3_audit.AUDIT.read_bytes())
    single = comparisons["single"]
    return {
        "version": 1, "pin": single["pin"],
        "captures": {mode: {"native_observation_sha256": comparisons[mode]["native"]["observation_sha256"],
                            "rust_capture_sha256": comparisons[mode]["rust"]["capture_sha256"]} for mode in MODES},
        "dashboard": {mode: dashboard(comparisons[mode], inventory) for mode in MODES},
        "transpile": {"rust_report_sha256": transpile["rust"]["report_sha256"],
                      "configurations": transpile["configurations"], "baselines": transpile["baselines"],
                      "matched": transpile["matched"], "outcomes": transpile["outcomes"],
                      "extra_runs": len(transpile["extra_runs"])},
        "residuals": {"count": residuals["count"], "rows": residuals["rows"]},
        "dispositions": {"complete": audit["complete"], "totals": audit["totals"],
                         "groups": {group: value["counts"] for group, value in audit["groups"].items()}},
        "consumable": [{"phase": phase, "item": item} for phase, item in CONSUMABLE],
        "timing": timing(),
    }


def percent(matched, rows):
    return f"{100 * matched / rows:.2f}%" if rows else "n/a"


def render(record):
    """The Phase 3 report as a page."""
    single, concurrent = (record["dashboard"][mode] for mode in MODES)
    lines = ["# Phase 3 report: emit", "",
             "Generated by `scripts/phase3_report.py build` from `data/phase3/report.json`; do not edit. "
             "[PHASE3-T8.md](PHASE3-T8.md) is the written record.",
             "", "## Captures", "", "| Mode | Rust capture | Native observation |", "| --- | --- | --- |"]
    for mode in MODES:
        capture = record["captures"][mode]
        lines.append(f"| {mode} | `{capture['rust_capture_sha256'][:16]}` | "
                     f"`{capture['native_observation_sha256'][:16]}` |")
    lines += ["", "## Final counts", "",
              f"Executed rows: {single['rows']}. Rows matching in every domain: {single['all_domains_met']} "
              f"single-threaded, {concurrent['all_domains_met']} concurrent. Rows requiring the declaration domain: "
              f"{single['declaration_required']}.", "",
              "| Domain | " + " | ".join(f"{mode} {category}" for mode in MODES
                                         for category in ("match", "disabled", "other")) + " |",
              "| --- |" + " ---: |" * 6]
    for domain in phase3_compare.DOMAINS:
        cells = []
        for mode in MODES:
            categories = record["dashboard"][mode]["domains"][domain]
            other = {category: count for category, count in categories.items()
                     if category not in phase3_compare.MATCHED and count}
            cells += [str(categories.get("match", 0)), str(categories.get("disabled", 0)),
                      ", ".join(f"{count} {category}" for category, count in sorted(other.items())) or "0"]
        lines.append(f"| {domain} | " + " | ".join(cells) + " |")
    transpile = record["transpile"]
    lines += ["", "A disabled domain is one the pinned runner does not run for that row, with the pin's reason.", "",
              "## Transpile", "",
              f"{transpile['matched']} of {transpile['baselines']} transpile baselines match over "
              f"{transpile['configurations']} configurations (`TranspileModule` and `TranspileDeclaration`); "
              f"{transpile['extra_runs']} runs the pin did not make.", "",
              "## Pass rates", "", "Rows matching in every domain, per group value and mode. `area` is the "
              "conformance directory; `target`, `module`, `jsx` and `declaration` are the test's settings, "
              "`unset` where the test sets none; `sourcemap` is whether the row has a `.js.map` reference.", ""]
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
        lines.append(f"- `{row['id']}`: " + "; ".join(f"{mode} {', '.join(sorted(domains))}"
                                                       for mode, domains in sorted(row["modes"].items())))
    dispositions = record["dispositions"]
    totals = dispositions["totals"]
    lines += ["", "## Dispositions", "",
              f"{totals['total']} functions in scope: {totals['mapped']} mapped, {totals['equivalent']} equivalent, "
              f"{totals['later']} later, {totals['gap']} gap; the audit is "
              + ("complete." if dispositions["complete"] else "not complete."), "",
              "| Checkpoint | Functions | Mapped | Equivalent | Later | Gap |", "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for group, counts in dispositions["groups"].items():
        lines.append(f"| {group} | {counts['total']} | {counts['mapped']} | {counts['equivalent']} | "
                     f"{counts['later']} | {counts['gap']} |")
    lines += ["", "## What Phases 4 to 7 can consume", ""]
    lines += [f"- {entry['phase']}: {entry['item']}." for entry in record["consumable"]]
    lines += ["", "## Performance facts Phase 7 owns", ""]
    measured = record["timing"]
    if measured is None:
        lines.append("- No emit timing capture is recorded (`scripts/phase3_timing.py`).")
    else:
        host = measured["host"]
        lines.append(f"- The bounded emit timing capture ({measured['scope']}): {measured['rows']} rows, Go "
                     f"{measured['go_seconds']:.1f} s, Rust {measured['rust_seconds']:.1f} s, Rust over Go "
                     f"{measured['rust_over_go']:.3f}, on {host['os']} {host['architecture']} with {host['cpus']} "
                     "CPUs. It includes each harness's per-variant process cost and is a disclosure, not a "
                     "benchmark; no threshold applies.")
    lines += ["- The pinned algorithms keep their own quadratic paths (the printer's `getTextOfNode`, several "
              "transforms and the checker's contextual-type walks); the port follows them.", ""]
    return "\n".join(lines)


def text(record):
    return json.dumps(record, indent=1, sort_keys=True) + "\n"


def current(comparisons, transpile):
    record = build_record(comparisons, transpile)
    return (RECORD.is_file() and RECORD.read_text() == text(record)
            and PAGE.is_file() and PAGE.read_text() == render(record))


def build(native, rust, transpile_rust, *, check=False):
    comparisons = phase3_residuals.comparisons(native, rust)
    transpile = transpile_comparison(transpile_rust)
    if transpile is None:
        raise ValueError(f"the transpile run at {transpile_rust} is missing or stale")
    record = build_record(comparisons, transpile)
    if check:
        if not current(comparisons, transpile):
            raise ValueError("the committed Phase 3 report differs from the rebuilt one")
    else:
        RECORD.write_text(text(record))
        PAGE.write_text(render(record))
    return {"residuals": record["residuals"]["count"],
            "all_domains_met": {mode: record["dashboard"][mode]["all_domains_met"] for mode in MODES}}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build",))
    for mode in MODES:
        parser.add_argument(f"--native-{mode}", type=Path, default=ROOT / f"target/phase3/native-{mode}")
        parser.add_argument(f"--rust-{mode}", type=Path, default=ROOT / f"target/phase3/rust-{mode}")
    parser.add_argument("--transpile", type=Path, default=ROOT / "target/phase3/transpile-rust")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    native = {mode: getattr(args, f"native_{mode}") for mode in MODES}
    rust = {mode: getattr(args, f"rust_{mode}") for mode in MODES}
    print(json.dumps(build(native, rust, args.transpile, check=args.check), indent=1))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 report failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
