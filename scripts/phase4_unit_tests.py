#!/usr/bin/env python3
"""Phase 4 unit-test rosters (docs/PHASE4-plan.md section 4, X0 "The unit-test rosters").

`data/phase4/unit-tests.json` lists, from the pinned sources:

* every `func TestX(t *testing.T)` of the Phase 4 packages: the directories
  whose `PORTS.toml` files are all `phase = 4` and keep at least one in-scope
  file after decision 4 (so `compiler`, shared with Phases 2 and 3, and
  `pprof`, moved to Phase 7, are not among them);
* the `execute/tsctests` test functions that assert directly instead of
  producing a baseline (kind `direct`); the ones that call the runner's
  `run(t, scenario)` (kind `baseline`) are listed with their scenario folders
  and belong to the scenario inventory, and `TestMain(m *testing.M)` is
  neither.

Each roster entry carries its package, file and function, the literal names of
its `t.Run` subtests, the checkpoint that owns it (plan section 4: the package's
checkpoint; a direct `tsctests` test by its command lines, `--watch` X5,
`--build` X3, otherwise X1), its host condition (`any`, `macos`, `linux`; from
the file name's GOOS/GOARCH suffix, the `//go:build` line and `runtime.GOOS`
skips in the body) and its status: `ported` with the Rust test that carries it
(a `// source:` or `// port:` comment naming `<test file>:<TestName>` above a
Rust `fn` under `crates/` or `tools/`), `not_applicable` with the reason
(Windows-only tests, the Go FFI trampoline test), or `pending` with its
checkpoint.

The test files are bound by their digests and the test functions by count and
digest, so a change to the pinned tests is reviewed here.

    build    # writes data/phase4/unit-tests.json; exits 1 on problems
    check    # the committed roster is current and valid
    summary  # counts by package, checkpoint, host and status
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase2_audit  # noqa: E402
import phase4_audit  # noqa: E402

ROSTER = ROOT / "data/phase4/unit-tests.json"
PIN = phase2_audit.REVIEWED_PIN
UPSTREAM = ROOT / "upstream"
TSCTESTS = "internal/execute/tsctests"
MARKER_ROOTS = ("crates", "tools")
# The reviewed denominators: the test files with their digests, and every
# top-level test function of them (count, sha256 of the canonical value).
REVIEWED_TEST_FILES = (27, "f78c6546e8a968292533869d53bba022f403e155249878a35e9693aa1a5637c7")
REVIEWED_TEST_FUNCTIONS = (226, "91bb95a23163cb464df634a5f9dc0593d564e808c4974ed08ca846f293f15a1f")

# Plan section 4: the checkpoint whose witnesses include a package's tests.
PACKAGE_CHECKPOINTS = {
    "cmd/tsc": "X1",
    "internal/execute/tsc": "X1",
    "internal/nativepath": "X1",
    "internal/execute/incremental": "X2",
    "internal/execute/build": "X3",
    "internal/fswatch": "X4",
    "internal/execute/watchmanager": "X5",
    "internal/tracing": "X6",
}
# Tests that do not apply to this port, with the reason. Windows-only tests are
# found from their host condition.
NOT_APPLICABLE = {
    "tsc/internal/fswatch/fsevents_darwin_ffi_arm64_test.go:TestCallbackASMTouchesOnlySafeRegisters": (
        "The Go FFI trampoline test: it disassembles a Go test binary to check that fsEventsCallbackASM, the Go "
        "assembly callback FSEvents enters from C, touches only AAPCS caller-saved registers. The port declares "
        "FSEvents, CoreFoundation and libdispatch extern \"C\" with framework linkage and has no Go-ABI trampoline "
        "(plan X4)."),
}
WINDOWS_REASON = "Windows-only (file name suffix or build constraint); Windows is not a target (ADR 0002)"
# The plan's counts (section 1, "The acceptance corpus", and section 5).
PLAN = {
    "unit": 146,
    "unit_by_package": {"internal/fswatch": 124, "internal/execute/watchmanager": 7,
                        "internal/execute/incremental": 5, "internal/nativepath": 5, "internal/execute/build": 2,
                        "internal/tracing": 2, "cmd/tsc": 1},
    "direct": 14, "direct_content_mapper": 12, "direct_race": 2, "baseline": 61, "tsctests_functions": 76,
    "nativepath_windows": 4,
}
NOTES = {
    "per_backend": ("runForEachWatcher runs a subtest per available backend. In the pin, macOS has fsevents and "
                    "kqueue and Linux has inotify, fanotify and the test-only fanotify-no-rename; the kqueue subtests "
                    "do not apply (decision 4, ADR 0002), and the port runs FSEvents on macOS and inotify, fanotify "
                    "and fanotify-no-rename on Linux (plan X4)."),
    "baseline": ("The tsctests functions that call the runner's run(t, scenario) produce the 517 baselines; the "
                 "scenario inventory (X0) owns them, scenario by scenario."),
}

KNOWN_OS = {"aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "js", "linux", "nacl",
            "netbsd", "openbsd", "plan9", "solaris", "wasip1", "windows", "zos"}
UNIX_OS = {"aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "linux", "netbsd",
           "openbsd", "solaris"}
KNOWN_ARCH = {"386", "amd64", "amd64p32", "arm", "armbe", "arm64", "arm64be", "loong64", "mips", "mipsle", "mips64",
              "mips64le", "mips64p32", "mips64p32le", "ppc", "ppc64", "ppc64le", "riscv", "riscv64", "s390", "s390x",
              "sparc", "sparc64", "wasm"}
HOSTS = {"macos": (("darwin", "arm64"), ("darwin", "amd64")),
         "linux": (("linux", "amd64"), ("linux", "arm64")),
         "windows": (("windows", "amd64"), ("windows", "arm64"))}

TEST_FUNC = re.compile(r"^func (Test\w*)\((\w+) \*testing\.([TM])\) \{", re.M)
TOP_FUNC = re.compile(r"^func (?:\([^)]*\) )?(\w+)\b", re.M)
PLAIN_FUNC = re.compile(r"^func (\w+)\b", re.M)
T_RUN = re.compile(r"\bt\.Run\(\s*(\"(?:[^\"\\]|\\.)*\"|`[^`]*`)?")
SKIP = re.compile(r"\bt\.Skip(?:f|Now)?\((\"(?:[^\"\\]|\\.)*\")?")
GOOS_SKIP = re.compile(r"if runtime\.GOOS (==|!=) \"(\w+)\" \{\n(?:[^\n]*\n){0,2}?[^\n]*\bt\.Skip")
STRING_SLICE = re.compile(r"\[\]string\{([^{}]*)\}")
RUN_SCENARIO = re.compile(r"\.run\(t, \"([^\"]+)\"\)")
PORTED = re.compile(r"^\s*//[/!]?\s*(?:source|port):\s*(tsc/\S+_test\.go):(Test\w+)\s*$")
RUST_FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)")


def packages(root=ROOT):
    """The Phase 4 packages: every ledger file of the directory is phase 4 and one is in the audit's scope."""
    ledger = tomllib.loads((root / "PORTS.toml").read_text())
    if ledger["pin"] != PIN:
        raise ValueError("ledger pin differs from the reviewed Phase 4 roster")
    by_directory = {}
    for entry in ledger["file"]:
        by_directory.setdefault(entry["go"].rsplit("/", 1)[0], []).append(entry)
    in_scope = set(phase4_audit.scope_files(root, ledger["file"]))
    return sorted(directory.removeprefix("tsc/") for directory, entries in by_directory.items()
                  if all(entry.get("phase") == 4 for entry in entries)
                  and any(entry["go"] in in_scope for entry in entries))


