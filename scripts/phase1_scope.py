"""The Phase 1 operation scope: every in-scope Go symbol and its disposition.

Built from the two audit inputs the plan names -- PORTS.toml and
status/unmapped-functions.json -- joined against the actual Rust sources each
ledger row claims as its home. The ledger is an audit input, not a task count:
an unmapped Go function may already be represented in Rust, and a mapped file
may still have no behavioral witness. Every row therefore carries how its
disposition was reached, so a rule-derived guess is never mistaken for a review.

Dispositions are the five the plan fixes: covered, implemented_untested,
missing, equivalent_rust and later_phase.
"""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PHASE = 1

DISPOSITIONS = ("covered", "implemented_untested", "missing", "equivalent_rust", "later_phase")


def ledger() -> list[dict]:
    return tomllib.loads((ROOT / "PORTS.toml").read_text())["file"]


def unmapped() -> dict[str, list[str]]:
    raw = json.loads((ROOT / "status/unmapped-functions.json").read_text())
    return raw["unmapped_functions"]


def unmapped_ids() -> set[str]:
    """Every operation id the audit input reports as unmapped."""
    return {entry for entries in unmapped().values() for entry in entries}


def inventory() -> list[dict]:
    """Every Go function at the pin, from the committed inventory.

    data/go-functions.tsv is the complete inventory, not just the unmapped
    remainder. Building the scope from it is what makes a *mapped* operation
    with no behavioral witness visible; the unmapped list alone would hide it.
    """
    rows: list[dict] = []
    lines = (ROOT / "data/go-functions.tsv").read_text().splitlines()
    header = None
    for line in lines:
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if header is None:
            header = fields
            continue
        rows.append(dict(zip(header, fields)))
    return rows


def snake(name: str) -> str:
    """Go exported/unexported name to the Rust name this port would use."""
    stripped = re.sub(r"^\(\*?[A-Za-z0-9_]+\)\.", "", name)
    out = re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", stripped)
    out = re.sub(r"(?<=[A-Z])([A-Z][a-z])", r"_\1", out)
    return out.lower()



