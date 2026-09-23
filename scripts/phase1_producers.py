"""Replay Phase 1 correctness evidence into three scoped tracker producers.

No command here starts a compiler, corpus or benchmark. Supply existing capture
roots with --capture FAMILY=DIR (program means the full syntax capture). Missing
captures and source-stale captures remain unavailable; malformed artifacts fail
replay. Independent current families are retained. `check` checks harness
contracts and deliberately does not require feature parity.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import gzip
import json
from pathlib import Path
import sys
import subprocess
import tomllib

import phase1_baselines as baselines
import phase1_capture as capture
import phase1_hosts as hosts
import phase1_scope as scope
import phase1_syntax as syntax
from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
GROUPS = {"foundations": ("leaves", "filesystem", "config", "syntax"),
          "config": ("config", "filesystem"), "syntax": ("syntax", "program")}
CASE_FILES = {"config": "data/phase1/config-cases.json", "syntax": "data/phase1/syntax-cases.json"}
DEFAULT = ROOT / "target/phase1-acceptance"


def read(path: Path):
    return strict_json_loads(path.read_bytes())


def encode(value) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode()


def sha(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def rust_packages(family: str) -> list[str]:
    """Derive local path dependencies independently of a capture's claimed list."""
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    inherited = workspace["workspace"].get("dependencies", {})
    pending = [ROOT / "tools/phase1" / family]
    for table in [*workspace.get("patch", {}).values(), workspace.get("replace", {})]:
        pending.extend(ROOT / dep["path"] for dep in table.values()
                       if isinstance(dep, dict) and "path" in dep)
    seen = set()
    while pending:
        directory = pending.pop().resolve()
        directory.relative_to(ROOT.resolve())
        if directory in seen:
            continue
        seen.add(directory)
        manifest = tomllib.loads((directory / "Cargo.toml").read_text())
        for context in [manifest, *manifest.get("target", {}).values()]:
            for category in ("dependencies", "dev-dependencies", "build-dependencies"):
                for name, dep in context.get(category, {}).items():
                    base = directory
                    if isinstance(dep, dict) and dep.get("workspace"):
                        dep, base = inherited[name], ROOT
                    if isinstance(dep, dict) and "path" in dep:
                        pending.append(base / dep["path"])
    return sorted(str(p.relative_to(ROOT)) for p in seen)


def source_closure(producer: str) -> dict[str, str]:
    if producer not in GROUPS:
        raise ValueError(f"unknown producer {producer}")
    paths = {"PORTS.toml", "data/go-functions.tsv", "status/runs.toml", "data/phase1/coverage-report.json.gz"}
    import phase1_coverage
    for runner, inventory, _ in phase1_coverage.RUST_ROUTES.values():
        paths.update((runner, inventory))
    result = {}
    # Live declaration/port-marker classification belongs to the inventory
    # audit, not replay. Each group binds its executable dependency closures
    # plus the frozen audit records it actually consumes.
    for family in GROUPS[producer]:
        if family == "program":
            result.update(syntax.rust_closure())
            from phase1_syntax_schedule import schedule_inputs as native_closure
            result.update(native_closure())
        else:
            result.update(capture.source_closure(family, rust_packages(family)))
    paths.update(str(p.relative_to(ROOT)) for p in (ROOT / "scripts").glob("phase1*.py"))
    paths.update(str(p.relative_to(ROOT)) for p in (ROOT / "data/phase1").rglob("*.json"))
    if producer == "foundations":
        import phase1_integration
        paths.update(phase1_integration.input_paths(ROOT))
    for name in sorted(paths):
        path = ROOT / name
        if not path.is_file():
            raise ValueError(f"producer input missing: {name}")
        result[name] = sha(path.read_bytes())
    return {name: value for name, value in sorted(result.items()) if Path(name).name != ".DS_Store"}


