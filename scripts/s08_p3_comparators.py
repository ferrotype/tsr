"""Replay P3 comparator observations against the frozen P0 native capture.

This checks signs, exact permutation inventories and the foreign-owner boundary.
It does not certify the relation diagnostic/display protocol or E2 acceptance.
"""
import argparse
import json
from pathlib import Path

from s08_oracle import digest, strict_json_loads
from s04 import same_json_value


def signs(value):
    if type(value) is list:
        return [signs(item) for item in value]
    if type(value) is not int:
        raise ValueError("comparator result must be an integer")
    return (value > 0) - (value < 0)


def compare(request_raw, native_raw, inputs, actual):
    request_hash, native_hash = digest(request_raw), digest(native_raw)
    native = strict_json_loads(native_raw)
    requests = strict_json_loads(request_raw)
    if type(inputs["version"]) is not int or inputs["version"] != 1:
        raise ValueError("unsupported ordering input version")
    for value in (inputs, actual):
        if value["request_sha256"] != request_hash or value["native_sha256"] != native_hash:
            raise ValueError("ordering capture provenance mismatch")
    if native["request_sha256"] != request_hash:
        raise ValueError("native capture names different requests")
    ids = [row["id"] for row in requests]
    if len(ids) != len(set(ids)) or set(inputs["cases"]) != set(ids):
        raise ValueError("ordering request inventory mismatch")
    if [row["id"] for row in actual["rows"]] != ids or [row["id"] for row in native["rows"]] != ids:
        raise ValueError("ordering observation inventory mismatch")
    expected_residuals = dict(native["supplemental"]["residuals"])
    boundary = expected_residuals.pop("foreign-checker")
    if boundary != {"state": "panic", "message": "Cannot compare types from different checkers"}:
        raise ValueError("native foreign-checker boundary changed")
    if actual["foreign_checker"] != {"state": "rejected", "error": "Arena(WrongOwner)"}:
        raise ValueError("Rust failed to reject a foreign checker handle")
    if set(actual["residuals"]) != set(expected_residuals):
        raise ValueError("residual comparator inventory mismatch")
    for name, expected in expected_residuals.items():
        if not same_json_value(signs(expected), actual["residuals"][name]):
            raise ValueError(f"residual comparator differs: {name}")
    matrices = permutations = 0
    for expected, observed in zip(native["rows"], actual["rows"], strict=True):
        # The first mode supplies P0's frozen direct comparator inputs. Relations
        # execute in their own driver, so this is deliberately a direct replay.
        native_orders = expected["groups"][0]["union_ordering"]
        groups = inputs["cases"][expected["id"]]
        if len(groups) != len(observed["ordering"]):
            raise ValueError("missing comparator group")
        native_names = list(dict.fromkeys(row["type"] for row in native_orders))
        if [group["type"] for group in groups] != native_names:
            raise ValueError("native union inventory changed")
        for group, result in zip(groups, observed["ordering"], strict=True):
            name = group["type"]
            rows = [row for row in native_orders if row["type"] == name]
            pairwise = [row["pairwise"] for row in rows if "pairwise" in row]
            orders = [{"input": row["input"], "sorted": row["sorted"]} for row in rows if "input" in row]
            if len(pairwise) != 1 or [row["input"] for row in orders] != group["inputs"]:
                raise ValueError("frozen ordering input drift")
            if not same_json_value(result, {"state": "executed", "type": name, "pairwise": signs(pairwise[0]), "permutations": orders}):
                raise ValueError(f"comparator observation differs: {expected['id']}/{name}")
            matrices += 1
            permutations += len(orders)
    return {"matched": True, "request_sha256": request_hash, "native_sha256": native_hash,
            "residual_families": len(expected_residuals), "union_matrices": matrices,
            "permutations": permutations,
            "boundary": "Go panic and Rust WrongOwner both reject foreign checker handles",
            "scope": "direct comparator signs and exact permutations; not full P0 or E2"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("requests", "native", "inputs", "actual", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    args = parser.parse_args()
    result = compare(args.requests.read_bytes(), args.native.read_bytes(),
                     strict_json_loads(args.inputs.read_bytes()), strict_json_loads(args.actual.read_bytes()))
    result["actual_sha256"] = digest(args.actual.read_bytes())
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
