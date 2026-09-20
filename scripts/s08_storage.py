#!/usr/bin/env python3
"""P1 intrinsic/string constructor and allocation diagnostic, not an E5 producer."""

import argparse
import json
from pathlib import Path
import shutil

from s04 import same_json_value
from s04_common import command, strict_json_loads
from s08_oracle import ROOT, canonical, digest, run_overlay


def capture(directory):
    directory = Path(directory)
    request_path = ROOT / "data/s08/storage-pilot.json"
    requests = strict_json_loads(request_path.read_bytes())
    source_path = ROOT / "tools/s08/oracle/storage_pilot_test.go"
    sources = {str(p.relative_to(ROOT)): digest(p.read_bytes()) for pattern in (
        "crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.*", "rust-toolchain.toml",
        "scripts/s08_storage.py", "scripts/s08_oracle.py", "scripts/s04.py", "scripts/s04_common.py",
        "scripts/s04_runtime.py", "scripts/tracking-bootstrap.py", ".gitmodules",
        "tools/s08/oracle/storage_pilot_test.go", "data/s08/storage-pilot.json", "data/s04/toolchains.toml",
        "data/upstream.json", ".cargo/config.toml",
    ) for p in ROOT.glob(pattern) if p.is_file()}
    go = run_overlay(directory / "go", "checker", source_path.read_text(), requests, "TestS08StoragePilot")
    rust_version = command(["rustc", "--version", "--verbose"], cwd=ROOT).decode()
    build = command(["cargo", "build", "--release", "--locked", "-p", "tsr_checker", "--features", "storage-pilot",
                     "--example", "storage_pilot", "--message-format=json"], cwd=ROOT)
    (directory / "cargo-build.ndjson").write_bytes(build)
    artifacts = [row for line in build.splitlines() if (row := strict_json_loads(line)).get("reason") == "compiler-artifact"
                 and row["target"]["name"] == "storage_pilot" and row.get("executable")]
    if len(artifacts) != 1 or "storage-pilot" not in artifacts[0]["features"]:
        raise ValueError("missing or ambiguous storage pilot compiler artifact")
    binary = directory.resolve() / "rust-storage-pilot"
    shutil.copy2(artifacts[0]["executable"], binary)
    raw = canonical(requests) + b"\n"
    rust_bytes = command([str(binary)], cwd=ROOT, data=raw)
    (directory / "rust-observations.json").write_bytes(rust_bytes)
    rust = strict_json_loads(rust_bytes)
    if len(go["roots"]) != len(requests) or not same_json_value(go["roots"], rust["roots"]):
        raise ValueError("constructor observations differ; raw results retained")
    for key in ("type_records", "string_cache_entries"):
        if go["census"][key] != rust["census"][key]:
            raise ValueError(f"constructor count differs: {key}")
    if any(digest((ROOT / name).read_bytes()) != value for name, value in sources.items()):
        raise ValueError("pilot sources changed during capture")
    provenance = strict_json_loads((directory / "go/provenance.json").read_bytes())
    report = {"version":1, "parity":True, "actions":len(requests), "request_sha256":digest(raw),
              "pin":provenance["pin"],
              "rust_version":rust_version,
              "scope":"Intrinsic/string construction, interning and fresh/regular edges only; not E2/E5 or the full P1 census",
              "limitations":["Input text backing and empty checker/store setup precede the interval",
                             "Go retained endpoint is process-wide post-GC HeapAlloc; Rust is live requested allocation bytes",
                             "A negative Go retained delta is unusable; positive values still include runtime noise and are not a footprint gate",
                             "No timing conclusion; object/union/tuple/alias and full-checker storage remain unmeasured"],
              "rust_executable_sha256":digest(binary.read_bytes()), "source_inputs":sources,
              "go":{key:go[key] for key in ("census","allocator","go","goos","goarch")},
              "rust":{key:rust[key] for key in ("census","allocator")}}
    report["go_retained_endpoint_nonnegative"] = go["allocator"]["retained_delta"] >= 0
    (directory / "report.json").write_bytes(canonical(report)+b"\n")
    print(json.dumps({key:value for key,value in report.items() if key != "source_inputs"},sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output",type=Path,required=True)
    capture(parser.parse_args().output)