def baseline_owners() -> dict[str, tuple[str, str]]:
    """One owner per output, even though matchFiles serves two grouped producers."""
    index = read(ROOT / "data/phase1/config-baselines.json")
    expected = {row["output"].removeprefix("tsc/testdata/baselines/reference/")
                for group in index["groups"].values() for row in group["outputs"]}
    if len(expected) != 309 or index["total_outputs"] != 309:
        raise ValueError("config output inventory must contain exactly 309 distinct paths")
    owners = {}
    for family in ("filesystem", "config"):
        for request in capture.load_requests(capture.FAMILIES[family])["requests"]:
            name = request.get("baseline")
            if name is None:
                continue
            if name not in expected or name in owners:
                raise ValueError(f"unknown or multiply owned config output: {name}")
            owners[name] = (family, request["case"])
    if set(owners) != expected:
        raise ValueError(f"config outputs missing requests: {sorted(expected - set(owners))}")
    return dict(sorted(owners.items()))


def expected_tests(producer: str) -> list[str]:
    if producer == "config":
        return sorted(baseline_owners())
    if producer == "syntax":
        return syntax.select(read(syntax.SCHEDULE), read(syntax.NATIVE), full=True)
    return []


def case_manifest_problems() -> list[str]:
    problems = []
    for producer, path in CASE_FILES.items():
        expected = expected_tests(producer)
        if not (ROOT / path).is_file() or read(ROOT / path) != expected:
            problems.append(f"{path} differs from its independently derived inventory")
    return problems


def require_rows(rows: list[dict], identities: list[str], *, key: str = "case") -> dict[str, dict]:
    actual = [row.get(key) for row in rows]
    if not identities or len(identities) != len(set(identities)) or actual != identities:
        raise ValueError("report rows must cover the nonempty ordered inventory exactly once")
    allowed = set(capture.RESULTS)
    for row in rows:
        if row.get("result") not in allowed or row["result"] in ("harness_failed", "not_run"):
            raise ValueError(f"invalid/incomplete row {row.get(key)}: {row.get('result')}")
    return {row[key]: row for row in rows}


def qualifications() -> dict[str, dict]:
    document = read(ROOT / "data/phase1/approved-differences.json")
    if document.get("version") != 1 or document.get("pin") != capture.pin():
        raise ValueError("approved differences do not name the current pin/version")
    approved = {}
    for item in document["differences"]:
        identity, family = item["case"], item["family"]
        if identity in approved or family not in capture.FAMILIES or not item.get("approved_by"):
            raise ValueError(f"invalid approved difference: {identity}")
        requests = capture.load_requests(capture.FAMILIES[family])["requests"]
        request = next((r for r in requests if r["case"] == identity), None)
        if request is None or sha(capture.request_bytes(request)) != item["request_sha256"]:
            raise ValueError(f"approved difference request changed: {identity}")
        if not (ROOT / item["decision"].split("#")[0]).is_file():
            raise ValueError(f"approved difference decision missing: {identity}")
        approved[identity] = item
    return approved


def accepted(row: dict, approved: dict[str, dict]) -> bool:
    if row["result"] == "match":
        return True
    item = approved.get(row.get("case"))
    return bool(row["result"] == "different" and item is not None
                and capture.canonical(row.get("native")) == capture.canonical(item["native"])
                and capture.canonical(row.get("rust")) == capture.canonical(item["rust"]))


