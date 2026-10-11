#!/usr/bin/env python3
"""Build and test the pinned compiler budget consumer, retaining its identity."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
from s04 import go_environment, verified_upstream


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_record():
    files = sorted(HERE.rglob("*.go")) + [HERE / "build.py", ROOT / "data/upstream.json",
                                             ROOT / "data/s04/toolchains.toml",
                                             ROOT / "scripts/s04.py", ROOT / "scripts/s04_runtime.py",
                                             ROOT / "scripts/tracking-bootstrap.py"]
    return {str(p.relative_to(ROOT)): digest(p) for p in files}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if any(p.casefold() == "target" for p in output.parts):
        parser.error("retained artifacts must be outside target")
    output.mkdir(parents=True, exist_ok=False)
    before = source_record()
    upstream = verified_upstream()
    env = go_environment()
    pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
    commands = []
    with tempfile.TemporaryDirectory(prefix="phase7-budget-go-") as temporary:
        checkout = Path(temporary)
        archive = subprocess.check_output(["git", "archive", pin, "tsc/go.mod", "tsc/go.sum", "tsc/internal"], cwd=upstream)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
            stream.extractall(checkout, filter="data")
        entry = checkout / "tsc/cmd/phase7-budget"
        for name in ("body", "native", "js"):
            shutil.copytree(HERE / name, entry / name)
        for command, environment in [
            (["go", "test", "-mod=readonly", "./cmd/phase7-budget/body"], env),
            (["go", "build", "-trimpath", "-mod=readonly", "-ldflags=-s -w", "-o", str(output / "budget-go"), "./cmd/phase7-budget/native"], env),
            (["go", "build", "-trimpath", "-mod=readonly", "-ldflags=-s -w", "-o", str(output / "budget-go.wasm"), "./cmd/phase7-budget/js"], dict(env, GOOS="js", GOARCH="wasm")),
        ]:
            commands.append(command)
            subprocess.run(command, cwd=checkout / "tsc", env=environment, check=True)
        goroot = Path(subprocess.check_output(["go", "env", "GOROOT"], env=env, text=True).strip())
        shutil.copy2(goroot / "lib/wasm/wasm_exec.js", output / "wasm_exec.cjs")
    verified_upstream()
    if source_record() != before:
        raise ValueError("Go consumer sources changed during build")
    files = (output / "budget-go", output / "budget-go.wasm", output / "wasm_exec.cjs")
    record = {"version": 1, "mode": "phase7-budget", "measurement_contract": 2,
              "performance_eligible": True, "source_guard": True, "build_exit": 0,
              "go_gc_policy": "runtime.GC after prepare/prepare_end and checkpoint",
              "pin": pin, "sources": before, "commands": commands,
              "go": subprocess.check_output(["go", "version"], env=env, text=True).strip(),
              "artifacts": {p.name: {"sha256": digest(p), "bytes": p.stat().st_size} for p in files}}
    (output / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    print(output)


if __name__ == "__main__":
    main()
