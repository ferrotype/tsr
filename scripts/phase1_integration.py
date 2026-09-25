#!/usr/bin/env python3
"""F5a integration contracts: runnable witnesses, never inferred parity.

`check` is child-free and establishes preparation only. `observe-localized-config`
executes a bounded production-API fixture and reports the known writer boundary.
Complete S11 and package measurements remain their existing producers' work.
"""
import argparse
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = "data/phase1/integration.json"
WITNESSES = {
    "localized-config-diagnostics", "ordered-package-resolution",
    "ordered-config-resolution", "retained-program-snapshot",
    "live-cached-filesystem", "installed-generated-assets", "compiler-syntax-diagnostics",
}
OBLIGATIONS = {
    "filesystem-callback-delegation", "delegate-versus-missing", "casing-and-symlinks",
    "raw-mapper-bytes", "stream-lifecycle", "callback-cancellation-progress",
    "bounded-router", "internal-options-completion", "framing-and-identity",
}
BOUNDARIES = {
    "strict-Unicode-filesystem-wire", "blocked-synchronous-workers-deferred-Phase5",
    "parse-cache-injection-deferred-Phase5", "production-mapper-host-deferred-Phase5",
    "semantic-fourslash-deferred-Phase5", "no-wire-configuration-callback",
    "raw-plugin-stream-not-method-proxy",
}

# These contracts previously named `workspace`, whose producer only runs
# cargo check. Keep a small, executable inventory for the otherwise ungated
# Rust witnesses instead of crediting compilation as test execution.
RUST_WITNESS_TESTS = {
    "witness/ast-generated-concrete-update-clone-contracts": (
        ["cargo", "test", "--locked", "-p", "tsr_ast", "--test", "generated_runtime", "--", "--test-threads=1"],
        ["factory_masks_counts_and_hooks_follow_the_pinned_observation_order",
         "raw_slice_updates_use_backing_identity_and_all_empty_slices_compare_same",
         "enumeration_uses_kind_but_clone_and_transformation_use_payload",
         "owner_validation_includes_references_omitted_from_child_enumeration",
         "concrete_entries_preserve_custom_interception_and_prevalidation_text_counts",
         "concrete_entries_validate_in_field_order_before_node_allocation_and_hooks",
         "concrete_large_payload_keeps_all_fields_forged_kind_and_lazy_compatibility"]),
    "witness/binder-container-flags-source-contract": (
        ["cargo", "test", "--locked", "-p", "tsr_binder", "--lib", "container_classification::tests::", "--", "--test-threads=1"],
        ["container_classification::tests::" + name for name in (
            "fixed_container_rules_do_not_inspect_payload_or_parent", "method_rules_read_only_the_selected_parent_kind",
            "block_rules_include_signature_and_static_block_parents", "property_rules_inspect_initializer_without_requiring_a_parent",
            "local_dynamic_rules_preserve_checked_contract_failures")]),
    "witness/nativepath-raw-eintr-retry": (
        ["cargo", "test", "--locked", "-p", "tsr_vfs", "--lib", "os::native::tests::interrupted_syscalls_retry_but_other_and_wrapped_errors_return_once", "--", "--exact"],
        ["os::native::tests::interrupted_syscalls_retry_but_other_and_wrapped_errors_return_once"]),
    "f5b/native-navigation-rescan": (
        ["cargo", "test", "--locked", "-p", "tsr_astnav", "--lib", "tests::jsx_shift_rescan_matches_the_pinned_private_operation", "--", "--exact"],
        ["tests::jsx_shift_rescan_matches_the_pinned_private_operation"]),
    "witness/s08-p5-errors-rust": (
        ["cargo", "test", "--locked", "-p", "tsr_compiler", "--test", "diagnostic_writer", "native_plain_pretty_and_error_baseline_bytes_match", "--", "--exact"],
        ["native_plain_pretty_and_error_baseline_bytes_match"]),
}


def rust_witness_result(rows):
    if not isinstance(rows, list) or [row.get("id") for row in rows] != list(RUST_WITNESS_TESTS):
        raise ValueError("Rust witness execution inventory differs")
    result = {}
    for row in rows:
        command, expected = RUST_WITNESS_TESTS[row["id"]]
        if row.get("command") != command or type(row.get("exit_code")) is not int or not isinstance(row.get("stdout"), str):
            raise ValueError("Rust witness command/output is malformed")
        outcomes = re.findall(r"^test (\S+) \.\.\. (\w+)$", row["stdout"], re.MULTILINE)
        names = [name for name, _ in outcomes]
        if sorted(names) != sorted(expected) or any(state not in ("ok", "FAILED") for _, state in outcomes):
            raise ValueError(f"Rust witness did not execute its exact tests: {row['id']}")
        passed = all(state == "ok" for _, state in outcomes)
        if row["exit_code"] != (0 if passed else 101):
            raise ValueError("Rust witness exit code disagrees with test outcomes")
        result[row["id"]] = "match" if passed else "different"
    return result


