//! Text words select a source suffix or a reusable source/owned exception slot.
use super::FieldKey;
use crate::JsString;
use hashbrown::HashMap;
use std::ops::Range;
use ts_jsstring::SourceText;

const EXTENDED: u32 = u32::MAX;
const POOL_LIMIT: usize = (u32::MAX >> 1) as usize;

/// The all-ones start is reserved; unrepresentable source ranges keep ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextEntry {
    start: u32,
    len: u32,
}

impl TextEntry {
    const OWNED: Self = Self {
        start: u32::MAX,
        len: 0,
    };
    const VACANT: Self = Self {
        start: u32::MAX,
        len: 1,
    };

    fn source(value: &JsString, source: &SourceText) -> Option<Self> {
        let bytes = value.as_bytes();
        // As in TokenValue::into_js_string, addresses select a checked range
        // in the retained source. No pointer is reconstructed from an integer.
        let start = bytes
            .as_ptr()
            .addr()
            .checked_sub(source.as_bytes().as_ptr().addr())?;
        let end = start.checked_add(bytes.len())?;
        source.as_bytes().get(start..end)?;
        Self::from_range(start..end)
    }

    fn from_range(range: Range<usize>) -> Option<Self> {
        let start = u32::try_from(range.start).ok()?;
        if start == u32::MAX {
            return None;
        }
        Some(Self {
            start,
            len: u32::try_from(range.end.checked_sub(range.start)?).ok()?,
        })
    }

    fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}

enum PooledText<'a> {
    Source(Range<usize>),
    Owned(&'a JsString),
}

#[derive(Default)]
pub(super) struct TextPool {
    entries: Vec<TextEntry>,
    free: Vec<u32>,
    // Cooked/foreign values and exhausted pool words retain their original Arc.
    owned: HashMap<FieldKey, JsString, std::hash::RandomState>,
}

fn raw_range(word: u32, end: i32, source: &SourceText) -> Range<usize> {
    let end = usize::try_from(end).expect("source-backed identifier has a nonnegative end");
    let start = end
        .checked_sub((word >> 1) as usize)
        .expect("source-backed identifier length");
    assert!(
        end <= source.len(),
        "source-backed identifier ends within source"
    );
    start..end
}

fn matching_raw(bytes: &[u8], end: i32, source: &SourceText) -> Option<u32> {
    let end = usize::try_from(end).ok()?;
    let start = end.checked_sub(bytes.len())?;
    if bytes.len() <= POOL_LIMIT && source.as_bytes().get(start..end)? == bytes {
        Some(u32::try_from(bytes.len()).expect("checked identifier length") << 1)
    } else {
        None
    }
}

