#!/usr/bin/env python3
"""Phase 2 C0.4: compare the Rust run with the native contract and bucket it.

Domains per executed variant: errors (byte-exact rendering, the rendered
diagnostic set and the native pre/post-emit sets), types and symbols (bytes
plus the walker's pulls for that walk, numeric type IDs excluded as in S08),
public display (default TypeToString on every walked type), and the three
runner sub-tests (trace bytes, union-ordering and parent-pointer verdicts).

Every domain lands in exactly one category: match, different, failed
(production panic, deadline or error), unsupported (a named production
refusal), disabled (native disables the domain), unexecuted. Harness defects
are not categories: they invalidate the run and are listed separately.

Each non-matching row gets one bucket and one checkpoint. The bucket is the
first observed cause; secondary dimensions (families, configuration, first
differing diagnostic code, area) are counted, never used to invent a result.

    report --native DIR --rust DIR [--previous REPORT] [--record]
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
import s08_e2_contract as contract  # noqa: E402
import s08_p4 as p4  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
from s08_queries import action  # noqa: E402
import phase2_corpus  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402

RECORD = ROOT / "data/phase2/first-comparison.json"
DOMAINS = ("errors", "types", "symbols", "display", "trace", "union_ordering", "parent_pointers")
CATEGORIES = ("match", "different", "failed", "unsupported", "disabled", "unexecuted")
TYPE_PULLS = ("GetTypeAtLocation", "TypeToTypeNode")
SYMBOL_PULLS = ("GetSymbolAtLocation", "SymbolToStringEx")
MODULE = phase2_inventory.MODULE
TARGET = phase2_inventory.TARGET
JSX = phase2_inventory.JSX


def outcome(category, **detail):
    if category not in CATEGORIES:
        raise ValueError("unknown category " + category)
    return dict(detail, category=category)


def first_failure(row):
    """The first failed operation of a completed row, in execution order."""
    load = row["load"]
    if load["state"] != "executed":
        return load
    for name in ("config", "program", "syntactic", "semantic", "global", "declaration", "suggestion"):
        phase = row["phases"].get(name)
        if phase is None:
            continue
        if "files" in phase:
            for file in phase["files"]:
                if file["result"]["state"] != "executed":
                    return dict(file["result"], phase=name)
        elif phase["state"] != "executed":
            return dict(phase, phase=name)
    return None


UNSUPPORTED_TEXT = "unsupported checker operation: "  # tsr_checker::Error::Unsupported's Display


def failure_outcome(value):
    """Map a failed operation value to unsupported or failed."""
    if value.get("state") == "not_implemented":
        return outcome("unsupported", operation=value.get("reason", "not implemented"))
    if value.get("class") == "unsupported":
        return outcome("unsupported", operation=value["reason"])
    reason = value.get("reason") or ""
    if reason.startswith(UNSUPPORTED_TEXT):
        return outcome("unsupported", operation=reason[len(UNSUPPORTED_TEXT):])
    return outcome("failed", reason=f"{value.get('class', value.get('state'))}: {value.get('reason', '')}"[:300])


def query_domain(queries, operations):
    files = {}
    return [action(q, files) for q in queries if q["operation"] in operations], files


def compare_walk(native, rust, key, operations):
    if rust["state"] != "executed":
        return failure_outcome(rust)
    expected, actual = native[key], rust[key]
    left = query_domain(native["queries"], operations)
    right = query_domain(rust["queries"], operations)
    differences = []
    if expected != actual:
        differences.append("bytes")
    if left != right:
        differences.append("pulls")
    return outcome("different" if differences else "match", differences=differences)


def first_code(expected, actual):
    for left, right in zip(expected, actual):
        if left != right:
            return left["code"]
    if len(expected) != len(actual):
        longer = expected if len(expected) > len(actual) else actual
        return longer[min(len(expected), len(actual))]["code"]
    return None


def compare_errors(native, rust):
    errors = rust["error_baseline"]
    if errors["state"] != "executed":
        cause = first_failure(rust) or errors
        return failure_outcome(cause if cause.get("state") in ("failed", "not_implemented") else errors)
    differences = []
    for label, left, right in (
        ("native_pre_post", native["error_pre_diagnostics"], native["error_post_diagnostics"]),
        ("post_diagnostics", native["error_post_diagnostics"], errors["diagnostics"]),
        ("render_diagnostics", native["error_diagnostics"], errors["diagnostics"]),
        ("inputs", native["error_render_inputs"], errors["inputs"]),
        ("pretty", native["error_pretty"], errors["pretty"]),
        ("bytes", native["errors"], errors["baseline"]),
    ):
        if left != right:
            differences.append(label)
    code = first_code(native["error_diagnostics"], errors["diagnostics"]) if differences else None
    return outcome("different" if differences else "match", differences=differences, first_code=code)


def compare_display(native, rust):
    walk = rust["type_symbol_baselines"]
    if walk["state"] != "executed":
        return failure_outcome(walk)
    expected = contract.display_queries(native.get("public_type_strings"))
    actual = contract.display_queries(walk.get("public_type_strings"))
    if actual is None:
        return outcome("failed", reason="public TypeToString did not execute")
    return outcome("match" if expected == actual else "different")


def compare_subtest(name, native, rust):
    value = rust["phase2"]
    if value == {"state": "not_reached"}:
        cause = first_failure(rust)
        return failure_outcome(cause) if cause else outcome("unexecuted", reason="sub-tests not reached")
    observed = value[name]
    if observed["state"] == "failed":
        return failure_outcome(observed)
    if name == "trace":
        if native["trace"]["state"] == "disabled":
            return outcome("disabled")
        return outcome("match" if observed == native["trace"] else "different")
    if name == "union_ordering":
        passed = native["union_ordering"]["inconsistent"] == 0
        return outcome("match" if (observed["inconsistent"] == 0) == passed else "different",
                       native_unions=native["union_ordering"]["unions"], rust_unions=observed["unions"])
    passed = native["parent_pointers"]["failure"] is None
    return outcome("match" if (observed["failure"] is None) == passed else "different",
                   failure=observed["failure"])


def compare_row(native, rust, harness, attribution=None):
    """Per-domain outcomes of one variant; None when the row is a harness defect.
    A domain the native runner disables is `disabled` whatever Rust did."""
    if harness:
        return None
    result = observed_row(native, rust, attribution)
    if native["state"] == "executed":
        if native["types"]["state"] == "disabled":
            result.update(types=outcome("disabled"), symbols=outcome("disabled"), display=outcome("disabled"))
        if native["trace"]["state"] == "disabled":
            result["trace"] = outcome("disabled")
    return result


def observed_row(native, rust, attribution):
    if native["state"] != "executed":
        return {domain: outcome("unexecuted", reason="native " + native["state"]) for domain in DOMAINS}
    if "fatal" in rust:
        fatal = rust["fatal"]
        reason = f"{fatal['class']}: {fatal['reason']}"[:300]
        if rust.get("panic_location"):
            reason += " at " + rust["panic_location"]
        if attribution:
            reason = attribution + " (" + reason + ")"
        return {domain: outcome("failed", reason=reason) for domain in DOMAINS}
    result = {"errors": compare_errors(native, rust)}
    disabled = native["types"]["state"] == "disabled"
    walk = rust["type_symbol_baselines"]
    if disabled:
        if walk != {"state": "not_requested"}:
            raise ValueError("Rust walked a natively disabled baseline")
        result.update(types=outcome("disabled"), symbols=outcome("disabled"), display=outcome("disabled"))
    elif walk["state"] != "executed":
        # A walk that never ran (the adapter's placeholder) or was refused for an
        # incomplete prerequisite inherits the first failed operation's cause.
        cause = first_failure(rust)
        inherited = walk["state"] == "not_implemented" or walk.get("class") == "baseline_prerequisite"
        value = failure_outcome(cause if cause and inherited else walk)
        result.update(types=value, symbols=value, display=value)
    else:
        result["types"] = compare_walk(native, walk, "types", TYPE_PULLS)
        result["symbols"] = compare_walk(native, walk, "symbols", SYMBOL_PULLS)
        result["display"] = compare_display(native, rust)
    for name in ("trace", "union_ordering", "parent_pointers"):
        result[name] = compare_subtest(name, native, rust)
    return result


def area(result, rust, attributions=None):
    """Execution evidence for the checker area; never inferred from syntax."""
    attributions = attributions or {}
    for domain in DOMAINS:
        value = result[domain]
        if value["category"] == "unsupported":
            return "unsupported: " + value["operation"]
    if "fatal" in rust:
        location = rust.get("panic_location") or ""
        if location.startswith("crates/"):
            parts = location.split("/")
            return f"panic: {parts[1]}::{Path(parts[-1].split(':')[0]).stem}"
        return "failed: " + (attributions.get(rust["id"]) or rust["fatal"]["class"])
    for domain in DOMAINS:
        value = result[domain]
        if value["category"] == "failed":
            return "failed: " + value["reason"].split(":")[0]
    errors = result["errors"]
    if errors["category"] == "different":
        if errors["differences"] == ["native_pre_post"] or "native_pre_post" in errors["differences"]:
            return "emit order: native pre/post-emit sets differ"
        return f"diagnostics: TS{errors['first_code']}" if errors.get("first_code") is not None else "diagnostics: rendering"
    for domain in DOMAINS[1:]:
        if result[domain]["category"] == "different":
            return "different: " + domain
    return "match"


def owner(row, result, bucket):
    """The checkpoint that owns completing this row."""
    if bucket.startswith("unsupported: content-mapper"):
        return "Phase 5 (content mapper)"
    if bucket.startswith("emit order"):
        return "C5 / Phase 3 (emit order)"
    if bucket == "different: trace":
        return "C0 gate, Phase 1 resolver"
    if bucket in ("different: union_ordering",):
        return "C2"
    if row["checkpoint"] != "regression":
        return row["checkpoint"]
    # A regression-subset row that no longer matches reopens the foundations.
    return "C1"


def configuration(row):
    options = row["options"]
    return {"module": MODULE.get(options.get("module"), "unset"), "target": TARGET.get(options.get("target"), "unset"),
            "jsx": JSX.get(options.get("jsx"), "unset"), "strict": options.get("strict") is True}


def load_rust(directory):
    replayed = phase2_corpus.replay(directory)
    requests = strict_json_loads((Path(directory) / "requests.json").read_bytes())
    rows = [p4.read(Path(directory) / "cases" / f"{i:05d}" / "result.json")["row"] for i in range(len(requests))]
    harness = {entry["id"]: entry["problem"] for entry in replayed["harness_errors"]}
    attributions = {entry["id"]: entry["attribution"] for entry in replayed["production_failures"]}
    return replayed, requests, rows, harness, attributions


def domain_digest(native, rust, domain):
    if "fatal" in rust:
        return digest(canonical(rust["fatal"]))
    if domain == "errors":
        return digest(canonical(rust["error_baseline"]))
    if domain in ("types", "symbols", "display"):
        walk = rust["type_symbol_baselines"]
        if walk["state"] != "executed":
            return digest(canonical(walk))
        if domain == "display":
            return digest(canonical(walk.get("public_type_strings")))
        pulls = TYPE_PULLS if domain == "types" else SYMBOL_PULLS
        return digest(canonical([walk[domain], query_domain(walk["queries"], pulls)]))
    value = rust["phase2"]
    return digest(canonical(value.get(domain, value)))


def report(native_dir, rust_dir, previous=None, record=False):
    native_dir, native_report, native_rows = phase2_native.load_capture(native_dir)
    phase2_native.current(native_report)
    replayed, requests, rust_rows, harness, attributions = load_rust(rust_dir)
    partial = replayed["summary"]["partial"]
    if record and partial:
        raise ValueError("a partial Rust run is informational and cannot be recorded as acceptance")
    all_inventory = phase2_inventory.executed()
    if [r["id"] for r in all_inventory] != [r["id"] for r in native_rows]:
        raise ValueError("native rows differ from the full executed inventory")
    metadata = p4.read(Path(rust_dir) / "capture.json")
    if (metadata["inventory_sha256"] != digest(phase2_inventory.INVENTORY.read_bytes())
            or metadata["native"]["observation_sha256"] != native_report["observation_sha256"]):
        raise ValueError("Rust capture belongs to a different inventory or native capture")
    inventory_rows = phase2_corpus.select_rows(all_inventory, **replayed["selection"])
    wanted = {row["id"] for row in inventory_rows}
    native_rows = [row for row in native_rows if row["id"] in wanted]
    if not ([r["id"] for r in inventory_rows] == [r["id"] for r in requests] == [r["id"] for r in rust_rows]):
        raise ValueError("inventory, native and Rust rows are missing, extra or reordered")
    earlier = strict_json_loads(Path(previous).read_bytes()) if previous else None
    rows, categories = [], {domain: Counter() for domain in DOMAINS}
    buckets = defaultdict(list)
    dimensions = {name: Counter() for name in ("family", "module", "target", "jsx", "strict", "first_code", "area")}
    unsupported_operations = Counter()
    for inventory, native, rust in zip(inventory_rows, native_rows, rust_rows, strict=True):
        result = compare_row(native, rust, harness.get(inventory["id"]), attributions.get(inventory["id"]))
        if result is None:
            rows.append({"id": inventory["id"], "harness_error": harness[inventory["id"]]})
            continue
        for domain in DOMAINS:
            categories[domain][result[domain]["category"]] += 1
            if result[domain]["category"] == "unsupported":
                unsupported_operations[result[domain]["operation"]] += 1
        matched = all(result[d]["category"] in ("match", "disabled") for d in DOMAINS)
        entry = {"id": inventory["id"], "checkpoint": inventory["checkpoint"], "s08": inventory["s08_tier"],
                 "outcomes": {d: result[d]["category"] for d in DOMAINS},
                 "digests": {d: domain_digest(native, rust, d) for d in DOMAINS}}
        if not matched:
            bucket = area(result, rust, attributions)
            entry["bucket"] = bucket
            entry["owner"] = owner(inventory, result, bucket)
            entry["details"] = {d: result[d] for d in DOMAINS if result[d]["category"] not in ("match", "disabled")}
            buckets[(entry["owner"], bucket)].append(inventory)
            for family in inventory["families"] or ["(none)"]:
                dimensions["family"][family] += 1
            for key, value in configuration(inventory).items():
                dimensions[key][str(value)] += 1
            code = result["errors"].get("first_code")
            dimensions["first_code"][str(code) if code is not None else "(none)"] += 1
            dimensions["area"][bucket] += 1
        rows.append(entry)
    changed = []
    if earlier:
        before = {row["id"]: row for row in earlier["rows"]}
        for row in rows:
            old = before.get(row["id"])
            if not old or "digests" not in row or "digests" not in old:
                continue
            for domain in DOMAINS:
                if (old["outcomes"][domain] == row["outcomes"][domain] and row["outcomes"][domain] == "different"
                        and old["digests"][domain] != row["digests"][domain]):
                    changed.append({"id": row["id"], "domain": domain})
    regression = [row for row in rows if row.get("s08") == "acceptance" and "outcomes" in row]
    executed = [row for row in rows if "outcomes" in row]
    summary = {
        "version": 1, "pin": native_report["pin"],
        "native_observation_sha256": native_report["observation_sha256"],
        "rust_capture_sha256": replayed["capture_sha256"], "rust_source_stable": replayed["source_stable"],
        "inventory_sha256": digest(phase2_inventory.INVENTORY.read_bytes()),
        "partial": partial, "selection": replayed["selection"],
        "executed": len(rows), "harness_errors": len(harness),
        "categories": {d: dict(sorted(categories[d].items())) for d in DOMAINS},
        "all_domains_match": sum(all(o in ("match", "disabled") for o in row["outcomes"].values()) for row in executed),
        "regression_subset": {"rows": len(regression),
                              "all_domains_match": sum(all(o in ("match", "disabled") for o in row["outcomes"].values())
                                                       for row in regression),
                              "by_domain": {d: dict(sorted(Counter(row["outcomes"][d] for row in regression).items()))
                                            for d in DOMAINS}},
        "by_checkpoint": {name: dict(sorted(Counter("match" if all(o in ("match", "disabled") for o in row["outcomes"].values())
                                                    else "open" for row in executed if row["checkpoint"] == name).items()))
                          for name in ("regression", "C2", "C3", "C4")},
        "unsupported_operations": dict(unsupported_operations.most_common()),
        "buckets": [{"owner": key[0], "bucket": key[1], "variants": len(members),
                     "representative": min(members, key=lambda r: (r["source_bytes"], r["id"]))["id"]}
                    for key, members in sorted(buckets.items(), key=lambda kv: (-len(kv[1]), kv[0]))],
        "dimensions": {name: dict(counter.most_common()) for name, counter in dimensions.items()},
        "changed_observations": changed,
        "previous": digest(Path(previous).read_bytes()) if previous else None,
    }
    full = dict(summary, rows=rows)
    Path(rust_dir, "comparison.json").write_bytes(canonical(full) + b"\n")
    if record:
        if not replayed["source_stable"]:
            raise ValueError("record requires a Rust capture of the current sources")
        RECORD.write_bytes(json.dumps(summary, indent=1, sort_keys=True).encode() + b"\n")
    brief = {k: summary[k] for k in ("partial", "executed", "harness_errors", "all_domains_match", "categories")}
    brief["regression_all_domains_match"] = summary["regression_subset"]["all_domains_match"]
    brief["buckets"] = len(summary["buckets"])
    print(json.dumps(brief, sort_keys=True, indent=1))
    return full


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("report")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--rust", type=Path, required=True)
    sub.add_argument("--previous", type=Path, help="an earlier comparison.json to diff row observations against")
    sub.add_argument("--record", action="store_true", help="write data/phase2/first-comparison.json")
    args = parser.parse_args()
    report(args.native, args.rust, args.previous, args.record)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 compare failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
