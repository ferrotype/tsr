"""The Phase 1 config/options baseline index: 309 pinned reference outputs.

Enumerates the four reference groups recursively, records each output's exact
path and hash, and traces the native invocation and rendering authority that
produced it. Authority is read from the pinned Go test sources, never inferred
from a baseline's own contents and never reconstructed from Rust.

`config/matchFiles` has no Go invocation at this pin. That is recorded as a
named blocker rather than repaired by inventing a renderer; see
docs/PHASE1-progress.md.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = "tsc/testdata/baselines/reference"

# Group -> (reference subdirectory, expected output count). The counts are the
# plan's frozen denominator; a mismatch is a hard failure, not a silent resize.
GROUPS: dict[str, tuple[str, int]] = {
    "matchFiles": ("config/matchFiles", 142),
    "tsconfigParsing": ("config/tsconfigParsing", 87),
    "parseCommandLine": ("tsoptions/commandLineParsing/parseCommandLine", 53),
    "parseBuildOptions": ("tsoptions/commandLineParsing/parseBuildOptions", 27),
}
TOTAL = 309

# The only two subfolders any pinned Go test writes. Verified by grepping
# `baseline.Options{Subfolder: ...}` across tsc/ at the pin.
WRITTEN_SUBFOLDERS = ("config/tsconfigParsing", "tsoptions/commandLineParsing")

COMMAND_LINE_TEST = "tsc/internal/tsoptions/commandlineparser_test.go"
CONFIG_TEST = "tsc/internal/tsoptions/tsconfigparsing_test.go"
VFSMATCH_TEST = "tsc/internal/vfs/vfsmatch/vfsmatch_test.go"


def upstream() -> Path:
    return ROOT / "upstream"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def outputs(group: str) -> list[Path]:
    """Every reference output in a group, recursively.

    `config/matchFiles` contains two outputs under a nested directory, because
    the originating test title contains a slash ("Expands z to z/star..."). A
    maxdepth-1 glob silently returns 140 and quietly shrinks the denominator.
    """
    subdirectory, _ = GROUPS[group]
    root = upstream() / REFERENCE / subdirectory
    # Finder metadata written into the checkout is not a reference output.
    return sorted(p for p in root.rglob("*") if p.is_file() and p.name != ".DS_Store")


def subfolder_writers(text: str) -> set[str]:
    return set(re.findall(r'baseline\.Options\{Subfolder:\s*"([^"]+)"', text))


def authority(group: str) -> dict:
    """The native invocation and renderer that produce a group's outputs."""
    if group in ("parseCommandLine", "parseBuildOptions"):
        renderer = (
            "formatNewBaseline" if group == "parseCommandLine" else "formatNewBaselineBuild"
        )
        return {
            "status": "established",
            "native_test_source": COMMAND_LINE_TEST,
            "renderer": f"{COMMAND_LINE_TEST}::{renderer}",
            "invocation": f"{COMMAND_LINE_TEST}::createSubScenario({group!r}, name, args)",
            "subfolder": "tsoptions/commandLineParsing",
            # createSubScenario sets testName = scenarioKind + "/" + name and
            # assert*ParseResult writes baseline.Run(t, testName+".js", ...).
            "output_relationship": "one invocation renders exactly one output",
            "output_name_rule": "<group>/<sub-scenario name>.js",
        }
    if group == "tsconfigParsing":
        return {
            "status": "established",
            "native_test_source": CONFIG_TEST,
            "renderer": f"{CONFIG_TEST}::baselineParseConfigWith",
            "secondary_renderer": f"{CONFIG_TEST}::TestParseConfigFileTextToJson (inline assembly)",
            "invocation": (
                f"{CONFIG_TEST}::TestParseJsonConfigFileContent and "
                "TestParseJsonSourceFileConfigFileContent, plus their variant callers"
            ),
            "subfolder": "config/tsconfigParsing",
            "output_relationship": (
                "one invocation renders one output, but a single output may concatenate "
                "several parsed configs: baselineParseConfigWith loops over input []testConfig"
            ),
            "output_name_rule": '<title> with {json,jsonSourceFile} api.js, or "<title> jsonParse.js"',
        }
    # matchFiles
    return {
        # Still blocked, deliberately. The owner settled the authority question
        # on 2026-09-20 by approving a carried test renderer, but approval is
        # not an observation: the group stays blocked until native results
        # rendered through that renderer are shown to reproduce all 142 frozen
        # files. F2a owns that work and F3a reuses the seam.
        "status": "blocked",
        "blocker": "phase1-matchfiles-renderer-not-yet-verified",
        "owner_decision": (
            "2026-09-20: keep all 309 byte-for-byte baselines and carry a test-only "
            "implementation of the matchFiles envelope, with pinned Go matching and "
            "configuration behavior retained as the semantic authority. Implementation and "
            "native-byte verification are owned by F2a; expected result sections are never "
            "copied from the baseline."
        ),
        "native_semantic_authority": (
            f"{VFSMATCH_TEST}::TestReadDirectory and TestReadDirectoryMatchesTypeScriptBaselines "
            "assert ordered matchFiles() results"
        ),
        "renderer": None,
        "invocation": None,
        "subfolder": "config/matchFiles",
        "reason": (
            "No pinned Go test writes baseline.Options{Subfolder: \"config/matchFiles\"}, and the "
            "group's envelope (a 'config:' heading followed by the whole ParsedCommandLine as "
            "JSON, with no 'FileNames::' section) matches no renderer in the pin. "
            "baselineParseConfigWith emits configFileName::/CompilerOptions::/FileNames::/Errors:: "
            "instead. The vfsmatch tests reference "
            "tsc/testdata/fixtures/testRunner/unittests/config/matchFiles.ts, which is absent at "
            "this pin. These are promoted TypeScript outputs whose renderer was not ported."
        ),
        "output_relationship": "unknown until authority is decided",
        "output_name_rule": "<title> with {json,jsonSourceFile} api.js",
    }


