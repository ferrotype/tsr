//! Synchronous VFS callbacks for compiler workers. The connection owner pumps
//! replies and outgoing frames; a worker only waits on its own one-shot slot.
//! Retiring a router settles every slot before an embedding joins its workers.
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Sender},
    Arc, Mutex,
};

use serde_json::{value::RawValue, Value};
use tsr_vfs::{Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId};

use crate::{
    filesystem::Host,
    protocol::fits,
    wire::{wire, Json},
};

/// Share one allocator with the test-host session and its worker router. Reset
/// retains this allocator so late replies cannot address a later request.
#[derive(Clone, Default)]
pub struct CallbackIds(Arc<AtomicU64>);
impl CallbackIds {
    pub(crate) fn next(&self) -> Option<String> {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .ok()
            .map(|id| format!("callback:{}", id + 1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeError {
    Canceled,
    Disconnected,
    Retired,
    TooManyPending,
    InvalidReply,
}
impl From<BridgeError> for Error {
    fn from(error: BridgeError) -> Self {
        Self::Io(match error {
            BridgeError::Canceled => std::io::ErrorKind::Interrupted,
            BridgeError::Disconnected => std::io::ErrorKind::BrokenPipe,
            BridgeError::Retired => std::io::ErrorKind::ConnectionAborted,
            BridgeError::TooManyPending => std::io::ErrorKind::WouldBlock,
            BridgeError::InvalidReply => std::io::ErrorKind::InvalidData,
        })
    }
}
struct Pending {
    request: Arc<AtomicBool>,
    reply: Sender<Result<Value, BridgeError>>,
}
#[derive(Default)]
struct State {
    stopped: Option<BridgeError>,
    pending: BTreeMap<String, Pending>,
}
struct Shared {
    ids: CallbackIds,
    outgoing: Sender<Json>,
    state: Mutex<State>,
}

/// Owned by the input router, never by a worker. Dropping it disconnects all
/// workers; workers cannot keep the connection alive through their filesystem.
pub struct CallbackRouter {
    shared: Arc<Shared>,
    host: Arc<Host>,
}
impl CallbackRouter {
    pub(crate) fn new(ids: CallbackIds, outgoing: Sender<Box<RawValue>>, host: Arc<Host>) -> Self {
        Self {
            shared: Arc::new(Shared {
                ids,
                outgoing,
                state: Mutex::new(State::default()),
            }),
            host,
        }
    }
    pub fn filesystem(&self) -> (CallbackFs, CancelCalls) {
        let request = Arc::new(AtomicBool::new(false));
        (
            CallbackFs {
                shared: self.shared.clone(),
                host: self.host.clone(),
                request: request.clone(),
            },
            CancelCalls {
                shared: self.shared.clone(),
                request,
            },
        )
    }
    /// A response already validated by the connection's JSON-RPC decoder.
    /// False means it was canceled/retired or is not this router's callback.
    /// The caller must route S11's own pending callbacks before calling this.
    pub fn complete(&self, id: &str, result: Result<Value, BridgeError>) -> bool {
        let pending = self
            .shared
            .state
            .lock()
            .expect("callback state poisoned")
            .pending
            .remove(id);
        if let Some(pending) = pending {
            let _ = pending.reply.send(result);
            true
        } else {
            false
        }
    }
    pub fn pending_count(&self) -> usize {
        self.shared
            .state
            .lock()
            .expect("callback state poisoned")
            .pending
            .len()
    }
    /// Stop admissions and wake all parked workers. This never waits for a
    /// worker, writes a frame synchronously, or invokes caller code under a lock.
    pub fn retire(&self) {
        self.shared.stop(BridgeError::Retired);
    }
}
impl Drop for CallbackRouter {
    fn drop(&mut self) {
        self.shared.stop(BridgeError::Disconnected);
    }
}
impl Shared {
    fn stop(&self, error: BridgeError) {
        let pending = {
            let mut state = self.state.lock().expect("callback state poisoned");
            state.stopped.get_or_insert(error);
            std::mem::take(&mut state.pending)
        };
        for (id, pending) in pending {
            let _ = self.outgoing.send(
                wire!({"jsonrpc":"2.0", "method":"$/cancelRequest", "params":wire!({"id":id})}),
            );
            let _ = pending.reply.send(Err(error));
        }
    }
}

/// Explicit cancellation of all filesystem calls belonging to one request.
/// A request may use several workers. Their slots settle once; late replies
/// are ignored without retaining a growing tombstone map.
pub struct CancelCalls {
    shared: Arc<Shared>,
    request: Arc<AtomicBool>,
}
impl CancelCalls {
    pub fn is_canceled(&self) -> bool {
        self.request.load(Ordering::Relaxed)
    }
    pub fn owns(&self, id: &str) -> bool {
        self.shared
            .state
            .lock()
            .expect("callback state poisoned")
            .pending
            .get(id)
            .is_some_and(|pending| Arc::ptr_eq(&pending.request, &self.request))
    }
    pub fn cancel(&self) {
        let mut state = self.shared.state.lock().expect("callback state poisoned");
        self.request.store(true, Ordering::Relaxed);
        state.pending.retain(|id, pending| {
            if !Arc::ptr_eq(&pending.request, &self.request) {
                return true;
            }
            let _ = self.shared.outgoing.send(
                wire!({"jsonrpc":"2.0", "method":"$/cancelRequest", "params":wire!({"id":id})}),
            );
            let _ = pending.reply.send(Err(BridgeError::Canceled));
            false
        });
    }
}

#[derive(Clone)]
pub struct CallbackFs {
    shared: Arc<Shared>,
    host: Arc<Host>,
    request: Arc<AtomicBool>,
}
impl CallbackFs {
    fn call(&self, operation: &str, path: &[u8]) -> Result<Value, Error> {
        let path = std::str::from_utf8(path).map_err(|_| Error::InvalidPath)?;
        let receiver = {
            let mut state = self.shared.state.lock().expect("callback state poisoned");
            if let Some(error) = state.stopped {
                return Err(error.into());
            }
            if self.request.load(Ordering::Relaxed) {
                return Err(BridgeError::Canceled.into());
            }
            if !self.host.enabled(operation) {
                drop(state);
                return self
                    .host
                    .complete(operation, path, Value::Null)
                    .map_err(|_| BridgeError::InvalidReply.into());
            }
            if state.pending.len() >= 64 {
                return Err(BridgeError::TooManyPending.into());
            }
            let id = self
                .shared
                .ids
                .next()
                .ok_or_else(|| Error::from(BridgeError::Retired))?;
            let frame = wire!({"jsonrpc":"2.0", "id":id, "method":operation, "params":path});
            if !fits(&frame) {
                return Err(BridgeError::InvalidReply.into());
            }
            let (reply, receiver) = mpsc::channel();
            state.pending.insert(
                id.clone(),
                Pending {
                    request: self.request.clone(),
                    reply,
                },
            );
            // Unbounded mpsc send only queues a frame. Holding the short state
            // lock orders admission before a racing cancellation notification.
            if self.shared.outgoing.send(frame).is_err() {
                state.pending.remove(&id);
                drop(state);
                self.shared.stop(BridgeError::Disconnected);
                return Err(BridgeError::Disconnected.into());
            }
            receiver
        };
        // No connection or filesystem lock is held during this wait.
        let result = receiver
            .recv()
            .map_err(|_| Error::from(BridgeError::Disconnected))??;
        self.host
            .complete(operation, path, result)
            .map_err(|_| BridgeError::InvalidReply.into())
    }
}
impl FileSystem for CallbackFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.host.case_sensitive
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        let value = self.call("readFile", path)?;
        Ok(value["content"]
            .as_str()
            .map(|content| FileContent::loaded(Arc::<[u8]>::from(content.as_bytes()))))
    }
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(self
            .call("fileExists", path)?
            .as_bool()
            .expect("validated bool"))
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(self
            .call("directoryExists", path)?
            .as_bool()
            .expect("validated bool"))
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        if self.file_exists(path)? {
            Ok(Some(FileInfo::basic(false, 0)))
        } else if self.directory_exists(path)? {
            Ok(Some(FileInfo::basic(true, 0)))
        } else {
            Ok(None)
        }
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        let value = self.call("getAccessibleEntries", path)?;
        let names = |key: &str| {
            value[key]
                .as_array()
                .expect("validated entries")
                .iter()
                .map(|name| {
                    tsr_jsstring::JsString::from_bytes(
                        name.as_str().expect("validated name").as_bytes(),
                    )
                })
                .collect()
        };
        Ok(Entries {
            files: Some(names("files")),
            directories: Some(names("directories")),
            symlinks: None,
        })
    }
    fn realpath(&self, path: &[u8]) -> Result<tsr_jsstring::JsString, Error> {
        Ok(tsr_jsstring::JsString::from_bytes(
            self.call("realpath", path)?
                .as_str()
                .expect("validated path")
                .as_bytes(),
        ))
    }
}

#[cfg(test)]
mod tests;
