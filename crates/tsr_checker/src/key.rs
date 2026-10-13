//! Cache keys (`keyBuilder` and the `get*Key` functions in
//! `tsc/internal/checker/checker.go`).
//!
//! Upstream writes a byte stream (type ids, symbol ids, node ids, lengths, text)
//! and hashes it with xxh3 into a 128-bit `CacheHashKey`, ignoring collisions.
//! This port writes the same byte stream and keeps the bytes as the key: no
//! collision can substitute one type for another. Common short keys stay inline
//! during construction and storage, including cache hits. Longer keys own their
//! bytes; the census charges that backing separately from the map buckets.
//!
//! Symbol identities in keys are upstream's lazily assigned runtime ids
//! (`ast.GetSymbolId`); node identities are the packed storage id, which is
//! unique for the checker's lifetime. Neither is meant to reproduce Go's hash
//! bytes; keys only have to be unique within one checker.

use crate::TypeId;
use std::borrow::Borrow;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use tsr_arena::NodeId;

// The length, bytes and enum tag fit beside a boxed slice in 24 bytes on a
// 64-bit target. A larger inline array would grow every map key to 32 bytes.
// This covers simple relations (13 bytes), no-alias instantiations (5), and
// type lists of up to three elements (20).
const INLINE_CAPACITY: usize = 22;

/// The key bytes upstream hashes.
#[derive(Clone)]
pub struct CacheKey(KeyBytes);

#[derive(Clone)]
enum KeyBytes {
    Inline {
        len: u8,
        bytes: [u8; INLINE_CAPACITY],
    },
    Heap(Box<[u8]>),
}

impl CacheKey {
    /// The map or vector already charges the inline bytes in its own storage.
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn heap_bytes(&self) -> usize {
        match &self.0 {
            KeyBytes::Inline { .. } => 0,
            KeyBytes::Heap(bytes) => bytes.len(),
        }
    }
}

impl Deref for CacheKey {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match &self.0 {
            KeyBytes::Inline { len, bytes } => &bytes[..usize::from(*len)],
            KeyBytes::Heap(bytes) => bytes,
        }
    }
}

impl AsRef<[u8]> for CacheKey {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl Borrow<[u8]> for CacheKey {
    fn borrow(&self) -> &[u8] {
        self
    }
}

impl std::fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Eq for CacheKey {}

impl PartialOrd for CacheKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CacheKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

pub(crate) struct KeyBuilder {
    bytes: KeyBuffer,
}

enum KeyBuffer {
    Inline {
        len: u8,
        bytes: [u8; INLINE_CAPACITY],
    },
    Heap(Vec<u8>),
}

impl Default for KeyBuilder {
    fn default() -> Self {
        Self {
            bytes: KeyBuffer::Inline {
                len: 0,
                bytes: [0; INLINE_CAPACITY],
            },
        }
    }
}

impl KeyBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.hash
    pub fn finish(self) -> CacheKey {
        CacheKey(match self.bytes {
            KeyBuffer::Inline { len, bytes } => KeyBytes::Inline { len, bytes },
            KeyBuffer::Heap(bytes) => KeyBytes::Heap(bytes.into_boxed_slice()),
        })
    }

