#!/usr/bin/env python3
"""Provision and measure the independently pinned parse-and-bind workload.

Development captures are raw observations. They do not publish E5/E6 until the
workload graph parity and repeated-sample qualification have also succeeded.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

from s04 import verified_upstream, go_environment
from s04_common import command, strict_json_loads
from s07_inventory import workload
from s04_ownership import instrumentation_environment

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT.parent / ".ts-rust-workloads"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def source_fingerprint():
    """Bind captures to source bytes, including uncommitted implementation files."""
    sources = set()
    for crate in ("tsr_arena", "tsr_ast", "tsr_bench", "tsr_binder", "tsr_core", "tsr_diagnostics", "tsr_json", "tsr_locale", "tsr_jsnum", "tsr_jsstring", "tsr_parser", "tsr_scanner"):
        sources.update(p for p in (ROOT / "crates" / crate).rglob("*") if p.is_file() and p.suffix in {".rs", ".toml"})
    for directory in ("scripts/s07_oracle", "tools/s07/benchmark", ".cargo"):
        sources.update(p for p in (ROOT / directory).rglob("*") if p.is_file() and "__pycache__" not in p.parts)
    sources.update(ROOT.glob("scripts/s07_benchmark*.py"))
    sources.update(ROOT / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "data/upstream.json", "data/s04/toolchains.toml", "data/s07/vscode-files.json", "data/s07/vscode-parse-options.json", "data/s07/bindworkload-probes.json", "data/workloads.toml", "status/experiments.toml", "scripts/s07_binder.py", "scripts/s07_inventory.py", "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py", "scripts/s04_ownership.py", "scripts/s06_protocol.py", "scripts/s06_process.py"))
    sources = {path for path in sources if path.exists()}
    files = {str(path.relative_to(ROOT)): sha(path.read_bytes()) for path in sorted(sources)}
    return {"sha256": sha(json.dumps(files, sort_keys=True, separators=(",", ":")).encode()), "files": files}


def native_environment(base=None):
    env = instrumentation_environment(os.environ.copy() if base is None else base, ROOT)
    env["CARGO_TERM_COLOR"] = "never"
    # Keep the named Go GC policy; a caller-imposed memory limit would silently
    # change both CPU and RSS. Both drivers use the same provisioned input bytes.
    for key in ("GOMEMLIMIT", "GODEBUG", "GOMAXPROCS", "GOGC", "LD_PRELOAD", "DYLD_INSERT_LIBRARIES"):
        env.pop(key, None)
    for key in list(env):
        if key.startswith("CARGO_PROFILE_") or key == "CARGO_BUILD_TARGET" or key.lower().startswith("mimalloc_"):
            env.pop(key, None)
    env.update(GOTOOLCHAIN="local", GOGC="100")
    return env



def cargo_configuration_paths(env=None, root=None):
    env = native_environment() if env is None else env
    root = ROOT if root is None else Path(root)
    cargo_home = Path(env.get("CARGO_HOME", Path.home()/".cargo")).expanduser().resolve()
    directories = {cargo_home, *(path/".cargo" for path in (root, *root.parents))}
    return sorted({directory/name for directory in directories for name in ("config", "config.toml")})


def cargo_configuration(env=None):
    """Fingerprint caller Cargo configuration without recording private contents."""
    return {str(path): sha(path.read_bytes()) if path.is_file() else None for path in cargo_configuration_paths(env)}


def profile_configuration(env, profile, root=None):
    """Enforce native profiles without replacing caller registry settings.

    Cargo profile package tables override global profile values; override each
    declared package selector too. Compiler flags/wrappers cannot be neutralized
    by assigning empty arrays (Cargo merges arrays), so reject those overrides.
    """
    root = ROOT if root is None else Path(root)
    if profile not in {"release", "dev"}:
        raise ValueError("unknown benchmark Cargo profile")
    configurations = []
    for path in cargo_configuration_paths(env, root):
        if not path.is_file() or path.name == "config.toml" and path.with_name("config").is_file():
            continue
        data = tomllib.loads(path.read_text())
        build = data.get("build", {})
        if any(build.get(key) for key in ("rustc", "rustc-wrapper", "rustc-workspace-wrapper", "rustdoc", "rustflags", "rustdocflags")):
            raise ValueError("benchmark cannot establish its compiler/profile with Cargo compiler overrides in " + str(path))
        if any(target.get("rustflags") for target in data.get("target", {}).values() if isinstance(target, dict)):
            raise ValueError("benchmark cannot establish its release profile with Cargo target rustflags in " + str(path))
        for key in data.get("env", {}):
            if key.startswith(("RUST", "CARGO_PROFILE_", "CARGO_TARGET_", "CARGO_BUILD_")) or key in {"CARGO_ENCODED_RUSTFLAGS", "CARGO_ENCODED_RUSTDOCFLAGS"}:
                raise ValueError("benchmark cannot establish its compiler/profile with Cargo environment override " + key)
        configurations.append(data)
    configurations.append(tomllib.loads((root / "Cargo.toml").read_text()))
    packages = set()
    for configuration in configurations:
        packages.update(configuration.get("profile", {}).get(profile, {}).get("package", {}))
    ordinary = {"opt-level": 3, "debug": False, "debug-assertions": False,
                "overflow-checks": False, "codegen-units": 1, "incremental": False,
                "strip": "none", "split-debuginfo": "off"}
    if profile == "dev":
        ordinary.update({"opt-level": 0, "debug": 2, "debug-assertions": True,
                         "overflow-checks": True, "codegen-units": 256, "incremental": True})
    prefix = "profile." + profile + "."
    fields = {prefix+key: value for key,value in ordinary.items()}
    fields.update({prefix+"panic": "unwind", prefix+"lto": "fat" if profile == "release" else False, prefix+"rpath": False})
    for package in sorted(packages):
        for key,value in ordinary.items():
            fields[f"{prefix}package.{json.dumps(package)}.{key}"] = value
    # Proc macros/build scripts keep Cargo's release build-override defaults;
    # they are outside the native measured operation.
    for key,value in {**ordinary, "opt-level": 0, "codegen-units": 256, "debug": False}.items():
        fields[prefix+"build-override."+key] = value
    return [argument for key,value in fields.items() for argument in ("--config",key+"="+json.dumps(value))]


def release_configuration(env, root=None):
    return profile_configuration(env, "release", root)


def rust_native_toolchain(env, root=None, toolchain=None):
    root = ROOT if root is None else Path(root)
    stable = toolchain or tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    raw = command(["rustc", "+"+stable, "-vV"], cwd=root, env=env).decode()
    values = dict(line.split(": ", 1) for line in raw.splitlines() if ": " in line)
    if values.get("release") != stable or not values.get("host") or any(character.isspace() for character in values["host"]):
        raise ValueError("benchmark Rust compiler does not match the pinned stable/native host")
    return stable, values["host"]


def cargo_executable(messages, manifest, name, kind, features, release=True):
    """Select Cargo's actual successful bin artifact, never a guessed path."""
    observed = [strict_json_loads(line) for line in messages.splitlines() if line]
    if any(type(row) is not dict or type(row.get("reason")) is not str for row in observed):
        raise ValueError("Cargo emitted a non-object build message")
    for row in observed:
        if row["reason"] == "compiler-artifact" and (
                type(row.get("manifest_path")) is not str or type(row.get("target")) is not dict
                or type(row.get("filenames")) is not list or any(type(name) is not str for name in row["filenames"])
                or type(row.get("features")) is not list or any(type(feature) is not str for feature in row["features"])
                or type(row.get("profile")) is not dict):
            raise ValueError("Cargo emitted a malformed compiler artifact")
    finished = [row for row in observed if row.get("reason") == "build-finished"]
    if len(finished) != 1 or finished[0].get("success") is not True or observed[-1] != finished[0]:
        raise ValueError("Cargo did not finish a successful native benchmark build")
    selected = [row for row in observed if row.get("reason") == "compiler-artifact"
                and Path(row.get("manifest_path", "")).resolve() == manifest.resolve()
                and row.get("target", {}).get("name") == name
                and row["target"].get("kind") == [kind]]
    if len(selected) != 1:
        raise ValueError("Cargo did not report exactly one benchmark executable")
    artifact = selected[0]
    if (artifact.get("features") != features
            or artifact.get("profile") != {"opt_level":"3" if release else "0","debuginfo":0 if release else 2,"debug_assertions":not release,"overflow_checks":not release,"test":False}
            or type(artifact["profile"]["debuginfo"]) is not int
            or any(type(artifact["profile"][field]) is not bool for field in ("debug_assertions", "overflow_checks", "test"))
            or artifact.get("target",{}).get("crate_types") != ["bin"]):
        raise ValueError("Cargo benchmark artifact has the wrong features or release profile")
    executable = artifact.get("executable")
    if type(executable) is not str or not Path(executable).is_absolute() or executable not in artifact.get("filenames", []):
        raise ValueError("Cargo benchmark artifact lacks an exact executable path")
    return Path(executable)


