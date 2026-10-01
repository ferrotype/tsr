#!/usr/bin/env python3
"""Phase 3 T0: compare a Rust emit run with the native emit contract.

Domains per executed variant: `reprint` (the printer over every non-library
source file, with and without comments), `output`, `sourcemap`,
`sourcemap_record` (the runner's three emit sub-tests) and `emit_diagnostics`
(the emit result's diagnostics). Every domain of every row lands in exactly
one category: match, different, failed (production panic, deadline or
error), unsupported (a named production refusal), disabled (the native runner
disables the sub-test; it carries the pin's reason), unexecuted (the row or
domain was not run). Harness defects are not categories: they invalidate the
report (`valid: false`) and are listed separately.

`reprint` matches when both sides list the same files (name, source digest,
order), each file has the pin's script kind and language variant, and every
file's two printings agree: equal digest and size, or a Rust refusal whose
reason is the pinned panic's message. A refusal where the pin printed, or with
another reason than the pin's panic, is `different`. Its differences are
bucketed by the extension of the first differing file, by kind and, file by
file, by the Rust refusal reason.

    report --native DIR --rust DIR [--output FILE] [--record]
    modes  --native DIR --rust DIR --native-concurrent DIR --rust-concurrent DIR

The Rust capture must be `scripts/phase3_corpus.py`'s, complete, taken against
the same native capture; a partial (sample) capture reports its own rows.
`--record` writes the acceptance summary (the report without its rows) to
data/phase3/first-comparison.json; only a full, harness-valid single-mode run
of the current sources is recorded. `modes` reports each mode's run against its
own mode's native capture and the per-row, per-domain outcome differences
between the two runs (docs/PHASE3-plan.md section 5, "Both modes agree"); it
writes modes.json into the concurrent capture.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import s08_p4 as p4  # noqa: E402
import phase3_corpus  # noqa: E402
import phase3_inventory  # noqa: E402
import phase3_native  # noqa: E402

RECORD = ROOT / "data/phase3/first-comparison.json"
DOMAINS = ("reprint", "output", "sourcemap", "sourcemap_record", "emit_diagnostics")
CATEGORIES = ("match", "different", "failed", "unsupported", "disabled", "unexecuted")
MATCHED = ("match", "disabled")
BASELINE_STATES = ("content", "no_content", "not_baselined")
FILE_KIND = ("script_kind", "language_variant")
EXAMPLES = 3
DECLARATION_EXTENSIONS = (".d.ts", ".d.mts", ".d.cts")


def outcome(category, **detail):
    if category not in CATEGORIES:
        raise ValueError("unknown category " + category)
    return dict(detail, category=category)


def failure_outcome(value):
    """A failed or refused operation: unsupported when the production names it so."""
    if value.get("class") == "unsupported":
        return outcome("unsupported", operation=value["reason"])
    reason = f"{value.get('class', value.get('state'))}: {value.get('reason', '')}"[:300]
    if value.get("location"):
        reason += " at " + value["location"]
    return outcome("failed", reason=reason)


def extension(name):
    lowered = name.lower()
    for suffix in DECLARATION_EXTENSIONS:
        if lowered.endswith(suffix):
            return suffix
    suffix = Path(name).suffix
    return suffix or "(none)"


def name_of(name_hex):
    return bytes.fromhex(name_hex).decode("utf-8", "surrogateescape")


def print_agrees(native, rust):
    """One file's printing, one comment mode: the same bytes, or the pin's
    panic where Rust refuses for the same reason (the panic's message)."""
    if native["state"] == "printed" and rust["state"] == "printed":
        return native["sha256"] == rust["sha256"] and native["bytes"] == rust["bytes"]
    return native["state"] == "panic" and rust["state"] == "refused" and rust["reason"] == native["message"]


def print_difference(native, rust):
    """The kind of a printing disagreement."""
    if rust["state"] == "refused":
        return "refusal_reason" if native["state"] == "panic" else "rust_refused"
    if native["state"] == "panic":
        return "native_panic"
    return "text"


def file_kind(item):
    return {key: item[key] for key in FILE_KIND}


def strip_text(value):
    return {key: item for key, item in value.items() if key != "text_hex"}


def compare_reprint(native, rust):
    value = rust["reprint"]
    if value["state"] == "not_requested":
        return outcome("unexecuted", reason="reprint not requested")
    if value["state"] == "not_reached":
        return failure_outcome(rust["load"])
    if value["state"] == "failed":
        return failure_outcome(value)
    expected = [(item["name_hex"], item["source_sha256"]) for item in native["reprint"]]
    actual = [(item["name_hex"], item["source_sha256"]) for item in value["files"]]
    if expected != actual:
        first = next((i for i, (left, right) in enumerate(zip(expected, actual)) if left != right),
                     min(len(expected), len(actual)))
        differing = (expected[first] if first < len(expected) else actual[first])[0]
        return outcome("different", kind="file_list", file=name_of(differing), extension=extension(name_of(differing)),
                       native_files=len(expected), rust_files=len(actual))
    for left, right in zip(native["reprint"], value["files"], strict=True):
        if file_kind(left) != file_kind(right):
            name = name_of(left["name_hex"])
            return outcome("different", kind="file_kind", file=name, extension=extension(name),
                           native=file_kind(left), rust=file_kind(right))
    panics = [(item, key) for item in value["files"] for key in ("comments", "no_comments")
              if item[key]["state"] == "failed"]
    if panics:
        item, key = panics[0]
        return dict(failure_outcome(item[key]), file=name_of(item["name_hex"]), mode=key)
    for left, right in zip(native["reprint"], value["files"], strict=True):
        for key in ("comments", "no_comments"):
            if print_agrees(left[key], right[key]):
                continue
            name = name_of(left["name_hex"])
            return outcome("different", kind=print_difference(left[key], right[key]), file=name,
                           extension=extension(name), mode=key, native=strip_text(left[key]),
                           rust=strip_text(right[key]))
    return outcome("match")


def compare_baseline(domain, native, rust):
    expected = native[domain]
    if expected["state"] == "disabled":
        return outcome("disabled", reason=expected["reason"])
    value = rust[domain]
    if value["state"] == "not_requested":
        return outcome("unexecuted", reason="emit not requested")
    if value["state"] == "failed":
        return failure_outcome(value)
    if expected["state"] == "failed":
        return outcome("unexecuted", reason="native " + expected.get("reason", "failed"))
    if value.get("state") not in BASELINE_STATES:
        raise ValueError(f"unknown Rust {domain} outcome")
    same = (value["state"] == expected["state"] and value.get("text_hex") == expected.get("text_hex")
            and value.get("name") == expected.get("name"))
    return outcome("match" if same else "different", native_state=expected["state"], rust_state=value["state"])


def compare_emit_diagnostics(native, rust):
    value = rust["emit"]
    if value["state"] == "not_requested":
        return outcome("unexecuted", reason="emit not requested")
    if value["state"] == "failed":
        return failure_outcome(value)
    expected = native["emit"]
    same = (value.get("diagnostics") == expected.get("diagnostics")
            and value.get("emit_skipped") == expected.get("emit_skipped"))
    return outcome("match" if same else "different")


def compare_row(native, rust, problem=None, attribution=None):
    """Per-domain outcomes of one variant; never blank."""
    if native["state"] != "executed":
        return {domain: outcome("unexecuted", reason="native " + native["state"]) for domain in DOMAINS}
    if problem:
        return {domain: outcome("failed", reason="harness: " + problem) for domain in DOMAINS}
    if "fatal" in rust:
        fatal = rust["fatal"]
        reason = f"{fatal['class']}: {fatal['reason']}"[:300]
        if rust.get("panic_location"):
            reason += " at " + rust["panic_location"]
        if attribution:
            reason = attribution + " (" + reason + ")"
        result = {domain: outcome("failed", reason=reason) for domain in DOMAINS}
        if native["output"]["state"] == "disabled":
            result["output"] = outcome("disabled", reason=native["output"]["reason"])
        return result
    return {"reprint": compare_reprint(native, rust),
            "output": compare_baseline("output", native, rust),
            "sourcemap": compare_baseline("sourcemap", native, rust),
            "sourcemap_record": compare_baseline("sourcemap_record", native, rust),
            "emit_diagnostics": compare_emit_diagnostics(native, rust)}


def refusals(native, rust):
    """Files where Rust refused, by reason, and whether the pin panicked there
    with that same message (a confirmed refusal)."""
    found = []
    if "fatal" in rust or rust["reprint"]["state"] != "executed":
        return found
    for left, right in zip(native["reprint"], rust["reprint"]["files"]):
        if (left["name_hex"], left["source_sha256"]) != (right["name_hex"], right["source_sha256"]):
            break
        for key in ("comments", "no_comments"):
            if right[key]["state"] == "refused":
                confirmed = left[key]["state"] == "panic" and left[key]["message"] == right[key]["reason"]
                found.append((right[key]["reason"], confirmed, name_of(left["name_hex"]), key))
    return found


def bucket(table, key, row_id, unit=1):
    entry = table[key]
    entry["count"] += unit
    if row_id not in entry["examples"] and len(entry["examples"]) < EXAMPLES:
        entry["examples"].append(row_id)
    entry["rows"].add(row_id)


def finish(table):
    return {key: {"count": value["count"], "rows": len(value["rows"]), "examples": value["examples"]}
            for key, value in sorted(table.items(), key=lambda kv: (-kv[1]["count"], kv[0]))}


def by_domain(table):
    return [dict(domain=key[0], reason=key[1], **value) for key, value in finish(table).items()]


def new_table():
    return defaultdict(lambda: {"count": 0, "rows": set(), "examples": []})


def load_native(native_dir):
    """The native capture, complete and current with its inputs."""
    directory, native_report, observed = phase3_native.load_capture(native_dir)
    phase3_native.current(native_report)
    return directory, native_report, observed


def report(native_dir, rust_dir, *, native=None, capture=None):
    """The categorized comparison. `native` and `capture` take an already
    loaded native capture (`load_native`) and Rust capture
    (`phase3_corpus.load_capture`) of the same directories."""
    native_dir, native_report, observed = native or load_native(native_dir)
    metadata, requests, rows, stderrs = capture or phase3_corpus.load_capture(rust_dir)
    if (metadata["native"]["report_sha256"] != digest((Path(native_dir) / "report.json").read_bytes())
            or metadata["native"]["observation_sha256"] != native_report["observation_sha256"]):
        raise ValueError("the Rust capture was taken against another native capture")
    if metadata["mode"] != native_report["mode"]:
        raise ValueError("the Rust capture's mode differs from the native capture's")
    native_by_id = {row["id"]: row for row in observed}
    harness = []
    results = []
    counts = {domain: Counter() for domain in DOMAINS}
    by_extension, by_kind, by_refusal, confirmed_refusals = new_table(), new_table(), new_table(), new_table()
    failed, unsupported = new_table(), new_table()
    for request, row, stderr in zip(requests, rows, stderrs, strict=True):
        native_row = native_by_id[request["id"]]
        problem, attribution = None, None
        if "fatal" in row:
            owner, reason = phase3_corpus.attribute(row, stderr.read_bytes())
            if owner == "harness":
                problem = reason
            else:
                attribution = reason
        else:
            problems = phase3_corpus.completed_problems(row)
            problem = "; ".join(problems) if problems else None
        if problem:
            harness.append({"id": request["id"], "problem": problem})
        outcomes = compare_row(native_row, row, problem, attribution)
        if set(outcomes) != set(DOMAINS):
            raise ValueError("a row lacks a domain: " + request["id"])
        for domain, value in outcomes.items():
            counts[domain][value["category"]] += 1
            if value["category"] == "failed":
                bucket(failed, (domain, value["reason"]), request["id"])
            elif value["category"] == "unsupported":
                bucket(unsupported, (domain, value["operation"]), request["id"])
        reprint = outcomes["reprint"]
        if reprint["category"] == "different":
            bucket(by_extension, reprint["extension"], request["id"])
            bucket(by_kind, reprint["kind"], request["id"])
        if "fatal" not in row and native_row["state"] == "executed":
            for reason, confirmed, _name, _mode in refusals(native_row, row):
                bucket(confirmed_refusals if confirmed else by_refusal, reason, request["id"])
        entry = {"id": request["id"], "outcomes": {d: outcomes[d]["category"] for d in DOMAINS}}
        details = {d: outcomes[d] for d in DOMAINS if outcomes[d]["category"] not in MATCHED}
        if details:
            entry["details"] = details
        results.append(entry)
    summary = {
        "rows": len(results), "partial": metadata["partial"], "mode": metadata["mode"],
        "valid": not harness, "harness_errors": len(harness),
        "domains": {domain: {category: counts[domain][category] for category in CATEGORIES} for domain in DOMAINS},
        "all_domains_met": sum(all(o in MATCHED for o in row["outcomes"].values()) for row in results),
        "unsupported_rows": sum(any(o == "unsupported" for o in row["outcomes"].values()) for row in results),
    }
    return {
        "version": 2, "pin": native_report["pin"], "summary": summary,
        "native": {"report_sha256": metadata["native"]["report_sha256"],
                   "observation_sha256": native_report["observation_sha256"], "mode": native_report["mode"]},
        "rust": {"capture_sha256": digest(p4.canonical(metadata) + b"\n"), "selection": metadata["selection"],
                 "requests_sha256": metadata["requests_sha256"],
                 "source_stable": phase3_corpus.sources() == metadata["build"]["sources"]},
        "inventory_sha256": digest(phase3_inventory.INVENTORY.read_bytes()),
        "buckets": {
            "reprint_by_extension": finish(by_extension),
            "reprint_by_kind": finish(by_kind),
            "reprint_by_refusal": finish(by_refusal),
            "reprint_refusals_matching_a_pinned_panic": finish(confirmed_refusals),
            "failed": by_domain(failed),
            "unsupported": by_domain(unsupported),
        },
        "harness_errors": harness,
        "rows": results,
    }


def acceptance_summary(comparison):
    """The recorded part of a comparison: everything but the per-row report."""
    return {key: value for key, value in comparison.items() if key != "rows"}


def record(comparison, path=RECORD):
    """Write the acceptance summary of a full, harness-valid single-mode run of
    the current sources."""
    summary = comparison["summary"]
    if summary["partial"]:
        raise ValueError("a partial Rust run is informational and cannot be recorded")
    if summary["mode"] != "single":
        raise ValueError("the recorded comparison is the single-threaded mode's")
    if not summary["valid"]:
        raise ValueError("a run with harness errors cannot be recorded")
    if not comparison["rust"]["source_stable"]:
        raise ValueError("record requires a Rust capture of the current sources")
    Path(path).write_bytes(json.dumps(acceptance_summary(comparison), indent=1, sort_keys=True).encode() + b"\n")


def row_digest(row):
    """A Rust row without its mode, which is the only field the two modes'
    requests differ in."""
    return digest(canonical({key: value for key, value in row.items() if key != "mode"}))


def modes(native_dir, rust_dir, native_concurrent_dir, rust_concurrent_dir, *, write=True, loaded=None):
    """Each mode's run against its own mode's native capture, then the per-row,
    per-domain outcome differences between the two runs. `loaded` maps
    "single"/"concurrent" to already loaded (native, capture) pairs."""
    loaded = loaded or {}
    pairs = {}
    for mode, native_path, rust_path in (("single", native_dir, rust_dir),
                                         ("concurrent", native_concurrent_dir, rust_concurrent_dir)):
        native, capture = loaded.get(mode) or (load_native(native_path), phase3_corpus.load_capture(rust_path))
        if native[1]["mode"] != mode or capture[0]["mode"] != mode:
            raise ValueError(f"the {mode} run and its native capture must both be {mode}-mode")
        pairs[mode] = (native, capture)
    single_capture, concurrent_capture = pairs["single"][1], pairs["concurrent"][1]
    if single_capture[0]["selection"] != concurrent_capture[0]["selection"]:
        raise ValueError("the two runs selected different rows")
    single = report(native_dir, rust_dir, native=pairs["single"][0], capture=single_capture)
    concurrent = report(native_concurrent_dir, rust_concurrent_dir, native=pairs["concurrent"][0],
                        capture=concurrent_capture)
    differences, observation_changes = [], []
    for left, right, left_raw, right_raw in zip(single["rows"], concurrent["rows"], single_capture[2],
                                                concurrent_capture[2], strict=True):
        if left["id"] != right["id"]:
            raise ValueError("the two runs' rows are reordered")
        for domain in DOMAINS:
            if left["outcomes"][domain] != right["outcomes"][domain]:
                differences.append({"id": left["id"], "domain": domain, "single": left["outcomes"][domain],
                                    "concurrent": right["outcomes"][domain]})
        if row_digest(left_raw) != row_digest(right_raw):
            observation_changes.append(left["id"])
    summary = {"version": 1, "rows": len(single["rows"]),
               "single": {"rust_capture_sha256": single["rust"]["capture_sha256"],
                          "native_observation_sha256": single["native"]["observation_sha256"],
                          "all_domains_met": single["summary"]["all_domains_met"],
                          "harness_errors": single["summary"]["harness_errors"],
                          "partial": single["summary"]["partial"], "source_stable": single["rust"]["source_stable"]},
               "concurrent": {"rust_capture_sha256": concurrent["rust"]["capture_sha256"],
                              "native_observation_sha256": concurrent["native"]["observation_sha256"],
                              "all_domains_met": concurrent["summary"]["all_domains_met"],
                              "harness_errors": concurrent["summary"]["harness_errors"],
                              "partial": concurrent["summary"]["partial"],
                              "source_stable": concurrent["rust"]["source_stable"]},
               "outcome_differences": len(differences), "differences": differences[:100],
               "rust_observation_differences": len(observation_changes),
               "rust_observation_examples": observation_changes[:EXAMPLES]}
    if write:
        p4.atomic(Path(rust_concurrent_dir) / "modes.json", summary)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("report")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--rust", type=Path, required=True)
    sub.add_argument("--output", type=Path, help="default: <rust>/comparison.json")
    sub.add_argument("--record", action="store_true", help="write data/phase3/first-comparison.json")
    both = commands.add_parser("modes", help="compare the single-threaded and concurrent runs' outcomes")
    both.add_argument("--native", type=Path, default=ROOT / "target/phase3/native-single")
    both.add_argument("--rust", "--single", dest="rust", type=Path, required=True)
    both.add_argument("--native-concurrent", type=Path, default=ROOT / "target/phase3/native-concurrent")
    both.add_argument("--rust-concurrent", "--concurrent", dest="rust_concurrent", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "modes":
        summary = modes(args.native, args.rust, args.native_concurrent, args.rust_concurrent)
        print(json.dumps({key: summary[key] for key in ("rows", "outcome_differences", "rust_observation_differences",
                                                         "single", "concurrent")}, sort_keys=True))
        if summary["outcome_differences"]:
            raise SystemExit(1)
        return
    result = report(args.native, args.rust)
    target = args.output or args.rust / "comparison.json"
    p4.atomic(target, result)
    if args.record:
        record(result)
    print(json.dumps({"summary": result["summary"],
                      "reprint_by_extension": result["buckets"]["reprint_by_extension"],
                      "reprint_by_kind": result["buckets"]["reprint_by_kind"],
                      "reprint_by_refusal": result["buckets"]["reprint_by_refusal"]}, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 compare failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
