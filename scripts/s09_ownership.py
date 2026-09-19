"""Exact production generation, registry, retention and request scratch tests.

The S09-4 and S09-1 criteria do not complete E3, certify checker algorithms or
extend the arena example's allocation counters to checker storage. S09-3 is
printing and insertion formatting together; the printing observation stays
published as its component.
"""

from pathlib import Path
import re
import sys

from s04_common import strict_json_loads
from s06_ownership import validate_output
import s09_format
import s09_printing


SUITES = {
    "generation": ("tsr_arena", "lease::"),
    "pool": ("tsr_project", "tests::"),
    "registry": ("tsr_api", "tests::"),
    "scratch": ("tsr_api", "printing::scratch_checks::"),
    "insertion": ("tsr_api", "formatting::scratch_checks::"),
    "results": ("tsr_project", "retention::results::"),
    "ast": ("tsr_project", "retention::ast::"),
    "builder": ("tsr_checker", "node_builder::cache::retention::"),
}
OWNERSHIP_SUITES = ("generation", "pool", "registry")
# S09-1 and S09-2: each criterion is one suite. The first two run over a
# program-backed pool and the third over the checker's cached node builder. They
# are scored apart from S09-4, so a regression names the contract it broke.
RETENTION = {"checker_result_retention": "results", "checker_ast_retention": "ast",
             "builder_cache_retention": "builder"}
MODES = {"debug", "release", "miri", "address_sanitizer"}
CRITERIA = ("shared_pool_panic_retirement", "release_boundaries")


def load_cases(root):
    return validate_manifest(strict_json_loads(
        (Path(root) / "data/s09/ownership-cases.json").read_bytes()))


def validate_manifest(manifest):
    if (type(manifest) is not dict or set(manifest) != {"version", "suites"}
            or type(manifest["version"]) is not int or manifest["version"] != 4
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
        # The two request suites compare against frozen native rows, which are
        # checked against their inputs before the tests that read them run.
        verify = {"scratch": s09_printing.verify_frozen, "insertion": s09_format.verify_insertion_frozen}.get(name)
        if verify is not None:
            try:
                verify(root=root)
            except (ValueError, OSError) as error:
                print(f"S09 {name} fixture {mode} failed verification: {error}", file=sys.stderr)
                outcomes[name] = False
                continue
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
        for name in OWNERSHIP_SUITES:
            metrics[f"checker_ownership_{name}_{mode}"] = outcomes[name]
        ownership_passed = all(outcomes[name] for name in OWNERSHIP_SUITES)
        metrics[f"shared_pool_panic_retirement_{mode}"] = ownership_passed
        metrics[f"release_boundaries_{mode}"] = ownership_passed and arena_modes[mode]
        metrics[f"api_print_scratch_disposal_{mode}"] = outcomes["scratch"]
        metrics[f"api_scratch_disposal_{mode}"] = outcomes["scratch"] and outcomes["insertion"]
        for criterion, name in RETENTION.items():
            metrics[f"{criterion}_{mode}"] = outcomes[name]
    for criterion in (*CRITERIA, *RETENTION):
        metrics[criterion] = all(metrics[f"{criterion}_{mode}"] for mode in MODES)
    # Count a suite only after every inventoried test has a validated passing
    # observation in every mode. This is not a heap or live-owner measurement.
    metrics["checker_ownership_tests"] = sum(
        len(suite["cases"]) for name, suite in manifest["suites"].items()
        if name in OWNERSHIP_SUITES and all(outcomes[name] for outcomes in modes.values()))
    metrics["checker_retention_tests"] = sum(
        len(manifest["suites"][name]["cases"]) for name in RETENTION.values()
        if all(outcomes[name] for outcomes in modes.values()))
    # S09-3: printing and insertion formatting, each in every mode.
    metrics["api_scratch_disposal"] = all(metrics[f"api_scratch_disposal_{mode}"] for mode in MODES)
    metrics["api_scratch_tests"] = (
        sum(len(manifest["suites"][name]["cases"]) for name in ("scratch", "insertion"))
        if metrics["api_scratch_disposal"] else 0)
    metrics["api_print_scratch_disposal"] = all(outcomes["scratch"] for outcomes in modes.values())
    metrics["api_print_scratch_tests"] = (
        len(manifest["suites"]["scratch"]["cases"]) if metrics["api_print_scratch_disposal"] else 0)
