"""The Phase 1 operation scope: every in-scope Go symbol and its disposition.

Built from PORTS.toml, the pinned function inventory and Rust port markers. The
status/unmapped-functions.json audit view is derived from those same inputs;
reading it as a producer input would introduce a generated-status cycle. The
ledger is an audit input, not a task count:
an unmapped Go function may already be represented in Rust, and a mapped file
may still have no behavioral witness. Every row therefore carries how its
disposition was reached, so a rule-derived guess is never mistaken for a review.

Dispositions are the five the plan fixes: covered, implemented_untested,
missing, equivalent_rust and later_phase.
"""

from __future__ import annotations

import bisect
from collections import Counter
import functools
import gzip
import itertools
import json
import hashlib
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
    """Derive xtask's unmapped set from sources, not its generated status view.

    The generated worklist is an audit output, not an input to a producer: using
    it makes recording status invalidate the next producer fingerprint. Keep
    the same source-file eligibility and exact line-marker semantics as
    xtask::scan_markers/read_inventory. Annotation-to-function attribution is
    deliberately stricter elsewhere; this answers only xtask's mapped bit.
    """
    sources = {entry["go"] for entry in ledger()
               if entry.get("kind") == "source" and entry.get("status") != "out-of-scope"}
    known = {entry["id"] for entry in inventory() if entry["file"] in sources}
    markers: set[str] = set()
    for path in (ROOT / "crates").rglob("*.rs"):
        if "target" in path.relative_to(ROOT / "crates").parts:
            continue
        for line in path.read_text(errors="replace").splitlines():
            trimmed = line.lstrip()
            for prefix in ("/// port:", "//! port:", "// port:"):
                if trimmed.startswith(prefix):
                    marker = trimmed.removeprefix(prefix).strip()
                    if ":" in marker:
                        markers.add(marker)
                    break
    return known - markers


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
#
# A `mutation_kill` witness (docs/PHASE1-mutation-witnesses.md) covers too, but
# never by being declared: only the operations `recorded_mutations` finds bound
# to current artifacts and current Rust spans confer anything. Every consumer
# reads covering links through `covering_witness_operations` for that reason.
WITNESS_KINDS = ("rust_gated", "rust_ungated", "native_authority", "mutation_kill")
COVERING_WITNESS_KINDS = ("rust_gated", "mutation_kill")


def case_claims_digest(case: dict) -> str:
    """Bind the reviewed declaration, excluding only derived observation fields."""
    claims = {key: value for key, value in case.items()
              if key not in ("last_result", "result_evidence", "missing_operations")}
    return hashlib.sha256(json.dumps(claims, sort_keys=True).encode()).hexdigest()


def recorded_results(cases: dict) -> dict[str, str]:
    """A recorded outcome belongs to its exact request and operation claims.

    Old unbound records remain visible but cannot establish preparation or
    coverage until an authenticated capture records them again.
    """
    import phase1_capture as capture

    requests: dict[str, dict] = {}
    for family in {case.get("family") for case in cases.get("cases", [])}:
        if family in capture.FAMILIES:
            requests.update({row["case"]: row for row in capture.load_requests(capture.FAMILIES[family])["requests"]})
    results = {}
    for case in cases.get("cases", []):
        evidence = case.get("result_evidence", {})
        request = requests.get(case["id"])
        valid = (request is not None
                 and evidence.get("request_sha256") == capture.digest(capture.request_bytes(request))
                 and evidence.get("claims_sha256") == case_claims_digest(case)
                 and evidence.get("result") == case.get("last_result")
                 and evidence.get("missing_operations", []) == case.get("missing_operations", [])
                 and isinstance(evidence.get("capture_sha256"), str)
                 and re.fullmatch(r"[0-9a-f]{64}", evidence["capture_sha256"]) is not None)
        results[case["id"]] = case.get("last_result", "not_run") if valid else "not_run"
    return results


# ---------------------------------------------------------------------------
# Mutation-kill witnesses.
#
# The rules are docs/PHASE1-mutation-witnesses.md. A witness declares, per
# oracle, the operations it claims and the mutants that kill them; its recorded
# `mutation_evidence` (written only by `phase1.py record-mutations`) names the
# kill pairs and binds every committed artifact they rest on. This section is
# the child-free check: it runs no tool, rebuilds nothing, and recomputes every
# binding from committed bytes, the current oracle code and, in the live view,
# the current Rust sources.
#
# Staleness has two grains, both deliberate. A changed declaration, artifact,
# request inventory, Go oracle source, instrumentation or stage rule stales the
# whole witness: nothing it recorded can be trusted. A changed mutated span, a
# changed excused home or a new Rust home returns only the operations that rest
# on it to pending, so an edit to one parser function does not unwitness the
# other eight hundred; an edit elsewhere unwitnesses nothing.
#
# Two views, and which callers use them. The LIVE view (no committed scope)
# also reads the current Rust sources: spans, markers and homes. Only the
# inventory builder (`scope.build`, through `cases_by_operation`), `phase1.py
# inventory --check` and `record-mutations` use it, and `phase1_producers.py
# check` compares the committed scope.json with that live build. The COMMITTED
# view reads no Rust source: an operation is credited only while its artifacts
# bind and the committed scope.json still links it to the witness, which the
# live checks decided when scope.json was written. Coverage, preparation,
# rosters and the P1B receipt use it, so a routine edit inside a claimed span
# makes `check` report scope drift (and the confirmation receipt fail) without
# turning every producer unhealthy.
# ---------------------------------------------------------------------------
MUTATION_DIRECTORY = "data/phase1/mutation"
MUTATION_MANIFEST = f"{MUTATION_DIRECTORY}/manifest.json"
MUTATION_RESULTS = f"{MUTATION_DIRECTORY}/results.json.gz"
MUTATION_METRIC = "run.foundations.mutation_witnesses_complete"
# The P1B receipt: replays every recorded kill pair and its control, the base
# rows, and a hits-only trace of the excused homes, against a fresh schemata
# build of the current sources (phase1_integration validates).
MUTATION_CONFIRM_COMMAND = ["python3", "scripts/phase1_mutation_run.py", "confirm",
                            "--results", MUTATION_RESULTS, "--plan", MUTATION_MANIFEST]
# Each oracle's committed request inventory: the rows a native freeze and a
# kill must name, with each row's committed request digest. A native freeze
# must cover the selected inventory rows exactly, in order. `select` keeps only
# inventory rows whose field equals the value (the syntax schedule's rows that
# load; the others have no program to observe).
MUTATION_ORACLES = {
    "e1": {"inventory": "data/s06/requests.json", "rows": "requests", "request": "request_sha256",
           "select": None},
    "binder": {"inventory": "data/s07/binder-requests.json", "rows": "requests", "request": "request_sha256",
               "select": None},
    "syntax": {"inventory": "data/phase1/syntax-schedule.json", "rows": "rows",
               "request": "loading_request_sha256", "select": ("load", "loaded")},
    # The S06 requests again, with a per-node SubtreeFacts stage.
    "facts": {"inventory": "data/s06/requests.json", "rows": "requests", "request": "request_sha256",
              "select": None},
}
# Final states. `not_credited_multi_op`: the mutant's only differing rows are
# rows of an oracle that never credits a site carrying several operations (see
# MUTATION_SINGLE_OPERATION_ORACLES). `budget` (the row budget stopped the
# mutant with candidate rows left) and `not_run` are not final: a campaign with
# either is partial and records nothing.
MUTATION_STATES = ("killed", "survived", "crash", "timeout", "not_reached", "build_failed", "unsupported",
                   "not_credited_multi_op")
MUTATION_PARTIAL_STATES = ("budget", "not_run")
# States that prove a candidate row executed the mutated site. A home with any
# of these cannot be declared unreached, whatever the reviewer writes.
MUTATION_REACHED_STATES = ("killed", "survived", "crash", "timeout", "not_credited_multi_op", "budget")
# Oracles whose rows are whole programs (a syntax row loads its files and some
# sixty bundled libraries). Go enters nearly every parser operation on every
# such row, so "the operations Go entered on the kill row" cannot tell which
# operation of a shared site made the difference: there a kill never credits a
# site that carries markers for more than one operation.
MUTATION_SINGLE_OPERATION_ORACLES = ("syntax",)
# How an unreached-home excuse must begin: reach is measured in production and
# observation stages alike, and the text names every traced oracle after it.
MUTATION_UNREACHED_PHRASE = "no production or observation reach on"
MUTATION_SITE_KINDS = ("fn", "macro_fn", "arm", "stmt")
# A control computes the paired mutant's replacement and discards it, so the
# replacement's own side effects (an allocated missing node moves the node,
# text and identifier counters every oracle compares) are observed without the
# replaced value. It is never credited.
MUTATION_CONTROL = "control"
# A replacement that allocates must be paired with a control: a kill then has
# to differ from the control, not only from native. Checked here as well as in
# the plan, so a plan that forgets a control confers nothing.
_ALLOCATING = re.compile(r"\b(?:create|new)_\w*\s*\(")
# The fields a claimed mutant copies from the plan's manifest entry
# (tools/phase1/mutation/splicer). `ops` are the operations whose markers stand
# on the site, `markers` maps each to its marker line and `span` is the 1-based
# inclusive line range (from the topmost marker) whose bytes `span_sha256`
# digests, all at plan time; together they let the check find the span again
# after unrelated edits move it. `control` is the paired control's id, or null.
MUTANT_FIELDS = ("key", "op", "ops", "file", "function", "site_kind", "site_line", "markers", "span",
                 "span_sha256", "operator", "control")
