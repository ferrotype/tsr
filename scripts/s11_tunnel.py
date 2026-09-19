"""Opaque stream contract probes, separate from actual pinned mapper observations."""
import base64

CHUNK = 32 * 1024
WINDOW = 64 * 1024


def b64(data):
    return base64.b64encode(data).decode("ascii")


def open_stream(peer, name="mapper", stream="stream:1", options=None):
    import s11_contracts as c
    identity = peer.request("test/openPlugin", {"name": name})
    callback = peer.begin(identity, "testhost/spawnPlugin", {"name": name, "options": options or {}})
    peer.complete(identity, callback, {"stream": stream}, {"stream": stream})
    for channel in ("stdout", "stderr"):
        peer.notification("testhost/streamCredit", {"stream": stream, "channel": channel, "bytes": WINDOW})
    return stream


def write_bytes(peer, stream, data):
    peer.notify("test/streamCredit", {"stream": stream, "bytes": len(data)})
    identity = peer.request("test/streamWrite", {"stream": stream, "data": b64(data)})
    message = peer.read()
    import s11_contracts as c
    c.equal(set(message), {"jsonrpc", "method", "params"})
    c.equal(message["method"], "testhost/streamData")
    c.equal(message["params"], {"stream": stream, "data": b64(data)})
    actual = base64.b64decode(message["params"]["data"], validate=True)
    peer.result(identity, None)
    return actual


def read_bytes(peer, stream, data, channel="stdout", read_size=CHUNK):
    import s11_contracts as c
    peer.notify("test/streamData", {"stream": stream, "channel": channel, "data": b64(data)})
    actual = bytearray()
    while len(actual) < len(data):
        identity = peer.request("test/streamRead", {"stream": stream, "channel": channel, "maxBytes": read_size})
        response = peer.read()
        count = min(read_size, len(data) - len(actual))
        c.equal(response, {"jsonrpc": "2.0", "id": identity,
                           "result": {"data": b64(data[len(actual):len(actual)+count]), "eof": False}})
        actual.extend(base64.b64decode(response["result"]["data"], validate=True))
        peer.notification("testhost/streamCredit", {"stream": stream, "channel": channel, "bytes": count})
    return bytes(actual)


def close_stream(peer, stream):
    identity = peer.request("test/closeStream", {"stream": stream})
    peer.notification("testhost/streamClose", {"stream": stream})
    peer.result(identity, None)


def setup(peer, count=1):
    import s11_contracts as c
    plugins = [{"name": "mapper" if i == 0 else f"mapper{i}", "options": {}} for i in range(count)]
    config = c.initialize(peer, c.configuration(plugins=plugins, callbacks=["readFile"]))
    streams = [open_stream(peer, p["name"], f"stream:{i+1}") for i, p in enumerate(plugins)]
    return config, streams


def binary_chunks(peer, _):
    import s11_contracts as c
    _, (stream,) = setup(peer)
    data = bytes(range(256)) * 128
    c.equal(write_bytes(peer, stream, data), data)
    c.equal(read_bytes(peer, stream, data, read_size=997), data)
    c.equal(read_bytes(peer, stream, b"stderr\x00\xff\r\n", "stderr"), b"stderr\x00\xff\r\n")
    close_stream(peer, stream)


def concurrent_frames(peer, _):
    import s11_contracts as c
    _, (stream, other) = setup(peer, 2)
    # Two complete mapper requests are sent before either response. Their
    # contents and identities are opaque to the tunnel; replies arrive reversed.
    first = c.encode({"jsonrpc":"2.0", "id":1, "method":"transform", "params":{}})
    second = c.encode({"jsonrpc":"2.0", "id":2, "method":"transform", "params":{}})
    peer.notify("test/streamCredit", {"stream": stream, "bytes": len(first)+len(second)})
    identities = [peer.request("test/streamWrite", {"stream":stream, "data":b64(data)}) for data in (first, second)]
    for identity, data in zip(identities, (first, second)):
        peer.notification("testhost/streamData", {"stream":stream, "data":b64(data)})
        peer.result(identity, None)
    c.equal(write_bytes(peer, other, b"unrelated"), b"unrelated")
    replies = c.encode({"jsonrpc":"2.0","id":2,"result":None}) + c.encode({"jsonrpc":"2.0","id":1,"result":None})
    # Fragment inside a frame header, then coalesce across the reply boundary.
    actual = read_bytes(peer, stream, replies[:3]) + read_bytes(peer, stream, replies[3:])
    c.equal(actual, replies)
    close_stream(peer, stream); close_stream(peer, other)


