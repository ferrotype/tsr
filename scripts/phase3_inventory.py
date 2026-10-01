#!/usr/bin/env python3
"""Phase 3 T0: the frozen emit acceptance inventory.

One row per executed variant of the Phase 2 denominator (13,432), with what the
pinned compiler runner owes for it in the emit sub-tests, joined from committed
inputs and never from a native or Rust run:

* data/phase2/inventory.json -- the executed rows: identity, effective options,
  reference baselines by kind. Its rows are bound by their own digest, so an
  input-digest re-freeze of that file leaves this inventory current;
* data/s07/subset.json -- the request fields (settings, digests, configured
  name) and each test's units, from which `has_non_dts_files` follows as the
  runner computes it;
* the pinned runner's `skippedEmitTests` table, read from the source at the pin;
* the transpile test files and reference baselines at the pin.

Per row, `output` says whether the runner's `output` sub-test runs (`runs`, or
`disabled` with the pin's reason) and what it owes: a `.js` reference, or
`<no content>` graded as such, classed `noEmit`, `declaration_only` or `other`.
`sourcemap` and `sourcemap_record` say whether a `.js.map` or `.sourcemap.txt`
reference exists; a Rust emit that produces one where none exists is a
difference. `emit_options` are the options that select transforms.

The transpile section lists the runner's test files and the reference
baselines; the configurations behind them are the native capture's
(`scripts/phase3_native.py transpile`), reviewed to cover exactly these.

    python3 scripts/phase3_inventory.py freeze   # writes data/phase3/inventory.json
    python3 scripts/phase3_inventory.py check    # rebuilds and requires byte identity
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "data/phase3/inventory.json"
PHASE2 = "data/phase2/inventory.json"
SUBSET = "data/s07/subset.json"
RUNNER = "upstream/tsc/internal/testrunner/compiler_runner.go"
TRANSPILE_CASES = "upstream/tsc/testdata/tests/cases/transpile"
TRANSPILE_REFERENCES = "upstream/tsc/testdata/baselines/reference/transpile"
PRODUCER = "scripts/phase3_inventory.py"
EMIT_KINDS = (".js", ".js.map", ".sourcemap.txt")
EMIT_OPTIONS = ("target", "module", "jsx", "declaration", "emitDeclarationOnly", "isolatedDeclarations", "sourceMap",
                "inlineSourceMap", "declarationMap", "noEmit", "noEmitOnError", "importHelpers", "noEmitHelpers",
                "removeComments", "newLine", "emitBOM", "stripInternal", "outDir", "declarationDir", "sourceRoot",
                "mapRoot", "inlineSources")
DECLARATION_ONLY = "no input file other than declaration files"


def digest(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def git_blob(raw: bytes) -> str:
    return hashlib.sha1(b"blob %d\0" % len(raw) + raw).hexdigest()  # noqa: S324 - git's object name


def encode(value) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def skipped_emit_tests(source: str) -> dict:
    """`skippedEmitTests` of the pinned compiler runner: basename -> reason."""
    start = source.index("var skippedEmitTests = map[string]string{\n")
    body = source[start:source.index("\n}\n", start)].splitlines()[1:]
    table = {}
    for line in body:
        match = re.fullmatch(r'\t"([^"\\]+)":\s+"([^"\\]+)",', line)
        if not match:
            raise ValueError("unrecognised skippedEmitTests entry: " + line)
        table[match[1]] = match[2]
    if not table:
        raise ValueError("skippedEmitTests is empty at the pin")
    return table


def build():
    raw = {name: (ROOT / name).read_bytes() for name in (PHASE2, SUBSET, RUNNER)}
    phase2 = json.loads(raw[PHASE2])
    executed = [row for row in phase2["rows"] if row["tier"] == "executed"]
    subset = json.loads(raw[SUBSET])
    index = {variant["id"]: (case, variant) for case in subset["cases"] for variant in case["variants"]}
    skipped = skipped_emit_tests(raw[RUNNER].decode())
    rows = []
    for row in executed:
        case, variant = index[row["id"]]
        source = case["source"]
        name = Path(source["path"]).name
        extension = Path(name).suffix
        base = name[:-len(extension)]
        configured = variant["configured_name"]
        if not configured.startswith(base) or not configured.endswith(extension):
            raise ValueError("invalid configured filename: " + row["id"])
        label = configured[len(base):-len(extension)]
        if label and (not label.startswith("(") or not label.endswith(")")):
            raise ValueError("invalid configuration label: " + row["id"])
        has_non_dts = any(not unit["name"].endswith(".d.ts") for unit in source["units"])
        references = {kind: row["references"][kind] for kind in EMIT_KINDS if kind in row["references"]}
        if not has_non_dts:
            output = {"state": "disabled", "reason": DECLARATION_ONLY}
        elif name in skipped:
            output = {"state": "disabled", "reason": skipped[name]}
        elif ".js" in references:
            output = {"state": "runs", "owes": "reference"}
        else:
            output = {"state": "runs", "owes": "no_content",
                      "class": "noEmit" if row["options"].get("noEmit") is True else "other"}
        if output["state"] == "disabled" and ".js" in references:
            raise ValueError("a disabled output sub-test has a .js reference: " + row["id"])
        rows.append({
            "id": row["id"], "path": source["path"], "suite": row["suite"], "configured_name": configured,
            "configuration_name": label[1:-1] if label else "", "settings": source["configurations"][variant["configuration"]],
            "raw_sha256": source["raw_sha256"], "loaded_sha256": source["loaded_sha256"],
            "has_non_dts_files": has_non_dts, "output": output,
            "sourcemap": "reference" if ".js.map" in references else "absent",
            "sourcemap_record": "reference" if ".sourcemap.txt" in references else "absent",
            "references": references,
            "emit_options": {key: row["options"][key] for key in EMIT_OPTIONS if key in row["options"]},
            "sample": row["sample"],
        })
    cases = sorted(path for path in (ROOT / TRANSPILE_CASES).iterdir() if path.is_file())
    baselines = sorted(path for path in (ROOT / TRANSPILE_REFERENCES).iterdir() if path.is_file())
    transpile = {
        "cases": [{"path": str(path.relative_to(ROOT / "upstream")), "sha256": digest(path.read_bytes())} for path in cases],
        "references": [{"name": "transpile/" + path.name, "git_blob": git_blob(path.read_bytes())} for path in baselines],
    }
    return {
        "version": 1, "producer": PRODUCER, "pin": phase2["pin"],
        "scope": ("Phase 3 T0 emit acceptance inventory: the executed variants of the Phase 2 denominator with the "
                  "emit sub-tests the pinned runner owes for each, and the transpile runner's test files and "
                  "reference baselines. No native or Rust execution is inferred from this document."),
        "inputs": {"phase2_executed_rows_sha256": digest(encode(executed).encode()), SUBSET: digest(raw[SUBSET]),
                   RUNNER: digest(raw[RUNNER])},
        "skipped_emit_tests": skipped,
        "counts": counts(rows, transpile),
        "transpile": transpile,
        "rows": rows,
    }


def counts(rows, transpile):
    output = Counter()
    for row in rows:
        item = row["output"]
        output["disabled" if item["state"] == "disabled" else item["owes"]] += 1
    return {
        "executed": len(rows),
        "references": {kind: sum(kind in row["references"] for row in rows) for kind in EMIT_KINDS},
        "output": dict(sorted(output.items())),
        "output_disabled": dict(sorted(Counter(row["output"]["reason"] for row in rows
                                               if row["output"]["state"] == "disabled").items())),
        "no_content_class": dict(sorted(Counter(row["output"]["class"] for row in rows
                                                if row["output"].get("owes") == "no_content").items())),
        "without_js_reference": sum(".js" not in row["references"] for row in rows),
        "sample": sum(row["sample"] for row in rows),
        "transpile_cases": len(transpile["cases"]), "transpile_references": len(transpile["references"]),
    }


def render(document) -> bytes:
    head = encode({key: value for key, value in document.items() if key != "rows"})
    body = ",\n".join(encode(row) for row in document["rows"])
    return (head[:-1] + ',"rows":[\n' + body + "\n]}\n").encode()


def read():
    """The committed inventory, verified against its current inputs."""
    document = json.loads(INVENTORY.read_bytes())
    executed = [row for row in json.loads((ROOT / PHASE2).read_bytes())["rows"] if row["tier"] == "executed"]
    current = {"phase2_executed_rows_sha256": digest(encode(executed).encode()),
               SUBSET: digest((ROOT / SUBSET).read_bytes()), RUNNER: digest((ROOT / RUNNER).read_bytes())}
    for name, expected in document["inputs"].items():
        if current.get(name) != expected:
            raise ValueError(f"inventory input changed since freeze: {name}; rerun freeze and review")
    return document


REQUEST_FIELDS = ("id", "path", "raw_sha256", "loaded_sha256", "settings", "configuration_name", "configured_name")


def requests(document=None):
    """The native and Rust request rows, in inventory order."""
    document = document or read()
    return [dict({key: row[key] for key in REQUEST_FIELDS}, acceptance_tier="executed") for row in document["rows"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("freeze", "check"))
    parser.add_argument("--output", type=Path, default=INVENTORY)
    args = parser.parse_args()
    rendered = render(build())
    if args.command == "freeze":
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(rendered)
    elif not args.output.exists() or args.output.read_bytes() != rendered:
        print("phase3 inventory is stale or was edited; rerun freeze and review the diff", file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps(json.loads(rendered)["counts"], sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 inventory failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
