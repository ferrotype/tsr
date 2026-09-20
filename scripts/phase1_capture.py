"""Phase 1 capture, comparison and reporting.

`capture` runs the native and Rust children and stores their exact bytes with a
provenance record. `compare` re-reads a stored capture and never runs a child,
so a replay is read-only. Neither step may change the reviewed inventory.

The result vocabulary is the plan's: match, different, not_implemented,
native_unavailable, harness_failed and not_run. Only `match` is feature parity;
`harness_failed` invalidates a capture rather than counting as a non-match.

Two properties this module has to get right, because getting them wrong lets a
capture claim parity it has not earned:

* The source closure must contain every input that can change an observation,
  including the production Rust the driver links. A capture that omits
  `tsr_tsoptions/src/glob.rs` would keep reporting `match` after the matcher
  changed. The closure is derived from `cargo metadata`, not hand-listed, and
  replay recomputes the expected key set rather than trusting the recorded one.
* A response document is validated as an ordered sequence before it is indexed
  by case id. Indexing first would silently accept duplicate rows, extra rows,
  a reordered response or an unknown status.
"""

from __future__ import annotations

import hashlib
import json
import platform
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import command, strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402

RESULTS = ("match", "different", "not_implemented", "native_unavailable", "harness_failed", "not_run")

# `s08_oracle.canonical` serialises with sort_keys=True, so two observations
# whose JSON objects differ only in member order compare equal. That is fatal
# for any case whose subject *is* order -- ordered maps and sets, JSON member
# order, iteration order. Such a request declares `order_sensitive: true`, and
# then two extra rules apply:
#
#   * the observation may not carry a multi-key JSON object anywhere, because
#     that object's order is exactly what canonicalisation destroys. Ordered
#     data travels as entry arrays ([["a",1],["b",2]]) or raw bytes, which is
#     the representation the plan prescribes.
#   * both sides are additionally compared as emitted, not canonicalised.
ORDER_SENSITIVE_KEY = "order_sensitive"

# Each side may only report the statuses it can legitimately produce. A Rust
# driver cannot declare the native authority unavailable, and the native probe
# cannot declare a Rust entry point missing.
RUST_STATUSES = ("observed", "not_implemented", "harness_failed")
NATIVE_STATUSES = ("observed", "native_unavailable", "harness_failed")

# Shared helpers whose behavior changes an observation even though they are not
# family specific. The s04 group is load-bearing rather than incidental:
# `s04.py::verified_upstream` loads `tracking-bootstrap.py` to authenticate the
# submodule, and `s04.py::go_environment` reads the required Go version out of
# `data/s04/toolchains.toml`. Omitting them let a changed Go pin leave existing
# captures looking current.
SHARED_SCRIPTS = (
    "scripts/phase1_capture.py",
    "scripts/phase1.py",
    "scripts/phase1_invocations.py",
    "scripts/s04.py",
    "scripts/s04_common.py",
    "scripts/s04_runtime.py",
    "scripts/s08_oracle.py",
    "scripts/tracking-bootstrap.py",
)
# Build and toolchain inputs that select dependency versions, codegen and the
# Go toolchain the native probes run under.
BUILD_INPUTS = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "data/upstream.json",
    "data/s04/toolchains.toml",
)

FAMILIES = {
    "pilot": {
        "requests": "data/phase1/requests/pilot.json",
        "native_probes": [
            {
                "name": "vfsmatch",
                "package": "vfs/vfsmatch",
                "probe": "tools/phase1/pilot/vfsmatch_probe_test.go",
                "test": "TestPhase1PilotReadDirectory",
            },
            {
                "name": "commandline",
                "package": "tsoptions",
                "probe": "tools/phase1/pilot/commandline_probe_test.go",
                "test": "TestPhase1PilotCommandLine",
                # Compiles into the pinned tsoptions_test package to reach the
                # real formatNewBaseline renderer; see run_probe.
                "trimpath": False,
            },
            {
                "name": "json",
                "package": "json",
                "probe": "tools/phase1/pilot/json_probe_test.go",
                "test": "TestPhase1PilotJson",
            },
            {
                "name": "locale",
                "package": "locale",
                "probe": "tools/phase1/pilot/locale_probe_test.go",
                "test": "TestPhase1PilotLocale",
            },
        ],
        "rust_package": "tsr_tsoptions",
        "rust_target_kind": "example",
        "rust_example": "phase1_pilot",
        "rust_target": "crates/tsr_tsoptions/examples/phase1_pilot.rs",
        "rust_driver": "tools/phase1/pilot/rust_observation.rs",
    },
}
# The six command families the plan names. Only `pilot` is wired at F0; the
# rest are registered so `inventory --check` can report them as unprepared
# rather than silently omitting them.
DECLARED_FAMILIES = ("leaves", "filesystem", "config", "syntax", "utilities", "integration")

