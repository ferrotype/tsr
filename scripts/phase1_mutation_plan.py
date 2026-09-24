"""Phase 1 mutation witnesses: mutation sites, the mutant plan and the scratch schemata.

docs/PHASE1-mutation-witnesses.md is the evidence contract. This module does
the first three steps of it and nothing that runs an oracle:

- `sites` resolves every requested operation's Rust sites with the scope's own
  marker rules: `phase1_scope._PORT_ANNOTATION` attributes a `port:` marker to
  the function whose header follows it, and any other line-prefix marker (the
  xtask/`phase1_scope.unmapped_ids` rule) is a statement or match-arm marker.
  Files under crates/*/src only; test-only (`cfg(test)`) code is skipped.
- `plan` hands the sites to the `phase1_mutation_splicer` binary, which picks
  type-directed operators with syn, pairs every mutant whose replacement
  allocates (a `create_*`/`new_*` call) with a control that computes the same
  replacement and discards it, numbers the mutants, records every operation's
  `homes` (each production marker site: function, macro-body function, match
  arm or statement, with its mutants, or none and the reason), and writes
  plan.json. `check_plan` verifies those invariants on the written document.
- `schemata` syncs a scratch copy of the source tree (tracked and untracked,
  not ignored; `data` and `upstream` are symlinked, never copied), splices the
  plan into it, and touches every spliced file so cargo rebuilds.

The committed sources are never edited. A span digest (`span_sha256`) is the
sha256 of the exact lines a mutant's site covers; `span_intact` recomputes it
from current sources, relocating by the operation's marker, so a coverage
binding can tell an edit to the mutated span from an unrelated edit.
"""

from __future__ import annotations

import argparse
import bisect
import functools
import gzip
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase1_scope  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
from s08_oracle import canonical, digest  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
COVERAGE_REPORT = Path("data/phase1/coverage-report.json.gz")
ROOT_CAUSE = "operation_witness_missing"
# A recorded mutation claim that no longer binds (phase1_coverage).
STALE_CAUSE = "mutation_witness_stale"
SPLICER = "phase1_mutation_splicer"
SWITCH = Path("tools/phase1/mutation/switch")
DEFAULT_OUTPUT = Path("target/phase1-mutation")
WS_MARKER = ".phase1-mutation-ws.json"
SPLICE_REPORT = ".phase1-mutation-splice.json"
# Linked into the scratch copy, never copied: the pinned checkout and the data.
LINKED = ("data", "upstream")
# The scope's line-prefix marker rule (phase1_scope.unmapped_ids, xtask
# scan_markers): the scope's own constant where it names one, else the tuple
# unmapped_ids spells inline. A test pins the two together.
MARKER_PREFIXES = tuple(getattr(phase1_scope, "_MARKER_PREFIXES", ("/// port:", "//! port:", "// port:")))
# The crates the acceptance check compiles in a spliced copy, besides every
# crate the plan mutates.
CHECK_PACKAGES = ("phase1_mutation_driver", "phase1_syntax", "tsr_parser", "tsr_binder", "tsr_ast",
                  "tsr_compiler")


def default_ops(root: Path = ROOT) -> list[str]:
    """The operations the mutation witnesses target, stable across recording.

    Those whose coverage root cause is operation_witness_missing or
    mutation_witness_stale, plus every operation a mutation_kill witness of the
    report claims. A recorded witness takes its operations out of the first set;
    they stay planned, or re-planning would change the committed manifest that
    every witness binds.
    """
    report = strict_json_loads(gzip.decompress((Path(root) / COVERAGE_REPORT).read_bytes()))
    ops = {row["id"] for row in report["operations"] if row.get("root_cause") in (ROOT_CAUSE, STALE_CAUSE)}
    ops.update(op for witness in report.get("witnesses", []) if witness.get("kind") == "mutation_kill"
               for op in witness.get("claimed_operations", []))
    return sorted(ops)


def rust_sources(root: Path) -> list[Path]:
    """Every `.rs` file under crates/*/src/, in path order; a symlink is not a home."""
    return sorted(path for path in (Path(root) / "crates").glob("*/src/**/*.rs")
                  if path.is_file() and not path.is_symlink())


