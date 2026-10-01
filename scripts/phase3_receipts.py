#!/usr/bin/env python3
"""Phase 3 contract witnesses and their receipts (docs/PHASE3-plan.md, section 5).

`t1-contracts` to `t8-contracts` run the Rust contract suites
`crates/tsr_compiler/tests/t1_contracts.rs` to `t8_contracts.rs` in debug and
release. Each witness binds the sources whose change can alter its outcome:
the production, workspace and generator inputs Phase 2's contract witnesses
bind, the receipt machinery and this script, and the witness's own fixtures.

The receipts are Phase 2's (`scripts/phase2_producers.py`, version 2): a
receipt records every run's command, exit code and output, the exact test
inventory and the digests of the bound sources, and it is current only while
every run passed every inventoried test and no bound source changed.
`observe` and `receipt_current` here run that machinery with the Phase 3
witness registered for the duration of the call, and keep the receipts under
data/phase3/receipts.

  observe ID      run witness ID and write data/phase3/receipts/ID.json; a
                  failing suite keeps its receipt for diagnosis and fails
  current [ID..]  print, as JSON, whether each receipt (default: all) is
                  current (true), stale or failing (false) or missing (null);
                  the exit code is 0 only when every one is current

A producer reads a receipt with `receipt_current("tN-contracts")`.
"""
from __future__ import annotations

import argparse
import contextlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase2_producers as phase2  # noqa: E402

RECEIPTS = phase2.ROOT / "data/phase3/receipts"
# Phase 2's production, workspace and generator inputs (everything under
# `crates`, which includes the suites and their support module), the receipt
# machinery this script runs on, and this script.
SOURCES = [*phase2.PRODUCTION_PATTERNS, "scripts/phase2_producers.py", "scripts/phase3_receipts.py"]
# The deep-input fixture of the corpus that T3's recursion contract emits.
STRESS = "upstream/tsc/testdata/tests/cases/compiler/binderBinaryExpressionStress.ts"


def witness(checkpoint, minimum_tests, extra=()):
    """The witness of checkpoint `checkpoint`: its suite in debug and release."""
    suite = f"t{checkpoint}_contracts"
    return {"commands": [["cargo", "test", "-p", "tsr_compiler", "--test", suite, "--locked", *release]
                         for release in ([], ["--release"])],
            "test_source": f"crates/tsr_compiler/tests/{suite}.rs", "minimum_tests": minimum_tests,
            "test_modules": {}, "sources": [*SOURCES, *extra]}


WITNESSES = {
    "t1-contracts": witness(1, 2),
    "t2-contracts": witness(2, 2),
    "t3-contracts": witness(3, 2, [STRESS]),
    "t4-contracts": witness(4, 2),
    "t5-contracts": witness(5, 2),
    "t6-contracts": witness(6, 2),
    "t7-contracts": witness(7, 2),
    "t8-contracts": witness(8, 5),
}


@contextlib.contextmanager
def registered(identity):
    """`identity` registered with Phase 2's machinery, and removed again."""
    spec = WITNESSES.get(identity)
    if spec is None:
        raise ValueError(f"unknown witness {identity!r}")
    if identity in phase2.WITNESSES:
        raise ValueError(f"witness {identity!r} is already registered with Phase 2")
    phase2.WITNESSES[identity] = spec
    try:
        yield spec
    finally:
        del phase2.WITNESSES[identity]


def observe(identity, output=RECEIPTS):
    """Run one witness and write its receipt; raises when the suite fails."""
    with registered(identity):
        return phase2.observe(identity, output)


def receipt_current(identity, path=None):
    """True when the receipt is current, False when it is stale or records a
    failure, None when it does not exist."""
    with registered(identity):
        return phase2.receipt_current(identity, Path(path) if path else RECEIPTS / f"{identity}.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("observe").add_argument("witness", choices=sorted(WITNESSES))
    commands.add_parser("current").add_argument("witness", nargs="*", choices=[[], *sorted(WITNESSES)])
    args = parser.parse_args()
    if args.command == "observe":
        observe(args.witness)
        return 0
    states = {identity: receipt_current(identity) for identity in (args.witness or sorted(WITNESSES))}
    print(json.dumps(states, sort_keys=True))
    return 0 if all(state is True for state in states.values()) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 receipts failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