# ---------------------------------------------------------------------------
# Phase 1 membership.
#
# The ledger's `phase` column is a coarse label, not this plan's scope. Taking
# it literally both *excludes* work the plan puts in Phase 1 (AST, scanner,
# parser and the compiler runner's syntactic operations are labelled phase 0)
# and *includes* editor, emit and API test helpers the plan explicitly pushes to
# later phases. Membership is therefore declared here against the plan's
# section 2 scope, with a reason per package, and the ledger is used only as an
# audit input.
#
# Value is (membership, reason). Membership is "full", "partial" or a phase
# number for work that belongs to a later phase.
# ---------------------------------------------------------------------------
MEMBERSHIP: dict[str, tuple[object, str]] = {
    # Core, collections, text, numbers and paths.
    "internal/core": ("full", "core helpers and option semantics"),
    "internal/collections": ("full", "ordered maps/sets and copy-on-write scopes"),
    "internal/tspath": ("full", "path operations"),
    "internal/nativepath": ("full", "native path helpers"),
    "internal/stringutil": ("full", "string helpers; actual home is tsr_jsstring"),
    "internal/jsnum": ("full", "number semantics"),
    "internal/semver": ("full", "semver parse and ranges"),
    "internal/debug": ("full", "shared debug helpers used by in-scope packages"),
    "internal/osutil": ("full", "OS helpers used by the VFS family"),
    "internal/symlinks": ("full", "symlink resolution used by module resolution"),
    # JSON and locale.
    "internal/json": ("full", "caller-visible JSON contract"),
    "internal/locale": ("full", "locale parse, canonicalization and fallback"),
    # Diagnostics and library assets.
    "internal/diagnostics": ("full", "generated message identities and localization"),
    "internal/diagnosticwriter": (
        "full",
        "the formatting slice the config baselines render through",
    ),
    "internal/bundled": ("full", "packaged library access"),
    # Both glob dialects, kept separate on purpose.
    "internal/glob": ("full", "the LSP/test glob grammar"),
    "internal/vfs/vfsmatch": ("full", "the configuration matching dialect"),
    # Package JSON.
    "internal/packagejson": ("full", "package JSON; actual home is tsr_module"),
    # The VFS family.
    "internal/vfs/internal": ("full", "shared VFS internals"),
    "internal/vfs/osvfs": ("full", "live OS filesystem"),
    "internal/vfs/iovfs": ("full", "io/fs-backed filesystem"),
    "internal/vfs/cachedvfs": ("full", "caching wrapper"),
    "internal/vfs/trackingvfs": ("full", "tracking wrapper"),
    "internal/vfs/wrapvfs": ("full", "wrapping adapter"),
    "internal/vfs/vfstest": ("full", "deterministic test filesystem used by the traces"),
    "internal/vfs/vfsmock": ("full", "mock filesystem, audited separately from OS behavior"),
    # Syntax.
    "internal/ast": ("full", "AST utilities, owners and lazy storage"),
    "internal/scanner": ("full", "scanner"),
    "internal/parser": ("full", "parser and JSDoc"),
    "internal/binder": ("full", "file-owned binding"),
    "internal/astnav": ("full", "syntax navigation"),
    "internal/evaluator": ("full", "the reusable constant evaluator"),
    # Options and module resolution.
    "internal/tsoptions": ("full", "config, command-line and build-option parsing"),
    "internal/tsoptions/tsoptionstest": ("full", "the pinned config test host"),
    "internal/module": ("full", "module resolution"),
    # The compiler runner, partially.
    "internal/compiler": (
        "partial",
        "only the runner's parse/bind/syntactic operations are Phase 1; program "
        "orchestration for checking and emit belongs to phases 2 and 3. F4a enumerates "
        "the exact syntactic surface",
    ),
    # Test infrastructure this phase's own comparisons run through.
    "internal/testrunner": ("full", "corpus expansion used by the parser/binder inventories"),
    "internal/testutil": ("full", "shared test helpers"),
    "internal/testutil/baseline": ("full", "the baseline renderer the 309 outputs go through"),
    "internal/testutil/filefixture": ("full", "fixture loading used by the config tests"),
    "internal/testutil/harnessutil": ("full", "option and skip policy used by the corpus"),
    "internal/testutil/parsetestutil": ("full", "parser test helpers"),
    "internal/testutil/stringtestutil": ("full", "string test helpers"),
    "internal/testutil/tsbaseline": ("full", "baseline helpers used by syntactic comparisons"),
    "internal/repo": ("full", "repository path resolution used by every native probe"),
    # Later phases. Named so the inventory records a destination and a reason
    # instead of silently dropping them.
    "internal/checker": (2, "checker semantics"),
    "internal/pseudochecker": (2, "syntactic-only checker slice owned by phase 2"),
    "internal/printer": (3, "emit printing"),
    "internal/transformers": (3, "transforms"),
    "internal/transformers/declarations": (3, "declaration emit"),
    "internal/transformers/estransforms": (3, "ES transforms"),
    "internal/transformers/inliners": (3, "emit inliners"),
    "internal/transformers/jsxtransforms": (3, "JSX transforms"),
    "internal/transformers/moduletransforms": (3, "module transforms"),
    "internal/transformers/tstransforms": (3, "TypeScript transforms"),
    "internal/sourcemap": (3, "source maps"),
    "internal/outputpaths": (3, "emit output paths"),
    "internal/transpile": (3, "transpilation"),
    "internal/testutil/emittestutil": (3, "emit test helpers"),
    "cmd/tsc": (4, "CLI execution"),
    "internal/execute": (4, "CLI execution"),
    "internal/execute/build": (4, "build scheduling"),
    "internal/execute/incremental": (4, "incremental build state"),
    "internal/execute/tsc": (4, "CLI driver"),
    "internal/execute/tsctests": (4, "CLI test harness"),
    "internal/execute/watchmanager": (4, "watch"),
    "internal/fswatch": (4, "native file watching"),
    "internal/tracing": (4, "tracing"),
    "internal/pprof": (4, "profiling"),
    "internal/testutil/jstest": (4, "JS execution test helpers"),
    "internal/ls": (5, "language service"),
    "internal/ls/autoimport": (5, "auto-import service"),
    "internal/ls/change": (5, "language service change tracking"),
    "internal/ls/lsconv": (5, "language service conversions"),
    "internal/ls/lsutil": (5, "language service helpers"),
    "internal/lsp": (5, "LSP server"),
    "internal/lsp/lsproto": (5, "LSP protocol"),
    "internal/lsp/lspwatcher": (5, "LSP watcher"),
    "internal/fourslash": (5, "fourslash"),
    "internal/project": (5, "project system"),
    "internal/project/ata": (5, "automatic type acquisition"),
    "internal/project/background": (5, "project background work"),
    "internal/project/dirty": (5, "project dirty tracking"),
    "internal/project/logging": (5, "project logging"),
    "internal/contentmapper": (5, "content mapper execution"),
    "internal/ipc": (5, "IPC transport"),
    "internal/jsonrpc": (5, "JSON-RPC"),
    "internal/spanmap": (5, "span mapping"),
    "internal/modulespecifiers": (5, "module specifier generation for the language service"),
    "internal/format": (5, "formatting services"),
    "internal/testutil/autoimporttestutil": (5, "auto-import test helpers"),
    "internal/testutil/lsptestutil": (5, "LSP test helpers"),
    "internal/testutil/projecttestutil": (5, "project test helpers"),
    "internal/testutil/contentmappertest": (5, "content mapper test helpers"),
    "internal/testutil/fsbaselineutil": (5, "filesystem baseline helpers for the project system"),
    "internal/api": (6, "API server"),
    "internal/api/encoder": (6, "API wire encoder"),
}

