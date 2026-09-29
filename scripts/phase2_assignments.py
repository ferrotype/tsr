#!/usr/bin/env python3
"""Phase 2 C6.7: checker-assignment witnesses on the capture host's GOOS/GOARCH.

The pinned compiler checker pool associates every program file with one of
its checkers by a weighted FENNEL partition of the import graph
(compiler/checkerpool.go). The recorder builds the C6.0 concurrent-mode
oracle with a diagnostic overlay (never an input of the canonical oracle) in
which `createCheckers` reports, per program, the inputs it read (file names,
node counts, text lengths, import counts, declaration flags, the import
adjacency) and what it computed (the policy regime, the final weights, the
stream order, the associations). It runs every executed corpus row in the
concurrent mode, and a synthetic set of graphs at 2, 4 and 8 checkers through
the same association step, chosen so that score ties, the least-loaded
fallback, the one-percent slack, every regime, import normalization and the
fused multiply-subtract of arm64's score arithmetic each decide an
assignment. The synthetic step is the pin's helper sequence over given inputs;
it is trusted only after it reproduces every corpus record.

    record  --output DIR [--shards N] [--jobs J] [--record]
    compare --capture DIR [--native DIR] [--jobs J] [--record]   # the Rust partitioner over the same programs
    fusion  [--record | --check]          # the arm64/amd64 score arithmetic

`record --record` writes data/phase2/c6-assignments.json: the host, the
toolchain, the overlay fingerprint, the synthetic set in full and the corpus
records by digest (they stay in the capture directory). `compare --record`
writes data/phase2/c6-assignment-comparison.json: the Rust result bound to
that record, to the Rust sources it was built from and to the host's machine,
which the checker producer requires current for `c6_assignments`. `fusion --record` writes
data/phase2/c6-fusion-arm64-go1.27.1.txt from the Go compiler's assembly;
`--check` requires the committed file to equal a fresh extraction.
"""
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from fractions import Fraction
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase2_native as native  # noqa: E402
import phase2_native_concurrent as concurrent  # noqa: E402
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

INPUTS = ROOT / "tools/phase2/assignments"
RECORD = ROOT / "data/phase2/c6-assignments.json"
COMPARISON = ROOT / "data/phase2/c6-assignment-comparison.json"
FUSION = ROOT / "data/phase2/c6-fusion-arm64-go1.27.1.txt"
EXAMPLE = "phase2_assignments"
TEST = "TestPhase2Assignments"
SCRIPT_INPUTS = (*concurrent.SCRIPT_INPUTS, "scripts/phase2_assignments.py",
                 "tools/phase2/assignments/assignments_hook.go", "tools/phase2/assignments/assignments_test.go")
POOL_ANCHOR = ("\t\t\tassociations = getCheckerAssociationsInOrder(fileWeights, adjacentFiles, fileOrder, "
               "checkerCount, policy.balancePenaltyMultiplier)\n\t\t}\n")
POOL_HOOK = ("\t\t\tassociations = getCheckerAssociationsInOrder(fileWeights, adjacentFiles, fileOrder, "
             "checkerCount, policy.balancePenaltyMultiplier)\n"
             "\t\t\tphase2RecordAssignments(p.program, checkerCount, importCounts, isDeclarationFile, "
             "adjacentFiles, &policy, fileWeights, fileOrder, associations)\n"
             "\t\t} else {\n"
             "\t\t\tphase2RecordAssignments(p.program, checkerCount, nil, nil, nil, nil, nil, nil, associations)\n"
             "\t\t}\n")
PLAN_FIELDS = ("checker_count", "node_counts", "text_lengths", "associations")
MULTI_FIELDS = ("import_counts", "is_declaration_file", "adjacency", "policy", "file_weights", "order")
STEP_OUTPUTS = ("policy", "file_weights", "order", "associations")


def input_digests():
    return {name: digest((ROOT / name).read_bytes()) for name in SCRIPT_INPUTS}


