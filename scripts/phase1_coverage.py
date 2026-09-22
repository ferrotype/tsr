"""Child-free Phase A coverage join, separate from fresh production parity.

The report retains every F0 operation and every request, including historical
pilot failures. A last_result is historical metadata, never a current producer
result. Operation preparation gaps cannot disappear behind passing corpus rows.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import gzip
import hashlib
import json
from pathlib import Path
import shlex

import phase1_capture as capture
import phase1_scope as scope

ROOT = Path(__file__).resolve().parents[1]
ROUTES = {
    "leaves": ("run.foundations.leaves_complete", "P1A-F1a", "P1B-F1b"),
    "filesystem": ("run.foundations.filesystem_complete", "P1A-F2a", "P1B-F2b"),
    "config": ("run.config.direct_complete", "P1A-F3a", "P1B-F3b"),
    "syntax": ("run.foundations.utilities_complete", "P1A-F4a", "P1B-F4b"),
    "pilot": (None, "P1A-F0", None),
}
OWNERS = {"leaves": "F1b", "filesystem": "F2b", "config": "F3b", "syntax": "F4b"}

# Audited execution routes, not guesses from the witness's human-readable
# `producer` text. Each runner below checks an exact named test inventory or
# compares every row of its declared corpus before emitting the metric.
RUST_ROUTES = {
    "run.program.helpers": ("scripts/s07_program_helpers.py", "data/s07/program-helper-tests.json", (
        "rust-contract/tsr-tsoptions-boundary", "witness/s07-config-mappers-rust", "witness/s07-config-resolver-rust",
        "witness/s07-config-source-rust", "witness/s07-include-reasons-rust", "witness/s07-module-trace-rust",
        "witness/s07-packagejson-rust", "witness/s07-parsed-command-line-rust", "witness/s07-path-observations-rust",
        "witness/s07-program-loader-rust", "witness/s07-verify-options-rust", "witness/s07-program-boundaries-rust",
        "witness/s07-include-helper-paths-rust")),
    "run.binder.helpers": ("scripts/s07_helpers.py", "data/s07/helper-tests.json", (
        "witness/s07-ast-helpers-rust", "witness/s07-bound-clone-rust", "witness/s07-diagnostic-order-rust",
        "witness/s07-helper-tests-go-sort", "witness/s07-helper-tests-pattern", "witness/s07-resolver-audited-helper-paths",
        "witness/s07-resolver-rust", "witness/s07-scanner-declaration-text-path", "witness/s07-scanner-helpers-rust")),
    "run.e1.ast_utilities": ("scripts/s06_utilities.py", "data/s06/utility-tests.json", (
        "witness/s06-accessor-observations-rust", "witness/s06-ast-utilities-middle-rust",
        "witness/s06-utilities-front-rust", "witness/s06-utilities-tail-rust")),
    "run.e1.ast_runtime": ("scripts/s06.py", "data/s06/fixtures.json", (
        "witness/s06-factory-fixtures-rust", "witness/s06-kind-stringer")),
    "run.e1.parity": ("scripts/s06.py", "data/s06/cases.json", ("witness/s06-e1-parse-corpus",)),
    "run.binder.parity": ("scripts/s07_producers.py", "data/s07/binder-cases.json", ("witness/s07-binder-corpus",)),
    "run.binder.supplemental_parity": ("scripts/s07_producers.py", "data/s07/binder-probes.json", (
        "witness/s07-binder-supplemental-diagnostics-paths", "witness/s07-binder-supplemental-generated-names")),
    "run.scanner.parity": ("scripts/s05.py", "data/s05/cases.json", (
        "witness/s05-scanner-cases", "witness/s05-scanner-operation-actions")),
    "run.e4.parity": ("scripts/s04.py", "data/s04/e4-cases.json", (
        "witness/s04-e4-probes", "witness/s04-scanner-position-actions")),
}


def metric_contributors(cases, witnesses, external_inventories=()):
    """Every published behavior route must name at least one actual witness."""
    contributors = defaultdict(list)
    for row in cases:
        for metric in row["producer_metrics"]:
            contributors[metric].append(row["id"])
    for row in witnesses:
        for metric in row["producer_metrics"]:
            contributors[metric].append(row["id"])
    for row in external_inventories:
        if type(row.get("contributor_count")) is not int or row["contributor_count"] <= 0:
            raise ValueError("external metric inventory has no contributing observations: " + row["id"])
        for metric in row["producer_metrics"]:
            contributors[metric].append("inventory:" + row["id"])
    required = {row[0] for row in ROUTES.values() if row[0]} | {"run.config.parity"} | set(RUST_ROUTES)
    required.add("run.foundations.rust_witnesses_complete")
    required.update(("run.syntax.parity", "run.foundations.integration_complete"))
    empty = sorted(metric for metric in required if not contributors[metric])
    if empty:
        raise ValueError("metrics have no contributing observations: " + ", ".join(empty))
    return {metric: sorted(set(ids)) for metric, ids in sorted(contributors.items())}


def _bytes(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode()


def _sha(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def build(root: Path = ROOT, *, supplemental_prepared_cases=()) -> dict:
    """Rebuild all links without subprocesses or reading untracked captures."""
    inputs: dict[str, str] = {}
    problems: list[str] = []

    def load(relative: str) -> dict:
        path = root / relative
        raw = path.read_bytes()
        inputs[relative] = _sha(raw)
        return json.loads(raw)

    document = load("data/phase1/scope.json")
    manifest = load("data/phase1/cases.json")
    review = load("data/phase1/coverage-review.json")
    approval_document = load("data/phase1/approved-differences.json")
    baselines = load("data/phase1/config-baselines.json")
    pin = load("data/upstream.json")["pin"]
    for name, value in (("scope", document), ("cases", manifest), ("review", review), ("baselines", baselines),
                        ("approved differences", approval_document)):
        if value.get("pin") != pin:
            problems.append(f"{name}: pin differs")
    problems.extend(scope.verify(document))
    problems.extend(scope.gap_record_problems(manifest))
    operations = {row["id"]: row for row in document["operations"]}
    # Derive the universe from the pinned inventory, not from report rows. A
    # missing operation cannot make the complete report smaller and greener.
    inventory = root / "data/go-functions.tsv"
    inputs["data/go-functions.tsv"] = _sha(inventory.read_bytes())
    lines = [line.split("\t") for line in inventory.read_text().splitlines()
             if line and not line.startswith("#")]
    headers, *values = lines
    known = [dict(zip(headers, row)) for row in values]
    required = {row["id"] for row in known
                if scope.MEMBERSHIP.get(row["package"], (None,))[0] in ("full", "partial")}
    if required != operations.keys():
        problems.append("scope operation identity set differs from the complete pinned inventory")
    decisions = review["reviewed_operation_destinations"]
    reviewed = {row["operation"]: row for row in decisions}
    if len(reviewed) != len(decisions):
        problems.append("duplicate reviewed operation destination")
    for identity, decision in reviewed.items():
        row = operations.get(identity, {})
        source = root / "upstream" / identity.rsplit(":", 1)[0]
        if not source.is_file() or _sha(source.read_bytes()) != decision.get("go_source_sha256"):
            problems.append(f"{identity}: reviewed pinned source changed")
        if row.get("disposition") != "later_phase" or row.get("destination_phase") != decision["destination_phase"]:
            problems.append(f"{identity}: scope and reviewed destination disagree")
    unresolved = set(review["unresolved_compiler_destinations"])
    unused_rows = review.get("reviewed_unused_compiler_operations", [])
    unused = {row["operation"]: row for row in unused_rows}
    if len(unused) != len(unused_rows):
        problems.append("duplicate reviewed unused compiler operation")
    for identity, decision in unused.items():
        row = operations.get(identity, {})
        source = root / "upstream" / identity.rsplit(":", 1)[0]
        if not row or not identity.startswith("tsc/internal/compiler/"):
            problems.append(f"{identity}: unknown reviewed unused compiler operation")
        if not source.is_file() or _sha(source.read_bytes()) != decision.get("go_source_sha256"):
            problems.append(f"{identity}: reviewed pinned source changed")
        if not decision.get("reason") or not decision.get("evidence") or row.get("roster", {}).get("state") != "exempt:unused_at_pin":
            problems.append(f"{identity}: unused review lacks an exact roster exemption and evidence")
    roster = load("data/phase1/syntax-roster.json")
    compiler_exemptions = {row["operation"] for row in roster["exemptions"]
                          if (row["category"] == "later_step" or row["operation"] in unused)
                          and row["operation"].startswith("tsc/internal/compiler/")}
    if (reviewed.keys() & unresolved or reviewed.keys() & unused.keys() or unused.keys() & unresolved
            or reviewed.keys() | unused.keys() | unresolved != compiler_exemptions):
        problems.append("compiler destination review does not account for every later-step exemption exactly once")

    requests: dict[str, tuple[str, str, dict]] = {}
    for family, config in capture.FAMILIES.items():
        paths = config["requests"]
        if isinstance(paths, str):
            paths = [paths]
        for relative in paths:
            for request in load(relative)["requests"]:
                identity = request["case"]
                if identity in requests:
                    problems.append(f"duplicate request ownership: {identity}")
                requests[identity] = (family, relative, request)
    cases = manifest["cases"]
    case_ids = [row["id"] for row in cases]
    if len(case_ids) != len(set(case_ids)):
        problems.append("duplicate case ownership in cases.json")
    if set(case_ids) != requests.keys():
        problems.append("case/request identity sets differ: " + ", ".join(sorted(set(case_ids) ^ requests.keys())))
    approvals: dict[str, dict] = {}
    for row in approval_document["differences"]:
        identity = row["case"]
        request = requests.get(identity)
        if identity in approvals or request is None:
            problems.append(f"{identity}: duplicate or unknown approved difference")
            continue
        if row.get("family") != request[0] or row.get("request_sha256") != _sha(capture.request_bytes(request[2])):
            problems.append(f"{identity}: approved difference request changed")
        if not row.get("approved_by") or not (root / row.get("decision", "").split("#")[0]).is_file():
            problems.append(f"{identity}: approved difference lacks a recorded owner decision")
        if "native" not in row or "rust" not in row or row["native"] == row["rust"]:
            problems.append(f"{identity}: approved difference lacks two distinct complete observations")
        approvals[identity] = row

    native: dict[str, list[dict]] = defaultdict(list)
    for path in sorted((root / "data/phase1/native").rglob("observations.json")):
        relative = str(path.relative_to(root))
        for row in load(relative).get("observations", []):
            if row.get("result") == "observed":
                native[row["case"]].append({"artifact": relative, "observation_sha256": _sha(_bytes(row.get("observation")))})
    links: dict[str, list[str]] = defaultdict(list)
    recorded_results = scope.recorded_results(manifest)
    joined_cases = []
    baseline_links: dict[str, list[str]] = defaultdict(list)
    for row in sorted(cases, key=lambda value: value["id"]):
        identity, family = row["id"], row["family"]
        scheduled = requests.get(identity)
        if not scheduled:
            continue
        request_family, request_path, request = scheduled
        if request_family != family:
            problems.append(f"{identity}: request belongs to {request_family}, case claims {family}")
        if family not in ROUTES:
            problems.append(f"{identity}: no producer/sprint route for {family}")
            continue
        config = capture.FAMILIES[family]
        metric, preparation_item, production_item = ROUTES[family]
        routes = [metric] if metric else []
        if row.get("baseline"):
            baseline = row["baseline"]
            baseline_path = baseline["path"] if isinstance(baseline, dict) else baseline
            baseline_links[baseline_path].append(identity)
            metric, production_item = "run.config.parity", "P1B-F3b"
            routes = [metric]
            if family == "filesystem":
                routes.append("run.foundations.filesystem_complete")
        for operation in row.get("operations", []):
            if operation not in operations:
                problems.append(f"{identity}: orphan operation {operation}")
            links[operation].append(identity)
        if not set(row.get("coverage_operations", row.get("operations", []))) <= set(row.get("operations", [])):
            problems.append(f"{identity}: coverage claims an operation outside its case")
        for action, claimed in row.get("operation_actions", {}).items():
            if not set(claimed) <= set(row.get("operations", [])):
                problems.append(f"{identity}/{action}: action claims an operation outside its case")
        if not row.get("native_authority"):
            problems.append(f"{identity}: missing native authority")
        result = recorded_results.get(identity, "not_run")
        if result not in capture.RESULTS:
            problems.append(f"{identity}: unknown recorded result {result!r}")
        if result not in ("native_unavailable", "not_run") and not native[identity]:
            problems.append(f"{identity}: recorded comparison has no frozen native observation")
        joined_cases.append({
            "id": identity, "family": family, "request": request_path,
            "request_sha256": _sha(capture.request_bytes(request)),
            "operations": row.get("operations", []), "actions": row.get("operation_actions", {}),
            "native_authority": row.get("native_authority"), "native_observations": native[identity],
            "expected_contract": row.get("expected_contract"), "missing_operations": row.get("missing_operations", []),
            "rust_driver": config.get("rust_driver", config["rust_target"]),
            "comparator": "scripts/phase1_capture.py:compare", "recorded_result": result,
            "historical_result": row.get("last_result", "not_run"),
            "producer_metric": metric, "producer_metrics": routes, "preparation_sprint_item": preparation_item,
            "production_sprint_item": production_item,
            "acceptance": family != "pilot",
            "approved_difference": ({"decision": approvals[identity]["decision"],
                                      "scope": "Only the exact recorded request/native/Rust pair; current acceptance requires replay."}
                                     if identity in approvals else None),
            "reproduce": f"python3 scripts/phase1.py capture --family {family} --case {identity} --output target/phase1-review-case",
        })
    expected_outputs = {entry["output"].removeprefix("tsc/testdata/baselines/reference/") for group in baselines["groups"].values() for entry in group["outputs"]}
    if len(expected_outputs) != 309 or set(baseline_links) != expected_outputs:
        problems.append("config baseline output identities are not exactly the 309 frozen paths")
    for path, ids in baseline_links.items():
        if len(ids) != 1:
            problems.append(f"baseline {path}: duplicate case ownership {ids}")

    witness_ids: set[str] = set()
    witness_routes = {}
    for metric, (runner, inventory, identities) in RUST_ROUTES.items():
        for relative in (runner, inventory):
            inputs[relative] = _sha((root / relative).read_bytes())
        for identity in identities:
            witness_routes[identity] = {"producer_metrics": [metric], "runner": runner, "inventory": inventory}
    from phase1_integration import RUST_WITNESS_TESTS
    for identity, (command, tests) in RUST_WITNESS_TESTS.items():
        witness_routes[identity] = {"producer_metrics": ["run.foundations.rust_witnesses_complete"],
                                   "runner": "scripts/phase1_integration.py", "command": command, "tests": tests}
    joined_witnesses = []
    for witness in manifest.get("witnesses", []):
        identity = witness["id"]
        if identity in witness_ids or identity in set(case_ids):
            problems.append(f"duplicate witness identity: {identity}")
        witness_ids.add(identity)
        if not (root / witness.get("artifact", "")).is_file():
            problems.append(f"{identity}: witness artifact missing")
        if witness.get("kind") == "rust_gated" and witness.get("operations"):
            route = witness_routes.get(identity)
            if route is None:
                problems.append(f"{identity}: Rust witness has no audited producer route")
            joined_witnesses.append({"id": identity, "operations": witness["operations"],
                                     **(route or {"producer_metrics": []})})
        for operation in witness.get("operations", []):
            if operation not in operations:
                problems.append(f"{identity}: orphan witnessed operation {operation}")
            if witness.get("kind") == "rust_gated":
                links[operation].append(identity)
    operation_rows, gaps = [], []
    preparing_cases = {row["id"] for row in joined_cases
                       if row["acceptance"] and row["recorded_result"] in scope.PREPARING_RESULTS}
    supplemental = set(supplemental_prepared_cases)
    eligible_supplements = {row["id"] for row in joined_cases
                            if row["acceptance"] and row["recorded_result"] == "native_unavailable"}
    if not supplemental <= eligible_supplements:
        raise ValueError("supplemental preparation must name native-unavailable acceptance cases")
    # Only the producer passes these IDs, after authenticating a platform
    # capture. Recorded outcomes and committed coverage remain unchanged.
    preparing_cases.update(supplemental)
    gated_witnesses = {row["id"] for row in manifest.get("witnesses", []) if row.get("kind") == "rust_gated"}
    for row in document["operations"]:
        identity, disposition = row["id"], row["disposition"]
        roster = row["roster"]
        step = roster.get("step")
        if not set(row.get("cases", [])) <= set(links[identity]):
            problems.append(f"{identity}: scope claims a case/witness that does not link this operation")
        root_cause = None
        if identity in unresolved:
            root_cause = "compiler_destination_unreviewed"
        elif roster.get("state") == "exempt:later_step" and identity not in reviewed:
            # A transfer out of F1a/F3a is not a transfer out of Phase 1. Keep
            # these operations visible until an exact witness or a reviewed
            # phase destination supplies their actual owner.
            if not (set(links[identity]) & (preparing_cases | gated_witnesses)):
                root_cause = "later_step_unresolved"
        elif roster.get("state") == "pending" and not (set(links[identity]) & (preparing_cases | gated_witnesses)):
            root_cause = ("implementation_unverified" if disposition == "missing" and row.get("basis_kind") == "rule"
                          else "operation_witness_missing")
        joined = {"id": identity, "family": step, "disposition": disposition,
                  "destination_phase": row["destination_phase"], "roster_state": roster.get("state"),
                  "links": sorted(set(links[identity])), "root_cause": root_cause,
                  "owner": "F5a" if identity in unresolved else OWNERS.get(step, "F5b")}
        operation_rows.append(joined)
        if root_cause:
            gaps.append({**joined, "native": identity, "rust": row.get("annotated_home") or row.get("rust_home"),
                         "reason": row["basis"], "dependencies": row.get("depends_on", []),
                         "reproduce": "python3 scripts/phase1_coverage.py explain --operation " + shlex.quote(identity)})
    case_causes = {"different": "observation_difference", "not_implemented": "reported_missing_operation",
                   "native_unavailable": "native_platform_unavailable", "harness_failed": "harness_failure",
                   "not_run": "current_request_observation_missing"}
    case_gaps = [{"id": row["id"], "family": row["family"], "recorded_result": row["recorded_result"],
                  "historical_result": row["historical_result"],
                  "root_cause": "approved_semantic_difference" if row["approved_difference"] and row["recorded_result"] == "different" else case_causes[row["recorded_result"]],
                  "expected_contract": row["expected_contract"], "missing_operations": row["missing_operations"],
                  "acceptance": row["acceptance"], "native": row["native_observations"],
                  "rust": {"driver": row["rust_driver"], "recorded_result": row["recorded_result"]},
                  "approved_difference": row["approved_difference"],
                  **({"approved_native": approvals[row["id"]]["native"], "approved_rust": approvals[row["id"]]["rust"]}
                     if row["id"] in approvals else {}),
                  "reproduce": row["reproduce"]}
                 for row in joined_cases if row["recorded_result"] != "match"]
    families = {family: dict(sorted(Counter(row["recorded_result"] for row in joined_cases if row["family"] == family).items()))
                for family in sorted(ROUTES)}
    causes: dict[str, list[dict]] = defaultdict(list)
    for row in gaps:
        causes[row["root_cause"]].append(row)
    # The complete program corpus and cross-family observations have their
    # own validators. They are not leaf operation witnesses and must not be
    # attributed to every operation reached by a large program. Record their
    # exact, nonempty denominator and validator separately.
    import phase1_syntax
    import phase1_integration
    syntax_schedule = load("data/phase1/syntax-schedule.json")
    syntax_native = load("data/phase1/syntax-native.json")
    syntax_cases = load("data/phase1/syntax-cases.json")
    expected_syntax = phase1_syntax.select(syntax_schedule, syntax_native, full=True)
    if not syntax_cases or syntax_cases != expected_syntax or len(set(syntax_cases)) != len(syntax_cases):
        problems.append("external syntax metric inventory differs from the exact native schedule")
    integration = load("data/phase1/integration.json")
    integration_ids = [row.get("id") for row in integration["witnesses"]]
    if set(integration_ids) != phase1_integration.WITNESSES or len(set(integration_ids)) != len(integration_ids):
        problems.append("external integration metric inventory differs from the required witnesses")
    external_inventories = [
        {"id": "full-program-syntax", "producer_metrics": ["run.syntax.parity"],
         "inventory": "data/phase1/syntax-cases.json", "inventory_sha256": inputs["data/phase1/syntax-cases.json"],
         "contributor_count": len(syntax_cases), "validator": "scripts/phase1_producers.py:replay_program/require_rows",
         "claim": "Every executable native schedule row must appear exactly once in the authenticated full program report."},
        {"id": "integration", "producer_metrics": ["run.foundations.integration_complete"],
         "inventory": "data/phase1/integration.json", "inventory_sha256": inputs["data/phase1/integration.json"],
         "contributor_count": len(integration_ids) + 3, "observations": integration_ids + ["transport", "generation", "rust-witnesses"],
         "validator": "scripts/phase1_integration.py:evaluate",
         "claim": "All named cases and source-bound receipts must execute; transport requires 89 cases and the Rust witness receipt requires its exact test inventory."},
    ]
    try:
        contributions = metric_contributors(joined_cases, joined_witnesses, external_inventories)
    except ValueError as error:
        problems.append(str(error))
        contributions = {}
    for relative in ("scripts/phase1_coverage.py", "scripts/phase1_scope.py", "scripts/phase1_capture.py", "scripts/phase1_integration.py"):
        inputs[relative] = _sha((root / relative).read_bytes())
    return {"version": 1, "pin": pin, "healthy": not problems,
            "preparation_complete": not problems and not gaps,
            "interpretation": "Recorded comparisons and witness links only; current production acceptance requires authenticated capture replay. Historical pilot rows do not feed production metrics.",
            "counts": {"operations": len(operation_rows), "cases": len(joined_cases), "config_outputs": len(baseline_links),
                       "pending_operations": len(gaps), "nonmatching_cases": len(case_gaps),
                       "reviewed_later_phase_operations": len(reviewed),
                       "recorded_differences_with_scoped_approval": sum(row["recorded_result"] == "different" and row["approved_difference"] is not None for row in joined_cases)},
            "families": families, "operations": operation_rows, "cases": joined_cases,
            "witnesses": joined_witnesses, "metric_contributors": contributions,
            "external_inventories": external_inventories,
            "gaps": gaps, "case_gaps": case_gaps,
            "root_causes": [{"cause": cause, "count": len(rows), "example": rows[0]} for cause, rows in sorted(causes.items())],
            "input_sha256": dict(sorted(inputs.items())), "problems": problems,
            **({"supplemental_prepared_cases": sorted(supplemental)} if supplemental else {})}


def verify(document: dict, root: Path = ROOT) -> list[str]:
    """A report must equal a full recomputation, not just its own totals."""
    expected = build(root)
    problems = list(expected["problems"])
    if document != expected:
        problems.append("coverage report differs from current complete inputs")
    return problems


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("report", "check", "explain"))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--operation")
    args = parser.parse_args()
    document = build()
    if args.command == "explain":
        if args.output or not args.operation:
            parser.error("explain requires --operation and cannot write a partial coverage report")
        row = next((row for row in document["operations"] if row["id"] == args.operation), None)
        if row is None:
            parser.error("unknown pinned operation")
        linked = set(row["links"])
        print(json.dumps({"operation": row,
                          "gap": next((gap for gap in document["gaps"] if gap["id"] == args.operation), None),
                          "cases": [case for case in document["cases"] if case["id"] in linked],
                          "witnesses": [witness for witness in document["witnesses"] if witness["id"] in linked]}, indent=2))
        return
    if args.output and args.command == "report":
        args.output.parent.mkdir(parents=True, exist_ok=True)
        raw = _bytes(document)
        args.output.write_bytes(gzip.compress(raw, mtime=0) if args.output.suffix == ".gz" else raw)
    if args.output and args.command == "check":
        raw = args.output.read_bytes()
        document["problems"] += verify(json.loads(gzip.decompress(raw) if args.output.suffix == ".gz" else raw))
    print(json.dumps({key: document[key] for key in ("healthy", "preparation_complete", "counts", "families", "problems")}, indent=2))
    raise SystemExit(1 if document["problems"] else 0)


if __name__ == "__main__":
    main()
