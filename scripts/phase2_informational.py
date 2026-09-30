#!/usr/bin/env python3
"""Phase 2 C7.0: the skip list, data/phase2/informational.json.

Every informational row of data/phase2/inventory.json with its native reason.
The option-guard rows name the pinned guard rules their options fire, the
file-name skips the pinned `skippedTests` list; every classification is
checked against both pinned rules, and every checkpoint's claims must have
been made over the current inventory (no executed row changed classification
since C0). It lives beside phase2_inventory.py because that script is an input
of the recorded native captures.

    python3 scripts/phase2_informational.py write   # writes the skip list
    python3 scripts/phase2_informational.py check   # byte identity and claim bindings
"""
from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from phase2_inventory import INVENTORY, ROOT, RUNNER_TEST_FILE, build, digest, read, render  # noqa: E402

INFORMATIONAL = ROOT / "data/phase2/informational.json"
# The pinned sources of the runner's two skips.
GUARD_SOURCE = "tsc/internal/testutil/harnessutil/harnessutil.go"
RUNNER_SOURCE = "tsc/internal/testrunner/compiler_runner.go"
# harnessutil.SkipUnsupportedCompilerOptions after compiling, including the
# failOnUnsupportedCompilerOptions call it starts with, as option predicates over
# the inventory's effective options (core enum values as in MODULE, TARGET and
# RESOLUTION). Each option-guard row must fire one; no executed row fires any.
GUARD_RULES = (
    ("module", "module is AMD (failOnUnsupportedCompilerOptions)", lambda options: options.get("module") == 2),
    ("outFile", "outFile is set (failOnUnsupportedCompilerOptions)", lambda options: bool(options.get("outFile"))),
    ("module", "module is UMD or System", lambda options: options.get("module") in (3, 4)),
    ("moduleResolution", "moduleResolution is Node10 or Classic",
     lambda options: options.get("moduleResolution") in (1, 2)),
    ("esModuleInterop", "esModuleInterop is false", lambda options: options.get("esModuleInterop") is False),
    ("allowSyntheticDefaultImports", "allowSyntheticDefaultImports is false",
     lambda options: options.get("allowSyntheticDefaultImports") is False),
    ("baseUrl", "baseUrl is set", lambda options: bool(options.get("baseUrl"))),
    ("target", "target is ES5", lambda options: options.get("target") == 1),
    ("alwaysStrict", "alwaysStrict is false", lambda options: options.get("alwaysStrict") is False),
)
GUARD_CALLS = ("t.Fatalf(\"unsupported module kind %s\"", "t.Fatalf(\"unsupported outFile %s\"",
               "t.Skipf(\"unsupported module kind %s\"", "t.Skipf(\"unsupported module resolution kind %d\"",
               "t.Skipf(\"esModuleInterop=false is unsupported\")",
               "t.Skipf(\"allowSyntheticDefaultImports=false is unsupported\")",
               "t.Skipf(\"unsupported baseUrl %s\"", "t.Skipf(\"unsupported target %s\"",
               "t.Skipf(\"alwaysStrict=false is unsupported\")")
REASONS = {
    "option_guard_skip": "harnessutil.SkipUnsupportedCompilerOptions skips verification after compiling",
    "filename_skip": "testrunner.skippedTests names the physical source, so the runner never enumerates it",
    "not_enumerated": "the file does not match compilerBaselineRegex, so EnumerateTestFiles never lists it",
}


def skipped_test_names(source):
    """The pinned runner's skippedTests list."""
    match = re.search(r"(?s)\nvar skippedTests = \[\]string\{(.*?)\n\}", source)
    if match is None:
        raise ValueError("the pinned runner no longer declares skippedTests")
    names = re.findall(r'(?m)^\s*"([^"]+)",\s*$', match.group(1))
    if not names or len(names) != len(set(names)):
        raise ValueError("the pinned skippedTests list is empty or repeats a name")
    return names


