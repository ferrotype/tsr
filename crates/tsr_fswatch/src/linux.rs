#![forbid(unsafe_code)]
use crate::{
    fanotify::{parse_dfid_names, DfidName, HandleKey},
    linux_ffi as ffi, lock,
    walkdir::walk_dir,
    watcher::{is_in_directory_or_self, join_suffix, os_path, Backend, DirWatch},
    Error,
};
use rustix::{
    event::{poll, PollFd, PollFlags},
    fd::OwnedFd,
    fs::inotify as ino,
    io::{read, write},
    pipe::{pipe_with, PipeFlags},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Inotify,
    Fanotify,
}
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Inotify(i32),
    Fanotify(HandleKey),
}
#[derive(Clone)]
struct Subscription {
    path: Vec<u8>,
    physical: Vec<u8>,
    watch: Arc<DirWatch>,
}
struct State {
    fd: Arc<OwnedFd>,
    mode: Mode,
    mask: u64,
    no_rename: bool,
    subscriptions: HashMap<Key, Vec<Subscription>>,
    watches: Vec<Arc<DirWatch>>,
}
struct LinuxBackend {
    state: Arc<Mutex<State>>,
    wake: Mutex<Option<OwnedFd>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
const INOTIFY_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_DELETE_SELF
    | libc::IN_MODIFY
    | libc::IN_MOVE_SELF
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_DONT_FOLLOW
    | libc::IN_ONLYDIR
    | libc::IN_EXCL_UNLINK;
const FAN_BASE: u64 = libc::FAN_CREATE
    | libc::FAN_DELETE
    | libc::FAN_MODIFY
    | libc::FAN_DELETE_SELF
    | libc::FAN_MOVE_SELF
    | libc::FAN_ONDIR
    | libc::FAN_EVENT_ON_CHILD;
const FAN_ADD: u32 = libc::FAN_MARK_ADD | libc::FAN_MARK_ONLYDIR | libc::FAN_MARK_DONT_FOLLOW;
pub(crate) fn fanotify_available() -> bool {
    ffi::init().is_ok()
}
pub(crate) fn inotify() -> Result<Arc<dyn Backend>, Error> {
    LinuxBackend::new(Mode::Inotify, false).map(|b| b as Arc<dyn Backend>)
}
pub(crate) fn fanotify() -> Result<Arc<dyn Backend>, Error> {
    LinuxBackend::new(Mode::Fanotify, false).map(|b| b as Arc<dyn Backend>)
}
#[cfg(test)]
pub(crate) fn fanotify_no_rename() -> Result<Arc<dyn Backend>, Error> {
    LinuxBackend::new(Mode::Fanotify, true).map(|backend| backend as Arc<dyn Backend>)
}
impl LinuxBackend {
    fn new(mode: Mode, no_rename: bool) -> Result<Arc<Self>, Error> {
        let fd = Arc::new(match mode {
            Mode::Inotify => ino::init(ino::CreateFlags::NONBLOCK | ino::CreateFlags::CLOEXEC)?,
            Mode::Fanotify => ffi::init()?,
        });
        let (reader, writer) = pipe_with(PipeFlags::NONBLOCK | PipeFlags::CLOEXEC)?;
        let state = Arc::new(Mutex::new(State {
            fd: fd.clone(),
            mode,
            mask: 0,
            no_rename,
            subscriptions: HashMap::new(),
            watches: Vec::new(),
        }));
        let work = state.clone();
        let worker = thread::Builder::new()
            .name(
                match mode {
                    Mode::Inotify => "fswatch-inotify",
                    Mode::Fanotify => "fswatch-fanotify",
                }
                .into(),
            )
            .spawn(move || {
                if let Err(error) = run(work.clone(), fd, reader) {
                    let watches = lock(&work).watches.clone();
                    for watch in watches {
                        watch.notify_error(Error::WatchTerminated.context(error.to_string()));
                    }
                }
            })?;
        Ok(Arc::new(Self {
            state,
            wake: Mutex::new(Some(writer)),
            worker: Mutex::new(Some(worker)),
        }))
    }
}
impl Backend for LinuxBackend {
    fn add_many(&self, watches: &[Arc<DirWatch>]) -> Result<(), Error> {
        let mut state = lock(&self.state);
        let mut added = Vec::new();
        for watch in watches {
            if state
                .watches
                .iter()
                .any(|existing| Arc::ptr_eq(existing, watch))
            {
                continue;
            }
            if let Err(error) = state.subscribe(watch) {
                state.close_watch(watch);
                for prior in &added {
                    state.close_watch(prior);
                }
                return Err(error);
            }
            state.watches.push(watch.clone());
            added.push(watch.clone());
        }
        Ok(())
    }
    fn remove(&self, watch: &Arc<DirWatch>) -> Result<(), Error> {
        lock(&self.state).close_watch(watch);
        Ok(())
    }
    fn shutdown(&self) {
        if let Some(wake) = lock(&self.wake).take() {
            let _ = write(&wake, b"X");
        }
        if let Some(worker) = lock(&self.worker).take() {
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}
impl Drop for LinuxBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn run(state: Arc<Mutex<State>>, fd: Arc<OwnedFd>, wake: OwnedFd) -> Result<(), Error> {
    let mut buffer = [0u8; 8192];
    loop {
        let mut fds = [
            PollFd::new(&wake, PollFlags::IN),
            PollFd::new(&*fd, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
        if !fds[0].revents().is_empty() {
            return Ok(());
        }
        if fds[1].revents().is_empty() {
            continue;
        }
        let mut touched = Vec::new();
        loop {
            let length = match read(&*fd, &mut buffer) {
                Ok(0) => break,
                Ok(length) => length,
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            };
            let mut state = lock(&state);
            match state.mode {
                Mode::Inotify => state.inotify_events(&buffer[..length], &mut touched)?,
                Mode::Fanotify => state.fanotify_events(&buffer[..length], &mut touched)?,
            }
        }
        for watch in touched {
            watch.notify();
        }
    }
}
fn touched(watches: &mut Vec<Arc<DirWatch>>, watch: &Arc<DirWatch>) {
    if !watches.iter().any(|existing| Arc::ptr_eq(existing, watch)) {
        watches.push(watch.clone());
    }
}
impl State {
    fn subscribe(&mut self, watch: &Arc<DirWatch>) -> Result<(), Error> {
        if self.mode == Mode::Fanotify && self.mask == 0 {
            self.mask = if self.no_rename {
                FAN_BASE | libc::FAN_MOVED_FROM | libc::FAN_MOVED_TO
            } else {
                FAN_BASE | libc::FAN_RENAME
            };
            if !self.no_rename {
                match ffi::mark(&self.fd, &watch.physical_dir, FAN_ADD, self.mask) {
                    Ok(()) => loop {
                        match ffi::mark(
                            &self.fd,
                            &watch.physical_dir,
                            libc::FAN_MARK_REMOVE | libc::FAN_MARK_ONLYDIR,
                            self.mask,
                        ) {
                            Err(rustix::io::Errno::INTR) => continue,
                            _ => break,
                        }
                    },
                    Err(rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP) => {
                        self.mask = FAN_BASE | libc::FAN_MOVED_FROM | libc::FAN_MOVED_TO
                    }
                    Err(_) => {}
                }
            }
        }
        if !watch.recursive {
            return self
                .add_dir(watch, &watch.dir, &watch.physical_dir)
                .map_err(|error| self.subscription_error(watch, &watch.dir, error));
        }
        walk_dir(&watch.physical_dir, true, &mut |physical, directory| {
            if directory {
                let path = watch.display_path(physical);
                self.add_dir(watch, &path, physical)
                    .map_err(|error| self.subscription_error(watch, &path, error))?;
            }
            Ok(())
        })
    }
    fn subscription_error(&self, watch: &Arc<DirWatch>, path: &[u8], error: Error) -> Error {
        if self.mode == Mode::Fanotify {
            Error::DirectoryWatch {
                directory: watch.dir.clone(),
                source: Box::new(error.context_prefix(format!(
                    "fanotify_mark on '{}' failed",
                    String::from_utf8_lossy(path)
                ))),
            }
        } else {
            error
        }
    }
    fn add_dir(
        &mut self,
        watch: &Arc<DirWatch>,
        path: &[u8],
        physical: &[u8],
    ) -> Result<(), Error> {
        let key = match self.mode {
            Mode::Inotify => Key::Inotify(ino::add_watch(
                &*self.fd,
                physical,
                ino::WatchFlags::from_bits_retain(INOTIFY_MASK),
            )?),
            Mode::Fanotify => {
                ffi::mark(&self.fd, physical, FAN_ADD, self.mask).map_err(ffi::unsupported)?;
                match ffi::handle_key(physical) {
                    Ok(key) => Key::Fanotify(key),
                    Err(error) => {
                        let _ = ffi::mark(
                            &self.fd,
                            physical,
                            libc::FAN_MARK_REMOVE | libc::FAN_MARK_ONLYDIR,
                            self.mask,
                        );
                        return Err(error);
                    }
                }
            }
        };
        self.subscriptions
            .entry(key)
            .or_default()
            .push(Subscription {
                path: path.to_vec(),
                physical: physical.to_vec(),
                watch: watch.clone(),
            });
        Ok(())
    }
    fn close_watch(&mut self, watch: &Arc<DirWatch>) {
        let keys: Vec<_> = self.subscriptions.keys().cloned().collect();
        for key in keys {
            let list = self.subscriptions.get_mut(&key).unwrap();
            let path = list
                .iter()
                .find(|sub| Arc::ptr_eq(&sub.watch, watch))
                .map(|sub| sub.physical.clone());
            let Some(path) = path else {
                continue;
            };
            list.retain(|sub| !Arc::ptr_eq(&sub.watch, watch));
            if list.is_empty() {
                match &key {
                    Key::Inotify(wd) => {
                        let _ = ino::remove_watch(&*self.fd, *wd);
                    }
                    Key::Fanotify(_) if self.mask != 0 => {
                        let _ = ffi::mark(&self.fd, &path, libc::FAN_MARK_REMOVE, self.mask);
                    }
                    _ => {}
                }
                self.subscriptions.remove(&key);
            }
        }
        self.watches
            .retain(|existing| !Arc::ptr_eq(existing, watch));
    }
    fn drop_path(&mut self, path: &[u8], recursive: bool) {
        let keys: Vec<_> = self.subscriptions.keys().cloned().collect();
        for key in keys {
            let list = self.subscriptions.get_mut(&key).unwrap();
            list.retain(|sub| {
                if recursive {
                    !is_in_directory_or_self(path, &sub.path)
                } else {
                    sub.path != path
                }
            });
            if list.is_empty() {
                if let Key::Inotify(wd) = key {
                    let _ = ino::remove_watch(&*self.fd, wd);
                }
                self.subscriptions.remove(&key);
            }
        }
    }
    fn overflow(&self, touch: &mut Vec<Arc<DirWatch>>) {
        // A deleted watch root can remain retained by its public Watch until
        // Close. Only roots with active native subscriptions receive overflow.
        for subscription in self.subscriptions.values().flatten() {
            subscription.watch.events.set_error(Error::Overflow);
            touched(touch, &subscription.watch);
        }
    }
    fn inotify_events(
        &mut self,
        mut data: &[u8],
        touch: &mut Vec<Arc<DirWatch>>,
    ) -> Result<(), Error> {
        while data.len() >= 16 {
            let wd = i32::from_ne_bytes(data[..4].try_into().unwrap());
            let mask = u32::from_ne_bytes(data[4..8].try_into().unwrap());
            let length = u32::from_ne_bytes(data[12..16].try_into().unwrap()) as usize;
            let end = 16usize
                .checked_add(length)
                .filter(|end| *end <= data.len())
                .ok_or_else(|| Error::Message("invalid inotify event length".into()))?;
            let name = &data[16..end];
            let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
            if mask & libc::IN_Q_OVERFLOW != 0 {
                self.overflow(touch);
            } else {
                let subscriptions = self
                    .subscriptions
                    .get(&Key::Inotify(wd))
                    .cloned()
                    .unwrap_or_default();
                for sub in subscriptions {
                    let path = join_suffix(&sub.path, name);
                    let physical = join_suffix(&sub.physical, name);
                    let directory = mask & libc::IN_ISDIR != 0;
                    if mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                        sub.watch.events.create(&path);
                        if directory && sub.watch.recursive {
                            let _ = walk_dir(&physical, true, &mut |p, is_dir| {
                                if is_dir {
                                    let _ = self.add_dir(&sub.watch, &sub.watch.display_path(p), p);
                                }
                                Ok(())
                            });
                        }
                    } else if mask & libc::IN_MODIFY != 0 {
                        sub.watch.events.update(&path);
                    } else if mask
                        & (libc::IN_DELETE
                            | libc::IN_DELETE_SELF
                            | libc::IN_MOVED_FROM
                            | libc::IN_MOVE_SELF)
                        != 0
                    {
                        let self_event = mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0;
                        if self_event && path != sub.watch.dir {
                            continue;
                        }
                        if self_event || directory {
                            self.drop_path(&path, true);
                        }
                        sub.watch.events.remove(&path);
                        if self_event && path == sub.watch.dir {
                            sub.watch.events.set_error(
                                Error::WatchTerminated.context("watched directory removed"),
                            );
                        }
                    }
                    touched(touch, &sub.watch);
                }
            }
            data = &data[end..];
        }
        Ok(())
    }
    fn fanotify_events(
        &mut self,
        mut data: &[u8],
        touch: &mut Vec<Arc<DirWatch>>,
    ) -> Result<(), Error> {
        while data.len() >= 24 {
            if data[4] != 3 {
                return Err(Error::Message(format!(
                    "unsupported fanotify metadata version: {}",
                    data[4]
                )));
            }
            let length = u32::from_ne_bytes(data[..4].try_into().unwrap()) as usize;
            let metadata = usize::from(u16::from_ne_bytes([data[6], data[7]]));
            if metadata < 24 || length < metadata || length > data.len() {
                break;
            }
            let mask = u64::from_ne_bytes(data[8..16].try_into().unwrap());
            ffi::close_event_fd(i32::from_ne_bytes(data[16..20].try_into().unwrap()));
            if mask & libc::FAN_Q_OVERFLOW != 0 {
                self.overflow(touch);
            } else {
                let (primary, rename) = parse_dfid_names(&data[metadata..length]);
                if mask & libc::FAN_RENAME != 0 {
                    if let Some(old) =
                        primary.filter(|record| !record.name.is_empty() && record.name != b".")
                    {
                        self.fanotify_event(
                            libc::FAN_DELETE | (mask & libc::FAN_ONDIR),
                            &old,
                            touch,
                        );
                    }
                    if let Some(new) =
                        rename.filter(|record| !record.name.is_empty() && record.name != b".")
                    {
                        self.fanotify_event(
                            libc::FAN_CREATE | (mask & libc::FAN_ONDIR),
                            &new,
                            touch,
                        );
                    }
                } else if let Some(primary) = primary {
                    self.fanotify_event(mask, &primary, touch);
                }
            }
            data = &data[length..];
        }
        Ok(())
    }
    fn fanotify_event(&mut self, mask: u64, record: &DfidName, touch: &mut Vec<Arc<DirWatch>>) {
        let subscriptions = self
            .subscriptions
            .get(&Key::Fanotify(record.key.clone()))
            .cloned()
            .unwrap_or_default();
        for sub in subscriptions {
            let self_event = record.name.is_empty() || record.name == b".";
            let path = if self_event {
                sub.path.clone()
            } else {
                join_suffix(&sub.path, &record.name)
            };
            let directory = mask & libc::FAN_ONDIR != 0;
            let create = mask & (libc::FAN_CREATE | libc::FAN_MOVED_TO) != 0;
            let delete = mask & (libc::FAN_DELETE | libc::FAN_MOVED_FROM) != 0;
            if create && delete && !self_event && std::fs::symlink_metadata(os_path(&path)).is_err()
            {
                sub.watch.events.create(&path);
                sub.watch.events.remove(&path);
                touched(touch, &sub.watch);
                continue;
            }
            if mask
                & (libc::FAN_DELETE
                    | libc::FAN_DELETE_SELF
                    | libc::FAN_MOVED_FROM
                    | libc::FAN_MOVE_SELF)
                != 0
            {
                let self_mask = mask & (libc::FAN_DELETE_SELF | libc::FAN_MOVE_SELF) != 0;
                if !self_mask || path == sub.watch.dir {
                    self.drop_path(&path, self_mask || directory);
                    sub.watch.events.remove(&path);
                    touched(touch, &sub.watch);
                    if self_mask && path == sub.watch.dir {
                        sub.watch
                            .events
                            .set_error(Error::WatchTerminated.context("watched directory removed"));
                    }
                }
            }
            if create {
                sub.watch.events.create(&path);
                if directory && sub.watch.recursive {
                    let _ = walk_dir(&sub.watch.physical_path(&path), true, &mut |p, is_dir| {
                        if is_dir {
                            let _ = self.add_dir(&sub.watch, &sub.watch.display_path(p), p);
                        }
                        Ok(())
                    });
                }
                touched(touch, &sub.watch);
            }
            if mask & libc::FAN_MODIFY != 0 {
                sub.watch.events.update(&path);
                touched(touch, &sub.watch);
            }
        }
    }
}

#[cfg(test)]
mod tests;
