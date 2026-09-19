use crate::{
    counters::Track,
    ids::{allocate_slot, next_arena},
    ArenaId, Counters, Error,
};

/// Exclusive construction storage. Only its owning builder exposes mutation.
pub(crate) struct Arena<T> {
    pub(crate) id: ArenaId,
    pages: Vec<Page<T>>,
    len: usize,
    counters: Counters,
}

struct Page<T> {
    values: Vec<T>,
    _allocation: Track,
}

// Small owners pay for two slots initially. Once a page reaches 256 slots,
// subsequent pages keep that bound rather than recopying the entire arena.
// Slot identities remain contiguous and lookup is constant time.
fn page_position(index: usize) -> (usize, usize) {
    if index >= 510 {
        return (8 + (index - 510) / 256, (index - 510) % 256);
    }
    let page = (usize::BITS - 2 - (index + 2).leading_zeros()) as usize;
    (page, index - ((2 << page) - 2))
}

impl<T> Arena<T> {
    pub(crate) fn new(counters: &Counters) -> Self {
        Self::with_id(next_arena(), counters)
    }

    /// Storage for an identity reserved elsewhere. Callers hand out each reserved
    /// identity exactly once; `CheckerIdentity` enforces that for checkers.
    pub(crate) fn with_id(id: ArenaId, counters: &Counters) -> Self {
        Self {
            id,
            pages: Vec::new(),
            len: 0,
            counters: counters.clone(),
        }
    }

    pub(crate) fn push(&mut self, value: T) -> u32 {
        let slot = allocate_slot(self.len);
        let (page, offset) = page_position(self.len);
        if page == self.pages.len() {
            let capacity = if page < 8 { 2 << page } else { 256 };
            self.pages.push(Page {
                values: Vec::with_capacity(capacity),
                _allocation: self.counters.allocation(),
            });
        }
        debug_assert_eq!(self.pages[page].values.len(), offset);
        self.pages[page].values.push(value);
        self.len += 1;
        slot
    }

    pub(crate) fn get(&self, arena: ArenaId, slot: u32) -> Result<&T, Error> {
        if arena != self.id {
            return Err(Error::WrongOwner);
        }
        self.get_slot(slot)
    }

    pub(crate) fn get_slot(&self, slot: u32) -> Result<&T, Error> {
        let index = slot.checked_sub(1).ok_or(Error::InvalidSlot)? as usize;
        if index >= self.len {
            return Err(Error::InvalidSlot);
        }
        let (page, offset) = page_position(index);
        self.pages[page]
            .values
            .get(offset)
            .ok_or(Error::InvalidSlot)
    }

    /// Bytes the page vectors hold or reserve, plus the page directory; the
    /// storage census charges these, not `len * size_of::<T>()`.
    pub(crate) fn structural_bytes(&self) -> usize {
        self.pages.capacity() * size_of::<Page<T>>()
            + self
                .pages
                .iter()
                .map(|page| page.values.capacity() * size_of::<T>())
                .sum::<usize>()
    }

    pub(crate) fn counters(&self) -> &Counters {
        &self.counters
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.pages.iter().flat_map(|page| page.values.iter())
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get_mut(&mut self, arena: ArenaId, slot: u32) -> Result<&mut T, Error> {
        if arena != self.id {
            return Err(Error::WrongOwner);
        }
        self.get_slot_mut(slot)
    }

    pub(crate) fn get_slot_mut(&mut self, slot: u32) -> Result<&mut T, Error> {
        let index = slot.checked_sub(1).ok_or(Error::InvalidSlot)? as usize;
        if index >= self.len {
            return Err(Error::InvalidSlot);
        }
        let (page, offset) = page_position(index);
        self.pages[page]
            .values
            .get_mut(offset)
            .ok_or(Error::InvalidSlot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_preserves_every_slot_and_page_boundary_without_copying_prior_pages() {
        let counters = Counters::new();
        let before = counters.snapshot();
        {
            let mut arena = Arena::new(&counters);
            let mut previous_addresses = Vec::new();
            for value in 0..4096 {
                let slot = arena.push(value);
                assert_eq!(slot, value + 1);
                previous_addresses.push(std::ptr::from_ref(arena.get_slot(slot).unwrap()));
                for &index in &[
                    0, 1, 2, 5, 6, 13, 14, 29, 30, 61, 62, 125, 126, 253, 254, 509, 510, 765, 766,
                    1021,
                ] {
                    if index <= value {
                        assert_eq!(*arena.get_slot(index + 1).unwrap(), index);
                        assert_eq!(
                            std::ptr::from_ref(arena.get_slot(index + 1).unwrap()),
                            previous_addresses[index as usize]
                        );
                    }
                }
            }
            assert_eq!(
                arena.values().copied().collect::<Vec<_>>(),
                (0..4096).collect::<Vec<_>>()
            );
            assert_eq!(arena.get_slot(0), Err(Error::InvalidSlot));
            assert_eq!(arena.get_slot(4097), Err(Error::InvalidSlot));
            *arena.get_mut(arena.id, 511).unwrap() = 42;
            assert_eq!(arena.get_slot(511), Ok(&42));
        }
        assert_eq!(counters.snapshot(), before);
    }
}