# The P1B mutation receipt (docs/PHASE1-mutation-witnesses.md, section 5.5):
# `phase1_mutation_run.py confirm` splices the full committed manifest into a
# fresh schemata build of the current sources, replays every recorded kill pair
# with its control and the pairs' no-mutant base rows, and re-traces every
# traced oracle (hits only) for the mutants of excused homes. The receipt lists
# every pair, so this validator compares each against the results artifact
# rather than trusting counts.
MUTATION_RECEIPT = "mutation-witnesses"
# The per-pair state of a pair that reproduced its recorded stages, mutant
# digests and control digests exactly. Every other state is a lost kill.
MUTATION_PAIR_REPRODUCED = "reproduced"
MUTATION_PAIR_FIELDS = ("key", "oracle", "row", "request_sha256", "state", "stages", "mutant", "control")


def mutation_recorded_kills(results):
    """Every recorded kill of every killed mutant, in the results artifact's order.

    Each is {id, key, oracle, row, request_sha256, stages, mutant, control}:
    what the receipt must reproduce. A (key, oracle, row) recorded twice makes
    the artifact malformed.
    """
    kills, seen = [], set()
    for mutant in results.get("mutants", []) if isinstance(results, dict) else []:
        if not isinstance(mutant, dict) or mutant.get("state") != "killed":
            continue
        for kill in mutant.get("kills") or []:
            if not isinstance(kill, dict):
                continue
            oracle = kill.get("oracle", mutant.get("oracle", results.get("oracle")))
            identity = (mutant.get("key"), oracle, kill.get("row"))
            if identity in seen:
                raise ValueError(f"mutation results record kill {identity} twice")
            seen.add(identity)
            kills.append({"id": mutant.get("id"), "key": mutant.get("key"), "oracle": oracle, "row": kill.get("row"),
                          "request_sha256": kill.get("request_sha256"), "stages": sorted(kill.get("stages") or []),
                          "mutant": kill.get("mutant"), "control": kill.get("control")})
    return kills


def mutation_confirm_pairs(results):
    """The (mutant id, key, oracle, row) kill pairs a confirmation must replay."""
    return [(kill["id"], kill["key"], kill["oracle"], kill["row"]) for kill in mutation_recorded_kills(results)]


def mutation_results_excused(results):
    """The mutants of every home the results excuse as unreached: what confirm re-traces."""
    operations = results.get("operations") if isinstance(results, dict) else None
    return sorted({key for operation in (operations or {}).values() if isinstance(operation, dict)
                   for home in operation.get("homes") or [] if isinstance(home, dict) and home.get("excused") is True
                   for key in home.get("mutants") or []})


def mutation_expectation(root=ROOT):
    """What a mutation receipt must answer, derived from committed data only.

    Witness binding is read in the committed view (no Rust source), like
    coverage. `excused` are the mutants of every home a bound witness excuses
    as unreached; the receipt's hits-only trace must cover them.
    """
    import phase1_scope as scope
    path = root / scope.MUTATION_RESULTS
    manifest_path = root / scope.MUTATION_MANIFEST
    if not path.is_file() or not manifest_path.is_file():
        raise ValueError(f"mutation artifacts {scope.MUTATION_RESULTS} and {scope.MUTATION_MANIFEST} are required")
    raw = path.read_bytes()
    results = strict_json_loads(gzip.decompress(raw))
    manifest = strict_json_loads(manifest_path.read_bytes())
    kills = mutation_recorded_kills(results)
    inputs = results.get("inputs") if isinstance(results.get("inputs"), dict) else {}
    campaigns = sorted(inputs.get("traced_oracles") or inputs.get("oracles") or {})
    natives = {}
    for oracle in sorted(set(campaigns) | {kill["oracle"] for kill in kills}):
        native = root / scope.mutation_native_path(oracle) if isinstance(oracle, str) else None
        if native is None or not native.is_file():
            raise ValueError(f"no committed native freeze for mutation oracle {oracle!r}")
        natives[oracle] = sha(native)
    cases = load(root, "data/phase1/cases.json")
    committed = load(root, "data/phase1/scope.json")
    records = scope.recorded_mutations(cases, root, committed_scope=committed)
    witnesses = {row["id"]: row for row in cases.get("witnesses", []) if row.get("kind") == "mutation_kill"}
    bound = sorted(identity for identity, record in records.items()
                   if record["state"] == "bound" and record["operations"])
    digest = hashlib.sha256(raw).hexdigest()
    if any(witnesses[identity]["mutation_evidence"]["results_sha256"] != digest for identity in bound):
        raise ValueError("a bound mutation witness records a results artifact other than the committed one")
    recorded = [(kill["key"], witnesses[identity]["oracle"], kill["row"]) for identity in bound
                for kill in witnesses[identity]["mutation_evidence"]["kills"]
                if kill["op"] in records[identity]["operations"]]
    homes = manifest.get("homes") if isinstance(manifest.get("homes"), dict) else {}
    excused = sorted({key for identity in bound for operation in records[identity]["operations"]
                      for home in homes.get(operation) or [] if isinstance(home, dict)
                      and scope.home_identity(home) in (witnesses[identity].get("not_claimed") or {})
                      for key in home.get("mutants") or []})
    return {"results_sha256": digest, "manifest_sha256": sha(manifest_path), "native_sha256": natives,
            "kills": kills, "pairs": mutation_confirm_pairs(results),
            "killed_mutants": sum(isinstance(m, dict) and m.get("state") == "killed" for m in results.get("mutants", [])),
            "declared": sorted(witnesses), "bound": bound, "recorded": recorded, "excused": excused,
            "results_excused": mutation_results_excused(results), "campaigns": campaigns}


