//! The native ownership boundary. Each stream owns its callback context until
//! Stop -> Invalidate -> serial-queue barrier -> Release has completed.

use super::{process_events, WatchSnapshot};
use crate::watcher::os_path;
use crate::Error;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2_core_foundation::{
    CFArray, CFMutableString, CFRetained, CFString, CFStringNormalizationForm,
};
use objc2_core_services::{
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagUseCFTypes, ConstFSEventStreamRef,
    FSEventStreamContext, FSEventStreamCreate, FSEventStreamFlushSync, FSEventStreamInvalidate,
    FSEventStreamRef, FSEventStreamRelease, FSEventStreamSetDispatchQueue, FSEventStreamStart,
    FSEventStreamStop, FSEventsGetCurrentEventId,
};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;

const UTF8: u32 = 0x08000100;

struct CallbackContext {
    watches: Vec<WatchSnapshot>,
}

pub(super) struct Stream {
    // The bindings hide the pointee type, so their public reference alias is
    // retained here. Construction guarantees this owned pointer is non-null.
    stream: FSEventStreamRef,
    queue: DispatchRetained<DispatchQueue>,
    // The allocation's address remains stable when Stream moves. FSEvents has
    // no retain/release callbacks, so this is its sole owner.
    _context: Box<CallbackContext>,
    started: bool,
}

// SAFETY: Native stream lifecycle calls are serialized by the backend mutex.
// FSEvents itself dispatches through its private serial queue; the callback
// accesses only immutable context and synchronized DirWatch/AtomicBool data.
// No API exposes this pointer or permits destruction from its own callback.
unsafe impl Send for Stream {}

impl Stream {
    // port: tsc/internal/fswatch/fsevents_darwin.go:fsEventsBackend.startStream
    pub(super) fn new(paths: &[Vec<u8>], watches: Vec<WatchSnapshot>) -> Result<Self, Error> {
        let strings: Vec<_> = paths
            .iter()
            .map(|path| {
                cf_string(path).ok_or_else(|| Error::Message("CFStringCreate returned NULL".into()))
            })
            .collect::<Result<_, _>>()?;
        let mut pointers: Vec<*const c_void> = strings
            .iter()
            .map(|value| CFRetained::as_ptr(value).as_ptr().cast_const().cast())
            .collect();
        // SAFETY: Every pointer is a live CFString. The array does not retain
        // them (matching the pin's null callbacks); `strings` outlives Create,
        // which copies the watched paths into the stream.
        let path_array = unsafe {
            CFArray::new(
                None,
                pointers.as_mut_ptr(),
                pointers.len() as isize,
                std::ptr::null(),
            )
        }
        .ok_or_else(|| Error::Message("CFArrayCreate returned NULL".into()))?;
        let queue = DispatchQueue::new("typescript.fswatch.fsevents.stream", None);
        let mut context = Box::new(CallbackContext { watches });
        let mut native_context = FSEventStreamContext {
            version: 0,
            info: std::ptr::from_mut(context.as_mut()).cast(),
            retain: None,
            release: None,
            copyDescription: None,
        };
        // SAFETY: The context allocation and queue are retained by Stream,
        // paths contain CFStrings, and callback implements this exact ABI.
        let stream = unsafe {
            FSEventStreamCreate(
                None,
                Some(callback),
                &mut native_context,
                &path_array,
                u64::MAX,
                0.001,
                kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents,
            )
        };
        if stream.is_null() {
            return Err(Error::Message("FSEventStreamCreate returned NULL".into()));
        }
        let mut result = Self {
            stream,
            queue,
            _context: context,
            started: false,
        };
        // SAFETY: The stream is owned, not yet started, and the queue lives
        // through invalidation and draining in Drop.
        unsafe {
            FSEventStreamSetDispatchQueue(stream, Some(&result.queue));
        }
        // SAFETY: The stream has a live callback context and dispatch queue.
        if !unsafe { FSEventStreamStart(stream) } {
            return Err(Error::Message("error starting FSEvents stream".into()));
        }
        result.started = true;
        // SAFETY: The stream is started. This caller never executes on its
        // dispatch queue; callbacks do not need any backend lifecycle lock.
        unsafe {
            FSEventStreamFlushSync(stream);
        }
        Ok(result)
    }
}

