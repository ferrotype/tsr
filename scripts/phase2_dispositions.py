#!/usr/bin/env python3
"""Phase 2 C7.4: the operation and dependency disposition (docs/PHASE2-C7-plan.md, C7.4).

`build` gives every function of the Phase 2 files (the ledger's phase-2
entries in `PORTS.toml`) one disposition, from `data/go-functions.tsv`, the
port markers under `crates/` and the seven checkpoint audits:

- `mapped`, with its first marker site;
- `equivalent`, with the audit's Rust site and reason;
- `later`, with the audit's owner, which must be a later phase.

A later audit's disposition supersedes an earlier one's. A function with none,
with an open one (`gap`, `missing_mapping`), with a Rust site that does not
exist or handed to a Phase 2 checkpoint is rejected. The result is
`data/phase2/dispositions.json`.

It also lists every checkpoint's handoffs to Phases 3 to 5 with their
validator fields (C7.4 (d); whether each still withholds a row is the
residual list's question) and the Phase 1 items Phase 2 inherited, each closed
with the evidence the build checks (C7.4 (e)).

`build` also binds each Phase 2 file in the ledger: status `ported`, its Rust
paths (the files that hold its functions' markers and equivalent sites, with
the paths already listed) and the run-level `run.checker` checks that verify
it, so that `cargo xtask status` derives `verified` once the checker run is
current. `build --check` rebuilds both without writing and fails unless the
committed files already hold them. The producer's `c7_dispositions` is
`complete()`.
"""
from __future__ import annotations
import argparse
import csv
import importlib.util
import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import phase2_audit  # noqa: E402

DISPOSITIONS = ROOT / "data/phase2/dispositions.json"
LEDGER = ROOT / "PORTS.toml"
AUDITS = tuple(ROOT / f"data/phase2/c{n}-audit.json" for n in range(1, 8))
PHASE2_OWNERS = {f"C{n}" for n in range(1, 8)}
LATER_OWNERS = ("Phase 3", "Phase 4", "Phase 5")
OPEN = ("gap", "missing_mapping")
MARKER = phase2_audit.MARKER

# The run-level checker metrics that verify each Phase 2 file (C7.4 (a)).
PARITY = ("run.checker.errors_parity == 1", "run.checker.types_parity == 1", "run.checker.symbols_parity == 1")
DISPLAY = ("run.checker.display_parity == 1",)
# Files that declare only types: their Rust homes hold the ported types.
TYPE_HOMES = {
    "tsc/internal/ipc/protocol.go": ("crates/tsr_ipc/src/protocol.rs",),
    "tsc/internal/modulespecifiers/types.go": ("crates/tsr_checker/src/host.rs", "crates/tsr_checker/src/module_specifiers.rs",
                                               "crates/tsr_checker/src/module_specifiers_paths.rs"),
}
NODE_BUILDER = {f"tsc/internal/checker/{name}.go" for name in (
    "nodebuilder", "nodebuilder_hover", "nodebuilderimpl", "nodebuilderscopes", "nodecopy",
    "pseudotypenodebuilder", "symboltracker", "printer")}


# The Phase 1 items Phase 2 inherited (C1.9), each closed with the evidence
# that closes it: a path and a text it must contain.
INHERITANCES = (
    {"item": "the 23 loader-side project-reference operations", "owner": "Phase 1", "state": "closed",
     "evidence": ["docs/PHASE1-F5b-destinations.md", "These 23 operations were never exceptions. The Phase 1 closure ported them"]},
    {"item": "the six checker-facing project-reference Program accessors and isSourceFromProjectReference",
     "owner": "C1", "state": "closed",
     "evidence": ["docs/PHASE2-C1-plan.md", "C1 ports them in C1.2"]},
    {"item": "generic qualified-name serialization's fourth scoped typeParameterSymbolList", "owner": "C5",
     "state": "closed",
     "evidence": ["crates/tsr_checker/src/node_builder_scopes.rs",
                  "pub(super) symbols: CopyOnWriteSet<SymbolId, crate::types::FastState>,"]},
)
LATER = ("Phase 3", "Phase 4", "Phase 5")


def inheritances(root=ROOT):
    """The inherited items, each with its evidence checked."""
    for entry in INHERITANCES:
        path, text = entry["evidence"]
        if text not in (root / path).read_text():
            raise ValueError(f"the evidence for {entry['item']} no longer holds")
    return [dict(entry) for entry in INHERITANCES]


