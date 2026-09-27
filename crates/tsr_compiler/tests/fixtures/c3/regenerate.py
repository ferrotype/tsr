#!/usr/bin/env python3
"""Record the pinned tsgo diagnostics for every C3 fixture (lightweight native observations).

Each fixture is checked alone with `--noEmit --target esnext --ignoreConfig --pretty false`
plus its options from fixtures.json; `.native.txt` keeps the raw output and `.native.json`
the parsed diagnostics (line, column, code, message with chain lines joined by newlines),
bound to the pin, the source digest and the executable digest.
"""
import hashlib, json, pathlib, re, subprocess, sys
# Records the fixtures of its own directory, or of the directory named as the
# first argument (other checkpoints' native fixtures reuse this recorder).
FIX = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(__file__).resolve().parents[5]
TSGO = ROOT / "target/tsgo"
pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
head = subprocess.check_output(["git", "-C", str(ROOT / "upstream/tsc"), "rev-parse", "HEAD"], text=True).strip()
if head != pin:
    sys.exit(f"upstream/tsc is at {head}, not the pin {pin}")
sha = lambda b: hashlib.sha256(b).hexdigest()
LINE = re.compile(r"^(?:(?P<file>[^(]+)\((?P<line>\d+),(?P<col>\d+)\): )?error TS(?P<code>\d+): (?P<message>.*)$")
manifest = json.loads((FIX / "fixtures.json").read_text())
for name, flags in manifest.items():
    command = ["tsc", "--noEmit", "--target", "esnext", "--ignoreConfig", "--pretty", "false", *flags, name]
    run = subprocess.run([str(TSGO), *command[1:]], cwd=FIX, capture_output=True, text=True)
    text = run.stdout
    diagnostics, current, unparsed = [], None, []
    for line in text.split("\n"):
        m = LINE.match(line)
        if m:
            # A global diagnostic has no file position: line and column are null.
            current = {"line": int(m["line"]) if m["line"] else None, "column": int(m["col"]) if m["col"] else None,
                       "code": int(m["code"]), "message": m["message"]}
            diagnostics.append(current)
        elif current is not None and line.startswith("  "):
            current["message"] += "\n" + line
        elif line.strip():
            unparsed.append(line)
    # Only a clean run is an expectation: nothing on stderr, every output line
    # parsed, and an exit code that agrees with the diagnostics (0 without,
    # 2 with). A crash or an unparsed error never becomes an empty expectation.
    problems = []
    if run.stderr:
        problems.append("stderr: " + run.stderr.strip()[:800])
    if unparsed:
        problems.append("unparsed output: " + " | ".join(unparsed)[:800])
    if run.returncode not in (0, 2) or (run.returncode == 0) != (not diagnostics):
        problems.append(f"exit code {run.returncode} with {len(diagnostics)} diagnostics")
    if problems:
        sys.exit(f"{name}: " + "; ".join(problems))
    (FIX / (name + ".native.txt")).write_text(text)
    record = {"pin": pin, "source_sha256": sha((FIX / name).read_bytes()), "native_executable_sha256": sha(TSGO.read_bytes()),
              "native_command": command, "native_output_sha256": sha(text.encode()), "exit_code": run.returncode,
              "native_stderr": run.stderr, "diagnostics": diagnostics}
    (FIX / (name + ".native.json")).write_text(json.dumps(record, indent=2) + "\n")
    print(f"{name}: {len(diagnostics)} diagnostics, exit {run.returncode}")
