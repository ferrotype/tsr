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
    return sorted(p for p in root.rglob("*") if p.is_file())


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
        "status": "blocked",
        "blocker": "phase1-matchfiles-baseline-authority",
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
