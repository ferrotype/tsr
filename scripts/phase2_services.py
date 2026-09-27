#!/usr/bin/env python3
"""Phase 2 C5.5: the recorded fourslash reference for the checker services.

A diagnostic overlay of the pin (never an edit of `upstream/`) instruments every
exported checker entry point the language service can call and runs the pinned
fourslash suite one test at a time. Each test's record holds its virtual file
system, the programs its checkers check (as loading requests with the snapshot
texts), and per checker the calls in order, with arguments and results
described by recorder-local tokens and the fields that build them. The Rust
replay rebuilds each program, replays each checker's calls in order through the
checker's entry points and compares the results per operation.

    record  [--output DIR] [--run REGEX]   # neutrality, two recorded runs, the record and manifest
    verify  [--output DIR]                 # re-record; the committed manifest must reproduce
    replay  [--output DIR] [--test NAME]   # replay the record against Rust; writes the receipt
    fixture                                # the C5.8 contract subset of the record

`record` builds the suite twice (with and without the overlay), runs it three
times (once plain, twice recorded) and requires the same fourslash outcome for
every test in all three. A test whose two recorded runs differ depends on the
language server's timing (diagnostic file order, canceled requests, mid-edit
snapshots); it is excluded and named in the manifest. `--run` is for
development and never writes the committed files.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import lzma
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04 import go_environment, verified_upstream  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

TOOLS = ROOT / "tools/phase2/services"
INSTRUMENT = TOOLS / "instrument/main.go"
RECORDER = TOOLS / "overlay/checker_phase2_services.go"
REGISTRY = TOOLS / "overlay/core_phase2_services.go"
PACKAGE = "./internal/fourslash/tests"

MANIFEST = ROOT / "data/phase2/services-replay.json"
RECORD = ROOT / "data/phase2/services-replay.json.xz"
RECEIPT = ROOT / "data/phase2/receipts/c5-services.json"
APPROVALS = ROOT / "data/phase2/services-approvals.json"
OUTPUT = ROOT / "target/phase2/services"
# Everything whose change can alter the replay's outcome: the production
# crates and the workspace (as for the contract witnesses), the replay driver
# and this script.
REPLAY_SOURCES = ["crates", "tools/**/Cargo.toml", "tools/s08/relater-prototype", "xtask", "tools/s03",
                  "scripts/generate_locale_tables.py", "Cargo.toml", "Cargo.lock", ".cargo",
                  "rust-toolchain.toml", "tools/phase2/services/replay", "scripts/phase2_services.py"]
REPLAY_BUILD = ["cargo", "build", "--release", "--locked", "-p", "tsr_compiler", "--features", "services-replay",
                "--example", "phase2_services"]
REPLAY_BINARY = ROOT / "target/release/examples/phase2_services"
# Outcomes of a replayed call. `excluded` names a program kind the replay does
# not rebuild by design; every other outcome but `match` keeps an operation open.
MATCHED = {"match", "excluded"}

FOURSLASH_BEGIN_ANCHOR = "\tfsFromMap := vfstest.FromMap(testfs, harnessOptions.UseCaseSensitiveFileNames)\n"
FOURSLASH_BEGIN = """	phase2Files := make(map[string]string, len(testfs))
	phase2Symlinks := make(map[string]string)
	for phase2Name, phase2Value := range testfs {
		switch phase2Value := phase2Value.(type) {
		case string:
			phase2Files[phase2Name] = phase2hex.EncodeToString([]byte(phase2Value))
		case []byte:
			phase2Files[phase2Name] = phase2hex.EncodeToString(phase2Value)
		case *phase2fstest.MapFile:
			if phase2Value.Mode&phase2fs.ModeSymlink != 0 {
				phase2Symlinks[phase2Name] = string(phase2Value.Data)
			} else {
				phase2Files[phase2Name] = phase2hex.EncodeToString(phase2Value.Data)
			}
		}
	}
	core.Phase2ServicesBegin(&core.Phase2ServicesTest{Name: t.Name(), Files: phase2Files, Symlinks: phase2Symlinks})
