import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("cp1_replay_tests", Path(__file__).with_name("replay.py"))
replay = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(replay)


def summary(ratio=0.94, upper=0.99):
    modes = {}
    for workers in ("1", "8"):
        modes[workers] = {}
        for metric in replay.runner.METRICS:
            modes[workers][metric] = {"samples_per_variant": 7, "control_relative_mad": 0.01,
                "candidate_relative_mad": 0.01, "ratio": ratio if metric == "wall_time_ns" else 1.0,
                "bootstrap": {"upper": upper} if metric == "wall_time_ns" else None}
    return {"modes": modes, "screening_status": "eligible_for_review",
            "nonregression_conditions_met": True, "targeted_pipeline_win": True}


class ArchiveTests(unittest.TestCase):
    def test_helper_closure_includes_subprocess_only_capture_scripts(self):
        helpers = replay.helper_files()
        for path, expected in replay.runner.tool_fingerprint().items():
            self.assertEqual(helpers[Path(path).relative_to(replay.ROOT).as_posix()], expected)
        self.assertIn("scripts/s07_benchmark_child.py", helpers)

    def test_binder_source_inventory_and_request_identity_are_pinned(self):
        members = {"binder/report.json": b'{"production_inputs":{"example.rs":"abc"}}',
                   "binder/requests.ndjson": b'{"id":"request-1","primary":"row-1","equal":true}\n'}
        expected = {name.removeprefix("binder/"): replay.sha(raw) for name, raw in members.items()}
        with mock.patch.object(replay, "BINDER_RAW_SHAS", expected):
            replay.check_binder_identity(members)
            for name, changed in (("binder/report.json", b'{"production_inputs":{}}'),
                                  ("binder/requests.ndjson", members["binder/requests.ndjson"].replace(b'request-1', b'request-2'))):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    replay.check_binder_identity({**members, name: changed})

    def test_graph_inventory_requires_all_original_streams_even_empty_stderr(self):
        members = {"graphs/report.json": b""} | {
            f"graphs/workers-{workers}/{name}": b"" for workers in (1, 8)
            for name in ("oracle.ndjson", "rust.ndjson", "oracle.stderr", "rust.stderr",
                         "failures.ndjson", "binding-paths.stdout", "binding-paths.stderr")}
        replay.check_graph_inventory(members)
        for name in members:
            with self.subTest(name=name), self.assertRaises(ValueError):
                replay.check_graph_inventory({key: value for key, value in members.items() if key != name})

    def e3_fixture(self):
        inventory = {"version": 2, "common": {}, "groups": {}}
        for (name, (package, prefix)), count in zip(replay.COMMON.items(), (14, 10, 3), strict=True):
            inventory["common"][name] = {"package": package, "filter": prefix, "exact": False,
                                        "cases": [prefix + f"case_{index:02d}" for index in range(count)]}
        for name in replay.GROUPS:
            case = "ownership_tests::" + name
            inventory["groups"][name] = {"package": "ts_compiler", "filter": case, "exact": True, "cases": [case]}
        text = ""
        suites = {**inventory["common"], **inventory["groups"]}
        for mode in sorted(replay.MODES):
            option = {"debug": "", "release": "--release", "miri": "miri", "address_sanitizer": "-Zbuild-std"}[mode]
            for suite in suites.values():
                cases = suite["cases"]
                text += f"+ cargo {option} test --package {suite['package']} {suite['filter']}\n"
                text += f"running {len(cases)} tests\n"
                text += "".join(f"test {case} ... ok\n" for case in cases)
                text += f"test result: ok. {len(cases)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n"
        measured = {"metrics": {}}
        replay.publish_metrics(measured, {mode: {name: True for name in suites} for mode in replay.MODES}, inventory)
        return inventory, json.dumps(measured), text

    def validate_e3_fixture(self, inventory, stdout, stderr):
        evidence = {"exit_code": 0, "valid_capture": True, "stdout": stdout, "stderr": stderr,
                    "stdout_sha256": replay.sha(stdout.encode()), "stderr_sha256": replay.sha(stderr.encode())}
        raw, cases = json.dumps(evidence).encode(), json.dumps(inventory).encode()
        members = {"e3/evidence.json": raw, "e3/stdout": stdout.encode(), "e3/stderr": stderr.encode(),
                   "e3/ownership-cases.json": cases}
        with mock.patch.object(replay, "E3_SHA", replay.sha(raw)), mock.patch.object(replay, "E3_INVENTORY_SHA", replay.sha(cases)):
            return replay.check_e3(members)

    def test_e3_requires_all_116_named_observations(self):
        inventory, stdout, stderr = self.e3_fixture()
        self.assertEqual(self.validate_e3_fixture(inventory, stdout, stderr)["successful_named_test_observations"], 116)
        for change in (lambda text: text.replace(" ... ok", " ... FAILED", 1),
                       lambda text: text.replace("test bind_tests::case_00 ... ok\n", "", 1),
                       lambda text: text[:text.rindex("+ cargo ")]):
            with self.assertRaises(ValueError):
                self.validate_e3_fixture(inventory, stdout, change(stderr))

    def test_archive_inventory_rejects_missing_changed_extra_and_duplicate_members(self):
        for entries in ([('proof/a', b'a')], [], [('proof/a', b'b')],
                        [('proof/a', b'a'), ('proof/b', b'b')], [('proof/a', b'a'), ('proof/a', b'a')]):
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                archive = directory / 'data.tar.xz'
                with tarfile.open(archive, 'w:xz') as stream:
                    for name, raw in entries:
                        entry = tarfile.TarInfo(name); entry.size = len(raw)
                        stream.addfile(entry, io.BytesIO(raw))
                manifest = {"version": 1, "kind": "s07_bis_cp1_core_read_archive", "policy": replay.POLICY,
                            "source_manifest_sha256": replay.MANIFEST_SHAS,
                            "archive": {"path": archive.name, **replay.identity(archive.read_bytes())},
                            "members": {"proof/a": replay.identity(b'a')}, "member_count": 1, "uncompressed_bytes": 1}
                (directory / 'manifest.json').write_text(json.dumps(manifest))
                if entries == [('proof/a', b'a')]:
                    self.assertEqual(replay.read_archive(directory)[1], {'proof/a': b'a'})
                else:
                    with self.assertRaises(ValueError):
                        replay.read_archive(directory)

    def test_explicit_rejection_is_allowed_even_after_screen_eligibility(self):
        self.assertEqual(replay.check_policy(summary(), "keep")["production_decision"], "keep")
        self.assertFalse(replay.check_policy(summary(), "reject")["screen_requires_rejection"])

    def test_neutral_screen_cannot_use_infrastructure_exception(self):
        value = summary(ratio=0.99, upper=1.01)
        value.update(screening_status="no_demonstrated_win", targeted_pipeline_win=False)
        with self.assertRaisesRegex(ValueError, "infrastructure exception"):
            replay.check_policy(value, "keep")
        self.assertTrue(replay.check_policy(value, "reject")["screen_requires_rejection"])

    def test_each_worker_and_metric_requires_nonregression_and_quiet_samples(self):
        for workers in ("1", "8"):
            for metric in replay.runner.METRICS:
                for noise in (False, True):
                    value = summary()
                    row = value["modes"][workers][metric]
                    if noise:
                        row["candidate_relative_mad"] = 0.051
                    elif metric == "wall_time_ns":
                        row["bootstrap"]["upper"] = 1.021
                    else:
                        row["ratio"] = 1.021
                    value.update(screening_status="inconclusive" if noise else "regressing_or_uncertain",
                                 nonregression_conditions_met=False)
                    with self.subTest(workers=workers, metric=metric, noise=noise):
                        self.assertTrue(replay.check_policy(value, "reject")["screen_requires_rejection"])
                        with self.assertRaises(ValueError):
                            replay.check_policy(value, "keep")

    def test_missing_mode_metric_or_sample_count_fails(self):
        for mutation in (lambda value: value["modes"].pop("8"),
                         lambda value: value["modes"]["1"].pop("allocated_bytes"),
                         lambda value: value["modes"]["1"]["wall_time_ns"].update(samples_per_variant=6)):
            value = summary()
            mutation(value)
            with self.assertRaises(ValueError):
                replay.check_policy(value, "reject")

    def test_archive_paths_reject_escape_and_ambiguous_spelling(self):
        for name in ("../x", "/x", "a/../x", "a//x", "./x", ""):
            with self.subTest(name=name), self.assertRaises(ValueError):
                replay.safe_name(name)
        self.assertEqual(replay.safe_name("graphs/workers-1/rust.ndjson"), "graphs/workers-1/rust.ndjson")


if __name__ == "__main__":
    unittest.main()