def backpressure(peer, _):
    import s11_contracts as c
    config, (stream,) = setup(peer)
    peer.error(peer.request("test/streamWrite", {"stream":stream,"data":b64(b"x")}), -32002)
    for _ in range(2):
        peer.notify("test/streamData", {"stream":stream,"channel":"stdout","data":b64(b"x"*CHUNK)})
    # A full stream cannot starve filesystem callbacks or cancellation.
    identity, callback = c.start_fs(peer)
    c.cancel(peer, identity, callback)
    peer.reply(callback, {"content":"late"})
    c.check_state(peer, config, {"mapper":"open"})
    identity = peer.request("test/streamRead", {"stream":stream,"channel":"stdout","maxBytes":CHUNK})
    peer.result(identity, {"data":b64(b"x"*CHUNK),"eof":False})
    peer.notification("testhost/streamCredit", {"stream":stream,"channel":"stdout","bytes":CHUNK})
    peer.notify("test/streamData", {"stream":stream,"channel":"stdout","data":b64(b"y"*CHUNK)})
    close_stream(peer, stream)


def credit_overrun(peer, _):
    _, (stream,) = setup(peer)
    peer.notify("test/streamCredit", {"stream":stream,"bytes":WINDOW+1})
    peer.finish(failure=True, close_input=False)


def receive_overrun(peer, _):
    _, (stream,) = setup(peer)
    for _ in range(3):
        peer.notify("test/streamData", {"stream":stream,"channel":"stdout","data":b64(b"x"*CHUNK)})
    peer.finish(failure=True, close_input=False)


def eof_exit(peer, _):
    import s11_contracts as c
    config, (stream,) = setup(peer)
    identity = peer.request("test/streamRead", {"stream":stream,"channel":"stdout","maxBytes":CHUNK})
    peer.result(identity, {"data":None,"eof":False})
    peer.notify("test/streamData", {"stream":stream,"channel":"stdout","data":b64(b"last")})
    for channel in ("stdout", "stderr"):
        peer.notify("test/streamEnd", {"stream":stream,"channel":channel})
    peer.notify("test/streamExit", {"stream":stream,"code":17})
    c.check_state(peer, config)
    peer.result(peer.request("test/streamStatus", {"stream":stream}), {"closed":False,"exit":{"state":"exited","code":17}})
    identity = peer.request("test/streamRead", {"stream":stream,"channel":"stdout","maxBytes":CHUNK})
    peer.result(identity, {"data":b64(b"last"),"eof":True})
    peer.error(peer.request("test/streamWrite", {"stream":stream,"data":b64(b"x")}), -32002)
    close_stream(peer, stream)


def early_exit(peer, _):
    _, (stream,) = setup(peer)
    peer.notify("test/streamExit", {"stream":stream,"code":0})
    peer.finish(failure=True, close_input=False)


def close_late(peer, _):
    import s11_contracts as c
    config, (stream,) = setup(peer)
    close_stream(peer, stream)
    peer.notify("test/streamData", {"stream":stream,"channel":"stdout","data":b64(b"late")})
    peer.notify("test/streamExit", {"stream":stream,"code":None})
    peer.result(peer.request("test/streamStatus", {"stream":stream}), {"closed":True,"exit":{"state":"unavailable"}})
    peer.result(peer.request("test/streamRead", {"stream":stream,"channel":"stdout","maxBytes":CHUNK}), {"data":"","eof":True})
    peer.result(peer.request("test/closeStream", {"stream":stream}), None)
    c.check_state(peer, config)
    new = open_stream(peer, stream="replacement")
    peer.notify("test/streamClose", {"stream":stream})
    c.check_state(peer, config, {"mapper":"open"})
    c.equal(write_bytes(peer, new, b"new child"), b"new child")
    close_stream(peer, new)


def client_close(peer, _):
    import s11_contracts as c
    config, (stream,) = setup(peer)
    peer.notify("test/streamClose", {"stream":stream})
    c.check_state(peer, config)
    peer.error(peer.request("test/streamWrite", {"stream":stream,"data":b64(b"x")}), -32002)
    peer.result(peer.request("test/closeStream", {"stream":stream}), None)


def invalid_base64(peer, _):
    _, (stream,) = setup(peer)
    # Nonzero discarded padding bits must not be accepted as another encoding.
    peer.notify("test/streamData", {"stream":stream,"channel":"stdout","data":"Zh=="})
    peer.finish(failure=True, close_input=False)


