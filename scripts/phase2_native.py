#!/usr/bin/env python3
"""Phase 2 C0.2: capture the native checker contract for every executed variant.

The oracle is the S08 access-only overlay (harness stage hooks, the original
baseline writers, the walker's public pulls and TypeToString schedule, the
native error selection) with three more observations generated mechanically
from the pinned runner: `verifyUnionOrdering` over every checker of the
program, `verifyParentPointers`, and the module-resolution trace that
`verifyModuleResolution` baselines. Assertions become recorded verdicts; the
walk itself, its order and its stop-at-first-failure behavior are the pin's.
The capture runs in upstream's default single-threaded test-program mode.

    capture --output DIR [--shards N] [--scheme contiguous|interleaved] [--jobs J]
    verify  --capture DIR              # second sharding, identical row digests
    review  --capture DIR [--record]   # native outputs equal the committed references

`--record` writes data/phase2/native-provenance.json. Raw outputs stay in the
capture directory, referenced by digest.
"""
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
import s08_baselines  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_inventory  # noqa: E402

PROVENANCE = ROOT / "data/phase2/native-provenance.json"
TEST = "TestPhase2Checker"
DRIVER = "testrunner/phase2_checker_test.go"
SUBTESTS = "testrunner/phase2_subtests_test.go"
STAGES = s08_baselines.NATIVE_STAGES | {"native_ordering", "native_parents", "native_trace"}
SCRIPT_INPUTS = ("scripts/phase2_native.py", "scripts/phase2_inventory.py", "scripts/s08_baselines.py",
                 "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py", "scripts/s08_oracle.py",
                 "scripts/tracking-bootstrap.py", "scripts/s07_acceptance.py", "scripts/s08_manifest.py",
                 "data/s04/toolchains.toml", "data/upstream.json", "data/phase2/inventory.json",
                 "data/s07/subset.json", "tools/s08/oracle/baselines_test.go",
                 "tools/s08/oracle/baselines_bridge.go", "tools/s08/oracle/diagnostics_observer.go")


def contract_digest(row):
    """Row digest over the observed contract. Numeric walker type IDs are not
    part of it (S08 P0, s08_queries.action): they vary with process-shared
    harness caches, while every baseline byte does not."""
    queries = [{key: value for key, value in query.items() if key != "type_id"} for query in row.get("queries", [])]
    return digest(canonical(dict(row, queries=queries)))


def replace_exact(source, before, after, count=1):
    return s08_baselines.replace_exact(source, before, after, count)


def pinned_body(source, header, footer):
    """The text between an exact function header and its closing footer."""
    start = source.index(header) + len(header)
    end = source.index(footer, start)
    if source.count(header) != 1:
        raise ValueError("pinned anchor drift: " + header.strip())
    return source[start:end]


