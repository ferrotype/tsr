#!/usr/bin/env python3
"""Paired Phase 4 native CLI witnesses, preserving raw child output.

This development runner does not promote a supplied executable to ledger evidence:
its report binds both images and current sources, but build provenance must also be
provided by X7. No source edits, capture replacement, or child build is implicit.
Only timestamps and the two explicit temporary-root spellings are normalized.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
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
import phase4_corpus as corpus  # noqa: E402

SETTLED = re.compile(rb"Found ([0-9]+) errors?\. Watching for file changes\.")
CLEAR = rb"\x1b\[2J\x1b\[3J\x1b\[H"
CLOCK = re.compile(rb"^(" + CLEAR + rb")?(?:\[(?:[0-9]{1,2}:){2}[0-9]{2}(?: [AP]M)?\] ?|(?:[0-9]{1,2}:){2}[0-9]{2}(?: [AP]M)? - )", re.MULTILINE)
CYCLE = re.compile(rb"(?:" + CLEAR + rb")?(?:Starting compilation in watch mode|File change detected\. Starting incremental compilation)\.\.\.")
OUTPUT_SUFFIXES = (".js", ".d.ts", ".map", ".tsbuildinfo")


def write_json(path, value):
    Path(path).write_bytes(canonical(value) + b"\n")


def normalized(data, root):
    # Resolve the longest name first: /var may alias /private/var on Darwin.
    names = sorted({str(root).encode(), str(root.resolve()).encode()}, key=len, reverse=True)
    for name in names:
        data = data.replace(name, b"<root>")
    # Preserve terminal-control bytes; only the known cycle-start line below
    # is omitted when extracting diagnostics from the last completed cycle.
    return CLOCK.sub(rb"\1", data)


def settled_output(data, root):
    data = normalized(data, root).replace(b"\r\n", b"\n")
    matches = list(SETTLED.finditer(data))
    if not matches:
        raise ValueError("watch output contains no completed cycle")
    last = matches[-1]
    # Multiple native cycles may coalesce a burst differently. Compare the last
    # complete cycle, never discard diagnostics from within that cycle.
    start = matches[-2].end() if len(matches) > 1 else 0
    prefix = data[start:last.start()]
    lines = [line for line in prefix.splitlines() if line.strip() and not CYCLE.fullmatch(line)]
    return {"errors": int(last.group(1)), "diagnostics": b"\n".join(lines).decode("utf-8"),
            "status": last.group().decode("ascii")}


def outputs(root):
    result = {}
    for path in sorted(root.rglob("*")):
        if (path.is_file() and path.name.endswith(OUTPUT_SUFFIXES)
                and "node_modules" not in path.relative_to(root).parts
                and path.relative_to(root).parts[0] not in ("src", "lib.d.ts")):
            result[path.relative_to(root).as_posix()] = normalized(path.read_bytes(), root).hex()
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
        config = json.loads((root / "tsconfig.json").read_text())
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
        write_json(directory / "invocation.json", {"arguments": arguments, "cwd": str(root)})

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
        return {"status": self.process.returncode, "forced": forced}


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
            and go["termination"]["status"] == rust["termination"]["status"])


def run(args):
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    before = corpus.sources()
    binaries = {"go": args.go_bin.resolve(), "rust": args.rust_bin.resolve()}
    images = {}
    for runtime, source in binaries.items():
        image = directory / (runtime + "-executable")
        shutil.copy2(source, image)
        binaries[runtime] = image
        images[runtime] = digest(image.read_bytes())
    report = {"version": 1, "host": platform.platform(), "images": images, "sources": before, "witnesses": {}}
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
    report["source_stable"] = before == corpus.sources()
    report["pass"] = bool(report["witnesses"]) and all(row["pass"] for row in report["witnesses"].values())
    report["artifacts"] = {path.relative_to(directory).as_posix(): digest(path.read_bytes())
                           for path in sorted(directory.rglob("*")) if path.is_file()
                           and path.name != "report.json" and "real" not in path.relative_to(directory).parts}
    write_json(directory / "report.json", report)
    return 0 if report["pass"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--go-bin", type=Path, required=True)
    parser.add_argument("--rust-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--only", action="append", choices=("watch", "watch-symlink", "build-watch", "build-watch-symlink"))
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
