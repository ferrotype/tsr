#!/usr/bin/env python3
"""Phase 2 C0.6: the `checker` producer (status/runs.toml `[checker]`).

Replays the recorded captures and never runs a corpus: the native capture at
target/phase2/native and the Rust capture at target/phase2/rust. Metrics:

* inventory_frozen -- data/phase2/inventory.json rebuilds byte for byte from
  current inputs;
* native_verified -- the native capture is current, verified from a second
  sharding, agrees with every committed reference, and its review equals the
  committed data/phase2/native-provenance.json;
* harness_valid -- the Rust capture is complete, current, built from the
  current sources, bound to the current native capture and requests, and has
  no harness error;
* result_recorded -- that run replays and its comparison equals the committed
  data/phase2/first-comparison.json;
* errors_parity, types_parity, symbols_parity, display_parity -- ratios over
  the executed denominator (native-disabled domains count as met; failures and
  unsupported rows count against); trace_parity over the variants that trace;
  ordering and parent_pointers over the executed denominator;
* unsupported_required -- executed variants with an unsupported domain;
* blockers_named -- data/phase2/blockers.json equals the register rebuilt from
  the evidence and names every withheld observation;
* regression_parity -- the S08 acceptance variants the runner executes
  (9,367) with every domain met.

C1 (docs/PHASE2-C1-plan.md, C1.10) adds, from four committed authorities that
are `[checker]` inputs by name:

* c1_regressions -- domains met in data/phase2/c1-baseline.json.gz (the
  authenticated C1-start row report, bound to the same native observation and
  inventory) and not met now, over the whole denominator;
* c1_open -- claimed rows with a domain not met, plus every unresolved
  candidate in data/phase2/c1-claims.json;
* c1_failures -- failed foundation/claimed/candidate rows, plus failed rows
  without a traced return to another owner or a covering failure blocker;
* c1_audit_complete -- data/phase2/c1-audit.json checks with no open
  disposition (scripts/phase2_audit.py);
* c1_contracts -- data/phase2/receipts/c1-contracts.json records exactly the
  debug and release witness commands, every expected test passing, and
  unchanged production, workspace and generator inputs;
* c1_complete -- regression_parity == 1, c1_open == 0, c1_regressions == 0,
  c1_failures == 0, c1_audit_complete and c1_contracts; false while any of
  them is unavailable.

For C2, the shared checkpoint accounting additionally requires complete
baseline-bound claims, current domain-bound handoffs, no unresolved C2
blocker share, all five C0 prerequisites, and an authenticated measurement.
Missing authorities leave c2_complete false. Declaration diagnostics remain
inside errors, one of the same seven comparison domains.

`observe --witness c1-contracts` or `c2-contracts` runs the contract tests and
writes that receipt. `observe --witness c2-measurement --measurement DIR`
authenticates an existing full checkerbench capture and joins its production
inputs with the exit corpus. It never runs a benchmark or updates old capture
metadata: captures lacking newly registered source hashes are stale.
No threshold is introduced; PLAN's Phase 2 gate binds the parity
metrics at C7. A missing or stale capture leaves the correctness metrics
unavailable.
"""
from __future__ import annotations

import argparse
import contextlib
import json
from pathlib import Path
import re
import subprocess
import sys
import math

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_audit  # noqa: E402
import phase2_blockers  # noqa: E402
import phase2_compare  # noqa: E402
import phase2_corpus  # noqa: E402
import phase2_inventory  # noqa: E402
import phase2_native  # noqa: E402
import phase2_native_concurrent  # noqa: E402