# ---------------------------------------------------------------------------
# Markers, with the scope's rules.
# ---------------------------------------------------------------------------

class _Lines:
    def __init__(self, text: str):
        self.starts = [0] + [match.end() for match in re.finditer("\n", text)]

    def of(self, offset: int) -> int:
        return bisect.bisect_right(self.starts, offset)


def markers(text: str) -> list[dict]:
    """Every `port:` marker in `text`, as the scope reads it.

    A marker `_PORT_ANNOTATION` matches is function-level: it names the `fn`
    whose header follows (`function`, header on `fn_line`). Every other
    line-prefix marker is a statement or match-arm marker.
    """
    lines = _Lines(text)
    function_level = {}
    for match in phase1_scope._PORT_ANNOTATION.finditer(text):
        function_level[(lines.of(match.start()), match.group(1))] = (match.group(2), lines.of(match.start(2)))
    found = [{"line": line, "op": op, "kind": "fn", "function": function, "fn_line": fn_line}
             for (line, op), (function, fn_line) in function_level.items()]
    for number, raw in enumerate(text.split("\n"), 1):
        trimmed = raw.lstrip()
        for prefix in MARKER_PREFIXES:
            if trimmed.startswith(prefix):
                op = trimmed.removeprefix(prefix).strip()
                if ":" in op and (number, op) not in function_level:
                    found.append({"line": number, "op": op, "kind": "statement"})
                break
    return sorted(found, key=lambda marker: (marker["line"], marker["op"]))


# ---------------------------------------------------------------------------
# Test-only code.
# ---------------------------------------------------------------------------

# Where a comment, string or character literal can start. Identifier
# characters before a quote make it part of an identifier-prefixed literal
# (`b"..."`, `br#"..."#`), which the alternatives spell out.
_LITERAL = re.compile(r"""
    (?P<line>//[^\n]*)
  | (?P<block>/\*)
  | (?<![A-Za-z0-9_])[bc]?r(?P<hashes>\#*)(?P<raw>")
  | (?P<string>(?<![A-Za-z0-9_])[bc]?"(?:[^"\\]|\\.)*")
  | (?P<char>(?<![A-Za-z0-9_])b?'(?:[^'\\\n]|\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.))')
""", re.VERBOSE | re.DOTALL)
_NOT_NEWLINE = re.compile(r"[^\n]")


@functools.lru_cache(maxsize=8)
def code_mask(text: str) -> str:
    """`text` with comments, strings and character literals blanked.

    Offsets and newlines are preserved, so braces and attributes found in the
    mask are real code at the same positions in `text`. A lifetime or label
    (`'a`) is code; a character literal (`'a'`, `'\\n'`) is not.
    """
    pieces, done, size = [], 0, len(text)
    for_search = 0
    while match := _LITERAL.search(text, for_search):
        start = match.start()
        if match.group("block"):
            depth, end = 1, match.end()
            while end < size and depth:
                if text.startswith("/*", end):
                    depth, end = depth + 1, end + 2
                elif text.startswith("*/", end):
                    depth, end = depth - 1, end + 2
                else:
                    end += 1
        elif match.group("raw"):
            close = text.find('"' + match.group("hashes"), match.end())
            end = size if close < 0 else close + 1 + len(match.group("hashes"))
        else:
            end = match.end()
        pieces.append(text[done:start])
        pieces.append(_NOT_NEWLINE.sub(" ", text[start:end]))
        done = for_search = end
    pieces.append(text[done:])
    return "".join(pieces)


def _balanced(mask: str, start: int) -> int:
    """The index just past the bracket group opening at `start`."""
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = []
    for at in range(start, len(mask)):
        c = mask[at]
        if c in pairs:
            stack.append(pairs[c])
        elif stack and c == stack[-1]:
            stack.pop()
            if not stack:
                return at + 1
    return len(mask)


def _split_top(text: str) -> list[str]:
    parts, depth, current = [], 0, ""
    for c in text:
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        if c == "," and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += c
    return parts + ([current] if current else [])