def subtests(upstream):
    """Union ordering and parent pointers from the pinned bodies, assertions as verdicts."""
    runner = (upstream / "tsc/internal/testrunner/compiler_runner.go").read_text()
    ordering = pinned_body(runner, 'func (c *compilerTest) verifyUnionOrdering(t *testing.T) {\n'
                                   '\tt.Run("union ordering", func(t *testing.T) {\n', "\t})\n}\n")
    ordering = replace_exact(ordering, "\t\tp.ForEachCheckerParallel(func(_ int, c *checker.Checker) {\n",
                             "\t\tp.ForEachCheckerParallel(func(_ int, c *checker.Checker) {\n"
                             "\t\t\tmu.Lock(); checkers++; mu.Unlock()\n")
    ordering = replace_exact(ordering, "\t\t\t\ttypes := union.Types()\n",
                             "\t\t\t\ttypes := union.Types()\n\t\t\t\tmu.Lock(); unions++; mu.Unlock()\n")
    for name in ("reversed", "shuffled"):
        ordering = replace_exact(
            ordering,
            f'assert.Assert(t, slices.Equal({name}, types), "compareTypes does not sort union types consistently")',
            f"check(slices.Equal({name}, types))")
    parents = pinned_body(runner, 'func (c *compilerTest) verifyParentPointers(t *testing.T) {\n'
                                  '\tt.Run("source file parent pointers", func(t *testing.T) {\n', "\t})\n}\n")
    parents = replace_exact(parents, "\t\t\tif n == nil {\n\t\t\t\treturn false\n\t\t\t}\n",
                            "\t\t\tif n == nil {\n\t\t\t\treturn false\n\t\t\t}\n\t\t\tnodes++\n")
    parents = replace_exact(parents, 'assert.Assert(t, n.Parent != nil, "parent node does not exist")',
                            'if !verdict(n.Parent != nil, "parent node does not exist") { return true }')
    parents = replace_exact(parents,
                            'assert.Assert(t, n.Parent == parent, "parent node does not match traversed parent: "'
                            '+n.Kind.String()+": "+elab)',
                            'if !verdict(n.Parent == parent, "parent node does not match traversed parent: "'
                            '+n.Kind.String()+": "+elab) { return true }')
    parents = replace_exact(parents, "\t\t\tn.ForEachChild(verifier)\n",
                            "\t\t\tif n.ForEachChild(verifier) { return true }\n")
    parents = replace_exact(parents, "\t\t\tparent = f.AsNode()\n\t\t\tf.AsNode().ForEachChild(verifier)\n",
                            "\t\t\tparent = f.AsNode()\n\t\t\tfiles++\n"
                            "\t\t\tif f.AsNode().ForEachChild(verifier) { break }\n")
    return f'''package testrunner

// Generated by scripts/phase2_native.py from the pinned compiler_runner.go
// bodies of verifyUnionOrdering and verifyParentPointers. Each assertion is a
// recorded verdict; the parent walk still stops at its first failure.
import (
	"math/rand/v2"
	"slices"
	"sync"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/testutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/baseline"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/tsbaseline"
)

var _ = rand.New

func phase2SingleThreaded() bool {{ return testutil.TestProgramIsSingleThreaded() }}

func phase2UnionOrdering(c *compilerTest) map[string]any {{
	var mu sync.Mutex
	checkers, unions, inconsistent := 0, 0, 0
	check := func(ok bool) {{
		if !ok {{
			mu.Lock(); inconsistent++; mu.Unlock()
		}}
	}}
{ordering}	return map[string]any{{"state": "executed", "checkers": checkers, "unions": unions, "inconsistent": inconsistent}}
}}

func phase2ParentPointers(c *compilerTest) map[string]any {{
	files, nodes := 0, 0
	var failure any
	verdict := func(ok bool, message string) bool {{
		if !ok && failure == nil {{
			failure = message
		}}
		return ok
	}}
{parents}	return map[string]any{{"state": "executed", "files": files, "nodes": nodes, "failure": failure}}
}}

// verifyModuleResolution and DoModuleResolutionBaseline, without the file comparison.
func phase2Trace(c *compilerTest) tsbaseline.S08Baseline {{
	if !c.options.TraceResolution.IsTrue() {{
		return tsbaseline.S08Baseline{{State: "disabled"}}
	}}
	if c.result.Trace == "" {{
		return tsbaseline.S08BaselineValue(baseline.NoContent)
	}}
	return tsbaseline.S08BaselineValue(c.result.Trace)
}}

var _ = ast.NodeIsSynthesized
'''


OBSERVER = """package harnessutil

import "github.com/microsoft/TypeScript/tsc/internal/ast"

// Phase 2 overlay hook: the post-emit program's declaration diagnostics, the
// ones the error baseline contains, observed where the harness appends them.
var Phase2ObserveDeclarations func([]*ast.Diagnostic)
"""