impl TextPool {
    fn pooled(&self, key: FieldKey, word: u32) -> PooledText<'_> {
        if word == EXTENDED {
            PooledText::Owned(
                self.owned
                    .get(&key)
                    .expect("extended text word belongs to its field"),
            )
        } else {
            let entry = self
                .entries
                .get((word >> 1) as usize)
                .copied()
                .filter(|entry| *entry != TextEntry::VACANT)
                .expect("text word names an occupied pool slot");
            if entry == TextEntry::OWNED {
                PooledText::Owned(
                    self.owned
                        .get(&key)
                        .expect("text word names an occupied pool slot"),
                )
            } else {
                PooledText::Source(entry.range())
            }
        }
    }

    pub(super) fn bytes<'a>(
        &'a self,
        key: FieldKey,
        word: u32,
        end: i32,
        source: &'a SourceText,
    ) -> &'a [u8] {
        if word & 1 == 0 {
            &source.as_bytes()[raw_range(word, end, source)]
        } else {
            match self.pooled(key, word) {
                PooledText::Source(range) => &source.as_bytes()[range],
                PooledText::Owned(value) => value.as_bytes(),
            }
        }
    }

    pub(super) fn owned(
        &self,
        key: FieldKey,
        word: u32,
        end: i32,
        source: &SourceText,
    ) -> JsString {
        if word & 1 == 0 {
            source
                .slice(raw_range(word, end, source))
                .expect("validated source suffix")
        } else {
            match self.pooled(key, word) {
                PooledText::Source(range) => {
                    source.slice(range).expect("validated pooled source range")
                }
                PooledText::Owned(value) => value.clone(),
            }
        }
    }

    pub(super) fn insert(
        &mut self,
        key: FieldKey,
        value: JsString,
        end: i32,
        source: &SourceText,
    ) -> u32 {
        if crate::AstPayloadStore::is_identifier_text(key.shape, key.field) {
            if let Some(word) = matching_raw(value.as_bytes(), end, source) {
                return word;
            }
        }
        self.insert_pool(key, value, source, POOL_LIMIT)
    }

    fn insert_pool(
        &mut self,
        key: FieldKey,
        value: JsString,
        source: &SourceText,
        limit: usize,
    ) -> u32 {
        let index = if let Some(index) = self.free.pop() {
            index
        } else if self.entries.len() < limit.min(POOL_LIMIT) {
            u32::try_from(self.entries.len()).expect("bounded text pool")
        } else {
            self.owned.insert(key, value);
            return EXTENDED;
        };
        let entry = TextEntry::source(&value, source).unwrap_or_else(|| {
            self.owned.insert(key, value);
            TextEntry::OWNED
        });
        if index as usize == self.entries.len() {
            self.entries.push(entry);
        } else {
            self.entries[index as usize] = entry;
        }
        (index << 1) | 1
    }

    pub(super) fn release(&mut self, key: FieldKey, word: u32) {
        if word & 1 == 0 {
            return;
        }
        if word == EXTENDED {
            self.owned.remove(&key);
        } else {
            let index = word >> 1;
            let slot = self
                .entries
                .get_mut(index as usize)
                .expect("owned text pool slot");
            assert!(
                *slot != TextEntry::VACANT,
                "text pool slot released exactly once"
            );
            if *slot == TextEntry::OWNED {
                self.owned.remove(&key);
            }
            *slot = TextEntry::VACANT;
            self.free.push(index);
        }
    }

    pub(super) fn change_end(
        &mut self,
        key: FieldKey,
        word: u32,
        old_end: i32,
        new_end: i32,
        source: &SourceText,
    ) -> u32 {
        if !crate::AstPayloadStore::is_identifier_text(key.shape, key.field) {
            return word;
        }
        if let Some(raw) = matching_raw(self.bytes(key, word, old_end, source), new_end, source) {
            self.release(key, word);
            return raw;
        }
        if word & 1 != 0 {
            return word;
        }
        let value = self.owned(key, word, old_end, source);
        self.insert_pool(key, value, source, POOL_LIMIT)
    }
}

