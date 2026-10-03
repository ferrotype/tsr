//! FSEvents streams and event classification from the pinned Darwin backend.
//!
//! Rust can enter its C callback directly, so the Go assembly-to-pipe bridge
//! is unnecessary. The callback only records events and wakes the debouncer;
//! user code never executes on the stream's serial dispatch queue.

#![deny(unsafe_code)]

use crate::watcher::{is_in_directory_or_self, os_path, Backend, DirWatch};
use crate::{lock, Error};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[allow(unsafe_code)]
mod native;
use native::Stream;

const MUST_SCAN_SUB_DIRS: u32 = 0x1;
const USER_DROPPED: u32 = 0x2;
const KERNEL_DROPPED: u32 = 0x4;
const HISTORY_DONE: u32 = 0x10;
const ITEM_CREATED: u32 = 0x100;
const ITEM_REMOVED: u32 = 0x200;
const ITEM_RENAMED: u32 = 0x800;
const IGNORED_FLAGS: u32 = 0x10000 | 0x20000 | 0x40000 | 0x100000 | 0x200000 | 0x400000;
const PATHS_PER_STREAM: usize = 512;

#[derive(Clone)]
struct WatchSnapshot {
    watch: Arc<DirWatch>,
    terminated: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    watches: Vec<WatchSnapshot>,
    streams: Vec<Stream>,
    closed: bool,
}

#[derive(Default)]
struct FsEventsBackend(Mutex<State>);

// port: tsc/internal/fswatch/fsevents_darwin.go:newFSEventsBackend
pub(crate) fn new() -> Result<Arc<dyn Backend>, Error> {
    Ok(Arc::new(FsEventsBackend::default()))
}

pub(crate) fn available() -> bool {
    true
}

pub(crate) fn canonicalize(path: &[u8]) -> Vec<u8> {
    native::canonicalize(path)
}

impl State {
    // port: tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.activeWatchesLocked
    fn active_watches(&self) -> Vec<WatchSnapshot> {
        self.watches
            .iter()
            .filter(|entry| !entry.terminated.load(Ordering::Acquire))
            .cloned()
            .collect()
    }
}

impl Backend for FsEventsBackend {
    // port: tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.subscribeMany
    fn add_many(&self, watches: &[Arc<DirWatch>]) -> Result<(), Error> {
        if watches.is_empty() {
            return Ok(());
        }
        // Validate the whole batch before touching the existing stream set.
        for watch in watches {
            let metadata = std::fs::metadata(os_path(&watch.physical_dir))?;
            if !metadata.is_dir() {
                return Err(std::io::Error::from_raw_os_error(20).into());
            }
        }
        let mut state = lock(&self.0);
        if state.closed {
            return Err(Error::Unavailable);
        }
        for watch in watches {
            state
                .watches
                .retain(|entry| !Arc::ptr_eq(&entry.watch, watch));
            state.watches.push(WatchSnapshot {
                watch: watch.clone(),
                terminated: Arc::new(AtomicBool::new(false)),
            });
        }
        let streams = match start_streams(&state.active_watches(), Stream::new) {
            Ok(streams) => streams,
            Err(error) => {
                state
                    .watches
                    .retain(|entry| !watches.iter().any(|watch| Arc::ptr_eq(&entry.watch, watch)));
                return Err(error);
            }
        };
        let old_streams = std::mem::replace(&mut state.streams, streams);
        // No callback takes this mutex. Keep mutations serialized through the
        // barrier so another caller cannot replace a partially stopped set.
        drop(old_streams);
        Ok(())
    }

    // port: tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.closeWatch
    fn remove(&self, watch: &Arc<DirWatch>) -> Result<(), Error> {
        let mut state = lock(&self.0);
        let Some(index) = state
            .watches
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.watch, watch))
        else {
            return Ok(());
        };
        let removed = state.watches.remove(index);
        removed.terminated.store(true, Ordering::Release);
        // If replacement fails, old streams remain usable for the other
        // watches; their snapshot ignores the terminated subscription.
        let streams = start_streams(&state.active_watches(), Stream::new)?;
        let old_streams = std::mem::replace(&mut state.streams, streams);
        drop(old_streams);
        Ok(())
    }

    fn shutdown(&self) {
        let mut state = lock(&self.0);
        if state.closed {
            return;
        }
        state.closed = true;
        for entry in state.watches.drain(..) {
            entry.terminated.store(true, Ordering::Release);
        }
        state.streams.clear();
    }

    fn sequence(&self) -> Option<u64> {
        Some(native::current_event_id())
    }
}