def replay_family(family: str, directory: Path) -> dict:
    if family == "filesystem":
        archive = replay_host_capture(directory)
        report = archive["report"]
        report["capture_identity"] = archive["capture_identity"]
        approved = qualifications()
        report["approved_differences"] = [row["case"] for row in report["rows"]
                                          if row["result"] == "different" and accepted(row, approved)]
        return report
    # Do not trust a provenance that omitted a dependency and its manifest.
    provenance = read(directory / "provenance.json")
    if provenance.get("family") != family:
        raise ValueError(f"{family} cannot consume a capture from another family")
    if provenance.get("partial") is True:
        raise ValueError(f"{family} needs its own complete capture")
    recorded = provenance.get("source_closure", {})
    current = capture.source_closure(family, rust_packages(family))
    # Authenticate artifacts before reporting ordinary source staleness. A
    # changed payload is invalid even if the source also changed afterwards.
    report = capture.compare(directory)
    if report["family"] != family or report.get("partial"):
        raise ValueError(f"{family} needs its own complete capture")
    if recorded != current:
        changed = sorted(k for k in recorded.keys() | current.keys() if recorded.get(k) != current.get(k))
        raise capture.StaleCapture(f"{family} capture inputs changed: {', '.join(changed[:8])}")
    identities = [row["case"] for row in capture.load_requests(capture.FAMILIES[family])["requests"]]
    require_rows(report["rows"], identities)
    approved = qualifications()
    report["approved_differences"] = [row["case"] for row in report["rows"]
                                      if row["result"] == "different" and accepted(row, approved)]
    report["capture_identity"] = sha((directory / "provenance.json").read_bytes())
    return report


def filesystem_requests() -> list[dict]:
    return capture.load_requests(capture.FAMILIES["filesystem"])["requests"]


def host_inventory(goos: str, requests: list[dict] | None = None) -> list[str]:
    """Select exactly this host's applicable rows; any/posix require both CI OSes."""
    requests = filesystem_requests() if requests is None else requests
    required = {target for request in requests for target in hosts.required_goos(request)}
    if goos not in required:
        raise ValueError(f"unsupported filesystem capture host: {goos}")
    return [request["case"] for request in requests if hosts.applies(request, goos)]


def replay_host_capture(directory: Path) -> dict:
    """Authenticate a separate archive covering its host's complete inventory."""
    provenance = read(directory / "provenance.json")
    if provenance.get("family") != "filesystem":
        raise ValueError("host capture family differs")
    report = capture.compare(directory)
    goos = provenance.get("host", {}).get("goos")
    if report.get("host", {}).get("goos") != goos:
        raise ValueError("host capture authenticated GOOS differs")
    expected = host_inventory(goos)
    selected = provenance.get("selected_cases")
    full = [request["case"] for request in filesystem_requests()]
    # A full capture may also retain native-unavailable rows on excluded hosts.
    # Bounded CI captures select only applicable requests, without recapturing
    # another host's cases or inserting its responses into this archive.
    if (not isinstance(selected, list) or not selected or len(selected) != len(set(selected))
            or set(selected) not in (set(expected), set(full))):
        raise ValueError("host capture selection differs from its applicable inventory")
    actual = [row for row in report["rows"] if row["result"] != "not_run"]
    # Provenance records sorted selected IDs. Observations retain request-file
    # ordering, which is independently checked instead of sorted to hide drift.
    require_rows(actual, full if set(selected) == set(full) else expected)
    current = capture.source_closure("filesystem", rust_packages("filesystem"))
    if provenance.get("source_closure") != current:
        raise capture.StaleCapture(f"{goos} host capture current source closure differs")
    return {"capture_identity": sha((directory / "provenance.json").read_bytes()),
            "host": provenance["host"], "selected_cases": selected, "report": report}


def attach_platform_captures(family: str, report: dict, directories: list[Path]) -> dict[str, str]:
    """Attach per-host archives without replacing any row of the base capture.

    Missing/stale hosts cannot certify preparation. Keep other current hosts
    available when one archive is stale; malformed artifacts still fail replay.
    """
    if family != "filesystem":
        raise ValueError("host captures are only consumed by filesystem")
    archives = {}
    unavailable = {}
    seen = {report.get("host", {}).get("goos")}
    for directory in directories:
        try:
            archive = replay_host_capture(directory)
        except capture.StaleCapture as error:
            unavailable[str(directory)] = str(error)
            continue
        goos = archive["host"]["goos"]
        if goos in seen:
            raise ValueError(f"duplicate filesystem host capture: {goos}")
        seen.add(goos)
        archives[goos] = archive
    report["host_captures"] = archives
    return unavailable