# Planned ledger crates whose behavior actually lives elsewhere today. The plan
# records these boundaries explicitly (section 1, gap 6); an absent crate
# directory is a location observation, not proof the behavior is missing.
KNOWN_HOMES: dict[str, str] = {
    "internal/packagejson": "tsr_module::package_json / tsr_module::package_maps",
    "internal/stringutil": "tsr_jsstring",
    "internal/vfs/vfsmatch": "tsr_tsoptions::glob (configuration matching dialect)",
    "internal/glob": "no Rust home; the LSP/test glob grammar is a separate dialect",
    "internal/evaluator": "partly tsr_checker::template",
    "internal/collections": "tsr_core and consumer-local collections",
    "internal/json": "no dedicated Rust home; order-sensitive readers live in their consumers",
    "internal/locale": "no Rust home at this pin",
}

# A Go name whose snake form is this short or this common matches unrelated Rust
# helpers, so a hit is not evidence. These stay `missing` pending a real witness.
GENERIC_NAMES = frozenset(
    "new len get set find first last next name key value clone copy identity equal compare "
    "contains index insert remove push pop size count parse format read write open close".split()
)


def workspace_symbol_index() -> dict[str, list[str]]:
    """Every `fn` name defined anywhere under crates/, with its locations.

    Searching the whole workspace rather than the ledger's claimed files is
    deliberate: 117 of the 169 Phase 1 ledger rows record no Rust home, and
    several name a crate directory that does not exist because the behavior
    moved (see KNOWN_HOMES).
    """
    index: dict[str, list[str]] = {}
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        text = path.read_text(errors="replace")
        rel = str(path.relative_to(ROOT))
        for name in set(re.findall(r"\bfn\s+([a-z0-9_]+)", text)):
            index.setdefault(name, []).append(rel)
    return index


