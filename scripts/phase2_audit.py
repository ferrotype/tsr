#!/usr/bin/env python3
"""Phase 2 C1.1: the foundation function audit (data/phase2/c1-audit.json).

Every pinned Go function in a C1 group carries exactly one disposition with
evidence:

* mapped -- a `port:` marker names it in crates/ (checked by scanning, the
  same rule as the ledger, never typed by hand);
* equivalent -- a Rust field, accessor or shared helper implements it; the
  Rust site (path:line) must exist;
* missing_mapping -- implemented, marker still to add; not allowed at exit;
* gap -- not implemented; names the C1 item that closes it; not allowed at
  exit;
* later -- handed to a named checkpoint (C2..C7) or phase, with the reason.

The groups are explicit function-id lists, so the denominator is reviewable.
`check` verifies the document against the pinned inventory and the sources;
`worksheet` prints the functions of a group that still lack a disposition.

    check [--groups c1] [--allow-open]
    worksheet [--groups c1]
"""
from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import ROOT  # noqa: E402

AUDIT = ROOT / "data/phase2/c1-audit.json"
INVENTORY = ROOT / "data/go-functions.tsv"
DISPOSITIONS = ("mapped", "equivalent", "missing_mapping", "gap", "later")
OPEN = ("missing_mapping", "gap")
CHECKPOINTS = ("C2", "C3", "C4", "C5", "C6", "C7", "Phase 3", "Phase 4", "Phase 5")
MARKER = re.compile(r"^\s*(?://[/!]?)\s*port:\s*(\S+)")


def inventory():
    """Pinned function ids, from the generated inventory (`file:Receiver.name`)."""
    with INVENTORY.open(newline="") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    header = rows[1]
    index = header.index("id")
    return {row[index] for row in rows[2:] if len(row) > index}


def markers(root=ROOT):
    """Every `port:` marker under crates/, as the ledger scans them."""
    found = set()
    for path in sorted((root / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        for line in path.read_text(errors="replace").splitlines():
            match = MARKER.match(line)
            if match and ":" in match.group(1):
                found.add(match.group(1))
    return found


def load(path=AUDIT):
    return strict_json_loads(Path(path).read_bytes())


def problems(document, *, allow_open=False, root=ROOT, known=None, mapped=None):
    """Every defect of the audit document; an empty list means it checks."""
    known = inventory() if known is None else known
    mapped = markers(root) if mapped is None else mapped
    found = []
    groups = document.get("groups") or {}
    dispositions = document.get("dispositions") or {}
    if document.get("version") != 1:
        found.append("unsupported audit version")
    seen = set()
    for group, members in groups.items():
        if not isinstance(members, list) or not members:
            found.append(f"group {group} is empty")
            continue
        for identity in members:
            if identity not in known:
                found.append(f"{group}: {identity} is not in the pinned inventory")
            if identity in seen:
                found.append(f"{identity} is listed in two groups")
            seen.add(identity)
    for identity, entry in dispositions.items():
        if identity not in seen:
            found.append(f"disposition for {identity}, which no group lists")
            continue
        kind = entry.get("disposition")
        if kind not in DISPOSITIONS:
            found.append(f"{identity}: unknown disposition {kind!r}")
            continue
        if kind == "mapped" and identity not in mapped:
            found.append(f"{identity}: recorded as mapped but no port marker names it")
        if kind != "mapped" and identity in mapped:
            found.append(f"{identity}: a port marker names it, so its disposition must be mapped")
        if kind == "equivalent":
            site = entry.get("rust")
            if not site or ":" not in str(site):
                found.append(f"{identity}: equivalent needs a Rust site path:line")
            else:
                path, _, line = str(site).rpartition(":")
                target = root / path
                if not target.is_file() or not line.isdigit() or int(line) > len(target.read_bytes().split(b"\n")):
                    found.append(f"{identity}: Rust site {site} does not exist")
        if kind == "gap" and not entry.get("item"):
            found.append(f"{identity}: gap needs the C1 item that closes it")
        if kind == "later" and entry.get("owner") not in CHECKPOINTS:
            found.append(f"{identity}: later needs an owner among {', '.join(CHECKPOINTS)}")
        if kind in ("equivalent", "gap", "later") and not entry.get("reason"):
            found.append(f"{identity}: {kind} needs a reason")
        if kind in OPEN and not allow_open:
            found.append(f"{identity}: {kind} is open")
    for identity in sorted(seen):
        if identity not in dispositions:
            if identity in mapped:
                continue  # a marker is the disposition
            found.append(f"{identity}: no disposition and no port marker")
    return found


def summary(document, *, root=ROOT, mapped=None):
    mapped = markers(root) if mapped is None else mapped
    dispositions = document.get("dispositions") or {}
    counts = {}
    for group, members in (document.get("groups") or {}).items():
        tally = {kind: 0 for kind in DISPOSITIONS} | {"undisposed": 0}
        for identity in members:
            entry = dispositions.get(identity)
            if entry:
                tally[entry["disposition"]] += 1
            elif identity in mapped:
                tally["mapped"] += 1
            else:
                tally["undisposed"] += 1
        counts[group] = tally
    return counts


def complete(document, *, root=ROOT):
    """True when every grouped function is disposed and none is open."""
    return not problems(document, allow_open=False, root=root)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("check", "worksheet"))
    parser.add_argument("--audit", type=Path, default=AUDIT)
    parser.add_argument("--allow-open", action="store_true", help="accept missing_mapping and gap dispositions")
    args = parser.parse_args()
    document = load(args.audit)
    if args.command == "worksheet":
        mapped = markers()
        for group, members in document.get("groups", {}).items():
            for identity in members:
                if identity not in mapped and identity not in (document.get("dispositions") or {}):
                    print(f"{group}\t{identity}")
        return
    found = problems(document, allow_open=args.allow_open)
    print(json.dumps({"complete": not found, "problems": found[:50], "problem_count": len(found),
                      "summary": summary(document)}, indent=1, sort_keys=True))
    if found:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 audit failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
