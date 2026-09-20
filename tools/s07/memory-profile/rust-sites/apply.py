#!/usr/bin/env python3
"""Add safe allocation scopes to a disposable, already census-instrumented tree."""

import argparse
import difflib
import hashlib
import json
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
MARKER = "// Diagnostic allocation sites: staged copy only."


def sha(value):
    return hashlib.sha256(value).hexdigest()


def patches(stage):
    changes = {}
    sites = []

    def read(relative):
        if relative not in changes:
            source = (stage / relative).read_text()
            if relative.endswith(".rs") and not source.startswith((ROOT / relative).read_text()):
                raise ValueError(f"stage no longer has the production source prefix: {relative}")
            changes[relative] = source
        return changes[relative]

    def register(relative, start, variant, name, scope):
        # Before site edits, the census only appends declarations, so these
        # coordinates are also coordinates in the production source.
        original = (ROOT / relative).read_text()
        source = (stage / relative).read_text()
        line = source.count("\n", 0, start) + 1
        if line > len(original.splitlines()):
            raise ValueError(f"site is not in production source: {relative}:{line}")
        sites.append(dict(variant=variant, name=name, source_file=relative,
                          source_line=line, source_sha256=sha(original.encode()),
                          scope=scope))

    def function(relative, function_name, variant, name):
        source = read(relative)
        pattern = r"\bfn " + re.escape(function_name) + r"\s*(?:<[^{};]*?>)?\s*\([^{};]*?\)[^{};]*?\{"
        matches = list(re.finditer(pattern, source, re.S))
        expected = 2 if relative == "crates/tsr_ast/src/storage.rs" and function_name in {"node_slice", "text_slice"} else 1
        if expected == 2:
            matches = [match for match in matches if "&mut self" in match.group()]
        if len(matches) != expected:
            raise ValueError(f"expected {expected} {relative}:{function_name}, got {len(matches)}")
        base = (stage / relative).read_text()
        anchor = list(re.finditer(pattern, base, re.S))
        if expected == 2:
            anchor = [match for match in anchor if "&mut self" in match.group()]
        if len(anchor) != expected:
            raise ValueError(f"ambiguous original anchor: {relative}:{function_name}")
        register(relative, anchor[0].start(), variant, name, "function body, inclusive callees")
        sites[-1]["source_occurrences"] = [base.count("\n", 0, match.start()) + 1 for match in anchor]
        qualifier = "crate" if relative.startswith("crates/tsr_jsstring/") else "tsr_jsstring"
        statement = f"\n        let _memory_site = {qualifier}::memory_sites::site({qualifier}::memory_sites::Site::{variant});"
        for match in reversed(matches):
            source = source[:match.end()] + statement + source[match.end():]
        changes[relative] = source

    def expression(relative, expression, variant, name, expected=1):
        source = read(relative)
        base = (stage / relative).read_text()
        if source.count(expression) != expected or base.count(expression) != expected:
            raise ValueError(f"expression count changed: {relative}: {expression}")
        register(relative, base.index(expression), variant, name,
                 f"exact expression ({expected} source occurrence(s))")
        sites[-1]["source_occurrences"] = [base.count("\n", 0, match.start()) + 1
                                           for match in re.finditer(re.escape(expression), base)]
        replacement = ("{ let _memory_site = tsr_jsstring::memory_sites::site("
                       f"tsr_jsstring::memory_sites::Site::{variant}); {expression} }}")
        changes[relative] = source.replace(expression, replacement)

    for relative, name, variant, label in [
        ("tsr_arena/src/arena.rs", "push", "ArenaPush", "arena.page_and_directory_growth"),
        ("tsr_arena/src/file.rs", "push", "CoreNodeSlots", "arena.core_node_slots"),
        ("tsr_arena/src/file.rs", "push_aux", "CoreAuxSlots", "arena.core_aux_slots"),
        ("tsr_arena/src/node_slots.rs", "insert", "FlowBindingSlots", "binding.flow_slots_and_foreign_map"),
        ("tsr_ast/src/bind_result.rs", "node_mut", "BoundNodeOverlay", "binding.node_overlay_copy_and_map"),
        ("tsr_ast/src/bind_result.rs", "binding_mut", "FullBindings", "binding.full_record_map"),
        ("tsr_ast/src/symbols.rs", "append", "DeclarationAppend", "symbol.declaration_append"),
        ("tsr_ast/src/symbols.rs", "alloc_with_capacity", "DeclarationBacking", "symbol.declaration_backing"),
        ("tsr_binder/src/declarations.rs", "new_symbol", "SymbolSlots", "symbol.arena_slots"),
        ("tsr_binder/src/flow.rs", "new_flow_node_ex", "FlowNodeSlots", "flow.node_slots"),
        ("tsr_binder/src/flow.rs", "new_flow_list", "FlowListSlots", "flow.list_slots"),
        ("tsr_jsstring/src/jsstring.rs", "from_bytes", "StringBacking", "string.from_bytes_arc_backing"),
        ("tsr_jsstring/src/source_text.rs", "from_bytes", "SourceTextDecode", "source_text.decode_and_backing"),
        ("tsr_scanner/src/identifier.rs", "append_token_value", "ScannerAppend", "scanner.append_cooked_token"),
        ("tsr_scanner/src/identifier.rs", "scan_identifier_parts", "ScannerIdentifier", "scanner.identifier_parts"),
        ("tsr_scanner/src/literal.rs", "scan_string", "ScannerString", "scanner.string_literal"),
        ("tsr_scanner/src/literal.rs", "scan_template_and_set_token_value", "ScannerTemplate", "scanner.template_literal"),
        ("tsr_scanner/src/number.rs", "scan_number", "ScannerNumber", "scanner.number_and_cache"),
        ("tsr_ast/src/storage.rs", "node_slice", "NodeSliceBacking", "syntax.node_slice_compaction_and_aux"),
        ("tsr_ast/src/storage.rs", "text_slice", "TextSliceBacking", "syntax.text_slice_compaction_and_aux"),
    ]:
        function("crates/" + relative, name, variant, label)

    expression("crates/tsr_binder/src/declarations.rs",
               "self.table_mut(table).insert(name, Some(symbol))",
               "SymbolMapInsert", "symbol.table_insert", expected=2)
    # Concrete payload allocations occur before Factory::new_node, so a
    # factory-wide scope alone would miss every one of these Box allocations.
    relative = "crates/tsr_ast/src/data_generated.rs"
    payloads = re.findall(r"Self::(\w+)\(Box::new\(data\)\)", read(relative))
    if len(payloads) != 37 or len(set(payloads)) != len(payloads):
        raise ValueError("boxed payload inventory changed; independently classify it")
    for payload in payloads:
        expression(relative, f"Self::{payload}(Box::new(data))",
                   "Payload" + payload, "payload.box." + payload)

    variants = ",\n    ".join(site["variant"] for site in sites)
    metadata = ",\n    ".join(
        "Metadata { name: " + json.dumps(site["name"]) + ", source_file: " +
        json.dumps(site["source_file"]) + ", source_line: " + str(site["source_line"]) + " }"
        for site in sites)
    collector = (HERE / "collector.rs").read_text().replace("/* SITE_VARIANTS */", variants)
    collector = collector.replace("/* SITE_METADATA */", metadata)
    changes["crates/tsr_jsstring/src/memory_sites.rs"] = collector
    lib = "crates/tsr_jsstring/src/lib.rs"
    changes[lib] = read(lib) + "\n" + MARKER + "\npub mod memory_sites;\n"
    manifest = "crates/tsr_jsstring/Cargo.toml"
    original = read(manifest)
    if "alloc_tracker" in original:
        raise ValueError("allocation dependency already present")
    dependency = 'alloc_tracker = { version = "=0.5.25", default-features = false }\n'
    if "[dependencies]\n" in original:
        changes[manifest] = original.replace("[dependencies]\n", "[dependencies]\n" + dependency, 1)
    else:
        changes[manifest] = original + "\n[dependencies]\n" + dependency
    changes["crates/tsr_jsstring/examples/memory_sites_probe.rs"] = (HERE / "probe.rs").read_text()
    return changes, sites