def overlay_sources(upstream):
    sources = native.overlay_sources(upstream)
    pool = (upstream / "tsc/internal/compiler/checkerpool.go").read_text()
    sources["compiler/checkerpool.go"] = native.replace_exact(pool, POOL_ANCHOR, POOL_HOOK)
    sources["compiler/phase2_assignments.go"] = (INPUTS / "assignments_hook.go").read_text()
    sources["compiler/phase2_assignments_test.go"] = (INPUTS / "assignments_test.go").read_text()
    driver = sources[native.DRIVER]
    driver = native.replace_exact(driver, '\t"github.com/microsoft/TypeScript/tsc/internal/ast"\n',
                                  '\t"github.com/microsoft/TypeScript/tsc/internal/ast"\n'
                                  '\t"github.com/microsoft/TypeScript/tsc/internal/compiler"\n')
    driver = native.replace_exact(driver, "\t\t\t\tharnessutil.Phase2ObserveDeclarations = nil\n",
                                  "\t\t\t\tharnessutil.Phase2ObserveDeclarations = nil\n"
                                  "\t\t\t\tcompiler.Phase2ObserveAssignments = nil\n")
    driver = native.replace_exact(driver, '\t\t\tstage("native_parse")\n',
                                  '\t\t\trow["assignments"] = []any{}\n'
                                  "\t\t\tcompiler.Phase2ObserveAssignments = func(record map[string]any) {\n"
                                  '\t\t\t\trow["assignments"] = append(row["assignments"].([]any), record)\n'
                                  "\t\t\t}\n"
                                  '\t\t\tstage("native_parse")\n')
    sources[native.DRIVER] = driver
    return sources


def build(directory, upstream, env):
    sources = overlay_sources(upstream)
    replacements = {}
    for name, source in sorted(sources.items()):
        path = directory / "overlay" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        replacements[str(upstream / "tsc/internal" / name)] = str(path)
    for name in (native.DRIVER, native.SUBTESTS, "compiler/phase2_assignments.go", "compiler/phase2_assignments_test.go"):
        if (upstream / "tsc/internal" / name).exists():
            raise ValueError("overlay would replace a pinned source file: " + name)
    (directory / "overlay.json").write_bytes(canonical({"Replace": replacements}))
    repo_flag = "-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath=" + str(directory / "unmatched-prefix")
    binaries = {}
    for name, package in (("oracle.test", "./internal/testrunner"), ("assignments.test", "./internal/compiler")):
        binary = directory / name
        command = ["go", "test", "-c", "-o", str(binary), "-trimpath", "-mod=readonly", repo_flag,
                   "-overlay", str(directory / "overlay.json"), package]
        with (directory / f"{name}.stdout").open("wb") as out, (directory / f"{name}.stderr").open("wb") as err:
            completed = subprocess.run(command, cwd=upstream / "tsc", env=env, stdout=out, stderr=err, check=False)
        if completed.returncode or not binary.exists():
            raise ValueError(f"overlay build failed; see {directory / (name + '.stderr')}")
        binaries[name] = {"command": command, "binary_sha256": digest(binary.read_bytes())}
    return {"binaries": binaries,
            "overlay_sha256": {name: digest(source.encode()) for name, source in sorted(sources.items())}}


def run_step(binary, directory, cases, env):
    """The association step of the overlay's compiler package over `cases`."""
    directory.mkdir(parents=True, exist_ok=True)
    raw = b"".join(canonical(case) + b"\n" for case in cases)
    (directory / "input.ndjson").write_bytes(raw)
    step_env = dict(env, PHASE2_ASSIGNMENT_INPUT=str(directory / "input.ndjson"),
                    PHASE2_ASSIGNMENT_OUTPUT=str(directory / "output.ndjson"),
                    PHASE2_ASSIGNMENT_SUMMARY=str(directory / "summary.json"))
    with (directory / "go.stdout").open("wb") as out, (directory / "go.stderr").open("wb") as err:
        completed = subprocess.run([str(binary), "-test.run", f"^{TEST}$", "-test.count=1"],
                                   cwd=directory, env=step_env, stdout=out, stderr=err, check=False)
    if completed.returncode:
        raise ValueError(f"association step failed; see {directory}")
    summary = strict_json_loads((directory / "summary.json").read_bytes())
    results = [strict_json_loads(line) for line in (directory / "output.ndjson").read_bytes().splitlines()]
    if summary["rows"] != len(cases) or [r["id"] for r in results] != [c["id"] for c in cases]:
        raise ValueError("association step rows are missing, extra or reordered")
    return summary, results


# The synthetic set. A Python replica of the association step selects cases
# (it never decides a result); the Go step's outputs are the evidence.

def _fma(a, b, c):
    return float(Fraction(a) * Fraction(b) + Fraction(c))