def section_headings(raw: bytes) -> list[str]:
    text = raw.decode("utf-8", errors="replace")
    return sorted(
        set(
            re.findall(
                r"^(config:|Fs::|configFileName::|CompilerOptions::|TypeAcquisition::|"
                r"FileNames::|Errors::)",
                text,
                re.M,
            )
        )
    )


def build() -> dict:
    groups = {}
    for group, (subdirectory, expected) in GROUPS.items():
        rows = []
        for path in outputs(group):
            raw = path.read_bytes()
            rows.append(
                {
                    "output": str(path.relative_to(upstream())),
                    "name": str(path.relative_to(upstream() / REFERENCE / subdirectory)),
                    "bytes": len(raw),
                    "sha256": digest(raw),
                    "sections": section_headings(raw),
                }
            )
        if len(rows) != expected:
            raise ValueError(
                f"{group}: expected {expected} outputs at the pin, enumerated {len(rows)}"
            )
        groups[group] = {
            "subdirectory": f"{REFERENCE}/{subdirectory}",
            "expected_outputs": expected,
            "authority": authority(group),
            "outputs": rows,
        }
    total = sum(len(g["outputs"]) for g in groups.values())
    if total != TOTAL:
        raise ValueError(f"expected {TOTAL} reference outputs, enumerated {total}")
    return {
        "version": 1,
        "pin": pin(),
        "total_outputs": TOTAL,
        "written_subfolders": list(WRITTEN_SUBFOLDERS),
        "groups": groups,
    }


def pin() -> str:
    import json

    return json.loads((ROOT / "data/upstream.json").read_text())["pin"]


EXCEPTIONS = ROOT / "data/phase1/baseline-exceptions.json"

# The owner's amendment to the requirement that all 309 reference outputs be
# reproduced exactly.
#
# A carried renderer can fail to reproduce a frozen baseline for three quite
# different reasons, and collapsing them loses the only one worth acting on:
#
#   upstream            the frozen bytes and the pinned Go genuinely disagree,
#                       because the baseline predates a change the port made.
#                       Nothing we write can reconcile them without
#                       reimplementing the TypeScript behavior the bytes came
#                       from, which would fabricate agreement with something
#                       that is not the pin.
#   renderer_defect     our renderer is wrong and could produce the bytes.
#   missing_observation the pin can produce it and the renderer does not ask.
#
# Only `upstream` may be excepted, only individually, and only with the pinned
# line that establishes it. An exception is NOT a pass: the output keeps its
# original file and hash, records what the pin actually produces, and is
# counted in its own bucket. And it keeps its Go-versus-Rust semantic
# comparison, because what the exception waives is the rendered byte
# comparison, never the question of whether the port behaves like the pin.
EXCEPTION_KINDS = ("upstream",)