NATIVE = ROOT / "target/phase2/native"
RUST = ROOT / "target/phase2/rust"
# C6: the concurrent test-program mode's native and Rust captures.
NATIVE_CONCURRENT = ROOT / "target/phase2/native-concurrent"
RUST_CONCURRENT = ROOT / "target/phase2/rust-concurrent"
MATCHED = ("match", "disabled")
CLAIMS = ROOT / "data/phase2/c1-claims.json"
AUDIT = phase2_audit.AUDIT
BASELINE = phase2_compare.BASELINE
RECEIPTS = ROOT / "data/phase2/receipts"
CHECKPOINT_AUTHORITIES = {
    "C1": {"claims": CLAIMS, "audit": AUDIT, "baseline": BASELINE},
    "C2": {name: ROOT / f"data/phase2/c2-{name}{'.json.gz' if name == 'baseline' else '.json'}"
           for name in ("claims", "audit", "baseline", "measurement")},
    # C3 has no measurement obligation (docs/PHASE2-C3-plan.md section 1).
    "C3": {name: ROOT / f"data/phase2/c3-{name}{'.json.gz' if name == 'baseline' else '.json'}"
           for name in ("claims", "audit", "baseline")},
    # C4 has no measurement obligation (docs/PHASE2-C4-plan.md section 1).
    "C4": {name: ROOT / f"data/phase2/c4-{name}{'.json.gz' if name == 'baseline' else '.json'}"
           for name in ("claims", "audit", "baseline")},
    # C5 has no measurement obligation; its services replay is a completion
    # input (docs/PHASE2-C5-plan.md, sections 1 and C5.9).
    "C5": {**{name: ROOT / f"data/phase2/c5-{name}{'.json.gz' if name == 'baseline' else '.json'}"
              for name in ("claims", "audit", "baseline")},
           "services": ROOT / "data/phase2/services-replay.json"},
    # C6 owns no rows. Its completion also needs the concurrent native
    # capture, the assignment witnesses and their Rust comparison, and the two
    # modes' outcome parity (docs/PHASE2-C6-plan.md, C6.10).
    "C6": {**{name: ROOT / f"data/phase2/c6-{name}{'.json.gz' if name == 'baseline' else '.json'}"
              for name in ("claims", "audit", "baseline", "assignments")},
           "assignment_comparison": ROOT / "data/phase2/c6-assignment-comparison.json",
           "concurrent": phase2_native_concurrent.PROVENANCE},
    # C7 owns no rows and has no baseline: its completion is the P2B exit on
    # the final inputs, read through its own authorities (docs/PHASE2-C7-plan.md,
    # C7.7).
    "C7": {"claims": ROOT / "data/phase2/c7-claims.json", "audit": ROOT / "data/phase2/c7-audit.json",
           "baseline": ROOT / "data/phase2/c7-baseline.json.gz"},
}
WITNESSES = {
    "c1-contracts": {
        # The contracts drive production entry points over loaded programs, so
        # the test lives with the program loader (tsr_compiler).
        "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe", "--test", "c1_contracts", "--locked"],
                     ["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe", "--test", "c1_contracts", "--locked", "--release"]],
        "test_source": "crates/tsr_compiler/tests/c1_contracts.rs",
        # Everything whose change can alter the contract tests' outcome.
        # Cover all workspace production dependencies (including non-Rust
        # bundled inputs), the local dev dependency and generator assets.
        "sources": ["crates", "tools/**/Cargo.toml", "tools/s08/relater-prototype", "xtask", "tools/s03",
                    "scripts/generate_locale_tables.py", "Cargo.toml", "Cargo.lock",
                    ".cargo", "rust-toolchain.toml", "scripts/phase2_producers.py"],
    },
}
WITNESSES["c2-contracts"] = {
    "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe,creation-trace",
                  "--test", "c2_contracts", "--locked", *release] for release in ([], ["--release"])],
    "test_source": "crates/tsr_compiler/tests/c2_contracts.rs", "minimum_tests": 9,
    "test_modules": {
        "cross_product_limits": "support/c2_cross_product_limits.rs",
        "order_contract": "support/c2_order_contract.rs",
        "variance_limits": "support/c2_variance_limits.rs",
    },
    # Frozen native observations and their observer sources are contract inputs,
    # separate from production inputs shared by corpus and measurement builds.
    "sources": [*WITNESSES["c1-contracts"]["sources"],
                "data/phase2/c2-order-traces.json", "tools/phase2/order-trace",
                "scripts/phase2_order_trace.py", "data/upstream.json", "data/s04/toolchains.toml",
                "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py",
                "scripts/tracking-bootstrap.py", "scripts/s08_oracle.py"],
}
WITNESSES["c3-contracts"] = {
    "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe",
                  "--test", "c3_contracts", "--locked", *release] for release in ([], ["--release"])],
    "test_source": "crates/tsr_compiler/tests/c3_contracts.rs", "minimum_tests": 34,
    "test_modules": {"changes": "support/c3_changes.rs", "native": "support/c3_native_diagnostics.rs"},
    # The fixtures and their pinned native observations are contract inputs.
    "sources": [*WITNESSES["c1-contracts"]["sources"], "crates/tsr_compiler/tests/fixtures/c3",
                "upstream/tsc/testdata/tests/cases/compiler/binderBinaryExpressionStress.ts"],
}
WITNESSES["c5-contracts"] = {
    # The services replay contract compiles the replay driver, which reads the
    # checker through the services-replay feature's views.
    "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe,services-replay",
                  "--test", "c5_contracts", "--locked", *release] for release in ([], ["--release"])],
    "test_source": "crates/tsr_compiler/tests/c5_contracts.rs", "minimum_tests": 9,
    "test_modules": {},
    "sources": [*WITNESSES["c1-contracts"]["sources"], "crates/tsr_compiler/tests/fixtures/c5",
                "tools/phase2/services/replay"],
}
WITNESSES["c4-contracts"] = {
    "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe",
                  "--test", "c4_contracts", "--locked", *release] for release in ([], ["--release"])],
    "test_source": "crates/tsr_compiler/tests/c4_contracts.rs", "minimum_tests": 9,
    "test_modules": {},
    # The fixtures and their pinned native observations are contract inputs.
    "sources": [*WITNESSES["c1-contracts"]["sources"], "crates/tsr_compiler/tests/fixtures/c4"],
}
WITNESSES["c6-contracts"] = {
    "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe",
                  "--test", "c6_contracts", "--locked", *release] for release in ([], ["--release"])],
    "test_source": "crates/tsr_compiler/tests/c6_contracts.rs", "minimum_tests": 9,
    "test_modules": {},
    # The fixtures, the assignment record contract 1 reads, the corpus driver
    # contract 8 runs and the deep C3 input are contract inputs.
    "sources": [*WITNESSES["c1-contracts"]["sources"], "crates/tsr_compiler/tests/fixtures/c6",
                "crates/tsr_compiler/tests/fixtures/c1", "data/phase2/c6-assignments.json", "data/upstream.json",
                "tools/s08/p4", "tools/s08/p5", "tools/s07/config", "tools/s07/program", "tools/phase2/subtests.rs",
                "upstream/tsc/testdata/tests/cases/compiler/binderBinaryExpressionStress.ts"],
}
WITNESSES["c7-contracts"] = {
    "commands": [["cargo", "test", "-p", "tsr_compiler", "--features", "recursion-probe",
                  "--test", "c7_contracts", "--locked", *release] for release in ([], ["--release"])],
    "test_source": "crates/tsr_compiler/tests/c7_contracts.rs", "minimum_tests": 6,
    "test_modules": {},
    # The frozen rows and their native observations, the pin's content-mapper
    # baselines the contracts compare, and the corpus driver they run.
    "sources": [*WITNESSES["c1-contracts"]["sources"], "crates/tsr_compiler/tests/fixtures/c7", "data/upstream.json",
                "upstream/tsc/testdata/baselines/reference/compiler/contentMapper*.contentmapper",
                "tools/s08/p4", "tools/s08/p5", "tools/s07/config", "tools/s07/program", "tools/phase2/subtests.rs"],
}
PRODUCTION_PATTERNS = tuple(path for path in WITNESSES["c1-contracts"]["sources"]
                            if path != "scripts/phase2_producers.py")


def source_inputs(paths):
    """Digest files under listed paths/globs, excluding build and editor caches."""
    found = {}
    for entry in paths:
        bases = list(ROOT.glob(entry)) if any(char in entry for char in "*?[") else [ROOT / entry]
        if not bases or any(not base.exists() for base in bases):
            raise ValueError("missing witness source: " + entry)
        for base in bases:
            files = [base] if base.is_file() else sorted(p for p in base.rglob("*") if p.is_file())
            for path in files:
                if {"target", "__pycache__"} & set(path.relative_to(ROOT).parts) or path.name == ".DS_Store":
                    continue
                found[str(path.relative_to(ROOT))] = digest(path.read_bytes())
    return found


def production_inputs():
    """The production and configuration identity the captures must carry: the
    contract sources without the crates' test-only suites, which the contract
    receipts bind instead (docs/PHASE2-C7-plan.md decision 8)."""
    return {path: value for path, value in source_inputs(PRODUCTION_PATTERNS).items()
            if not phase2_corpus.test_only(path)}


