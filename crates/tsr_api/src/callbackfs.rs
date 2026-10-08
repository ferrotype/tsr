//! A file system whose selected operations are answered by the client over
//! the connection, so the pinned client's virtual file system can stand in
//! for the disk. The callbacks are chosen per connection by `--callbacks`;
//! everything else goes to the base file system.
//! port: tsc/internal/api/callbackfs.go
use std::sync::{Arc, Mutex, Weak};
use tsr_ipc::{Conn, Context};
use tsr_json::{Encode, Encoder, RawValue};
use tsr_jsstring::JsString;
use tsr_vfs::{Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId};

pub const CALLBACK_NAMES: [&str; 6] = [
    "readFile",
    "fileExists",
    "directoryExists",
    "getAccessibleEntries",
    "realpath",
    "writeFile",
];

/// port: tsc/internal/api/callbackfs.go:isCallbackName
#[must_use]
pub fn is_callback_name(name: &str) -> bool {
    CALLBACK_NAMES.contains(&name)
}

/// The connection is held weakly: it owns the session that owns this file
/// system, and a strong reference here would keep a finished connection,
/// its session and transport alive.
struct Connected {
    conn: Weak<dyn Conn>,
    ctx: Context,
}

fn detailed(text: impl Into<String>) -> Error {
    Error::from(tsr_vfs::iofs::IoError::message(text))
}

pub struct CallbackFs {
    base: Arc<dyn FileSystem>,
    enabled: Vec<String>,
    connected: Mutex<Option<Connected>>,
}

struct WriteFileParams<'a> {
    path: &'a [u8],
    data: &'a [u8],
}
impl Encode for WriteFileParams<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), tsr_json::Error> {
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::field(b"path", &JsString::from_bytes(self.path)),
                tsr_jsonrpc::field(b"data", &JsString::from_bytes(self.data)),
            ],
        )
    }
}

fn is_null(reply: &RawValue) -> bool {
    reply.0.is_empty() || reply.0.as_slice() == b"null"
}

/// A read callback's reply; a malformed one panics, as the pin's read
/// callbacks panic on their unmarshal errors.
fn decoded<T: tsr_json::Decode + Default>(reply: &RawValue) -> T {
    let mut value = T::default();
    tsr_json::unmarshal(&reply.0, &mut value, tsr_json::Options::default())
        .unwrap_or_else(|error| panic!("{error}"));
    value
}

impl CallbackFs {
    /// The names must be callback names; the pin panics otherwise.
    /// port: tsc/internal/api/callbackfs.go:newCallbackFS
    pub fn new(base: Arc<dyn FileSystem>, callbacks: &[String]) -> Result<Self, String> {
        for name in callbacks {
            if !is_callback_name(name) {
                return Err(format!("unknown callback name: {name}"));
            }
        }
        Ok(Self {
            base,
            enabled: callbacks.to_vec(),
            connected: Mutex::new(None),
        })
    }

    /// port: tsc/internal/api/callbackfs.go:callbackFS.SetConnection
    pub fn set_connection(&self, ctx: Context, conn: &Arc<dyn Conn>) {
        *self.connected.lock().expect("callback connection") = Some(Connected {
            conn: Arc::downgrade(conn),
            ctx,
        });
    }

    fn is_enabled(&self, name: &str) -> bool {
        self.enabled.iter().any(|enabled| enabled == name)
    }

    /// A callback's reply; the pin panics when no connection is set yet or the
    /// call fails, and so does this.
    /// port: tsc/internal/api/callbackfs.go:callbackFS.call
    fn call(&self, name: &str, params: &dyn Encode) -> RawValue {
        let (conn, ctx) = self
            .connection(name)
            .unwrap_or_else(|error| panic!("{error}"));
        match conn.call(&ctx, name, params) {
            Ok(reply) => reply,
            Err(error) => panic!("{error}"),
        }
    }

