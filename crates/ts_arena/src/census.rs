//! One allocation-identity context for a complete retained AST closure.
use crate::{ArenaId, FileId};
use std::collections::HashSet;

#[derive(Default)]
pub struct StorageCensus {
    allocations: HashSet<usize>,
    owners: HashSet<ArenaId>,
    bound_owners: HashSet<ArenaId>,
    bound_text: HashSet<usize>,
}
impl StorageCensus {
    pub fn new(allocations: HashSet<usize>) -> Self {
        Self {
            allocations,
            ..Self::default()
        }
    }
    pub fn exclude_owner(&mut self, id: ArenaId) {
        self.owners.insert(id);
        self.bound_owners.insert(id);
    }
    pub(crate) fn is_bound_owner(&self, id: FileId) -> bool {
        self.bound_owners.contains(&id.arena())
    }
    pub fn exclude_text(&mut self, backing: &[u8]) {
        self.bound_text.insert(backing.as_ptr() as usize);
    }
    pub fn into_allocations(self) -> HashSet<usize> {
        self.allocations
    }
    pub(crate) fn owner(&mut self, id: FileId) -> bool {
        self.owners.insert(id.arena())
    }
    pub fn allocation(&mut self, address: usize, bytes: usize) -> usize {
        if self.allocations.insert(address) {
            bytes
        } else {
            0
        }
    }
    /// Requested Arc allocation layout, including counters and tail padding.
    pub fn arc_slice_bytes<T>(len: usize) -> usize {
        std::alloc::Layout::new::<[usize; 2]>()
            .extend(std::alloc::Layout::array::<T>(len).expect("allocated Arc slice layout"))
            .expect("allocated Arc header layout")
            .0
            .pad_to_align()
            .size()
    }
    pub fn text(&mut self, backing: &[u8]) -> usize {
        let address = backing.as_ptr() as usize;
        if self.bound_text.contains(&address) {
            return 0;
        }
        self.allocation(address, Self::arc_slice_bytes::<u8>(backing.len()))
    }
}
