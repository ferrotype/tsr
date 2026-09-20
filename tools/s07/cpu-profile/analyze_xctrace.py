#!/usr/bin/env python3
"""Export and analyze completed Time Profiler traces; never launch a workload.

CPU weights come from Running rows in the time-profile table, not from elapsed
time or all-thread stack snapshots. Phase attribution requires the diagnostic
driver's wrappers. Function inclusive weights overlap and are never summed to
produce a process or phase denominator. The original XML remains authoritative.
"""
from __future__ import annotations

import argparse
from bisect import bisect_right
from collections import Counter, defaultdict
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET


MAX_XML_BYTES = 256 * 1024 * 1024
MAX_ROWS = 500_000
PHASES = ("preload", "parse", "publish", "bind", "retirement", "pipeline_other", "worker_unassigned", "driver", "runtime", "unknown")
WRAPPERS = {"profile_parse": "parse", "profile_publish": "publish",
            "profile_bind": "bind", "profile_retirement": "retirement", "profile_preload": "preload"}
COLUMNS = ("time", "thread", "process", "core", "thread-state", "weight", "stack")
DEVELOPER = Path("/Applications/Xcode.app/Contents/Developer")


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def xml_document(path):
    path = Path(path)
    if path.stat().st_size > MAX_XML_BYTES:
        raise ValueError(f"XML exceeds {MAX_XML_BYTES} byte analysis bound: {path}")
    raw = path.read_bytes()
    if b"<!DOCTYPE" in raw or b"<!ENTITY" in raw:
        raise ValueError("trace XML must not declare document entities")
    return ET.fromstring(raw), {"path": str(path.resolve()), "sha256": digest(raw), "bytes": len(raw)}


class References:
    def __init__(self, root):
        self.values = {}
        for element in root.iter():
            identity = element.get("id")
            if identity is not None:
                if identity in self.values:
                    raise ValueError(f"duplicate XML identity {identity}")
                self.values[identity] = element

    def resolve(self, element):
        seen = set()
        while element.get("ref") is not None:
            identity = element.get("ref")
            if identity in seen or identity not in self.values:
                raise ValueError(f"cyclic or unresolved XML reference {identity}")
            seen.add(identity)
            target = self.values[identity]
            if target.tag != element.tag:
                raise ValueError(f"XML reference changed element type: {identity}")
            element = target
        return element

    def details(self, element, depth=0):
        if depth > 32:
            raise ValueError("unexpected recursive frame metadata")
        element = self.resolve(element)
        return {"tag": element.tag, "attributes": dict(element.attrib),
                "text": element.text or "",
                "children": [self.details(child, depth + 1) for child in element]}


def integer(element, refs, *, positive=False):
    value = refs.resolve(element).text or ""
    if re.fullmatch(r"[0-9]+", value) is None:
        raise ValueError(f"invalid numeric {element.tag}: {value!r}")
    number = int(value)
    if number > 2**64 - 1 or positive and number == 0:
        raise ValueError(f"out-of-range numeric {element.tag}")
    return number


def demangle(names, executable=None):
    names = sorted(set(names))
    candidates = [name for name in names if name.startswith(("_R", "_Z")) and "\n" not in name]
    result = {name: name for name in names}
    if not candidates or executable is False:
        return result, {"executable": None, "changed_symbols": 0}
    if executable is None:
        bundled = DEVELOPER / "Toolchains/XcodeDefault.xctoolchain/usr/bin/c++filt"
        executable = str(bundled) if bundled.is_file() else shutil.which("c++filt")
    if executable is None:
        return result, {"executable": None, "changed_symbols": 0}
    output = subprocess.run([str(executable), "--no-strip-underscore"],
                            input=("\n".join(candidates) + "\n").encode(),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            check=True, timeout=60).stdout.decode().splitlines()
    if len(output) != len(candidates):
        raise ValueError("demangler changed symbol inventory")
    result.update(zip(candidates, output))
    return result, {"executable": str(Path(executable).resolve()),
                    "changed_symbols": sum(result[name] != name for name in names)}