def _receipt_failure_kind(row):
    """base (mutant 0), pair (a kill pair's key and state) or excused (an excused mutant's hits)."""
    if "rows" in row:
        return "excused"
    if row.get("mutant") == 0:
        return "base"
    if isinstance(row.get("key"), str) and isinstance(row.get("status"), str):
        return "pair"
    return None


def mutation_witness_result(output, exit_code, expectation):
    """Validate a confirmation receipt; raises on anything but a real replay.

    The receipt is phase1_mutation_run.confirm's. `pairs` lists every replayed
    pair as {key, oracle, row, request_sha256, state, stages, mutant, control}:
    it must name every recorded kill of the results exactly once, with its
    request digest, and a pair in state `reproduced` must carry exactly the
    recorded stages, mutant digests and control digests; every other state is a
    lost kill. `pairs_total` and `confirmed` count them. `base_rows` and
    `base_native` count the pairs' no-mutant rows and those still equal to
    native. `oracles` are the traced oracles (every campaign of the results),
    `retraced` their hits-only traces, and `excused_mutants` the number of
    excused-home mutants traced (the results' excused homes). `failures` names
    each base row that is not native (mutant 0), each lost pair (its key and
    state) and each excused mutant that executed, while producing or while
    observing (`rows`, or null when its span went stale). A pair of an
    allocating mutant is a kill only where it differs from native and from its
    control in one same stage, so a `reproduced` pair that does not is no
    replay. `result` is `pass`, with exit code 0, exactly when there is no
    failure and every oracle was re-traced.
    """
    kills = expectation["kills"]
    if not kills:
        raise ValueError("mutation witness inventory is empty")
    if not isinstance(output, dict) or output.get("version") != 1 or output.get("kind") != MUTATION_RECEIPT:
        raise ValueError("mutation receipt is not a version 1 confirmation")
    if output.get("results_sha256") != expectation["results_sha256"]:
        raise ValueError("mutation receipt replayed a different results artifact")
    if output.get("plan_sha256") != expectation["manifest_sha256"]:
        raise ValueError("mutation receipt replayed a plan other than the committed manifest")
    if output.get("native_sha256") != expectation["native_sha256"]:
        raise ValueError("mutation receipt compared against another native freeze")
    if output.get("oracles") != expectation["campaigns"]:
        raise ValueError("mutation receipt did not replay and re-trace exactly the campaign's oracles")
    expected = {(kill["key"], kill["oracle"], kill["row"]): kill for kill in kills}
    replayed = output.get("pairs")
    if not isinstance(replayed, list) or not all(isinstance(row, dict) and set(row) == set(MUTATION_PAIR_FIELDS)
                                                 for row in replayed):
        raise ValueError("mutation receipt has no per-pair list")
    identities = [(row["key"], row["oracle"], row["row"]) for row in replayed]
    if len(set(identities)) != len(identities) or set(identities) != set(expected):
        raise ValueError("mutation receipt pairs are missing, duplicated or not recorded kills")
    lost = {}
    for row, identity in zip(replayed, identities):
        recorded = expected[identity]
        if row["request_sha256"] != recorded["request_sha256"]:
            raise ValueError(f"mutation receipt pair {identity} replayed another request")
        if not isinstance(row["state"], str):
            raise ValueError(f"mutation receipt pair {identity} has no state")
        if row["state"] != MUTATION_PAIR_REPRODUCED:
            lost[identity] = row["state"]
            continue
        reproduced = (isinstance(row["stages"], list) and all(isinstance(stage, str) for stage in row["stages"])
                      and sorted(row["stages"]) == recorded["stages"]
                      and row["mutant"] == recorded["mutant"] and row["control"] == recorded["control"])
        if not reproduced:
            raise ValueError(f"mutation receipt pair {identity} is reproduced but not as recorded")
        # A kill of an allocating mutant differs from native and from its
        # control in one same stage; confirm reports any other pair as
        # `not_a_kill`, so a `reproduced` one that is not is no replay.
        control = recorded["control"]
        if control is not None and not (isinstance(control, dict) and isinstance(recorded["mutant"], dict) and any(
                recorded["mutant"].get(stage) != control.get(stage) for stage in recorded["stages"])):
            raise ValueError(f"mutation receipt pair {identity} is reproduced, but its mutant equals its control "
                             "in every stage where it differs from native")
    base = {(kill["oracle"], kill["row"]) for kill in kills}
    counts = {name: output.get(name) for name in ("killed_mutants", "pairs_total", "confirmed", "base_rows",
                                                   "base_native", "excused_mutants")}
    if any(type(value) is not int for value in counts.values()):
        raise ValueError("mutation receipt omits a replay count")
    if (counts["killed_mutants"], counts["pairs_total"], counts["confirmed"], counts["base_rows"],
            counts["excused_mutants"]) != (expectation["killed_mutants"], len(kills), len(kills) - len(lost),
                                           len(base), len(expectation["results_excused"])):
        raise ValueError("mutation receipt counts disagree with its pairs or the results artifact")
    if not 0 <= counts["base_native"] <= counts["base_rows"]:
        raise ValueError("mutation receipt counts are out of range")
    failures = output.get("failures")
    if not isinstance(failures, list) or not all(isinstance(row, dict) for row in failures):
        raise ValueError("mutation receipt has no failure list")
    kinds = [_receipt_failure_kind(row) for row in failures]
    if None in kinds:
        raise ValueError("mutation receipt names a failure of no known kind")
    failed_base = [(row.get("oracle"), row.get("row")) for row, kind in zip(failures, kinds) if kind == "base"]
    failed_pairs = [((row["key"], row.get("oracle"), row.get("row")), row["status"])
                    for row, kind in zip(failures, kinds) if kind == "pair"]
    excused_failures = [row for row, kind in zip(failures, kinds) if kind == "excused"]
    if (len(set(failed_base)) != len(failed_base) or not set(failed_base) <= base
            or len(failed_base) != counts["base_rows"] - counts["base_native"]
            or len({pair for pair, _ in failed_pairs}) != len(failed_pairs) or dict(failed_pairs) != lost):
        raise ValueError("mutation receipt failures disagree with its pairs or counts")
    traced = set(expectation["results_excused"])
    if not all(isinstance(row.get("key"), str) and row["key"] in traced and row.get("oracle") in expectation["campaigns"]
               and (row["rows"] is None or (type(row["rows"]) is int and row["rows"] > 0)) for row in excused_failures):
        raise ValueError("mutation receipt reports an excused-home failure outside the traced homes")
    if output.get("excused_stale") != sorted({row["key"] for row in excused_failures if row["rows"] is None}):
        raise ValueError("mutation receipt stale excused homes disagree with its failures")
    retraced = output.get("retraced")
    if retraced is not None and (not isinstance(retraced, dict) or set(retraced) != set(expectation["campaigns"])
                                 or not all(isinstance(value, dict) and type(value.get("rows")) is int
                                            and value["rows"] > 0 for value in retraced.values())):
        raise ValueError("mutation receipt re-trace does not cover every traced oracle")
    passed = not failures and retraced is not None
    if output.get("result") != ("pass" if passed else "fail") or exit_code != (0 if passed else 1):
        raise ValueError("mutation receipt result or exit code disagrees with its outcomes")
    if any(pair not in expected for pair in expectation["recorded"]):
        raise ValueError("a recorded kill pair is absent from the replayed inventory")
    untraced = sorted(set(expectation["excused"]) - traced)
    unbound = sorted(set(expectation["declared"]) - set(expectation["bound"]))
    return {"state": "match" if passed and expectation["bound"] and not unbound and not untraced else "different",
            "pairs": counts["pairs_total"], "confirmed": counts["confirmed"], "base_rows": counts["base_rows"],
            "base_native": counts["base_native"], "failures": failures[:20],
            "recorded_pairs_lost": sorted(pair for pair in expectation["recorded"] if pair in lost)[:20],
            "excused_hit": sorted({row["key"] for row in excused_failures} & set(expectation["excused"]))[:20],
            "untraced_excused": untraced[:20], "unbound_witnesses": unbound}


