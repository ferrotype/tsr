#!/usr/bin/env python3
"""Phase 3 C3: the transpile runner over `tsr_transpile`, compared with the
native capture of the pinned runner (`scripts/phase3_native.py transpile`).

The native capture holds, per configuration of the 25 transpile test files,
the runner's inputs (units, compiler options, `ReportDiagnostics`), each
run's composed baseline and the per-unit `TranspileModule` and
`TranspileDeclaration` outputs. `run` feeds those inputs to the Rust harness
(`crates/tsr_transpile/examples/phase3_transpile.rs` over
`tools/phase3/harness/transpile.rs`), which names the configuration, chooses
its runs and composes each baseline as the pinned runner does. `compare`
grades every native baseline `match`, `different` or `failed`, with both
texts for the differing ones and the per-unit outputs for attribution.
`mutate` breaks one branch of the crate or the harness at a time, rebuilds
and requires the comparison to stop matching.

    run      [--native PATH] --output DIR [--binary PATH]
    compare  [--native PATH] --rust DIR [--output FILE]
    mutate   [--native PATH] --output DIR [--mutant NAME ...]

`--native` is a capture directory (its `transpile.json`) or a recorded
document (`data/phase3/transpile-native.json`); the default is the capture
directory `target/phase3/transpile-native`. The native document is verified
before use, and is never modified: every baseline must equal the committed
reference file, every source digest the pinned test file, and a capture
directory's rows the recorded document's. `compare` writes
`comparison.json` beside the Rust rows (or to `--output`); its
`transpile_parity` is matched baselines over native baselines.
"""
from __future__ import annotations

import argparse
from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

EXAMPLE, PACKAGE = "phase3_transpile", "tsr_transpile"
DEFAULT_NATIVE = ROOT / "target/phase3/transpile-native"
RECORDED = ROOT / "data/phase3/transpile-native.json"
REFERENCE = ROOT / "upstream/tsc/testdata/baselines/reference"
CASES = ROOT / "upstream/tsc/testdata/tests/cases"
OUTCOMES = ("match", "different", "failed")
UNIT_FIELDS = ("unit_hex", "output_name_hex", "output_hex", "source_map_hex", "diagnostics")
SOURCES = ("crates/tsr_transpile/src/lib.rs", "crates/tsr_transpile/src/fs.rs",
           "crates/tsr_transpile/examples/phase3_transpile.rs", "tools/phase3/harness/transpile.rs",
           "tools/s08/p5/errors.rs", "tools/s08/p5/paths.rs")


def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def native_document(path):
    """The native document at `path` (a capture directory or a recorded
    document) and the digest of its bytes."""
    path = Path(path)
    source = path / "transpile.json" if path.is_dir() else path
    raw = source.read_bytes()
    return source, strict_json_loads(raw), digest(raw)


def load_native(path=DEFAULT_NATIVE):
    """The verified native document; raises on anything a tampered or
    partial capture would show."""
    source, document, document_sha256 = native_document(path)
    if document.get("version") != 1 or document.get("pin") != pin():
        raise ValueError("the native transpile capture is not a version 1 capture of the pin")
    rows = document["rows"]
    ids = [row["id"] for row in rows]
    if not rows or len(set(ids)) != len(ids):
        raise ValueError("native transpile rows are missing or duplicated")
    if document["configurations"] != len(rows):
        raise ValueError("the native configuration count differs from its rows")
    names = []
    for row in rows:
        if row.get("state") != "executed":
            raise ValueError("native transpile configuration did not execute: " + row["id"])
        case = CASES / row["file"]
        if not case.is_file() or digest(case.read_bytes()) != row["source_sha256"]:
            raise ValueError("native source digest differs from the pinned test file: " + row["id"])
        declarations = [run["declaration"] for run in row["runs"]]
        if declarations not in ([False], [True], [False, True]):
            raise ValueError("native runs are not the runner's: " + row["id"])
        for run in row["runs"]:
            baseline = run["baseline"]
            reference = REFERENCE / baseline["name"]
            if (baseline.get("state") != "content" or not reference.is_file()
                    or reference.read_bytes() != bytes.fromhex(baseline["text_hex"])):
                raise ValueError("native baseline differs from the committed reference: " + baseline["name"])
            if len(run["units"]) != len(row["units"]):
                raise ValueError("native run units differ from the configuration's: " + row["id"])
            names.append(baseline["name"])
    if document["baselines"] != len(names) or len(set(names)) != len(names):
        raise ValueError("the native baseline count differs from its runs")
    references = sorted("transpile/" + path.name for path in (REFERENCE / "transpile").iterdir())
    if sorted(names) != references:
        raise ValueError("the native baselines are not the committed transpile references")
    if (document["reference_disagreements"] or document["references_without_a_run"]
            or document["runs_without_a_reference"]):
        raise ValueError("the native capture recorded reference disagreements")
    if source.resolve() != RECORDED.resolve() and RECORDED.is_file():
        recorded = strict_json_loads(RECORDED.read_bytes())
        if canonical(recorded["rows"]) != canonical(rows):
            raise ValueError("the native capture's rows differ from the recorded document")
    return {"source": str(source), "document_sha256": document_sha256, "rows_sha256": digest(canonical(rows)),
            "configurations": len(rows), "baselines": len(names)}, rows