# Only selector inputs are normalized. Captured frame names, stack keys and
# report labels retain the spelling of the binary that produced the archive.
LEGACY_CRATE = re.compile(r"(?<![\w:])ts_(arena|ast|binder|parser|cpu_profile)(?=::)")


def selector_name(name):
    return LEGACY_CRATE.sub(r"tsr_\1", name)


def marker(name, function):
    # Match an actual driver function or its closure; never a generic parameter
    # mentioning another function. Symbol metadata itself remains unmodified.
    return re.match(r"^tsr_cpu_profile(?:::[A-Za-z_][A-Za-z_0-9]*)*::" + function
                    + r"(?=$|::|[<(])", selector_name(name)) is not None


def frame_names(frame):
    return [frame["name"], *(frame.get("physical_symbol") or {}).get("names", [])]


def phase_for(stack, worker=False):
    names = [name for frame in stack for name in frame_names(frame)]
    # Leaf-first: a concrete inner operation takes priority over its caller.
    for name in names:
        for function, phase in WRAPPERS.items():
            if marker(name, function):
                return phase
    if any(marker(name, "measure_pipeline") for name in names):
        return "pipeline_other"
    if worker:
        return "worker_unassigned"
    if not names:
        return "unknown"
    if any(selector_name(name).startswith("tsr_cpu_profile::") for name in names):
        return "driver"
    if names[0].startswith(("std::", "core::", "alloc::", "runtime.", "mi_", "_mi_", "pthread_", "_pthread_")):
        return "runtime"
    return "unknown"


def physical_symbols(binary, demangler):
    """Bind outer physical functions to the Mach-O UUID and linked addresses.

    Instruments can display one inlined callee in place of an enclosing frame.
    nm's actual text-symbol ranges recover that enclosing physical function,
    without treating arbitrary generic type mentions as executed function calls.
    """
    binary = Path(binary)
    raw = binary.read_bytes()
    uuid_output = subprocess.check_output(['/usr/bin/dwarfdump', '--uuid', str(binary)], timeout=60)
    identities = re.findall(r'^UUID: ([0-9A-F-]+) \(([^)]+)\)', uuid_output.decode(), re.M)
    if len(identities) != 1:
        raise ValueError('physical attribution requires one native Mach-O architecture')
    output = subprocess.check_output(['/usr/bin/nm', '-n', str(binary)], timeout=60)
    base = None
    entries = defaultdict(list)
    for line in output.decode().splitlines():
        match = re.fullmatch(r'([0-9a-fA-F]+) ([A-Za-z]) (.+)', line)
        if not match:
            continue
        address, kind, name = match.groups()
        if name == '__mh_execute_header':
            base = int(address, 16)
        if kind in {'t', 'T'}:
            # Mach-O prepends one underscore to linkage names.
            entries[int(address, 16)].append(name[1:] if name.startswith('_') else name)
    if base is None or not entries:
        raise ValueError('missing Mach-O executable header/text symbol table')
    names, _ = demangle((name for aliases in entries.values() for name in aliases), demangler)
    starts = sorted(entries)
    ranges = [{'start': start, 'end': end, 'names': sorted({names[name] for name in entries[start]}),
               'raw_names': sorted(entries[start])} for start, end in zip(starts, starts[1:])]
    return {'binary': str(binary.resolve()), 'binary_sha256': digest(raw),
            'uuid': identities[0][0], 'architecture': identities[0][1], 'linked_base': base,
            'nm_sha256': digest(output), 'dwarfdump_uuid_sha256': digest(uuid_output),
            'nm_text': output.decode(), 'dwarfdump_uuid_text': uuid_output.decode(),
            'method': 'nearest preceding text symbol through next distinct text-symbol address; aliases retained',
            'starts': starts[:-1], 'ranges': ranges}


