#!/usr/bin/env python3
"""Phase 4 X0: the scenario inventory of the command-line baselines.

The command-line acceptance corpus is the committed baselines under
`tsc/testdata/baselines/reference/{tsc,tsbuild,tscWatch,tsbuildWatch}`, each
the transcript of one scenario of the pinned Go package
`internal/execute/tsctests`. The scenarios are Go literals whose edits are
closures, so they are recorded from the pin, not transcribed:

    record   [--output FILE]       run the patched pinned suite, write data/phase4/scenarios.json.gz
    verify   [--inventory FILE] [--checks reproduce,identical,replay]
    check    [--inventory FILE]    the committed file is well formed and current (no Go run)
    pinned                         run and time the unpatched pinned suite (scratch output only)

How it runs. The pinned module (`tsc/go.mod`, `go.sum`, `internal` and the
four reference families) is exported with `git archive <pin>` into the
scratch tree `target/phase4/scenarios-tree/` (re-exported when the pin
changes), so nothing under `upstream/` is ever written: the pinned baseline
package writes its `local/` files beside the scratch tree's references. The
recorder is `go test -overlay` over that tree. The patch sources live in
`tools/phase4/recorder/`: `runner.go.diff`, `sys.go.diff` and `fs.go.diff`
are unified diffs against the pinned files, each headed by the pinned file's
SHA-256; the script requires the tree's file, the diff header and the
ledger's `source_hash` (PORTS.toml) to agree before it applies a diff
strictly (exact context, no fuzz), and fails if the pin's file changed.
`recorder.go` is added to the package as `phase4_recorder.go`. The patches
only add hook calls (and one explicit `testFs.Chtimes` that delegates), so the
patched run renders the same baselines; `record` refuses to write an
inventory unless they match the committed references byte for byte.

The pinned suite renders 516 baselines. The 517th committed reference,
`tsc/commandLine/adds-color-when-FORCE_COLOR-is-set.js`, is an orphan: the
pin's commit 31ff97358f removed its scenario and left the file. It is listed
in `orphan_references` and no scenario produces it.

Verification (`verify`):
  reproduce  a fresh patched run renders every committed reference it produces
             byte for byte, and the produced set plus `orphan_references` is
             exactly the four reference families;
  identical  the recording assembled from that run is byte-identical (the
             canonical JSON, and on the same host the gzip bytes) to the
             inventory;
  replay     a second overlay, `replay_test.go` added to the UNPATCHED package
             as `phase4_replay_test.go`, reads the inventory, rebuilds each
             scenario as a `tscInput` whose edits apply the recorded operations
             instead of the closures, runs it through the pin's own
             `tscInput.run`, and must pass for every scenario with no `local/`
             baseline written. It also checks each scenario's recorded initial
             state against the pin's `newTestSys` and every operation's clock
             readings. This proves the recording is sufficient input for a
             harness that is not the pin's.

THE FORMAT (version 1)

`data/phase4/scenarios.json.gz` is gzip (level 9, header mtime 0, no file
name) of one canonical JSON document followed by "\\n". Canonical JSON is
`json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)`:
object keys sorted by code point, no whitespace, every non-ASCII character
escaped as \\uXXXX (characters above U+FFFF as a surrogate pair). A JSON
reader that decodes escapes recovers the exact strings.

Bytes. Every file text (a file of the initial map, the text of a `write`) is
stored as `{"text": STRING}` when its bytes are valid UTF-8 (the bytes are the
string's UTF-8 encoding), otherwise as `{"text_hex": HEX}` (lowercase hex of
the bytes); never both. Go strings are bytes, so the distinction is exact.
Every other string (paths, arguments, captions, environment) is UTF-8; the
recorder fails on any that is not.

Top level, every key always present:
  format             "phase4-scenarios"
  version            1
  library_text       the pinned `tscDefaultLibContent`: the bytes newTestSys
                     writes to every library file it adds (UTF-8)
  orphan_references  [baseline path], committed references no scenario renders
  provenance         object, below
  scenarios          [scenario], sorted by `id` (code point order), unique

provenance:
  pin               the upstream commit (data/upstream.json)
  go, goos, goarch  `go env GOVERSION GOOS GOARCH` of the recording toolchain
  host              Python's platform.platform() of the recording host
  sources           {"fs.go.diff"|"recorder.go"|"runner.go.diff"|"sys.go.diff":
                     SHA-256 of the file in tools/phase4/recorder}
  pinned_sources    {"tsc/internal/execute/tsctests/{fs,runner,sys}.go": SHA-256},
                     the pinned bytes the diffs were applied to (= the ledger's
                     source_hash)
  scenario_count    516
  scenario_digests  {id: SHA-256 hex of the canonical JSON of that scenario
                     object, no trailing newline}

scenario (one `tscInput.run` of the pin), every key always present:
  id                   "<family>/<scenario>/<file>": the baseline's path under
                       testdata/baselines/reference
  family               "tsc" | "tsbuild" | "tscWatch" | "tsbuildWatch", the
                       runner's getBaselineSubFolder(): "tsbuild" when an
                       argument is -b/--b/-build/--build, plus "Watch" when one
                       is -w/--w/-watch/--watch
  scenario             the folder argument of `run(t, scenario)`
  file                 sub_scenario with every " " replaced by "-", plus ".js"
  sub_scenario         the tscInput's subScenario (the subtest's name)
  command_line_args    null | [string]: the first command's arguments; null is
                       Go's nil slice and [] an empty one (both exist; they
                       behave the same at the pin). The runner prints
                       "tsgo " + " ".join(args)
  cwd                  the effective working directory (the pin's default
                       /home/src/workspaces/project already applied)
  env                  {name: value}: the system's environment; a name that is
                       absent is unset (TS_TEST_TERMINAL_WIDTH, NO_COLOR, ...)
  output_is_tty        the effective WriteOutputIsTTY (default true applied)
  ignore_case          useCaseSensitiveFileNames is `not ignore_case`
  windows_style_root   "" or a root such as "C:/"; already applied to
                       `library.path`
  files                {absolute path: entry}: the tscInput's FileMap, which
                       vfstest.FromMapWithClock turns into the fake file system.
                       An entry is {"text"} / {"text_hex"} (a regular file) or
                       {"symlink": absolute target} (vfstest.Symlink)
  files_from_build     null, or {"command_line_args": [string], "paths":
                       [path, sorted]} for the one scenario whose map comes
                       from GetFileMapWithBuild: those `files` entries are the
                       outputs the pin's build (with those arguments) wrote
                       before the scenario started. Informative: a harness
                       uses `files` as given
  library              {"path": the default library directory, "files":
                       [name, sorted]}: the library files newTestSys added,
                       each `library_text` at path + "/" + name, after the map
                       (lib.d.ts, the target defaults and every lib file, minus
                       those `files` already provides). They are the testFs's
                       default libs: absent from the Input listing, shown as
                       `*Lib*` once read. The pin writes them in Go map order
                       (unspecified), one clock reading each, plus one per
                       directory the first write creates
  initial_clock_readings  the number of clock readings newTestSys made
                       (FromMapWithClock's one per map entry and one per
                       intermediate directory, then the library writes); a
                       harness building the same initial system must make the
                       same number before the first command
  edits                [edit], in the scenario's order

edit, every key present except shadow_operations:
  caption            the runner prints "Edit [i]:: <caption>"
  command_line_args  null (reuse the scenario's) | [string] (this edit's
                     command, for the incremental run and its shadow alike)
  expected_diff      "" or the explanation the runner prints when the
                     incremental and clean builds differ (7 edits)
  edit               false: the pin's edit closure is nil (operations is [])
  operations         [operation]: what the closure did to the incremental system
  shadow_operations  present only when the closure did something different on
                     the clean-build shadow: a closure that tests
                     sys.forIncrementalCorrectness, or one whose result depends
                     on a file the build rewrote (a prependFile to a
                     .tsbuildinfo). Absent means the same as operations

operation, in the order the closure performed them:
  {"op": "write", "path": P, "text"|"text_hex": BYTES, "clock_readings": N, ["via": H]}
      sys.writeFileNoError(P, BYTES): iovfs WriteFile on the MapFS itself
      (fsFromFileMap, not testFs: P is not added to the written-files set and
      is not removed from the default libs). MapFS.WriteFile resolves P and
      its parent through symlinks and stores the bytes at the resolved entry;
      if the parent does not exist the write fails, MkdirAll creates the
      missing directories (one clock reading each, outermost first) and the
      write is repeated. The file's mtime is one more clock reading. N is the
      readings made (1 when the parent exists).
  {"op": "remove", "path": P, "clock_readings": 0, ["via": H]}
      sys.removeNoError(P): MapFS.Remove of P's canonical entry and, for a
      directory, every entry below it; a missing P is not an error; no clock
      reading.
  {"op": "chtimes", "path": P, "clock_readings": 1}
      sys.FS().Chtimes(P, time.Time{}, sys.Now()): one clock reading, then
      P's entry gets that reading as its mtime (no symlink resolution; P must
      exist). One edit in the corpus does this.
  via (informative) names the helper the closure called when it was not
      writeFileNoError/removeNoError/Chtimes directly: replaceFileText,
      replaceFileTextAll, appendFile, prependFile, renameFileNoError. They
      reduce to readFileNoError (no state change, no clock reading) then
      writeFileNoError/removeNoError; the recorded BYTES are the text the pin
      computed, so a replay never re-reads a file.
  clock_readings must be reproduced exactly: TestClock advances one second per
      reading (its start is the wall time at system creation; no baseline
      prints an absolute time, but every mtime comparison depends on the order).

Replaying a scenario (what the pin's runner does with these inputs):
  1. Build the system: `files` through FromMapWithClock (useCaseSensitive =
     not ignore_case), then the library files; cwd, env, output_is_tty.
     `initial_clock_readings` readings have been made.
  2. Run the first command with `command_line_args`.
  3. For each edit i in order: on the incremental system apply `operations`;
     then for a non-watch scenario run the edit's command line (the edit's
     arguments, else the scenario's); for a watch scenario (the first command
     returned a watcher) send the changed paths, which the pin computes from
     the file-system state (FSDiffer.ChangedPaths), not from the operations,
     to the mock backend and run one watch cycle. Concurrently, the shadow: a
     fresh system built as in step 1 (forIncrementalCorrectness), then for
     j = 0..i apply edits[j].shadow_operations if present, else
     edits[j].operations, then run the same command line once (never a watch
     cycle). getDiffForIncremental compares the two.
  The rendered transcript must equal the committed reference at `id`.
"""
from __future__ import annotations