def test_only(predicate: str) -> bool:
    """A `cfg` predicate that compiles only under test: `test`, or `all(..)`
    with `test` among its operands. `any(test, feature = ..)` is kept."""
    predicate = re.sub(r"\s+", "", predicate)
    if predicate == "test":
        return True
    if predicate.startswith("all(") and predicate.endswith(")"):
        return "test" in _split_top(predicate[4:-1])
    return False


_ATTRIBUTE = re.compile(r"#\s*(!?)\s*\[")
_CFG = re.compile(r"\s*cfg\s*\(")


def _attributes(mask: str, start: int) -> tuple[list[tuple[bool, int, int]], int]:
    """Consecutive attributes from `start`: (inner, start, end) each, and the
    offset of the first non-attribute code after them."""
    found = []
    at = start
    while True:
        while at < len(mask) and mask[at].isspace():
            at += 1
        match = _ATTRIBUTE.match(mask, at)
        if not match:
            return found, at
        end = _balanced(mask, match.end() - 1)
        found.append((bool(match.group(1)), at, end))
        at = end


def _attribute_test_only(text: str, mask: str, start: int, end: int) -> bool:
    inner = mask[mask.index("[", start) + 1:end - 1]
    match = _CFG.match(inner)
    if match:
        predicate_start = mask.index("[", start) + 1 + match.end() - 1
        return test_only(text[predicate_start + 1:_balanced(mask, predicate_start) - 1])
    return re.fullmatch(r"\s*test\s*", inner) is not None


def _item_end(mask: str, start: int) -> int:
    """The end of the item or statement starting at `start`: its first `;` at
    depth 0, or the close of its first brace block."""
    depth = 0
    for at in range(start, len(mask)):
        c = mask[at]
        if c in "([":
            depth += 1
        elif c in ")]":
            depth -= 1
        elif c == "{":
            if depth == 0:
                return _balanced(mask, at)
            depth += 1
        elif c == "}":
            depth -= 1
            if depth < 0:
                return at
        elif c == ";" and depth == 0:
            return at + 1
    return len(mask)


def test_regions(text: str) -> list[tuple[int, int]]:
    """Line ranges (inclusive) of test-only items in one file."""
    mask = code_mask(text)
    lines = _Lines(text)
    regions = []
    for match in _ATTRIBUTE.finditer(mask):
        end = _balanced(mask, match.end() - 1)
        if not _attribute_test_only(text, mask, match.start(), end):
            continue
        if match.group(1):
            return [(1, lines.of(max(len(text) - 1, 0)))]
        _, code = _attributes(mask, end)
        regions.append((lines.of(match.start()), lines.of(max(_item_end(mask, code) - 1, code))))
    return regions


_MOD_DECLARATION = re.compile(r"(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")


def _declared_modules(text: str) -> list[tuple[str, str | None, bool, int]]:
    """`mod name;` declarations: (name, #[path], test-only, line)."""
    mask = code_mask(text)
    lines = _Lines(text)
    found = []
    attribute_ends = {}
    for match in _ATTRIBUTE.finditer(mask):
        if not match.group(1):
            attribute_ends[_balanced(mask, match.end() - 1)] = match.start()
    for match in _MOD_DECLARATION.finditer(mask):
        if match.start() and (mask[match.start() - 1].isalnum() or mask[match.start() - 1] == "_"):
            continue
        # Walk back over the attributes directly above the declaration.
        path, test, at = None, False, match.start()
        while True:
            back = at
            while back and mask[back - 1].isspace():
                back -= 1
            if back not in attribute_ends:
                break
            start = attribute_ends[back]
            test = test or _attribute_test_only(text, mask, start, back)
            attribute = text[start:back]
            path_match = re.fullmatch(r'#\s*\[\s*path\s*=\s*"([^"]*)"\s*\]', attribute)
            if path_match:
                path = path_match.group(1)
            at = start
        found.append((match.group(1), path, test, lines.of(match.start())))
    return found


