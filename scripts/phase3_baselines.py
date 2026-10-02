#!/usr/bin/env python3
"""Phase 3 C1: the witness of the ported emit baseline writers.

The writers (`tools/phase3/harness/baselines.rs`) port the runner's `.js`,
`.js.map` and `.sourcemap.txt` composition. The witness
(`crates/tsr_compiler/examples/phase3_baselines.rs`) feeds them the pin's own
outputs from a native emit capture taken with `--texts`
(`scripts/phase3_native.py capture --texts`) and compares what they compose
with the pin's baselines, domain by domain. The Rust program of each row
answers the writers' program queries and runs the declaration
re-compilation; nothing the Rust emitter computes is read.

    run      --native DIR --output DIR [--limit N] [--case ID ...] [--jobs J]
    mutate   --native DIR --output DIR [--jobs J]
    fixtures --native DIR --output DIR (--write | --check)

`run` writes `rows.ndjson` (one witness row per selected native row) and
`report.json` (outcomes per domain and native state, difference buckets with
three example ids each). `mutate` breaks one writer branch at a time, rebuilds
the witness and requires it to report differences on a fixed subset; the
sources are restored afterwards. `fixtures` writes (or checks) the writers'
fixtures, `scripts/tests/fixtures/phase3/baseline-writers.json`: real rows
with what the writers read from the program and the emit result, which the
Rust writer tests (`crates/tsr_compiler/tests/phase3_baselines.rs`) compose
without loading a program. Outputs go under a scratch directory or
`target/phase3/c1-*`; nothing here records evidence.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase3_native  # noqa: E402

EXAMPLE = "phase3_baselines"
DOMAINS = ("output", "sourcemap", "sourcemap_record")
OUTCOMES = ("match", "different", "failed", "disabled", "unverifiable")
ROW_STATES = ("executed", "unexecuted", "failed")
INPUT_GROUPS = ("ts_config_files", "to_be_compiled", "other_files")
EXAMPLES = 3
WRITER_SOURCES = ("tools/phase3/harness/baselines.rs", "tools/phase3/harness/sourcemap_record.rs",
                  "tools/phase3/harness/patience.rs", "tools/phase3/harness/program_view.rs",
                  "tools/phase3/harness/declaration_program.rs",
                  "crates/tsr_compiler/examples/phase3_baselines.rs")


def build():
    """The witness executable, as Cargo reports it."""
    command = ["cargo", "build", "--release", "-p", "tsr_compiler", "--example", EXAMPLE,
               "--message-format=json-render-diagnostics"]
    env = dict(os.environ, CARGO_INCREMENTAL="0")
    completed = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, check=False)
    executable = None
    for line in completed.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact" and message.get("executable")
                and message["target"]["name"] == EXAMPLE):
            executable = message["executable"]
    if completed.returncode or executable is None:
        raise ValueError("the witness did not build")
    return Path(executable)


def native_rows(native_dir):
    directory, report, observed = phase3_native.load_capture(native_dir, partial=True)
    if report.get("texts") is not True:
        raise ValueError("the witness needs a native capture taken with --texts")
    requests = strict_json_loads((directory / "requests.json").read_bytes())
    if [row["id"] for row in requests] != [row["id"] for row in observed]:
        raise ValueError("native capture rows do not follow its requests")
    return directory, report, requests, observed


def select(observed, limit=None, cases=()):
    if cases:
        wanted = set(cases)
        if len(wanted) != len(cases):
            raise ValueError("duplicate --case id")
        chosen = [index for index, row in enumerate(observed) if row["id"] in wanted]
        if len(chosen) != len(wanted):
            raise ValueError("unknown --case id")
        return chosen
    indices = list(range(len(observed)))
    return indices if limit is None else indices[:limit]


def subfolder(request):
    return "conformance" if "/tests/cases/conformance/" in request["path"] else "compiler"


def witness_request(request, row, loading, mode, dump_facts=False):
    """One witness line: the native row and the first compilation's loading
    request with the runner's input groups (`phase3_corpus.build_request`).
    `dump_facts` asks for what the writers read from the program and the emit
    result, which the writers' fixtures carry."""
    inputs = row.get("baseline_inputs", {})
    line = {"id": row["id"], "configured_name": request["configured_name"], "subfolder": subfolder(request),
            "mode": mode, "loading": loading, "native": row,
            "error_inputs": [item for group in INPUT_GROUPS for item in inputs.get(group, [])]}
    if dump_facts:
        line["dump_facts"] = True
    return line