# The fields of one plan home (`homes[op]`), each a marker site of the operation.
HOME_FIELDS = ("file", "function", "site_kind", "site_line", "span_sha256", "mutants")
MUTATION_EVIDENCE_FIELDS = ("claims_sha256", "manifest_sha256", "results_sha256", "native_sha256",
                            "go_reach_sha256")
_HEX = re.compile(r"[0-9a-f]{64}")
# The line-prefix marker rule of `unmapped_ids` and xtask: a comment line of its
# own whose whole remainder is the operation. Function-level annotations match
# it too; the plan's site kind decides which reading a site uses.
_MARKER_PREFIXES = ("/// port:", "//! port:", "// port:")
_CRATE_SOURCE = re.compile(r"crates/[^/]+/src/(?:[^/]+/)*[^/]+\.rs")
_FUNCTION_SITES = ("fn", "macro_fn")


def mutation_native_path(oracle: str) -> str:
    return f"{MUTATION_DIRECTORY}/native-{oracle}.json.gz"


def mutation_reach_path(oracle: str) -> str:
    return f"{MUTATION_DIRECTORY}/go-reach-{oracle}.json.gz"


def witness_claims_digest(witness: dict) -> str:
    """Bind a witness declaration, excluding only its recorded evidence."""
    claims = {key: value for key, value in witness.items() if key != "mutation_evidence"}
    return hashlib.sha256(json.dumps(claims, sort_keys=True).encode()).hexdigest()


def is_control(entry: dict) -> bool:
    """A plan entry that only observes a paired mutant's side effects."""
    return entry.get("operator") == MUTATION_CONTROL or "control_of" in entry


def allocates(entry: dict) -> bool:
    """Whether a plan entry's replacement calls a node or list constructor."""
    texts = [entry.get("operator")] + [insert.get("text") for insert in entry.get("insert") or []
                                       if isinstance(insert, dict)]
    return any(isinstance(text, str) and _ALLOCATING.search(text) for text in texts)


def home_identity(home: dict) -> str:
    """How a declaration's `not_claimed` names a plan home.

    A function home is its file and Rust function, as in round one; an arm or
    statement home adds its kind and plan-time line, which the manifest fixes.
    """
    base = f"{home.get('file')}:{home.get('function')}"
    return base if home.get("site_kind") in _FUNCTION_SITES else f"{base}#{home.get('site_kind')}@{home.get('site_line')}"


@functools.lru_cache(maxsize=256)
def _line_offsets(data: bytes) -> tuple[int, ...]:
    offsets = [0]
    position = data.find(b"\n")
    while position != -1:
        offsets.append(position + 1)
        position = data.find(b"\n", position + 1)
    if offsets[-1] == len(data):
        offsets.pop()
    return tuple(offsets)


def span_digest(data: bytes, start: int, end: int) -> str | None:
    """sha256 of lines start..=end (1-based) of `data`, each with its own "\\n".

    Lines are split on "\\n" only, exactly as Rust's `split_inclusive('\\n')`
    splits them, so the splicer and this check agree byte for byte. None when
    the range is not inside the file.
    """
    if type(start) is not int or type(end) is not int or start < 1 or end < start:
        return None
    offsets = _line_offsets(data)
    if end > len(offsets):
        return None
    stop = offsets[end] if end < len(offsets) else len(data)
    return hashlib.sha256(data[offsets[start - 1]:stop]).hexdigest()


_SOURCE_CACHE: dict[tuple[str, bytes], tuple[list, list]] = {}


def mutation_sources(root: Path = ROOT) -> dict:
    """Current port markers under crates/*/src, by the rules the scope uses.

    `functions` maps an operation to its function-level annotations
    (`_PORT_ANNOTATION`) as (file, Rust function, marker line); `statements`
    maps an operation to its line-prefix marker lines, function annotations
    included (`marker_homes` separates them).
    """
    functions: dict[str, list[tuple[str, str, int]]] = {}
    statements: dict[str, list[tuple[str, int]]] = {}
    texts: dict[str, bytes] = {}
    base = root / "crates"
    for path in sorted(base.glob("*/src/**/*.rs")):
        if "target" in path.relative_to(base).parts:
            continue
        relative = path.relative_to(root).as_posix()
        data = path.read_bytes()
        if b"port:" not in data:
            continue
        texts[relative] = data
        cached = _SOURCE_CACHE.get((relative, data))
        if cached is None:
            text = data.decode("utf-8", "replace")
            starts = [0] + [match.end() for match in re.finditer("\n", text)]
            line = lambda position: bisect.bisect_right(starts, position)  # noqa: E731
            statement_markers = []
            for number, raw in enumerate(text.split("\n"), 1):
                trimmed = raw.lstrip()
                prefix = next((prefix for prefix in _MARKER_PREFIXES if trimmed.startswith(prefix)), None)
                if prefix is not None and ":" in trimmed.removeprefix(prefix).strip():
                    statement_markers.append((trimmed.removeprefix(prefix).strip(), number))
            cached = ([(op, name, line(match.start())) for match in _PORT_ANNOTATION.finditer(text)
                       for op, name in [match.groups()]], statement_markers)
            if len(_SOURCE_CACHE) > 4096:
                _SOURCE_CACHE.clear()
            _SOURCE_CACHE[(relative, data)] = cached
        for op, name, line_number in cached[0]:
            functions.setdefault(op, []).append((relative, name, line_number))
        for op, line_number in cached[1]:
            statements.setdefault(op, []).append((relative, line_number))
    return {"functions": functions, "statements": statements, "texts": texts}


def marker_homes(sources: dict, operation: str) -> tuple[set[tuple[str, str]], Counter]:
    """The operation's current Rust homes: function homes, and marker sites per file.

    Function homes are (file, Rust function) of its `_PORT_ANNOTATION`s. Every
    other line-prefix marker of the operation marks an arm or statement site;
    they are counted per file, since only the plan can name them.
    """
    annotations = sources["functions"].get(operation, [])
    functions = {(file, name) for file, name, _line in annotations}
    annotated = {(file, line) for file, _name, line in annotations}
    markers = Counter(file for file, line in sources["statements"].get(operation, [])
                      if (file, line) not in annotated)
    return functions, markers


def mutation_site_static_problem(mutant: dict, operation: str) -> str | None:
    """What the committed plan alone says about a site: production source, a marker of `operation`."""
    if not isinstance(mutant.get("file"), str) or not _CRATE_SOURCE.fullmatch(mutant["file"]):
        return f"{mutant.get('file')} is not production source under crates/*/src"
    if (not isinstance(mutant.get("ops"), list) or operation not in mutant["ops"]
            or not isinstance(mutant.get("markers"), dict) or operation not in mutant["markers"]):
        return f"{mutant['file']}:{mutant.get('function')} is not an annotated Rust home of {operation}"
    return None


def mutation_site_problem(mutant: dict, operation: str, sources: dict) -> str | None:
    """None when the mutated span is current and is a Rust home of `operation`.

    The plan lists the operations whose markers stand on the site. The span is
    found again from a current marker of `operation` (a function annotation on
    the same function for a function site, the line-prefix marker for an arm or
    statement) at its plan-time offset, and must digest identically: an edit
    elsewhere that only moves the site does not stale it, any byte change inside
    the span (markers included) does, and a site never credits an operation
    whose marker is not on it.
    """
    static = mutation_site_static_problem(mutant, operation)
    if static:
        return static
    site = f"{mutant['file']}:{mutant['function']}"
    data = sources["texts"].get(mutant["file"])
    if mutant["site_kind"] in _FUNCTION_SITES:
        anchors = [line for file, name, line in sources["functions"].get(operation, [])
                   if file == mutant["file"] and name == mutant["function"]]
    else:
        anchors = [line for file, line in sources["statements"].get(operation, []) if file == mutant["file"]]
    if data is None or not anchors:
        return f"{site} no longer carries a {operation} marker"
    start, end = mutant["span"]
    planned = mutant["markers"][operation]
    if any(span_digest(data, start + line - planned, end + line - planned) == mutant["span_sha256"]
           for line in anchors):
        return None
    return f"mutated span {site} changed since the plan"