def _replica(case, fused):
    k = case["checker_count"]
    n = len(case["node_counts"])
    base = [max(case["node_counts"][i] + case["text_lengths"][i] // 100, 1) for i in range(n)]
    total = sum(base)
    declarations = sum(b for b, d in zip(base, case["is_declaration_file"]) if d)
    if declarations * k * 2 <= total:
        prioritize, multiplier, penalty = True, 1, 12
    elif k >= 4:
        prioritize, multiplier, penalty = False, 4, 16
    else:
        prioritize, multiplier, penalty = False, 1, 1
    base = [b * multiplier if not d else b for b, d in zip(base, case["is_declaration_file"])]
    imports = sum(case["import_counts"])
    unit = max(sum(base) // imports, 1) if imports else 0
    clamped = imports and sum(base) // imports == 0
    weights = [b + c * unit for b, c in zip(base, case["import_counts"])]
    order = (sorted(range(n), key=lambda i: (case["is_declaration_file"][i], -weights[i], i))
             if prioritize else list(range(n)))
    adjacency = case["adjacency"]
    total = sum(weights)
    edges = sum(len(a) for a in adjacency)
    average = (total + k - 1) // k
    maximum = max(max(weights), average + average // 100)
    alpha = float(penalty) * float(edges // 2) * math.sqrt(float(k)) / (float(total) * math.sqrt(float(total)))
    assoc, loads = [-1] * n, [0] * k
    features = set()
    for f in order:
        neighbors = [0] * k
        for a in adjacency[f]:
            if assoc[a] >= 0:
                neighbors[assoc[a]] += 1
        best, best_score = -1, -math.inf
        for c, load in enumerate(loads):
            if load + weights[f] > maximum:
                continue
            old, new = float(load), float(load + weights[f])
            if fused:
                score = _fma(-alpha, _fma(-old, math.sqrt(old), new * math.sqrt(new)), float(neighbors[c]))
            else:
                score = float(neighbors[c]) - alpha * (new * math.sqrt(new) - old * math.sqrt(old))
            if score == best_score:
                features.add("tie")
            if score > best_score or (score == best_score and (best < 0 or load < loads[best])):
                best, best_score = c, score
        if best < 0:
            features.add("fallback")
            best = min(range(k), key=lambda c: (loads[c], c))
        elif loads[best] + weights[f] > max(max(weights), average):
            features.add("slack")
        assoc[f] = best
        loads[best] += weights[f]
    features.add({(True, 1, 12): "source_dominated", (False, 4, 16): "declaration_heavy_strong",
                  (False, 1, 1): "declaration_heavy_small"}[(prioritize, multiplier, penalty)])
    if imports:
        features.add("imports")
    if clamped:
        features.add("import_unit_clamp")
    return assoc, features


class _Stream:
    """Deterministic integers from sha256 in counter mode (no `random`)."""

    def __init__(self, seed):
        self.seed, self.counter = seed, 0

    def below(self, bound):
        self.counter += 1
        value = int.from_bytes(hashlib.sha256(f"{self.seed}:{self.counter}".encode()).digest()[:8], "big")
        return value % bound


def _graph(stream, n, edges):
    adjacency = [[] for _ in range(n)]
    for _ in range(edges):
        a, b = stream.below(n), stream.below(n)
        if a != b:
            adjacency[a].append(b)
            adjacency[b].append(a)
    return adjacency


def _fusion_case(identifier, m):
    """A near-tie only rounding decides, with four checkers: file 0 (weight
    16) goes first to checker 0, file 1 (weight 9) has `m` edges to it. Loads
    16 -> 25 and 0 -> 9 make both load increments exact (61 and 27), the total
    weight 34^2 makes the penalty factor one rounded division, and `m` makes
    the two scores tie in real arithmetic; fused and unfused arithmetic then
    place file 1 differently. The other files only complete the weight and
    the edge count."""
    s, h = 34, None
    h = Fraction(m * s ** 3, 24 * 34)
    assert h.denominator == 1
    h = int(h)
    weights = [16, 9] + [9] * 125 + [6]
    assert sum(weights) == s * s
    n = len(weights)
    adjacency = [[] for _ in range(n)]
    for _ in range(m):
        adjacency[0].append(1)
        adjacency[1].append(0)
    for _ in range(h - m):
        adjacency[2].append(3)
        adjacency[3].append(2)
    return {"id": identifier, "checker_count": 4, "node_counts": weights, "text_lengths": [0] * n,
            "import_counts": [0] * n, "is_declaration_file": [False] * n, "adjacency": adjacency}


def synthetic_cases():
    cases = []
    for m in (66, 78):
        case = _fusion_case(f"fusion-m{m}", m)
        fused, _ = _replica(case, True)
        plain, _ = _replica(case, False)
        if fused == plain:
            raise ValueError("constructed fusion case does not depend on fusion: " + case["id"])
        cases.append(case)
    wanted = ("tie", "fallback", "slack", "source_dominated", "declaration_heavy_strong",
              "declaration_heavy_small", "imports", "import_unit_clamp")
    for k in (2, 4, 8):
        covered = Counter()
        stream = _Stream(f"c6-assignments-{k}")
        attempt = 0
        while any(covered[f] < 2 for f in wanted if not (f == "declaration_heavy_small" and k >= 4)
                  and not (f == "declaration_heavy_strong" and k < 4)):
            attempt += 1
            if attempt > 20000:
                raise ValueError(f"synthetic search did not cover every feature at {k} checkers: {dict(covered)}")
            n = k + 1 + stream.below(3 * k + 6)
            big = stream.below(4) == 0
            node_counts = [1 + stream.below(400 if big else 40) for _ in range(n)]
            text_lengths = [stream.below(3000) if stream.below(3) == 0 else 0 for _ in range(n)]
            declarations = [stream.below(3) == 0 for _ in range(n)]
            if stream.below(4) == 0:
                declarations = [stream.below(5) != 0 for _ in range(n)]
            imports = [stream.below(4) if stream.below(2) == 0 else 0 for _ in range(n)]
            if stream.below(3) == 0:
                imports = [0] * n
            if stream.below(5) == 0:
                node_counts = [node_counts[0]] * n
                text_lengths = [0] * n
            if stream.below(6) == 0:
                # More imports than base weight: the import unit clamps to 1.
                node_counts = [1 + stream.below(2) for _ in range(n)]
                text_lengths = [0] * n
                imports = [stream.below(12) for _ in range(n)]
            case = {"id": "", "checker_count": k, "node_counts": node_counts, "text_lengths": text_lengths,
                    "import_counts": imports, "is_declaration_file": declarations,
                    "adjacency": _graph(stream, n, stream.below(3 * n))}
            _, features = _replica(case, True)
            if not any(covered[f] < 2 for f in features & set(wanted)):
                continue
            covered.update(features & set(wanted))
            case["id"] = f"k{k}-{len(cases):03d}"
            case["features"] = sorted(features)
            cases.append(case)
    return cases


def step_input(case):
    return {key: case[key] for key in ("id", "checker_count", "node_counts", "text_lengths", "import_counts",
                                       "is_declaration_file", "adjacency")}


def normalize(plan):
    """The compared fields; absent and empty results are one value."""
    result = {key: plan.get(key) for key in PLAN_FIELDS if key in plan}
    if plan.get("checker_count", 0) > 1:
        for key in MULTI_FIELDS:
            result[key] = plan.get(key)
    return result


def record(output, shards, jobs, timeout_minutes, write_record):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    inputs = input_digests()
    built = build(output / "build", upstream, env)
    document, _subset, _pairs, request_rows = native.requests()
    groups = native.shard_indices(len(request_rows), shards, "contiguous")
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(concurrent.run_shard, output / "build/oracle.test", output / "shards" / f"{number:03d}",
                               [request_rows[i] for i in group], env, upstream, timeout_minutes)
                   for number, group in enumerate(groups)]
        results = [future.result() for future in futures]
    rows = [None] * len(request_rows)
    summaries = []
    for group, (code, summary, observed) in zip(groups, results, strict=True):
        if code not in (0, 1) or (code == 1 and not any(r["state"] == "upstream_failed" for r in observed)):
            raise ValueError("unexplained native test failure")
        summaries.append(summary)
        for index, row in zip(group, observed, strict=True):
            rows[index] = {"id": row["id"], "state": row["state"], "assignments": row.get("assignments", [])}
    with (output / "assignments.ndjson").open("wb") as stream:
        for row in rows:
            stream.write(canonical(row) + b"\n")
    hosts = {(s["go"], s["goos"], s["goarch"]) for s in summaries}
    if len(hosts) != 1:
        raise ValueError("native shards ran on different toolchains")
    go, goos, goarch = hosts.pop()

    # The synthetic step is trusted only after it reproduces createCheckers on
    # every distinct multi-checker corpus record.
    distinct = {}
    for row in rows:
        for position, record_ in enumerate(row["assignments"]):
            if record_["checker_count"] > 1:
                key = digest(canonical(step_input({**record_, "id": ""})))
                distinct.setdefault(key, (f"{row['id']}#{position}", record_))
    check_cases = [step_input({**record_, "id": identifier}) for identifier, record_ in distinct.values()]
    step_summary, check_results = run_step(output / "build/assignments.test", output / "step-check", check_cases, env)
    mismatched = [identifier for (identifier, record_), result in zip(distinct.values(), check_results, strict=True)
                  if any(result.get(key) != record_.get(key) for key in STEP_OUTPUTS)]
    if mismatched:
        raise ValueError(f"the synthetic association step differs from createCheckers on {len(mismatched)} "
                         f"records, first {mismatched[:5]}")
    cases = synthetic_cases()
    _synthetic_summary, synthetic_results = run_step(output / "build/assignments.test", output / "synthetic",
                                                    [step_input(case) for case in cases], env)
    synthetic = []
    for case, result in zip(cases, synthetic_results, strict=True):
        synthetic.append({**step_input(case), "features": case.get("features", ["fusion"]),
                          "native": {key: result.get(key) for key in STEP_OUTPUTS}})
    if (step_summary["go"], step_summary["goos"], step_summary["goarch"]) != (go, goos, goarch):
        raise ValueError("the association step ran on another toolchain")
    verified_upstream()
    if input_digests() != inputs:
        raise ValueError("inputs changed during the run")
    counts = Counter()
    for row in rows:
        records = row["assignments"]
        counts["rows"] += 1
        counts["rows_with_records"] += bool(records)
        counts["records"] += len(records)
        for record_ in records:
            counts[f"checkers_{record_['checker_count']}"] += 1
    report = {"version": 1, "pin": document["pin"], "inputs": inputs, "build": built,
              "go": go, "goos": goos, "goarch": goarch, "host": platform.platform(),
              "mode": concurrent.MODE, "sharding": {"shards": len(groups), "jobs": jobs},
              "assignments_sha256": digest((output / "assignments.ndjson").read_bytes()),
              "counts": dict(sorted(counts.items())),
              "step_check": {"distinct_multi_checker_records": len(check_cases), "equal": True},
              "synthetic": synthetic}
    (output / "report.json").write_bytes(canonical(report) + b"\n")
    if write_record:
        RECORD.write_text(json.dumps({key: report[key] for key in (
            "version", "pin", "go", "goos", "goarch", "host", "mode", "build", "inputs", "assignments_sha256",
            "counts", "step_check", "synthetic")}, indent=1, sort_keys=True) + "\n")
    print(json.dumps({"counts": report["counts"], "synthetic": len(synthetic),
                      "step_check": report["step_check"]}, sort_keys=True))
    return report


def build_rust(directory):
    completed = subprocess.run(["cargo", "build", "--release", "--locked", "-p", "tsr_compiler", "--example", EXAMPLE],
                               cwd=ROOT, capture_output=True, check=False)
    if completed.returncode:
        raise ValueError("Rust build failed:\n" + completed.stderr.decode(errors="replace")[-4000:])
    binary = ROOT / "target/release/examples" / EXAMPLE
    copy = directory / EXAMPLE
    copy.write_bytes(binary.read_bytes())
    copy.chmod(0o755)
    return copy, digest(copy.read_bytes())


def run_rust(binary, lines):
    raw = b"".join(canonical(line) + b"\n" for line in lines)
    completed = subprocess.run([str(binary)], input=raw, capture_output=True, check=False)
    if completed.returncode:
        raise ValueError("Rust assignment run failed:\n" + completed.stderr.decode(errors="replace")[-4000:])
    return [strict_json_loads(line) for line in completed.stdout.splitlines()]


def rust_sources_sha256():
    """The Rust inputs of the comparison: the corpus driver's production and
    adapter sources, which include the example and the partitioner."""
    import phase2_corpus

    return digest(canonical(phase2_corpus.sources()))


def compare(capture, jobs, native_dir, write_record=False):
    import phase2_corpus

    capture = Path(capture).resolve()
    report = strict_json_loads((capture / "report.json").read_bytes())
    if write_record and canonical({key: report[key] for key in json.loads(RECORD.read_text())}) != canonical(
            json.loads(RECORD.read_text())):
        raise ValueError("the capture is not the recorded assignment witness")
    sources = rust_sources_sha256()
    if digest((capture / "assignments.ndjson").read_bytes()) != report["assignments_sha256"]:
        raise ValueError("the capture's records changed")
    rows = [strict_json_loads(line) for line in (capture / "assignments.ndjson").read_bytes().splitlines()]
    directory = capture / "compare"
    directory.mkdir(exist_ok=True)
    binary, binary_sha256 = build_rust(directory)
    _native_report, requests, _native_rows = phase2_corpus.requests(native_dir)
    requests = {request["id"]: request for request in requests}
    wanted = [row for row in rows if row["assignments"]]
    lines = [{"kind": "program", "id": row["id"], "request": requests[row["id"]]} for row in wanted]
    chunks = [lines[i::jobs] for i in range(jobs)]
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        outputs = list(pool.map(lambda chunk: run_rust(binary, chunk), chunks))
    rust = {}
    for output in outputs:
        for result in output:
            rust[result["id"]] = result
    differences, equal, by_count, unsupported = [], 0, Counter(), []
    for row in wanted:
        result = rust[row["id"]]
        if result["state"] == "unsupported":
            unsupported.append({"id": row["id"], "reason": result["reason"]})
            continue
        if result["state"] != "computed":
            differences.append({"id": row["id"], "kind": result["state"], "reason": result.get("reason")})
            continue
        native_record = next((r for r in row["assignments"] if r["files"] == result["files"]), None)
        if native_record is None:
            differences.append({"id": row["id"], "kind": "files", "native": row["assignments"][0]["files"][:8],
                                "rust": result["files"][:8]})
            continue
        left, right = normalize(native_record), normalize(result["plan"])
        if left != right:
            fields = sorted(key for key in set(left) | set(right) if left.get(key) != right.get(key))
            differences.append({"id": row["id"], "kind": "plan", "fields": fields,
                                "checker_count": native_record["checker_count"]})
            continue
        equal += 1
        by_count[native_record["checker_count"]] += 1
    synthetic_lines = [{"kind": "synthetic", **step_input(case)} for case in report["synthetic"]]
    synthetic_equal, synthetic_differences = 0, []
    for case, result in zip(report["synthetic"], run_rust(binary, synthetic_lines), strict=True):
        plan = result["plan"]
        observed = {key: plan.get(key) for key in STEP_OUTPUTS}
        if observed == case["native"]:
            synthetic_equal += 1
        else:
            synthetic_differences.append({"id": case["id"], "features": case["features"],
                                          "fields": sorted(k for k in observed if observed[k] != case["native"][k])})
    if rust_sources_sha256() != sources:
        raise ValueError("the Rust sources changed during the comparison")
    result = {"host": platform.platform(), "machine": platform.machine(), "rust_binary_sha256": binary_sha256,
              "rust_sources_sha256": sources, "record_sha256": digest(RECORD.read_bytes()),
              "capture": {"goos": report["goos"], "goarch": report["goarch"], "go": report["go"],
                          "assignments_sha256": report["assignments_sha256"]},
              "programs": len(wanted), "programs_equal": equal, "equal_by_checker_count": dict(sorted(by_count.items())),
              "unsupported": len(unsupported), "unsupported_reasons": dict(Counter(u["reason"] for u in unsupported)),
              "unsupported_rows": [u["id"] for u in unsupported],
              "program_differences": differences[:50], "program_difference_count": len(differences),
              "synthetic": len(report["synthetic"]), "synthetic_equal": synthetic_equal,
              "synthetic_differences": synthetic_differences,
              "equal": not differences and not synthetic_differences}
    (directory / "comparison.json").write_bytes(canonical(result) + b"\n")
    if write_record:
        if not result["equal"]:
            raise ValueError("an unequal comparison is not recorded")
        COMPARISON.write_text(json.dumps({key: result[key] for key in (
            "machine", "rust_sources_sha256", "record_sha256", "capture", "programs", "programs_equal",
            "equal_by_checker_count", "unsupported", "unsupported_reasons", "unsupported_rows", "synthetic",
            "synthetic_equal", "equal")}, indent=1, sort_keys=True) + "\n")
    print(json.dumps({key: result[key] for key in ("programs", "programs_equal", "equal_by_checker_count",
                                                     "unsupported", "unsupported_reasons",
                                                     "program_difference_count", "synthetic", "synthetic_equal",
                                                     "equal")}, sort_keys=True))
    if differences:
        print(json.dumps(differences[:5], sort_keys=True))
    return result


FUSION_LINES = re.compile(r"checkerpool\.go:(186|208|209|210|211)\)|math/sqrt\.go:\d+\)")


def fusion_listing(upstream, env, goarch, goamd64=None):
    build_env = dict(env, GOARCH=goarch)
    if goamd64:
        build_env["GOAMD64"] = goamd64
    completed = subprocess.run(
        ["go", "build", "-gcflags=github.com/microsoft/TypeScript/tsc/internal/compiler=-S", "-o", os.devnull,
         "./internal/compiler"], cwd=upstream / "tsc", env=build_env, capture_output=True, check=False)
    if completed.returncode:
        raise ValueError("assembly listing failed:\n" + completed.stderr.decode(errors="replace")[-4000:])
    text = completed.stdout.decode() + completed.stderr.decode()
    lines, inside = [], False
    for line in text.splitlines():
        if " STEXT " in line:
            inside = "compiler.getCheckerAssociationsInOrder STEXT" in line
            continue
        if inside and FUSION_LINES.search(line):
            fields = line.split(None, 2)
            location = re.sub(r"\(.*/(tsc/internal/compiler/checkerpool\.go|src/math/sqrt\.go)", r"(\1", fields[2])
            lines.append(" ".join([fields[1], *location.split()]))
    if not lines:
        raise ValueError("no score lines in the assembly listing")
    return lines


def fusion_text():
    upstream = verified_upstream()
    env = go_environment()
    version = subprocess.run(["go", "version"], cwd=upstream / "tsc", env=env, capture_output=True,
                             check=True).stdout.decode().split()[2]
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    parts = [
        "# The score arithmetic of compiler/checkerpool.go getCheckerAssociationsInOrder, as compiled by",
        f"# {version} at pin {pin}: `go build -gcflags=<compiler package>=-S ./internal/compiler`,",
        "# the instructions of lines 186 (alpha), 208-211 (weights, penalty, score) and math.Sqrt.",
        "# Produced by `scripts/phase2_assignments.py fusion --record`; `--check` re-extracts it.",
        "#",
        "# arm64: line 210 is FMULD then FMSUBD, `newWeight*sqrt(newWeight) - oldWeight*sqrt(oldWeight)`",
        "# with the second product fused into the subtraction; line 211 is FMSUBD, `n - alpha*(...)` with",
        "# the product fused into the score. amd64 (v1 and v3) rounds every product and difference.",
        "# The Rust partitioner reproduces the fusion on aarch64 only (docs/PHASE2-C6-plan.md, decision 4).",
    ]
    for label, goarch, goamd64 in (("arm64", "arm64", None), ("amd64 GOAMD64=v1", "amd64", "v1"),
                                   ("amd64 GOAMD64=v3", "amd64", "v3")):
        parts += ["", f"## {label}", *fusion_listing(upstream, env, goarch, goamd64)]
    return "\n".join(parts) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    rec = sub.add_parser("record")
    rec.add_argument("--output", required=True)
    rec.add_argument("--shards", type=int, default=8)
    rec.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    rec.add_argument("--timeout-minutes", type=int, default=60)
    rec.add_argument("--record", action="store_true")
    cmp_ = sub.add_parser("compare")
    cmp_.add_argument("--capture", required=True)
    cmp_.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    cmp_.add_argument("--native", default=str(ROOT / "target/phase2/native"),
                      help="the verified native capture whose harness inputs the corpus requests carry")
    cmp_.add_argument("--record", action="store_true", help="write data/phase2/c6-assignment-comparison.json")
    fus = sub.add_parser("fusion")
    group = fus.add_mutually_exclusive_group()
    group.add_argument("--record", action="store_true")
    group.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.command == "record":
        record(args.output, args.shards, args.jobs, args.timeout_minutes, args.record)
    elif args.command == "compare":
        result = compare(args.capture, args.jobs, args.native, args.record)
        if not result["equal"]:
            sys.exit(1)
    else:
        text = fusion_text()
        if args.record:
            FUSION.write_text(text)
        elif args.check:
            if FUSION.read_text() != text:
                sys.exit("the committed fusion evidence differs from the toolchain's assembly")
            print("fusion evidence current")
        else:
            sys.stdout.write(text)


if __name__ == "__main__":
    main()
