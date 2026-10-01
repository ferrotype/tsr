#!/usr/bin/env python3
"""Phase 3 T0: capture the native emit contract for every executed variant.

The oracle is an access-only overlay of the pinned compiler runner
(`tools/phase3/oracle/emit_test.go`): it compiles each variant of the Phase 2
denominator as the runner does and records

- the emitted files (name, digest, size) and the emit result;
- the text the runner's `output`, `sourcemap` and `sourcemap record` sub-tests
  compose for their baselines, taken where `baseline.Run` would compare it;
- the pinned printer's reprint of every non-library source file, with and
  without comments (digests; `--texts` keeps the bytes).

A sub-test the runner does not run for a row is `disabled` with the pin's
reason. The transpile runner is captured by its own command.

The rows are the requests of data/phase3/inventory.json (the executed variants
of the Phase 2 denominator, in the S08 request shape). A capture binds to the
digest of those requests, not to the inventory file, so re-classifying a row
leaves the capture current. `review` also holds each native outcome against
what the inventory says the runner owes. The Phase 2 scripts are inputs of
recorded Phase 2 captures; this script imports none of them and edits none.

    capture   --output DIR [--mode single|concurrent] [--shards N] [--scheme contiguous|interleaved]
              [--jobs J] [--oracle-from DIR] [--stride K] [--limit N] [--case ID ...] [--texts]
    verify    --capture DIR              # second sharding, identical row digests
    review    --capture DIR [--record]   # native baselines equal the committed references
    transpile --output DIR [--oracle-from DIR] [--record]

`--record` writes data/phase3/native-provenance-<mode>.json (review) or
data/phase3/transpile-native.json (transpile). Raw outputs stay in the capture
directory, referenced by digest.
"""
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import platform
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase3_inventory  # noqa: E402
import s08_baselines  # noqa: E402
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

DATA = ROOT / "data/phase3"
BRIDGES = ROOT / "tools/phase3/oracle"
TEST, TRANSPILE_TEST = "TestPhase3Emit", "TestPhase3Transpile"
DRIVER = "testrunner/phase3_emit_test.go"
OBSERVER = "testutil/baseline/phase3_observer.go"
MODES = {"single": True, "concurrent": False}
DOMAINS = ("output", "sourcemap", "sourcemap_record")
DOMAIN_STATES = ("content", "no_content", "not_baselined", "disabled", "failed")
STAGES = {"native_parse", "native_setup", "native_options", "native_compile", "native_option_guard",
          "native_output", "native_sourcemap", "native_sourcemap_record", "native_reprint"}
SCRIPT_INPUTS = ("scripts/phase3_native.py", "scripts/s08_baselines.py", "scripts/s04.py", "scripts/s04_common.py",
                 "scripts/s04_runtime.py", "scripts/s08_oracle.py", "data/s04/toolchains.toml", "data/upstream.json",
                 "tools/s08/oracle/diagnostics_observer.go", "tools/phase3/oracle/emit_test.go",
                 "tools/phase3/oracle/baseline_observer.go")
REFERENCE = "tsc/testdata/baselines/reference"


def pinned_upstream():
    """The verified pin at its physical path: Go resolves the working
    directory, and an overlay keyed by a symlinked path would not apply."""
    return verified_upstream().resolve()


def replace_exact(source, before, after, count=1):
    if source.count(before) != count:
        raise ValueError(f"overlay anchor occurs {source.count(before)} times, expected {count}: {before[:60]!r}")
    return source.replace(before, after)


