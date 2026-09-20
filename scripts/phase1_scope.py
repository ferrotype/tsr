"""The Phase 1 operation scope: every in-scope Go symbol and its disposition.

Built from the two audit inputs the plan names -- PORTS.toml and
status/unmapped-functions.json -- joined against the actual Rust sources each
ledger row claims as its home. The ledger is an audit input, not a task count:
an unmapped Go function may already be represented in Rust, and a mapped file
may still have no behavioral witness. Every row therefore carries how its
disposition was reached, so a rule-derived guess is never mistaken for a review.

Dispositions are the five the plan fixes: covered, implemented_untested,
missing, equivalent_rust and later_phase.
"""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PHASE = 1

DISPOSITIONS = ("covered", "implemented_untested", "missing", "equivalent_rust", "later_phase")


def ledger() -> list[dict]:
    return tomllib.loads((ROOT / "PORTS.toml").read_text())["file"]


def unmapped() -> dict[str, list[str]]:
    raw = json.loads((ROOT / "status/unmapped-functions.json").read_text())
    return raw["unmapped_functions"]


def unmapped_ids() -> set[str]:
    """Every operation id the audit input reports as unmapped."""
    return {entry for entries in unmapped().values() for entry in entries}


def inventory() -> list[dict]:
    """Every Go function at the pin, from the committed inventory.

    data/go-functions.tsv is the complete inventory, not just the unmapped
    remainder. Building the scope from it is what makes a *mapped* operation
    with no behavioral witness visible; the unmapped list alone would hide it.
    """
    rows: list[dict] = []
    lines = (ROOT / "data/go-functions.tsv").read_text().splitlines()
    header = None
    for line in lines:
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if header is None:
            header = fields
            continue
        rows.append(dict(zip(header, fields)))
    return rows


def snake(name: str) -> str:
    """Go exported/unexported name to the Rust name this port would use."""
    stripped = re.sub(r"^\(\*?[A-Za-z0-9_]+\)\.", "", name)
    out = re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", stripped)
    out = re.sub(r"(?<=[A-Z])([A-Z][a-z])", r"_\1", out)
    return out.lower()


# Planned ledger crates whose behavior actually lives elsewhere today. The plan
# records these boundaries explicitly (section 1, gap 6); an absent crate
# directory is a location observation, not proof the behavior is missing.
KNOWN_HOMES: dict[str, str] = {
    "internal/packagejson": "tsr_module::package_json / tsr_module::package_maps",
    "internal/stringutil": "tsr_jsstring",
    "internal/vfs/vfsmatch": "tsr_tsoptions::glob (configuration matching dialect)",
    "internal/glob": "no Rust home; the LSP/test glob grammar is a separate dialect",
    "internal/evaluator": "partly tsr_checker::template",
    "internal/collections": "tsr_core and consumer-local collections",
    "internal/json": "no dedicated Rust home; order-sensitive readers live in their consumers",
    "internal/locale": "no Rust home at this pin",
}

# A Go name whose snake form is this short or this common matches unrelated Rust
# helpers, so a hit is not evidence. These stay `missing` pending a real witness.
GENERIC_NAMES = frozenset(
    "new len get set find first last next name key value clone copy identity equal compare "
    "contains index insert remove push pop size count parse format read write open close".split()
)


def workspace_symbol_index() -> dict[str, list[str]]:
    """Every `fn` name defined anywhere under crates/, with its locations.

    Searching the whole workspace rather than the ledger's claimed files is
    deliberate: 117 of the 169 Phase 1 ledger rows record no Rust home, and
    several name a crate directory that does not exist because the behavior
    moved (see KNOWN_HOMES).
    """
    index: dict[str, list[str]] = {}
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        text = path.read_text(errors="replace")
        rel = str(path.relative_to(ROOT))
        for name in set(re.findall(r"\bfn\s+([a-z0-9_]+)", text)):
            index.setdefault(name, []).append(rel)
    return index