def _is_mutation_receipt(stdout):
    try:
        output = strict_json_loads(stdout)
    except ValueError:
        return False
    return isinstance(output, dict) and output.get("kind") == MUTATION_RECEIPT


def load(root, path):
    return strict_json_loads((root / path).read_bytes())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def request_index(root):
    result = {}
    # The original combined leaves/pilot schedules overlap current group files.
    # Integration identities belong to their current grouped request files.
    for path in sorted((root / "data/phase1/requests").glob("*-*.json")):
        for row in load(root, path.relative_to(root))["requests"]:
            identity = row.get("case", row.get("id"))
            if identity in result:
                raise ValueError(f"duplicate integration case identity: {identity}")
            result[identity] = {"request": row, "path": str(path.relative_to(root))}
    return result


def input_paths(root=ROOT):
    """Static integration dependencies; producer adds transitive Rust closure."""
    document = load(root, MANIFEST)
    paths = {MANIFEST, "scripts/phase1_integration.py", "scripts/phase1_generation.py",
             "scripts/s04_common.py", "scripts/s11.py", "scripts/s11_contracts.py",
             "scripts/generate_locale_tables.py", "scripts/s03.py", "data/upstream.json",
             "data/s04/toolchains.toml", "data/s03/generated.json",
             "data/phase1/locale-tables-manifest.json", "tools/packaging/packages.json", "tools/packaging/verification.json",
             "crates/tsr_bundled/bundled/manifest.json", "upstream/package.json", "scripts/package_assets.py",
             "tools/s10/toolchains.json", "LICENSE", "NOTICE", "licenses/GO-BSD-3-Clause.txt",
             ".gitmodules", "data/s03/api-special-codecs.json", "scripts/s05_tables.py",
             "crates/tsr_testhost/Cargo.toml", "data/s07/program-requests.json"}
    for folder in ("data/s11", "tools/s11", "tools/phase1/locale", "tools/s03", "xtask"):
        paths.update(str(p.relative_to(root)) for p in (root / folder).rglob("*")
                     if p.is_file() and p.name != ".DS_Store" and "__pycache__" not in p.parts)
    # Archive verification builds every public package, including packages no
    # Phase 1 adapter depends on. Bind their manifests, build scripts and assets
    # as well as Rust sources; a dependency-only closure misses archive changes.
    for package in load(root, "tools/packaging/packages.json")["packages"]:
        if package["publish"]:
            paths.add(package["manifest"])
            directory = (root / package["manifest"]).parent
            paths.update(str(p.relative_to(root)) for p in directory.rglob("*")
                         if p.is_file() and p.name != ".DS_Store"
                         and not any(part in {".git", "target", "__pycache__"}
                                     for part in p.relative_to(directory).parts))
    requests = request_index(root)
    for witness in document["witnesses"]:
        for reference in witness["references"]:
            paths.add(requests[reference]["path"] if witness["kind"] == "cases" else reference)
        paths.update(test["path"] for test in witness.get("additional_tests", []))
    generated = load(root, document["generation"]["generated_manifest"])
    locale = load(root, document["generation"]["locale_manifest"])
    paths.update(generated["files"])
    paths.update(locale["outputs"])
    paths.update(p for p in locale["inputs"] if not p.startswith("golang.org/"))
    return sorted(paths)