def witness_tests(spec):
    """Exact contract inventory, including explicitly reviewed test modules."""
    root = ROOT / spec["test_source"]
    source = root.read_text()
    sources = [("", source)]
    for module, relative in spec.get("test_modules", {}).items():
        binding = rf'(?m)^#\[path = "{re.escape(relative)}"\]\s*\nmod {re.escape(module)};'
        path = root.parent / relative
        if (len(re.findall(binding, source)) != 1
                or not path.resolve().is_relative_to(root.parent.resolve())):
            raise ValueError("contract test module binding differs: " + module)
        sources.append((module + "::", path.read_text()))
    tests = []
    for prefix, content in sources:
        names = re.findall(r"(?m)^#\[test\]\s*\nfn ([A-Za-z_][A-Za-z_0-9]*)\(", content)
        if len(names) != len(set(names)) or content.count("#[test]") != len(names):
            raise ValueError("contract test inventory is duplicated or not top-level")
        tests.extend(prefix + name for name in names)
    if len(tests) < spec.get("minimum_tests", 1) or len(tests) != len(set(tests)):
        raise ValueError("contract test inventory is empty or incomplete")
    return sorted(tests)


def successful_test_output(stdout, expected):
    """Every expected test ran once, passed, and none was filtered or ignored."""
    results = re.findall(r"(?m)^test (\S+) \.\.\. (\S+)\s*$", stdout)
    totals = re.findall(r"(?m)^test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", stdout)
    return (len(totals) == 1 and int(totals[0]) == len(expected)
            and sorted(name for name, _ in results) == expected
            and all(result == "ok" for _, result in results))


