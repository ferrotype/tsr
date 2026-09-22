"""Phase 1 F4a plan tasks 2 and 3: the producer of the syntax schedule.

This module is the only Python a schedule capture depends on beyond the shared
S04/S07 helpers, so it alone is a recorded schedule input: validators, the smoke
and documentation in scripts/phase1_syntax.py can change without making a
committed capture stale. See scripts/phase1_syntax.py for the checks.
"""

from __future__ import annotations

import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

ROOT = Path(__file__).resolve().parents[1]
SUBSET = "data/s07/subset.json"


def _digest(relative: str) -> str:
    return hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()


# ---------------------------------------------------------------------------
# Plan tasks 2 and 3: the syntax-only program schedule and its native capture.
#
# The schedule has one row per declared compiler variant -- all 15,206, not the
# E2 subset's eligible 10,728 -- rebuilt from a fresh run of the native
# preprocessing the S07 subset already uses (TestS07SubsetExport). Each rebuilt
# loading request must hash to the `loading_request_sha256` data/s07/subset.json
# recorded, so the schedule loads the programs S07 froze and nothing else.
#
# A row that cannot run the syntactic operation keeps its native reason instead
# of becoming an empty result:
#
# * options_rejected -- the harness fails in SetOptionsFromTestConfig before it
#   compiles anything; the row keeps the native CLI option diagnostics.
# * content_mapper -- the program parses mapper-transformed text, and a loading
#   request cannot carry the mapper. Syntactic diagnostics of mapped files are
#   the Phase 5 content-mapper integration's; an unmapped load must not stand in
#   for them.
#
# Every other row is loaded by the pinned compiler.NewProgram and observed with
# Program.GetSyntacticDiagnostics alone. The native harness's own selection --
# the skippedTests filename list and SkipUnsupportedCompilerOptions -- is
# executed and recorded per row but selects nothing: a row the native runner
# skips is still a real program with real syntactic diagnostics.
#
# The syntactic observation is NOT the committed .errors.txt. Those baselines
# also hold program, semantic, global and declaration diagnostics, rendered by
# the error-baseline writer. This schedule renders the syntactic list through
# the production diagnosticwriter only (plain and pretty), so no row can be
# compared with, or mistaken for, a whole error baseline.
# ---------------------------------------------------------------------------

SCHEDULE = ROOT / "data/phase1/syntax-schedule.json"
NATIVE = ROOT / "data/phase1/syntax-native.json"
PROBE = "tools/phase1/syntax/program_probe_test.go"
PROBE_TEST = "TestPhase1SyntaxSchedule"
# Inputs that decide a schedule row or a native observation. The pinned Go
# sources are authenticated by verified_upstream(); these are ours.
SCHEDULE_INPUTS = (
    PROBE,
    "scripts/phase1_syntax_schedule.py",
    "scripts/s07_subset.py",
    "scripts/s06_corpus.py",
    "scripts/s06_protocol.py",
    "scripts/s08_oracle.py",
    "scripts/s04.py",
    "scripts/s04_common.py",
    "data/s04/toolchains.toml",
    "data/upstream.json",
    "data/s07/subset.json",
    "data/s06/corpus.json",
)
BOUNDARIES = {
    "options_rejected": "the native harness fails in SetOptionsFromTestConfig before compiling; the row "
                        "keeps the native command-line option diagnostics and never loads",
    "content_mapper": "the program parses mapper-transformed text that a loading request cannot carry; "
                      "its syntactic diagnostics are the named Phase 5 content-mapper boundary",
}
SELECTIONS = {
    "runs": "the native compiler runner compiles and verifies this variant",
    "filename_skip": "testrunner.skippedTests names the physical source, so the native runner never "
                     "enumerates it",
    "option_guard_skip": "harnessutil.SkipUnsupportedCompilerOptions skips verification after compiling",
    "options_rejected": "harnessutil.SetOptionsFromTestConfig fails before the runner reaches the guard",
}


def _json_canonical(value: object) -> bytes:
    from s08_oracle import canonical

    return canonical(value) + b"\n"


def schedule_request_bytes(probes: list[dict]) -> bytes:
    """Serialize the children’s requests without reordering semantic option maps."""
    from s07_subset import json_bytes

    return json_bytes(probes)