def test_files(upstream, directories):
    """The pinned `_test.go` files of the packages, relative to `tsc/`, sorted."""
    files = []
    for directory in directories:
        files.extend(sorted(f"{directory}/{path.name}" for path in (upstream / "tsc" / directory).glob("*_test.go")))
    return files


def file_constraint(name):
    """The GOOS and GOARCH a file name restricts to (go/build's goodOSArchFile), or None for each."""
    stem = name.split(".", 1)[0]
    if "_" not in stem:
        return None, None
    parts = stem[stem.index("_"):].split("_")
    if parts and parts[-1] == "test":
        parts = parts[:-1]
    if len(parts) >= 2 and parts[-2] in KNOWN_OS and parts[-1] in KNOWN_ARCH:
        return parts[-2], parts[-1]
    if parts and parts[-1] in KNOWN_OS:
        return parts[-1], None
    if parts and parts[-1] in KNOWN_ARCH:
        return None, parts[-1]
    return None, None


def build_line(text):
    """The `//go:build` expression before the package clause, or None."""
    for line in text.splitlines():
        if line.startswith("//go:build "):
            return line.removeprefix("//go:build ").strip()
        if line.startswith("package "):
            return None
    return None


def evaluate(expression, goos, goarch):
    """A //go:build expression for one GOOS/GOARCH (identifiers, !, &&, ||, parentheses)."""
    tokens = re.findall(r"\(|\)|!|&&|\|\||[\w.]+", expression)
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take():
        nonlocal position
        position += 1
        return tokens[position - 1]

    def term(name):
        return name == goos or name == goarch or (name == "unix" and goos in UNIX_OS)

    def primary():
        token = take()
        if token == "!":
            return not primary()
        if token == "(":
            value = disjunction()
            if take() != ")":
                raise ValueError(f"unbalanced build constraint {expression!r}")
            return value
        return term(token)

    def conjunction():
        value = primary()
        while peek() == "&&":
            take()
            value = primary() and value
        return value

    def disjunction():
        value = conjunction()
        while peek() == "||":
            take()
            value = conjunction() or value
        return value

    value = disjunction()
    if position != len(tokens):
        raise ValueError(f"unparsed build constraint {expression!r}")
    return value


