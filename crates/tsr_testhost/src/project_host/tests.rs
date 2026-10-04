#![allow(
    clippy::needless_pass_by_value,
    reason = "the test client consumes inline JSON messages at its call sites"
)]
use super::*;
use serde_json::json;
use std::time::Duration;
struct Peer {
    input: InputSender,
    output: mpsc::Receiver<Json>,
    join: Option<thread::JoinHandle<io::Result<()>>>,
}
impl Peer {
    fn new() -> Self {
        let (send, output) = mpsc::channel();
        let (connection, input) = Connection::new(send);
        Self {
            input,
            output,
            join: Some(thread::spawn(|| {
                let result = connection.run();
                if let Err(error) = &result {
                    eprintln!("router error: {error}");
                }
                result
            })),
        }
    }
    fn send(&self, value: Value) {
        self.input
            .send(Input::Message(serde_json::to_vec(&value).unwrap()))
            .unwrap();
    }
    fn next(&self) -> Value {
        serde_json::from_str(
            self.output
                .recv_timeout(Duration::from_secs(5))
                .expect("router stopped making progress")
                .get(),
        )
        .unwrap()
    }
    fn request(&self, id: i32, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    }
    fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }
    fn reply(&self, id: &Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":null}));
    }
    fn until(&self, id: i32) -> Value {
        loop {
            let frame = self.next();
            if frame["id"] == id && frame.get("method").is_none() {
                return frame;
            }
            if frame["id"].is_string() && frame.get("method").is_some() {
                self.reply(&frame["id"]);
            } else {
                assert_ne!(frame["method"], "testhost/failure", "{frame}");
            }
        }
    }
    fn initialize(&self, callbacks: &[&str]) {
        self.request(1,"test/initialize",json!({"version":3,"caseSensitive":true,
            "base":{"/tsconfig.json":"{\"compilerOptions\":{\"noLib\":true},\"files\":[\"main.ts\",\"lib.d.ts\"]}","/lib.d.ts":"declare const stable: number;"},
            "symlinks":{},"callbacks":callbacks,"plugins":[],"options":{"noLib":true},
            "project":{"currentDirectory":"/","defaultLibraryPath":"/","positionEncoding":"utf-16"}}));
        let result = self.until(1);
        assert_eq!(result["result"]["version"], 3, "{result}");
        assert_eq!(
            self.next(),
            json!({"jsonrpc":"2.0","method":"testhost/initialized","params":{"version":3}})
        );
    }
    fn open(&self) {
        self.notify("textDocument/didOpen",json!({"textDocument":{"uri":"file:///main.ts","version":1,"languageId":"typescript","text":"const x = stable;"}}));
    }
    fn edit(&self) {
        self.notify("textDocument/didChange",json!({"textDocument":{"uri":"file:///main.ts","version":2},"contentChanges":[{"text":"const x = stable + 1;"}]}));
    }
    fn state(&self, id: i32) -> Value {
        self.request(id, "test/projectState", json!({}));
        let frame = self.until(id);
        assert!(frame.get("error").is_none(), "{frame}");
        frame["result"].clone()
    }
    fn finish(mut self) {
        self.input.send(Input::End(Ok(()))).unwrap();
        self.join.take().unwrap().join().unwrap().unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = self.input.send(Input::End(Ok(())));
            let _ = join.join();
        }
    }
}

#[test]
fn endpoint_projects_are_real_and_identity_changes_follow_the_compiler_cache() {
    let peer = Peer::new();
    peer.initialize(&[]);
    peer.open();
    let first = peer.state(2);
    assert_eq!(first["projects"][0]["name"], "/tsconfig.json");
    assert_eq!(first["openFiles"][0]["defaultProject"], "/tsconfig.json");
    assert_eq!(first, peer.state(3), "no action means the same identities");
    peer.edit();
    let next = peer.state(4);
    assert_ne!(
        first["projects"][0]["program"],
        next["projects"][0]["program"]
    );
    let find = |state: &Value, name: &str| {
        state["projects"][0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["fileName"] == name)
            .unwrap()["id"]
            .clone()
    };
    assert_ne!(find(&first, "/main.ts"), find(&next, "/main.ts"));
    assert_eq!(find(&first, "/lib.d.ts"), find(&next, "/lib.d.ts"));
    peer.finish();
}

