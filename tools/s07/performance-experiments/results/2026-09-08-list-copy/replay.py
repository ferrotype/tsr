#!/usr/bin/env python3
"""Offline list-copy checkpoint replay; no native executable is run."""
import argparse
from contextlib import redirect_stderr
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import shlex
import tarfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
BASE_PATH = HERE.parent / "2026-09-08-cp1/replay.py"
BASE_SHA = "78ef7c105a2e4ff69a976d0f6273033c4b0a0731d778b288668bd7c254358b13"
if hashlib.sha256(BASE_PATH.read_bytes()).hexdigest() != BASE_SHA:
    raise ValueError("the frozen CP1 replay dependency changed")
SPEC = importlib.util.spec_from_file_location("list_copy_frozen_cp1_replay", BASE_PATH)
base = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(base)
from s07_depth import native_rows
runner = base.runner
require, sha, identity, safe_name = base.require, base.sha, base.identity, base.safe_name
strict_json_loads, canonical = base.strict_json_loads, base.canonical
KIND = "s07_bis_cp1_list_copy_archive"
POLICY = {**base.POLICY, "checkpoint": "CP1 bounded syntax-list ID copies"}

# Filled only from the root's independently recorded captures. Missing pins
# deliberately prevent preparation/packaging; tests inject small observations.
CAPTURE = {
    "source_manifest_sha256": {
        "control": "3a57976f667b9857891edbe9f76de5261c43480ac8c62d946b4545d9ee19d931",
        "candidate": "b03b64ebf3faed4bb2245fa7b76db4289c3a1e510e796e86337c0782fb5f3578",
    },
    "graph_report_sha256": "c6ce91ed6eff72b9dfb771fb2951e0996b09dc7d4d6d317b21622b349daa5875",
    "screen_report_sha256": "8ccc065b2c33b45ddfed83ca3b678c7d0c87d927f017aaf7e6026ab7680f4f10",
    "e3_evidence_sha256": "31cde17440f7568517c8a5ad81ab473e959848d05bb80cac5774b66e2dc66914",
    "e3_inventory_sha256": "5f0f7a9050e690075c25d16d64d2273e07dce50db1cdcf3b4d4a14a39a8b4577",
    "binder_evidence_sha256": "fd6389d0fb7035c747e2991e37b6e595d87dad2c91e16fc2730c5f970b668334",
    "code_review_receipt_sha256": "54a06fecee4bb4655a657a581996062672fa7339d6cd0908be021accc95af964",
    "binder_raw_sha256": {
        "report.json": "f00267af701ca079d82bbc4748bc1598ecdff85a23155dca418ae2b35287d26a",
        "requests.ndjson": "3322a8d2afbad685fce923143f9e914a70f0d82a9ea6772200e7ae86bb79ba8f",
    },
    "depth_raw_sha256": {
        "report.json": "71953670e7c38c491d9a9e8b29e1791983e5343ede16017afb49992e65a55c60",
        "native.log": "c6932ca0c0bc3c972a663efd7e01853c8b42a212fda8eb0c3a121cb8bef4204e",
        "oracle.stderr": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "rust.stderr": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    },
    "depth_cases_sha256": "a25a3166c2e24462324859b20a660f3ea9db6bd17117eb5bccd47e16905718c9",
    "proof_logs": {
        "cp1-list-copy-binder.log": "cd7d0a9cc37fad943f94a464668b7c589f9464d07b3d19be86a326a2a952481e",
        "cp1-list-copy-screen.log": "84e3de206ab7d97a4ddd91d380e1487e876757f32ccb321d472fb0f11e09b338",
        "cp1-list-copy-graphs.log": "24f5975c0982a8404f406173b2ba7f2a878cca72493f26b003349f381fd07693",
        "cp1-list-copy-build.log": "91408b2a3dd180f958bb84717b454b221740df828da17b08b0d03480a754b236",
        "cp1-list-copy-e3.log": "cb2c7c040104a6b9ea1b3ee5eae9dc14f0ebc0c650c2a3c274d95e52017df6e4",
        "cp1-list-copy-workspace-tests.log": "18d0dc43a6e4ce01667522d5b0b1cef49e2b3280ab70bd832701c4a1d452aa87",
        "cp1-list-copy-ast-doctests.log": "5dc3be586ce3c89482b3f9cd15ee5f880ec38a4abb6d72163f07ab74ec1b1a81",
    },
    "failed_attempts": {
        "cp1-list-copy-build-preflight-failed.log": "9db6adefeead782c1549e4835506165d7d0fff50a429a151c3bcd704c80d5ed1",
        "cp1-list-copy-binder-first-depth-failed.log": "533c7648b36a0cb887e631c1b1a1032dfd40a8d0c2a9f6e4910483dec1a2c5c1",
        "cp1-list-copy-first-binder/depth/native.log": "1ea3d1feabb7b9834de1551ff79a6bb66b82839cfe79852c47b0f51904aa1ee5",
        "cp1-list-copy-first-binder/depth/report.json": "7b165a9c05b2ae389405b110ffb0723f07576d0c81a862124b8bdc53d2bd1098",
        "cp1-list-copy-first-binder/evidence.json": "923e6da7c22f05e463303b8233e7b86873ecd7ac504153c8bb9722cc2ce538dc",
        "cp1-list-copy-first-binder/failures.ndjson": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "cp1-list-copy-first-binder/oracle.stderr": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "cp1-list-copy-first-binder/preservation.json": "57590f20d5a047b943c9f9897ddba5f91232ed8268427191f4e05c46cd73a243",
        "cp1-list-copy-first-binder/producer/binder-depth-cases.json": "a25a3166c2e24462324859b20a660f3ea9db6bd17117eb5bccd47e16905718c9",
        "cp1-list-copy-first-binder/producer/s07_depth.py": "49a100666cabe1383eccfe31aa00bb711be3592dd97f58189435ade6793af6b1",
        "cp1-list-copy-first-binder/producer-preservation.json": "d2cff8b97666db5f3c70f4155a1a2c6957d6a3ff59c065995bb741e4caca3fdf",
        "cp1-list-copy-first-binder/report.json": "7d9fd08201d50d9a8f8357ede8beae682abbcb6aedccb08e9b1766e2f8815a73",
        "cp1-list-copy-first-binder/requests.ndjson": "3322a8d2afbad685fce923143f9e914a70f0d82a9ea6772200e7ae86bb79ba8f",
        "cp1-list-copy-first-binder/rust.stderr": "805fab3de26fc8f60a52fc272ae522e56f6d72db8a26a13a4c2648def3234a77",
    },
}
BINDER_METRICS = {
    "depth": True, "graph_contracts": True, "helper_tests": 18, "helpers": True,
    "primary_requests": 22343, "primary_rows": 12829, "protocol": True,
    "protocol_tests": 32, "reached_bind": True, "resolvers": True,
    "supplemental_parity": 1.0, "supplemental_requests": 18,
}