def platforms(name, text):
    """The (goos, goarch) pairs of HOSTS the file builds for."""
    goos, goarch = file_constraint(name)
    expression = build_line(text)
    return {pair for pairs in HOSTS.values() for pair in pairs
            if (goos is None or pair[0] == goos) and (goarch is None or pair[1] == goarch)
            and (expression is None or evaluate(expression, *pair))}


def body_end(text, start):
    """The end of the Go function declared at `start`: its body's closing brace, skipping comments, string and
    rune literals."""
    index, parens, depth, inside = start, 0, 0, False
    while index < len(text):
        char = text[index]
        if text.startswith("//", index):
            index = text.find("\n", index)
            if index < 0:
                return len(text)
            continue
        if text.startswith("/*", index):
            index = text.index("*/", index) + 2
            continue
        if char in "\"'":
            index += 1
            while text[index] != char:
                index += 2 if text[index] == "\\" else 1
        elif char == "`":
            index = text.index("`", index + 1)
        elif char == "(":
            parens += 1
        elif char == ")":
            parens -= 1
        elif char == "{" and (inside or parens == 0):
            inside = True
            depth += 1
        elif char == "}" and inside:
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    raise ValueError("unterminated Go function")


def functions(text, pattern=None):
    """Top-level functions: name -> source text, from the signature to the body's closing brace."""
    found = {}
    for match in (pattern or TOP_FUNC).finditer(text):
        found.setdefault(match.group(1), text[match.start():body_end(text, match.start())])
    return found


def host_of(pairs, body):
    """The host condition after the body's runtime.GOOS skips: any, macos, linux, windows or none."""
    systems = {goos for goos, _ in pairs}
    for operator, goos in GOOS_SKIP.findall(body):
        systems = systems - {goos} if operator == "==" else systems & {goos}
    hosts = {host for host, pairs_of in HOSTS.items() if systems & {goos for goos, _ in pairs_of}}
    if {"macos", "linux"} <= hosts:
        return "any"
    if hosts == {"windows"}:
        return "windows"
    return next(iter(sorted(hosts - {"windows"})), "none")


