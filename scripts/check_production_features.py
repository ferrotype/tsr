#!/usr/bin/env python3
"""Build production targets without the harnesses' opt-in compatibility APIs.

Workspace/all-features builds unify normal dependency features and cannot prove
this boundary. Select only workspace packages under crates/, then inspect the
actual compiler artifacts as well as requiring compilation to succeed.
"""

import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
FORBIDDEN_FEATURE = "go-slice-compat"


def check(root=ROOT):
    root = root.resolve()
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=root, text=True,
    ))
    members = set(metadata["workspace_members"])
    packages = sorted((package for package in metadata["packages"]
                       if package["id"] in members
                       and Path(package["manifest_path"]).resolve().is_relative_to(root / "crates")),
                      key=lambda package: package["name"])
    core = [package for package in packages if package["name"] == "tsr_core"]
    if len(core) != 1:
        raise ValueError("production package selection must include tsr_core exactly once")
    command = ["cargo", "check", "--locked", "--message-format=json"]
    for package in packages:
        command.extend(["--package", package["name"]])
    result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, text=True, check=False)
    saw_core = False
    forbidden = False
    for line in result.stdout.splitlines():
        event = json.loads(line)
        if event.get("reason") == "compiler-message":
            rendered = event["message"].get("rendered")
            if rendered:
                print(rendered, file=sys.stderr, end="")
        if event.get("reason") == "compiler-artifact" and event["package_id"] == core[0]["id"]:
            saw_core = True
            forbidden |= FORBIDDEN_FEATURE in event["features"]
    result.check_returncode()
    if not saw_core:
        raise ValueError("production build did not report a tsr_core artifact")
    if forbidden:
        raise ValueError(f"production build enabled tsr_core/{FORBIDDEN_FEATURE}")
    return len(packages)


if __name__ == "__main__":
    try:
        count = check()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
    print(f"{count} production packages compile without tsr_core/{FORBIDDEN_FEATURE}")