import argparse
import gzip
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import time
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import command, strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

FORMAT = "phase4-scenarios"
VERSION = 1
RECORDER = ROOT / "tools/phase4/recorder"
INVENTORY = ROOT / "data/phase4/scenarios.json.gz"
SCRATCH = ROOT / "target/phase4"
TREE = SCRATCH / "scenarios-tree"
PACKAGE = "tsc/internal/execute/tsctests"
PATCHED = ("fs.go", "runner.go", "sys.go")
SOURCES = ("fs.go.diff", "recorder.go", "runner.go.diff", "sys.go.diff")
ADDED_RECORDER = "phase4_recorder.go"
ADDED_REPLAY = "phase4_replay_test.go"
FAMILIES = ("tsc", "tsbuild", "tscWatch", "tsbuildWatch")
REFERENCES = "tsc/testdata/baselines/reference"
EXPECTED_FAMILIES = {"tsc": 217, "tsbuild": 192, "tscWatch": 42, "tsbuildWatch": 65}
EXPECTED_SCENARIOS = sum(EXPECTED_FAMILIES.values())
REPLAY_TEST = "TestPhase4Replay"
CHECKS = ("reproduce", "identical", "replay")
VIA = {"replaceFileText", "replaceFileTextAll", "appendFile", "prependFile", "renameFileNoError"}
ABSOLUTE = re.compile(r"/|[A-Za-z]:/")
TOP_KEYS = {"format", "version", "library_text", "orphan_references", "provenance", "scenarios"}
PROVENANCE_KEYS = {"pin", "go", "goos", "goarch", "host", "sources", "pinned_sources", "scenario_count",
                   "scenario_digests"}
