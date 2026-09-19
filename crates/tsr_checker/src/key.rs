//! Cache keys (`keyBuilder` and the `get*Key` functions in
//! `tsc/internal/checker/checker.go`).
//!
//! Upstream writes a byte stream (type ids, symbol ids, node ids, lengths, text)
//! and hashes it with xxh3 into a 128-bit `CacheHashKey`, ignoring collisions.
//! This port writes the same byte stream and keeps the bytes as the key: no
//! collision can substitute one type for another, and no hashing dependency is
//! needed yet. The census charges the key backing to the caches that hold it;
//! P1's storage measurement decides whether a 128-bit hash of the same stream
//! replaces the bytes (a vetted xxh3 crate is an expected dependency, ADR 0017).
//!
//! Symbol identities in keys are upstream's lazily assigned runtime ids
//! (`ast.GetSymbolId`); node identities are the packed storage id, which is
//! unique for the checker's lifetime. Neither is meant to reproduce Go's hash
//! bytes; keys only have to be unique within one checker.

use crate::TypeId;
use ts_arena::NodeId;

/// The key bytes upstream hashes.
pub type CacheKey = Box<[u8]>;

#[derive(Default)]
pub(crate) struct KeyBuilder {
    bytes: Vec<u8>,
}

impl KeyBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.hash
    pub fn finish(self) -> CacheKey {
        self.bytes.into_boxed_slice()
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeByte
    pub fn write_byte(&mut self, byte: u8) {
        self.bytes.push(byte);
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeString
    pub fn write_string(&mut self, text: &[u8]) {
        self.bytes.extend_from_slice(text);
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeUint32
    pub fn write_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    // port: tsc/internal/checker/checker.go:keyBuilder.writeUint64
    pub fn write_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
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
