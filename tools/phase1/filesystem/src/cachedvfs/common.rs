//! The native Common probe's injected I/O fixture. Its independent stat and
//! directory-entry modes are inputs, not a model of Common's classification.
use crate::fs_trace as trace;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tsr_vfs::{
    iofs::{self, FileMode, Fs, Handle, Info, IoError, Sys, Time, WalkError},
    iovfs::Common,
    Error,
};

struct Node {
    name: String,
    entry: FileMode,
    stat: Option<FileMode>,
    content: Arc<[u8]>,
}
struct Fixture {
    nodes: BTreeMap<String, Node>,
    unreadable: BTreeSet<String>,
    now: Time,
    calls: Mutex<Vec<Value>>,
}
impl Fixture {
    fn build(a: &Value) -> Result<Self, String> {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ".".into(),
            Node {
                name: ".".into(),
                entry: mode("dir")?,
                stat: Some(mode("dir")?),
                content: Arc::from([]),
            },
        );
        for row in trace::array(a, "nodes")? {
            let name = trace::text(row, "name")?.to_owned();
            let kind = trace::text(row, "stat")?;
            let node = Node {
                name: name.clone(),
                entry: mode(trace::text(row, "entry")?)?,
                stat: if kind == "missing" {
                    None
                } else {
                    Some(mode(kind)?)
                },
                content: trace::unhex(
                    row.get("content_hex").and_then(Value::as_str).unwrap_or(""),
                )?
                .into(),
            };
            nodes.insert(name, node);
        }
        let unreadable = trace::array(a, "read_dir_errors")?
            .iter()
            .map(|s| {
                s.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "bad read_dir_errors".into())
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            nodes,
            unreadable,
            now: trace::instant(a, "now")?,
            calls: Mutex::default(),
        })
    }
    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
    fn record(&self, row: Value) {
        self.calls.lock().unwrap().push(row);
    }
    fn find(&self, op: &'static str, name: &[u8]) -> Result<&Node, IoError> {
        if !iofs::valid_path(name) {
            return Err(IoError::path(op, name, IoError::Invalid));
        }
        self.nodes
            .get(std::str::from_utf8(name).unwrap())
            .filter(|node| node.stat.is_some())
            .ok_or_else(|| IoError::path(op, name, IoError::NotExist))
    }
    fn info(&self, node: &Node, mode: FileMode) -> Info {
        Info {
            name: node.name.rsplit('/').next().unwrap().as_bytes().to_vec(),
            size: node.content.len() as u64,
            mode,
            mod_time: self.now,
            sys: Sys::Nil,
        }
    }
}
fn mode(kind: &str) -> Result<FileMode, String> {
    Ok(match kind {
        "file" => FileMode(0o644),
        "dir" => FileMode::DIR | FileMode(0o755),
        "symlink" => FileMode::SYMLINK | FileMode(0o777),
        "irregular" => FileMode::IRREGULAR,
        "fifo" => FileMode(1 << 25),
        _ => return Err(format!("unknown fixture kind {kind}")),
    })
}
fn error(error: Option<&IoError>) -> &'static str {
    trace::error(error.map(|e| Error::Io(e.kind())))
}
impl Fs for Fixture {
    fn open(&self, name: &[u8]) -> Result<Handle, IoError> {
        let node = self.find("open", name);
        self.record(json!([
            "open",
            std::str::from_utf8(name).unwrap(),
            error(node.as_ref().err())
        ]));
        let node = node?;
        Ok(Handle::file(
            name,
            self.info(node, node.stat.unwrap()),
            node.content.clone(),
        ))
    }
    fn stat(&self, name: &[u8]) -> Result<Arc<Info>, IoError> {
        let node = self.find("stat", name);
        self.record(json!([
            "stat",
            std::str::from_utf8(name).unwrap(),
            error(node.as_ref().err())
        ]));
        let node = node?;
        Ok(Arc::new(self.info(node, node.stat.unwrap())))
    }
    fn read_dir(&self, name: &[u8]) -> Result<Vec<Arc<Info>>, IoError> {
        let text = std::str::from_utf8(name).unwrap();
        let result = self.find("readdir", name).and_then(|node| {
            if self.unreadable.contains(text) {
                return Err(IoError::path("readdir", name, IoError::Permission));
            }
            if !node.stat.unwrap().is_dir() {
                return Err(IoError::path("readdir", name, IoError::Invalid));
            }
            Ok(self
                .nodes
                .values()
                .filter(|n| {
                    n.name != "."
                        && n.name.rsplit_once('/').map_or(".", |(parent, _)| parent) == text
                })
                .map(|node| Arc::new(self.info(node, node.entry)))
                .collect::<Vec<_>>())
        });
        self.record(json!([
            "read_dir",
            text,
            result.as_ref().map_or(0, Vec::len),
            error(result.as_ref().err())
        ]));
        result
    }
    fn read_file(&self, name: &[u8]) -> Result<Arc<[u8]>, IoError> {
        let result = self.find("open", name).and_then(|node| {
            if node.stat.unwrap().is_dir() {
                Err(IoError::path("read", name, IoError::Invalid))
            } else {
                Ok(node.content.clone())
            }
        });
        self.record(json!([
            "read_file",
            std::str::from_utf8(name).unwrap(),
            result.as_ref().map_or(0, |b| b.len()),
            error(result.as_ref().err())
        ]));
        result
    }
}
struct State {
    common: Common,
    fixture: Arc<Fixture>,
    roots: Arc<Mutex<Vec<Value>>>,
    reparse: Arc<Mutex<Vec<Value>>>,
}
impl State {
    fn build(a: &Value) -> Result<Self, String> {
        let fixture = Arc::new(Fixture::build(a)?);
        let roots = Arc::new(Mutex::new(Vec::new()));
        let reparse = Arc::new(Mutex::new(Vec::new()));
        let served = fixture.clone();
        let calls = roots.clone();
        let root_for = Box::new(move |root: &[u8]| {
            calls
                .lock()
                .unwrap()
                .push(json!(std::str::from_utf8(root).unwrap()));
            if root == b"/" {
                Some(served.clone() as Arc<dyn Fs>)
            } else {
                None
            }
        });
        let is_reparse_point = match trace::text(a, "is_reparse_point")? {
            "nil" => None,
            value @ ("true" | "false") => {
                let answer = value == "true";
                let calls = reparse.clone();
                Some(Box::new(move |p: &[u8]| {
                    calls
                        .lock()
                        .unwrap()
                        .push(json!(std::str::from_utf8(p).unwrap()));
                    answer
                }) as tsr_vfs::iovfs::IsReparsePoint)
            }
            other => return Err(format!("invalid reparse predicate {other}")),
        };
        Ok(Self {
            common: Common {
                root_for,
                is_reparse_point,
            },
            fixture,
            roots,
            reparse,
        })
    }
}
pub fn replay(actions: &[Value]) -> Result<Vec<Value>, String> {
    let mut state: Option<State> = None;
    let mut rows = Vec::new();
    for action in actions {
        if let Some(s) = &state {
            s.roots.lock().unwrap().clear();
            s.reparse.lock().unwrap().clear();
            s.fixture.take();
        }
        let op = trace::text(action, "op")?;
        let mut row = trace::guarded(op, |row| {
            if op == "from_common" {
                state = Some(State::build(action)?);
                return Ok(());
            }
            let common = &state
                .as_ref()
                .ok_or("Common used before construction")?
                .common;
            if op == "walk" {
                let root = trace::text(action, "root")?;
                let mut visited = Vec::new();
                // Require callback controls before entering production, so malformed
                // requests cannot be mistaken for observed callback failures.
                for key in ["skip_dir_at", "skip_all_at", "fail_at"] {
                    trace::text(action, key)?;
                }
                let sentinel = IoError::message("phase1 sentinel");
                let result = common.walk_dir(root.as_bytes(), &mut |path, entry, failure| {
                    let path = std::str::from_utf8(path).unwrap();
                    let decision = super::walk_decision(action, path).unwrap();
                    visited.push(json!([
                        path,
                        entry.is_some(),
                        entry.map_or("", |e| std::str::from_utf8(&e.name).unwrap()),
                        entry.is_some_and(Info::is_dir),
                        error(failure.as_ref()),
                        decision
                    ]));
                    match decision {
                        "skip_dir" => Err(WalkError::SkipDir),
                        "skip_all" => Err(WalkError::SkipAll),
                        "sentinel" => Err(WalkError::Other(sentinel.clone())),
                        _ => Ok(()),
                    }
                });
                row["visited"] = json!(visited);
                row["error"] = json!(if result.as_ref().err() == Some(&sentinel) {
                    "sentinel"
                } else {
                    error(result.as_ref().err())
                });
                return Ok(());
            }
            let path = trace::text(action, "path")?.as_bytes();
            match op {
                "root_and_path" => {
                    let (fs, root, rest) = common.root_and_path(path);
                    row["root_name"] = json!(String::from_utf8(root).unwrap());
                    row["rest"] = json!(String::from_utf8(rest).unwrap());
                    row["fs_nil"] = json!(fs.is_none());
                }
                "stat" => {
                    row["stat"] = common.stat(path).map_or(Value::Null, |info| {
                        json!([
                            String::from_utf8(info.name.clone()).unwrap(),
                            info.size,
                            info.is_dir(),
                            if action["mod_time"] == "class" {
                                "aliased".into()
                            } else {
                                info.mod_time.format_rfc3339_nano()
                            }
                        ])
                    });
                }
                "file_exists" => row["value"] = json!(common.file_exists(path)),
                "directory_exists" => row["value"] = json!(common.directory_exists(path)),
                "entries" => trace::entries(row, common.get_accessible_entries(path)),
                "read_file" => {
                    let file = common.read_file(path);
                    row["ok"] = json!(file.is_some());
                    row["contents_hex"] = json!(trace::hex(file.as_deref().unwrap_or_default()));
                }
                _ => return Err(format!("unsupported Common action {op}")),
            }
            Ok(())
        })?;
        for key in ["path", "root"] {
            if let Some(value) = action.get(key) {
                row[key] = value.clone();
            }
        }
        row["roots"] = json!(state
            .as_ref()
            .map_or_else(Vec::new, |s| std::mem::take(&mut *s.roots.lock().unwrap())));
        row["reparse"] = json!(state.as_ref().map_or_else(Vec::new, |s| std::mem::take(
            &mut *s.reparse.lock().unwrap()
        )));
        row["fs_calls"] = json!(state.as_ref().map_or_else(Vec::new, |s| s.fixture.take()));
        rows.push(row);
    }
    Ok(rows)
}
