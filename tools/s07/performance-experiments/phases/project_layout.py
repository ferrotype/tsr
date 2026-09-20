#!/usr/bin/env python3
"""Project typed page/directory policies from the measured per-file core census.

All sizes are requested payload sizes. This executes arithmetic, not replacement
storage, allocation profiling or acceptance. Full-range escapes remain unknown.
"""
import argparse
from collections import Counter
from functools import lru_cache
import gzip
import json
from pathlib import Path
from statistics import median
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE))
import probe
from s04_common import strict_json_loads

POLICIES = [(1, 32), (1, 64), (1, 128), (2, 32), (2, 64), (2, 128), (2, 256), (4, 32), (4, 64)]
STRATEGIES = ("fixed_vec", "optional_box_vec", "packed_u16_index", "optional_one_or_many", "optional_inline_first")


@lru_cache(None)
def page_plan(count, first, limit):
    if count < 0 or first < 1 or limit < first or first & (first - 1) or limit & (limit - 1):
        raise ValueError("page policy needs nonnegative count and power-of-two bounds")
    capacity, pages, size = 0, 0, first
    while count > capacity and size < limit:
        capacity += size
        pages += 1
        size *= 2
    if count > capacity:
        extra = (count - capacity + limit - 1) // limit
        capacity += extra * limit
        pages += extra
    return capacity, pages


@lru_cache(None)
def vector_plan(count, element_size, first=4):
    if count < 0 or element_size < 0 or first < 1:
        raise ValueError("invalid directory vector inputs")
    if not count:
        return (0, 0, 0)
    capacity, requests, allocations = first, first * element_size, 1
    while capacity < count:
        capacity *= 2
        requests += capacity * element_size
        allocations += 1
    return capacity * element_size, requests, allocations


def compile_directory_sketch(output):
    output.mkdir(parents=True, exist_ok=True)
    executable = output / "directory-sketch"
    source = HERE / "directory_sketch.rs"
    subprocess.run(["rustc", "--edition=2021", "-Dwarnings", str(source), "-o", str(executable)], check=True)
    version = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True).stdout
    raw = subprocess.run([str(executable)], capture_output=True, text=True, check=True).stdout
    return ({name: {"size": int(size), "alignment": int(align)} for name, size, align in map(str.split, raw.splitlines())},
            {"rustc": version, "source_sha256": probe.runner.digest(source), "binary_sha256": probe.runner.digest(executable)})


def page_costs(rows, shapes, first, limit, sizes, page_size=None):
    result = {"used_syntax": 0, "used_bound": 0, "capacity_syntax": 0, "capacity_bound": 0,
              "payload_pages": 0, "active_shapes": 0, "one_page_shapes": 0,
              "directory_vec_live": 0, "directory_vec_requests": 0, "directory_vec_allocations": 0,
              "many_directory_live": 0, "many_directory_requests": 0, "many_directory_allocations": 0,
              "tail_directory_live": 0, "tail_directory_requests": 0, "tail_directory_allocations": 0}
    active_per_file = []
    for row in rows:
        active = 0
        for name, count in row["core_shapes"].items():
            syntax, bound = shapes[name]["syntax_layout"]["size"], shapes[name]["bound_layout"]["size"]
            if not bound:
                continue
            active += 1
            capacity, pages = page_plan(count, first, limit)
            result["used_syntax"] += count * syntax
            result["used_bound"] += count * bound
            result["capacity_syntax"] += capacity * syntax
            result["capacity_bound"] += capacity * bound
            result["payload_pages"] += pages
            result["one_page_shapes"] += pages == 1
            for label, length, initial in [("directory_vec", pages, 4),
                                            ("many_directory", pages if pages > 1 else 0, 2),
                                            ("tail_directory", pages - 1, 1)]:
                live, requests, allocations = vector_plan(length, page_size or sizes["Page"]["size"], initial)
                result[label + "_live"] += live
                result[label + "_requests"] += requests
                result[label + "_allocations"] += allocations
        active_per_file.append(active)
    result["active_shapes"] = sum(active_per_file)
    return result, active_per_file


