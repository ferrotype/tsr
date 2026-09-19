"""S11 counterexamples for configuration atomicity and lossless bounded forwarding."""
import hashlib


def progress_overflow(peer, _):
    import s11_contracts as c
    config = c.initialize(peer, c.configuration(callbacks=["readFile"]))
    identity, callback = c.start_fs(peer)
    other, other_callback = c.start_fs(peer, path="/other.ts")
    message = {"jsonrpc": "2.0", "method": "test/callbackProgress",
               "params": {"callback": callback, "value": ""}}
    message["params"]["value"] = "x" * (c.MAX_BODY - len(c.json_bytes(message)))
    c.equal(len(c.json_bytes(message)), c.MAX_BODY)
    peer.send(message)
    peer.notification("$/cancelRequest", {"id": callback})
    peer.end(identity, callback)
    peer.error(identity, -32001)
    # The overflow ends only this operation; its late reply cannot complete it twice.
    peer.notify("test/callbackProgress", {"callback": callback, "value": "late"})
    peer.complete(other, other_callback, {"content": "unrelated"}, {"content": "unrelated"})
    peer.reply(callback, {"content": "late"})
    c.check_state(peer, config)
    identity, callback = c.start_fs(peer)
    peer.complete(identity, callback, {"content": "next"}, {"content": "next"})


def plugin_progress_overflow(peer, _):
    import s11_contracts as c
    config = c.initialize(peer, c.configuration(plugins=[{"name": "mapper", "options": {}}]))
    identity, callback = c.plugin_call(peer, "spawn")
    message = {"jsonrpc": "2.0", "method": "test/callbackProgress",
               "params": {"callback": callback, "value": ""}}
    message["params"]["value"] = "x" * (c.MAX_BODY - len(c.json_bytes(message)))
    peer.send(message)
    peer.notification("$/cancelRequest", {"id": callback})
    peer.notification("testhost/retirePlugin", {"name": "mapper"})
    peer.end(identity, callback)
    peer.error(identity, -32001)
    peer.reply(callback, None)
    c.check_state(peer, config, {"mapper": "retired"})
    peer.error(peer.request("test/plugin", {"name": "mapper", "method": "spawn", "params": {}}), -32002)


def initialization_capacity(peer, _):
    import s11_contracts as c
    config = c.configuration(plugins=[{"name": f"p{i}", "options": {}} for i in range(32)])
    config["plugins"][0]["options"]["padding"] = ""
    request = {"jsonrpc": "2.0", "id": 1, "method": "test/initialize", "params": config}
    config["plugins"][0]["options"]["padding"] = "x" * (c.MAX_BODY - len(c.json_bytes(request)))
    c.equal(len(c.json_bytes(request)), c.MAX_BODY)
    c.require(len(c.json_bytes({"jsonrpc": "2.0", "id": 1, "result": c.state(config)})) > c.MAX_BODY,
              "fixture must expand in the initialization response")
    peer.error(peer.request("test/initialize", config), -32602)
    c.initialize(peer)


def canceled_applied(peer, initialize, result):
    import s11_contracts as c
    if initialize:
        identity = peer.request("test/initialize", c.configuration(options={"attempt": 1}))
        options = {"attempt": 1}
    else:
        c.initialize(peer, c.configuration(options={"strict": False}))
        options = {"strict": True}
        identity = peer.request("test/setOptions", {"options": options})
    callback = peer.begin(identity, "testhost/configuration", {"options": options})
    c.cancel(peer, identity, callback)
    # A buffered follow-up must never observe old state after the ambiguous reply.
    peer.wire(c.encode({"jsonrpc": "2.0", "id": callback, "result": result}) +
              c.encode({"jsonrpc": "2.0", "id": peer.next_id, "method": "test/state", "params": {}}))
    peer.finish(failure=True)
    c.require(b"canceled configuration may have been applied" in peer.errors,
              "must diagnose the configuration split")


def canceled_options_applied(peer, _):
    canceled_applied(peer, False, {"ready": True})


def canceled_initialization_applied(peer, _):
    canceled_applied(peer, True, {"ready": True})


def canceled_configuration_ambiguous(peer, _):
    canceled_applied(peer, False, {"ready": "unknown"})


def lossless_numbers(peer, _):
    import s11_contracts as c
    # Include a decimal and a negative zero as well as an oversized integer.
    # Value-level Python float equality would miss decimal/negative-zero changes.
    payload = (b'{"integer":123456789012345678901234567890,'
               b'"decimal":0.12345678901234567890123456789,"zero":-0,'
               b'"exponent":1.234567890123456789e+200,'
               b'"ordinary":{"$serde_json::private::Number":"123"}}')

    def send(message):
        body = c.json_bytes(message).replace(b'"__opaque_payload__"', payload)
        peer.wire(b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body)

    def check_raw():
        c.require(payload in peer.last_body, "opaque numeric tokens changed in transit")
        peer.transcript.append({"exact_numeric_frame": peer.last_body.decode(),
                                "sha256": hashlib.sha256(peer.last_body).hexdigest()})

    config = c.configuration(options="__opaque_payload__",
                             plugins=[{"name": "mapper", "options": "__opaque_payload__"}])
    send({"jsonrpc": "2.0", "id": 1, "method": "test/initialize", "params": config})
    peer.read()
    callback = peer.read()["id"]
    check_raw()
    peer.reply(callback, {"ready": True})
    peer.end(1, callback)
    peer.read()
    check_raw()
    peer.notification("testhost/initialized", {"version": 1})
    for identity, method, result in [(2, "spawn", None), (3, "initialize", {})]:
        peer.request("test/plugin", {"name": "mapper", "method": method, "params": {}}, identity=identity)
        peer.read()
        callback = peer.read()["id"]
        check_raw()
        peer.complete(identity, callback, result, result)
    send({"jsonrpc": "2.0", "id": 4, "method": "test/plugin",
          "params": {"name": "mapper", "method": "transform", "params": "__opaque_payload__"}})
    peer.read()
    callback = peer.read()["id"]
    check_raw()
    send({"jsonrpc": "2.0", "method": "test/callbackProgress",
          "params": {"callback": callback, "value": "__opaque_payload__"}})
    peer.read()
    check_raw()
    send({"jsonrpc": "2.0", "id": callback, "result": "__opaque_payload__"})
    peer.end(4, callback)
    peer.read()
    check_raw()
    peer.request("test/plugin", {"name": "mapper", "method": "transform", "params": {}}, identity=5)
    peer.read()
    callback = peer.read()["id"]
    send({"jsonrpc": "2.0", "id": callback,
          "error": {"code": 17, "message": "remote", "data": "__opaque_payload__"}})
    peer.end(5, callback)
    response = peer.read()
    c.equal(response["error"]["code"], -32001)
    check_raw()


CASES = [
    ("oversized-progress-preserves-unrelated-work", "transport", progress_overflow),
    ("oversized-plugin-progress-retires-plugin", "transport", plugin_progress_overflow),
    ("initialization-state-size-admission", "controls", initialization_capacity),
    ("canceled-options-applied-retires-session", "controls", canceled_options_applied),
    ("canceled-initialization-applied-retires-session", "controls", canceled_initialization_applied),
    ("canceled-configuration-ambiguous-retires-session", "controls", canceled_configuration_ambiguous),
    ("lossless-opaque-numbers", "transport", lossless_numbers),
]