def test_module_files(root: Path) -> set[str]:
    """Source files compiled only under test: declared by `#[cfg(test)] mod x;`
    (or from inside test-only code), and every module those declare in turn.
    A declaration inside an inline `mod m { .. }` would be resolved as if at the
    file's top level; no current declaration sits in one."""
    root = Path(root)
    children: dict[str, list[tuple[str, bool]]] = {}
    for path in rust_sources(root):
        rel = path.relative_to(root).as_posix()
        text = path.read_text(errors="replace")
        regions = test_regions(text)
        base = path.parent if path.name in ("lib.rs", "main.rs", "mod.rs") else path.parent / path.stem
        for name, explicit, test, line in _declared_modules(text):
            if explicit is not None:
                candidates = [path.parent / explicit]
            else:
                candidates = [base / f"{name}.rs", base / name / "mod.rs"]
            for candidate in candidates:
                if candidate.is_file():
                    child = candidate.resolve().relative_to(root.resolve()).as_posix()
                    inside = any(start <= line <= end for start, end in regions)
                    children.setdefault(rel, []).append((child, test or inside))
                    break
    tests = {child for declared in children.values() for child, test in declared if test}
    pending = list(tests)
    while pending:
        for child, _ in children.get(pending.pop(), []):
            if child not in tests:
                tests.add(child)
                pending.append(child)
    return tests


# ---------------------------------------------------------------------------
# Sites.
# ---------------------------------------------------------------------------

def _resolve(root: Path, ops) -> tuple[list[dict], list[dict]]:
    root = Path(root)
    wanted = set(ops)
    tests = test_module_files(root)
    found, skipped = [], {}
    for path in rust_sources(root):
        rel = path.relative_to(root).as_posix()
        text = path.read_text(errors="replace")
        matches = [marker for marker in markers(text) if marker["op"] in wanted]
        if not matches:
            continue
        regions = [] if rel in tests else test_regions(text)
        for marker in matches:
            probe = marker.get("fn_line", marker["line"])
            if rel in tests or any(start <= probe <= end for start, end in regions):
                skipped.setdefault(marker["op"], []).append(f"{rel}:{marker['line']}")
                continue
            site = {"op": marker["op"], "file": rel, "marker_line": marker["line"], "marker_kind": marker["kind"]}
            if marker["kind"] == "fn":
                site.update(function=marker["function"], fn_line=marker["fn_line"])
            found.append(site)
    found.sort(key=lambda site: (site["file"], site["marker_line"], site["op"]))
    sited = {site["op"] for site in found}
    unsited = []
    for op in sorted(wanted - sited):
        if op in skipped:
            reason = "cfg_test_only: every port: marker is in test-only code (" + ", ".join(skipped[op]) + ")"
        else:
            reason = "no_marker: no port: marker under crates/*/src"
        unsited.append({"op": op, "reason": reason})
    return found, unsited


def sites(root: Path = ROOT, ops=None) -> list[dict]:
    """[{op, file, marker_line, marker_kind[, function, fn_line]}] for `ops`."""
    return _resolve(root, default_ops(root) if ops is None else ops)[0]


# ---------------------------------------------------------------------------
# Span digests and keys (the splicer defines the same).
# ---------------------------------------------------------------------------

def span_bytes(data: bytes, start: int, end: int) -> bytes | None:
    """Lines `start..=end` (1-based, split on `\\n` only) with their newlines."""
    lines = data.split(b"\n")
    count = len(lines) - 1 if data.endswith(b"\n") or not data else len(lines)
    if start < 1 or start > end or end > count:
        return None
    return b"\n".join(lines[start - 1:end]) + (b"\n" if end < len(lines) else b"")


def span_sha256(data: bytes, start: int, end: int) -> str | None:
    span = span_bytes(data, start, end)
    return None if span is None else hashlib.sha256(span).hexdigest()


def mutant_key(op: str, file: str, function: str, site_line: int, operator: str, span_digest: str) -> str:
    return hashlib.sha256(f"{op}|{file}|{function}|{site_line}|{operator}|{span_digest}".encode()).hexdigest()[:16]


