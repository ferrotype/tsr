#!/usr/bin/env python3
"""ADR 0010 diagnostic overlay; never an input of the canonical native oracle.

Builds from the verified pin and refuses patch drift. Every replacement source,
toolchain argument and executable is retained and digested. Diagnostic hooks
observe existing fields and natural id assignment; they do not request ids.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from s04 import go_environment, verified_upstream
from s04_common import command, strict_json_loads
from s08_oracle import ROOT, canonical, digest

INPUTS = ROOT / "tools/phase2/order-trace"


def replace_once(text, before, after):
    if text.count(before) != 1:
        raise ValueError("diagnostic overlay anchor drift: " + before[:90])
    return text.replace(before, after, 1)


def replacements(upstream):
    """Replace virtual pin sources; no edits in the upstream checkout."""
    result = {}
    for package, name in (("ast", "ast_trace.go"), ("checker", "checker_trace.go"),
                          ("checker", "driver_test.go")):
        result[f"tsc/internal/{package}/c2_{name}"] = (INPUTS / name).read_text()
    for package in ("checker", "binder"):
        relative = f"tsc/internal/{package}/{package}.go"
        source = (upstream / relative).read_text()
        anchor = ("result.Flags = flags | ast.SymbolFlagsTransient\n\tresult.Name = name\n\treturn result"
                  if package == "checker" else "result.Flags = flags\n\tresult.Name = name\n\treturn result")
        source = replace_once(source, anchor, anchor.replace("\n\treturn result", "\n\tast.C2TraceSymbolBirth(result)\n\treturn result"))
        if package == "checker":
            source = replace_once(source, "\tt.data = data\n\tif c.tracer != nil {", "\tt.data = data\n\tc2TraceTypeBirth(t)\n\tif c.tracer != nil {")
            source = replace_once(source, "\t\tsym.Name = e.primitive\n\t\tresult[e.builtin] = sym", "\t\tsym.Name = e.primitive\n\t\tast.C2TraceSymbolBirth(sym)\n\t\tresult[e.builtin] = sym")
            source = replace_once(source, "\t\tslices.SortStableFunc(types, CompareTypes)",
                                  '\t\tc2TraceTypeSort("sort_begin", types)\n\t\tslices.SortStableFunc(types, CompareTypes)\n\t\tc2TraceTypeSort("sort_end", types)')
        result[relative] = source
    relative = "tsc/internal/binder/nameresolver.go"
    source = (upstream / relative).read_text()
    anchor = 'r.ArgumentsSymbol = &ast.Symbol{Name: "arguments", Flags: ast.SymbolFlagsProperty | ast.SymbolFlagsTransient}'
    result[relative] = replace_once(source, anchor, anchor + "\n\t\tast.C2TraceSymbolBirth(r.ArgumentsSymbol)")
    relative = "tsc/internal/ast/utilities.go"
    source = (upstream / relative).read_text()
    result[relative] = replace_once(source,
        "if !symbol.id.CompareAndSwap(0, id) {\n\t\t\tid = symbol.id.Load()\n\t\t}",
        "if !symbol.id.CompareAndSwap(0, id) {\n\t\t\tid = symbol.id.Load()\n\t\t} else {\n\t\t\tC2TraceSymbolAssigned(symbol, id)\n\t\t}")
    relative = "tsc/internal/checker/utilities.go"
    source = (upstream / relative).read_text()
    source = replace_once(source, "\tslices.SortFunc(symbols, c.compareSymbols)",
        '\tc2TraceSymbolSort("sort_begin", symbols)\n\tslices.SortFunc(symbols, c.compareSymbols)\n\tc2TraceSymbolSort("sort_end", symbols)')
    source = replace_once(source, "return int(ast.GetSymbolId(s1)) - int(ast.GetSymbolId(s2))",
        "result := int(ast.GetSymbolId(s1)) - int(ast.GetSymbolId(s2))\n\tc2TraceSymbolFallback(s1,s2,result)\n\treturn result")
    source = replace_once(source, "return int(t1.id) - int(t2.id)",
        "result := int(t1.id) - int(t2.id)\n\tc2TraceTypeFallback(t1,t2,result)\n\treturn result")
    result[relative] = source
    return result


def build_native(output):
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    initial_inputs = native_inputs()
    upstream = verified_upstream()
    env = go_environment()
    sources = replacements(upstream)
    mapping = {}
    for relative, source in sources.items():
        target = output / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(source)
        mapping[str(upstream / relative)] = str(target)
    command(["gofmt", "-w", *mapping.values()], cwd=upstream / "tsc", env=env)
    overlay = output / "overlay.json"
    overlay.write_bytes(canonical({"Replace": mapping}) + b"\n")
    executable = output / "native-order-trace"
    args = ["go", "test", "-c", "-trimpath", "-mod=readonly", "-overlay", str(overlay),
            "-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath=/c2-diagnostic-unmatched-prefix",
            "-o", str(executable), "./internal/checker"]
    stdout = command(args, cwd=upstream / "tsc", env=env)
    (output / "build.stdout").write_bytes(stdout)
    report = {"version":1, "pin":strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"],
              "command":args, "go":command(["go", "version"], cwd=upstream / "tsc", env=env).decode().strip(),
              "toolchain_local":env["GOTOOLCHAIN"] == "local", "executable":str(executable),
              "executable_sha256":digest(executable.read_bytes()), "overlay_sha256":digest(overlay.read_bytes()),
              "replacements":{relative:digest((output / relative).read_bytes()) for relative in sources},
              "inputs":native_inputs()}
    if initial_inputs != native_inputs():
        raise ValueError("native trace inputs changed during build")
    (output / "build.json").write_bytes(canonical(report) + b"\n")
    return report


def read(path):
    return strict_json_loads(Path(path).read_bytes())


def native_inputs():
    paths = [Path(__file__), *sorted(INPUTS.glob("*.go")), INPUTS / "witnesses.json"]
    paths += [ROOT / p for p in ("data/upstream.json", "data/s04/toolchains.toml",
                                "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py",
                                "scripts/tracking-bootstrap.py", "scripts/s08_oracle.py")]
    return {str(p.relative_to(ROOT)): digest(p.read_bytes()) for p in paths}


def rust_inputs():
    import phase2_corpus
    return phase2_corpus.sources() | native_inputs() | {
        "data/phase2/c2-order-traces.json": digest((ROOT / "data/phase2/c2-order-traces.json").read_bytes()),
        "scripts/s07_benchmark.py": digest((ROOT / "scripts/s07_benchmark.py").read_bytes()),
    }


def build_rust(output, release=False):
    from s07_benchmark import cargo_executable, native_environment
    import shutil
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    initial = rust_inputs()
    manifest = ROOT / "crates/tsr_compiler/Cargo.toml"
    modes = {"ordinary": ["relation-probe"], "diagnostic": ["creation-trace", "relation-probe"]}
    binaries = {}
    for mode, features in modes.items():
        args = ["cargo", "build", "--locked", "--example", "c2_order_trace", "--features", ",".join(features),
                "--message-format=json", "--manifest-path", str(manifest)]
        if release:
            args.append("--release")
        messages = command(args, cwd=ROOT, env=native_environment()).decode()
        binary = cargo_executable(messages, manifest, "c2_order_trace", "example", features, release=release)
        copied = output / mode
        shutil.copy2(binary, copied)
        (output / (mode + ".build.ndjson")).write_text(messages)
        binaries[mode] = {"path": str(copied), "sha256": digest(copied.read_bytes()), "features": features,
                          "command": args, "messages_sha256": digest(messages.encode())}
    if initial != rust_inputs():
        raise ValueError("Rust trace inputs changed during build")
    result = {"version": 1, "inputs": initial, "release": release, "binaries": binaries,
              "rustc": command(["rustc", "--version"], cwd=ROOT).decode().strip()}
    (output / "build.json").write_bytes(canonical(result) + b"\n")
    return result


def checked_native(directory):
    directory = directory.resolve()
    report = read(directory / "build.json")
    if report["inputs"] != native_inputs() or report["pin"] != read(ROOT / "data/upstream.json")["pin"]:
        raise ValueError("native trace build inputs are stale")
    if report["toolchain_local"] is not True:
        raise ValueError("native trace build did not pin the Go toolchain")
    for relative, expected in report["replacements"].items():
        path = directory / relative
        if not path.resolve().is_relative_to(directory) or digest(path.read_bytes()) != expected:
            raise ValueError("native trace replacement drift")
    if digest((directory / "overlay.json").read_bytes()) != report["overlay_sha256"]:
        raise ValueError("native overlay drift")
    binary = Path(report["executable"])
    if digest(binary.read_bytes()) != report["executable_sha256"]:
        raise ValueError("native trace executable drift")
    return report, binary


def checked_rust(directory):
    report = read(directory / "build.json")
    if report["inputs"] != rust_inputs():
        raise ValueError("Rust trace build inputs are stale")
    for mode, entry in report["binaries"].items():
        if digest(Path(entry["path"]).read_bytes()) != entry["sha256"]:
            raise ValueError("Rust trace executable drift")
        if digest((directory / (mode + ".build.ndjson")).read_bytes()) != entry["messages_sha256"]:
            raise ValueError("Rust trace Cargo artifact report drift")
    if set(report["binaries"]) != {"ordinary", "diagnostic"}:
        raise ValueError("Rust trace must compare ordinary and diagnostic executables")
    return report


def witness_manifest():
    manifest = read(INPUTS / "witnesses.json")
    if manifest["pin"] != read(ROOT / "data/upstream.json")["pin"] or manifest["version"] != 1:
        raise ValueError("trace witness pin drift")
    expected = {"intrinsic-order", "declarationless-duplicate-symbols", "equal-first-declaration-symbols", "reverse-mapped-order"}
    if len(manifest["witnesses"]) != 4 or {w["id"] for w in manifest["witnesses"]} != expected:
        raise ValueError("trace witness inventory differs")
    return manifest


def run_witness(binary, request, output, tracing, native=False):
    env = dict(os.environ, C2_ORDER_REQUEST=str(request.resolve()), C2_ORDER_OUTPUT=str(output.resolve()),
               C2_ORDER_TRACE="1" if tracing else "0", TS_TEST_PROGRAM_SINGLE_THREADED="true")
    args = [str(binary)] + (["-test.run=^TestC2OrderTrace$", "-test.count=1"] if native else [])
    stdout = command(args, cwd=verified_upstream() / "tsc" if native else ROOT, env=env)
    output.with_suffix(".stdout").write_bytes(stdout)
    return read(output)


def validate_trace(witness, observed):
    """Authenticate every referenced birth/assignment, and the actual witness tie.

    Numeric ids and allocation sequences intentionally are not compared between
    runtimes. Labels of the queried source types/properties identify operands;
    the final sort permutation and normalized pair direction carry the contract.
    """
    events = observed["trace"]
    if not isinstance(events, list):
        raise ValueError("trace events must be ordered")
    if not events:
        raise ValueError("missing creation trace")
    births, assigned, families, sorts, ties = {}, {}, set(), [], set()
    kind = "symbol" if witness["request"].get("property") else "type"
    operands = observed["order"]["before"] if kind == "symbol" else observed["types"]
    def key(value, kind):
        token = value.get("token")
        if type(token) is not int or token <= 0:
            raise ValueError("unobserved trace origin")
        return (kind, value.get("owner", 0), token)
    labels = {key(value, kind): i for i, value in enumerate(operands)}
    if len(labels) != len(witness["request"]["queries"]):
        raise ValueError("witness operands collapsed before comparison")
    def snapshot(value, kind):
        if not isinstance(value, dict) or "unobserved" in value:
            raise ValueError("unobserved comparison operand")
        ident = key(value, kind)
        if ident not in births:
            raise ValueError("comparison precedes observed birth")
        semantic_id = value.get("semantic_id")
        if type(semantic_id) is not int or semantic_id < 0:
            raise ValueError("invalid semantic identity")
        if semantic_id != assigned.get(ident, 0):
            raise ValueError("semantic identity lacks its natural assignment")
        if kind == "type" and value.get("symbol") is not None:
            snapshot(value["symbol"], "symbol")
        if kind == "symbol" and not isinstance(value.get("declarations"), list):
            raise ValueError("unobserved declarations")
        return ident
    for event in events:
        event_kind = event.get("kind")
        if event_kind not in ("type", "symbol"):
            raise ValueError("unknown trace object kind")
        operation = event.get("event")
        if operation == "birth":
            ident = key(event, event_kind)
            origin = event.get("origin", {})
            sites = origin.get("stack", [origin])
            if ident in births or not sites or any(not site.get("file") or not isinstance(site.get("line"), int) or site["line"] <= 0 for site in sites):
                raise ValueError("invalid or duplicate birth origin")
            births[ident] = event
            assigned[ident] = event.get("semantic_id", 0)
            if type(assigned[ident]) is not int or assigned[ident] < 0 or (event_kind == "type" and assigned[ident] != event["token"]):
                raise ValueError("invalid type birth identity")
            if event_kind == "symbol" and assigned[ident] != 0:
                raise ValueError("symbol birth already has an unobserved semantic id")
        elif operation == "id_assignment":
            ident = key(event, event_kind)
            if event_kind != "symbol" or ident not in births or assigned.get(ident) != 0 or type(event.get("semantic_id")) is not int or event["semantic_id"] <= 0:
                raise ValueError("invalid natural symbol-id assignment")
            assigned[ident] = event["semantic_id"]
        elif operation in ("sort_begin", "sort_end"):
            ids = [snapshot(value, event_kind) for value in event["values"]]
            if operation == "sort_begin":
                sorts.append((event_kind, event["operation"], ids))
            else:
                if not sorts:
                    raise ValueError("sort output without input")
                prior_kind, prior_operation, prior_ids = sorts.pop()
                if (prior_kind, prior_operation) != (event_kind, event["operation"]) or sorted(prior_ids) != sorted(ids):
                    raise ValueError("sort did not retain its input operands")
        elif operation == "fallback":
            left, right = snapshot(event["left"], event_kind), snapshot(event["right"], event_kind)
            delta = event["left"]["semantic_id"] - event["right"]["semantic_id"]
            if type(event.get("sign")) is not int or event["sign"] != (delta > 0) - (delta < 0):
                raise ValueError("fallback sign differs from ids actually used")
            branch = event.get("branch")
            if branch not in ("final_type_id", "declarationless", "equal_first_declaration"):
                raise ValueError("unclassified comparator fallback")
            if event_kind == "symbol":
                a, b = event["left"], event["right"]
                if a["name"] != b["name"] or (branch == "declarationless" and (a["declarations"] or b["declarations"])) or (branch == "equal_first_declaration" and (not a["declarations"] or not b["declarations"] or a["declarations"][0] != b["declarations"][0])):
                    raise ValueError("symbol operands do not witness the reported tie")
            families.add((event_kind, branch, event["left"]["flags"], event["right"]["flags"],
                          event["left"].get("object_flags", 0), event["right"].get("object_flags", 0)))
            if event_kind == kind and left in labels and right in labels and left != right and branch == witness["required_branch"]:
                a, b = labels[left], labels[right]
                ties.add((min(a,b), max(a,b), event["sign"] if a < b else -event["sign"]))
        else:
            raise ValueError("unknown creation trace event")
    if sorts or not ties:
        raise ValueError("source witness did not complete its actual comparator fallback")
    if witness["id"] == "reverse-mapped-order" and not all(v["object_flags"] & 1024 for v in operands):
        raise ValueError("reverse-mapped witness is not reverse mapped")
    return {"ties": sorted(ties), "families": sorted(families), "births":len(births), "events":len(events)}


def freeze_native(native, output):
    build, binary = checked_native(native)
    manifest = witness_manifest()
    run = native / "witnesses"
    run.mkdir(exist_ok=False)
    rows = []
    for witness in manifest["witnesses"]:
        request = run / (witness["id"] + ".request.json")
        request.write_bytes(canonical(witness["request"]))
        on = run_witness(binary, request, run / (witness["id"] + ".on.json"), True, native=True)
        off = run_witness(binary, request, run / (witness["id"] + ".off.json"), False, native=True)
        summary = validate_trace(witness, on)
        if off["trace"] or on["ordinary"] != off["ordinary"]:
            raise ValueError("native tracing changed ordinary output")
        rows.append({"id":witness["id"], "on":on, "off":off, "summary":summary})
    checked_native(native)
    result = {"version":1, "manifest":manifest, "build":build, "rows":rows}
    output.write_bytes(canonical(result) + b"\n")
    return {"native_witnesses":len(rows), "sha256":digest(output.read_bytes())}


def verify_frozen():
    frozen = read(ROOT / "data/phase2/c2-order-traces.json")
    if frozen["manifest"] != witness_manifest() or frozen["build"]["inputs"] != native_inputs():
        raise ValueError("frozen native creation traces are stale")
    rows = frozen["rows"]
    if [row["id"] for row in rows] != [row["id"] for row in frozen["manifest"]["witnesses"]]:
        raise ValueError("frozen trace row inventory mismatch")
    for witness, row in zip(frozen["manifest"]["witnesses"], rows, strict=True):
        if canonical(validate_trace(witness, row["on"])) != canonical(row["summary"]) or row["off"]["trace"] or row["on"]["ordinary"] != row["off"]["ordinary"]:
            raise ValueError("invalid frozen native trace")
    return frozen


def capture(rust, output):
    frozen = verify_frozen()
    build = checked_rust(rust)
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    results = []
    for witness, native in zip(frozen["manifest"]["witnesses"], frozen["rows"], strict=True):
        request = output / (witness["id"] + ".request.json")
        request.write_bytes(canonical(witness["request"]))
        row = {"id":witness["id"], "runs":{}}
        for mode, tracing in (("ordinary", False), ("diagnostic", False), ("diagnostic", True)):
            name = mode + ("-on" if tracing else "-off")
            path = output / (witness["id"] + "." + name + ".json")
            observed = run_witness(Path(build["binaries"][mode]["path"]), request, path, tracing)
            row["runs"][name] = {"path":path.name,"sha256":digest(path.read_bytes())}
            if observed["ordinary"] != native["on"]["ordinary"]:
                raise ValueError(f"{witness['id']}: ordinary output differs ({name})")
            if tracing:
                summary = validate_trace(witness, observed)
                if any(canonical(summary[key]) != canonical(native["summary"][key]) for key in ("ties", "families")):
                    raise ValueError(f"{witness['id']}: source-labeled fallback ordering differs")
                row["summary"] = summary
            elif observed["trace"]:
                raise ValueError("disabled tracing emitted events")
        results.append(row)
    checked_rust(rust)
    report = {"version":1, "native_sha256":digest((ROOT / "data/phase2/c2-order-traces.json").read_bytes()),
              "build":build, "rows":results}
    (output / "capture.json").write_bytes(canonical(report) + b"\n")
    return verify(output)


def verify(directory):
    frozen = verify_frozen()
    report = read(directory / "capture.json")
    if report["native_sha256"] != digest((ROOT / "data/phase2/c2-order-traces.json").read_bytes()) or report["build"]["inputs"] != rust_inputs():
        raise ValueError("creation-trace capture is stale")
    if [r["id"] for r in report["rows"]] != [r["id"] for r in frozen["rows"]]:
        raise ValueError("trace capture row schedule differs")
    for witness, native, row in zip(frozen["manifest"]["witnesses"], frozen["rows"], report["rows"], strict=True):
        if set(row["runs"]) != {"ordinary-off", "diagnostic-off", "diagnostic-on"}:
            raise ValueError("trace mode inventory differs")
        for name, entry in row["runs"].items():
            path = directory / entry["path"]
            if not path.resolve().is_relative_to(directory.resolve()) or digest(path.read_bytes()) != entry["sha256"]:
                raise ValueError("trace raw observation drift")
            observed = read(path)
            if observed["ordinary"] != native["on"]["ordinary"]:
                raise ValueError("trace ordinary output mismatch")
            if name == "diagnostic-on":
                summary = validate_trace(witness, observed)
                if canonical(summary) != canonical(row["summary"]) or any(canonical(summary[key]) != canonical(native["summary"][key]) for key in ("ties", "families")):
                    raise ValueError("trace fallback ordering mismatch")
            elif observed["trace"]:
                raise ValueError("disabled trace is nonempty")
    return {"valid":True, "witnesses":len(report["rows"]), "release":report["build"]["release"],
            "capture_sha256":digest((directory / "capture.json").read_bytes())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build-native", "freeze-native", "build-rust", "capture", "verify"))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--native", type=Path)
    parser.add_argument("--rust", type=Path)
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()
    if args.command == "build-native":
        result = build_native(args.output)
    elif args.command == "freeze-native":
        result = freeze_native(args.native, args.output)
    elif args.command == "build-rust":
        result = build_rust(args.output, args.release)
    elif args.command == "capture":
        result = capture(args.rust, args.output)
    else:
        result = verify(args.output)
    print(json.dumps(result, indent=1))


if __name__ == "__main__":
    main()