def overlay_sources(upstream):
    sources = s08_baselines.overlay_sources(upstream, walker_inputs=True, error_inputs=True, public_type_strings=True)
    harness = sources["testutil/harnessutil/harnessutil.go"]
    sources["testutil/harnessutil/harnessutil.go"] = replace_exact(
        harness,
        "\tif postProgram.Options().GetEmitDeclarations() {\n"
        "\t\tpostErrors = append(postErrors, postProgram.GetDeclarationDiagnostics(ctx, nil)...)\n\t}\n",
        "\tif postProgram.Options().GetEmitDeclarations() {\n"
        "\t\tphase2Declarations := postProgram.GetDeclarationDiagnostics(ctx, nil)\n"
        "\t\tif Phase2ObserveDeclarations != nil { Phase2ObserveDeclarations(phase2Declarations) }\n"
        "\t\tpostErrors = append(postErrors, phase2Declarations...)\n\t}\n")
    sources["testutil/harnessutil/phase2_observer.go"] = OBSERVER
    driver = sources.pop("testrunner/s08_baselines_test.go")
    driver = replace_exact(driver, "\t\t\t\tharnessutil.S08ObserveDiagnostics = nil\n",
                           "\t\t\t\tharnessutil.S08ObserveDiagnostics = nil\n"
                           "\t\t\t\tharnessutil.Phase2ObserveDeclarations = nil\n")
    driver = replace_exact(driver, "\t\t\tstage(\"native_parse\")\n",
                           "\t\t\tharnessutil.Phase2ObserveDeclarations = func(values []*ast.Diagnostic) {\n"
                           "\t\t\t\trestore := harnessutil.S08EnterObservation()\n"
                           "\t\t\t\trow[\"declaration_diagnostics\"] = s08ErrorDiagnostics(values)\n"
                           "\t\t\t\trestore()\n\t\t\t}\n"
                           "\t\t\tstage(\"native_parse\")\n")
    driver = replace_exact(driver, "func TestS08Baselines(t *testing.T) {", f"func {TEST}(t *testing.T) {{")
    for name in ("REQUESTS", "OUTPUT", "SUMMARY"):
        driver = replace_exact(driver, f'os.Getenv("S08_{name}")', f'os.Getenv("PHASE2_{name}")')
    driver = replace_exact(driver, '(request.AcceptanceTier != "acceptance" && request.AcceptanceTier != "informational")',
                           'request.AcceptanceTier != "executed"')
    driver = replace_exact(driver, 'if request.AcceptanceTier != "informational" || os.Getenv("S08_INCLUDE_INFORMATIONAL") != "1" {',
                           "{")
    driver = replace_exact(driver, "\t\t\tif t.Failed() {\n\t\t\t\t// Preserve the stage of a nonfatal native assertion",
                           '\t\t\trow["emit_declarations"] = c.result.Program.Options().GetEmitDeclarations()\n'
                           '\t\t\tstage("native_ordering")\n\t\t\trow["union_ordering"] = phase2UnionOrdering(c)\n'
                           '\t\t\tstage("native_parents")\n\t\t\trow["parent_pointers"] = phase2ParentPointers(c)\n'
                           '\t\t\tstage("native_trace")\n\t\t\trow["trace"] = phase2Trace(c)\n'
                           '\t\t\tstage("harness_observation")\n'
                           "\t\t\tif t.Failed() {\n\t\t\t\t// Preserve the stage of a nonfatal native assertion")
    driver = replace_exact(driver, '"rows": len(requests), "go": runtime.Version()',
                           '"rows": len(requests), "single_threaded": phase2SingleThreaded(), "go": runtime.Version()')
    sources[DRIVER] = driver
    sources[SUBTESTS] = subtests(upstream)
    return sources