_METADATA: dict | None = None


def sha_file(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def pin() -> str:
    return strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]


def gitlink() -> str:
    """The recorded upstream submodule commit, so a moved pin invalidates a capture."""
    entry = subprocess.run(
        ["git", "ls-files", "--stage", "--", "upstream"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    if len(entry) < 2 or entry[0] != "160000":
        raise ValueError("upstream is not a gitlink; the pin cannot be authenticated")
    return entry[1]


def metadata() -> dict:
    global _METADATA
    if _METADATA is None:
        _METADATA = json.loads(
            subprocess.run(
                ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        )
    return _METADATA


def workspace_closure(package: str) -> list[Path]:
    """Every workspace package directory the named package links, transitively.

    Third-party crates are pinned by Cargo.lock, which is hashed separately, so
    only repository-owned sources need enumerating here.
    """
    document = metadata()
    packages = {p["id"]: p for p in document["packages"]}
    local = {pid for pid, p in packages.items() if p.get("source") is None}
    nodes = {n["id"]: n for n in document["resolve"]["nodes"]}
    roots = [pid for pid, p in packages.items() if p["name"] == package]
    if not roots:
        raise ValueError(f"cargo metadata has no workspace package named {package!r}")
    seen: set[str] = set()
    stack = list(roots)
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if dep["pkg"] in local:
                stack.append(dep["pkg"])
    return sorted(Path(packages[pid]["manifest_path"]).parent for pid in seen)


def workspace_package_paths(package: str) -> list[str]:
    return [str(p.relative_to(ROOT)) for p in workspace_closure(package)]


def source_closure(family: str, packages: list[str] | None = None) -> dict[str, str]:
    """Every input whose change can alter this family's observations.

    Derived, not hand-listed: the Rust half comes from the driver package's
    workspace dependency closure, so production code such as
    `tsr_tsoptions/src/glob.rs` and the whole `tsr_vfs` crate is covered. A
    capture that silently omitted them could keep reporting `match` after the
    matcher changed.
    """
    spec = FAMILIES[family]
    paths: set[Path] = set()

    paths.add(Path(spec["requests"]))
    paths.add(Path(spec["rust_driver"]))
    paths.add(Path(spec["rust_target"]))
    for probe in spec["native_probes"]:
        paths.add(Path(probe["probe"]))

    # Everything under the family's adapter directory, recursively, so a file
    # in a nested directory is not missed by a shallow glob.
    adapter = ROOT / "tools/phase1" / family
    if adapter.is_dir():
        paths.update(p.relative_to(ROOT) for p in adapter.rglob("*") if p.is_file())

    for name in (*SHARED_SCRIPTS, *BUILD_INPUTS):
        paths.add(Path(name))
    cargo_config = ROOT / ".cargo"
    if cargo_config.is_dir():
        paths.update(p.relative_to(ROOT) for p in cargo_config.rglob("*") if p.is_file())

    # At capture time the package set is resolved from cargo metadata. On
    # replay it comes from the authenticated provenance instead, so `compare`
    # spawns no build tool: a dependency added since the capture still fails,
    # because Cargo.toml and Cargo.lock are hashed above.
    directories = (
        [ROOT / p for p in packages]
        if packages is not None
        else workspace_closure(spec["rust_package"])
    )
    for directory in directories:
        for path in directory.rglob("*"):
            if not path.is_file():
                continue
            relative = path.relative_to(ROOT)
            if relative.parts[0] == "target" or "target" in relative.parts[:2]:
                continue
            if path.suffix in (".rs", ".toml") or path.name in ("README.md", "NOTICE", "LICENSE"):
                paths.add(relative)

    closure: dict[str, str] = {}
    for relative in sorted({str(p) for p in paths}):
        path = ROOT / relative
        if path.is_file():
            closure[relative] = sha_file(path)
    return closure


def build_rust(family: str) -> Path:
    spec = FAMILIES[family]
    # A family's driver is either an example on an existing crate or a private
    # harness binary under tools/phase1/. Both are supported so a family whose
    # leaf crates no published crate depends on directly can own its host.
    kind = spec.get("rust_target_kind", "example")
    if kind not in ("example", "bin"):
        raise ValueError(f"unknown rust_target_kind {kind!r} for {family}")
    selector = ["--example", spec["rust_example"]] if kind == "example" else ["--bin", spec["rust_example"]]
    messages = command(
        [
            "cargo", "build", "--locked", "--offline", "-p", spec["rust_package"],
            *selector, "--message-format=json",
        ],
        cwd=ROOT,
    )
    executable = None
    for line in messages.splitlines():
        if not line.strip():
            continue
        record = json.loads(line)
        if (
            record.get("reason") == "compiler-artifact"
            and record.get("target", {}).get("name") == spec["rust_example"]
            and record.get("executable")
        ):
            executable = record["executable"]
    if executable is None:
        raise ValueError(f"cargo reported no executable for the {family} driver")
    return Path(executable)


def run_probe(directory: Path, package: str, source: str, request: dict, test: str,
              trimpath: bool = True) -> dict:
    """Run one access-only Go probe under an overlay and authenticate its output.

    This mirrors `s08_oracle.run_overlay`, which is reused wherever it fits. It
    exists because `run_overlay` always passes `-trimpath`, and a probe that
    compiles into the pinned `tsoptions_test` package cannot: that package links
    `internal/testutil/baseline`, whose `init` calls `repo.TestDataPath()`, and
    `repo` panics with "repo root cannot be found when built with -trimpath".
    Sharing that package is the whole point of the renderer seam, so the flag is
    dropped for those probes and the choice is recorded in provenance.
    """
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    source_path = directory / "export_test.go"
    source_path.write_text(source)
    request_path = directory / "requests.json"
    request_bytes = canonical(request) + b"\n"
    request_path.write_bytes(request_bytes)
    output = directory / "observations.json"
    virtual = upstream / "tsc/internal" / package / "phase1_probe_export_test.go"
    if virtual.exists():
        raise ValueError(f"overlay would replace a source file: {virtual}")
    overlay = directory / "overlay.json"
    overlay.write_bytes(canonical({"Replace": {str(virtual): str(source_path)}}))
    env.update(S08_REQUESTS=str(request_path), S08_OUTPUT=str(output))
    arguments = ["go", "test", "-mod=readonly"]
    if trimpath:
        arguments.append("-trimpath")
    arguments += ["-overlay", str(overlay), f"./internal/{package}", "-run", f"^{test}$",
                  "-count=1", "-timeout=5m"]
    stdout = command(arguments, cwd=upstream / "tsc", env=env)
    (directory / "go-test.stdout").write_bytes(stdout)
    verified_upstream()
    report = strict_json_loads(output.read_bytes())
    if report["request_sha256"] != digest(request_bytes):
        raise ValueError(f"the {package} probe observed a different request inventory")
    (directory / "provenance.json").write_bytes(canonical({
        "pin": pin(), "package": package, "test": test, "trimpath": trimpath,
        "source_sha256": digest(source.encode()),
        "request_sha256": digest(request_bytes),
        "output_sha256": digest(output.read_bytes()),
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"],
        "toolchain_local": env["GOTOOLCHAIN"] == "local",
    }) + b"\n")
    return report


def order_safe_problems(value: object, path: str = "observation") -> list[str]:
    """Locate JSON objects whose member order canonicalisation would erase."""
    problems: list[str] = []
    if isinstance(value, dict):
        if len(value) > 1:
            problems.append(
                f"{path} is a {len(value)}-key JSON object; an order-sensitive observation must "
                "use an entry array or raw bytes, because canonicalisation sorts object keys"
            )
        for key, item in value.items():
            problems.extend(order_safe_problems(item, f"{path}.{key}"))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            problems.extend(order_safe_problems(item, f"{path}[{index}]"))
    return problems


def validate_response(document: object, requests: list[dict], side: str) -> list[dict]:
    """Validate an observation document as an ordered sequence.

    Called before anything is indexed by case id. Indexing first would accept a
    duplicated row, an extra failing row, a reordered response or an unknown
    status, because the later dictionary build would quietly drop or reorder
    them.
    """
    statuses = RUST_STATUSES if side == "rust" else NATIVE_STATUSES
    if not isinstance(document, dict):
        raise ValueError(f"{side} response is not a JSON object")
    rows = document.get("observations")
    if not isinstance(rows, list):
        raise ValueError(f"{side} response has no observations array")
    if len(rows) != len(requests):
        raise ValueError(
            f"{side} response has {len(rows)} rows for {len(requests)} requests"
        )
    seen: set[str] = set()
    for index, (row, request) in enumerate(zip(rows, requests)):
        where = f"{side} row {index}"
        if not isinstance(row, dict):
            raise ValueError(f"{where} is not an object")
        case = row.get("case")
        if case != request["case"]:
            raise ValueError(
                f"{where} reports case {case!r} where the request schedule has "
                f"{request['case']!r}; the response is reordered or substituted"
            )
        if case in seen:
            raise ValueError(f"{where} duplicates case {case!r}")
        seen.add(case)
        operation = row.get("operation")
        if operation != request.get("operation"):
            raise ValueError(
                f"{where} reports operation {operation!r} for case {case!r}, "
                f"but the request asked for {request.get('operation')!r}"
            )
        result = row.get("result")
        if result not in statuses:
            raise ValueError(
                f"{where} reports unknown {side} status {result!r}; "
                f"allowed statuses are {', '.join(statuses)}"
            )
        if result == "observed" and "observation" not in row:
            raise ValueError(f"{where} is observed but carries no observation payload")
        if result == "observed" and request.get(ORDER_SENSITIVE_KEY):
            problems = order_safe_problems(row["observation"])
            if problems:
                raise ValueError(
                    f"{where} answers order-sensitive case {case!r} with an order-erasing "
                    f"representation: {problems[0]}"
                )
        if result == "not_implemented":
            missing = row.get("missing_operation")
            required = ("operation", "go_authority", "intended_signature", "production_home")
            if not isinstance(missing, dict) or any(not missing.get(k) for k in required):
                raise ValueError(
                    f"{where} is not_implemented without a complete missing_operation record"
                )
        if result == "native_unavailable" and not row.get("reason"):
            raise ValueError(f"{where} is native_unavailable without a recorded reason")
        if result == "harness_failed" and not (row.get("error") or row.get("reason")):
            raise ValueError(f"{where} is harness_failed without a recorded cause")
    return rows


def capture(family: str, output: Path, cases: list[str] | None = None) -> dict:
    if family not in FAMILIES:
        raise ValueError(
            f"family {family!r} has no adapter yet; declared families are "
            + ", ".join(DECLARED_FAMILIES)
        )
    spec = FAMILIES[family]
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)

    before = source_closure(family)
    recorded_pin, recorded_gitlink = pin(), gitlink()
    if recorded_pin != recorded_gitlink:
        raise ValueError("data/upstream.json and the upstream gitlink disagree on the pin")

    document = strict_json_loads((ROOT / spec["requests"]).read_bytes())
    selected = document["requests"]
    partial = False
    if cases:
        wanted = set(cases)
        unknown = sorted(wanted - {r["case"] for r in selected})
        if unknown:
            raise ValueError("unknown case ids: " + ", ".join(unknown))
        selected = [r for r in selected if r["case"] in wanted]
        partial = len(selected) != len(document["requests"])

    # The children read exactly these bytes; hash the serialized request, not an
    # earlier in-memory object.
    request_document = {"version": document["version"], "family": family, "requests": selected}
    request_bytes = canonical(request_document) + b"\n"
    request_path = output / "requests.json"
    request_path.write_bytes(request_bytes)

    # One native probe per Go package. Each sees the whole schedule and declines
    # the operations it does not serve, so every case has a native row.
    native_reports = {}
    seen_probe_names = set()
    for probe in spec["native_probes"]:
        # Keyed by the probe's own name, not its package: a family may need two
        # probes in one Go package, for instance to give a process-global
        # default a fresh process per case.
        name = probe["name"]
        if name in seen_probe_names:
            raise ValueError(f"duplicate native probe name {name!r} in family {family}")
        seen_probe_names.add(name)
        report = run_probe(
            output / "native" / name,
            probe["package"],
            (ROOT / probe["probe"]).read_text(),
            request_document,
            probe["test"],
            probe.get("trimpath", True),
        )
        validate_response(report, selected, "native")
        native_reports[name] = {
            "package": probe["package"],
            "directory": f"native/{name}",
            "observations_sha256": sha_file(output / "native" / name / "observations.json"),
            "go": report.get("go"),
            "goos": report.get("goos"),
            "goarch": report.get("goarch"),
            "trimpath": probe.get("trimpath", True),
        }

    executable = build_rust(family)
    rust_path = output / "rust-observations.json"
    command([str(executable), str(request_path), str(rust_path)], cwd=ROOT)
    validate_response(strict_json_loads(rust_path.read_bytes()), selected, "rust")

    after = source_closure(family)
    if before != after:
        changed = sorted(
            set(before) ^ set(after)
            | {k for k in set(before) & set(after) if before[k] != after[k]}
        )
        raise ValueError(
            "a source input changed while capturing; the capture is invalid: "
            + ", ".join(changed[:5])
        )

    provenance = {
        "version": 3,
        "family": family,
        "pin": recorded_pin,
        "upstream_gitlink": recorded_gitlink,
        "partial": partial,
        "selected_cases": sorted(r["case"] for r in selected),
        "requests_sha256": digest(request_bytes),
        "native_probes": native_reports,
        "rust_observations_sha256": sha_file(rust_path),
        "rust_binary_sha256": sha_file(executable),
        "workspace_packages": workspace_package_paths(spec["rust_package"]),
        "source_closure": before,
        "source_closure_size": len(before),
        "host": {"platform": platform.platform(), "python": platform.python_version()},
    }
    (output / "provenance.json").write_bytes(canonical(provenance) + b"\n")
    return provenance


def authenticate(directory: Path) -> dict:
    """Public entry point: validate a stored capture without reading its rows."""
    return _authenticate(Path(directory).resolve())


def _authenticate(directory: Path) -> dict:
    provenance = strict_json_loads((directory / "provenance.json").read_bytes())
    if provenance.get("version") != 3:
        raise ValueError(
            f"capture provenance version {provenance.get('version')!r} is not readable by this "
            "comparator; recapture the family"
        )
    family = provenance["family"]

    if provenance.get("pin") != pin():
        raise ValueError("capture pin differs from the repository pin")
    if provenance.get("upstream_gitlink") != gitlink():
        raise ValueError("capture was taken against a different upstream gitlink")

    checks = {
        "requests.json": provenance["requests_sha256"],
        "rust-observations.json": provenance["rust_observations_sha256"],
    }
    for probe in provenance["native_probes"].values():
        checks[f"{probe['directory']}/observations.json"] = probe["observations_sha256"]
    for name, expected in checks.items():
        if sha_file(directory / name) != expected:
            raise ValueError(f"capture artifact {name} does not match its recorded hash")

    # Recompute the expected closure rather than trusting the recorded one: a
    # capture that recorded too few inputs must not authenticate just because
    # the few it recorded are unchanged.
    expected_closure = source_closure(family, provenance.get("workspace_packages"))
    recorded = provenance["source_closure"]
    if len(recorded) != provenance.get("source_closure_size"):
        raise ValueError("capture source closure size disagrees with its own contents")
    absent = sorted(set(expected_closure) - set(recorded))
    if absent:
        raise ValueError(
            f"capture omitted {len(absent)} source input(s) that can change its observations: "
            + ", ".join(absent[:5])
        )
    extra = sorted(set(recorded) - set(expected_closure))
    if extra:
        raise ValueError(
            f"capture recorded {len(extra)} input(s) that are no longer part of the closure: "
            + ", ".join(extra[:5])
        )
    for relative, expected in recorded.items():
        path = ROOT / relative
        if not path.is_file():
            raise ValueError(f"capture input {relative} no longer exists")
        if sha_file(path) != expected:
            raise ValueError(f"capture input {relative} changed after the capture")
    return provenance


def _merge_native(directory: Path, provenance: dict, requests: list[dict]) -> dict[str, dict]:
    """One native row per case, taking the probe that actually served it.

    Every probe reports every case; the one that owns an operation returns
    `observed` and the rest return `native_unavailable`. Two probes claiming the
    same case is a contradiction, not a merge.

    A harness failure is collected across *all* probes and raised before any
    merging happens. Merging first hid it two ways: an earlier
    `native_unavailable` won the `setdefault`, and an `observed` row from the
    owning probe overwrote a failure reported by another.
    """
    documents = {}
    failures: list[str] = []
    for probe_name, probe in sorted(provenance["native_probes"].items()):
        document = strict_json_loads((directory / probe["directory"] / "observations.json").read_bytes())
        rows = validate_response(document, requests, "native")
        documents[probe_name] = rows
        for row in rows:
            if row["result"] == "harness_failed":
                cause = row.get("error") or row.get("reason") or "no cause recorded"
                failures.append(f"{probe_name} failed on {row['case']}: {cause}")
    if failures:
        raise ValueError(
            f"{len(failures)} native harness failure(s) invalidate this capture: "
            + "; ".join(failures[:5])
        )

    merged: dict[str, dict] = {}
    for probe_name, rows in documents.items():
        for row in rows:
            case = row["case"]
            if row["result"] != "observed":
                merged.setdefault(case, row)
                continue
            existing = merged.get(case)
            if existing is not None and existing.get("result") == "observed":
                raise ValueError(
                    f"two native probes both observed case {case}; the authority is ambiguous"
                )
            merged[case] = dict(row, native_probe=probe_name)
    return merged


def validate_capture(directory: Path) -> tuple[dict, list[dict], dict[str, dict], dict[str, dict]]:
    """Full validation of a stored capture, short of comparing the two sides.

    Authentication alone checks bytes, pin, gitlink and the source closure; it
    never reads a row. That is not enough to accept a capture, because a
    correctly hashed capture can still contain a `harness_failed` row. This
    adds the content checks:

    * every response is a valid ordered sequence for the request schedule, and
    * neither side reports a harness failure.

    Rust parity is deliberately *not* required. `not_implemented` is the
    expected preparation-time result for an operation this phase has still to
    write; `harness_failed` means the observation never happened at all.
    """
    directory = Path(directory).resolve()
    provenance = _authenticate(directory)
    requests = strict_json_loads((directory / "requests.json").read_bytes())["requests"]
    # _merge_native raises on any native harness failure, across every probe.
    native_rows = _merge_native(directory, provenance, requests)
    rust_list = validate_response(
        strict_json_loads((directory / "rust-observations.json").read_bytes()), requests, "rust"
    )
    failures = [
        f"{row['case']}: {row.get('error') or row.get('reason') or 'no cause recorded'}"
        for row in rust_list
        if row["result"] == "harness_failed"
    ]
    if failures:
        raise ValueError(
            f"{len(failures)} Rust harness failure(s) invalidate this capture: "
            + "; ".join(failures[:5])
        )
    missing_rows = sorted({r["case"] for r in requests} - set(native_rows))
    if missing_rows:
        raise ValueError(
            "a native probe produced no row for selected case(s): " + ", ".join(missing_rows[:5])
        )
    return provenance, requests, native_rows, {row["case"]: row for row in rust_list}


def compare(directory: Path, require_parity: bool = False) -> dict:
    """Compare stored outputs. Runs no child process."""
    directory = Path(directory).resolve()
    provenance, requests, native_rows, rust_rows = validate_capture(directory)

    inventory = strict_json_loads((ROOT / FAMILIES[provenance["family"]]["requests"]).read_bytes())
    all_cases = [r["case"] for r in inventory["requests"]]
    selected = {r["case"] for r in requests}
    unknown = selected - set(all_cases)
    if unknown:
        raise ValueError(
            "captured cases are absent from the frozen inventory: " + ", ".join(sorted(unknown))
        )

    rows = []
    for case in all_cases:
        if case not in selected:
            rows.append({"case": case, "result": "not_run", "reason": "outside the captured selection"})
            continue
        native = native_rows.get(case)
        rust = rust_rows.get(case)
        if native is None or rust is None:
            rows.append({"case": case, "result": "harness_failed",
                         "reason": "a child produced no row for a selected case"})
            continue
        if rust["result"] == "harness_failed":
            rows.append({"case": case, "result": "harness_failed", "reason": rust.get("error", "")})
            continue
        if native["result"] == "harness_failed":
            rows.append({"case": case, "result": "harness_failed",
                         "reason": native.get("error") or native.get("reason", "")})
            continue
        if rust["result"] == "not_implemented":
            rows.append({"case": case, "result": "not_implemented",
                         "missing_operation": rust.get("missing_operation"),
                         "native_result": native.get("result"),
                         "native_observation": native.get("observation")})
            continue
        if native["result"] == "native_unavailable":
            rows.append({"case": case, "result": "native_unavailable", "reason": native.get("reason", "")})
            continue
        request = next(r for r in requests if r["case"] == case)
        if request.get(ORDER_SENSITIVE_KEY):
            # Compare as emitted as well, so a pure member-order difference is
            # still a difference rather than being canonicalised away.
            same = (
                json.dumps(native.get("observation"), separators=(",", ":"))
                == json.dumps(rust.get("observation"), separators=(",", ":"))
            )
        else:
            same = canonical(native.get("observation")) == canonical(rust.get("observation"))
        rows.append({
            "case": case,
            "result": "match" if same else "different",
            **({} if same else {"native": native.get("observation"), "rust": rust.get("observation")}),
        })

    counts = {result: 0 for result in RESULTS}
    for row in rows:
        counts[row["result"]] += 1
    required_non_match = counts["different"] + counts["not_implemented"] + counts["native_unavailable"]
    report = {
        "version": 2,
        "family": provenance["family"],
        "pin": provenance["pin"],
        "partial": provenance["partial"],
        "capture": str(directory),
        "requests_sha256": provenance["requests_sha256"],
        "counts": counts,
        "parity": counts["match"] / len(rows) if rows else 0.0,
        "required_non_match": required_non_match,
        "rows": rows,
    }
    if counts["harness_failed"]:
        raise ValueError(
            f"{counts['harness_failed']} case(s) failed in the harness; the capture is invalid"
        )
    if require_parity and (required_non_match or counts["not_run"]):
        raise ValueError(
            "parity required but the report has "
            f"{required_non_match} non-matching and {counts['not_run']} unrun case(s)"
        )
    return report


def join(reports: list[dict], selection_required: bool = False) -> dict:
    """Join verified reports by case id, rejecting duplicates and conflicts."""
    rows: dict[str, dict] = {}
    families: dict[str, str] = {}
    for report in reports:
        family = report["family"]
        if family in families and families[family] != report["requests_sha256"]:
            raise ValueError(f"reports for family {family} use different request schedules")
        families[family] = report["requests_sha256"]
        for row in report["rows"]:
            case = row["case"]
            if case in rows and rows[case]["result"] != row["result"]:
                if rows[case]["result"] == "not_run":
                    rows[case] = row
                    continue
                if row["result"] == "not_run":
                    continue
                raise ValueError(f"case {case} reported twice with different results")
            rows.setdefault(case, row)
    counts = {result: 0 for result in RESULTS}
    for row in rows.values():
        counts[row["result"]] += 1
    if selection_required and counts["not_run"]:
        raise ValueError(f"full-family acceptance rejects {counts['not_run']} unrun case(s)")
    return {
        "version": 2,
        "families": sorted(families),
        "counts": counts,
        "total": len(rows),
        "rows": [rows[case] for case in sorted(rows)],
    }
