#!/usr/bin/env python3
"""Build the macOS diagnostic adapters; do not run profiles or certify E5/E6."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[3]
TOOLS = Path(__file__).resolve().parent
RUST = TOOLS / "rust"
sys.path.insert(0, str(ROOT / "scripts"))
from s04 import verified_upstream
from s04_common import command, strict_json_loads
from s07_benchmark import (
    cargo_configuration_paths, go_native_environment, native_environment,
    release_configuration, rust_native_toolchain, source_fingerprint,
)


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def tool_inputs():
    return {str(path.relative_to(ROOT)): sha(path) for path in sorted(TOOLS.rglob("*"))
            if path.is_file() and path.suffix in {".py", ".rs", ".go", ".toml", ".lock"}}


def dependency_inputs():
    names = ("s07_benchmark.py", "s07_inventory.py", "s04.py", "s04_common.py",
             "s04_ownership.py", "tracking-bootstrap.py")
    return {"scripts/" + name: sha(ROOT / "scripts" / name) for name in names}


def cargo_config(env):
    return {str(path): sha(path) if path.is_file() else None
            for path in cargo_configuration_paths(env, RUST)}


def registry_lock():
    """Every standalone registry version/checksum must come from the root lock."""
    def entries(path):
        rows = tomllib.loads(path.read_text())["package"]
        result = {}
        for row in rows:
            source = row.get("source")
            if source is None:
                continue
            if not source.startswith("registry+") or not isinstance(row.get("checksum"), str):
                raise ValueError("unsupported non-registry or unchecked dependency in " + str(path))
            key = (row["name"], row["version"], source)
            if key in result:
                raise ValueError("duplicate locked registry dependency")
            result[key] = row["checksum"]
        return result

    workspace = entries(ROOT / "Cargo.lock")
    standalone = entries(RUST / "Cargo.lock")
    for key, checksum in standalone.items():
        if workspace.get(key) != checksum:
            raise ValueError("diagnostic dependency differs from workspace lock: " + repr(key))
    return {"workspace_sha256": sha(ROOT / "Cargo.lock"),
            "standalone_sha256": sha(RUST / "Cargo.lock"),
            "registry_packages": len(standalone)}


def symbol_configuration(env):
    # Reuse the benchmark's compiler-override checks and package-selector
    # enforcement, taking the symbol settings from this standalone manifest.
    profile = tomllib.loads((RUST / "Cargo.toml").read_text())["profile"]["release"]
    expected = {"opt-level": 3, "debug": 2, "debug-assertions": False,
                "overflow-checks": False, "lto": "fat", "codegen-units": 1,
                "panic": "unwind", "strip": "none", "split-debuginfo": "packed"}
    if profile != expected:
        raise ValueError("diagnostic release manifest profile changed")
    baseline = release_configuration(env, RUST)
    result = []
    for index in range(0, len(baseline), 2):
        key, value = baseline[index + 1].split("=", 1)
        field = key.rsplit(".", 1)[-1]
        if ".build-override." not in key and field in profile:
            value = json.dumps(profile[field])
        result.extend(("--config", key + "=" + value))
    return profile, result


def rust_artifact(messages):
    rows = [strict_json_loads(line) for line in messages.splitlines() if line]
    if not rows or any(type(row) is not dict or type(row.get("reason")) is not str for row in rows):
        raise ValueError("malformed Cargo build messages")
    finished = [row for row in rows if row["reason"] == "build-finished"]
    if len(finished) != 1 or finished[0].get("success") is not True or rows[-1] != finished[0]:
        raise ValueError("Cargo did not complete the diagnostic build")
    candidates = [row for row in rows if row["reason"] == "compiler-artifact"
                  and Path(row.get("manifest_path", "")).resolve() == RUST / "Cargo.toml"
                  and row.get("target", {}).get("name") == "tsr_cpu_profile"
                  and row["target"].get("kind") == ["bin"]]
    if len(candidates) != 1:
        raise ValueError("Cargo must report exactly one diagnostic executable")
    artifact = candidates[0]
    expected = {"opt_level": "3", "debuginfo": 2, "debug_assertions": False,
                "overflow_checks": False, "test": False}
    if (artifact.get("features") != [] or artifact.get("profile") != expected
            or type(artifact["profile"]["debuginfo"]) is not int
            or any(type(artifact["profile"][key]) is not bool
                   for key in ("debug_assertions", "overflow_checks", "test"))
            or artifact["target"].get("crate_types") != ["bin"]):
        raise ValueError("Cargo diagnostic artifact has the wrong features/profile")
    executable = artifact.get("executable")
    if (type(executable) is not str or not Path(executable).is_absolute()
            or executable not in artifact.get("filenames", []) or not Path(executable).is_file()):
        raise ValueError("Cargo did not identify an existing absolute executable")
    return artifact


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/s07-cpu-profiles")
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise ValueError("the diagnostic profile build supports local macOS only")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = native_environment()
    # Keep generated build files outside the source/tool fingerprint, and reuse
    # the diagnostic symbol build without changing the caller's Cargo home.
    env["CARGO_TARGET_DIR"] = str(ROOT / "target/s07-cpu-build")
    env["DEVELOPER_DIR"] = "/Applications/Xcode.app/Contents/Developer"
    stable, host = rust_native_toolchain(env)
    if host not in {"aarch64-apple-darwin", "x86_64-apple-darwin"}:
        raise ValueError("unsupported native macOS Rust target")
    go_env = go_native_environment()
    if (go_env["GOOS"] != "darwin"
            or go_env["GOARCH"] != {"aarch64-apple-darwin": "arm64", "x86_64-apple-darwin": "amd64"}[host]):
        raise ValueError("Rust and Go native hosts disagree")
    upstream = verified_upstream()
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    before = {"source_fingerprint": source_fingerprint(), "tool_inputs": tool_inputs(),
              "dependency_inputs": dependency_inputs(), "cargo_configuration": cargo_config(env),
              "registry_lock": registry_lock()}
    profile, overrides = symbol_configuration(env)
    build_rust = ["cargo", "+" + stable, "build", "--release", "--locked",
                  "--manifest-path", str(RUST / "Cargo.toml"), "--bin", "tsr_cpu_profile",
                  "--target", host, "--message-format=json-render-diagnostics", *overrides]
    messages = command(build_rust, cwd=RUST, env=env)
    (output / "rust-cargo-messages.ndjson").write_bytes(messages)
    artifact = rust_artifact(messages)
    rust = Path(artifact["executable"])
    symbols = Path(str(rust) + ".dSYM")
    if not symbols.is_dir() or not any(symbols.rglob("DWARF/*")):
        raise ValueError("packed macOS debug symbols are missing beside the executable")
    symbol_hashes = {str(path.relative_to(symbols)): sha(path)
                     for path in sorted(symbols.rglob("*")) if path.is_file()}

    go = output / "go-cpu-profile"
    build_go = ["go", "build", "-trimpath", "-mod=readonly", "-buildvcs=false",
                "-o", str(go), "./internal/s07cpuprofile"]
    archive = command(["git", "archive", pin, "tsc/go.mod", "tsc/go.sum", "tsc/internal"], cwd=upstream)
    with tempfile.TemporaryDirectory(prefix="s07-cpu-profile-") as temporary:
        checkout = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(archive)) as stream:
            stream.extractall(checkout, filter="data")
        bridge = checkout / "tsc/internal/s07cpuprofile"
        bridge.mkdir()
        shutil.copyfile(TOOLS / "go/main.go", bridge / "main.go")
        command(build_go, cwd=checkout / "tsc", env=go_env)

    verified_upstream()
    after = {"source_fingerprint": source_fingerprint(), "tool_inputs": tool_inputs(),
             "dependency_inputs": dependency_inputs(), "cargo_configuration": cargo_config(env),
             "registry_lock": registry_lock()}
    if before != after:
        raise ValueError("build inputs changed; no completed build manifest is published")
    write_json(output / "rust-artifact.json", artifact)
    metadata = {"schema": 1, "diagnostic_only": True, **before, "upstream_pin": pin,
                "revision": command(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip(),
                "rust_artifact": artifact,
                "rust_artifact_file": {"path": str(output / "rust-artifact.json"),
                                       "sha256": sha(output / "rust-artifact.json")},
                "binaries": {"rust": {"path": str(rust), "sha256": sha(rust)},
                             "go": {"path": str(go), "sha256": sha(go)}},
                "symbols": {"path": str(symbols), "files": symbol_hashes},
                "rust": {"toolchain": stable, "host": host, "profile": profile,
                         "version": command(["rustc", "+" + stable, "-vV"], cwd=ROOT, env=env).decode(),
                         "command": build_rust, "cwd": str(RUST),
                         "target_dir": env["CARGO_TARGET_DIR"], "developer_dir": env["DEVELOPER_DIR"]},
                "go": {"version": command(["go", "version"], cwd=ROOT, env=go_env).decode().strip(),
                       "host_os": go_env["GOOS"], "host_arch": go_env["GOARCH"],
                       "command": build_go, "cwd": "temporary pinned export/tsc",
                       "export_sha256": hashlib.sha256(archive).hexdigest()},
                "configuration_policy": "caller Cargo/Rustup homes and registry/cache settings preserved; benchmark native compiler and GC environment checks reused"}
    write_json(output / "build.json", metadata)
    print(json.dumps({"diagnostic_only": True, "build_manifest": str(output / "build.json"),
                      "build_manifest_sha256": sha(output / "build.json"),
                      "binaries": metadata["binaries"]}, sort_keys=True))


if __name__ == "__main__":
    main()