def oversized_chunk(peer, _):
    _, (stream,) = setup(peer)
    peer.error(peer.request("test/streamWrite", {"stream":stream,"data":b64(b"x"*(CHUNK+1))}), -32602)
    close_stream(peer, stream)


def spawn_cancel(peer, _):
    import s11_contracts as c
    config = c.initialize(peer, c.configuration(plugins=[{"name":"mapper","options":{}}]))
    identity = peer.request("test/openPlugin", {"name":"mapper"})
    callback = peer.begin(identity, "testhost/spawnPlugin", {"name":"mapper","options":{}})
    c.cancel(peer, identity, callback, plugin="mapper")
    peer.reply(callback, {"stream":"late-child"})
    c.check_state(peer, config, {"mapper":"retired"})
    peer.error(peer.request("test/openPlugin", {"name":"mapper"}), -32002)


def spawn_failure(peer, _):
    import s11_contracts as c
    config = c.initialize(peer, c.configuration(plugins=[{"name":"mapper","options":{}}]))
    identity = peer.request("test/openPlugin", {"name":"mapper"})
    callback = peer.begin(identity, "testhost/spawnPlugin", {"name":"mapper","options":{}})
    peer.error(peer.request("test/openPlugin", {"name":"mapper"}), -32002)
    remote = {"code":17,"message":"not spawned"}
    peer.reply(callback, error=remote); peer.end(identity, callback); peer.error(identity,-32001,remote=remote)
    c.check_state(peer, config)
    stream=open_stream(peer);close_stream(peer,stream)


def duplicate_stream(peer, _):
    import s11_contracts as c
    c.initialize(peer,c.configuration(plugins=[{"name":n,"options":{}} for n in ("mapper","other")]))
    stream=open_stream(peer)
    identity=peer.request("test/openPlugin",{"name":"other"})
    callback=peer.begin(identity,"testhost/spawnPlugin",{"name":"other","options":{}})
    peer.reply(callback,{"stream":stream});peer.end(identity,callback)
    peer.notification("testhost/retirePlugin",{"name":"other"});peer.error(identity,-32001)
    c.equal(write_bytes(peer,stream,b"first is still live"),b"first is still live")
    close_stream(peer,stream)


def stream_shutdown(peer, _):
    _, (stream,) = setup(peer)
    identity=peer.request("test/shutdown",{})
    peer.notification("testhost/streamClose",{"stream":stream})
    peer.result(identity,None);peer.finish(close_input=False)


def identity_limit(peer, _):
    import s11_contracts as c
    c.initialize(peer, c.configuration(plugins=[{"name":n,"options":{}} for n in ("mapper","other")]))
    for i in range(63):
        stream = open_stream(peer, stream=f"stream:{i}")
        close_stream(peer, stream)
    # Pending spawns reserve the remaining identity budget before invoking the host.
    identity = peer.request("test/openPlugin", {"name":"mapper"})
    callback = peer.begin(identity, "testhost/spawnPlugin", {"name":"mapper","options":{}})
    peer.error(peer.request("test/openPlugin", {"name":"other"}), -32002)
    peer.reply(callback, error={"code":17,"message":"not spawned"})
    peer.end(identity,callback);peer.error(identity,-32001)
    stream = open_stream(peer, name="other", stream="stream:63")
    close_stream(peer, stream)
    peer.error(peer.request("test/openPlugin", {"name":"mapper"}), -32002)



CASES = [
    ("stream-binary-chunks-and-stderr", "controls", binary_chunks),
    ("stream-concurrent-fragmented-coalesced-mapper-frames", "controls", concurrent_frames),
    ("stream-backpressure-keeps-cancellation-live", "transport", backpressure),
    ("stream-credit-overrun", "transport", credit_overrun),
    ("stream-receive-window-overrun", "transport", receive_overrun),
    ("stream-eof-exit-preserves-unread-bytes", "controls", eof_exit),
    ("stream-exit-before-eof", "transport", early_exit),
    ("stream-local-close-late-data-unavailable-exit", "controls", close_late),
    ("stream-client-close", "controls", client_close),
    ("stream-noncanonical-base64", "transport", invalid_base64),
    ("stream-oversized-chunk", "transport", oversized_chunk),
    ("stream-spawn-cancel-retires-late-child", "controls", spawn_cancel),
    ("stream-spawn-error-retry", "controls", spawn_failure),
    ("stream-duplicate-identity", "transport", duplicate_stream),
    ("stream-shutdown", "transport", stream_shutdown),
    ("stream-identity-budget-admission", "controls", identity_limit),
]