impl TextPool {
    /// Entry and free-list capacities, the owned-text table and the owned
    /// strings' backings (source-backed entries alias the file text).
    #[cfg(test)]
    pub(super) fn structural_bytes(&self) -> usize {
        self.structural_bytes_with(&mut ts_arena::StorageCensus::default())
    }
    pub(super) fn structural_bytes_with(&self, census: &mut ts_arena::StorageCensus) -> usize {
        self.entries.capacity() * size_of::<TextEntry>()
            + self.free.capacity() * size_of::<u32>()
            + self.owned.allocation_size()
            + self
                .owned
                .values()
                .map(|text| census.text(text.backing_bytes()))
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_shares_text_identity_across_pools_and_type_storage() {
        let source = SourceText::from_loaded_bytes(b"".as_slice());
        let text = JsString::from_bytes(b"shared across display generations".as_slice());
        let mut one = TextPool::default();
        let mut two = TextPool::default();
        one.insert(FieldKey::new(3, 0, 0), text.clone(), -1, &source);
        two.insert(FieldKey::new(3, 0, 0), text.clone(), -1, &source);
        let mut census = ts_arena::StorageCensus::default();
        let first = one.structural_bytes_with(&mut census);
        let second = two.structural_bytes_with(&mut census);
        assert_eq!(
            first - second,
            ts_arena::StorageCensus::arc_slice_bytes::<u8>(text.backing_bytes().len())
        );
        let mut prior = std::collections::HashSet::new();
        prior.insert(text.backing_bytes().as_ptr() as usize);
        let mut census = ts_arena::StorageCensus::new(prior);
        assert_eq!(one.structural_bytes_with(&mut census), second);
    }

    #[test]
    fn census_counts_shared_owned_text_backing_once() {
        let source = SourceText::from_loaded_bytes(b"".as_slice());
        let mut pool = TextPool::default();
        let text = JsString::from_bytes(b"shared synthetic text".as_slice());
        pool.insert(FieldKey::new(3, 0, 0), text.clone(), -1, &source);
        pool.insert(FieldKey::new(3, 1, 0), text.clone(), -1, &source);
        let containers = pool.entries.capacity() * size_of::<TextEntry>()
            + pool.free.capacity() * size_of::<u32>()
            + pool.owned.allocation_size();
        assert_eq!(
            pool.structural_bytes() - containers,
            ts_arena::StorageCensus::arc_slice_bytes::<u8>(text.backing_bytes().len())
        );
    }

    #[test]
    fn final_ranges_release_temporary_text_and_reuse_exception_slots() {
        assert_eq!(std::mem::size_of::<TextEntry>(), 8);
        let last_start = (u32::MAX - 1) as usize;
        assert_eq!(
            TextEntry::from_range(last_start..last_start + 1)
                .unwrap()
                .range(),
            last_start..last_start + 1
        );
        assert!(TextEntry::from_range(u32::MAX as usize..u32::MAX as usize).is_none());
        assert!(TextEntry::from_range(0..u32::MAX as usize + 1).is_none());
        let source = SourceText::from_loaded_bytes(b"alpha beta alpha".as_slice());
        let mut pool = TextPool::default();
        for row in 0..1000 {
            let key = FieldKey::new(1, row, 0);
            let word = pool.insert(key, JsString::from_bytes(b"alpha".as_slice()), -1, &source);
            assert_eq!(word & 1, 1);
            let raw = pool.change_end(key, word, -1, 5, &source);
            assert_eq!(raw, 10);
            assert_eq!(pool.bytes(key, raw, 5, &source), b"alpha");
            assert_eq!(pool.entries.len(), 1);
            assert_eq!(pool.entries[0], TextEntry::VACANT);
            assert!(pool.owned.is_empty());
        }
        let key = FieldKey::new(1, 1001, 0);
        let raw = pool.insert(key, JsString::from_bytes(b"alpha".as_slice()), 5, &source);
        // Moving the range over different bytes preserves semantic text.
        let escaped = pool.change_end(key, raw, 5, 10, &source);
        assert_eq!(escaped & 1, 1);
        assert_eq!(pool.bytes(key, escaped, 10, &source), b"alpha");
        assert!(pool.owned.is_empty());
        let retained = pool.owned(key, escaped, 10, &source);
        assert_eq!(retained.as_bytes().as_ptr(), source.as_bytes().as_ptr());
        let raw_again = pool.change_end(key, escaped, 10, 16, &source);
        assert_eq!(raw_again, raw);
        assert_eq!(pool.bytes(key, raw_again, 16, &source), b"alpha");
        assert_eq!(pool.entries[0], TextEntry::VACANT);

        // Nonidentifier text keeps an explicit range independent of node.end.
        let range_key = FieldKey::new(3, 1002, 0);
        let range_word = pool.insert(range_key, source.slice(6..10).unwrap(), -1, &source);
        assert_eq!(range_word, escaped);
        assert_eq!(pool.entries.len(), 1);
        assert!(pool.owned.is_empty());
        assert_eq!(pool.bytes(range_key, range_word, -1, &source), b"beta");
        assert_eq!(
            pool.change_end(range_key, range_word, -1, 5, &source),
            range_word
        );
        let range_owned = pool.owned(range_key, range_word, 5, &source);
        assert_eq!(
            range_owned.as_bytes().as_ptr(),
            source.as_bytes()[6..].as_ptr()
        );
        pool.release(range_key, range_word);

        // Equal bytes from another allocation must keep that allocation alive.
        let foreign = JsString::from_bytes(b"beta".as_slice());
        let foreign_pointer = foreign.as_bytes().as_ptr();
        assert_ne!(foreign_pointer, source.as_bytes()[6..].as_ptr());
        let foreign_word = pool.insert(range_key, foreign, -1, &source);
        assert_eq!(foreign_word, range_word);
        assert_eq!(pool.owned.len(), 1);
        let foreign_owned = pool.owned(range_key, foreign_word, -1, &source);
        assert_eq!(foreign_owned.as_bytes().as_ptr(), foreign_pointer);
        pool.release(range_key, foreign_word);
        assert!(pool.owned.is_empty());
        drop(pool);
        drop(source);
        assert_eq!(retained.as_bytes(), b"alpha");
        assert_eq!(range_owned.as_bytes(), b"beta");
        assert_eq!(foreign_owned.as_bytes(), b"beta");
    }

    #[test]
    fn decoded_wtf8_and_full_pool_escape_preserve_bytes_and_field_identity() {
        let source = SourceText::from_loaded_bytes(b"\\u0061 #a".as_slice());
        let mut pool = TextPool::default();
        let first = FieldKey::new(1, 0, 0);
        let second = FieldKey::new(2, 0, 0);
        let first_word = pool.insert(first, JsString::from_bytes(b"a".as_slice()), 6, &source);
        assert_eq!(first_word & 1, 1);
        let second_word = pool.insert_pool(
            second,
            JsString::from_bytes(b"\xed\xa0\x80\xff".as_slice()),
            &source,
            0,
        );
        assert_eq!(second_word, EXTENDED);
        assert_eq!(pool.bytes(first, first_word, 6, &source), b"a");
        let owned = pool.owned(second, second_word, -1, &source);
        assert_eq!(owned.as_bytes(), b"\xed\xa0\x80\xff");
        assert_eq!(
            owned.as_bytes().as_ptr(),
            pool.bytes(second, second_word, -1, &source).as_ptr()
        );
        assert_eq!(owned.validity(), ts_jsstring::Validity::Raw);
        let third = FieldKey::new(2, 1, 0);
        let third_word = pool.insert_pool(third, source.slice(7..9).unwrap(), &source, 0);
        assert_eq!(third_word, EXTENDED);
        assert_eq!(pool.bytes(third, third_word, -1, &source), b"#a");
        assert_eq!(
            pool.bytes(second, second_word, -1, &source),
            b"\xed\xa0\x80\xff"
        );
        pool.release(first, first_word);
        pool.release(second, second_word);
        assert_eq!(pool.bytes(third, third_word, -1, &source), b"#a");
        pool.release(third, third_word);
        assert!(pool.owned.is_empty());
        drop(pool);
        drop(source);
        assert_eq!(owned.as_bytes(), b"\xed\xa0\x80\xff");

        // Source range entries retain each view's classification, including
        // an empty view and a slice through the middle of a UTF-8 scalar.
        let source = SourceText::from_bytes(b"\xef\xbb\xbfx\xc3\xa9\xed\xa0\x80\xffy".as_slice());
        let mut pool = TextPool::default();
        let mut retained = Vec::new();
        for (row, range) in [0..0, 1..3, 2..3, 3..6, 6..7, 0..8].into_iter().enumerate() {
            let value = source.slice(range).unwrap();
            let pointer = value.as_bytes().as_ptr();
            let bytes = value.as_bytes().to_vec();
            let validity = value.validity();
            let key = FieldKey::new(3, row as u32, 0);
            let word = pool.insert(key, value, -1, &source);
            assert!(pool.owned.is_empty());
            let owned = pool.owned(key, word, -1, &source);
            assert_eq!(owned.as_bytes().as_ptr(), pointer);
            assert_eq!(owned.validity(), validity);
            pool.release(key, word);
            retained.push((owned, bytes, validity));
        }
        assert_eq!(pool.entries.len(), 1);
        drop(pool);
        drop(source);
        for (owned, bytes, validity) in retained {
            assert_eq!(owned.as_bytes(), bytes);
            assert_eq!(owned.validity(), validity);
        }
    }
}