def configure():
    """Configure only this isolated import, leaving historical replay intact."""
    base.MANIFEST_SHAS = CAPTURE["source_manifest_sha256"]
    base.POLICY = POLICY


def validate_pins():
    values = [*CAPTURE["source_manifest_sha256"].values(),
              *(CAPTURE[key] for key in ("graph_report_sha256", "screen_report_sha256",
                  "e3_evidence_sha256", "e3_inventory_sha256", "binder_evidence_sha256",
                  "code_review_receipt_sha256", "depth_cases_sha256")),
              *CAPTURE["binder_raw_sha256"].values(), *CAPTURE["proof_logs"].values(),
              *CAPTURE["depth_raw_sha256"].values(),
              *CAPTURE["failed_attempts"].values()]
    require(all(type(value) is str and re.fullmatch(r"[0-9a-f]{64}", value)
                for value in values), "capture identities are not finalized")


def helper_files():
    # The old helper already unions imported modules and subprocess-only tools.
    helpers = base.helper_files()
    for path in (Path(__file__).resolve(), HERE / "test_replay.py"):
        helpers[path.relative_to(ROOT).as_posix()] = sha(path.read_bytes())
    return dict(sorted(helpers.items()))


def prepare(directory):
    validate_pins()
    configure()
    require(not (directory / "prepared.json").exists(), "preparation already exists")
    directory.mkdir(parents=True, exist_ok=True)
    helpers = helper_files()
    for name, expected in helpers.items():
        raw = (ROOT / name).read_bytes()
        require(sha(raw) == expected, "helper changed during preparation")
        path = directory / "prepared-tools" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(raw)
    value = {"version": 1, "kind": KIND, "policy": POLICY, "capture": CAPTURE,
             "helpers": helpers, "capture_tools": runner.tool_fingerprint()}
    runner.write_json(directory / "prepared.json", value)
    return {"prepared_sha256": sha((directory / "prepared.json").read_bytes()),
            "helper_count": len(helpers)}


