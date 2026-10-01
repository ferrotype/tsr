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
order) and every file's two results agree: equal digest and size, or a pinned
panic where Rust refuses. Its differences are bucketed by the extension of the
first differing file and, file by file, by the Rust refusal reason.

    report --native DIR --rust DIR [--output FILE]

The Rust capture must be `scripts/phase3_corpus.py`'s, complete, taken against
the same native capture; a partial (sample) capture reports its own rows.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import digest  # noqa: E402
import s08_p4 as p4  # noqa: E402
import phase3_corpus  # noqa: E402
import phase3_native  # noqa: E402

DOMAINS = ("reprint", "output", "sourcemap", "sourcemap_record", "emit_diagnostics")
CATEGORIES = ("match", "different", "failed", "unsupported", "disabled", "unexecuted")
BASELINE_STATES = ("content", "no_content", "not_baselined")
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
    panic where Rust refuses."""
    if native["state"] == "printed" and rust["state"] == "printed":
        return native["sha256"] == rust["sha256"] and native["bytes"] == rust["bytes"]
    return native["state"] == "panic" and rust["state"] == "refused"


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
    panics = [(item, key) for item in value["files"] for key in ("comments", "no_comments")
              if item[key]["state"] == "failed"]
    if panics:
        item, key = panics[0]
        return dict(failure_outcome(item[key]), file=name_of(item["name_hex"]), mode=key)
    for left, right in zip(native["reprint"], value["files"], strict=True):
        for key in ("comments", "no_comments"):
            if print_agrees(left[key], right[key]):
                continue
            if right[key]["state"] == "refused":
                kind = "rust_refused"
            elif left[key]["state"] == "panic":
                kind = "native_panic"
            else:
                kind = "text"
            name = name_of(left["name_hex"])
            return outcome("different", kind=kind, file=name, extension=extension(name), mode=key,
                           native=strip_text(left[key]), rust=strip_text(right[key]))
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
    """Files where Rust refused and the pin printed, by reason, and refusals
    the pin's panic confirms."""
    found = []
    if "fatal" in rust or rust["reprint"]["state"] != "executed":
        return found
    for left, right in zip(native["reprint"], rust["reprint"]["files"]):
        if (left["name_hex"], left["source_sha256"]) != (right["name_hex"], right["source_sha256"]):
            break
        for key in ("comments", "no_comments"):
            if right[key]["state"] == "refused":
                found.append((right[key]["reason"], left[key]["state"] == "panic", name_of(left["name_hex"]), key))
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


def report(native_dir, rust_dir):
    native_dir, native_report, observed = phase3_native.load_capture(native_dir)
    phase3_native.current(native_report)
    metadata, requests, rows, stderrs = phase3_corpus.load_capture(rust_dir)
    if (metadata["native"]["report_sha256"] != digest((native_dir / "report.json").read_bytes())
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
        native = native_by_id[request["id"]]
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
        outcomes = compare_row(native, row, problem, attribution)
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
        if "fatal" not in row and native["state"] == "executed":
            for reason, confirmed, _name, _mode in refusals(native, row):
                bucket(confirmed_refusals if confirmed else by_refusal, reason, request["id"])
        entry = {"id": request["id"], "outcomes": {d: outcomes[d]["category"] for d in DOMAINS}}
        details = {d: outcomes[d] for d in DOMAINS if outcomes[d]["category"] not in ("match", "disabled", "unsupported")}
        if details:
            entry["details"] = details
        results.append(entry)
    summary = {
        "rows": len(results), "partial": metadata["partial"], "mode": metadata["mode"],
        "valid": not harness, "harness_errors": len(harness),
        "domains": {domain: {category: counts[domain][category] for category in CATEGORIES} for domain in DOMAINS},
    }
    return {
        "version": 1, "summary": summary,
        "native": {"report_sha256": metadata["native"]["report_sha256"],
                   "observation_sha256": native_report["observation_sha256"]},
        "rust": {"capture_sha256": digest(p4.canonical(metadata) + b"\n"), "selection": metadata["selection"]},
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


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("report")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--rust", type=Path, required=True)
    sub.add_argument("--output", type=Path, help="default: <rust>/comparison.json")
    args = parser.parse_args()
    result = report(args.native, args.rust)
    target = args.output or args.rust / "comparison.json"
    p4.atomic(target, result)
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
