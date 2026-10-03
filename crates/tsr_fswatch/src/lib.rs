//! Native filesystem subscriptions, ported from the pinned `fswatch` package.
//! A retained [`Watch`] keeps its backend alive. Closing the final watch
//! releases its native worker and debounce worker; callback panics are isolated.
mod debounce;
mod event;
mod fallback;
#[cfg(any(target_os = "linux", test))]
mod fanotify;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux_ffi;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(test)]
mod test_support;
#[cfg(any(target_os = "linux", test))]
mod walkdir;
pub(crate) mod watcher;

pub use event::{Event, EventKind};
pub use watcher::{
    all_watchers, default_watcher, fanotify, fsevents, inotify, kqueue, windows, Ignore, Watch,
    WatchCallback, WatchDirectoryRequest, WatchOptions, Watcher,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    WatchTerminated,
    Unavailable,
    FilesystemUnsupported,
    Message(String),
    Os {
        code: i32,
        message: String,
    },
    TaggedFilesystemUnsupported {
        source: Box<Error>,
    },
    DirectoryWatch {
        directory: Vec<u8>,
        source: Box<Error>,
    },
    Context {
        source: Box<Error>,
        message: String,
    },
    Prefix {
        source: Box<Error>,
        message: String,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Overflow => "fswatch: event overflow; some changes were missed",
            Self::WatchTerminated => "fswatch: watch terminated",
            Self::Unavailable => "fswatch: watcher not available on this platform",
            Self::FilesystemUnsupported => {
                "fswatch: watcher backend unsupported on this filesystem"
            }
            Self::Message(message) | Self::Os { message, .. } => message,
            Self::TaggedFilesystemUnsupported { source } => {
                return write!(f, "{}: {source}", Self::FilesystemUnsupported);
            }
            Self::DirectoryWatch { source, .. } => return write!(f, "{source}"),
            Self::Context { source, message } => return write!(f, "{source}: {message}"),
            Self::Prefix { source, message } => return write!(f, "{message}: {source}"),
        })
    }
}
impl Error {
    pub fn context(self, message: impl Into<String>) -> Self {
        Self::Context {
            source: Box::new(self),
            message: message.into(),
        }
    }
    pub fn context_prefix(self, message: impl Into<String>) -> Self {
        Self::Prefix {
            source: Box::new(self),
            message: message.into(),
        }
    }
    pub fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Os { code, .. } => Some(*code),
            Self::Context { source, .. }
            | Self::Prefix { source, .. }
            | Self::TaggedFilesystemUnsupported { source }
            | Self::DirectoryWatch { source, .. } => source.raw_os_error(),
            _ => None,
        }
    }
    pub fn watch_directory(&self) -> Option<&[u8]> {
        match self {
            Self::DirectoryWatch { directory, .. } => Some(directory),
            Self::Context { source, .. }
            | Self::Prefix { source, .. }
            | Self::TaggedFilesystemUnsupported { source } => source.watch_directory(),
            _ => None,
        }
    }
    pub fn is_overflow(&self) -> bool {
        matches!(self, Self::Overflow)
            || matches!(self,Self::Context{source,..}|Self::Prefix{source,..}|Self::TaggedFilesystemUnsupported{source}|Self::DirectoryWatch{source,..} if source.is_overflow())
    }
    pub fn is_watch_terminated(&self) -> bool {
        matches!(self, Self::WatchTerminated)
            || matches!(self,Self::Context{source,..}|Self::Prefix{source,..}|Self::TaggedFilesystemUnsupported{source}|Self::DirectoryWatch{source,..} if source.is_watch_terminated())
    }
    pub fn is_filesystem_unsupported(&self) -> bool {
        matches!(
            self,
            Self::FilesystemUnsupported | Self::TaggedFilesystemUnsupported { .. }
        ) || matches!(self,Self::Context{source,..}|Self::Prefix{source,..}|Self::TaggedFilesystemUnsupported{source}|Self::DirectoryWatch{source,..} if source.is_filesystem_unsupported())
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Context { source, .. }
            | Self::Prefix { source, .. }
            | Self::TaggedFilesystemUnsupported { source }
            | Self::DirectoryWatch { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        match error.raw_os_error() {
            Some(code) => Self::Os {
                code,
                message: error.to_string(),
            },
            None => Self::Message(error.to_string()),
        }
    }
}
impl From<rustix::io::Errno> for Error {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Os {
            code: error.raw_os_error(),
            message: error.to_string(),
        }
    }
}
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