def check_policy(summary, decision):
    result = base.check_policy(summary, decision)
    result["checkpoint_decision"] = result.pop("production_decision")
    result["final_production_promotion_established"] = False
    return result


def check_e3(members):
    raw = members["e3/evidence.json"]
    require(sha(raw) == CAPTURE["e3_evidence_sha256"], "E3 evidence identity changed")
    evidence = strict_json_loads(raw)
    require(evidence["exit_code"] == 0 and evidence["valid_capture"] is True,
            "E3 is not a completed valid capture")
    for label in ("stdout", "stderr"):
        require(members["e3/" + label] == evidence[label].encode()
                and sha(members["e3/" + label]) == evidence[label + "_sha256"],
                "E3 raw output changed")
    cases = members["e3/ownership-cases.json"]
    require(sha(cases) == CAPTURE["e3_inventory_sha256"], "exact list-copy E3 inventory changed")
    inventory = base.validate_manifest(strict_json_loads(cases))
    suites = {**inventory["common"], **inventory["groups"]}
    require(sum(len(suite["cases"]) for suite in suites.values()) == 32,
            "E3 inventory omitted list-copy cases")
    text = evidence["stderr"]
    starts = list(re.finditer(r"^\+ (.+)$", text, re.MULTILINE))
    modes = {mode: {} for mode in base.MODES}
    for position, match in enumerate(starts):
        args = shlex.split(match.group(1))
        selected = [name for name, suite in suites.items()
                    if suite["package"] in args and suite["filter"] in args]
        if not selected:
            continue
        require(len(selected) == 1, "ambiguous E3 ownership command")
        name = selected[0]
        mode = ("miri" if "miri" in args else "address_sanitizer" if "-Zbuild-std" in args
                else "release" if "--release" in args else "debug")
        require(name not in modes[mode], "duplicate E3 suite/mode")
        stop = starts[position + 1].start() if position + 1 < len(starts) else len(text)
        with redirect_stderr(io.StringIO()):
            base.validate_output(text[match.end():stop].encode(), suites[name]["cases"],
                                 mode, "archived list-copy ownership")
        modes[mode][name] = True
    measured = {"metrics": {}}
    base.publish_metrics(measured, modes, inventory)
    original = strict_json_loads(evidence["stdout"])
    require(all(original["metrics"].get(key) == value for key, value in measured["metrics"].items())
            and measured["metrics"]["program_ownership_tests"] == 32
            and all(measured["metrics"][name] for name in base.GROUPS),
            "E3 named outcomes differ from metrics")
    return {"evidence_sha256": CAPTURE["e3_evidence_sha256"], "distinct_cases": 32,
            "successful_named_test_observations": 128, "suites_per_mode": len(suites),
            "modes": sorted(modes), "metrics_sha256": sha(canonical(measured["metrics"])),
            "scope": "Recorded S07 cases only; not a new instrumentation run or full future E3 coverage"}