def exceptions() -> dict:
    if not EXCEPTIONS.is_file():
        return {"version": 1, "exceptions": []}
    return json.loads(EXCEPTIONS.read_text())


def exceptions_by_output() -> dict[str, dict]:
    return {entry["output"]: entry for entry in exceptions().get("exceptions", [])}


def exception_problems(index: dict) -> list[str]:
    """Validate the exception ledger against the index it amends."""
    document = exceptions()
    problems: list[str] = []
    if not EXCEPTIONS.is_file():
        return problems
    if document.get("pin") != pin():
        problems.append(
            f"baseline-exceptions.json records pin {document.get('pin')!r}, not {pin()!r}"
        )
    if not document.get("approved_by"):
        problems.append("baseline-exceptions.json records no owner approval")
    if not document.get("amends"):
        problems.append(
            "baseline-exceptions.json does not say which requirement it amends; an exception "
            "that does not name what it changes is a quiet reinterpretation"
        )
    known = {
        row["name"]: (group, row)
        for group, committed in index.get("groups", {}).items()
        for row in committed["outputs"]
    }
    seen: set[str] = set()
    for entry in document.get("exceptions", []):
        name = entry.get("output", "<unnamed>")
        if name in seen:
            problems.append(f"baseline-exceptions: duplicate entry for {name}")
        seen.add(name)
        if name not in known:
            problems.append(f"baseline-exceptions: {name} is not an output in the committed index")
            continue
        group, row = known[name]
        if entry.get("kind") not in EXCEPTION_KINDS:
            problems.append(
                f"baseline-exceptions: {name} has kind {entry.get('kind')!r}; only "
                f"{', '.join(EXCEPTION_KINDS)} may be excepted, because the other attributions "
                "name work rather than a fact about the baseline"
            )
        for field in ("reason", "evidence", "pin_produces", "baseline_records", "semantic_comparison"):
            if not entry.get(field):
                problems.append(f"baseline-exceptions: {name} records no {field}")
        if entry.get("original_sha256") != row["sha256"]:
            problems.append(
                f"baseline-exceptions: {name} records a hash that is not the pin's, so the "
                "original file it claims to retain is not the one indexed"
            )
        # An excepted output is not a passing one. Claiming both would put it in
        # two buckets and inflate the verified count.
        if row.get("rendering_verified"):
            problems.append(
                f"baseline-exceptions: {name} is excepted but the index also records its "
                "rendering as verified; an exception is not a pass"
            )
    return problems


def verify(index: dict) -> list[str]:
    """Re-enumerate the pin and report every disagreement with a committed index."""
    problems: list[str] = []
    mapping = index.get("invocation_mapping")
    if mapping is None:
        problems.append(
            "the index has no invocation mapping; run `phase1.py map-baselines` so each "
            "output names the test that renders it"
        )
    if index.get("pin") != pin():
        problems.append(f"index pin {index.get('pin')!r} is not the repository pin {pin()!r}")
    for group, (subdirectory, expected) in GROUPS.items():
        committed = index.get("groups", {}).get(group)
        if committed is None:
            problems.append(f"{group}: missing from the committed index")
            continue
        actual = {}
        for path in outputs(group):
            name = str(path.relative_to(upstream() / REFERENCE / subdirectory))
            actual[name] = digest(path.read_bytes())
        recorded = {row["name"]: row["sha256"] for row in committed["outputs"]}
        if len(recorded) != len(committed["outputs"]):
            problems.append(f"{group}: duplicate output names in the committed index")
        if len(actual) != expected:
            problems.append(f"{group}: pin has {len(actual)} outputs, expected {expected}")
        for name in sorted(set(actual) - set(recorded)):
            problems.append(f"{group}: {name} exists at the pin but is not indexed")
        for name in sorted(set(recorded) - set(actual)):
            problems.append(f"{group}: {name} is indexed but absent at the pin")
        for name in sorted(set(actual) & set(recorded)):
            if actual[name] != recorded[name]:
                problems.append(f"{group}: {name} hash differs from the pin")

        # Every output must carry a per-output mapping decision, and a group may
        # only claim a completed mapping when each of its outputs was rendered
        # by a pinned test and reproduced the committed bytes.
        verified = 0
        for row in committed["outputs"]:
            if "rendering_verified" not in row or "invocation" not in row:
                problems.append(f"{group}: {row['name']} has no per-output invocation record")
                continue
            if row["rendering_verified"]:
                if not row.get("invocation"):
                    problems.append(
                        f"{group}: {row['name']} claims verified rendering with no invocation"
                    )
                verified += 1
        authority = committed.get("authority", {})
        if authority.get("verified_outputs") != verified:
            problems.append(
                f"{group}: authority claims {authority.get('verified_outputs')} verified outputs, "
                f"rows record {verified}"
            )
        if authority.get("mapping_complete") != (verified == len(committed["outputs"])):
            problems.append(f"{group}: mapping_complete disagrees with its own rows")
        if authority.get("status") == "established" and not authority.get("mapping_complete"):
            problems.append(
                f"{group}: authority is established but its per-output mapping is incomplete"
            )
    return problems