# Parsed artifacts and derived contexts, keyed by content digests only, so a
# changed byte is always re-read: the harness asks for the same bindings many
# times per check (scope, rosters, preparation, coverage) and the artifacts are
# large. Nothing cached is ever mutated except the per-context reach memo.
_ARTIFACT_CACHE: dict[tuple[str, str], object] = {}
_CONTEXT_CACHE: dict[tuple, tuple[dict | None, str | None]] = {}


def _artifact(root: Path, relative: str) -> tuple[str, object]:
    """(sha256 of the committed bytes, parsed JSON); gzip is judged by suffix."""
    raw = (root / relative).read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    key = (relative, digest)
    if key not in _ARTIFACT_CACHE:
        if len(_ARTIFACT_CACHE) > 32:
            _ARTIFACT_CACHE.clear()
        _ARTIFACT_CACHE[key] = json.loads(gzip.decompress(raw) if relative.endswith(".gz") else raw)
    return digest, _ARTIFACT_CACHE[key]


def _kill_oracle(results: dict, mutant: dict, kill: dict) -> object:
    return kill.get("oracle", mutant.get("oracle", results.get("oracle")))


def mutation_go_bindings(oracle: str) -> tuple[dict | None, str | None]:
    """What the current oracle code says the committed Go artifacts must record.

    The native freeze records the digests of the oracle's Go sources
    (`oracle_sources`); the Go-reach index records the instrumentation digest
    (patched sources and build flags), the stage rule it decoded with and that
    rule's digest. All are recomputed from the tree (`phase1_mutation_go`,
    file reads only), so an edited oracle, instrumentation patch or stage rule
    stales every witness resting on the old freeze or index instead of letting
    it keep crediting. `unstable_ops` are the pool-dependent operations the
    code names; the index must list at least these (the reach run adds any
    whose row set differed between its two instrumented runs).
    """
    try:
        import phase1_mutation_go as go
        bindings = {"oracle_sources": go.oracle_sources(oracle),
                    "instrumentation_sha256": go.instrumentation_digest(oracle),
                    "stage_rule": go.stage_rule(oracle), "stage_rule_sha256": go.stage_rule_digest(oracle),
                    "unstable_ops": sorted(go.POOL_OPERATIONS)}
    except (AttributeError, KeyError, OSError, TypeError, ValueError) as error:
        return None, f"the current code cannot state the {oracle} Go bindings: {error}"
    return bindings, None


def mutation_artifacts(oracle: object, artifact: object, root: Path = ROOT) -> tuple[dict | None, str | None]:
    """Load and cross-check the committed artifacts one oracle's kills rest on.

    Returns (context, None) or (None, reason). Checks only what is independent
    of any recorded evidence: formats, pins, the native freeze against the
    committed request inventory and the current oracle sources, and the Go-reach
    index against that freeze, the current instrumentation and stage rule.
    """
    spec = MUTATION_ORACLES.get(oracle) if isinstance(oracle, str) else None
    if spec is None:
        return None, f"unknown mutation oracle {oracle!r}"
    if not isinstance(artifact, str) or not artifact.startswith(MUTATION_DIRECTORY + "/"):
        return None, f"results artifact {artifact!r} is not under {MUTATION_DIRECTORY}"
    files = {"manifest": MUTATION_MANIFEST, "results": artifact, "native": mutation_native_path(oracle),
             "reach": mutation_reach_path(oracle), "inventory": spec["inventory"],
             "functions": "data/go-functions.tsv"}
    loaded = {}
    for name, relative in files.items():
        if not (root / relative).is_file():
            return None, f"{relative} is missing"
        try:
            loaded[name] = ((hashlib.sha256((root / relative).read_bytes()).hexdigest(), None)
                            if name == "functions" else _artifact(root, relative))
        except (OSError, ValueError, EOFError) as error:
            return None, f"{relative} is unreadable: {error}"
    bindings, reason = mutation_go_bindings(oracle)
    if reason:
        return None, reason
    pin = json.loads((root / "data/upstream.json").read_text())["pin"]
    key = (oracle, artifact, pin, json.dumps(spec, sort_keys=True), json.dumps(bindings, sort_keys=True),
           *(loaded[name][0] for name in files))
    if key not in _CONTEXT_CACHE:
        if len(_CONTEXT_CACHE) > 16:
            _CONTEXT_CACHE.clear()
        _CONTEXT_CACHE[key] = _mutation_context(oracle, artifact, spec, pin, files, loaded, bindings)
    return _CONTEXT_CACHE[key]


def _manifest_homes(manifest: dict, by_key: dict) -> tuple[dict | None, dict, str | None]:
    """The plan's `homes`: every marker site of every planned operation.

    Returns (homes by operation, (operation, mutant key) -> home identity,
    None) or (None, {}, reason) when the manifest records homes malformed. A
    manifest without `homes` (round one's) gives (None, {}, None): it cannot
    discharge the several-homes rule, so nothing it planned is credited.
    """
    homes = manifest.get("homes")
    if homes is None:
        return None, {}, None
    if not isinstance(homes, dict):
        return None, {}, "mutation manifest homes are not a map from operation to homes"
    parsed: dict[str, list[dict]] = {}
    home_of: dict[tuple[str, str], str] = {}
    for operation, rows in homes.items():
        if not isinstance(rows, list):
            return None, {}, f"mutation manifest homes of {operation} are not a list"
        for row in rows:
            if (not isinstance(row, dict) or not set(HOME_FIELDS) <= set(row)
                    or row["site_kind"] not in MUTATION_SITE_KINDS or not isinstance(row["mutants"], list)):
                return None, {}, f"mutation manifest has a malformed home of {operation}"
            for key in row["mutants"]:
                entry = by_key.get(key) if isinstance(key, str) else None
                if entry is None or is_control(entry) or operation not in (entry.get("ops") or []):
                    return None, {}, f"mutation manifest home of {operation} names mutant {key!r}, which is not a planned mutant of it"
                if (operation, key) in home_of:
                    return None, {}, f"mutation manifest puts mutant {key} in two homes of {operation}"
                home_of[(operation, key)] = home_identity(row)
            parsed.setdefault(operation, []).append({**{field: row[field] for field in HOME_FIELDS},
                                                     "id": home_identity(row)})
    return parsed, home_of, None