def classify(
    entry: dict,
    symbol: str,
    mapped: bool,
    index: dict[str, list[str]],
    coverage: list[str],
) -> tuple[str, str]:
    """Return (disposition, basis). Basis records how the disposition was reached.

    `covered` requires an *exact* operation-level link, which is what F0 asks
    for. The ledger's `verify` field cannot supply one: its entries are
    file-level producer metric expressions such as `run.e1.parity >= 0.999`.
    They say a source file's port is exercised by a producer; they do not say
    which operation any single metric witnesses. Treating their presence as
    coverage marked 2,703 operations `covered` with no artifact behind any of
    them. They are retained per row as `ledger_verification` for context, and
    a mapped operation without an exact link is `implemented_untested`.
    """
    status = entry.get("status")
    verify = entry.get("verify") or []

    if status == "out-of-scope":
        return "later_phase", "ledger marks the source file out of scope for the port"
    # An exact link is the strongest evidence there is, and it does not depend on
    # whether the ledger happens to map the symbol: a rust_gated witness runs the
    # Rust, so the operation is covered either way.
    if coverage:
        return (
            "covered",
            f"witnessed by {len(coverage)} exact link(s): " + ", ".join(coverage[:3]),
        )
    if mapped:
        if coverage:
            return (
                "covered",
                f"witnessed by {len(coverage)} exact case link(s): " + ", ".join(coverage[:3]),
            )
        if verify:
            return (
                "implemented_untested",
                f"ledger maps the symbol and its source file carries {len(verify)} file-level "
                "producer metric(s), which do not witness this operation individually",
            )
        return "implemented_untested", "ledger maps the symbol but records no verification link"

    candidate = snake(symbol)
    locations = index.get(candidate, [])
    if locations and candidate not in GENERIC_NAMES and len(candidate) >= 6:
        shown = ", ".join(locations[:2]) + (" ..." if len(locations) > 2 else "")
        return (
            "implemented_untested",
            f"unmapped in the audit input, but `{candidate}` is defined at {shown}; "
            "needs a behavioral witness to become covered",
        )
    if locations:
        return (
            "missing",
            f"unmapped; `{candidate}` matches an unrelated generic Rust helper, which is not evidence",
        )
    home = KNOWN_HOMES.get(entry["package"])
    if home:
        return "missing", f"unmapped and absent from the actual home for this package ({home})"
    return "missing", "unmapped in the audit input and absent from every Rust crate by name"


def package_dependencies() -> dict[str, list[str]]:
    """Internal package imports, read from the pinned Go sources.

    Recorded per operation so a gap queue can be ordered by real dependency,
    not by guesswork about which leaf to port first.
    """
    upstream = ROOT / "upstream"
    dependencies: dict[str, set[str]] = {}
    for package in MEMBERSHIP:
        directory = upstream / "tsc" / package
        if not directory.is_dir():
            continue
        found: set[str] = set()
        for path in directory.glob("*.go"):
            text = path.read_text(errors="replace")
            for match in re.finditer(r'"github\.com/microsoft/TypeScript/tsc/(internal/[^"]+)"', text):
                if match.group(1) != package:
                    found.add(match.group(1))
        dependencies[package] = found
    return {k: sorted(v) for k, v in dependencies.items()}


# A committed artifact only witnesses Rust coverage when a producer actually
# runs Rust against it. `data/s07/path-observations.json` and
# `semver-observations.json` look like witnesses and are not: their producers
# (`s07_path_helpers.py`, `s07_semver.py`) invoke `go test` only and never
# execute Rust, so they are native authorities. Counting them would mark ~24
# operations covered on the strength of a Go-only run.
WITNESS_KINDS = ("rust_gated", "rust_ungated", "native_authority")
COVERING_WITNESS_KINDS = ("rust_gated",)


def cases_by_operation() -> dict[str, list[str]]:
    """Invert the committed case manifest so each operation names its cases.

    Prepared cases and `rust_gated` witnesses both count as exact coverage
    links. A native authority does not: it supplies an expected value, not
    evidence that Rust reproduces it.
    """
    path = ROOT / "data/phase1/cases.json"
    if not path.is_file():
        return {}
    document = json.loads(path.read_text())
    inverted: dict[str, list[str]] = {}
    for case in document.get("cases", []):
        # A prepared case only covers an operation once it actually compares.
        # A case whose last result is `not_implemented` witnesses the gap; it
        # does not close it, and calling that covered would report 108 absent
        # implementations as done.
        # Absent means unrun, which is not evidence either. Only a recorded
        # match covers.
        if case.get("last_result") != "match":
            continue
        for operation in case.get("coverage_operations", case.get("operations", [])):
            inverted.setdefault(operation, []).append(case["id"])
    for witness in document.get("witnesses", []):
        if witness.get("kind") not in COVERING_WITNESS_KINDS:
            continue
        for operation in witness.get("operations", []):
            inverted.setdefault(operation, []).append(witness["id"])
    return {k: sorted(v) for k, v in inverted.items()}