def verify_written_subfolders() -> list[str]:
    """The authority claim itself is re-derived, not trusted from the index."""
    written: set[str] = set()
    for path in (upstream() / "tsc").rglob("*_test.go"):
        written |= subfolder_writers(path.read_text(errors="replace"))
    unexpected = sorted(
        s
        for s in written
        if s.startswith(("config/", "tsoptions/")) and s not in WRITTEN_SUBFOLDERS
    )
    problems = [f"unexpected baseline subfolder written by a pinned test: {s}" for s in unexpected]
    for expected in WRITTEN_SUBFOLDERS:
        if expected not in written:
            problems.append(f"no pinned test writes the expected subfolder {expected}")
    return problems


# Which step prepares which baseline groups, and where that step's frozen
# native observations live. A group belongs to exactly one step, so the 309
# outputs are partitioned rather than double counted: F2a carries the 142
# `config/matchFiles` outputs, F3a the other 167.
STEP_OUTPUT_GROUPS: dict[str, tuple[str, str, tuple[tuple[str, str], ...]]] = {
    # step: (case family, frozen native family directory, ((group, probe), ...))
    "filesystem": ("filesystem", "filesystem", (("matchFiles", "matchfiles"),)),
    "config": ("config", "config", (
        ("parseCommandLine", "commandline"),
        ("parseBuildOptions", "commandline"),
        ("tsconfigParsing", "tsconfigparsing"),
    )),
}