def _mutation_context(oracle: str, artifact: str, spec: dict, pin: str, files: dict,
                      loaded: dict, bindings: dict) -> tuple[dict | None, str | None]:
    manifest, results, native, reach, inventory = (loaded[name][1] for name in list(files)[:5])
    if not isinstance(inventory, dict) or not isinstance(inventory.get(spec["rows"]), list):
        return None, f"{spec['inventory']} has no {spec['rows']} list"
    if not isinstance(manifest, dict) or not isinstance(manifest.get("mutants"), list):
        return None, "mutation manifest has no mutant list"
    by_key: dict[str, dict] = {}
    by_id: dict[int, dict] = {}
    for mutant in manifest["mutants"]:
        if not isinstance(mutant, dict) or not isinstance(mutant.get("key"), str) or mutant["key"] in by_key:
            return None, "mutation manifest has a malformed or duplicated mutant key"
        if type(mutant.get("id")) is int:
            if mutant["id"] in by_id:
                return None, "mutation manifest has a duplicated mutant id"
            by_id[mutant["id"]] = mutant
        by_key[mutant["key"]] = mutant
    homes, home_of, reason = _manifest_homes(manifest, by_key)
    if reason:
        return None, reason
    if not isinstance(results, dict) or results.get("version") != 1 or not isinstance(results.get("mutants"), list):
        return None, "mutation results are not a version 1 mutant list"
    # phase1_mutation_run.results binds the campaign's plan and, per oracle, the
    # native freeze and Go-reach index it selected candidate rows with.
    inputs = results.get("inputs") if isinstance(results.get("inputs"), dict) else {}
    if inputs.get("plan_sha256") != loaded["manifest"][0]:
        return None, f"the results ran a plan other than the committed {MUTATION_MANIFEST}"
    campaign = (inputs.get("oracles") or {}).get(oracle) if isinstance(inputs.get("oracles"), dict) else None
    if not isinstance(campaign, dict):
        return None, f"the results record no {oracle} campaign"
    for name, field in (("native", "native_sha256"), ("reach", "go_reach_sha256")):
        if campaign.get(field) != loaded[name][0]:
            return None, f"the {oracle} campaign ran against another {files[name]}"
    outcomes: dict[str, dict] = {}
    for record in results["mutants"]:
        if not isinstance(record, dict) or not isinstance(record.get("key"), str) or record["key"] in outcomes:
            return None, "mutation results have a malformed or duplicated mutant key"
        if record["key"] not in by_key:
            return None, f"mutation results name mutant {record['key']} the manifest does not"
        outcomes[record["key"]] = record
    for name, document in (("native", native), ("reach", reach)):
        if not isinstance(document, dict) or document.get("version") != 1 or document.get("oracle") != oracle:
            return None, f"{files[name]} is not a version 1 {oracle} artifact"
        if document.get("pin") != pin:
            return None, f"{files[name]} records pin {document.get('pin')!r}, not {pin!r}"
    if reach.get("native_sha256") != loaded["native"][0]:
        return None, f"{files['reach']} was checked against a different native freeze"
    # The Go side as the current code states it (mutation_go_bindings).
    if native.get("oracle_sources") != bindings["oracle_sources"]:
        return None, f"{files['native']} was frozen from other {oracle} oracle sources than the current ones"
    if reach.get("instrumentation_sha256") != bindings["instrumentation_sha256"]:
        return None, f"{files['reach']} was measured with other instrumentation than the current one"
    if reach.get("stage_rule") != bindings["stage_rule"] or reach.get("stage_rule_sha256") != bindings["stage_rule_sha256"]:
        return None, f"{files['reach']} was decoded under another stage rule than the current {oracle} rule"
    if reach.get("go_functions_sha256") != loaded["functions"][0]:
        return None, f"{files['reach']} mapped functions with another {files['functions']}"
    unstable = reach.get("unstable_ops")
    if not isinstance(unstable, list) or not all(isinstance(op, str) for op in unstable):
        return None, f"{files['reach']} does not list its unstable (pool- or memo-dependent) operations"
    if not set(bindings["unstable_ops"]) <= set(unstable):
        return None, f"{files['reach']} omits unstable operations the current code names"
    # phase1_mutation_go writes `op_row_gaps` (first index, then increments);
    # the interface's plain `ops` index lists are read too.
    reach_index = reach.get("op_row_gaps", reach.get("ops"))
    if not isinstance(reach_index, dict):
        return None, f"{files['reach']} has no operation index"
    rows: dict[str, tuple[int, dict]] = {}
    for index, row in enumerate(native.get("rows") or []):
        if not isinstance(row, dict) or not isinstance(row.get("row"), str) or row["row"] in rows:
            return None, f"{files['native']} has a malformed or duplicated row"
        rows[row["row"]] = (index, row)
    if not rows:
        return None, f"{files['native']} freezes no rows"
    if reach.get("rows_checked") != len(rows):
        return None, f"{files['reach']} checked {reach.get('rows_checked')!r} rows, the native freeze has {len(rows)}"
    selected = [row for row in inventory[spec["rows"]] if isinstance(row, dict)
                and (spec["select"] is None or row.get(spec["select"][0]) == spec["select"][1])]
    committed = [(row.get("id"), row.get(spec["request"])) for row in selected]
    frozen = [(row["row"], row.get("request_sha256")) for row in native["rows"]]
    if frozen != committed:
        return None, f"{files['native']} rows differ from the committed request inventory {spec['inventory']}"
    # Every traced oracle: a campaign's, and any the results say were traced.
    traced = inputs.get("traced_oracles") if isinstance(inputs.get("traced_oracles"), list) else []
    campaigns = sorted(set(inputs["oracles"]) | {oracle for oracle in traced if isinstance(oracle, str)})
    reached = {key: _rust_reached(outcomes.get(key), set(campaigns)) for key in by_key}
    if homes is not None:
        homes = {operation: [dict(home, reach=[reached[key] for key in home["mutants"]]) for home in entries]
                 for operation, entries in homes.items()}
    source_inputs = {path: digest for path, digest in bindings["oracle_sources"].items() if isinstance(path, str)}
    return {"oracle": oracle, "artifact": artifact, "manifest": by_key, "manifest_ids": by_id,
            "results_document": results, "results": outcomes, "rows": rows, "requests": dict(committed),
            "reach": reach_index, "reach_gaps": "op_row_gaps" in reach, "reach_rows": {},
            "unstable": frozenset(unstable), "campaigns": campaigns, "homes": homes, "home_of": home_of,
            "digests": {"manifest_sha256": loaded["manifest"][0], "results_sha256": loaded["results"][0],
                        "native_sha256": loaded["native"][0], "go_reach_sha256": loaded["reach"][0]},
            "inputs": {**source_inputs, **{relative: loaded[name][0] for name, relative in files.items()}}}, None


def _reach_counts(entry: object) -> tuple[int, ...] | None:
    """One reach-table entry: [eligible rows, all rows] (production only, as round
    two wrote it) or [eligible rows, all rows, observation rows]; None if malformed."""
    if (isinstance(entry, list) and len(entry) in (2, 3)
            and all(type(value) is int and value >= 0 for value in entry)):
        return tuple(entry)
    return None


def _rust_reached(record: dict | None, campaigns: set[str]) -> bool | None:
    """Whether any driver executed a mutant's site: True, False, or None if unknown.

    Reach is Rust reach on any traced row, not only on candidate rows, and in
    observation stages as well as production ones: a home the drivers execute
    where Go never enters the operation, or only while observing (the e1
    encoder, a graph dump), is reached and cannot be excused as unreached.
    phase1_mutation_run.results records, per mutant and traced oracle,
    `reach[oracle] = [eligible rows, all rows, observation rows]` from the full
    trace (the third counts rows whose observation stages executed the site
    without activating it). Proof of non-reach needs that three-count entry,
    zero on every count, for every oracle campaign of the results; a reached
    state or a positive count anywhere is reach. A two-count entry measured
    production only, so it can show reach but never prove its absence.
    """
    if not isinstance(record, dict):
        return None
    if record.get("state") in MUTATION_REACHED_STATES:
        return True
    runs = record.get("oracles") if isinstance(record.get("oracles"), dict) else {}
    if any(isinstance(run, dict) and (run.get("state") in MUTATION_REACHED_STATES
                                      or any(type(run.get(field)) is int and run[field] > 0
                                             for field in ("rust_rows", "observe_rows")))
           for run in runs.values()):
        return True
    table = record.get("reach")
    if not isinstance(table, dict):
        return None
    counts = {oracle: _reach_counts(entry) for oracle, entry in table.items()}
    if any(entry is not None and any(entry) for entry in counts.values()):
        return True
    if not campaigns or not all(len(counts.get(oracle) or ()) == 3 for oracle in campaigns):
        return None
    return False


def reach_indices(value: object, gaps: bool) -> frozenset[int]:
    """Native row indices of one Go-reach entry; malformed entries reach nothing."""
    if not isinstance(value, list) or not set(map(type, value)) <= {int}:
        return frozenset()
    if not gaps:
        return frozenset(value)
    if value and (value[0] < 0 or min(value[1:], default=1) < 1):
        return frozenset()
    return frozenset(itertools.accumulate(value))


def _go_reached(context: dict, operation: str, index: int) -> bool:
    rows = context["reach_rows"].get(operation)
    if rows is None:
        rows = reach_indices(context["reach"].get(operation), context["reach_gaps"])
        context["reach_rows"][operation] = rows
    return index in rows


def _oracle_kills(context: dict, record: dict) -> list[dict]:
    """A mutant's recorded kills under the context's oracle (row ids are shared between oracles)."""
    return [kill for kill in record.get("kills") or [] if isinstance(kill, dict)
            and _kill_oracle(context["results_document"], record, kill) == context["oracle"]]


def _paired_control(context: dict, entry: dict) -> tuple[dict | None, str | None]:
    """(control entry or None, None), or (None, why the mutant cannot be credited)."""
    control_id = entry.get("control")
    if control_id is None:
        if allocates(entry):
            return None, "its replacement allocates but the plan pairs it with no control"
        return None, None
    control = context["manifest_ids"].get(control_id) if type(control_id) is int else None
    if (control is None or not is_control(control) or control.get("control_of") != entry.get("id")
            or any(control.get(field) != entry.get(field)
                   for field in ("file", "function", "site_kind", "site_line", "span", "span_sha256", "ops"))):
        return None, f"its control {control_id!r} is not a control planned at the same site"
    return control, None


