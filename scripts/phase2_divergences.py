#!/usr/bin/env python3
"""Phase 2 C7.3: divergences and approved scope (docs/PHASE2-C7-plan.md, C7.3).

`data/divergences.toml` is the one ADR 0004 ledger E2 also reads, so an entry
here uses E2's schema and metric names, validated by E2's own `approvals`:
scope, kind, rationale, approval, pin and one observation witness per variant
and metric. For Phase 2 a metric stands for comparison domains:
`errors_parity` for `errors`, `types_parity` for `types` and `symbols`, and
`type_to_string_parity` for `display`.

`check` validates every entry against both modes' final comparisons. A witness
must name a row that differs in its domains in each mode: a row that now
matches leaves the witness unused, and failed, unsupported or
native-unavailable rows cannot be approved. Its digests must equal the
observations: the Rust `error_baseline` digest is the comparison's, and the
native one is recomputed from the native row as E2 hashes it. A witness for a
metric whose digests this check does not recompute fails, so an approval
never rests on an unchecked digest.

A proposal is a draft entry, without approval, in
`data/phase2/divergence-proposals.json`; it goes to the owner and changes no
outcome. `approved_coverage` and `proposed_variants` serve the residual list;
`valid` is the producer's `c7_divergences_valid`.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare  # noqa: E402
import phase2_inventory  # noqa: E402

LEDGER = ROOT / "data/divergences.toml"
PROPOSALS = ROOT / "data/phase2/divergence-proposals.json"
DOMAINS = {"errors_parity": ("errors",), "types_parity": ("types", "symbols"),
           "type_to_string_parity": ("display",)}
# The metrics whose witness digests this check recomputes.
CHECKED = {"errors_parity"}
NATIVE_ERROR_FIELDS = ("error_pre_diagnostics", "error_post_diagnostics", "error_diagnostics",
                       "error_render_inputs", "error_pretty", "errors")


def canonical_digest(value):
    """The digest E2 gives a witnessed observation."""
    from s08_oracle import digest
    from s08_p4 import canonical
    return digest(canonical(value))


def pin():
    return json.loads((ROOT / "data/upstream.json").read_text())["pin"]


def approvals(ledger=None):
    """The ledger's witnesses keyed by (variant, metric), validated by E2's schema."""
    import s08_e2_contract
    ledger = tomllib.loads(LEDGER.read_text()) if ledger is None else ledger
    ids = {row["id"] for row in phase2_inventory.executed()}
    return s08_e2_contract.approvals(ledger, pin(), ids)


def approved_coverage(ledger=None):
    """The (variant, domain) pairs the ledger's witnesses approve."""
    return {(variant, domain) for (variant, metric) in approvals(ledger) for domain in DOMAINS[metric]}


def proposed_variants(path=PROPOSALS):
    """Variants named by draft entries awaiting the owner."""
    if not Path(path).is_file():
        return set()
    drafts = json.loads(Path(path).read_text())
    if not isinstance(drafts, list):
        raise ValueError("divergence proposals must be a list of draft entries")
    return {witness["variant_id"] for draft in drafts for witness in draft.get("observations", [])}


def problems(witnesses, comparisons, native_rows):
    """Each witness's defect against the comparisons of both modes."""
    found = []
    rows = {mode: {row["id"]: row for row in comparison["rows"]} for mode, comparison in comparisons.items()}
    for (variant, metric), witness in sorted(witnesses.items()):
        label = f"{witness['approval']} {variant} {metric}"
        if metric not in CHECKED:
            found.append(f"{label}: this check does not recompute {metric} digests")
            continue
        for mode, by_id in rows.items():
            row = by_id.get(variant)
            outcomes = [row["outcomes"][domain] for domain in DOMAINS[metric]] if row else []
            if not row or all(outcome in phase2_compare.MATCHED for outcome in outcomes):
                found.append(f"{label}: unused in the {mode} comparison")
            elif any(outcome not in ("different", *phase2_compare.MATCHED) for outcome in outcomes):
                found.append(f"{label}: a {', '.join(sorted(set(outcomes)))} row cannot be approved ({mode})")
            elif row.get("digests", {}).get("errors") != witness["rust_sha256"]:
                found.append(f"{label}: the Rust observation changed ({mode})")
            native = native_rows.get(mode, {}).get(variant)
            if native is None or canonical_digest({key: native[key] for key in NATIVE_ERROR_FIELDS}) \
                    != witness["native_sha256"]:
                found.append(f"{label}: the native observation differs ({mode})")
    return found


def native_rows(native, native_concurrent, variants):
    import phase2_native
    import phase2_native_concurrent
    found = {}
    for mode, directory, module in (("single", native, phase2_native),
                                    ("concurrent", native_concurrent, phase2_native_concurrent)):
        _, _, rows = module.load_capture(directory)
        found[mode] = {row["id"]: row for row in rows if row["id"] in variants}
    return found


def valid(comparison, concurrent, *, native=ROOT / "target/phase2/native",
          native_concurrent=ROOT / "target/phase2/native-concurrent", ledger=None):
    """Every approved witness holds against both final comparisons."""
    if comparison is None or concurrent is None:
        raise ValueError("the divergence check needs both modes' comparisons")
    witnesses = approvals(ledger)
    if not witnesses:
        return True
    rows = native_rows(native, native_concurrent, {variant for variant, _ in witnesses})
    return not problems(witnesses, {"single": comparison, "concurrent": concurrent}, rows)


def check(native, rust, native_concurrent, rust_concurrent):
    witnesses = approvals()
    comparisons = {"single": phase2_compare.report(native, rust, write=False),
                   "concurrent": phase2_compare.report(native_concurrent, rust_concurrent, write=False)}
    rows = native_rows(native, native_concurrent, {variant for variant, _ in witnesses}) if witnesses else {}
    found = problems(witnesses, comparisons, rows)
    return {"approved_witnesses": len(witnesses), "proposals": len(proposed_variants()), "problems": found}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("check",))
    parser.add_argument("--native", type=Path, default=ROOT / "target/phase2/native")
    parser.add_argument("--rust", type=Path, required=True, help="the final single-threaded capture")
    parser.add_argument("--native-concurrent", type=Path, default=ROOT / "target/phase2/native-concurrent")
    parser.add_argument("--rust-concurrent", type=Path, required=True, help="the final concurrent capture")
    args = parser.parse_args()
    result = check(args.native, args.rust, args.native_concurrent, args.rust_concurrent)
    print(json.dumps(result, indent=1))
    if result["problems"]:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 divergences failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