def loading_requests(ids):
    """The Phase 2 loading requests of the selected rows, bound by the
    inventory's digests."""
    import phase2_corpus
    import phase2_inventory
    import s08_p4 as p4

    loading = phase2_corpus.loading_requests()
    rows = json.loads(phase2_inventory.INVENTORY.read_bytes())["rows"]
    digests = {row["id"]: row["loading_request_sha256"] for row in rows if row["tier"] == "executed"}
    result = {}
    for vid in ids:
        request = loading[vid]
        if digest(p4.canonical(request) + b"\n") != digests[vid]:
            raise ValueError("loading request differs from the inventory: " + vid)
        result[vid] = request
    return result


def run_shard(binary, directory, lines):
    directory.mkdir(parents=True, exist_ok=False)
    with (directory / "requests.ndjson").open("wb") as stream:
        for line in lines:
            stream.write(json.dumps(line, separators=(",", ":")).encode() + b"\n")
    with (directory / "stderr").open("wb") as err:
        completed = subprocess.run([str(binary), str(directory / "requests.ndjson"), str(directory / "rows.ndjson")],
                                   stdout=subprocess.DEVNULL, stderr=err, check=False)
    if completed.returncode:
        raise ValueError(f"witness shard failed (exit {completed.returncode}); see {directory}")
    rows = [strict_json_loads(line) for line in (directory / "rows.ndjson").read_bytes().splitlines()]
    if [row["id"] for row in rows] != [line["id"] for line in lines]:
        raise ValueError("witness rows are missing, extra or reordered: " + str(directory))
    (directory / "requests.ndjson").unlink()
    return rows


def validate_row(row):
    """The witness row contract; raises on a malformed row."""
    if row.get("state") not in ROW_STATES:
        raise ValueError("unknown witness row state: " + str(row.get("id")))
    if row["state"] == "failed":
        if not row.get("reason") or row.get("class") not in ("witness", "panic"):
            raise ValueError("witness failure without a class and reason: " + row["id"])
        return
    if row["state"] == "unexecuted":
        return
    for domain in DOMAINS:
        item = row.get(domain)
        if not isinstance(item, dict) or item.get("outcome") not in OUTCOMES:
            raise ValueError(f"malformed {domain} outcome: {row['id']}")
        if item["outcome"] == "failed" and not item.get("reason"):
            raise ValueError(f"{domain} failure without a reason: {row['id']}")
    if row.get("order", {}).get("state") not in ("match", "different"):
        raise ValueError("output order not compared: " + row["id"])


def bucket(domain, item):
    outcome = item["outcome"]
    if outcome == "different":
        if item["native_state"] != item["rust_state"]:
            return f"{domain}: different state {item['native_state']} -> {item['rust_state']}"
        if item.get("native_name") != item.get("rust_name"):
            return f"{domain}: different baseline name"
        return f"{domain}: different text"
    if outcome == "failed":
        return f"{domain}: failed: {str(item['reason'])[:120]}"
    if outcome == "unverifiable":
        verdict = "matches" if item.get("without_repeat_matches") else "differs"
        return f"{domain}: unverifiable (the composition without the repeat blocks {verdict})"
    return None


