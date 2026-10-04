use crate::{
    connection::{Connection, Input},
    runtime::Options,
};
use std::{
    sync::{mpsc, Arc},
    time::Duration,
};
use tsr_json::RawValue;
use tsr_lsproto::{Id, Message};

struct TestConnection {
    input: Input,
    receive: mpsc::Receiver<RawValue>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl TestConnection {
    fn new() -> Self {
        let mut files = tsr_vfs::MemoryBuilder::new(b"/", true);
        files.insert_loaded(
            b"/p/tsconfig.json",
            br#"{"compilerOptions":{"strict":true,"noLib":true},"files":["main.ts"]}"#.as_slice(),
        );
        files.insert_loaded(b"/p/main.ts", b"const x: number = 1;".as_slice());
        let (send, receive) = mpsc::channel();
        let (connection, input) = Connection::new(
            Options::new(
                tsr_project::session::SessionOptions::default(),
                Arc::new(files.finish()),
            ),
            &tsr_ipc::Context::background(),
            Arc::new(move |value| {
                send.send(value)
                    .map_err(|_| crate::error(-32603, "test receiver closed"))
            }),
            Box::new(std::io::sink()),
        );
        let worker = std::thread::spawn(move || connection.run());
        Self {
            input,
            receive,
            worker: Some(worker),
        }
    }
    fn send(&self, id: Option<Id>, method: &str, params: Option<&str>) {
        self.input
            .receive(Message {
                id,
                method: method.into(),
                params: params.map(|p| RawValue(p.as_bytes().to_vec())),
                ..Default::default()
            })
            .unwrap();
    }
    fn response(&self, id: &Id) -> Message {
        loop {
            let raw = self
                .receive
                .recv_timeout(Duration::from_secs(15))
                .expect("server response");
            let mut message = Message::default();
            tsr_json::unmarshal(&raw.0, &mut message, tsr_json::Options::default()).unwrap();
            if message.is_response() && message.id.as_ref() == Some(id) {
                return message;
            }
            if message.method == "workspace/configuration" {
                self.input
                    .receive(Message {
                        id: message.id,
                        result: Some(RawValue(b"[{}, {}, {}, {}]".to_vec())),
                        ..Default::default()
                    })
                    .unwrap();
            } else if message.is_request() {
                self.input
                    .receive(Message {
                        id: message.id,
                        result: Some(RawValue(b"null".to_vec())),
                        ..Default::default()
                    })
                    .unwrap();
            }
        }
    }
    fn initialize(&self, encoding: &str) {
        self.send(Some(Id::string("initialize")), "initialize", Some(&format!(r#"{{"processId":null,"rootUri":"file:///p","capabilities":{{"general":{{"positionEncodings":["{encoding}"]}},"workspace":{{"configuration":true,"didChangeWatchedFiles":{{"dynamicRegistration":true}}}}}}}}"#)));
        let response = self.response(&Id::string("initialize"));
        assert!(response.error.is_none(), "{:?}", response.error);
        assert!(String::from_utf8(response.result.unwrap().0)
            .unwrap()
            .contains(encoding));
        self.send(None, "initialized", Some("{}"));
    }
}
impl Drop for TestConnection {
    fn drop(&mut self) {
        self.input.end();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

#[test]
fn initialization_edit_diagnostics_and_shutdown_use_the_real_session() {
    for encoding in ["utf-8", "utf-16"] {
        let server = TestConnection::new();
        server.send(
            Some(Id::int(9)),
            "custom/projectInfo",
            Some(r#"{"textDocument":{"uri":"file:///p/main.ts"}}"#),
        );
        assert_eq!(server.response(&Id::int(9)).error.unwrap().code, -32002);
        server.initialize(encoding);
        server.send(None, "textDocument/didOpen", Some(r#"{"textDocument":{"uri":"file:///p/main.ts","version":1,"languageId":"typescript","text":"/*😄*/ const x: number = \"bad\";"}}"#));
        server.send(
            Some(Id::string("diags")),
            "textDocument/diagnostic",
            Some(r#"{"textDocument":{"uri":"file:///p/main.ts"}}"#),
        );
        let response = server.response(&Id::string("diags"));
        assert!(response.error.is_none(), "{:?}", response.error);
        let mut report = tsr_lsproto::RelatedFullDocumentDiagnosticReport::default();
        tsr_json::unmarshal(
            &response.result.unwrap().0,
            &mut report,
            tsr_json::Options::default(),
        )
        .unwrap();
        let d = report
            .items
            .iter()
            .flatten()
            .find(|d| d.code.as_deref().and_then(|c| c.integer.as_deref()) == Some(&2322))
            .expect("real assignability diagnostic");
        assert_eq!(
            d.range.start.character,
            if encoding == "utf-8" { 15 } else { 13 }
        );
        server.send(None, "textDocument/didChange", Some(r#"{"textDocument":{"uri":"file:///p/main.ts","version":2},"contentChanges":[{"text":"const x: number = 1;"}]}"#));
        server.send(
            Some(Id::int(-1)),
            "textDocument/diagnostic",
            Some(r#"{"textDocument":{"uri":"file:///p/main.ts"}}"#),
        );
        let response = server.response(&Id::int(-1));
        assert!(response.error.is_none(), "{:?}", response.error);
        tsr_json::unmarshal(
            &response.result.unwrap().0,
            &mut report,
            tsr_json::Options::default(),
        )
        .unwrap();
        assert!(report.items.is_empty());
        server.send(
            Some(Id::int(5)),
            "custom/projectInfo",
            Some(r#"{"textDocument":{"uri":"file:///p/main.ts"}}"#),
        );
        assert_eq!(
            server.response(&Id::int(5)).result.unwrap().0,
            br#"{"configFilePath":"/p/tsconfig.json"}"#
        );
        server.send(Some(Id::int(2)), "textDocument/hover", Some(r#"{"textDocument":{"uri":"file:///p/main.ts"},"position":{"line":0,"character":6}}"#));
        assert_eq!(server.response(&Id::int(2)).error.unwrap().code, -32601);
        server.send(Some(Id::int(0)), "shutdown", None);
        assert_eq!(server.response(&Id::int(0)).result.unwrap().0, b"null");
        server.send(None, "exit", None);
    }
}

#[test]
fn cancellation_of_a_queued_request_is_not_lost_while_configuration_waits() {
    let server = TestConnection::new();
    server.initialize("utf-16");
    server.send(
        Some(Id::int(10)),
        "custom/projectInfo",
        Some(r#"{"textDocument":{"uri":"file:///p/main.ts"}}"#),
    );
    server.send(None, "$/cancelRequest", Some(r#"{"id":10}"#));
    let response = server.response(&Id::int(10));
    assert_eq!(response.error.unwrap().code, -32800);
    server.send(
        Some(Id::int(11)),
        "custom/projectInfo",
        Some(r#"{"textDocument":{"uri":"file:///p/main.ts"}}"#),
    );
    assert!(server.response(&Id::int(11)).error.is_none());
}

#[test]
fn output_queue_does_not_need_a_running_writer() {
    let server = TestConnection::new();
    server.send(
        Some(Id::int(1)),
        "initialize",
        Some(r#"{"processId":null,"rootUri":null,"capabilities":{}}"#),
    );
    // The transport's reader remains available even if no output is consumed.
    server.send(
        Some(Id::int(2)),
        "initialize",
        Some(r#"{"processId":null,"rootUri":null,"capabilities":{}}"#),
    );
    assert!(server.response(&Id::int(1)).error.is_none());
    assert_eq!(server.response(&Id::int(2)).error.unwrap().code, -32600);
}

#[test]
fn malformed_json_does_not_retire_the_connection() {
    let server = TestConnection::new();
    server
        .input
        .receive_bytes(br#"{"jsonrpc":"2.0", "id":1, "method":"initialize","params":]"#)
        .unwrap();
    let raw = server.receive.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut error = Message::default();
    tsr_json::unmarshal(&raw.0, &mut error, tsr_json::Options::default()).unwrap();
    assert_eq!(error.error.unwrap().code, -32600);
    assert!(error.id.is_none());
    server.initialize("utf-16");
    server.send(Some(Id::int(1)), "shutdown", None);
    assert!(server.response(&Id::int(1)).error.is_none());
    server.send(None, "exit", None);
}
