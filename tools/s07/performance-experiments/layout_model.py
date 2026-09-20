#!/usr/bin/env python3
"""CP0 diagnostic model: compile schema-shaped layouts and reconcile frozen bytes.

No production benchmarks or tracker metrics are emitted. Missing observations
remain unknown; weighted bounds are not an implemented replacement census.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ARCHIVE = ROOT / "tools/s07/memory-profile/results/2026-09-08"
TYPE_MAP = {
    "Option<NodeId>": "u32", "Option<NodeListId>": "u32",
    "NodeSlice": "u32", "TextSlice": "u32", "JsString": "Text8",
    "bool": "bool", "i32": "i32", "NodeKind": "i16",
}
BIND_TYPES = {"*FlowNode", "*Node", "*Symbol", "SymbolTable"}
BUDGET = {"syntax_and_binding": 880_000_000, "syntax_only": 780_000_000,
          "binding_fields": 100_000_000, "auxiliary": 140_000_000,
          "symbols_flows_lists_tables": 440_000_000, "text": 190_000_000,
          "other_and_unknown": 50_000_000, "live_endpoint": 1_700_000_000,
          "pipeline_requests": 1_900_000_000, "request_traffic": 350_000_000,
          "parse_traffic": 200_000_000, "bind_traffic": 125_000_000,
          "publish_scheduling_traffic": 25_000_000}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def strict_json(data: bytes):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result
    def constant(value):
        raise ValueError(f"non-finite JSON value: {value}")
    return json.loads(data, object_pairs_hook=pairs, parse_constant=constant)


def archived_inputs() -> tuple[dict, dict]:
    manifest = strict_json((ARCHIVE / "manifest.json").read_bytes())
    wanted = ["native/rust-1-0-census.json", "native/rust-1-0-report.json", "native/rust-8-0-report.json",
              "sites/rust-1-0-sites.json"]
    result, hashes = {}, {}
    # Read named members without extracting paths supplied by an archive.
    with tarfile.open(ARCHIVE / manifest["archive"]["path"], "r:xz") as archive:
        for name in wanted:
            member = archive.getmember(name)
            if not member.isfile():
                raise ValueError(f"not a regular archived file: {name}")
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError(f"unreadable archived file: {name}")
            data = stream.read()
            expected = manifest["files"][name]
            if len(data) != expected["bytes"] or digest(data) != expected["sha256"]:
                raise ValueError(f"archived input digest mismatch: {name}")
            result[name] = strict_json(data)
            hashes[name] = digest(data)
    return result, hashes


def schema_shapes(source: str, schema: dict) -> dict:
    shapes = {}
    for name, body in re.findall(r"pub struct (\w+)Data \{([^{}]*)\}", source):
        fields = re.findall(r"    pub ([\w#]+): ([^\n]+),", body)
        if len(fields) != body.count("pub "):
            raise ValueError(f"unrecognized generated field in {name}")
        for _, field_type in fields:
            if field_type not in TYPE_MAP:
                raise ValueError(f"unsupported generated type: {field_type}")
        shapes[name] = {"fields": fields, "binding": [], "facts": False, "unknown": []}
    defined = set(schema["nodes"]["definitions"])
    if set(shapes) != defined:
        raise ValueError(f"generated/schema shape mismatch: {set(shapes) ^ defined}")
    for name, field, field_type in re.findall(
            r'DeferredField\s*\{\s*node: "([^"]+)",\s*field: "([^"]+)",'
            r'\s*upstream_type: "([^"]+)"', source):
        if field_type == "atomic.Uint32" and field == "facts":
            shapes[name]["facts"] = True
        elif field_type in BIND_TYPES:
            shapes[name]["binding"].append(field)
        else:
            shapes[name]["unknown"].append({"field": field, "type": field_type})
    return shapes


def compile_layouts(shapes: dict, output: Path, rustc: str) -> tuple[dict, dict]:
    output.mkdir(parents=True, exist_ok=True)
    sketch = (HERE / "layout/sketch.rs").read_text()
    declarations, print_calls = [], []
    for name, shape in sorted(shapes.items()):
        fields = [f"f{index}: {TYPE_MAP[kind]}" for index, (_, kind) in enumerate(shape["fields"])]
        if shape["facts"]:
            fields.append("facts: std::sync::atomic::AtomicU32")
        # Unknown checker-only fields are not silently charged as zero. Their
        # omission is carried into feasibility blockers and the shape inventory.
        for mode in ["Syntax", "Bound"]:
            all_fields = fields + ([f"binding{index}: u32" for index, _ in enumerate(shape["binding"])]
                                   if mode == "Bound" else [])
            typename = mode + name
            declarations.append(f"#[repr(C)] struct {typename} {{ {', '.join(all_fields)} }}")
            print_calls.append(f'println!("{typename} {{}} {{}}", std::mem::size_of::<{typename}>(), std::mem::align_of::<{typename}>());')
    for typename in ["Header24", "Header32", "Text8", "FullId"]:
        print_calls.append(f'println!("{typename} {{}} {{}}", std::mem::size_of::<{typename}>(), std::mem::align_of::<{typename}>());')
    print_calls.append('println!("VecHeader {} {}", std::mem::size_of::<Vec<u32>>(), std::mem::align_of::<Vec<u32>>());')
    source = sketch + "\n" + "\n".join(declarations) + "\nfn main() {\n" + "\n".join(print_calls) + "\n}\n"
    generated = output / "generated.rs"
    generated.write_text(source)
    executable, test_binary = output / "layout-sketch", output / "layout-tests"
    version = subprocess.run([rustc, "-vV"], capture_output=True, text=True, check=True).stdout
    subprocess.run([rustc, "--edition=2021", "-Dwarnings", str(generated), "-o", str(executable)], check=True)
    subprocess.run([rustc, "--edition=2021", "-Dwarnings", "--test", str(generated), "-o", str(test_binary)], check=True)
    tests = subprocess.run([str(test_binary)], capture_output=True, text=True, check=True)
    (output / "test-output.txt").write_text(tests.stdout + tests.stderr)
    stdout = subprocess.run([str(executable)], capture_output=True, text=True, check=True).stdout
    layouts = {}
    for line in stdout.splitlines():
        name, size, alignment = line.split()
        layouts[name] = {"size": int(size), "alignment": int(alignment)}
    return layouts, {"rustc": version, "generated_source_sha256": digest(source.encode()),
                     "executable_sha256": digest(executable.read_bytes()), "tests": tests.stdout}


def weighted_bounds(counts: dict[str, int], sizes: dict[str, int], duplicates: int) -> dict:
    """Bounds after removing exactly N overlay records of unknown shapes.

    Removing the largest sizes minimizes projected core bytes; removing the
    smallest maximizes them. No shape is assigned a fictitious measured count.
    """
    if duplicates < 0 or duplicates > sum(counts.values()):
        raise ValueError("invalid overlay count")
    total = sum(count * sizes[name] for name, count in counts.items())
    def removal(reverse):
        remaining, removed = duplicates, 0
        for name in sorted(counts, key=lambda name: sizes[name], reverse=reverse):
            amount = min(remaining, counts[name])
            removed += amount * sizes[name]
            remaining -= amount
        return removed
    return {"lower": total - removal(True), "upper": total - removal(False),
            "raw_with_overlays": total, "removed_records": duplicates}


def traffic_model(report: dict) -> dict:
    before, after = report["pre_pipeline"], report["retained_endpoint"]
    if type(report["workers"]) is not int or report["workers"] not in (1, 8):
        raise ValueError("unsupported worker count")
    for snapshot in [before, after]:
        for key in ["live_requested_bytes", "total_requested_bytes"]:
            if type(snapshot[key]) is not int or snapshot[key] < 0:
                raise ValueError(f"invalid native counter: {key}")
    start, end = before["live_requested_bytes"], after["live_requested_bytes"]
    requests = after["total_requested_bytes"] - before["total_requested_bytes"]
    growth = end - start
    traffic = requests - growth
    for field, value in [("pipeline_allocated_bytes", requests), ("pipeline_live_growth_bytes", growth),
                         ("pipeline_superseded_or_freed_requested_bytes", traffic)]:
        if type(report[field]) is not int or report[field] != value:
            raise ValueError(f"native counter identity mismatch: {field}")
    result = {"start_live": start, "end_live": end, "pipeline_requests": requests,
              "live_growth": growth, "freed_or_superseded_requests": traffic,
              "traffic_reduction_to_350mb": traffic - BUDGET["request_traffic"],
              "maximum_traffic_at_live_and_request_ceilings": BUDGET["pipeline_requests"] - BUDGET["live_endpoint"] + start,
              "pipeline_requests_at_1700mb_live_350mb_traffic": BUDGET["live_endpoint"] - start + BUDGET["request_traffic"],
              "required_request_reduction_to_1900mb": requests - BUDGET["pipeline_requests"],
              "required_live_reduction_to_1700mb": end - BUDGET["live_endpoint"]}
    if report["workers"] == 1:
        totals = report["elapsed_worker_totals"]
        if any(type(totals[f"{phase}_{suffix}"]) is not int
               for phase in ["parse", "publish", "bind"]
               for suffix in ["allocated_bytes", "live_growth_bytes"]):
            raise ValueError("invalid global phase counter")
        result["phase_observations"] = {
            phase: {"requests": totals[f"{phase}_allocated_bytes"],
                    "live_growth": totals[f"{phase}_live_growth_bytes"],
                    "freed_or_superseded": totals[f"{phase}_allocated_bytes"] - totals[f"{phase}_live_growth_bytes"]}
            for phase in ["parse", "publish", "bind"]}
        # Global phase intervals can include scheduling requests on other threads;
        # this measured signed remainder must not be clamped into a fake zero.
        result["outside_phase_signed_remainder"] = {
            "requests": requests - sum(v["requests"] for v in result["phase_observations"].values()),
            "live_growth": growth - sum(v["live_growth"] for v in result["phase_observations"].values())}
    else:
        result["phase_observations"] = None
    return result


def model(census: dict, report: dict, sites: list, shapes: dict, layouts: dict) -> dict:
    rows = census["rows"]
    counts = {name[len("NodeData."):-len(".records")]: row["elements"]
              for name, row in rows.items() if name.startswith("NodeData.") and name.endswith(".records")}
    core = rows["core.nodes.page_payload"]
    overlays = rows["BindResult.nodes"]["elements"]
    lazy = rows.get("lazy.nodes.page_payload.initialized_slots", {}).get("elements", 0)
    if sum(counts.values()) != core["elements"] + overlays + lazy:
        raise ValueError("shape census does not reconcile to core + overlays + lazy")
    if lazy:
        raise ValueError("this initial core layout projection needs a separated lazy shape census")
    if set(counts) - set(shapes):
        raise ValueError(f"uncounted generated shape: {set(counts) - set(shapes)}")
    inventory = {}
    for name, shape in shapes.items():
        inventory[name] = {**shape, "observed_records_including_overlays": counts.get(name, 0),
                           "syntax_layout": layouts["Syntax" + name], "bound_layout": layouts["Bound" + name]}
    weighted = {mode: weighted_bounds(counts, {name: layouts[mode + name]["size"] for name in counts}, overlays)
                for mode in ["Syntax", "Bound"]}
    weighted["BindingIncrement"] = weighted_bounds(counts,
        {name: layouts["Bound" + name]["size"] - layouts["Syntax" + name]["size"] for name in counts}, overlays)
    files = report["files"]
    directory = rows["core.nodes.page_directory"]["capacity_payload_bytes"]
    fixed_directory = files * len(shapes) * layouts["VecHeader"]["size"]
    candidates = []
    for header in ["Header24", "Header32"]:
        header_used = core["elements"] * layouts[header]["size"]
        header_capacity = core["capacity_elements"] * layouts[header]["size"]
        total_low = header_capacity + weighted["Bound"]["lower"] + directory + fixed_directory
        total_high = header_capacity + weighted["Bound"]["upper"] + directory + fixed_directory
        # Header occupancy is a sensitivity, not a measurement of the future
        # per-shape pages. A separate exact per-file census is still required.
        spare_proxy = (weighted["Bound"]["upper"] * (core["capacity_elements"] - core["elements"])
                       + core["elements"] - 1) // core["elements"]
        candidates.append({"header": header, "header_used_bytes": header_used,
            "header_capacity_at_current_slot_count": header_capacity,
            "current_header_page_directory_capacity": directory,
            "fixed_per_file_shape_vec_headers": fixed_directory,
            "used_payload_plus_header_capacity_range": {"lower": total_low, "upper": total_high},
            "upper_with_header_occupancy_payload_spare_proxy": total_high + spare_proxy,
            "payload_spare_proxy_bytes_unmeasured": spare_proxy,
            "remaining_of_880mb_before_payload_spare_escapes_and_runtime_ids": BUDGET["syntax_and_binding"] - total_high,
            "syntax_only_before_payload_spare_escapes_and_runtime_ids": header_capacity + weighted["Syntax"]["upper"] + directory + fixed_directory,
            "feasibility": "unproved"})
    capacity = sum(row["capacity_payload_bytes"] for row in rows.values())
    arc = sum(row["shared_header_estimate_bytes"] for row in rows.values())
    live = report["retained_endpoint"]["live_requested_bytes"]
    relevant = [row for row in sites if row["scope_kind"] == "inclusive_site" and row["requested_bytes"]
                and (row["name"].startswith(("scanner.", "string.", "syntax.", "binding.", "symbol."))
                     or row["name"] == "arena.page_and_directory_growth")]
    scoped = {}
    for phase in ["parse", "bind", "publish"]:
        totals = [row["requested_bytes"] for row in sites if row["phase"] == phase and row["scope_kind"] == "phase"]
        unions = [row["requested_bytes"] for row in sites if row["phase"] == phase and row["scope_kind"] == "selected_union"]
        if len(totals) != 1 or len(unions) != 1 or unions[0] > totals[0]:
            raise ValueError(f"invalid source-scope union for {phase}")
        scoped[phase] = {"phase_requests": totals[0], "selected_source_union_requests": unions[0],
                         "unclassified_requests": totals[0] - unions[0]}
    unknowns = ["Core/overlay shape split and per-file/per-shape capacities require a new census; only rigorous weighted bounds are available.",
        "Actual field occupancy, escape counts/control allocation, foreign/lazy fallback, pooled-name index and runtime-identity storage remain unmeasured.",
        "Per-shape page slack/directory allocation and allocator map controls are not supplied by the old census.",
        "The native live-minus-census remainder is retained explicitly; it exceeds the entire 50 MB other/unknown budget.",
        "Structural temporary request sites are inclusive, not disjoint freed traffic; list Vec-to-box conversion need not physically copy.",
        "No replacement auxiliary/symbol/flow/list/table implementation is measured; their budget ceilings are not forecasts.",
        "Only the executing rustc host layout is measured; three other supported native targets remain to be checked."]
    return {"schema": 1, "diagnostic_only": True, "outcome": "feasibility_unproved",
        "workload": {key: report[key] for key in ["files", "loaded_bytes", "loaded_input_sha256", "nodes", "symbols"]},
        "budgets_bytes": BUDGET, "native_traffic": traffic_model(report),
        "baseline_census_reconciliation": {"capacity_payload_bytes": capacity, "arc_header_estimate_bytes": arc,
            "known_plus_estimate": capacity + arc, "native_endpoint_live_bytes": live,
            "unattributed_requested_live_bytes": live - capacity - arc,
            "known_spare_payload_bytes": sum(row["capacity_payload_bytes"] - row["used_payload_bytes"] for row in rows.values())},
        "shape_count": len(shapes), "core_records": core["elements"], "overlay_records_deduplicated": overlays,
        "weighted_payload_bounds": weighted, "candidate_layouts": candidates, "shape_inventory": inventory,
        "directory_and_escape_sensitivity": {"per_file_fixed_vec_headers_bytes": len(shapes) * layouts["VecHeader"]["size"],
            "all_file_fixed_vec_headers_bytes": fixed_directory,
            "one_all_node_exception_bitmap_bytes": (core["elements"] + 7) // 8,
            "exception_map_bytes": None, "one_all_node_u32_directory_bytes": core["elements"] * 4,
            "one_rare_shape_fixed_256_slot_page": {name: layouts["Bound" + name]["size"] * 256 for name in shapes},
            "note": "Bitmap is charged once per field/index domain that needs it; actual per-file rounding and exception containers add cost. No all-node directory is included twice with the header ordinal."},
        "structural_request_sites_inclusive_do_not_sum": relevant,
        "separate_scope_capture_request_coverage": scoped, "unknowns": unknowns}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target/s07-bis-layout")
    parser.add_argument("--rustc", default="rustc", help="compiler command; effective version is recorded")
    args = parser.parse_args()
    output = args.output_dir.resolve()
    source_path, schema_path = ROOT / "crates/tsr_ast/src/data_generated.rs", ROOT / "upstream/tools/scripts/tsc/ast.json"
    source, schema_data = source_path.read_bytes(), schema_path.read_bytes()
    shapes = schema_shapes(source.decode(), strict_json(schema_data))
    layouts, build = compile_layouts(shapes, output, args.rustc)
    inputs, hashes = archived_inputs()
    result = model(inputs["native/rust-1-0-census.json"], inputs["native/rust-1-0-report.json"],
                   inputs["sites/rust-1-0-sites.json"], shapes, layouts)
    result["native_traffic_eight_workers"] = traffic_model(inputs["native/rust-8-0-report.json"])
    result["provenance"] = {"archived_inputs_sha256": hashes,
        "generated_ast_sha256": digest(source), "upstream_schema_sha256": digest(schema_data),
        "model_sha256": digest(Path(__file__).read_bytes()),
        "archive_manifest_sha256": digest((ARCHIVE / "manifest.json").read_bytes()),
        "sketch_sha256": digest((HERE / "layout/sketch.rs").read_bytes()), **build}
    result["measured_layouts"] = layouts
    (output / "report.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"report": str(output / "report.json"), "outcome": result["outcome"],
                      "candidate_layouts": result["candidate_layouts"]}, indent=2))


if __name__ == "__main__":
    main()