SCENARIO_KEYS = {"id", "family", "scenario", "file", "sub_scenario", "command_line_args", "cwd", "env",
                 "output_is_tty", "ignore_case", "windows_style_root", "files", "files_from_build", "library",
                 "initial_clock_readings", "edits"}
EDIT_KEYS = {"caption", "command_line_args", "expected_diff", "edit", "operations"}
ROW_KEYS = {"family", "scenario", "file", "sub_scenario", "command_line_args", "cwd", "env", "output_is_tty",
            "ignore_case", "windows_style_root", "files", "files_from_build", "library_path", "library_files",
            "library_text_hex", "initial_clock_readings", "edits", "helper_calls", "shadows", "baseline_sha256",
            "errors"}


# --- patches -----------------------------------------------------------------

def pin():
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def ledger_hashes():
    """The ledger's source_hash of each patched pinned file."""
    ledger = tomllib.loads((ROOT / "PORTS.toml").read_text())
    wanted = {f"{PACKAGE}/{name}" for name in PATCHED}
    hashes = {entry["go"]: entry["source_hash"] for entry in ledger["file"] if entry.get("go") in wanted}
    if set(hashes) != wanted:
        raise ValueError("the ledger has no source_hash for " + ", ".join(sorted(wanted - set(hashes))))
    return hashes


def source_digests(directory=RECORDER):
    return {name: digest((Path(directory) / name).read_bytes()) for name in SOURCES}


