#!/usr/bin/env python3
"""Phase 3 function audit (docs/PHASE3-plan.md section 5, "Function disposition").

The scope is every pinned function (`data/go-functions.tsv`) of the 73 Phase 3
upstream files plus the `Emit` family of `compiler/program.go`:

* the 71 files `PORTS.toml` lists with `phase = 3`;
* `compiler/emitter.go` and `compiler/emitHost.go`, which T0 moves to Phase 3
  (decision 1); the union holds before and after the ledger move;
* from `program.go`, which stays Phase 4's file, the plan's seven `Emit`-family
  operations (section 2: `Emit`, `HandleNoEmitOptions`, `CombineEmitResults`,
  `IsEmitBlocked`, `getSourceFilesToEmit`, `GetSourceFileFromReference`,
  `CommandLine`) and the three other `program.go` operations the owner-approved
  destination audit gave Phase 3 (`data/phase1/coverage-review.json`).

The scope is bound below by count and digest, as Phase 2's reviewed groups
were, so a change to the ledger or the pin is reviewed here too.

Each function gets one status, by `scripts/phase2_audit.py`'s rules:

* `mapped` -- exactly one `port:` marker under `crates/` names it (the ledger's
  scan, never typed by hand);
* `equivalent` -- folded into another function, or without a caller at the
  pin while the same fact is computed elsewhere; a reviewed entry names the
  Rust site by file and an anchor text that must occur exactly once there, and
  the build resolves it to `path:line`;
* `later` -- handed to a later phase the plan names, with the reason;
* `gap` -- a reviewed entry naming the checkpoint that closes it;
* `pending_c3` -- the transpile package, which unit C3 ports concurrently.

Problems (the audit is invalid): two or more markers for one function, a
marker whose id under a Phase 3 file or package is not in the inventory, an
unmarked function with no reviewed entry, a reviewed entry for a marked
function, an unresolvable anchor. `gap` and `pending_c3` are open: valid, but
the audit is complete only without them (T8's closure).

    build               # writes data/phase3/audit.json; exits 1 on problems
    check [--complete]  # the committed audit is current and valid (and complete)
    summary             # the table by checkpoint group
"""
from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_audit  # noqa: E402

AUDIT = ROOT / "data/phase3/audit.json"
MARKER = phase2_audit.MARKER
PIN = phase2_audit.REVIEWED_PIN
MOVED = ("tsc/internal/compiler/emitter.go", "tsc/internal/compiler/emitHost.go")
PROGRAM = "tsc/internal/compiler/program.go"
EMIT_FAMILY = tuple(f"{PROGRAM}:{name}" for name in (
    # The plan's seven (section 2, "Program-level emit").
    "Program.Emit", "HandleNoEmitOptions", "CombineEmitResults", "Program.IsEmitBlocked",
    "Program.getSourceFilesToEmit", "Program.GetSourceFileFromReference", "Program.CommandLine",
    # The destination audit's other Phase 3 program.go operations.
    "GetDiagnosticsOfAnyProgram", "Program.GetDeclarationDiagnostics",
    "Program.getDeclarationDiagnosticsForFile"))
# The reviewed denominator: (count, sha256 of the canonical sorted list).
REVIEWED_FILES = (73, "6e883461649ba605bf6798f2b8e3b20c0eb438424a7b4c911c7696121b5fdf93")
REVIEWED_FUNCTIONS = (1942, "83d6eedd09f8b84a89b0c76674e298e8baac6b6281611ab5c3fb20112dd830f0")
STATUSES = ("mapped", "equivalent", "later", "gap", "pending_c3", "duplicate")
OPEN = ("gap", "pending_c3")
LATER_OWNERS = ("Phase 4", "Phase 5", "Phase 6", "Phase 7")
CHECKPOINTS = tuple(f"T{number}" for number in range(1, 9))
PENDING = {"tsc/internal/transpile/": (
    "C3", "Unit C3 ports the transpile package concurrently; crate tsr_transpile is not on this branch.")}

