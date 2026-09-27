#!/usr/bin/env python3
"""Record the pinned tsgo diagnostics for every C4 case (lightweight native observations).

A case checks one root file of this directory under its own flags, with
`--noEmit --target esnext --ignoreConfig --pretty false --skipLibCheck`; the
other files it lists (imported modules, the `react` package under
`node_modules`) are bound by digest. `<case>.native.json` keeps the parsed
diagnostics (file as printed, line, column, code, message with chain lines
joined by newlines), bound to the pin, the source digests and the executable
digest. Only a clean run is an expectation, as for the C3 recorder.
"""
import hashlib, json, pathlib, re, subprocess, sys
FIX = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(__file__).resolve().parents[5]
TSGO = ROOT / "target/tsgo"
pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
head = subprocess.check_output(["git", "-C", str(ROOT / "upstream/tsc"), "rev-parse", "HEAD"], text=True).strip()
if head != pin:
    sys.exit(f"upstream/tsc is at {head}, not the pin {pin}")
sha = lambda b: hashlib.sha256(b).hexdigest()
LINE = re.compile(r"^(?:(?P<file>[^(]+)\((?P<line>\d+),(?P<col>\d+)\): )?error TS(?P<code>\d+): (?P<message>.*)$")
BASE = ["--noEmit", "--target", "esnext", "--ignoreConfig", "--pretty", "false", "--skipLibCheck"]
manifest = json.loads((FIX / "fixtures.json").read_text())
only = set(sys.argv[1:])
for case, spec in manifest.items():
    if only and case not in only:
        continue
    command = ["tsc", *BASE, *spec["flags"], spec["root"]]
    run = subprocess.run([str(TSGO), *command[1:]], cwd=FIX, capture_output=True, text=True)
    text = run.stdout
    diagnostics, current, unparsed = [], None, []
    for line in text.split("\n"):
        m = LINE.match(line)
        if m:
            # A global diagnostic has no file position: file, line and column are null.
            current = {"file": m["file"], "line": int(m["line"]) if m["line"] else None,
                       "column": int(m["col"]) if m["col"] else None, "code": int(m["code"]),
                       "message": m["message"]}
            diagnostics.append(current)
        elif current is not None and line.startswith("  "):
            current["message"] += "\n" + line
        elif line.strip():
            unparsed.append(line)
    problems = []
    if run.stderr:
        problems.append("stderr: " + run.stderr.strip()[:800])
    if unparsed:
        problems.append("unparsed output: " + " | ".join(unparsed)[:800])
    if run.returncode not in (0, 2) or (run.returncode == 0) != (not diagnostics):
        problems.append(f"exit code {run.returncode} with {len(diagnostics)} diagnostics")
    if problems:
        sys.exit(f"{case}: " + "; ".join(problems))
    sources = {name: sha((FIX / name).read_bytes()) for name in [spec["root"], *spec.get("files", [])]}
    record = {"pin": pin, "sources_sha256": sources, "native_executable_sha256": sha(TSGO.read_bytes()),
              "native_command": command, "native_output_sha256": sha(text.encode()), "exit_code": run.returncode,
              "native_stderr": run.stderr, "diagnostics": diagnostics}
    (FIX / (case + ".native.json")).write_text(json.dumps(record, indent=2) + "\n")
    print(f"{case}: {len(diagnostics)} diagnostics, exit {run.returncode}")
