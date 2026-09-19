"""Version 2 regressions for admission, request identities and raw JSON."""
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


def options_roundtrip(peer, _):
    import s11_contracts as c
    config=c.initialize(peer,c.configuration(options={"strict":False}))
    value={"strict":True,"target":"ESNext","nested":[None,"😀"]}
    peer.result(peer.request("test/setOptions",{"options":value}),{"options":value})
    config["options"]=value;c.check_state(peer,config)
    peer.error(peer.request("test/setOptions",{"options":[]}),-32602)
    c.check_state(peer,config)


def lifecycle(peer, _):
    import s11_contracts as c
    peer.error(peer.request("test/setOptions",{"options":{}}),-32002)
    peer.error(peer.request("test/openPlugin",{"name":"mapper"}),-32002)
    for config in (c.configuration(version=1),c.configuration(options=[]),c.configuration(callbacks=["readFile"]*2),
                   c.configuration(plugins=[{"name":"p","options":{}}]*2),
                   c.configuration(plugins=[{"name":str(i),"options":{}} for i in range(33)])):
        peer.error(peer.request("test/initialize",config),-32602)
    c.initialize(peer)
    peer.error(peer.request("test/initialize",c.configuration()),-32002)
    peer.error(peer.request("test/plugin",{"name":"mapper","method":"spawn","params":{}}),-32601)
    peer.error(peer.request("test/openPlugin",{"name":"missing"}),-32602)


def ids(peer, _):
    import s11_contracts as c
    config=c.initialize(peer,c.configuration(callbacks=["readFile"]))
    for identity in (0,-1,2147483647,-2147483648,"","1","callback:1","😀"):
        peer.result(peer.request("test/state",{},identity=identity),c.state(config))
        peer.result(peer.request("test/state",{},identity=identity),c.state(config))
    params={"operation":"readFile","path":"/x"}
    peer.request("test/fs",params,identity="callback:1")
    callback=peer.begin("callback:1","readFile","/x")
    c.equal(callback,"callback:1")
    c.cancel(peer,"callback:1",callback)
    peer.request("test/fs",params,identity="callback:1")
    next_callback=peer.begin("callback:1","readFile","/x")
    peer.notify("test/callbackProgress",{"callback":callback,"value":"old operation"})
    peer.reply(callback,{"content":"old"})
    peer.complete("callback:1",next_callback,{"content":"new"},{"content":"new"})
    # A numeric ID and its string spelling can be in flight simultaneously.
    for identity in (7,"7"):
        peer.request("test/fs",params,identity=identity)
        cb=peer.begin(identity,"readFile","/x")
        if identity == 7: first=cb
        else: second=cb
    peer.complete("7",second,{"content":"string"},{"content":"string"})
    peer.complete(7,first,{"content":"integer"},{"content":"integer"})


def duplicate_id(peer, _):
    import s11_contracts as c
    c.initialize(peer,c.configuration(callbacks=["readFile"]))
    identity,_=c.start_fs(peer)
    peer.request("test/state",{},identity=identity)
    peer.finish(failure=True,close_input=False)


def result_expansion(peer, _):
    import s11_contracts as c
    config=c.initialize(peer,c.configuration(callbacks=["readFile"]))
    identity="request:" + "x"*1024
    peer.request("test/fs",{"operation":"readFile","path":"/x"},identity=identity)
    callback=peer.begin(identity,"readFile","/x")
    result={"content":""};message={"jsonrpc":"2.0","id":callback,"result":result}
    result["content"]="x"*(c.MAX_BODY-len(c.json_bytes(message)))
    peer.send(message);peer.end(identity,callback);peer.error(identity,-32001)
    c.check_state(peer,config)


def options_capacity(peer, _):
    import s11_contracts as c
    from s11_tunnel import open_stream, close_stream
    config=c.initialize(peer,c.configuration(plugins=[{"name":"mapper","options":{}}]))
    stream=open_stream(peer)
    prospective=c.configuration(plugins=config["plugins"],options={"padding":""})
    envelope={"jsonrpc":"2.0","id":-2147483648,"result":c.state(prospective,{"mapper":"open"})}
    prospective["options"]["padding"]="x"*(c.MAX_BODY-1-len(c.json_bytes(envelope)))
    peer.error(peer.request("test/setOptions",{"options":prospective["options"]}),-32602)
    c.check_state(peer,config,{"mapper":"open"})
    close_stream(peer,stream)


def lossless_numbers(peer, _):
    import s11_contracts as c
    payload=(b'{"integer":123456789012345678901234567890,"decimal":0.12345678901234567890123456789,'
             b'"zero":-0,"exponent":1.234567890123456789e+200,"ordinary":{"$serde_json::private::Number":"123"}}')
    def send(message):
        body=c.json_bytes(message).replace(b'"__opaque_payload__"',payload)
        peer.wire(b"Content-Length: "+str(len(body)).encode()+b"\r\n\r\n"+body)
    def exact():
        c.require(payload in peer.last_body,"opaque numeric tokens changed")
        peer.transcript.append({"exact_numeric_frame":peer.last_body.decode(),"sha256":hashlib.sha256(peer.last_body).hexdigest()})
    config=c.configuration(options="__opaque_payload__",callbacks=["readFile"],plugins=[{"name":"mapper","options":"__opaque_payload__"}])
    send({"jsonrpc":"2.0","id":1,"method":"test/initialize","params":config})
    peer.read();exact();peer.notification("testhost/initialized",{"version":2})
    send({"jsonrpc":"2.0","id":2,"method":"test/setOptions","params":{"options":"__opaque_payload__"}})
    peer.read();exact()
    peer.next_id=3
    identity=peer.request("test/openPlugin",{"name":"mapper"})
    peer.read();callback=peer.read()["id"];exact()
    send({"jsonrpc":"2.0","method":"test/callbackProgress","params":{"callback":callback,"value":"__opaque_payload__"}})
    peer.read();exact()
    send({"jsonrpc":"2.0","id":callback,"error":{"code":17,"message":"remote","data":"__opaque_payload__"}})
    peer.end(identity,callback);peer.read();exact()


CASES = [
    ("options-internal-stub-roundtrip", "controls", options_roundtrip),
    ("version2-lifecycle-registration-validation", "controls", lifecycle),
    ("lsp-identities-reuse-and-directional-collision", "transport", ids),
    ("duplicate-inflight-client-id", "transport", duplicate_id),
    ("oversized-progress-preserves-unrelated-work", "transport", progress_overflow),
    ("initialization-state-size-admission", "controls", initialization_capacity),
    ("string-id-callback-result-expansion", "transport", result_expansion),
    ("options-reserve-plugin-state-capacity", "controls", options_capacity),
    ("lossless-opaque-numbers", "transport", lossless_numbers),
]