def rust_executable(messages, manifest, instrumented):
    return cargo_executable(messages, manifest, "ts-bench", "bin", ["allocation"] if instrumented else [])


def build_rust(instrumented=False):
    if type(instrumented) is not bool:
        raise ValueError("benchmark instrumentation mode must be boolean")
    env = native_environment()
    stable, host = rust_native_toolchain(env)
    args = ["cargo", "+"+stable, "build", "--release", "--locked", "--package", "tsr_bench", "--bin", "ts-bench", "--target", host, "--message-format=json-render-diagnostics", *release_configuration(env)]
    if instrumented:
        args.extend(["--features", "allocation"])
    destination = CACHE / "s07-benchmark" / ("rust-benchmark-allocation" if instrumented else "rust-benchmark")
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Diagnostic checkouts can share package identities and a target directory.
    # Cargo may then report a fresh artifact whose dep-info still names that
    # other checkout. A source snapshot does not prove those bytes were built.
    # Use an empty target directory for each measured executable, overriding
    # both caller output/intermediate configuration; retain registry caches.
    (ROOT / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="s07-native-build-", dir=ROOT / "target") as temporary:
        build_directory = Path(temporary).resolve()
        args.extend(["--target-dir", str(build_directory),
                     "--config", "build.build-dir=" + json.dumps(str(build_directory))])
        executable = rust_executable(command(args, cwd=ROOT, env=env), ROOT / "crates/tsr_bench/Cargo.toml", instrumented)
        if not executable.resolve().is_relative_to(build_directory):
            raise ValueError("Cargo benchmark artifact escaped its isolated build directory")
        shutil.copyfile(executable, destination)
    destination.chmod(0o755)
    return destination, env


