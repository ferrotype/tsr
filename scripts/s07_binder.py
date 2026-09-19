#!/usr/bin/env python3
"""Strict independent parser/binder graph observations; smoke runs never publish parity."""
import argparse
from contextlib import ExitStack
from functools import lru_cache
import hashlib
import json
from pathlib import Path
import shutil
import sys

from s04 import verified_upstream
from s04_common import command, strict_json_loads
from s04 import panic_class as leaf_panic_class
from s06_build import ROOT, oracle_export
from s06_process import Process as S06Process
from s06_protocol import (canonical, exact_keys, hex_bytes, integer, text,
                          validate_request as validate_parser_request)

OPERATIONS = ["parse", "parsed_graph", "bind", "bound_graph", "repeat_bind", "repeated_graph"]
GRAPH_STAGES = {"parsed_graph", "bound_graph", "repeated_graph"}
KINDS = ("node", "list", "nodes", "texts", "symbol", "table", "declarations", "flow", "flow_list")
FIELDS = {
    "source": "root node_count text_count identifier_count symbol_count script_kind language_variant declaration_file is_bound common_js external_module global_exports patterns diagnostics",
    "node": "id kind flags pos end parent payload fields jsdoc symbol local_symbol locals next_container flow_node end_flow_node return_flow_node fallthrough_flow_node",
    "list": "id pos end nodes missing modifier_flags is_modifier",
    "nodes": "id values backing_group backing_start", "texts": "id values_hex backing_group backing_start",
    "symbol": "id flags check_flags name declarations value_declaration members exports parent export_symbol",
    "table": "id entries", "declarations": "id values capacity capacity_values backing_group backing_start",
    "flow": "id flags data synthetic antecedent antecedents", "flow_list": "id flow next",
    "counts": " ".join(KINDS),
}
DIAGNOSTIC_FIELDS = "file pos end code category source_hex key_hex text_hex args_hex chain related unnecessary deprecated skipped"


def convert_request(request):
    validate_parser_request(request)
    if request["op"] != "parse":
        raise ValueError("S07 primary accepts parser-entry requests only")
    return {**request, "op": "bind", "operations": list(OPERATIONS)}


def validate_request(request):
    original = {**request, "op": "parse", "operations": ["parse", "node_index_before", "encode_source_file", "node_index_after"]}
    validate_parser_request(original)
    if request["op"] != "bind" or request["operations"] != OPERATIONS:
        raise ValueError("invalid S07 operation sequence")


def uint(value):
    return integer(value, 0, 2**63-1, "unsigned observation")


def boolean(value):
    if type(value) is not bool:
        raise ValueError("observation requires boolean")


def sequence(value):
    if type(value) is not list:
        raise ValueError("observation requires array")
    return value


def name_identity(value, ref):
    exact_keys(value, ("raw_hex", "identity"), "symbol name")
    raw = hex_bytes(value["raw_hex"], "symbol name bytes")
    identity = value["identity"]
    if identity is None:
        return
    exact_keys(identity, ("kind", "ref", "prefix_hex", "suffix_hex"), "identity-derived name")
    if identity["kind"] not in ("node", "symbol"):
        raise ValueError("unsupported name identity domain")
    ref(identity["kind"], identity["ref"], nullable=False)
    prefix = hex_bytes(identity["prefix_hex"], "name prefix")
    suffix = hex_bytes(identity["suffix_hex"], "name suffix")
    if not raw.startswith(prefix) or not raw.endswith(suffix):
        raise ValueError("name identity fragments do not reconstruct the literal bytes")
    end = len(raw)-len(suffix) if suffix else len(raw)
    number = raw[len(prefix):end]
    if not number or not number.isdigit() or number[:1] == b"0":
        raise ValueError("name identity component is not canonical positive decimal")
    if identity["kind"] == "node":
        if not prefix.startswith(b'\xfe"') or not prefix.endswith(b'"pattern@') or suffix:
            raise ValueError("invalid ambient pattern identity delimiters")
    elif prefix != b"\xfe#" or not suffix.startswith(b"@#"):
        raise ValueError("invalid private class identity delimiters")