def span_intact(root: Path, mutant: dict) -> bool:
    """Whether a planned mutant's span is unchanged in the sources under `root`.

    The span is found again through its operations' markers, at the same offset
    from the marker as when planned, so code moving above it does not count as
    an edit; any change inside the span does.
    """
    path = Path(root) / mutant["file"]
    if not path.is_file():
        return False
    data = path.read_bytes()
    start, end = mutant["span"]
    current = markers(data.decode(errors="replace"))
    for op, planned in sorted(mutant["markers"].items()):
        for marker in current:
            if marker["op"] == op:
                shift = marker["line"] - planned
                if span_sha256(data, start + shift, end + shift) == mutant["span_sha256"]:
                    return True
    return False


# ---------------------------------------------------------------------------
# The source tree.
# ---------------------------------------------------------------------------

def _git(root: Path, *args: str) -> bytes:
    return subprocess.run(["git", "-C", str(root), *args], check=True, stdout=subprocess.PIPE).stdout


def source_files(root: Path = ROOT) -> list[str]:
    """Tracked and untracked-but-not-ignored files that exist, as posix paths."""
    listed = _git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard").split(b"\0")
    files = set()
    for raw in listed:
        if not raw:
            continue
        rel = raw.decode()
        path = Path(root) / rel
        if path.is_symlink() or path.is_file():
            files.add(rel)
    return sorted(files)


def _gitlinks(root: Path) -> dict[str, str]:
    links = {}
    for row in _git(root, "ls-files", "-s", "-z").split(b"\0"):
        if row.startswith(b"160000 "):
            meta, path = row.split(b"\t", 1)
            links[path.decode()] = meta.split()[1].decode()
    return links


def source_tree_sha(root: Path = ROOT, files=None) -> str:
    """The git tree id of the working tree's source files, computed without
    writing any object: equal to `git rev-parse HEAD^{tree}` in a clean checkout
    with no untracked files."""
    root = Path(root)
    tree: dict = {}

    def place(rel: str, entry: tuple[str, bytes]) -> None:
        node = tree
        parts = rel.split("/")
        for part in parts[:-1]:
            node = node.setdefault(part, {})
        node[parts[-1]] = entry

    for rel in source_files(root) if files is None else files:
        path = root / rel
        if path.is_symlink():
            mode, data = "120000", os.readlink(path).encode()
        else:
            data = path.read_bytes()
            mode = "100755" if os.stat(path).st_mode & 0o100 else "100644"
        place(rel, (mode, hashlib.sha1(b"blob %d\0" % len(data) + data).digest()))
    for rel, sha in _gitlinks(root).items():
        place(rel, ("160000", bytes.fromhex(sha)))

    def write(node: dict) -> bytes:
        entries = []
        for name, value in node.items():
            if isinstance(value, dict):
                entries.append((name.encode() + b"/", b"40000", name.encode(), write(value)))
            else:
                entries.append((name.encode(), value[0].encode(), name.encode(), value[1]))
        body = b"".join(mode + b" " + name + b"\0" + sha for _, mode, name, sha in sorted(entries))
        return hashlib.sha1(b"tree %d\0" % len(body) + body).digest()

    return write(tree).hex()


# ---------------------------------------------------------------------------
# Plan and schemata.
# ---------------------------------------------------------------------------

def _target_dir(root: Path) -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR")
    return Path(configured) if configured else Path(root) / "target"


def build_splicer(root: Path = ROOT) -> Path:
    """Builds the splicer from `root` and returns the binary."""
    subprocess.run(["cargo", "build", "--offline", "--locked", "-p", SPLICER, "--bin", SPLICER],
                   cwd=root, check=True)
    binary = _target_dir(root) / "debug" / SPLICER
    if not binary.is_file():
        raise FileNotFoundError(binary)
    return binary


def plan(root: Path = ROOT, ops=None, out: Path | None = None, splicer: Path | None = None) -> dict:
    """Resolves the sites of `ops` (default: the witness-missing operations),
    plans their mutants and writes plan.json (sites.json beside it)."""
    root = Path(root).resolve()
    ops = sorted(set(default_ops(root) if ops is None else ops))
    out = Path(out) if out is not None else root / DEFAULT_OUTPUT / "plan.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    found, unsited = _resolve(root, ops)
    sites_path = out.with_name("sites.json")
    sites_path.write_bytes(canonical({"version": 1, "ops": ops, "sites": found, "unsited": unsited}) + b"\n")
    binary = splicer or build_splicer(root)
    subprocess.run([str(binary), "plan", "--root", str(root), "--sites", str(sites_path), "--out", str(out),
                    "--root-tree", source_tree_sha(root)], check=True)
    document = strict_json_loads(out.read_bytes())
    check_plan(document, ops)
    return document