def physical_for(frame, symbols):
    binary = frame.get('binary') or {}
    if symbols is None or binary.get('UUID') != symbols['uuid']:
        return None
    if binary.get('arch') != symbols['architecture']:
        raise ValueError('trace and symbol table architecture disagree')
    linked = int(frame['address'], 16) - int(binary['load-addr'], 16) + symbols['linked_base']
    index = bisect_right(symbols['starts'], linked) - 1
    if index < 0 or linked >= symbols['ranges'][index]['end']:
        return None
    return {**symbols['ranges'][index], 'linked_address': linked}


def source_locations(metadata):
    result = []
    for source in metadata:
        if source['tag'] == 'source':
            for child in source['children']:
                if child['tag'] == 'path':
                    result.append({'file': child['text'], 'line': int(source['attributes']['line'])})
    return result


def member(name, owner, method):
    # An owner's appearance inside another function's generic type is not a
    # frame for that owner. Only the actual leading method path can match.
    return any(re.match(re.escape(prefix) + r'(?=$|::|[<(])', name) is not None
               for prefix in (f'<{owner}>::{method}', f'{owner}::{method}'))


def query_flags(stack):
    names = [selector_name(name) for frame in stack for name in frame_names(frame)]
    def has(owner, method):
        return any(member(name, owner, method) for name in names)
    lookup = has('tsr_ast::storage::AstView', 'node') or any(
        has('tsr_arena::file::StorageView<tsr_ast::Node>', method) for method in ('node', 'node_here'))
    core = has('tsr_ast::storage::AstView', 'validate_core')
    binding = has('tsr_ast::bind_result::BindBuilder', 'validate')
    new_node = has('tsr_ast::storage::AstBuilder', 'new_node_before_hook')
    edges = has('tsr_ast::storage::AstView', 'validate_data') or has('tsr_ast::data_generated::NodeData', 'validate_references')
    return {'ast_lookup_union': lookup, 'validate_core': core, 'bind_builder_validate': binding,
            'new_node_before_hook': new_node, 'new_node_edge_validation': new_node and edges,
            'edge_validation_any': edges,
            'validation_union': core or binding or edges}


def validation_category(flags):
    if flags['bind_builder_validate']:
        return 'bind_builder_validation'
    if flags['validate_core']:
        return 'core_graph_validation'
    if flags['new_node_edge_validation']:
        return 'new_node_edge_validation'
    if flags['edge_validation_any']:
        return 'other_edge_validation'
    if flags['new_node_before_hook']:
        return 'new_node_other'
    return 'other'


def function_key(frame):
    binary = frame.get("binary") or {}
    return (binary.get("UUID", ""), binary.get("name", ""), frame["name"])