def mutation_kill_problem(context: dict, sources: dict | None, operation: str, mutant: dict,
                          kill_ref: dict) -> str | None:
    """None when (operation, mutant, row) is a kill under the section 1 rules.

    With `sources` (the live view) the mutated span must also be current.
    """
    key, row = mutant["key"], kill_ref.get("row")
    if not isinstance(row, str):
        return f"malformed kill row {row!r}"
    if is_control(mutant):
        return f"mutant {key} is a control; controls are never credited"
    record = context["results"].get(key)
    if record is None:
        return f"mutant {key} is absent from the results"
    if record.get("state") != "killed":
        return f"mutant {key} is {record.get('state')!r}, not killed"
    kills = [kill for kill in record.get("kills") or [] if isinstance(kill, dict) and kill.get("row") == row
             and _kill_oracle(context["results_document"], record, kill) == context["oracle"]]
    if len(kills) != 1:
        return f"row {row!r} is not exactly one recorded {context['oracle']} kill of mutant {key}"
    kill = kills[0]
    digest = kill_ref.get("request_sha256")
    if not isinstance(digest, str) or kill.get("request_sha256") != digest:
        return f"row {row!r}: recorded request digest differs from the results"
    if context["requests"].get(row) != digest:
        return f"row {row!r}: request differs from the committed inventory"
    frozen = context["rows"].get(row)
    if frozen is None or frozen[1].get("request_sha256") != digest:
        return f"row {row!r}: not in the frozen native observations"
    index, native = frozen
    outcomes, digests = native.get("outcomes"), native.get("digests")
    if not isinstance(outcomes, dict) or not outcomes or any(value != "ok" for value in outcomes.values()):
        return f"row {row!r}: the native observation did not complete every stage"
    if not isinstance(digests, dict) or not digests or kill.get("native") != digests:
        return f"row {row!r}: kill native digests differ from the frozen native row"
    if kill.get("base") != digests:
        return f"row {row!r}: the unmutated Rust observation does not match native"
    mutated = kill.get("mutant")
    if (not isinstance(mutated, dict) or set(mutated) != set(digests)
            or not all(isinstance(value, str) for value in mutated.values())):
        return f"row {row!r}: the mutant did not complete every compared stage"
    differing = sorted(stage for stage in digests if mutated[stage] != digests[stage])
    if not differing:
        return f"row {row!r}: no compared stage differs from native"
    if sorted(kill.get("stages") or []) != differing:
        return f"row {row!r}: recorded differing stages disagree with the digests"
    entry = context["manifest"].get(key, {})
    control, reason = _paired_control(context, entry)
    if reason:
        return f"mutant {key}: {reason}"
    if control is not None:
        recorded = kill.get("control")
        if (not isinstance(recorded, dict) or set(recorded) != set(digests)
                or not all(isinstance(value, str) for value in recorded.values())):
            return f"row {row!r}: the kill records no completed run of the paired control"
        # The replaced value must show in a stage where the mutant differs
        # from native: a mutant-control difference only in stages where the
        # mutant equals native (the control's own side effect there) while
        # the native difference is shared with the control proves nothing.
        if not any(mutated[stage] != recorded[stage] for stage in differing):
            return (f"row {row!r}: the mutant equals its control in every stage where it differs from native, "
                    "so only the replacement's side effects (not the replaced value) moved the digests")
    if not isinstance(kill.get("reach"), dict) or kill["reach"].get("rust") is not True:
        return f"row {row!r}: the results do not confirm the mutated site executed"
    site_operations = set(mutant.get("ops") or []) | set(entry.get("ops") or [])
    if context["oracle"] in MUTATION_SINGLE_OPERATION_ORACLES and len(site_operations) > 1:
        return (f"row {row!r}: a {context['oracle']} row is a whole program, so a kill of a site that carries "
                f"markers for {len(site_operations)} operations credits none of them (not_credited_multi_op)")
    if operation in context["unstable"]:
        return f"row {row!r}: Go reach of {operation} depends on pool or memo state, so it never witnesses"
    if not _go_reached(context, operation, index):
        return f"row {row!r}: the pinned Go operation {operation} was not entered on this row"
    if sources is None:
        return mutation_site_static_problem(mutant, operation)
    return mutation_site_problem(mutant, operation, sources)


def unreached_reason(context: dict, home: dict) -> str:
    """The not_claimed text for a home no traced oracle executes; it names the oracles."""
    return (f"{MUTATION_UNREACHED_PHRASE} {', '.join(context['campaigns'])}: the full traces of these oracles "
            f"executed none of its {len(home['mutants'])} mutant site(s) on any row, in production or "
            "observation stages, and the confirmation receipt re-traces it")


def _home_drift(sources: dict, operation: str, homes: list[dict]) -> list[str]:
    """Live check: the operation's current marker sites are exactly the plan's homes."""
    functions, markers = marker_homes(sources, operation)
    planned_functions = {(home["file"], home["function"]) for home in homes if home["site_kind"] in _FUNCTION_SITES}
    planned_markers = Counter(home["file"] for home in homes if home["site_kind"] not in _FUNCTION_SITES)
    problems = [f"new Rust home {file}:{name} since the plan" for file, name in sorted(functions - planned_functions)]
    problems += [f"Rust home {file}:{name} no longer carries the marker" for file, name in
                 sorted(planned_functions - functions)]
    problems += [f"{file} now has {markers[file]} arm or statement marker(s) of the operation, the plan {planned_markers[file]}"
                 for file in sorted(set(markers) | set(planned_markers)) if markers[file] != planned_markers[file]]
    return problems


def mutation_home_problems(witness: dict, context: dict, sources: dict | None, operation: str,
                           killed_homes: set[str]) -> list[str]:
    """Every plan home of the operation is killed or proven unreached.

    Homes are every marker site the plan records (function, macro function,
    match arm, statement). A home not killed must be listed under not_claimed
    with a reason that says it has no production or observation reach on
    every traced oracle, carry at least one mutant, and have every mutant's
    Rust reach measured, and empty, in production and observation stages of
    every oracle campaign of the results. A home with no mutant or unmeasured
    reach keeps the operation pending. The live view also requires excused
    homes' spans to be current and the current marker sites to be exactly the
    plan's.
    """
    if context["homes"] is None:
        return ["the mutation manifest records no Rust homes, so the several-homes rule cannot be checked"]
    homes = context["homes"].get(operation) or []
    if not homes:
        return [f"the mutation manifest records no Rust home of {operation}"]
    identities = [home["id"] for home in homes]
    if len(set(identities)) != len(identities):
        return ["the mutation manifest lists two Rust homes of this operation under one identity"]
    not_claimed = witness.get("not_claimed") or {}
    problems = []
    for home in homes:
        if home["id"] in killed_homes:
            continue
        reason = not_claimed.get(home["id"]) if isinstance(not_claimed, dict) else None
        if not isinstance(reason, str) or not reason.strip():
            problems.append(f"Rust home {home['id']} is neither killed nor listed under not_claimed")
        elif not home["mutants"]:
            problems.append(f"Rust home {home['id']} has no mutant, so its reach was never measured")
        elif True in home["reach"]:
            problems.append(f"Rust home {home['id']} is reached by a driver but not killed")
        elif None in home["reach"]:
            problems.append(f"Rust home {home['id']} was not measured in every traced oracle "
                            f"({', '.join(context['campaigns'])}), in production and observation stages")
        elif any(oracle not in reason for oracle in context["campaigns"]):
            problems.append(f"the not_claimed reason for {home['id']} does not name every traced oracle "
                            f"({', '.join(context['campaigns'])})")
        elif MUTATION_UNREACHED_PHRASE not in reason:
            problems.append(f"the not_claimed reason for {home['id']} does not say "
                            f"'{MUTATION_UNREACHED_PHRASE}' the traced oracles")
        elif sources is not None:
            drift = next((problem for key in home["mutants"]
                          for problem in [mutation_site_problem(context["manifest"][key], operation, sources)]
                          if problem), None)
            if drift:
                problems.append(f"excused Rust home {home['id']}: {drift}")
    if sources is not None:
        problems += _home_drift(sources, operation, homes)
    return problems