def check_binder(members, builds):
    for name, expected in CAPTURE["binder_raw_sha256"].items():
        require(sha(members["binder/" + name]) == expected, "binder raw capture changed: " + name)
    raw = members["binder/evidence.json"]
    require(sha(raw) == CAPTURE["binder_evidence_sha256"], "binder evidence identity changed")
    evidence = strict_json_loads(raw)
    require(evidence["exit_code"] == 0 and evidence["valid_capture"] is True, "binder capture failed")
    for label in ("stdout", "stderr"):
        require(sha(evidence[label].encode()) == evidence[label + "_sha256"],
                "binder evidence output changed")
    report = strict_json_loads(members["binder/report.json"])
    require(report["source_stable"] is True, "binder source changed during capture")
    source = builds["candidate"]["source_fingerprint"]["files"]
    for name, expected in report["production_inputs"].items():
        actual = source[name] if name in source else sha(members["binder/source/" + safe_name(name)])
        require(actual == expected, "binder production source differs from candidate: " + name)
    for name in ("oracle.stderr", "rust.stderr"):
        require("binder/" + name in members, "binder capture omitted child stderr")
    rows = [strict_json_loads(line) for line in members["binder/requests.ndjson"].splitlines()]
    require(len(rows) == report["requests"] == 22361 and len({row["id"] for row in rows}) == len(rows),
            "binder request observations are missing or duplicated")
    primary, reached = {}, {"oracle": 0, "rust": 0}
    for index, row in enumerate(rows):
        require(row["equal"] is True and row["first_difference"] is None, "binder request failed")
        if index < 22343:
            require(type(row["primary"]) is str, "primary binder observation lost its row")
            primary[row["primary"]] = primary.get(row["primary"], True) and row["equal"]
            for runtime in reached:
                reached[runtime] += any(stage["stage"] == "bind" for stage in row["stages"][runtime])
        else:
            require(row["primary"] is None, "supplemental observation became primary")
    require(len(primary) == report["primary_rows"] == report["passed_rows"] == 12829
            and report["primary_requests"] == 22343 and primary == report["tests"]
            and reached == report["reached_bind"] == {"oracle": 22343, "rust": 22343}
            and report["supplemental"] == {"requests": 18, "passed": 18, "parity": 1.0}
            and report["failed_requests"] == 0 and report["parity"] == 1.0
            and members["binder/failures.ndjson"] == b"", "binder aggregates differ from observations")
    output = strict_json_loads(evidence["stdout"])
    require(output["tests"] == {name: "pass" for name in primary},
            "binder evidence differs from observed rows")
    check_binder_metrics(output["metrics"])
    return {"evidence_sha256": CAPTURE["binder_evidence_sha256"],
            "production_inputs_verified": len(report["production_inputs"]), "primary_requests": 22343,
            "primary_rows": len(primary), "supplemental_requests": 18, "reached_bind": reached,
            "validated_metrics": output["metrics"],
            "scope": "Recorded per-request comparisons and row membership; full binder graph streams omitted"}


def check_binder_metrics(metrics):
    require(type(metrics) is dict and set(metrics) == set(BINDER_METRICS),
            "binder metric inventory changed")
    for name, expected in BINDER_METRICS.items():
        require(type(metrics[name]) is type(expected) and metrics[name] == expected,
                "binder metric failed or changed: " + name)