def summarize(toc, samples, *, top=30, demangler=None, binary=None, symbol_map=None):
    if not 1 <= top <= 1000:
        raise ValueError("top function bound must be between 1 and 1000")
    toc_root, toc_info = xml_document(toc)
    runs = toc_root.findall("run")
    if len(runs) != 1:
        raise ValueError("analysis requires exactly one trace run")
    run = runs[0]
    target = run.find("info/target/process")
    tables = run.findall("data/table[@schema='time-profile']")
    if target is None or len(tables) != 1 or tables[0].get("record-waiting-threads") != "0":
        raise ValueError("expected one target and CPU-only Time Profiler table")
    if target.get("return-exit-status") != "0":
        raise ValueError("target did not exit successfully")
    target_pid = int(target.attrib["pid"])
    saved_map = None
    saved_map_info = None
    if symbol_map is not None:
        map_raw = Path(symbol_map).read_bytes()
        saved_map = json.loads(map_raw)
        if saved_map.get('schema') != 1 or not isinstance(saved_map.get('display_names'), dict):
            raise ValueError('unknown offline symbol-map schema')
        saved_map_info = {'path': str(Path(symbol_map).resolve()), 'sha256': digest(map_raw)}
    if binary is None and saved_map is None:
        candidates = [process.get('path') for process in run.findall('processes/process')
                      if process.get('pid') == str(target_pid)]
        if len(candidates) == 1 and candidates[0] and Path(candidates[0]).is_file():
            binary = candidates[0]
    symbols = saved_map['physical_symbols'] if saved_map else physical_symbols(binary, demangler) if binary is not None else None
    if saved_map is not None:
        if (digest(symbols['nm_text'].encode()) != symbols['nm_sha256']
                or digest(symbols['dwarfdump_uuid_text'].encode()) != symbols['dwarfdump_uuid_sha256']
                or symbols['starts'] != [item['start'] for item in symbols['ranges']]
                or symbols['starts'] != sorted(set(symbols['starts']))
                or any(item['start'] >= item['end'] for item in symbols['ranges'])):
            raise ValueError('offline symbol-map provenance/ranges changed')
    root, sample_info = xml_document(samples)
    nodes = root.findall("node")
    if len(nodes) != 1:
        raise ValueError("analysis requires exactly one exported CPU table")
    table = nodes[0]
    schema = table.find("schema")
    if (schema is None or schema.get("name") != "time-profile"
            or tuple(col.findtext("mnemonic") for col in schema.findall("col")) != COLUMNS):
        raise ValueError("unknown time-profile column schema")
    rows = table.findall("row")
    if not rows or len(rows) > MAX_ROWS:
        raise ValueError("missing CPU rows or row count exceeds analysis bound")
    refs = References(root)
    frame_elements = {identity: item for identity, item in refs.values.items() if item.tag == "frame"}
    displayed = {item.get('name', '') for item in frame_elements.values()}
    if saved_map:
        names = saved_map['display_names']
        if not displayed <= names.keys() or any(not isinstance(names[name], str) for name in displayed):
            raise ValueError('offline symbol map omitted displayed symbols')
        demangler_info = saved_map['demangler']
    else:
        names, demangler_info = demangle(displayed, demangler)
    frames = {}
    for identity, element in frame_elements.items():
        binary = element.find("binary")
        frame = {"id": identity, "raw_name": element.get("name", ""),
                            "name": names[element.get("name", "")],
                            "address": element.get("addr"),
                            "attributes": dict(element.attrib),
                            "binary": dict(refs.resolve(binary).attrib) if binary is not None else None,
                            "source_and_inline_metadata": [refs.details(child) for child in element if child.tag != "binary"]}
        frame['source_locations'] = source_locations(frame['source_and_inline_metadata'])
        frame['physical_symbol'] = physical_for(frame, symbols)
        frames[identity] = frame
    if symbols is not None and not any(frame['physical_symbol'] is not None for frame in frames.values()):
        raise ValueError('supplied binary UUID has no corresponding physical trace frames')
    cpu_ns = 0
    cpu_rows = 0
    all_weight = 0
    missing_ns = 0
    states = Counter()
    state_rows = Counter()
    exclusions = Counter()
    phase_ns = Counter()
    phase_rows = Counter()
    thread_ns = Counter()
    thread_rows = Counter()
    thread_names = {}
    weights = Counter()
    self_ns = defaultdict(Counter)
    self_rows = defaultdict(Counter)
    inclusive_ns = defaultdict(Counter)
    inclusive_rows = defaultdict(Counter)
    query_ns = defaultdict(Counter)
    query_rows = defaultdict(Counter)
    validation_ns = defaultdict(Counter)
    source_ns = 0
    physical_ns = 0
    worker_ns = 0
    function_frames = defaultdict(set)
    timestamps = []
    missing_rows = 0
    for row in rows:
        if len(row) != len(COLUMNS):
            raise ValueError("CPU row has missing or extra columns")
        time, thread, process, core, state, weight, tagged = row
        if (tuple(element.tag for element in list(row)[:6])
                != ("sample-time", "thread", "process", "core", "thread-state", "weight")
                or tagged.tag not in {"tagged-backtrace", "sentinel"}):
            raise ValueError("CPU row changed a column type")
        timestamp = integer(time, refs)
        value = integer(weight, refs, positive=True)
        process = refs.resolve(process)
        pid_element = process.find("pid")
        if pid_element is None:
            raise ValueError("CPU row has no process identity")
        pid = integer(pid_element, refs)
        state_text = refs.resolve(state).text or ""
        states[state_text] += value
        state_rows[state_text] += 1
        all_weight += value
        if pid != target_pid or state_text != "Running":
            exclusions["other_process" if pid != target_pid else "non_running"] += value
            continue
        thread = refs.resolve(thread)
        tid_element = thread.find("tid")
        owner = thread.find("process")
        if tid_element is None or owner is None:
            raise ValueError("CPU row has no thread identity")
        owner_pid = refs.resolve(owner).find("pid")
        if owner_pid is None or integer(owner_pid, refs) != pid:
            raise ValueError("CPU row and thread process identities disagree")
        tid = integer(tid_element, refs)
        thread_names[tid] = thread.get("fmt", "")
        thread_ns[tid] += value
        thread_rows[tid] += 1
        cpu_rows += 1
        cpu_ns += value
        weights[value] += 1
        timestamps.append(timestamp)
        tagged = refs.resolve(tagged)
        if tagged.tag == "sentinel":
            stack = []
        elif tagged.tag == "tagged-backtrace":
            back = tagged.find("backtrace")
            if back is None:
                raise ValueError("tagged CPU stack has no backtrace")
            back = refs.resolve(back)
            if any(child.tag != "frame" for child in back):
                raise ValueError("unknown backtrace member")
            stack = [frames[refs.resolve(child).attrib["id"]] for child in back]
        else:
            raise ValueError("unknown CPU stack representation")
        worker = thread.get('fmt', '').startswith('ts-parser ')
        if worker:
            worker_ns += value
        phase = phase_for(stack, worker)
        phase_ns[phase] += value
        phase_rows[phase] += 1
        flags = query_flags(stack)
        for bucket in ('all', phase):
            for query, matched in flags.items():
                if matched:
                    query_ns[bucket][query] += value
                    query_rows[bucket][query] += 1
            validation_ns[bucket][validation_category(flags)] += value
        if stack and stack[0]['source_locations']:
            source_ns += value
        if any(frame['physical_symbol'] for frame in stack):
            physical_ns += value
        if not stack:
            missing_ns += value
            missing_rows += 1
            continue
        leaf = function_key(stack[0])
        unique = {function_key(frame) for frame in stack}
        for frame in stack:
            function_frames[function_key(frame)].add(frame["id"])
        for bucket in ("all", phase):
            self_ns[bucket][leaf] += value
            self_rows[bucket][leaf] += 1
            # Recursion, duplicate addresses and inlined copies of one function
            # still contribute at most one inclusive weight per sampled stack.
            for key in unique:
                inclusive_ns[bucket][key] += value
                inclusive_rows[bucket][key] += 1
    if not cpu_rows or sum(phase_ns.values()) != cpu_ns:
        raise ValueError("no target Running samples or phase accounting mismatch")

    def ranking(values, counts, denominator):
        return [{"binary_uuid": key[0], "binary": key[1], "name": key[2],
                 "cpu_weight_ns": value, "sample_rows": counts[key],
                 "fraction_of_bucket_cpu": value / denominator if denominator else 0,
                 "frame_ids": sorted(function_frames[key], key=int)}
                for key, value in sorted(values.items(), key=lambda item: (-item[1], item[0]))[:top]]

    return {"schema": 1, "operation": "xctrace_cpu_analysis", "target": dict(target.attrib),
            "trace_summary": {child.tag: child.text for child in run.findall("info/summary/*") if len(child) == 0},
            "cpu_table_settings": dict(tables[0].attrib), "artifacts": {"toc": toc_info, "samples": sample_info},
            "demangler": demangler_info,
            "physical_symbols": {key: value for key, value in symbols.items() if key not in {'starts', 'ranges', 'nm_text', 'dwarfdump_uuid_text'}} if symbols else None,
            "offline_symbol_map": saved_map_info,
            "scope": {"cpu_denominator": "sum of recorded weights for target-process Running rows",
                      "phase_method": "exclusive nearest physical or displayed driver wrapper; unassigned worker mass explicit; no generic-name inference",
                      "inclusive": "one weight per distinct binary/function per sample; overlapping rows must not be summed",
                      "self": "leaf-frame weight; missing stacks remain in the CPU denominator",
                      "source_metadata": "exported source location for displayed DWARF frame; not a complete inline ancestry",
                      "queries": "union of matching displayed/physical functions per row; query groups overlap and must not be summed",
                      "validation_exclusive": "per-row priority: binding validation, core graph validation, new-node edges, other edges, new-node other, other",
                      "sample_count": "sampling observations, never function invocation counts",
                      "time_filter": "entire exported trace; phase wrappers isolate measured operations"},
            "exported_rows": len(rows), "exported_weight_ns": all_weight,
            "state_weight_ns": dict(states), "state_rows": dict(state_rows), "excluded_weight_ns": dict(exclusions),
            "cpu_sample_rows": cpu_rows, "cpu_weight_ns": cpu_ns,
            "weight_histogram": {str(key): value for key, value in sorted(weights.items())},
            "first_sample_ns": min(timestamps), "last_sample_ns": max(timestamps),
            "missing_stack_rows": missing_rows, "missing_stack_weight_ns": missing_ns,
            "coverage": {"leaf_source_weight_ns": source_ns, "any_physical_symbol_weight_ns": physical_ns,
                         "worker_cpu_weight_ns": worker_ns, "worker_unassigned_weight_ns": phase_ns['worker_unassigned']},
            "threads": [{"tid": tid, "name": thread_names[tid], "cpu_weight_ns": value, "sample_rows": thread_rows[tid]}
                        for tid, value in sorted(thread_ns.items())],
            "phases": {phase: {"cpu_weight_ns": phase_ns[phase], "sample_rows": phase_rows[phase],
                                "fraction_of_process_cpu": phase_ns[phase] / cpu_ns,
                                "union_queries": {name: {'cpu_weight_ns': value, 'sample_rows': query_rows[phase][name]}
                                                  for name, value in query_ns[phase].items()},
                                "validation_exclusive_weight_ns": dict(validation_ns[phase]),
                                "top_self": ranking(self_ns[phase], self_rows[phase], phase_ns[phase]),
                                "top_inclusive": ranking(inclusive_ns[phase], inclusive_rows[phase], phase_ns[phase])}
                       for phase in PHASES},
            "union_queries": {name: {'cpu_weight_ns': value, 'sample_rows': query_rows['all'][name]}
                              for name, value in query_ns['all'].items()},
            "validation_exclusive_weight_ns": dict(validation_ns['all']),
            "top_self": ranking(self_ns["all"], self_rows["all"], cpu_ns),
            "top_inclusive": ranking(inclusive_ns["all"], inclusive_rows["all"], cpu_ns),
            "frames": sorted(frames.values(), key=lambda item: int(item["id"]))}


