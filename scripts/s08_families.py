#!/usr/bin/env python3
"""P1 storage-families trace: original Go constructors and the Rust port over one frozen action
sequence, with every result compared and the live storage counted on both sides."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess

from s04 import go_environment, same_json_value, verified_upstream
from s04_common import command, strict_json_loads
from s08_oracle import ROOT, canonical, digest

REQUESTS = "data/s08/storage-families.json"
OBSERVATIONS = "data/s08/families-observations.json"
BRIDGE = "tools/s08/oracle/families/bridge.go"
DRIVER = "tools/s08/oracle/families/driver_test.go"
SOURCES = (
    "scripts/s08_families.py", "scripts/s08_oracle.py", "scripts/s04.py", "scripts/s04_common.py",
    "scripts/s04_runtime.py", "scripts/tracking-bootstrap.py", "data/s04/toolchains.toml",
    "data/upstream.json", ".gitmodules", ".cargo/config.toml", BRIDGE, DRIVER, REQUESTS,
)
SOURCE_GLOBS = ("crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.*", "rust-toolchain.toml")
NATIVE_SOURCES = ("internal/checker/checker.go", "internal/checker/types.go", "internal/checker/utilities.go",
                  "internal/checker/links.go", "internal/core/arena.go", "internal/core/linkstore.go", "go.mod", "go.sum")


def hexed(text):
    return text.encode().hex()


def trace(strict_null_checks):
    """The frozen P1 action sequence: every constructor family the port has, with the
    orderings, interning hits, fresh/regular links and reductions upstream exercises."""
    actions = []

    def add(action):
        actions.append(action)
        return len(actions) - 1

    def builtin(name):
        return add({"op": "builtin", "name": name})

    string_t, number_t, boolean_t = builtin("stringType"), builtin("numberType"), builtin("booleanType")
    bigint_t, undefined_t, null_t = builtin("bigintType"), builtin("undefinedType"), builtin("nullType")
    never_t, any_t, es_symbol_t = builtin("neverType"), builtin("anyType"), builtin("esSymbolType")
    void_t, unknown_t = builtin("voidType"), builtin("unknownType")
    # String literals: interning, arbitrary bytes, fresh/regular links from both sides.
    literals = [add({"op": "string", "text_hex": hexed(text)}) for text in ("a", "b", "", "z", "aa", "A")]
    add({"op": "string", "text_hex": hexed("a")})
    literals.append(add({"op": "string", "text_hex": "eda080"}))
    literals.append(add({"op": "string", "text_hex": "ffc0af"}))
    literals.append(add({"op": "string", "text_hex": "f09f9880"}))
    fresh_a = add({"op": "fresh", "root": literals[0]})
    add({"op": "fresh", "root": fresh_a})
    add({"op": "regular", "root": fresh_a})
    add({"op": "regular", "root": literals[1]})
    # Numbers: integers, negatives, fractions, +0/-0 sharing, NaN, infinities, interning.
    numbers = []
    for value in (0.0, -0.0, 1.0, 2.0, 10.0, -1.0, 0.5, 1e21, float("inf"), float("-inf"), float("nan"), 9007199254740993.0):
        numbers.append(add({"op": "number", "bits_hex": format(__import__("struct").unpack("<Q", __import__("struct").pack("<d", value))[0], "016x")}))
    add({"op": "number", "bits_hex": format(0x7FF8000000000000, "016x")})
    fresh_one = add({"op": "fresh", "root": numbers[2]})
    add({"op": "regular", "root": fresh_one})
    # Bigints: zero, sign, leading zeros, interning.
    bigints = [add({"op": "bigint", "negative": neg, "digits": digits}) for neg, digits in ((False, "0"), (True, "0"), (False, "00042"), (True, "42"), (False, "123456789012345678901234567890"))]
    add({"op": "bigint", "negative": False, "digits": "42"})
    add({"op": "fresh", "root": bigints[3]})
    # Booleans through the builtins.
    true_t, false_t = builtin("trueType"), builtin("regularFalseType")
    add({"op": "fresh", "root": false_t})
    add({"op": "regular", "root": true_t})
    # Unions: ordering, deduplication, literal reduction, nullable handling, cache reuse,
    # union-of-union fast path, boolean pairs, never removal, none reduction.
    u1 = add({"op": "union", "types": [number_t, string_t], "reduction": "literal"})
    add({"op": "union", "types": [string_t, number_t], "reduction": "literal"})
    u2 = add({"op": "union", "types": [literals[0], literals[1], literals[3]], "reduction": "literal"})
    add({"op": "union", "types": [literals[3], literals[0], literals[1], literals[0]], "reduction": "literal"})
    add({"op": "union", "types": [u2, string_t], "reduction": "literal"})
    add({"op": "union", "types": [u2, numbers[2]], "reduction": "literal"})
    add({"op": "union", "types": [u1, undefined_t], "reduction": "literal"})
    add({"op": "union", "types": [undefined_t, u1], "reduction": "literal"})
    add({"op": "union", "types": [u1, null_t, undefined_t], "reduction": "literal"})
    add({"op": "union", "types": [never_t, string_t], "reduction": "literal"})
    add({"op": "union", "types": [never_t, never_t], "reduction": "literal"})
    add({"op": "union", "types": [any_t, string_t], "reduction": "literal"})
    add({"op": "union", "types": [unknown_t, string_t], "reduction": "literal"})
    add({"op": "union", "types": [true_t, false_t], "reduction": "literal"})
    add({"op": "union", "types": [boolean_t, string_t], "reduction": "literal"})
    add({"op": "union", "types": [fresh_a, literals[0]], "reduction": "literal"})
    add({"op": "union", "types": [fresh_a, literals[0]], "reduction": "none"})
    add({"op": "union", "types": [numbers[0], numbers[2], numbers[10], numbers[5], numbers[8]], "reduction": "literal"})
    add({"op": "union", "types": [bigints[0], bigints[3], bigint_t], "reduction": "literal"})
    add({"op": "union", "types": [void_t, undefined_t], "reduction": "literal"})
    add({"op": "union", "types": [es_symbol_t, string_t, number_t, u1], "reduction": "literal"})
    # Aliases: an alias symbol changes the union key and the constituent order.
    alias_symbol = add({"op": "symbol", "flags": 1 << 19, "text_hex": hexed("Alias"), "check_flags": 0})
    aliased = add({"op": "union_alias", "types": [literals[0], literals[1]], "alias_symbol": alias_symbol, "alias_args": []})
    add({"op": "union_alias", "types": [literals[1], literals[0]], "alias_symbol": alias_symbol, "alias_args": []})
    add({"op": "union", "types": [aliased, literals[3]], "reduction": "literal"})
    add({"op": "union", "types": [aliased, literals[0]], "reduction": "literal"})
    # Type parameters, with and without symbols; unions of type parameters order by id.
    tp_symbol = add({"op": "symbol", "flags": 1 << 18, "text_hex": hexed("T"), "check_flags": 0})
    tp1 = add({"op": "type_parameter", "symbol": tp_symbol})
    tp2 = add({"op": "type_parameter", "symbol": None})
    add({"op": "union", "types": [tp2, tp1, string_t], "reduction": "literal"})
    # Anonymous object types: member ordering by name, optional/readonly, empty, with symbol.
    obj_symbol = add({"op": "symbol", "flags": 1 << 12, "text_hex": hexed("Obj"), "check_flags": 0})
    members = [
        {"text_hex": hexed("b"), "type": string_t, "optional": False, "readonly": False},
        {"text_hex": hexed("a"), "type": number_t, "optional": True, "readonly": False},
        {"text_hex": hexed("c"), "type": u1, "optional": False, "readonly": True},
        {"text_hex": hexed("10"), "type": literals[0], "optional": False, "readonly": False},
        {"text_hex": hexed("2"), "type": literals[1], "optional": False, "readonly": False},
        {"text_hex": "fe74797065", "type": any_t, "optional": False, "readonly": False},
    ]
    obj1 = add({"op": "anonymous", "symbol": obj_symbol, "members": members})
    obj2 = add({"op": "anonymous", "symbol": None, "members": members[:2]})
    add({"op": "anonymous", "symbol": None, "members": []})
    add({"op": "union", "types": [obj2, obj1, string_t], "reduction": "literal"})
    # Tuple targets: cache reuse, element kinds, readonly, and references to them.
    target_ab = add({"op": "tuple_target", "elements": ["required", "required"], "readonly": False})
    add({"op": "tuple_target", "elements": ["required", "required"], "readonly": False})
    target_ro = add({"op": "tuple_target", "elements": ["required", "required"], "readonly": True})
    target_opt = add({"op": "tuple_target", "elements": ["required", "optional", "optional"], "readonly": False})
    target_rest = add({"op": "tuple_target", "elements": ["required", "rest"], "readonly": False})
    target_var = add({"op": "tuple_target", "elements": ["variadic", "required"], "readonly": False})
    add({"op": "tuple_target", "elements": [], "readonly": False})
    tuple1 = add({"op": "tuple", "types": [string_t, number_t]})
    add({"op": "tuple", "types": [string_t, number_t]})
    tuple2 = add({"op": "tuple", "types": [number_t, string_t]})
    add({"op": "reference", "target": target_ab, "args": [literals[0], literals[1]]})
    add({"op": "reference", "target": target_ro, "args": [string_t, number_t]})
    add({"op": "reference", "target": target_ab, "args": [string_t, number_t]})
    add({"op": "union", "types": [tuple2, tuple1], "reduction": "literal"})
    add({"op": "tuple", "types": []})
    for target in (target_opt, target_rest, target_var):
        add({"op": "union", "types": [target, never_t], "reduction": "literal"})
    # Template literal types: literal folding, placeholders, interning, nested templates, unions.
    t1 = add({"op": "template", "texts": [hexed("a"), hexed("b")], "types": [string_t]})
    add({"op": "template", "texts": [hexed("a"), hexed("b")], "types": [string_t]})
    add({"op": "template", "texts": [hexed("x"), hexed("")], "types": [literals[0]]})
    add({"op": "template", "texts": [hexed(""), hexed("")], "types": [number_t]})
    add({"op": "template", "texts": [hexed("<"), hexed(">")], "types": [t1]})
    add({"op": "template", "texts": [hexed("p"), hexed("")], "types": [u2]})
    add({"op": "template", "texts": [hexed(""), hexed("!")], "types": [undefined_t]})
    add({"op": "template", "texts": [hexed(""), hexed("")], "types": [string_t]})
    add({"op": "template", "texts": [hexed(""), hexed("")], "types": [numbers[2]]})
    add({"op": "template", "texts": [hexed("q"), hexed("")], "types": [bigint_t]})
    add({"op": "template", "texts": [hexed(""), hexed("-"), hexed("")], "types": [string_t, number_t]})
    add({"op": "union", "types": [t1, string_t], "reduction": "literal"})
    # Signatures and synthetic nodes: checker-created declarations and embedded types.
    param = add({"op": "symbol", "flags": 1 << 0, "text_hex": hexed("x"), "check_flags": 0})
    add({"op": "call_signature", "parameters": [param], "return": string_t})
    add({"op": "call_signature", "parameters": [], "return": u1})
    add({"op": "synthetic_expression", "type": u1})
    add({"op": "synthetic_expression", "type": tuple1})
    return {"version": 1, "options": {"strict_null_checks": strict_null_checks, "exact_optional_property_types": False}, "actions": actions}


def requests():
    return {"version": 1, "traces": [trace(True), trace(False)]}


# P3 extends the Rust census beyond this frozen P1 constructor trace. Keep the
# exact inventory explicit: these families have no paired Go measurement here.
# They remain in Rust's total; no missing Go family is interpreted as zero.
P3_UNPAIRED_FAMILIES = {
    "mapped", "reverse_mapped", "instantiation_expression", "index", "indexed_access",
    "string_mapping", "substitution", "conditional", "mappers", "inference", "relations",
    "query_links", "conditional_roots", "variance", "late_members", "mapped_symbol_links",
    "signature_caches", "declarations", "program_indices", "resolution", "diagnostics",
}

# P4 operation state is charged even when this P1 trace never exercises it.
# Matching Go state will be added to the paired census at P7.
P4_UNPAIRED_FAMILIES = {
    "flow_analysis", "enum_links", "enum_relations", "body_check_state",
    "call_resolution", "deferred_checks", "iteration_cache", "evolving_arrays", "module_aliases",
}


def validate(request, rust, go):
    for name in ("roots", "named", "counts", "prefix_counts"):
        if not same_json_value(rust[name], go[name]):
            raise ValueError(f"storage-families {name} differ; raw results retained")
    if len(rust["roots"]) != len(request["actions"]):
        raise ValueError("observation count does not match the trace")
    for key in ("families", "types", "unavailable"):
        if key not in rust["census"] or key not in go["census"]:
            raise ValueError(f"census is missing {key}")
    unpaired = P3_UNPAIRED_FAMILIES | P4_UNPAIRED_FAMILIES
    if set(rust["census"]["families"]) != set(go["census"]["families"]) | unpaired:
        raise ValueError("census families differ from the P1 inventory plus named P3/P4 additions")
    if unpaired & set(go["census"]["families"]):
        raise ValueError("Go now measures a P3/P4 family; review the paired inventory")


def run_go(directory, request):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    upstream = verified_upstream()
    env = go_environment()
    raw = canonical(request) + b"\n"
    (directory / "requests.json").write_bytes(raw)
    overlay = {}
    for name, source in {"s08_families_bridge.go": ROOT / BRIDGE, "s08_families_driver_test.go": ROOT / DRIVER}.items():
        virtual = upstream / "tsc/internal/checker" / name
        if virtual.exists():
            raise ValueError(f"overlay would replace a source file: {virtual}")
        path = directory / name
        path.write_bytes(source.read_bytes())
        overlay[str(virtual)] = str(path)
    (directory / "overlay.json").write_bytes(canonical({"Replace": overlay}) + b"\n")
    env.update(S08_REQUESTS=str(directory / "requests.json"), S08_OUTPUT=str(directory / "observations.json"))
    cmd = ["go", "test", "-mod=readonly", "-trimpath", "-overlay", str(directory / "overlay.json"), "./internal/checker",
           "-run", "^TestS08StorageFamilies$", "-count=1", "-timeout=10m"]
    (directory / "command.json").write_bytes(canonical(cmd) + b"\n")
    with (directory / "stdout").open("wb") as stdout, (directory / "stderr").open("wb") as stderr:
        run = subprocess.run(cmd, cwd=upstream / "tsc", env=env, stdout=stdout, stderr=stderr, timeout=600)
    if run.returncode:
        raise ValueError(f"Go storage-families observation failed; retained at {directory}")
    verified_upstream()
    observed = strict_json_loads((directory / "observations.json").read_bytes())
    if observed["request_sha256"] != digest(raw):
        raise ValueError("Go observed a different trace")
    return observed


def build_rust(directory):
    build = command(["cargo", "build", "--release", "--locked", "-p", "ts_checker", "--features", "storage-pilot",
                     "--example", "storage_families", "--message-format=json"], cwd=ROOT)
    (directory / "cargo-build.ndjson").write_bytes(build)
    artifacts = [row for line in build.splitlines() if (row := strict_json_loads(line)).get("reason") == "compiler-artifact"
                 and row["target"]["name"] == "storage_families" and row.get("executable")]
    if len(artifacts) != 1 or "storage-pilot" not in artifacts[0]["features"]:
        raise ValueError("missing or ambiguous storage families compiler artifact")
    binary = directory / "rust-storage-families"
    shutil.copy2(artifacts[0]["executable"], binary)
    return binary


def capture(directory, freeze):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    spec = requests()
    frozen_path = ROOT / REQUESTS
    if freeze:
        frozen_path.write_bytes(json.dumps(spec, indent=1, sort_keys=True).encode() + b"\n")
    frozen = strict_json_loads(frozen_path.read_bytes())
    if not same_json_value(frozen, spec):
        raise ValueError("frozen storage-families trace does not match the generator; regenerate with --freeze")
    sources = {str(p.relative_to(ROOT)): digest(p.read_bytes()) for pattern in SOURCE_GLOBS for p in ROOT.glob(pattern) if p.is_file()}
    sources.update({name: digest((ROOT / name).read_bytes()) for name in SOURCES})
    binary = build_rust(directory)
    rust_version = command(["rustc", "--version", "--verbose"], cwd=ROOT).decode()
    traces = []
    frozen_observations = {"version": 1, "pin": strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"], "traces": []}
    for index, request in enumerate(spec["traces"]):
        go = run_go(directory / f"go-{index}", request)
        raw = canonical(request) + b"\n"
        rust_bytes = command([str(binary)], cwd=ROOT, data=raw)
        (directory / f"rust-observations-{index}.json").write_bytes(rust_bytes)
        rust = strict_json_loads(rust_bytes)
        validate(request, rust, go)
        traces.append({"options": request["options"], "actions": len(request["actions"]), "request_sha256": digest(raw),
                       "go": {key: go[key] for key in ("prefix_counts", "counts", "real_counts", "census", "allocator")},
                       "rust": {key: rust[key] for key in ("prefix_counts", "counts", "census", "allocator")}})
        frozen_observations["traces"].append({"options": request["options"], "request_sha256": digest(raw),
                                              "named": go["named"], "roots": go["roots"], "counts": go["counts"],
                                              "prefix_counts": go["prefix_counts"], "real_counts": go["real_counts"]})
        frozen_observations.update({key: go[key] for key in ("go", "goos", "goarch")})
    if any(digest((ROOT / name).read_bytes()) != value for name, value in sources.items()):
        raise ValueError("sources changed during capture")
    if freeze:
        (ROOT / OBSERVATIONS).write_bytes(json.dumps(frozen_observations, indent=1, sort_keys=True).encode() + b"\n")
    else:
        expected = strict_json_loads((ROOT / OBSERVATIONS).read_bytes())
        if not same_json_value(expected, frozen_observations):
            raise ValueError("frozen Go observations drifted; raw results retained")
    upstream = verified_upstream()
    report = {"version": 1, "parity": True, "pin": frozen_observations["pin"], "rust_version": rust_version,
              "rust_executable_sha256": digest(binary.read_bytes()), "source_inputs": sources,
              "native_sources": {p: digest((upstream / "tsc" / p).read_bytes()) for p in NATIVE_SOURCES},
              "runtime": {key: frozen_observations[key] for key in ("go", "goos", "goarch")},
              "scope": "P1 storage families over a checker prepared as NewChecker's type prefix; not the subset census, not E5",
              "unpaired_rust_families": sorted(P3_UNPAIRED_FAMILIES),
              "limitations": [
                  "Go structural bytes are struct sizes, slice and arena-chunk capacities and hinted-replica map allocations; Rust bytes are vector capacities, Arc allocations and hashbrown allocation sizes",
                  "Requested bytes are reported for NewChecker's prefix and for the trace's constructor calls separately: Go as TotalAlloc traffic and malloc calls, Rust as mimalloc requested allocations; neither interval includes observation or census work",
                  "The Go checker also creates globalThis's object type and autoArrayType in initializeChecker; the trace checker stops before it and real_counts records the difference",
                  "The checker AST arenas are unavailable on both sides and reported as a named gap",
                  "Rust additionally charges P3 stores and full shared text backings; the P1 Go observer has not been extended to those stores, so aggregate bytes are not a paired footprint result",
                  "No timing conclusion; the subset's type distribution is not modeled",
              ],
              "traces": traces}
    (directory / "report.json").write_bytes(canonical(report) + b"\n")
    print(json.dumps({key: value for key, value in report.items() if key not in ("source_inputs", "native_sources")}, sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--freeze", action="store_true", help="write the trace and Go observations into data/s08")
    args = parser.parse_args()
    capture(args.output, args.freeze)