HUNK = re.compile(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def parse_patch(text):
    """A unified diff of one file: (path, base sha256, [(old_start, old_len, new_len, lines)])."""
    lines = text.splitlines(keepends=True)
    if len(lines) < 3 or not lines[0].startswith("--- ") or not lines[1].startswith("+++ "):
        raise ValueError("a patch starts with --- and +++ headers")
    path, _, base = lines[0][4:].rstrip("\n").partition("\t")
    if not base.startswith("sha256:") or not re.fullmatch(r"[0-9a-f]{64}", base[7:]):
        raise ValueError("a patch's --- header names the pinned file's sha256")
    if lines[1][4:].split("\t")[0].rstrip("\n") != path:
        raise ValueError("a patch's headers name different files")
    hunks = []
    index = 2
    while index < len(lines):
        match = HUNK.fullmatch(lines[index].rstrip("\n"))
        if not match:
            raise ValueError(f"malformed hunk header: {lines[index]!r}")
        old_start, old_len, new_len = int(match[1]), int(match[2] or 1), int(match[4] or 1)
        index += 1
        body = []
        old_seen = new_seen = 0
        while index < len(lines) and (old_seen < old_len or new_seen < new_len):
            line = lines[index]
            if line[:1] not in (" ", "-", "+") or not line.endswith("\n"):
                raise ValueError(f"malformed hunk line: {line!r}")
            old_seen += line[0] in " -"
            new_seen += line[0] in " +"
            body.append((line[0], line[1:]))
            index += 1
        if (old_seen, new_seen) != (old_len, new_len):
            raise ValueError("a hunk's line counts do not match its header")
        hunks.append((old_start, old_len, new_len, body))
    return path, base[7:], hunks


def apply_patch(original, patch_text):
    """Apply a unified diff exactly: every context and removed line must match at its stated position."""
    _, _, hunks = parse_patch(patch_text)
    source = original.splitlines(keepends=True)
    result = []
    position = 0
    for old_start, old_len, _, body in hunks:
        start = old_start - 1 if old_len else old_start
        if start < position or start > len(source):
            raise ValueError("patch hunks overlap or start beyond the file")
        result.extend(source[position:start])
        position = start
        for tag, text in body:
            if tag in " -":
                if position >= len(source) or source[position] != text:
                    raise ValueError(f"patch does not apply at line {position + 1}: {text!r}")
                position += 1
            if tag in " +":
                result.append(text)
    result.extend(source[position:])
    return "".join(result)


def patched_sources(tree):
    """The patched files for the tree, after checking each pinned file against the ledger and the diff."""
    ledger = ledger_hashes()
    result = {}
    for name in PATCHED:
        patch_text = (RECORDER / f"{name}.diff").read_text()
        path, base, _ = parse_patch(patch_text)
        pinned_path = f"{PACKAGE}/{name}"
        pinned = (tree / pinned_path).read_bytes()
        if path != pinned_path:
            raise ValueError(f"{name}.diff patches {path}, not {pinned_path}")
        if not digest(pinned) == ledger[pinned_path] == base:
            raise ValueError(f"the pinned {pinned_path} changed: tree {digest(pinned)}, ledger "
                             f"{ledger[pinned_path]}, patch base {base}; re-derive the patch")
        result[name] = apply_patch(pinned.decode(), patch_text)
    return result


# --- the scratch tree and the Go runs -------------------------------------------

def prepare_tree(upstream):
    """Export the pinned module and the four reference families once per pin."""
    current = pin()
    paths = ["tsc/go.mod", "tsc/go.sum", "tsc/internal", *(f"{REFERENCES}/{family}" for family in FAMILIES)]
    marker = TREE / "export.json"
    identity = canonical({"pin": current, "paths": paths})
    if not marker.exists() or marker.read_bytes() != identity:
        if TREE.exists():
            shutil.rmtree(TREE)
        TREE.mkdir(parents=True)
        archive = command(["git", "archive", current, *paths], cwd=upstream)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
            stream.extractall(TREE, filter="data")
        marker.write_bytes(identity)
    local = TREE / "tsc/testdata/baselines/local"
    if local.exists():
        shutil.rmtree(local)
    return TREE


def local_baselines(tree):
    local = tree / "tsc/testdata/baselines/local"
    return sorted(str(path.relative_to(local)) for path in local.rglob("*") if path.is_file()) if local.exists() else []


def go_identity(env):
    values = command(["go", "env", "GOVERSION", "GOOS", "GOARCH"], cwd=ROOT, env=env).decode().split()
    return dict(zip(("go", "goos", "goarch"), values, strict=True))


def test_environment():
    env = go_environment()
    for name in ("TSGO_BASELINE_TRACKING_DIR", "TS_TEST_PROGRAM_SINGLE_THREADED", "PHASE4_RECORD_DIR",
                 "PHASE4_REPLAY_INVENTORY", "PHASE4_REPLAY_SUMMARY"):
        env.pop(name, None)
    return env


def build(directory, tree, env, replacements, name):
    """`go test -c` of the pinned package with the given overlay; returns the binary."""
    for target in replacements:
        if not target.startswith(str(tree / PACKAGE) + "/"):
            raise ValueError("an overlay entry outside the package: " + target)
    (directory / "overlay.json").write_bytes(canonical({"Replace": replacements}))
    binary = directory / f"{name}.test"
    repo_flag = ("-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath="
                 + str(directory / "unmatched-prefix"))
    arguments = ["go", "test", "-c", "-o", str(binary), "-trimpath", "-mod=readonly", repo_flag]
    if replacements:
        arguments += ["-overlay", str(directory / "overlay.json")]
    arguments.append("./" + PACKAGE.removeprefix("tsc/"))
    completed = subprocess.run(arguments, cwd=tree / "tsc", env=env, capture_output=True, check=False)
    (directory / f"{name}.build.log").write_bytes(completed.stdout + completed.stderr)
    if completed.returncode or not binary.exists():
        raise ValueError(f"the {name} build failed; see {directory / f'{name}.build.log'}")
    return binary


def run_binary(directory, tree, env, binary, name, arguments):
    started = time.monotonic()
    completed = subprocess.run([str(binary), "-test.count=1", "-test.timeout=20m", *arguments],
                               cwd=tree / PACKAGE, env=env, capture_output=True, check=False)
    elapsed = time.monotonic() - started
    output = completed.stdout + completed.stderr
    (directory / f"{name}.log").write_bytes(output)
    return completed.returncode, output.decode(errors="replace"), elapsed


def fresh(directory):
    directory = Path(directory).resolve()
    if directory.exists():
        shutil.rmtree(directory)
    directory.mkdir(parents=True)
    return directory


# --- recording -------------------------------------------------------------------

def text_entry(hex_value):
    """Bytes as {"text"} when they are UTF-8, else {"text_hex"}."""
    raw = bytes.fromhex(hex_value)
    try:
        return {"text": raw.decode("utf-8")}
    except UnicodeDecodeError:
        return {"text_hex": raw.hex()}


def operation_entry(op):
    if set(op) - {"op", "path", "text_hex", "clock_readings", "via"}:
        raise ValueError("unexpected operation fields: " + str(sorted(op)))
    entry = {"op": op["op"], "path": op["path"], "clock_readings": op["clock_readings"]}
    if op["op"] == "write":
        entry.update(text_entry(op["text_hex"]))
    elif "text_hex" in op:
        raise ValueError("only a write carries text")
    if op.get("via"):
        entry["via"] = op["via"]
    return entry


def scenario_entry(row):
    edits = []
    for edit in row["edits"]:
        entry = {"caption": edit["caption"], "command_line_args": edit["command_line_args"],
                 "expected_diff": edit["expected_diff"], "edit": edit["edit"],
                 "operations": [operation_entry(op) for op in edit["operations"]]}
        if edit["edit"]:
            if edit["shadow_operations"] is None:
                raise ValueError("an edit closure never ran on a shadow")
            if edit["shadow_operations"] != edit["operations"]:
                entry["shadow_operations"] = [operation_entry(op) for op in edit["shadow_operations"]]
        elif edit["operations"] or edit["shadow_operations"] is not None:
            raise ValueError("a nil edit recorded operations")
        edits.append(entry)
    files = {}
    for path, value in row["files"].items():
        if set(value) == {"text_hex"}:
            files[path] = text_entry(value["text_hex"])
        elif set(value) == {"symlink"}:
            files[path] = {"symlink": value["symlink"]}
        else:
            raise ValueError("a file entry is a text or a symlink: " + path)
    return {
        "id": f"{row['family']}/{row['scenario']}/{row['file']}",
        "family": row["family"], "scenario": row["scenario"], "file": row["file"],
        "sub_scenario": row["sub_scenario"], "command_line_args": row["command_line_args"],
        "cwd": row["cwd"], "env": row["env"], "output_is_tty": row["output_is_tty"],
        "ignore_case": row["ignore_case"], "windows_style_root": row["windows_style_root"],
        "files": files, "files_from_build": row["files_from_build"],
        "library": {"path": row["library_path"], "files": row["library_files"]},
        "initial_clock_readings": row["initial_clock_readings"], "edits": edits,
    }


def reference_paths(upstream):
    root = upstream / REFERENCES
    return sorted(str(path.relative_to(root)) for family in FAMILIES
                  for path in (root / family).rglob("*") if path.is_file())


def compare_baselines(upstream, produced_dir, produced):
    """Every produced baseline against its committed reference; the references nobody produced."""
    references = reference_paths(upstream)
    root = upstream / REFERENCES
    matched, different = [], []
    for path in produced:
        reference = root / path
        if reference.is_file() and reference.read_bytes() == (produced_dir / path).read_bytes():
            matched.append(path)
        else:
            different.append(path)
    on_disk = sorted(str(path.relative_to(produced_dir)) for path in produced_dir.rglob("*") if path.is_file())
    if on_disk != sorted(produced):
        raise ValueError("the run wrote baselines that no row describes, or rows without baselines")
    return {"references": len(references), "produced": len(produced), "matched": len(matched),
            "different": different, "orphans": sorted(set(references) - set(produced)),
            "unknown": sorted(set(produced) - set(references))}


def summarize(rows):
    operations, via, helpers = {}, {}, {}
    for row in rows:
        for name, count in row["helper_calls"].items():
            helpers[name] = helpers.get(name, 0) + count
        for edit in row["edits"]:
            for op in edit["operations"]:
                operations[op["op"]] = operations.get(op["op"], 0) + 1
                key = op.get("via") or "direct"
                via[key] = via.get(key, 0) + 1
    families = {}
    for row in rows:
        families[row["family"]] = families.get(row["family"], 0) + 1
    shadow = sum(1 for row in rows for edit in row["edits"]
                 if edit["edit"] and edit["shadow_operations"] != edit["operations"])
    return {"scenarios": len(rows), "families": dict(sorted(families.items())),
            "with_edits": sum(1 for row in rows if row["edits"]),
            "edits": sum(len(row["edits"]) for row in rows),
            "edits_with_closure": sum(1 for row in rows for edit in row["edits"] if edit["edit"]),
            "edits_with_shadow_operations": shadow,
            "shadow_runs": sum(row["shadows"] for row in rows),
            "operations": dict(sorted(operations.items())), "operations_via": dict(sorted(via.items())),
            "closure_calls": dict(sorted(helpers.items()))}


def record_run(directory):
    """One patched run: the assembled inventory, the reproduction result, the summary and timings."""
    timings = {}
    started = time.monotonic()
    upstream = verified_upstream().resolve()
    env = test_environment()
    tree = prepare_tree(upstream)
    directory = fresh(directory)
    overlay = directory / "overlay"
    overlay.mkdir()
    replacements = {}
    for name, text in patched_sources(tree).items():
        (overlay / name).write_text(text)
        replacements[str(tree / PACKAGE / name)] = str(overlay / name)
    if (tree / PACKAGE / ADDED_RECORDER).exists():
        raise ValueError("the recorder would replace a pinned file")
    shutil.copyfile(RECORDER / "recorder.go", overlay / ADDED_RECORDER)
    replacements[str(tree / PACKAGE / ADDED_RECORDER)] = str(overlay / ADDED_RECORDER)
    timings["prepare_seconds"] = time.monotonic() - started
    started = time.monotonic()
    binary = build(directory, tree, env, replacements, "recorder")
    timings["build_seconds"] = time.monotonic() - started
    output = directory / "out"
    output.mkdir()
    env["PHASE4_RECORD_DIR"] = str(output)
    status, log, timings["run_seconds"] = run_binary(directory, tree, env, binary, "recorder", [])
    if status:
        raise ValueError("the patched pinned suite failed:\n" + log[-4000:])
    if local_baselines(tree):
        raise ValueError("the pinned baseline check wrote local baselines: " + ", ".join(local_baselines(tree)[:10]))
    started = time.monotonic()
    rows = [strict_json_loads(line) for line in (output / "rows.ndjson").read_bytes().splitlines()]
    for row in rows:
        if set(row) != ROW_KEYS:
            raise ValueError("a recorder row has unexpected fields")
        if row["errors"]:
            raise ValueError(f"{row['family']}/{row['scenario']}/{row['file']}: " + "; ".join(row["errors"]))
    scenarios = sorted((scenario_entry(row) for row in rows), key=lambda entry: entry["id"])
    ids = [entry["id"] for entry in scenarios]
    if len(set(ids)) != len(ids):
        raise ValueError("two scenarios render the same baseline")
    library_texts = {row["library_text_hex"] for row in rows}
    if len(library_texts) != 1:
        raise ValueError("the scenarios disagree on the library text")
    library_text = bytes.fromhex(library_texts.pop()).decode("utf-8")
    comparison = compare_baselines(upstream, output / "baselines", ids)
    for row in rows:
        path = output / "baselines" / f"{row['family']}/{row['scenario']}/{row['file']}"
        if digest(path.read_bytes()) != row["baseline_sha256"]:
            raise ValueError("a written baseline is not the one the row describes: " + str(path))
    provenance = {
        "pin": pin(), **go_identity(env), "host": platform.platform(),
        "sources": source_digests(),
        "pinned_sources": ledger_hashes(),
        "scenario_count": len(scenarios),
        "scenario_digests": {entry["id"]: digest(canonical(entry)) for entry in scenarios},
    }
    document = {"format": FORMAT, "version": VERSION, "library_text": library_text,
                "orphan_references": comparison["orphans"], "provenance": provenance, "scenarios": scenarios}
    timings["assemble_seconds"] = time.monotonic() - started
    return document, comparison, summarize(rows), timings


def reproduced(comparison):
    return (not comparison["different"] and not comparison["unknown"]
            and comparison["matched"] == comparison["produced"] == EXPECTED_SCENARIOS
            and comparison["matched"] + len(comparison["orphans"]) == comparison["references"])


def render(document):
    return gzip.compress(canonical(document) + b"\n", compresslevel=9, mtime=0)


# --- check -------------------------------------------------------------------------

def read_inventory(data):
    """The document of a gzip container, after checking the container and the canonical form."""
    if data[:4] != b"\x1f\x8b\x08\x00" or data[4:8] != b"\x00\x00\x00\x00":
        raise ValueError("the inventory is gzip with no flags and header mtime 0")
    payload = gzip.decompress(data)
    document = strict_json_loads(payload)
    if payload != canonical(document) + b"\n":
        raise ValueError("the inventory is not canonical JSON")
    return document


def check_bytes(entry, where):
    keys = set(entry) & {"text", "text_hex"}
    if len(keys) != 1:
        raise ValueError(where + ": exactly one of text and text_hex")
    if "text" in entry and not isinstance(entry["text"], str):
        raise ValueError(where + ": text is a string")
    if "text_hex" in entry:
        value = entry["text_hex"]
        if not isinstance(value, str) or not re.fullmatch(r"(?:[0-9a-f]{2})*", value):
            raise ValueError(where + ": text_hex is lowercase hex")
        try:
            bytes.fromhex(value).decode("utf-8")
        except UnicodeDecodeError:
            pass
        else:
            raise ValueError(where + ": UTF-8 bytes are stored as text")


def check_strings(value, where, nullable=False):
    if nullable and value is None:
        return
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ValueError(where + " is a list of strings" + (" or null" if nullable else ""))


def check_operations(operations, where):
    if not isinstance(operations, list):
        raise ValueError(where + " is a list")
    for index, op in enumerate(operations):
        at = f"{where}[{index}]"
        if not isinstance(op, dict) or not isinstance(op.get("path"), str) or not ABSOLUTE.match(op["path"]):
            raise ValueError(at + ": an operation has an absolute path")
        kind, readings = op.get("op"), op.get("clock_readings")
        if type(readings) is not int:
            raise ValueError(at + ": clock_readings is an integer")
        if "via" in op and op["via"] not in VIA:
            raise ValueError(at + ": unknown via " + str(op["via"]))
        if kind == "write":
            if set(op) - {"text", "text_hex", "via"} != {"op", "path", "clock_readings"} or readings < 1:
                raise ValueError(at + ": a write has a path, its bytes and at least one clock reading")
            check_bytes(op, at)
        elif kind == "remove":
            if set(op) - {"via"} != {"op", "path", "clock_readings"} or readings != 0:
                raise ValueError(at + ": a remove has a path and no clock reading")
        elif kind == "chtimes":
            if set(op) != {"op", "path", "clock_readings"} or readings != 1:
                raise ValueError(at + ": a chtimes has a path and one clock reading")
        else:
            raise ValueError(at + ": unknown operation " + str(kind))


def check_scenario(scenario):
    if not isinstance(scenario, dict) or set(scenario) != SCENARIO_KEYS:
        raise ValueError("a scenario has exactly the documented keys: " + str(scenario.get("id")))
    sid = scenario["id"]
    if scenario["family"] not in FAMILIES or sid != f"{scenario['family']}/{scenario['scenario']}/{scenario['file']}":
        raise ValueError(sid + ": the id is family/scenario/file")
    if scenario["file"] != scenario["sub_scenario"].replace(" ", "-") + ".js":
        raise ValueError(sid + ": the file is the sub-scenario's baseline name")
    check_strings(scenario["command_line_args"], sid + ": command_line_args", nullable=True)
    for key in ("cwd", "windows_style_root", "scenario", "sub_scenario"):
        if not isinstance(scenario[key], str):
            raise ValueError(f"{sid}: {key} is a string")
    for key in ("output_is_tty", "ignore_case"):
        if not isinstance(scenario[key], bool):
            raise ValueError(f"{sid}: {key} is a boolean")
    if not isinstance(scenario["env"], dict) or any(not isinstance(v, str) for v in scenario["env"].values()):
        raise ValueError(sid + ": env maps names to strings")
    if not isinstance(scenario["files"], dict):
        raise ValueError(sid + ": files is an object")
    for path, entry in scenario["files"].items():
        if not isinstance(entry, dict):
            raise ValueError(f"{sid}: {path} is an object")
        if set(entry) == {"symlink"}:
            if not isinstance(entry["symlink"], str):
                raise ValueError(f"{sid}: {path} is a symlink to a path")
        else:
            check_bytes(entry, f"{sid}: {path}")
    build = scenario["files_from_build"]
    if build is not None:
        if not isinstance(build, dict) or set(build) != {"command_line_args", "paths"}:
            raise ValueError(sid + ": files_from_build has command_line_args and paths")
        check_strings(build["command_line_args"], sid + ": files_from_build arguments")
        check_strings(build["paths"], sid + ": files_from_build paths")
        if build["paths"] != sorted(build["paths"]) or not set(build["paths"]) <= set(scenario["files"]):
            raise ValueError(sid + ": the build's paths are sorted entries of files")
    library = scenario["library"]
    if not isinstance(library, dict) or set(library) != {"path", "files"} or not isinstance(library["path"], str):
        raise ValueError(sid + ": library has a path and files")
    check_strings(library["files"], sid + ": library files")
    if library["files"] != sorted(set(library["files"])):
        raise ValueError(sid + ": library files are sorted and unique")
    if type(scenario["initial_clock_readings"]) is not int or scenario["initial_clock_readings"] < 1:
        raise ValueError(sid + ": initial_clock_readings is a positive integer")
    if not isinstance(scenario["edits"], list):
        raise ValueError(sid + ": edits is a list")
    for index, edit in enumerate(scenario["edits"]):
        at = f"{sid}: edit {index}"
        if not isinstance(edit, dict) or not EDIT_KEYS <= set(edit) <= EDIT_KEYS | {"shadow_operations"}:
            raise ValueError(at + " has the documented keys")
        if not isinstance(edit["caption"], str) or not isinstance(edit["expected_diff"], str) \
                or not isinstance(edit["edit"], bool):
            raise ValueError(at + ": caption, expected_diff and edit are typed")
        check_strings(edit["command_line_args"], at + ": command_line_args", nullable=True)
        check_operations(edit["operations"], at + " operations")
        if "shadow_operations" in edit:
            check_operations(edit["shadow_operations"], at + " shadow_operations")
            if not edit["edit"] or edit["shadow_operations"] == edit["operations"]:
                raise ValueError(at + ": shadow_operations is present only when it differs")
        if not edit["edit"] and edit["operations"]:
            raise ValueError(at + ": an edit without a closure has no operations")


def check_document(document, *, current_pin=None, current_sources=None, current_ledger=None):
    """Raise ValueError unless the document is a well-formed, current inventory."""
    if not isinstance(document, dict) or set(document) != TOP_KEYS:
        raise ValueError("the inventory has exactly the documented top-level keys")
    if document["format"] != FORMAT or document["version"] != VERSION:
        raise ValueError(f"the inventory is {FORMAT} version {VERSION}")
    if not isinstance(document["library_text"], str) or not document["library_text"]:
        raise ValueError("library_text is a non-empty string")
    check_strings(document["orphan_references"], "orphan_references")
    provenance = document["provenance"]
    if not isinstance(provenance, dict) or set(provenance) != PROVENANCE_KEYS:
        raise ValueError("the provenance has exactly the documented keys")
    if provenance["pin"] != (current_pin or pin()):
        raise ValueError("the inventory was recorded at another pin; re-record it")
    if provenance["sources"] != (current_sources or source_digests()):
        raise ValueError("the recorder sources changed since the recording (stale patch digests); re-record")
    if provenance["pinned_sources"] != (current_ledger or ledger_hashes()):
        raise ValueError("the patched pinned files' ledger hashes changed; re-derive the patches and re-record")
    for key in ("go", "goos", "goarch", "host"):
        if not isinstance(provenance[key], str) or not provenance[key]:
            raise ValueError(f"provenance {key} is a non-empty string")
    scenarios = document["scenarios"]
    if not isinstance(scenarios, list):
        raise ValueError("scenarios is a list")
    for scenario in scenarios:
        check_scenario(scenario)
    ids = [scenario["id"] for scenario in scenarios]
    if ids != sorted(set(ids)):
        raise ValueError("scenarios are sorted by id and unique")
    if set(ids) & set(document["orphan_references"]):
        raise ValueError("an orphan reference is rendered by a scenario")
    families = {family: sum(1 for scenario in scenarios if scenario["family"] == family) for family in FAMILIES}
    if len(scenarios) != EXPECTED_SCENARIOS or families != EXPECTED_FAMILIES:
        raise ValueError(f"the inventory has {len(scenarios)} scenarios {families}; the pin renders "
                         f"{EXPECTED_SCENARIOS} {EXPECTED_FAMILIES}")
    if provenance["scenario_count"] != len(scenarios):
        raise ValueError("the provenance's scenario_count is not the number of scenarios")
    digests = {scenario["id"]: digest(canonical(scenario)) for scenario in scenarios}
    if provenance["scenario_digests"] != digests:
        changed = sorted(key for key in set(digests) | set(provenance["scenario_digests"])
                         if digests.get(key) != provenance["scenario_digests"].get(key))
        raise ValueError("scenario digests do not match the scenarios: " + ", ".join(changed[:5]))
    return {"scenarios": len(scenarios), "families": families,
            "edits": sum(len(scenario["edits"]) for scenario in scenarios),
            "operations": sum(len(edit["operations"]) for scenario in scenarios for edit in scenario["edits"])}


# --- verify --------------------------------------------------------------------------

def replay(directory, inventory):
    """The recorded scenarios through the unpatched pinned runner."""
    timings = {}
    started = time.monotonic()
    upstream = verified_upstream().resolve()
    env = test_environment()
    tree = prepare_tree(upstream)
    directory = fresh(directory)
    if (tree / PACKAGE / ADDED_REPLAY).exists():
        raise ValueError("the replay test would replace a pinned file")
    for name in PATCHED:
        if digest((tree / PACKAGE / name).read_bytes()) != ledger_hashes()[f"{PACKAGE}/{name}"]:
            raise ValueError("the scratch tree's pinned files changed")
    shutil.copyfile(RECORDER / "replay_test.go", directory / ADDED_REPLAY)
    binary = build(directory, tree, env, {str(tree / PACKAGE / ADDED_REPLAY): str(directory / ADDED_REPLAY)}, "replay")
    timings["build_seconds"] = time.monotonic() - started
    summary = directory / "summary.json"
    env.update(PHASE4_REPLAY_INVENTORY=str(Path(inventory).resolve()), PHASE4_REPLAY_SUMMARY=str(summary))
    status, log, timings["run_seconds"] = run_binary(directory, tree, env, binary, "replay",
                                                     ["-test.run", f"^{REPLAY_TEST}$", "-test.v"])
    passed = len(re.findall(rf"^\s+--- PASS: {REPLAY_TEST}/", log, flags=re.MULTILINE))
    failed = re.findall(rf"^\s*--- FAIL: {REPLAY_TEST}/(\S+)", log, flags=re.MULTILINE)
    local = local_baselines(tree)
    for path in local:
        target = directory / "differing" / path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(tree / "tsc/testdata/baselines/local" / path, target)
    reported = strict_json_loads(summary.read_bytes()) if summary.exists() else {}
    scenarios = reported.get("scenarios")
    result = {"status": status, "scenarios": scenarios, "subtests_passed": passed, "subtests_failed": failed,
              "differing": local,
              "matched": status == 0 and reported.get("failed") is False and not failed and not local
              and scenarios == passed == EXPECTED_SCENARIOS}
    if not result["matched"] and not failed and not local:
        result["log_tail"] = log[-3000:]
    return result, timings


def verify(inventory, checks):
    document = read_inventory(Path(inventory).read_bytes())
    check_document(document)
    report, timings = {}, {}
    if {"reproduce", "identical"} & checks:
        started = time.monotonic()
        recorded, comparison, _, run_timings = record_run(SCRATCH / "scenarios-verify/record")
        timings["record_run"] = run_timings
        if "reproduce" in checks:
            report["reproduce"] = {"matched": comparison["matched"], "produced": comparison["produced"],
                                   "references": comparison["references"], "different": comparison["different"],
                                   "orphans": comparison["orphans"],
                                   "orphans_as_recorded": comparison["orphans"] == document["orphan_references"],
                                   "passed": reproduced(comparison)
                                   and comparison["orphans"] == document["orphan_references"]}
        if "identical" in checks:
            # Each verify process records on its own; identity is checked on the JSON bytes,
            # and on the gzip bytes as well (zlib is deterministic on one host).
            json_identical = canonical(recorded) == canonical(document)
            gzip_identical = render(recorded) == Path(inventory).read_bytes()
            changed = [] if json_identical else sorted(
                key for key in set(recorded["provenance"]["scenario_digests"]) | set(document["provenance"]["scenario_digests"])
                if recorded["provenance"]["scenario_digests"].get(key) != document["provenance"]["scenario_digests"].get(key))
            report["identical"] = {"json_identical": json_identical, "gzip_identical": gzip_identical,
                                   "changed_scenarios": changed[:20],
                                   "passed": json_identical and gzip_identical}
        timings["reproduce_and_identical_seconds"] = time.monotonic() - started
    if "replay" in checks:
        started = time.monotonic()
        result, replay_timings = replay(SCRATCH / "scenarios-verify/replay", inventory)
        timings["replay_run"] = replay_timings
        timings["replay_seconds"] = time.monotonic() - started
        report["replay"] = {**result, "passed": result["matched"]}
    return report, timings


def pinned_run():
    """The unpatched pinned package, as its own CI runs it, in the scratch tree."""
    started = time.monotonic()
    upstream = verified_upstream().resolve()
    env = test_environment()
    tree = prepare_tree(upstream)
    directory = fresh(SCRATCH / "scenarios-pinned")
    binary = build(directory, tree, env, {}, "pinned")
    build_seconds = time.monotonic() - started
    status, log, run_seconds = run_binary(directory, tree, env, binary, "pinned", ["-test.v"])
    return {"status": status, "build_seconds": round(build_seconds, 2), "run_seconds": round(run_seconds, 2),
            "top_level_passed": len(re.findall(r"^--- PASS: ", log, flags=re.MULTILINE)),
            "failed": re.findall(r"^\s*--- FAIL: (\S+)", log, flags=re.MULTILINE),
            "local_baselines": local_baselines(tree)}


def rounded(value):
    if isinstance(value, dict):
        return {key: rounded(item) for key, item in value.items()}
    return round(value, 2) if isinstance(value, float) else value


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sub = commands.add_parser("record")
    sub.add_argument("--output", type=Path, default=INVENTORY)
    sub = commands.add_parser("verify")
    sub.add_argument("--inventory", type=Path, default=INVENTORY)
    sub.add_argument("--checks", default=",".join(CHECKS))
    sub = commands.add_parser("check")
    sub.add_argument("--inventory", type=Path, default=INVENTORY)
    commands.add_parser("pinned")
    args = parser.parse_args()
    if args.command == "record":
        started = time.monotonic()
        document, comparison, summary, timings = record_run(SCRATCH / "scenarios-record")
        if not reproduced(comparison):
            raise ValueError("the patched run does not reproduce the references: "
                             + json.dumps({key: comparison[key] for key in ("matched", "produced", "references",
                                                                              "different", "orphans", "unknown")}))
        check_document(document)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(render(document))
        timings["total_seconds"] = time.monotonic() - started
        print(json.dumps({"inventory": str(args.output), "bytes": args.output.stat().st_size,
                          "reproduced": f"{comparison['matched']}/{comparison['produced']}",
                          "orphan_references": comparison["orphans"], **summary,
                          "timings": rounded(timings)}, indent=1))
    elif args.command == "verify":
        checks = set(args.checks.split(","))
        if not checks or checks - set(CHECKS):
            raise ValueError("checks are " + ", ".join(CHECKS))
        report, timings = verify(args.inventory, checks)
        print(json.dumps({**report, "timings": rounded(timings)}, indent=1))
        if not all(item["passed"] for item in report.values()):
            raise SystemExit(1)
    elif args.command == "check":
        print(json.dumps(check_document(read_inventory(args.inventory.read_bytes()))))
    else:
        result = pinned_run()
        print(json.dumps(result, indent=1))
        if result["status"] or result["failed"] or result["local_baselines"]:
            raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        print("phase4 scenarios failed: " + str(error), file=sys.stderr)
        raise SystemExit(1) from error
