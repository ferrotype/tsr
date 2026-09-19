//! Multiplicative hasher for id- and name-keyed tables. SipHash's flooding
//! resistance buys nothing for arena ids, small integers and symbol names, and
//! it was a measurable share of every table lookup in the checker.
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);
impl FastHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}
impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Whole words straight from the slice, and the tail folded by shifts:
        // no per-chunk conversion check and no library copy for a few bytes.
        // Symbol names are hashed on every table lookup, so this is hot.
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.add(u64::from_le_bytes(*chunk));
        }
        if !rest.is_empty() {
            let mut word = 0u64;
            for (index, &byte) in rest.iter().enumerate() {
                word |= u64::from(byte) << (8 * index);
            }
            self.add(word ^ (rest.len() as u64) << 56);
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
}
pub type FastState = BuildHasherDefault<FastHasher>;