"""
FOURSLASH_END_ANCHOR = "\t\terr := closeClient()\n"
FOURSLASH_END = "\t\tcore.Phase2ServicesEnd()\n"
FOURSLASH_IMPORT_ANCHOR = "import (\n"
FOURSLASH_IMPORTS = 'import (\n\tphase2hex "encoding/hex"\n\tphase2fs "io/fs"\n\tphase2fstest "testing/fstest"\n'


def fourslash_overlay(upstream):
    source = (upstream / "tsc/internal/fourslash/fourslash.go").read_text()
    for anchor in (FOURSLASH_BEGIN_ANCHOR, FOURSLASH_END_ANCHOR, FOURSLASH_IMPORT_ANCHOR):
        if source.count(anchor) != 1:
            raise ValueError("fourslash.go anchor is not unique: " + anchor.strip())
    source = source.replace(FOURSLASH_IMPORT_ANCHOR, FOURSLASH_IMPORTS, 1)
    source = source.replace(FOURSLASH_BEGIN_ANCHOR, FOURSLASH_BEGIN_ANCHOR + FOURSLASH_BEGIN, 1)
    return source.replace(FOURSLASH_END_ANCHOR, FOURSLASH_END_ANCHOR + FOURSLASH_END, 1)


def overlay_sources(upstream, env, scratch):
    """Every overlay file by its path under tsc/internal, and the instrumented entry points."""
    generated = scratch / "instrumented"
    if generated.exists():
        shutil.rmtree(generated)
    generated.mkdir(parents=True)
    completed = subprocess.run(["go", "run", str(INSTRUMENT), "-src", str(upstream / "tsc/internal/checker"),
                                "-out", str(generated)], cwd=INSTRUMENT.parent, env=dict(env, GOFLAGS="-mod=mod"),
                               capture_output=True, check=False)
    if completed.returncode:
        raise ValueError("instrumenter failed: " + completed.stderr.decode()[-2000:])
    entry_points = json.loads(completed.stdout)
    sources = {"checker/" + path.name: path.read_text() for path in sorted(generated.iterdir())}
    sources["checker/zz_phase2_services.go"] = RECORDER.read_text()
    sources["core/zz_phase2_services.go"] = REGISTRY.read_text()
    sources["fourslash/fourslash.go"] = fourslash_overlay(upstream)
    return sources, entry_points


def go_test_binary(output, upstream, env, name, overlay):
    """`go test -c` of the fourslash tests, with the overlay or without it."""
    binary = output / name
    # The repo package locates the checkout from its own source path, which
    # -trimpath would otherwise remove (as the native oracle build does).
    repo_flag = "-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath=" + str(output / "unmatched-prefix")
    command = ["go", "test", "-c", "-o", str(binary), "-trimpath", "-mod=readonly", repo_flag]
    if overlay:
        command += ["-overlay", str(output / "overlay.json")]
    command.append(PACKAGE)
    with (output / f"{name}.build.stdout").open("wb") as out, (output / f"{name}.build.stderr").open("wb") as err:
        completed = subprocess.run(command, cwd=upstream / "tsc", env=env, stdout=out, stderr=err, check=False)
    if completed.returncode or not binary.exists():
        raise ValueError(f"{name} build failed; see " + str(output / f"{name}.build.stderr"))
    return command


def build(output, upstream, env):
    output.mkdir(parents=True, exist_ok=True)
    sources, entry_points = overlay_sources(upstream, env, output)
    replacements = {}
    for name, source in sorted(sources.items()):
        path = output / "overlay" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        replacements[str(upstream / "tsc/internal" / name)] = str(path)
    (output / "overlay.json").write_bytes(canonical({"Replace": replacements}))
    command = go_test_binary(output, upstream, env, "fourslash.test", overlay=True)
    go_test_binary(output, upstream, env, "fourslash-plain.test", overlay=False)
    fingerprint = {name: digest(source.encode()) for name, source in sorted(sources.items())}
    return {"command": [part.replace(str(output), "<output>") for part in command],
            "overlay_sha256": fingerprint, "entry_points": len(entry_points)}


def run_suite(binary, directory, upstream, env, run=None, record=None, timeout_minutes=240):
    """Run the fourslash suite one test at a time; return the per-test outcomes."""
    command = [str(binary), "-test.v", "-test.count=1", "-test.parallel=1", f"-test.timeout={timeout_minutes}m"]
    if run:
        command.append("-test.run=" + run)
    run_env = dict(env)
    if record:
        run_env["PHASE2_SERVICES_RECORD"] = str(record)
    stem = Path(record).stem if record else "plain"
    with (directory / f"{stem}.stdout").open("wb") as out, (directory / f"{stem}.stderr").open("wb") as err:
        subprocess.run(command, cwd=upstream / "tsc/internal/fourslash/tests", env=run_env, stdout=out, stderr=err,
                       check=False)
    return outcomes((directory / f"{stem}.stdout").read_text(errors="replace"))


OUTCOME = re.compile(r"^\s*--- (PASS|FAIL|SKIP): (\S+)")


def outcomes(text):
    result = {}
    for line in text.splitlines():
        match = OUTCOME.match(line)
        if match:
            result[match.group(2)] = match.group(1)
    return dict(sorted(result.items()))


def recorded_tests(raw_path):
    """Each recorded test segment's events in order, without the recorder's
    sequence numbers.

    Yields (name, [event line bytes]); a test with no checker call records
    nothing, and a test that starts several servers records one segment each,
    in order, under its name."""
    current = None
    with open(raw_path, "rb") as lines:
        for line in lines:
            event = json.loads(line)
            event.pop("n", None)
            kind = event["e"]
            if kind == "test":
                if current is not None:
                    yield current
                current = (event["name"], [canonical(event)])
            elif kind == "end":
                if current is not None:
                    current[1].append(canonical(event))
                    yield current
                current = None
            elif current is None:
                raise ValueError("event outside a test: " + line.decode()[:200])
            else:
                current[1].append(canonical(event))
    if current is not None:
        yield current


def test_digest(events):
    return digest(b"\n".join(events))


def test_digests(raw_path):
    """Each test's digest over all its segments in order."""
    hashes = {}
    for name, events in recorded_tests(raw_path):
        hashes.setdefault(name, hashlib.sha256()).update(b"\n".join(events) + b"\n")
    return {name: value.hexdigest() for name, value in hashes.items()}


