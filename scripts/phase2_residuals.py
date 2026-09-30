#!/usr/bin/env python3
"""Phase 2 C7.1: the residual list (docs/PHASE2-C7-plan.md, C7.1).

`build` first rebinds every checkpoint's handoffs to the final single-threaded
capture (`phase2_claims.rebind`), so each open handoff is re-validated, then
compares both modes' final captures with their native captures and writes
`data/phase2/residuals.json`. It lists each executed row that does not match
in some domain in either mode, with:

- the checkpoint or phase that owns its cause: a validated handoff's owner,
  otherwise the row's inventory checkpoint;
- the validated trace that attributes it, when a handoff does;
- the register's blocker that withholds it, if one does;
- its resolution path: `checkpoint` when a checkpoint reopens, `divergence`
  when the divergence ledger proposes an entry for it, and `joint` when the
  cause is outside Phase 2.

A (row, domain) pair that an approved divergence covers (C7.3) is not a
residual. `build --check` rebuilds without writing and fails unless the
committed list and every claims file are already current.

The producer's `c7_residuals` is `verified_count`: the committed list's
length, once its bindings name the comparisons the producer read and its rows
are exactly those comparisons' unmatched rows.
"""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_compare  # noqa: E402
import phase2_inventory  # noqa: E402

RESIDUALS = ROOT / "data/phase2/residuals.json"
MATCHED = phase2_compare.MATCHED
MODES = ("single", "concurrent")
RESOLUTIONS = ("checkpoint", "divergence", "joint")
# The owners inside Phase 2; any other owner (a later phase) is joint.
CHECKPOINTS = tuple(f"C{n}" for n in range(1, 8))


def unmatched(comparison):
    """Each row's domains that do not match, for rows with any."""
    return {row["id"]: sorted(domain for domain, outcome in row["outcomes"].items() if outcome not in MATCHED)
            for row in comparison["rows"]
            if any(outcome not in MATCHED for outcome in row["outcomes"].values())}


def divergence_coverage():
    """The approved (row, domain) pairs and the rows with a proposed entry,
    from C7.3's ledger check; an empty ledger covers nothing."""
    try:
        import phase2_divergences
    except ImportError:
        return set(), set()
    return phase2_divergences.approved_coverage(), phase2_divergences.proposed_variants()


def open_rows(single, concurrent, covered=frozenset()):
    """Each residual row's unmatched domains per mode, without covered pairs."""
    modes = {"single": unmatched(single), "concurrent": unmatched(concurrent)}
    rows = {}
    for vid in sorted(set(modes["single"]) | set(modes["concurrent"])):
        domains = {mode: [domain for domain in modes[mode].get(vid, []) if (vid, domain) not in covered]
                   for mode in MODES}
        if any(domains.values()):
            rows[vid] = domains
    return rows


def residual_rows(single, concurrent, *, owners, handoffs, register, covered=frozenset(), proposed=frozenset()):
    """The residual list's rows from both comparisons and the attributions."""
    blockers = {}
    for entry in (register or {}).get("entries", []):
        for ownership in entry.get("ownership", []):
            blockers.setdefault(ownership["variant"], entry["id"])
    rows = []
    for vid, domains in open_rows(single, concurrent, covered).items():
        handoff = handoffs.get(vid)
        owner = handoff["owner"] if handoff else owners[vid]
        attribution = None
        if handoff:
            attribution = {"from": handoff.get("from"), "go": handoff["go"], "cause": handoff["cause"],
                           "trace": handoff["trace"]}
        resolution = ("divergence" if vid in proposed
                      else "checkpoint" if owner in CHECKPOINTS else "joint")
        rows.append({"id": vid, "domains": domains, "owner": owner, "attribution": attribution,
                     "blocker": blockers.get(vid), "resolution": resolution})
    return rows


def bindings(single, concurrent):
    return {mode: {"rust_capture_sha256": comparison["rust_capture_sha256"],
                   "native_observation_sha256": comparison["native_observation_sha256"]}
            for mode, comparison in zip(MODES, (single, concurrent), strict=True)}


def document(single, concurrent, rows):
    return {"version": 1, "captures": bindings(single, concurrent), "rows": rows}


def render(value):
    return json.dumps(value, indent=1, sort_keys=True) + "\n"