def _hash_problems(root, manifest, field):
    problems = []
    for path, expected in manifest.get(field, {}).items():
        if not (root / path).is_file() or sha(root / path) != expected:
            problems.append(f"generated output differs from manifest: {path}")
    return problems


def check(root=ROOT, document=None):
    document = load(root, MANIFEST) if document is None else document
    problems = []
    if document.get("version") != 1:
        problems.append("unsupported integration manifest version")
    witnesses = document.get("witnesses", [])
    ids = [w.get("id") for w in witnesses]
    if len(ids) != len(set(ids)) or set(ids) != WITNESSES:
        problems.append("integration witness inventory missing, extra or duplicated")
    requests = request_index(root)
    rows = []
    for witness in witnesses:
        identity = witness["id"]
        if witness.get("kind") not in {"cases", "test", "driver", "installed"}:
            problems.append(f"{identity}: unknown integration witness kind")
        if not witness.get("claim") or not witness.get("command") or witness.get("owner") != "F5b":
            problems.append(f"{identity}: missing claim, command or owner")
        refs = witness.get("references", [])
        if not refs or len(refs) != len(set(refs)):
            problems.append(f"{identity}: missing or duplicated witness references")
        for reference in refs:
            if witness.get("kind") == "cases":
                if reference not in requests:
                    problems.append(f"{identity}: unknown case {reference}")
            elif not (root / reference).is_file():
                problems.append(f"{identity}: missing fixture {reference}")
        tests = witness.get("additional_tests", [])
        if witness.get("kind") == "test":
            tests = [dict(path=refs[0], test=witness.get("test", "")), *tests]
        for test in tests:
            source = root / test["path"]
            if not source.is_file() or not re.search(r"\bfn\s+" + re.escape(test["test"]) + r"\s*\(", source.read_text()):
                problems.append(f"{identity}: missing named test {test['test']}")
        # Source/fixture availability proves runnable preparation, not execution.
        rows.append({**witness, "state": "pending", "reason": witness.get("dependency", "requires current contributing observations"), "prepared": True})
    transport = document.get("transport", {})
    mapping = transport.get("cases", [])
    transport_ids = [row.get("case") for row in mapping]
    expected = load(root, "data/s11/cases.json")
    if transport_ids != expected or len(expected) != 89:
        problems.append("S11 mapping must preserve all 89 case identities in order")
    counted = Counter()
    for row in mapping:
        obligations = row.get("obligations", [])
        if not obligations or len(obligations) != len(set(obligations)) or set(obligations) - OBLIGATIONS:
            problems.append(f"{row.get('case')}: unknown, empty or duplicate transport obligation")
        counted.update(obligations)
        authority = ("pinned-Go-stream" if row.get("case", "").startswith("mapper/") else
                     "pinned-Go-callbackFS" if row.get("case", "").startswith("fs/") else "ADR-0019-contract")
        if row.get("authority") != authority:
            problems.append(f"{row.get('case')}: wrong native/contract authority")
    if set(counted) != OBLIGATIONS:
        problems.append("a continuing S11 obligation has no case")
    mapper_ids = [row["id"] for row in load(root, "data/s11/mapper-fixtures.json")]
    if transport.get("mapper_recordings") != mapper_ids or len(mapper_ids) != 5:
        problems.append("mapper mapping must name the five actual Go stream recordings")
    if set(document.get("boundaries", [])) != BOUNDARIES:
        problems.append("integration boundary inventory changed or omitted")
    generated = load(root, "data/s03/generated.json")
    locale = load(root, "data/phase1/locale-tables-manifest.json")
    pin = load(root, "data/upstream.json")["pin"]
    if generated.get("upstreamPin") != pin or locale.get("pin") != pin:
        problems.append("generation manifests do not name the current pin")
    from phase1_generation import LOCALE_OUTPUTS
    if set(locale.get("outputs", {})) != LOCALE_OUTPUTS:
        problems.append("locale generation output inventory incomplete")
    if "crates/tsr_diagnostics/src/locales_generated.rs" not in generated.get("files", {}):
        problems.append("generated diagnostic locale messages are absent")
    problems.extend(_hash_problems(root, generated, "files"))
    problems.extend(_hash_problems(root, locale, "outputs"))
    package = load(root, "upstream/package.json")
    pins = package["volta"]
    if package["packageManager"].split("+", 1)[0] != "npm@" + pins["npm"]:
        problems.append("pinned npm versions disagree")
    return {
        "version": 1, "prepared": not problems, "complete": False, "problems": problems,
        "witnesses": rows,
        "transport": {"prepared": not problems, "cases": len(mapping), "obligations": dict(sorted(counted.items())),
                      "mapper_recordings": mapper_ids, "state": "pending", "producer": "testhost",
                      "reason": "Reuse only evidence validated current by the tracker; this map is not execution evidence."},
        "generation": {"outputs_authenticated": not _hash_problems(root, generated, "files") and not _hash_problems(root, locale, "outputs"),
                       "state": "pending", "reason": "exact regeneration and untouched-client checks use the gen producer",
                       "node": pins["node"], "npm": pins["npm"],
                       "node_bootstrap": document["generation"]["node_bootstrap"],
                       "npm_bootstrap": document["generation"]["npm_bootstrap"]},
        "boundaries": document.get("boundaries", []),
    }


