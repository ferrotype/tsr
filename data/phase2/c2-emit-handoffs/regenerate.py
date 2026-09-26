#!/usr/bin/env python3
"""Reproduce only three native pre/post-emit errors and their causal Go stacks.

The overlay and Go cache are separate from canonical native captures. Pinned
checker logic is unchanged; observations are inserted at diagnostic production.
"""
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = pathlib.Path(__file__).resolve().parent
WORK = ROOT / "target/phase2/c2-emit-handoffs"
sys.path.insert(0, str(ROOT / "scripts"))
from s04 import go_environment, verified_upstream
from phase2_native import SCRIPT_INPUTS, overlay_sources, replace_exact


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def stacks_from_log(text):
    lines = text.splitlines()
    events = []
    case = phase = None
    for index, line in enumerate(lines):
        if line.startswith("=== RUN   TestPhase2Checker/"):
            case = line.removeprefix("=== RUN   TestPhase2Checker/")
        elif line.startswith("C2PHASE "):
            phase = line.removeprefix("C2PHASE ")
        elif line.startswith("C2STACK "):
            frames = []
            for j in range(index + 1, len(lines) - 1):
                value = lines[j]
                if value.startswith(("C2STACK", "C2PHASE", "=== ", "--- ")):
                    break
                if value and not value[0].isspace() and lines[j + 1].startswith("\t"):
                    frames.append({"function": value.rsplit("(", 1)[0],
                                   "source": lines[j + 1].strip().split(" +0x")[0]})
            events.append({"id": case, "phase": phase,
                           "site": line.removeprefix("C2STACK "), "frames": frames})
    return events


upstream = verified_upstream()
WORK.mkdir(parents=True, exist_ok=True)
sources = overlay_sources(upstream)
checker = (upstream / "tsc/internal/checker/checker.go").read_text()
checker = replace_exact(checker,
    "diagnostic := c.error(errorNode, diagnostics.Type_parameter_0_has_a_circular_constraint, c.TypeToString(t))",
    'c2EmitStack("2313", c.currentNode, errorNode)\n\t\t\t\tdiagnostic := c.error(errorNode, diagnostics.Type_parameter_0_has_a_circular_constraint, c.TypeToString(t))')
checker = replace_exact(checker,
    "c.error(c.currentNode, diagnostics.Type_of_property_0_circularly_references_itself_in_mapped_type_1,",
    'c2EmitStack("2615", c.currentNode, nil)\n\t\t\tc.error(c.currentNode, diagnostics.Type_of_property_0_circularly_references_itself_in_mapped_type_1,')
sources["checker/checker.go"] = checker
sources["checker/c2_emit_stack.go"] = (HERE / "observer.go").read_text()
key = "testutil/harnessutil/harnessutil.go"
sources[key] = replace_exact(sources[key],
    "preProgram := createProgram(host, preConfig)",
    'fmt.Println("C2PHASE PRE")\n\tpreProgram := createProgram(host, preConfig)')
sources[key] = replace_exact(sources[key],
    "emitResult := postProgram.Emit(ctx, compiler.EmitOptions{})",
    'fmt.Println("C2PHASE EMIT")\n\temitResult := postProgram.Emit(ctx, compiler.EmitOptions{})\n\tfmt.Println("C2PHASE POST")')
replacements = {}
for name, source in sources.items():
    destination = WORK / "overlay" / name
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(source)
    replacements[str(upstream / "tsc/internal" / name)] = str(destination)
write_json(WORK / "overlay.json", {"Replace": replacements})
env = go_environment()
# Reuse the separate observer cache; do not write the host's canonical cache.
env.update(GOCACHE=str(ROOT / "target/phase2/c2-property-elision-native/go-build"),
           PHASE2_REQUESTS=str(HERE / "requests.json"),
           PHASE2_OUTPUT=str(WORK / "observations.ndjson"),
           PHASE2_SUMMARY=str(WORK / "summary.json"))
command = ["go", "test", "-mod=readonly", "-trimpath",
           "-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath=" + str(WORK / "unmatched"),
           "-overlay", str(WORK / "overlay.json"), "./internal/testrunner",
           "-run", "^TestPhase2Checker$", "-count=1", "-v", "-timeout=2m"]
with (WORK / "run.log").open("w") as log:
    subprocess.run(command, cwd=upstream / "tsc", env=env, stdout=log, stderr=log, check=True)
rows = [json.loads(line) for line in (WORK / "observations.ndjson").read_text().splitlines()]
requests = json.loads((HERE / "requests.json").read_text())
assert [r["id"] for r in rows] == [r["id"] for r in requests]
assert len(rows) == 3 and all(r["state"] == "executed" for r in rows)
observations = []
for row in rows:
    name = row["id"].rsplit("/", 1)[1].split(".ts#")[0]
    frozen = json.loads((ROOT / "crates/tsr_compiler/tests/fixtures/c2/constraint_gaps" / (name + ".native.json")).read_text())
    assert row["error_pre_diagnostics"] == frozen["diagnostics_before_emit"]
    assert row["error_post_diagnostics"] == frozen["diagnostics_after_emit"]
    assert row["errors"] == frozen["errors"]
    observations.append({"id":row["id"], "raw_source_sha256":row["raw_sha256"],
        "loaded_source_sha256":row["loaded_sha256"], "emit_declarations":row["emit_declarations"],
        "diagnostics_before_emit":row["error_pre_diagnostics"],
        "diagnostics_after_emit":row["error_post_diagnostics"], "errors":row["errors"]})
write_json(HERE / "observations.json", observations)
stacks = stacks_from_log((WORK / "run.log").read_text())
for request in requests:
    events = [e for e in stacks if e["id"] == request["id"] and e["phase"] == "EMIT"]
    assert events
    expected = "ConstEnumInliningTransformer" if "incorrectRecursive" in request["id"] else "ImportElisionTransformer"
    assert all(any(expected in frame["function"] for frame in event["frames"]) for event in events)
write_json(HERE / "stacks.json", stacks)
summary = json.loads((WORK / "summary.json").read_text())
assert summary["rows"] == 3 and summary["single_threaded"] is True
assert summary["request_sha256"] == sha(HERE / "requests.json")
write_json(HERE / "provenance.json", {
    "version":1, "pin":json.loads((ROOT / "data/upstream.json").read_text())["pin"],
    "command":command, "go":{key:summary[key] for key in ["go", "goos", "goarch", "single_threaded"]},
    "artifacts":{name:sha(HERE / name) for name in ["requests.json", "observations.json", "stacks.json", "observer.go", "regenerate.py"]},
    "producer_inputs":{name:sha(ROOT / name) for name in SCRIPT_INPUTS},
    "overlay_sha256":{name:hashlib.sha256(source.encode()).hexdigest() for name, source in sorted(sources.items())},
    "raw_log_sha256":sha(WORK / "run.log"), "raw_observations_sha256":sha(WORK / "observations.ndjson"),
    "scope":"Exactly three errors-domain handoffs. Separate pre/post programs preserve pinned emit-before-semantic order. No checker queries, counter changes, threshold changes, or diagnostic changes were added by the observer. Stack arguments/addresses omitted from the retained normalized frames."})
print("Captured three exact pre/post diagnostic observations and", len(stacks), "native stacks")