def observe(identity, output=RECEIPTS):
    """Run one contract witness and retain its outcome and source binding."""
    spec = WITNESSES.get(identity)
    if spec is None:
        raise ValueError(f"unknown witness {identity!r}")
    inputs = source_inputs(spec["sources"])
    tests = witness_tests(spec)
    runs = []
    for command in spec["commands"]:
        process = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        runs.append({"command": command, "exit_code": process.returncode,
                     "stdout_sha256": digest(process.stdout), "stderr_sha256": digest(process.stderr),
                     "stdout": process.stdout.decode(errors="replace"),
                     "stdout_tail": process.stdout.decode(errors="replace")[-2000:],
                     "stderr_tail": process.stderr.decode(errors="replace")[-2000:]})
    if inputs != source_inputs(spec["sources"]):
        raise ValueError("sources changed while the witness ran")
    record = {"version": 2, "witness": identity, "state": "observed", "runs": runs,
              "tests": tests, "source_inputs": inputs}
    Path(output).mkdir(parents=True, exist_ok=True)
    path = Path(output) / f"{identity}.json"
    path.write_bytes(json.dumps(record, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({"receipt": str(path), "exit_codes": [run["exit_code"] for run in runs]}))
    # The receipt keeps a failing run for diagnosis, but the observation fails:
    # a refreshed receipt must never hide a failing suite.
    failed = [" ".join(run["command"]) for run in runs
              if run["exit_code"] != 0 or not successful_test_output(run["stdout"], tests)]
    if failed:
        raise ValueError(f"{identity} failed or ran other tests than its inventory: " + "; ".join(failed))
    return record


def receipt_current(identity, path=None):
    """True when the receipt records success and none of its source inputs changed."""
    path = Path(path) if path else RECEIPTS / f"{identity}.json"
    if not path.is_file():
        return None
    spec = WITNESSES.get(identity)
    if spec is None:
        return False
    record = strict_json_loads(path.read_bytes())
    if (not isinstance(record, dict) or record.get("version") != 2
            or record.get("state") != "observed" or record.get("witness") != identity):
        return False
    runs = record.get("runs")
    if (not isinstance(runs, list) or any(not isinstance(run, dict) for run in runs) or not spec["commands"]
            or [run.get("command") for run in runs] != spec["commands"]):
        return False
    tests = witness_tests(spec)
    if record.get("tests") != tests:
        return False
    for run in runs:
        stdout = run.get("stdout")
        if (type(run.get("exit_code")) is not int or run["exit_code"] != 0 or not isinstance(stdout, str)
                or digest(stdout.encode()) != run.get("stdout_sha256")
                or not successful_test_output(stdout, tests)):
            return False
    return record.get("source_inputs") == source_inputs(spec["sources"])


def _c1_claim_metrics(comparison, claims, blockers=None):
    """The C1 exit metrics from a full comparison and the four authorities.

    Candidates remain open until their trace resolves ownership. Failed rows
    need an explicit traced return or a registered blocker covering that row;
    absence of a panic-module attribution is not evidence of another owner.
    """
    phase2_compare.validate_complete_rows(comparison)
    rows = {row["id"]: row for row in comparison["rows"]}
    metrics = {}
    if claims is not None:
        if (not isinstance(claims, dict) or claims.get("version") != 1
                or not isinstance(claims.get("rows"), list) or not claims["rows"]
                or not isinstance(claims.get("foundation_modules"), list) or not claims["foundation_modules"]
                or any(not isinstance(name, str) or not name for name in claims["foundation_modules"])
                or len(set(claims["foundation_modules"])) != len(claims["foundation_modules"])):
            raise ValueError("claims authority needs version 1, rows and a unique nonempty foundation module inventory")
        entries = {}
        known_blockers = {entry["id"]: entry for entry in (blockers or {}).get("entries", [])}
        for entry in claims.get("rows", []):
            vid, status = entry["id"], entry.get("status")
            if vid not in rows:
                raise ValueError(f"claim names an unknown executed variant: {vid}")
            if vid in entries:
                raise ValueError(f"duplicate claim identity: {vid}")
            if status not in ("claimed", "candidate", "returned", "blocked"):
                raise ValueError(f"claim {vid} has unknown status {status!r}")
            if status in ("returned", "blocked") and (not isinstance(entry.get("evidence"), str)
                                                       or not entry["evidence"].strip()):
                raise ValueError(f"{status} claim {vid} needs trace evidence")
            if status == "returned" and entry.get("owner") not in phase2_audit.CHECKPOINTS:
                raise ValueError(f"returned claim {vid} needs another checkpoint owner")
            if status == "blocked":
                identity = phase2_blockers.stable_blocker(entry)
                blocker = (phase2_blockers.covering_blocker(blockers, vid, identity) if identity is not None
                           else known_blockers.get(entry.get("blocker")))
                if blocker is None or vid not in blocker.get("variants", []):
                    raise ValueError(f"blocked claim {vid} names no registered blocker covering the variant")
            entries[vid] = entry
        claimed = {vid for vid, entry in entries.items() if entry["status"] == "claimed"}
        candidates = {vid for vid, entry in entries.items() if entry["status"] == "candidate"}
        metrics["c1_open"] = (len(candidates)
                              + sum(any(o not in MATCHED for o in rows[vid]["outcomes"].values()) for vid in claimed))
        modules = {f"panic: tsr_checker::{name}" for name in claims.get("foundation_modules", [])}
        failed = [row for row in rows.values() if "failed" in row["outcomes"].values()]
        returned = {vid for vid, entry in entries.items() if entry["status"] == "returned"
                    and entry.get("bucket") == rows[vid].get("bucket")}
        registered_failures = {vid for blocker in known_blockers.values()
                               if blocker.get("kind") == "failed" and blocker.get("owner") in phase2_audit.CHECKPOINTS
                               and blocker.get("evidence")
                               for vid in blocker.get("variants", [])}
        metrics["c1_failures"] = sum(row.get("bucket") in modules or row["id"] in claimed | candidates
                                     or row["id"] not in returned | registered_failures for row in failed)
    return metrics


CHECKPOINTS = ("C2", "C3", "C4", "C5", "C6", "C7")


def newest_checkpoint(loaded):
    """The one checkpoint whose completion this run may compute: the latest with authorities."""
    return max(loaded, key=CHECKPOINTS.index) if loaded else None


def drop_historical_completion(metrics, checkpoint):
    """A checkpoint's completion is a recorded historical fact (C3 plan, section 3):
    `P2B-Cn` closes on the checker run recorded at that checkpoint's exit, and a later
    run reports the checkpoint's accounting but never recomputes `_measured` or
    `_complete` on its own capture; nor, for C5, its services replay."""
    for suffix in ("_measured", "_services", "_complete"):
        metrics.pop(checkpoint.lower() + suffix, None)


def checkpoint_metrics(checkpoint, comparison, claims, audit_ok, baseline, contracts_ok, regression_parity,
                       blockers=None, *, handoffs=None, measured_ok=None, baseline_sha256=None,
                       prerequisites=None, inventory=None, incoming=None, services_ok=None, extra=None):
    """Shared exit accounting; a status label alone never exempts a current row."""
    phase2_compare.validate_complete_rows(comparison)
    prefix = checkpoint.lower()
    metrics = {}
    if baseline is not None:
        metrics[prefix + "_regressions"] = len(phase2_compare.regressions_against(baseline, comparison))
    if checkpoint == "C1":
        metrics.update(_c1_claim_metrics(comparison, claims, blockers))
    elif claims is not None and baseline is not None:
        inventory = phase2_inventory.executed() if inventory is None else inventory
        owners = {row["id"]: row["checkpoint"] for row in inventory}
        rows = {row["id"]: row for row in comparison["rows"]}
        handoffs, incoming = handoffs or {}, incoming or {}
        if (not isinstance(claims, dict) or claims.get("version") != 1 or not isinstance(claims.get("rows"), list)
                or claims.get("pin") != baseline["pin"] or claims.get("pin") != comparison["pin"]
                or claims.get("inventory_sha256") != baseline["inventory_sha256"]
                or claims.get("baseline_sha256") != baseline_sha256
                or claims.get("rust_capture_sha256") != baseline["rust_capture_sha256"]):
            raise ValueError(f"{checkpoint} claims authority has missing or mismatched baseline bindings")
        entries = {}
        for entry in claims["rows"]:
            vid = entry.get("id")
            if vid not in rows or vid in entries:
                raise ValueError("unknown or duplicate checkpoint claim: " + str(vid))
            if entry.get("status") not in ("open", "closed", "handed", "blocked"):
                raise ValueError("unknown checkpoint claim status: " + str(entry.get("status")))
            failure_return = (entry["status"] in ("handed", "blocked") and vid in handoffs
                              and "failed" in rows[vid]["outcomes"].values())
            if owners.get(vid) != checkpoint and vid not in incoming and not failure_return:
                raise ValueError("checkpoint claim requires a validated incoming handoff: " + vid)
            if entry["status"] == "closed" and not re.fullmatch(r"[0-9a-f]{7,40}", entry.get("commit", "")):
                raise ValueError("closed claim needs its fixing commit: " + vid)
            entries[vid] = entry
        required = {row["id"] for row in baseline["rows"] if owners.get(row["id"]) == checkpoint
                    and any(value not in MATCHED for value in row["outcomes"].values())}
        if not required <= entries.keys():
            raise ValueError("claims omit baseline-open checkpoint variants: " + ", ".join(sorted(required - entries.keys())))
        # Only a validated transfer can remove all currently unmet domains.
        excluded = {vid for vid, handoff in handoffs.items() if vid in entries
                    and entries[vid]["status"] in ("handed", "blocked")
                    and handoff["owner"] != checkpoint
                    and {d for d, value in rows[vid]["outcomes"].items() if value not in MATCHED} <= set(handoff["domains"])}
        for vid in list(excluded):
            handoff = handoffs[vid]
            if entries[vid]["status"] == "blocked":
                identity = dict(handoff["blocker"], domains=handoff["domains"])
                if phase2_blockers.covering_blocker(blockers, vid, identity) is None:
                    excluded.remove(vid)
        metrics[prefix + "_open"] = sum((owners.get(vid) == checkpoint or vid in incoming) and vid not in excluded
                                         and any(value not in MATCHED for value in row["outcomes"].values())
                                         for vid, row in rows.items())
        metrics[prefix + "_handoffs"] = sum(entry["status"] == "handed" for entry in entries.values())
        metrics[prefix + "_failures"] = sum("failed" in row["outcomes"].values() and vid not in excluded
                                             for vid, row in rows.items())
        if blockers is not None:
            metrics[prefix + "_blockers_open"] = sum(
                any(item.get("effective_owner") == checkpoint for item in entry.get("ownership", []))
                or (not entry.get("ownership") and checkpoint in entry.get("owner", "").split("/"))
                for entry in blockers.get("entries", []))
    for suffix, value in (("audit_complete", audit_ok), ("contracts", contracts_ok)):
        if value is not None:
            metrics[prefix + "_" + suffix] = value
    counters = ["open", "regressions", "failures"]
    booleans = ["audit_complete", "contracts"]
    if checkpoint != "C1":
        counters.append("blockers_open")
    # Only a checkpoint whose authorities include a measurement record (C2)
    # binds its completion to one; C3 records none (its plan, section 1).
    if "measurement" in CHECKPOINT_AUTHORITIES.get(checkpoint, {}):
        metrics[prefix + "_measured"] = measured_ok is True
        booleans.append("measured")
    # A checkpoint with a services authority (C5) completes only on a current
    # replay record in which every operation is replayed or approved.
    if "services" in CHECKPOINT_AUTHORITIES.get(checkpoint, {}):
        metrics[prefix + "_services"] = services_ok is True
        booleans.append("services")
    # C6's own evidence: the concurrent mode's capture and harness, the two
    # modes' parity and the assignments; failures count either mode's rows.
    if extra is not None:
        booleans_extra = [name for name, value in extra.items() if isinstance(value, bool)]
        metrics.update(extra)
        if prefix + "_failures_concurrent" in extra:
            metrics[prefix + "_failures"] = metrics.get(prefix + "_failures", 0) + extra[prefix + "_failures_concurrent"]
            del metrics[prefix + "_failures_concurrent"]
    else:
        booleans_extra = []
    evidence_ok = checkpoint == "C1" or (prerequisites is not None and all(
        prerequisites.get(name) is True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                                    "result_recorded", "blockers_named")))
    metrics[prefix + "_complete"] = (evidence_ok and regression_parity == 1
        and all(metrics.get(prefix + "_" + name) == 0 for name in counters)
        and all(metrics.get(prefix + "_" + name) is True for name in booleans)
        and all(metrics.get(name) is True for name in booleans_extra))
    return metrics


