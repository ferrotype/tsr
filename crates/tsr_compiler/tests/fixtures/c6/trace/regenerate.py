#!/usr/bin/env python3
"""Record the pinned tsgo trace of the C6 tracing witness (docs/PHASE2-C6-plan.md, contract 7).

The witness is `lib.d.ts` (the global interfaces a `--noLib` program needs)
and `a.ts`, checked by the pinned tsgo with `--noEmit --ignoreConfig
--singleThreaded --noLib --generateTrace` in a temporary directory. The
normalized trace keeps the checker's events (the `check` and `checkTypes`
categories, begin, end and instant events), without timestamps, process or
thread ids; sampled `X` events are timing-dependent and dropped, as the pin's
deterministic mode never writes them. The program's own `checkSourceFiles`
event is the compiler's, not the checker's, and is dropped too. Paths are
rewritten from the temporary directory to the virtual root `/` the contract
loads the witness at. `native.json` keeps the events and the one checker's
type records, bound to the pin, the source digests and the executable digest.
"""
import hashlib, json, pathlib, shutil, subprocess, sys, tempfile
FIX = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(__file__).resolve().parents[6]
TSGO = ROOT / "target/tsgo"
FILES = ["lib.d.ts", "a.ts"]
pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
head = subprocess.check_output(["git", "-C", str(ROOT / "upstream/tsc"), "rev-parse", "HEAD"], text=True).strip()
if head != pin:
    sys.exit(f"upstream/tsc is at {head}, not the pin {pin}")
sha = lambda b: hashlib.sha256(b).hexdigest()
FLAGS = ["--noEmit", "--ignoreConfig", "--singleThreaded", "--noLib", "--generateTrace", "out"]


def normalize(value, prefixes):
    if isinstance(value, str):
        for prefix in prefixes:
            if value.startswith(prefix):
                return value[len(prefix):]
        return value
    if isinstance(value, list):
        return [normalize(item, prefixes) for item in value]
    if isinstance(value, dict):
        return {key: normalize(item, prefixes) for key, item in value.items()}
    return value


with tempfile.TemporaryDirectory() as directory:
    work = pathlib.Path(directory).resolve()
    for name in FILES:
        shutil.copyfile(FIX / name, work / name)
    run = subprocess.run([str(TSGO), *FLAGS, *FILES], cwd=work, capture_output=True, text=True)
    if run.returncode != 0:
        sys.exit("the witness does not check cleanly:\n" + run.stdout + run.stderr)
    prefixes = [str(work), str(work).lower()]
    events = []
    for event in json.loads((work / "out/trace.json").read_text()):
        if event["ph"] not in ("B", "E", "I") or event["cat"] not in ("check", "checkTypes"):
            continue
        if event["name"] == "checkSourceFiles":
            continue
        kept = {"ph": event["ph"], "cat": event["cat"], "name": event["name"]}
        if event.get("args"):
            kept["args"] = normalize(event["args"], prefixes)
        events.append(kept)
    legend = json.loads((work / "out/legend.json").read_text())
    if [entry["checkerId"] for entry in legend] != [0]:
        sys.exit("a single-threaded program must have exactly one checker")
    types = normalize(json.loads((work / "out/types_0.json").read_text()), prefixes)

record = {
    "pin": pin,
    "command": ["tsgo", *FLAGS, *FILES],
    "tsgo_sha256": sha(TSGO.read_bytes()),
    "sources": {name: sha((FIX / name).read_bytes()) for name in FILES},
    "events": events,
    "types": types,
}
(FIX / "native.json").write_text(json.dumps(record, indent=1, sort_keys=True) + "\n")
print(json.dumps({"events": len(events), "types": len(types)}))