def witnessed_gaps() -> dict[str, list[str]]:
    """Operations whose prepared case runs but reports a missing Rust entry point."""
    path = ROOT / "data/phase1/cases.json"
    if not path.is_file():
        return {}
    document = json.loads(path.read_text())
    gaps: dict[str, list[str]] = {}
    for case in document.get("cases", []):
        if case.get("last_result") != "not_implemented":
            continue
        for operation in case.get("missing_operations", case.get("operations", [])):
            gaps.setdefault(operation, []).append(case["id"])
    return {k: sorted(v) for k, v in gaps.items()}


def witness_problems() -> list[str]:
    """Validate the committed witness records against the repository."""
    path = ROOT / "data/phase1/cases.json"
    if not path.is_file():
        return []
    document = json.loads(path.read_text())
    problems: list[str] = []
    seen: set[str] = set()
    for witness in document.get("witnesses", []):
        identity = witness.get("id", "<unnamed>")
        if identity in seen:
            problems.append(f"duplicate witness id {identity}")
        seen.add(identity)
        if witness.get("kind") not in WITNESS_KINDS:
            problems.append(f"{identity}: unknown witness kind {witness.get('kind')!r}")
        artifact = witness.get("artifact", "")
        if not artifact or not (ROOT / artifact).exists():
            problems.append(f"{identity}: artifact {artifact!r} does not exist")
        if witness.get("kind") == "rust_gated" and not witness.get("rust_gate"):
            problems.append(
                f"{identity}: a rust_gated witness must name the producer command that runs Rust"
            )
        if not witness.get("witnesses"):
            problems.append(f"{identity}: no description of what it actually witnesses")
    return problems


def leaf_preparation(scope: dict, cases: dict) -> dict:
    """Preparation is not parity: a classified gap is runnable, an absent link isn't.

    Keep the conservative package roster until an operation has an explicit
    reviewed home elsewhere. In particular, do not hide unlinked core helpers
    or generated/runtime mechanisms merely because new traces did not use them.
    """
    packages = {"internal/" + name for name in (
        "core", "collections", "stringutil", "jsnum", "semver", "json",
        "locale", "diagnostics", "bundled",
    )}
    prepared: dict[str, list[str]] = {}
    for case in cases.get("cases", []):
        if case.get("family") != "leaves" or case.get("last_result") not in (
            "match", "different", "not_implemented"
        ):
            continue
        for operation in case.get("operations", []):
            prepared.setdefault(operation, []).append(case["id"])
    for witness in cases.get("witnesses", []):
        if witness.get("kind") == "rust_gated":
            for operation in witness.get("operations", []):
                prepared.setdefault(operation, []).append(witness["id"])
    required = [r for r in scope["operations"] if r["go_package"] in packages]
    pending = [{"operation": r["id"], "rust_home": r["rust_home"],
                "disposition": r["disposition"]}
               for r in required if r["id"] not in prepared]
    return {"version": 1, "pin": scope["pin"], "complete": bool(required) and not pending,
            "total_operations": len(required), "prepared_operations": len(required) - len(pending),
            "pending": pending}


