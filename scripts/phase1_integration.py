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
    expected = {row["id"]: row for row in document["witnesses"] if row["kind"] != "cases"}
    expected.update({
        "transport": {"command": ["python3", "scripts/s11.py", "capture"]},
        "generation": {"command": document["generation"]["command"]},
        "rust-witnesses": {"command": ["python3", "scripts/phase1_integration.py", "observe-rust-witnesses"]},
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
        if item["exit_code"] != 0:
            measured[identity] = "failed"
            continue
        try:
            if identity == "localized-config-diagnostics":
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
