//! Immutable JavaScript value bytes; each view carries its own validity tag.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use crate::wtf8::{code_points, decode_rune, CodePoints, RUNE_ERROR};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Validity {
    Utf8,
    Wtf8,
    Raw,
}

/// Immutable byte string; clones and byte slices share the original allocation.
#[derive(Clone, Debug)]
pub struct JsString {
    storage: Arc<[u8]>,
    start_tag: usize,
    end_tag: usize,
}

// A non-zero-sized Rust allocation cannot exceed isize::MAX bytes. Each byte
// endpoint therefore leaves its high bit free. The two bits hold the three
// validity states without adding a word to every identifier and symbol name.
const OFFSET_MASK: usize = isize::MAX as usize;
const TAG_BIT: usize = !OFFSET_MASK;

impl JsString {
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        let storage = bytes.into();
        let length = storage.len();
        Self::from_shared_range(storage, 0..length)
    }

    pub(crate) fn from_shared_range(storage: Arc<[u8]>, range: Range<usize>) -> Self {
        let validity = classify(&storage[range.clone()]);
        Self::from_validated_range(storage, range, validity)
    }

    fn from_validated_range(storage: Arc<[u8]>, range: Range<usize>, validity: Validity) -> Self {
        assert!(
            range.end <= OFFSET_MASK,
            "byte allocation exceeds Rust's addressable object size"
        );
        Self {
            storage,
            start_tag: range.start
                | if validity == Validity::Wtf8 {
                    TAG_BIT
                } else {
                    0
                },
            end_tag: range.end
                | if validity == Validity::Raw {
                    TAG_BIT
                } else {
                    0
                },
        }
    }

    fn range(&self) -> Range<usize> {
        self.start_tag & OFFSET_MASK..self.end_tag & OFFSET_MASK
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.storage[self.range()]
    }

    /// Full shared backing, for retained-storage accounting. A slice can keep
    /// bytes outside its visible range alive; callers can deduplicate backings
    /// by this slice's address without cloning the allocation.
    pub fn backing_bytes(&self) -> &[u8] {
        &self.storage
    }

    pub fn as_str(&self) -> Option<&str> {
        if self.validity() == Validity::Utf8 {
            // Revalidate the safe view; the crate does not need unchecked UTF-8 access.
            std::str::from_utf8(self.as_bytes()).ok()
        } else {
            None
        }
    }

    pub fn validity(&self) -> Validity {
        if self.end_tag & TAG_BIT != 0 {
            Validity::Raw
        } else if self.start_tag & TAG_BIT != 0 {
            Validity::Wtf8
        } else {
            Validity::Utf8
        }
    }
    pub fn len(&self) -> usize {
        self.range().len()
    }
    pub fn is_empty(&self) -> bool {
        self.range().is_empty()
    }

    /// Bounds are checked in bytes; endpoints need not be character boundaries.
    pub fn slice(&self, range: Range<usize>) -> Option<Self> {
        let parent = self.as_bytes();
        let bytes = parent.get(range.clone())?;
        // In a validated UTF-8 view, only continuation bytes lie inside a
        // scalar. Check relative endpoints without rescanning the parent.
        let boundary = |offset| offset == parent.len() || parent[offset] & 0xc0 != 0x80;
        let validity =
            if self.validity() == Validity::Utf8 && boundary(range.start) && boundary(range.end) {
                Validity::Utf8
            } else {
                classify(bytes)
            };
        let start = self.range().start;
        Some(Self::from_validated_range(
            Arc::clone(&self.storage),
            start + range.start..start + range.end,
            validity,
        ))
    }

    pub fn code_points(&self) -> CodePoints<'_> {
        code_points(self.as_bytes())
    }
}

impl Default for JsString {
    fn default() -> Self {
        Self::from_bytes(&[][..])
    }
}

// Borrowed hash-map lookup uses the same byte equality and hashing as JsString.
impl std::borrow::Borrow<[u8]> for JsString {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl AsRef<[u8]> for JsString {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}
impl Eq for JsString {}
impl PartialOrd for JsString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for JsString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}
impl Hash for JsString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

pub fn classify(bytes: &[u8]) -> Validity {
    if std::str::from_utf8(bytes).is_ok() {
        return Validity::Utf8;
    }
    let mut offset = 0;
    while offset < bytes.len() {
        let (rune, width) = decode_rune(&bytes[offset..]);
        if rune == RUNE_ERROR && width == 1 {
            return Validity::Raw;
        }
        offset += width;
    }
    Validity::Wtf8
}