def comparable(value):
    """Only the two source-defined numeric identity components are substituted."""
    if type(value) is dict:
        if set(value) == {"raw_hex", "identity"} and value["identity"] is not None:
            return {"identity": comparable(value["identity"])}
        return {key: comparable(item) for key, item in value.items()}
    if type(value) is list:
        return [comparable(item) for item in value]
    return value


@lru_cache(maxsize=1)
def syntax_schema():
    schema = strict_json_loads((ROOT / "data/s03/schema/ast.json").read_bytes())
    sys.path.insert(0, str(ROOT / "tools/s07/binder"))
    from generate_syntax import category
    return {node["name"]: [(f["name"], category(f["type"])) for f in node["fields"]
                           if not f["goOnly"] and not f["noGo"] and f["name"] != "Flags"]
            for node in schema["nodes"]}

class GraphValidator:
    def __init__(self):
        self.counts = dict.fromkeys(KINDS, 0)
        self.references = []
        self.source = False
        self.closed = False
        self.digest = hashlib.sha256()
        self.syntax = syntax_schema()
        self.aliases = {}

    def ref(self, kind, value, *, nullable=True):
        uint(value)
        if not nullable and value == 0:
            raise ValueError("nonnull graph edge is nil")
        self.references.append((kind, value))

    def diagnostic(self, value):
        if value is None:
            return
        exact_keys(value, DIAGNOSTIC_FIELDS.split(), "diagnostic")
        self.ref("node", value["file"])
        for field in ("pos", "end", "code", "category"):
            integer(value[field], -2**31, 2**31-1, "diagnostic integer")
        for field in ("source_hex", "key_hex", "text_hex"):
            hex_bytes(value[field], "diagnostic text")
        for item in sequence(value["args_hex"]):
            hex_bytes(item, "diagnostic argument")
        for field in ("chain", "related"):
            for item in sequence(value[field]):
                self.diagnostic(item)
        for field in ("unnecessary", "deprecated", "skipped"):
            boolean(value[field])

    def accept(self, kind, value):
        if self.closed or kind not in FIELDS:
            raise ValueError("extra or unknown graph record")
        exact_keys(value, FIELDS[kind].split(), "graph " + kind)
        self.digest.update(canonical([kind, value])+b"\n")
        if kind == "source":
            if self.source or any(self.counts.values()):
                raise ValueError("duplicate or reordered graph source")
            self.source = True
            for field, domain in (("root", "node"), ("common_js", "node"), ("external_module", "node"), ("global_exports", "table")):
                self.ref(domain, value[field], nullable=field != "root")
            for field in ("node_count", "text_count", "identifier_count", "symbol_count"):
                uint(value[field])
            for field in ("script_kind", "language_variant"):
                integer(value[field], -2**31, 2**31-1, field)
            for field in ("is_bound", "declaration_file"):
                boolean(value[field])
            for item in sequence(value["patterns"]):
                if item is None:
                    continue
                exact_keys(item, ("text_hex", "star_index", "symbol"), "ambient pattern")
                raw = hex_bytes(item["text_hex"], "ambient pattern bytes")
                integer(item["star_index"], -1, len(raw), "star index")
                self.ref("symbol", item["symbol"])
            indices = dict.fromkeys(("parse", "js", "jsdoc", "bind"), 0)
            previous = -1
            for item in sequence(value["diagnostics"]):
                if type(item) is not list or len(item) != 3 or item[0] not in indices:
                    raise ValueError("invalid diagnostic collection")
                order = list(indices).index(item[0])
                if order < previous or item[1] != indices[item[0]]:
                    raise ValueError("reordered or duplicate diagnostic")
                previous = order
                indices[item[0]] += 1
                self.diagnostic(item[2])
            return
        if not self.source:
            raise ValueError("graph record precedes its source")
        if kind == "counts":
            if value != self.counts or any(type(item) is not int for item in value.values()):
                raise ValueError("graph cardinality mismatch")
            for domain, reference in self.references:
                if reference > self.counts[domain]:
                    raise ValueError(f"dangling {domain} reference {reference}")
            for spans in self.aliases.values():
                spans.sort(key=lambda item: item[0])
                if spans[0][0] != 0:
                    raise ValueError("backing component lacks its zero origin")
                contents = []
                for start, values in spans:
                    if contents and start >= len(contents):
                        raise ValueError("disconnected ranges falsely share a backing group")
                    overlap = min(len(values), len(contents)-start)
                    if values[:overlap] != contents[start:start+overlap]:
                        raise ValueError("aliased ranges have inconsistent visible contents")
                    contents.extend(values[overlap:])
            self.closed = True
            return
        self.counts[kind] += 1
        if type(value["id"]) is not int or value["id"] != self.counts[kind]:
            raise ValueError("duplicate/missing/reordered canonical graph identity")
        if kind in ("nodes", "texts", "declarations"):
            group = uint(value["backing_group"]); start = uint(value["backing_start"])
            values = sequence(value[{"nodes": "values", "texts": "values_hex", "declarations": "capacity_values"}[kind]])
            if not values:
                if group or start:
                    raise ValueError("zero visible range has backing identity")
            else:
                if group == 0:
                    raise ValueError("nonempty visible range lacks backing identity")
                if group not in self.aliases and group != len(self.aliases)+1:
                    raise ValueError("noncanonical backing group order")
                self.aliases.setdefault(group, []).append((start, values))
        if kind == "node":
            integer(value["kind"], -32768, 32767, "node kind")
            integer(value["flags"], 0, 2**32-1, "node flags")
            for field in ("pos", "end"):
                integer(value[field], -2**31, 2**31-1, field)
            domains = {"parent": "node", "symbol": "symbol", "local_symbol": "symbol", "locals": "table", "next_container": "node",
                       "flow_node": "flow", "end_flow_node": "flow", "return_flow_node": "flow", "fallthrough_flow_node": "flow"}
            for field, domain in domains.items():
                self.ref(domain, value[field])
            for item in sequence(value["jsdoc"]):
                self.ref("node", item, nullable=False)
            fields = sequence(value["fields"])
            expected = self.syntax.get(value["payload"])
            if expected is None or len(fields) != len(expected):
                raise ValueError("unknown or incomplete syntax payload")
            for item, (field, category) in zip(fields, expected):
                if type(item) is not list or len(item) != 3 or item[:2] != [field, category]:
                    raise ValueError("reordered/missing syntax payload field")
                v = item[2]
                if category in ("node", "list", "modifiers", "nodes", "strings"):
                    self.ref({"modifiers": "list", "strings": "texts"}.get(category, category), v)
                elif category == "text":
                    hex_bytes(v, "syntax text")
                elif category == "bool":
                    boolean(v)
                elif category in ("kind", "number"):
                    integer(v, -2**31, 2**31-1, "syntax number")
                elif v is not None:
                    raise ValueError("checker type escaped the binder observation domain")
        elif kind == "list":
            for field in ("pos", "end"):
                integer(value[field], -2**31, 2**31-1, field)
            self.ref("nodes", value["nodes"])
            for field in ("missing", "is_modifier"):
                boolean(value[field])
            integer(value["modifier_flags"], 0, 2**32-1, "modifier flags")
        elif kind in ("nodes", "declarations"):
            for item in sequence(value["values"]):
                self.ref("node", item)
            if kind == "declarations":
                integer(value["capacity"], len(value["values"]), 2**32-1, "declaration capacity")
                capacity = sequence(value["capacity_values"])
                if len(capacity) != value["capacity"] or capacity[:len(value["values"])] != value["values"]:
                    raise ValueError("inconsistent declaration capacity contents")
                for item in capacity:
                    self.ref("node", item)
        elif kind == "texts":
            for item in sequence(value["values_hex"]):
                hex_bytes(item, "text slice item")
        elif kind == "symbol":
            for field in ("flags", "check_flags"):
                integer(value[field], 0, 2**32-1, "symbol flags")
            name_identity(value["name"], self.ref)
            for field, domain in (("declarations", "declarations"), ("value_declaration", "node"), ("members", "table"), ("exports", "table"), ("parent", "symbol"), ("export_symbol", "symbol")):
                self.ref(domain, value[field])
        elif kind == "table":
            previous = None
            for item in sequence(value["entries"]):
                if type(item) is not list or len(item) != 2:
                    raise ValueError("invalid symbol table edge")
                name_identity(item[0], self.ref)
                key = bytes.fromhex(item[0]["raw_hex"])
                if previous is not None and key <= previous:
                    raise ValueError("symbol table keys are not unique raw-byte order")
                previous = key
                self.ref("symbol", item[1])
        elif kind == "flow":
            integer(value["flags"], 0, 2**32-1, "flow flags")
            data = value["data"]
            synthetic = value["synthetic"]
            if data is not None and data[0] in ("switch", "reduce"):
                exact_keys(synthetic, ("owner_flow", "payload", "kind", "flags", "pos", "end", "parent"), "synthetic flow identity")
                if synthetic["owner_flow"] != value["id"] or synthetic["payload"] != data[0]:
                    raise ValueError("synthetic payload owner/discriminant mismatch")
                for field in ("kind", "flags", "pos", "end"):
                    integer(synthetic[field], -2**31, 2**32-1, "synthetic header")
                self.ref("node", synthetic["parent"])
            elif synthetic is not None:
                raise ValueError("ordinary flow data has synthetic identity")
            if data is not None:
                sequence(data)
                if not data or data[0] not in ("ast", "switch", "reduce") or len(data) != {"ast": 2, "switch": 4, "reduce": 3}[data[0]]:
                    raise ValueError("invalid synthetic flow payload")
                if data[0] == "ast":
                    self.ref("node", data[1], nullable=False)
                elif data[0] == "switch":
                    self.ref("node", data[1])
                    for item in data[2:]:
                        integer(item, -2**31, 2**31-1, "switch clause index")
                else:
                    self.ref("flow", data[1]); self.ref("flow_list", data[2])
            self.ref("flow", value["antecedent"])
            self.ref("flow_list", value["antecedents"])
        elif kind == "flow_list":
            self.ref("flow", value["flow"]); self.ref("flow_list", value["next"])


