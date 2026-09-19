#!/usr/bin/env python3
"""Check producers for status/runs.toml: fmt, clippy, deny and selftest.

Each subcommand runs one reviewed command set in the repository root and
prints exactly one JSON object to stdout; all tool output goes to stderr.
A measured failure (the tool ran and reported problems) is a false metric
with exit code 0. A missing tool or an interrupted process exits non-zero,
so the run is recorded as a failed producer rather than as a measurement.
Consumers must enforce the recorded booleans with `cargo xtask check-metrics`;
successful capture alone does not mean that a check passed.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path


def run(args, cwd):
    print("+ " + " ".join(args), file=sys.stderr)
    return subprocess.run(args, cwd=cwd, stdout=sys.stderr, stderr=sys.stderr, check=False).returncode


def require(args, cwd):
    """The tool itself must be present and answer --version; otherwise there is no measurement."""
    try:
        completed = subprocess.run(args, cwd=cwd, stdout=sys.stderr, stderr=sys.stderr, check=False)
    except OSError as error:
        raise OSError(f"{args[0]} is not available: {error}") from error
    if completed.returncode != 0:
        raise OSError(f"{' '.join(args)} failed; the tool is not usable")


def fmt(root):
    require(["cargo", "fmt", "--version"], root)
    return {"clean": run(["cargo", "fmt", "--all", "--check"], root) == 0}


def clippy(root):
    require(["cargo", "clippy", "--version"], root)
    command = ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings"]
    return {"clean": run(command, root) == 0}


def deny(root):
    require(["cargo", "deny", "--version"], root)
    return {"clean": run(["cargo", "deny", "--locked", "check"], root) == 0}


def selftest(root):
    require(["cargo", "--version"], root)
    require(["go", "version"], root)
    passed = run(["cargo", "test", "--package", "xtask", "--locked"], root) == 0
    passed = run([sys.executable, "scripts/run_tests.py"], root) == 0 and passed
    return {"pass": passed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("check", choices=("fmt", "clippy", "deny", "selftest"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        metrics = {"fmt": fmt, "clippy": clippy, "deny": deny, "selftest": selftest}[args.check](root)
    except (OSError, subprocess.SubprocessError) as error:
        print(f"{args.check} check failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"metrics": metrics}, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