def arch_of(pairs):
    arches = sorted({goarch for _, goarch in pairs})
    all_arches = sorted({goarch for host in ("macos", "linux") for _, goarch in HOSTS[host]})
    return None if arches == all_arches else arches


def called_helpers(body, helpers):
    """The package helpers the body names (one level): name -> source text."""
    return {name: helper for name, helper in helpers.items() if re.search(rf"\b{re.escape(name)}\b", body)}


def command_lines(text):
    """Every string in a string-slice literal of the text: the command lines a tsctests test runs."""
    arguments = set()
    for literal in STRING_SLICE.findall(text):
        arguments.update(re.findall(r"\"([^\"]*)\"", literal))
    return arguments


def direct_checkpoint(arguments):
    if {"--watch", "-w"} & arguments:
        return "X5"
    if {"--build", "-b", "--b", "-build"} & arguments:
        return "X3"
    return "X1"


def subtests(body):
    names, computed = [], False
    for match in T_RUN.finditer(body):
        if match.group(1) and match.group(1).startswith('"'):
            names.append(json.loads(match.group(1)))
        elif match.group(1):
            names.append(match.group(1)[1:-1])
        else:
            computed = True
    if re.search(r"\b(runForEachWatcher|runWalkDirTest)\(", body):
        computed = True
    return names, computed


def ported_tests(root=ROOT):
    """Rust tests that name a pinned Go test: `tsc/<file>:<TestName>` -> [{site, test}]."""
    found = {}
    for base in MARKER_ROOTS:
        for path in sorted((root / base).rglob("*.rs")):
            if "target" in path.relative_to(root).parts:
                continue
            lines = path.read_text(errors="replace").splitlines()
            for index, line in enumerate(lines):
                match = PORTED.match(line)
                if not match:
                    continue
                test = next((RUST_FN.match(after).group(1) for after in lines[index + 1:index + 8]
                             if RUST_FN.match(after)), None)
                relative = path.relative_to(root).as_posix()
                found.setdefault(f"{match.group(1)}:{match.group(2)}", []).append(
                    {"site": f"{relative}:{index + 1}", "test": f"{relative}::{test}" if test else None})
    return found


def binding(value):
    return (len(value), digest(canonical(value if isinstance(value, dict) else sorted(value))))