# The allocation rule `phase1_scope.allocates` applies to plan entries; the
# splicer implements the same rule, and a test pins the two together.
_ALLOCATING = re.compile(r"\b(?:create|new)_\w*\s*\(")
# The fields a control copies from the mutant it controls.
CONTROL_FIELDS = ("op", "ops", "file", "function", "site_kind", "site_line", "markers", "span", "span_sha256",
                  "crate", "hit_fn", "return_category")
HOME_FIELDS = ("file", "function", "site_kind", "site_line", "span", "span_sha256", "marker_line", "mutants",
               "reason")
SITE_KINDS = ("fn", "macro_fn", "arm", "stmt")
# `hit_parser` stays live while observing. Go counts observation-time entries
# only for these operations (e1 lazy JSDoc parsing; phase1_mutation_go
# `observe_counts`), so only a site in these sources whose every operation is
# one of them may call it; the splicer's `hit_fn` applies the same rule.
PARSER_SOURCES = "crates/tsr_parser/src/"
PARSER_OPERATIONS = "tsc/internal/parser/"
# Every use of the switch crate in an inserted text, with the id when the use
# is a plain `name(id)` call.
_SWITCH_USE = re.compile(r"::phase1_mutants::(\w+)(?:\((\d+)\))?")


def is_control(mutant: dict) -> bool:
    return mutant.get("operator") == "control" or "control_of" in mutant


def hit_function(mutant: dict) -> str:
    """The switch function a plan entry's guards must call: `hit_parser` for a
    tsr_parser site whose every operation is an internal/parser function,
    `hit` (production only) for every other site."""
    ops = mutant.get("ops") or []
    parser = mutant["file"].startswith(PARSER_SOURCES) and all(op.startswith(PARSER_OPERATIONS) for op in ops)
    return "hit_parser" if ops and parser else "hit"


def allocates(mutant: dict) -> bool:
    """Whether a plan entry's replacement calls a `create_*`/`new_*` constructor."""
    texts = [mutant.get("operator")] + [insert.get("text") for insert in mutant.get("insert") or []]
    return any(isinstance(text, str) and _ALLOCATING.search(text) for text in texts)


