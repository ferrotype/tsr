#!/usr/bin/env python3
"""Phase 3 T8: the residual list, data/phase3/residuals.json (docs/PHASE3-plan.md
section 4, T8's closure).

Every executed row that does not match in one of its domains in either mode's
final comparison: the reprint witness, output, source maps, the source-map
record, emit diagnostics, and the declaration domain where the row requires it
(the producer's `declaration_parity` rule). `disabled` matches, as in the
parity metrics. The list is rebuilt from the two comparisons and never edited:
`build` writes it, `check` requires the committed list to equal the rebuilt
one, and the `emit` producer's `residuals` is its count, emitted only while the
committed list is current.

    build  [--native-single DIR] [--rust-single DIR] [--native-concurrent DIR] [--rust-concurrent DIR]
    check  (the same options)
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT  # noqa: E402
import phase3_compare  # noqa: E402
import phase3_native  # noqa: E402

RESIDUALS = ROOT / "data/phase3/residuals.json"
MODES = phase3_native.MODES


def residual_domains(row):
    """The domains of one comparison row that do not match."""
    return {domain: outcome for domain, outcome in sorted(row["outcomes"].items())
            if outcome not in phase3_compare.MATCHED
            and (domain != "declaration" or row["declaration_required"])}


def document(comparisons):
    """The residual list of one comparison per mode."""
    if set(comparisons) != set(MODES):
        raise ValueError("the residual list needs both modes' comparisons")
    rows = {}
    for mode in MODES:
        for row in comparisons[mode]["rows"]:
            domains = residual_domains(row)
            if domains:
                rows.setdefault(row["id"], {})[mode] = domains
    return {
        "version": 1,
        "pin": strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"],
        "comparisons": {mode: {"native": comparisons[mode]["native"]["observation_sha256"],
                               "rust": comparisons[mode]["rust"]["capture_sha256"]} for mode in MODES},
        "count": len(rows),
        "rows": [{"id": identity, "modes": modes} for identity, modes in sorted(rows.items())],
    }


def render(residuals):
    return json.dumps(residuals, indent=1, sort_keys=True) + "\n"


def committed(path=RESIDUALS):
    return strict_json_loads(Path(path).read_bytes()) if Path(path).is_file() else None


def comparisons(native, rust):
    return {mode: phase3_compare.report(native[mode], rust[mode]) for mode in MODES}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "check"))
    for mode in MODES:
        parser.add_argument(f"--native-{mode}", type=Path, default=ROOT / f"target/phase3/native-{mode}")
        parser.add_argument(f"--rust-{mode}", type=Path, default=ROOT / f"target/phase3/rust-{mode}")
    args = parser.parse_args()
    native = {mode: getattr(args, f"native_{mode}") for mode in MODES}
    rust = {mode: getattr(args, f"rust_{mode}") for mode in MODES}
    rebuilt = document(comparisons(native, rust))
    if args.command == "build":
        RESIDUALS.write_text(render(rebuilt))
    elif committed() != rebuilt:
        print("data/phase3/residuals.json differs from the rebuilt list; rerun build", file=sys.stderr)
        return 1
    print(json.dumps({"count": rebuilt["count"]}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 residuals failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
