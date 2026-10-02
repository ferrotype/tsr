#!/usr/bin/env python3
"""Phase 4 X0: run the Rust command-line harness over the scenario inventory.

`tools/phase4/tsctests` (package and binary `phase4_tsctests`) replays each
recorded scenario of `data/phase4/scenarios.json.gz` through the ported pinned
runner and writes one result row per scenario with the transcript it rendered
(the row format is `tools/phase4/tsctests/src/row.rs`). The whole suite is one
process and takes about a second, so a capture is one run of that process,
not the S08 per-row protocol (no per-row process, deadline or resume). What it
keeps of Phase 3's discipline (`scripts/phase3_corpus.py`) is the binding and
the replay: a capture is bound to the inventory it ran over, the executable
and the sources it was built from, and every raw artifact by digest; `replay`
recomputes the result from the raw rows and refuses anything that changed.

    run    --output DIR [--jobs N] [--replace] [SELECTOR ...]   # default selector: all
    replay --output DIR                                         # recompute from the raw rows

A selector is the binary's: `all`, a family (`tsc`, `tsbuild`, `tscWatch`,
`tsbuildWatch`) or a scenario id (with or without `.js`). Anything but `all`
is a partial capture: informational, never recorded. The default capture
directory of the recorded run is target/phase4/rust.

A capture directory holds:

  executable                  the built binary, copied (bound by digest)
  build.json                  the cargo command, the binary's digest and the
                              source fingerprint (path: sha256) it was built from
  build.stdout, build.stderr  cargo's raw output
  rows.jsonl, summary.json, baselines/<id>
                              the binary's output: one row per selected scenario
                              in inventory order, its summary, each transcript
  stdout, stderr              the binary's raw streams (stdout is the summary)
  capture.json                the binding: the inventory (path, digest, pin,
                              scenario count), the selection and the digest of
                              the selected ids, build.json's and the executable's
                              digests, the digests of rows.jsonl, summary.json,
                              stdout and stderr, the binary's exit status
  timing.json                 wall times (not bound)
  replayed.json               the replay's result

Every row is validated: its shape, exactly one state (`completed` with the
transcript's digest and size and `unexpected_diff`; `unsupported` with the
named operation; `failed` with class `panic` or `harness`, a reason and a
location or null), the scenario's id, family and recorded digest, its
progress against the scenario's edits, and the transcript file's digest and
size; the ids equal the selection's, in inventory order; the summary agrees
with the rows. Harness defects are separate from product outcomes: a
`harness` failure, or a panic located outside `crates/`, is a harness error
and invalidates the run; a production panic and a named refusal are measured
outcomes.
"""
from __future__ import annotations

import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase4_scenarios as scenarios  # noqa: E402

PACKAGE = "phase4_tsctests"
INVENTORY = scenarios.INVENTORY
FAMILIES = scenarios.FAMILIES
DEFAULT_OUTPUT = ROOT / "target/phase4/rust"
ROW_VERSION = 1
CAPTURE_VERSION = 1
ROW_BASE = {"version", "id", "family", "baseline", "scenario_sha256", "progress", "transcript", "state"}
STATE_FIELDS = {"completed": {"sha256", "bytes", "unexpected_diff"}, "unsupported": {"operation"},
                "failed": {"class", "reason", "location"}}
FAILURE_CLASSES = ("panic", "harness")
STAGES = ("setup", "header", "initial", "edit", "done")
PROGRESS_FIELDS = {"stage", "commands", "edits_completed", "edits"}
SUMMARY_FIELDS = {"version", "pin", "inventory", "selectors", "rows", "families", "states"}
# What decides the binary: the production crates (not their test-only
# suites), the harness, the Phase 3 patience diff it includes by path, and the
# build configuration. The inventory is bound separately, by digest.
SOURCE_PATTERNS = ("crates/**/*", "tools/phase4/tsctests/**/*", "tools/phase3/harness/patience.rs", "Cargo.toml",
                   "Cargo.lock", "rust-toolchain*", ".cargo/**/*", "tools/**/Cargo.toml", "xtask/Cargo.toml")
# The capture's raw artifacts bound by digest in capture.json.
ARTIFACTS = ("rows.jsonl", "summary.json", "stdout", "stderr", "build.json", "executable")
HEX64 = re.compile(r"[0-9a-f]{64}")


def test_only(path):
    """A crate's integration suites and fixtures (`crates/<crate>/tests/**`),
    which no harness binary builds (the rule of scripts/phase2_corpus.py)."""
    parts = path.split("/")
    return len(parts) > 3 and parts[0] == "crates" and parts[2] == "tests"