def _write_rows(path: Path, document: dict) -> None:
    """Canonical JSON with one `rows` element per line, so a changed row is a one-line diff."""
    def encode(value: object) -> str:
        return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)

    head = encode({key: value for key, value in document.items() if key != "rows"})
    rows = ",\n".join(encode(row) for row in document["rows"])
    path.write_text(head[:-1] + ',"rows":[\n' + rows + "\n]}\n")


def schedule_inputs() -> dict[str, str]:
    import s07_subset

    # The exported preprocessing requests are an input to this producer too.
    # Reuse their closure so a new copied bridge cannot be missed here.
    return {name: _digest(name) for name in sorted(set(SCHEDULE_INPUTS) | set(s07_subset.PRODUCER_INPUTS))}


def schedule_requests(observations: Path) -> tuple[list[dict], list[dict]]:
    """Schedule rows and probe requests for every variant, from native preprocessing.

    `observations` is a TestS07SubsetExport output; read_observations
    authenticates it against the pin and the frozen S06 corpus before any row is
    built.
    """
    import copy
    import s07_subset

    _inventory, cases, _libraries, _corpus = s07_subset.read_observations(observations)
    committed = {variant["id"]: variant for case in json.loads((ROOT / SUBSET).read_text())["cases"]
                 for variant in case["variants"]}
    rows, probes = [], []
    for case in cases:
        source = case["case"]
        primary = s07_subset.primary_id(source["path"])
        for configuration, variant in enumerate(case["variants"]):
            rid = f"{primary}#configuration={configuration}"
            request = copy.deepcopy(variant["request"])
            request["id"] = rid
            digest = s07_subset.sha256(s07_subset.json_bytes(request))
            frozen = committed.get(rid)
            if frozen is None:
                raise ValueError(f"{rid} is not a data/s07/subset.json variant")
            if frozen["loading_request_sha256"] != digest:
                raise ValueError(f"{rid}: native preprocessing built a different loading request than S07 froze")
            rejected = bool(variant["option_diagnostics"])
            boundary = "options_rejected" if rejected else "content_mapper" if variant["content_mappers"] else None
            rows.append({
                "id": rid, "primary": primary, "configuration": configuration,
                "configured_name": variant["configured_name"], "loading_request_sha256": digest,
                "e2_disposition": frozen["disposition"], "boundary": boundary,
                "option_diagnostics": variant["option_diagnostics"] if rejected else [],
                "request_shape": _request_shape(request),
            })
            probes.append({"id": rid, "path": source["path"], "guard": not rejected,
                           "load": boundary is None, "request": request})
    if len(rows) != len(committed) or {row["id"] for row in rows} != set(committed):
        raise ValueError("native preprocessing does not reproduce every S07 variant")
    return rows, probes


def run_probe(directory: Path, probes: list[dict]) -> dict:
    """Run the native syntax probe once over every probe request.

    The probe is compiled into the pinned testrunner package through a go test
    overlay, so the checkout is never edited. A failed Go run yields nothing.
    """
    from s04 import go_environment, verified_upstream
    from s04_common import command, strict_json_loads

    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    virtual = upstream / "tsc/internal/testrunner/phase1_syntax_probe_test.go"
    if virtual.exists():
        raise ValueError(f"overlay would replace a source file: {virtual}")
    request_bytes = schedule_request_bytes(probes)
    (directory / "requests.json").write_bytes(request_bytes)
    output = directory / "observations.json"
    (directory / "overlay.json").write_bytes(_json_canonical({"Replace": {str(virtual): str(ROOT / PROBE)}}))
    env.update(PHASE1_SYNTAX_REQUESTS=str(directory / "requests.json"), PHASE1_SYNTAX_OUTPUT=str(output))
    inputs = schedule_inputs()
    repo = f"-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={directory}/unmatched-prefix"
    stdout = command(["go", "test", "-trimpath", "-mod=readonly", repo, "-overlay", str(directory / "overlay.json"),
                      "./internal/testrunner", "-run", f"^{PROBE_TEST}$", "-count=1", "-timeout=60m"],
                     cwd=upstream / "tsc", env=env)
    (directory / "go-test.stdout").write_bytes(stdout)
    verified_upstream()
    if schedule_inputs() != inputs:
        raise ValueError("syntax schedule inputs changed during the native run")
    report = strict_json_loads(output.read_bytes())
    if report["request_sha256"] != hashlib.sha256(request_bytes).hexdigest():
        raise ValueError("the native probe observed a different request inventory")
    return report


def _selection(native: dict) -> str:
    if native["filename_skip"]:
        return "filename_skip"
    return {"allowed": "runs", "skipped": "option_guard_skip", "not_reached": "options_rejected"}[native["option_guard"]]