#[test]
fn blocked_read_does_not_stop_host_state_progress_or_reset_and_late_replies() {
    let peer = Peer::new();
    peer.initialize(&["readFile"]);
    peer.open();
    let callback = peer.next();
    assert_eq!(callback["method"], "readFile");
    peer.request(2, "test/state", json!({}));
    assert_eq!(
        peer.next()["id"],
        2,
        "router serves host state while worker is blocked"
    );
    peer.notify(
        "test/callbackProgress",
        json!({"callback":callback["id"],"value":{"step":"reading"}}),
    );
    assert_eq!(peer.next()["params"]["value"]["step"], "reading");
    peer.request(3, "test/reset", json!({}));
    loop {
        let frame = peer.next();
        if frame["id"] == 3 {
            assert_eq!(frame["result"]["reset"], true, "{frame}");
            break;
        }
        // Cancellation may race completion of the document notification. Its
        // failure is observed before reset, and must not poison the next test.
        assert!(
            matches!(
                frame["method"].as_str(),
                Some("$/cancelRequest" | "testhost/failure")
            ),
            "{frame}"
        );
    }
    peer.initialize(&["readFile"]);
    peer.reply(&callback["id"]);
    peer.open();
    let new_callback = peer.next();
    assert_eq!(new_callback["method"], "readFile");
    assert_ne!(callback["id"], new_callback["id"]);
    peer.reply(&new_callback["id"]);
    assert_eq!(peer.state(4)["openFiles"].as_array().unwrap().len(), 1);
    peer.finish();
}

#[test]
fn canceling_a_snapshot_read_releases_its_callbacks_but_the_next_request_still_works() {
    let peer = Peer::new();
    peer.initialize(&["readFile"]);
    peer.open();
    let first = peer.state(2);
    peer.edit();
    peer.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":"file:///lib.d.ts","type":2}]}),
    );
    peer.request(3, "test/projectState", json!({}));
    let callback = peer.next();
    assert_eq!(callback["method"], "readFile", "{callback}");
    peer.notify("$/cancelRequest", json!({"id":3}));
    loop {
        let frame = peer.next();
        if frame["id"] == 3 {
            assert_eq!(frame["error"]["code"], -32800, "{frame}");
            break;
        }
        assert_eq!(frame["method"], "$/cancelRequest", "{frame}");
    }
    peer.reply(&callback["id"]);
    let next = peer.state(4);
    assert_ne!(
        first["projects"][0]["program"],
        next["projects"][0]["program"]
    );
    peer.finish();
}

#[test]
fn disconnect_settles_a_worker_waiting_for_the_host() {
    let peer = Peer::new();
    peer.initialize(&["readFile"]);
    peer.open();
    assert_eq!(peer.next()["method"], "readFile");
    peer.finish();
}

#[test]
fn reset_has_a_reserved_queue_slot_when_every_regular_slot_is_in_flight() {
    let peer = Peer::new();
    peer.initialize(&["readFile"]);
    peer.open();
    assert_eq!(peer.next()["method"], "readFile");
    for version in 2..65 {
        peer.notify("textDocument/didChange",json!({"textDocument":{"uri":"file:///main.ts","version":version},"contentChanges":[{"text":"const x = stable;"}]}));
    }
    peer.request(2, "test/reset", json!({}));
    loop {
        let frame = peer.next();
        if frame["id"] == 2 {
            assert_eq!(frame["result"]["reset"], true, "{frame}");
            break;
        }
        assert!(
            matches!(
                frame["method"].as_str(),
                Some("$/cancelRequest" | "testhost/failure")
            ),
            "{frame}"
        );
    }
    peer.initialize(&[]);
    assert!(peer.state(3)["openFiles"].as_array().unwrap().is_empty());
    peer.finish();
}