def c1_metrics(comparison, claims, audit_ok, baseline, contracts_ok, regression_parity, blockers=None):
    return checkpoint_metrics("C1", comparison, claims, audit_ok, baseline, contracts_ok, regression_parity, blockers)


def measurement_identity(directory, comparison, rust, *, context=None):
    """Independently replay each executable; join complete production inputs."""
    import s08_checkerbench as benchmark
    directory, rust = Path(directory), Path(rust)
    result = quietly(benchmark.report, directory)
    capture_raw = (directory / "capture.json").read_bytes()
    capture = strict_json_loads(capture_raw)
    build_raw = (directory / "build.json").read_bytes()
    build = strict_json_loads(build_raw)
    if context is None:
        corpus = strict_json_loads((rust / "capture.json").read_bytes())
        replay = quietly(phase2_corpus.replay, rust)
    else:
        context.require_directories(context.native_directory, rust)
        corpus, replay = context.metadata, context.replayed
    required = production_inputs()
    if (result.get("smoke") or not result.get("source_stable")
            or result.get("pin") != comparison["pin"] or capture.get("pin") != comparison["pin"]
            or replay["summary"]["partial"] or replay["summary"]["harness_errors"]
            or not replay["source_stable"] or replay["capture_sha256"] != comparison["rust_capture_sha256"]
            or capture["build_sha256"] != digest(build_raw)):
        raise ValueError("measurement requires current independent full captures at the exit pin")
    for name, inputs in (("measurement", build["sources"]), ("corpus", corpus["build"]["sources"])):
        if any(inputs.get(path) != value for path, value in required.items()):
            raise ValueError(name + " capture lacks the current complete production/configuration identity")
    for name in ("elapsed_ratio", "retained_bytes_ratio", "type_footprint_ratio"):
        value = result.get("metrics", {}).get(name)
        if type(value) not in (int, float) or not math.isfinite(value) or value <= 0:
            raise ValueError("measurement did not establish " + name)
    return {"version": 1, "pin": comparison["pin"], "directory": measurement_location(directory),
            "capture_sha256": digest(capture_raw), "build_sha256": digest(build_raw),
            "corpus_capture_sha256": comparison["rust_capture_sha256"],
            "production_sources_sha256": digest(canonical(required))}


def measurement_current(comparison, rust, path=None, *, context=None):
    path = Path(path) if path else CHECKPOINT_AUTHORITIES["C2"]["measurement"]
    if not path.is_file():
        return None
    record = strict_json_loads(path.read_bytes())
    if not isinstance(record, dict) or not isinstance(record.get("directory"), str):
        return False
    directory = Path(record["directory"])
    if directory.is_absolute() or ".." in directory.parts or not directory.parts:
        return False
    return record == measurement_identity(ROOT / directory, comparison, rust, context=context)


def measurement_location(directory):
    """A portable capture locator, separate from its authenticated identity."""
    try:
        return Path(directory).resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError as error:
        raise ValueError("measurement capture must be inside the checkout") from error