def host_coverage(report: dict, approved: dict[str, dict] | None = None) -> dict:
    """Require each (case, GOOS) observation; another host cannot replace it."""
    approved = qualifications() if approved is None else approved
    requests = filesystem_requests()
    required = sorted({goos for request in requests for goos in hosts.required_goos(request)})
    archives = dict(report.get("host_captures", {}))
    goos = report.get("host", {}).get("goos")
    if goos in archives:
        raise ValueError(f"duplicate filesystem host capture: {goos}")
    if goos and report.get("capture_identity"):
        archives[goos] = {"capture_identity": report["capture_identity"], "report": report}
    indexed = {target: {row["case"]: row for row in archive["report"]["rows"]}
               for target, archive in archives.items() if archive.get("capture_identity")}
    missing, failures, complete_cases = [], [], []
    for request in requests:
        identity = request["case"]
        complete = True
        for target in hosts.required_goos(request):
            row = indexed.get(target, {}).get(identity)
            if row is None or row["result"] == "not_run":
                missing.append({"case": identity, "goos": target})
                complete = False
            elif not accepted(row, approved):
                failures.append({"case": identity, "goos": target, "result": row["result"]})
                complete = False
        if complete:
            complete_cases.append(identity)
    return {"required_hosts": required,
            "captures": {target: archive["capture_identity"] for target, archive in sorted(archives.items())},
            "missing_hosts": [target for target in required if target not in indexed],
            "missing": missing, "failures": failures, "complete_cases": complete_cases,
            "complete": bool(requests) and not missing and not failures}


def apply_platform_preparation(reports: dict, health: dict) -> None:
    """Close historical unavailable links only after all required hosts pass."""
    import phase1_coverage
    report = reports.get("filesystem")
    if report is None:
        return
    coverage = host_coverage(report)
    report["host_coverage"] = coverage
    cases = read(ROOT / "data/phase1/cases.json")
    recorded = scope.recorded_results(cases)
    observed = {identity for identity in coverage["complete_cases"]
                if recorded.get(identity) in ("native_unavailable", "not_applicable")}
    if not observed:
        return
    document = read(ROOT / "data/phase1/scope.json")
    for family in scope.STEP_PACKAGES:
        health["preparations"][family] = scope.leaf_preparation(document, cases, family,
                                                              supplemental_prepared_cases=observed)
    health["coverage"] = phase1_coverage.build(supplemental_prepared_cases=observed)


def replay_program(directory: Path) -> dict:
    problems = syntax.schedule_problems()
    if problems:
        raise ValueError("invalid native syntax schedule: " + "; ".join(problems))
    report = syntax.replay(directory)
    if report["selection"] != "full" or not report["rust_sources_current"]:
        raise ValueError("syntax acceptance requires a current full capture")
    require_rows(report["rows"], expected_tests("syntax"), key="id")
    report["capture_identity"] = sha((directory / "provenance.json").read_bytes())
    return report


def platform_summary(directory: Path, output: Path) -> dict:
    """Record a separate host archive without claiming multi-host preparation."""
    provenance = read(directory / "provenance.json")
    try:
        archive = replay_host_capture(directory)
        goos = archive["host"]["goos"]
        applicable = set(host_inventory(goos))
        rows = [row for row in archive["report"]["rows"] if row["case"] in applicable]
        approved = qualifications()
        state = ("match" if all(accepted(row, approved) for row in rows) else
                 "unavailable" if any(row["result"] == "native_unavailable" for row in rows) else "different")
        detail = {"state": state, **archive}
    except capture.StaleCapture as error:
        detail = {"state": "unavailable", "reason": str(error),
                  "capture_identity": sha((directory / "provenance.json").read_bytes()),
                  "host": provenance.get("host"), "selected_cases": provenance.get("selected_cases")}
    detail["version"] = 2
    output.mkdir(parents=True, exist_ok=True)
    payload = encode(detail)
    artifact = output / ("host-" + sha(payload) + ".json")
    artifact.write_bytes(payload)
    return {"state": detail["state"], "artifact": str(artifact), "sha256": sha(payload)}