def summarize(rows, observed):
    native = {row["id"]: row for row in observed}
    states = Counter(row["state"] for row in rows)
    domains = {domain: defaultdict(Counter) for domain in DOMAINS}
    buckets = defaultdict(list)
    details = {}
    order, source_maps, declaration = Counter(), Counter(), Counter()
    dts_file_errors = {"native": [], "composed": []}
    for row in rows:
        if row["state"] == "failed":
            key = f"row: {row['class']}: {str(row['reason'])[:120]}"
            buckets[key].append(row["id"])
            details.setdefault(key, {"reason": row["reason"], "location": row.get("location")})
            continue
        if row["state"] != "executed":
            continue
        order[row["order"]["state"]] += 1
        if row["order"]["state"] != "match":
            buckets["order: different"].append(row["id"])
            details.setdefault("order: different", row["order"])
        source_maps[row["source_maps"]["state"]] += 1
        if row["source_maps"]["state"] == "failed":
            key = f"source maps: {str(row['source_maps']['reason'])[:120]}"
            buckets[key].append(row["id"])
            details.setdefault(key, row["source_maps"])
        declaration[row.get("declaration", {}).get("state", "not_run")] += 1
        if row.get("declaration", {}).get("state") == "failed":
            key = f"declaration: {str(row['declaration']['reason'])[:120]}"
            buckets[key].append(row["id"])
            details.setdefault(key, row["declaration"])
        if row.get("declaration", {}).get("diagnostics", 0) > 0:
            dts_file_errors["composed"].append(row["id"])
        output = native[row["id"]]["output"]
        if output.get("state") == "content" and b"//// [DtsFileErrors]" in bytes.fromhex(output["text_hex"]):
            dts_file_errors["native"].append(row["id"])
        for domain in DOMAINS:
            item = row[domain]
            native_state = native[row["id"]][domain]["state"]
            domains[domain][item["outcome"]][native_state] += 1
            key = bucket(domain, item)
            if key is not None:
                buckets[key].append(row["id"])
                details.setdefault(key, item)
    return {
        "rows": len(rows), "states": dict(sorted(states.items())),
        "domains": {domain: {outcome: dict(sorted(by_state.items())) for outcome, by_state in sorted(values.items())}
                    for domain, values in domains.items()},
        "order": dict(order), "source_maps": dict(source_maps), "declaration": dict(declaration),
        "dts_file_errors": {"native": len(dts_file_errors["native"]), "composed": len(dts_file_errors["composed"]),
                            "native_only": sorted(set(dts_file_errors["native"]) - set(dts_file_errors["composed"])),
                            "composed_only": sorted(set(dts_file_errors["composed"]) - set(dts_file_errors["native"]))},
        "buckets": [{"bucket": key, "count": len(ids), "examples": ids[:EXAMPLES], "detail": details[key]}
                    for key, ids in sorted(buckets.items(), key=lambda item: (-len(item[1]), item[0]))],
    }


def witness(native_dir, output, *, limit=None, cases=(), jobs=4, binary=None, dump_facts=False):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    directory, report, requests, observed = native_rows(native_dir)
    chosen = select(observed, limit, cases)
    ids = [observed[index]["id"] for index in chosen]
    loading = loading_requests(ids)
    binary = binary or build()
    lines = [witness_request(requests[index], observed[index], loading[observed[index]["id"]], report["mode"],
                             dump_facts) for index in chosen]
    shards = max(1, min(jobs, len(lines)))
    size = -(-len(lines) // shards) if lines else 0
    groups = [lines[start:start + size] for start in range(0, len(lines), size)] if lines else []
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(run_shard, binary, output / "shards" / f"{number:03d}", group)
                   for number, group in enumerate(groups)]
        rows = [row for future in futures for row in future.result()]
    for row in rows:
        validate_row(row)
    with (output / "rows.ndjson").open("wb") as stream:
        for row in rows:
            stream.write(canonical(row) + b"\n")
    summary = summarize(rows, [observed[index] for index in chosen])
    document = {
        "version": 1,
        "native": {"directory": str(directory), "report_sha256": digest((directory / "report.json").read_bytes()),
                   "observation_sha256": report["observation_sha256"], "mode": report["mode"], "texts": report["texts"]},
        "witness": {"binary_sha256": digest(binary.read_bytes()),
                    "sources": {name: digest((ROOT / name).read_bytes()) for name in WRITER_SOURCES}},
        "selection": {"limit": limit, "cases": list(cases), "rows": len(rows), "partial": len(rows) != len(observed)},
        **summary,
    }
    (output / "report.json").write_bytes(json.dumps(document, indent=1, sort_keys=True).encode() + b"\n")
    brief = {key: document[key] for key in ("rows", "states", "domains", "order", "source_maps", "declaration")}
    brief["buckets"] = [{"bucket": item["bucket"], "count": item["count"], "examples": item["examples"]}
                        for item in document["buckets"]]
    print(json.dumps(brief, indent=1, sort_keys=True))
    return document


