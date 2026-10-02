#!/usr/bin/env python3
"""Paired Phase 4 native CLI witnesses, preserving raw child output.

Use --build with a phase4_native.py build for replayable acceptance evidence.
Supplying --go-bin and --rust-bin remains a development observation without build
provenance. No source edits, capture replacement, or child build is implicit.
Only timestamps and the two explicit temporary-root spellings are normalized.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s08_oracle import ROOT, canonical, digest  # noqa: E402
from s04_common import strict_json_loads  # noqa: E402
import phase4_corpus as corpus  # noqa: E402

SETTLED = re.compile(rb"Found ([0-9]+) errors?\. Watching for file changes\.")
CLEAR = rb"\x1b\[2J\x1b\[3J\x1b\[H"
CLOCK = re.compile(rb"^(" + CLEAR + rb")?(?:\[(?:[0-9]{1,2}:){2}[0-9]{2}(?: [AP]M)?\] ?|(?:[0-9]{1,2}:){2}[0-9]{2}(?: [AP]M)? - )", re.MULTILINE)
CYCLE = re.compile(rb"(?:" + CLEAR + rb")?(?:Starting compilation in watch mode|File change detected\. Starting incremental compilation)\.\.\.")
OUTPUT_SUFFIXES = (".js", ".d.ts", ".map", ".tsbuildinfo")
MODES = ("watch", "watch-symlink", "build-watch", "build-watch-symlink")


def write_json(path, value):
    Path(path).write_bytes(canonical(value) + b"\n")


def normalized(data, root, *, roots=None):
    # Resolve the longest name first: /var may alias /private/var on Darwin.
    names = sorted({str(path).encode() for path in (roots or (root, root.resolve()))}, key=len, reverse=True)
    for name in names:
        data = data.replace(name, b"<root>")
    # Preserve terminal-control bytes; only the known cycle-start line below
    # is omitted when extracting diagnostics from the last completed cycle.
    return CLOCK.sub(rb"\1", data)


def settled_output(data, root, *, roots=None):
    data = normalized(data, root, roots=roots).replace(b"\r\n", b"\n")
    matches = list(SETTLED.finditer(data))
    if not matches:
        raise ValueError("watch output contains no completed cycle")
    last = matches[-1]
    if data[last.end():].strip():
        raise ValueError("watch output ends with an unfinished cycle")
    # Multiple native cycles may coalesce a burst differently. Compare the last
    # complete cycle, never discard diagnostics from within that cycle.
    start = matches[-2].end() if len(matches) > 1 else 0
    prefix = data[start:last.start()]
    lines = [line for line in prefix.splitlines() if line.strip() and not CYCLE.fullmatch(line)]
    return {"errors": int(last.group(1)), "diagnostics": b"\n".join(lines).decode("utf-8"),
            "status": last.group().decode("ascii")}


def outputs(root, *, normalization_roots=None):
    result = {}
    for path in sorted(root.rglob("*")):
        if (path.is_file() and path.name.endswith(OUTPUT_SUFFIXES)
                and "node_modules" not in path.relative_to(root).parts
                and path.relative_to(root).parts[0] not in ("src", "lib.d.ts")):
            result[path.relative_to(root).as_posix()] = normalized(path.read_bytes(), root, roots=normalization_roots).hex()
    return result


def put(root, path, text):
    target = root / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)


def prepare(root):
    root.mkdir(parents=True, exist_ok=True)
    document = corpus.read_document()
    put(root, "lib.d.ts", document["library_text"])
    put(root, "base.json", json.dumps({"compilerOptions": {"strict": True, "noUnusedParameters": False}}))
    put(root, "tsconfig.json", json.dumps({"extends": "./base.json", "compilerOptions": {
        "noLib": True, "module": "nodenext", "target": "es2020", "composite": True,
        "outDir": "./out", "rootDir": ".", "tsBuildInfoFile": "./out/state.tsbuildinfo",
        "preserveWatchOutput": True}, "include": ["lib.d.ts", "src/**/*"]}))
    put(root, "src/value.ts", "export const value: number = 1;\n")
    put(root, "node_modules/pkg/package.json", '{"name":"pkg","version":"1.0.0","types":"a.d.ts"}\n')
    put(root, "node_modules/pkg/a.d.ts", "export interface Box { value: number }\n")
    put(root, "node_modules/pkg/b.d.ts", "export interface Box { value: string }\n")
    put(root, "src/main.ts", main_text())


def main_text(extra="", use=""):
    return ('import { value } from "./value";\nimport type { Box } from "pkg";\n'
            + extra + 'const unused = 1;\nexport const answer: number = value' + use + ';\n'
            'export const box: Box = { value: 1 };\nexport function identity(unused: number) { return answer; }\n')


def apply_step(root, index):
    if index == 0:
        return "initial"
    if index == 1:
        put(root, "src/value.ts", "export const value: number = 2;\n")
        return "modified source"
    if index == 2:
        put(root, "src/main.ts", main_text('import { extra } from "./future/extra";\n', " + extra"))
        return "failed import in absent directory"
    if index == 3:
        put(root, "src/future/extra.ts", "export const extra: number = 3;\n")
        return "create absent directory and resolve import"
    if index == 4:
        (root / "src/future/extra.ts").unlink()
        return "delete imported source"
    if index == 5:
        put(root, "src/future/extra.ts", "export const extra: number = 3;\n")
        (root / "src/future/extra.ts").rename(root / "src/future/renamed.ts")
        put(root, "src/main.ts", main_text('import { extra } from "./future/renamed";\n', " + extra"))
        return "rename imported source and update reference"
    if index == 6:
        config = strict_json_loads((root / "tsconfig.json").read_bytes())
        config["compilerOptions"]["noUnusedLocals"] = True
        put(root, "tsconfig.json", json.dumps(config))
        return "compiler option config edit"
    if index == 7:
        put(root, "base.json", json.dumps({"compilerOptions": {"strict": True, "noUnusedParameters": True}}))
        return "extended config edit"
    if index == 8:
        put(root, "node_modules/pkg/package.json", '{"name":"pkg","version":"1.0.0","types":"b.d.ts"}\n')
        return "package json resolution edit"
    if index == 9:
        for value in range(10):
            put(root, "src/value.ts", f"export const value: number = {value};\n")
        return "burst of edits"
    raise ValueError("unknown live edit")


class WatchSession:
    def __init__(self, executable, root, directory, arguments):
        self.directory = directory
        directory.mkdir(parents=True)
        self.root = root
        self.stdout = bytearray()
        self.stderr = bytearray()
        self.selector = selectors.DefaultSelector()
        env = os.environ.copy()
        env.pop("FORCE_COLOR", None)
        env["NO_COLOR"] = "1"
        self.process = subprocess.Popen([str(executable), *arguments], cwd=root, env=env,
                                        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        for name, pipe in (("stdout", self.process.stdout), ("stderr", self.process.stderr)):
            os.set_blocking(pipe.fileno(), False)
            self.selector.register(pipe, selectors.EVENT_READ, name)
        write_json(directory / "invocation.json", {"command": [str(executable), *arguments],
                   "cwd": str(root), "physical_cwd": str(root.resolve()),
                   "environment": {"NO_COLOR": "1", "FORCE_COLOR": None},
                   "settle_timeout_seconds": 30, "quiet_seconds": 1.2, "termination_timeout_seconds": 10})

    def drain(self, wait):
        changed = False
        for key, _ in self.selector.select(wait):
            data = os.read(key.fileobj.fileno(), 65536)
            if data:
                getattr(self, key.data).extend(data)
                changed = True
            else:
                self.selector.unregister(key.fileobj)
                key.fileobj.close()
        return changed

    def settle(self, offset, timeout=30.0):
        deadline = time.monotonic() + timeout
        quiet = time.monotonic()
        while time.monotonic() < deadline:
            if self.drain(0.1):
                quiet = time.monotonic()
            if self.process.poll() is not None:
                raise ValueError(f"watch process exited {self.process.returncode} before cancellation")
            if SETTLED.search(self.stdout[offset:]) and time.monotonic() - quiet >= 1.2:
                if self.stderr:
                    raise ValueError("watch process wrote stderr")
                fragment = normalized(bytes(self.stdout[offset:]), self.root)
                last = list(SETTLED.finditer(fragment))[-1]
                if fragment[last.end():].strip():
                    continue  # An additional rebuild has started but has not settled yet.
                return settled_output(bytes(self.stdout[offset:]), self.root)
        raise ValueError("watch process did not complete a cycle within 30 seconds")

    def close(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
        deadline = time.monotonic() + 10
        while self.process.poll() is None and time.monotonic() < deadline:
            self.drain(0.1)
        forced = self.process.poll() is None
        if forced:
            self.process.kill()
        self.process.wait()
        while self.selector.get_map():
            self.drain(0.05)
        self.selector.close()
        (self.directory / "stdout").write_bytes(self.stdout)
        (self.directory / "stderr").write_bytes(self.stderr)
        result = {"status": self.process.returncode, "forced": forced}
        write_json(self.directory / "termination.json", result)
        return result


def watch(executable, directory, *, build, symlink):
    directory.mkdir(parents=True)
    # Use a physically lowercase root on Darwin. The pinned Go watcher stores
    # case-folded subscriptions but compares physical FSEvents paths as bytes;
    # /Users/... therefore does not deliver events in the pin. Preserve this
    # known native limitation in the invocation instead of patching the oracle.
    temp_parent = "/private/tmp" if sys.platform == "darwin" else None
    temporary = tempfile.TemporaryDirectory(prefix="tsr-phase4-live-", dir=temp_parent)
    root = Path(temporary.name) / "real"
    prepare(root)
    cwd = root
    if symlink:
        cwd = Path(temporary.name) / "linked"
        cwd.symlink_to(root, target_is_directory=True)
    arguments = (["-b", "--watch"] if build else ["--watch"]) + ["--pretty", "false"]
    session = WatchSession(executable, cwd, directory / "process", arguments)
    rows, error = [], None
    try:
        for index in range(10):
            offset = len(session.stdout)
            name = apply_step(root, index)
            output = session.settle(offset)
            end = len(session.stdout)
            step = directory / f"step-{index}"
            step.mkdir()
            (step / "stdout").write_bytes(session.stdout[offset:end])
            (step / "stderr").write_bytes(session.stderr)
            shutil.copytree(root, step / "project")
            write_json(step / "observation.json", {"index": index, "step": name,
                       "stdout_start": offset, "stdout_end": end})
            rows.append({"step": name, "output": output, "files": outputs(root)})
            write_json(directory / "rows.json", rows)
    except (OSError, ValueError) as problem:
        error = str(problem)
    finally:
        termination = session.close()
        shutil.copytree(root, directory / "project", symlinks=True)
        temporary.cleanup()
    result = {"rows": rows, "termination": termination, "error": error}
    write_json(directory / "result.json", result)
    return result


def same_watch(go, rust):
    return (go["error"] is None and rust["error"] is None
            and len(go["rows"]) == len(rust["rows"]) == 10 and go["rows"] == rust["rows"]
            and not go["termination"]["forced"] and not rust["termination"]["forced"]
            and go["termination"]["status"] == rust["termination"]["status"]
            and go["termination"]["status"] in (0, -signal.SIGINT, 128 + signal.SIGINT))


def run(args):
    import phase4_native as native
    directory = args.output.resolve()
    build_dir = getattr(args, "build", None)
    if build_dir:
        build_dir = build_dir.resolve()
        build_record = native.verify_build(build_dir, expected_host=native.host())
        binaries = {runtime: build_dir / (runtime + "-executable") for runtime in ("go", "rust")}
        if getattr(args, "go_bin", None) or getattr(args, "rust_bin", None):
            raise ValueError("--build supplies both executables; do not combine it with --go-bin/--rust-bin")
    else:
        if not args.go_bin or not args.rust_bin:
            raise ValueError("supply --build, or both --go-bin and --rust-bin for a development capture")
        binaries = {"go": args.go_bin.resolve(), "rust": args.rust_bin.resolve()}
    directory.mkdir(parents=True, exist_ok=False)
    before = native.sources()
    images = {}
    for runtime, source in binaries.items():
        image = directory / (runtime + "-executable")
        shutil.copy2(source, image)
        binaries[runtime] = image
        images[runtime] = digest(image.read_bytes())
    report = {"version": 2, "host": native.host(), "images": images, "sources": before, "witnesses": {},
              "repo_root": build_record["repo_root"] if build_dir else str(ROOT),
              "build_root": str(build_dir) if build_dir else None, "directory": str(directory),
              "build_sha256": digest((build_dir / "report.json").read_bytes()) if build_dir else None}
    for build in (False, True):
        for symlink in (False, True):
            name = ("build-watch" if build else "watch") + ("-symlink" if symlink else "")
            if args.only and name not in args.only:
                continue
            observed = {runtime: watch(image, directory / name / runtime, build=build, symlink=symlink)
                        for runtime, image in binaries.items()}
            report["witnesses"][name] = {**observed, "pass": same_watch(observed["go"], observed["rust"])}
            write_json(directory / "report.json", report)
            print(name + ": " + ("pass" if report["witnesses"][name]["pass"] else "different or incomplete"), flush=True)
    report["source_stable"] = before == native.sources()
    report["pass"] = report["source_stable"] and bool(report["witnesses"]) and all(row["pass"] for row in report["witnesses"].values())
    report["artifacts"] = native.artifacts(directory)
    write_json(directory / "report.json", report)
    return 0 if report["pass"] else 1


def verify_witnesses(build_dir, capture_dir, *, expected_host=None):
    """Recompute the full live witness from retained streams and file snapshots."""
    import phase4_native as native
    try:
        return _verify_witnesses(Path(build_dir).resolve(), Path(capture_dir).resolve(), expected_host)
    except (OSError, KeyError, TypeError, AttributeError, IndexError, UnicodeError) as error:
        raise ValueError("malformed or incomplete live capture") from error


def _verify_witnesses(build_dir, directory, expected_host):
    import phase4_native as native
    report = native.capture_report(build_dir, directory, expected_host=expected_host)
    capture_root = native.recorded_root(report["directory"])
    for runtime in ("go", "rust"):
        image = directory / (runtime + "-executable")
        if digest(image.read_bytes()) != report["images"][runtime] or not os.access(image, os.X_OK):
            raise ValueError("live copied executable differs from the authenticated build")
    groups = report["witnesses"]
    if not groups:
        native.check_layout(directory, {"go-executable", "rust-executable"})
        return {"metrics": {}, "identities": {}, "details": {"host": report["host"]}}
    if set(groups) != set(MODES):
        raise ValueError("incomplete live mode inventory")
    passed = True
    fixed, trees = {"go-executable", "rust-executable"}, set()
    for mode in MODES:
        group = groups[mode]
        if not isinstance(group, dict) or set(group) != {"go", "rust", "pass"}:
            raise ValueError("incomplete paired live observation")
        observed = {}
        for runtime in ("go", "rust"):
            base = directory / mode / runtime
            prefix = f"{mode}/{runtime}"
            fixed.update(prefix + "/" + name for name in ("result.json", "rows.json", "process/invocation.json",
                                                         "process/stdout", "process/stderr", "process/termination.json"))
            trees.add(prefix + "/project")
            recorded = native.read_json(base / "result.json")
            rows = native.read_json(base / "rows.json")
            if (recorded != group[runtime] or not isinstance(rows, list) or len(rows) != 10
                    or recorded.get("rows") != rows or recorded.get("error") is not None):
                raise ValueError("incomplete live edit schedule")
            invocation = native.read_json(base / "process/invocation.json")
            cwd, physical = Path(invocation["cwd"]), Path(invocation["physical_cwd"])
            symlink = mode.endswith("-symlink")
            if (not cwd.is_absolute() or not physical.is_absolute() or ".." in cwd.parts
                    or ".." in physical.parts or physical.name != "real"
                    or cwd.parent != physical.parent or cwd.name != ("linked" if symlink else "real")
                    or not physical.parent.name.startswith("tsr-phase4-live-")):
                raise ValueError("unexpected live project root")
            arguments = (["-b", "--watch"] if mode.startswith("build-") else ["--watch"]) + ["--pretty", "false"]
            expected_invocation = {"command": [str(capture_root / (runtime + "-executable")), *arguments],
                                   "cwd": str(cwd), "physical_cwd": str(physical),
                                   "environment": {"NO_COLOR": "1", "FORCE_COLOR": None},
                                   "settle_timeout_seconds": 30, "quiet_seconds": 1.2,
                                   "termination_timeout_seconds": 10}
            if invocation != expected_invocation:
                raise ValueError("unexpected live process invocation")
            stdout = (base / "process/stdout").read_bytes()
            stderr = (base / "process/stderr").read_bytes()
            termination = native.read_json(base / "process/termination.json")
            if (not isinstance(termination, dict) or set(termination) != {"status", "forced"}
                    or type(termination["status"]) is not int or type(termination["forced"]) is not bool
                    or recorded["termination"] != termination):
                raise ValueError("malformed live termination outcome")
            replayed, end = [], 0
            with tempfile.TemporaryDirectory(prefix="phase4-live-replay-") as temporary:
                expected = Path(temporary) / "project"
                prepare(expected)
                for index in range(10):
                    name = apply_step(expected, index)
                    step = base / f"step-{index}"
                    step_prefix = prefix + f"/step-{index}"
                    fixed.update(step_prefix + "/" + name for name in ("stdout", "stderr", "observation.json"))
                    trees.add(step_prefix + "/project")
                    meta = native.read_json(step / "observation.json")
                    raw = (step / "stdout").read_bytes()
                    if (not isinstance(meta, dict) or set(meta) != {"index", "step", "stdout_start", "stdout_end"}
                            or any(type(meta[key]) is not int for key in ("index", "stdout_start", "stdout_end"))
                            or meta["index"] != index or meta["step"] != name
                            or meta["stdout_start"] != end or meta["stdout_end"] != end + len(raw)
                            or stdout[end:meta["stdout_end"]] != raw or (step / "stderr").read_bytes()):
                        raise ValueError("live step stream or ordering disagrees with the process")
                    end = meta["stdout_end"]
                    native.check_inputs(step / "project", expected)
                    replayed.append({"step": name, "output": settled_output(raw, cwd, roots=[cwd, physical]),
                                     "files": outputs(step / "project", normalization_roots=[cwd, physical])})
                if native.files(base / "project") != native.files(base / "step-9/project"):
                    raise ValueError("live final files differ from the settled observation")
            if stdout[end:].strip() or rows != replayed:
                raise ValueError("live summary disagrees with raw output")
            observed[runtime] = {"rows": replayed, "termination": termination, "error": None}
            passed &= not stderr and any(name.endswith(".tsbuildinfo") for name in replayed[0]["files"])
        passed &= same_watch(observed["go"], observed["rust"])
    identity = digest((directory / "report.json").read_bytes())
    native.check_layout(directory, fixed, trees)
    return {"metrics": {"live_watch_parity": bool(passed)}, "identities": {"live_watch_parity": identity},
            "details": {"host": report["host"]}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=Path, help="authenticated phase4_native.py build directory")
    parser.add_argument("--go-bin", type=Path)
    parser.add_argument("--rust-bin", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--only", action="append", choices=MODES)
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