def mutation_declaration_problems(witness: dict, known: set[str] | None = None) -> list[str]:
    """Structural rules for a declared mutation_kill witness (not bindings)."""
    identity = witness.get("id", "<unnamed>")
    problems = []
    operations = witness.get("operations")
    if not isinstance(operations, list) or not operations or not all(isinstance(op, str) for op in operations):
        problems.append(f"{identity}: a mutation witness must claim a nonempty operation list")
        operations = []
    elif len(set(operations)) != len(operations):
        problems.append(f"{identity}: duplicated claimed operation")
    if known is not None:
        problems.extend(f"{identity}: claims {op}, which is not an operation in the frozen scope"
                        for op in operations if op not in known)
    if witness.get("oracle") not in MUTATION_ORACLES:
        problems.append(f"{identity}: unknown mutation oracle {witness.get('oracle')!r}")
    artifact = witness.get("artifact")
    if not isinstance(artifact, str) or not artifact.startswith(MUTATION_DIRECTORY + "/"):
        problems.append(f"{identity}: a mutation witness's artifact must be a committed {MUTATION_DIRECTORY} results file")
    if not isinstance(witness.get("mutation_gate"), str) or not witness["mutation_gate"].strip():
        problems.append(f"{identity}: a mutation witness must name its mutation gate command")
    not_claimed = witness.get("not_claimed", {})
    if not isinstance(not_claimed, dict) or not all(
            isinstance(key, str) and isinstance(value, str) and value.strip() for key, value in not_claimed.items()):
        problems.append(f"{identity}: not_claimed must map each home or operation to a reason")
    mutants = witness.get("mutants")
    if not isinstance(mutants, list) or not mutants:
        return problems + [f"{identity}: a mutation witness must list its claimed mutants"]
    keys = set()
    for mutant in mutants:
        if not isinstance(mutant, dict) or set(mutant) != set(MUTANT_FIELDS):
            problems.append(f"{identity}: claimed mutant must carry exactly {', '.join(MUTANT_FIELDS)}")
            continue
        span, ops, markers = mutant["span"], mutant["ops"], mutant["markers"]
        if (not all(isinstance(mutant[key], str) for key in ("key", "op", "file", "function", "operator"))
                or mutant["site_kind"] not in MUTATION_SITE_KINDS
                or type(mutant["site_line"]) is not int or mutant["site_line"] < 1
                or not isinstance(ops, list) or not ops or ops[0] != mutant["op"]
                or not all(isinstance(op, str) for op in ops) or len(set(ops)) != len(ops)
                or not isinstance(markers, dict) or set(markers) != set(ops)
                or not all(type(line) is int and line >= 1 for line in markers.values())
                or not isinstance(span, list) or len(span) != 2 or not all(type(value) is int for value in span)
                or not 1 <= span[0] <= span[1]
                or not isinstance(mutant["span_sha256"], str) or not _HEX.fullmatch(mutant["span_sha256"])
                or not (mutant["control"] is None or (type(mutant["control"]) is int and mutant["control"] >= 1))):
            problems.append(f"{identity}: malformed claimed mutant {mutant.get('key')!r}")
            continue
        if mutant["operator"] == MUTATION_CONTROL:
            problems.append(f"{identity}: claimed mutant {mutant['key']} is a control; controls are never credited")
        if not _CRATE_SOURCE.fullmatch(mutant["file"]):
            problems.append(f"{identity}: mutant {mutant['key']} is not in production source under crates/*/src")
        if mutant["key"] in keys:
            problems.append(f"{identity}: duplicated claimed mutant {mutant['key']}")
        keys.add(mutant["key"])
    evidence = witness.get("mutation_evidence")
    if evidence is not None:
        kills = evidence.get("kills") if isinstance(evidence, dict) else None
        pairs = [(kill.get("op"), kill.get("key"), kill.get("row")) for kill in kills or [] if isinstance(kill, dict)]
        if not isinstance(kills, list) or len(pairs) != len(kills) or len(set(pairs)) != len(pairs):
            problems.append(f"{identity}: recorded mutation evidence has a malformed or duplicated kill")
    return problems


def _mutation_record(witness: dict, root: Path, sources: dict | None,
                     scope_links: dict[str, set[str]] | None = None) -> dict:
    claimed = [op for op in witness.get("operations") or [] if isinstance(op, str)]

    def unbound(state: str, reason: str, inputs: dict | None = None) -> dict:
        return {"state": state, "operations": [], "stale_operations": {op: reason for op in claimed},
                "reason": reason, "inputs": inputs or {}}

    evidence = witness.get("mutation_evidence")
    if evidence is None:
        return unbound("unrecorded", "declared but never recorded; run phase1.py record-mutations")
    problems = mutation_declaration_problems(witness)
    if problems:
        return unbound("stale", problems[0])
    if not isinstance(evidence, dict) or evidence.get("result") != "killed":
        return unbound("stale", "recorded mutation evidence is not a killed result")
    if evidence.get("claims_sha256") != witness_claims_digest(witness):
        return unbound("stale", "the declaration changed after its evidence was recorded")
    context, reason = mutation_artifacts(witness.get("oracle"), witness.get("artifact"), root)
    if reason:
        return unbound("stale", reason)
    for field in MUTATION_EVIDENCE_FIELDS[1:]:
        if evidence.get(field) != context["digests"][field]:
            return unbound("stale", f"{field.removesuffix('_sha256')} artifact changed after the evidence was recorded",
                           context["inputs"])
    mutants = {mutant["key"]: mutant for mutant in witness["mutants"]}
    for key, mutant in mutants.items():
        if {field: context["manifest"].get(key, {}).get(field) for field in MUTANT_FIELDS} != mutant:
            return unbound("stale", f"claimed mutant {key} differs from the manifest", context["inputs"])
    kills: dict[str, list[dict]] = {}
    used = set()
    for kill in evidence.get("kills") or []:
        kills.setdefault(kill.get("op"), []).append(kill)
        used.add(kill.get("key"))
    if set(mutants) - used:
        return unbound("stale", f"claimed mutant {sorted(set(mutants) - used)[0]} has no recorded kill",
                       context["inputs"])
    credited, stale = [], {}
    for operation in claimed:
        problems, killed_homes = [], set()
        for kill in kills.get(operation, []):
            mutant = mutants.get(kill.get("key"))
            problem = ("the kill names an unclaimed mutant" if mutant is None
                       else mutation_kill_problem(context, sources, operation, mutant, kill))
            if problem:
                problems.append(f"{kill.get('key')}: {problem}")
            elif (operation, mutant["key"]) in context["home_of"]:
                killed_homes.add(context["home_of"][(operation, mutant["key"])])
        if not kills.get(operation):
            problems.append("no recorded kill")
        problems += mutation_home_problems(witness, context, sources, operation, killed_homes)
        if not problems and scope_links is not None and witness.get("id") not in scope_links.get(operation, ()):
            problems.append("the committed scope.json does not link it; a Rust span or home changed when it was "
                            "last written, or it predates this recording (phase1.py inventory --check names the cause)")
        if problems:
            stale[operation] = problems[0] + (f" (+{len(problems) - 1} more)" if len(problems) > 1 else "")
        else:
            credited.append(operation)
    return {"state": "bound", "operations": sorted(credited), "stale_operations": stale, "reason": None,
            "inputs": context["inputs"]}


def recorded_mutations(cases: dict, root: Path = ROOT, *, committed_scope: dict | None = None) -> dict[str, dict]:
    """Which claimed operations each mutation_kill witness still confers.

    Child-free: reads committed artifacts, the current oracle code and, without
    `committed_scope` (the live view), the current Rust sources. With
    `committed_scope` (the committed view) no Rust source is read, and an
    operation is credited only while that scope document still links it to
    the witness. Per witness: `state` is `bound`, `stale` (a declaration or
    artifact binding no longer holds, so it confers nothing) or `unrecorded`;
    `operations` are the credited ones; `stale_operations` gives the visible
    reason for each claimed operation that is not credited; `inputs` the
    artifact digests consumed.
    """
    witnesses = [w for w in cases.get("witnesses", []) if w.get("kind") == "mutation_kill"]
    if not witnesses:
        return {}
    if committed_scope is None:
        sources, links = mutation_sources(root), None
    else:
        sources = None
        links = {row["id"]: set(row.get("cases", [])) for row in committed_scope.get("operations", [])
                 if isinstance(row, dict) and "id" in row}
    return {witness.get("id"): _mutation_record(witness, root, sources, links) for witness in witnesses}


def _site_operations(mutant: dict, sources: dict) -> list[str]:
    """The planned operations whose markers still stand on the mutant's current span."""
    return [op for op in mutant["ops"] if mutation_site_problem(mutant, op, sources) is None]