impl Drop for Stream {
    // Native unique ownership replaces the Go atomic pointer swap and pinner.
    // port: tsc/internal/fswatch/fsevents_darwin.go:stopFSEventsStream
    // port: tsc/internal/fswatch/fsevents_darwin.go:teardownStream
    fn drop(&mut self) {
        // SAFETY: Unique ownership means this happens once. Stop/Invalidate
        // prevent new callbacks, while the context and queue remain alive.
        unsafe {
            if self.started {
                FSEventStreamStop(self.stream);
            }
            FSEventStreamInvalidate(self.stream);
        }
        // This waits for classification too: unlike Go, Rust classifies in
        // the C callback and does not enqueue a second pipe-based worker.
        self.queue.exec_sync(|| {});
        // SAFETY: All queued callbacks have completed. Release precedes field
        // destruction, so native code cannot observe freed callback state.
        unsafe {
            FSEventStreamRelease(self.stream);
        }
    }
}

// port: tsc/internal/fswatch/fsevents_darwin_ffi.go:fsEventsGetCurrentEventID
pub(super) fn current_event_id() -> u64 {
    // SAFETY: This process-wide query takes no pointers or ownership.
    unsafe { FSEventsGetCurrentEventId() }
}

unsafe extern "C-unwind" fn callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<u32>,
    ids: NonNull<u64>,
) {
    // Never unwind across the framework ABI, even if internal event handling
    // panics. Aborting also avoids silently continuing after losing a batch.
    if catch_unwind(AssertUnwindSafe(|| {
        if info.is_null() || count == 0 {
            return;
        }
        // SAFETY: These values are provided by FSEvents for this callback.
        // UseCFTypes requests a CFArray<CFString>; flags/ids have `count`
        // elements and remain valid until this function returns. Stream owns
        // `info` until invalidation and the dispatch barrier finish.
        let (context, paths, flags, ids) = unsafe {
            (
                &*info.cast::<CallbackContext>(),
                paths.cast::<CFArray<CFString>>().as_ref(),
                std::slice::from_raw_parts(flags.as_ptr(), count),
                std::slice::from_raw_parts(ids.as_ptr(), count),
            )
        };
        let path_count = paths.len();
        let events = (0..count).map(|index| {
            let path = if index < path_count {
                // SAFETY: The native batch is immutable for this callback;
                // the checked index is within CFArray's CFIndex-sized count.
                // This borrow does not escape, so no CFRetain is necessary.
                cf_string_to_nfc(unsafe { paths.get_unchecked(index as isize) })
            } else {
                Vec::new()
            };
            (path, flags[index], ids[index])
        });
        process_events(&context.watches, events, |path| {
            std::fs::symlink_metadata(os_path(path)).is_ok()
        });
    }))
    .is_err()
    {
        std::process::abort();
    }
}

// port: tsc/internal/fswatch/fsevents_darwin_ffi.go:cfStringCreate
fn cf_string(bytes: &[u8]) -> Option<CFRetained<CFString>> {
    // Keep the pin's CString semantics (including its first-NUL boundary).
    let mut terminated = Vec::with_capacity(bytes.len() + 1);
    terminated.extend_from_slice(bytes);
    terminated.push(0);
    // SAFETY: The pointer is NUL-terminated and lives through this copying
    // constructor. CFString rejects invalid UTF-8 instead of replacing bytes.
    unsafe { CFString::with_c_string(None, terminated.as_ptr().cast(), UTF8) }
}

