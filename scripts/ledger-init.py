#!/usr/bin/env python3
"""Generate the ledger, function inventory and provenance manifest together.

Usage: scripts/ledger-init.py [path-to-TypeScript-checkout] [--pin COMMIT]

Defaults to this repository's upstream/ checkout. Pass ../TypeScript explicitly
when bootstrapping before that checkout exists. The checkout must be clean and
--pin, when provided, must resolve to its HEAD. Inputs are read from Git blobs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

REPO_ROOT = Path(__file__).resolve().parent.parent
GENERATED_FILE_FIELDS = ("go", "package", "crate", "phase", "kind", "pin", "source_hash", "loc")

# Longest-prefix map from Go package (relative to tsc/) to (Rust crate, parity phase).
CRATES = {
    "cmd/tsc": ("tsc", 4),
    "internal/api/encoder": ("tsr_encoder", 0),
    "internal/api": ("tsr_api", 6),
    "internal/ast": ("tsr_ast", 0),
    "internal/astnav": ("tsr_astnav", 1),
    "internal/binder": ("tsr_binder", 1),
    "internal/bundled": ("tsr_bundled", 1),
    "internal/checker": ("tsr_checker", 2),
    "internal/collections": ("tsr_collections", 1),
    "internal/compiler": ("tsr_compiler", 4),
    "internal/contentmapper": ("tsr_contentmapper", 5),
    "internal/core": ("tsr_core", 1),
    "internal/debug": ("tsr_core", 1),
    "internal/diagnostics": ("tsr_diagnostics", 1),
    "internal/diagnosticwriter": ("tsr_diagnosticwriter", 4),
    "internal/evaluator": ("tsr_evaluator", 1),
    "internal/execute/build": ("tsr_build", 4),
    "internal/execute/incremental": ("tsr_incremental", 4),
    "internal/execute/tsc": ("tsr_execute", 4),
    "internal/execute/tsctests": ("tsr_tsctests", 4),
    "internal/execute/watchmanager": ("tsr_execute", 4),
    "internal/execute": ("tsr_execute", 4),
    "internal/format": ("tsr_format", 5),
    "internal/fourslash": ("tsr_fourslash", 5),
    "internal/fswatch": ("tsr_fswatch", 4),
    "internal/glob": ("tsr_glob", 1),
    "internal/ipc": ("tsr_ipc", 6),
    "internal/jsnum": ("tsr_jsnum", 1),
    "internal/json": ("tsr_json", 1),
    "internal/jsonrpc": ("tsr_jsonrpc", 6),
    "internal/locale": ("tsr_locale", 1),
    "internal/ls/autoimport": ("tsr_autoimport", 5),
    "internal/ls": ("tsr_ls", 5),
    "internal/lsp/lsproto": ("tsr_lsproto", 5),
    "internal/lsp": ("tsr_lsp", 5),
    "internal/module": ("tsr_module", 1),
    "internal/modulespecifiers": ("tsr_modulespecifiers", 2),
    "internal/nativepath": ("tsr_vfs", 4),
    "internal/nodebuilder": ("tsr_nodebuilder", 2),
    "internal/osutil": ("tsr_vfs", 1),
    "internal/outputpaths": ("tsr_outputpaths", 3),
    "internal/packagejson": ("tsr_packagejson", 1),
    "internal/parser": ("tsr_parser", 0),
    "internal/pprof": ("tsr_pprof", 4),
    "internal/printer": ("tsr_printer", 3),
    "internal/project": ("tsr_project", 5),
    "internal/pseudochecker": ("tsr_pseudochecker", 3),
    "internal/repo": ("tsr_testutil", 1),
    "internal/scanner": ("tsr_scanner", 0),
    "internal/semver": ("tsr_semver", 1),
    "internal/sourcemap": ("tsr_sourcemap", 3),
    "internal/spanmap": ("tsr_spanmap", 5),
    "internal/stringutil": ("tsr_stringutil", 1),
    "internal/symlinks": ("tsr_module", 1),
    "internal/testrunner": ("tsr_testrunner", 1),
    "internal/testutil": ("tsr_testutil", 1),
    "internal/tracing": ("tsr_tracing", 4),
    "internal/transformers/declarations": ("tsr_declarations", 3),
    "internal/transformers": ("tsr_transformers", 3),
    "internal/transpile": ("tsr_transpile", 3),
    "internal/tsoptions": ("tsr_tsoptions", 1),
    "internal/tspath": ("tsr_tspath", 1),
    "internal/vfs": ("tsr_vfs", 1),
}
HARNESS_PREFIXES = ("internal/testutil", "internal/testrunner", "internal/fourslash", "internal/execute/tsctests",
                    "internal/repo", "internal/vfs/vfstest", "internal/vfs/vfsmock", "internal/tsoptions/tsoptionstest")
TARGETS = [("darwin", "arm64"), ("darwin", "amd64"), ("linux", "amd64"), ("linux", "arm64")]
KNOWN_OS = {"aix", "android", "darwin", "dragonfly", "freebsd", "illumos", "ios", "js", "linux", "netbsd", "openbsd", "plan9", "solaris", "wasip1", "windows"}
KNOWN_ARCH = {"386", "amd64", "arm", "arm64", "loong64", "mips", "mips64", "mips64le", "mipsle", "ppc64", "ppc64le", "riscv64", "s390x", "wasm"}


def crate_for(pkg):
    best = None
    for prefix, val in CRATES.items():
        if pkg == prefix or pkg.startswith(prefix + "/"):
            if best is None or len(prefix) > len(best[0]):
                best = (prefix, val)
    return best[1] if best else ("unmapped", 9)


def eval_constraint(expr, goos, goarch):
    """Evaluate a //go:build expression for one target. Unknown tags are false."""
    tokens = re.findall(r"\(|\)|&&|\|\||!|[A-Za-z0-9_.]+", expr)
    pos = 0

    def tag(t):
        if t == goos or t == goarch:
            return True
        if t == "unix":
            return goos in {"darwin", "linux", "freebsd", "netbsd", "openbsd", "dragonfly", "aix", "solaris", "illumos", "ios", "android"}
        if t.startswith("go1."):
            return True
        return False

    def parse_or():
        nonlocal pos
        v = parse_and()
        while pos < len(tokens) and tokens[pos] == "||":
            pos += 1
            v = parse_and() or v
        return v

    def parse_and():
        nonlocal pos
        v = parse_not()
        while pos < len(tokens) and tokens[pos] == "&&":
            pos += 1
            v = parse_not() and v
        return v

    def parse_not():
        nonlocal pos
        if tokens[pos] == "!":
            pos += 1
            return not parse_not()
        if tokens[pos] == "(":
            pos += 1
            v = parse_or()
            pos += 1  # ')'
            return v
        t = tokens[pos]
        pos += 1
        return tag(t)

    return parse_or()


