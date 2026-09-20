"""Separate, fully executed AST utility/accessor evidence for the S06 gate."""
from pathlib import Path
import re
import subprocess
import sys

from s04_common import strict_json_loads
from s06_protocol import canonical, exact_keys, sha256

ROOT = Path(__file__).resolve().parents[1]
GROUPS = {"front": ("utilities_front", ""),
          "middle": ("lib", "utilities_middle::tests::"),
          "tail": ("utilities_tail", ""),
          "accessors": ("node_accessors", "")}


def load_manifest(root):
    raw = (root / "data/s06/utility-tests.json").read_bytes()
    manifest = strict_json_loads(raw)
    exact_keys(manifest, ("version", "pin", "groups"), "utility evidence manifest")
    pin = strict_json_loads((root / "data/upstream.json").read_bytes())["pin"]
    if type(manifest["version"]) is not int or manifest["version"] != 1 or manifest["pin"] != pin:
        raise ValueError("utility evidence version or upstream pin changed")
    if type(manifest["groups"]) is not list or len(manifest["groups"]) != len(GROUPS):
        raise ValueError("all four utility/accessor families are required")
    names = []
    for group in manifest["groups"]:
        exact_keys(group, ("name", "checker", "target", "prefix", "tests"), "utility evidence group")
        if type(group["name"]) is not str:
            raise ValueError("utility family name must be a string")
        names.append(group["name"])
        if group["name"] not in GROUPS or (group["target"], group["prefix"]) != GROUPS[group["name"]]:
            raise ValueError("unknown utility family or Rust test target")
        path = group["checker"]
        if type(path) is not str or not path.endswith(".py") or Path(path).is_absolute() or ".." in Path(path).parts:
            raise ValueError("utility checker must be a repository Python path")
        prefix = group["prefix"]
        tests = group["tests"]
        if type(prefix) is not str or type(tests) is not list or not tests or any(
            type(name) is not str or re.fullmatch(r"[a-z0-9_:]+", name) is None or not name.startswith(prefix)
            for name in tests
        ) or tests != sorted(set(tests)):
            raise ValueError("utility tests must be nonempty, sorted, unique exact names")
        if group["target"] == "lib" and not prefix:
            raise ValueError("utility library groups need an explicit test namespace")
    if set(names) != set(GROUPS) or len(set(names)) != len(names):
        raise ValueError("duplicate or missing utility families")
    return manifest, sha256(raw)


def invoke(args, root, prefix, *, timeout=300):
    print("+ " + " ".join(map(str, args)), file=sys.stderr)
    try:
        result = subprocess.run(args, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        prefix.with_suffix(".stdout").write_bytes(error.stdout or b"")
        prefix.with_suffix(".stderr").write_bytes(error.stderr or b"")
        prefix.with_suffix(".command.json").write_bytes(canonical({
            "command": list(map(str, args)), "returncode": None,
            "timed_out": True, "timeout_seconds": timeout,
        }) + b"\n")
        print(f"Command exceeded {timeout}s; partial output retained at {prefix}", file=sys.stderr)
        raise
    prefix.with_suffix(".stdout").write_bytes(result.stdout)
    prefix.with_suffix(".stderr").write_bytes(result.stderr)
    prefix.with_suffix(".command.json").write_bytes(canonical({"command": list(map(str,args)), "returncode": result.returncode})+b"\n")
    return result


def setup(args, root, prefix, *, timeout=300):
    result = invoke(args, root, prefix, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"AST utility setup/check failed: {args}; output retained at {prefix}")
    return result.stdout


def test_inventory(output, group):
    names = re.findall(r"^(\S+): test$", output.decode("utf-8"), re.MULTILINE)
    actual = [name for name in names if name.startswith(group["prefix"])]
    if sorted(actual) != group["tests"] or len(actual) != len(set(actual)):
        raise ValueError(f"AST utility named test inventory drift: {group['name']}")


def test_result(result, name):
    """A completed failed assertion is measured false; missing execution is invalid."""
    output = result.stdout.decode("utf-8")
    rows = re.findall(r"^test (\S+) \.\.\. (\S+)$", output, re.MULTILINE)
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;.*$", output, re.MULTILINE)
    running = re.findall(r"^running (\d+) tests?$", output, re.MULTILINE)
    if len(rows) != 1 or rows[0][0] != name or rows[0][1] not in ("ok", "FAILED") or len(summaries) != 1 or running != ["1"]:
        raise ValueError(f"AST utility test did not complete its exact observation: {name}")
    passed = rows[0][1] == "ok"
    expected = ("ok", "1", "0", "0", "0") if passed else ("FAILED", "0", "1", "0", "0")
    if summaries[0][:5] != expected or result.returncode != (0 if passed else 101):
        raise ValueError(f"inconsistent AST utility test outcome: {name}")
    return passed


def measure(root=ROOT, report_dir=None):
    root = Path(root)
    manifest, digest = load_manifest(root)
    directory = Path(report_dir) if report_dir is not None else root / "target/s06-reports/utilities"
    directory.mkdir(parents=True, exist_ok=True)
    for group in manifest["groups"]:
        setup([sys.executable, group["checker"], "--check"], root, directory / (group["name"]+"-oracle"))
    targets = sorted({group["target"] for group in manifest["groups"]})
    args = ["cargo", "test", "--package", "tsr_ast", "--locked", "--release", "--no-run", "--message-format=json"]
    for target in targets:
        args.extend(["--lib"] if target == "lib" else ["--test", target])
    output = setup(args, root, directory / "rust-build")
    binaries = {}
    for line in output.splitlines():
        artifact = strict_json_loads(line)
        if artifact.get("reason") != "compiler-artifact" or not artifact.get("executable"):
            continue
        target = artifact["target"]
        name = "lib" if target["kind"] == ["lib"] and target["name"] == "tsr_ast" else target["name"]
        if name not in targets or name in binaries or not artifact["profile"]["test"]:
            raise ValueError("unexpected or duplicate AST utility test executable")
        binaries[name] = artifact["executable"]
    if set(binaries) != set(targets):
        raise ValueError("Cargo omitted an AST utility test executable")
    inventories = {target: setup([binary, "--list", "--format=terse"], root, directory / (target+"-inventory")) for target,binary in binaries.items()}
    details = []
    for group in manifest["groups"]:
        test_inventory(inventories[group["target"]], group)
        for index,name in enumerate(group["tests"]):
            prefix = directory / f"{group['name']}-{index:02d}"
            result = invoke([binaries[group["target"]], name, "--exact", "--test-threads=1", "--color=never"], root, prefix)
            passed = test_result(result, name)
            details.append({"group":group["name"], "test":name, "pass":passed, "returncode":result.returncode, "artifact_prefix":str(prefix)})
            print(f"AST utility {name}: {'pass' if passed else 'fail'}", file=sys.stderr)
    summary = {"metric":all(item["pass"] for item in details), "tests":len(details), "groups":len(GROUPS), "manifest_sha256":digest, "cases":details}
    (directory / "results.json").write_bytes(canonical(summary)+b"\n")
    return summary