def validate(value):
    """Schema of a residual list."""
    if (not isinstance(value, dict) or set(value) != {"version", "captures", "rows"} or value["version"] != 1
            or not isinstance(value["captures"], dict) or set(value["captures"]) != set(MODES)
            or not isinstance(value["rows"], list)):
        raise ValueError("a residual list needs version 1, both modes' capture bindings and rows")
    seen = set()
    for row in value["rows"]:
        if (not isinstance(row, dict)
                or set(row) != {"id", "domains", "owner", "attribution", "blocker", "resolution"}
                or row["id"] in seen or row["resolution"] not in RESOLUTIONS
                or not isinstance(row["domains"], dict) or set(row["domains"]) != set(MODES)
                or not any(row["domains"].values())
                or any(not set(domains) <= set(phase2_compare.DOMAINS) for domains in row["domains"].values())):
            raise ValueError("malformed residual row: " + str(row.get("id") if isinstance(row, dict) else row))
        seen.add(row["id"])
    return value


def verified_count(comparison, concurrent, path=RESIDUALS):
    """The committed list's length, once it names these comparisons and lists
    exactly their unmatched rows; otherwise the list is stale and raises."""
    if comparison is None or concurrent is None:
        raise ValueError("the residual list needs both modes' comparisons")
    value = validate(json.loads(Path(path).read_bytes()))
    if value["captures"] != bindings(comparison, concurrent):
        raise ValueError("the residual list names other captures")
    covered, _ = divergence_coverage()
    expected = open_rows(comparison, concurrent, covered)
    if {row["id"]: row["domains"] for row in value["rows"]} != expected:
        raise ValueError("the residual list differs from the comparisons' unmatched rows")
    return len(value["rows"])


def build(native, rust, native_concurrent, rust_concurrent, *, check=False, path=RESIDUALS):
    import phase2_blockers
    import phase2_claims
    import phase2_producers
    context = phase2_compare.load_context(native, rust)
    single = phase2_compare.report(native, rust, write=False, context=context)
    concurrent = phase2_compare.report(native_concurrent, rust_concurrent, write=False)
    capture_sha256, digests = phase2_claims.capture_digests(rust)
    rebinds, stale = {}, []
    for checkpoint, (claims_path, _) in sorted(phase2_blockers.CHECKPOINT_CLAIMS.items()):
        claims = json.loads(claims_path.read_bytes())
        result = phase2_claims.rebind(claims, single, capture_sha256, digests)
        stale += [(checkpoint, vid) for vid in result["stale"] + result["changed"]]
        rebinds[claims_path] = claims
    changed = [claims_path for claims_path, claims in rebinds.items()
               if render(claims) != claims_path.read_text()]
    if check and changed:
        raise ValueError("claims files are not bound to the final capture: "
                         + ", ".join(str(claims_path.relative_to(ROOT)) for claims_path in changed))
    if not check:
        for claims_path in changed:
            claims_path.write_text(render(rebinds[claims_path]))
    handoffs = {}
    for checkpoint in phase2_producers.CHECKPOINTS:
        authorities = phase2_producers.CHECKPOINT_AUTHORITIES[checkpoint]
        _, _, outgoing, incoming = phase2_blockers.load_handoffs(
            checkpoint, authorities["claims"], authorities["audit"], single, context)
        handoffs.update(outgoing)
        handoffs.update(incoming)
    register = (json.loads(phase2_blockers.REGISTER.read_bytes())
                if phase2_blockers.REGISTER.exists() else None)
    covered, proposed = divergence_coverage()
    owners = {row["id"]: row["checkpoint"] for row in phase2_inventory.executed()}
    rows = residual_rows(single, concurrent, owners=owners, handoffs=handoffs, register=register,
                         covered=covered, proposed=proposed)
    value = document(single, concurrent, rows)
    rendered = render(value)
    if check:
        if not Path(path).is_file() or Path(path).read_text() != rendered:
            raise ValueError("the committed residual list differs from the rebuilt one")
    else:
        Path(path).write_text(rendered)
    return {"residuals": len(rows), "rebound_claims": [str(p.relative_to(ROOT)) for p in changed],
            "open_handoffs": [f"{checkpoint} {vid}" for checkpoint, vid in stale],
            "owners": sorted({row["owner"] for row in rows})}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build",))
    parser.add_argument("--native", type=Path, default=ROOT / "target/phase2/native")
    parser.add_argument("--rust", type=Path, required=True, help="the final single-threaded capture")
    parser.add_argument("--native-concurrent", type=Path, default=ROOT / "target/phase2/native-concurrent")
    parser.add_argument("--rust-concurrent", type=Path, required=True, help="the final concurrent capture")
    parser.add_argument("--check", action="store_true", help="fail unless the committed list is current")
    args = parser.parse_args()
    result = build(args.native, args.rust, args.native_concurrent, args.rust_concurrent, check=args.check)
    print(json.dumps(result, indent=1))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 residuals failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
