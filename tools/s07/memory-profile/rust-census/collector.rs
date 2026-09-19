//! Diagnostic only. Appended to a staged tsr_jsstring; never a production API.
//! Walk counts owned heap children, not the enclosing inline value.
use std::{collections::{BTreeMap, HashMap, HashSet}, hash::BuildHasher,
    mem::{size_of, size_of_val}, sync::{Arc, OnceLock}};

#[derive(Default, Debug)]
pub struct Row {
    pub containers: u64,
    pub allocations: u64,
    pub elements: u64,
    pub capacity_elements: u64,
    pub used_payload_bytes: u64,
    pub capacity_payload_bytes: u64,
    pub shared_references: u64,
    pub shared_header_estimate_bytes: u64,
    pub unreported_layout_containers: u64,
}

#[derive(Default, Debug)]
pub struct Collector {
    pub rows: BTreeMap<String, Row>,
    seen_shared: HashSet<usize>,
}
pub type Report = Collector;

pub trait Walk {
    fn walk(&self, census: &mut Collector, category: &str);
}

impl Collector {
    pub fn record(&mut self, category: &str, used: usize, capacity: usize,
                  elements: usize, capacity_elements: usize, allocations: usize) {
        assert!(used <= capacity);
        let row = self.rows.entry(category.to_owned()).or_default();
        row.containers += 1;
        row.allocations += allocations as u64;
        row.elements += elements as u64;
        row.capacity_elements += capacity_elements as u64;
        row.used_payload_bytes += used as u64;
        row.capacity_payload_bytes += capacity as u64;
    }
    pub fn unknown(&mut self, category: &str) {
        self.rows.entry(category.to_owned()).or_default().unreported_layout_containers += 1;
    }
    pub fn shared<T: Walk + ?Sized>(&mut self, value: &Arc<T>, category: &str) {
        // Arc data pointers identify live backing allocations, including empty
        // byte slices. No pointer is reconstructed or dereferenced from its ID.
        let address = Arc::as_ptr(value).cast::<()>().addr();
        let row = self.rows.entry(category.to_owned()).or_default();
        row.shared_references += 1;
        if self.seen_shared.insert(address) {
            let bytes = size_of_val(&**value);
            self.record(category, bytes, bytes, 1, 1, 1);
            // Arc layout is not a public ABI; keep the two-counter estimate
            // separate from known payload and allocator size-class slack.
            self.rows.get_mut(category).unwrap().shared_header_estimate_bytes +=
                (2 * size_of::<usize>()) as u64;
            value.as_ref().walk(self, category);
        }
    }
    pub fn vector<T>(&mut self, value: &Vec<T>, category: &str) {
        self.record(category, value.len() * size_of::<T>(), value.capacity() * size_of::<T>(),
                    value.len(), value.capacity(), usize::from(value.capacity() > 0 && size_of::<T>() > 0));
    }
    pub fn json(&self) -> String {
        // Category strings are generated ASCII identifiers. JSON escaping here
        // still handles all characters without a diagnostic serde dependency.
        fn quoted(value: &str) -> String {
            let mut result = String::from("\"");
            for ch in value.chars() {
                match ch {
                    '"' => result.push_str("\\\""), '\\' => result.push_str("\\\\"),
                    '\n' => result.push_str("\\n"), '\r' => result.push_str("\\r"),
                    '\t' => result.push_str("\\t"),
                    ch if ch < '\u{20}' => result.push_str(&format!("\\u{:04x}", ch as u32)),
                    ch => result.push(ch),
                }
            }
            result.push('"'); result
        }
        let mut result = String::from("{\"schema\":1,\"domain\":\"retained-owned-storage-census\",\"rows\":{");
        for (index, (name, row)) in self.rows.iter().enumerate() {
            if index > 0 { result.push(','); }
            result.push_str(&quoted(name)); result.push(':');
            result.push_str(&format!("{{\"containers\":{},\"allocations\":{},\"elements\":{},\"capacity_elements\":{},\"used_payload_bytes\":{},\"capacity_payload_bytes\":{},\"shared_references\":{},\"shared_header_estimate_bytes\":{},\"unreported_layout_containers\":{}}}",
                row.containers,row.allocations,row.elements,row.capacity_elements,row.used_payload_bytes,
                row.capacity_payload_bytes,row.shared_references,row.shared_header_estimate_bytes,
                row.unreported_layout_containers));
        }
        result.push_str("},\"limitations\":[\"HashMap capacity is the public guaranteed-entry capacity; buckets, control bytes and table padding remain unreported\",\"BTreeMap reports occupied key/value payload only; node capacity, edges and allocator padding remain unreported\",\"Arc counter headers are a separate estimate, not public ABI or allocator usable bytes\",\"Allocator size-class rounding, metadata, free blocks, thread caches, stacks, code and global statics are outside this census\",\"Shared backing payload is charged once to its first owning field encountered; references in other categories do not add bytes\",\"Census allocations and temporary inspection buffers are created after the native snapshot and excluded from the retained graph walk\"]}");
        result
    }
}

macro_rules! no_heap { ($($ty:ty),* $(,)?) => { $(impl Walk for $ty {
    fn walk(&self, _: &mut Collector, _: &str) {}
})* }; }
no_heap!((), bool, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize,
    std::sync::atomic::AtomicU32, std::sync::atomic::AtomicU64, std::sync::atomic::AtomicUsize);
