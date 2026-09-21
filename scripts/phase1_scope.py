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


# `/// port: tsc/internal/<pkg>/<file>.go:<Symbol>` above a `fn`. The annotation
# is the author's own statement of which Go operation the function ports, and it
# is stronger evidence than the name it happens to have.
#
# The tail is a LOOKAHEAD on purpose. `re.finditer` does not overlap, so a
# pattern that consumed the `fn` header would swallow every annotation stacked
# above it and read only the first -- and stacking is how this tree records one
# Rust function serving two pinned operations, which it does 113 times,
# including `format` serving both WriteFormatDiagnostics and
# FormatDiagnosticsWithColorAndContext. A zero-width tail lets each stacked
# annotation start its own match.
#
# The skip group accepts attribute lines as well as comments, because an
# attribute between the doc comment and the item is ordinary Rust and stopped
# the earlier pattern dead, and it accepts a macro metavariable in place of a
# visibility keyword, because this tree declares items that way inside macro
# bodies.
_PORT_ANNOTATION = re.compile(
    r"port:\s*(tsc/[^\s`]+\.go:[A-Za-z0-9_.]+)"
    # Everything after the operation id is a LOOKAHEAD, so a match consumes only
    # the annotation itself. re.finditer does not overlap, and this tree stacks
    # two annotations above one `fn` 113 times to record one Rust function
    # serving two pinned operations -- `format` serves both
    # WriteFormatDiagnostics and FormatDiagnosticsWithColorAndContext. A pattern
    # that consumed the header, or that refused to skip a sibling annotation,
    # reads exactly one of each pair and silently drops the other.
    r"(?=[^\n]*\n"
    # Skip further comment and attribute lines, including sibling annotations.
    r"(?:[^\S\n]*(?://[^\n]*|#!?\[[^\n]*)\n)*?"
    # The item header. A macro metavariable stands in for a visibility keyword
    # inside macro bodies, which this tree also does.
    r"[^\S\n]*(?:\$[a-z_]+\s+|pub(?:\([^)]*\))?\s+)?"
    r"(?:const\s+|async\s+|unsafe\s+|extern\s+\"[^\"]*\"\s+)*"
    r"fn\s+([a-z0-9_]+))"
)
# 220 of the 4,306 `port:` lines in crates/ are deliberately not matched. 66 sit
# above a `let`, and most of the rest above an `if`, a `match` or a match arm:
# they annotate a STATEMENT inside a body, not the item that ports the
# operation, so reading them as a function-level declaration would attribute the
# whole function to whatever a line inside it happens to mirror. 17 more sit
# above a multi-line `#[allow(...)]`, which the single-line attribute skip does
# not span; that is a real gap and a small one, left rather than answered with a
# brace-balanced skip.


def declared_ports() -> dict[str, set[str]]:
    """Rust fn name -> the Go operation ids its own `port:` annotations name.

    A by-name match is a guess; a `port:` annotation is a claim. Where the two
    disagree the annotation wins, and the guess must not be reported as
    evidence. Two Go packages can carry a function of the same name with
    DIFFERENT behavior -- `isDoubleQuotedString` exists in both
    `internal/parser` (which tests the single-quote token flag) and
    `internal/tsoptions` (which does not) -- and the by-name rule attributed the
    Rust port of the first to the second, which is the over-attribution this
    whole scope exists to avoid.
    """
    declared: dict[str, set[str]] = {}
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        text = path.read_text(errors="replace")
        for operation, name in _PORT_ANNOTATION.findall(text):
            declared.setdefault(name, set()).add(operation)
    return declared