def build_document(root=ROOT, upstream=None, ported=None):
    """The roster document; its `problems` list is empty when it is valid."""
    upstream = UPSTREAM if upstream is None else upstream
    ported = ported_tests(root) if ported is None else ported
    directories = packages(root)
    files = test_files(upstream, directories)
    problems = []
    digests, sources = {}, {}
    for relative in files:
        data = (upstream / "tsc" / relative).read_bytes()
        digests[f"tsc/{relative}"] = digest(data)
        sources[relative] = data.decode()
    # Package-level helpers of the test files (plain functions, not methods), by package.
    helpers = {}
    for relative, text in sources.items():
        helpers.setdefault(relative.rsplit("/", 1)[0], {}).update(
            {name: body for name, body in functions(text, PLAIN_FUNC).items() if not name.startswith("Test")})
    roster, baselines, neither, all_tests = [], [], [], []
    for relative, text in sources.items():
        directory, name = relative.rsplit("/", 1)
        pairs = platforms(name, text)
        bodies = functions(text)
        for match in TEST_FUNC.finditer(text):
            function, kind_of_t = match.group(1), match.group(3)
            identity = f"tsc/{relative}:{function}"
            all_tests.append(identity)
            body = bodies[function]
            if kind_of_t == "M":
                neither.append({"id": identity, "reason": "TestMain(m *testing.M) sets the package up; it is neither "
                                                          "a unit test nor a baseline test"})
                continue
            if directory == TSCTESTS and RUN_SCENARIO.search(body):
                baselines.append({"id": identity, "file": f"tsc/{relative}", "function": function,
                                  "scenario_folders": sorted(set(RUN_SCENARIO.findall(body))),
                                  "asserts_directly": bool(re.search(r"\bassert\.|\bt\.(Fatal|Error)", body))})
                continue
            names, computed = subtests(body)
            called = called_helpers(body, helpers.get(directory, {}))
            effective = "\n".join([body, *called.values()])
            entry = {"id": identity, "package": directory, "file": f"tsc/{relative}", "function": function,
                     "kind": "direct" if directory == TSCTESTS else "unit",
                     "subtests": names, "computed_subtests": computed,
                     "per_backend": bool(re.search(r"\brunForEachWatcher\(|\bavailableWatchers\b", body)),
                     "skips": [json.loads(message) for message in SKIP.findall(effective) if message]}
            if directory == TSCTESTS:
                arguments = command_lines(effective)
                entry["checkpoint"] = direct_checkpoint(arguments)
                entry["command_line_flags"] = sorted(argument for argument in arguments if argument.startswith("-"))
            else:
                entry["checkpoint"] = PACKAGE_CHECKPOINTS.get(directory)
                if entry["checkpoint"] is None:
                    problems.append(f"{identity}: no checkpoint for package {directory}")
            entry["host"] = host_of(pairs, effective)
            arch = arch_of({pair for pair in pairs if pair[0] != "windows"})
            if arch and entry["host"] != "windows":
                entry["arch"] = arch
            carriers = ported.get(identity, [])
            if entry["host"] == "windows":
                entry |= {"status": "not_applicable", "reason": WINDOWS_REASON}
            elif identity in NOT_APPLICABLE:
                entry |= {"status": "not_applicable", "reason": NOT_APPLICABLE[identity]}
            elif carriers:
                entry |= {"status": "ported", "rust": carriers}
                if any(carrier["test"] is None for carrier in carriers):
                    problems.append(f"{identity}: a Rust comment names it without a test function after it")
            else:
                entry["status"] = "pending"
            if carriers and entry["status"] == "not_applicable":
                problems.append(f"{identity}: not applicable, but a Rust test names it")
            roster.append(entry)
    known = set(all_tests)
    prefixes = tuple(f"tsc/{directory}/" for directory in directories)
    unknown = {identity: [carrier["site"] for carrier in carriers] for identity, carriers in ported.items()
               if identity.startswith(prefixes) and identity not in known}
    for identity, sites in sorted(unknown.items()):
        problems.append(f"{identity}: a Rust comment names no pinned test ({', '.join(sites)})")
    for identity in sorted(set(NOT_APPLICABLE) - known):
        problems.append(f"{identity}: reviewed as not applicable, but not a pinned test")
    if binding(digests) != REVIEWED_TEST_FILES:
        problems.append(f"the test files differ from the reviewed {REVIEWED_TEST_FILES[0]}")
    if binding(all_tests) != REVIEWED_TEST_FUNCTIONS:
        problems.append(f"the test functions differ from the reviewed {REVIEWED_TEST_FUNCTIONS[0]}")
    counts = count(roster, baselines, neither)
    return {
        "version": 1,
        "pin": PIN,
        "scope": {
            "description": ("The Go tests of the Phase 4 packages (every PORTS.toml file of the directory is "
                            "phase 4, and one is in docs/PHASE4-plan.md's scope after decision 4) and the "
                            "execute/tsctests functions that assert directly (docs/PHASE4-plan.md sections 1, 4 "
                            "and 5)."),
            "packages": directories,
            "test_files": digests,
            "test_files_sha256": binding(digests)[1],
            "test_functions": len(all_tests),
            "test_functions_sha256": binding(all_tests)[1],
            "rust_roots": list(MARKER_ROOTS),
        },
        "notes": NOTES,
        "tests": roster,
        "baseline_tests": baselines,
        "neither": neither,
        "counts": counts,
        "plan": PLAN,
        "plan_differences": plan_differences(counts),
        "unknown_ported": unknown,
        "problems": problems,
    }