class Stream:
    def __init__(self, request):
        self.request = request
        self.begun = self.ended = self.failed = False
        self.seq = self.stage_index = 0
        self.graph = None
        self.graph_digests = {}
        self.outcomes = []
        self.fragment = None
        self.logical_seq = 0

    def accept(self, record):
        if self.ended:
            raise ValueError("record after terminal end")
        if type(record.get("version")) is not int or record["version"] != 1 or record.get("id") != self.request["id"]:
            raise ValueError("wrong stream request identity")
        tag = record.get("tag")
        if tag == "begin":
            exact_keys(record, ("version", "id", "tag", "op"), "begin")
            if self.begun or record["op"] != "bind":
                raise ValueError("duplicate or wrong begin")
            self.begun = True
            return record
        if not self.begun:
            raise ValueError("record before begin")
        if tag == "end":
            exact_keys(record, ("version", "id", "tag", "observations", "stages"), "end")
            if type(record["observations"]) is not int or type(record["stages"]) is not int or record["observations"] != self.seq or record["stages"] != self.stage_index or not (self.failed or self.stage_index == len(OPERATIONS)):
                raise ValueError("incomplete or inconsistent terminal end")
            self.ended = True
            return {**record, "observations": self.logical_seq}
        if self.failed or self.stage_index == len(OPERATIONS):
            raise ValueError("record after stage termination")
        stage = OPERATIONS[self.stage_index]
        if record.get("stage") != stage:
            raise ValueError("reordered or unknown stage")
        if tag == "observation":
            exact_keys(record, ("version", "id", "tag", "seq", "stage", "kind", "value"), "observation")
            if type(record["seq"]) is not int or record["seq"] != self.seq or stage not in GRAPH_STAGES:
                raise ValueError("duplicate/reordered observation or observation during native operation")
            self.seq += 1
            if self.graph is None:
                self.graph = GraphValidator()
            if record["kind"] == "fragment":
                value = record["value"]
                exact_keys(value, ("record_kind", "part", "parts", "payload_hex"), "graph fragment")
                if value["record_kind"] not in FIELDS:
                    raise ValueError("unknown fragmented graph kind")
                integer(value["parts"], 2, 1024, "fragment count")
                integer(value["part"], 0, value["parts"]-1, "fragment part")
                raw = hex_bytes(value["payload_hex"], "fragment bytes")
                if not raw or len(raw) > 128*1024 or (value["part"] < value["parts"]-1 and len(raw) != 128*1024):
                    raise ValueError("invalid graph fragment size")
                if self.fragment is None:
                    self.fragment = {"kind": value["record_kind"], "parts": value["parts"], "chunks": []}
                fragment = self.fragment
                if (value["record_kind"], value["parts"], value["part"]) != (fragment["kind"], fragment["parts"], len(fragment["chunks"])):
                    raise ValueError("missing/duplicate/reordered/interleaved graph fragment")
                fragment["chunks"].append(raw)
                if len(fragment["chunks"]) != fragment["parts"]:
                    return None
                record = {**record, "kind": fragment["kind"], "value": strict_json_loads(b"".join(fragment["chunks"]))}
                self.fragment = None
            elif self.fragment is not None:
                raise ValueError("incomplete graph fragment")
            self.graph.accept(record["kind"], record["value"])
            record = {**record, "seq": self.logical_seq}
            self.logical_seq += 1
        elif tag == "stage":
            exact_keys(record, ("version", "id", "tag", "stage", "outcome", "message_hex"), "stage")
            if self.fragment is not None:
                raise ValueError("stage with incomplete graph fragment")
            message = hex_bytes(record["message_hex"], "stage message")
            if record["outcome"] not in ("ok", "panic"):
                raise ValueError("binder/source parser has no returned-error outcome")
            if record["outcome"] == "ok" and message:
                raise ValueError("successful stage has an error message")
            if stage in GRAPH_STAGES:
                if record["outcome"] != "ok" or self.graph is None or not self.graph.closed:
                    raise ValueError("observer failure disguised as a native outcome")
                self.graph_digests[stage] = self.graph.digest.hexdigest()
                self.graph = None
                if stage == "repeated_graph" and self.graph_digests[stage] != self.graph_digests["bound_graph"]:
                    raise ValueError("repeat binding changed the same runtime's graph")
            self.failed = record["outcome"] != "ok"
            self.outcomes.append({"stage": stage, "outcome": record["outcome"], "message_hex": record["message_hex"]})
            self.stage_index += 1
        else:
            raise ValueError("unknown record tag")
        return record