def check_depth(members, builds):
    for name, expected in CAPTURE["depth_raw_sha256"].items():
        require(sha(members["depth/" + name]) == expected, "depth raw capture changed: " + name)
    cases_raw = members["depth/cases.json"]
    require(sha(cases_raw) == CAPTURE["depth_cases_sha256"], "depth request inventory changed")
    inventory = strict_json_loads(cases_raw)
    report = strict_json_loads(members["depth/report.json"])
    require(report["schema"] == 1 and report["scope"] == "supplemental-binder-depth-only"
            and report["metrics"] == {"binder_depth": True}
            and report["source_changed_during_capture"] is False and "failure" not in report
            and report["requests_sha256"] == CAPTURE["depth_cases_sha256"], "depth capture did not pass")
    source = builds["candidate"]["source_fingerprint"]["files"]
    for name, expected in report["inputs"].items():
        actual = source[name] if name in source else sha(members["depth/source/" + safe_name(name)])
        require(actual == expected, "depth source differs from frozen candidate: " + name)
    native = native_rows(members["depth/native.log"], inventory)
    require(len(native) == 12 and native == report["native"]["rows"]
            and report["native"]["log_sha256"] == CAPTURE["depth_raw_sha256"]["native.log"],
            "depth native observations differ from raw log")
    requests, rows = inventory["graph_requests"], report["graph_rows"]
    require(len(rows) == len(requests) == 12 and [row["id"] for row in rows] == [row["id"] for row in requests]
            and len({row["id"] for row in rows}) == 12, "depth graph request inventory changed")
    for row in rows:
        require(row["passed"] is True and row["equal"] is True and row["first_difference"] is None
                and row["primary"] is None and set(row["stages"]) == {"oracle", "rust"}
                and all(stage["outcome"] == "ok" for stages in row["stages"].values() for stage in stages),
                "depth graph observation failed")
    return {"report_sha256": CAPTURE["depth_raw_sha256"]["report.json"], "native_observations": 12,
            "graph_observations": 12, "source_inputs_verified": len(report["inputs"]),
            "scope": "Raw native counter validation and recorded per-request comparisons; full depth graph streams omitted"}


def check_proofs(members):
    for prefix, records in (("proofs/", CAPTURE["proof_logs"]),
                            ("failed-attempts/", CAPTURE["failed_attempts"])):
        require({name.removeprefix(prefix) for name in members if name.startswith(prefix)} == set(records),
                "proof/failed-attempt inventory changed")
        for name, expected in records.items():
            require(sha(members[prefix + safe_name(name)]) == expected, "proof log changed: " + name)


def check_first_binder(members):
    prefix = "failed-attempts/cp1-list-copy-first-binder/"
    evidence = strict_json_loads(members[prefix + "evidence.json"])
    for label in ("stdout", "stderr"):
        require(sha(evidence[label].encode()) == evidence[label + "_sha256"],
                "first binder evidence output changed")
    require(evidence["exit_code"] == 0 and evidence["valid_capture"] is True,
            "first binder attempt was a measured diagnostic failure")
    metrics = strict_json_loads(evidence["stdout"])["metrics"]
    require(metrics == {**BINDER_METRICS, "depth": False} and metrics["depth"] is False,
            "first binder depth failure was erased or changed")
    report = strict_json_loads(members[prefix + "depth/report.json"])
    require(report["failure"] == "native depth guard was not exercised",
            "first native depth diagnostic changed")
    return {"evidence_sha256": CAPTURE["failed_attempts"]["cp1-list-copy-first-binder/evidence.json"],
            "valid_capture": True, "accepted": False, "failed_metric": "depth",
            "scope": "Original diagnostic failure preserved, not reclassified as successful validation"}


