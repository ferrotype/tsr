"""Phase 1 capture, comparison and reporting.

`capture` runs the native and Rust children and stores their exact bytes with a
provenance record. `compare` re-reads a stored capture and never runs a child,
so a replay is read-only. Neither step may change the reviewed inventory.

The result vocabulary is the plan's: match, different, not_implemented,
native_unavailable, not_applicable, harness_failed and not_run. Only `match` is feature parity;
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
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from s04 import go_environment, verified_upstream  # noqa: E402
from s04_common import command, strict_json_loads  # noqa: E402
from s08_oracle import ROOT, canonical, digest  # noqa: E402
import phase1_hosts as hosts  # noqa: E402

class StaleCapture(ValueError):
    """Authenticated artifacts no longer describe the current inputs."""


def request_bytes(value: object) -> bytes:
    """Requests retain object insertion order, including ordered config maps.

    Observation metadata may be canonicalized; compiler input cannot be sorted:
    `paths` pattern precedence follows the order supplied by the caller.
    """
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode() + b"\n"


def package_input_files(directory: Path) -> list[Path]:
    """Package inputs excluding prose and host metadata, retaining embedded assets.

    Literal include_str!/include_bytes! assets remain inputs regardless of suffix.
    A build script or computed include can choose arbitrary files, so such a
    package conservatively retains markdown too.
    """
    files = [p for p in directory.rglob("*") if p.is_file() and p.name != ".DS_Store"
             and not any(part in (".git", "target", "__pycache__")
                         for part in p.relative_to(directory).parts)]
    embedded: set[Path] = set()
    arbitrary_inputs = (directory / "build.rs").is_file()
    for source in files:
        if source.suffix != ".rs":
            continue
        text = source.read_text()
        for match in re.finditer(r'include_(?:str|bytes)\s*!\s*\(\s*([^)]*)', text):
            literal = re.fullmatch(r'"([^"\\]*)"\s*,?\s*', match[1])
            if literal:
                embedded.add((source.parent / literal[1]).resolve())
            else:
                arbitrary_inputs = True
    return sorted({p for p in files if p.suffix.lower() != ".md" or arbitrary_inputs or p.resolve() in embedded}
                  | {p for p in embedded if p.is_file()})


RESULTS = ("match", "different", "not_implemented", "native_unavailable", "not_applicable", "harness_failed", "not_run")

# `s08_oracle.canonical` serialises with sort_keys=True, so two observations
# whose JSON objects differ only in member order compare equal. That is fatal
# for any case whose subject *is* order -- ordered maps and sets, JSON member
# order, iteration order.
#
# The fix is representational, not a different comparison. Canonicalisation
# preserves *array* order, so ordered data travels as an array and compares
# correctly through the existing canonical path. Comparing the raw emitted
# bytes instead would be actively wrong: Go's encoding/json sorts map keys
# while Rust's serde_json preserve_order does not, so two agreeing sides would
# differ on named fields alone.
#
# A request declares `order_sensitive: true`. Its observation must then carry
# the ordered payload under `ordered` as a list, and no value nested inside
# those elements may be a multi-key object, because such an object is an
# ordered map whose order canonicalisation would erase. An element may itself
# be a multi-key object: those are named result fields, and their order carries
# no information.
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
    "scripts/phase1_scope.py",
    "scripts/phase1_hosts.py",
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
    "leaves": {
        # One request fragment per coverage group, so each group owns its file.
        "requests": [
            "data/phase1/requests/leaves.json",
            "data/phase1/requests/leaves-collections.json",
            "data/phase1/requests/leaves-core.json",
            "data/phase1/requests/leaves-options.json",
            "data/phase1/requests/leaves-helpers.json",
            "data/phase1/requests/leaves-json.json",
            "data/phase1/requests/leaves-text.json",
            "data/phase1/requests/leaves-locale.json",
            "data/phase1/requests/leaves-diagnostics.json",
            "data/phase1/requests/leaves-bundled.json",
        ],
        "native_probes": [
            {"name": "collections", "package": "collections",
             "probe": "tools/phase1/leaves/collections_probe_test.go",
             "test": "TestPhase1LeavesCollections"},
            {"name": "core", "package": "core",
             "probe": "tools/phase1/leaves/core_probe_test.go",
             "test": "TestPhase1LeavesCore"},
            # Three probes share package `core`: the option getters and the
            # generic helpers are separate surfaces with separate action
            # vocabularies, and one overlay file per probe keeps them so.
            {"name": "options", "package": "core",
             "probe": "tools/phase1/leaves/options_probe_test.go",
             "test": "TestPhase1LeavesOptions"},
            {"name": "helpers", "package": "core",
             "probe": "tools/phase1/leaves/helpers_probe_test.go",
             "test": "TestPhase1LeavesHelpers"},
            {"name": "json", "package": "json",
             "probe": "tools/phase1/leaves/json_probe_test.go",
             "test": "TestPhase1LeavesJson"},
            {"name": "stringutil", "package": "stringutil",
             "probe": "tools/phase1/leaves/text_probe_test.go",
             "test": "TestPhase1LeavesText"},
            {"name": "semver", "package": "semver",
             "probe": "tools/phase1/leaves/semver_probe_test.go",
             "test": "TestPhase1LeavesSemver"},
            {"name": "jsnum", "package": "jsnum",
             "probe": "tools/phase1/leaves/jsnum_probe_test.go",
             "test": "TestPhase1LeavesJsnum"},
            # Two probes in one package: the second observes the process-global
            # default locale, which needs its own process to be honest.
            {"name": "locale", "package": "locale",
             "probe": "tools/phase1/leaves/locale_probe_test.go",
             "test": "TestPhase1LeavesLocale"},
            {"name": "locale-default", "package": "locale",
             "probe": "tools/phase1/leaves/locale_default_probe_test.go",
             "test": "TestPhase1LeavesLocaleDefault"},
            {"name": "diagnostics", "package": "diagnostics",
             "probe": "tools/phase1/leaves/diagnostics_probe_test.go",
             "test": "TestPhase1LeavesDiagnostics"},
            # bundledSourceDir locates its package through runtime.Caller(0),
            # which under -trimpath returns a wrong path and does NOT panic.
            {"name": "bundled", "package": "bundled",
             "probe": "tools/phase1/leaves/bundled_probe_test.go",
             "test": "TestPhase1LeavesBundled", "trimpath": False},
        ],
        "rust_package": "phase1_leaves",
        "rust_target_kind": "bin",
        "rust_example": "phase1_leaves",
        "rust_target": "tools/phase1/leaves/src/main.rs",
    },
    "filesystem": {
        # One request fragment per adapter surface, so each group owns its file.
        "requests": [
            "data/phase1/requests/filesystem-tspath.json",
            "data/phase1/requests/filesystem-glob.json",
            "data/phase1/requests/filesystem-vfsmatch.json",
            "data/phase1/requests/filesystem-vfstest.json",
            "data/phase1/requests/filesystem-cachedvfs.json",
            "data/phase1/requests/filesystem-wrapvfs.json",
            "data/phase1/requests/filesystem-iovfs.json",
            "data/phase1/requests/filesystem-vfsmock.json",
            "data/phase1/requests/filesystem-symlinks.json",
            "data/phase1/requests/filesystem-osvfs.json",
            "data/phase1/requests/filesystem-matchfiles.json",
            "data/phase1/requests/filesystem-composed.json",
        ],
        "native_probes": [
            {"name": "tspath", "package": "tspath",
             "probe": "tools/phase1/filesystem/tspath_probe_test.go",
             "test": "TestPhase1FilesystemTspath"},
            {"name": "glob", "package": "glob",
             "probe": "tools/phase1/filesystem/glob_probe_test.go",
             "test": "TestPhase1FilesystemGlob"},
            {"name": "vfsmatch", "package": "vfs/vfsmatch",
             "probe": "tools/phase1/filesystem/vfsmatch_probe_test.go",
             "test": "TestPhase1FilesystemVfsmatch"},
            {"name": "vfstest", "package": "vfs/vfstest",
             "probe": "tools/phase1/filesystem/vfstest_probe_test.go",
             "test": "TestPhase1FilesystemVfstest"},
            {"name": "cachedvfs", "package": "vfs/cachedvfs",
             "probe": "tools/phase1/filesystem/cachedvfs_probe_test.go",
             "test": "TestPhase1FilesystemCachedvfs"},
            {"name": "wrapvfs", "package": "vfs/wrapvfs",
             "probe": "tools/phase1/filesystem/wrapvfs_probe_test.go",
             "test": "TestPhase1FilesystemWrapvfs"},
            {"name": "iovfs", "package": "vfs/iovfs",
             "probe": "tools/phase1/filesystem/iovfs_probe_test.go",
             "test": "TestPhase1FilesystemIovfs"},
            {"name": "vfsmock", "package": "vfs/vfsmock",
             "probe": "tools/phase1/filesystem/vfsmock_probe_test.go",
             "test": "TestPhase1FilesystemVfsmock"},
            # The live OS group mutates a real filesystem, so it stays inside a
            # per-case temporary root and its cases declare host applicability:
            # one host's results never certify the other.
            # internal/symlinks is its own Go package, so it needs its own
            # overlay file: a probe compiled into `tspath` cannot reach it.
            {"name": "symlinks", "package": "symlinks",
             "probe": "tools/phase1/filesystem/symlinks_probe_test.go",
             "test": "TestPhase1FilesystemSymlinks"},
            {"name": "osvfs", "package": "vfs/osvfs",
             "probe": "tools/phase1/filesystem/osvfs_probe_test.go",
             "test": "TestPhase1FilesystemOsvfs"},
            {"name": "composed", "package": "execute",
             "probe": "tools/phase1/filesystem/composed_probe_test.go",
             "test": "TestPhase1FilesystemComposed"},
            # The carried config/matchFiles renderer. It compiles into the
            # pinned tsoptions_test package to reach that package's own
            # helpers, which links internal/testutil/baseline, whose init calls
            # repo.TestDataPath() -- and repo panics under -trimpath. So the
            # flag is dropped for this probe, as it is for the F0 pilot's
            # command-line probe, and the choice is recorded in provenance.
            # `helper` is a second overlay source compiled into `tsoptions`
            # itself, so the renderer can reach the pinned unexported
            # `getWildcardDirectories` for the raw-JSON entry point, whose
            # result carries no ConfigFile. Overlay-only; the pin is untouched.
            {"name": "matchfiles", "package": "tsoptions",
             "probe": "tools/phase1/filesystem/matchfiles_probe_test.go",
             "helper": "tools/phase1/filesystem/matchfiles_inpackage_test.go",
             "test": "TestPhase1FilesystemMatchFiles",
             "trimpath": False},
        ],
        "rust_package": "phase1_filesystem",
        "rust_target_kind": "bin",
        "rust_example": "phase1_filesystem",
        "rust_target": "tools/phase1/filesystem/src/main.rs",
    },
    "config": {
        "requests": [
            "data/phase1/requests/config-commandline.json",
            "data/phase1/requests/config-tsconfigparsing.json",
            "data/phase1/requests/config-host.json",
            "data/phase1/requests/config-commandlineops.json",
            "data/phase1/requests/config-configparse.json",
            "data/phase1/requests/config-module.json",
            "data/phase1/requests/config-packagejson.json",
            "data/phase1/requests/config-diagwriter.json",
        ],
        "renderer": {"package": "tsoptions", "test": "TestPhase1ConfigRender",
                     "probe": "tools/phase1/config/commandline_probe_test.go",
                     "helper": "tools/phase1/config/renderer_test.go", "trimpath": False,
                     "extra_sources": {"phase1_config_test.go": "tools/phase1/config/tsconfigparsing_probe_test.go"}},
        "native_probes": [
            # The 53 + 27 `tsoptions/commandLineParsing` outputs. It compiles
            # into the pinned `tsoptions_test` package so it can call that
            # package's own `formatNewBaseline` and `formatNewBaselineBuild`
            # rather than restating their assembly, and the pinned
            # `createVerifyNullForNonNullIncluded` for the eight outputs that
            # need a synthesised option declaration. That package links
            # internal/testutil/baseline, whose init calls repo.TestDataPath(),
            # and repo panics under -trimpath, so the flag is dropped here as
            # it is for the F0 pilot's command-line probe.
            {"name": "commandline", "package": "tsoptions",
             "probe": "tools/phase1/config/commandline_probe_test.go",
             "helper": "tools/phase1/config/renderer_test.go",
             "test": "TestPhase1ConfigCommandLine",
             "trimpath": False},
            # The 87 `config/tsconfigParsing` outputs. Same package and the
            # same -trimpath reason. This one carries the two section
            # assemblies, because the pinned primary renderer ends in
            # `baseline.Run` -- which writes into the pin and fails the test on
            # any difference -- and the secondary one is inline in a test body.
            {"name": "tsconfigparsing", "package": "tsoptions",
             "probe": "tools/phase1/config/tsconfigparsing_probe_test.go",
             "helper": "tools/phase1/config/renderer_test.go",
             "extra_sources": {"phase1_commandline_test.go": "tools/phase1/config/commandline_probe_test.go"},
             "test": "TestPhase1ConfigTsconfigParsing",
             "trimpath": False},
            # The pinned parse-config host factory. Its own package, so its own
            # overlay; it links neither `internal/repo` nor
            # `internal/testutil/baseline`, so it keeps -trimpath.
            {"name": "host", "package": "tsoptions/tsoptionstest",
             "probe": "tools/phase1/config/tsoptionstest_probe_test.go",
             "test": "TestPhase1ConfigHost"},
            {"name": "diagwriter", "package": "diagnosticwriter",
             "probe": "tools/phase1/config/diagwriter_probe_test.go",
             "test": "TestPhase1ConfigDiagnosticWriter"},
            # internal/packagejson. Its pinned test file imports internal/repo,
            # which panics under -trimpath, so the flag is dropped here too.
            {"name": "packagejson", "package": "packagejson",
             "probe": "tools/phase1/config/packagejson_probe_test.go",
             "test": "TestPhase1ConfigPackageJson",
             "trimpath": False},
            {"name": "module", "package": "module",
             "probe": "tools/phase1/config/module_probe_test.go",
             "test": "TestPhase1ConfigModule"},
            {"name": "configparse", "package": "tsoptions",
             "probe": "tools/phase1/config/configparse_probe_test.go",
             "helper": "tools/phase1/config/configparse_inpackage_test.go",
             "test": "TestPhase1ConfigParse",
             "trimpath": False},
            {"name": "commandlineops", "package": "tsoptions",
             "probe": "tools/phase1/config/commandlineops_probe_test.go",
             "helper": "tools/phase1/config/commandlineops_inpackage_test.go",
             "test": "TestPhase1ConfigCommandLineOps",
             "trimpath": False},
        ],
        "rust_package": "phase1_config",
        "rust_target_kind": "bin",
        "rust_example": "phase1_config",
        "rust_target": "tools/phase1/config/src/main.rs",
    },
    # F4a. The case layer of the syntax step; the corpus layer (the syntax
    # schedule over every compiler variant) lives in scripts/phase1_syntax.py.
    "syntax": {
        "requests": [
            "data/phase1/requests/syntax-diagnostics.json",
            "data/phase1/requests/syntax-astnav.json",
            "data/phase1/requests/syntax-evaluator.json",
            "data/phase1/requests/syntax-parse-outputs.json",
            "data/phase1/requests/syntax-debug.json",
            "data/phase1/requests/syntax-scanner-ast.json",
            "data/phase1/requests/syntax-project-references.json",
            "tools/phase1/syntax/ast-generated/requests.json",
        ],
        "native_probes": [
            {"name": "scanner_ast", "package": "scanner",
             "probe": "tools/phase1/syntax/scanner_ast_probe_test.go",
             "test": "TestPhase1SyntaxScannerAst", "trimpath": False},
            # Existing ast tests use repo paths during package initialization.
            {"name": "generated_ast", "package": "ast",
             "probe": "tools/phase1/syntax/ast-generated/probe_test.go",
             "test": "TestPhase1GeneratedAST", "trimpath": False,
             "extra_sources": {
                 "phase1_generated_predicates_test.go": "tools/phase1/syntax/ast-generated/predicates_test.go",
                 "phase1_generated_shapes_test.go": "tools/phase1/syntax/ast-generated/shapes_test.go",
                 "phase1_generated_shapes_runtime_test.go": "tools/phase1/syntax/ast-generated/shapes_runtime_test.go"}},
            # In-package so a probe may reach unexported program state. The
            # compiler package's tests link internal/repo, which panics under
            # -trimpath, so the flag is dropped as for the config probes.
            {"name": "diagnostics", "package": "compiler",
             "probe": "tools/phase1/syntax/diagnostics_probe_test.go",
             "test": "TestPhase1SyntaxDiagnostics",
             "trimpath": False},
            # astnav's own tests link internal/testutil/baseline and repo, so
            # -trimpath is dropped here too.
            {"name": "astnav", "package": "astnav",
             "probe": "tools/phase1/syntax/astnav_probe_test.go",
             "test": "TestPhase1SyntaxAstnav",
             "trimpath": False},
            # The evaluator package has no test file of its own; this is its
            # first, and it links nothing that needs a real source path.
            {"name": "evaluator", "package": "evaluator",
             "probe": "tools/phase1/syntax/evaluator_probe_test.go",
             "test": "TestPhase1SyntaxEvaluator"},
            # In-package in the parser, which owns the side fields. Its tests
            # link internal/repo, so -trimpath is dropped.
            {"name": "parse_outputs", "package": "parser",
             "probe": "tools/phase1/syntax/utilities_probe_test.go",
             "test": "TestPhase1SyntaxParseOutputs",
             "trimpath": False},
            {"name": "debug", "package": "debug",
             "probe": "tools/phase1/syntax/debug_probe_test.go",
             "test": "TestPhase1SyntaxDebug"},
            # In-package in the compiler, like the diagnostics probe: it
            # reaches the loader, the raw verifier writes and processing
            # diagnostics, and the package's tests link internal/repo.
            {"name": "project_references", "package": "compiler",
             "probe": "tools/phase1/syntax/project_references_probe_test.go",
             "test": "TestPhase1SyntaxProjectReferences",
             "trimpath": False},
        ],
        "rust_package": "phase1_syntax",
        "rust_target_kind": "bin",
        "rust_example": "phase1_syntax",
        "rust_target": "tools/phase1/syntax/src/main.rs",
        # Included by #[path]; it lives outside the harness package directory,
        # so the workspace closure would not see it.
        "rust_driver": "tools/s07/program/rust_observation.rs",
    },
}
# The six command families the plan names. Only `pilot` is wired at F0; the
# rest are registered so `inventory --check` can report them as unprepared
# rather than silently omitting them.
DECLARED_FAMILIES = ("leaves", "filesystem", "config", "syntax", "utilities", "integration")

_METADATA: dict | None = None


def request_files(spec: dict) -> list[str]:
    """A family's request files, in declared order.

    A family may be split into per-group fragments so each coverage group owns
    its own file. They are merged in declared order and duplicate case ids
    across fragments are refused.
    """
    declared = spec["requests"]
    return [declared] if isinstance(declared, str) else list(declared)


def load_requests(spec: dict) -> dict:
    version = 1
    merged: list[dict] = []
    seen: dict[str, str] = {}
    for relative in request_files(spec):
        document = strict_json_loads((ROOT / relative).read_bytes())
        version = document.get("version", version)
        for request in document["requests"]:
            case = request["case"]
            if document.get("family") == "filesystem" or spec.get("rust_package") == "phase1_filesystem":
                if "hosts" not in request:
                    raise ValueError(f"{case}: filesystem request must declare structured hosts")
                hosts.request_hosts(request)
            if case in seen:
                raise ValueError(
                    f"duplicate case id {case!r} in {relative} and {seen[case]}"
                )
            seen[case] = relative
            merged.append(request)
    return {"version": version, "requests": merged}


def operation_coverage_problems(requests: list[dict], cases: dict | None = None) -> list[str]:
    """Validate F4a's reviewed links against the exact requests they credit.

    A successful request cannot keep credit for a removed action or a changed
    source merely because its case id stayed the same. The per-action links
    remain reviewed evidence; this check does not infer coverage from a call.
    """
    if cases is None:
        cases = strict_json_loads((ROOT / "data/phase1/cases.json").read_bytes())
    declared = {case["id"]: case for case in cases["cases"] if case.get("family") == "syntax"}
    problems = []
    for request in requests:
        identity = request["case"]
        case = declared.get(identity)
        if case is None:
            problems.append(f"{identity}: no syntax case owns its operation coverage")
            continue
        if case.get("request_sha256") != digest(request_bytes(request)):
            problems.append(f"{identity}: request changed since its operation coverage was reviewed")
        actions = request.get("actions")
        if actions is None:
            action_names = {request["operation"]}
        elif (not isinstance(actions, list) or not actions
              or any(not isinstance(action, dict) or not isinstance(action.get("op"), str)
                     or not action["op"] for action in actions)):
            problems.append(f"{identity}: invalid actions in the operation coverage request")
            continue
        else:
            action_names = {action["op"] for action in actions}
        links = case.get("operation_actions")
        if not isinstance(links, dict) or set(links) != action_names:
            problems.append(f"{identity}: operation_actions must name exactly its requested actions")
            continue
        if any(not isinstance(operations, list)
               or any(not isinstance(operation, str) or not operation for operation in operations)
               or len(operations) != len(set(operations)) for operations in links.values()):
            problems.append(f"{identity}: invalid operation_actions coverage links")
            continue
        linked = {operation for operations in links.values() for operation in operations}
        if linked != set(case.get("operations", [])):
            problems.append(f"{identity}: operation_actions do not account for exactly its claimed operations")
    return problems


def validate_operation_coverage(family: str, requests: list[dict]) -> None:
    if family == "syntax":
        problems = operation_coverage_problems(requests)
        if problems:
            raise ValueError("invalid syntax operation coverage: " + "; ".join(problems[:5]))


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

    paths.update(Path(name) for name in request_files(spec))
    # The pilot's driver and Cargo target are different files (a thin example
    # includes the driver); a harness binary is its own target.
    if spec.get("rust_driver"):
        paths.add(Path(spec["rust_driver"]))
    paths.add(Path(spec["rust_target"]))
    for probe in spec["native_probes"]:
        paths.add(Path(probe["probe"]))

    # Everything under the family's adapter directory, recursively, so a file
    # in a nested directory is not missed by a shallow glob.
    adapter = ROOT / "tools/phase1" / family
    if adapter.is_dir():
        paths.update(p.resolve().relative_to(ROOT.resolve()) for p in package_input_files(adapter))

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
        paths.update(path.resolve().relative_to(ROOT.resolve()) for path in package_input_files(directory))

    closure: dict[str, str] = {}
    for relative in sorted({str(p) for p in paths}):
        path = ROOT / relative
        # Finder metadata and interpreter caches are not build inputs.
        # Including it makes an otherwise identical Linux capture unreplayable
        # on macOS and lets opening a source folder stale its observations.
        if path.is_file() and path.name != ".DS_Store" and "__pycache__" not in path.parts:
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
              trimpath: bool = True, helper: str | None = None,
              extra_sources: dict[str, str] | None = None) -> dict:
    """Run one access-only Go probe under an overlay and authenticate its output.

    This mirrors `s08_oracle.run_overlay`, which is reused wherever it fits. It
    exists because `run_overlay` always passes `-trimpath`, and a probe that
    compiles into the pinned `tsoptions_test` package cannot: that package links
    `internal/testutil/baseline`, whose `init` calls `repo.TestDataPath()`, and
    `repo` panics with "repo root cannot be found when built with -trimpath".
    Sharing that package is the whole point of the renderer seam, so the flag is
    dropped for those probes and the choice is recorded in provenance.

    `helper` is an optional second overlay source, compiled *into* the pinned
    package rather than its external test package. A probe that lives in
    `<package>_test` can only reach exported identifiers; a probe that needs a
    pinned unexported entry point declares an in-package companion here instead
    of editing the pin. Both files are overlay-only: neither is written into
    `upstream/`, and `verified_upstream()` re-checks the tree afterwards.
    """
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    source_path = directory / "export_test.go"
    source_path.write_text(source)
    request_path = directory / "requests.json"
    serialized_request = request_bytes(request)
    request_path.write_bytes(serialized_request)
    output = directory / "observations.json"
    virtual = upstream / "tsc/internal" / package / "phase1_probe_export_test.go"
    if virtual.exists():
        raise ValueError(f"overlay would replace a source file: {virtual}")
    replace = {str(virtual): str(source_path)}
    helper_path = None
    if helper is not None:
        helper_path = directory / "export_inpackage_test.go"
        helper_path.write_text(helper)
        helper_virtual = (upstream / "tsc/internal" / package
                          / "phase1_probe_inpackage_export_test.go")
        if helper_virtual.exists():
            raise ValueError(f"overlay would replace a source file: {helper_virtual}")
        replace[str(helper_virtual)] = str(helper_path)
    for name, text in (extra_sources or {}).items():
        if Path(name).name != name or not name.endswith("_test.go"):
            raise ValueError("invalid extra overlay source name")
        target = upstream / "tsc/internal" / package / name
        if target.exists() or str(target) in replace:
            raise ValueError(f"overlay would replace an existing source: {target}")
        local = directory / name
        local.write_text(text)
        replace[str(target)] = str(local)
    overlay = directory / "overlay.json"
    overlay.write_bytes(canonical({"Replace": replace}))
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
    if report["request_sha256"] != digest(serialized_request):
        raise ValueError(f"the {package} probe observed a different request inventory")
    (directory / "provenance.json").write_bytes(canonical({
        "pin": pin(), "package": package, "test": test, "trimpath": trimpath,
        "source_sha256": digest(source.encode()),
        "helper_sha256": digest(helper.encode()) if helper is not None else None,
        "extra_sources_sha256": {name:digest(source.encode()) for name, source in (extra_sources or {}).items()},
        "request_sha256": digest(serialized_request),
        "output_sha256": digest(output.read_bytes()),
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"],
        "toolchain_local": env["GOTOOLCHAIN"] == "local",
    }) + b"\n")
    return report


def order_safe_problems(observation: object) -> list[str]:
    """Check an order-sensitive observation uses an order-preserving shape."""
    if not isinstance(observation, dict) or "ordered" not in observation:
        return [
            "an order-sensitive observation must carry its ordered payload under `ordered`"
        ]
    payload = observation["ordered"]
    if not isinstance(payload, list):
        return [
            "`ordered` must be a list; a JSON object's member order is lost to canonicalisation"
        ]

    def nested(value: object, path: str) -> list[str]:
        if isinstance(value, dict):
            if len(value) > 1:
                return [
                    f"{path} is a {len(value)}-key object nested inside the ordered payload; "
                    "ordered data must be an entry array, because canonicalisation sorts keys"
                ]
            return [p for k, v in value.items() for p in nested(v, f"{path}.{k}")]
        if isinstance(value, list):
            return [p for i, v in enumerate(value) for p in nested(v, f"{path}[{i}]")]
        return []

    problems: list[str] = []
    for index, element in enumerate(payload):
        where = f"ordered[{index}]"
        if isinstance(element, dict):
            # The element's own named fields are fine; their values are not.
            for key, value in element.items():
                problems.extend(nested(value, f"{where}.{key}"))
        else:
            problems.extend(nested(element, where))
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
        if "metadata" in row and not isinstance(row["metadata"], dict):
            raise ValueError(f"{where}: metadata must be an object")
        result = row.get("result")
        if result not in statuses:
            raise ValueError(
                f"{where} reports unknown {side} status {result!r}; "
                f"allowed statuses are {', '.join(statuses)}"
            )
        if result == "observed" and "observation" not in row:
            raise ValueError(f"{where} is observed but carries no observation payload")
        if "actions" in request:
            actions = request["actions"]
            if not isinstance(actions, list) or not actions or any(
                not isinstance(action, dict) or not isinstance(action.get("op"), str)
                or not action["op"] for action in actions
            ):
                raise ValueError(f"{where}: actions must be a nonempty array of named operations")
            if result == "observed":
                observed = row["observation"]
                trace = observed.get("ordered") if isinstance(observed, dict) else None
                if not isinstance(trace, list) or len(trace) != len(actions):
                    raise ValueError(f"{where}: observation omitted or added an action result")
        if result == "observed" and isinstance(row["observation"], dict):
            for action in row["observation"].get("ordered", []):
                if (isinstance(action, dict) and "unsupported_action" in action) or (
                    isinstance(action, list) and action and action[0] == "unsupported_action"
                ):
                    raise ValueError(f"{where}: unsupported action is a harness failure: {action}")
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
            if not isinstance(missing, dict) or any(not isinstance(missing.get(k), str) or not missing[k] for k in required):
                raise ValueError(
                    f"{where} is not_implemented without a complete missing_operation record"
                )
        if result == "native_unavailable" and not row.get("reason"):
            raise ValueError(f"{where} is native_unavailable without a recorded reason")
        if result == "harness_failed" and not (row.get("error") or row.get("reason")):
            raise ValueError(f"{where} is harness_failed without a recorded cause")
    return rows


def validate_renderer(directory: Path, requests: dict, rendered: dict) -> None:
    """Child-free check that rendering retained every Rust result unchanged."""
    raw = strict_json_loads((directory / "rust-raw-observations.json").read_bytes())
    before = validate_response(raw, requests["requests"], "rust")
    bridge_input = strict_json_loads((directory / "renderer/requests.json").read_bytes())
    if bridge_input != {**requests, "observations": before}:
        raise ValueError("renderer input does not carry the captured Rust results")
    bridge_output = strict_json_loads((directory / "renderer/observations.json").read_bytes())
    if bridge_output != rendered:
        raise ValueError("Rust observations differ from the renderer output")
    validate_rendered_rows(requests, raw, rendered)


def validate_rendered_rows(requests: dict, raw: dict, rendered: dict) -> None:
    """Validate the shared envelope seam, also for integration receipts."""
    before = validate_response(raw, requests["requests"], "rust")
    after = validate_response(rendered, requests["requests"], "rust")
    for request, original, final in zip(requests["requests"], before, after):
        if request.get("subject") in ("commandLineBaseline", "tsconfigParsingBaseline") and original["result"] == "observed":
            observation = final.get("observation", {})
            expected_keys = {"baseline", "typed", "rendered", "rendered_sha256"}
            if set(observation) != expected_keys or observation["typed"] != original["observation"]:
                raise ValueError("renderer changed or omitted a typed Rust result")
            text = observation["rendered"]
            if not isinstance(text, str) or digest(text.encode()) != observation["rendered_sha256"]:
                raise ValueError("renderer byte digest does not match")
            if observation["baseline"] != request["baseline"]:
                raise ValueError("renderer substituted a baseline identity")
            if {k:v for k,v in original.items() if k != "observation"} != {k:v for k,v in final.items() if k != "observation"}:
                raise ValueError("renderer changed the Rust outcome")
        elif original != final:
            raise ValueError("renderer changed an unrelated Rust row")


def case_claims(requests: list[dict]) -> dict[str, str | None]:
    """Capture declarations, not the mutable results subsequently recorded there.

    Undeclared development requests can be observed, but cannot grant coverage.
    """
    from phase1_scope import case_claims_digest
    path = ROOT / "data/phase1/cases.json"
    declarations = {row["id"]: row for row in strict_json_loads(path.read_bytes())["cases"]} if path.is_file() else {}
    for request in requests:
        case = declarations.get(request["case"])
        if case is not None and case.get("family") == "filesystem":
            hosts.validate_case(request, case)
    return {row["case"]: case_claims_digest(declarations[row["case"]])
            if row["case"] in declarations else None for row in requests}


def capture(family: str, output: Path, cases: list[str] | None = None) -> dict:
    if family not in FAMILIES:
        raise ValueError(
            f"family {family!r} has no adapter yet; declared families are "
            + ", ".join(DECLARED_FAMILIES)
        )
    spec = FAMILIES[family]
    document = load_requests(spec)
    validate_operation_coverage(family, document["requests"])
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)

    before = source_closure(family)
    recorded_pin, recorded_gitlink = pin(), gitlink()
    if recorded_pin != recorded_gitlink:
        raise ValueError("data/upstream.json and the upstream gitlink disagree on the pin")

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
    serialized_request = request_bytes(request_document)
    captured_claims = case_claims(selected)
    request_path = output / "requests.json"
    request_path.write_bytes(serialized_request)

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
            (ROOT / probe["helper"]).read_text() if probe.get("helper") else None,
            {name: (ROOT / path).read_text() for name, path in probe.get("extra_sources", {}).items()},
        )
        validate_response(report, selected, "native")
        native_reports[name] = {
            "package": probe["package"],
            "helper": probe.get("helper"),
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
    raw_document = strict_json_loads(rust_path.read_bytes())
    validate_response(raw_document, selected, "rust")
    renderer_record = None
    if renderer := spec.get("renderer"):
        # Preserve exactly what the Rust executable emitted. Only the pinned
        # test envelope and serialization run in this second Go invocation.
        raw_path = output / "rust-raw-observations.json"
        raw_path.write_bytes(rust_path.read_bytes())
        rendered = run_probe(
            output / "renderer", renderer["package"],
            (ROOT / renderer["probe"]).read_text(),
            {**request_document, "observations": raw_document["observations"]},
            renderer["test"], renderer.get("trimpath", True),
            (ROOT / renderer["helper"]).read_text(),
            {name: (ROOT / path).read_text() for name, path in renderer.get("extra_sources", {}).items()},
        )
        validate_response(rendered, selected, "rust")
        rust_path.write_bytes(canonical(rendered) + b"\n")
        renderer_record = {name: sha_file(output / name) for name in (
            "rust-raw-observations.json", "renderer/requests.json",
            "renderer/observations.json", "renderer/provenance.json",
        )}
        validate_renderer(output, request_document, rendered)

    if case_claims(selected) != captured_claims:
        raise ValueError("case declarations changed while capturing; the capture is invalid")
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

    native_hosts = {(record.get("goos"), record.get("goarch")) for record in native_reports.values()}
    if len(native_hosts) != 1 or any(not goos or not goarch for goos, goarch in native_hosts):
        raise ValueError("native probes disagree on their capture host or omit GOOS/GOARCH")
    goos, goarch = next(iter(native_hosts))
    provenance = {
        "version": 3,
        "family": family,
        "pin": recorded_pin,
        "upstream_gitlink": recorded_gitlink,
        "partial": partial,
        "selected_cases": sorted(r["case"] for r in selected),
        "requests_sha256": digest(serialized_request),
        "case_claims": captured_claims,
        "native_probes": native_reports,
        "rust_observations_sha256": sha_file(rust_path),
        "rust_binary_sha256": sha_file(executable),
        "renderer_artifacts": renderer_record,
        "workspace_packages": workspace_package_paths(spec["rust_package"]),
        "source_closure": before,
        "source_closure_size": len(before),
        "host": {"platform": platform.platform(), "python": platform.python_version(),
                 "goos": goos, "goarch": goarch},
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

    checks = {
        "requests.json": provenance["requests_sha256"],
        "rust-observations.json": provenance["rust_observations_sha256"],
    }
    if renderer_artifacts := provenance.get("renderer_artifacts"):
        checks.update(renderer_artifacts)
    for probe in provenance["native_probes"].values():
        checks[f"{probe['directory']}/observations.json"] = probe["observations_sha256"]
    for name, expected in checks.items():
        if sha_file(directory / name) != expected:
            raise ValueError(f"capture artifact {name} does not match its recorded hash")

    # Host applicability is an evidence boundary, not a user-editable label.
    # Tie it to each authenticated raw native response, including probes that
    # declined every selected request. Legacy non-host families may omit it.
    host = provenance.get("host", {})
    if family == "filesystem" or "goos" in host:
        goos = host.get("goos")
        if family == "filesystem" and goos is None:
            raise StaleCapture("filesystem capture predates authenticated host applicability; recapture this host")
        if family == "filesystem":
            hosts.applies({"hosts": ["any"]}, goos)
        if not goos or not provenance["native_probes"]:
            raise ValueError("capture has no authenticated native GOOS")
        for name, probe in provenance["native_probes"].items():
            raw = strict_json_loads((directory / probe["directory"] / "observations.json").read_bytes())
            if raw.get("goos") != goos or probe.get("goos") != goos:
                raise ValueError(f"capture host GOOS disagrees with native probe {name}")
            if "goarch" in host and (raw.get("goarch") != host["goarch"] or probe.get("goarch") != host["goarch"]):
                raise ValueError(f"capture host GOARCH disagrees with native probe {name}")

    requests = strict_json_loads((directory / "requests.json").read_bytes())["requests"]
    if provenance.get("case_claims") != case_claims(requests):
        raise StaleCapture("reviewed case declarations changed or were not bound by the capture")

    if provenance.get("pin") != pin():
        raise StaleCapture("capture pin differs from the repository pin")
    if provenance.get("upstream_gitlink") != gitlink():
        raise StaleCapture("capture was taken against a different upstream gitlink")

    # Recompute the expected closure rather than trusting the recorded one: a
    # capture that recorded too few inputs must not authenticate just because
    # the few it recorded are unchanged.
    expected_closure = source_closure(family, provenance.get("workspace_packages"))
    recorded = provenance["source_closure"]
    if len(recorded) != provenance.get("source_closure_size"):
        raise ValueError("capture source closure size disagrees with its own contents")
    absent = sorted(set(expected_closure) - set(recorded))
    if absent:
        raise StaleCapture(
            f"capture omitted {len(absent)} source input(s) that can change its observations: "
            + ", ".join(absent[:5])
        )
    extra = sorted(set(recorded) - set(expected_closure))
    if extra:
        raise StaleCapture(
            f"capture recorded {len(extra)} input(s) that are no longer part of the closure: "
            + ", ".join(extra[:5])
        )
    for relative, expected in recorded.items():
        path = ROOT / relative
        if not path.is_file():
            raise StaleCapture(f"capture input {relative} no longer exists")
        if sha_file(path) != expected:
            raise StaleCapture(f"capture input {relative} changed after the capture")
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
                existing = merged.setdefault(case, dict(row, native_reasons={}))
                if existing["result"] != "observed":
                    existing["native_reasons"][probe_name] = row["reason"]
                    # Keep the owning probe's host limitation even when other
                    # probes declined the case earlier in the merge.
                    existing["reason"] = "; ".join(
                        f"{name}: {cause}" for name, cause in existing["native_reasons"].items()
                    )
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
    validate_operation_coverage(provenance["family"], requests)
    # _merge_native raises on any native harness failure, across every probe.
    native_rows = _merge_native(directory, provenance, requests)
    if FAMILIES[provenance["family"]].get("renderer"):
        if not provenance.get("renderer_artifacts"):
            raise ValueError("capture omitted the shared renderer provenance")
        validate_renderer(directory, strict_json_loads((directory / "requests.json").read_bytes()),
                          strict_json_loads((directory / "rust-observations.json").read_bytes()))
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

    inventory = load_requests(FAMILIES[provenance["family"]])
    all_cases = [r["case"] for r in inventory["requests"]]
    selected = {r["case"] for r in requests}
    unknown = selected - set(all_cases)
    if unknown:
        raise ValueError(
            "captured cases are absent from the frozen inventory: " + ", ".join(sorted(unknown))
        )

    current_requests = {request["case"]: request for request in inventory["requests"]}
    for request in requests:
        if request_bytes(request) != request_bytes(current_requests[request["case"]]):
            raise StaleCapture(f"{request['case']}: captured request differs from the current request")

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
        if native["result"] == "native_unavailable":
            applicable = (provenance["family"] != "filesystem"
                          or hosts.applies(current_requests[case], provenance["host"]["goos"]))
            rows.append({"case": case, "result": "native_unavailable" if applicable else "not_applicable",
                         "native_result": native["result"],
                         "reason": native.get("reason", ""), "rust_result": rust["result"],
                         **({"missing_operation": rust["missing_operation"]}
                            if rust["result"] == "not_implemented" else {})})
            continue
        if rust["result"] == "not_implemented":
            rows.append({"case": case, "result": "not_implemented",
                         "missing_operation": rust.get("missing_operation"),
                         "native_result": native.get("result"),
                         "native_observation": native.get("observation")})
            continue
        # Canonicalisation preserves array order, and order-sensitive cases are
        # required to put their ordered payload in an array, so this comparison
        # sees order differences without being confused by named-field order.
        # All families compare their complete observation. Optional row-level
        # metadata (e.g. native renderer provenance) is authenticated, not a
        # compiler result, and never changes this comparison rule.
        same = canonical(native.get("observation")) == canonical(rust.get("observation"))
        rows.append({
            "case": case,
            "result": "match" if same else "different",
            **({} if same else {"native": native.get("observation"), "rust": rust.get("observation")}),
        })

    captured_requests = {request["case"]: request for request in requests}
    for row in rows:
        row["hosts"] = hosts.request_hosts(current_requests[row["case"]])
        if row["case"] in captured_requests:
            row["request_sha256"] = digest(request_bytes(captured_requests[row["case"]]))
            row["claims_sha256"] = provenance["case_claims"][row["case"]]

    counts = {result: 0 for result in RESULTS}
    for row in rows:
        counts[row["result"]] += 1
    required_non_match = counts["different"] + counts["not_implemented"] + counts["native_unavailable"]
    # A host-applicable partial capture leaves excluded, unselected rows visibly
    # not_run. They still make --require-parity reject a partial schedule, but
    # they are not part of this host's denominator.
    excluded_unrun = sum(row["result"] == "not_run"
                        and not hosts.applies(current_requests[row["case"]], provenance["host"]["goos"])
                        for row in rows) if provenance["family"] == "filesystem" else 0
    denominator = len(rows) - counts["not_applicable"] - excluded_unrun
    report = {
        "version": 2,
        "family": provenance["family"],
        "pin": provenance["pin"],
        "partial": provenance["partial"],
        "capture": str(directory),
        "requests_sha256": provenance["requests_sha256"],
        "capture_sha256": sha_file(directory / "provenance.json"),
        "host": provenance.get("host", {}),
        "counts": counts,
        "applicable_cases": denominator,
        "parity": counts["match"] / denominator if denominator else 0.0,
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
    family_hosts: dict[str, str | None] = {}
    for report in reports:
        family = report["family"]
        goos = report.get("host", {}).get("goos")
        if family in family_hosts and family_hosts[family] != goos:
            raise ValueError(f"reports for family {family} have different hosts; retain separate host captures")
        family_hosts[family] = goos
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
