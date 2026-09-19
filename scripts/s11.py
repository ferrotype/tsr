#!/usr/bin/env python3
"""Capture S11 subprocess transport contracts and pinned Go host observations."""
import argparse
import base64
import hashlib
import io
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

from s04 import go_environment, same_json_value, verified_upstream
from s04_common import strict_json_loads

ROOT = Path(__file__).resolve().parents[1]
REPORTS = ROOT / "target/s11-reports"


def command(args, *, cwd=ROOT, env=None):
    print("+ " + " ".join(map(str, args)), file=sys.stderr)
    result = subprocess.run(args, cwd=cwd, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=240, check=False)
    if result.stderr:
        sys.stderr.buffer.write(result.stderr)
    if result.returncode:
        sys.stderr.buffer.write(result.stdout)
        raise RuntimeError(f"command exited {result.returncode}: {args}")
    return result.stdout


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=True, sort_keys=True, indent=2) + "\n")


def inventory(values, label):
    if not isinstance(values, list) or not values or any(type(v) is not str or not v for v in values):
        raise ValueError(f"{label}: expected nonempty string list")
    if len(values) != len(set(values)):
        raise ValueError(f"{label}: duplicate ID")
    return values


def rust_binary():
    raw = command(["cargo", "build", "-p", "tsr_testhost", "--locked", "--message-format=json"])
    binaries = []
    for line in raw.splitlines():
        item = strict_json_loads(line)
        if item.get("reason") == "compiler-artifact" and item["target"]["name"] == "tsr_testhost" and item.get("executable"):
            binaries.append(Path(item["executable"]))
    if len(binaries) != 1:
        raise ValueError("Cargo did not report exactly one tsr_testhost executable")
    return binaries[0]


def oracle():
    upstream = verified_upstream()
    pin = strict_json_loads((ROOT / "data/upstream.json").read_bytes())["pin"]
    env = go_environment()
    REPORTS.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="oracle-", dir=REPORTS) as temporary:
        checkout = Path(temporary)
        raw = command(["git", "archive", pin, "tsc/go.mod", "tsc/go.sum", "tsc/internal"], cwd=upstream)
        try:
            with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
                archive.extractall(checkout, filter="data")
        except (tarfile.TarError, ValueError) as error:
            raise RuntimeError(f"cannot export pinned S11 oracle: {error}") from error
        for source, target in (
            ("callbackfs_test.go", "internal/api/s11_callbackfs_test.go"),
            ("mapper_test.go", "internal/testutil/contentmappertest/s11_mapper_test.go"),
        ):
            shutil.copyfile(ROOT / "tools/s11" / source, checkout / "tsc" / target)
        for kind, package, test, input_key, output_key in (
            ("fs", "./internal/api", "TestS11CallbackFS", "S11_INPUT", "S11_OUTPUT"),
            ("mapper", "./internal/testutil/contentmappertest", "TestS11Mapper", "S11_MAPPER_INPUT", "S11_MAPPER_OUTPUT"),
        ):
            destination = REPORTS / f"go-{kind}.json"
            destination.unlink(missing_ok=True)
            env.update({input_key: str(ROOT / f"data/s11/{kind}-fixtures.json"), output_key: str(destination)})
            # The upstream repo package intentionally resolves test paths from
            # runtime.Caller; retain that one package's real export path.
            repo_path = f"-gcflags=github.com/microsoft/TypeScript/tsc/internal/repo=-trimpath={checkout}/s11-unmatched"
            output = command(["go", "test", "-trimpath", repo_path, "-mod=readonly", package,
                              "-run", f"^{test}$", "-count=1", "-timeout=90s"], cwd=checkout / "tsc", env=env)
            (REPORTS / f"go-{kind}.stdout").write_bytes(output)
            strict_json_loads(destination.read_bytes())
    verified_upstream()


def validate_observations(fixtures, observations, label):
    expected = inventory([f["id"] for f in fixtures], label + " fixtures")
    actual = inventory([o["id"] for o in observations], label + " observations")
    if actual != expected:
        raise ValueError(f"{label}: omitted, extra or reordered oracle observations")
    return dict(zip(expected, observations))