def in_scope(path, head):
    base = os.path.basename(path)[:-3]
    parts = base.split("_")
    suffix_os = suffix_arch = None
    if len(parts) >= 3 and parts[-2] in KNOWN_OS and parts[-1] in KNOWN_ARCH:
        suffix_os, suffix_arch = parts[-2], parts[-1]
    elif len(parts) >= 2 and parts[-1] in KNOWN_OS:
        suffix_os = parts[-1]
    elif len(parts) >= 2 and parts[-1] in KNOWN_ARCH:
        suffix_arch = parts[-1]
    m = re.search(r"^//go:build (.+)$", head, re.M)
    for goos, goarch in TARGETS:
        if suffix_os and suffix_os != goos:
            continue
        if suffix_arch and suffix_arch != goarch:
            continue
        if m and not eval_constraint(m.group(1).strip(), goos, goarch):
            continue
        return True
    return False


def git(upstream, *args, input=None):
    result = subprocess.run(["git", "-C", str(upstream), *args], input=input, capture_output=True)
    if result.returncode:
        raise ValueError(result.stderr.decode(errors="replace").strip())
    return result.stdout


def resolve_pin(upstream, pin):
    return git(upstream, "rev-parse", "--verify", "--end-of-options", pin + "^{commit}").decode().strip()