def check_plan(document: dict, ops=None) -> None:
    """Raises ValueError unless the plan's controls and homes are consistent.

    Every entry calls the switch function its site allows (`hit_function`), and
    every use of the switch crate it inserts is a call of that function with its
    own id; every
    mutant whose replacement allocates names a control, and only those do;
    a control sits at its mutant's site with the same span and is never a home
    mutant; every requested operation has a home list; every home mutant is a
    planned non-control mutant of the operation, and every non-control mutant
    stands in exactly one home of each operation on its site; a home with no
    mutant says why.
    """
    by_id = {mutant["id"]: mutant for mutant in document["mutants"]}
    if len(by_id) != len(document["mutants"]) or sorted(by_id) != list(range(1, len(by_id) + 1)):
        raise ValueError("mutant ids are not 1..n, each once")
    by_key = {mutant["key"]: mutant for mutant in document["mutants"]}
    if len(by_key) != len(by_id):
        raise ValueError("duplicated mutant key")
    for mutant in document["mutants"]:
        expected = hit_function(mutant)
        if mutant.get("hit_fn") != expected:
            raise ValueError(f"mutant {mutant['id']} calls {mutant.get('hit_fn')}, but its site allows only "
                             f"{expected}")
        calls = [use.groups() for insert in mutant.get("insert") or [] for use in _SWITCH_USE.finditer(insert["text"])]
        if not calls or any(call != (expected, str(mutant["id"])) for call in calls):
            raise ValueError(f"mutant {mutant['id']}: its inserted switch calls {calls} are not "
                             f"{expected}({mutant['id']})")
        if is_control(mutant):
            paired = by_id.get(mutant.get("control_of"))
            if (mutant.get("operator") != "control" or paired is None or is_control(paired)
                    or paired.get("control") != mutant["id"]
                    or any(mutant.get(field) != paired.get(field) for field in CONTROL_FIELDS)):
                raise ValueError(f"control {mutant['id']} is not paired with a mutant at its site")
        elif allocates(mutant) != (mutant.get("control") is not None):
            raise ValueError(f"mutant {mutant['id']} ({mutant['operator']}): allocation and control disagree")
    homes = document.get("homes")
    if not isinstance(homes, dict):
        raise ValueError("the plan records no homes")
    if ops is not None and (missing := sorted(set(ops) - set(homes))):
        raise ValueError(f"{len(missing)} requested operations have no home list, first {missing[:3]}")
    placed = {}
    for op, rows in homes.items():
        for home in rows:
            if set(home) != set(HOME_FIELDS) or home["site_kind"] not in SITE_KINDS:
                raise ValueError(f"malformed home of {op}: {home}")
            if not home["mutants"] and not home["reason"]:
                raise ValueError(f"home {home['file']}:{home['site_line']} of {op} has no mutant and no reason")
            for key in home["mutants"]:
                mutant = by_key.get(key)
                if mutant is None or is_control(mutant) or op not in mutant["ops"]:
                    raise ValueError(f"home of {op} names {key}, which is not a planned mutant of it")
                placed[(op, key)] = placed.get((op, key), 0) + 1
    for mutant in document["mutants"]:
        if not is_control(mutant):
            for op in mutant["ops"]:
                if placed.get((op, mutant["key"])) != 1:
                    raise ValueError(f"mutant {mutant['key']} is not in exactly one home of {op}")
    accounted = {op for mutant in document["mutants"] for op in mutant["ops"]}
    accounted |= {entry["op"] for entry in document["unsupported"]}
    if ops is not None and (missing := sorted(set(ops) - accounted)):
        raise ValueError(f"plan leaves {len(missing)} operations unaccounted for: {missing[:5]}")


def _sync(root: Path, ws: Path) -> list[str]:
    """Makes `ws` hold exactly the source files of `root`. Unchanged files keep
    their mtime (so cargo can reuse work); changed ones are written fresh."""
    files = [rel for rel in source_files(root) if rel.split("/", 1)[0] not in LINKED]
    keep = set(files)
    for rel in files:
        source, target = root / rel, ws / rel
        if source.is_symlink():
            link = os.readlink(source)
            if not (target.is_symlink() and os.readlink(target) == link):
                if target.exists() or target.is_symlink():
                    target.unlink()
                target.parent.mkdir(parents=True, exist_ok=True)
                os.symlink(link, target)
            continue
        data = source.read_bytes()
        if target.is_symlink():
            target.unlink()
        if not (target.is_file() and target.read_bytes() == data):
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        shutil.copymode(source, target)
    top_level = {"target", WS_MARKER, SPLICE_REPORT, *LINKED}
    directories = []
    for directory, subdirectories, names in os.walk(ws, topdown=True):
        here = Path(directory)
        if here == ws:
            subdirectories[:] = [name for name in subdirectories if name not in top_level]
            names = [name for name in names if name not in top_level]
        else:
            directories.append(here)
        for name in names:
            rel = (here / name).relative_to(ws).as_posix()
            if rel not in keep:
                (here / name).unlink()
    for here in sorted(directories, key=lambda path: -len(path.parts)):
        if not here.is_symlink() and not any(here.iterdir()):
            here.rmdir()
    for name in LINKED:
        link = ws / name
        if not (link.is_symlink() and Path(os.readlink(link)) == root / name):
            if link.is_symlink() or link.is_file():
                link.unlink()
            elif link.exists():
                raise ValueError(f"{link} exists and is not a link")
            os.symlink(root / name, link)
    return files