def fs_rows(binary, fixtures, observations):
    from s11_contracts import Peer
    rows = []
    for fixture in fixtures:
        row = {"id": "fs/" + fixture["id"], "category": "go-filesystem", "pass": False}
        peer = Peer(binary, [])
        try:
            # Peer helpers below are deliberately implemented here using its wire
            # IO only, so the independent oracle remains the result authority.
            initialize_peer(peer, fixture)
            peer.send({"jsonrpc":"2.0", "id":2, "method":"test/fs", "params":{
                "operation":fixture["operation"], "path":fixture["path"]}})
            callbacks = []
            if fixture["enabled"]:
                begin = peer.read()
                callback = peer.read()
                expect_progress(begin, 2, callback["id"], "begin")
                if not same_json_value(callback, {"jsonrpc":"2.0","id":callback["id"],
                        "method":fixture["operation"],"params":fixture["path"]}):
                    raise ValueError("filesystem callback envelope mismatch")
                callbacks.append({"method":callback["method"], "params":callback["params"]})
                peer.send({"jsonrpc":"2.0", "id":callback["id"], "result":fixture["response"]})
                expect_progress(peer.read(), 2, callback["id"], "end")
            result = peer.read()
            expected = observations[fixture["id"]]
            actual = {"id":fixture["id"], "result":result.get("result"), "callbacks":callbacks}
            check_response(result, 2, {"result":expected["result"]})
            if not same_json_value(actual, expected):
                raise ValueError(f"FS mismatch: expected {expected!r}, actual {result!r}, callbacks {callbacks!r}")
            peer.send({"jsonrpc":"2.0", "id":3, "method":"test/shutdown", "params":{}})
            if peer.read() != {"jsonrpc":"2.0", "id":3, "result":None}:
                raise ValueError("shutdown response mismatch")
            peer.finish()
            row["pass"] = True
        except Exception as error:
            row["error"] = str(error)
        finally:
            peer.__exit__()
            row["transcript"] = peer.transcript
        rows.append(row)
    return rows


def check_response(actual, identity, observation):
    """Null is a value, never permission to omit a required response field."""
    if set(observation) == {"result"}:
        expected = {"jsonrpc":"2.0", "id":identity, "result":observation["result"]}
    elif set(observation) == {"error"}:
        expected = {"jsonrpc":"2.0", "id":identity,
                    "error":{"code":-32001,"message":"callback failed","data":observation["error"]}}
    else:
        raise ValueError("observation requires exactly one result/error")
    if not same_json_value(actual, expected):
        raise ValueError(f"response envelope mismatch: {actual!r} != {expected!r}")


def expect_progress(message, request, callback, phase):
    if type(callback) is not str or not callback.startswith("callback:") or not callback[9:].isdigit():
        raise ValueError("invalid callback identity")
    expected = {"jsonrpc":"2.0", "method":"testhost/progress",
                "params":{"id":request,"callback":callback,"phase":phase}}
    if not same_json_value(message, expected):
        raise ValueError(f"progress mismatch: {message!r}")


def initialize_peer(peer, fixture, plugins=None):
    params = {"version":2, "caseSensitive":fixture.get("caseSensitive", True),
              "base":fixture.get("base", {}), "symlinks":fixture.get("symlinks", {}),
              "callbacks":[fixture["operation"]] if fixture.get("enabled") else [],
              "options":{}, "plugins":plugins or []}
    peer.send({"jsonrpc":"2.0", "id":1, "method":"test/initialize", "params":params})
    result, signal = peer.read(), peer.read()
    state = {"version":2,"caseSensitive":params["caseSensitive"],"options":{},
             "plugins":[{**p,"state":"registered"} for p in params["plugins"]]}
    check_response(result, 1, {"result":state})
    if signal != {"jsonrpc":"2.0", "method":"testhost/initialized", "params":{"version":2}}:
        raise ValueError("initialization signal mismatch")


def mapper_rows(binary, fixtures, observations):
    from s11_contracts import Peer
    from s11_tunnel import open_stream, write_bytes, read_bytes, close_stream, CHUNK
    rows = []
    for fixture in fixtures:
        row = {"id":"mapper/" + fixture["id"], "category":"go-mapper", "pass":False}
        peer = Peer(binary, [])
        try:
            initialize_peer(peer, {}, [{"name":fixture["mapper"], "options":{}}])
            peer.next_id = 2
            observation = observations[fixture["id"]]
            validate_mapper_bytes(fixture, observation)
            stream = open_stream(peer, fixture["mapper"])
            measured = {}
            for key, transfer in (("server_bytes", write_bytes), ("plugin_bytes", read_bytes)):
                data = base64.b64decode(observation[key], validate=True)
                # First fragment cuts the native Content-Length header; subsequent
                # chunks can cross frame boundaries. Compare concatenated bytes.
                chunks = [data[:3]] + [data[i:i+CHUNK] for i in range(3, len(data), CHUNK)]
                actual = b"".join(transfer(peer, stream, chunk) for chunk in chunks if chunk)
                if actual != data:
                    raise ValueError("mapper tunnel changed " + key)
                measured[key] = {"bytes":len(actual), "sha256":hashlib.sha256(actual).hexdigest()}
            row["streams"] = measured
            close_stream(peer, stream)
            identity = peer.request("test/shutdown", {})
            peer.result(identity, None)
            peer.finish()
            row["pass"] = True
        except Exception as error:
            row["error"] = str(error)
        finally:
            peer.__exit__()
            row["transcript"] = peer.transcript
        rows.append(row)
    return rows


