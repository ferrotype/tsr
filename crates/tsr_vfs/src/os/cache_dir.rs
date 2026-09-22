//! Go's os.UserCacheDir selection. None asks the caller to use os.TempDir;
//! an invalid XDG override must not fall through to HOME.
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Copy)]
enum Platform {
    Windows,
    Darwin,
    Unix,
}

pub(super) fn user_cache_dir(getenv: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        Platform::Darwin
    } else {
        Platform::Unix
    };
    select(platform, getenv)
}

fn select(platform: Platform, getenv: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let nonempty = |key| {
        getenv(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    match platform {
        Platform::Windows => nonempty("LocalAppData"),
        Platform::Darwin => nonempty("HOME").map(|home| home.join("Library/Caches")),
        Platform::Unix => match nonempty("XDG_CACHE_HOME") {
            Some(path) => path.is_absolute().then_some(path),
            None => nonempty("HOME").map(|home| home.join(".cache")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{select, Platform};
    use std::path::PathBuf;

    // Expectations from Go 1.27.1 src/os/file.go:UserCacheDir. In particular,
    // Go rejects a relative XDG override but accepts a relative HOME.
    #[test]
    fn empty_values_and_invalid_overrides_request_temp_fallback() {
        for (platform, entries, expected) in [
            (Platform::Windows, vec![], None),
            (Platform::Windows, vec![("LocalAppData", "")], None),
            (
                Platform::Windows,
                vec![("LocalAppData", "cache")],
                Some("cache"),
            ),
            (Platform::Darwin, vec![("HOME", "")], None),
            (
                Platform::Darwin,
                vec![("HOME", "home")],
                Some("home/Library/Caches"),
            ),
            (
                Platform::Unix,
                vec![("XDG_CACHE_HOME", "relative"), ("HOME", "/home")],
                None,
            ),
            (
                Platform::Unix,
                vec![("XDG_CACHE_HOME", ""), ("HOME", "")],
                None,
            ),
            (Platform::Unix, vec![("HOME", "home")], Some("home/.cache")),
            (
                Platform::Unix,
                vec![("XDG_CACHE_HOME", "/cache")],
                Some("/cache"),
            ),
            (
                Platform::Unix,
                vec![("XDG_CACHE_HOME", ""), ("HOME", "/home")],
                Some("/home/.cache"),
            ),
        ] {
            assert_eq!(
                select(platform, |key| entries
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| (*value).into())),
                expected.map(PathBuf::from),
                "{entries:?}"
            );
        }
    }
}
