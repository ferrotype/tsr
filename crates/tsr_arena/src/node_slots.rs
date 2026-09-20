//! Dense, paged side fields for allocator-issued node identities. This table
//! does not validate node existence or retain node storage; its caller must do
//! both before installing an edge. The primary arena avoids storing each key.
use crate::{ArenaId, NodeId};
use std::collections::HashMap;

const PAGE: usize = 256;

#[derive(Debug)]
pub struct NodeSlots<T> {
    arena: ArenaId,
    pages: Vec<Option<Box<[Option<T>; PAGE]>>>,
    foreign: HashMap<NodeId, T>,
    len: usize,
}

impl<T> NodeSlots<T> {
    pub fn new(arena: ArenaId) -> Self {
        Self {
            arena,
            pages: Vec::new(),
            foreign: HashMap::new(),
            len: 0,
        }
    }
    pub fn get(&self, id: &NodeId) -> Option<&T> {
        if id.arena() != self.arena {
            return self.foreign.get(id);
        }
        let index = id.slot() as usize - 1;
        self.pages
            .get(index / PAGE)?
            .as_ref()?
            .get(index % PAGE)?
            .as_ref()
    }
    pub fn insert(&mut self, id: NodeId, value: T) -> Option<T> {
        let previous = if id.arena() == self.arena {
            let index = id.slot() as usize - 1;
            if self.pages.len() <= index / PAGE {
                self.pages.resize_with(index / PAGE + 1, || None);
            }
            let page = self.pages[index / PAGE]
                .get_or_insert_with(|| Box::new(std::array::from_fn(|_| None)));
            page[index % PAGE].replace(value)
        } else {
            self.foreign.insert(id, value)
        };
        if previous.is_none() {
            self.len += 1;
        }
        previous
    }
    pub fn remove(&mut self, id: &NodeId) -> Option<T> {
        let previous = if id.arena() == self.arena {
            let index = id.slot() as usize - 1;
            self.pages
                .get_mut(index / PAGE)?
                .as_mut()?
                .get_mut(index % PAGE)?
                .take()
        } else {
            self.foreign.remove(id)
        };
        if previous.is_some() {
            self.len -= 1;
        }
        previous
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn capacity(&self) -> usize {
        self.pages.iter().filter(|page| page.is_some()).count() * PAGE + self.foreign.capacity()
    }
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &T)> {
        self.pages
            .iter()
            .enumerate()
            .flat_map(move |(page_index, page)| {
                page.iter().flat_map(move |page| {
                    page.iter().enumerate().filter_map(move |(offset, value)| {
                        // Every occupied slot was inserted with a nonzero allocated ID.
                        value.as_ref().map(|value| {
                            (
                                NodeId::new(self.arena, (page_index * PAGE + offset + 1) as u32),
                                value,
                            )
                        })
                    })
                })
            })
            .chain(self.foreign.iter().map(|(&id, value)| (id, value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Counters, FileBuilder, Node};
    #[test]
    fn holes_overwrites_removals_and_other_arenas_preserve_exact_identities() {
        let counters = Counters::new();
        let mut first = FileBuilder::<u32>::new((&b""[..]).into(), &counters);
        let ids: Vec<_> = (0..1025).map(|n| first.push(Node::new(0, n))).collect();
        let mut second = FileBuilder::<u32>::new((&b""[..]).into(), &counters);
        let foreign = second.push(Node::new(0, 99));
        let mut slots = NodeSlots::new(ids[0].arena());
        assert!(slots.is_empty());
        for &index in &[0, 255, 256, 768, 1024] {
            assert_eq!(slots.insert(ids[index], index), None);
        }
        assert_eq!(slots.insert(foreign, 99), None);
        assert_eq!(slots.insert(ids[256], 42), Some(256));
        assert_eq!(slots.remove(&ids[255]), Some(255));
        assert_eq!(slots.remove(&ids[255]), None);
        assert_eq!(slots.get(&ids[255]), None);
        assert_eq!(slots.get(&ids[500]), None);
        assert_eq!(slots.get(&ids[256]), Some(&42));
        let actual: std::collections::BTreeMap<_, _> =
            slots.iter().map(|(id, &value)| (id, value)).collect();
        let expected = [
            (ids[0], 0),
            (ids[256], 42),
            (ids[768], 768),
            (ids[1024], 1024),
            (foreign, 99),
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
        assert_eq!(slots.len(), 5);
    }
}