def build_allocation_probe(mode, toolchain):
    if mode not in {"debug", "release"}:
        raise ValueError("unknown allocator preflight profile")
    env = native_environment()
    selected, host = rust_native_toolchain(env, toolchain=toolchain)
    args = ["cargo", "+"+selected, "build", "--locked", "--package", "tsr_bench",
            "--example", "allocation_probe", "--features", "allocation", "--target", host,
            "--message-format=json-render-diagnostics",
            *profile_configuration(env, "release" if mode == "release" else "dev")]
    if mode == "release":
        args.append("--release")
    binary = cargo_executable(command(args, cwd=ROOT, env=env), ROOT / "crates/tsr_bench/Cargo.toml",
                              "allocation_probe", "example", ["allocation"], mode == "release")
    return binary, env


def go_native_environment(base=None):
    env = native_environment(go_environment() if base is None else base)
    # An empty shell value does not mask a persisted `go env -w` setting. Promote
    # registry/cache settings explicitly, then disable the compiler settings file.
    preserved = ("GOPROXY", "GONOPROXY", "GONOSUMDB", "GOPRIVATE", "GOSUMDB", "GOVCS", "GOAUTH",
                 "GOMODCACHE", "GOCACHE", "GOCACHEPROG", "GOPATH", "GOTMPDIR")
    values = strict_json_loads(command(["go", "env", "-json", *preserved], cwd=ROOT,
                                      env={**env, "GOEXPERIMENT": "none"}))
    if set(values) != set(preserved) or any(type(value) is not str for value in values.values()):
        raise ValueError("invalid Go registry/cache configuration")
    env.update(values)
    for key in ("GOOS", "GOARCH", "GOARM", "GOARM64", "GOAMD64", "GO386", "GOMIPS", "GOMIPS64", "GOPPC64", "GORISCV64", "GOEXPERIMENT", "GOFIPS140"):
        env.pop(key, None)
    env.update(GOENV="off", CGO_ENABLED="0", GOWORK="off", GOFLAGS="")
    host = strict_json_loads(command(["go", "env", "-json", "GOHOSTOS", "GOHOSTARCH"], cwd=ROOT, env=env))
    if set(host) != {"GOHOSTOS", "GOHOSTARCH"} or any(type(value) is not str or not value.isalnum() for value in host.values()):
        raise ValueError("invalid Go native host")
    env.update(GOOS=host["GOHOSTOS"], GOARCH=host["GOHOSTARCH"])
    return env


def build_go():
    upstream = verified_upstream()
    env = go_native_environment()
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    destination = CACHE / "s07-benchmark"
    destination.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="export-", dir=destination) as temporary:
        checkout = Path(temporary)
        archive = command(["git", "archive", pin, "tsc/go.mod", "tsc/go.sum", "tsc/internal"], cwd=upstream)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
            stream.extractall(checkout, filter="data")
        bridge = checkout / "tsc/internal/s07benchmark"
        bridge.mkdir()
        shutil.copyfile(ROOT / "tools/s07/benchmark/main.go", bridge / "main.go")
        shutil.copyfile(ROOT / "tools/s07/benchmark/graph.go", bridge / "graph.go")
        shutil.copyfile(ROOT / "scripts/s07_oracle/graph.go", bridge / "canonical_graph.go")
        for name in ("syntax_bridge.go", "access_bridge.go"):
            shutil.copyfile(ROOT / "scripts/s07_oracle" / name, checkout / "tsc/internal/ast" / ("s07_" + name))
        shutil.copyfile(ROOT / "scripts/s07_oracle/parser_bridge.go", checkout / "tsc/internal/parser/s07_parser_bridge.go")
        executable = destination / "go-benchmark"
        command(["go", "build", "-trimpath", "-mod=readonly", "-o", str(executable), "./internal/s07benchmark"], cwd=checkout / "tsc", env=env)
    return executable, env