def export(trace, prefix):
    developer = Path(os.environ.get("DEVELOPER_DIR", DEVELOPER))
    tool = developer / "usr/bin/xctrace"
    if not tool.is_file():
        raise ValueError("set DEVELOPER_DIR to a full Xcode installation")
    env = {**os.environ, "DEVELOPER_DIR": str(developer)}
    prefix = Path(prefix)
    prefix.parent.mkdir(parents=True, exist_ok=True)
    toc, samples = (prefix.with_name(prefix.name + suffix) for suffix in (".toc.xml", ".time-profile.xml"))
    for mode, destination in ((["--toc"], toc), (["--xpath", '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]'], samples)):
        command = [str(tool), "export", "--input", str(trace), *mode, "--output", str(destination)]
        print("+ " + " ".join(command), file=sys.stderr, flush=True)
        subprocess.run(command, env=env, stdout=sys.stderr, check=True, timeout=120)
    return toc, samples


def contract_checks():
    """Small artificial counterexamples, not additional workload observations."""
    import tempfile
    toc = ('<trace-toc><run number="1"><info><target><process pid="42" '
           'return-exit-status="0"/></target></info><data><table schema="time-profile" '
           'record-waiting-threads="0"/></data></run></trace-toc>')
    schema = '<schema name="time-profile">' + ''.join(
        '<col><mnemonic>' + name + '</mnemonic></col>' for name in
        ('time', 'thread', 'process', 'core', 'thread-state', 'weight', 'stack')) + '</schema>'
    first = ('<row><sample-time>1</sample-time><thread id="1" fmt="worker">'
             '<tid>8</tid><process id="2"><pid>42</pid></process></thread><process ref="2"/>'
             '<core>0</core><thread-state>Running</thread-state><weight>2</weight>'
             '<tagged-backtrace><backtrace><frame id="3" name="leaf" addr="0xa">'
             '<binary id="4" UUID="uuid" name="app"/><source-location file="fixture.rs" line="7"/>'
             '</frame><frame id="5" name="leaf" addr="0xb"><binary ref="4"/></frame>'
             '<frame id="6" name="tsr_cpu_profile::profile_parse"><binary ref="4"/></frame>'
             '<frame id="7" name="tsr_cpu_profile::measure_pipeline"><binary ref="4"/></frame>'
             '</backtrace></tagged-backtrace></row>')

    def row(time, weight, body, state='Running', pid=42):
        return (f'<row><sample-time>{time}</sample-time><thread ref="1"/>'
                f'<process><pid>{pid}</pid></process><core>0</core><thread-state>{state}</thread-state>'
                f'<weight>{weight}</weight>{body}</row>')

    binding = ('<tagged-backtrace><backtrace><frame ref="3"/>'
               '<frame id="8" name="tsr_cpu_profile::profile_bind"><binary ref="4"/></frame>'
               '<frame ref="6"/></backtrace></tagged-backtrace>')
    retirement = ('<tagged-backtrace><backtrace><frame ref="3"/>'
                  '<frame id="9" name="tsr_cpu_profile::profile_retirement"><binary ref="4"/></frame>'
                  '</backtrace></tagged-backtrace>')
    body = (first + row(2, 7, binding) + row(3, 11, '<sentinel/>')
            + row(4, 13, '<sentinel/>', state='Waiting')
            + row(5, 17, '<sentinel/>', pid=43) + row(6, 19, retirement))
    document = '<trace-query-result><node>' + schema + body + '</node></trace-query-result>'
    passed = []
    with tempfile.TemporaryDirectory(prefix='s07-cpu-analyzer-') as folder:
        t, s = Path(folder) / 'toc.xml', Path(folder) / 'samples.xml'
        t.write_text(toc)
        s.write_text(document)
        result = summarize(t, s, demangler=False)
        assert result['cpu_weight_ns'] == 39 and result['cpu_sample_rows'] == 4
        assert result['exported_weight_ns'] == 69 and result['exported_rows'] == 6
        passed.append('variable CPU weights; blocked and other-process rows excluded')
        assert {k: v['cpu_weight_ns'] for k, v in result['phases'].items() if v['cpu_weight_ns']} == {
            'parse': 2, 'bind': 7, 'retirement': 19, 'unknown': 11}
        passed.append('nearest wrapper phases are disjoint; inner binding beats outer parsing')
        assert result['missing_stack_weight_ns'] == 11 and result['missing_stack_rows'] == 1
        passed.append('missing stacks retained in denominator')
        leaf = next(item for item in result['top_inclusive'] if item['name'] == 'leaf')
        assert leaf['cpu_weight_ns'] == 28 and leaf['sample_rows'] == 3
        passed.append('recursive/address-duplicate inclusive frames counted once per sample')
        assert result['frames'][0]['source_and_inline_metadata'][0]['attributes'] == {
            'file': 'fixture.rs', 'line': '7'}
        passed.append('raw source metadata and distinct addresses preserved')
        physical = {'UUID': 'uuid', 'arch': 'arm64', 'load-addr': '0x2000'}
        symbols = {'uuid': 'uuid', 'architecture': 'arm64', 'linked_base': 0x1000,
                   'starts': [0x1100], 'ranges': [{'start': 0x1100, 'end': 0x1200,
                                                'names': ['tsr_cpu_profile::profile_bind']}]}
        frame = {'name': 'tsr_parser::worker::on_parser_worker', 'address': '0x2110', 'binary': physical}
        frame['physical_symbol'] = physical_for(frame, symbols)
        assert frame['physical_symbol']['linked_address'] == 0x1110 and phase_for([frame]) == 'bind'
        passed.append('ASLR-relative physical wrapper beats displayed inline-callee name')
        assert physical_for({**frame, 'binary': {**physical, 'UUID': 'other'}}, symbols) is None
        assert phase_for([{'name': 'tsr_cpu_profile::main::{closure#0}'}], worker=True) == 'worker_unassigned'
        assert not member('alloc::Vec<tsr_ast::storage::AstView>::node', 'tsr_ast::storage::AstView', 'node')
        passed.append('wrong UUID and generic mentions cannot qualify phases or queries')
        for name, changed in (
            ('unknown reference', document.replace('<frame ref="3"/>', '<frame ref="999"/>', 1)),
            ('duplicate identity', document.replace('id="5"', 'id="3"', 1)),
            ('wrong column type', document.replace('<weight>2</weight>', '<core>2</core>', 1)),
            ('nonpositive CPU weight', document.replace('<weight>2</weight>', '<weight>0</weight>', 1)),
            ('thread process disagreement', document.replace('<pid>42</pid></process></thread>', '<pid>41</pid></process></thread>', 1)),
        ):
            s.write_text(changed)
            try:
                summarize(t, s, demangler=False)
            except ValueError:
                passed.append('reject ' + name)
            else:
                raise AssertionError('accepted ' + name)
        s.write_text(document)
        t.write_text(toc.replace('record-waiting-threads="0"', 'record-waiting-threads="1"'))
        try:
            summarize(t, s, demangler=False)
        except ValueError:
            passed.append('reject waiting-thread trace mode')
        else:
            raise AssertionError('accepted waiting-thread trace mode')
    return {'scope': 'artificial analyzer contracts, not compiler parity', 'passed': passed, 'tests': len(passed)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    exporter = commands.add_parser("export", help="export a completed trace and analyze its CPU table")
    exporter.add_argument("trace", type=Path)
    exporter.add_argument("--output-prefix", type=Path, required=True)
    analyzer = commands.add_parser("analyze", help="analyze already exported XML without Instruments")
    analyzer.add_argument("--toc", type=Path, required=True)
    analyzer.add_argument("--samples", type=Path, required=True)
    analyzer.add_argument("--output", type=Path, required=True)
    commands.add_parser("selftest", help="run bounded synthetic analyzer contract checks")
    for command in (exporter, analyzer):
        command.add_argument("--top", type=int, default=30)
        command.add_argument("--demangler", type=Path)
        command.add_argument("--no-demangle", action="store_true")
        command.add_argument("--binary", type=Path, help="matching Mach-O executable; defaults to the recorded process path when available")
        command.add_argument("--symbol-map", type=Path, help="archived physical/display symbol map; requires no live binary or demangler")
    args = parser.parse_args()
    if args.command == "selftest":
        print(json.dumps(contract_checks(), sort_keys=True))
        return
    if args.demangler and args.no_demangle:
        parser.error("--demangler and --no-demangle are mutually exclusive")
    if args.symbol_map and (args.binary or args.demangler or args.no_demangle):
        parser.error('--symbol-map cannot be combined with live symbolization options')
    if args.command == "export":
        toc, samples = export(args.trace, args.output_prefix)
        output = args.output_prefix.with_name(args.output_prefix.name + ".summary.json")
    else:
        toc, samples, output = args.toc, args.samples, args.output
    report = summarize(toc, samples, top=args.top, demangler=False if args.no_demangle else args.demangler,
                       binary=args.binary, symbol_map=args.symbol_map)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n")
    print(json.dumps({"summary": str(output.resolve()), "cpu_sample_rows": report["cpu_sample_rows"],
                      "cpu_weight_ns": report["cpu_weight_ns"], "missing_stack_rows": report["missing_stack_rows"]}))


if __name__ == "__main__":
    main()