# One broken branch each: the declaration block's separator, the source-map
# preview link's final newline, the record's marker padding, and the
# DtsFileErrors gate. Each anchor occurs exactly once.
MUTANTS = (
    ("dts_separator", "tools/phase3/harness/baselines.rs",
     "    if !result.dts.is_empty() {\n        js_code.extend_from_slice(b\"\\r\\n\\r\\n\");",
     "    if !result.dts.is_empty() {\n        js_code.extend_from_slice(b\"\\r\\n\");"),
    ("preview_link_newline", "tools/phase3/harness/baselines.rs",
     "    hash.push(b'\\n');\n    Ok(hash)", "    Ok(hash)"),
    ("record_marker_padding", "tools/phase3/harness/sourcemap_record.rs",
     "        if marker_id.len() < 2 {\n            marker_id.push(b' ');\n        }\n", ""),
    ("dts_file_errors_gate", "tools/phase3/harness/baselines.rs",
     "        if compiled.diagnostics > 0 {", "        if compiled.diagnostics > 0 || compiled.diagnostics == 0 {"),
)


def mutation_cases(observed):
    """Every row with a source-map baseline or a DtsFileErrors block, and the
    first 300 rows."""
    chosen = set(row["id"] for row in observed[:300])
    for row in observed:
        if row.get("state") != "executed":
            continue
        if any(row[domain]["state"] == "content" for domain in DOMAINS[1:]):
            chosen.add(row["id"])
        output = row["output"]
        if output.get("state") == "content" and b"//// [DtsFileErrors]" in bytes.fromhex(output["text_hex"]):
            chosen.add(row["id"])
    return [row["id"] for row in observed if row["id"] in chosen]