    /// The connection and context of `set_connection`; the lock is not held
    /// across the call, so callers serialize on the connection itself. A
    /// connection that has ended counts as unset.
    /// port: tsc/internal/api/callbackfs.go:callbackFS.call
    fn connection(&self, name: &str) -> Result<(Arc<dyn Conn>, Context), Error> {
        let connected = self.connected.lock().expect("callback connection");
        let Some(connected) = connected.as_ref() else {
            return Err(detailed(format!(
                "CallbackFS: {name} called before connection set"
            )));
        };
        // A connection that has ended reports the pin's closed-connection
        // error, which its call would have returned.
        connected
            .conn
            .upgrade()
            .map(|conn| (conn, connected.ctx.clone()))
            .ok_or_else(|| detailed("ipc: connection closed"))
    }

    fn path_call(&self, name: &str, path: &[u8]) -> RawValue {
        self.call(name, &JsString::from_bytes(path))
    }
}

#[derive(Default)]
struct ContentReply {
    content: Option<String>,
}
impl tsr_json::Decode for ContentReply {
    fn decode(&mut self, input: &mut tsr_json::Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| {
            if name == b"content" {
                input.value(&mut self.content)
            } else {
                input.skip_value()
            }
        })
    }
}

#[derive(Default)]
struct EntriesReply {
    files: Vec<JsString>,
    directories: Vec<JsString>,
}
impl tsr_json::Decode for EntriesReply {
    fn decode(&mut self, input: &mut tsr_json::Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|name, input| match name {
            b"files" => input.value(&mut self.files),
            b"directories" => input.value(&mut self.directories),
            _ => input.skip_value(),
        })
    }
}