def sources():
    """The source fingerprint of the harness binary: path to sha256."""
    result = {}
    for pattern in SOURCE_PATTERNS:
        for path in ROOT.glob(pattern):
            relative = path.relative_to(ROOT)
            if (path.is_file() and not ({"target", "__pycache__"} & set(relative.parts))
                    and path.name != ".DS_Store" and not test_only(relative.as_posix())):
                result[relative.as_posix()] = digest(path.read_bytes())
    return dict(sorted(result.items()))


def read_document(path=INVENTORY):
    """The scenario inventory, well formed and current (`phase4_scenarios.py check`)."""
    document = scenarios.read_inventory(Path(path).read_bytes())
    scenarios.check_document(document)
    return document


def selection(document, selectors):
    """The ids the binary selects for `selectors`, in inventory order (its
    `select`): `all`, a family or an id with or without `.js`."""
    if not selectors or any(not isinstance(item, str) or not item for item in selectors):
        raise ValueError("a selection names at least one selector")
    chosen = set()
    for selector in selectors:
        wanted = selector if selector.endswith(".js") else selector + ".js"
        matched = {item["id"] for item in document["scenarios"]
                   if selector == "all" or item["family"] == selector or item["id"] == wanted}
        if not matched:
            raise ValueError("no scenario matches " + selector)
        chosen |= matched
    return [item["id"] for item in document["scenarios"] if item["id"] in chosen]


def full(document, ids):
    return ids == [item["id"] for item in document["scenarios"]]


# --- the build ---------------------------------------------------------------------