def join(rows: list[dict], probes: list[dict], report: dict) -> tuple[dict, dict]:
    """The committed schedule and native observation documents."""
    observed = report["rows"]
    if [row["id"] for row in observed] != [probe["id"] for probe in probes]:
        raise ValueError("missing, extra, duplicate or reordered native syntax rows")
    schedule, native = [], []
    for row, probe, result in zip(rows, probes, observed, strict=True):
        guard = result["option_guard"]
        if guard not in ("allowed", "skipped", "not_reached") or (guard == "not_reached") == probe["guard"]:
            raise ValueError(f"{row['id']}: unclassified native option guard outcome {guard!r}")
        load = result["load"]
        if load == "panic":
            raise ValueError(f"{row['id']}: unexpected native syntax panic invalidates the capture: "
                             f"{result.get('panic')}")
        if load not in ("loaded", "panic", "not_loaded") or (load == "not_loaded") == probe["load"]:
            raise ValueError(f"{row['id']}: unclassified native load outcome {load!r}")
        syntactic = "observed" if load == "loaded" else "load_panicked" if load == "panic" else "boundary"
        schedule.append({**row, "native_selection": _selection(result), "filename_skip": result["filename_skip"],
                         "option_guard": guard, "load": load, "syntactic": syntactic})
        if load == "loaded":
            native.append({key: result[key] for key in
                           ("id", "files", "file_names_sha256", "syntactic", "plain_hex", "pretty_hex")})
        elif load == "panic":
            native.append({"id": row["id"], "panic": result["panic"]})
    pin = json.loads((ROOT / "data/upstream.json").read_text())["pin"]
    provenance = {"pin": pin, "go": report["go"], "goos": report["goos"], "goarch": report["goarch"],
                  "request_sha256": report["request_sha256"], "inputs": schedule_inputs()}
    counts = {
        "variants": len(schedule),
        "by_syntactic": dict(sorted(Counter(row["syntactic"] for row in schedule).items())),
        "by_boundary": dict(sorted(Counter(row["boundary"] for row in schedule if row["boundary"]).items())),
        "by_native_selection": dict(sorted(Counter(row["native_selection"] for row in schedule).items())),
        "by_e2_disposition": dict(sorted(Counter(row["e2_disposition"] for row in schedule).items())),
        "with_syntactic_diagnostics": sum(1 for row in native if row.get("syntactic")),
        "syntactic_diagnostics": sum(len(row.get("syntactic") or []) for row in native),
    }
    schedule_document = {
        "version": 1, "provenance": provenance, "counts": counts,
        "boundaries": BOUNDARIES, "selections": SELECTIONS,
        "phase": "Program.GetSyntacticDiagnostics only; not the whole .errors.txt baseline",
        "rows": schedule,
    }
    native_document = {"version": 1, "provenance": provenance,
                       "rendering": {"writer": "diagnosticwriter.WriteFormatDiagnostics and "
                                               "FormatDiagnosticsWithColorAndContext",
                                     "new_line": "\r\n", "current_directory": "request cwd"},
                       "rows": native}
    return schedule_document, native_document



EXTENSIONS = (".d.ts", ".d.mts", ".d.cts", ".tsx", ".ts", ".mts", ".cts", ".jsx", ".js", ".mjs", ".cjs", ".json")


def _extension(name: str) -> str:
    lowered = name.lower()
    return next((extension for extension in EXTENSIONS if lowered.endswith(extension)), "other")


def _request_shape(request: dict) -> list:
    """The request facts the smoke stratifies on, kept in the schedule so the
    selection can be recomputed from committed data alone."""
    options = request["options"]
    return [sorted({_extension(name) for name in request["files"]}), options.get("checkJs"),
            options.get("allowJs"), options.get("experimentalDecorators")]



def capture(directory: Path, *, write_committed: bool = False) -> dict:
    """Native preprocessing, then the native syntax probe, once."""
    import s07_subset

    directory = Path(directory).resolve()
    observations = directory / "source-observations.ndjson"
    if not observations.is_file():
        s07_subset.export_observations(observations)
    rows, probes = schedule_requests(observations)
    report = run_probe(directory / "native", probes)
    schedule, native = join(rows, probes, report)
    for document, path in ((schedule, SCHEDULE), (native, NATIVE)):
        target = path if write_committed else directory / path.name
        _write_rows(target, document)
    return schedule["counts"]
