"""Deterministic Rust tables from access-only observations of the pinned Go data."""

import hashlib
import json
from pathlib import Path
import tomllib

from s04_common import command, strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ("tsc/internal/scanner/scanner.go", "tsc/internal/scanner/unicodeproperties.go",
          "tsc/internal/stringutil/identifier_parts_generated.go", "tsc/internal/ast/kind_generated.go")


def rust_string_literal(text):
    # JSON's \uXXXX escapes are not Rust string escapes.
    escaped = []
    for character in text:
        if character in ('"', "\\"):
            escaped.append("\\" + character)
        elif ord(character) < 0x20 or ord(character) == 0x7F:
            escaped.append(f"\\u{{{ord(character):x}}}")
        else:
            escaped.append(character)
    return '"' + "".join(escaped) + '"'


def rust_byte_literal(text):
    escaped = []
    for byte in text.encode("utf-8"):
        if byte in (ord('"'), ord("\\")):
            escaped.append("\\" + chr(byte))
        elif 0x20 <= byte < 0x7F:
            escaped.append(chr(byte))
        else:
            escaped.append(f"\\x{byte:02x}")
    return 'b"' + "".join(escaped) + '"'


def validate_tables(tables):
    if type(tables) is not dict or set(tables) != {"version", "scanner", "identifier", "simple_fold", "go_unicode_version"} or type(tables["version"]) is not int or tables["version"] != 1:
        raise ValueError("invalid table export envelope")
    if type(tables["go_unicode_version"]) is not str or not tables["go_unicode_version"]:
        raise ValueError("missing Go Unicode version")
    identifier = tables["identifier"]
    if type(identifier) is not dict or set(identifier) != {"start", "part"}:
        raise ValueError("invalid identifier table inventory")
    for name, rows in identifier.items():
        if type(rows) is not list or not rows:
            raise ValueError(f"empty {name} ranges")
        previous = -1
        for row in rows:
            if type(row) is not list or len(row) != 3 or any(type(value) is not int for value in row):
                raise ValueError("invalid range tuple")
            lo, hi, stride = row
            if not previous < lo <= hi <= 0x10FFFF or stride <= 0 or (hi-lo) % stride:
                raise ValueError("invalid/overlapping Unicode range")
            previous = hi
    fold = tables["simple_fold"]
    if type(fold) is not list or not fold:
        raise ValueError("empty SimpleFold table")
    previous = -1
    for row in fold:
        if type(row) is not list or len(row) != 2 or any(type(value) is not int for value in row):
            raise ValueError("invalid fold pair")
        if not previous < row[0] <= 0x10FFFF or not 0 <= row[1] <= 0x10FFFF or row[0] == row[1]:
            raise ValueError("invalid/duplicate fold pair")
        previous = row[0]
    mapping = dict(fold)
    for start in mapping:
        seen, current = set(), start
        while current not in seen:
            seen.add(current)
            if current not in mapping:
                raise ValueError("broken SimpleFold cycle")
            current = mapping[current]
        if current != start:
            raise ValueError("SimpleFold does not form a cycle")
    scanner = tables["scanner"]
    if type(scanner) is not dict or set(scanner) != {"keywords", "tokens", "non_binary", "binary", "strings", "values"}:
        raise ValueError("invalid scanner table inventory")
    for name in ("keywords", "tokens"):
        entries = scanner[name]
        if type(entries) is not dict or not entries or any(type(key) is not str or not key or type(value) is not int or not 0 <= value < 351 for key, value in entries.items()):
            raise ValueError("invalid token table")
    if any(scanner["tokens"].get(key) != value for key, value in scanner["keywords"].items()):
        raise ValueError("keywords disagree with token table")
    if type(scanner["values"]) is not dict or set(scanner["values"]) != {"General_Category", "Script", "Script_Extensions"}:
        raise ValueError("invalid property value domains")
    aliases = scanner["non_binary"]
    if type(aliases) is not dict or not aliases or any(type(key) is not str or not key or type(value) is not str or value not in scanner["values"] for key, value in aliases.items()):
        raise ValueError("invalid nonbinary aliases")
    for entries in [scanner["binary"], scanner["strings"], *scanner["values"].values()]:
        if type(entries) is not list or not entries or any(type(value) is not str or not value for value in entries) or entries != sorted(set(entries)):
            raise ValueError("property sets must be nonempty sorted unique strings")
    if scanner["values"]["Script"] != scanner["values"]["Script_Extensions"]:
        raise ValueError("pinned script domains unexpectedly differ")