def build() -> dict:
    rows: list[dict] = []
    index = workspace_symbol_index()
    gaps = witnessed_gaps()
    missing_ids = unmapped_ids()
    entries = {e["go"]: e for e in ledger()}
    dependencies = package_dependencies()
    case_links = cases_by_operation()
    for function in inventory():
        package = function["package"]
        membership, reason = MEMBERSHIP[package]
        if membership not in ("full", "partial"):
            continue
        entry = entries.get(function["file"]) or {
            "package": package, "status": None, "crate": None, "rust": [], "verify": [],
        }
        identity = function["id"]
        symbol = (
            f'{function["receiver"]}.{function["name"]}' if function.get("receiver") else function["name"]
        )
        mapped = identity not in missing_ids
        linked = case_links.get(identity, [])
        witnessing = gaps.get(identity, [])
        disposition, basis = classify(entry, symbol, mapped, index, linked)
        if witnessing and disposition != "covered":
            disposition = "missing"
            basis = (
                f"a prepared case runs and reports the Rust entry point absent: "
                + ", ".join(witnessing[:3])
            )
        linked = sorted(set(linked) | set(witnessing))
        if linked and disposition == "missing":
            # A case exists for it, so the gap is witnessed rather than merely
            # inferred from the audit input.
            basis += f"; witnessed by {len(linked)} prepared case(s)"
        rows.append(
            {
                "id": identity,
                "go_source": function["file"],
                "go_package": package,
                "symbol": symbol,
                "membership": membership,
                "membership_reason": reason,
                "mapped_in_ledger": mapped,
                "ledger_status": entry.get("status"),
                "ledger_phase": entry.get("phase"),
                "ledger_crate": entry.get("crate"),
                "rust_home": list(entry.get("rust") or []),
                "actual_home": KNOWN_HOMES.get(package),
                # File-level producer metrics from the ledger. Context, not an
                # operation-level coverage claim; see classify().
                "ledger_verification": list(entry.get("verify") or []),
                "disposition": disposition,
                "basis": basis,
                "basis_kind": "rule",
                "cases": linked,
                "depends_on": dependencies.get(package, []),
                "destination_phase": PHASE if disposition != "later_phase" else None,
            }
        )
    rows.sort(key=lambda r: r["id"])
    counts: dict[str, int] = {d: 0 for d in DISPOSITIONS}
    for row in rows:
        counts[row["disposition"]] += 1
    later = {
        package: {"destination_phase": membership, "reason": reason}
        for package, (membership, reason) in sorted(MEMBERSHIP.items())
        if membership not in ("full", "partial")
    }
    return {
        "version": 2,
        "membership": {
            package: {"membership": membership, "reason": reason}
            for package, (membership, reason) in sorted(MEMBERSHIP.items())
            if membership in ("full", "partial")
        },
        "excluded_packages": later,
        "pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
        "phase": PHASE,
        "dispositions": list(DISPOSITIONS),
        "counts": counts,
        "total_operations": len(rows),
        "operations": rows,
    }


def verify(scope: dict) -> list[str]:
    problems: list[str] = []
    rows = scope.get("operations", [])
    if not rows:
        problems.append("scope has no operations; an empty inventory cannot be complete")
    ids = [r["id"] for r in rows]
    if len(ids) != len(set(ids)):
        problems.append("scope contains duplicate operation ids")
    for row in rows:
        if row.get("disposition") not in DISPOSITIONS:
            problems.append(f"{row.get('id')}: unclassified disposition {row.get('disposition')!r}")
        if not row.get("basis"):
            problems.append(f"{row.get('id')}: disposition has no recorded basis")
        if row.get("disposition") == "later_phase" and row.get("destination_phase") is not None:
            problems.append(f"{row.get('id')}: later_phase row must name a destination outside phase 1")
        if row.get("disposition") == "equivalent_rust" and row.get("basis_kind") != "review":
            problems.append(f"{row.get('id')}: equivalent_rust requires a reviewed behavioral witness")
        if row.get("disposition") == "covered" and not row.get("cases"):
            problems.append(
                f"{row.get('id')}: covered requires exact case links, but none are recorded"
            )
        if row.get("disposition") == "implemented_untested" and row.get("cases"):
            problems.append(
                f"{row.get('id')}: an operation with exact case links cannot be implemented_untested"
            )
    counts: dict[str, int] = {d: 0 for d in DISPOSITIONS}
    for row in rows:
        if row.get("disposition") in counts:
            counts[row["disposition"]] += 1
    if counts != scope.get("counts"):
        problems.append("scope counts disagree with its own operation rows")
    if scope.get("total_operations") != len(rows):
        problems.append("scope total_operations disagrees with its own operation rows")
    return problems
