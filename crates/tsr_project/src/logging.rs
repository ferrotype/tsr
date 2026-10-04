//! Project logging. The native entry supplies local wall-clock formatting;
//! tests supply a fixed stamp. Neither clock lookup nor formatting runs under
//! a project/session lock.
use std::{
    fmt,
    io::Write,
    sync::{Arc, Mutex},
};

pub type Timestamp = Arc<dyn Fn() -> String + Send + Sync>;

struct Output {
    writer: Box<dyn Write + Send>,
    verbose: bool,
}
struct Inner {
    output: Mutex<Output>,
    timestamp: Timestamp,
}
pub trait LogSink: Send + Sync {
    fn log(&self, message: &str);
    fn set_verbose(&self, value: bool);
    fn is_verbose(&self) -> bool;
}
#[derive(Clone, Default)]
pub struct Logger(Option<Arc<dyn LogSink>>);
impl Logger {
    // port: tsc/internal/project/logging/logger.go:NewLogger
    pub fn new(writer: impl Write + Send + 'static, timestamp: Timestamp) -> Self {
        Self(Some(Arc::new(Inner {
            output: Mutex::new(Output {
                writer: Box::new(writer),
                verbose: false,
            }),
            timestamp,
        })))
    }
    // port: tsc/internal/project/logging/logger.go:NewNopLogger
    pub fn nop() -> Self {
        Self::default()
    }
    /// An LSP session routes project logs through its connection logger, so
    /// changes in client verbosity take effect without replacing the session.
    pub fn from_sink(sink: Arc<dyn LogSink>) -> Self {
        Self(Some(sink))
    }
    // port: tsc/internal/project/logging/logger.go:logger.Logf
    pub fn log(&self, message: fmt::Arguments<'_>) {
        if let Some(inner) = &self.0 {
            inner.log(&message.to_string());
        }
    }
    // port: tsc/internal/project/logging/logger.go:logger.SetVerbose
    pub fn set_verbose(&self, value: bool) {
        if let Some(inner) = &self.0 {
            inner.set_verbose(value);
        }
    }
    // port: tsc/internal/project/logging/logger.go:logger.IsVerbose
    pub fn is_verbose(&self) -> bool {
        self.0.as_ref().is_some_and(|inner| inner.is_verbose())
    }
    // port: tsc/internal/project/logging/logger.go:logger.Verbose
    #[must_use]
    pub fn verbose(&self) -> Self {
        if self.is_verbose() {
            self.clone()
        } else {
            Self::nop()
        }
    }
}
impl LogSink for Inner {
    fn log(&self, message: &str) {
        let timestamp = (self.timestamp)();
        // Like the pin, logging must not fail the operation on a bad sink.
        let _ = writeln!(self.output.lock().unwrap().writer, "{timestamp} {message}");
    }
    fn set_verbose(&self, value: bool) {
        self.output.lock().unwrap().verbose = value;
    }
    fn is_verbose(&self) -> bool {
        self.output.lock().unwrap().verbose
    }
}

struct Entry {
    timestamp: String,
    message: String,
    child: Option<usize>,
}
#[derive(Default)]
struct Node {
    verbose: bool,
    logs: Vec<Entry>,
}
struct Tree {
    name: String,
    nodes: Mutex<Vec<Node>>,
    timestamp: Timestamp,
}
/// Forks retain one indexed tree, with no parent/child Arc cycle. Each branch
/// can collect concurrently; formatting takes a consistent snapshot of it.
pub struct LogTree {
    tree: Arc<Tree>,
    node: usize,
}
impl LogTree {
    // port: tsc/internal/project/logging/logtree.go:NewLogTree
    pub fn new(name: String, timestamp: Timestamp) -> Self {
        Self {
            tree: Arc::new(Tree {
                name,
                nodes: Mutex::new(vec![Node::default()]),
                timestamp,
            }),
            node: 0,
        }
    }
    // port: tsc/internal/project/logging/logtree.go:LogTree.Logf
    pub fn log(&self, message: fmt::Arguments<'_>) {
        let entry = Entry {
            timestamp: (self.tree.timestamp)(),
            message: message.to_string(),
            child: None,
        };
        self.tree.nodes.lock().unwrap()[self.node].logs.push(entry);
    }
    // port: tsc/internal/project/logging/logtree.go:LogTree.Fork
    #[must_use]
    pub fn fork(&self, message: String) -> Self {
        let timestamp = (self.tree.timestamp)();
        let mut nodes = self.tree.nodes.lock().unwrap();
        let node = nodes.len();
        let verbose = nodes[self.node].verbose;
        nodes.push(Node {
            verbose,
            ..Default::default()
        });
        nodes[self.node].logs.push(Entry {
            timestamp,
            message,
            child: Some(node),
        });
        Self {
            tree: self.tree.clone(),
            node,
        }
    }
    // port: tsc/internal/project/logging/logtree.go:LogTree.SetVerbose
    pub fn set_verbose(&self, value: bool) {
        self.tree.nodes.lock().unwrap()[self.node].verbose = value;
    }
    // port: tsc/internal/project/logging/logtree.go:LogTree.IsVerbose
    pub fn is_verbose(&self) -> bool {
        self.tree.nodes.lock().unwrap()[self.node].verbose
    }
    // port: tsc/internal/project/logging/logtree.go:LogTree.Verbose
    pub fn verbose(&self) -> Option<&Self> {
        self.is_verbose().then_some(self)
    }
}
impl fmt::Display for LogTree {
    // port: tsc/internal/project/logging/logtree.go:LogTree.String
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        assert_eq!(self.node, 0, "can only call String on root LogTree");
        writeln!(f, "======== {} ========", self.tree.name)?;
        let nodes = self.tree.nodes.lock().unwrap();
        let mut stack = vec![(0, 0, 0)];
        while let Some((node, entry, level)) = stack.pop() {
            let Some(log) = nodes[node].logs.get(entry) else {
                continue;
            };
            stack.push((node, entry + 1, level));
            for _ in 0..level {
                f.write_str("\t")?;
            }
            writeln!(f, "{} {}", log.timestamp, log.message)?;
            if let Some(child) = log.child {
                stack.push((child, 0, level + 1));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn log_sinks_and_forks_keep_native_format_and_verbose_inheritance() {
        let stamp: Timestamp = Arc::new(|| "[10:11:12.345]".into());
        let buffer = Buffer::default();
        let logger = Logger::new(buffer.clone(), stamp.clone());
        logger.verbose().log(format_args!("hidden"));
        logger.set_verbose(true);
        logger.verbose().log(format_args!("visible {}", 2));
        assert_eq!(*buffer.0.lock().unwrap(), b"[10:11:12.345] visible 2\n");
        let tree = LogTree::new("snapshot".into(), stamp);
        tree.set_verbose(true);
        tree.log(format_args!("before"));
        let child = tree.fork("project".into());
        assert!(child.is_verbose());
        tree.set_verbose(false);
        child.verbose().unwrap().log(format_args!("parse"));
        tree.log(format_args!("after"));
        assert_eq!(tree.to_string(), "======== snapshot ========\n[10:11:12.345] before\n[10:11:12.345] project\n\t[10:11:12.345] parse\n[10:11:12.345] after\n");
        let weak = Arc::downgrade(&tree.tree);
        drop(tree);
        assert!(weak.upgrade().is_some());
        drop(child);
        assert!(weak.upgrade().is_none());
    }
}