def requests():
    """Executed rows in inventory order, with the S08 request fields."""
    document = phase2_inventory.read()
    subset = strict_json_loads((ROOT / "data/s07/subset.json").read_bytes())
    index = {variant["id"]: (case, variant) for case in subset["cases"] for variant in case["variants"]}
    pairs, result = [], []
    for row in phase2_inventory.executed(document):
        case, variant = index[row["id"]]
        name = Path(case["source"]["path"]).name
        extension = Path(name).suffix
        base = name[:-len(extension)]
        configured = variant["configured_name"]
        if not configured.startswith(base) or not configured.endswith(extension):
            raise ValueError("invalid configured filename: " + row["id"])
        label = configured[len(base):-len(extension)]
        if label and (not label.startswith("(") or not label.endswith(")")):
            raise ValueError("invalid configuration label: " + row["id"])
        pairs.append((case, variant))
        result.append({"id": row["id"], "path": case["source"]["path"], "acceptance_tier": "executed",
                       "raw_sha256": case["source"]["raw_sha256"], "loaded_sha256": case["source"]["loaded_sha256"],
                       "settings": case["source"]["configurations"][variant["configuration"]],
                       "configuration_name": label[1:-1] if label else "", "configured_name": configured})
    return document, subset, pairs, result


def input_digests():
    return {name: digest((ROOT / name).read_bytes()) for name in SCRIPT_INPUTS}