def operation_counts(events):
    counts = Counter()
    for line in events:
        if line.startswith(b'{"args"'):
            counts[json.loads(line)["op"]] += 1
    return counts


RUNS = ("first", "second", "third")


def record_runs(output, upstream, env, run=None, runs=RUNS):
    """Build, run the suite plain and once recorded per name in `runs`; return
    the build, the outcomes (plain first) and each recorded run's test digests."""
    oracle = build(output, upstream, env)
    plain = run_suite(output / "fourslash-plain.test", output, upstream, env, run=run)
    recorded = [run_suite(output / "fourslash.test", output, upstream, env, run=run, record=output / f"{which}.ndjson")
                for which in runs]
    digests = [test_digests(output / f"{which}.ndjson") for which in runs]
    return oracle, (plain, *recorded), digests


def write_record(output, raw, keep):
    """The committed record: the kept tests' events in test-name order (the
    suite's parallel tests run in no fixed order), xz-compressed, and its digests."""
    stream = output / "record.ndjson"
    unordered = output / "record.unordered.ndjson"
    spans = defaultdict(list)
    operations = Counter()
    with unordered.open("wb") as out:
        for name, events in recorded_tests(raw):
            if name in keep:
                operations.update(operation_counts(events))
                start = out.tell()
                out.write(b"\n".join(events) + b"\n")
                spans[name].append((start, out.tell() - start))
    with unordered.open("rb") as source, stream.open("wb") as out:
        for name in sorted(spans):
            for start, length in spans[name]:
                source.seek(start)
                out.write(source.read(length))
    unordered.unlink()
    record_sha256 = digest(stream.read_bytes())
    compressed = output / "record.ndjson.xz"
    with stream.open("rb") as source, compressed.open("wb") as target:
        subprocess.run(["xz", "-T0", "-6", "-c"], stdin=source, stdout=target, check=True)
    return compressed, record_sha256, operations


def stable_tests(digests):
    """The tests every recorded run recorded identically."""
    return {name for name, value in digests[0].items() if all(run.get(name) == value for run in digests[1:])}