def check_code_review(members, builds):
    prefix = "generated-code/"
    raw = members[prefix + "receipt.json"]
    require(sha(raw) == CAPTURE["code_review_receipt_sha256"], "code-review receipt changed")
    receipt = strict_json_loads(raw)
    require(type(receipt["version"]) is int and receipt["version"] == 1
            and set(receipt["binaries"]) == {"control", "candidate"}, "code-review roles changed")
    expected = {"receipt.json", *receipt["review_artifacts"]}
    commands = receipt["commands"]
    require(len(commands) == 16 and len({row["label"] for row in commands}) == 16,
            "code-review command inventory changed")
    expected.update(safe_name(row[label]) for row in commands for label in ("stdout", "stderr"))
    require(len(expected) == 37
            and {name.removeprefix(prefix) for name in members if name.startswith(prefix)} == expected,
            "code-review directory inventory changed")
    for role, binary in receipt["binaries"].items():
        artifact = builds[role]["artifacts"]["normal"]
        require(binary["sha256"] == binary["sha256_after_inspection"] == artifact["sha256"]
                and binary["bytes"] == builds[role]["inventory"][artifact["path"]]["bytes"]
                and binary["manifest_sha256"] == CAPTURE["source_manifest_sha256"][role],
                "code review did not inspect the frozen normal binary")
        source = builds[role]["source_fingerprint"]["files"]["crates/ts_binder/src/containers.rs"]
        require(binary["frozen_containers_sha256"] == source
                and sha(members[prefix + role + "-containers.rs"]) == source,
                "code review source differs from frozen candidate/control")
    for row in commands:
        require(type(row["returncode"]) is int and row["returncode"] == 0,
                "code-review command failed")
        for role, binary in receipt["binaries"].items():
            if row["label"].startswith(role + "-"):
                require(row["argv"][-1] == binary["path"], "code-review command used another binary")
        for label in ("stdout", "stderr"):
            name = safe_name(row[label])
            expected.add(name)
            require(sha(members[prefix + name]) == row[label + "_sha256"],
                    "code-review raw command output changed")
    for name, expected_sha in receipt["review_artifacts"].items():
        require(sha(members[prefix + safe_name(name)]) == expected_sha, "code-review artifact changed")
    return {"receipt_sha256": CAPTURE["code_review_receipt_sha256"], "successful_commands": 16,
            "members_verified": len(expected), "binary_mode": "normal", "architecture": "arm64",
            "elapsed_time_attribution_established": False,
            "scope": "Preserved static-code review; caller-frame subtotals exclude descendants and slow paths"}


def read_archive(directory):
    manifest = strict_json_loads((directory / "manifest.json").read_bytes())
    require(type(manifest["version"]) is int and manifest["version"] == 1
            and manifest["kind"] == KIND and manifest["policy"] == POLICY
            and manifest["capture"] == CAPTURE, "archive identity/policy changed")
    path = directory / safe_name(manifest["archive"]["path"])
    require(identity(path.read_bytes()) == {key: manifest["archive"][key] for key in ("bytes", "sha256")},
            "archive bytes changed")
    members = {}
    with tarfile.open(path, "r:xz") as stream:
        for entry in stream:
            name = safe_name(entry.name)
            require(entry.isfile() and name not in members and name in manifest["members"],
                    "invalid/duplicate/extra archive member")
            raw = stream.extractfile(entry).read()
            require(identity(raw) == manifest["members"][name], "archive member changed: " + name)
            members[name] = raw
    require(set(members) == set(manifest["members"]) and len(members) == manifest["member_count"]
            and sum(map(len, members.values())) == manifest["uncompressed_bytes"],
            "archive member inventory incomplete")
    return manifest, members