def provision_inputs(executable, env):
    workload(False, CACHE, True)
    manifest = strict_json_loads((ROOT / "data/s07/vscode-files.json").read_bytes())
    prefix = f"vscode-{manifest['commit']}/"
    destination = CACHE / prefix.rstrip("/")
    destination.mkdir(parents=True, exist_ok=True)
    wanted = {row["path"]: row for row in manifest["files"]}
    seen = set()
    inputs = []
    with tarfile.open(CACHE / f"vscode-{manifest['commit']}.tar.gz", "r:gz") as archive:
        for entry in archive:
            if not entry.name.startswith(prefix) or entry.name[len(prefix):] not in wanted:
                continue
            relative = entry.name[len(prefix):]
            if not entry.isfile() or relative in seen or Path(relative).is_absolute() or ".." in Path(relative).parts:
                raise ValueError("invalid or duplicate workload path " + relative)
            seen.add(relative)
            raw = archive.extractfile(entry).read()
            expected = wanted[relative]
            if len(raw) != expected["bytes"] or sha(raw) != expected["sha256"]:
                raise ValueError("workload source changed: " + relative)
            local = destination / relative
            local.parent.mkdir(parents=True, exist_ok=True)
            if not local.exists() or local.read_bytes() != raw:
                local.write_bytes(raw)
    if seen != set(wanted):
        raise ValueError("workload archive has missing files")
    for row in manifest["files"]:
        inputs.append({"filename": "/vscode/" + row["path"], "path": "/vscode/" + row["path"],
                       "local": str(destination / row["path"]), "script_kind": 0, "jsx": False, "force": False})
    source = CACHE / "s07-benchmark/options-input.json"
    source.write_text(json.dumps(inputs))
    observed = strict_json_loads(command([str(executable), str(source), "options"], cwd=ROOT, env=env))
    if len(observed) != len(inputs):
        raise ValueError("Go parse-options preflight changed workload cardinality")
    for requested, actual in zip(inputs, observed, strict=True):
        for field in ("filename", "path", "local"):
            if requested[field] != actual[field]:
                raise ValueError("Go parse-options preflight changed identity")
    output = CACHE / "s07-benchmark/inputs.json"
    output.write_text(json.dumps(observed, separators=(",", ":")) + "\n")
    # Cache paths are host-local transport, not part of the frozen options.
    frozen = [{key: value for key, value in row.items() if key != "local"} for row in observed]
    document = {"version": 1, "compiler_options": {"target": "ESNext", "all_other_options": "pinned zero values"},
                "metadata": "empty SourceFileMetaData; no package resolution in source-tree workload",
                "authority": "ast.GetExternalModuleIndicatorOptions + core.GetScriptKindFromFileName",
                "parser_target_parameter": "absent at pin; parser entry supports filename/path/jsx/force only",
                "files": frozen}
    return output, document


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("preflight", "capture", "verify-capture"))
    parser.add_argument("--write-options", action="store_true")
    parser.add_argument("--graph-report", type=Path, default=ROOT / "target/s07-bindworkload/report.json")
    parser.add_argument("--output", type=Path, default=ROOT / "target/s07-benchmark")
    args = parser.parse_args()
    if args.operation != "preflight":
        if args.write_options:
            raise ValueError("measurement cannot rewrite its frozen options")
        if args.operation == "capture":
            from s07_benchmark_measure import capture
            report = capture(args.graph_report, args.output)
        else:
            from s07_benchmark_report import read_capture
            report, _ = read_capture(args.output, args.graph_report)
        print(json.dumps(report["metrics"], sort_keys=True))
        return
    binary, env = build_go()
    inputs, document = provision_inputs(binary, env)
    frozen = ROOT / "data/s07/vscode-parse-options.json"
    content = (json.dumps(document, indent=2, sort_keys=True) + "\n").encode()
    if args.write_options:
        frozen.write_bytes(content)
    elif not frozen.exists() or frozen.read_bytes() != content:
        raise ValueError("Go workload parse options differ; review then freeze with --write-options")
    print(json.dumps({"inputs": str(inputs), "go_binary": str(binary), "binary_sha256": sha(binary.read_bytes()), "files": len(document["files"]), "options_sha256": sha(content)}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, tarfile.TarError, subprocess.SubprocessError) as error:
        print(f"S07 benchmark failed: {error}", file=sys.stderr)
        sys.exit(1)
