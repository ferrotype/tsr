use super::{WatchRegistry, WatchedFiles, Watcher};
use crate::{
    background::Queue,
    clock::{self, Clock},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tsr_core::CancellationToken;
use tsr_ipc::Context;
use tsr_jsstring::JsString;

/// The language-server client supplies registration. Its calls must observe the
/// supplied cancellation scope; the router continues receiving while they wait.
pub trait WatchClient: Send + Sync {
    fn watch_files(
        &self,
        context: &Context,
        id: &JsString,
        watcher: &Watcher,
    ) -> Result<(), String>;
    fn unwatch_files(&self, context: &Context, id: &JsString) -> Result<(), String>;
}
pub type WatchSet = BTreeMap<JsString, Arc<WatchedFiles>>;
pub struct WatchManager {
    registry: WatchRegistry,
    client: Arc<dyn WatchClient>,
    clock: Arc<dyn Clock>,
    context: Context,
    token: CancellationToken,
    queue: Queue,
    errors: Mutex<Vec<String>>,
}
impl WatchManager {
    pub fn new(client: Arc<dyn WatchClient>) -> Arc<Self> {
        Self::with_clock(client, clock::system())
    }
    pub(crate) fn with_clock(client: Arc<dyn WatchClient>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            registry: WatchRegistry::default(),
            client,
            clock,
            context: Context::background().with_cancel(),
            token: CancellationToken::new(),
            queue: Queue::new(),
            errors: Mutex::default(),
        })
    }
    pub fn enqueue(self: &Arc<Self>, old: WatchSet, new: WatchSet) {
        let manager = self.clone();
        self.queue.enqueue(self.token.clone(), move |_| {
            let errors = manager.update(&old, &new);
            manager.errors.lock().unwrap().extend(errors);
        });
    }
    pub fn wait(&self) {
        self.queue.wait();
    }
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.lock().unwrap())
    }
    pub fn close(&self) {
        self.stop();
        self.queue.wait();
    }
    pub(crate) fn stop(&self) {
        self.token.cancel();
        self.context.cancel();
        self.queue.close();
    }
    fn call(&self, action: impl FnOnce(&Context) -> Result<(), String>) -> Result<(), String> {
        let context = self.context.with_cancel();
        let expiry = context.clone();
        let timer = self
            .clock
            .after(Duration::from_secs(1), Box::new(move || expiry.expire()));
        let result = action(&context);
        drop(timer);
        context.cancel();
        result
    }
    // port: tsc/internal/project/session.go:Session.updateWatches
    fn update(&self, old: &WatchSet, new: &WatchSet) -> Vec<String> {
        let mut errors = Vec::new();
        for (key, new) in new {
            let previous = old.get(key);
            if previous.is_none_or(|old| old.id() != new.id()) {
                errors.extend(self.update_watch(previous.map(AsRef::as_ref), Some(new)));
            } else if self.registry.is_pending(new.id()) {
                errors.extend(self.update_watch(None, Some(new)));
            }
        }
        for (key, old) in old {
            if !new.contains_key(key) {
                errors.extend(self.update_watch(Some(old), None));
            }
        }
        errors
    }
    // port: tsc/internal/project/session.go:updateWatch
    fn update_watch(&self, old: Option<&WatchedFiles>, new: Option<&WatchedFiles>) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some(new) = new {
            let watchers = new.watchers();
            let mut acquired = Vec::new();
            for (index, watcher) in watchers.iter().enumerate() {
                let id = JsString::from_bytes(
                    [watchers.id.as_bytes(), b".", index.to_string().as_bytes()].concat(),
                );
                if self.registry.acquire(watcher, id.clone()) {
                    acquired.push((id, watcher));
                }
            }
            for (id, watcher) in &acquired {
                if let Err(error) =
                    self.call(|context| self.client.watch_files(context, id, watcher))
                {
                    errors.push(error);
                }
            }
            if errors.is_empty() {
                self.registry.clear_pending(&watchers.id);
            } else {
                // Roll back all newly acquired registrations, including the
                // successful calls. The next snapshot retries the same IDs.
                for (_, watcher) in acquired {
                    self.registry.release(watcher);
                }
                self.registry.mark_pending(&watchers.id);
            }
        }
        if let Some(old) = old {
            let removed: Vec<_> = old
                .watchers()
                .iter()
                .filter_map(|w| self.registry.release(w))
                .collect();
            for id in removed {
                if let Err(error) = self.call(|context| self.client.unwatch_files(context, &id)) {
                    errors.push(error);
                }
            }
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clock::manual::ManualClock,
        watch::{PatternsAndIgnored, ALL_CHANGES},
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    struct Client {
        fail: AtomicBool,
        started: mpsc::Sender<JsString>,
        successes: Mutex<Vec<JsString>>,
    }
    impl WatchClient for Client {
        fn watch_files(&self, context: &Context, id: &JsString, _: &Watcher) -> Result<(), String> {
            if self.fail.load(Ordering::Acquire) {
                let (send, recv) = mpsc::sync_channel(1);
                let stop = context.after_func(move || {
                    let _ = send.send(());
                });
                self.started.send(id.clone()).unwrap();
                recv.recv().unwrap();
                stop.stop();
                return Err(context.err().unwrap().to_string());
            }
            self.started.send(id.clone()).unwrap();
            self.successes.lock().unwrap().push(id.clone());
            Ok(())
        }
        fn unwatch_files(&self, _: &Context, _: &JsString) -> Result<(), String> {
            Ok(())
        }
    }
    // source: tsc/internal/project/watchtimeout_test.go:TestUpdateWatchTimeoutAndRollback
    #[test]
    fn timeouts_roll_back_and_an_unchanged_snapshot_retries_the_exact_ids() {
        let (send, recv) = mpsc::channel();
        let client = Arc::new(Client {
            fail: AtomicBool::new(true),
            started: send,
            successes: Mutex::default(),
        });
        let clock = ManualClock::new();
        let manager = WatchManager::with_clock(client.clone(), clock.clone());
        let watchers = WatchedFiles::new(
            JsString::from_bytes(b"root files".as_slice()),
            ALL_CHANGES,
            false,
        )
        .with_input(PatternsAndIgnored {
            patterns_inside_workspace: vec![
                JsString::from_bytes(b"/src/**/*".as_slice()),
                JsString::from_bytes(b"/extra/**/*".as_slice()),
            ],
            ..Default::default()
        });
        let key = JsString::from_bytes(b"config".as_slice());
        let new = WatchSet::from([(key, watchers.clone())]);
        manager.enqueue(WatchSet::new(), new.clone());
        let first = recv.recv_timeout(Duration::from_secs(5)).unwrap();
        clock.advance(Duration::from_secs(1));
        let second = recv.recv_timeout(Duration::from_secs(5)).unwrap();
        // Each call gets its own fresh deadline, rather than sharing a batch deadline.
        clock.advance(Duration::from_secs(1));
        manager.wait();
        assert_eq!(manager.take_errors(), vec!["context deadline exceeded"; 2]);
        assert!(manager.registry.is_pending(watchers.id()));
        client.fail.store(false, Ordering::Release);
        manager.enqueue(new.clone(), new);
        manager.wait();
        assert_eq!(client.successes.lock().unwrap().as_slice(), [first, second]);
        assert!(!manager.registry.is_pending(watchers.id()));
        assert!(manager.take_errors().is_empty());
        manager.close();
    }
}
