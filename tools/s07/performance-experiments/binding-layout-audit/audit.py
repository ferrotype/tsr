#!/usr/bin/env python3
"""Compile layout observations using an existing tsr_ast rlib; no production build.

The base inventory is checked independently against normalized SchemaAPI output
and the actual generated/handwritten Go struct embeddings. This is a diagnostic,
not a replacement-layout, allocation, RSS or performance claim.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
FIELDS = {
    "FlowNode", "Symbol", "LocalSymbol", "Locals", "NextContainer",
    "EndFlowNode", "ReturnFlowNode", "FallthroughFlowNode",
}
INPUTS = [
    "data/s03/schema/ast.json", "tools/s03/ast-export.mts",
    "xtask/src/gen/ast.rs", "crates/tsr_ast/src/data_generated.rs",
    "crates/tsr_ast/src/lib.rs", "crates/tsr_ast/src/lists.rs",
    "crates/tsr_ast/src/node_kind.rs", "crates/tsr_arena/src/ids.rs",
    "crates/tsr_jsstring/src/jsstring.rs", "crates/tsr_ast/src/flow.rs",
    "upstream/tools/scripts/tsc/ast.json",
    "upstream/tools/scripts/tsc/generate-go-ast.ts",
    "upstream/tsc/internal/ast/ast_generated.go", "upstream/tsc/internal/ast/ast.go",
    "upstream/tsc/internal/binder/binder.go",
    "tools/s07/performance-experiments/phases/layout-projection.json.gz",
    "tools/s07/performance-experiments/binding-layout-audit/audit.py",
    "tools/s07/performance-experiments/binding-layout-audit/sketch.rs",
    "tools/s07/performance-experiments/binding-layout-audit/test_audit.py",
]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def closure(names, bases, stack=()):
    result = []
    for name in names:
        if name not in bases:
            raise ValueError(f"unknown base {name}")
        if name in stack:
            raise ValueError(f"cyclic base {name}")
        result.append(name)
        result.extend(closure(bases[name]["extends"], bases, (*stack, name)))
    duplicates = [name for name, count in Counter(result).items() if count > 1]
    if duplicates:
        raise ValueError(f"duplicate embedded base: {duplicates}")
    return result


def category(bases):
    if "LocalsContainerBase" in bases:
        return "locals_or_function"
    flow, declaration = "FlowNodeBase" in bases, "DeclarationBase" in bases
    if flow and declaration:
        return "flow_and_declaration_without_locals"
    if flow:
        return "flow_without_declaration_or_locals"
    if declaration:
        return "declaration_without_flow_or_locals"
    return "other"


def base_inventory(schema, go_source):
    bases = {base["name"]: base for base in schema["bases"]}
    structs = dict(re.findall(r"type (\w+) struct\s*\{([^{}]*)\}", go_source))
    def physical_fields(name):
        return {field: typ for field, typ in re.findall(r"^\s*(\w+)\s+(\*?\w+)", structs[name], re.M)
                if field in FIELDS}
    groups, rows = {}, {}
    for node in schema["nodes"]:
        name = node["name"]
        if name in rows:
            raise ValueError(f"duplicate shape {name}")
        all_bases = closure(node["extends"], bases)
        if set(all_bases) != set(node["baseTypes"]):
            raise ValueError(f"resolver transitive base mismatch: {name}")
        if name not in structs:
            raise ValueError(f"missing concrete Go struct {name}")
        embeds = re.findall(r"^\s*(\w+)\s*(?://[^\n]*)?$", structs[name], re.M)
        embeds = [base for base in embeds if base in bases]
        if Counter(embeds) != Counter(node["extends"]):
            raise ValueError(f"Go/schema direct embeddings differ: {name}")
        # Independently check the transitive physical Go base graph as well.
        for base in all_bases:
            if base not in structs:
                raise ValueError(f"missing physical Go base {base}")
            physical = re.findall(r"^\s*(\w+)\s*(?://[^\n]*)?$", structs[base], re.M)
            physical = [item for item in physical if item in bases]
            if Counter(physical) != Counter(bases[base]["extends"]):
                raise ValueError(f"Go/schema base embeddings differ: {base}")
        group = category(all_bases)
        groups.setdefault(group, []).append(name)
        binding = [field["name"] for field in node["fields"]
                   if field["goOnly"] and field["name"] in FIELDS]
        origins = {field: base for base in [*all_bases, name] for field in physical_fields(base)}
        physical_binding = {field: typ for base in [*all_bases, name] for field, typ in physical_fields(base).items()}
        normalized_binding = {field["name"]: field["type"]["name"] for field in node["fields"]
                              if field["goOnly"] and field["name"] in FIELDS}
        if physical_binding != normalized_binding:
            raise ValueError(f"Go/schema binding fields differ: {name}")
        rows[name] = {"category": group, "direct_bases": node["extends"],
                      "transitive_bases": all_bases, "binding_fields": binding,
                      "binding_field_origins": origins, "direct_binding_fields": physical_fields(name)}
    totals = {base: sum(base in row["transitive_bases"] for row in rows.values())
              for base in ["FlowNodeBase", "DeclarationBase", "LocalsContainerBase",
                           "FunctionLikeBase", "FunctionLikeWithBodyBase", "ExportableBase", "BodyBase"]}
    return {"shape_count": len(rows), "partition_counts": {key: len(value) for key, value in groups.items()},
            "partition_members": groups, "overlapping_base_totals": totals,
            "flow_and_declaration_total_including_locals": sum(
                {"FlowNodeBase", "DeclarationBase"} <= set(row["transitive_bases"]) for row in rows.values()),
            "binding_fields_outside_three_bases": {name: row["binding_fields"] for name, row in rows.items()
                if row["category"] == "other" and row["binding_fields"]},
            "shapes_with_binding_fields": sum(bool(row["binding_fields"]) for row in rows.values()),
            "binding_field_occurrences": dict(Counter(field for row in rows.values() for field in row["binding_fields"])),
            "direct_binding_fields_by_shape": {name: row["direct_binding_fields"] for name, row in rows.items()
                                               if row["direct_binding_fields"]},
            "known_kind_count": len(schema["kinds"]), "kind_alias_count": len(schema["kindAliases"]),
            "multi_kind_shapes": {node["name"]: len(node["kinds"]) for node in schema["nodes"] if node["multiKind"]},
            "rows": rows}


def generated_inventory(source):
    found = re.search(r"pub enum NodeData \{(.*?)^\}", source, re.S | re.M)
    if not found:
        raise ValueError("missing generated enum")
    variants = []
    for line in found[1].strip().splitlines():
        match = re.fullmatch(r"\s*(\w+)\((Box<)?(\w+Data)>?\),", line)
        if not match:
            raise ValueError(f"unknown enum entry: {line}")
        name, box, typ = match.groups()
        variants.append({"name": name, "type": typ, "boxed": bool(box)})
    fields = dict(re.findall(r"pub struct (\w+)Data \{([^{}]*)\}", source))
    if len(variants) != len(fields) or {row["name"] for row in variants} != set(fields):
        raise ValueError("enum/payload shape inventory mismatch")
    for variant in variants:
        body = fields[variant["name"]]
        types = re.findall(r"    pub [\w#]+: ([^\n]+),", body)
        if len(types) != body.count("pub "):
            raise ValueError("unknown generated payload field")
        budget = sum(32 if typ == "JsString" else 16 if typ in {"NodeSlice", "TextSlice"} else 8
                     for typ in types)
        if variant["boxed"] != (budget > 32):
            raise ValueError(f"generator boxing budget differs: {variant['name']}")
        variant["conservative_generator_budget"] = budget
    return variants


def render(variants):
    template = (HERE / "sketch.rs").read_text()
    def enum(name, augment_identifier=False):
        fields = []
        for row in variants:
            typ = row["type"]
            if augment_identifier and row["name"] == "Identifier":
                if row["boxed"]:
                    raise ValueError("Identifier is no longer inline")
                typ = f"WithBinding<{typ}>"
            elif row["boxed"]:
                typ = f"Box<{typ}>"
            fields.append(f"    {row['name']}({typ}),")
        return f"enum {name} {{\n" + "\n".join(fields) + "\n}\n"
    observations = [f'observe::<{row["type"]}>("{row["name"]}");' for row in variants]
    return template.replace("// ENUMS", enum("CopiedData") + enum("IdentifierPlusEightData", True)).replace(
        "// PAYLOAD_OBSERVATIONS", "\n    ".join(observations))


def run(rlib, output, rustc):
    import gzip
    before = {path: sha(ROOT / path) for path in INPUTS}
    schema = json.loads((ROOT / INPUTS[0]).read_text())
    source = (ROOT / "crates/tsr_ast/src/data_generated.rs").read_text()
    go_source = "\n".join((ROOT / path).read_text() for path in [
        "upstream/tsc/internal/ast/ast_generated.go", "upstream/tsc/internal/ast/ast.go"])
    bases = base_inventory(schema, go_source)
    variants = generated_inventory(source)
    if {row["name"] for row in variants} != set(bases["rows"]):
        raise ValueError("generated enum/schema inventory differs")
    output.mkdir(parents=True, exist_ok=True)
    generated, binary = output / "generated.rs", output / "layout-sketch"
    generated.write_text(render(variants))
    version = subprocess.run([*rustc, "-vV"], capture_output=True, text=True, check=True).stdout
    # The probe imports actual production payload types; only its replacement
    # enum/Node frame are synthetic. No approximate stand-ins for JsString/IDs.
    command = [*rustc, "--edition=2021", "-Dwarnings", "-Cpanic=abort", "-Clto=thin", str(generated),
               "--extern", f"tsr_ast={rlib}", "-L", f"dependency={rlib.parent}", "-o", str(binary)]
    rlib_hash = sha(rlib)
    subprocess.run(command, check=True)
    raw = subprocess.run([str(binary)], capture_output=True, text=True, check=True).stdout
    (output / "stdout.txt").write_text(raw)
    layouts = {name: {"size": int(size), "alignment": int(alignment)}
               for name, size, alignment in map(str.split, raw.splitlines())}
    for row in variants:
        row.update(layouts[row["name"]])
    census = json.loads(gzip.decompress((ROOT / INPUTS[15]).read_bytes()))
    count = census["core_nodes"]
    increment = layouts["NodeWithIdentifierPlusEight"]["size"] - layouts["ActualNode"]["size"]
    assert layouts["CopiedData"] == layouts["ActualNodeData"]
    assert layouts["NodeWithCopiedData"] == layouts["ActualNode"]
    if sha(rlib) != rlib_hash or before != {path: sha(ROOT / path) for path in INPUTS}:
        raise ValueError("source or linked rlib changed during layout observation")
    return {"version": 1, "diagnostic_only": True,
            "provenance": {"source_sha256": before, "rustc": version, "command": command,
                "linked_ast_rlib": str(rlib), "linked_ast_rlib_sha256": rlib_hash,
                "generated_source_sha256": sha(generated), "binary_sha256": sha(binary),
                "stdout_sha256": sha(output / "stdout.txt")},
            "actual_and_candidate_layouts": {key: value for key, value in layouts.items() if key not in bases["rows"]},
            "nonboxed_32_byte_payloads": [row["name"] for row in variants if not row["boxed"] and row["size"] == 32],
            "nonboxed_32_byte_payloads_with_binding_fields": [row["name"] for row in variants
                if not row["boxed"] and row["size"] == 32 and bases["rows"][row["name"]]["binding_fields"]],
            "payload_inventory": variants, "base_inventory": bases,
            "frozen_core_nodes": count, "additional_used_bytes_per_core_node": increment,
            "additional_used_core_node_bytes": count * increment,
            "qualifications": [
                "IdentifierPlusEight retains the existing inline/boxed decisions and adds one Option<FlowId> to Identifier only.",
                "CopiedData uses the real payload types and enum order; its size/alignment and copied Node frame must match real NodeData/Node.",
                "The 83/109 partition gives locals precedence; its flow/declaration groups are not total base occurrence counts.",
                "CaseOrDefaultClause is outside the three bases but owns a binder-written FallthroughFlowNode.",
                "Existing generator excludes deferred fields before its >32 budget decision; storing them requires an explicit rule change.",
                "Reboxing enlarged payloads can preserve the enum ceiling while adding separate payload allocations; this candidate does not price that alternative.",
                "Node-count multiplication is occupied core-record bytes only; page spare/capacity, directories, replacement side tables and allocation/RSS effects are not priced.",
                "This probe reuses an existing rlib rather than rebuilding production. The rlib and input declarations are recorded; compiler execution verifies copied enum/frame sizes, not a build-from-current-source provenance claim.",
                "Payload shape is independent of SyntaxKind: multi-kind structs and aliases do not add distinct payload variants.",
                "No timing, memory allocation capture or production mutation is performed.",
            ]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rlib", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolchain", help="must match the existing rlib; defaults to the workspace toolchain")
    args = parser.parse_args()
    rustc = ["rustc", f"+{args.toolchain}"] if args.toolchain else ["rustc"]
    result = run(args.rlib.resolve(), args.output.resolve(), rustc)
    (args.output / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"output": str(args.output / "result.json"),
                      "nonboxed_32_byte_payloads": result["nonboxed_32_byte_payloads"],
                      "layouts": result["actual_and_candidate_layouts"],
                      "partition": result["base_inventory"]["partition_counts"],
                      "outside_bases": result["base_inventory"]["binding_fields_outside_three_bases"]}, indent=2))