def localized_result(root, observed):
    expected_keys = {"id", "locale", "code", "leaf_localized", "writer_localized"}
    if set(observed) != expected_keys or observed["id"] != "localized-config-diagnostics" or observed["code"] != 5023 or observed["locale"] != "de-DE":
        raise ValueError("localized config observation has the wrong schedule")
    table = strict_json_loads(gzip.decompress((root / "upstream/tsc/internal/diagnostics/loc/de-DE.json.gz").read_bytes()))
    # Independent pinned localization data. No Rust-generated table is authority.
    expected = table["Unknown_compiler_option_0_5023"].replace("{0}", "notAnOption")
    if observed["leaf_localized"] != expected:
        raise ValueError("leaf localization no longer agrees with pinned Go catalog")
    return {"id": observed["id"], "status": "match" if observed["writer_localized"] == expected else "different",
            "expected": expected, "actual": observed["writer_localized"], "observation": observed,
            "authority": "pinned Go de-DE diagnostic catalog; structural locale propagation requirement",
            "scope": "config parser to production writer text; not a 309-baseline count"}


def validate_installed_observation(root, observed):
    """Require the archive consumer's actual locale/message/ownership outputs."""
    if not isinstance(observed, dict) or set(observed) != {
            "encoded_bytes", "libraries", "locale", "diagnostic_code",
            "localized_message", "owners_returned_to_baseline"}:
        raise ValueError("installed consumer omits generated-asset observations")
    table = strict_json_loads(gzip.decompress((root / "upstream/tsc/internal/diagnostics/loc/de-DE.json.gz").read_bytes()))
    expected = table["Type_0_is_not_assignable_to_type_1_2322"].replace("{0}", "number").replace("{1}", "string")
    if (type(observed["encoded_bytes"]) is not int or observed["encoded_bytes"] <= 0
            or type(observed["libraries"]) is not int or observed["libraries"] != 108
            or type(observed["diagnostic_code"]) is not int or observed["diagnostic_code"] != 2322
            or observed["locale"] != "de-DE" or observed["localized_message"] != expected
            or observed["owners_returned_to_baseline"] is not True):
        raise ValueError("installed consumer generated-asset observations differ")


def localized_integration_result(root, leaf, envelopes):
    import phase1_localized
    leaf_result = localized_result(root, leaf)
    envelope_result = phase1_localized.compare(envelopes, root)
    return {"leaf": leaf_result, "envelopes": {**envelope_result, "observation": envelopes},
            "status": "match" if leaf_result["status"] == envelope_result["status"] == "match" else "different"}


