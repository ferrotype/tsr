#!/usr/bin/env python3
"""Export pinned x/text registry/CLDR data; never change the module cache or upstream.

The two Go overlays expose data and the diagnostic matcher's compiled index,
not answers to the leaf requests. The manifest authenticates every copied Go
source and the exporter; --check repeats the export and compares final bytes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import tempfile

from s04 import go_environment, verified_upstream
from s04_common import command
from s05_tables import rust_string_literal

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = "crates/tsr_locale/src/tables_generated.rs"
MANIFEST = "data/phase1/locale-tables-manifest.json"
EXPORTS = ("internal_export_test.go", "matcher_export_test.go")
FOLDERS = ("internal/language", "internal/language/compact", "internal/tag", "language")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def render(a, b):
    # The pinned diagnostic roster has no index entries requiring a continuation.
    # Do not silently use the specialized selector against a changed roster.
    if any(row[8] != 0 for rows in b["candidates"].values() for row in rows):
        raise ValueError("matcher continuation changed; port it before regenerating")
    out = ["// Generated from golang.org/x/text v0.38.0 (CLDR 32). Do not edit.",
           "// Run python3 scripts/generate_locale_tables.py --write."]
    def decl(name, ty, value):
        out.append(f"pub(crate) static {name.upper()}: {ty} = {value};")
    def numbers(values):
        return "[" + ",".join(numbers(value) for value in values) + "]" if isinstance(values, list) else f"{values:_}"
    def strs(values):
        return "[" + ",".join(json.dumps(x) for x in values) + "]"
    for name in ("languages", "scripts", "regions", "variants"):
        decl(name, "&[(&str,u16)]", "&[" + ",".join(
            "(" + json.dumps(key) + "," + str(value) + ")"
            for key, value in sorted(a[name].items())) + "]")
    decl("language_names", "&[(u16,&str)]", "&[" + ",".join(
        "(" + key + "," + json.dumps(value) + ")"
        for key, value in sorted(a["language_names"].items(), key=lambda row: int(row[0]))) + "]")
    for name in ("region_names", "script_names"):
        decl(name, "&[&str]", "&" + strs(a[name]))
    decl("grandfathered", "&[(&str,&str)]", "&[" + ",".join(
        "(" + json.dumps(key) + "," + json.dumps(value) + ")"
        for key, value in sorted(a["grandfathered"].items())) + "]")
    for name in ("aliases", "alias_types", "region_aliases", "likely_script", "likely_lang",
                 "likely_lang_list", "likely_region", "likely_region_list", "likely_region_group",
                 "region_inclusion", "region_inclusion_bits", "region_containment"):
        values = a[name]
        ty = "u64" if name in ("region_inclusion_bits", "region_containment") else "u16"
        if isinstance(values[0], list):
            ty = f"[{ty};{len(values[0])}]"
        decl(name, f"&[{ty}]", "&" + numbers(values))
    for name in ("match_region", "paradigms", "region_groups"):
        values = b[name]
        ty = f"[u16;{len(values[0])}]" if isinstance(values[0], list) else "u16"
        decl(name, f"&[{ty}]", "&" + numbers(values))
    decl("candidates", "&[(u16,&[[u16;9]])]", "&[" + ",".join(
        "(" + key + ",&" + numbers(value) + ")"
        for key, value in sorted(b["candidates"].items(), key=lambda row: int(row[0]))) + "]")
    return "\n".join(out) + "\n"


def generate():
    upstream = verified_upstream()
    env = go_environment()
    env["GOWORK"] = "off"
    module = (upstream / "tsc/go.mod").read_text()
    match = re.search(r"^\s*golang.org/x/text\s+(\S+)", module, re.MULTILINE)
    if not match or match[1] != "v0.38.0":
        raise ValueError("locale tables require review at the new x/text pin")
    spec = "golang.org/x/text@" + match[1]
    downloaded = json.loads(command(["go", "mod", "download", "-json", spec], cwd=upstream / "tsc", env=env))
    sums = (upstream / "tsc/go.sum").read_text().splitlines()
    for suffix, key in (("", "Sum"), ("/go.mod", "GoModSum")):
        if f"golang.org/x/text {match[1]}{suffix} {downloaded[key]}" not in sums:
            raise ValueError("x/text download differs from the pinned go.sum")
    source = Path(downloaded["Dir"])
    inputs = {}
    stage = ROOT / "target/phase1"
    stage.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="locale-export-", dir=stage) as temporary:
        output = Path(temporary)
        copied = output / "xtext"
        files = [source / name for name in ("go.mod", "go.sum", "LICENSE")]
        files += sorted(path for folder in FOLDERS for path in (source / folder).glob("*.go")
                        if not path.name.endswith("_test.go"))
        for path in files:
            relative = path.relative_to(source)
            destination = copied / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            data = path.read_bytes()
            inputs[f"{spec}/{relative}"] = digest(data)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        (output / "go.mod").write_text("module phase1-locale-export\n\ngo 1.27.1\n\n"
                                      "require golang.org/x/text v0.38.0\n\nreplace golang.org/x/text => ./xtext\n")
        (output / "go.sum").write_text("\n".join(line for line in sums if line.startswith("golang.org/x/text ")) + "\n")
        replacement = {str(copied / folder / "phase1_export_test.go"): str(ROOT / "tools/phase1/locale" / exporter)
                       for folder, exporter in zip(("internal/language", "language"), EXPORTS)}
        (output / "overlay.json").write_text(json.dumps({"Replace": replacement}))
        parse_source = source / "internal/language/parse_test.go"
        inputs[f"{spec}/internal/language/parse_test.go"] = digest(parse_source.read_bytes())
        tags = {json.loads(value) for value in re.findall(r'in:\s*("(?:[^"\\]|\\.)*")', parse_source.read_text())}
        tags.update(["en-t-fr-est", "en_t_pt_MLt", "en-u-ca-gregory-ca-buddhist", "en-t-zz-cyrl", "de-abcdefg", "de-1606nict-1694acad-1901-1959acad-1996-abl1943-akuapem-aluku-ao1990-1901-aranes"])
        for language in ("und", "zh", "cmn", "yue", "sr", "sh", "gsw", "pt", "no", "nb", "fil", "en", "fr", "de"):
            for suffix in ("", "-Latn", "-Cyrl", "-Hant", "-Hans", "-US", "-TW", "-HK", "-BR", "-PT", "-GB", "-CH"):
                tags.add(language + suffix)
        (output / "cases.json").write_text(json.dumps(sorted(tags)))
        env.update(PHASE1_LOCALE_CASES=str(output / "cases.json"),
                   PHASE1_LOCALE_CONFORMANCE=str(output / "conformance.json"))
        env.update(PHASE1_LOCALE_INTERNAL=str(output / "internal.json"),
                   PHASE1_LOCALE_MATCHER=str(output / "matcher.json"))
        command(["go", "test", "-count=1", "-overlay", str(output / "overlay.json"), "-run", "^TestPhase1Locale",
                 "golang.org/x/text/internal/language", "golang.org/x/text/language"], cwd=output, env=env)
        internal, matcher = (json.loads((output / name).read_text()) for name in ("internal.json", "matcher.json"))
        rust = output / "tables.rs"
        rust.write_text(render(internal, matcher))
        command(["rustfmt", "--edition", "2021", "--config-path", str(ROOT / "rustfmt.toml"), str(rust)], cwd=ROOT)
        generated = rust.read_bytes()
        rows = json.loads((output / "conformance.json").read_text())
        test = ["// Generated native expectations from pinned x/text. Do not edit.",
                "#[test]", "fn pinned_parse_recovery_and_diagnostic_matching() {",
                "let rows: &[(&str,&str,&str,i32)] = &["]
        test.extend("(" + ",".join(rust_string_literal(value) for value in row[:3]) + f",{row[3]})," for row in rows)
        test.extend(["];", "let mut differences = Vec::new();",
                     "for &(input, expected_tag, expected_error, expected_match) in rows {",
                     "let (tag, error) = tsr_locale::Locale::parse_detailed(input);",
                     "let actual = (tag.tag_string(), error.map(|e|e.to_string()).unwrap_or_default(), tag.diagnostic_match().map_or(-1, |i|i as i32));",
                     "if actual != (expected_tag.to_owned(), expected_error.to_owned(), expected_match) { differences.push((input, actual, (expected_tag,expected_error,expected_match))); }",
                     "}", 'assert!(differences.is_empty(), "{differences:#?}");', "}"])
        test_path = output / "native.rs"
        test_path.write_text("\n".join(test) + "\n")
        command(["rustfmt", "--edition", "2021", "--config-path", str(ROOT / "rustfmt.toml"), str(test_path)], cwd=ROOT)
        native = test_path.read_bytes()
    for path in ["scripts/generate_locale_tables.py", "scripts/s04.py", "scripts/s04_common.py",
                 "scripts/s04_runtime.py", "scripts/s05_tables.py", "scripts/tracking-bootstrap.py",
                 "data/s04/toolchains.toml",
                 "upstream/tsc/go.mod", "upstream/tsc/go.sum", "rustfmt.toml",
                 *(f"tools/phase1/locale/{name}" for name in EXPORTS)]:
        inputs[path] = digest((ROOT / path).read_bytes())
    manifest = {"version": 1, "pin": json.loads((ROOT / "data/upstream.json").read_text())["pin"],
                "module": spec, "inputs": dict(sorted(inputs.items())), "outputs": {OUTPUT: digest(generated), "crates/tsr_locale/tests/native_generated.rs": digest(native)}}
    return {OUTPUT: generated, "crates/tsr_locale/tests/native_generated.rs": native, MANIFEST: (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for path, data in generate().items():
        destination = ROOT / path
        if args.write:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        elif not destination.exists() or destination.read_bytes() != data:
            raise SystemExit(f"locale generation drift: {path}")
    print("locale tables generated" if args.write else "locale tables reproduce")


if __name__ == "__main__":
    main()