def schemata(root: Path = ROOT, plan_path: Path | None = None, ws: Path | None = None,
             splicer: Path | None = None) -> Path:
    """Syncs a scratch copy of `root` into `ws`, splices `plan_path` into it and
    returns `ws`. Refuses a `ws` that is not empty and not one of ours."""
    root = Path(root).resolve()
    plan_path = (Path(plan_path) if plan_path is not None else root / DEFAULT_OUTPUT / "plan.json").resolve()
    ws = (Path(ws) if ws is not None else root / DEFAULT_OUTPUT / "ws").resolve()
    if ws == root or root.is_relative_to(ws):
        raise ValueError(f"refusing to use {ws} as a scratch workspace")
    if ws.exists() and any(ws.iterdir()) and not (ws / WS_MARKER).is_file():
        raise ValueError(f"{ws} is not empty and is not a mutation workspace")
    ws.mkdir(parents=True, exist_ok=True)
    (ws / WS_MARKER).write_bytes(canonical({"version": 1, "root": str(root), "state": "syncing"}) + b"\n")
    _sync(root, ws)
    if not (ws / SWITCH / "Cargo.toml").is_file():
        raise FileNotFoundError(ws / SWITCH / "Cargo.toml")
    binary = splicer or build_splicer(root)
    report_path = ws / SPLICE_REPORT
    subprocess.run([str(binary), "splice", "--root", str(ws), "--plan", str(plan_path), "--switch",
                    str(ws / SWITCH), "--report", str(report_path)], check=True)
    report = strict_json_loads(report_path.read_bytes())
    now = time.time()
    for rel in [entry["file"] for entry in report["files"]] + report["manifests"]:
        os.utime(ws / rel, (now, now))
    plan_document = strict_json_loads(plan_path.read_bytes())
    current_tree = source_tree_sha(root)
    if plan_document["root_tree"] != current_tree:
        print(f"note: sources changed since planning (plan {plan_document['root_tree']}, now {current_tree}); "
              "every mutated span was verified", file=sys.stderr)
    (ws / WS_MARKER).write_bytes(canonical({
        "version": 1, "root": str(root), "state": "spliced", "plan": str(plan_path),
        "plan_sha256": digest(plan_path.read_bytes()), "plan_root_tree": plan_document["root_tree"],
        "root_tree": current_tree, "splice_report_sha256": digest(report_path.read_bytes()),
    }) + b"\n")
    return ws


def check(ws: Path, plan_document: dict, packages=CHECK_PACKAGES) -> None:
    """`cargo check --offline` of the acceptance packages and every mutated crate in `ws`."""
    selected = sorted(set(packages) | {mutant["crate"] for mutant in plan_document["mutants"]})
    args = ["cargo", "check", "--offline"]
    for package in selected:
        args += ["-p", package]
    subprocess.run(args, cwd=ws, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("sites", "plan"):
        command = commands.add_parser(name)
        command.add_argument("--ops", type=Path, help="JSON list of operation ids (default: the witness-missing ones)")
        command.add_argument("--out", type=Path, help="output file")
    command = commands.add_parser("schemata")
    command.add_argument("--plan", type=Path, default=ROOT / DEFAULT_OUTPUT / "plan.json")
    command.add_argument("--ws", type=Path, default=ROOT / DEFAULT_OUTPUT / "ws")
    command.add_argument("--check", action="store_true", help="cargo check the spliced copy afterwards")
    args = parser.parse_args()
    if args.command == "schemata":
        ws = schemata(ROOT, args.plan, args.ws)
        if args.check:
            check(ws, strict_json_loads(args.plan.read_bytes()))
        print(json.dumps({"ws": str(ws)}))
        return
    ops = strict_json_loads(args.ops.read_bytes()) if args.ops else None
    if args.command == "sites":
        found, unsited = _resolve(ROOT, default_ops(ROOT) if ops is None else ops)
        document = canonical({"sites": found, "unsited": unsited}) + b"\n"
        if args.out:
            args.out.write_bytes(document)
        else:
            sys.stdout.buffer.write(document)
        return
    document = plan(ROOT, ops, args.out)
    print(json.dumps({"plan": str(args.out or ROOT / DEFAULT_OUTPUT / "plan.json"), **document["counts"]},
                     sort_keys=True))


if __name__ == "__main__":
    main()