// port: tsc/internal/fswatch/fsevents_darwin_ffi.go:cfStringToGo
fn cf_string_bytes(value: &CFString) -> Vec<u8> {
    let Some(size) = CFString::maximum_size_for_encoding(value.length(), UTF8)
        .checked_add(1)
        .filter(|size| *size > 0)
    else {
        return Vec::new();
    };
    let mut bytes = vec![0; size as usize];
    // SAFETY: The allocated buffer is exactly the declared size; CFString
    // writes at most this size, including the terminating NUL.
    if !unsafe { value.c_string(bytes.as_mut_ptr().cast(), size, UTF8) } {
        return Vec::new();
    }
    if let Some(end) = bytes.iter().position(|byte| *byte == 0) {
        bytes.truncate(end);
    }
    bytes
}

// port: tsc/internal/fswatch/fsevents_darwin_ffi.go:cfStringToNFC
fn cf_string_to_nfc(value: &CFString) -> Vec<u8> {
    if let Some(normalized) = CFMutableString::new_copy(None, 0, Some(value)) {
        // SAFETY: This freshly allocated mutable copy is exclusively owned,
        // and C is a supported normalization form.
        unsafe {
            CFMutableString::normalize(Some(&normalized), CFStringNormalizationForm::C);
        }
        let bytes = cf_string_bytes(&normalized);
        if !bytes.is_empty() {
            return bytes;
        }
    }
    cf_string_bytes(value)
}

// port: tsc/internal/fswatch/fsevents_darwin_ffi.go:normalizeNFC
pub(super) fn canonicalize(path: &[u8]) -> Vec<u8> {
    if path.is_ascii() {
        return path.to_vec();
    }
    if let Some(value) = cf_string(path) {
        let normalized = cf_string_to_nfc(&value);
        if !normalized.is_empty() {
            return normalized;
        }
    }
    path.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::tests::TempDir;
    use crate::watcher::DirWatch;
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn native_callback_normalizes_paths_and_preserves_sequence() {
        let watch = DirWatch::for_test(b"/root", b"/root", true);
        let mut context = CallbackContext {
            watches: vec![WatchSnapshot {
                watch: watch.clone(),
                terminated: Arc::new(AtomicBool::new(false)),
            }],
        };
        let path = cf_string("/root/cafe\u{301}.ts".as_bytes()).unwrap();
        let paths = CFArray::from_retained_objects(&[path]);
        let mut flags = [super::super::ITEM_CREATED];
        let mut ids = [1742];
        // SAFETY: These owned arrays and context supply exactly the same
        // initialized, live arguments as the UseCFTypes FSEvents callback.
        unsafe {
            callback(
                std::ptr::null(),
                std::ptr::from_mut(&mut context).cast(),
                1,
                CFRetained::as_ptr(&paths).cast(),
                NonNull::new(flags.as_mut_ptr()).unwrap(),
                NonNull::new(ids.as_mut_ptr()).unwrap(),
            );
        }
        let (events, error) = watch.events.drain_for_sequences(&[1741, 1742]);
        assert!(error.is_none());
        assert_eq!(events[0][0].event.path, "/root/caf\u{e9}.ts".as_bytes());
        assert!(events[1].is_empty());
    }

    #[test]
    fn stop_drains_serial_queue_before_releasing_callback_context() {
        let root = TempDir::new();
        let watch = DirWatch::for_test(root.bytes(), root.bytes(), true);
        let weak_watch = Arc::downgrade(&watch);
        let stream = Stream::new(
            &[root.bytes().to_vec()],
            vec![WatchSnapshot {
                watch,
                terminated: Arc::new(AtomicBool::new(false)),
            }],
        )
        .unwrap();
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        stream.queue.exec_async(move || {
            entered_sender.send(()).unwrap();
            release_receiver.recv().unwrap();
        });
        entered_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let (finished_sender, finished_receiver) = mpsc::channel();
        let teardown = std::thread::spawn(move || {
            drop(stream);
            finished_sender.send(()).unwrap();
        });
        assert!(finished_receiver
            .recv_timeout(Duration::from_millis(50))
            .is_err());
        assert!(
            weak_watch.upgrade().is_some(),
            "context released before queue drained"
        );
        release_sender.send(()).unwrap();
        finished_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        teardown.join().unwrap();
        assert!(weak_watch.upgrade().is_none());
    }
}