GROUPS = {
    "T1": "printer",
    "T2": "source maps",
    "T3": "transform framework and TypeScript transforms",
    "T4": "module and inliner transforms",
    "T5": "ES downlevel transforms",
    "T6": "JSX transform",
    "T7": "declaration emit and the pseudochecker",
    "T8": "emitter, emit host, Emit family, output paths and transpile",
}
FRAMEWORK = {f"tsc/internal/transformers/{name}.go" for name in (
    "chain", "destructuring", "modifiervisitor", "transformer", "utilities")}


def group_of(go):
    """The checkpoint group of a Phase 3 file (plan section 4)."""
    for prefix, group in (("tsc/internal/printer/", "T1"), ("tsc/internal/sourcemap/", "T2"),
                          ("tsc/internal/transformers/tstransforms/", "T3"),
                          ("tsc/internal/transformers/moduletransforms/", "T4"),
                          ("tsc/internal/transformers/inliners/", "T4"),
                          ("tsc/internal/transformers/estransforms/", "T5"),
                          ("tsc/internal/transformers/jsxtransforms/", "T6"),
                          ("tsc/internal/transformers/declarations/", "T7"),
                          ("tsc/internal/pseudochecker/", "T7"),
                          ("tsc/internal/outputpaths/", "T8"), ("tsc/internal/transpile/", "T8")):
        if go.startswith(prefix):
            return group
    if go in FRAMEWORK:
        return "T3"
    if go in (*MOVED, PROGRAM):
        return "T8"
    raise ValueError(f"no checkpoint group for {go}")


# Reviewed dispositions of the functions no marker names. An equivalent site
# is (path, anchor): the anchor must occur exactly once in the file.
REVIEWED = {
    "tsc/internal/compiler/emitHost.go:emitHost.GetRedirectTargets": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_compiler/src/checker_module_specifiers.rs",
                 "for (alias, destination) in &program.redirect_paths {"),
        "reason": "No pinned caller reaches it through the emit host: it is there because DeclarationEmitHost "
                  "embeds ModuleSpecifierGenerationHost, and neither the declaration transformer nor its "
                  "tracker generates module specifiers through the host (the node builder uses the checker's "
                  "program, nodebuilder.go:285). The Program.GetRedirectTargets read it forwards to is inline "
                  "at the checker host's module-specifier site."},
    "tsc/internal/compiler/emitHost.go:emitHost.ResolveModuleName": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_module/src/resolver.rs", "pub fn resolve("),
        "reason": "No caller and no interface requirement at the pin (moduletransforms' "
                  "getExternalModuleNameFromPath, which names such a host, is a stub returning \"\"); Phase 1's "
                  "review records the Program wrapper with the same body unused at the pin. The nil-redirect "
                  "resolution it forwards to is Resolver::resolve."},
    "tsc/internal/compiler/emitter.go:isSourceFileNotJson": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_compiler/src/declaration_diagnostics.rs",
                 "let diagnostics = if source.script_kind == ScriptKind::JSON"),
        "reason": "Its one caller, getDeclarationDiagnostics, filters JSON files out; the port tests the file's "
                  "script kind inline (ast.IsJsonSourceFile)."},
    "tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.setPos": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/change_tracker_writer.rs", "fn on_before_emit_node(&mut self"),
        "reason": "Each before-emit handler records the last non-trivia position inline, as the pin's handlers "
                  "call setPos; nodes and lists keep separate maps."},
    "tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.setEnd": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/change_tracker_writer.rs", "fn on_after_emit_node(&mut self"),
        "reason": "Each after-emit handler records the last non-trivia position inline, as the pin's handlers "
                  "call setEnd; nodes and lists keep separate maps."},
    "tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.getPos": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/change_tracker_writer.rs", "fn range_of_node(&self"),
        "reason": "range_of_node and range_of_list read the recorded start, zero when absent as the pin's map "
                  "read, for the positions assignPositionsToNode assigns."},
    "tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.getEnd": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/change_tracker_writer.rs", "fn range_of_node(&self"),
        "reason": "range_of_node and range_of_list read the recorded end, zero when absent as the pin's map "
                  "read, for the positions assignPositionsToNode assigns."},
    "tsc/internal/printer/utilities.go:isNotPrologueDirective": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_ast/src/utilities.rs", "pub fn is_prologue_directive(view: AstView<'_>"),
        "reason": "No caller at the pin; the predicate it negates is ported at the Rust site."},
    "tsc/internal/printer/utilities.go:tryGetEnd": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/printer.rs", "fn greatest_end(end: i64, ends: &[Option<i64>])"),
        "reason": "greatestEnd's helper: the callers pass each candidate end as an Option read from the node, "
                  "list or range, and greatest_end skips None as tryGetEnd's nil test does (its panic on an "
                  "unhandled type has no Rust counterpart)."},
    "tsc/internal/printer/utilities.go:findSpanEnd": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/emit_context/environment.rs", "let find_span_end = |array: &[NodeId]"),
        "reason": "Its callers are in mergeEnvironment; the port's one local span finder serves both of the "
                  "pin's helpers."},
    "tsc/internal/printer/utilities.go:findSpanEndWithEmitContext": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_printer/src/emit_context/environment.rs", "let find_span_end = |array: &[NodeId]"),
        "reason": "Its callers are in mergeEnvironment; the port's one local span finder serves both of the "
                  "pin's helpers, with the emit-context tests as closures over the context."},
    "tsc/internal/pseudochecker/type.go:PseudoTypeDefault.AsPseudoType": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_pseudochecker/src/types.rs", "fn new_pseudo_type(data: PseudoTypeData)"),
        "reason": "newPseudoType reaches the embedded header through it; the Rust payload enum is the pseudo "
                  "type and its kind derives from the variant, so construction is the allocation."},
    "tsc/internal/pseudochecker/type.go:PseudoObjectElement.AsPseudoObjectElement": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_pseudochecker/src/types.rs", "fn new_pseudo_object_element("),
        "reason": "newPseudoObjectElement reaches the embedded header through it; the Rust element holds the "
                  "shared fields beside its payload variant, from which the kind derives."},
    "tsc/internal/transformers/tstransforms/legacydecorators.go:elideNodes": {
        "disposition": "equivalent",
        "rust": ("crates/tsr_transformers/src/tstransforms/legacydecorators.rs", "fn elide_modifiers("),
        "reason": "No caller at the pin and no unused Rust copy; elide_modifiers, its modifier-list twin, "
                  "performs the same elision (an empty list at the original's location)."},
}