class Process(S06Process):
    def send(self, request):
        # Reuse the bounded absolute-deadline writer while validating the S07
        # input shape here; the underlying S06 validator sees the same parse bytes.
        validate_request(request)
        import os
        import selectors
        import time
        if self.request_expires is not None:
            raise RuntimeError("previous binder request incomplete")
        raw = canonical(request)+b"\n"
        if len(raw) > 16*1024*1024:
            raise ValueError("oversized binder request")
        self.request_expires = time.monotonic()+self.deadline
        view = memoryview(raw)
        while view:
            self.wait(self.process.stdin, selectors.EVENT_WRITE, self.request_expires)
            try:
                written = os.write(self.process.stdin.fileno(), view)
            except BlockingIOError:
                continue
            view = view[written:]

    def observations(self, request):
        state = Stream(request)
        while not state.ended:
            record = self.read()
            record = state.accept(record)
            if state.ended:
                self.request_expires = None
            if record is not None:
                yield record


def build_oracle():
    command([sys.executable, "tools/s07/binder/generate_syntax.py", "--check"], cwd=ROOT)
    destination = ROOT / "target/s07-oracle/go-binder"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with oracle_export() as (checkout, env, pin):
        package = checkout / "tsc/internal/s07binder"
        package.mkdir()
        for name in ("json.go", "protocol.go", "binder.go", "graph.go", "graph_stream.go", "protocol_test.go"):
            shutil.copyfile(ROOT / "scripts/s07_oracle" / name, package / name)
        for name in ("syntax_bridge.go", "access_bridge.go"):
            shutil.copyfile(ROOT / "scripts/s07_oracle" / name, checkout / "tsc/internal/ast" / ("s07_"+name))
        shutil.copyfile(ROOT / "scripts/s07_oracle/parser_bridge.go", checkout / "tsc/internal/parser/s07_bridge.go")
        command(["go", "test", "-trimpath", "-mod=readonly", "./internal/s07binder", "-count=1"], cwd=checkout/"tsc", env=env)
        command(["go", "build", "-trimpath", "-mod=readonly", "-o", str(destination), "./internal/s07binder"], cwd=checkout/"tsc", env=env)
        verified_upstream()
    return destination, pin


