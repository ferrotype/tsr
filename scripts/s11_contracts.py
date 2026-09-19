"""Process-level S11 wire contracts; these are not Go semantic observations."""

from __future__ import annotations

import json
import copy
import hashlib
import os
from pathlib import Path
import selectors
import subprocess
import time

from s04 import same_json_value
from s04_common import strict_json_loads


TIMEOUT = 10.0
MAX_BODY = 8 * 1024 * 1024
MAX_HEADER = 8192
MAX_STDERR = 1024 * 1024
ROOT = Path(__file__).resolve().parents[1]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def equal(actual, expected):
    if not same_json_value(expected, actual):
        raise AssertionError(f"expected {repr(expected)[:2000]}, received {repr(actual)[:2000]}")


def json_bytes(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False,
                      separators=(",", ":")).encode("utf-8")


def encode(value):
    body = json_bytes(value)
    return b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body


class Peer:
    """Bounded framed child I/O, including draining stderr while stdout waits."""

    def __init__(self, binary, transcript):
        self.transcript = transcript
        self.process = subprocess.Popen([str(binary), "--stdio"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        bufsize=0)
        self.selector = selectors.DefaultSelector()
        self.output = bytearray()
        self.errors = bytearray()
        self.last_body = b""
        self.next_id = 1
        self.closed_stdout = False
        self.finished = False
        for stream, name in ((self.process.stdout, "stdout"),
                             (self.process.stderr, "stderr")):
            os.set_blocking(stream.fileno(), False)
            self.selector.register(stream, selectors.EVENT_READ, name)
        os.set_blocking(self.process.stdin.fileno(), False)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=TIMEOUT)
        if not self.finished:
            # Once the child is reaped, preserve any final diagnostic bytes
            # without another wait or allowing an error-path output flood.
            for stream, target, limit in ((self.process.stdout, self.output, MAX_BODY + MAX_HEADER),
                                          (self.process.stderr, self.errors, MAX_STDERR)):
                while len(target) < limit:
                    try:
                        data = os.read(stream.fileno(), min(65536, limit - len(target)))
                    except BlockingIOError:
                        break
                    if not data:
                        break
                    target.extend(data)
            self.transcript.append({"cleanup_exit": self.process.returncode,
                                    "stderr": self.errors.decode("utf-8", "replace"),
                                    "stdout_unconsumed_hex": self.output.hex()})
        self.selector.close()
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            stream.close()

    def _pump(self, deadline):
        remaining = deadline - time.monotonic()
        require(remaining > 0, "child I/O deadline exceeded")
        ready = self.selector.select(remaining)
        require(ready, "child I/O deadline exceeded")
        writable = False
        for key, events in ready:
            if key.data == "stdin":
                writable = bool(events & selectors.EVENT_WRITE)
                continue
            data = os.read(key.fileobj.fileno(), 65536)
            if not data:
                self.selector.unregister(key.fileobj)
                if key.data == "stdout":
                    self.closed_stdout = True
                continue
            target = self.output if key.data == "stdout" else self.errors
            target.extend(data)
            limit = MAX_BODY + MAX_HEADER if key.data == "stdout" else MAX_STDERR
            require(len(target) <= limit, f"unconsumed {key.data} exceeds byte limit")
        return writable

    def wire(self, wire, *, record=True):
        if record:
            if len(wire) <= 65536:
                self.transcript.append({"send_bytes_hex": wire.hex()})
            else:
                self.transcript.append({"send_wire_bytes": len(wire),
                                        "sha256": hashlib.sha256(wire).hexdigest()})
        deadline = time.monotonic() + TIMEOUT
        self.selector.register(self.process.stdin, selectors.EVENT_WRITE, "stdin")
        try:
            remaining = memoryview(wire)
            while remaining:
                if self._pump(deadline):
                    try:
                        count = os.write(self.process.stdin.fileno(), remaining)
                    except BlockingIOError:
                        continue
                    require(count > 0, "child stdin made no progress")
                    remaining = remaining[count:]
        finally:
            self.selector.unregister(self.process.stdin)

    def record_message(self, direction, message, body=None):
        body = json_bytes(message) if body is None else body
        if len(body) <= 65536:
            # A caller later replacing its option dictionary must not rewrite
            # the already-observed initialization transcript.
            self.transcript.append({direction: copy.deepcopy(message)})
        else:
            self.transcript.append({direction + "_large_frame": {
                "body_bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(),
                "id": message.get("id"), "method": message.get("method")}})

    def send(self, message):
        self.record_message("send", message)
        self.wire(encode(message), record=False)

    def request(self, method, params, *, identity=None):
        if identity is None:
            identity = self.next_id
            self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": identity, "method": method, "params": params})
        return identity

    def notify(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def reply(self, identity, result=None, *, error=None):
        message = {"jsonrpc": "2.0", "id": identity}
        message["result" if error is None else "error"] = result if error is None else error
        self.send(message)

    def read(self):
        deadline = time.monotonic() + TIMEOUT
        while b"\r\n\r\n" not in self.output:
            require(len(self.output) <= MAX_HEADER, "stdout is not bounded framing")
            require(not self.closed_stdout,
                    f"EOF before expected response; stderr={bytes(self.errors)!r}")
            self._pump(deadline)
        boundary = self.output.index(b"\r\n\r\n") + 4
        require(boundary <= MAX_HEADER, "stdout header exceeds limit")
        headers = bytes(self.output[:boundary - 4]).split(b"\r\n")
        # The endpoint writer has one exact header: extra stdout cannot hide as metadata.
        require(len(headers) == 1 and headers[0].startswith(b"Content-Length: "),
                f"unexpected stdout headers: {headers!r}")
        decimal = headers[0][len(b"Content-Length: "):]
        require(decimal.isdigit(), "nondecimal response Content-Length")
        length = int(decimal)
        require(0 < length <= MAX_BODY, "response body length out of bounds")
        while len(self.output) < boundary + length:
            require(not self.closed_stdout, "EOF in response body")
            self._pump(deadline)
        body = bytes(self.output[boundary:boundary + length])
        del self.output[:boundary + length]
        self.last_body = body
        value = strict_json_loads(body.decode("utf-8"))
        require(type(value) is dict and value.get("jsonrpc") == "2.0",
                "stdout body is not a JSON-RPC object")
        self.record_message("receive", value, body)
        return value

    def expect(self, message):
        equal(self.read(), message)

    def result(self, identity, result):
        self.expect({"jsonrpc": "2.0", "id": identity, "result": result})

    def error(self, identity, code, *, remote=None):
        message = self.read()
        equal(set(message), {"jsonrpc", "id", "error"})
        equal(message["id"], identity)
        error = message["error"]
        require(type(error) is dict and type(error.get("message")) is str,
                "application error has no message")
        equal(error["code"], code)
        if remote is not None:
            equal(error, {"code": code, "message": "callback failed", "data": remote})

    def notification(self, method, params):
        self.expect({"jsonrpc": "2.0", "method": method, "params": params})

    def begin(self, identity, method, params):
        message = self.read()
        equal(set(message), {"jsonrpc", "method", "params"})
        equal(message["method"], "testhost/progress")
        progress = message["params"]
        callback = progress.get("callback")
        require(type(callback) is str and callback.startswith("callback:"),
                "callback identity is not in the callback namespace")
        equal(progress, {"id": identity, "callback": callback, "phase": "begin"})
        self.expect({"jsonrpc": "2.0", "id": callback, "method": method, "params": params})
        return callback

    def end(self, identity, callback):
        self.notification("testhost/progress",
                          {"id": identity, "callback": callback, "phase": "end"})

    def complete(self, identity, callback, result, expected):
        self.reply(callback, result)
        self.end(identity, callback)
        self.result(identity, expected)

    def finish(self, *, failure=False, close_input=True):
        if close_input:
            self.process.stdin.close()
        deadline = time.monotonic() + TIMEOUT
        while self.selector.get_map():
            self._pump(deadline)
        code = self.process.wait(timeout=max(0.001, deadline - time.monotonic()))
        require(not self.output, f"unexpected trailing stdout: {bytes(self.output)!r}")
        if failure:
            equal(code, 1)
            require(bytes(self.errors).startswith(b"test-host transport failed: "),
                    "protocol failure was not the endpoint's diagnosed rejection")
        else:
            equal(code, 0)
            require(not self.errors, f"successful session wrote stderr: {bytes(self.errors)!r}")
        self.transcript.append({"exit": code, "stderr": self.errors.decode("utf-8", "replace")})
        self.finished = True


def configuration(**changes):
    value = {"version": 1, "caseSensitive": False, "base": {}, "symlinks": {},
             "callbacks": [], "options": {}, "plugins": []}
    value.update(changes)
    return value


def state(config, states=None):
    states = states or {}
    return {"version": 1, "caseSensitive": config["caseSensitive"],
            "options": config["options"],
            "plugins": [dict(plugin, state=states.get(plugin["name"], "registered"))
                        for plugin in config["plugins"]]}


def initialize(peer, config=None):
    config = config or configuration()
    identity = peer.request("test/initialize", config)
    callback = peer.begin(identity, "testhost/configuration", {"options": config["options"]})
    peer.complete(identity, callback, {"ready": True}, state(config))
    peer.notification("testhost/initialized", {"version": 1})
    return config


def check_state(peer, config, states=None):
    peer.result(peer.request("test/state", {}), state(config, states))


def start_fs(peer, operation="readFile", path="/file.ts"):
    identity = peer.request("test/fs", {"operation": operation, "path": path})
    callback = peer.begin(identity, operation, path)
    return identity, callback


def plugin_call(peer, method, params=None, name="mapper", options=None):
    params = {} if params is None else params
    identity = peer.request("test/plugin", {"name": name, "method": method, "params": params})
    callback = peer.begin(identity, "testhost/plugin",
                          {"name": name, "method": method, "params": params,
                           "options": {} if options is None else options})
    return identity, callback


def cancel(peer, identity, callback, *, plugin=None):
    peer.notify("$/cancelRequest", {"id": identity})
    peer.notification("$/cancelRequest", {"id": callback})
    if plugin is not None:
        peer.notification("testhost/retirePlugin", {"name": plugin})
    peer.end(identity, callback)
    peer.error(identity, -32800)


def initialization_barrier(peer, _):
    config = configuration(options={"strict": True, "label": "é😀"})
    identity = peer.request("test/initialize", config)
    callback = peer.begin(identity, "testhost/configuration", {"options": config["options"]})
    peer.error(peer.request("test/state", {}), -32002)
    peer.error(peer.request("test/fs", {"operation": "readFile", "path": "/x"}), -32002)
    peer.complete(identity, callback, {"ready": True}, state(config))
    peer.notification("testhost/initialized", {"version": 1})
    check_state(peer, config)


def configuration_failure_retry(peer, _):
    config = configuration()
    identity = peer.request("test/initialize", config)
    callback = peer.begin(identity, "testhost/configuration", {"options": {}})
    remote = {"code": 701, "message": "configuration unavailable", "data": {"attempt": 1}}
    peer.reply(callback, error=remote)
    peer.end(identity, callback)
    peer.error(identity, -32001, remote=remote)
    peer.error(peer.request("test/state", {}), -32002)
    initialize(peer, config)


def configuration_malformed_ready(peer, _):
    identity = peer.request("test/initialize", configuration())
    callback = peer.begin(identity, "testhost/configuration", {"options": {}})
    peer.reply(callback, {"ready": True, "extra": 1})
    peer.finish(failure=True)
    require(b"invalid configuration acknowledgment" in peer.errors,
            "ambiguous configuration acknowledgment must retire the session")


def options_roundtrip(peer, _):
    config = initialize(peer, configuration(options={"strict": False}))
    replacement = {"strict": True, "target": "ESNext", "nested": {"values": [None, "😀"]}}
    identity = peer.request("test/setOptions", {"options": replacement})
    callback = peer.begin(identity, "testhost/configuration", {"options": replacement})
    check_state(peer, config)
    peer.complete(identity, callback, {"ready": True}, {"options": replacement})
    config["options"] = replacement
    check_state(peer, config)
    identity = peer.request("test/setOptions", {"options": {"strict": False}})
    callback = peer.begin(identity, "testhost/configuration", {"options": {"strict": False}})
    remote = {"code": 702, "message": "rejected options"}
    peer.reply(callback, error=remote)
    peer.end(identity, callback)
    peer.error(identity, -32001, remote=remote)
    check_state(peer, config)


def lifecycle_validation(peer, _):
    peer.error(peer.request("test/setOptions", {"options": {}}), -32002)
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": {}}),
               -32002)
    peer.error(peer.request("test/initialize", configuration(version=2)), -32602)
    peer.error(peer.request("test/initialize", configuration(options=[])), -32602)
    peer.error(peer.request("test/initialize", configuration(callbacks=["readFile", "readFile"])),
               -32602)
    initialize(peer)
    peer.error(peer.request("test/initialize", configuration()), -32002)
    peer.error(peer.request("test/unknown", {}), -32601)


