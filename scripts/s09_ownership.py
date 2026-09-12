"""Exact production generation, checker-pool and API registry ownership tests.

These two S09-4 criteria do not complete E3, certify checker algorithms or
extend the arena example's allocation counters to checker storage.
"""

from pathlib import Path
import re
import sys

from s04_common import strict_json_loads
from s06_ownership import validate_output


SUITES = {
    "generation": ("ts_arena", "lease::"),
    "pool": ("ts_project", "tests::"),
    "registry": ("ts_api", "tests::"),
}
MODES = {"debug", "release", "miri", "address_sanitizer"}
CRITERIA = ("shared_pool_panic_retirement", "release_boundaries")


def load_cases(root):
    return validate_manifest(strict_json_loads(
        (Path(root) / "data/s09/ownership-cases.json").read_bytes()))


def validate_manifest(manifest):
    if (type(manifest) is not dict or set(manifest) != {"version", "suites"}
            or type(manifest["version"]) is not int or manifest["version"] != 1
            or type(manifest["suites"]) is not dict
            or set(manifest["suites"]) != set(SUITES)):
        raise ValueError("invalid S09 ownership inventory")
    for name, (package, prefix) in SUITES.items():
        suite = manifest["suites"][name]
        if (type(suite) is not dict or set(suite) != {"package", "filter", "cases"}
                or (suite["package"], suite["filter"]) != (package, prefix)):
            raise ValueError("S09 ownership suite changed scope: " + name)
        cases = suite["cases"]
        if (type(cases) is not list or not cases
                or any(type(case) is not str or not case.startswith(prefix)
                       or re.fullmatch(r"(?:[a-z0-9_]+::)+[a-z0-9_]+", case) is None
                       for case in cases)
                or cases != sorted(set(cases))):
            raise ValueError("S09 ownership needs sorted unique exact test names: " + name)
    return manifest


def measure(root, invoke, prefix, options, env, manifest, mode):
    validate_manifest(manifest)
    if mode not in MODES:
        raise ValueError("invalid S09 ownership measurement mode")
    outcomes = {}
    for name, suite in manifest["suites"].items():
        args = [*prefix, "test", "--package", suite["package"], "--lib", "--locked",
                *options, suite["filter"], "--", "--test-threads=1", "--nocapture"]
        try:
            validate_output(invoke(root, args, env), suite["cases"], mode, f"S09 ownership {name}")
        except (RuntimeError, ValueError) as error:
            print(f"S09 ownership {name} {mode} failed: {error}", file=sys.stderr)
            outcomes[name] = False
        else:
            outcomes[name] = True
    return outcomes


def publish_metrics(report, modes, arena_modes, manifest):
    validate_manifest(manifest)
    if (type(modes) is not dict or set(modes) != MODES
            or any(type(outcomes) is not dict or set(outcomes) != set(SUITES)
                   or any(type(value) is not bool for value in outcomes.values())
                   for outcomes in modes.values())):
        raise ValueError("S09 ownership must execute every suite in all four modes")
    if (type(arena_modes) is not dict or set(arena_modes) != MODES
            or any(type(value) is not bool for value in arena_modes.values())):
        raise ValueError("S09 release boundaries require the arena suite in all four modes")
    metrics = report["metrics"]
    for mode, outcomes in modes.items():
        for name, passed in outcomes.items():
            metrics[f"checker_ownership_{name}_{mode}"] = passed
        metrics[f"shared_pool_panic_retirement_{mode}"] = all(outcomes.values())
        metrics[f"release_boundaries_{mode}"] = all(outcomes.values()) and arena_modes[mode]
    for criterion in CRITERIA:
        metrics[criterion] = all(metrics[f"{criterion}_{mode}"] for mode in MODES)
    # Count a suite only after every inventoried test has a validated passing
    # observation in every mode. This is not a heap or live-owner measurement.
    metrics["checker_ownership_tests"] = sum(
        len(suite["cases"]) for name, suite in manifest["suites"].items()
        if all(outcomes[name] for outcomes in modes.values()))
