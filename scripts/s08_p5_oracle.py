"""P5 multi-source observation overlays; the frozen P0 oracle stays unchanged."""
from pathlib import Path
from s08_oracle import ROOT, canonical, digest, verified_upstream, go_environment
from s04_common import command, strict_json_loads


def run_overlay(directory, package, source, request, test_name, *, extra_sources=None):
    """Keep the exact request/overlay/output; failed Go execution yields no result."""
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
    virtual = upstream / "tsc/internal" / package / "codex_s08_export_test.go"
    if virtual.exists():
        raise ValueError(f"overlay would replace a source file: {virtual}")
    overlay = directory / "overlay.json"
    replacements = {str(virtual): str(source_path)}
    for logical, contents in (extra_sources or {}).items():
        relative = Path(logical)
        if relative.is_absolute() or '..' in relative.parts:
            raise ValueError('overlay path escapes internal packages')
        native = upstream / 'tsc/internal' / relative
        if str(native) in replacements:
            raise ValueError('duplicate overlay source')
        retained = directory / 'sources' / relative
        retained.parent.mkdir(parents=True, exist_ok=True)
        retained.write_text(contents)
        replacements[str(native)] = str(retained)
    overlay.write_bytes(canonical({"Replace": replacements}))
    env.update(S08_REQUESTS=str(request_path), S08_OUTPUT=str(output))
    repo_flags = ([f"-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={directory}/unmatched-prefix"]
                  if package in ("testrunner", "testutil/tsbaseline") else [])
    stdout = command(["go", "test", "-trimpath", "-mod=readonly", *repo_flags, "-overlay", str(overlay),
                      f"./internal/{package}", "-run", f"^{test_name}$", "-count=1",
                      "-timeout=5m"], cwd=upstream / "tsc", env=env)
    (directory / "go-test.stdout").write_bytes(stdout)
    verified_upstream()
    report = strict_json_loads(output.read_bytes())
    if report["request_sha256"] != digest(request_bytes):
        raise ValueError("Go observed a different request inventory")
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    provenance = {
        "pin": pin, "source_sha256": digest(source.encode()),
        "request_sha256": digest(request_bytes), "output_sha256": digest(output.read_bytes()),
        "go": report["go"], "goos": report["goos"], "goarch": report["goarch"],
        "toolchain_local": env["GOTOOLCHAIN"] == "local",
    }
    if extra_sources is not None:
        provenance['extra_sources_sha256'] = {name: digest(text.encode()) for name, text in extra_sources.items()}
    (directory / "provenance.json").write_bytes(canonical(provenance) + b"\n")
    return report