def rust_binary():
    output = command(["cargo", "build", "--package", "tsr_binder", "--example", "binder", "--release", "--locked", "--message-format=json"], cwd=ROOT)
    items = [strict_json_loads(line) for line in output.splitlines() if line.strip()]
    binaries = [item["executable"] for item in items if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "binder" and item.get("executable")]
    if len(binaries) != 1:
        raise ValueError("Cargo did not identify exactly one binder binary")
    return Path(binaries[0])


def smoke_requests():
    from s06_protocol import parser_request
    result = []
    for path in sorted((ROOT / "tools/s07/binder/cases").iterdir()):
        if path.suffix not in (".ts", ".js", ".tsx", ".jsx", ".json"):
            continue
        kind = {".ts": 3, ".js": 1, ".tsx": 4, ".jsx": 2, ".json": 6}[path.suffix]
        result.append(convert_request(parser_request("s07/smoke/"+path.name, None, path.read_bytes(), "/s07/"+path.name, "/s07/"+path.name, kind)))
    generated = [
        ("unknown-script-kind", b"", 0),
        ("invalid-source-bytes", b'const raw = "\xff\xed\xa0\x80"; module.exports = raw;', 1),
        ("large-fragmented-text", b'const large = "'+b'x'*(256*1024)+b'";', 3),
        ("deep-blocks", b'function deep(x) {'+b'{'*2048+b'x++;'+b'}'*2048+b'}', 3),
    ]
    for name,source,kind in generated:
        result.append(convert_request(parser_request("s07/smoke/"+name,None,source,"/s07/"+name+".ts","/s07/"+name+".ts",kind)))
    return result