def classify(entry: dict, symbol: str, mapped: bool, index: dict[str, list[str]]) -> tuple[str, str]:
    """Return (disposition, basis). Basis records how the disposition was reached."""
    status = entry.get("status")
    verify = entry.get("verify") or []

    if status == "out-of-scope":
        return "later_phase", "ledger marks the source file out of scope for the port"
    if mapped:
        if verify:
            return "covered", f"ledger maps the symbol and records {len(verify)} verification link(s)"
        return "implemented_untested", "ledger maps the symbol but records no verification link"

    candidate = snake(symbol)
    locations = index.get(candidate, [])
    if locations and candidate not in GENERIC_NAMES and len(candidate) >= 6:
        shown = ", ".join(locations[:2]) + (" ..." if len(locations) > 2 else "")
        return (
            "implemented_untested",
            f"unmapped in the audit input, but `{candidate}` is defined at {shown}; "
            "needs a behavioral witness to become covered",
        )
    if locations:
        return (
            "missing",
            f"unmapped; `{candidate}` matches an unrelated generic Rust helper, which is not evidence",
        )
    home = KNOWN_HOMES.get(entry["package"])
    if home:
        return "missing", f"unmapped and absent from the actual home for this package ({home})"
    return "missing", "unmapped in the audit input and absent from every Rust crate by name"


def build() -> dict:
    rows: list[dict] = []
    index = workspace_symbol_index()
    missing_ids = unmapped_ids()
    entries = {e["go"]: e for e in ledger() if e.get("phase") == PHASE}
    for function in inventory():
        entry = entries.get(function["file"])
        if entry is None:
            continue
        identity = function["id"]
        symbol = (
            f'{function["receiver"]}.{function["name"]}' if function.get("receiver") else function["name"]
        )
        mapped = identity not in missing_ids
        disposition, basis = classify(entry, symbol, mapped, index)
        rows.append(
            {
                "id": identity,
                "go_source": function["file"],
                "go_package": function["package"],
                "symbol": symbol,
                "mapped_in_ledger": mapped,
                "ledger_status": entry.get("status"),
                "ledger_crate": entry.get("crate"),
                "rust_home": list(entry.get("rust") or []),
                "actual_home": KNOWN_HOMES.get(entry["package"]),
                "disposition": disposition,
                "basis": basis,
                "basis_kind": "rule",
                "destination_phase": PHASE if disposition != "later_phase" else None,
            }
        )
    rows.sort(key=lambda r: r["id"])
    counts: dict[str, int] = {d: 0 for d in DISPOSITIONS}
    for row in rows:
        counts[row["disposition"]] += 1
    return {
        "version": 1,
        "pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
        "phase": PHASE,
        "dispositions": list(DISPOSITIONS),
        "counts": counts,
        "total_operations": len(rows),
        "operations": rows,
    }


def verify(scope: dict) -> list[str]:
    problems: list[str] = []
    rows = scope.get("operations", [])
    if not rows:
        problems.append("scope has no operations; an empty inventory cannot be complete")
    ids = [r["id"] for r in rows]
    if len(ids) != len(set(ids)):
        problems.append("scope contains duplicate operation ids")
    for row in rows:
        if row.get("disposition") not in DISPOSITIONS:
            problems.append(f"{row.get('id')}: unclassified disposition {row.get('disposition')!r}")
        if not row.get("basis"):
            problems.append(f"{row.get('id')}: disposition has no recorded basis")
        if row.get("disposition") == "later_phase" and row.get("destination_phase") is not None:
            problems.append(f"{row.get('id')}: later_phase row must name a destination outside phase 1")
        if row.get("disposition") == "equivalent_rust" and row.get("basis_kind") != "review":
            problems.append(f"{row.get('id')}: equivalent_rust requires a reviewed behavioral witness")
    counts: dict[str, int] = {d: 0 for d in DISPOSITIONS}
    for row in rows:
        if row.get("disposition") in counts:
            counts[row["disposition"]] += 1
    if counts != scope.get("counts"):
        problems.append("scope counts disagree with its own operation rows")
    if scope.get("total_operations") != len(rows):
        problems.append("scope total_operations disagrees with its own operation rows")
    return problems
