"""Go-only S07 subset preparation; eligibility never reads Rust observations."""

from collections import Counter, defaultdict
import copy
import hashlib
import io
import json
from pathlib import Path
import shutil
import tarfile

from s04 import verified_upstream
from s04_common import command, strict_json_loads
from s06_build import GO_TEST_FLAGS, oracle_export
from s06_corpus import freeze_records, membership, primary_id, validate_export_record

ROOT = Path(__file__).resolve().parents[1]
TABLES = ROOT / "tools/s07/subset"
PIN = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
PRODUCER_INPUTS = (
    "tools/s07/subset/export_test.go", "tools/s07/subset/options_bridge.go",
    "scripts/s07_subset.py", "scripts/s06_build.py",
    "scripts/s06_oracle/export_test.go", "scripts/s06_oracle/metadata_bridge.go",
    "scripts/s06_oracle/export_boundaries_test.go", "scripts/s06_oracle/fixture_export_test.go",
    "scripts/s04.py", "scripts/s04_common.py", "scripts/s04_runtime.py", "scripts/tracking-bootstrap.py",
    "data/s04/toolchains.toml", "data/upstream.json", ".gitmodules",
)


def json_bytes(value):
    # core.CompilerOptions.Paths and ParsedCommandLine.Raw are ordered source
    # maps. Canonicalize structural keys, preserving those semantic map orders.
    def ordered(item, preserve=False):
        if isinstance(item, dict):
            keys = item if preserve else sorted(item)
            return {key: ordered(item[key], preserve or key in {"paths", "config_raw"}) for key in keys}
        if isinstance(item, list):
            return [ordered(child, preserve) for child in item]
        return item
    return (json.dumps(ordered(value), ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def producer_inputs():
    return {name: sha256((ROOT / name).read_bytes()) for name in PRODUCER_INPUTS}


def export_observations(destination, paths=None):
    """Publish a complete observation file only after the Go producer succeeds."""
    destination = Path(destination).resolve()
    if paths is None:
        paths, _ = membership(verified_upstream(), PIN)
    inputs = producer_inputs()
    with oracle_export() as (checkout, env, pin):
        assert pin == PIN
        fixture_archive = command(["git", "archive", PIN, "tsc/testdata/tests/lib"], cwd=verified_upstream())
        with tarfile.open(fileobj=io.BytesIO(fixture_archive)) as stream:
            stream.extractall(checkout, filter="data")
        shutil.copyfile(TABLES / "export_test.go", checkout / "tsc/internal/testrunner/s07_subset_export_test.go")
        shutil.copyfile(TABLES / "options_bridge.go", checkout / "tsc/internal/testutil/harnessutil/s07_subset_options_bridge.go")
        request, output = checkout / "s07-input.json", checkout / "s07-output.ndjson"
        request.write_bytes(json_bytes(paths))
        env.update(S07_SUBSET_INPUT=str(request), S07_SUBSET_OUTPUT=str(output), S06_EXTRACT_ONLY="0")
        repo_path = f"-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s07-unmatched-prefix"
        command(["go", "test", *GO_TEST_FLAGS, repo_path, "./internal/testrunner", "-run",
                 "^TestS07SubsetExport$", "-count=1", "-timeout=15m"], cwd=checkout / "tsc", env=env)
        verified_upstream()
        if producer_inputs() != inputs:
            raise ValueError("S07 source producer changed during export")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(output, destination)
        provenance = {"version": 1, "pin": PIN, "producer_inputs": inputs,
                      "observation_sha256": sha256(output.read_bytes()), "physical_paths": paths}
        destination.with_suffix(".provenance.json").write_bytes(json_bytes(provenance))


def read_observations(path):
    path = Path(path)
    provenance = strict_json_loads(path.with_suffix(".provenance.json").read_bytes())
    if (provenance["pin"] != PIN or provenance["producer_inputs"] != producer_inputs()
            or provenance["observation_sha256"] != sha256(path.read_bytes())):
        raise ValueError("S07 syntax/config observations are stale or have changed")
    records = []
    with path.open("rb") as stream:
        while line := stream.readline(64 * 1024 * 1024 + 1):
            if len(line) > 64 * 1024 * 1024 or not line.endswith(b"\n"):
                raise ValueError("S07 observation exceeds bounded NDJSON record size")
            records.append(strict_json_loads(line))
    if not records or set(records[0]) != {"record_kind", "kinds", "option_keys", "pragma_names"} or records[0]["record_kind"] != "syntax_kinds":
        raise ValueError("missing pinned SyntaxKind inventory")
    inventory = records.pop(0)
    physical, libraries = membership(verified_upstream(), PIN)
    if provenance["physical_paths"] != physical:
        raise ValueError("partial S07 observation cannot classify the primary corpus")
    cases, libs, fixture_libs, source = [], [], [], []
    for row in records:
        if row.get("record_kind") == "case":
            validate_export_record(row["case"])
            cases.append(row)
            source.append(row["case"])
        elif row.get("record_kind") == "library":
            lib = {k: row[k] for k in ("path", "filename", "text_hex")}
            lib["kind"] = "library"
            validate_export_record(lib)
            source.append(lib)
            libs.append(row)
        elif row.get("record_kind") == "fixture_library":
            fixture_libs.append(row)
        else:
            raise ValueError("unknown S07 observation record")
    if [row["path"] for row in source] != physical + libraries:
        raise ValueError("S07 observations have missing, extra, duplicate or reordered physical cases/libraries")
    fixture_paths = command(["git", "ls-tree", "-r", "--name-only", PIN, "tsc/testdata/tests/lib"], cwd=verified_upstream()).decode().splitlines()
    if [row["path"] for row in fixture_libs] != fixture_paths:
        raise ValueError("missing, extra, duplicate or reordered pinned fixture libraries")
    for lib in fixture_libs:
        if bytes.fromhex(lib["raw_hex"]) != (verified_upstream() / lib["path"]).read_bytes():
            raise ValueError("fixture library bytes differ from source pin")
    # Reestablish the same physical and virtual decoding boundary independently;
    # stale parser qualifications or lost configurations fail before selection.
    documents, _ = freeze_records(source, verified_upstream(), PIN)
    corpus = documents["corpus.json"]
    frozen = strict_json_loads((ROOT / "data/s06/corpus.json").read_bytes())
    if corpus != frozen:
        raise ValueError("fresh S07 preprocessing differs from frozen S06 physical/virtual corpus")
    return inventory, cases, libs + fixture_libs, frozen


def table(name, key):
    value = strict_json_loads((TABLES / name).read_bytes())
    rows = value["rows"]
    result = {}
    for row in rows:
        if row[key] in result or row["disposition"] not in {"allow", "reject", "irrelevant"} or not row["reason"]:
            raise ValueError(f"invalid or duplicate {name} classification")
        result[row[key]] = row
    return result


def baseline_inventory():
    prefix = "tsc/testdata/baselines/reference/"
    raw = command(["git", "ls-tree", "-r", PIN, prefix], cwd=verified_upstream()).decode()
    index = defaultdict(list)
    suffixes = (".errors.txt", ".types", ".symbols", ".js", ".d.ts", ".js.map", ".d.ts.map", ".sourcemap.txt", ".trace.json")
    for line in raw.splitlines():
        info, path = line.split("\t", 1)
        for suffix in suffixes:
            if path.endswith(suffix):
                index[path[:-len(suffix)]].append({"path": path, "git_blob": info.split()[2], "kind": suffix})
                break
    return index


def source_reasons(variant, syntax_table):
    reasons = []
    for unit in variant["syntax"]:
        observed = unit["syntax"]
        if (type(observed["kinds"]) is not dict or not observed["kinds"]
                or set(observed["first_positions"]) != set(observed["kinds"])
                or any(type(n) is not int or n <= 0 for n in observed["kinds"].values())
                or any(type(n) is not int for n in observed["first_positions"].values())
                or type(observed["nonempty_type_arguments"]) is not list
                or any(type(n) is not int for n in observed["nonempty_type_arguments"])):
            raise ValueError("invalid syntax count/position/type-argument observation")
        if set(observed["kinds"]) - set(syntax_table):
            raise ValueError("unclassified syntax kind")
        if unit["route"] == "initial_config_direct":
            continue
        for kind in sorted(observed["kinds"]):
            if kind not in syntax_table:
                raise ValueError(f"unclassified syntax kind {kind}")
            if syntax_table[kind]["disposition"] == "reject":
                reasons.append({"rule": syntax_table[kind]["family"], "unit": unit["unit"], "kind": kind,
                                "position": observed["first_positions"][kind]})
        if observed["nonempty_type_arguments"]:
            reasons.append({"rule": "nonempty_type_arguments", "unit": unit["unit"], "position": observed["nonempty_type_arguments"][0]})
    if variant["project_references"]:
        reasons.append({"rule": "project_references"})
    if variant["content_mappers"] and variant["request"]["options"].get("runExternalCode") is True:
        reasons.append({"rule": "content_mapper_execution"})
    return reasons


def emitted_only(variant, baselines):
    kinds = {b["kind"] for b in baselines}
    return (variant["harness_options"]["NoTypesAndSymbols"] is True
            and bool(kinds & {".js", ".d.ts", ".js.map", ".d.ts.map", ".sourcemap.txt"})
            and not kinds & {".errors.txt", ".types", ".symbols"})


def classify(observations):
    inventory, cases, libs, corpus = read_observations(observations)
    syntax_table = table("syntax-table.json", "kind")
    directive_table = table("directive-table.json", "key")
    option_table = table("option-table.json", "key")
    pragma_table = table("pragma-table.json", "key")
    config_option_table = table("config-option-table.json", "key")
    config_root_table = table("config-root-table.json", "key")
    if inventory["kinds"] != list(syntax_table):
        raise ValueError("total SyntaxKind table differs from actual pinned enum inventory")
    if inventory["option_keys"] != list(option_table):
        raise ValueError("total option table differs from actual core.CompilerOptions field inventory")
    if inventory["pragma_names"] != sorted(pragma_table):
        raise ValueError("total pragma table differs from pinned extractPragmas source guards")
    observed_directives = {key for row in cases for key in row["case"]["raw_settings"]}
    if observed_directives != set(directive_table):
        raise ValueError("total directive table differs from complete pinned corpus inventory")
    for name, field, rows in (("config options","config_option_keys",config_option_table),("config roots","config_root_keys",config_root_table)):
        observed = {key for case in cases for variant in case["variants"] for key in variant[field] or []}
        if observed != set(rows):
            raise ValueError(f"total {name} table differs from complete pinned config inventory")
    for name, rows in (("directives",directive_table),("options",option_table),("pragmas",pragma_table),("config options",config_option_table),("config roots",config_root_table)):
        if any(row["disposition"] == "reject" for row in rows.values()):
            raise ValueError(f"{name} table rejection requires an explicit executable value/boundary predicate")
    baselines = baseline_inventory()
    frozen_cases = {c["path"]: c for c in corpus["cases"]}
    summaries, requests = [], []
    counts = Counter()
    directories = defaultdict(Counter)
    features = defaultdict(Counter)
    directives = defaultdict(Counter)
    options = defaultdict(Counter)
    for case in cases:
        c = case["case"]
        cid = primary_id(c["path"])
        directory = str(Path(cid).parent)
        counts["physical_cases"] += 1
        for key in c["raw_settings"]:
            if key not in directive_table:
                raise ValueError(f"unclassified directive {key} in {cid}")
            directives[key]["physical_cases"] += 1
        if len(case["variants"]) != len(c["variants"]):
            raise ValueError(f"missing effective configuration in {cid}")
        variants = []
        for i, variant in enumerate(case["variants"]):
            if variant["configuration"] != i:
                raise ValueError(f"duplicate/reordered variant in {cid}")
            actual_inputs = [(u["unit"], u["route"]) for u in variant["syntax"]]
            expected_inputs = [(u["unit"], u["route"]) for u in c["variants"][i]["inputs"]]
            if actual_inputs != expected_inputs:
                raise ValueError(f"missing, extra, duplicate or reordered syntax input in {cid}")
            for key in variant["request"]["options"]:
                if key not in option_table:
                    raise ValueError(f"unclassified effective option {key} in {cid}")
                options[key]["all_variants"] += 1
            for unit in variant["syntax"]:
                for pragma in unit["syntax"]["pragmas"] or []:
                    if pragma["Name"] not in pragma_table:
                        raise ValueError(f"unclassified pragma {pragma['Name']} in {cid}")
            basename = str(Path(variant["configured_name"]).with_suffix(""))
            baseline_key = "tsc/testdata/baselines/reference/" + cid.split("/")[0] + "/" + basename
            identities = baselines.get(baseline_key, [])
            reasons = source_reasons(variant, syntax_table)
            if emitted_only(variant, identities):
                reasons.append({"rule": "emitted_output_only", "baseline_key": baseline_key})
            eligible = not reasons
            disposition = "eligible" if eligible else "excluded"
            counts[disposition + "_variants"] += 1
            if eligible and any(u["syntax"]["diagnostics"] for u in variant["syntax"] if u["route"] != "initial_config_direct"):
                counts["eligible_malformed_variants"] += 1
            directories[directory][disposition + "_variants"] += 1
            observed_kinds = {kind for unit in variant["syntax"] if unit["route"] != "initial_config_direct" for kind in unit["syntax"]["kinds"]}
            for family in {syntax_table[kind]["family"] for kind in observed_kinds}:
                features[family][disposition + "_variants"] += 1
            rid = f"{cid}#configuration={i}"
            request = copy.deepcopy(variant["request"])
            request["id"] = rid
            if eligible:
                requests.append(request)
                for key in c["raw_settings"]:
                    directives[key]["eligible_variants"] += 1
                for key in request["options"]:
                    options[key]["eligible_variants"] += 1
            variants.append({"id": rid, "configuration": i, "configured_name": variant["configured_name"],
                             "disposition": disposition, "reasons": reasons, "options": request["options"],
                             "harness_options": variant["harness_options"], "config_options": variant["config_options"],
                             "config_raw": variant["config_raw"], "config_option_keys": variant["config_option_keys"],
                             "config_root_keys": variant["config_root_keys"], "extended_configs": variant["extended_configs"],
                             "option_diagnostics": variant["option_diagnostics"], "config_diagnostics": variant["config_diagnostics"],
                             "option_outcome": "rejected" if variant["option_diagnostics"] else "accepted",
                             "source_syntax": variant["syntax"], "baselines": identities,
                             "loading_request_sha256": sha256(json_bytes(request)), "roots": request["roots"],
                             "effective_lib_closure": None, "dependency_closure": None})
        if any(v["disposition"] == "eligible" for v in variants):
            counts["eligible_physical_cases"] += 1
            directories[directory]["eligible_physical_cases"] += 1
        summaries.append({"id": cid, "source": frozen_cases[c["path"]], "variants": variants})
    rule = {"version": 1, "pin": PIN, "state": "candidate_pending_dependency_closure_and_review",
            "classifier_inputs": {name: sha256((ROOT / name).read_bytes()) for name in ("scripts/s07_subset.py", "scripts/s06_corpus.py", "scripts/s06_protocol.py")},
            "boundary": "effective_variant; every qualifying variant of every compiler/conformance case",
            "tables": {name: sha256((TABLES / name).read_bytes()) for name in
                       ("syntax-table.json", "directive-table.json", "option-table.json", "pragma-table.json", "config-option-table.json", "config-root-table.json")},
            "predicates": {"type_arguments": "nonempty Node.TypeArguments on the pinned supported payload kinds",
                           "project_references": "len(parsed_config.ProjectReferences) != 0",
                           "content_mapper_execution": "effective runExternalCode == true and configured content mappers nonempty",
                           "emitted_output_only": "NoTypesAndSymbols true and emitted baseline present and no errors/types/symbols baseline for exact configured name"},
            "diagnostics_are_exclusions": False,
            "counts": counts, "by_directory": directories, "by_feature": features,
            "directives": directives, "effective_options": options}
    return {"rule": rule, "cases": summaries, "libraries": libs, "requests": requests}


def prepare(observations, destination):
    result = classify(observations)
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    for key, name in (("rule", "subset-rule.candidate.json"), ("cases", "subset.candidate.json"),
                      ("libraries", "library-syntax.candidate.json"), ("requests", "loading-requests.candidate.json")):
        (destination / name).write_bytes(json_bytes(result[key]))
    return result["rule"]["counts"]