impl Drop for FsEventsBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// port: tsc/internal/fswatch/fsevents_darwin.go:startFSEventsStreams
fn start_streams<T>(
    watches: &[WatchSnapshot],
    mut start: impl FnMut(&[Vec<u8>], Vec<WatchSnapshot>) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    if watches.is_empty() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<_> = watches
        .iter()
        .map(|entry| entry.watch.physical_dir.clone())
        .collect();
    paths.sort_unstable();
    paths.dedup();
    if let Ok(stream) = start(&paths, watches.to_vec()) {
        return Ok(vec![stream]);
    }
    let mut streams = Vec::with_capacity(paths.len().div_ceil(PATHS_PER_STREAM));
    for chunk in paths.chunks(PATHS_PER_STREAM) {
        let subset = watches_for_paths(watches, chunk);
        // Already started chunks are dropped, and therefore drained, on any
        // later failure. Even a small failed shared stream is retried once.
        streams.push(start(chunk, subset)?);
    }
    Ok(streams)
}

// port: tsc/internal/fswatch/fsevents_darwin.go:watchesForFSEventsPaths
fn watches_for_paths(watches: &[WatchSnapshot], paths: &[Vec<u8>]) -> Vec<WatchSnapshot> {
    watches
        .iter()
        .filter(|entry| paths.binary_search(&entry.watch.physical_dir).is_ok())
        .cloned()
        .collect()
}

// port: tsc/internal/fswatch/fsevents_darwin.go:fseventsDisplayPath
fn display_path(watch: &DirWatch, raw_path: &[u8]) -> Option<Vec<u8>> {
    if is_in_directory_or_self(&watch.physical_dir, raw_path) {
        Some(watch.display_path(raw_path))
    } else if watch.physical_dir != watch.dir && is_in_directory_or_self(&watch.dir, raw_path) {
        Some(raw_path.to_vec())
    } else {
        None
    }
}

// port: tsc/internal/fswatch/fsevents_darwin.go:fseventsOverflowMatches
fn overflow_matches(watch: &DirWatch, raw_path: &[u8]) -> bool {
    is_in_directory_or_self(&watch.physical_dir, raw_path)
        || is_in_directory_or_self(raw_path, &watch.physical_dir)
        || (watch.physical_dir != watch.dir
            && (is_in_directory_or_self(&watch.dir, raw_path)
                || is_in_directory_or_self(raw_path, &watch.dir)))
}

// port: tsc/internal/fswatch/fsevents_darwin.go:fsEventsCallback
fn process_events(
    watches: &[WatchSnapshot],
    events: impl IntoIterator<Item = (Vec<u8>, u32, u64)>,
    mut path_exists: impl FnMut(&[u8]) -> bool,
) {
    let mut touched = vec![false; watches.len()];
    for (raw_path, flags, event_id) in events {
        if raw_path.is_empty() {
            continue;
        }
        if flags & MUST_SCAN_SUB_DIRS != 0 {
            let message = if flags & USER_DROPPED != 0 {
                "events were dropped by the FSEvents client"
            } else if flags & KERNEL_DROPPED != 0 {
                "events were dropped by the kernel"
            } else {
                "too many events"
            };
            let error = Error::Overflow.context_prefix(message);
            for (index, entry) in watches.iter().enumerate() {
                if !entry.terminated.load(Ordering::Acquire)
                    && overflow_matches(&entry.watch, &raw_path)
                {
                    entry.watch.events.set_error(error.clone());
                    touched[index] = true;
                }
            }
        }
        if flags & HISTORY_DONE != 0 {
            break;
        }
        if flags & !IGNORED_FLAGS == 0 {
            continue;
        }
        let removed = flags & ITEM_REMOVED != 0;
        let created = flags & ITEM_CREATED != 0;
        let renamed = flags & ITEM_RENAMED != 0;
        let mut exists = None;
        for (index, entry) in watches.iter().enumerate() {
            if entry.terminated.load(Ordering::Acquire) {
                continue;
            }
            let watch = &entry.watch;
            let Some(path) = display_path(watch, &raw_path) else {
                continue;
            };
            let is_root = path == watch.dir;
            if is_root && !removed && !renamed {
                continue;
            }
            let delete = if removed && !created {
                true
            } else if renamed || (removed && created) {
                !*exists.get_or_insert_with(|| path_exists(&raw_path))
            } else {
                false
            };
            if delete {
                if is_root {
                    watch.events.remove_watch_root_at(&path, event_id);
                } else {
                    watch.events.remove_at(&path, event_id);
                }
                let error = Error::WatchTerminated.context("watched directory removed");
                watch.terminate_callbacks_for_deleted_root(&path, event_id, error.clone());
                if is_root {
                    entry.terminated.store(true, Ordering::Release);
                    watch.events.set_error(error);
                }
            } else if is_root {
                watch.events.update_watch_root_at(&path, event_id);
            } else {
                watch.events.update_at(&path, event_id);
            }
            touched[index] = true;
        }
    }
    for (entry, touched) in watches.iter().zip(touched) {
        if touched {
            entry.watch.notify();
        }
    }
}

#[cfg(test)]
mod tests;