def _complete_campaign(context: dict) -> None:
    """Refuse a campaign that did not run every planned mutant to a final state.

    Controls run only beside a kill of their mutant, so they are not planned
    mutants here and their records, if any, are ignored. A mutant that is not
    killed must have run to a final state in every traced oracle: one the row
    budget stopped (`budget`), one a traced oracle never ran (`not_run`, or no
    entry for that oracle) and one a traced oracle skipped (`--skip-killed`,
    recorded as `skipped_by`) with no kill elsewhere to justify the skip make
    the campaign partial, whatever the results' own `partial` flag says. A
    killed mutant is final wherever else it stopped: its kills stand on their
    own, and non-reach is read from the full-trace reach tables.
    """
    if context["results_document"].get("partial") is True:
        raise ValueError("a partial mutation campaign cannot record evidence; run every planned mutant")
    planned = {key for key, entry in context["manifest"].items() if not is_control(entry)}
    unrun = sorted(planned - set(context["results"]))
    if unrun:
        raise ValueError(f"{len(unrun)} planned mutant(s) have no result ({', '.join(unrun[:3])}); "
                         "a kill can only be recorded from a complete campaign")
    partial, unknown = [], []
    for key in sorted(planned):
        record = context["results"][key]
        state = record.get("state")
        runs = record.get("oracles") if isinstance(record.get("oracles"), dict) else {}
        if state in MUTATION_PARTIAL_STATES:
            partial.append(key)
        elif state not in MUTATION_STATES or not runs:
            unknown.append(key)
        elif state != "killed" and (record.get("skipped_by") or any(
                not isinstance(runs.get(oracle), dict) or runs[oracle].get("state") not in MUTATION_STATES
                for oracle in context["campaigns"])):
            partial.append(key)
    if partial:
        raise ValueError(f"{len(partial)} mutant(s) were not run to a final state in every traced oracle "
                         f"({', '.join(partial[:3])}): a campaign with a budget, not_run or unjustified skipped "
                         "mutant is partial")
    if unknown:
        raise ValueError(f"{len(unknown)} mutant(s) were not run to a final state ({', '.join(unknown[:3])})")


def _claimed_mutant(entry: dict) -> dict | None:
    mutant = {field: entry.get(field) for field in MUTANT_FIELDS}
    return mutant if not mutation_declaration_problems({"operations": [mutant["op"]], "oracle": "e1",
                                                        "artifact": MUTATION_RESULTS, "mutation_gate": "x",
                                                        "mutants": [mutant]}) else None


def mutation_evidence(witness: dict, root: Path = ROOT) -> dict:
    """Derive a declared witness's evidence from the committed artifacts, or refuse.

    Records every kill of a claimed mutant that qualifies for a claimed
    operation under the section 1 rules, then requires the result to bind with
    every claimed operation credited (live view): a claim the campaign does not
    support is refused here rather than recorded stale.
    """
    problems = mutation_declaration_problems(witness)
    if problems:
        raise ValueError("; ".join(problems[:5]))
    context, reason = mutation_artifacts(witness["oracle"], witness["artifact"], root)
    if reason:
        raise ValueError(reason)
    _complete_campaign(context)
    sources = mutation_sources(root)
    claimed = set(witness["operations"])
    kills = []
    for mutant in witness["mutants"]:
        record = context["results"].get(mutant["key"])
        if record is None or record.get("state") != "killed":
            raise ValueError(f"claimed mutant {mutant['key']} is {(record or {}).get('state')!r}; only a killed "
                             "mutant can witness an operation")
        if {field: context["manifest"][mutant["key"]].get(field) for field in MUTANT_FIELDS} != mutant:
            raise ValueError(f"claimed mutant {mutant['key']} differs from the manifest")
        for operation in claimed.intersection(_site_operations(mutant, sources)):
            for kill in _oracle_kills(context, record):
                reference = {"key": mutant["key"], "row": kill.get("row"), "request_sha256": kill.get("request_sha256")}
                if mutation_kill_problem(context, sources, operation, mutant, reference) is None:
                    kills.append({"op": operation, **reference})
    missing = sorted(claimed - {kill["op"] for kill in kills})
    if missing:
        reason = next((problem for mutant in witness["mutants"]
                       for kill in _oracle_kills(context, context["results"][mutant["key"]])[:3]
                       for problem in [mutation_kill_problem(context, sources, missing[0], mutant, {
                           "key": mutant["key"], "row": kill.get("row"), "request_sha256": kill.get("request_sha256")})]
                       if problem), f"no claimed mutant records a qualifying {witness['oracle']} kill")
        raise ValueError(f"{len(missing)} claimed operation(s) are not witnessed by this campaign, "
                         f"e.g. {missing[0]}: {reason}")
    evidence = {"claims_sha256": witness_claims_digest(witness), **context["digests"],
                "kills": sorted(kills, key=lambda kill: (kill["op"], kill["key"], kill["row"])), "result": "killed"}
    record = _mutation_record({**witness, "mutation_evidence": evidence}, root, sources)
    if record["state"] != "bound":
        raise ValueError(record["reason"])
    if record["stale_operations"]:
        operation, why = sorted(record["stale_operations"].items())[0]
        raise ValueError(f"{len(record['stale_operations'])} claimed operation(s) are not witnessed by this "
                         f"campaign, e.g. {operation}: {why}")
    return evidence


def declare_mutation_witness(oracle: str, artifact: str, root: Path = ROOT) -> tuple[dict, dict[str, str]]:
    """Derive the declaration a campaign supports for one oracle.

    Claims each operation with a qualifying kill in every plan home, or with
    its other homes proven unreached in every traced oracle (every mutant
    measured with no Rust reach), which are then listed under not_claimed with
    a reason naming those oracles. Everything else stays unclaimed, with the
    reason returned for the owner's review; nothing is claimed on a home that
    has no mutant or whose reach was never measured.
    """
    context, reason = mutation_artifacts(oracle, artifact, root)
    if reason:
        raise ValueError(reason)
    _complete_campaign(context)
    if context["homes"] is None:
        raise ValueError("the mutation manifest records no Rust homes; re-plan with a splicer that writes `homes`")
    sources = mutation_sources(root)
    qualifying: dict[str, dict[str, dict]] = {}
    unclaimed: dict[str, str] = {}
    for key, entry in sorted(context["manifest"].items()):
        if is_control(entry):
            continue
        mutant = _claimed_mutant(entry)
        if mutant is None:
            unclaimed.setdefault(str(entry.get("op")), f"mutant {key} is malformed in the manifest")
            continue
        record = context["results"][key]
        if record.get("state") != "killed":
            unclaimed.setdefault(mutant["op"], f"mutant {key} is {record.get('state')}")
            continue
        operations = _site_operations(mutant, sources)
        if not operations:
            unclaimed.setdefault(mutant["op"], mutation_site_problem(mutant, mutant["op"], sources) or "site moved")
        for operation in operations:
            problem = f"no recorded {oracle} kill"
            for kill in _oracle_kills(context, record):
                reference = {"key": key, "row": kill.get("row"), "request_sha256": kill.get("request_sha256")}
                problem = mutation_kill_problem(context, sources, operation, mutant, reference)
                if problem is None:
                    qualifying.setdefault(operation, {})[key] = mutant
                    break
            if problem is not None:
                unclaimed.setdefault(operation, f"mutant {key}: {problem}")
    operations, mutants, not_claimed = [], {}, {}
    for operation, found in sorted(qualifying.items()):
        killed = {context["home_of"][(operation, key)] for key in found if (operation, key) in context["home_of"]}
        # Offer every other home as excused; the home rule then names why one
        # cannot be (reached, no mutant, unmeasured), or accepts them all.
        excused = {home["id"]: unreached_reason(context, home) for home in context["homes"].get(operation) or []
                   if home["id"] not in killed}
        problems = mutation_home_problems({"not_claimed": excused}, context, sources, operation, killed)
        if problems:
            unclaimed[operation] = problems[0]
            continue
        operations.append(operation)
        mutants.update(found)
        not_claimed.update(excused)
    for operation in operations:
        unclaimed.pop(operation, None)
    if not operations:
        raise ValueError(f"the campaign witnesses no {oracle} operation under the mutation witness rules")
    witness = {
        "id": f"mutation/{oracle}", "kind": "mutation_kill", "oracle": oracle, "artifact": artifact,
        "operations": operations, "mutation_gate": " ".join(MUTATION_CONFIRM_COMMAND),
        "mutants": [mutants[key] for key in sorted(mutants)], "not_claimed": dict(sorted(not_claimed.items())),
        "witnesses": (f"Mutation kills on the frozen {oracle} oracle. For each claimed operation, a mutant of "
                      "every Rust home a traced Phase 1 oracle executes (in production or observation stages) "
                      "changes a compared stage on a frozen row where the unmutated Rust equals the frozen native "
                      "observation and the pinned Go operation was entered; for a replacement that allocates, the "
                      "mutant also differs from its paired control in a stage where it differs from native. A "
                      "whole-program (syntax) kill never credits a site that carries markers for several "
                      "operations. Survivors, crashes and timeouts confer nothing and are kept in the results "
                      "artifact; they are never equivalent_rust claims."),
    }
    return witness, dict(sorted(unclaimed.items()))