def build_oracle(directory, upstream, env):
    sources = overlay_sources(upstream)
    replacements = {}
    for name, source in sorted(sources.items()):
        path = directory / "overlay" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        replacements[str(upstream / "tsc/internal" / name)] = str(path)
    for name in (DRIVER, SUBTESTS):
        if (upstream / "tsc/internal" / name).exists():
            raise ValueError("overlay would replace a pinned source file: " + name)
    (directory / "overlay.json").write_bytes(canonical({"Replace": replacements}))
    binary = directory / "oracle.test"
    repo_flag = "-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath=" + str(directory / "unmatched-prefix")
    command = ["go", "test", "-c", "-o", str(binary), "-trimpath", "-mod=readonly", repo_flag,
               "-overlay", str(directory / "overlay.json"), "./internal/testrunner"]
    with (directory / "build.stdout").open("wb") as out, (directory / "build.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=upstream / "tsc", env=env, stdout=out, stderr=err, check=False)
    if completed.returncode or not binary.exists():
        raise ValueError("native oracle build failed; see " + str(directory / "build.stderr"))
    return {"command": command, "binary_sha256": digest(binary.read_bytes()),
            "overlay_sha256": {name: digest(source.encode()) for name, source in sorted(sources.items())}}


def shard_indices(count, shards, scheme):
    if shards < 1:
        raise ValueError("at least one shard")
    if scheme == "interleaved":
        return [list(range(start, count, shards)) for start in range(shards)]
    size = -(-count // shards)
    return [list(range(start, min(count, start + size))) for start in range(0, count, size)]


def run_shard(binary, directory, requests, env, upstream, timeout_minutes):
    directory.mkdir(parents=True)
    raw = canonical(requests) + b"\n"
    (directory / "requests.json").write_bytes(raw)
    shard_env = dict(env, PHASE2_REQUESTS=str(directory / "requests.json"),
                     PHASE2_OUTPUT=str(directory / "observations.ndjson"),
                     PHASE2_SUMMARY=str(directory / "go-summary.json"))
    shard_env.pop("TS_TEST_PROGRAM_SINGLE_THREADED", None)
    command = [str(binary), "-test.run", f"^{TEST}$", "-test.count=1", f"-test.timeout={timeout_minutes}m"]
    with (directory / "go.stdout").open("wb") as out, (directory / "go.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=upstream / "tsc/internal/testrunner", env=shard_env,
                                   stdout=out, stderr=err, check=False)
    if not (directory / "go-summary.json").exists():
        raise ValueError(f"native shard did not complete (exit {completed.returncode}); see {directory}")
    summary = strict_json_loads((directory / "go-summary.json").read_bytes())
    if summary["request_sha256"] != digest(raw) or summary["rows"] != len(requests):
        raise ValueError("native shard observed a different request inventory: " + str(directory))
    if summary.get("single_threaded") is not True:
        raise ValueError("native shard did not run upstream's single-threaded test programs")
    observed = [strict_json_loads(line) for line in (directory / "observations.ndjson").read_bytes().splitlines()]
    if [row["id"] for row in observed] != [row["id"] for row in requests]:
        raise ValueError("native shard rows are missing, extra or reordered: " + str(directory))
    return completed.returncode, summary, observed


def validate(pairs, requests, observed, file_observations):
    """Reject harness defects; return per-row input-identity mismatches."""
    if [row["id"] for row in observed] != [row["id"] for row in requests]:
        raise ValueError("missing, duplicate, extra or reordered native observation")
    mismatches = []
    for (_case, variant), request, result in zip(pairs, requests, observed, strict=True):
        state = result["state"]
        if state not in ("executed", "upstream_failed", "upstream_skipped", "harness_failed"):
            raise ValueError("unclassified native outcome: " + request["id"])
        if result["acceptance_tier"] != "executed":
            raise ValueError("native row changed its tier: " + request["id"])
        if state == "harness_failed":
            raise ValueError(f"native harness failed for {request['id']}: {result.get('failure')}")
        if state == "upstream_skipped":
            raise ValueError("an executed variant was skipped by the native option guard: " + request["id"])
        if state == "upstream_failed":
            failure = result.get("failure")
            if not isinstance(failure, dict) or failure.get("stage") not in STAGES or not failure.get("message"):
                raise ValueError("native failure lacks an explicit stage and message: " + request["id"])
            if result.get("execution_stage") != failure["stage"]:
                raise ValueError("native failure differs from its execution stage: " + request["id"])
            continue
        if result.get("failure") is not None or "panic" in result or result.get("execution_stage") != "complete":
            raise ValueError("executed native row carries failure metadata: " + request["id"])
        for key in ("raw_sha256", "loaded_sha256"):
            if result[key] != request[key]:
                raise ValueError("native observation read a different input: " + request["id"])
        for key in ("options", "harness_options"):
            if not s08_baselines.same_json_value(result[key], variant[key]):
                mismatches.append({"id": request["id"], "field": key})
        # S07 froze a dependency closure only for its eligible variants; the
        # newly included rows are bound by their loading-request digest.
        if variant["dependency_closure"] is not None:
            expected_files = [{"name": f["Name"], "path": f["Path"], "sha256": f["SHA256"], "bytes": f["Bytes"]}
                              for f in (file_observations[i] for i in variant["dependency_closure"])]
            if not s08_baselines.same_json_value(result["files"], expected_files):
                mismatches.append({"id": request["id"], "field": "loaded_files"})
        elif not isinstance(result["files"], list) or not result["files"]:
            raise ValueError("native row lacks its loaded files: " + request["id"])
        for key in ("types", "symbols", "errors", "trace"):
            item = result[key]
            if item["state"] not in ("content", "no_content", "disabled"):
                raise ValueError("unknown native baseline outcome: " + request["id"])
            if item["state"] == "content":
                bytes.fromhex(item["text_hex"])
            elif "text_hex" in item:
                raise ValueError("non-content outcome carries text: " + request["id"])
        disabled = variant["harness_options"]["NoTypesAndSymbols"]
        if any((result[key]["state"] == "disabled") != disabled for key in ("types", "symbols")):
            raise ValueError("baseline enablement differs from the frozen harness options: " + request["id"])
        if result["errors"]["state"] == "disabled":
            raise ValueError("error baseline cannot be disabled: " + request["id"])
        if (result["trace"]["state"] == "disabled") != (variant["options"].get("traceResolution") is not True):
            raise ValueError("trace enablement differs from traceResolution: " + request["id"])
        if type(result["emit_declarations"]) is not bool:
            raise ValueError("declaration request not observed: " + request["id"])
        if ("declaration_diagnostics" in result) != result["emit_declarations"]:
            raise ValueError("declaration diagnostics observed without the request, or missing: " + request["id"])
        ordering, parents = result["union_ordering"], result["parent_pointers"]
        if ordering.get("state") != "executed" or any(type(ordering.get(k)) is not int
                                                      for k in ("checkers", "unions", "inconsistent")):
            raise ValueError("malformed union-ordering observation: " + request["id"])
        if parents.get("state") != "executed" or any(type(parents.get(k)) is not int for k in ("files", "nodes")):
            raise ValueError("malformed parent-pointer observation: " + request["id"])
        for key in ("error_pre_diagnostics", "error_post_diagnostics", "error_diagnostics"):
            if not isinstance(result.get(key), list):
                raise ValueError("structured native diagnostics missing: " + request["id"])
        if not disabled and result.get("public_type_strings", {}).get("state") != "executed":
            raise ValueError("public TypeToString schedule missing: " + request["id"])
    return mismatches


def capture(output, shards, scheme, jobs, timeout_minutes, *, binary_from=None, limit=None):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    inputs = input_digests()
    document, subset, pairs, request_rows = requests()
    if limit is not None:
        pairs, request_rows = pairs[:limit], request_rows[:limit]
    raw = canonical(request_rows) + b"\n"
    (output / "requests.json").write_bytes(raw)
    if binary_from is None:
        oracle = build_oracle(output / "oracle", upstream, env)
        binary = output / "oracle/oracle.test"
    else:
        oracle = strict_json_loads((Path(binary_from) / "report.json").read_bytes())["oracle"]
        binary = Path(binary_from) / "oracle/oracle.test"
        if digest(binary.read_bytes()) != oracle["binary_sha256"]:
            raise ValueError("captured oracle binary changed")
    groups = shard_indices(len(request_rows), shards, scheme)
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(run_shard, binary, output / "shards" / f"{number:03d}",
                               [request_rows[i] for i in group], env, upstream, timeout_minutes)
                   for number, group in enumerate(groups)]
        results = [future.result() for future in futures]
    observed = [None] * len(request_rows)
    summaries = []
    for group, (code, summary, rows) in zip(groups, results, strict=True):
        if code not in (0, 1) or (code == 1 and not any(r["state"] == "upstream_failed" for r in rows)):
            raise ValueError("unexplained native test failure")
        summaries.append(summary)
        for index, row in zip(group, rows, strict=True):
            observed[index] = row
    with (output / "observations.ndjson").open("wb") as stream:
        for row in observed:
            stream.write(canonical(row) + b"\n")
    mismatches = validate(pairs, request_rows, observed, subset["file_observations"])
    verified_upstream()
    if input_digests() != inputs:
        raise ValueError("capture inputs changed during the native run")
    hosts = {(s["go"], s["goos"], s["goarch"]) for s in summaries}
    if len(hosts) != 1:
        raise ValueError("native shards ran on different toolchains")
    go, goos, goarch = hosts.pop()
    report = {
        "version": 1, "pin": document["pin"], "inputs": inputs, "oracle": oracle,
        "inventory_sha256": digest(phase2_inventory.INVENTORY.read_bytes()),
        "requests": len(request_rows), "request_sha256": digest(raw), "partial": limit is not None,
        "observation_sha256": digest((output / "observations.ndjson").read_bytes()),
        "row_sha256": [contract_digest(row) for row in observed],
        "row_digest": "sha256 of the canonical row without numeric walker type_id values",
        "sharding": {"shards": len(groups), "scheme": scheme, "jobs": jobs},
        "states": dict(Counter(row["state"] for row in observed)),
        "failure_stages": dict(Counter(row["failure"]["stage"] for row in observed if row["state"] == "upstream_failed")),
        "mismatches": mismatches,
        "go": go, "goos": goos, "goarch": goarch, "host": platform.platform(), "single_threaded": True,
        "scope": ("Pinned native checker observations for the Phase 2 denominator in upstream's default "
                  "single-threaded mode; no Rust comparison"),
    }
    (output / "report.json").write_bytes(canonical(report) + b"\n")
    print(json.dumps({k: report[k] for k in ("requests", "states", "failure_stages", "sharding")}, sort_keys=True))
    return report


def load_capture(directory, *, partial=False):
    directory = Path(directory).resolve()
    report = strict_json_loads((directory / "report.json").read_bytes())
    if report["partial"] and not partial:
        raise ValueError("a --limit smoke capture is never verified, reviewed or recorded")
    if digest((directory / "requests.json").read_bytes()) != report["request_sha256"]:
        raise ValueError("captured requests changed")
    raw = (directory / "observations.ndjson").read_bytes()
    if digest(raw) != report["observation_sha256"]:
        raise ValueError("captured observations changed")
    observed = [strict_json_loads(line) for line in raw.splitlines()]
    if [contract_digest(row) for row in observed] != report["row_sha256"]:
        raise ValueError("captured row digests changed")
    return directory, report, observed


def current(report):
    if report["inputs"] != input_digests():
        stale = sorted(name for name, value in report["inputs"].items() if input_digests().get(name) != value)
        raise ValueError("native capture is stale; changed inputs: " + ", ".join(stale))
    if report["oracle"]["overlay_sha256"] != {name: digest(text.encode()) for name, text
                                              in sorted(overlay_sources(verified_upstream()).items())}:
        raise ValueError("native oracle overlay changed since the capture")


def verify(directory, shards, scheme, jobs, timeout_minutes, *, partial=False):
    directory, report, observed = load_capture(directory, partial=partial)
    current(report)
    other = directory / f"verify-{scheme}-{shards}"
    if other.exists():
        shutil.rmtree(other)
    if (shards, scheme) == (report["sharding"]["shards"], report["sharding"]["scheme"]):
        raise ValueError("verification needs a different sharding than the capture")
    second = capture(other, shards, scheme, jobs, timeout_minutes, binary_from=directory,
                     limit=report["requests"] if report["partial"] else None)
    differing = [row["id"] for row, left, right in zip(observed, report["row_sha256"], second["row_sha256"], strict=True)
                 if left != right]
    if differing:
        raise ValueError(f"second sharding differs on {len(differing)} rows, first {differing[:5]}")
    raw_only = sum(left != right for left, right in zip(
        (directory / "observations.ndjson").read_bytes().splitlines(),
        (other / "observations.ndjson").read_bytes().splitlines(), strict=True))
    result = {"verified": True, "rows": len(observed), "capture_sharding": report["sharding"],
              "verify_sharding": second["sharding"], "observation_sha256": report["observation_sha256"],
              "rows_differing_only_in_type_ids": raw_only}
    (directory / "verified.json").write_bytes(canonical(result) + b"\n")
    print(json.dumps(result, sort_keys=True))
    return result


def git_blob(content):
    return hashlib.sha1(b"blob %d\0" % len(content) + content).hexdigest()


def reference(row, kind):
    blob = row["references"].get(kind)
    if blob is None:
        return None
    suite = row["suite"]
    stem = row["configured_name"].rsplit(".", 1)[0]
    path = ROOT / "upstream/tsc/testdata/baselines/reference" / suite / (stem + kind)
    content = path.read_bytes()
    if git_blob(content) != blob:
        raise ValueError(f"reference {path} differs from the frozen git blob")
    return content


def review(directory, record):
    directory, report, observed = load_capture(directory)
    current(report)
    verified = directory / "verified.json"
    if not verified.exists() or strict_json_loads(verified.read_bytes())["observation_sha256"] != report["observation_sha256"]:
        raise ValueError("review requires a verified capture (run verify first)")
    document, _subset, pairs, request_rows = requests()
    if digest(canonical(request_rows) + b"\n") != report["request_sha256"]:
        raise ValueError("inventory requests changed since the capture")
    disagreements, pre_post = [], []
    references = Counter()
    for row, result in zip(phase2_inventory.executed(document), observed, strict=True):
        if result["state"] != "executed":
            continue
        for kind, key in ((".errors.txt", "errors"), (".types", "types"), (".symbols", "symbols"), (".trace.json", "trace")):
            expected = reference(row, kind)
            actual = result[key]
            if actual["state"] == "disabled":
                ok = expected is None
            elif actual["state"] == "no_content":
                ok = expected is None
            else:
                ok = expected is not None and bytes.fromhex(actual["text_hex"]) == expected
            references[(kind, actual["state"], ok)] += 1
            if not ok:
                disagreements.append({"id": row["id"], "kind": kind, "native_state": actual["state"],
                                      "reference_present": expected is not None})
        if result["error_pre_diagnostics"] != result["error_post_diagnostics"]:
            pre_post.append(row["id"])
    executed_rows = [r for r in observed if r["state"] == "executed"]
    summary = {
        "version": 1, "pin": report["pin"], "capture_report_sha256": digest((directory / "report.json").read_bytes()),
        "request_sha256": report["request_sha256"], "observation_sha256": report["observation_sha256"],
        "inventory_sha256": report["inventory_sha256"], "inputs": report["inputs"], "oracle": report["oracle"],
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"], "host": report["host"],
        "single_threaded": report["single_threaded"],
        "requests": report["requests"], "states": report["states"], "failure_stages": report["failure_stages"],
        "input_mismatches": report["mismatches"],
        "sharding": {"capture": report["sharding"], "verify": strict_json_loads(verified.read_bytes())["verify_sharding"]},
        "reference_outcomes": [{"kind": k, "native_state": s, "agrees": ok, "variants": n}
                               for (k, s, ok), n in sorted(references.items())],
        "reference_disagreements": disagreements,
        "pre_post_different": pre_post,
        "declaration_requests": sum(r["emit_declarations"] for r in executed_rows),
        "trace_rows": sum(r["trace"]["state"] != "disabled" for r in executed_rows),
        "union_ordering_inconsistent": [r["id"] for r in executed_rows if r["union_ordering"]["inconsistent"]],
        "parent_pointer_failures": [r["id"] for r in executed_rows if r["parent_pointers"]["failure"] is not None],
        "row_sha256": report["row_sha256"],
    }
    (directory / "review.json").write_bytes(canonical(summary) + b"\n")
    if record:
        PROVENANCE.parent.mkdir(parents=True, exist_ok=True)
        PROVENANCE.write_bytes(canonical(summary) + b"\n")
    brief = {k: summary[k] for k in ("requests", "states", "declaration_requests", "trace_rows")}
    brief.update(reference_disagreements=len(disagreements), pre_post_different=len(pre_post),
                 union_ordering_inconsistent=len(summary["union_ordering_inconsistent"]),
                 parent_pointer_failures=len(summary["parent_pointer_failures"]),
                 input_mismatches=len(report["mismatches"]))
    print(json.dumps(brief, sort_keys=True))
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("capture", "verify"):
        sub = commands.add_parser(name)
        sub.add_argument("--shards", type=int, default=8 if name == "capture" else 5)
        sub.add_argument("--scheme", choices=("contiguous", "interleaved"),
                         default="contiguous" if name == "capture" else "interleaved")
        sub.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
        sub.add_argument("--timeout-minutes", type=int, default=240)
        if name == "capture":
            sub.add_argument("--output", type=Path, required=True)
            sub.add_argument("--limit", type=int, help="development smoke over the first N rows; never recorded")
        else:
            sub.add_argument("--capture", type=Path, required=True)
            sub.add_argument("--smoke", action="store_true", help="verify a --limit capture's sharding only")
    sub = commands.add_parser("review")
    sub.add_argument("--capture", type=Path, required=True)
    sub.add_argument("--record", action="store_true")
    sub = commands.add_parser("overlay", help="write the generated overlay sources for inspection")
    sub.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "capture":
        capture(args.output, args.shards, args.scheme, args.jobs, args.timeout_minutes, limit=args.limit)
    elif args.command == "verify":
        verify(args.capture, args.shards, args.scheme, args.jobs, args.timeout_minutes, partial=args.smoke)
    elif args.command == "review":
        review(args.capture, args.record)
    else:
        for name, source in overlay_sources(verified_upstream()).items():
            path = args.output / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("phase2 native failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