def requests(rows):
    """One harness request per native configuration: the runner's inputs."""
    return [{"id": row["id"], "file": row["file"], "configuration_name": row["configuration_name"],
             "units": row["units"], "options": row["options"],
             "report_diagnostics": row["harness_options"]["ReportDiagnostics"]} for row in rows]


def build():
    """The harness executable, as Cargo reports it."""
    command = ["cargo", "build", "-p", PACKAGE, "--example", EXAMPLE, "--message-format=json-render-diagnostics"]
    env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_PROFILE_DEV_DEBUG="0")
    completed = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, check=False)
    executable = None
    for line in completed.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact" and message.get("executable")
                and message["target"]["name"] == EXAMPLE):
            executable = message["executable"]
    if completed.returncode or executable is None:
        raise ValueError("the transpile harness did not build")
    return Path(executable)


def validate_row(row):
    """The harness row contract; raises on a malformed row."""
    if row.get("state") == "failed":
        if row.get("class") not in ("harness", "panic") or not row.get("reason"):
            raise ValueError("harness failure without a class and reason: " + str(row.get("id")))
        return
    if row.get("state") != "executed" or not isinstance(row.get("runs"), list):
        raise ValueError("malformed harness row: " + str(row.get("id")))
    for run in row["runs"]:
        baseline = run.get("baseline", {})
        if not isinstance(run.get("declaration"), bool) or not baseline.get("name"):
            raise ValueError("malformed harness run: " + row["id"])
        if baseline.get("state") == "content":
            bytes.fromhex(baseline["text_hex"])
        elif baseline.get("state") != "no_content":
            raise ValueError("malformed harness baseline: " + row["id"])
        for unit in run.get("units", []):
            if set(unit) != set(UNIT_FIELDS):
                raise ValueError("malformed harness unit: " + row["id"])