def shard_indices(count, shards, scheme):
    if shards < 1:
        raise ValueError("at least one shard")
    if scheme == "interleaved":
        return [list(range(start, count, shards)) for start in range(shards)]
    size = -(-count // shards)
    return [list(range(start, min(count, start + size))) for start in range(0, count, size)]


def provenance(mode):
    return DATA / f"native-provenance-{mode}.json"


def input_digests():
    return {name: digest((ROOT / name).read_bytes()) for name in SCRIPT_INPUTS}


def overlay_sources(upstream):
    """The S08 stage and option-rejection hooks of the harness, the baseline
    observer, and the driver. No pinned algorithm is replaced."""
    s08 = s08_baselines.overlay_sources(upstream)
    sources = {name: s08[name] for name in ("testutil/harnessutil/harnessutil.go",
                                             "testutil/harnessutil/s08_diagnostics_observer.go")}
    run = (upstream / "tsc/internal/testutil/baseline/baseline.go").read_text()
    sources["testutil/baseline/baseline.go"] = replace_exact(
        run, "func Run(t *testing.T, fileName string, actual string, opts Options) {\n",
        "func Run(t *testing.T, fileName string, actual string, opts Options) {\n"
        "\tif Phase3Observe != nil && Phase3Observe(fileName, actual, opts) {\n\t\treturn\n\t}\n")
    sources[OBSERVER] = (BRIDGES / "baseline_observer.go").read_text()
    sources[DRIVER] = (BRIDGES / "emit_test.go").read_text()
    return sources


def build_oracle(directory, upstream, env):
    sources = overlay_sources(upstream)
    replacements = {}
    for name, source in sorted(sources.items()):
        path = directory / "overlay" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        replacements[str(upstream / "tsc/internal" / name)] = str(path)
    for name in (DRIVER, OBSERVER):
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


def oracle_binary(output, upstream, env, oracle_from):
    """Build the oracle, or reuse a capture's binary after checking its digest
    and the current overlay."""
    if oracle_from is None:
        return build_oracle(output / "oracle", upstream, env), output / "oracle/oracle.test"
    source = Path(oracle_from).resolve()
    oracle = strict_json_loads((source / "report.json").read_bytes())["oracle"]
    binary = source / "oracle/oracle.test"
    if digest(binary.read_bytes()) != oracle["binary_sha256"]:
        raise ValueError("the reused oracle binary changed")
    if oracle["overlay_sha256"] != {name: digest(text.encode()) for name, text
                                    in sorted(overlay_sources(upstream).items())}:
        raise ValueError("the reused oracle was built from another overlay")
    copy = output / "oracle/oracle.test"
    if copy != binary:
        copy.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(binary, copy)
    return oracle, copy


def run_shard(binary, directory, requests, env, upstream, timeout_minutes, mode, texts):
    directory.mkdir(parents=True)
    raw = canonical(requests) + b"\n"
    (directory / "requests.json").write_bytes(raw)
    shard_env = dict(env, PHASE3_REQUESTS=str(directory / "requests.json"),
                     PHASE3_OUTPUT=str(directory / "observations.ndjson"),
                     PHASE3_SUMMARY=str(directory / "go-summary.json"))
    shard_env.pop("TS_TEST_PROGRAM_SINGLE_THREADED", None)
    shard_env.pop("PHASE3_TEXTS", None)
    if not MODES[mode]:
        shard_env["TS_TEST_PROGRAM_SINGLE_THREADED"] = "false"
    if texts:
        shard_env["PHASE3_TEXTS"] = "1"
    command = [str(binary), "-test.run", f"^{TEST}$", "-test.count=1", f"-test.timeout={timeout_minutes}m"]
    with (directory / "go.stdout").open("wb") as out, (directory / "go.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=upstream / "tsc/internal/testrunner", env=shard_env,
                                   stdout=out, stderr=err, check=False)
    if not (directory / "go-summary.json").exists():
        raise ValueError(f"native shard did not complete (exit {completed.returncode}); see {directory}")
    summary = strict_json_loads((directory / "go-summary.json").read_bytes())
    if summary["request_sha256"] != digest(raw) or summary["rows"] != len(requests):
        raise ValueError("native shard observed a different request inventory: " + str(directory))
    if summary.get("single_threaded") is not MODES[mode]:
        raise ValueError(f"native shard did not run upstream's {mode} test programs")
    observed = [strict_json_loads(line) for line in (directory / "observations.ndjson").read_bytes().splitlines()]
    if [row["id"] for row in observed] != [row["id"] for row in requests]:
        raise ValueError("native shard rows are missing, extra or reordered: " + str(directory))
    return completed.returncode, summary, observed


def validate(requests, observed):
    """Reject harness defects and malformed rows."""
    for request, result in zip(requests, observed, strict=True):
        vid, state = request["id"], result["state"]
        if state not in ("executed", "upstream_failed"):
            raise ValueError(f"native emit row {vid} is {state}: {result.get('failure')}")
        if state == "upstream_failed":
            failure = result.get("failure")
            if not isinstance(failure, dict) or failure.get("stage") not in STAGES or not failure.get("message"):
                raise ValueError("native failure lacks an explicit stage and message: " + vid)
            continue
        if result.get("failure") is not None or result.get("execution_stage") != "complete":
            raise ValueError("executed native row carries failure metadata: " + vid)
        for key in ("raw_sha256", "loaded_sha256"):
            if result[key] != request[key]:
                raise ValueError("native observation read a different input: " + vid)
        for domain in DOMAINS:
            item = result[domain]
            if item.get("state") not in DOMAIN_STATES:
                raise ValueError(f"unknown {domain} outcome: {vid}")
            if item["state"] == "content":
                bytes.fromhex(item["text_hex"])
            elif "text_hex" in item:
                raise ValueError("non-content outcome carries text: " + vid)
            if item["state"] in ("disabled", "failed") and not item.get("reason"):
                raise ValueError(f"{domain} {item['state']} without a reason: {vid}")
        if not result["has_non_dts_files"] and result["output"]["state"] != "disabled":
            raise ValueError("the output sub-test ran for a row without a non-declaration input: " + vid)
        if any(result[domain]["state"] == "disabled" for domain in DOMAINS[1:]):
            raise ValueError("the runner never disables a source-map sub-test: " + vid)
        if result["emit"]["state"] not in ("executed", "absent"):
            raise ValueError("malformed emit result: " + vid)
        for kind in ("js", "dts", "maps"):
            for item in result["outputs"][kind]:
                bytes.fromhex(item["name_hex"])
                if len(item["sha256"]) != 64 or type(item["bytes"]) is not int:
                    raise ValueError("malformed output entry: " + vid)
        if not isinstance(result["reprint"], list):
            raise ValueError("reprint observation missing: " + vid)
        for item in result["reprint"]:
            for key in ("comments", "no_comments"):
                if item[key]["state"] not in ("printed", "panic"):
                    raise ValueError("malformed reprint entry: " + vid)


def contract_digest(row):
    return digest(canonical(row))


def select(request_rows, limit, stride, cases):
    """A development subset: named cases, else every `stride`th row, cut at `limit`."""
    if cases:
        wanted = set(cases)
        chosen = [row for row in request_rows if row["id"] in wanted]
        if len(chosen) != len(wanted):
            raise ValueError("unknown --case id")
        return chosen
    chosen = request_rows[::stride]
    return chosen if limit is None else chosen[:limit]


def capture(output, mode, shards, scheme, jobs, timeout_minutes, *, oracle_from=None, limit=None, stride=1, cases=(), texts=False):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    upstream = pinned_upstream()
    env = go_environment()
    inputs = input_digests()
    document = phase3_inventory.read()
    request_rows = phase3_inventory.requests(document)
    full = len(request_rows)
    request_rows = select(request_rows, limit, stride, cases)
    raw = canonical(request_rows) + b"\n"
    (output / "requests.json").write_bytes(raw)
    oracle, binary = oracle_binary(output, upstream, env, oracle_from)
    groups = shard_indices(len(request_rows), min(shards, max(1, len(request_rows))), scheme)
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [pool.submit(run_shard, binary, output / "shards" / f"{number:03d}",
                               [request_rows[i] for i in group], env, upstream, timeout_minutes, mode, texts)
                   for number, group in enumerate(groups)]
        results = [future.result() for future in futures]
    observed = [None] * len(request_rows)
    summaries = []
    for group, (code, summary, rows) in zip(groups, results, strict=True):
        failed = any(r["state"] == "upstream_failed" or any(r.get(d, {}).get("state") == "failed" for d in DOMAINS)
                     for r in rows)
        if code not in (0, 1) or (code == 1 and not failed):
            raise ValueError("unexplained native test failure")
        summaries.append(summary)
        for index, row in zip(group, rows, strict=True):
            observed[index] = row
    with (output / "observations.ndjson").open("wb") as stream:
        for row in observed:
            stream.write(canonical(row) + b"\n")
    validate(request_rows, observed)
    verified_upstream()
    if input_digests() != inputs:
        raise ValueError("capture inputs changed during the native run")
    hosts = {(s["go"], s["goos"], s["goarch"]) for s in summaries}
    if len(hosts) != 1:
        raise ValueError("native shards ran on different toolchains")
    go, goos, goarch = hosts.pop()
    executed = [row for row in observed if row["state"] == "executed"]
    report = {
        "version": 1, "pin": document["pin"], "inputs": inputs, "oracle": oracle,
        "requests": len(request_rows), "request_sha256": digest(raw), "partial": len(request_rows) != full,
        "texts": texts,
        "observation_sha256": digest((output / "observations.ndjson").read_bytes()),
        "row_sha256": [contract_digest(row) for row in observed],
        "sharding": {"shards": len(groups), "scheme": scheme, "jobs": jobs},
        "states": dict(Counter(row["state"] for row in observed)),
        "failure_stages": dict(Counter(row["failure"]["stage"] for row in observed if row["state"] == "upstream_failed")),
        "domains": {domain: dict(Counter(row[domain]["state"] for row in executed)) for domain in DOMAINS},
        "go": go, "goos": goos, "goarch": goarch, "host": platform.platform(),
        "single_threaded": MODES[mode], "mode": mode,
        "scope": ("Pinned native emit observations for the Phase 2 denominator in upstream's "
                  f"{mode} test-program mode; no Rust comparison"),
    }
    (output / "report.json").write_bytes(canonical(report) + b"\n")
    print(json.dumps({k: report[k] for k in ("requests", "states", "failure_stages", "domains", "sharding")}, sort_keys=True))
    return report


def load_capture(directory, *, partial=False):
    directory = Path(directory).resolve()
    report = strict_json_loads((directory / "report.json").read_bytes())
    raw = (directory / "observations.ndjson").read_bytes()
    if digest(raw) != report["observation_sha256"]:
        raise ValueError("native emit capture changed since its report")
    if report["partial"] and not partial:
        raise ValueError("a partial capture cannot stand for the denominator")
    observed = [strict_json_loads(line) for line in raw.splitlines()]
    if [contract_digest(row) for row in observed] != report["row_sha256"]:
        raise ValueError("native emit rows differ from their recorded digests")
    return directory, report, observed


def current(report):
    now = input_digests()
    if report["inputs"] != now:
        stale = sorted(name for name, value in report["inputs"].items() if now.get(name) != value)
        raise ValueError("native emit capture is stale; changed inputs: " + ", ".join(stale))
    if report["oracle"]["overlay_sha256"] != {name: digest(text.encode()) for name, text
                                              in sorted(overlay_sources(pinned_upstream()).items())}:
        raise ValueError("native oracle overlay changed since the capture")


def verify(directory, shards, scheme, jobs, timeout_minutes, *, partial=False):
    directory, report, observed = load_capture(directory, partial=partial)
    current(report)
    other = directory / f"verify-{scheme}-{shards}"
    if other.exists():
        shutil.rmtree(other)
    if (shards, scheme) == (report["sharding"]["shards"], report["sharding"]["scheme"]):
        raise ValueError("verification needs a different sharding than the capture")
    requests = strict_json_loads((directory / "requests.json").read_bytes())
    second = capture(other, report["mode"], shards, scheme, jobs, timeout_minutes, oracle_from=directory,
                     cases=[row["id"] for row in requests] if report["partial"] else (), texts=report["texts"])
    differing = [row["id"] for row, left, right in zip(observed, report["row_sha256"], second["row_sha256"], strict=True)
                 if left != right]
    result = {"verified": not differing, "rows": len(observed), "capture_sharding": report["sharding"],
              "verify_sharding": second["sharding"], "observation_sha256": report["observation_sha256"],
              "differing": differing}
    (directory / "verified.json").write_bytes(canonical(result) + b"\n")
    print(json.dumps(dict(result, differing=differing[:10], differing_count=len(differing)), sort_keys=True))
    if differing:
        raise ValueError(f"second sharding differs on {len(differing)} rows")
    return result


def reference_bytes(name):
    path = ROOT / "upstream" / REFERENCE / name
    return path.read_bytes() if path.is_file() else None


def against_inventory(row, result):
    """Where the native outcome is not what the inventory says the runner owes."""
    found = []
    if result["has_non_dts_files"] is not row["has_non_dts_files"]:
        found.append("has_non_dts_files")
    output, native = row["output"], result["output"]
    if output["state"] == "disabled":
        expected = ("disabled", output["reason"])
    else:
        expected = ("content" if output["owes"] == "reference" else "no_content", None)
    if (native["state"], native.get("reason")) != expected:
        found.append("output")
    for domain in DOMAINS[1:]:
        states = ("content",) if row[domain] == "reference" else ("no_content", "not_baselined")
        if result[domain]["state"] not in states:
            found.append(domain)
    for domain, kind in zip(DOMAINS, phase3_inventory.EMIT_KINDS, strict=True):
        if result[domain]["state"] == "content" and (
                phase3_inventory.git_blob(bytes.fromhex(result[domain]["text_hex"])) != row["references"].get(kind)):
            found.append(domain + "_reference")
    return [{"id": row["id"], "field": field} for field in found]


def review(directory, record, *, partial=False):
    if partial and record:
        raise ValueError("a partial capture is never recorded")
    directory, report, observed = load_capture(directory, partial=partial)
    current(report)
    verified = directory / "verified.json"
    if (not verified.exists() or strict_json_loads(verified.read_bytes())["observation_sha256"] != report["observation_sha256"]
            or not strict_json_loads(verified.read_bytes())["verified"]):
        raise ValueError("review requires a verified capture (run verify first)")
    outcomes, disagreements = Counter(), []
    reprint = Counter()
    owed = {row["id"]: row for row in phase3_inventory.read()["rows"]}
    inventory_disagreements = []
    for result in observed:
        if result["state"] != "executed":
            continue
        inventory_disagreements.extend(against_inventory(owed[result["id"]], result))
        for domain in DOMAINS:
            item = result[domain]
            state = item["state"]
            if state in ("content", "no_content"):
                expected = reference_bytes(item["name"])
                ok = (expected is None) if state == "no_content" else (expected == bytes.fromhex(item["text_hex"]))
                if not ok:
                    disagreements.append({"id": result["id"], "domain": domain, "name": item["name"],
                                          "native_state": state, "reference_present": expected is not None})
            else:
                ok = None
            outcomes[(domain, state, ok)] += 1
        for item in result["reprint"]:
            for key in ("comments", "no_comments"):
                reprint[(key, item[key]["state"])] += 1
    executed = [row for row in observed if row["state"] == "executed"]
    summary = {
        "version": 1, "pin": report["pin"], "mode": report["mode"], "single_threaded": report["single_threaded"],
        "capture_report_sha256": digest((directory / "report.json").read_bytes()),
        "request_sha256": report["request_sha256"], "observation_sha256": report["observation_sha256"],
        "inputs": report["inputs"], "oracle": report["oracle"],
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"], "host": report["host"],
        "requests": report["requests"], "states": report["states"], "failure_stages": report["failure_stages"],
        "sharding": {"capture": report["sharding"], "verify": strict_json_loads(verified.read_bytes())["verify_sharding"]},
        "reference_outcomes": [{"domain": d, "native_state": s, "agrees": ok, "variants": n}
                               for (d, s, ok), n in sorted(outcomes.items(), key=lambda item: (item[0][0], item[0][1], str(item[0][2])))],
        "reference_disagreements": disagreements,
        "inventory_disagreements": inventory_disagreements,
        "disabled": {domain: dict(Counter(row[domain]["reason"] for row in executed if row[domain]["state"] == "disabled"))
                     for domain in DOMAINS},
        "failed_domains": [{"id": row["id"], "domain": domain, "reason": row[domain]["reason"]}
                           for row in executed for domain in DOMAINS if row[domain]["state"] == "failed"],
        "reprint": [{"mode": key, "state": state, "files": n} for (key, state), n in sorted(reprint.items())],
        "emit_skipped": sum(row["emit"].get("emit_skipped") is True for row in executed),
        "pre_post_different": [row["id"] for row in executed if row.get("pre_diagnostics") != row.get("post_diagnostics")],
        "row_sha256": report["row_sha256"],
    }
    (directory / "review.json").write_bytes(canonical(summary) + b"\n")
    if record:
        DATA.mkdir(parents=True, exist_ok=True)
        provenance(report["mode"]).write_bytes(canonical(summary) + b"\n")
    brief = {"requests": summary["requests"], "states": summary["states"],
             "reference_disagreements": len(disagreements),
             "inventory_disagreements": len(inventory_disagreements), "failed_domains": len(summary["failed_domains"]),
             "reference_outcomes": summary["reference_outcomes"], "reprint": summary["reprint"],
             "emit_skipped": summary["emit_skipped"], "pre_post_different": len(summary["pre_post_different"])}
    print(json.dumps(brief, sort_keys=True))
    return summary


def transpile(output, oracle_from, record):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    upstream = pinned_upstream()
    env = go_environment()
    inputs = input_digests()
    oracle, binary = oracle_binary(output, upstream, env, oracle_from)
    run_env = dict(env, PHASE3_OUTPUT=str(output / "observations.ndjson"), PHASE3_SUMMARY=str(output / "go-summary.json"))
    command = [str(binary), "-test.run", f"^{TRANSPILE_TEST}$", "-test.count=1", "-test.timeout=10m"]
    with (output / "go.stdout").open("wb") as out, (output / "go.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=upstream / "tsc/internal/testrunner", env=run_env,
                                   stdout=out, stderr=err, check=False)
    if completed.returncode or not (output / "go-summary.json").exists():
        raise ValueError(f"native transpile run failed (exit {completed.returncode}); see {output}")
    summary = strict_json_loads((output / "go-summary.json").read_bytes())
    rows = [strict_json_loads(line) for line in (output / "observations.ndjson").read_bytes().splitlines()]
    if summary["rows"] != len(rows) or len({row["id"] for row in rows}) != len(rows):
        raise ValueError("native transpile rows are missing or duplicated")
    baselines, disagreements = 0, []
    for row in rows:
        if row["state"] != "executed":
            raise ValueError("native transpile configuration failed: " + row["id"])
        for run in row["runs"]:
            item = run["baseline"]
            baselines += 1
            expected = reference_bytes(item["name"])
            if item["state"] != "content" or expected != bytes.fromhex(item["text_hex"]):
                disagreements.append({"id": row["id"], "name": item["name"]})
    references = sorted(path.name for path in (ROOT / "upstream" / REFERENCE / "transpile").iterdir())
    produced = sorted(Path(run["baseline"]["name"]).name for row in rows for run in row["runs"])
    if input_digests() != inputs:
        raise ValueError("capture inputs changed during the native run")
    document = {
        "version": 1, "pin": strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"],
        "inputs": inputs, "oracle": oracle, "go": summary["go"], "goos": summary["goos"], "goarch": summary["goarch"],
        "host": platform.platform(), "configurations": len(rows), "baselines": baselines,
        "reference_disagreements": disagreements,
        "references_without_a_run": sorted(set(references) - set(produced)),
        "runs_without_a_reference": sorted(set(produced) - set(references)),
        "scope": ("The pinned transpile runner: each configuration's inputs, each unit's TranspileModule and "
                  "TranspileDeclaration result, and the baseline text the runner composes; no Rust comparison"),
        "rows": rows,
    }
    (output / "transpile.json").write_bytes(canonical(document) + b"\n")
    if record:
        DATA.mkdir(parents=True, exist_ok=True)
        (DATA / "transpile-native.json").write_bytes(json.dumps(document, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({k: document[k] for k in ("configurations", "baselines", "reference_disagreements",
                                                "references_without_a_run", "runs_without_a_reference")}, sort_keys=True))
    return document


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("capture")
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--mode", choices=sorted(MODES), default="single")
    sub.add_argument("--oracle-from", type=Path)
    sub.add_argument("--limit", type=int)
    sub.add_argument("--stride", type=int, default=1)
    sub.add_argument("--case", action="append", default=[])
    sub.add_argument("--texts", action="store_true")
    for name in ("capture", "verify"):
        target = sub if name == "capture" else commands.add_parser("verify")
        if name == "verify":
            target.add_argument("--capture", type=Path, required=True)
            target.add_argument("--partial", action="store_true")
        target.add_argument("--shards", type=int, default=8 if name == "capture" else 5)
        target.add_argument("--scheme", choices=("contiguous", "interleaved"),
                            default="contiguous" if name == "capture" else "interleaved")
        target.add_argument("--jobs", type=int, default=4)
        target.add_argument("--timeout-minutes", type=int, default=120)
    sub = commands.add_parser("review")
    sub.add_argument("--capture", type=Path, required=True)
    sub.add_argument("--record", action="store_true")
    sub.add_argument("--partial", action="store_true")
    sub = commands.add_parser("transpile")
    sub.add_argument("--output", type=Path, required=True)
    sub.add_argument("--oracle-from", type=Path)
    sub.add_argument("--record", action="store_true")
    args = parser.parse_args()
    if args.command == "capture":
        capture(args.output, args.mode, args.shards, args.scheme, args.jobs, args.timeout_minutes,
                oracle_from=args.oracle_from, limit=args.limit, stride=args.stride, cases=args.case, texts=args.texts)
    elif args.command == "verify":
        verify(args.capture, args.shards, args.scheme, args.jobs, args.timeout_minutes, partial=args.partial)
    elif args.command == "review":
        review(args.capture, args.record, partial=args.partial)
    else:
        transpile(args.output, args.oracle_from, args.record)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 native failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