def observe_measurement(directory, native, rust):
    context = quietly(phase2_compare.load_context, native, rust)
    comparison = quietly(phase2_compare.report, native, rust, write=False, context=context)
    phase2_compare.validate_complete_rows(comparison)
    record = measurement_identity(directory, comparison, rust, context=context)
    path = CHECKPOINT_AUTHORITIES["C2"]["measurement"]
    path.write_bytes(json.dumps(record, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({"receipt": str(path), "capture_sha256": record["capture_sha256"]}))
    return record


def quietly(function, *args, **kwargs):
    """Library calls print human summaries; stdout carries only the metrics."""
    with contextlib.redirect_stdout(sys.stderr):
        return function(*args, **kwargs)


def ratio(rows, domain, *, exclude_disabled=False):
    selected = [row for row in rows if not (exclude_disabled and row["outcomes"][domain] == "disabled")]
    if not selected:
        return None
    return sum(row["outcomes"][domain] in MATCHED for row in selected) / len(selected)


def services_current(manifest, comparison, *, context=None):
    """C5.5: the recorded fourslash reference verifies and the replay record is current."""
    if not Path(manifest).is_file():
        return False
    import phase2_services
    return phase2_services.current(manifest, comparison, context=context)


def assignments_current(record_path, comparison_path, executed):
    """C6.7: the recorded assignment witnesses are current and the recorded
    Rust comparison equals them for the current Rust sources on the record's
    architecture."""
    import phase2_assignments

    record = strict_json_loads(Path(record_path).read_bytes())
    comparison = strict_json_loads(Path(comparison_path).read_bytes())
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    machines = {"arm64": ("arm64", "aarch64"), "amd64": ("x86_64", "amd64")}
    return (record.get("pin") == pin and record.get("inputs") == phase2_assignments.input_digests()
            and record.get("step_check", {}).get("equal") is True
            and comparison.get("record_sha256") == digest(Path(record_path).read_bytes())
            and comparison.get("rust_sources_sha256") == phase2_assignments.rust_sources_sha256()
            and comparison.get("machine") in machines.get(record.get("goarch"), ())
            and comparison.get("equal") is True
            and comparison.get("programs") == executed
            and comparison.get("programs_equal") + comparison.get("unsupported") == executed
            and comparison.get("synthetic_equal") == comparison.get("synthetic") == len(record.get("synthetic", [])))


def two_mode_state(native, rust, native_concurrent, rust_concurrent, executed, committed):
    """The concurrent mode's native capture and Rust run, and their comparison
    with the single-threaded run: the state the run-level and C6 metrics share."""
    state = {"native_verified_concurrent": False, "harness_valid_concurrent": False,
             "report": None, "modes": None, "concurrent": None}
    try:
        _, report, _ = phase2_native_concurrent.load_capture(native_concurrent)
        phase2_native_concurrent.current(report)
        verified = strict_json_loads((Path(native_concurrent) / "verified.json").read_bytes())
        review = quietly(phase2_native_concurrent.review, native_concurrent, False)
        state["report"] = report
        state["native_verified_concurrent"] = (verified["observation_sha256"] == report["observation_sha256"]
                                               and review == strict_json_loads(Path(committed).read_bytes())
                                               and not review["reference_disagreements"]
                                               and not review["input_mismatches"]
                                               and review["states"] == {"executed": executed})
    except (OSError, ValueError, KeyError) as error:
        print("concurrent native capture unavailable: " + str(error), file=sys.stderr)
        return state
    try:
        context = quietly(phase2_compare.load_context, native_concurrent, rust_concurrent)
        replayed, capture = context.replayed, context.metadata
        _, current_requests, _ = quietly(phase2_corpus.requests, native_concurrent, mode="concurrent")
        state["harness_valid_concurrent"] = (
            capture.get("mode") == "concurrent" and not replayed["summary"]["partial"]
            and replayed["summary"]["harness_errors"] == 0 and replayed["source_stable"]
            and replayed["summary"]["observed"] == executed
            and capture["native"]["observation_sha256"] == report["observation_sha256"]
            and capture["requests_sha256"] == digest(phase2_corpus.p4.canonical(current_requests) + b"\n"))
    except (OSError, ValueError, KeyError) as error:
        print("concurrent Rust capture unavailable: " + str(error), file=sys.stderr)
        return state
    if not state["harness_valid_concurrent"]:
        return state
    try:
        state["modes"] = quietly(phase2_compare.modes, native, rust, native_concurrent, rust_concurrent, write=False)
        state["concurrent"] = quietly(phase2_compare.report, native_concurrent, rust_concurrent, write=False,
                                      context=context)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("mode comparison unavailable: " + str(error), file=sys.stderr)
    return state


def concurrent_metrics(native, rust, native_concurrent, rust_concurrent, document, comparison, claims, authorities,
                       *, state=None):
    """C6: the concurrent mode's native capture and Rust run, the two modes'
    outcome parity and the assignment witnesses."""
    metrics = {"native_verified_concurrent": False, "harness_valid_concurrent": False,
               "c6_mode_parity": False, "c6_assignments": False}
    executed = document["counts"]["executed"]
    try:
        metrics["c6_assignments"] = assignments_current(authorities["assignments"],
                                                        authorities["assignment_comparison"], executed)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("C6 assignments unavailable: " + str(error), file=sys.stderr)
    if state is None:
        state = two_mode_state(native, rust, native_concurrent, rust_concurrent, executed, authorities["concurrent"])
    for name in ("native_verified_concurrent", "harness_valid_concurrent"):
        metrics[name] = state[name]
    modes, report = state["modes"], state["report"]
    if modes is None or state["concurrent"] is None:
        return metrics
    native_modes = (claims or {}).get("native_modes", {})
    metrics["c6_mode_parity"] = (
        modes["outcome_differences"] == 0 and modes["single"]["harness_errors"] == 0
        and modes["single"]["rust_capture_sha256"] == comparison["rust_capture_sha256"]
        and native_modes.get("differences") == []
        and native_modes.get("single", {}).get("capture_observation_sha256") == comparison["native_observation_sha256"]
        and native_modes.get("concurrent", {}).get("capture_observation_sha256") == report["observation_sha256"])
    single_failed = {row["id"] for row in comparison["rows"] if "failed" in row.get("outcomes", {}).values()}
    metrics["c6_failures_concurrent"] = sum("failed" in row.get("outcomes", {}).values()
                                            and row["id"] not in single_failed for row in state["concurrent"]["rows"])
    return metrics


RUN_LEVEL = ("native_verified_concurrent", "harness_valid_concurrent", "mode_parity", "assignments", "services",
             "content_mappers")


def run_level_metrics(state, comparison, inventory, *, context=None):
    """The properties of the final inputs, computed on every run whatever the
    current checkpoint (C7.7): the ledger's verify checks bind them, while the
    checkpoint-scoped names stay what they were when their checkpoints closed.
    Mode parity compares the two runs' outcomes, each against its own verified
    native capture, as C6's did without its claims' native-mode binding."""
    metrics = {name: state.get(name, False) for name in RUN_LEVEL[:2]}
    metrics.update({name: False for name in RUN_LEVEL[2:]})
    modes = state["modes"]
    if modes is not None and state["concurrent"] is not None:
        metrics["mode_parity"] = (state["native_verified_concurrent"] and state["harness_valid_concurrent"]
                                  and modes["outcome_differences"] == 0 and modes["single"]["harness_errors"] == 0
                                  and modes["single"]["rust_capture_sha256"] == comparison["rust_capture_sha256"])
    authorities = CHECKPOINT_AUTHORITIES["C6"]
    try:
        metrics["assignments"] = assignments_current(authorities["assignments"], authorities["assignment_comparison"],
                                                     len(inventory))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("assignments unavailable: " + str(error), file=sys.stderr)
    try:
        metrics["services"] = services_current(CHECKPOINT_AUTHORITIES["C5"]["services"], comparison, context=context)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("services replay unavailable: " + str(error), file=sys.stderr)
    # C7.8: the content-mapper rows match in every domain in both modes, and
    # the contracts receipt is current.
    mapped = {row["id"] for row in inventory if row["content_mapper"]}
    if mapped and state["concurrent"] is not None:
        matched = [row["id"] for rows in (comparison["rows"], state["concurrent"]["rows"]) for row in rows
                   if row["id"] in mapped and "outcomes" in row
                   and all(value in MATCHED for value in row["outcomes"].values())]
        metrics["content_mappers"] = receipt_current("c7-contracts") is True and len(matched) == 2 * len(mapped)
    return metrics


# The checker runs recorded at the exits of the checkpoints that closed while
# current (docs/PHASE2-C7-plan.md, section 2); sprints/P2B.toml names the same
# artifacts. C3 and C4 close on the final run instead (decision 2).
RECORDED_COMPLETIONS = {
    "C1": ("96b7c65f5f42d1eb95e5137360a15c95d7dcac7e47c2654bd3c822a4a675b1b6", "c1_complete"),
    "C2": ("74c7960a279872c3aed1b15cb25de880040a9abfd47c0291f692d3cddcf56ce2", "c2_complete"),
    "C5": ("815eeb4348732ebf902eb8b90d4774ef7bb7294c3c2dcb0f98be3aad8794affd", "c5_complete"),
    "C6": ("afc6eeb4063234e68657f25ab972a9d65eddb10f253e4b53ddfc4c7d4077bd82", "c6_complete"),
}
# The PLAN's Phase 2 gate as sprints/P2B.toml's exit states it for the checker run.
P2B_EXIT = (("harness_valid", True), ("errors_parity", 1), ("types_parity", 1), ("symbols_parity", 1),
            ("display_parity", 1), ("trace_parity", 1), ("ordering", 1), ("parent_pointers", 1),
            ("unsupported_required", 0))
# C7.6's prerequisite producers: every declared run but `checker`, which the
# tracker marks as an incomplete attempt while its producer runs.
PREREQUISITE_RUNS = ("binder", "bindworkload", "checkerbench", "checkertext", "clippy", "config", "deny",
                     "e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "fmt", "foundations", "gen", "oracle",
                     "program", "relater", "scanner", "selftest", "syntax", "testhost", "workspace")


def recorded_metric(evidence_id, metric, run="checker", root=ROOT):
    """A metric of one recorded evidence artifact, read as `xtask check` reads a
    `recorded.` condition: the artifact must match its id and record a success."""
    path = Path(root) / "status/evidence" / f"{evidence_id}.json"
    if not path.is_file():
        return None
    raw = path.read_bytes()
    record = strict_json_loads(raw)
    if (digest(raw) != evidence_id or record.get("schema_version") != 1 or record.get("run_id") != run
            or record.get("exit_code") != 0 or record.get("valid_capture") is not True
            or digest(record["stdout"].encode()) != record.get("stdout_sha256")):
        return None
    return strict_json_loads(record["stdout"].encode()).get("metrics", {}).get(metric)


def evidence_states():
    """The live evidence state of every declared run, never a generated view."""
    completed = subprocess.run(["cargo", "xtask", "evidence-states"], cwd=ROOT, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, check=False)
    if completed.returncode:
        raise ValueError("evidence states unavailable: " + completed.stderr.decode(errors="replace")[-2000:])
    return strict_json_loads(completed.stdout)


def c7_optional(name, function):
    """A C7 authority's check; one not built yet, or failing, reads false."""
    try:
        return function()
    except (ImportError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"C7 {name} unavailable: " + str(error), file=sys.stderr)
        return None


def c7_metrics(metrics, audit_ok, states, state, comparison, *, root=ROOT):
    """C7's metrics and its completion: the P2B exit on the final inputs, the
    run-level two-mode metrics, every P2B-Cn item closed, and C7's own seven."""
    out = {"c7_audit_complete": audit_ok is True,
           "c7_content_mappers": metrics.get("content_mappers") is True and audit_ok is True,
           "c7_evidence_current": (states is not None and set(states) == {*PREREQUISITE_RUNS, "checker"}
                                   and all(states[run] == "current" for run in PREREQUISITE_RUNS))}
    import phase2_informational
    out["c7_informational_listed"] = c7_optional("informational", phase2_informational.current) is True
    residuals = c7_optional("residuals", lambda: __import__("phase2_residuals").verified_count(
        comparison, state["concurrent"]))
    if isinstance(residuals, int):
        out["c7_residuals"] = residuals
    out["c7_dispositions"] = c7_optional("dispositions", lambda: __import__("phase2_dispositions").complete()) is True
    out["c7_divergences_valid"] = c7_optional("divergences", lambda: __import__("phase2_divergences").valid(
        comparison, state["concurrent"])) is True
    out["c7_report"] = c7_optional("report", lambda: __import__("phase2_report").current(
        comparison, state["concurrent"])) is True
    exit_ok = all(metrics.get(name) is value if isinstance(value, bool) else metrics.get(name) == value
                  for name, value in P2B_EXIT)
    recorded = all(recorded_metric(evidence_id, metric, root=root) is True
                   for evidence_id, metric in RECORDED_COMPLETIONS.values())
    prerequisites = all(metrics.get(name) is True for name in ("inventory_frozen", "native_verified", "harness_valid",
                                                                "result_recorded", "blockers_named"))
    out["c7_complete"] = (prerequisites and exit_ok and recorded and out.get("c7_residuals") == 0
                          and all(metrics.get(name) is True for name in RUN_LEVEL)
                          and all(out[name] is True for name in (
                              "c7_informational_listed", "c7_dispositions", "c7_divergences_valid",
                              "c7_evidence_current", "c7_report", "c7_content_mappers")))
    return out


def checker(native=NATIVE, rust=RUST, native_concurrent=NATIVE_CONCURRENT, rust_concurrent=RUST_CONCURRENT):
    metrics = {"inventory_frozen": False, "native_verified": False, "harness_valid": False,
               "result_recorded": False, "blockers_named": False, "c1_complete": False, "c2_complete": False,
               "c3_complete": False, "c4_complete": False, "c5_complete": False}
    try:
        document = phase2_inventory.read()
        metrics["inventory_frozen"] = (phase2_inventory.INVENTORY.read_bytes()
                                       == phase2_inventory.render(phase2_inventory.build())
                                       and document["counts"]["executed"] > 0)
    except (OSError, ValueError, KeyError) as error:
        print("inventory unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    try:
        _, report, _ = phase2_native.load_capture(native)
        phase2_native.current(report)
        verified = strict_json_loads((Path(native) / "verified.json").read_bytes())
        review = quietly(phase2_native.review, native, False)
        committed = strict_json_loads(phase2_native.PROVENANCE.read_bytes())
        metrics["native_verified"] = (verified["observation_sha256"] == report["observation_sha256"]
                                      and review == committed and not review["reference_disagreements"]
                                      and not review["input_mismatches"]
                                      and review["states"] == {"executed": document["counts"]["executed"]})
    except (OSError, ValueError, KeyError) as error:
        print("native capture unavailable: " + str(error), file=sys.stderr)
    if not metrics["native_verified"]:
        return {"metrics": metrics}
    try:
        context = quietly(phase2_compare.load_context, native, rust)
        replayed, capture = context.replayed, context.metadata
        _, current_requests, _ = quietly(phase2_corpus.requests, native)
        metrics["harness_valid"] = (not replayed["summary"]["partial"] and replayed["summary"]["harness_errors"] == 0
                                    and replayed["source_stable"]
                                    and replayed["summary"]["observed"] == document["counts"]["executed"]
                                    and capture["native"]["observation_sha256"] == report["observation_sha256"]
                                    and capture["requests_sha256"] == digest(
                                        phase2_corpus.p4.canonical(current_requests) + b"\n"))
    except (OSError, ValueError, KeyError) as error:
        print("Rust capture unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    if not metrics["harness_valid"]:
        print("Rust capture is partial, stale or invalid; acceptance metrics withheld", file=sys.stderr)
        return {"metrics": metrics}
    try:
        comparison = quietly(phase2_compare.report, native, rust, write=False, context=context)
    except (OSError, ValueError, KeyError) as error:
        print("comparison unavailable: " + str(error), file=sys.stderr)
        return {"metrics": metrics}
    summary = phase2_compare.acceptance_summary(comparison)
    recorded = json.loads(phase2_compare.RECORD.read_bytes()) if phase2_compare.RECORD.exists() else None
    metrics["result_recorded"] = (metrics["harness_valid"] and recorded is not None
                                  and phase2_compare.acceptance_summary(recorded) == summary)
    rows = [row for row in comparison["rows"] if "outcomes" in row]
    for metric, domain in (("errors_parity", "errors"), ("types_parity", "types"), ("symbols_parity", "symbols"),
                           ("display_parity", "display"), ("ordering", "union_ordering"),
                           ("parent_pointers", "parent_pointers")):
        metrics[metric] = ratio(rows, domain)
    metrics["trace_parity"] = ratio(rows, "trace", exclude_disabled=True)
    metrics["unsupported_required"] = sum(any(o == "unsupported" for o in row["outcomes"].values()) for row in rows)
    regression = [row for row in rows if row["s08"] == "acceptance"]
    metrics["regression_parity"] = (sum(all(o in MATCHED for o in row["outcomes"].values()) for row in regression)
                                    / len(regression)) if regression else None
    executed_rows = phase2_inventory.executed()
    state = two_mode_state(native, rust, native_concurrent, rust_concurrent, document["counts"]["executed"],
                           CHECKPOINT_AUTHORITIES["C6"]["concurrent"])
    metrics.update(run_level_metrics(state, comparison, executed_rows, context=context))
    try:
        claims = strict_json_loads(CLAIMS.read_bytes()) if CLAIMS.is_file() else None
        audit_ok = phase2_audit.complete(phase2_audit.load(AUDIT)) if AUDIT.is_file() else None
        baseline = phase2_compare.load_baseline(BASELINE) if BASELINE.is_file() else None
        blockers = (strict_json_loads(phase2_blockers.REGISTER.read_bytes())
                    if phase2_blockers.REGISTER.exists() else None)
        metrics.update(c1_metrics(comparison, claims, audit_ok, baseline, receipt_current("c1-contracts"),
                                  metrics["regression_parity"], blockers))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("C1 metrics unavailable: " + str(error), file=sys.stderr)
        metrics["c1_complete"] = False
    register = None
    loaded, all_transfers, all_incoming = {}, {}, {}
    for checkpoint in CHECKPOINTS:
        authorities = CHECKPOINT_AUTHORITIES[checkpoint]
        try:
            claims, audit, transfers, incoming = phase2_blockers.load_handoffs(
                checkpoint, authorities["claims"], authorities["audit"], comparison, context)
            for vid in set(transfers) & set(all_transfers):
                raise ValueError(f"variant {vid} is handed off by two checkpoints")
            loaded[checkpoint] = (claims, audit, transfers, incoming)
            all_transfers.update(transfers)
            all_incoming.update(incoming)
        except (OSError, ValueError, KeyError, TypeError) as error:
            print(f"{checkpoint} handoff authorities unavailable: " + str(error), file=sys.stderr)
    try:
        if len(loaded) != len(CHECKPOINTS):
            raise ValueError("every checkpoint's handoff authorities must validate before the register is built")
        register = quietly(phase2_blockers.build, native, rust, context=context, comparison=comparison,
                           handoffs=all_transfers, incoming=all_incoming)
        committed = json.loads(phase2_blockers.REGISTER.read_bytes()) if phase2_blockers.REGISTER.exists() else None
        metrics["blockers_named"] = register == committed and phase2_blockers.complete(register, comparison)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("blocker register unavailable: " + str(error), file=sys.stderr)
    current = newest_checkpoint(loaded)
    for checkpoint in CHECKPOINTS:
        authorities = CHECKPOINT_AUTHORITIES[checkpoint]
        prefix = checkpoint.lower()
        try:
            if checkpoint not in loaded:
                raise ValueError(f"{checkpoint} handoff authority validation did not complete")
            claims, audit, transfers, incoming = loaded[checkpoint]
            audit_ok = phase2_audit.complete(audit) if audit is not None else None
            baseline = (phase2_compare.load_baseline(authorities["baseline"])
                        if authorities["baseline"].is_file() else None)
            measured = (measurement_current(comparison, rust, context=context)
                        if "measurement" in authorities and checkpoint == current else None)
            services = (services_current(authorities["services"], comparison, context=context)
                        if "services" in authorities and checkpoint == current else None)
            if checkpoint == "C7":
                if checkpoint == current:
                    metrics.update(c7_metrics(metrics, audit_ok, c7_optional("evidence states", evidence_states),
                                              state, comparison))
                continue
            extra = (concurrent_metrics(native, rust, native_concurrent, rust_concurrent, document, comparison,
                                        claims, authorities, state=state)
                     if "concurrent" in authorities and checkpoint == current else None)
            metrics.update(checkpoint_metrics(
                checkpoint, comparison, claims, audit_ok, baseline, receipt_current(prefix + "-contracts"),
                metrics["regression_parity"], register, handoffs=transfers, measured_ok=measured,
                baseline_sha256=digest(authorities["baseline"].read_bytes()) if baseline is not None else None,
                prerequisites=metrics, incoming=incoming, services_ok=services, extra=extra))
        except (OSError, ValueError, KeyError, TypeError) as error:
            print(f"{checkpoint} metrics unavailable: " + str(error), file=sys.stderr)
            metrics[prefix + "_complete"] = False
        if checkpoint != current:
            drop_historical_completion(metrics, checkpoint)
    print("checker evidence: " + canonical({"native": report["observation_sha256"],
                                             "rust": comparison["rust_capture_sha256"]}).decode(), file=sys.stderr)
    return {"metrics": {k: v for k, v in metrics.items() if v is not None}}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("checker", "observe"))
    parser.add_argument("--native", type=Path, default=NATIVE)
    parser.add_argument("--rust", type=Path, default=RUST)
    parser.add_argument("--native-concurrent", type=Path, default=NATIVE_CONCURRENT)
    parser.add_argument("--rust-concurrent", type=Path, default=RUST_CONCURRENT)
    parser.add_argument("--witness", help="contract witness identity for observe")
    parser.add_argument("--measurement", type=Path, help="existing checkerbench capture for c2-measurement")
    args = parser.parse_args()
    if args.command == "observe":
        if not args.witness:
            raise ValueError("observe requires --witness")
        if args.witness == "c2-measurement":
            if args.measurement is None:
                raise ValueError("c2-measurement requires --measurement DIR; it does not run a benchmark")
            observe_measurement(args.measurement, args.native, args.rust)
        else:
            observe(args.witness)
        return
    print(json.dumps(checker(args.native, args.rust, args.native_concurrent, args.rust_concurrent), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 producer failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