def apply(stage):
    stage = stage.resolve()
    if (stage == ROOT or (ROOT / "crates") in stage.parents
            or not (stage / "census-manifest.json").is_file()):
        raise ValueError("requires an existing disposable census stage")
    if (stage / "sites-manifest.json").exists():
        raise ValueError("already instrumented")
    changes, sites = patches(stage)
    manifest = []
    patch = []
    for relative, text in sorted(changes.items()):
        path = stage / relative
        production = ROOT / relative
        if (path.is_symlink() or stage not in path.resolve().parents
                or path.exists() and production.exists() and path.samefile(production)):
            raise ValueError(f"destination aliases or escapes production: {relative}")
        old = path.read_bytes() if path.exists() else b""
        new = text.encode()
        manifest.append(dict(path=relative, before_sha256=sha(old) if path.exists() else None,
                             after_sha256=sha(new), before_bytes=len(old), after_bytes=len(new)))
        patch.extend(difflib.unified_diff(old.decode().splitlines(keepends=True),
                     text.splitlines(keepends=True), fromfile="a/" + relative,
                     tofile="b/" + relative))
    for relative, text in sorted(changes.items()):
        path = stage / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    patch_bytes = "".join(patch).encode()
    (stage / "sites.patch").write_bytes(patch_bytes)
    record = dict(schema=1, stage=str(stage), operation="staged-safe-allocation-scopes",
                  allocator_dependency=dict(name="alloc_tracker", version="0.5.25",
                      crate_sha256="70bc5b36f4124124cdeae56ea9527e8db3e90f7ac7382c0b7a07d780163629a4"),
                  census_manifest_sha256=sha((stage / "census-manifest.json").read_bytes()),
                  patch_sha256=sha(patch_bytes), sites=sites, files=manifest,
                  generator_files=[dict(name=p.name, sha256=sha(p.read_bytes()))
                                   for p in sorted(HERE.iterdir()) if p.suffix in {".py", ".rs"}])
    (stage / "sites-manifest.json").write_text(json.dumps(record, indent=2) + "\n")
    return record


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", required=True, type=Path)
    print(json.dumps(apply(parser.parse_args().stage), indent=2))