def verify_members(manifest, members):
    configure()
    preparation = strict_json_loads(members["prepared.json"])
    require(preparation["version"] == 1 and preparation["kind"] == KIND
            and preparation["policy"] == POLICY and preparation["capture"] == CAPTURE,
            "preparation identity changed")
    require(preparation["helpers"] == helper_files(), "replay implementation/helper closure changed")
    for name, expected in preparation["helpers"].items():
        require(sha(members["tools/" + safe_name(name)]) == expected, "helper snapshot changed")
    builds, frozen = base.check_builds(members)
    graph, graphs = base.check_graphs(members, builds, frozen, CAPTURE["graph_report_sha256"])
    screen, proof = base.check_screen(members, builds, CAPTURE["graph_report_sha256"],
                                      CAPTURE["screen_report_sha256"], manifest["checkpoint_decision"])
    proof.pop("production_decision")
    proof.update(check_policy(proof["summary"], manifest["checkpoint_decision"]))
    require(manifest["screening_status"] == proof["screening_status"], "archive decision summary changed")
    e3 = check_e3(members)
    binder = check_binder(members, builds)
    depth = check_depth(members, builds)
    check_proofs(members)
    first_binder = check_first_binder(members)
    code_review = check_code_review(members, builds)
    for record in (graph, screen):
        require(record["tool_fingerprint"] == preparation["capture_tools"], "capture helper identity changed")
        for path, expected in record["tool_fingerprint"].items():
            relative = Path(path).relative_to(ROOT).as_posix()
            require(sha(members["tools/" + relative]) == expected, "capture helper snapshot changed")
    return {"version": 1, "diagnostic_only": True, "members_verified": len(members),
            "source_manifest_sha256": CAPTURE["source_manifest_sha256"], "graph_replay": graphs,
            "screen_replay": proof, "e3_replay": e3, "binder_replay": binder,
            "depth_replay": depth,
            "first_binder_attempt": first_binder,
            "generated_code_review": code_review,
            "failed_attempts_retained": sorted(CAPTURE["failed_attempts"]),
            "native_child_reexecuted": False, "final_go_relative_gates_established": False,
            "final_production_promotion_established": False}


def replay(directory):
    validate_pins()
    manifest, members = read_archive(directory)
    return {**verify_members(manifest, members), "archive_sha256": manifest["archive"]["sha256"]}