impl<T: ?Sized> Walk for &T { fn walk(&self, _: &mut Collector, _: &str) {} }
impl<T: Walk> Walk for Option<T> {
    fn walk(&self, c: &mut Collector, category: &str) { if let Some(value) = self { value.walk(c, category); } }
}
impl<T: Walk, E: Walk> Walk for Result<T, E> {
    fn walk(&self, c: &mut Collector, category: &str) { match self { Ok(v) => v.walk(c, category), Err(v) => v.walk(c, category) } }
}
impl<T: Walk + ?Sized> Walk for Box<T> {
    fn walk(&self, c: &mut Collector, category: &str) {
        let bytes = size_of_val(&**self); c.record(category, bytes, bytes, 1, 1, usize::from(bytes > 0));
        self.as_ref().walk(c, category);
    }
}
impl<T: Walk + ?Sized> Walk for Arc<T> {
    fn walk(&self, c: &mut Collector, category: &str) { c.shared(self, category); }
}
impl<T: Walk> Walk for [T] {
    fn walk(&self, c: &mut Collector, category: &str) { for value in self { value.walk(c, category); } }
}
impl<T: Walk, const N: usize> Walk for [T; N] {
    fn walk(&self, c: &mut Collector, category: &str) { self.as_slice().walk(c, category); }
}
impl<T: Walk> Walk for Vec<T> {
    fn walk(&self, c: &mut Collector, category: &str) {
        c.vector(self, category); self.as_slice().walk(c, category);
    }
}
impl<K: Walk, V: Walk, S: BuildHasher> Walk for HashMap<K, V, S> {
    fn walk(&self, c: &mut Collector, category: &str) {
        c.record(category, self.len() * size_of::<(K,V)>(), self.capacity() * size_of::<(K,V)>(),
            self.len(), self.capacity(), usize::from(self.capacity() > 0));
        if self.capacity() > 0 { c.unknown(category); }
        for (key,value) in self { key.walk(c, category); value.walk(c, category); }
    }
}
impl<K: Walk, V: Walk> Walk for BTreeMap<K, V> {
    fn walk(&self, c: &mut Collector, category: &str) {
        // Unknown tree node count is NOT claimed as one allocation.
        // std's BTree nodes have separate key/value arrays, not tuple buckets.
        // Per-tuple padding can otherwise make this claimed lower bound larger
        // than the actual occupied key/value payload (e.g. u8/u64).
        let occupied = self.len() * (size_of::<K>() + size_of::<V>());
        c.record(category, occupied, occupied,
            self.len(), self.len(), 0);
        if !self.is_empty() { c.unknown(category); }
        for (key,value) in self { key.walk(c, category); value.walk(c, category); }
    }
}
impl<T: Walk> Walk for OnceLock<T> {
    fn walk(&self, c: &mut Collector, category: &str) {
        if let Some(value) = self.get() { value.walk(c, category); }
    }
}
impl<A: Walk, B: Walk> Walk for (A, B) {
    fn walk(&self, c: &mut Collector, category: &str) { self.0.walk(c, category); self.1.walk(c, category); }
}

#[cfg(test)]
mod census_contracts {
    use super::*;
    #[test]
    fn shared_backing_and_vec_slack_are_distinct() {
        let backing: Arc<[u8]> = Arc::from(&b"abcdef"[..]);
        let mut c = Collector::default(); backing.walk(&mut c, "one"); backing.clone().walk(&mut c, "two");
        assert_eq!(c.rows["one"].capacity_payload_bytes, 6);
        assert_eq!(c.rows["two"].capacity_payload_bytes, 0);
        assert_eq!(c.rows["two"].shared_references, 1);
        let mut values = Vec::with_capacity(9); values.extend([1_u64,2]); values.walk(&mut c,"vector");
        assert_eq!(c.rows["vector"].used_payload_bytes,16);
        assert_eq!(c.rows["vector"].capacity_payload_bytes, values.capacity() as u64*8);
    }
    #[test]
    fn unused_once_lock_and_boxed_variant_size_are_observed_without_initialization() {
        let lazy = OnceLock::<Vec<u64>>::new(); let mut c=Collector::default(); lazy.walk(&mut c,"lazy");
        assert!(lazy.get().is_none()); assert!(c.rows.is_empty());
        Box::new([0_u8;37]).walk(&mut c,"box"); assert_eq!(c.rows["box"].capacity_payload_bytes,37);
    }
    #[test]
    fn jsstring_substrings_charge_the_full_shared_backing_once() {
        let source=crate::JsString::from_bytes(&b"abcdef"[..]);
        let short=source.slice(2..4).unwrap();
        let mut c=Collector::default();short.walk(&mut c,"substring");source.walk(&mut c,"source");
        assert_eq!(c.rows["JsString.storage"].capacity_payload_bytes,6);
        assert_eq!(c.rows["JsString.storage"].allocations,1);
        assert_eq!(c.rows["JsString.storage"].shared_references,2);
    }
    #[test]
    fn tree_payload_excludes_invented_tuple_padding() {
        let tree=BTreeMap::from([(1_u8,2_u64)]);let mut c=Collector::default();
        tree.walk(&mut c,"tree");assert_eq!(c.rows["tree"].used_payload_bytes,9);
        assert_eq!(c.rows["tree"].allocations,0);
        assert_eq!(c.rows["tree"].unreported_layout_containers,1);
    }
}