def covering_witness_operations(cases: dict, root: Path = ROOT, *,
                                committed_scope: dict | None = None) -> list[tuple[str, list[str]]]:
    """(witness id, operations it confers) for every covering witness, in order.

    A rust_gated witness runs Rust through an audited gate and confers what it
    names. A mutation_kill witness confers only its currently bound operations,
    in the view `committed_scope` selects (see `recorded_mutations`).
    """
    mutations = recorded_mutations(cases, root, committed_scope=committed_scope)
    links = []
    for witness in cases.get("witnesses", []):
        kind = witness.get("kind")
        if kind == "rust_gated":
            links.append((witness["id"], list(witness.get("operations", []))))
        elif kind == "mutation_kill":
            links.append((witness["id"], list(mutations.get(witness.get("id"), {}).get("operations", []))))
    return links


def cases_by_operation() -> dict[str, list[str]]:
    """Invert the committed case manifest so each operation names its cases.

    Prepared cases, `rust_gated` witnesses and bound `mutation_kill` witnesses
    count as exact coverage links. A native authority does not: it supplies an
    expected value, not evidence that Rust reproduces it.
    """
    path = ROOT / "data/phase1/cases.json"
    if not path.is_file():
        return {}
    document = json.loads(path.read_text())
    inverted: dict[str, list[str]] = {}
    results = recorded_results(document)
    for case in document.get("cases", []):
        # A prepared case only covers an operation once it actually compares.
        # A case whose last result is `not_implemented` witnesses the gap; it
        # does not close it, and calling that covered would report 108 absent
        # implementations as done.
        # Absent means unrun, which is not evidence either. Only a recorded
        # match covers.
        if results[case["id"]] != "match":
            continue
        for operation in case.get("coverage_operations", case.get("operations", [])):
            inverted.setdefault(operation, []).append(case["id"])
    for identity, operations in covering_witness_operations(document):
        for operation in operations:
            inverted.setdefault(operation, []).append(identity)
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
    results = recorded_results(document)
    for case in document.get("cases", []):
        if results[case["id"]] != "not_implemented":
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
    return witness_problems_in(json.loads(path.read_text()))


def witness_problems_in(document: dict) -> list[str]:
    """Validate the witness records of a case manifest against the repository."""
    problems: list[str] = []
    seen: set[str] = set()
    known: set[str] | None = None
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
        if witness.get("kind") == "mutation_kill":
            # Declaration shape only. Whether the recorded kills still bind is
            # recorded_mutations' question, and a stale binding is pending
            # work, not a malformed manifest.
            if known is None:
                known = {row["id"] for row in json.loads((ROOT / "data/phase1/scope.json").read_text())["operations"]}
            problems.extend(mutation_declaration_problems(witness, known))
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
    # F4a. Every package no earlier step owns: the syntax front end, the
    # compiler (whose 340 operations F4a's survey classified but whose
    # non-syntactic ones stay here until a disposition decision moves them),
    # the reusable syntax services, and upstream's remaining test harness.
    "syntax": frozenset(
        "internal/" + name
        for name in (
            "ast", "scanner", "parser", "binder", "astnav", "evaluator", "debug",
            "compiler", "repo", "testrunner", "testutil", "testutil/harnessutil",
            "testutil/tsbaseline", "testutil/parsetestutil", "testutil/stringtestutil",
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


def prepared_links(cases: dict, step: str = "leaves", committed_scope: dict | None = None) -> dict[str, list[str]]:
    """Every operation a step's case or gated witness has actually run for.

    Preparation is not coverage: a case reporting `not_implemented` prepares its
    operation -- it runs and classifies the gap -- while covering nothing. The
    gate and the exemption validator must agree on that set, so both read it
    from here. Asking cases_by_operation() instead, which answers only for
    recorded matches, let an operation be prepared and exempted at once.

    Mutation witnesses count in the committed view when a scope document is
    given (rosters and preparation are checked against the committed scope),
    and in the live view otherwise.
    """
    links: dict[str, list[str]] = {}
    results = recorded_results(cases)
    for case in cases.get("cases", []):
        if case.get("family") not in STEP_FAMILIES[step] or results[case["id"]] not in PREPARING_RESULTS:
            continue
        for operation in case.get("operations", []):
            links.setdefault(operation, []).append(case["id"])
    for identity, operations in covering_witness_operations(cases, committed_scope=committed_scope):
        for operation in operations:
            links.setdefault(operation, []).append(identity)
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
            f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path} is absent; "
            f"the {step} roster has no reviewed ledger"
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
    prepared = prepared_links(cases, step, scope)
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
    "syntax": ("syntax",),
}


def leaf_preparation(scope: dict, cases: dict, step: str = "leaves", *, supplemental_prepared_cases=()) -> dict:
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
    results = recorded_results(cases)
    supplemental = set(supplemental_prepared_cases)
    by_id = {case["id"]: case for case in cases.get("cases", [])}
    for identity in supplemental:
        case = by_id.get(identity)
        if case is None or results.get(identity) not in ("native_unavailable", "not_applicable"):
            raise ValueError(f"{identity}: platform witness may only supplement a bound native-unavailable case")
        results[identity] = "match"
    for case in cases.get("cases", []):
        if case.get("family") not in families or results[case["id"]] not in PREPARING_RESULTS:
            continue
        for operation in case.get("operations", []):
            prepared.setdefault(operation, []).append(case["id"])
    witnessed: dict[str, list[str]] = {}
    # The committed view: preparation is judged against this scope document.
    for identity, operations in covering_witness_operations(cases, committed_scope=scope):
        for operation in operations:
            witnessed.setdefault(operation, []).append(identity)
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
    destinations = reviewed_destinations()
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
        destination = destinations.get(identity)
        if destination:
            disposition = "later_phase"
            basis = destination["reason"] + " " + destination["evidence"]
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
                "destination_phase": (
                    destination["destination_phase"] if destination
                    else None if disposition == "later_phase" else PHASE
                ),
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
        legacy_exclusion = (
            row.get("ledger_status") == "out-of-scope"
            and row.get("destination_phase") is None
            and row.get("roster", {}).get("state") in (
                "exempt:build_tooling", "exempt:build_variant", "exempt:later_step"
            )
        )
        if row.get("disposition") == "later_phase" and not legacy_exclusion and (
            type(row.get("destination_phase")) is not int
            or row["destination_phase"] not in range(2, 8)
        ):
            problems.append(f"{row.get('id')}: later_phase row must name a destination outside phase 1")
        if row.get("disposition") == "later_phase" and not legacy_exclusion and row.get("basis_kind") != "review":
            problems.append(f"{row.get('id')}: later_phase requires an explicit reviewed destination")
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


def reviewed_destinations() -> dict[str, dict]:
    """Apply exact accepted-plan boundaries, never a package/name heuristic.

    The old ``later_step`` roster category only exempts a preparation step.
    It does not remove an operation from Phase 1. This separate, pin-bound
    review names the destination of the partial compiler surface explicitly.
    Unknown IDs, a changed pinned source or duplicate decisions are errors.
    """
    path = ROOT / "data/phase1/coverage-review.json"
    if not path.is_file():
        return {}
    review = json.loads(path.read_text())
    pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
    if review.get("version") != 1 or review.get("pin") != pin or not review.get("authority"):
        raise ValueError("coverage-review.json has no current pinned review authority")
    known = {row["id"]: row for row in inventory()}
    decisions: dict[str, dict] = {}
    unused = review.get("reviewed_unused_compiler_operations", [])
    for row in [*review.get("reviewed_operation_destinations", []), *unused]:
        identity = row.get("operation")
        original = known.get(identity)
        if identity in decisions or original is None:
            raise ValueError(f"coverage-review.json: duplicate or unknown operation {identity}")
        if original["package"] != "internal/compiler":
            raise ValueError(f"{identity}: only the reviewed partial compiler scope can move")
        phase = row.get("destination_phase")
        if row in unused and phase is not None:
            raise ValueError(f"{identity}: unused operation cannot also name a destination")
        if row not in unused and (type(phase) is not int or phase not in range(2, 8)):
            raise ValueError(f"{identity}: invalid reviewed destination phase {phase!r}")
        if not row.get("reason") or not row.get("evidence"):
            raise ValueError(f"{identity}: reviewed destination lacks a reason or source evidence")
        source = ROOT / "upstream" / original["file"]
        if hashlib.sha256(source.read_bytes()).hexdigest() != row.get("go_source_sha256"):
            raise ValueError(f"{identity}: reviewed pinned source changed")
        decisions[identity] = row
    unresolved = review.get("unresolved_compiler_destinations", [])
    if len(set(unresolved)) != len(unresolved) or set(unresolved) & decisions.keys():
        raise ValueError("coverage-review.json: duplicate or simultaneously resolved destination")
    if any(identity not in known for identity in unresolved):
        raise ValueError("coverage-review.json: unknown unresolved operation")
    return {identity: row for identity, row in decisions.items() if "destination_phase" in row}