def inventory(root=ROOT):
    """Pinned functions in inventory order: (id, file)."""
    with (root / "data/go-functions.tsv").open(newline="") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    if rows[0] != ["# upstream " + PIN]:
        raise ValueError("function inventory pin differs from the reviewed Phase 3 scope")
    header = rows[1]
    file_index, id_index = header.index("file"), header.index("id")
    return [(row[id_index], row[file_index]) for row in rows[2:] if len(row) > id_index]


def scope_files(root=ROOT):
    """The 73 Phase 3 files: the ledger's phase-3 entries and the two moved files."""
    ledger = tomllib.loads((root / "PORTS.toml").read_text())
    if ledger["pin"] != PIN:
        raise ValueError("ledger pin differs from the reviewed Phase 3 scope")
    return sorted({entry["go"] for entry in ledger["file"] if entry.get("phase") == 3} | set(MOVED))


def scope(root=ROOT, functions=None):
    """The scope's files and function ids (inventory order, the Emit family last)."""
    files = scope_files(root)
    functions = inventory(root) if functions is None else functions
    known = {identity for identity, _ in functions}
    missing = [identity for identity in EMIT_FAMILY if identity not in known]
    if missing:
        raise ValueError("Emit family functions not in the inventory: " + ", ".join(missing))
    chosen = set(files)
    ids = [identity for identity, go in functions if go in chosen] + list(EMIT_FAMILY)
    return files, ids, known


def binding(values):
    return (len(values), digest(canonical(sorted(values))))


