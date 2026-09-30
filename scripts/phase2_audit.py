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
# C3.1 scope at the recorded C2 exit (bb29600): the complete flow.go and jsdoc.go
# inventories plus the checker.go, grammarchecks.go and utilities.go functions of
# the C3 areas that no C1 or C2 group or C2 ownership entry lists (JSX and
# decorators are C4's; the exported API, services, node builder and emit
# resolver are C5's). Generated from status/unmapped-functions.json.
C3_REVIEWED_GROUPS = {
    "C3.2 narrowing and flow (flow.go)": (130, "8a714694cb960bea3d8af77ccd1cd1fea3c237a6ee7325a69b89f04536204818"),
    "C3.3 iteration, async and generators": (11, "1c05df5e1c15b1f402a3fa86174e34f9af6813be6d2d1e4000f3d059afc6b3e5"),
    "C3.4 classes, this and members": (51, "8755a05d6d75c1fdf09b017b98401e86fbb201b3c3c0e9c04372b1b82c8c534d"),
    "C3.5 namespaces, modules and aliases": (33, "2aae81b58f592a4ac63405e42725a6a0d3d74a15668b1e99c06d17affd477f65"),
    "C3.6 JavaScript and JSDoc (jsdoc.go)": (4, "d0ca0e916e5e8adb952ac7323f60045d688b6ccf9822723d3f64f9f2aecfccd9"),
    "C3.7 statements, expressions and grammar (grammarchecks.go share)": (136, "9a852a12aa9866f64b9b3557d263d1209184da0d4d4a27f406f3c88802a8b422"),
    "C3.8 utilities (utilities.go share)": (143, "56cda335a5295c55602fbc240e65793810f50c688d5dc299cd223f0c04bcb3a9"),
}
C3_COMPLETE_FILES = {"C3.2 narrowing and flow (flow.go)": "tsc/internal/checker/flow.go:"}
C4_REVIEWED_GROUPS = {
    "C4.2-C4.5 JSX (jsx.go)": (59, "178da8f43e12d0920e98a9e8a2359f8f6dbc47e35bf2834bdb963088022f0ff2"),
    "C4 JSX seams (checker, grammar, relater, utilities; C5 wrappers)": (14, "c9202b9c9fc202e1d3514c6d2cc1768a02b049954187e1078f8c8c19d3adecea"),
    "C4.6-C4.7 decorators and metadata": (47, "8c3d1ce5c8ae1acd5483a5603b8b83e35afa14d6d6c5d683998789e86a0428d7"),
}
C4_COMPLETE_FILES = {"C4.2-C4.5 JSX (jsx.go)": "tsc/internal/checker/jsx.go:"}
C5_REVIEWED_GROUPS = {
    "C5.6 emit resolver (emitresolver.go)": (64, "c226fbc63e72ec1e1706dec39f6bab38ed08ef731aa6ce0aa7aa9900df7fed10"),
    "C5.3 name accessibility (symbolaccessibility.go)": (37, "72c01eba7dbe5e3e826b87267074af10f2024889f734268267757251c7cb1aaa"),
    "C5.4 node builder entry points (nodebuilder.go)": (29, "714a1af6f3af8e1080334d7fb011c8cfb0816d739a88cb93dfb001cc082eccb1"),
    "C5.4 node builder (nodebuilderimpl.go)": (113, "ef912a047b70f50e51acaf9265e2b4fa0ca3e265ad59942cd81352668cc4995c"),
    "C5.4 node builder scopes (nodebuilderscopes.go)": (4, "7da83b8ff5fae84071b44af3f9d19956dc7e0a8c1a6c6ab0706315417314fe8a"),
    "C5.4 node reuse and recovery (nodecopy.go)": (28, "2e7de8b635ac6858f9c0ea7b665c093ee071be1e3354192d5f857dcf348203f4"),
    "C5.4 symbol tracker (symboltracker.go)": (14, "77fe669302de167beb41780e1546f1c261ad556737f503e295d364dc5969fa04"),
    "C5.4 printer entry points (printer.go)": (27, "30bb882347848752bc77b1bcd18cc46e2c8139d79adfda952e76bf1c10fa5d52"),
    "C5.4 pseudo type nodes (pseudotypenodebuilder.go)": (9, "02303962badf1a5679fa5c98ef672e786aff3ebb1f37b33c18f71351c7df01f5"),
    "C5.5 hover expansion (nodebuilder_hover.go)": (18, "db2cbed7c51c21e5f8795ac56667834d68fc2db606441570a507b225239cc98d"),
    "C5.5 services (services.go)": (66, "c97624de2644a05b1ad31464aba0886a3d7e75682717660e22b9749c2384dd94"),
    "C5.2 public query surface (exports.go)": (89, "2049428b080279123da441506fe69f60ea1e1691db8059c5313b210d85ee94ab"),
    "C5 checker entries (checker.go API surface)": (11, "39ebad5e044902b5660f0bf7d3484f681b12d7f5c9106521f6e77cba8b30950e"),
}
# Every file group of the C5 scope covers its complete pinned file.
C5_COMPLETE_FILES = {group: "tsc/internal/checker/" + group.rsplit("(", 1)[1].rstrip(")") + ":"
                     for group in C5_REVIEWED_GROUPS if group.endswith(".go)")}