def lean_directory_costs(strategy, costs, active_per_file, sizes, page_kind, ledger, shape_count=192):
    """Charge complete typed stores, including logical len and counter ledger.

    The owner-ledger variant charges its one ledger separately in the header
    envelope. The packed variant has an explicit u8 tag, sufficient for all
    192 shapes; capacities and superseded entry arrays remain visible.
    """
    files, active = len(active_per_file), costs["active_shapes"]
    stem = page_kind + ("Owner" if ledger == "owner" else "")
    store = sizes[stem + "Store"]["size"]
    root = files * shape_count * sizes["OptionalPointer"]["size"]
    storage, storage_requests, boxes = 0, 0, 0
    label = "directory_vec"
    if strategy == "fixed_vec":
        root = files * shape_count * store
    elif strategy == "optional_box_vec":
        storage = active * store
        storage_requests, boxes = storage, active
    elif strategy == "packed_u16_index":
        alignment = sizes["VecHeader"]["alignment"]
        unrounded = shape_count * 2 + sizes["VecHeader"]["size"]
        root = files * ((unrounded + alignment - 1) // alignment * alignment)
        plans = [vector_plan(n, sizes["Packed" + stem + "Store"]["size"], 1) for n in active_per_file]
        storage = sum(p[0] for p in plans)
        storage_requests, boxes = sum(p[1] for p in plans), sum(p[2] for p in plans)
    elif strategy == "optional_one_or_many":
        storage = active * sizes[stem + "OneOrManyStore"]["size"]
        storage_requests, boxes, label = storage, active, "many_directory"
    elif strategy == "optional_inline_first":
        storage = active * sizes[stem + "InlineStore"]["size"]
        storage_requests, boxes, label = storage, active, "tail_directory"
    else:
        raise ValueError("unknown lean directory policy")
    return {"live": root + storage + costs[label + "_live"],
            "requests": root + storage_requests + costs[label + "_requests"],
            "shape_roots": root, "shape_storage": storage,
            "page_directories": costs[label + "_live"],
            "allocations_excluding_embedded_root": boxes + costs[label + "_allocations"]}


def lean_header_costs(rows, first, limit, sizes, page_kind):
    # A header arena always needs exactly one ledger. With owner-wide tracking
    # this is the owner ledger and counts payload pages too; with per-store
    # tracking it counts header pages only. Either way all 48 root bytes count.
    result = header_costs(rows, 24, first, limit, {**sizes, "Page": sizes[page_kind + "Page"]})
    added = len(rows) * (sizes[page_kind + "Store"]["size"] - sizes["VecHeader"]["size"])
    result["directory_live"] += added
    result["directory_requests"] += added
    result["root_includes_used_length_and_ledger"] = True
    return result


def compact_page_candidates(rows, model, sizes):
    candidates = []
    # Thin Box<[T; N]> descriptors only apply to constant-N pages, including
    # the first page. Variable 1/2/.../N pages use fat Box<[T]> descriptors.
    payload_policies = [("Fat", first, limit) for first, limit in POLICIES]
    payload_policies += [("Thin", n, n) for n in (1, 2, 4, 8, 16, 32, 64)]
    header_policies = [("Fat", 2, 256), ("Fat", 1, 64)]
    header_policies += [("Thin", n, n) for n in (16, 32, 64, 128, 256)]
    headers = {(kind, first, limit): lean_header_costs(rows, first, limit, sizes, kind)
               for kind, first, limit in header_policies}
    for kind, first, limit in payload_policies:
        costs, active = page_costs(rows, model["shape_inventory"], first, limit, sizes, sizes[kind + "Page"]["size"])
        for ledger in ("store", "owner"):
            for strategy in STRATEGIES:
                directory = lean_directory_costs(strategy, costs, active, sizes, kind, ledger)
                for header_policy, header in headers.items():
                    live = header["capacity"] + header["directory_live"] + costs["capacity_bound"] + directory["live"]
                    requests = header["capacity"] + header["directory_requests"] + costs["capacity_bound"] + directory["requests"]
                    candidates.append({"representation": "default_initialized_box_pages", "ledger_scope": ledger,
                        "payload_policy": {"descriptor": kind, "first": first, "maximum": limit},
                        "header_policy": {"descriptor": header_policy[0], "first": header_policy[1], "maximum": header_policy[2]},
                        "directory_strategy": strategy, "live_accounted_bytes": live,
                        "requests_accounted_bytes": requests, "directory_superseded_requests": requests - live,
                        "remaining_of_880mb_before_unknowns": 880_000_000 - live,
                        "syntax_attributing_all_shared_directories_to_syntax": live - costs["capacity_bound"] + costs["capacity_syntax"],
                        "binding_payload_increment": costs["capacity_bound"] - costs["capacity_syntax"],
                        "payload_spare": costs["capacity_bound"] - costs["used_bound"],
                        "default_initialized_spare_storage_span": header["capacity"] - header["used"] + costs["capacity_bound"] - costs["used_bound"],
                        "payloads": costs, "header": header, "directory": directory,
                        "allocation_calls_excluding_embedded_owner_root": header["pages"] + header["directory_allocations"]
                            + costs["payload_pages"] + directory["allocations_excluding_embedded_root"],
                        "feasibility": "unproved"})
    candidates.sort(key=lambda item: item["live_accounted_bytes"])
    return candidates, headers


def flat_candidates(rows, model, sizes, headers):
    candidates = []
    shapes = model["shape_inventory"]
    for first in (1, 4):
        used, capacity, requests, allocations, active_counts = 0, 0, 0, 0, []
        syntax_capacity = 0
        for row in rows:
            active = 0
            for name, count in row["core_shapes"].items():
                bound, syntax = shapes[name]["bound_layout"]["size"], shapes[name]["syntax_layout"]["size"]
                if not bound:
                    continue
                active += 1
                live, requested, calls = vector_plan(count, bound, first)
                used += count * bound
                capacity += live
                requests += requested
                allocations += calls
                syntax_capacity += vector_plan(count, syntax, first)[0]
            active_counts.append(active)
        for ledger in ("store", "owner"):
            for strategy in ("fixed_vec", "optional_box_vec", "packed_u16_index"):
                # Flat vectors already carry their logical length in Vec.
                flat_sizes = {**sizes, "FlatOwnerStore": sizes["VecHeader"]}
                dummy = {"active_shapes": sum(active_counts), "directory_vec_live": 0,
                         "directory_vec_requests": 0, "directory_vec_allocations": 0}
                directory = lean_directory_costs(strategy, dummy, active_counts, flat_sizes, "Flat", ledger)
                for header_policy, header in headers.items():
                    live = header["capacity"] + header["directory_live"] + capacity + directory["live"]
                    requested = header["capacity"] + header["directory_requests"] + requests + directory["requests"]
                    candidates.append({"representation": "flat_payload_vectors", "ledger_scope": ledger,
                        "payload_policy": {"initial_capacity": first, "growth": "doubling"},
                        "header_policy": {"descriptor": header_policy[0], "first": header_policy[1], "maximum": header_policy[2]},
                        "directory_strategy": strategy, "live_accounted_bytes": live,
                        "requests_accounted_bytes": requested, "superseded_requests": requested - live,
                        "remaining_of_880mb_before_unknowns": 880_000_000 - live,
                        "payload_vector_superseded_requests": requests - capacity,
                        "payload_used": used, "payload_capacity": capacity,
                        "payload_requests": requests, "payload_allocations": allocations,
                        "syntax_attributing_all_shared_directories_to_syntax": live - capacity + syntax_capacity,
                        "binding_payload_increment": capacity - syntax_capacity,
                        "header": header, "directory": directory,
                        "allocation_calls_excluding_embedded_owner_root": header["pages"] + header["directory_allocations"]
                            + allocations + directory["allocations_excluding_embedded_root"],
                        "feasibility": "unproved"})
    candidates.sort(key=lambda item: item["live_accounted_bytes"])
    return candidates


def word_class_candidates(rows, model, sizes, headers):
    """Bounded word-row model, not generated accessors or a production API.

    Replacing each composite's atomic word with a facts-table index retains the
    existing bound word width. Its AtomicU32 is separately charged. Shape tags
    remain in the header and select field decoding; no typed references are
    projected into scalar word storage.
    """
    shapes = model["shape_inventory"]
    widths = sorted({s["bound_layout"]["size"] // 4 for s in shapes.values() if s["bound_layout"]["size"]})
    if any(s["bound_layout"]["size"] % 4 or s["bound_layout"]["alignment"] > 4 for s in shapes.values()):
        raise ValueError("word rows need an explicitly handled aligned scalar schema")
    classes = {str(width): {"syntax_layout": sizes["Word" + str(width)], "bound_layout": sizes["Word" + str(width)]}
               for width in widths}
    grouped, facts_rows, binding_used = [], [], 0
    for row in rows:
        counts, facts = Counter(), 0
        for name, count in row["core_shapes"].items():
            shape = shapes[name]
            if shape["bound_layout"]["size"]:
                counts[str(shape["bound_layout"]["size"] // 4)] += count
            if shape["facts"]:
                facts += count
            binding_used += count * (shape["bound_layout"]["size"] - shape["syntax_layout"]["size"])
        grouped.append({"core_shapes": dict(counts)})
        facts_rows.append({"core_shapes": {"facts": facts}})
    facts_policies = {}
    for count in (32, 64, 128, 256):
        fact = header_costs(facts_rows, sizes["AtomicFacts"]["size"], count, count, {**sizes, "Page": sizes["ThinPage"]})
        # Owner ledger already lives in the header store; retain the facts
        # store's own checked logical length, including for a file with no facts.
        length_bytes = len(rows) * sizes["LogicalLength"]["size"]
        fact["directory_live"] += length_bytes
        fact["directory_requests"] += length_bytes
        facts_policies[count] = fact
    candidates = []
    payload_policies = [("Thin", n, n) for n in (4, 8, 16, 32, 64)] + [("Fat", 2, n) for n in (32, 64, 128)]
    for kind, first, maximum in payload_policies:
        costs, active = page_costs(grouped, classes, first, maximum, sizes, sizes[kind + "Page"]["size"])
        for strategy in STRATEGIES:
            directory = lean_directory_costs(strategy, costs, active, sizes, kind, "owner", len(classes))
            for header_policy, header in headers.items():
                for facts_policy, facts in facts_policies.items():
                    live = sum((header["capacity"], header["directory_live"], costs["capacity_bound"], directory["live"], facts["capacity"], facts["directory_live"]))
                    requests = sum((header["capacity"], header["directory_requests"], costs["capacity_bound"], directory["requests"], facts["capacity"], facts["directory_requests"]))
                    candidates.append({"representation": "word_classes_plus_indexed_atomics", "ledger_scope": "owner",
                        "payload_policy": {"descriptor": kind, "first": first, "maximum": maximum},
                        "header_policy": {"descriptor": header_policy[0], "first": header_policy[1], "maximum": header_policy[2]},
                        "facts_policy": {"descriptor": "Thin", "first": facts_policy, "maximum": facts_policy},
                        "directory_strategy": strategy, "live_accounted_bytes": live,
                        "requests_accounted_bytes": requests, "directory_superseded_requests": requests - live,
                        "remaining_of_880mb_before_unknowns": 880_000_000 - live,
                        "syntax_attributing_all_shared_directories_and_spare_to_syntax": live - binding_used,
                        "binding_used_increment": binding_used,
                        "extra_used_atomic_storage": facts["used"],
                        "default_initialized_spare_storage_span": header["capacity"] - header["used"] + costs["capacity_bound"] - costs["used_bound"] + facts["capacity"] - facts["used"],
                        "header": header, "payloads": costs, "directory": directory, "facts": facts,
                        "allocation_calls_excluding_embedded_owner_root": header["pages"] + header["directory_allocations"]
                            + costs["payload_pages"] + directory["allocations_excluding_embedded_root"] + facts["pages"] + facts["directory_allocations"],
                        "feasibility": "unproved"})
    candidates.sort(key=lambda item: item["live_accounted_bytes"])
    atomic = []
    if sizes["AtomicWord1"] != sizes["Word1"] or sizes["AtomicWord16"] != sizes["Word16"]:
        raise ValueError("AtomicU32 word arrays differ from the scalar layout model")
    for candidate in candidates:
        if candidate["facts_policy"]["first"] != 32:
            continue  # No facts table in this variant; avoid four duplicates.
        facts = candidate["facts"]
        item = {key: value for key, value in candidate.items() if key not in ("facts", "facts_policy", "extra_used_atomic_storage")}
        item["representation"] = "atomic_word_classes"
        item["live_accounted_bytes"] -= facts["capacity"] + facts["directory_live"]
        item["requests_accounted_bytes"] -= facts["capacity"] + facts["directory_requests"]
        item["directory_superseded_requests"] = item["requests_accounted_bytes"] - item["live_accounted_bytes"]
        item["remaining_of_880mb_before_unknowns"] = 880_000_000 - item["live_accounted_bytes"]
        item["syntax_attributing_all_shared_directories_and_spare_to_syntax"] = item["live_accounted_bytes"] - binding_used
        item["default_initialized_spare_storage_span"] -= facts["capacity"] - facts["used"]
        item["allocation_calls_excluding_embedded_owner_root"] -= facts["pages"] + facts["directory_allocations"]
        atomic.append(item)
    atomic.sort(key=lambda item: item["live_accounted_bytes"])
    return {"word_widths": widths, "active_file_class_pairs": sum(len(row["core_shapes"]) for row in grouped),
            "composite_nodes": sum(sum(row["core_shapes"].values()) for row in facts_rows),
            "lowest_accounted_candidate": candidates[0],
            "candidates": [{key: value for key, value in item.items() if key not in ("header", "payloads", "directory", "facts")}
                           for item in candidates],
            "atomic_word_variant": {"lowest_accounted_candidate": atomic[0],
                "candidates": [{key: value for key, value in item.items() if key not in ("header", "payloads", "directory")}
                               for item in atomic],
                "additional_obligations": ["Every word is AtomicU32; private setters may use get_mut, while shared scalar getters must use explicit loads.",
                    "Existing composite-facts atomic ordering still applies. Relaxed scalar reads must not be treated as compiler-equivalent to plain loads.",
                    "The absence of a separate facts table saves bytes but is not evidence of neutral CPU cost; pilot actual hot accessors and bounds checks before migration."]},
            "additional_obligations": ["Generate checked field-value accessors, not references projected into u32 words; encode byte/string/link fallbacks without shrinking their public domains.",
                "Replace each composite atomic slot with an index and preserve atomic ordering in the separately owned facts table; index bounds and mutation/publication semantics need tests.",
                "Independent open syntax kind and concrete shape tags remain in the 24-byte header; shape-to-word-width mapping is static, but all public constructed shapes need coverage.",
                "Owner-ledger drop order, partial-construction cleanup, full-range escapes, runtime IDs and all other CP0 unknowns remain unproved."]}


def directory_costs(strategy, costs, active_per_file, sizes):
    files, active = len(active_per_file), costs["active_shapes"]
    if strategy == "fixed_vec":
        root, storage, label, boxes = files * sizes["FixedDirectory"]["size"], 0, "directory_vec", 0
    elif strategy == "optional_box_vec":
        root = files * sizes["OptionalDirectory"]["size"]
        storage, label, boxes = active * sizes["VecHeader"]["size"], "directory_vec", active
    elif strategy == "packed_u16_index":
        root, storage, label, boxes = files * sizes["PackedDirectory"]["size"], 0, "directory_vec", 0
        allocations = [vector_plan(count, sizes["PackedPages"]["size"], 1) for count in active_per_file]
        storage = sum(item[0] for item in allocations)
        requests = sum(item[1] for item in allocations)
        return {"live": root + storage + costs[label + "_live"],
                "requests": root + requests + costs[label + "_requests"],
                "shape_roots": root, "shape_storage": storage,
                "page_directories": costs[label + "_live"],
                "allocations_excluding_embedded_root": sum(item[2] for item in allocations) + costs[label + "_allocations"]}
    elif strategy == "optional_one_or_many":
        root = files * sizes["OptionalDirectory"]["size"]
        storage, label, boxes = active * sizes["OneOrMany"]["size"], "many_directory", active
    elif strategy == "optional_inline_first":
        root = files * sizes["OptionalDirectory"]["size"]
        storage, label, boxes = active * sizes["InlineFirst"]["size"], "tail_directory", active
    else:
        raise ValueError("unknown directory policy")
    return {"live": root + storage + costs[label + "_live"],
            "requests": root + storage + costs[label + "_requests"],
            "shape_roots": root, "shape_storage": storage, "page_directories": costs[label + "_live"],
            "allocations_excluding_embedded_root": boxes + costs[label + "_allocations"]}


def header_costs(rows, header_size, first, limit, sizes):
    result = {"used": 0, "capacity": 0, "page_slots": 0, "pages": 0, "directory_live": 0,
              "directory_requests": 0, "directory_allocations": 0}
    for row in rows:
        count = sum(row["core_shapes"].values())
        capacity, pages = page_plan(count, first, limit)
        live, requests, allocations = vector_plan(pages, sizes["Page"]["size"])
        result["used"] += count * header_size
        result["capacity"] += capacity * header_size
        result["page_slots"] += capacity
        result["pages"] += pages
        result["directory_live"] += live
        result["directory_requests"] += requests
        result["directory_allocations"] += allocations
    root = len(rows) * sizes["VecHeader"]["size"]
    result["directory_live"] += root
    result["directory_requests"] += root
    return result


def project(rows, model, sizes):
    shapes = model["shape_inventory"]
    total = Counter()
    for row in rows:
        total.update(row["core_shapes"])
    if set(total) - set(shapes):
        raise ValueError("shape census contains an unknown layout")
    if sizes["FixedDirectory"]["size"] != len(shapes) * sizes["VecHeader"]["size"]:
        raise ValueError("directory sketch shape count differs from generated schema")
    original = header_costs(rows, 80, 2, 256, sizes)
    if original["page_slots"] != 20_968_456 or original["pages"] != 156_916:
        raise ValueError("per-file page model does not reproduce archived core capacity")
    candidates = []
    for first, limit in POLICIES:
        costs, active = page_costs(rows, shapes, first, limit, sizes)
        for strategy in STRATEGIES:
            directory = directory_costs(strategy, costs, active, sizes)
            for header_first, header_limit in [(2, 256), (1, 64), (1, 32)]:
                header = header_costs(rows, 24, header_first, header_limit, sizes)
                live = header["capacity"] + header["directory_live"] + costs["capacity_bound"] + directory["live"]
                requests = header["capacity"] + header["directory_requests"] + costs["capacity_bound"] + directory["requests"]
                syntax = header["capacity"] + header["directory_live"] + costs["capacity_syntax"] + directory["live"]
                candidates.append({"payload_policy": {"first": first, "maximum": limit},
                    "header_policy": {"first": header_first, "maximum": header_limit},
                    "directory_strategy": strategy, "live_accounted_bytes": live,
                    "requests_accounted_bytes": requests, "directory_superseded_requests": requests - live,
                    "remaining_of_880mb_before_unknowns": 880_000_000 - live,
                    "syntax_attributing_all_shared_directories_to_syntax": syntax,
                    "binding_payload_increment": costs["capacity_bound"] - costs["capacity_syntax"],
                    "payload_spare": costs["capacity_bound"] - costs["used_bound"],
                    "payloads": costs, "header": header, "directory": directory,
                    "allocation_calls_excluding_embedded_owner_root": header["pages"] + header["directory_allocations"]
                        + costs["payload_pages"] + directory["allocations_excluding_embedded_root"],
                    "feasibility": "unproved"})
    candidates.sort(key=lambda item: item["live_accounted_bytes"])
    ordinary = next(item for item in candidates if item["payload_policy"] == {"first": 2, "maximum": 256}
                    and item["header_policy"] == {"first": 2, "maximum": 256} and item["directory_strategy"] == "fixed_vec")
    payload_used = sum(total[name] * shapes[name]["bound_layout"]["size"] for name in total)
    syntax_used = sum(total[name] * shapes[name]["syntax_layout"]["size"] for name in total)
    compact, headers = compact_page_candidates(rows, model, sizes)
    flat = flat_candidates(rows, model, sizes, headers)
    compact_best = {kind + "_" + ledger: next(item for item in compact
        if item["payload_policy"]["descriptor"] == kind and item["ledger_scope"] == ledger)
        for kind in ("Fat", "Thin") for ledger in ("store", "owner")}
    summarize = lambda item: {key: value for key, value in item.items() if key not in ("payloads", "header", "directory")}
    return {"version": 2, "diagnostic_only": True, "outcome": "feasibility_unproved",
        "files": len(rows), "core_nodes": sum(total.values()), "observed_shapes": len(total),
        "generated_shapes": len(shapes), "active_file_shape_pairs": sum(len(row["core_shapes"]) for row in rows),
        "active_shapes_per_file": {"minimum": min(len(row["core_shapes"]) for row in rows),
            "median": median(len(row["core_shapes"]) for row in rows), "maximum": max(len(row["core_shapes"]) for row in rows)},
        "exact_weighted_used_bytes": {"syntax": syntax_used, "bound": payload_used, "binding_increment": payload_used - syntax_used},
        "core_shape_counts": dict(sorted(total.items())), "directory_layouts": sizes,
        "original_header_page_model_reproduction": original,
        "naive_existing_page_policy_with_fixed_shape_vectors": ordinary,
        "lowest_accounted_candidate": candidates[0], "candidates": candidates,
        "compact_box_pages": {"best_by_representation_and_ledger": compact_best,
            "lowest_accounted_candidate": compact[0], "candidates": list(map(summarize, compact))},
        "flat_payload_vectors": {"lowest_accounted_candidate": flat[0], "candidates": list(map(summarize, flat))},
        "word_class_pages": word_class_candidates(rows, model, sizes, headers),
        "unknowns": ["Replacement payload pages/directories are projected policies, not actual allocated capacities or measured native requests.",
            "Full-range parent/edge/text escapes, runtime IDs, compatibility state, pooled-name metadata and owner roots beyond the charged directory fields remain unknown.",
            "Unused or checker-only deferred schema fields are still excluded as explicitly recorded by CP0; uncommon constructed payload eligibility needs its fallback.",
            "Allocator rounding/control data and the 257.480 MB baseline requested-live remainder are not solved by this model.",
            "Current Page descriptors and default-initialized Box pages are separate models. Box models charge logical lengths and either per-store or owner-level ledgers; neither drop/counter protocol is implemented or proved.",
            "Box spare storage is physically initialized but logically invalid; safe initialization can add stores or stack temporaries. Its CPU cost and padding writes are not inferred from the storage span.",
            "Flat payload vectors account for full replacement requests during growth. Whether relocation is semantically allowed in a new exclusive storage design still needs proof.",
            "A low byte count does not establish low CPU cost: optional directories add indirection, packed directories add tag/index routing, and smaller pages add allocations.",
            "This prices only syntax/binding and their directory growth; scanner/list/string/symbol/flow traffic must still fit the independent 350 MB traffic ceiling."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--build", type=Path, required=True)
    parser.add_argument("--build-sha", required=True)
    parser.add_argument("--output", type=Path, default=HERE / "layout-projection.json.gz")
    parser.add_argument("--scratch", type=Path, default=ROOT / "target/s07-bis-directory-layout")
    args = parser.parse_args()
    capture_path, build_path = args.capture.resolve(), args.build.resolve()
    build = probe.validate_build(build_path, args.build_sha)
    capture = strict_json_loads((capture_path / "report.json").read_bytes())
    if (capture.get("status") != "complete" or capture.get("kind") != "owned_core_shapes"
            or capture["build_manifest_sha256"] != args.build_sha):
        raise ValueError("shape capture does not identify this completed immutable build")
    for name, sha in capture["raw_sha256"].items():
        path = capture_path / name
        if not path.resolve().is_relative_to(capture_path) or probe.runner.digest(path) != sha:
            raise ValueError("raw shape child output changed")
    if probe.runner.digest(capture_path / "observations.json") != capture["observations_sha256"]:
        raise ValueError("shape observation bytes changed")
    observation = strict_json_loads((capture_path / "sample-0-consuming.stdout").read_bytes())
    probe.validate_observation(observation, build["expected_work"], "consuming", True)
    shape_path = capture_path / "core-shapes.ndjson"
    if probe.runner.digest(shape_path) != capture["summary"]["core_shapes_sha256"]:
        raise ValueError("shape capture bytes changed")
    shape_summary = probe.validate_shapes(shape_path, build["expected_work"])
    if shape_summary != capture["summary"]["core_shapes"] or shape_summary["bound_in_place_files"] != observation["bound_in_place_files"]:
        raise ValueError("shape summary and child binding paths disagree")
    data = shape_path.read_bytes()
    rows = [strict_json_loads(line) for line in data.splitlines()]
    model_path = HERE.parent / "layout/baseline-model.json"
    model = strict_json_loads(model_path.read_bytes())
    if (build["source_fingerprint"]["files"]["crates/tsr_ast/src/data_generated.rs"]
            != model["provenance"]["generated_ast_sha256"]):
        raise ValueError("compiled payload layouts belong to a different generated schema")
    sizes, compilation = compile_directory_sketch(args.scratch.resolve())
    result = project(rows, model, sizes)
    result["provenance"] = {"shape_capture_report_sha256": probe.runner.digest(capture_path / "report.json"),
        "core_shapes_sha256": probe.runner.digest(shape_path), "build_manifest_sha256": args.build_sha,
        "build_source_fingerprint": build["source_fingerprint"]["sha256"], "build_revision": build["revision"],
        "generated_layout_model_sha256": probe.runner.digest(model_path), "projection_script_sha256": probe.runner.digest(Path(__file__)),
        "loaded_input_sha256": build["expected_work"]["loaded_input_sha256"], "directory_compilation": compilation,
        "note": "Binary/source provenance comes from the immutable phase build, not the newer checkout census-launch fingerprint."}
    encoded = (json.dumps(result, indent=2, sort_keys=True) + "\n").encode()
    args.output.write_bytes(gzip.compress(encoded, mtime=0) if args.output.suffix == ".gz" else encoded)
    (args.scratch / "layout-projection.json").write_bytes(encoded)
    (HERE / "core-shapes.ndjson.gz").write_bytes(gzip.compress(data, mtime=0))
    proof = {"capture": capture, "build_manifest": build,
             "raw_stdout": (capture_path / "sample-0-consuming.stdout").read_text(),
             "raw_stderr": (capture_path / "sample-0-consuming.stderr").read_text()}
    (HERE / "core-shapes-provenance.json.gz").write_bytes(gzip.compress(json.dumps(proof, sort_keys=True).encode(), mtime=0))
    print(json.dumps({"active_file_shape_pairs": result["active_file_shape_pairs"],
        "exact_weighted_used_bytes": result["exact_weighted_used_bytes"],
        "compact_box_best": result["compact_box_pages"]["best_by_representation_and_ledger"],
        "flat_best": result["flat_payload_vectors"]["lowest_accounted_candidate"]}, indent=2))


if __name__ == "__main__":
    main()
