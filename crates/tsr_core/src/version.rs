//! The compiler version strings every consumer embeds.
//!
//! Module resolution traces the full version (trace 6208) and parses it for
//! `typesVersions` range tests; the package.json cache and the global typings
//! cache location use the major.minor prefix. One home keeps those consumers
//! from drifting apart when the pin moves.

use std::sync::LazyLock;

/// Go keeps this in a package variable so a release build can override it
/// with ldflags (version.go:7-8). The port has no such override.
const VERSION: &str = "7.1.0-dev";

/// Go computes the prefix once, when the package is initialized.
static VERSION_MAJOR_MINOR: LazyLock<&'static str> = LazyLock::new(|| major_minor(VERSION));

/// port: tsc/internal/core/version.go:Version
pub fn version() -> &'static str {
    VERSION
}

/// port: tsc/internal/core/version.go:VersionMajorMinor
pub fn version_major_minor() -> &'static str {
    *VERSION_MAJOR_MINOR
}

/// The initializer of the pinned `versionMajorMinor` variable (version.go:14-29):
/// everything before the second `.`, and a panic when there is no second `.`.
fn major_minor(version: &str) -> &str {
    let mut seen_major = false;
    let index = version.char_indices().find_map(|(index, rune)| {
        if rune == '.' {
            if seen_major {
                return Some(index);
            }
            seen_major = true;
        }
        None
    });
    match index {
        Some(index) => &version[..index],
        None => panic!("invalid version string: {version}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_version_and_its_major_minor_prefix() {
        assert_eq!(version(), "7.1.0-dev");
        assert_eq!(version_major_minor(), "7.1");
    }

    #[test]
    fn major_minor_stops_at_the_second_dot() {
        assert_eq!(major_minor("1.2.3"), "1.2");
        assert_eq!(major_minor("10.20.30-beta.1"), "10.20");
        assert_eq!(major_minor("1.2."), "1.2");
        assert_eq!(major_minor(".."), ".");
    }

    #[test]
    #[should_panic(expected = "invalid version string: 7.1")]
    fn major_minor_panics_without_a_second_dot() {
        major_minor("7.1");
    }

    #[test]
    #[should_panic(expected = "invalid version string: 7")]
    fn major_minor_panics_without_any_dot() {
        major_minor("7");
    }
}