def annotated_homes() -> dict[str, list[str]]:
    """Go operation id -> the Rust files whose `port:` annotations claim it.

    `rust_home` carries the LEDGER's claim, which PORTS.toml records per source
    FILE, so a package the ledger does not map reports an empty home even when
    the port exists -- all 40 `internal/diagnosticwriter` rows did, while
    crates/tsr_compiler/src/diagnostic_writer/ carried explicit annotations for
    ten of them. This is the other source, kept beside the ledger's rather than
    merged into it: a ledger claim and an author's annotation are different
    kinds of evidence and a reader should be able to tell which one answered.
    """
    homes: dict[str, set[str]] = {}
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "/src/" not in str(path.as_posix()):
            continue
        text = path.read_text(errors="replace")
        rel = str(path.relative_to(ROOT))
        for operation, _name in _PORT_ANNOTATION.findall(text):
            homes.setdefault(operation, set()).add(rel)
    return {operation: sorted(files) for operation, files in homes.items()}


def annotations_outside_src() -> list[dict]:
    """`port:` annotations that do not live in a crate's `src/`.

    A `port:` marker is a claim that this code IS the port of a pinned
    operation. Outside `src/` it cannot be: an example binary or a test file is
    not a production home. Every one of these is therefore either a second,
    independent implementation carrying the production marker -- a drift risk,
    because two bodies now answer to one marker and nothing compares them -- or
    a marker that should say it is a re-implementation. Reported rather than
    refused: these predate this step, and turning someone else's drift into a
    hard failure here would be the wrong place to do it.
    """
    found: list[dict] = []
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        if "/src/" in str(path.as_posix()):
            continue
        text = path.read_text(errors="replace")
        rel = str(path.relative_to(ROOT))
        for operation, name in _PORT_ANNOTATION.findall(text):
            found.append({"file": rel, "rust_fn": name, "operation": operation})
    return found