def verify_checkout(upstream, requested_pin=None):
    if not (upstream / "tsc").is_dir():
        raise ValueError(f"missing upstream checkout at {upstream}; pass ../TypeScript explicitly for bootstrap")
    top = Path(git(upstream, "rev-parse", "--show-toplevel").decode().strip()).resolve()
    if top != upstream.resolve():
        raise ValueError(f"upstream must be the checkout root, not {upstream}")
    head = resolve_pin(upstream, "HEAD")
    if requested_pin and resolve_pin(upstream, requested_pin) != head:
        raise ValueError(f"requested pin {requested_pin} does not match upstream HEAD {head}")
    if git(upstream, "status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("upstream checkout is dirty; commit, remove or restore its changes before generating")
    return head


def source_paths(upstream, pin):
    paths = git(upstream, "ls-tree", "-rz", "--name-only", pin, "--", "tsc/internal", "tsc/cmd")
    result = []
    for raw in paths.split(b"\0"):
        if not raw:
            continue
        path = raw.decode("utf-8")
        if any(c in path for c in "\r\n\t"):
            raise ValueError(f"unsupported control character in source path: {path!r}")
        parts = path.split("/")
        if (any(p == "testdata" or p.startswith(("_", ".")) for p in parts[:-1])
                or path.startswith("tsc/internal/fourslash/tests/")
                or not path.endswith(".go") or path.endswith("_test.go")):
            continue
        result.append(path)
    return sorted(result)


def read_blobs(upstream, pin, paths):
    requests = "".join(f"{pin}:{p}\n" for p in paths).encode()
    data = git(upstream, "cat-file", "--batch", input=requests)
    blobs = {}
    offset = 0
    for path in paths:
        end = data.index(b"\n", offset)
        header = data[offset:end].split()
        if len(header) != 3 or header[1] != b"blob":
            raise ValueError(f"cannot read pinned source {pin}:{path}")
        size = int(header[2])
        offset = end + 1
        blobs[path] = data[offset:offset + size]
        offset += size + 1
    return blobs


def string_list(value, field, path, migrate_scalar=False):
    if migrate_scalar and isinstance(value, str):
        value = [value] if value else []
    if not isinstance(value, list) or any(not isinstance(x, str) for x in value):
        raise ValueError(f"{path}: {field} must be a list of strings")
    return value


def ledger_generated_projection(ledger):
    """Validate and select the immutable provenance fields of a parsed ledger.

    Editable status/rust/verify and TOML formatting are deliberately excluded.
    Keep this projection and its canonical JSON encoding aligned with xtask.
    """
    if not isinstance(ledger, dict):
        raise ValueError("ledger must be a table")
    pin = ledger.get("pin")
    if not isinstance(pin, str) or re.fullmatch(r"[0-9a-f]{40}", pin) is None:
        raise ValueError("ledger pin must be a full lowercase commit SHA")
    files = ledger.get("file")
    if not isinstance(files, list):
        raise ValueError("ledger file must be an array of tables")
    projected = []
    seen = set()
    for index, entry in enumerate(files):
        if not isinstance(entry, dict):
            raise ValueError(f"ledger file[{index}] must be a table")
        result = {}
        for field in GENERATED_FILE_FIELDS:
            if field not in entry:
                raise ValueError(f"ledger file[{index}] is missing generated field {field}")
            value = entry[field]
            if field in {"phase", "loc"}:
                if type(value) is not int or not 0 <= value <= 2**63 - 1:
                    raise ValueError(f"ledger file[{index}] {field} must be a nonnegative 64-bit integer")
            elif not isinstance(value, str) or not value:
                raise ValueError(f"ledger file[{index}] {field} must be a nonempty string")
            elif field == "pin" and re.fullmatch(r"[0-9a-f]{40}", value) is None:
                raise ValueError(f"ledger file[{index}] pin must be a full lowercase commit SHA")
            elif field == "source_hash" and re.fullmatch(r"[0-9a-f]{64}", value) is None:
                raise ValueError(f"ledger file[{index}] source_hash must be a lowercase SHA-256 digest")
            elif field == "kind" and value not in {"source", "generated", "harness", "out-of-scope"}:
                raise ValueError(f"ledger file[{index}] has invalid kind {value!r}")
            result[field] = value
        if result["go"] in seen:
            raise ValueError(f"duplicate ledger source path {result['go']}")
        seen.add(result["go"])
        projected.append(result)
    return {"pin": pin, "file": sorted(projected, key=lambda entry: entry["go"])}


def ledger_generated_sha256(ledger):
    projection = ledger_generated_projection(ledger)
    canonical = json.dumps(projection, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def build_ledger(upstream, pin, previous, blobs):
    old_pin = resolve_pin(upstream, previous["pin"]) if previous.get("pin") else None
    resolved_pins = {previous["pin"]: old_pin, old_pin: old_pin} if old_pin else {}

    def full_pin(recorded):
        if recorded not in resolved_pins:
            resolved_pins[recorded] = resolve_pin(upstream, recorded)
        return resolved_pins[recorded]

    existing = {entry["go"]: entry for entry in previous.get("file", [])}
    if len(existing) != len(previous.get("file", [])):
        raise ValueError("existing ledger contains duplicate source paths")
    # Old scaffolds had no content hash. Obtain it from their recorded commit
    # before comparing, rather than treating unproven content as unchanged.
    legacy = {}
    for path, entry in existing.items():
        if not entry.get("source_hash"):
            recorded_pin = full_pin(entry.get("pin") or old_pin or pin)
            legacy.setdefault(recorded_pin, []).append(path)
    old_hashes = {}
    for recorded_pin, paths in legacy.items():
        old_hashes.update({p: hashlib.sha256(b).hexdigest() for p, b in read_blobs(upstream, recorded_pin, paths).items()})
    entries = []
    for path, content in blobs.items():
        text = content.decode("utf-8")
        head = "\n".join(text.splitlines()[:15])
        pkg = path.removeprefix("tsc/").rsplit("/", 1)[0]
        krate, phase = crate_for(pkg)
        if any(pkg == p or pkg.startswith(p + "/") for p in HARNESS_PREFIXES):
            kind = "harness"
        elif path.endswith("_generated.go") or "Code generated" in head:
            kind = "generated"
        else:
            kind = "source"
        if not in_scope(path, head):
            kind = "out-of-scope"
        prev = existing.get(path, {})
        status = prev.get("status", "out-of-scope" if kind == "out-of-scope" else "planned")
        if status == "verified":
            status = "ported"  # Verification is derived from evidence, never a hand-written status.
        if status not in {"planned", "in-progress", "ported", "out-of-scope"}:
            raise ValueError(f"{path}: unsupported status {status!r}")
        source_hash = hashlib.sha256(content).hexdigest()
        synchronized_pin = pin
        if prev:
            recorded = prev.get("pin") or old_pin or pin
            synchronized_pin = full_pin(recorded)
            previous_hash = prev.get("source_hash") or old_hashes.get(path)
            if previous_hash == source_hash and synchronized_pin == old_pin:
                synchronized_pin = pin
        entries.append({
            "go": path, "package": pkg, "crate": krate, "phase": phase,
            "kind": kind, "status": status,
            "rust": string_list(prev.get("rust", []), "rust", path, migrate_scalar=True),
            "verify": string_list(prev.get("verify", []), "verify", path),
            "pin": synchronized_pin, "source_hash": source_hash, "loc": text.count("\n"),
        })
    lines = [
        "# Port ledger: non-test Go files from the pinned upstream tsc module.",
        "# Regenerate with scripts/ledger-init.py; status, rust and verify are preserved.",
        "# status: planned | in-progress | ported | out-of-scope; verification is computed.",
        "# pin records last synchronization; source_hash hashes current upstream bytes.",
        "# kind: source | generated | harness | out-of-scope",
        f'pin = "{pin}"', "",
    ]
    for entry in entries:
        lines.append("[[file]]")
        for field, value in entry.items():
            lines.append(f"{field} = {json.dumps(value, ensure_ascii=False)}")
        lines.append("")
    return ("\n".join(lines) + "\n").encode(), entries


def validate_inventory(content, pin, source_files):
    lines = content.decode("utf-8").splitlines()
    if len(lines) < 2 or lines[0] != f"# upstream {pin}" or lines[1] != "file\tpackage\treceiver\tname\tstart\tend\tid":
        raise ValueError("function inventory has missing or mismatched provenance/schema")
    keys = set()
    for line in lines[2:]:
        cols = line.split("\t")
        if len(cols) != 7 or cols[0] not in source_files or not cols[6].startswith(cols[0] + ":"):
            raise ValueError(f"invalid function inventory row: {line}")
        if cols[6] in keys:
            raise ValueError(f"duplicate function inventory ID: {cols[6]}")
        keys.add(cols[6])


def publish(outputs):
    """Stage every output first, then publish the manifest last as a commit marker.

    A crash between replacements leaves hash mismatches, which consumers reject.
    Generation errors before this point cannot partially replace existing files.
    """
    staged = []
    try:
        for destination, content in outputs:
            destination.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as out:
                temp = Path(out.name)
                staged.append((temp, destination))
                out.write(content)
                out.flush()
                os.fsync(out.fileno())
            os.chmod(temp, 0o644)
        for temp, destination in staged:
            os.replace(temp, destination)
    finally:
        for temp, _ in staged:
            temp.unlink(missing_ok=True)


def generate(output_root, upstream, requested_pin=None, inventory_command=None):
    pin = verify_checkout(upstream, requested_pin)
    ledger_path = output_root / "PORTS.toml"
    previous = tomllib.loads(ledger_path.read_text()) if ledger_path.exists() else {}
    paths = source_paths(upstream, pin)
    if not paths:
        raise ValueError("upstream pin contains no compiler Go source files")
    ledger, entries = build_ledger(upstream, pin, previous, read_blobs(upstream, pin, paths))
    command = inventory_command or ["go", "run", str(REPO_ROOT / "scripts/go-inventory/main.go")]
    result = subprocess.run([*command, "--pin", pin, str(upstream / "tsc")], capture_output=True)
    if result.returncode:
        raise ValueError("function inventory generation failed: " + result.stderr.decode(errors="replace").strip())
    inventory = result.stdout
    validate_inventory(inventory, pin, set(paths))
    if verify_checkout(upstream, pin) != pin:
        raise ValueError("upstream changed during generation")
    manifest = (json.dumps({
        "schema_version": 2, "pin": pin,
        "ledger_generated_sha256": ledger_generated_sha256(tomllib.loads(ledger.decode("utf-8"))),
        "inventory_sha256": hashlib.sha256(inventory).hexdigest(),
    }, indent=2) + "\n").encode()
    publish([(ledger_path, ledger), (output_root / "data/go-functions.tsv", inventory),
             (output_root / "data/upstream.json", manifest)])
    return entries, pin


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("upstream", nargs="?", type=Path, default=REPO_ROOT / "upstream")
    parser.add_argument("--pin", help="commit that must resolve to the clean upstream checkout's HEAD")
    args = parser.parse_args()
    try:
        entries, pin = generate(REPO_ROOT, args.upstream.resolve(), args.pin)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"ledger-init: {error}", file=sys.stderr)
        return 1
    print(f"Generated {len(entries)} ledger entries and function inventory at {pin}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