def receipt(identity, command, source_inputs, stdout, *, stderr="", exit_code=0, artifact=None):
    """Serialize outputs of an actually executed command; never executes a child.

    The caller fingerprints the complete producer closure before and after the
    command and may record a receipt only when those two maps are identical.
    """
    result = {"id": identity, "command": command, "source_inputs": source_inputs,
              "exit_code": exit_code, "stdout": stdout, "stderr": stderr,
              "stdout_sha256": hashlib.sha256(stdout.encode()).hexdigest(),
              "stderr_sha256": hashlib.sha256(stderr.encode()).hexdigest()}
    if artifact is not None:
        result["artifact"] = artifact
        result["artifact_sha256"] = hashlib.sha256(json.dumps(artifact, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return result


def evaluate(preparation, family_reports, receipts=(), *, source_inputs=None, root=ROOT):
    """Evaluate already authenticated family comparisons and measured receipts.

    `family_reports` must come directly from normal capture replay. Reports may
    not be constructed from the inventory's last-result convenience fields.
    Receipts are authenticated against the caller's freshly recomputed complete
    producer input map; absent receipts remain pending instead of becoming pass.
    """
    import copy
    result = copy.deepcopy(preparation)
    problems = result["problems"]
    cases = {}
    for report in family_reports:
        for row in report.get("rows", []):
            identity = row.get("case")
            if identity in cases:
                problems.append(f"duplicate contributing integration case: {identity}")
            if row.get("result") not in {"match", "different", "not_implemented", "native_unavailable", "not_applicable", "harness_failed", "not_run"}:
                problems.append(f"{identity}: malformed contributing comparison")
            cases[identity] = row.get("result")
    document = load(root, MANIFEST)
    import phase1_scope
    expected = {row["id"]: row for row in document["witnesses"] if row["kind"] != "cases"}
    expected.update({
        "transport": {"command": ["python3", "scripts/s11.py", "capture"]},
        "generation": {"command": document["generation"]["command"]},
        "rust-witnesses": {"command": ["python3", "scripts/phase1_integration.py", "observe-rust-witnesses"]},
        MUTATION_RECEIPT: {"command": phase1_scope.MUTATION_CONFIRM_COMMAND},
    })
    # Additional named tests are independent required observations. A passing
    # resolver case cannot stand in for the explicit source-order tie test.
    for row in document["witnesses"]:
        for test in row.get("additional_tests", []):
            expected[row["id"] + "/" + test["test"]] = {**test, "kind": "test"}
    measured = {}
    unavailable = {}
    for item in receipts:
        identity = item.get("id")
        if identity in measured:
            problems.append(f"duplicate integration receipt: {identity}")
            continue
        measured[identity] = "invalid"
        if identity not in expected:
            problems.append(f"unknown integration receipt: {identity}")
            continue
        if item.get("command") != expected[identity]["command"]:
            problems.append(f"{identity}: integration receipt command differs")
            continue
        if any(type(item.get(key)) is not str or hashlib.sha256(item[key].encode()).hexdigest() != item.get(key + "_sha256") for key in ("stdout", "stderr")):
            problems.append(f"{identity}: integration output digest differs")
            continue
        if type(item.get("exit_code")) is not int:
            problems.append(f"{identity}: missing measured exit code")
            continue
        if "artifact" in item or "artifact_sha256" in item:
            raw = json.dumps(item.get("artifact"), sort_keys=True, separators=(",", ":")).encode()
            if hashlib.sha256(raw).hexdigest() != item.get("artifact_sha256"):
                problems.append(f"{identity}: integration artifact digest differs")
                continue
        recorded_inputs = item.get("source_inputs")
        if not source_inputs or not isinstance(recorded_inputs, dict) or not recorded_inputs or any(
                not isinstance(name, str) or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
                for name, digest in recorded_inputs.items()):
            problems.append(f"{identity}: malformed integration receipt inputs")
            continue
        if recorded_inputs != source_inputs:
            measured[identity] = "unavailable"
            unavailable[identity] = {"reason": "integration receipt inputs changed", "changed_inputs": sorted(
                name for name in recorded_inputs.keys() | source_inputs.keys()
                if recorded_inputs.get(name) != source_inputs.get(name))}
            continue
        # A confirmation that printed its receipt reports lost kills through
        # exit code 1, so that output is validated either way; one that died
        # before printing a receipt failed like any other command.
        if item["exit_code"] != 0 and (identity != MUTATION_RECEIPT or not _is_mutation_receipt(item["stdout"])):
            measured[identity] = "failed"
            continue
        try:
            if identity == MUTATION_RECEIPT:
                observation = mutation_witness_result(strict_json_loads(item["stdout"]), item["exit_code"],
                                                      mutation_expectation(root))
                result["mutation_witness_observations"] = observation
                measured[identity] = observation["state"]
            elif identity == "localized-config-diagnostics":
                output = strict_json_loads(item["stdout"])
                actual = localized_integration_result(root, output["leaf"]["observation"], output["envelopes"]["observation"])
                if output != actual:
                    raise ValueError("localized comparison differs from independent replay")
                measured[identity] = actual["status"]
            elif expected[identity].get("kind") == "test":
                test = expected[identity]["test"]
                outcomes = re.findall(r"^test ([\w:]+) \.\.\. (\w+)$", item["stdout"], re.MULTILINE)
                wanted = [state for name, state in outcomes if name.rsplit("::", 1)[-1] == test]
                if wanted != ["ok"]:
                    raise ValueError("named Rust integration test did not execute exactly once")
                measured[identity] = "match"
            elif identity == "transport":
                output = strict_json_loads(item["stdout"])
                tests = output.get("tests", {})
                if set(tests) != set(load(root, "data/s11/cases.json")) or any(state not in ("pass", "fail") for state in tests.values()):
                    raise ValueError("S11 receipt does not contain all 89 measured cases")
                if type(output.get("metrics", {}).get("controls")) is not bool:
                    raise ValueError("S11 controls metric missing")
                measured[identity] = "match" if all(state == "pass" for state in tests.values()) and output["metrics"]["controls"] else "different"
            elif identity == "generation":
                metrics = strict_json_loads(item["stdout"])["metrics"]
                required = {"ast_schema": True, "patches_apply": True, "client_identical": True, "drift": False, "locale_complete": True}
                if any(type(metrics.get(key)) is not bool for key in required):
                    raise ValueError("generation receipt omits a required measurement")
                measured[identity] = "match" if all(metrics[k] is value for k, value in required.items()) else "different"
            elif identity == "rust-witnesses":
                observations = rust_witness_result(strict_json_loads(item["stdout"]))
                result["rust_witness_observations"] = observations
                measured[identity] = "match" if all(value == "match" for value in observations.values()) else "different"
            elif identity == "installed-generated-assets":
                artifact = item.get("artifact")
                raw = json.dumps(artifact, sort_keys=True, separators=(",", ":")).encode()
                if hashlib.sha256(raw).hexdigest() != item.get("artifact_sha256"):
                    raise ValueError("installed-consumer artifact digest differs")
                packages = {r["name"] for r in load(root, "tools/packaging/packages.json")["packages"] if r["publish"]}
                if not isinstance(artifact, dict) or artifact.get("state") != "pass" or set(artifact.get("archives", {})) != packages:
                    raise ValueError("installed-consumer report is partial or failed")
                commands = artifact.get("commands", [])
                if not any(row.get("log") == "consumer.log" and Path(row["command"][0]).name == "package_consumer" for row in commands):
                    raise ValueError("installed consumer did not execute")
                validate_installed_observation(root, artifact.get("consumer_observation"))
                measured[identity] = "match"
        except (ValueError, KeyError, TypeError, IndexError) as error:
            problems.append(f"{identity}: {error}")
    for row in result["witnesses"]:
        if row["kind"] == "cases":
            outcomes = [cases.get(identity, "pending") for identity in row["references"]]
            outcomes += [measured.get(row["id"] + "/" + test["test"], "pending") for test in row.get("additional_tests", [])]
            row["observations"] = {identity: cases.get(identity, "pending") for identity in row["references"]}
            row["additional_test_observations"] = {test["test"]: measured.get(row["id"] + "/" + test["test"], "pending") for test in row.get("additional_tests", [])}
            row["state"] = ("pending" if not outcomes else
                            "match" if all(state == "match" for state in outcomes) else
                            "pending" if "pending" in outcomes else "different")
        else:
            row["state"] = measured.get(row["id"], "pending")
        if row["state"] == "match":
            row.pop("reason", None)
    for key in ("transport", "generation"):
        result[key]["state"] = measured.get(key, "pending")
    result["unavailable"] = unavailable
    result["rust_witnesses"] = {"state": measured.get("rust-witnesses", "pending")}
    # Its own metric (P1B-F4b), deliberately outside `complete` (F5b).
    result["mutation_witnesses"] = {"state": measured.get(MUTATION_RECEIPT, "pending")}
    result["metric_observations"] = {"cases": dict(sorted(cases.items())), "receipts": dict(sorted(measured.items()))}
    result["complete"] = (not problems and {row["id"] for row in result["witnesses"]} == WITNESSES
                          and len(result["witnesses"]) == len(WITNESSES)
                          and all(row["state"] == "match" for row in result["witnesses"])
                          and all(result[k]["state"] == "match" for k in ("transport", "generation", "rust_witnesses")))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("check", "observe-localized-config", "observe-rust-witnesses"), default="check", nargs="?")
    args = parser.parse_args()
    if args.operation == "check":
        result = check()
    elif args.operation == "observe-rust-witnesses":
        result = []
        for identity, (command, _) in RUST_WITNESS_TESTS.items():
            child = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
            result.append({"id": identity, "command": command, "exit_code": child.returncode,
                           "stdout": child.stdout.decode(), "stderr": child.stderr.decode()})
        # Preserve build failures/invalid test execution as recorded output;
        # receipt replay will distinguish invalid execution from measured fail.
    else:
        import phase1_localized
        output = subprocess.run(["cargo", "run", "--locked", "-p", "tsr_compiler", "--example", "phase1_integration"],
                                cwd=ROOT, stdout=subprocess.PIPE, stderr=sys.stderr, check=True)
        envelopes = phase1_localized.observe()
        result = localized_integration_result(ROOT, strict_json_loads(output.stdout), envelopes["observation"])
    # The localized observation retains the renderer's exact ordered request
    # fields; sorting its nested raw result would invalidate that request hash
    # when the receipt is replayed.
    print(json.dumps(result, indent=2, sort_keys=args.operation != "observe-localized-config"))
    return int(isinstance(result, dict) and bool(result.get("problems")))


if __name__ == "__main__":
    raise SystemExit(main())
