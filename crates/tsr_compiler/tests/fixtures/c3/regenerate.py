#!/usr/bin/env python3
"""Record the pinned tsgo diagnostics for every C3 fixture (lightweight native observations).

Each fixture is checked alone with `--noEmit --target esnext --ignoreConfig --pretty false`
plus its options from fixtures.json; `.native.txt` keeps the raw output and `.native.json`
the parsed diagnostics (line, column, code, message with chain lines joined by newlines),
bound to the pin, the source digest and the executable digest.
"""
import hashlib, json, pathlib, re, subprocess, sys
FIX = pathlib.Path(__file__).resolve().parent
ROOT = FIX.parents[4]
TSGO = ROOT / "target/tsgo"
pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
head = subprocess.check_output(["git", "-C", str(ROOT / "upstream/tsc"), "rev-parse", "HEAD"], text=True).strip()
if head != pin:
    sys.exit(f"upstream/tsc is at {head}, not the pin {pin}")
sha = lambda b: hashlib.sha256(b).hexdigest()
LINE = re.compile(r"^(?P<file>[^(]+)\((?P<line>\d+),(?P<col>\d+)\): error TS(?P<code>\d+): (?P<message>.*)$")
manifest = json.loads((FIX / "fixtures.json").read_text())
for name, flags in manifest.items():
    command = ["tsc", "--noEmit", "--target", "esnext", "--ignoreConfig", "--pretty", "false", *flags, name]
    run = subprocess.run([str(TSGO), *command[1:]], cwd=FIX, capture_output=True, text=True)
    text = run.stdout
    diagnostics, current = [], None
    for line in text.split("\n"):
        m = LINE.match(line)
        if m:
            current = {"line": int(m["line"]), "column": int(m["col"]), "code": int(m["code"]), "message": m["message"]}
            diagnostics.append(current)
        elif current is not None and line.startswith("  "):
            current["message"] += "\n" + line
    (FIX / (name + ".native.txt")).write_text(text)
    record = {"pin": pin, "source_sha256": sha((FIX / name).read_bytes()), "native_executable_sha256": sha(TSGO.read_bytes()),
              "native_command": command, "native_output_sha256": sha(text.encode()), "exit_code": run.returncode,
              "diagnostics": diagnostics}
    (FIX / (name + ".native.json")).write_text(json.dumps(record, indent=2) + "\n")
    print(f"{name}: {len(diagnostics)} diagnostics, exit {run.returncode}")