def comparator_controls() -> dict[str, bool]:
    """Execute inexpensive positive and rejection controls in each producer."""
    rows = [{"case": key, "result": "match"} for key in ("one", "two")]
    results = {"positive": len(require_rows(rows, ["one", "two"])) == 2}
    broken = {"removed": rows[:1], "duplicate": [rows[0], rows[0]],
              "reordered": rows[::-1], "extra": rows + [rows[0]],
              "partial": [rows[0], {"case": "two", "result": "not_run"}],
              "unknown": [rows[0], {"case": "two", "result": "looks_good"}]}
    for name, value in broken.items():
        try:
            require_rows(value, ["one", "two"])
        except ValueError:
            results[name] = True
        else:
            results[name] = False
    request = {"case": "identity", "operation": "actual"}
    for name, response in {
        "wrong-operation": {"case": "identity", "operation": "other", "result": "observed", "observation": []},
        "wrong-side-status": {**request, "result": "native_unavailable", "reason": "unsupported"},
    }.items():
        try:
            capture.validate_response({"observations": [response]}, [request], "rust")
        except ValueError:
            results[name] = True
        else:
            results[name] = False
    return results


def harness_check(producer: str | None = None, *, current_classification: bool = True) -> dict:
    import phase1_coverage
    import phase1_integration
    # Manifest health is independent of pending behavior. It cannot certify
    # implementation; no comparison is reconstructed from `last_result` here.
    document = read(ROOT / "data/phase1/scope.json")
    cases = read(ROOT / "data/phase1/cases.json")
    problems = scope.verify(document)
    if producer is None:
        problems += scope.witness_problems()
    if producer is None and current_classification and document != scope.build():
        problems.append("scope.json differs from current source classification; run phase1.py inventory --write")
    problems += scope.roster_problems(document, cases) + scope.gap_record_problems(cases)
    problems += baselines.verify(read(ROOT / "data/phase1/config-baselines.json"))
    problems += case_manifest_problems()
    qualifications()
    coverage = phase1_coverage.build()
    report_path = ROOT / "data/phase1/coverage-report.json.gz"
    if not report_path.is_file():
        problems.append("committed coverage report is missing")
    else:
        problems += phase1_coverage.verify(strict_json_loads(gzip.decompress(report_path.read_bytes())))
    # Config and syntax neither publish nor consume integration metrics. Do
    # not let an unrelated archive/transport asset gate these two producers.
    integration = (phase1_integration.check() if producer in (None, "foundations") else
                   {"prepared": False, "complete": False, "problems": []})
    problems += coverage["problems"] + integration["problems"]
    controls = comparator_controls()
    problems += [f"comparator control failed: {name}" for name, passed in controls.items() if not passed]
    preparations = {family: scope.leaf_preparation(document, cases, family)
                    for family in scope.STEP_PACKAGES}
    return {"healthy": not problems, "problems": problems, "preparations": preparations,
            "coverage": coverage, "integration": integration, "controls": controls}