impl FileSystem for CallbackFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.base.use_case_sensitive_file_names()
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.base.snapshot_id()
    }
    /// Content, not found, or (an empty or null reply) fall through to the base.
    /// port: tsc/internal/api/callbackfs.go:callbackFS.ReadFile
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        if self.is_enabled("readFile") {
            let reply = self.path_call("readFile", path);
            if !is_null(&reply) {
                let wrapper: ContentReply = decoded(&reply);
                return Ok(wrapper
                    .content
                    .map(|content| FileContent::physical(content.into_bytes())));
            }
        }
        self.base.read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.base.stat(path)
    }
    /// port: tsc/internal/api/callbackfs.go:callbackFS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        if self.is_enabled("fileExists") {
            let reply = self.path_call("fileExists", path);
            if !is_null(&reply) {
                return Ok(decoded::<bool>(&reply));
            }
        }
        self.base.file_exists(path)
    }
    /// port: tsc/internal/api/callbackfs.go:callbackFS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        if self.is_enabled("directoryExists") {
            let reply = self.path_call("directoryExists", path);
            if !is_null(&reply) {
                return Ok(decoded::<bool>(&reply));
            }
        }
        self.base.directory_exists(path)
    }
    /// port: tsc/internal/api/callbackfs.go:callbackFS.GetAccessibleEntries
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        if self.is_enabled("getAccessibleEntries") {
            let reply = self.path_call("getAccessibleEntries", path);
            if !reply.0.is_empty() && reply.0.as_slice() != b"null" {
                let entries: EntriesReply = decoded(&reply);
                return Ok(Entries {
                    files: Some(entries.files),
                    directories: Some(entries.directories),
                    symlinks: None,
                });
            }
        }
        self.base.entries(path)
    }
    /// port: tsc/internal/api/callbackfs.go:callbackFS.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        if self.is_enabled("realpath") {
            let reply = self.path_call("realpath", path);
            if !is_null(&reply) {
                return Ok(decoded(&reply));
            }
        }
        self.base.realpath(path)
    }
    /// port: tsc/internal/api/callbackfs.go:callbackFS.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        if self.is_enabled("writeFile") {
            // The write side returns its errors where the read callbacks
            // panic, as the pin's does.
            let (conn, ctx) = self.connection("writeFile")?;
            return conn
                .call(&ctx, "writeFile", &WriteFileParams { path, data })
                .map(|_| ())
                .map_err(|error| detailed(error.to_string()));
        }
        self.base.write_file(path, data)
    }
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.base.append_file(path, data)
    }
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        self.base.remove(path)
    }
    fn change_times(
        &self,
        path: &[u8],
        a_time: tsr_vfs::iofs::Time,
        m_time: tsr_vfs::iofs::Time,
    ) -> Result<(), Error> {
        self.base.change_times(path, a_time, m_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A connection that answers callbacks from a table and records calls.
    struct Table {
        replies: HashMap<&'static str, &'static str>,
        calls: Mutex<Vec<(String, String)>>,
    }
    impl Conn for Table {
        fn run(&self, _: &Context) -> Result<(), tsr_ipc::Error> {
            Ok(())
        }
        fn call(
            &self,
            _: &Context,
            method: &str,
            params: &dyn Encode,
        ) -> Result<RawValue, tsr_ipc::Error> {
            let params =
                String::from_utf8(tsr_json::marshal(params, tsr_json::Options::default()).unwrap())
                    .unwrap();
            self.calls.lock().unwrap().push((method.into(), params));
            Ok(RawValue(self.replies[method].as_bytes().to_vec()))
        }
        fn notify(&self, _: &Context, _: &str, _: &dyn Encode) -> Result<(), tsr_ipc::Error> {
            Ok(())
        }
    }

    /// The returned table is the connection: the file system holds it
    /// weakly, as the production connection outlives its callbacks, so a
    /// test keeps it alive for as long as it calls back.
    fn connected(
        replies: HashMap<&'static str, &'static str>,
        enabled: &[&str],
    ) -> (CallbackFs, Arc<Table>) {
        let mut base = tsr_vfs::MemoryBuilder::new(b"/", true);
        base.insert_loaded(b"/disk.ts", &b"on disk"[..]);
        let table = Arc::new(Table {
            replies,
            calls: Mutex::new(Vec::new()),
        });
        let fs = CallbackFs::new(
            Arc::new(base.finish()),
            &enabled
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let conn: Arc<dyn Conn> = table.clone();
        fs.set_connection(Context::background(), &conn);
        (fs, table)
    }

    #[test]
    fn read_file_has_three_outcomes() {
        let (fs, _conn) = connected(
            HashMap::from([("readFile", r#"{"content":"virtual"}"#)]),
            &["readFile"],
        );
        assert_eq!(
            fs.read_file(b"/a.ts").unwrap().unwrap().raw.as_ref(),
            b"virtual"
        );
        let (fs, _conn) = connected(
            HashMap::from([("readFile", r#"{"content":null}"#)]),
            &["readFile"],
        );
        assert!(
            fs.read_file(b"/disk.ts").unwrap().is_none(),
            "null content blocks the fallback"
        );
        let (fs, table) = connected(HashMap::from([("readFile", "")]), &["readFile"]);
        assert_eq!(
            fs.read_file(b"/disk.ts").unwrap().unwrap().raw.as_ref(),
            b"on disk"
        );
        assert_eq!(
            table.calls.lock().unwrap()[0],
            ("readFile".into(), "\"/disk.ts\"".into())
        );
        let (fs, table) = connected(HashMap::new(), &[]);
        assert_eq!(
            fs.read_file(b"/disk.ts").unwrap().unwrap().raw.as_ref(),
            b"on disk"
        );
        assert!(
            table.calls.lock().unwrap().is_empty(),
            "disabled callbacks are never called"
        );
    }

    #[test]
    fn the_other_callbacks_follow_the_pin() {
        let replies = HashMap::from([
            ("fileExists", "true"),
            ("directoryExists", "false"),
            (
                "getAccessibleEntries",
                r#"{"files":["a.ts"],"directories":["sub"]}"#,
            ),
            ("realpath", "\"/real/a.ts\""),
            ("writeFile", "\"\""),
        ]);
        let (fs, table) = connected(
            replies,
            &CALLBACK_NAMES
                .map(str::to_owned)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert!(fs.file_exists(b"/x").unwrap());
        assert!(!fs.directory_exists(b"/x").unwrap());
        let entries = fs.entries(b"/").unwrap();
        assert_eq!(entries.files.unwrap()[0].as_bytes(), b"a.ts");
        assert_eq!(entries.directories.unwrap()[0].as_bytes(), b"sub");
        assert_eq!(fs.realpath(b"/x").unwrap().as_bytes(), b"/real/a.ts");
        fs.write_file(b"/out.js", b"text").unwrap();
        let calls = table.calls.lock().unwrap();
        assert_eq!(
            calls.last().unwrap(),
            &(
                "writeFile".to_string(),
                r#"{"path":"/out.js","data":"text"}"#.to_string()
            )
        );
        assert!(CallbackFs::new(
            Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish()),
            &["nope".into()]
        )
        .is_err());
    }
}