# C6 (docs/PHASE2-C6-plan.md, C6.1): the compiler checker pool, the tracer and
# the work groups as complete files; the program's checker driving, the
# cancellation functions and polling sites, the relater's tracer call and the
# checker constructor as reviewed selections.
C6_REVIEWED_GROUPS = {
    "C6.3 compiler checker pool (checkerpool.go)": (18, "e9a149f26d0847bea6710dcc6fe62eadc407ffaecc0614832523240623ce8e06"),
    "C6.2 tracer (tracer.go)": (38, "78fbe68a50f8e6652efb02b7da9fc43ca90d6c793018c1e4475f337743e70b7c"),
    "C6.4 work groups (workgroup.go)": (9, "92ad6e89f8a05b9914b85e10ec7e709c27b4ccd04c4a3ca058c41121bad7733f"),
    "C6.4 program driving (program.go)": (15, "9ebb13722d026a72b123c2bd2e6be23525842afb0d0ca9c77e3f40137333a171"),
    "C6.5 cancellation (checker.go, utilities.go, exports.go)": (7, "e3ccd4d5902edafa6a8dac4b9585beb4d00592869ad979c3135cfc7b4b6a4b89"),
    "C6.2 tracer call sites (relater.go)": (1, "95911734da0cd7e3cef044aff0631a9e10e4824d4c22188faea6b3f6d6308424"),
    "C6.3 checker constructor (checker.go)": (1, "a4f66d6b8a3775c48bc2fef0974040c7bc79bb37b8595af47f0d3041c33bc410"),
}
C6_COMPLETE_FILES = {"C6.3 compiler checker pool (checkerpool.go)": "tsc/internal/compiler/checkerpool.go:",
                     "C6.2 tracer (tracer.go)": "tsc/internal/checker/tracer.go:",
                     "C6.4 work groups (workgroup.go)": "tsc/internal/core/workgroup.go:"}
# C7 (docs/PHASE2-C7-plan.md, C7.8.0): the files content-mapper execution moves
# to Phase 2, each group covering its complete pinned files, and the program
# integration: every pinned ast, compiler and declaration-transformer function
# whose body handles content-mapped files (content mappers, span maps,
# supplemental files, mapped diagnostics or runExternalCode), marked or not.
C7_REVIEWED_GROUPS = {
    "C7.8.1 JSON-RPC base protocol (jsonrpc)": (20, "6178f6d89b77b279ce5abb24830c2db8d2bec2686154e771c0bcdd4ed0ab4bec"),
    "C7.8.1 IPC connection and protocol (ipc)": (27, "cb65666ca991ba68c5e6162ae1c4bae1a99381b6189b7516326130f227663e16"),
    "C7.8.2 content-mapper host (contentmapper)": (78, "a74f676529f82bc64b2ae3a57fafc625921a090ccf0546a4f04df6439de34901"),
    "C7.8.2 span maps (spanmap.go)": (34, "0d0d75c9f683e9f6a622b43c03e93b3d0bef1d1843b76266a17dfb33ecb23df5"),
    "C7.8.3 program integration (ast, compiler, declarations)": (56, "b89b410581028a4da1101c8c7caf824417804d8af25ded3279c82656895ce703"),
    "C7.8.4 test mappers (contentmappertest)": (22, "fba44f83109f320749816bd7a557b5d3580344661b25dacd4f866e16ce1424ac"),
    # C7.4 (b): the Phase 2 functions no port marker names and no earlier
    # audit disposed of, reviewed against their Rust sites.
    "C7.4 Phase 2 functions outside the checkpoint audits": (
        108, "c51edf93804eb1fb4763ce8e8ba37fca3a357b0db6dff40372f99b8e1d4921dc"),
    "C7.4 Phase 2 handoffs to C5 that C5 left unrecorded": (
        2, "510a39dad941f08f4954093a6f715797e8e3370bcf8f60c666c00276878dffe9"),
}
C7_COMPLETE_FILES = {
    "C7.8.1 JSON-RPC base protocol (jsonrpc)": ("tsc/internal/jsonrpc/baseproto.go:", "tsc/internal/jsonrpc/jsonrpc.go:"),
    "C7.8.1 IPC connection and protocol (ipc)": tuple(f"tsc/internal/ipc/{name}.go:" for name in (
        "conn", "conn_async", "protocol", "protocol_jsonrpc", "timing")),
    "C7.8.2 content-mapper host (contentmapper)": tuple(f"tsc/internal/contentmapper/{name}.go:" for name in (
        "contentmapper", "host", "hostimpl", "transform")),
    "C7.8.2 span maps (spanmap.go)": "tsc/internal/spanmap/spanmap.go:",
    "C7.8.4 test mappers (contentmappertest)": tuple(f"tsc/internal/testutil/contentmappertest/{name}.go:" for name in (
        "failing", "lisp", "protocol", "registry", "spawner", "supplemental", "supplemental_diagnostics",
        "supplemental_globals", "supplemental_module", "transforming")),
}
SCOPES = {"C1": (REVIEWED_GROUPS, COMPLETE_FILES, REQUIRED_HANDOFFS),
          "C2": (C2_REVIEWED_GROUPS, C2_COMPLETE_FILES, {}),
          "C3": (C3_REVIEWED_GROUPS, C3_COMPLETE_FILES, {}),
          "C4": (C4_REVIEWED_GROUPS, C4_COMPLETE_FILES, {}),
          "C5": (C5_REVIEWED_GROUPS, C5_COMPLETE_FILES, {}),
          "C6": (C6_REVIEWED_GROUPS, C6_COMPLETE_FILES, {}),
          "C7": (C7_REVIEWED_GROUPS, C7_COMPLETE_FILES, {})}


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
    if checkpoint not in SCOPES:
        return ["unsupported audit checkpoint"]
    scope_groups, complete_files, scope_handoffs = SCOPES[checkpoint]
    if reviewed is None:
        reviewed = scope_groups
    if required_handoffs is None:
        required_handoffs = scope_handoffs
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
        prefix = complete_files.get(group)  # one file's prefix, or a tuple of them
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