def aggregate(producer: str, reports: dict[str, dict], health: dict) -> dict:
    """Only this function translates authenticated observations to metrics."""
    rows = {}
    approved = qualifications()
    for family, report in reports.items():
        if family not in GROUPS[producer]:
            raise ValueError(f"unconsumed report {family} in {producer}")
        if family == "program":
            rows[family] = require_rows(report["rows"], expected_tests("syntax"), key="id")
        else:
            identities = [r["case"] for r in capture.load_requests(capture.FAMILIES[family])["requests"]]
            if family == "filesystem":
                if [row["case"] for row in report["rows"]] != identities:
                    raise ValueError("report rows must cover the nonempty ordered inventory exactly once")
                applicable = host_inventory(report.get("host", {}).get("goos"))
                rows[family] = require_rows([row for row in report["rows"] if row["case"] in applicable], applicable)
            else:
                rows[family] = require_rows(report["rows"], identities)
    filesystem_coverage = host_coverage(reports["filesystem"], approved) if "filesystem" in reports else None
    def complete(family):
        if family == "filesystem":
            return prepared(family)
        return prepared(family) and bool(rows.get(family)) and all(accepted(r, approved) for r in rows[family].values())
    def prepared(family):
        return (health["healthy"] and health["preparations"][family]["complete"] and family in rows
                and (family != "filesystem" or filesystem_coverage["complete"]))
    metrics = {"inventory_complete": health["healthy"]}
    tests = {}
    if producer == "foundations":
        metrics.update(harness_pass=health["healthy"], leaves_prepared=prepared("leaves"),
                       filesystem_prepared=prepared("filesystem"), utilities_prepared=prepared("syntax"),
                       integration_prepared=health["healthy"] and health["integration"]["prepared"] and health["coverage"]["preparation_complete"],
                       leaves_complete=complete("leaves"), filesystem_complete=complete("filesystem"),
                       utilities_complete=complete("syntax"),
                       rust_witnesses_complete=health["healthy"] and health["integration"].get("rust_witnesses", {}).get("state") == "match",
                       integration_complete=health["healthy"] and health["integration"].get("complete", False))
    elif producer == "config":
        owners = baseline_owners()
        tests = {name: "skip" if family not in rows else "pass" if rows[family][case]["result"] == "match" else "fail"
                 for name, (family, case) in owners.items()}
        baseline_cases = {case for family, case in owners.values() if family == "config"}
        direct = [r for key, r in rows.get("config", {}).items() if key not in baseline_cases]
        metrics.update(prepared=prepared("config") and prepared("filesystem"),
                       direct_complete=prepared("config") and bool(direct) and all(accepted(r, approved) for r in direct))
    else:
        metrics["prepared"] = prepared("syntax") and bool(rows.get("program"))
        tests = {name: "skip" if "program" not in rows else "pass" if rows["program"][name]["result"] == "match" else "fail"
                 for name in expected_tests("syntax")}
    if not health["healthy"]:
        metrics = {k: False if isinstance(v, bool) else v for k, v in metrics.items()}
        tests = dict.fromkeys(tests, "fail")
    return {"metrics": metrics, **({"tests": tests} if tests else {})}