def classify(
    entry: dict,
    symbol: str,
    mapped: bool,
    index: dict[str, list[str]],
    coverage: list[str],
    ports: dict[str, set[str]] | None = None,
    identity: str = "",
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
    ports = ports or {}

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
    # A same-named Rust function that declares itself the port of a DIFFERENT
    # Go operation is not evidence for this one. Without this the rule reads a
    # `port:` annotation as agreement merely because the names coincide.
    claimed = ports.get(candidate, set())
    if locations and claimed and identity not in claimed:
        return (
            "missing",
            f"unmapped in the audit input; `{candidate}` exists in Rust but its own annotation "
            f"names a different operation ({sorted(claimed)[0]}), so the name match is not evidence",
        )
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


# A committed artifact only witnesses Rust coverage when something actually runs
# Rust against it. That is usually the producer, and for
# `data/s07/path-observations.json` and `semver-observations.json` the producers
# (`s07_path_helpers.py`, `s07_semver.py`) invoke `go test` only and never
# execute Rust, so on the producer's account they are native authorities and
# counting them would mark ~24 operations covered on the strength of a Go-only
# run.
#
# A CONSUMER can gate just as well, which the first pass over this missed:
# crates/tsr_tspath/tests/go_observations.rs reads the frozen path requests and
# observations through include_str! and asserts the Rust answers equal them, so
# `cargo test` gates eight tspath operations against that same artifact. Both
# records exist, and they are about different things: the kind describes what
# runs, not what the file is.
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
    """Operations whose prepared case runs and reports a missing Rust entry point.

    A case PREPARES every operation it reaches and WITNESSES ABSENT only the
    ones the Rust driver actually reported missing, which is not the same list:
    a trace may drive five operations and find one of them unported. Falling
    back to `operations` folded the two together and marked an operation
    `missing` on the strength of a neighbour's gap -- an over-claim in the
    direction of saying the port has less than it does. So the narrower list is
    required rather than defaulted, and `record` derives it from the capture's
    own rust rows.
    """
    path = ROOT / "data/phase1/cases.json"
    if not path.is_file():
        return {}
    document = json.loads(path.read_text())
    gaps: dict[str, list[str]] = {}
    for case in document.get("cases", []):
        if case.get("last_result") != "not_implemented":
            continue
        for operation in case.get("missing_operations", []):
            gaps.setdefault(operation, []).append(case["id"])
    return {k: sorted(v) for k, v in gaps.items()}


def gap_record_problems(cases: dict) -> list[str]:
    """A case reporting `not_implemented` must name what the driver found absent."""
    problems: list[str] = []
    for case in cases.get("cases", []):
        if case.get("last_result") != "not_implemented":
            if case.get("missing_operations"):
                problems.append(
                    f"{case['id']}: records missing_operations but its last result is "
                    f"{case.get('last_result')!r}"
                )
            continue
        claimed = case.get("operations", [])
        missing = case.get("missing_operations")
        if not claimed:
            # A case may prepare an OUTPUT rather than an operation -- the
            # carried baseline renderer is 142 of them -- and then there is no
            # operation for a gap to name. Claiming one would attribute the
            # whole parse chain to every baseline, which is the over-attribution
            # this check exists to prevent, pointed the other way.
            if missing:
                problems.append(
                    f"{case['id']}: claims no operations but names a missing one; a case that "
                    "prepares an output witnesses no operation gap"
                )
            continue
        if not missing:
            problems.append(
                f"{case['id']}: reports not_implemented without naming which operation the "
                "driver found absent; run `phase1.py record --write` against a capture"
            )
            continue
        if not set(missing) <= set(claimed):
            problems.append(
                f"{case['id']}: names a missing operation the case does not claim to reach"
            )
    return problems


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


# ---------------------------------------------------------------------------
# The F1a leaf roster.
#
# F1a's exit condition is that *every leaf operation* is linked to a runnable
# prepared case or a verified existing witness. That makes the roster itself a
# claim: an operation dropped from it silently is work hidden behind a green
# gate. So membership is subtractive only through a reviewed ledger, where each
# removal names a category, the owner that does have it, and the evidence read
# at the pin.
#
# `unused_at_pin` is the one category with a mechanical check behind it: its
# evidence records how many times the symbol occurs in the pinned tree, and one
# occurrence is the definition itself. The plan's instruction for this file is
# to read a generic helper's callers before choosing its Rust contract, so a
# helper with no caller has no contract for this step to choose.
#
# An exemption is *not* a disposition. `later_step` in particular keeps the
# operation inside Phase 1 -- F3a is a Phase 1 step -- and only says F1a is not
# the step that prepares it. The one category that is also a disposition claim
# is `equivalent_rust`, which the plan already defines, so that one updates the
# scope row and is held to the same `basis_kind: "review"` bar.
# ---------------------------------------------------------------------------
# Each preparation step owns a package set and a ledger. The roster machinery
# below is written once over this map rather than per step, so a second step
# cannot quietly acquire a weaker gate than the first.
STEP_PACKAGES: dict[str, frozenset[str]] = {
    "leaves": frozenset(
        "internal/" + name
        for name in (
            "core", "collections", "stringutil", "jsnum", "semver", "json",
            "locale", "diagnostics", "bundled",
        )
    ),
    "filesystem": frozenset(
        "internal/" + name
        for name in (
            "tspath", "nativepath", "glob", "osutil", "symlinks",
            "vfs/vfsmatch", "vfs/internal", "vfs/osvfs", "vfs/iovfs",
            "vfs/cachedvfs", "vfs/trackingvfs", "vfs/wrapvfs",
            "vfs/vfstest", "vfs/vfsmock",
        )
    ),
    # F3a. `internal/compiler` is deliberately absent: F3a's program-loading
    # cases drive its loader, but the plan gives F4a the job of enumerating
    # which of its 340 operations are Phase 1 at all, and a package cannot be
    # rostered twice. Those cases link compiler operations without claiming to
    # account for them, and F4a's roster is where that accounting happens.
    "config": frozenset(
        "internal/" + name
        for name in (
            "tsoptions", "tsoptions/tsoptionstest", "module", "packagejson",
            "diagnosticwriter", "testutil/baseline", "testutil/filefixture",
        )
    ),
}

# The step a package belongs to, for the per-operation roster field. A package
# in two steps would make the gate ambiguous, so that is refused here.
_OWNED: dict[str, str] = {}
for _step, _packages in STEP_PACKAGES.items():
    for _package in _packages:
        if _package in _OWNED:
            raise ValueError(f"{_package} is claimed by both {_OWNED[_package]} and {_step}")
        _OWNED[_package] = _step

LEAF_PACKAGES = STEP_PACKAGES["leaves"]
FILESYSTEM_PACKAGES = STEP_PACKAGES["filesystem"]


def step_of(package: str) -> str | None:
    return _OWNED.get(package)


def roster_path(step: str) -> Path:
    return ROOT / f"data/phase1/{step}-roster.json"

ROSTER_CATEGORIES = {
    "build_tooling": "runs at build time and never in a compile; the port generates the same artifact elsewhere",
    "go_runtime": "a Go language mechanism -- scheduling, sync, arenas, vet markers -- with no caller-visible contract to reproduce",
    "generated_assertion": "not an operation: a compile-time assertion emitted by a generator",
    "later_step": "a real operation that a different named step prepares",
    "equivalent_rust": "the Go contract is reproduced exactly by a Rust language or standard library construct",
    "unused_at_pin": "exported but called by nothing at the pin, tests included, so no caller fixes the contract",
    "build_variant": "belongs to a build configuration this port does not produce, so no build reaches it",
    # Added by F3a. The six categories above all describe an operation with no
    # caller-visible compiler contract, or one another step owns. None of them
    # describes upstream's own test harness, which F3a is the first step to
    # roster: `internal/testutil/baseline` writes, tracks and diffs baseline
    # FILES, and `internal/testutil/filefixture` loads fixture inputs. The port
    # must reproduce the 309 reference outputs, and it does; it must not
    # reproduce the bookkeeping, because its comparisons are driven by
    # scripts/phase1*.py and by Rust tests that assert against frozen rows.
    # Bending `build_tooling` to cover this would have been the wrong kind of
    # convenience: that category says the port generates the same artifact
    # elsewhere, and there is no artifact here.
    "go_test_harness": "upstream's own test harness -- baseline file bookkeeping, fixture loading, "
                       "run tracking -- which exists to run the pinned tests; this port reproduces "
                       "the baselines, not the bookkeeping, because its comparisons run through its "
                       "own harness",
}

def leaf_roster(step: str = "leaves") -> dict:
    path = roster_path(step)
    if not path.is_file():
        return {"version": 1, "exemptions": []}
    return json.loads(path.read_text())


def roster_exemptions(step: str | None = None) -> dict[str, dict]:
    """Exemptions for one step, or for every step when none is named."""
    steps = [step] if step else list(STEP_PACKAGES)
    return {
        entry["operation"]: dict(entry, step=name)
        for name in steps
        for entry in leaf_roster(name).get("exemptions", [])
    }


PREPARING_RESULTS = ("match", "different", "not_implemented")


def prepared_links(cases: dict) -> dict[str, list[str]]:
    """Every leaf operation a case or gated witness has actually run for.

    Preparation is not coverage: a case reporting `not_implemented` prepares its
    operation -- it runs and classifies the gap -- while covering nothing. The
    gate and the exemption validator must agree on that set, so both read it
    from here. Asking cases_by_operation() instead, which answers only for
    recorded matches, let an operation be prepared and exempted at once.
    """
    links: dict[str, list[str]] = {}
    for case in cases.get("cases", []):
        if case.get("family") != "leaves" or case.get("last_result") not in PREPARING_RESULTS:
            continue
        for operation in case.get("operations", []):
            links.setdefault(operation, []).append(case["id"])
    for witness in cases.get("witnesses", []):
        if witness.get("kind") in COVERING_WITNESS_KINDS:
            for operation in witness.get("operations", []):
                links.setdefault(operation, []).append(witness["id"])
    return {k: sorted(v) for k, v in links.items()}


def roster_problems(
    scope: dict, cases: dict | None = None, step: str | None = None
) -> list[str]:
    """Validate a step's exemption ledger against the scope it subtracts from.

    With no step named, every declared step is validated, so a new step cannot
    be added without its ledger being held to the same bar as the first.
    """
    if step is None:
        return [p for name in STEP_PACKAGES for p in roster_problems(scope, cases, name)]
    packages = STEP_PACKAGES[step]
    path = roster_path(step)
    problems: list[str] = []
    if not path.is_file():
        return [
            f"{path.relative_to(ROOT)} is absent; the {step} roster has no reviewed ledger"
        ]
    document = leaf_roster(step)
    pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
    if document.get("pin") != pin:
        problems.append(
            f"{path.name} records pin {document.get('pin')!r}, not {pin!r}"
        )
    known = {row["id"]: row for row in scope.get("operations", [])}
    if cases is None:
        path = ROOT / "data/phase1/cases.json"
        cases = json.loads(path.read_text()) if path.is_file() else {}
    prepared = prepared_links(cases)
    seen: set[str] = set()
    label = path.stem
    for entry in document.get("exemptions", []):
        identity = entry.get("operation", "<unnamed>")
        if identity in seen:
            problems.append(f"{label}: duplicate exemption for {identity}")
        seen.add(identity)
        row = known.get(identity)
        if row is None:
            problems.append(f"{label}: {identity} is not an operation in the frozen scope")
            continue
        if row["go_package"] not in packages:
            problems.append(
                f"{label}: {identity} is not in a {step} package, so it was never on that roster"
            )
        category = entry.get("category")
        if category not in ROSTER_CATEGORIES:
            problems.append(f"{label}: {identity} has unknown category {category!r}")
        for field in ("owner", "evidence"):
            if not entry.get(field):
                problems.append(f"{label}: {identity} records no {field}")
        # An exemption and a prepared case are contradictory claims about the
        # same operation. Prefer the case and say so rather than silently
        # letting the ledger suppress work that was actually done.
        if identity in prepared:
            problems.append(
                f"{label}: {identity} is exempted but also prepared by "
                + ", ".join(prepared[identity][:3])
            )
        if category == "equivalent_rust" and row.get("disposition") != "equivalent_rust":
            problems.append(
                f"{label}: {identity} claims equivalent_rust but the scope row says "
                f"{row.get('disposition')!r}; rebuild the scope"
            )
    return problems


# The comparison family whose cases prepare each step's operations.
STEP_FAMILIES = {
    "leaves": ("leaves",),
    "filesystem": ("filesystem", "pilot"),
    "config": ("config",),
}


def leaf_preparation(scope: dict, cases: dict, step: str = "leaves") -> dict:
    """Preparation is not parity: a classified gap is runnable, an absent link isn't.

    Keep the conservative package roster until an operation has an explicit
    reviewed home elsewhere. In particular, do not hide unlinked helpers or
    generated/runtime mechanisms merely because new traces did not use them.

    Written once over STEP_PACKAGES so a later step cannot acquire a weaker
    gate than the first: the arithmetic, the exemption rules and the ledger
    validation are the same whichever step is asked for.
    """
    families = STEP_FAMILIES[step]
    prepared: dict[str, list[str]] = {}
    for case in cases.get("cases", []):
        if case.get("family") not in families or case.get("last_result") not in PREPARING_RESULTS:
            continue
        for operation in case.get("operations", []):
            prepared.setdefault(operation, []).append(case["id"])
    witnessed: dict[str, list[str]] = {}
    for witness in cases.get("witnesses", []):
        if witness.get("kind") in COVERING_WITNESS_KINDS:
            for operation in witness.get("operations", []):
                witnessed.setdefault(operation, []).append(witness["id"])
    exempt = roster_exemptions(step)
    required = [r for r in scope["operations"] if r["go_package"] in STEP_PACKAGES[step]]
    pending = [
        {"operation": r["id"], "rust_home": r["rust_home"], "disposition": r["disposition"]}
        for r in required
        if r["id"] not in prepared and r["id"] not in witnessed and r["id"] not in exempt
    ]
    accounted = [r for r in required if r["id"] not in {p["operation"] for p in pending}]
    by_category: dict[str, int] = {}
    for row in required:
        entry = exempt.get(row["id"])
        if entry and row["id"] not in prepared and row["id"] not in witnessed:
            by_category[entry["category"]] = by_category.get(entry["category"], 0) + 1
    problems = roster_problems(scope, cases, step)
    gap_problems = gap_record_problems({"cases": [
        case for case in cases.get("cases", []) if case.get("family") in families
    ]})
    # A step that prepares reference outputs as well as operations is held to
    # both. Written over the shared table so F3a's 80 command-line outputs get
    # F2a's gate rather than a new one; a step with no outputs reports none.
    outputs = None
    from phase1_baselines import STEP_OUTPUT_GROUPS, output_preparation

    if step in STEP_OUTPUT_GROUPS:
        outputs = output_preparation(cases, step)
    return {
        "version": 2,
        "step": step,
        "pin": scope["pin"],
        # `complete` is the published `<step>_prepared` result. It is false
        # while any operation on the roster is neither prepared, witnessed nor
        # exempted by a reviewed ledger entry, and false while that ledger
        # itself does not validate -- an exemption nobody can defend is not an
        # answer.
        "complete": bool(required) and not pending and not problems and not gap_problems
                    and (outputs is None or outputs["complete"]),
        "total_operations": len(required),
        "accounted_operations": len(accounted),
        "prepared_operations": sum(1 for r in required if r["id"] in prepared),
        "witnessed_operations": sum(
            1 for r in required if r["id"] not in prepared and r["id"] in witnessed
        ),
        "exempt_operations": sum(by_category.values()),
        "exempt_by_category": dict(sorted(by_category.items())),
        "pending": pending,
        "roster_problems": problems,
        "gap_problems": gap_problems,
        **({"outputs": outputs} if outputs is not None else {}),
    }


def build() -> dict:
    rows: list[dict] = []
    index = workspace_symbol_index()
    ports = declared_ports()
    homes = annotated_homes()
    gaps = witnessed_gaps()
    missing_ids = unmapped_ids()
    entries = {e["go"]: e for e in ledger()}
    dependencies = package_dependencies()
    case_links = cases_by_operation()
    exemptions = roster_exemptions()
    for function in inventory():  # noqa: PLR1702
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
        disposition, basis = classify(entry, symbol, mapped, index, linked, ports, identity)
        if witnessing and disposition != "covered":
            disposition = "missing"
            basis = (
                f"a prepared case runs and reports the Rust entry point absent: "
                + ", ".join(witnessing[:3])
            )
        basis_kind = "rule"
        # A reviewed `equivalent_rust` exemption is the one roster category that
        # is also a disposition claim, and the plan already defines that
        # disposition. Recording it here rather than only in the ledger keeps a
        # single answer per operation; verify() then holds it to the review bar.
        exemption = exemptions.get(identity)
        if exemption and exemption.get("category") == "equivalent_rust" and not linked:
            disposition = "equivalent_rust"
            basis = exemption["evidence"]
            basis_kind = "review"
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
                "annotated_home": homes.get(identity, []),
                "actual_home": KNOWN_HOMES.get(package),
                # File-level producer metrics from the ledger. Context, not an
                # operation-level coverage claim; see classify().
                "ledger_verification": list(entry.get("verify") or []),
                "disposition": disposition,
                "basis": basis,
                "basis_kind": basis_kind,
                "cases": linked,
                # Roster membership, orthogonal to the disposition: which step
                # owes a prepared case for this operation, whether it has one,
                # and if it is exempt, which reviewed ledger entry says so.
                "roster": {
                    "step": step_of(package),
                    "state": (
                        None
                        if step_of(package) is None
                        else "prepared" if linked
                        else f"exempt:{exemption['category']}" if exemption
                        else "pending"
                    ),
                    "owner": exemption["owner"] if exemption else None,
                },
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
