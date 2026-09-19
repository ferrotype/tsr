"""Direct Go helper observations plus the complete frozen Rust test inventory."""
from pathlib import Path
import re
import sys

from s04_common import strict_json_loads
from s06_protocol import canonical, exact_keys, sha256
from s06_utilities import setup, invoke, test_inventory, test_result

ROOT = Path(__file__).resolve().parents[1]
GROUPS = {
    "ast": ("tsr_ast", "binder_helpers::tests::", "scripts/s07_ast_helpers.py"),
    "scanner": ("tsr_scanner", "binder_helpers::tests::", "scripts/s07_scanner_helpers.py"),
    "resolver": ("tsr_binder", "name_resolver::tests::", "scripts/s07_resolver.py"),
    "diagnostic_order": ("tsr_ast", "diagnostic_order::tests::", "scripts/s07_diagnostic_order.py"),
    "pattern": ("tsr_core", "pattern::tests::", None),
    "sort": ("tsr_core", "go_sort::tests::", "tools/s06/node-index-sort/generate.py"),
    "bound_factory": ("tsr_binder", "bound_factory_tests::", "scripts/s07_bound_clone.py"),
}


def load_manifest():
    raw = (ROOT / "data/s07/helper-tests.json").read_bytes()
    manifest = strict_json_loads(raw)
    exact_keys(manifest, ("version", "pin", "groups"), "S07 helper inventory")
    if type(manifest["version"]) is not int or manifest["version"] != 1 or manifest["pin"] != strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]:
        raise ValueError("helper inventory version/pin drift")
    if type(manifest["groups"]) is not list or [group.get("name") for group in manifest["groups"]] != list(GROUPS):
        raise ValueError("missing, reordered or duplicate helper group")
    for group in manifest["groups"]:
        exact_keys(group, ("name", "package", "prefix", "checker", "tests"), "helper test group")
        if (group["package"], group["prefix"], group["checker"]) != GROUPS[group["name"]]:
            raise ValueError("helper group changed source checker or Rust namespace")
        tests = group["tests"]
        if (type(tests) is not list or not tests or any(type(name) is not str or not re.fullmatch(r"[a-z0-9_:]+", name) or not name.startswith(group["prefix"]) for name in tests)
                or tests != sorted(set(tests))):
            raise ValueError("invalid exact helper test names")
    return manifest, sha256(raw)


def measure(directory=ROOT / "target/s07-binder-reports/helpers"):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    manifest, digest = load_manifest()
    for group in manifest["groups"]:
        if group["checker"]:
            setup([sys.executable, group["checker"], "--check"], ROOT, directory / (group["name"] + "-oracle"))
    packages = sorted({group["package"] for group in manifest["groups"]})
    args = ["cargo", "test", "--locked", "--release", "--lib", "--no-run", "--message-format=json"]
    for package in packages:
        args.extend(["--package", package])
    output = setup(args, ROOT, directory / "build")
    binaries = {}
    for line in output.splitlines():
        artifact = strict_json_loads(line)
        if artifact.get("reason") != "compiler-artifact" or not artifact.get("executable"):
            continue
        target = artifact["target"]
        name = target["name"]
        if target["kind"] != ["lib"] or name not in packages or name in binaries or artifact["profile"]["test"] is not True:
            raise ValueError("unexpected/duplicate helper test binary")
        binaries[name] = artifact["executable"]
    if set(binaries) != set(packages):
        raise ValueError("missing helper test binary")
    inventories = {package: setup([binary, "--list", "--format=terse"], ROOT, directory / (package + "-inventory")) for package, binary in binaries.items()}
    results = []
    for group in manifest["groups"]:
        test_inventory(inventories[group["package"]], group)
        for index, name in enumerate(group["tests"]):
            prefix = directory / f'{group["name"]}-{index}'
            result = invoke([binaries[group["package"]], name, "--exact", "--test-threads=1", "--color=never"], ROOT, prefix)
            results.append({"group": group["name"], "test": name, "pass": test_result(result, name)})
    report = {"pass": all(row["pass"] for row in results), "tests": len(results), "groups": len(GROUPS), "inventory_sha256": digest, "results": results}
    (directory / "report.json").write_bytes(canonical(report) + b"\n")
    return report