def produce(producer: str, captures: dict[str, Path], output: Path, receipts: list[dict] | None = None,
            platform_captures: list[Path] | None = None) -> dict:
    before = source_closure(producer)
    health = harness_check(producer)
    if not health["healthy"]:
        raise ValueError("harness invalid: " + "; ".join(health["problems"][:12]))
    reports, unavailable = {}, {}
    for family in GROUPS[producer]:
        directory = captures.get(family, DEFAULT / family)
        if not directory.is_dir():
            if family in captures:
                raise ValueError(f"explicit capture does not exist: {directory}")
            unavailable[family] = f"no capture at {directory.relative_to(ROOT)}"
            continue
        try:
            reports[family] = replay_program(directory) if family == "program" else replay_family(family, directory)
        except capture.StaleCapture as error:
            unavailable[family] = str(error)
    if platform_captures is None and producer in ("foundations", "config") and "filesystem" in reports:
        base_host = reports.get("filesystem", {}).get("host", {}).get("goos")
        required = {goos for request in filesystem_requests() for goos in hosts.required_goos(request)}
        platform_captures = [path for goos in sorted(required) if goos != base_host
                             if (path := ROOT / "target/phase1-platform" / goos).is_dir()]
    unattached_hosts = {}
    if platform_captures:
        base = reports.get("filesystem")
        # An unavailable base cannot certify filesystem preparation, but it
        # must not discard independent current families or other host archives.
        # Authenticate explicit host inputs even when they cannot be consumed.
        destination = base if base is not None else {}
        stale = attach_platform_captures("filesystem", destination, platform_captures)
        unavailable.update({"filesystem/host/" + path: reason for path, reason in stale.items()})
        if base is None:
            unattached_hosts = destination["host_captures"]
    apply_platform_preparation(reports, health)
    if producer == "foundations":
        import phase1_integration
        health["integration"] = phase1_integration.evaluate(
            health["integration"], list(reports.values()), receipts or [], source_inputs=before)
        if health["integration"]["problems"]:
            raise ValueError("invalid integration evidence: " + "; ".join(health["integration"]["problems"]))
    result = aggregate(producer, reports, health)
    detail = {"version": 1, "producer": producer, "source_closure": before,
              "reports": reports, "unavailable": unavailable,
              **({"unattached_host_captures": unattached_hosts} if unattached_hosts else {}),
              "coverage": health["coverage"], "integration": health["integration"], "receipts": receipts or [], "result": result}
    if before != source_closure(producer):
        raise ValueError("producer inputs changed during replay")
    output.mkdir(parents=True, exist_ok=True)
    content = encode(detail)
    identity = sha(content)
    path = output / f"{producer}-{identity}.json"
    path.write_bytes(content)
    # xtask commits stderr with the run, binding the complete report identity
    # without squeezing digests into floating-point metrics.
    print(json.dumps({"phase1_report": str(path.relative_to(ROOT) if path.is_relative_to(ROOT) else path),
                      "sha256": identity, "captures": {k: v["capture_identity"] for k, v in reports.items()},
                      "unavailable": unavailable}, sort_keys=True), file=sys.stderr)
    return result


