#!/usr/bin/env python3
"""Phase 2 checkpoint function audits (data/phase2/c1-audit.json or c2-audit.json).

Every pinned Go function in a reviewed checkpoint group carries a disposition with
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

    check [--audit FILE] [--allow-open]
    worksheet [--audit FILE]
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
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_inventory  # noqa: E402

AUDIT = ROOT / "data/phase2/c1-audit.json"
INVENTORY = ROOT / "data/go-functions.tsv"
DISPOSITIONS = ("mapped", "equivalent", "missing_mapping", "gap", "later")
OPEN = ("missing_mapping", "gap")
CHECKPOINTS = ("C2", "C3", "C4", "C5", "C6", "C7", "Phase 3", "Phase 4", "Phase 5")


def later_owners(checkpoint):
    """Completed checkpoints do not consume incoming later-work dispositions."""
    ordered = ("C1", *CHECKPOINTS)
    return ordered[ordered.index(checkpoint) + 1:]


MARKER = re.compile(r"^\s*(?://[/!]?)\s*port:\s*(\S+)")
# Reviewed C1.1 denominator, independent of the editable dispositions. Changes
# to group membership require reviewing these bindings as well as the JSON.
# Relater and type-record groups additionally cover their complete Go files.
REVIEWED_PIN = "1f70213d4922b434345f639b441681e470c7cfc1"
REVIEWED_GROUPS = {
    "C1.2 symbols, merges, declarations": (29, "216e40ce7f81a058fda88011deb57b5b31957f756cb46fc8dc922caf9279f1b5"),
    "C1.3 members, signatures, index infos": (39, "e0fe197fe4960ac2adad5e7eaf60d0510e0e8d0a3ddede10caebe9f9e690008c"),
    "C1.4 arrays, tuples, enums, literals": (27, "480d3a3f22db8032b448f131075b59a371f496ce59079c57f6f2ef8f88791f31"),
    "C1.5 recursion and limits": (12, "c9bd1d3d56ed03db13717e5b7b225377f89f6c57c9cee2660fff10bdc1299d78"),
    "C1.6 relations (relater.go)": (189, "20ea2ac9992debc5068d5a26a616180e9fe18905d4e9e43e73bf1f104e6169f8"),
    "types.go records and accessors": (111, "e56a92b944ad201dec07e1e5be8744862547c0ea1f374af09e8a856fb0ddcc49"),
}
COMPLETE_FILES = {"C1.6 relations (relater.go)": "tsc/internal/checker/relater.go:",
                  "types.go records and accessors": "tsc/internal/checker/types.go:"}
REQUIRED_HANDOFFS = {"c2-variance-measurement": "C2"}
# C2.1's explicit checker list (parentheticals expanded), all inference.go and
# mapper.go functions. Contextual entries are the eleven unmarked at c583254,
# not a moving selection of whatever today's marker scan happens to omit.
C2_REVIEWED_GROUPS = {
    "C2.3 instantiation and keys": (16, "208ce84102086efab6c857eb59ffece773fa896edbc276709d6ba52d7207c3e8"),
    "C2.3 mappers (mapper.go)": (36, "83631d6254c9951e6c4937a27a09c28a04eb431f26cd3bd3709c579ba4abd4d0"),
    "C2.3 type parameters, constraints and defaults": (24, "e5bdc813c66b2b18718893d65e9f6cc0c5092be4c501d38f647706f85e18a8e1"),
    "C2.4 calls and overloads": (13, "713f9bf0bb1f18bd33ab43f22701444ece3795219221d127a48daa4fd19ff981"),
    "C2.4 inference (inference.go)": (77, "ef6d5c3e599c5aa93677bc6fd12cc2ad546cc5be6b942a734d97fb290ba97a82"),
    "C2.5 conditional and infer": (8, "d3ff4172523257d398c0ba0f67c27fa2bd028ba784bb04d8fecf5e32bd4051eb"),
    "C2.6 mapped, indexed access and keyof": (18, "2d56841c70085a7d16ffe80b6cb698eda68b121550052231e88ec6abe79d0d0a"),
    "C2.7 template literals and string mapping": (6, "c79b2b5119c85e5b42dd424f19a4ddcd5d553f81ff81abd0fd651fbe303af70c"),
    "C2.8 import types and instantiation expressions": (9, "80232aee2699467448966b72d02ebf779c4f2e3c7e99c2c2a26fb4666cd90a74"),
    "C2.9 contextual typing": (24, "35a119fd3118e400d80eecdb79a8ef2417fcc6c2b84fe3115b5ea8911681b43a"),
}
C2_COMPLETE_FILES = {"C2.4 inference (inference.go)": "tsc/internal/checker/inference.go:",
                     "C2.3 mappers (mapper.go)": "tsc/internal/checker/mapper.go:"}
# The C2.1 checker list names previously unmarked helpers, so ownership also
# binds already marked core operators from the modules named in C2.2--C2.9.
# This guard cannot make any audit disposition complete.
C2_OWNERSHIP = (337, "d689efb64d8edefc7b8e03f8769feab2628692f9dd094165979e7321e07e57b8")


def inventory():
    """Pinned function ids, from the generated inventory (`file:Receiver.name`)."""
    with INVENTORY.open(newline="") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    if rows[0] != ["# upstream " + REVIEWED_PIN]:
        raise ValueError("function inventory pin differs from the reviewed checkpoint scopes")
    if strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"] != REVIEWED_PIN:
        raise ValueError("upstream pin differs from the reviewed checkpoint scopes")
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


def problems(document, *, allow_open=False, root=ROOT, known=None, mapped=None,
             reviewed=None, pin=REVIEWED_PIN, required_handoffs=None, executed_ids=None):
    """Every defect of the audit document; an empty list means it checks."""
    if not isinstance(document, dict):
        return ["audit must be an object"]
    checkpoint = document.get("checkpoint", "C1")
    if checkpoint not in ("C1", "C2"):
        return ["unsupported audit checkpoint"]
    if reviewed is None:
        reviewed = REVIEWED_GROUPS if checkpoint == "C1" else C2_REVIEWED_GROUPS
    if required_handoffs is None:
        required_handoffs = REQUIRED_HANDOFFS if checkpoint == "C1" else {}
    complete_files = COMPLETE_FILES if checkpoint == "C1" else C2_COMPLETE_FILES
    allowed_owners = later_owners(checkpoint)
    known = inventory() if known is None else known
    mapped = markers(root) if mapped is None else mapped
    found = []
    groups = document.get("groups") or {}
    dispositions = document.get("dispositions") or {}
    if not isinstance(groups, dict) or not isinstance(dispositions, dict):
        return ["audit groups and dispositions must be objects"]
    if document.get("version") != 1:
        found.append("unsupported audit version")
    if document.get("pin") != pin:
        found.append(f"audit pin differs from the reviewed {checkpoint} scope")
    if not groups or set(groups) != set(reviewed):
        found.append(f"audit groups differ from the complete reviewed {checkpoint} scope")
    if checkpoint == "C2":
        ownership = document.get("ownership", {})
        if not isinstance(ownership, dict):
            ownership = {}
        functions, modules = ownership.get("functions"), ownership.get("modules")
        if (not isinstance(functions, list) or not all(isinstance(i, str) for i in functions)
                or not isinstance(modules, list) or not all(isinstance(i, str) for i in modules)
                or len(set(functions)) != len(functions) or len(set(modules)) != len(modules)
                or set(functions) - known
                or (len(functions), digest(canonical({"modules": modules, "functions": functions}))) != C2_OWNERSHIP):
            found.append("C2 ownership differs from its reviewed modules and pinned functions")
    seen = set()
    for group, members in groups.items():
        if not isinstance(members, list) or not members or not all(isinstance(m, str) for m in members):
            found.append(f"group {group} is empty or has invalid function identities")
            continue
        binding = (len(members), digest(canonical(sorted(members))))
        if group not in reviewed or binding != reviewed[group]:
            found.append(f"group {group} differs from its reviewed function inventory")
        prefix = complete_files.get(group)
        if prefix and set(members) != {identity for identity in known if identity.startswith(prefix)}:
            found.append(f"group {group} does not cover its complete pinned file")
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
        if not isinstance(entry, dict):
            found.append(f"{identity}: disposition must be an object")
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
                if (not target.resolve().is_relative_to((root / "crates").resolve()) or target.suffix != ".rs"
                        or not target.is_file() or not line.isdigit() or int(line) < 1
                        or int(line) > len(target.read_bytes().split(b"\n"))):
                    found.append(f"{identity}: Rust site {site} does not exist")
        if kind == "gap" and not entry.get("item"):
            found.append(f"{identity}: gap needs the {checkpoint} item that closes it")
        if kind == "later" and entry.get("owner") not in allowed_owners:
            found.append(f"{identity}: later needs an owner among {', '.join(allowed_owners)}")
        if kind in ("equivalent", "gap", "later") and (not isinstance(entry.get("reason"), str) or not entry["reason"].strip()):
            found.append(f"{identity}: {kind} needs a reason")
        if kind in OPEN and not allow_open:
            found.append(f"{identity}: {kind} is open")
    for identity in sorted(seen):
        if identity not in dispositions:
            if identity in mapped:
                continue  # a marker is the disposition
            found.append(f"{identity}: no disposition and no port marker")
    # Mapping and semantic review are separate: an existing marker cannot
    # certify a function whose source review identified an unresolved branch.
    issues = document.get("open_issues", [])
    if not isinstance(issues, list):
        found.append("open_issues must be a list")
        issues = []
    issue_ids = set()
    for issue in issues:
        if (not isinstance(issue, dict) or not isinstance(issue.get("id"), str) or not issue["id"]
                or issue["id"] in issue_ids or issue.get("go") not in seen
                or not isinstance(issue.get("item"), str) or not issue["item"].startswith(checkpoint + ".")
                or not isinstance(issue.get("reason"), str) or not issue["reason"].strip()):
            found.append("open issue needs a unique id, grouped function, checkpoint item and reason")
            continue
        issue_ids.add(issue["id"])
        if not allow_open:
            found.append(f"{issue['go']}: review issue {issue['id']} is open")
    # A mapped algorithm can still hand its outstanding measurement contract
    # to C2. Keep that obligation separate from function port dispositions.
    handoffs = document.get("handoffs", [])
    if not isinstance(handoffs, list):
        found.append("handoffs must be a list")
        handoffs = []
    handoff_ids = set()
    executed_ids = ({row["id"] for row in phase2_inventory.executed()}
                    if executed_ids is None else set(executed_ids))
    for entry in handoffs:
        if not isinstance(entry, dict):
            found.append("handoff must be an object")
            continue
        identity = entry.get("id")
        if not isinstance(identity, str) or not identity or identity in handoff_ids:
            found.append("handoff needs a nonempty unique id")
            continue
        handoff_ids.add(identity)
        if entry.get("owner") not in allowed_owners or (identity in required_handoffs
                                                     and entry.get("owner") != required_handoffs[identity]):
            found.append(f"handoff {identity}: invalid checkpoint owner")
        if not isinstance(entry.get("reason"), str) or not entry["reason"].strip():
            found.append(f"handoff {identity}: needs a reason")
        for field, allowed in (("functions", known), ("cases", executed_ids)):
            values = entry.get(field)
            if (not isinstance(values, list) or not values or any(not isinstance(value, str) for value in values)
                    or len(values) != len(set(values)) or set(values) - allowed):
                found.append(f"handoff {identity}: {field} need a complete unique known inventory")
    for identity in sorted(set(required_handoffs) - handoff_ids):
        found.append(f"missing required handoff {identity}")
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


def complete(document, *, root=ROOT, **kwargs):
    """True when every grouped function is disposed and none is open."""
    return not problems(document, allow_open=False, root=root, **kwargs)


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
                entry = (document.get("dispositions") or {}).get(identity, {})
                if identity not in mapped and (not entry or entry.get("disposition") in OPEN):
                    print(f"{group}\t{identity}\t{entry.get('disposition', 'undisposed')}")
        return
    found = problems(document, allow_open=args.allow_open)
    print(json.dumps({"complete": not found and complete(document), "valid": not found,
                      "problems": found[:50], "problem_count": len(found),
                      "summary": summary(document)}, indent=1, sort_keys=True))
    if found:
        raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 audit failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