def first_difference(a, b, path="$"):
    if type(a) is not type(b):
        return {"path": path, "oracle": a, "rust": b}
    if type(a) is dict:
        if set(a) != set(b):
            return {"path": path, "oracle_keys": sorted(a), "rust_keys": sorted(b)}
        for key in sorted(a):
            diff = first_difference(a[key], b[key], path+"."+key)
            if diff:
                return diff
    elif type(a) is list:
        if len(a) != len(b):
            return {"path": path+".length", "oracle": len(a), "rust": len(b)}
        for index, (left, right) in enumerate(zip(a, b)):
            diff = first_difference(left, right, f"{path}[{index}]")
            if diff:
                return diff
    elif a != b:
        return {"path": path, "oracle": a, "rust": b}
    return None


def panic_class(request, stage, raw, runtime):
    message = hex_bytes(raw, "panic bytes").decode("utf-8", "strict")
    if leaf_panic_class(message, runtime) == "bounds":
        return ["bounds"]
    if stage == "parse" and request["script_kind"] == 0 and message == "ScriptKind must be specified when parsing source file: "+request["filename"]:
        return ["script-kind-required", request["filename"]]
    return None


def compare(request, oracle, rust):
    difference = None
    stages = {"oracle": [], "rust": []}
    counts = {"oracle": 0, "rust": 0}
    streams = {"oracle": iter(oracle.observations(request)), "rust": iter(rust.observations(request))}
    raw_exact = True
    qualified_names = {"oracle": [], "rust": []}
    raw_diagnostics = {"oracle": {}, "rust": {}}
    def names(value, path):
        if type(value) is dict:
            if set(value) == {"raw_hex", "identity"} and value["identity"] is not None:
                yield {"path": path, **value}
            else:
                for key,item in value.items():yield from names(item,path+"."+key)
        elif type(value) is list:
            for index,item in enumerate(value):yield from names(item,f"{path}[{index}]")
    done = set()
    while len(done) != 2:
        records = {}
        raw_records = {}
        for runtime in streams:
            if runtime in done:
                records[runtime] = None; raw_records[runtime] = None
                continue
            try:
                record = next(streams[runtime]); counts[runtime] += 1
            except StopIteration:
                done.add(runtime); records[runtime] = None; raw_records[runtime] = None; continue
            raw_records[runtime] = record
            records[runtime] = comparable(record)
            if record["tag"] == "observation":
                context = {"stage":record["stage"],"sequence":record["seq"],"record_kind":record["kind"]}
                qualified_names[runtime].extend({**context,**item} for item in names(record["value"],"$.value"))
                if record["kind"] == "source":raw_diagnostics[runtime][record["stage"]]=record["value"]["diagnostics"]
            if record["tag"] == "stage":
                stages[runtime].append(record)
                if record["outcome"] == "panic":
                    identity = panic_class(request, record["stage"], record["message_hex"], runtime)
                    if identity is None and difference is None:
                        difference = {"path": "unclassified_panic", "runtime": runtime, "record": record}
                    records[runtime] = {**record, "message_hex": identity}
        raw_exact &= raw_records["oracle"] == raw_records["rust"]
        if difference is None:
            difference = first_difference(records["oracle"], records["rust"])
            if difference is not None:
                context = records["oracle"] or records["rust"]
                difference.update(stage=context.get("stage"), sequence=context.get("seq"), record_kind=context.get("kind"))
    return {"id": request["id"], "primary": request["primary"], "equal": difference is None,
            "first_difference": difference, "records": counts, "stages": stages,
            "raw_exact": raw_exact, "qualified_name_observations": qualified_names,
            "raw_diagnostics": raw_diagnostics}