def output_preparation(cases: dict, step: str) -> dict:
    """A step prepares outputs as well as operations, including excepted ones.

    Frozen native observations authenticate the rendered bytes. A baseline
    exception waives equality with the historical file, not the native
    observation or the runnable Rust comparison. Do not let `operations: []`
    make an output case disappear from the preparation gate: that is exactly
    what it would do, since an output case credits no operation by design.

    Written once over STEP_OUTPUT_GROUPS rather than per step, so F3a's 80
    command-line outputs are held to the bar F2a's 142 were, including the rule
    that an exception is refused on an output the index also marks rendered.
    """
    family, native_family, groups = STEP_OUTPUT_GROUPS[step]
    index = json.loads((ROOT / "data/phase1/config-baselines.json").read_text())
    expected: dict[str, dict] = {}
    for group, _probe in groups:
        subdirectory = GROUPS[group][0]
        for row in index["groups"][group]["outputs"]:
            expected[f"{subdirectory}/{row['name']}"] = row
    problems = exception_problems(index)
    directory = ROOT / "data/phase1/native" / native_family
    provenance_path = directory / "capture-provenance.json"
    rows: dict[str, dict] = {}
    recorded = None
    if not provenance_path.is_file():
        problems.append(f"{step} has no frozen native capture provenance")
    else:
        recorded = json.loads(provenance_path.read_text())
        if recorded.get("pin") != pin():
            problems.append(f"{step} native observations disagree with the pin")
            recorded = None
    # One probe may serve several groups -- `commandline` renders both the
    # parseCommandLine and the parseBuildOptions outputs -- so read each
    # distinct probe once. Reading it per group would present every row twice
    # and make the two-observers conflict below fire on its own duplicate.
    for probe in sorted({p for _group, p in groups}):
        if recorded is None:
            break
        native = directory / probe / "observations.json"
        declared = recorded.get("native_probes", {}).get(probe, {})
        if not native.is_file():
            problems.append(f"probe {probe} has no frozen native observations")
            continue
        if digest(native.read_bytes()) != declared.get("observations_sha256"):
            problems.append(f"probe {probe} native observations disagree with their digest")
            continue
        seen: set[str] = set()
        for row in json.loads(native.read_text())["observations"]:
            case_id = row["case"]
            if case_id in seen:
                problems.append(f"probe {probe} duplicates native row {case_id}")
                continue
            seen.add(case_id)
            if row.get("result") not in ("observed", "native_unavailable"):
                problems.append(
                    f"probe {probe}, case {case_id}: {row.get('result')!r}: {row.get('error', '')}"
                )
                continue
            # Every probe sees the whole family schedule and declines the cases
            # it does not serve, so a case has one observing row and N-1
            # declines. Keeping the first row seen would let an alphabetically
            # earlier probe's decline shadow the real observation -- which is
            # how this was first written, and the gate caught it. An observed
            # row wins; two observing probes for one output is a conflict.
            existing = rows.get(row["case"])
            if row.get("result") != "observed":
                rows.setdefault(row["case"], row)
                continue
            if existing is not None and existing.get("result") == "observed":
                problems.append(f"{row['case']}: two native probes both observed this output")
                continue
            rows[row["case"]] = row
    by_output: dict[str, list[dict]] = {}
    for case in cases.get("cases", []):
        if case.get("family") == family and case.get("baseline"):
            by_output.setdefault(case["baseline"], []).append(case)
    for unknown in sorted(set(by_output) - set(expected)):
        problems.append(f"{step} prepares unknown output {unknown}")
    exceptions_by_name = {
        f"{GROUPS[entry['group']][0]}/{name}": entry
        for name, entry in exceptions_by_output().items()
        if entry.get("group") in GROUPS
    }
    from phase1_scope import recorded_results

    results = recorded_results(cases)
    exact = excepted = 0
    for name, expected_row in expected.items():
        linked = by_output.get(name, [])
        if len(linked) != 1:
            problems.append(f"{name}: needs exactly one prepared case, found {len(linked)}")
            continue
        case = linked[0]
        if results[case["id"]] not in ("match", "different", "not_implemented"):
            problems.append(f"{name}: comparison is {results[case['id']]!r}, not prepared")
            continue
        row = rows.get(case["id"], {})
        observation = row.get("observation", {})
        # Older probes keep provenance beside rendered; newer ones separate
        # it from the compiler observation in authenticated row metadata.
        extra = row.get("metadata", {})
        if any(observation[key] != extra[key] for key in observation.keys() & extra.keys()):
            problems.append(f"{name}: conflicting observation and metadata fields")
            continue
        metadata = {**observation, **extra}
        if row.get("result") != "observed" or metadata.get("baseline") != name:
            problems.append(f"{name}: no corresponding native observation")
            continue
        rendered = observation.get("rendered")
        if not isinstance(rendered, str) or digest(rendered.encode()) != metadata.get("rendered_sha256"):
            problems.append(f"{name}: rendered bytes disagree with their digest")
            continue
        if metadata.get("expected_sha256") != expected_row["sha256"]:
            problems.append(f"{name}: native observation names a different reference digest")
            continue
        if digest(rendered.encode()) == expected_row["sha256"]:
            if name in exceptions_by_name:
                problems.append(f"{name}: exception now reproduces exactly and needs review")
            else:
                exact += 1
        elif name in exceptions_by_name:
            excepted += 1
        else:
            problems.append(f"{name}: rendering differs without an approved exception")
    declared_total = sum(GROUPS[group][1] for group, _probe in groups)
    return {"total_outputs": len(expected), "exact_outputs": exact,
            "excepted_outputs": excepted, "problems": problems,
            "complete": len(expected) == declared_total and not problems}


def matchfiles_preparation(cases: dict) -> dict:
    """F2a's share of the 309: the 142 carried `config/matchFiles` outputs."""
    return output_preparation(cases, "filesystem")