def observe(identity: str, output: Path) -> dict:
    """Run one fixed integration witness and retain its real outputs and inputs."""
    import phase1_integration as integration
    manifest = read(ROOT / integration.MANIFEST)
    witnesses = {row["id"]: row for row in manifest["witnesses"] if row["kind"] != "cases"}
    for row in manifest["witnesses"]:
        for test in row.get("additional_tests", []):
            witnesses[row["id"] + "/" + test["test"]] = test
    witnesses.update(transport={"command": ["python3", "scripts/s11.py", "capture"]},
                     generation={"command": manifest["generation"]["command"]},
                     **{"rust-witnesses": {"command": ["python3", "scripts/phase1_integration.py", "observe-rust-witnesses"]}})
    if identity not in witnesses:
        raise ValueError("unknown executable witness; case witnesses use normal family captures")
    command = witnesses[identity]["command"]
    before = source_closure("foundations")
    process = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    output.mkdir(parents=True, exist_ok=True)
    log_identity = sha(encode({"command": command, "inputs": before}) + process.stdout + process.stderr)
    log_root = output / (identity.replace("/", "--") + "-" + log_identity)
    log_root.mkdir(exist_ok=True)
    (log_root / "stdout.txt").write_bytes(process.stdout)
    (log_root / "stderr.txt").write_bytes(process.stderr)
    (log_root / "execution.json").write_bytes(encode({"command": command, "exit_code": process.returncode,
                                                       "source_inputs": before}))
    if before != source_closure("foundations"):
        raise ValueError(f"integration sources changed during execution; logs retained at {log_root}")
    artifact = None
    if identity == "installed-generated-assets" and process.returncode == 0:
        artifact = read(ROOT / command[command.index("--output") + 1] / "verified.json")
    record = integration.receipt(identity, command, before, process.stdout.decode(),
                                 stderr=process.stderr.decode(), exit_code=process.returncode, artifact=artifact)
    content = encode(record)
    output.mkdir(parents=True, exist_ok=True)
    path = output / (identity.replace("/", "--") + "-" + sha(content) + ".json")
    path.write_bytes(content)
    registry_path = DEFAULT / "integration-receipts.json"
    registry = read(registry_path) if registry_path.is_file() else {}
    if not isinstance(registry, dict) or any(not isinstance(k, str) or not isinstance(v, str) for k, v in registry.items()):
        raise ValueError("invalid integration receipt registry")
    registry[identity] = str(path.relative_to(ROOT) if path.is_relative_to(ROOT) else path)
    registry_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = registry_path.with_suffix(".tmp")
    temporary.write_bytes(encode(registry))
    temporary.replace(registry_path)
    return {"receipt": str(path), "sha256": sha(content), "exit_code": process.returncode}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=(*GROUPS, "check", "manifests", "observe", "platform", "host"))
    parser.add_argument("--capture", action="append", default=[], metavar="FAMILY=DIR")
    parser.add_argument("--receipt", type=Path, action="append", default=[])
    parser.add_argument("--host-capture", "--platform-capture", dest="platform_capture", type=Path, action="append", default=[],
                        help="separate authenticated filesystem capture covering its host's applicable inventory")
    parser.add_argument("--witness", help="integration test/driver, transport or generation identity")
    parser.add_argument("--structural", action="store_true",
                        help="check manifest/comparator contracts without refreshing the source-classification audit")
    parser.add_argument("--output", type=Path, default=ROOT / "target/phase1-producers")
    args = parser.parse_args(argv)
    try:
        captures = {}
        for value in args.capture:
            family, sep, directory = value.partition("=")
            if not sep or family not in (*capture.FAMILIES, "program") or family in captures:
                raise ValueError(f"invalid/duplicate capture argument: {value}")
            captures[family] = Path(directory).resolve()
        if args.command == "manifests":
            for producer, path in CASE_FILES.items():
                (ROOT / path).write_bytes(encode(expected_tests(producer)))
            result = {"written": list(CASE_FILES.values())}
        elif args.command == "observe":
            if not args.witness:
                raise ValueError("observe requires --witness")
            result = observe(args.witness, args.output.resolve())
        elif args.command == "check":
            report = harness_check(current_classification=not args.structural)
            result = {k: report[k] for k in ("healthy", "problems")}
            print(json.dumps(result, sort_keys=True))
            return 0 if report["healthy"] else 1
        elif args.command in ("platform", "host"):
            if set(captures) != {"filesystem"}:
                raise ValueError("host requires --capture filesystem=DIR")
            result = platform_summary(captures["filesystem"], args.output.resolve())
            print(json.dumps(result, sort_keys=True))
            return int(result["state"] != "match")
        else:
            unused = set(captures) - set(GROUPS[args.command])
            if unused:
                raise ValueError(f"captures not consumed by {args.command}: {sorted(unused)}")
            if args.receipt and args.command != "foundations":
                raise ValueError("integration receipts are consumed only by foundations")
            receipts = [read(p) for p in args.receipt]
            registry_path = DEFAULT / "integration-receipts.json"
            if args.command == "foundations" and not args.receipt and registry_path.is_file():
                registry = read(registry_path)
                if not isinstance(registry, dict):
                    raise ValueError("integration receipt registry must be an identity/path object")
                for identity, path in registry.items():
                    item = read(ROOT / path)
                    if item.get("id") != identity:
                        raise ValueError("integration receipt registry identity differs")
                    receipts.append(item)
            platforms = args.platform_capture
            if platforms and args.command not in ("foundations", "config"):
                raise ValueError("platform captures are consumed only by filesystem producers")
            result = produce(args.command, captures, args.output.resolve(), receipts, platforms or None)
        print(json.dumps(result, sort_keys=True))
        return 0
    except (OSError, ValueError, KeyError) as error:
        print(f"Phase 1 producer failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