def run_smoke(*, build=True):
    requests = smoke_requests()
    if build:
        oracle, pin = build_oracle(); rust = rust_binary()
    else:
        oracle = ROOT/"target/s07-oracle/go-binder"; rust = ROOT/"target/debug/examples/binder"
        pin = strict_json_loads((ROOT/"data/upstream.json").read_bytes())["pin"]
    output = ROOT/"target/s07-binder-smoke"; output.mkdir(parents=True, exist_ok=True)
    (output/"requests.json").write_bytes(canonical(requests)+b"\n")
    results = []
    with ExitStack() as stack:
        processes = {}
        for runtime, binary in (("oracle", oracle), ("rust", rust)):
            process = Process([str(binary)], output/(runtime+".stderr"))
            stack.callback(process.close)
            processes[runtime] = process
        for request in requests:
            for process in processes.values(): process.send(request)
            result = compare(request, **processes); results.append(result)
            print(request["id"], "equal" if result["equal"] else json.dumps(result["first_difference"], sort_keys=True), flush=True)
    report = {"scope": "supplemental-smoke-only", "upstream_pin": pin, "requests": len(requests), "matched": sum(item["equal"] for item in results), "results": results}
    (output/"report.json").write_bytes(canonical(report)+b"\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("build-oracle", "smoke", "test", "freeze", "e1"))
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--write-manifest", action="store_true")
    parser.add_argument("--filter", help="diagnostic subset only; emits no primary parity")
    args = parser.parse_args()
    if args.operation == "build-oracle":
        print(build_oracle()[0]); return
    if args.operation in ("freeze", "e1"):
        from s07_binder_corpus import freeze, capture
        if args.operation == "e1" and args.write_manifest:
            raise ValueError("capture cannot rewrite its frozen obligations")
        documents, requests, changes, digest = freeze(args.write_manifest)
        if args.operation == "freeze":
            print(json.dumps({"operation":"freeze" if args.write_manifest else "verify", "changed":changes,"preflight_stream_sha256":digest,"primary_requests":len(requests),"reached":documents["binder-probes.json"]["source_preflight"]["reached"]},sort_keys=True)); return
        from s07_binder_tests import run
        oracle = ROOT/"target/s07-oracle/go-binder"; rust = rust_binary()
        tests = run(oracle,rust,ROOT/"target/s07-binder-reports/protocol")
        report = capture(documents,requests,oracle,rust,ROOT/"target/s07-binder-reports",prefix=args.filter)
        report["protocol"] = tests
        print(json.dumps(report,sort_keys=True)); return
    if args.operation == "test":
        from s07_binder_tests import run
        oracle = ROOT/"target/s07-oracle/go-binder" if args.no_build else build_oracle()[0]
        rust = ROOT/"target/debug/examples/binder" if args.no_build else rust_binary()
        print(json.dumps(run(oracle,rust,ROOT/"target/s07-binder-tests"),sort_keys=True)); return
    report = run_smoke(build=not args.no_build)
    print(f"{report['matched']}/{report['requests']} supplemental smoke requests match; no primary metric published")
    if report["matched"] != report["requests"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