def manifest_of(oracle, runs, digests, pin, go_version, operations, record_sha256, record_xz_sha256):
    plain, first = runs[0], runs[1]
    first_digests = digests[0]
    stable = stable_tests(digests)
    excluded = sorted(name for name in first_digests if name not in stable)
    kept = {name: value for name, value in first_digests.items() if name in stable}
    return {
        "version": 1,
        "pin": pin,
        "go": go_version,
        "command": oracle["command"],
        "overlay_sha256": oracle["overlay_sha256"],
        "entry_points": oracle["entry_points"],
        "neutral": all(outcome == plain for outcome in runs[1:]),
        "recorded_runs": len(digests),
        "outcomes": dict(sorted(Counter(first.values()).items())),
        "skipped": sorted(name for name, outcome in first.items() if outcome == "SKIP"),
        "failed": sorted(name for name, outcome in first.items() if outcome == "FAIL"),
        "tests_recorded": len(first_digests),
        "excluded_nondeterministic": excluded,
        "tests_kept": len(kept),
        "calls": sum(operations.values()),
        "operations": dict(sorted(operations.items())),
        "test_digests": dict(sorted(kept.items())),
        "record_sha256": record_sha256,
        "record_xz_sha256": record_xz_sha256,
    }


def go_version(env):
    return subprocess.run(["go", "version"], env=env, capture_output=True, check=True).stdout.decode().split()[2]


def pin():
    return json.loads((ROOT / "data/upstream.json").read_text())["pin"]


def reused_runs(output):
    """The build and runs of an earlier `record` in `output` (development)."""
    previous = json.loads((output / "manifest.json").read_bytes())
    oracle = {key: previous[key] for key in ("command", "overlay_sha256", "entry_points")}
    runs = [outcomes((output / f"{which}.stdout").read_text(errors="replace")) for which in ("plain", *RUNS)]
    digests = [test_digests(output / f"{which}.ndjson") for which in RUNS]
    return oracle, tuple(runs), digests


def record(output, run=None, reuse=False):
    upstream = verified_upstream()
    env = go_environment()
    output.mkdir(parents=True, exist_ok=True)
    if reuse:
        oracle, runs, digests = reused_runs(output)
    else:
        oracle, runs, digests = record_runs(output, upstream, env, run=run)
    plain = runs[0]
    if not all(outcome == plain for outcome in runs[1:]):
        changed = sorted(name for name in set().union(*runs) if len({outcome.get(name) for outcome in runs}) != 1)
        raise ValueError("the recorder changed fourslash outcomes: " + ", ".join(changed[:20]))
    keep = stable_tests(digests)
    compressed, record_sha256, operations = write_record(output, output / "first.ndjson", keep)
    manifest = manifest_of(oracle, runs, digests, pin(), go_version(env), operations, record_sha256,
                           digest(compressed.read_bytes()))
    (output / "manifest.json").write_bytes(json.dumps(manifest, indent=1, sort_keys=True).encode() + b"\n")
    if run is None:
        shutil.copyfile(compressed, RECORD)
        MANIFEST.write_bytes((output / "manifest.json").read_bytes())
    print(json.dumps({key: manifest[key] for key in ("neutral", "outcomes", "tests_recorded", "tests_kept", "calls")}
                     | {"excluded_nondeterministic": len(manifest["excluded_nondeterministic"])}))
    return manifest


def test_pattern(name):
    """The -test.run pattern that selects exactly `name` (subtests included)."""
    return "/".join("^" + re.escape(part) + "$" for part in name.split("/"))


def retried(output, upstream, env, name, value, attempts=5):
    """Whether recording `name` alone reproduces `value` within `attempts` runs:
    a timing-dependent test can land on the same other result twice."""
    for attempt in range(attempts):
        raw = output / f"retry-{attempt}.ndjson"
        run_suite(output / "fourslash.test", output, upstream, env, run=test_pattern(name), record=raw)
        if test_digests(raw).get(name) == value:
            return True
    return False