def informational(document=None, root=ROOT):
    """C7.0: every informational row with its native reason and, for the runner's
    two skips, the pinned rule that selects it. Fails when the inventory's
    classification disagrees with the pinned guard or file-name rule."""
    document = document or read()
    guard_source = (root / "upstream" / GUARD_SOURCE).read_bytes()
    runner_source = (root / "upstream" / RUNNER_SOURCE).read_bytes()
    guard_text = guard_source.decode()
    start = guard_text.index("func SkipUnsupportedCompilerOptions(")
    end = guard_text.index("\n}\n", guard_text.index("func failOnUnsupportedCompilerOptions("))
    if [call for call in GUARD_CALLS if call in guard_text[start:end]] != list(GUARD_CALLS) or (
            guard_text[start:end].count("t.Skipf(") + guard_text[start:end].count("t.Fatalf(") != len(GUARD_CALLS)):
        raise ValueError("the pinned option guard differs from its reviewed rules")
    names = skipped_test_names(runner_source.decode())
    rows = []
    for row in document["rows"]:
        name = row["path"].rsplit("/", 1)[-1]
        fired = [rule for _, rule, predicate in GUARD_RULES if predicate(row["options"])]
        if row["tier"] == "executed":
            if fired or name in names or row["informational_reason"] is not None:
                raise ValueError("an executed row matches a native skip rule: " + row["id"])
            continue
        reason = row["informational_reason"]
        entry = {"id": row["id"], "path": row["path"], "reason": reason, "boundary": row["boundary"]}
        if reason == "option_guard_skip":
            if row["boundary"] != "options_rejected" and not fired:
                raise ValueError("an option-guard row fires no pinned guard rule: " + row["id"])
            entry["guard"] = fired
        elif reason == "filename_skip":
            if name not in names:
                raise ValueError("a filename-skip row is not in the pinned skippedTests: " + row["id"])
        elif reason != "not_enumerated" or RUNNER_TEST_FILE.search(row["path"]):
            raise ValueError("an informational row has no native reason: " + row["id"])
        rows.append(entry)
    return {
        "version": 1,
        "pin": document["pin"],
        "scope": ("Phase 2 C7.0 skip list: every informational variant of data/phase2/inventory.json with its "
                  "native reason; the option-guard rows name the pinned guard rules their options fire, and the "
                  "rows the option parser rejects before the guard carry the options_rejected boundary."),
        "inventory_sha256": digest(render(document)),
        "sources": {GUARD_SOURCE: digest(guard_source), RUNNER_SOURCE: digest(runner_source)},
        "reasons": REASONS,
        "guard_rules": [{"option": option, "rule": rule} for option, rule, _ in GUARD_RULES],
        "filename_rule": {"source": RUNNER_SOURCE, "skipped_tests": names},
        "counts": {"informational": dict(sorted(Counter(entry["reason"] for entry in rows).items())),
                   "options_rejected": sum(entry["boundary"] == "options_rejected" for entry in rows),
                   "executed": sum(row["tier"] == "executed" for row in document["rows"])},
        "rows": rows,
    }


def current(root=ROOT):
    """The committed skip list equals the one the inventory generates, and every
    checkpoint's claims were made over this inventory (no executed row changed
    classification since C0)."""
    inventory = INVENTORY.read_bytes()
    if render(build()) != inventory:
        raise ValueError("the inventory is stale; rerun freeze and review")
    if INFORMATIONAL.read_bytes() != render(informational(json.loads(inventory), root)):
        raise ValueError("the skip list differs from the inventory; rerun informational")
    for path in sorted((root / "data/phase2").glob("c*-claims.json")):
        claimed = json.loads(path.read_bytes()).get("inventory_sha256")
        if claimed is not None and claimed != digest(inventory):
            raise ValueError(f"{path.name} was made over another inventory")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("write", "check"))
    args = parser.parse_args()
    if args.command == "write":
        INFORMATIONAL.write_bytes(render(informational()))
    else:
        current()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print("phase2 informational failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