def build(directory):
    """Build the harness binary; returns the build record (written to build.json)."""
    before = sources()
    command = ["cargo", "build", "--locked", "-p", PACKAGE, "--bin", PACKAGE, "--message-format=json"]
    with (directory / "build.stdout").open("wb") as out, (directory / "build.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err, check=False)
    if completed.returncode:
        raise ValueError(f"the {PACKAGE} build failed; raw output retained in {directory}")
    binaries = set()
    for line in (directory / "build.stdout").read_bytes().splitlines():
        event = strict_json_loads(line)
        if (event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == PACKAGE
                and event["target"].get("kind") == ["bin"] and event.get("executable")):
            binaries.add(event["executable"])
    if len(binaries) != 1 or sources() != before:
        raise ValueError("ambiguous executable, or the sources changed during the build")
    executable = Path(next(iter(binaries)))
    environment = {key: value for key, value in sorted(os.environ.items())
                   if key.startswith("CARGO_PROFILE_") or key in ("CARGO_INCREMENTAL", "RUSTFLAGS", "CARGO_BUILD_TARGET")}
    return {"version": 1, "command": command, "environment": environment, "binary": str(executable.resolve()),
            "binary_sha256": digest(executable.read_bytes()), "sources": before}


# --- validation -------------------------------------------------------------------

def _digest_text(value, where):
    if not isinstance(value, str) or not HEX64.fullmatch(value):
        raise ValueError("malformed digest: " + where)


def _count(value, where):
    if type(value) is not int or value < 0:
        raise ValueError("malformed count: " + where)


def validate_row(scenario, scenario_sha256, row):
    """One row of the `phase4_tsctests` row contract; raises on any defect."""
    if not isinstance(row, dict) or row.get("version") != ROW_VERSION or row.get("id") != scenario["id"]:
        raise ValueError("missing, extra or reordered row, or another row version")
    state = row.get("state")
    if state not in STATE_FIELDS:
        raise ValueError("a row is in exactly one state: " + scenario["id"])
    if set(row) != ROW_BASE | STATE_FIELDS[state]:
        raise ValueError("missing or extra row field: " + scenario["id"])
    if row["family"] != scenario["family"] or row["baseline"] != scenario["id"]:
        raise ValueError("the row names another family or baseline: " + scenario["id"])
    if row["scenario_sha256"] != scenario_sha256:
        raise ValueError("the row ran another recording of the scenario: " + scenario["id"])
    progress = row["progress"]
    if not isinstance(progress, dict) or set(progress) != PROGRESS_FIELDS or progress["stage"] not in STAGES:
        raise ValueError("malformed progress: " + scenario["id"])
    for key in ("commands", "edits_completed", "edits"):
        _count(progress[key], "progress " + key)
    edits = len(scenario["edits"])
    if (progress["edits"] != edits or progress["edits_completed"] > edits or progress["commands"] > edits + 1
            or progress["edits_completed"] > progress["commands"]):
        raise ValueError("the progress does not fit the scenario's edits: " + scenario["id"])
    transcript = row["transcript"]
    if not isinstance(transcript, dict) or set(transcript) != {"sha256", "bytes"}:
        raise ValueError("malformed transcript: " + scenario["id"])
    _digest_text(transcript["sha256"], "transcript")
    _count(transcript["bytes"], "transcript size")
    if state == "completed":
        if row["sha256"] != transcript["sha256"] or row["bytes"] != transcript["bytes"]:
            raise ValueError("a completed row's baseline is not its transcript: " + scenario["id"])
        if row["unexpected_diff"] is not None and (not isinstance(row["unexpected_diff"], str)
                                                   or not row["unexpected_diff"]):
            raise ValueError("an unexpected difference is null or a text: " + scenario["id"])
        if (progress["stage"], progress["edits_completed"], progress["commands"]) != ("done", edits, edits + 1):
            raise ValueError("a completed row did not run every step: " + scenario["id"])
    elif state == "unsupported":
        if not isinstance(row["operation"], str) or not row["operation"]:
            raise ValueError("a refusal names its operation: " + scenario["id"])
        if progress["stage"] == "done":
            raise ValueError("a refusal after the last step: " + scenario["id"])
    else:
        if (row["class"] not in FAILURE_CLASSES or not isinstance(row["reason"], str) or not row["reason"]
                or row["location"] is not None and not isinstance(row["location"], str)):
            raise ValueError("malformed failure: " + scenario["id"])
    return row


def harness_problem(row):
    """The harness defect a row shows, or None: a harness-class failure, or a
    panic located outside the production crates."""
    if row["state"] != "failed":
        return None
    if row["class"] == "harness":
        return "harness failure: " + row["reason"]
    location = row["location"] or ""
    if not location.startswith("crates/"):
        return f"panic at {location or 'unknown location'}: {row['reason']}"[:400]
    return None


def states_by_family(rows):
    table = {}
    for row in rows:
        family = table.setdefault(row["state"], {})
        family[row["family"]] = family.get(row["family"], 0) + 1
    return {state: dict(sorted(value.items())) for state, value in sorted(table.items())}


def validate_summary(summary, document, selectors, rows):
    if not isinstance(summary, dict) or set(summary) != SUMMARY_FIELDS or summary["version"] != 1:
        raise ValueError("malformed summary.json")
    if summary["pin"] != document["provenance"]["pin"] or summary["inventory"] != {
            "scenarios": len(document["scenarios"]), "orphan_references": document["orphan_references"]}:
        raise ValueError("the summary names another inventory")
    if (summary["selectors"] != selectors or summary["rows"] != len(rows) or summary["families"] != list(FAMILIES)
            or summary["states"] != states_by_family(rows)):
        raise ValueError("the summary disagrees with the rows")


# --- capture ----------------------------------------------------------------------

def inventory_binding(document, path=INVENTORY):
    return {"path": str(Path(path).resolve().relative_to(ROOT)) if Path(path).resolve().is_relative_to(ROOT)
            else str(path), "sha256": digest(Path(path).read_bytes()), "pin": document["provenance"]["pin"],
            "scenarios": len(document["scenarios"])}


def bind(output, document, selectors, ids, exit_status, inventory=INVENTORY):
    """Write capture.json over the artifacts present in `output`."""
    metadata = {"version": CAPTURE_VERSION, "inventory": inventory_binding(document, inventory),
                "selectors": list(selectors), "ids_sha256": digest(canonical(ids)), "rows": len(ids),
                "partial": not full(document, ids), "exit_status": exit_status,
                "artifacts": {name: digest((output / name).read_bytes()) for name in ARTIFACTS}}
    with (output / "capture.json").open("xb") as stream:
        stream.write(canonical(metadata) + b"\n")
    return metadata


def capture_digest(metadata):
    return digest(canonical(metadata) + b"\n")


def prepare(output, replace):
    output = Path(output).resolve()
    if output.exists():
        if not replace:
            raise ValueError(f"{output} exists; pass --replace to discard that capture")
        if any(output.iterdir()) and not (output / "capture.json").exists() and not (output / "build.json").exists():
            raise ValueError(f"{output} is not a capture directory; refusing to replace it")
        shutil.rmtree(output)
    output.mkdir(parents=True)
    return output


def run(output, jobs, selectors=("all",), replace=False, inventory=INVENTORY):
    document = read_document(inventory)
    selectors = list(selectors) or ["all"]
    ids = selection(document, selectors)
    output = prepare(output, replace)
    timing = {}
    started = time.monotonic()
    record = build(output)
    timing["build_seconds"] = round(time.monotonic() - started, 3)
    shutil.copy2(record["binary"], output / "executable")
    if digest((output / "executable").read_bytes()) != record["binary_sha256"]:
        raise ValueError("the built executable changed while it was copied")
    with (output / "build.json").open("xb") as stream:
        stream.write(canonical(record) + b"\n")
    started = time.monotonic()
    with (output / "stdout").open("wb") as out, (output / "stderr").open("wb") as err:
        completed = subprocess.run([str(output / "executable"), "--output", str(output), "--scenarios",
                                    str(Path(inventory).resolve()), "--jobs", str(jobs), *selectors],
                                   cwd=ROOT, stdout=out, stderr=err, check=False)
    timing["run_seconds"] = round(time.monotonic() - started, 3)
    timing["jobs"] = jobs
    if completed.returncode:
        raise ValueError(f"the harness could not run (exit {completed.returncode}); see {output / 'stderr'}")
    bind(output, document, selectors, ids, completed.returncode, inventory)
    (output / "timing.json").write_bytes(canonical(timing) + b"\n")
    started = time.monotonic()
    result = replay(output, inventory=inventory)
    timing["replay_seconds"] = round(time.monotonic() - started, 3)
    (output / "timing.json").write_bytes(canonical(timing) + b"\n")
    return result, timing


SYNTHETIC = b"synthetic capture: no executable ran\n"


def completed_row(scenario, scenario_sha256, text):
    """A completed row as the binary writes one, for a transcript made elsewhere."""
    edits = len(scenario["edits"])
    sha256 = digest(text)
    return {"version": ROW_VERSION, "id": scenario["id"], "family": scenario["family"], "baseline": scenario["id"],
            "scenario_sha256": scenario_sha256,
            "progress": {"stage": "done", "commands": edits + 1, "edits_completed": edits, "edits": edits},
            "transcript": {"sha256": sha256, "bytes": len(text)},
            "state": "completed", "sha256": sha256, "bytes": len(text), "unexpected_diff": None}


def write_capture(output, document, selectors, transcripts, *, rows=None, replace=False, inventory=INVENTORY):
    """A capture laid out as `run` leaves one, from transcripts made elsewhere
    (the comparison's mutation check, the tests): completed rows unless `rows`
    gives them, a placeholder executable and a build record with no sources,
    so the capture is never source-stable and can never be recorded."""
    ids = selection(document, selectors)
    output = prepare(output, replace)
    digests = document["provenance"]["scenario_digests"]
    by_id = {item["id"]: item for item in document["scenarios"]}
    rows = rows if rows is not None else [completed_row(by_id[identity], digests[identity], transcripts[identity])
                                          for identity in ids]
    (output / "executable").write_bytes(SYNTHETIC)
    record = {"version": 1, "command": ["synthetic"], "environment": {}, "binary": None,
              "binary_sha256": digest(SYNTHETIC), "sources": {}}
    (output / "build.json").write_bytes(canonical(record) + b"\n")
    (output / "rows.jsonl").write_bytes(b"".join(json.dumps(row, separators=(",", ":")).encode() + b"\n"
                                                 for row in rows))
    for row in rows:
        path = output / "baselines" / row["id"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(transcripts[row["id"]])
    summary = {"version": 1, "pin": document["provenance"]["pin"],
               "inventory": {"scenarios": len(document["scenarios"]),
                             "orphan_references": document["orphan_references"]},
               "selectors": list(selectors), "rows": len(rows), "families": list(FAMILIES),
               "states": states_by_family(rows)}
    text = json.dumps(summary, indent=2).encode() + b"\n"
    for name in ("summary.json", "stdout"):
        (output / name).write_bytes(text)
    (output / "stderr").write_bytes(b"")
    bind(output, document, list(selectors), ids, 0, inventory)
    return output


class Capture:
    """A loaded, validated capture: its metadata, build record, the inventory,
    the selected scenarios, the rows and the transcripts by id."""

    def __init__(self, directory, metadata, record, document, selected, rows, transcripts, summary):
        self.directory, self.metadata, self.record, self.document = directory, metadata, record, document
        self.selected, self.rows, self.transcripts, self.summary = selected, rows, transcripts, summary

    @property
    def sha256(self):
        return capture_digest(self.metadata)


def load_capture(output, inventory=INVENTORY):
    """The capture with every raw artifact checked against its binding and every
    row validated against the current inventory."""
    output = Path(output).resolve()
    metadata = strict_json_loads((output / "capture.json").read_bytes())
    if not isinstance(metadata, dict) or metadata.get("version") != CAPTURE_VERSION:
        raise ValueError("not a phase4 capture")
    for name, expected in metadata["artifacts"].items():
        if digest((output / name).read_bytes()) != expected:
            raise ValueError("raw capture artifact changed: " + name)
    if set(metadata["artifacts"]) != set(ARTIFACTS):
        raise ValueError("missing or extra bound artifact")
    record = strict_json_loads((output / "build.json").read_bytes())
    if record["binary_sha256"] != metadata["artifacts"]["executable"]:
        raise ValueError("the captured executable is not the build's")
    document = read_document(inventory)
    if metadata["inventory"] != inventory_binding(document, inventory):
        raise ValueError("the capture was taken over another inventory; run it again")
    ids = selection(document, metadata["selectors"])
    if (digest(canonical(ids)) != metadata["ids_sha256"] or metadata["rows"] != len(ids)
            or metadata["partial"] != (not full(document, ids))):
        raise ValueError("the capture's selection disagrees with its binding")
    by_id = {item["id"]: item for item in document["scenarios"]}
    digests = document["provenance"]["scenario_digests"]
    lines = (output / "rows.jsonl").read_bytes().splitlines()
    rows = [strict_json_loads(line) for line in lines]
    if [row.get("id") if isinstance(row, dict) else None for row in rows] != ids:
        raise ValueError("missing, extra or reordered rows")
    transcripts = {}
    for row in rows:
        validate_row(by_id[row["id"]], digests[row["id"]], row)
        text = (output / "baselines" / row["id"]).read_bytes()
        if digest(text) != row["transcript"]["sha256"] or len(text) != row["transcript"]["bytes"]:
            raise ValueError("a transcript is not the one its row describes: " + row["id"])
        transcripts[row["id"]] = text
    written = sorted(path.relative_to(output / "baselines").as_posix()
                     for path in (output / "baselines").rglob("*") if path.is_file())
    if written != sorted(ids):
        raise ValueError("the capture holds transcripts no row describes")
    summary = strict_json_loads((output / "summary.json").read_bytes())
    validate_summary(summary, document, metadata["selectors"], rows)
    if (output / "stdout").read_bytes() != (output / "summary.json").read_bytes():
        raise ValueError("the binary's standard output is not its summary")
    if metadata["exit_status"] != 0:
        raise ValueError("the binary did not complete its run")
    return Capture(output, metadata, record, document, [by_id[item] for item in ids], rows, transcripts, summary)


def replay(output, *, write=True, capture=None, inventory=INVENTORY):
    """Recompute the capture's result from its raw rows; `write` keeps it as
    replayed.json (a producer replays without writing)."""
    capture = capture or load_capture(output, inventory)
    harness, production = [], []
    for row in capture.rows:
        problem = harness_problem(row)
        if problem:
            harness.append({"id": row["id"], "problem": problem})
        elif row["state"] == "failed":
            production.append({"id": row["id"], "reason": row["reason"], "location": row["location"]})
    summary = {"requested": capture.metadata["rows"], "observed": len(capture.rows),
               "states": dict(sorted(Counter(row["state"] for row in capture.rows).items())),
               "states_by_family": states_by_family(capture.rows),
               "operations": dict(sorted(Counter(row["operation"] for row in capture.rows
                                                 if row["state"] == "unsupported").items())),
               "stages": dict(sorted(Counter(row["progress"]["stage"] for row in capture.rows).items())),
               "unexpected_diffs": sum(row["state"] == "completed" and row["unexpected_diff"] is not None
                                       for row in capture.rows),
               "harness_errors": len(harness), "production_failures": len(production),
               "partial": capture.metadata["partial"]}
    result = {"version": 1, "summary": summary, "harness_errors": harness, "production_failures": production,
              "selectors": capture.metadata["selectors"], "capture_sha256": capture.sha256,
              "source_stable": sources() == capture.record["sources"]}
    if write:
        (Path(output) / "replayed.json").write_bytes(canonical(result) + b"\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("run")
    sub.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    sub.add_argument("--jobs", type=int, default=max(1, min(8, (os.cpu_count() or 2) - 2)))
    sub.add_argument("--replace", action="store_true", help="discard an existing capture at --output")
    sub.add_argument("selectors", nargs="*", default=["all"], metavar="SELECTOR")
    sub = commands.add_parser("replay")
    sub.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if args.command == "run":
        if args.jobs <= 0:
            parser.error("--jobs must be positive")
        result, timing = run(args.output, args.jobs, args.selectors, args.replace)
        print(json.dumps({**result["summary"], "source_stable": result["source_stable"], "timing": timing},
                         sort_keys=True))
    else:
        result = replay(args.output)
        print(json.dumps({**result["summary"], "source_stable": result["source_stable"]}, sort_keys=True))
    if result["summary"]["harness_errors"]:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 corpus failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