def package(args):
    validate_pins()
    configure()
    directory = args.directory.resolve()
    require(not (directory / "manifest.json").exists() and not (directory / "recorded-results.tar.xz").exists(),
            "package already exists; do not overwrite recorded results")
    preparation_raw = (directory / "prepared.json").read_bytes()
    preparation = strict_json_loads(preparation_raw)
    require(preparation["capture"] == CAPTURE and preparation["helpers"] == helper_files(),
            "preparation or replay implementation changed")
    for name, path in (("graph", args.graphs), ("screen", args.screen)):
        require(runner.digest(path / "report.json") == CAPTURE[name + "_report_sha256"],
                "externally recorded capture identity changed")
    paths = {"control": args.control, "candidate": args.candidate}
    builds = {role: runner.validate_bundle(path, CAPTURE["source_manifest_sha256"][role])
              for role, path in paths.items()}
    runner.verify_report(args.screen)
    members = {"prepared.json": preparation_raw}
    for role, path in paths.items():
        omitted = {record["path"] for record in builds[role]["artifacts"].values()}
        for name in ("manifest.json", *builds[role]["inventory"]):
            if name not in omitted:
                members[role + "/" + safe_name(name)] = (path / name).read_bytes()
    for prefix, path in (("graphs/", args.graphs), ("screen/", args.screen),
                         ("generated-code/", args.code_review)):
        for entry in sorted(path.rglob("*")):
            require(not entry.is_symlink(), "capture contains symlink")
            if entry.is_file():
                members[prefix + safe_name(entry.relative_to(path).as_posix())] = entry.read_bytes()
    for name, expected in preparation["helpers"].items():
        raw = (directory / "prepared-tools" / name).read_bytes()
        require(sha(raw) == expected, "prepared helper bytes changed")
        members["tools/" + safe_name(name)] = raw
    for prefix, records in (("proofs/", CAPTURE["proof_logs"]),
                            ("failed-attempts/", CAPTURE["failed_attempts"])):
        for name in records:
            members[prefix + safe_name(name)] = (args.logs / name).read_bytes()
    for label in ("e3", "binder"):
        raw = (ROOT / "status/evidence" / (CAPTURE[label + "_evidence_sha256"] + ".json")).read_bytes()
        members[label + "/evidence.json"] = raw
        if label == "e3":
            evidence = strict_json_loads(raw)
            for stream in ("stdout", "stderr"):
                members["e3/" + stream] = evidence[stream].encode()
    members["e3/ownership-cases.json"] = args.ownership_cases.read_bytes()
    for name in ("report.json", "requests.ndjson", "failures.ndjson", "oracle.stderr", "rust.stderr"):
        members["binder/" + name] = (args.binder / name).read_bytes()
    for name, expected in strict_json_loads(members["binder/report.json"])["production_inputs"].items():
        if name not in builds["candidate"]["source_fingerprint"]["files"]:
            raw = (ROOT / name).read_bytes()
            require(sha(raw) == expected, "additional binder source changed before packaging")
            members["binder/source/" + safe_name(name)] = raw
    for name in CAPTURE["depth_raw_sha256"]:
        members["depth/" + name] = (args.binder / "depth" / name).read_bytes()
    members["depth/cases.json"] = (ROOT / "data/s07/binder-depth-cases.json").read_bytes()
    for name, expected in strict_json_loads(members["depth/report.json"])["inputs"].items():
        if name not in builds["candidate"]["source_fingerprint"]["files"]:
            raw = (ROOT / name).read_bytes()
            require(sha(raw) == expected, "additional depth source changed before packaging")
            members["depth/source/" + safe_name(name)] = raw
    summary = strict_json_loads(members["screen/report.json"])
    manifest = {"version": 1, "kind": KIND, "policy": POLICY, "capture": CAPTURE,
                "checkpoint_decision": args.decision, "screening_status": summary["screening_status"]}
    verify_members(manifest, members)
    archive = directory / "recorded-results.tar.xz"
    with tarfile.open(archive, "w:xz", format=tarfile.PAX_FORMAT) as stream:
        for name, raw in sorted(members.items()):
            entry = tarfile.TarInfo(name)
            entry.size, entry.mode = len(raw), 0o444
            stream.addfile(entry, io.BytesIO(raw))
    manifest.update(archive={"path": archive.name, **identity(archive.read_bytes())},
                    members={name: identity(raw) for name, raw in sorted(members.items())},
                    member_count=len(members), uncompressed_bytes=sum(map(len, members.values())),
                    omitted="Executable bytes and original workload files; no native code is executed by replay.")
    runner.write_json(directory / "manifest.json", manifest)
    result = replay(directory)
    runner.write_json(directory / "replay.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "package", "replay"))
    parser.add_argument("--directory", type=Path, default=HERE)
    parser.add_argument("--control", type=Path, default=ROOT / "target/s07-bis/cp1-node-read-candidate")
    parser.add_argument("--candidate", type=Path, default=ROOT / "target/s07-bis/cp1-list-copy-candidate")
    parser.add_argument("--graphs", type=Path, default=ROOT / "target/s07-bis/cp1-list-copy-graphs")
    parser.add_argument("--screen", type=Path, default=ROOT / "target/s07-bis/cp1-list-copy-screen")
    parser.add_argument("--binder", type=Path, default=ROOT / "target/s07-binder-reports")
    parser.add_argument("--logs", type=Path, default=ROOT / "target/s07-bis")
    parser.add_argument("--ownership-cases", type=Path,
                        default=ROOT / "target/s07-bis/list-copy-archive-inputs/ownership-cases.json")
    parser.add_argument("--code-review", type=Path, default=ROOT / "target/s07-bis/cp1-list-copy-code-review")
    parser.add_argument("--decision", choices=("keep", "reject"))
    args = parser.parse_args()
    result = (prepare(args.directory) if args.command == "prepare" else package(args)
              if args.command == "package" else replay(args.directory))
    print(json.dumps(result, indent=2, sort_keys=True, allow_nan=False))


if __name__ == "__main__":
    main()
