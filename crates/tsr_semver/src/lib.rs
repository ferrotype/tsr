//! The pinned compiler's version and version-range contracts.

mod version;
mod version_range;

pub use version::{try_parse_version, ParseError, Version};
pub use version_range::{try_parse_version_range, VersionRange};