def run(native, output, binary=None):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    provenance, rows = load_native(native)
    binary = Path(binary) if binary else build()
    lines = requests(rows)
    request_bytes = b"".join(canonical(line) + b"\n" for line in lines)
    (output / "requests.ndjson").write_bytes(request_bytes)
    with (output / "stderr").open("wb") as err:
        completed = subprocess.run([str(binary), str(output / "requests.ndjson"), str(output / "rows.ndjson")],
                                   stdout=subprocess.DEVNULL, stderr=err, check=False)
    if completed.returncode:
        raise ValueError(f"the transpile harness failed (exit {completed.returncode}); see {output}")
    rust = [strict_json_loads(line) for line in (output / "rows.ndjson").read_bytes().splitlines()]
    if [row.get("id") for row in rust] != [line["id"] for line in lines]:
        raise ValueError("harness rows are missing, extra or reordered")
    for row in rust:
        validate_row(row)
    report = {
        "version": 1, "pin": pin(), "native": provenance,
        "harness": {"binary_sha256": digest(binary.read_bytes()),
                    "sources": {name: digest((ROOT / name).read_bytes()) for name in SOURCES}},
        "requests_sha256": digest(request_bytes), "rows_sha256": digest((output / "rows.ndjson").read_bytes()),
        "rows": len(rust), "states": dict(sorted(Counter(row["state"] for row in rust).items())),
    }
    (output / "report.json").write_bytes(json.dumps(report, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({key: report[key] for key in ("rows", "states")}, sort_keys=True))
    return report


def load_rust(directory, native_rows):
    """The Rust run's rows, bound to its report and to the native rows."""
    directory = Path(directory)
    report = strict_json_loads((directory / "report.json").read_bytes())
    raw = (directory / "rows.ndjson").read_bytes()
    if digest(raw) != report["rows_sha256"]:
        raise ValueError("the harness rows differ from their run report")
    if report["native"]["rows_sha256"] != digest(canonical(native_rows)):
        raise ValueError("the harness ran on other native rows")
    rows = [strict_json_loads(line) for line in raw.splitlines()]
    if [row.get("id") for row in rows] != [row["id"] for row in native_rows]:
        raise ValueError("harness rows are missing, extra or reordered")
    for row in rows:
        validate_row(row)
    return report, rows


def first_difference(native, rust):
    index = next((i for i, (a, b) in enumerate(zip(native, rust)) if a != b), min(len(native), len(rust)))
    return {"byte": index, "line": native[:index].count(b"\n") + 1}


def grade(native_row, rust_row):
    """Each native run of a configuration against the Rust run of the same
    kind: its outcome, and the per-unit attribution of a difference."""
    results = []
    rust_runs = {} if rust_row["state"] != "executed" else {run["declaration"]: run for run in rust_row["runs"]}
    for run in native_row["runs"]:
        native = run["baseline"]
        item = {"id": native_row["id"], "name": native["name"], "declaration": run["declaration"]}
        if rust_row["state"] != "executed":
            item.update(outcome="failed", reason=rust_row["reason"], **({"location": rust_row["location"]}
                                                                         if rust_row.get("location") else {}))
            results.append(item)
            continue
        rust = rust_runs.get(run["declaration"])
        if rust is None:
            item.update(outcome="different", reason="the harness made no such run")
            results.append(item)
            continue
        same = (rust["baseline"]["name"] == native["name"] and rust["baseline"]["state"] == native["state"]
                and rust["baseline"].get("text_hex") == native["text_hex"])
        units = [{field: native_unit[field] == rust_unit.get(field) for field in UNIT_FIELDS}
                 for native_unit, rust_unit in zip(run["units"], rust.get("units", []))]
        unit_count = len(rust.get("units", [])) == len(run["units"])
        item["units"] = {"count_matches": unit_count,
                         "fields": {field: sum(unit[field] for unit in units) for field in UNIT_FIELDS},
                         "of": len(run["units"])}
        if same:
            item["outcome"] = "match"
        else:
            native_text = bytes.fromhex(native["text_hex"])
            rust_text = bytes.fromhex(rust["baseline"].get("text_hex", ""))
            item.update(outcome="different", rust_name=rust["baseline"]["name"],
                        rust_state=rust["baseline"]["state"], first_difference=first_difference(native_text, rust_text),
                        native_text=native_text.decode("utf-8", "replace"),
                        rust_text=rust_text.decode("utf-8", "replace"),
                        differing_units=[{"unit": bytes.fromhex(native_unit["unit_hex"]).decode("utf-8", "replace"),
                                          "fields": [field for field in UNIT_FIELDS if not unit[field]]}
                                         for native_unit, unit in zip(run["units"], units) if not all(unit.values())])
        results.append(item)
    extra = sorted(set(rust_runs) - {run["declaration"] for run in native_row["runs"]})
    return results, extra


def compare(native, rust_dir, output=None):
    provenance, native_rows = load_native(native)
    report, rust_rows = load_rust(rust_dir, native_rows)
    results, extra_runs = [], []
    for native_row, rust_row in zip(native_rows, rust_rows):
        graded, extra = grade(native_row, rust_row)
        results.extend(graded)
        extra_runs.extend({"id": native_row["id"], "declaration": declaration} for declaration in extra)
    outcomes = Counter(item["outcome"] for item in results)
    baselines = provenance["baselines"]
    if len(results) != baselines:
        raise ValueError("graded baselines differ from the native count")
    matched = outcomes["match"]
    document = {
        "version": 1, "pin": pin(), "native": provenance,
        "rust": {"report_sha256": digest((Path(rust_dir) / "report.json").read_bytes()),
                 "rows_sha256": report["rows_sha256"], "binary_sha256": report["harness"]["binary_sha256"],
                 "sources": report["harness"]["sources"]},
        "configurations": provenance["configurations"], "baselines": baselines,
        "outcomes": {outcome: outcomes[outcome] for outcome in OUTCOMES},
        "matched": matched, "transpile_parity": matched / baselines, "extra_runs": extra_runs,
        "all_match": matched == baselines and not extra_runs,
        "results": results,
    }
    target = Path(output) if output else Path(rust_dir) / "comparison.json"
    target.write_bytes(json.dumps(document, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({key: document[key] for key in ("configurations", "baselines", "outcomes", "matched",
                                                       "transpile_parity", "all_match")}, sort_keys=True))
    return document


# One broken branch each, in the crate and in the harness. Each anchor occurs
# exactly once; every mutant must make the comparison stop matching.
MUTANTS = (
    ("isolated_declarations", "crates/tsr_transpile/src/lib.rs",
     "        opts.isolated_declarations = Tristate::TRUE;\n", ""),
    ("report_diagnostics", "crates/tsr_transpile/src/lib.rs",
     "    if options.report_diagnostics {\n", "    if !options.report_diagnostics {\n"),
    ("source_map_text", "crates/tsr_transpile/src/lib.rs",
     "            written.source_map_text = Some(JsString::from_bytes(text));\n", ""),
    ("barebones_lib", "crates/tsr_transpile/src/lib.rs",
     "declare var Symbol: SymbolConstructor;\n", ""),
    ("section_terminator", "tools/phase3/harness/transpile.rs",
     "    if !content.ends_with(b\"\\n\") {\n        result.extend_from_slice(b\"\\r\\n\");\n    }\n}\n",
     "    if !content.ends_with(b\"\\n\") {\n        result.extend_from_slice(b\"\\n\");\n    }\n}\n"),
    ("diagnostic_file_name", "tools/phase3/harness/transpile.rs",
     "                diagnostic_file_name = file_name(&output.program, file)?;\n", ""),
)


def mutate(native, output, names=()):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    chosen = [mutant for mutant in MUTANTS if not names or mutant[0] in names]
    if names and len(chosen) != len(set(names)):
        raise ValueError("unknown mutant")
    results = []
    try:
        for name, path, before, after in chosen:
            source = ROOT / path
            original = source.read_bytes()
            text = original.decode()
            if text.count(before) != 1:
                raise ValueError(f"mutant anchor occurs {text.count(before)} times: {name}")
            try:
                source.write_bytes(text.replace(before, after).encode())
                binary = build()
                run(native, output / name, binary=binary)
            finally:
                source.write_bytes(original)
            document = compare(native, output / name)
            results.append({"mutant": name, "file": path, "outcomes": document["outcomes"],
                            "killed": not document["all_match"],
                            "examples": [item["name"] for item in document["results"]
                                         if item["outcome"] != "match"][:3]})
    finally:
        build()
    summary = {"version": 1, "mutants": results, "all_killed": all(item["killed"] for item in results)}
    (output / "mutation.json").write_bytes(json.dumps(summary, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps(summary, indent=1, sort_keys=True))
    if not summary["all_killed"]:
        raise ValueError("a mutant survived")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("run")
    sub.add_argument("--native", type=Path, default=DEFAULT_NATIVE)
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--binary", type=Path)
    sub = commands.add_parser("compare")
    sub.add_argument("--native", type=Path, default=DEFAULT_NATIVE)
    sub.add_argument("--rust", type=Path, required=True)
    sub.add_argument("--output", type=Path)
    sub = commands.add_parser("mutate")
    sub.add_argument("--native", type=Path, default=DEFAULT_NATIVE)
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--mutant", action="append", default=[])
    args = parser.parse_args()
    if args.command == "run":
        run(args.native, args.output, args.binary)
    elif args.command == "compare":
        compare(args.native, args.rust, args.output)
    else:
        mutate(args.native, args.output, args.mutant)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 transpile failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