def mapper_messages(encoded):
    """Validate complete native frames without serializing their bytes again."""
    data = base64.b64decode(encoded, validate=True)
    messages = []
    while data:
        header, separator, rest = data.partition(b"\r\n\r\n")
        if not separator or not header.startswith(b"Content-Length: ") or not header[16:].isdigit():
            raise ValueError("invalid native mapper framing")
        length = int(header[16:])
        if length <= 0 or length > len(rest):
            raise ValueError("truncated native mapper frame")
        messages.append(strict_json_loads(rest[:length]))
        data = rest[length:]
    return messages


def validate_mapper_bytes(fixture, observation):
    requests = mapper_messages(observation["server_bytes"])
    responses = mapper_messages(observation["plugin_bytes"])
    if len(requests) != len(fixture["requests"]) or len(responses) != len(requests) or len(observation["responses"]) != len(requests):
        raise ValueError("native byte capture lost a mapper exchange")
    for index, (request, response, expected, outcome) in enumerate(zip(requests, responses, fixture["requests"], observation["responses"])):
        identity = f"{fixture['id']}:{index}"
        if not same_json_value(request, {"jsonrpc":"2.0", "id":identity, **expected}):
            raise ValueError("native bytes differ from frozen mapper requests")
        if not same_json_value(response, {"jsonrpc":"2.0", "id":identity, **outcome}):
            raise ValueError("native byte capture differs from parsed mapper outcome")


def hook_rows():
    """Execute the delayed internal hook through the real Session API."""
    cases = strict_json_loads((ROOT / "data/s11/hook-cases.json").read_bytes())
    inventory([case["id"] for case in cases], "internal cases")
    tests = inventory([case["test"] for case in cases], "internal test names")
    output = command(["cargo", "test", "-p", "tsr_testhost", "--locked", "--lib", "session::tests::", "--", "--color=never"])
    (REPORTS / "internal-hook.stdout").write_bytes(output)
    outcomes = re.findall(r"^test (session::tests::\w+) \.\.\. (\w+)$", output.decode(), re.MULTILINE)
    if len(outcomes) != len(tests) or set(name for name, _ in outcomes) != set(tests):
        raise ValueError("internal hook test inventory missing, extra or duplicated")
    observed = dict(outcomes)
    return [{"id":case["id"], "category":"internal-contract", "pass":observed[case["test"]] == "ok",
             "test":case["test"], "output_sha256":hashlib.sha256(output).hexdigest()} for case in cases]


def summarize(rows, cases):
    actual = inventory([r["id"] for r in rows], "measured cases")
    if actual != inventory(cases, "frozen cases"):
        raise ValueError("measured S11 inventory differs from frozen ordered cases")
    if any(type(r.get("pass")) is not bool for r in rows):
        raise ValueError("case outcomes must be measured booleans")
    controls = [r for r in rows if r["category"] in ("controls", "go-mapper", "internal-contract")]
    if not controls:
        raise ValueError("empty controls inventory")
    return {"metrics":{"controls":all(r["pass"] for r in controls)},
            "tests":{r["id"]:"pass" if r["pass"] else "fail" for r in rows}}


def capture():
    from s11_contracts import run
    REPORTS.mkdir(parents=True, exist_ok=True)
    oracle()
    binary = rust_binary()
    fixtures = {kind:strict_json_loads((ROOT / f"data/s11/{kind}-fixtures.json").read_bytes()) for kind in ("fs", "mapper")}
    observations = {kind:validate_observations(fixtures[kind], strict_json_loads((REPORTS / f"go-{kind}.json").read_bytes()), kind) for kind in fixtures}
    rows = fs_rows(binary, fixtures["fs"], observations["fs"]) + mapper_rows(binary, fixtures["mapper"], observations["mapper"]) + run(binary) + hook_rows()
    cases = strict_json_loads((ROOT / "data/s11/cases.json").read_bytes())
    summary = summarize(rows, cases)
    report = {"scope":"S11 transport only; no language-service or fourslash semantic assertions",
              "binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),
              "oracle":observations, "rows":rows, "summary":summary,
              "internal_hook_output":(REPORTS / "internal-hook.stdout").read_text()}
    write(REPORTS / "report.json", report)
    print(json.dumps(report, sort_keys=True), file=sys.stderr)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("capture", "oracle"), nargs="?", default="capture")
    args = parser.parse_args()
    if args.operation == "oracle":
        oracle()
    else:
        print(json.dumps(capture(), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"S11 producer failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
