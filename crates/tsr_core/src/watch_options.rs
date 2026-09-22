//! Watch configuration with owned lists, following the approved option-clone policy.
use crate::Tristate;
use tsr_jsstring::JsString;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchOptions {
    pub interval: Option<isize>,
    pub file_kind: WatchFileKind,
    pub directory_kind: WatchDirectoryKind,
    pub fallback_polling: PollingKind,
    pub sync_watch_dir: Tristate,
    pub exclude_dir: Option<Vec<JsString>>,
    pub exclude_files: Option<Vec<JsString>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchFileKind(pub i32);
impl WatchFileKind {
    pub const NONE: Self = Self(0);
    pub const FIXED_POLLING_INTERVAL: Self = Self(1);
    pub const PRIORITY_POLLING_INTERVAL: Self = Self(2);
    pub const DYNAMIC_PRIORITY_POLLING: Self = Self(3);
    pub const FIXED_CHUNK_SIZE_POLLING: Self = Self(4);
    pub const USE_FS_EVENTS: Self = Self(5);
    pub const USE_FS_EVENTS_ON_PARENT_DIRECTORY: Self = Self(6);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchDirectoryKind(pub i32);
impl WatchDirectoryKind {
    pub const NONE: Self = Self(0);
    pub const USE_FS_EVENTS: Self = Self(1);
    pub const FIXED_POLLING_INTERVAL: Self = Self(2);
    pub const DYNAMIC_PRIORITY_POLLING: Self = Self(3);
    pub const FIXED_CHUNK_SIZE_POLLING: Self = Self(4);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollingKind(pub i32);
impl PollingKind {
    pub const NONE: Self = Self(0);
    pub const FIXED_INTERVAL: Self = Self(1);
    pub const PRIORITY_INTERVAL: Self = Self(2);
    pub const DYNAMIC_PRIORITY: Self = Self(3);
    pub const FIXED_CHUNK_SIZE: Self = Self(4);
}