def render(tables, pin):
    validate_tables(tables)
    out = ["// Generated from the pinned Go scanner/stringutil tables. Do not edit.", f"// Upstream {pin}; identifier Unicode 15.1; SimpleFold Go Unicode {tables['go_unicode_version']}."]
    def integers(name, rows, width):
        out.append(f"pub(crate) const {name}: &[({', '.join(['u32']*width)})] = &[")
        out.extend("(" + ", ".join(f"{value:_}" for value in row) + ")," for row in rows)
        out.append("];")
    def strings(name, rows):
        out.append(f"pub(crate) const {name}: &[&str] = &[")
        out.extend(rust_string_literal(row) + "," for row in rows)
        out.append("];")
    def mapping(name, rows, numeric=False):
        out.append(f"pub(crate) const {name}: &[(&str, {'u16' if numeric else '&str'})] = &[")
        out.extend(f"({rust_string_literal(key)}, {value if numeric else rust_string_literal(value)})," for key, value in sorted(rows.items()))
        out.append("];")
    integers("IDENTIFIER_START", tables["identifier"]["start"], 3)
    integers("IDENTIFIER_PART", tables["identifier"]["part"], 3)
    scanner = tables["scanner"]
    mapping("KEYWORDS", scanner["keywords"], True)
    out.extend(["pub(crate) fn keyword_kind(bytes: &[u8]) -> Option<u16> {", "match bytes {"])
    out.extend(f"{rust_byte_literal(key)} => Some({value})," for key, value in sorted(scanner["keywords"].items()))
    out.extend(["_ => None,", "}", "}"])
    mapping("TOKENS", scanner["tokens"], True)
    mapping("NON_BINARY_PROPERTIES", scanner["non_binary"])
    strings("BINARY_PROPERTIES", scanner["binary"])
    strings("STRING_PROPERTIES", scanner["strings"])
    strings("GENERAL_CATEGORIES", scanner["values"]["General_Category"])
    strings("SCRIPT_VALUES", scanner["values"]["Script"])
    edition = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["edition"]
    return command(["rustfmt", "--edition", str(edition)], cwd=ROOT, data=("\n".join(out)+"\n").encode())


def render_fold(tables, pin):
    validate_tables(tables)
    out = ["// Generated from pinned Go unicode.SimpleFold. Do not edit.",
           f"// Upstream {pin}; Go Unicode {tables['go_unicode_version']}.",
           "pub(crate) const SIMPLE_FOLD: &[(u32, u32)] = &["]
    out.extend(f"({a:_}, {b:_})," for a,b in tables["simple_fold"])
    out.append("];")
    edition = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["edition"]
    return command(["rustfmt", "--edition", str(edition)], cwd=ROOT, data=("\n".join(out)+"\n").encode())


def update(tables, write=False):
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    raw = (json.dumps(tables, sort_keys=True, indent=2)+"\n").encode()
    generated = render(tables, pin)
    outputs = {"data/s05/tables.json": raw, "crates/tsr_scanner/src/tables_generated.rs": generated, "crates/tsr_jsstring/src/go_fold_generated.rs": render_fold(tables, pin)}
    manifest = {"version": 1, "upstream_pin": pin,
                "go_unicode_version": tables["go_unicode_version"], "identifier_unicode_version": "15.1.0",
                "inputs": {path: hashlib.sha256((ROOT / "upstream" / path).read_bytes()).hexdigest() for path in INPUTS},
                "outputs": {path: hashlib.sha256(content).hexdigest() for path, content in outputs.items()}}
    # The Go pin is owned by the shared runtime manifest, not this generator.
    manifest["go"] = tomllib.loads((ROOT / "data/s04/toolchains.toml").read_text())["go"]
    outputs["data/s05/tables-manifest.json"] = (json.dumps(manifest, sort_keys=True, indent=2)+"\n").encode()
    for name, content in outputs.items():
        path = ROOT / name
        if write:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        elif not path.exists() or path.read_bytes() != content:
            raise ValueError(f"S05 table drift: {name}")
    return manifest