def mutate(native_dir, output, jobs=4):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    _, _, _, observed = native_rows(native_dir)
    cases = mutation_cases(observed)
    results = []
    for name, path, before, after in MUTANTS:
        source = ROOT / path
        original = source.read_bytes()
        text = original.decode()
        if text.count(before) != 1:
            raise ValueError(f"mutant anchor occurs {text.count(before)} times: {name}")
        try:
            source.write_bytes(text.replace(before, after).encode())
            binary = build()
            document = witness(native_dir, output / name, cases=cases, jobs=jobs, binary=binary)
        finally:
            source.write_bytes(original)
        different = {domain: sum(by_state for outcome, states in document["domains"][domain].items()
                                 if outcome != "match" and outcome != "disabled" for by_state in states.values())
                     for domain in DOMAINS}
        results.append({"mutant": name, "file": path, "rows": document["rows"], "not_matching": different,
                        "killed": any(different.values())})
    build()
    summary = {"version": 1, "cases": len(cases), "mutants": results, "all_killed": all(r["killed"] for r in results)}
    (output / "mutation.json").write_bytes(json.dumps(summary, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps(summary, indent=1, sort_keys=True))
    if not summary["all_killed"]:
        raise ValueError("a mutant survived")
    return summary


FIXTURES = ROOT / "scripts/tests/fixtures/phase3/baseline-writers.json"
# One real row per writer path the tests pin: kind -> variant id.
FIXTURE_CASES = (
    ("plain", "compiler/2dArrays.ts#configuration=0"),
    ("declarations", "compiler/isolatedModulesDeclaration.ts#configuration=0"),
    ("sourcemap_and_record", "compiler/sourceMap-NewLine1.ts#configuration=0"),
    ("no_content", "compiler/checkJsFiles6.ts#configuration=0"),
    ("full_emit_paths", "compiler/declarationEmitWithComposite.ts#configuration=0"),
    ("json_output", "compiler/isolatedModules_resolveJsonModule_strict_outDir_commonJs.ts#configuration=0"),
    ("bom", "compiler/emitBOM.ts#configuration=0"),
    ("dts_file_errors", "compiler/declarationEmitToDeclarationDirWithDeclarationOption.ts#configuration=0"),
    ("inline_sources_map_root", "compiler/optionsSourcemapInlineSourcesMapRoot.ts#configuration=0"),
    ("inline_source_map", "compiler/jsFileCompilationWithMapFileAsJsWithInlineSourceMap.ts#configuration=0"),
    ("declaration_map_content_mapper", "compiler/contentMapperDeclarationEmitFailure.ts#configuration=0"),
)
NATIVE_FIXTURE_FIELDS = ("id", "baseline_inputs", "options", "harness_options", "diagnostics", "outputs", *DOMAINS)


def fixture_document(native_dir, output, jobs=4):
    """The fixtures, rebuilt from a native capture: every fixture row must
    match in all three domains."""
    _, report, requests, observed = native_rows(native_dir)
    ids = [vid for _, vid in FIXTURE_CASES]
    document = witness(native_dir, Path(output) / "fixtures", cases=ids, jobs=jobs, dump_facts=True)
    rows = {}
    with (Path(output) / "fixtures" / "rows.ndjson").open("rb") as stream:
        for line in stream:
            row = strict_json_loads(line)
            rows[row["id"]] = row
    native = {row["id"]: (index, row) for index, row in enumerate(observed)}
    fixtures = []
    for kind, vid in FIXTURE_CASES:
        row = rows[vid]
        if row["state"] != "executed" or any(row[domain]["outcome"] != "match" for domain in DOMAINS):
            raise ValueError(f"fixture row does not match the native baselines: {vid}")
        index, native_row = native[vid]
        request = requests[index]
        fixtures.append({"kind": kind, "id": vid, "configured_name": request["configured_name"],
                         "subfolder": subfolder(request), "facts": row["facts"],
                         "declaration": row.get("declaration", {"state": "none"}),
                         "native": {field: native_row[field] for field in NATIVE_FIXTURE_FIELDS}})
    del document
    return {"version": 1, "pin": report["pin"], "mode": report["mode"], "fixtures": fixtures}


def fixtures(native_dir, output, *, write=False, jobs=4):
    document = fixture_document(native_dir, output, jobs)
    text = json.dumps(document, indent=1, sort_keys=True).encode() + b"\n"
    if write:
        FIXTURES.parent.mkdir(parents=True, exist_ok=True)
        FIXTURES.write_bytes(text)
        print(f"wrote {len(document['fixtures'])} fixtures to {FIXTURES.relative_to(ROOT)}")
    elif not FIXTURES.exists() or FIXTURES.read_bytes() != text:
        raise ValueError("the writers' fixtures are stale; rerun `fixtures --write`")
    else:
        print(f"{len(document['fixtures'])} fixtures current")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("run")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--limit", type=int)
    sub.add_argument("--case", action="append", default=[])
    sub.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    sub = commands.add_parser("fixtures")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--output", type=Path, required=True)
    mode = sub.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--check", action="store_true")
    sub.add_argument("--jobs", type=int, default=4)
    sub = commands.add_parser("mutate")
    sub.add_argument("--native", type=Path, required=True)
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    args = parser.parse_args()
    if args.command == "run":
        if args.limit is not None and (args.limit <= 0 or args.case):
            parser.error("--limit must be positive and cannot be combined with --case")
        witness(args.native, args.output, limit=args.limit, cases=args.case, jobs=args.jobs)
    elif args.command == "fixtures":
        fixtures(args.native, args.output, write=args.write, jobs=args.jobs)
    else:
        mutate(args.native, args.output, args.jobs)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 baselines failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