def verify(output):
    """Re-record twice: every committed test reproduces its digest in one of the
    runs, or records differently in the two, or reproduces it when recorded
    alone within five attempts (the language server's timing, as the record's
    own exclusions show), at the same pin, overlay and outcomes. Any other test
    changed."""
    committed = json.loads(MANIFEST.read_bytes())
    upstream = verified_upstream()
    env = go_environment()
    output.mkdir(parents=True, exist_ok=True)
    oracle, runs, digests = record_runs(output, upstream, env, runs=RUNS[:2])
    plain, first = runs[0], runs[1]
    problems = []
    if committed["pin"] != pin():
        problems.append("pin changed")
    if committed["overlay_sha256"] != oracle["overlay_sha256"]:
        problems.append("overlay fingerprint changed")
    if not all(outcome == plain for outcome in runs[1:]):
        problems.append("the recorder changed fourslash outcomes")
    if committed["outcomes"] != dict(sorted(Counter(first.values()).items())):
        problems.append("fourslash outcomes changed")
    if committed["skipped"] != sorted(name for name, outcome in first.items() if outcome == "SKIP"):
        problems.append("the skipped tests changed")
    reproduced, unstable, changed = [], [], []
    for name, value in committed["test_digests"].items():
        new = [run.get(name) for run in digests]
        if value in new:
            reproduced.append(name)
        elif len(set(new)) > 1 or retried(output, upstream, env, name, value):
            unstable.append(name)
        else:
            changed.append(name)
    if changed:
        problems.append(f"{len(changed)} recorded tests changed: " + ", ".join(changed[:10]))
    if digest(RECORD.read_bytes()) != committed["record_xz_sha256"]:
        problems.append("the committed record is not the manifest's")
    report = {"verified": not problems, "problems": problems, "tests_reproduced": len(reproduced),
              "tests_unstable": sorted(unstable), "tests_changed": sorted(changed)}
    (output / "verify.json").write_bytes(json.dumps(report, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps(report, indent=1))
    return not problems


def approvals():
    """The owner's approvals of what the replay leaves to Phase 5: whole
    operations, program kinds it excludes, and reasons it cannot make a call.
    Only an entry with `approved_by` counts; the file also carries proposals."""
    if not APPROVALS.is_file():
        return {"operations": {}, "exclusions": {}, "unsupported": {}}, None
    document = json.loads(APPROVALS.read_bytes())
    approved = {section: {key: entry for key, entry in document.get(section, {}).items() if entry.get("approved_by")}
                for section in ("operations", "exclusions", "unsupported")}
    return approved, digest(APPROVALS.read_bytes())


def aggregate(shards):
    operations = defaultdict(Counter)
    unsupported = defaultdict(Counter)
    exclusions = defaultdict(Counter)
    programs = Counter()
    examples = defaultdict(list)
    for path in shards:
        for line in path.open():
            test = json.loads(line)
            for status in test["programs"].values():
                if "excluded" in status:
                    programs["excluded"] += 1
                    exclusions[status["excluded"]]["programs"] += 1
                elif "error" in status:
                    programs["error"] += 1
                elif status["same_files"] and status["same_order"]:
                    programs["same_files"] += 1
                else:
                    programs["different_files"] += 1
            for op, counts in test["operations"].items():
                operations[op].update(counts)
            for reason, calls in test["exclusions"].items():
                exclusions[reason]["calls"] += calls
            for op, reasons in test["unsupported"].items():
                unsupported[op].update(reasons)
            for op, details in test["details"].items():
                for detail in details:
                    if len(examples[op]) < 5:
                        examples[op].append(dict(detail, test=test["test"]))
    return operations, unsupported, exclusions, programs, examples


def classify(recorded, operations, unsupported, exclusions, approved, *, whole=True):
    """Each operation's state, each exclusion's approval, and completeness.

    An operation is `replayed` when every call matched or was excluded,
    `approved` when the owner approved it or every other call is unsupported
    for an approved reason, `incomplete` when a whole replay saw fewer calls
    than the record holds, and `open` otherwise. The replay is complete only
    over the whole record, with every operation replayed or approved and every
    exclusion approved."""
    states = {}
    for op in sorted(set(recorded) | set(operations)):
        counts = operations.get(op, Counter())
        other = sum(count for outcome, count in counts.items() if outcome not in MATCHED)
        covered = sum(count for reason, count in unsupported.get(op, {}).items() if reason in approved["unsupported"])
        if whole and sum(counts.values()) != recorded.get(op, 0):
            state = "incomplete"
        elif other == 0:
            state = "replayed"
        elif op in approved["operations"] or other == covered:
            state = "approved"
        else:
            state = "open"
        states[op] = {"state": state, "counts": dict(sorted(counts.items()))}
        if unsupported.get(op):
            states[op]["unsupported"] = dict(sorted(unsupported[op].items()))
    excluded = {reason: {**dict(counts), "approved": reason in approved["exclusions"]}
                for reason, counts in sorted(exclusions.items())}
    complete = (whole and all(entry["state"] in ("replayed", "approved") for entry in states.values())
                and all(entry["approved"] for entry in excluded.values()))
    return states, excluded, complete


def replay(output, tests=None, jobs=None):
    manifest_bytes = MANIFEST.read_bytes()
    manifest = json.loads(manifest_bytes)
    if digest(RECORD.read_bytes()) != manifest["record_xz_sha256"]:
        raise ValueError("the committed record is not the manifest's")
    import phase2_producers
    inputs = phase2_producers.source_inputs(REPLAY_SOURCES)
    subprocess.run(REPLAY_BUILD, cwd=ROOT, check=True)
    output.mkdir(parents=True, exist_ok=True)
    stream = output / "record.ndjson"
    with lzma.open(RECORD) as source, stream.open("wb") as target:
        shutil.copyfileobj(source, target, 1 << 20)
    if digest(stream.read_bytes()) != manifest["record_sha256"]:
        raise ValueError("the decompressed record is not the manifest's")
    jobs = jobs or os.cpu_count() or 1
    shards = [output / f"shard-{index}.ndjson" for index in range(jobs)]
    selection = [argument for name in tests or [] for argument in ("--test", name)]
    processes = [subprocess.Popen([str(REPLAY_BINARY), "--input", str(stream), "--output", str(shard),
                                   "--shard", str(index), "--shards", str(jobs), *selection],
                                  stderr=(output / f"shard-{index}.stderr").open("wb"))
                 for index, shard in enumerate(shards)]
    if any(process.wait() for process in processes):
        raise ValueError("a replay shard failed; see " + str(output))
    operations, unsupported, exclusions, programs, examples = aggregate(shards)
    approved, approvals_sha256 = approvals()
    states, excluded, complete = classify(manifest["operations"], operations, unsupported, exclusions, approved,
                                          whole=tests is None)
    totals = Counter()
    for counts in operations.values():
        totals.update(counts)
    receipt = {
        "version": 1, "state": "observed", "complete": complete,
        "manifest_sha256": digest(manifest_bytes), "record_sha256": manifest["record_sha256"],
        "approvals_sha256": approvals_sha256, "source_inputs": inputs,
        "calls": dict(sorted(totals.items())), "programs": dict(sorted(programs.items())),
        "operations": states, "exclusions": excluded,
        "replayed": sum(entry["state"] == "replayed" for entry in states.values()),
        "approved": sum(entry["state"] == "approved" for entry in states.values()),
        "open": sorted(op for op, entry in states.items() if entry["state"] not in ("replayed", "approved")),
    }
    if inputs != phase2_producers.source_inputs(REPLAY_SOURCES):
        raise ValueError("sources changed while the replay ran")
    (output / "examples.json").write_bytes(json.dumps(examples, indent=1, sort_keys=True).encode() + b"\n")
    (output / "report.json").write_bytes(json.dumps(receipt, indent=1, sort_keys=True).encode() + b"\n")
    if tests is None:
        RECEIPT.parent.mkdir(parents=True, exist_ok=True)
        RECEIPT.write_bytes(json.dumps(receipt, indent=1, sort_keys=True).encode() + b"\n")
    print(json.dumps({key: receipt[key] for key in ("complete", "calls", "programs", "replayed", "approved", "open")},
                     indent=1))
    return receipt


# The recorded tests the C5.8 contracts replay (crates/tsr_compiler/tests/
# c5_contracts.rs): hover expansion of each declaration kind, accessibility
# chains, declaration serialization and its resolver marks, signature help,
# string-literal completions with blocked inference, deprecation and
# implicit-any suggestions, and JSDoc parameter references.
CONTRACT_TESTS = [
    "TestQuickinfoVerbosityClassWithMixinBase", "TestQuickinfoVerbosityInterfaceMemberOrdering",
    "TestQuickinfoVerbosityConstEnum", "TestQuickinfoVerbosityNestedNamespace",
    "TestQuickinfoVerbosityNamespaceTypeAliases", "TestQuickinfoVerbosityConditionalType",
    "TestCompletionForComputedStringProperties", "TestQualifyModuleTypeNames",
    "TestCodeFixMissingTypeAnnotationOnExports11", "TestCodeFixMissingTypeAnnotationOnExports44_default_export",
    "TestCodeFixMissingTypeAnnotationOnExports55_generator_return", "TestCodeFixClassImplementInterfaceWithAmbientSignatures2",
    "TestQuickinfoVerbosityToplevelTruncation1", "TestReferences01", "TestSignatureHelp01",
    "TestCompletionForStringLiteral12", "TestCompletionsLiteralOverload", "TestJsdocDeprecated_suggestion1",
    "TestGetJavaScriptSyntacticDiagnostics16", "TestRenameJsDocTypeLiteral",
]
CONTRACT_FIXTURE = ROOT / "crates/tsr_compiler/tests/fixtures/c5/services-subset.ndjson"


def fixture():
    """The contract subset of the committed record, test by test in record order."""
    wanted = set(CONTRACT_TESTS)
    found = []
    with lzma.open(RECORD) as source, CONTRACT_FIXTURE.open("wb") as target:
        keep = False
        for line in source:
            if line.startswith(b'{"e":"test"'):
                name = json.loads(line)["name"]
                keep = name in wanted
                if keep:
                    found.append(name)
            if keep:
                target.write(line)
    missing = sorted(wanted - set(found))
    if missing:
        raise ValueError("contract tests not in the record: " + ", ".join(missing))
    print(json.dumps({"tests": len(found), "bytes": CONTRACT_FIXTURE.stat().st_size}))


def current(manifest, comparison=None, *, context=None):
    """C5.5 is current when the committed record is the manifest's and the replay
    receipt names this manifest, these approvals and the current sources, with
    every operation replayed or approved."""
    del comparison, context
    manifest = Path(manifest)
    if not manifest.is_file() or not RECORD.is_file() or not RECEIPT.is_file():
        return False
    document = json.loads(manifest.read_bytes())
    if digest(RECORD.read_bytes()) != document.get("record_xz_sha256") or document.get("pin") != pin():
        return False
    receipt = json.loads(RECEIPT.read_bytes())
    if receipt.get("version") != 1 or receipt.get("state") != "observed":
        return False
    if receipt.get("manifest_sha256") != digest(manifest.read_bytes()):
        return False
    if receipt.get("approvals_sha256") != approvals()[1]:
        return False
    import phase2_producers
    if receipt.get("source_inputs") != phase2_producers.source_inputs(REPLAY_SOURCES):
        return False
    return receipt.get("complete") is True


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("fixture", help="write the C5.8 contract subset of the record")
    for name in ("record", "verify", "replay"):
        sub = commands.add_parser(name)
        sub.add_argument("--output", type=Path, default=OUTPUT / name)
        if name == "record":
            sub.add_argument("--run", help="a -test.run pattern (development only; never committed)")
            sub.add_argument("--reuse", action="store_true",
                             help="re-derive the record from this output's earlier runs (no Go runs)")
        if name == "replay":
            sub.add_argument("--test", action="append", help="replay only this test (development only)")
            sub.add_argument("--jobs", type=int)
    args = parser.parse_args()
    if hasattr(args, "output"):
        args.output = args.output.resolve()
    if args.command == "fixture":
        fixture()
    elif args.command == "record":
        record(args.output, run=args.run, reuse=args.reuse)
    elif args.command == "verify":
        sys.exit(0 if verify(args.output) else 1)
    elif args.command == "replay":
        receipt = replay(args.output, tests=args.test, jobs=args.jobs)
        sys.exit(0 if receipt["complete"] else 1)


if __name__ == "__main__":
    main()