def tally(values):
    counts = {}
    for value in values:
        counts[value] = counts.get(value, 0) + 1
    return dict(sorted(counts.items()))


def count(roster, baselines, neither):
    unit = [entry for entry in roster if entry["kind"] == "unit"]
    direct = [entry for entry in roster if entry["kind"] == "direct"]
    return {
        "roster": len(roster), "unit": len(unit), "direct": len(direct),
        "baseline": len(baselines), "neither": len(neither),
        "tsctests_functions": len(direct) + len(baselines) + len(neither),
        "unit_by_package": tally(entry["package"] for entry in unit),
        "direct_by_file": tally(entry["file"].rsplit("/", 1)[1] for entry in direct),
        "by_checkpoint": tally(entry["checkpoint"] for entry in roster),
        "by_host": tally(entry["host"] for entry in roster),
        "by_status": tally(entry["status"] for entry in roster),
        "pending_by_checkpoint": tally(entry["checkpoint"] for entry in roster if entry["status"] == "pending"),
        "applicable_by_checkpoint_and_host": {
            checkpoint: tally(entry["host"] for entry in roster
                              if entry["checkpoint"] == checkpoint and entry["status"] != "not_applicable")
            for checkpoint in sorted({entry["checkpoint"] for entry in roster})},
    }


def plan_differences(counts):
    """Where the pin's counts differ from the plan's."""
    differences = []
    for key in ("unit", "direct", "baseline", "tsctests_functions"):
        if counts[key] != PLAN[key]:
            differences.append(f"{key}: plan {PLAN[key]}, pin {counts[key]}")
    for package in sorted(set(PLAN["unit_by_package"]) | set(counts["unit_by_package"])):
        planned, found = PLAN["unit_by_package"].get(package, 0), counts["unit_by_package"].get(package, 0)
        if planned != found:
            differences.append(f"unit tests of {package}: plan {planned}, pin {found}")
    for name, planned in (("contentmapper_watch_test.go", PLAN["direct_content_mapper"]),
                          ("watcher_race_test.go", PLAN["direct_race"])):
        found = counts["direct_by_file"].get(name, 0)
        if planned != found:
            differences.append(f"direct tests of {name}: plan {planned}, pin {found}")
    return differences


def render(document):
    return json.dumps(document, indent=1, sort_keys=True) + "\n"


def summary(document):
    counts = document["counts"]
    lines = [f"roster {counts['roster']}: unit {counts['unit']}, direct {counts['direct']}; "
             f"baseline {counts['baseline']}, neither {counts['neither']}"]
    for key in ("unit_by_package", "direct_by_file", "by_checkpoint", "by_host", "by_status", "pending_by_checkpoint"):
        lines.append(f"{key}: " + ", ".join(f"{name} {value}" for name, value in counts[key].items()))
    for difference in document["plan_differences"]:
        lines.append("differs from the plan: " + difference)
    return "\n".join(lines)


def check(*, root=ROOT, path=None, upstream=None):
    """Problems with the committed roster: stale or invalid."""
    document = build_document(root, upstream)
    path = ROSTER if path is None else path
    found = list(document["problems"])
    if not path.is_file() or path.read_text() != render(document):
        found.append(f"{path.relative_to(root) if path.is_relative_to(root) else path} differs from the rebuilt roster")
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("build", "check", "summary"))
    args = parser.parse_args()
    if args.command == "build":
        document = build_document()
        ROSTER.parent.mkdir(parents=True, exist_ok=True)
        ROSTER.write_text(render(document))
        print(summary(document))
        for problem in document["problems"]:
            print("problem: " + problem, file=sys.stderr)
        if document["problems"]:
            raise SystemExit(1)
    elif args.command == "check":
        found = check()
        for problem in found:
            print("problem: " + problem, file=sys.stderr)
        if found:
            raise SystemExit(1)
        print("phase4 unit-test roster current")
    else:
        print(summary(build_document()))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print("phase4 unit tests failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