def all_callbacks(peer, _):
    operations = [("readFile", {"content": ""}), ("fileExists", True),
                  ("directoryExists", False),
                  ("getAccessibleEntries", {"files": ["b.ts", "a.ts", "b.ts"],
                                            "directories": ["z", "a"]}),
                  ("realpath", "/Exact/Path.ts")]
    initialize(peer, configuration(callbacks=[op for op, _ in operations]))
    for operation, result in operations:
        identity, callback = start_fs(peer, operation, "/MiXeD/😀.ts")
        peer.complete(identity, callback, result, result)


def progress_outstanding(peer, _):
    initialize(peer, configuration(callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    value = {"completed": 1, "detail": ["loading", None]}
    peer.notify("test/callbackProgress", {"callback": callback, "value": value})
    peer.notification("testhost/progress",
                      {"id": identity, "callback": callback, "phase": "report", "value": value})
    peer.result(peer.request("test/state", {}), state(configuration(callbacks=["readFile"])))
    peer.complete(identity, callback, {"content": "done"}, {"content": "done"})


def out_of_order(peer, _):
    initialize(peer, configuration(callbacks=["readFile"]))
    first, first_callback = start_fs(peer, path="/first")
    second, second_callback = start_fs(peer, path="/second")
    require(first_callback != second_callback, "callback identity reused")
    peer.complete(second, second_callback, {"content": "second"}, {"content": "second"})
    peer.complete(first, first_callback, {"content": "first"}, {"content": "first"})


def cancellation_late_response(peer, _):
    config = initialize(peer, configuration(callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    cancel(peer, identity, callback)
    peer.notify("$/cancelRequest", {"id": identity})
    peer.notify("test/callbackProgress", {"callback": callback, "value": "too late"})
    peer.reply(callback, {"content": "late"})
    check_state(peer, config)
    identity, callback = start_fs(peer)
    peer.complete(identity, callback, {"content": "next"}, {"content": "next"})


def initialization_cancel_retry(peer, _):
    identity = peer.request("test/initialize", configuration())
    callback = peer.begin(identity, "testhost/configuration", {"options": {}})
    cancel(peer, identity, callback)
    peer.reply(callback, error={"code": -32800, "message": "configuration not applied"})
    peer.error(peer.request("test/state", {}), -32002)
    initialize(peer)


def remote_error_no_fallback(peer, _):
    initialize(peer, configuration(base={"/file.ts": "base must not leak"}, callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    remote = {"code": 77, "message": "remote read failed", "data": {"retry": False}}
    peer.reply(callback, error=remote)
    peer.end(identity, callback)
    peer.error(identity, -32001, remote=remote)


def malformed_callback_result(peer, _):
    initialize(peer, configuration(callbacks=["readFile", "fileExists", "getAccessibleEntries"]))
    for operation, malformed in [("readFile", {"content": 42}), ("fileExists", "false"),
                                 ("getAccessibleEntries", {"files": [1], "directories": []})]:
        identity, callback = start_fs(peer, operation)
        peer.reply(callback, malformed)
        peer.end(identity, callback)
        peer.error(identity, -32001)


def options_busy(peer, _):
    initialize(peer, configuration(callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    peer.error(peer.request("test/setOptions", {"options": {"strict": True}}), -32002)
    peer.complete(identity, callback, {"content": "done"}, {"content": "done"})


def options_blocks_new_work(peer, _):
    config = initialize(peer, configuration(callbacks=["readFile"], options={"strict": False},
                                            plugins=[{"name": "mapper", "options": {}}]))
    identity = peer.request("test/setOptions", {"options": {"strict": True}})
    callback = peer.begin(identity, "testhost/configuration", {"options": {"strict": True}})
    peer.error(peer.request("test/fs", {"operation": "readFile", "path": "/file.ts"}), -32002)
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": {}}),
               -32002)
    check_state(peer, config)
    peer.complete(identity, callback, {"ready": True}, {"options": {"strict": True}})
    config["options"] = {"strict": True}
    check_state(peer, config)
    identity, callback = start_fs(peer)
    peer.complete(identity, callback, {"content": "ok"}, {"content": "ok"})
    identity, callback = plugin_call(peer, "spawn")
    peer.complete(identity, callback, None, None)


def canceled_options_barrier(peer, _):
    config = initialize(peer, configuration(callbacks=["readFile"], options={"strict": False},
                                            plugins=[{"name": "mapper", "options": {}}]))
    identity = peer.request("test/setOptions", {"options": {"strict": True}})
    callback = peer.begin(identity, "testhost/configuration", {"options": {"strict": True}})
    cancel(peer, identity, callback)
    check_state(peer, config)
    peer.error(peer.request("test/fs", {"operation": "readFile", "path": "/file.ts"}), -32002)
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": {}}),
               -32002)
    peer.error(peer.request("test/setOptions", {"options": {}}), -32002)
    peer.reply(callback, error={"code": -32800, "message": "configuration not applied"})
    check_state(peer, config)
    identity, callback = start_fs(peer)
    peer.complete(identity, callback, {"content": "after acknowledgment"},
                  {"content": "after acknowledgment"})
    identity, callback = plugin_call(peer, "spawn")
    peer.complete(identity, callback, None, None)


def canceled_initialization_barrier(peer, _):
    identity = peer.request("test/initialize", configuration(options={"attempt": 1}))
    callback = peer.begin(identity, "testhost/configuration", {"options": {"attempt": 1}})
    cancel(peer, identity, callback)
    peer.error(peer.request("test/initialize", configuration(options={"attempt": 2})), -32002)
    peer.reply(callback, error={"code": -32800, "message": "configuration not applied"})
    peer.error(peer.request("test/state", {}), -32002)
    initialize(peer, configuration(options={"attempt": 2}))


def options_reserve_plugin_state_capacity(peer, _):
    config = initialize(peer, configuration(plugins=[{"name": "mapper", "options": {}}]))
    for method, result in [("spawn", None), ("initialize", {})]:
        identity, callback = plugin_call(peer, method)
        peer.complete(identity, callback, result, result)
    peer.next_id = 9007199254740970
    prospective = copy.deepcopy(config)
    prospective["options"] = {"padding": ""}
    envelope = {"jsonrpc": "2.0", "id": 9007199254740991,
                "result": state(prospective, {"mapper": "ready"})}
    prospective["options"]["padding"] = "x" * (MAX_BODY - 2 - len(json_bytes(envelope)))
    envelope["result"] = state(prospective, {"mapper": "ready"})
    equal(len(json_bytes(envelope)), MAX_BODY - 2)
    # A later dispose changes "ready" to the longer "registered". Accepting
    # these options would strand test/state after a successful lifecycle call.
    envelope["result"] = state(prospective)
    require(len(json_bytes(envelope)) > MAX_BODY, "fixture must overflow the registered state")
    identity = peer.request("test/setOptions", {"options": prospective["options"]})
    # Reject before any configuration callback or progress notification escapes.
    peer.error(identity, -32602)
    check_state(peer, config, {"mapper": "ready"})
    identity, callback = plugin_call(peer, "dispose")
    peer.complete(identity, callback, None, None)
    check_state(peer, config)


def conflicting_base_paths(peer, _):
    for config in [
            configuration(base={"/src/../same.ts": "first", "/same.ts": "second"}),
            configuration(base={"/Same.ts": "first", "/same.ts": "second"}),
            configuration(base={"/src": "file", "/src/child.ts": "child"}),
            configuration(base={"/Same.ts": "file"}, symlinks={"/same.ts": "/other.ts"})]:
        peer.error(peer.request("test/initialize", config), -32602)
        peer.error(peer.request("test/state", {}), -32002)
    initialize(peer)


def invalid_query_paths(peer, _):
    operations = ["readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath"]
    initialize(peer, configuration(callbacks=operations))
    for path in ("relative/file.ts", "/nul\x00file.ts"):
        for operation in operations:
            peer.error(peer.request("test/fs", {"operation": operation, "path": path}), -32602)


def oversized_outgoing_callback(peer, _):
    options = {"padding": "a" * (MAX_BODY // 2 + 128)}
    config = initialize(peer, configuration(plugins=[{"name": "mapper", "options": options}]))
    params = {"padding": "b" * (MAX_BODY // 2 + 128)}
    # Both client messages are valid frames. Their merged reverse request is
    # too large, so neither a progress begin nor a reservation may escape.
    identity = peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": params})
    peer.error(identity, -32602)
    check_state(peer, config)
    identity, callback = plugin_call(peer, "spawn", options=options)
    peer.complete(identity, callback, None, None)
    check_state(peer, config, {"mapper": "spawned"})


def oversized_callback_error_wrapper(peer, _):
    config = initialize(peer, configuration(callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    remote = {"code": 77, "message": "remote error", "data": ""}
    envelope = {"jsonrpc": "2.0", "id": callback, "error": remote}
    remote["data"] = "a" * (MAX_BODY - 1 - len(json_bytes(envelope)))
    equal(len(json_bytes(envelope)), MAX_BODY - 1)
    peer.send(envelope)
    peer.end(identity, callback)
    response = peer.read()
    equal(set(response), {"jsonrpc", "id", "error"})
    equal(response["id"], identity)
    equal(response["error"]["code"], -32001)
    require(type(response["error"].get("message")) is str, "bounded error omitted diagnostic")
    require("data" not in response["error"], "oversized remote data retained in bounded error")
    check_state(peer, config)


def oversized_callback_result_wrapper(peer, _):
    config = initialize(peer, configuration(plugins=[{"name": "mapper", "options": {}}]))
    for method in ("spawn", "initialize"):
        identity, callback = plugin_call(peer, method)
        result = None if method == "spawn" else {"positionEncoding": "utf-8", "diagnosticSource": "test"}
        peer.complete(identity, callback, result, result)
    # A legal 16-digit client ID is longer than the callback string ID, making
    # a near-limit raw result too large in its final client response envelope.
    peer.next_id = 9007199254740980
    identity, callback = plugin_call(peer, "transform")
    result = {"padding": ""}
    envelope = {"jsonrpc": "2.0", "id": callback, "result": result}
    result["padding"] = "x" * (MAX_BODY - 1 - len(json_bytes(envelope)))
    equal(len(json_bytes(envelope)), MAX_BODY - 1)
    require(len(json_bytes({"jsonrpc": "2.0", "id": identity, "result": result})) > MAX_BODY,
            "fixture does not overflow the client envelope")
    peer.send(envelope)
    peer.end(identity, callback)
    peer.error(identity, -32001)
    check_state(peer, config, {"mapper": "ready"})


def pending_limit(peer, _):
    config = initialize(peer, configuration(callbacks=["readFile"]))
    pending = [start_fs(peer, path=f"/{index}") for index in range(64)]
    peer.error(peer.request("test/fs", {"operation": "readFile", "path": "/overflow"}), -32002)
    first, callback = pending.pop(0)
    cancel(peer, first, callback)
    peer.error(peer.request("test/fs", {"operation": "readFile", "path": "/still-full"}), -32002)
    peer.reply(callback, {"content": "late"})
    replacement = start_fs(peer, path="/replacement")
    for identity, callback in pending + [replacement]:
        peer.complete(identity, callback, {"content": "ok"}, {"content": "ok"})
    check_state(peer, config)


def plugin_lifecycle(peer, _):
    options = {"extensions": [".vue"], "strict": True}
    config = initialize(peer, configuration(plugins=[{"name": "mapper", "options": options}]))
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "transform", "params": {}}),
               -32002)
    for method, params, result, expected_state in [
            ("spawn", {}, None, "spawned"),
            ("initialize", {"locale": "en"}, {"extensions": [".vue"]}, "ready"),
            ("openProject", {"projectId": "p"}, {"accepted": True}, "ready"),
            ("closeProject", {"projectId": "p"}, None, "ready"),
            ("dispose", {}, None, "registered")]:
        identity, callback = plugin_call(peer, method, params, options=options)
        peer.complete(identity, callback, result, result)
        check_state(peer, config, {"mapper": expected_state})


def mapper_payload(peer, observations):
    options = {"mode": "transport-only"}
    initialize(peer, configuration(plugins=[{"name": "mapper", "options": options}]))
    for method in ("spawn", "initialize"):
        identity, callback = plugin_call(peer, method, options=options)
        result = None if method == "spawn" else {"positionEncoding": "utf-8", "diagnosticSource": "test"}
        peer.complete(identity, callback, result, result)
    # The raw field round-trip is a transport contract. The producer compares
    # independently captured Go mapper observations separately.
    sample = (observations or {}).get("transport_fixture")
    if sample is None:
        sample = {"params": {"fileName": "/Component.vue", "text": "😀<script>x</script>"},
                  "result": {"sourceFiles": [{"fileName": "/Component.vue.ts", "text": "x",
                                              "mappings": [[0, 2, 1], [4, 0, 0]]}],
                             "diagnostics": [{"code": 9001, "message": "raw mapper payload"}]}}
    identity, callback = plugin_call(peer, "transform", sample["params"], options=options)
    peer.complete(identity, callback, sample["result"], sample["result"])


def plugin_error_and_busy(peer, _):
    config = initialize(peer, configuration(plugins=[{"name": "mapper", "options": {}}]))
    identity, callback = plugin_call(peer, "spawn")
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": {}}),
               -32002)
    remote = {"code": 88, "message": "spawner failed", "data": {"stage": "spawn"}}
    peer.reply(callback, error=remote)
    peer.end(identity, callback)
    peer.error(identity, -32001, remote=remote)
    check_state(peer, config)
    identity, callback = plugin_call(peer, "spawn")
    peer.reply(callback, {"unexpected": "non-null"})
    peer.end(identity, callback)
    peer.error(identity, -32001)
    check_state(peer, config)
    identity, callback = plugin_call(peer, "spawn")
    peer.complete(identity, callback, None, None)
    check_state(peer, config, {"mapper": "spawned"})


def plugin_cancel_retirement(peer, _):
    config = initialize(peer, configuration(plugins=[{"name": "mapper", "options": {}}]))
    identity, callback = plugin_call(peer, "spawn")
    cancel(peer, identity, callback, plugin="mapper")
    peer.reply(callback, None)
    check_state(peer, config, {"mapper": "retired"})
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": {}}),
               -32002)


def plugin_registration_limits(peer, _):
    duplicate = [{"name": "mapper", "options": {}}] * 2
    peer.error(peer.request("test/initialize", configuration(plugins=duplicate)), -32602)
    excessive = [{"name": f"mapper-{index}", "options": {}} for index in range(33)]
    peer.error(peer.request("test/initialize", configuration(plugins=excessive)), -32602)
    initialize(peer, configuration(plugins=excessive[:32]))
    peer.error(peer.request("test/plugin", {"name": "missing", "method": "spawn", "params": {}}),
               -32602)


def shutdown_pending(peer, _):
    initialize(peer, configuration(callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    shutdown = peer.request("test/shutdown", {})
    peer.notification("$/cancelRequest", {"id": callback})
    peer.end(identity, callback)
    peer.error(identity, -32800)
    peer.result(shutdown, None)
    peer.finish(close_input=False)


def eof_pending(peer, _):
    initialize(peer, configuration(callbacks=["readFile"]))
    start_fs(peer)
    peer.finish(failure=True)


def fragmented_coalesced(peer, _):
    initialize(peer)
    first = {"jsonrpc": "2.0", "id": peer.next_id, "method": "test/state", "params": {}}
    peer.next_id += 1
    for byte in encode(first):
        peer.wire(bytes([byte]))
    peer.result(first["id"], state(configuration()))
    messages = [{"jsonrpc": "2.0", "id": peer.next_id + index,
                 "method": "test/state", "params": {}} for index in range(2)]
    peer.next_id += 2
    peer.wire(b"".join(encode(message) for message in messages))
    for message in messages:
        peer.result(message["id"], state(configuration()))


def duplicate_request(peer, _):
    initialize(peer)
    identity = peer.request("test/state", {})
    peer.result(identity, state(configuration()))
    peer.request("test/state", {}, identity=identity)
    peer.finish(failure=True, close_input=False)


def unknown_callback(peer, _):
    initialize(peer)
    peer.reply("callback:99999", {})
    peer.finish(failure=True, close_input=False)


def duplicate_callback(peer, _):
    initialize(peer, configuration(callbacks=["readFile"]))
    identity, callback = start_fs(peer)
    peer.complete(identity, callback, {"content": "ok"}, {"content": "ok"})
    peer.reply(callback, {"content": "duplicate"})
    peer.finish(failure=True, close_input=False)


def malformed_envelope(peer, _):
    peer.send({"jsonrpc": "2.0", "id": 1, "result": None, "error": {"code": 1, "message": "x"}})
    peer.finish(failure=True, close_input=False)


def invalid_lengths(peer, _):
    peer.wire(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}")
    peer.finish(failure=True, close_input=False)


def oversized_length(peer, _):
    peer.wire(b"Content-Length: 8388609\r\n\r\n")
    peer.finish(failure=True, close_input=False)


def truncated_body(peer, _):
    peer.wire(b"Content-Length: 20\r\n\r\n{}")
    peer.finish(failure=True)


def duplicate_json(peer, _):
    body = b'{"jsonrpc":"2.0","id":1,"method":"test/initialize","params":{"version":1,"version":2}}'
    peer.wire(b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body)
    peer.finish(failure=True, close_input=False)


def invalid_utf8(peer, _):
    body = b'{"jsonrpc":"2.0","id":1,"method":"\xff","params":{}}'
    peer.wire(b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body)
    peer.finish(failure=True, close_input=False)


CASES = [
    ("initialization-barrier", "controls", initialization_barrier),
    ("configuration-failure-retry", "controls", configuration_failure_retry),
    ("configuration-malformed-ready", "controls", configuration_malformed_ready),
    ("options-roundtrip", "controls", options_roundtrip),
    ("lifecycle-validation", "controls", lifecycle_validation),
    ("enabled-filesystem-callbacks", "transport", all_callbacks),
    ("progress-while-outstanding", "transport", progress_outstanding),
    ("out-of-order-completion", "transport", out_of_order),
    ("cancel-late-result-next-request", "transport", cancellation_late_response),
    ("initialization-cancel-retry", "controls", initialization_cancel_retry),
    ("remote-error-no-fallback", "transport", remote_error_no_fallback),
    ("malformed-callback-result", "transport", malformed_callback_result),
    ("options-busy-barrier", "controls", options_busy),
    ("options-blocks-new-work", "controls", options_blocks_new_work),
    ("canceled-options-barrier", "controls", canceled_options_barrier),
    ("canceled-initialization-barrier", "controls", canceled_initialization_barrier),
    ("options-reserve-plugin-state-capacity", "controls", options_reserve_plugin_state_capacity),
    ("conflicting-injected-paths", "controls", conflicting_base_paths),
    ("invalid-query-paths", "transport", invalid_query_paths),
    ("oversized-outgoing-callback", "transport", oversized_outgoing_callback),
    ("oversized-callback-error-wrapper", "transport", oversized_callback_error_wrapper),
    ("oversized-callback-result-wrapper", "transport", oversized_callback_result_wrapper),
    ("pending-limit-includes-canceled", "transport", pending_limit),
    ("plugin-lifecycle-options", "controls", plugin_lifecycle),
    ("mapper-payload-roundtrip", "controls", mapper_payload),
    ("plugin-error-and-busy", "controls", plugin_error_and_busy),
    ("plugin-cancel-retirement", "controls", plugin_cancel_retirement),
    ("plugin-registration-limits", "controls", plugin_registration_limits),
    ("shutdown-pending", "transport", shutdown_pending),
    ("eof-pending", "transport", eof_pending),
    ("fragmented-coalesced-input", "transport", fragmented_coalesced),
    ("duplicate-client-id", "transport", duplicate_request),
    ("unknown-callback-id", "transport", unknown_callback),
    ("duplicate-callback-id", "transport", duplicate_callback),
    ("malformed-envelope", "transport", malformed_envelope),
    ("duplicate-content-length", "transport", invalid_lengths),
    ("oversized-content-length", "transport", oversized_length),
    ("truncated-frame-body", "transport", truncated_body),
    ("recursive-duplicate-json-key", "transport", duplicate_json),
    ("invalid-utf8-payload", "transport", invalid_utf8),
]


from s11_followups import CASES as FOLLOWUP_CASES
CASES += FOLLOWUP_CASES


def run(binary: Path, mapper_observations: dict | None = None) -> list[dict]:
    """Execute the exact fixture inventory, retaining each failed transcript."""
    manifest = strict_json_loads((ROOT / "data/s11/transport-cases.json").read_text())
    expected = [{"id": identity, "category": category} for identity, category, _ in CASES]
    require(bool(expected), "empty transport contract inventory")
    equal(manifest, expected)
    rows = []
    for identity, category, fixture in CASES:
        row = {"id": identity, "category": category, "pass": False, "transcript": []}
        try:
            with Peer(binary, row["transcript"]) as peer:
                fixture(peer, mapper_observations)
                if not peer.finished:
                    peer.finish()
            row["pass"] = True
        except Exception as error:
            # A malformed response must fail its named fixture even when it
            # violates a shape used by a later assertion (KeyError/TypeError).
            row["error"] = f"{type(error).__name__}: {error}"
        rows.append(row)
    return rows
