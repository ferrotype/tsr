import copy
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("list_copy_archive_tests", Path(__file__).with_name("replay.py"))
replay = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(replay)


def summary(ratio=0.94, upper=0.99):
    modes = {}
    for workers in ("1", "8"):
        modes[workers] = {}
        for metric in replay.runner.METRICS:
            modes[workers][metric] = {
                "samples_per_variant": 7, "control_relative_mad": 0.01,
                "candidate_relative_mad": 0.01, "ratio": ratio if metric == "wall_time_ns" else 1.0,
                "bootstrap": {"upper": upper} if metric == "wall_time_ns" else None,
            }
    return {"modes": modes, "screening_status": "eligible_for_review",
            "nonregression_conditions_met": True, "targeted_pipeline_win": True}


def e3_fixture():
    inventory = {"version": 2, "common": {}, "groups": {}}
    for (name, (package, prefix)), count in zip(replay.base.COMMON.items(), (14, 13, 3), strict=True):
        inventory["common"][name] = {"package": package, "filter": prefix, "exact": False,
                                    "cases": [prefix + f"case_{index:02d}" for index in range(count)]}
    for name in replay.base.GROUPS:
        case = "ownership_tests::" + name
        inventory["groups"][name] = {"package": "ts_compiler", "filter": case,
                                    "exact": True, "cases": [case]}
    text = ""
    suites = {**inventory["common"], **inventory["groups"]}
    for mode in sorted(replay.base.MODES):
        option = {"debug": "", "release": "--release", "miri": "miri",
                  "address_sanitizer": "-Zbuild-std"}[mode]
        for suite in suites.values():
            cases = suite["cases"]
            text += f"+ cargo {option} test --package {suite['package']} {suite['filter']}\n"
            text += f"running {len(cases)} tests\n"
            text += "".join(f"test {case} ... ok\n" for case in cases)
            text += f"test result: ok. {len(cases)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n"
    measured = {"metrics": {}}
    replay.base.publish_metrics(measured, {mode: {name: True for name in suites}
                                         for mode in replay.base.MODES}, inventory)
    return inventory, json.dumps(measured), text


def fixture_members(inventory, stdout, stderr):
    evidence = {"exit_code": 0, "valid_capture": True, "stdout": stdout, "stderr": stderr,
                "stdout_sha256": replay.sha(stdout.encode()), "stderr_sha256": replay.sha(stderr.encode())}
    raw, cases = json.dumps(evidence).encode(), json.dumps(inventory).encode()
    return {"e3/evidence.json": raw, "e3/stdout": stdout.encode(), "e3/stderr": stderr.encode(),
            "e3/ownership-cases.json": cases}


