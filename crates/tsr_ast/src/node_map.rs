//! Maps keyed only by allocator-issued NodeIds. Source names continue to use
//! randomized string hashing; these internal numeric identities need no byte
//! string hashing on every AST lookup.
use crate::NodeId;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
};

pub(crate) type NodeMap<T> = HashMap<NodeId, T, BuildHasherDefault<NodeHasher>>;

#[derive(Default)]
pub(crate) struct NodeHasher(u64);
impl Hasher for NodeHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }
}