def marker_sites(root=ROOT):
    """Every marker under crates/ with all of its sites, `path:line`, sorted by path."""
    sites = {}
    for path in sorted((root / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            match = MARKER.match(line)
            if match and ":" in match.group(1):
                sites.setdefault(match.group(1), []).append(f"{path.relative_to(root).as_posix()}:{number}")
    return sites


def resolve_site(site, root=ROOT):
    """A reviewed (path, anchor) site as `path:line`, or None when it does not resolve uniquely."""
    path, anchor = site
    target = root / path
    if not path.startswith("crates/") or target.suffix != ".rs" or not target.is_file():
        return None
    lines = [number for number, line in enumerate(target.read_text().splitlines(), 1) if anchor in line]
    return f"{path}:{lines[0]}" if len(lines) == 1 else None


def phase3_prefixes(files):
    """Marker ids under these prefixes belong to Phase 3: its files and package directories."""
    directories = {go.rsplit("/", 1)[0] + "/" for go in files if not go.startswith("tsc/internal/compiler/")}
    return tuple(sorted(directories)) + tuple(f"{go}:" for go in MOVED)


def build_document(root=ROOT, markers=None, reviewed=None, functions=None):
    """The audit document; its `problems` list is empty when it is valid."""
    reviewed = REVIEWED if reviewed is None else reviewed
    markers = marker_sites(root) if markers is None else markers
    files, ids, known = scope(root, functions)
    problems = []
    if binding(files) != REVIEWED_FILES:
        problems.append(f"the scope's files differ from the reviewed {REVIEWED_FILES[0]}")
    if binding(ids) != REVIEWED_FUNCTIONS:
        problems.append(f"the scope's functions differ from the reviewed {REVIEWED_FUNCTIONS[0]}")
    in_scope = set(ids)
    for identity in sorted(set(reviewed) - in_scope):
        problems.append(f"{identity}: reviewed, but not a Phase 3 function")
    entries = {}
    for identity in ids:
        go = identity.split(":", 1)[0]
        sites = markers.get(identity, [])
        entry = {"group": group_of(go)}
        review = reviewed.get(identity)
        pending = next((value for prefix, value in PENDING.items() if go.startswith(prefix)), None)
        if len(sites) > 1:
            entry |= {"status": "duplicate", "rust": sorted(sites)}
            problems.append(f"{identity}: {len(sites)} port markers ({', '.join(sorted(sites))})")
        elif sites:
            entry |= {"status": "mapped", "rust": sites}
            if review:
                problems.append(f"{identity}: a port marker names it, so its reviewed disposition is stale")
        elif review:
            kind = review.get("disposition")
            reason = review.get("reason")
            if not isinstance(reason, str) or not reason.strip():
                problems.append(f"{identity}: {kind} needs a reason")
            if kind == "equivalent":
                site = resolve_site(review.get("rust") or ("", ""), root)
                if site is None:
                    problems.append(f"{identity}: equivalent site {review.get('rust')} does not resolve to one line")
                entry |= {"status": "equivalent", "rust": [site] if site else [], "reason": reason}
            elif kind == "later":
                if review.get("owner") not in LATER_OWNERS:
                    problems.append(f"{identity}: later needs an owner among {', '.join(LATER_OWNERS)}")
                entry |= {"status": "later", "owner": review.get("owner"), "reason": reason}
            elif kind == "gap":
                if review.get("owner") not in CHECKPOINTS:
                    problems.append(f"{identity}: gap needs the checkpoint that closes it")
                entry |= {"status": "gap", "owner": review.get("owner"), "reason": reason}
            else:
                problems.append(f"{identity}: unknown reviewed disposition {kind!r}")
                entry |= {"status": "gap", "reason": reason}
        elif pending:
            entry |= {"status": "pending_c3", "owner": pending[0], "reason": pending[1]}
        else:
            entry |= {"status": "gap", "reason": "no marker and no reviewed disposition"}
            problems.append(f"{identity}: no port marker and no reviewed disposition")
        entries[identity] = entry
    prefixes = phase3_prefixes(files)
    unknown = {identity: sorted(sites) for identity, sites in markers.items()
               if identity not in known and identity.startswith(prefixes)}
    for identity, sites in sorted(unknown.items()):
        problems.append(f"{identity}: a port marker names no pinned function ({', '.join(sites)})")
    by_file = {}
    for identity, entry in entries.items():
        go = identity.split(":", 1)[0]
        row = by_file.setdefault(go, {"group": entry["group"], "functions": {}})
        row["functions"][identity] = {key: value for key, value in entry.items() if key != "group"}
    for go in files:
        by_file.setdefault(go, {"group": group_of(go), "functions": {}})
    by_file[PROGRAM]["selection"] = "the Emit family: the plan's seven and the destination audit's Phase 3 operations"
    for row in by_file.values():
        row["counts"] = tally(entry["status"] for entry in row["functions"].values())
    groups = {group: {"title": GROUPS[group],
                      "files": sorted(go for go, row in by_file.items() if row["group"] == group),
                      "counts": tally(entry["status"] for entry in entries.values() if entry["group"] == group)}
              for group in GROUPS}
    totals = tally(entry["status"] for entry in entries.values())
    return {
        "version": 1,
        "pin": PIN,
        "scope": {
            "description": ("Every pinned function of the 73 Phase 3 files (the 71 PORTS.toml phase-3 entries and "
                            "compiler/emitter.go and compiler/emitHost.go, which T0 moves) and the Emit family of "
                            "compiler/program.go (docs/PHASE3-plan.md sections 2 and 5)."),
            "files": len(files), "files_sha256": binding(files)[1],
            "functions": len(ids), "functions_sha256": binding(ids)[1],
            "moved_files": list(MOVED), "emit_family": list(EMIT_FAMILY),
        },
        "groups": groups,
        "totals": totals,
        "files": dict(sorted(by_file.items())),
        "unknown_markers": unknown,
        "problems": problems,
        "complete": not problems and not any(totals[kind] for kind in OPEN),
    }


def tally(statuses):
    counts = {kind: 0 for kind in STATUSES} | {"total": 0}
    for status in statuses:
        counts[status] += 1
        counts["total"] += 1
    return counts


def render(document):
    return json.dumps(document, indent=1, sort_keys=True) + "\n"


def table(document):
    columns = ("total", "mapped", "equivalent", "later", "gap", "pending_c3", "duplicate")
    lines = ["| Group | " + " | ".join(columns) + " |", "| --- |" + " ---: |" * len(columns)]
    for group, row in document["groups"].items():
        lines.append(f"| {group} {row['title']} | " + " | ".join(str(row["counts"][c]) for c in columns) + " |")
    lines.append("| all | " + " | ".join(str(document["totals"][c]) for c in columns) + " |")
    return "\n".join(lines)


def check(*, complete=False, root=ROOT, path=None):
    """Problems with the committed audit: stale, invalid, or (with `complete`) open."""
    document = build_document(root)
    path = AUDIT if path is None else path
    found = list(document["problems"])
    if not path.is_file() or path.read_text() != render(document):
        found.append(f"{path.relative_to(root) if path.is_relative_to(root) else path} differs from the rebuilt audit")
    if complete and not document["complete"]:
        found.append("the audit is not complete: " + ", ".join(
            f"{document['totals'][kind]} {kind}" for kind in OPEN if document["totals"][kind]))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "check", "summary"))
    parser.add_argument("--complete", action="store_true", help="check: also require no gap and no pending function")
    args = parser.parse_args()
    if args.command == "build":
        document = build_document()
        AUDIT.parent.mkdir(parents=True, exist_ok=True)
        AUDIT.write_text(render(document))
        print(table(document))
        for problem in document["problems"]:
            print("problem: " + problem, file=sys.stderr)
        if document["problems"]:
            raise SystemExit(1)
    elif args.command == "check":
        found = check(complete=args.complete)
        for problem in found:
            print("problem: " + problem, file=sys.stderr)
        if found:
            raise SystemExit(1)
        print("phase3 audit current")
    else:
        print(table(build_document()))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase3 audit failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