def later_phase_handoffs(root=ROOT):
    """Every checkpoint's handoffs to a later phase, with their validator fields."""
    found = []
    for path in sorted((root / "data/phase2").glob("c[0-9]-claims.json")):
        checkpoint = path.name.split("-")[0].upper()
        for row in json.loads(path.read_text()).get("rows", []):
            for key in ("handoff", "incoming"):
                handoff = row.get(key)
                if isinstance(handoff, dict) and handoff.get("owner") in LATER:
                    found.append({"checkpoint": checkpoint, "id": row["id"], "status": row.get("status"),
                                  **{field: handoff.get(field) for field in (
                                      "owner", "go", "cause", "domains", "blocker", "trace", "capture_sha256",
                                      "request_sha256", "raw_observation_sha256", "reproduce")}})
    return found


def content_mapper_files(root=ROOT):
    spec = importlib.util.spec_from_file_location("ledger_init", root / "scripts/ledger-init.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return set(module.CONTENT_MAPPER_FILES)


def verify_for(go, content_mappers):
    """The checks that verify one Phase 2 file."""
    if go in content_mappers:
        return ["run.checker.content_mappers == true"]
    if go == "tsc/internal/compiler/checkerpool.go":
        return ["run.checker.mode_parity == true", "run.checker.assignments == true"]
    if go.startswith("tsc/internal/modulespecifiers/") or go == "tsc/internal/nodebuilder/types.go":
        return list(DISPLAY)
    if go.startswith("tsc/internal/checker/"):
        checks = list(PARITY)
        if go in NODE_BUILDER:
            checks += DISPLAY
        if go == "tsc/internal/checker/services.go":
            checks.append("run.checker.services == true")
        return checks
    raise ValueError(f"no verification binding for Phase 2 file {go}")


def phase2_files(ledger):
    return [entry for entry in ledger["file"] if entry.get("phase") == 2]


def functions(files, root=ROOT):
    """The pinned function ids of the given Go files, in inventory order."""
    with (root / "data/go-functions.tsv").open(newline="") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    header = rows[1]
    file_index, id_index = header.index("file"), header.index("id")
    return [row[id_index] for row in rows[2:] if row[file_index] in files]


def marker_sites(root=ROOT):
    """Each marked function's first marker site, `path:line`, over sorted paths."""
    sites = {}
    for path in sorted((root / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            match = MARKER.match(line)
            if match and ":" in match.group(1):
                sites.setdefault(match.group(1), f"{path.relative_to(root).as_posix()}:{number}")
    return sites


def audit_dispositions(audits=AUDITS):
    """Each function's disposition in the latest audit that gives one."""
    found = {}
    for path in audits:
        checkpoint = path.name.split("-")[0].upper()
        for identity, entry in json.loads(path.read_text()).get("dispositions", {}).items():
            found[identity] = (checkpoint, entry)
    return found


def site_exists(site, root=ROOT):
    path, _, line = str(site).rpartition(":")
    target = root / path
    return (path.startswith("crates/") and target.suffix == ".rs" and target.is_file() and line.isdigit()
            and 1 <= int(line) <= len(target.read_bytes().split(b"\n")))


def dispositions(identities, markers, audits, root=ROOT):
    """Each function's disposition row, and the rejected ones with the cause."""
    rows, rejected = [], []
    for identity in identities:
        if identity in markers:
            rows.append({"id": identity, "disposition": "mapped", "rust": markers[identity]})
            continue
        if identity not in audits:
            rejected.append({"id": identity, "cause": "no marker and no audit disposition"})
            continue
        checkpoint, entry = audits[identity]
        kind = entry.get("disposition")
        if kind == "equivalent" and site_exists(entry.get("rust"), root):
            rows.append({"id": identity, "disposition": "equivalent", "rust": entry["rust"],
                         "reason": entry["reason"], "audit": checkpoint})
        elif kind == "later" and entry.get("owner") in LATER_OWNERS:
            rows.append({"id": identity, "disposition": "later", "owner": entry["owner"],
                         "reason": entry["reason"], "audit": checkpoint})
        elif kind == "mapped":
            rejected.append({"id": identity, "cause": f"{checkpoint} records it mapped but no marker names it"})
        elif kind in OPEN:
            rejected.append({"id": identity, "cause": f"{checkpoint} leaves it open ({kind})"})
        elif kind == "later":
            rejected.append({"id": identity, "cause": f"{checkpoint} hands it to {entry.get('owner')}, inside Phase 2"})
        else:
            rejected.append({"id": identity, "cause": f"{checkpoint}'s Rust site {entry.get('rust')} does not exist"})
    return rows, rejected


def ledger_bindings(ledger, rows, content_mappers):
    """Each Phase 2 file's status, Rust paths and verification checks."""
    homes = {}
    functioned = {row["id"].split(":", 1)[0] for row in rows}
    for row in rows:
        if row["disposition"] in ("mapped", "equivalent"):
            homes.setdefault(row["id"].split(":", 1)[0], set()).add(row["rust"].rpartition(":")[0])
    bindings = {}
    for go, paths in TYPE_HOMES.items():
        if go in functioned:
            raise ValueError(f"{go} declares functions; its Rust homes come from their dispositions")
        homes.setdefault(go, set()).update(paths)
    for entry in phase2_files(ledger):
        rust = sorted(homes.get(entry["go"], set()) | set(entry.get("rust", [])))
        if not rust:
            raise ValueError(f"Phase 2 file {entry['go']} has no Rust home")
        bindings[entry["go"]] = {"status": "ported", "rust": rust,
                                 "verify": verify_for(entry["go"], content_mappers)}
    return bindings


def render(value):
    return json.dumps(value, indent=1, sort_keys=True) + "\n"


def rebind_ledger(text, bindings):
    """The ledger text with each bound file's status, rust and verify lines."""
    blocks = text.split("\n[[file]]\n")
    seen = set()
    for index, block in enumerate(blocks[1:], 1):
        match = re.search(r'^go = "([^"]+)"$', block, re.M)
        if not match or match.group(1) not in bindings:
            continue
        binding = bindings[match.group(1)]
        seen.add(match.group(1))
        for field in ("status", "rust", "verify"):
            block, count = re.subn(rf"^{field} = .*$", f"{field} = {json.dumps(binding[field], ensure_ascii=False)}",
                                   block, count=1, flags=re.M)
            if count != 1:
                raise ValueError(f"ledger entry {match.group(1)} lacks {field}")
        blocks[index] = block
    if seen != set(bindings):
        raise ValueError("ledger lacks Phase 2 files: " + ", ".join(sorted(set(bindings) - seen)))
    return "\n[[file]]\n".join(blocks)


def build_document(root=ROOT):
    ledger_text = (root / "PORTS.toml").read_text()
    ledger = tomllib.loads(ledger_text)
    files = {entry["go"] for entry in phase2_files(ledger)}
    identities = functions(files, root)
    rows, rejected = dispositions(identities, marker_sites(root), audit_dispositions(
        tuple(root / f"data/phase2/c{n}-audit.json" for n in range(1, 8))), root)
    counts = {kind: sum(row["disposition"] == kind for row in rows) for kind in ("mapped", "equivalent", "later")}
    document = {"version": 1, "pin": ledger["pin"], "files": len(files), "functions": len(identities),
                "counts": counts, "rejected": rejected, "dispositions": rows,
                "handoffs": later_phase_handoffs(root), "inheritances": inheritances(root)}
    bindings = ledger_bindings(ledger, rows, content_mapper_files(root))
    return document, rebind_ledger(ledger_text, bindings)


def complete(root=ROOT):
    """The committed dispositions and ledger bindings are current and reject nothing."""
    document, ledger = build_document(root)
    committed = root / "data/phase2/dispositions.json"
    return (not document["rejected"] and committed.is_file() and committed.read_text() == render(document)
            and (root / "PORTS.toml").read_text() == ledger)


def build(*, check=False, root=ROOT):
    document, ledger = build_document(root)
    if check:
        if (root / "data/phase2/dispositions.json").read_text() != render(document):
            raise ValueError("the committed dispositions differ from the rebuilt ones")
        if (root / "PORTS.toml").read_text() != ledger:
            raise ValueError("the ledger's Phase 2 bindings differ from the rebuilt ones")
    else:
        (root / "data/phase2/dispositions.json").write_text(render(document))
        (root / "PORTS.toml").write_text(ledger)
    if document["rejected"]:
        raise ValueError(f"{len(document['rejected'])} functions have no valid disposition, first "
                         + json.dumps(document["rejected"][0]))
    return {key: document[key] for key in ("files", "functions", "counts")}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build",))
    parser.add_argument("--check", action="store_true", help="fail unless the committed files are current")
    args = parser.parse_args()
    print(json.dumps(build(check=args.check), indent=1))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase2 dispositions failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