class ListCopyArchiveTests(unittest.TestCase):
    def setUp(self):
        replay.configure()

    def validate_fixture(self, members):
        pins = {**replay.CAPTURE,
                "e3_evidence_sha256": replay.sha(members["e3/evidence.json"]),
                "e3_inventory_sha256": replay.sha(members["e3/ownership-cases.json"])}
        with mock.patch.object(replay, "CAPTURE", pins):
            return replay.check_e3(members)

    def test_frozen_dependency_and_complete_subprocess_helper_closure(self):
        helpers = replay.helper_files()
        self.assertEqual(helpers[replay.BASE_PATH.relative_to(replay.ROOT).as_posix()], replay.BASE_SHA)
        self.assertIn("scripts/s07_benchmark_child.py", helpers)
        self.assertIn(Path(replay.__file__).relative_to(replay.ROOT).as_posix(), helpers)
        for path, expected in replay.runner.tool_fingerprint().items():
            self.assertEqual(helpers[Path(path).relative_to(replay.ROOT).as_posix()], expected)

    def test_missing_capture_pins_fail_before_any_packaging(self):
        with mock.patch.object(replay, "CAPTURE", {**replay.CAPTURE, "graph_report_sha256": None}):
            with self.assertRaisesRegex(ValueError, "not finalized"):
                replay.validate_pins()

    def test_all_128_named_test_observations_are_required(self):
        inventory, stdout, text = e3_fixture()
        self.assertEqual(self.validate_fixture(fixture_members(inventory, stdout, text))[
            "successful_named_test_observations"], 128)
        for changed in (text.replace(" ... ok", " ... FAILED", 1),
                        text.replace("test exclusive_tests::case_12 ... ok\n", "", 1),
                        text[:text.rindex("+ cargo ")],
                        text + text[text.rindex("+ cargo "):]):
            with self.assertRaises(ValueError):
                self.validate_fixture(fixture_members(inventory, stdout, changed))

    def test_e3_rejects_old_29_case_inventory_and_raw_stream_drift(self):
        inventory, stdout, text = e3_fixture()
        old = copy.deepcopy(inventory)
        old["common"]["exclusive_binding"]["cases"] = old["common"]["exclusive_binding"]["cases"][:-3]
        with self.assertRaisesRegex(ValueError, "omitted list-copy cases"):
            self.validate_fixture(fixture_members(old, stdout, text))
        members = fixture_members(inventory, stdout, text)
        members["e3/stderr"] += b"changed"
        with self.assertRaisesRegex(ValueError, "raw output changed"):
            self.validate_fixture(members)

    def test_keep_and_reject_remain_checkpoint_decisions(self):
        for decision in ("keep", "reject"):
            result = replay.check_policy(summary(), decision)
            self.assertEqual(result["checkpoint_decision"], decision)
            self.assertNotIn("production_decision", result)
            self.assertFalse(result["final_production_promotion_established"])
        neutral = summary(0.99, 1.01)
        neutral.update(screening_status="no_demonstrated_win", targeted_pipeline_win=False)
        self.assertTrue(replay.check_policy(neutral, "reject")["screen_requires_rejection"])
        with self.assertRaises(ValueError):
            replay.check_policy(neutral, "keep")

    def test_every_worker_metric_and_sample_count_keeps_original_guard(self):
        for workers in ("1", "8"):
            for metric in replay.runner.METRICS:
                for noisy in (False, True):
                    value = summary()
                    row = value["modes"][workers][metric]
                    if noisy:
                        row["candidate_relative_mad"] = 0.051
                    elif metric == "wall_time_ns":
                        row["bootstrap"]["upper"] = 1.021
                    else:
                        row["ratio"] = 1.021
                    value.update(screening_status="inconclusive" if noisy else "regressing_or_uncertain",
                                 nonregression_conditions_met=False)
                    self.assertTrue(replay.check_policy(value, "reject")["screen_requires_rejection"])
                    with self.assertRaises(ValueError):
                        replay.check_policy(value, "keep")
        for mutation in (lambda value: value["modes"].pop("8"),
                         lambda value: value["modes"]["1"].pop("allocated_bytes"),
                         lambda value: value["modes"]["1"]["wall_time_ns"].update(samples_per_variant=6)):
            value = summary()
            mutation(value)
            with self.assertRaises(ValueError):
                replay.check_policy(value, "reject")

    def test_all_15_graph_streams_are_required(self):
        members = {"graphs/report.json": b""} | {
            f"graphs/workers-{workers}/{name}": b"" for workers in (1, 8)
            for name in ("oracle.ndjson", "rust.ndjson", "oracle.stderr", "rust.stderr",
                         "failures.ndjson", "binding-paths.stdout", "binding-paths.stderr")}
        self.assertEqual(len(members), 15)
        replay.base.check_graph_inventory(members)
        for name in members:
            with self.assertRaises(ValueError):
                replay.base.check_graph_inventory({key: raw for key, raw in members.items() if key != name})

    def test_proof_and_failed_attempt_logs_cannot_be_missing_changed_or_extra(self):
        pins = {**replay.CAPTURE, "proof_logs": {"build.log": replay.sha(b"ok")},
                "failed_attempts": {"refused.log": replay.sha(b"active compiler")}}
        members = {"proofs/build.log": b"ok", "failed-attempts/refused.log": b"active compiler"}
        with mock.patch.object(replay, "CAPTURE", pins):
            replay.check_proofs(members)
            for changed in ({"proofs/build.log": b"ok"},
                            {**members, "failed-attempts/refused.log": b"ok"},
                            {**members, "proofs/other.log": b"ok"}):
                with self.assertRaises(ValueError):
                    replay.check_proofs(changed)

    def test_binder_identity_fails_before_modified_inventory_is_interpreted(self):
        pins = {**replay.CAPTURE, "binder_raw_sha256": {
            "report.json": replay.sha(b'{"production_inputs":{"a.rs":"hash"}}'),
            "requests.ndjson": replay.sha(b'{"id":"frozen"}\n')}}
        with mock.patch.object(replay, "CAPTURE", pins):
            with self.assertRaisesRegex(ValueError, "raw capture changed"):
                replay.check_binder({"binder/report.json": b'{"production_inputs":{}}'}, {})

    def test_successful_graph_rows_do_not_override_failed_or_missing_binder_metrics(self):
        replay.check_binder_metrics(dict(replay.BINDER_METRICS))
        for name, expected in replay.BINDER_METRICS.items():
            for mutation in (lambda metrics: metrics.pop(name),
                             lambda metrics: metrics.update({name: False if expected is True else -1})):
                metrics = dict(replay.BINDER_METRICS)
                mutation(metrics)
                with self.subTest(metric=name), self.assertRaises(ValueError):
                    replay.check_binder_metrics(metrics)
        for name in ("depth", "protocol", "helpers", "resolvers", "graph_contracts", "reached_bind"):
            with self.subTest(metric=name), self.assertRaises(ValueError):
                replay.check_binder_metrics({**replay.BINDER_METRICS, name: 1})

    def test_original_depth_failure_stays_unaccepted(self):
        prefix = "failed-attempts/cp1-list-copy-first-binder/"
        for depth in (False, True):
            stdout = json.dumps({"metrics": {**replay.BINDER_METRICS, "depth": depth}})
            evidence = {"exit_code": 0, "valid_capture": True, "stdout": stdout, "stderr": "",
                        "stdout_sha256": replay.sha(stdout.encode()), "stderr_sha256": replay.sha(b"")}
            members = {prefix + "evidence.json": json.dumps(evidence).encode(),
                       prefix + "depth/report.json": b'{"failure":"native depth guard was not exercised"}'}
            if depth:
                with self.assertRaisesRegex(ValueError, "failure was erased"):
                    replay.check_first_binder(members)
            else:
                result = replay.check_first_binder(members)
                self.assertTrue(result["valid_capture"])
                self.assertFalse(result["accepted"])
                self.assertEqual(result["failed_metric"], "depth")

    def test_archive_inventory_rejects_missing_changed_extra_and_duplicate_members(self):
        # Exercise the reader without compression or any captured native work.
        original_open = tarfile.open
        for entries in ([('proof/a', b'a')], [], [('proof/a', b'b')],
                        [('proof/a', b'a'), ('proof/b', b'b')],
                        [('proof/a', b'a'), ('proof/a', b'a')]):
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                archive = directory / 'test.tar'
                with original_open(archive, 'w') as stream:
                    for name, raw in entries:
                        entry = tarfile.TarInfo(name)
                        entry.size = len(raw)
                        stream.addfile(entry, io.BytesIO(raw))
                manifest = {"version": 1, "kind": replay.KIND, "policy": replay.POLICY,
                            "capture": replay.CAPTURE,
                            "archive": {"path": archive.name, **replay.identity(archive.read_bytes())},
                            "members": {"proof/a": replay.identity(b'a')},
                            "member_count": 1, "uncompressed_bytes": 1}
                (directory / 'manifest.json').write_text(json.dumps(manifest))
                with mock.patch.object(replay.tarfile, 'open',
                                       side_effect=lambda path, mode: original_open(path, 'r:')):
                    if entries == [('proof/a', b'a')]:
                        self.assertEqual(replay.read_archive(directory)[1], {'proof/a': b'a'})
                    else:
                        with self.assertRaises(ValueError):
                            replay.read_archive(directory)

    def test_static_review_requires_both_normal_binaries_and_every_command_output(self):
        members, builds = {}, {}
        receipt = {"version": 1, "binaries": {}, "commands": [], "review_artifacts": {}}
        for role in ("control", "candidate"):
            source = role.encode()
            binary_sha = replay.sha(source + b"-binary")
            source_sha = replay.sha(source)
            binary = {"path": "/frozen/" + role, "sha256": binary_sha,
                      "sha256_after_inspection": binary_sha, "bytes": 123,
                      "manifest_sha256": replay.CAPTURE["source_manifest_sha256"][role],
                      "frozen_containers_sha256": source_sha}
            receipt["binaries"][role] = binary
            builds[role] = {"artifacts": {"normal": {"path": "normal", "sha256": binary_sha}},
                            "inventory": {"normal": {"bytes": 123}}, "source_fingerprint": {
                                "files": {"crates/ts_binder/src/containers.rs": source_sha}}}
            name = role + "-containers.rs"
            members["generated-code/" + name] = source
            receipt["review_artifacts"][name] = source_sha
            for index in range(8):
                label = f"{role}-{index}"
                row = {"label": label, "argv": ["objdump", binary["path"]], "returncode": 0}
                for stream in ("stdout", "stderr"):
                    name = label + "." + stream
                    row[stream], row[stream + "_sha256"] = name, replay.sha(b"")
                    members["generated-code/" + name] = b""
                receipt["commands"].append(row)
        for name in ("README.md", "findings.json"):
            members["generated-code/" + name] = b"review"
            receipt["review_artifacts"][name] = replay.sha(b"review")

        def check(value, current_members=members):
            raw = json.dumps(value).encode()
            pins = {**replay.CAPTURE, "code_review_receipt_sha256": replay.sha(raw)}
            with mock.patch.object(replay, "CAPTURE", pins):
                return replay.check_code_review({**current_members, "generated-code/receipt.json": raw}, builds)

        self.assertEqual(check(receipt)["members_verified"], 37)
        for mutation in (lambda value: value["binaries"]["candidate"].update(sha256="other"),
                         lambda value: value["commands"][0].update(returncode=1),
                         lambda value: value["commands"][0].update(argv=["objdump", "/other"])):
            changed = copy.deepcopy(receipt)
            mutation(changed)
            with self.assertRaises(ValueError):
                check(changed)
        omitted = {name: raw for name, raw in members.items() if name != "generated-code/control-0.stderr"}
        with self.assertRaisesRegex(ValueError, "directory inventory"):
            check(receipt, omitted)

    def test_depth_replays_native_counters_and_all_recorded_graph_requests(self):
        ids = [f"case-{index}" for index in range(12)]
        inventory = {"small_stack": [], "constructed": [{"id": name} for name in ids],
                     "graph_requests": [{"id": name} for name in ids]}
        native = [{"id": name, "guard_entries": 501, "actual_segment_growths": 1} for name in ids]
        native_log = b"S07_BINDER_DEPTH:" + json.dumps(native).encode() + b"\n"
        cases_raw = json.dumps(inventory).encode()
        report = {"schema": 1, "scope": "supplemental-binder-depth-only",
                  "metrics": {"binder_depth": True}, "source_changed_during_capture": False,
                  "requests_sha256": replay.sha(cases_raw), "inputs": {},
                  "native": {"rows": native, "log_sha256": replay.sha(native_log)},
                  "graph_rows": [{"id": name, "passed": True, "equal": True,
                      "first_difference": None, "primary": None,
                      "stages": {runtime: [{"outcome": "ok"}] for runtime in ("oracle", "rust")}}
                      for name in ids]}

        def check(value):
            members = {"depth/cases.json": cases_raw, "depth/native.log": native_log,
                       "depth/report.json": json.dumps(value).encode(),
                       "depth/oracle.stderr": b"", "depth/rust.stderr": b""}
            pins = {**replay.CAPTURE, "depth_cases_sha256": replay.sha(cases_raw),
                    "depth_raw_sha256": {name.removeprefix("depth/"): replay.sha(raw)
                        for name, raw in members.items() if name != "depth/cases.json"}}
            with mock.patch.object(replay, "CAPTURE", pins):
                return replay.check_depth(members, {"candidate": {"source_fingerprint": {"files": {}}}})

        self.assertEqual(check(report)["graph_observations"], 12)
        for mutation in (lambda value: value["graph_rows"].pop(),
                         lambda value: value["graph_rows"][1].update(id=ids[0]),
                         lambda value: value["graph_rows"][0].update(passed=False),
                         lambda value: value["native"]["rows"][0].update(guard_entries=1)):
            changed = copy.deepcopy(report)
            mutation(changed)
            with self.assertRaises(ValueError):
                check(changed)


if __name__ == "__main__":
    unittest.main()
