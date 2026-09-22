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


def _bytes(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode()


def _sha(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def build(root: Path = ROOT) -> dict:
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
    roster = load("data/phase1/syntax-roster.json")
    compiler_exemptions = {row["operation"] for row in roster["exemptions"]
                          if row["category"] == "later_step" and row["operation"].startswith("tsc/internal/compiler/")}
    if reviewed.keys() & unresolved or reviewed.keys() | unresolved != compiler_exemptions:
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
        if row.get("family") != request[0] or row.get("request_sha256") != _sha(capture.canonical(request[2]) + b"\n"):
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
        if row.get("baseline"):
            baseline = row["baseline"]
            baseline_path = baseline["path"] if isinstance(baseline, dict) else baseline
            baseline_links[baseline_path].append(identity)
            metric, production_item = "run.config.parity", "P1B-F3b"
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
        result = row.get("last_result", "not_run")
        if result not in capture.RESULTS:
            problems.append(f"{identity}: unknown recorded result {result!r}")
        if result not in ("native_unavailable", "not_run") and not native[identity]:
            problems.append(f"{identity}: recorded comparison has no frozen native observation")
        joined_cases.append({
            "id": identity, "family": family, "request": request_path,
            "request_sha256": _sha(capture.canonical(request) + b"\n"),
            "operations": row.get("operations", []), "actions": row.get("operation_actions", {}),
            "native_authority": row.get("native_authority"), "native_observations": native[identity],
            "rust_driver": config.get("rust_driver", config["rust_target"]),
            "comparator": "scripts/phase1_capture.py:compare", "recorded_result": result,
            "producer_metric": metric, "preparation_sprint_item": preparation_item,
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
    for witness in manifest.get("witnesses", []):
        identity = witness["id"]
        if identity in witness_ids or identity in set(case_ids):
            problems.append(f"duplicate witness identity: {identity}")
        witness_ids.add(identity)
        if not (root / witness.get("artifact", "")).is_file():
            problems.append(f"{identity}: witness artifact missing")
        for operation in witness.get("operations", []):
            if operation not in operations:
                problems.append(f"{identity}: orphan witnessed operation {operation}")
            if witness.get("kind") == "rust_gated":
                links[operation].append(identity)
    operation_rows, gaps = [], []
    preparing_cases = {row["id"] for row in joined_cases
                       if row["acceptance"] and row["recorded_result"] in scope.PREPARING_RESULTS}
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
                         "reproduce": "python3 scripts/phase1_coverage.py check"})
    case_gaps = [{"id": row["id"], "family": row["family"], "recorded_result": row["recorded_result"],
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
    for relative in ("scripts/phase1_coverage.py", "scripts/phase1_scope.py", "scripts/phase1_capture.py"):
        inputs[relative] = _sha((root / relative).read_bytes())
    return {"version": 1, "pin": pin, "healthy": not problems,
            "preparation_complete": not problems and not gaps,
            "interpretation": "Recorded comparisons and witness links only; current production acceptance requires authenticated capture replay. Historical pilot rows do not feed production metrics.",
            "counts": {"operations": len(operation_rows), "cases": len(joined_cases), "config_outputs": len(baseline_links),
                       "pending_operations": len(gaps), "nonmatching_cases": len(case_gaps),
                       "reviewed_later_phase_operations": len(reviewed),
                       "recorded_differences_with_scoped_approval": sum(row["recorded_result"] == "different" and row["approved_difference"] is not None for row in joined_cases)},
            "families": families, "operations": operation_rows, "cases": joined_cases,
            "gaps": gaps, "case_gaps": case_gaps,
            "root_causes": [{"cause": cause, "count": len(rows), "example": rows[0]} for cause, rows in sorted(causes.items())],
            "input_sha256": dict(sorted(inputs.items())), "problems": problems}


def verify(document: dict, root: Path = ROOT) -> list[str]:
    """A report must equal a full recomputation, not just its own totals."""
    expected = build(root)
    problems = list(expected["problems"])
    if document != expected:
        problems.append("coverage report differs from current complete inputs")
    return problems


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("report", "check"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    document = build()
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