    #[inline]
    fn extend(&mut self, input: &[u8]) {
        match &mut self.bytes {
            KeyBuffer::Inline { len, bytes } => {
                let start = usize::from(*len);
                if input.len() <= INLINE_CAPACITY - start {
                    let end = start + input.len();
                    bytes[start..end].copy_from_slice(input);
                    *len = end as u8;
                } else {
                    let mut heap =
                        Vec::with_capacity((start + input.len()).max(INLINE_CAPACITY * 2));
                    heap.extend_from_slice(&bytes[..start]);
                    heap.extend_from_slice(input);
                    self.bytes = KeyBuffer::Heap(heap);
                }
            }
            KeyBuffer::Heap(bytes) => bytes.extend_from_slice(input),
        }
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeByte
    pub fn write_byte(&mut self, byte: u8) {
        self.extend(&[byte]);
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeString
    pub fn write_string(&mut self, text: &[u8]) {
        self.extend(text);
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeUint32
    pub fn write_u32(&mut self, value: u32) {
        self.extend(&value.to_le_bytes());
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeUint64
    pub fn write_u64(&mut self, value: u64) {
        self.extend(&value.to_le_bytes());
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeInt
    pub fn write_int(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    /// `writeSymbol` takes the symbol's runtime id; the caller resolves it
    /// because only the checker knows which arena a symbol lives in.
    // port: tsc/internal/checker/checker.go:keyBuilder.writeSymbol
    pub fn write_symbol(&mut self, runtime_id: u64) {
        self.write_u64(runtime_id);
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeType
    pub fn write_type(&mut self, t: TypeId) {
        self.write_u32(t.get());
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeTypes
    pub fn write_types(&mut self, types: &[TypeId]) {
        self.write_int(types.len());
        for t in types {
            self.write_type(*t);
        }
    }

    /// An alias is its symbol's runtime id and its type arguments.
    // port: tsc/internal/checker/checker.go:keyBuilder.writeAlias
    pub fn write_alias(&mut self, alias: Option<(u64, &[TypeId])>) {
        match alias {
            Some((symbol, type_arguments)) => {
                self.write_byte(1);
                self.write_symbol(symbol);
                self.write_types(type_arguments);
            }
            None => self.write_byte(0),
        }
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeNodeId
    pub fn write_node_id(&mut self, id: u64) {
        self.write_u64(id);
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeNode
    pub fn write_node(&mut self, node: Option<NodeId>) {
        if let Some(node) = node {
            self.write_node_id(node.bits());
        }
    }
}

// port: tsc/internal/checker/checker.go:getTypeListKey
pub(crate) fn type_list_key(types: &[TypeId]) -> CacheKey {
    let mut builder = KeyBuilder::new();
    builder.write_types(types);
    builder.finish()
}

// port: tsc/internal/checker/checker.go:getTemplateTypeKey
pub(crate) fn template_type_key(texts: &[impl AsRef<[u8]>], types: &[TypeId]) -> CacheKey {
    let mut builder = KeyBuilder::new();
    builder.write_types(types);
    builder.write_byte(b'|');
    for text in texts {
        builder.write_int(text.as_ref().len());
    }
    builder.write_byte(b'|');
    for text in texts {
        builder.write_string(text.as_ref());
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct HashWrites(Vec<Vec<u8>>);

    impl Hasher for HashWrites {
        fn finish(&self) -> u64 {
            0
        }

        fn write(&mut self, bytes: &[u8]) {
            self.0.push(bytes.to_vec());
        }
    }

    fn hash_writes(value: &(impl Hash + ?Sized)) -> Vec<Vec<u8>> {
        let mut state = HashWrites::default();
        value.hash(&mut state);
        state.0
    }

    #[test]
    fn inline_and_spilled_keys_preserve_bytes_across_write_boundaries() {
        for len in [
            0,
            1,
            INLINE_CAPACITY - 1,
            INLINE_CAPACITY,
            INLINE_CAPACITY + 1,
            64,
            193,
        ] {
            let expected: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let mut whole = KeyBuilder::new();
            whole.write_string(&expected);
            let whole = whole.finish();
            let mut bytewise = KeyBuilder::new();
            for &byte in &expected {
                bytewise.write_byte(byte);
            }
            bytewise.write_string(&[]);
            let bytewise = bytewise.finish();
            let mut chunked = KeyBuilder::new();
            for chunk in expected.chunks(7) {
                chunked.write_string(chunk);
            }
            let chunked = chunked.finish();

            assert_eq!(whole.as_ref(), expected);
            assert_eq!(whole, bytewise);
            assert_eq!(whole, chunked);
            assert_eq!(hash_writes(&whole), hash_writes(expected.as_slice()));
            assert_eq!(hash_writes(&whole), hash_writes(&bytewise));
            assert_eq!(hash_writes(&whole), hash_writes(&chunked));
            assert_eq!(
                whole.heap_bytes(),
                if len <= INLINE_CAPACITY { 0 } else { len }
            );
            assert_eq!(bytewise.heap_bytes(), whole.heap_bytes());
            assert_eq!(chunked.heap_bytes(), whole.heap_bytes());
            assert_eq!(whole.clone(), whole);
        }
    }

    #[test]
    fn key_equality_order_and_hash_ignore_storage_representation() {
        let expected = [0, 255, 0, 17];
        let mut builder = KeyBuilder::new();
        builder.write_string(&expected);
        let inline = builder.finish();
        // Construct the other representation directly: equality and hashing
        // must observe bytes even if construction later changes its cutoff.
        let heap = CacheKey(KeyBytes::Heap(expected.into()));
        assert_eq!(inline, heap);
        assert_eq!(inline.cmp(&heap), Ordering::Equal);
        assert_eq!(hash_writes(&inline), hash_writes(&heap));
        assert_eq!(hash_writes(&inline), hash_writes(&expected[..]));

        let mut shorter = KeyBuilder::new();
        shorter.write_string(&expected[..3]);
        let shorter = shorter.finish();
        assert!(shorter < inline);
        assert_ne!(shorter, inline);
        assert_ne!(hash_writes(&shorter), hash_writes(&inline));

        let mut cache = crate::types::Map::default();
        assert_eq!(cache.insert(inline, 1), None);
        assert_eq!(cache.insert(heap, 2), Some(1));
        assert_eq!(cache.get(expected.as_slice()), Some(&2));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn typed_writes_cross_inline_boundary_without_changing_the_stream() {
        let mut builder = KeyBuilder::new();
        builder.write_string(&[0xa5; INLINE_CAPACITY - 1]);
        builder.write_u32(0x1234_5678);
        builder.write_u64(0x0123_4567_89ab_cdef);
        let key = builder.finish();
        let mut expected = vec![0xa5; INLINE_CAPACITY - 1];
        expected.extend_from_slice(&0x1234_5678_u32.to_le_bytes());
        expected.extend_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
        assert_eq!(key.as_ref(), expected);
        assert_eq!(key.heap_bytes(), expected.len());
        assert_eq!(hash_writes(&key), hash_writes(&expected[..]));
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn inline_key_fits_three_machine_words() {
        assert_eq!(size_of::<CacheKey>(), 24);
    }

    #[test]
    fn keys_are_the_upstream_byte_stream() {
        let one = TypeId::new(1).unwrap();
        let two = TypeId::new(2).unwrap();
        let key = type_list_key(&[one, two]);
        assert_eq!(
            &key[..],
            [2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0],
            "length as a little-endian u64, then u32 type ids"
        );
        assert_ne!(type_list_key(&[two, one]), key, "order is part of the key");
        let mut with_alias = KeyBuilder::new();
        with_alias.write_alias(Some((7, &[one])));
        let mut without = KeyBuilder::new();
        without.write_alias(None);
        assert_eq!(&without.finish()[..], [0]);
        assert_eq!(
            &with_alias.finish()[..],
            [1, 7, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]
        );
        let template = template_type_key(&[b"a".as_slice(), b"".as_slice()], &[one]);
        assert_eq!(template.len(), 8 + 4 + 1 + 16 + 1 + 1);
    }
}
