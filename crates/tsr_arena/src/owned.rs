//! Exclusive storage embedded in an outer owner, without an owning back-reference.
//! Publishing a concrete result restricts access to immutable borrows; this generic
//! primitive does not validate edges inside arbitrary payloads.

use crate::{arena::Arena, ArenaId, AuxId, Counters, Error, SymbolId};

macro_rules! owned_arena {
    ($name:ident, $id:ident) => {
        pub struct $name<T>(Arena<T>);
        impl<T> $name<T> {
            pub fn new(counters: &Counters) -> Self {
                Self(Arena::new(counters))
            }
            pub fn id(&self) -> ArenaId {
                self.0.id
            }
            pub fn len(&self) -> usize {
                self.0.len()
            }
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }
            /// Reserved record bytes and directory bytes, for storage censuses.
            pub fn structural_bytes(&self) -> usize {
                self.0.structural_bytes()
            }
            pub fn push(&mut self, value: T) -> $id {
                $id::new(self.0.id, self.0.push(value))
            }
            pub fn get(&self, id: $id) -> Result<&T, Error> {
                self.0.get(id.arena(), id.slot())
            }
            pub fn get_mut(&mut self, id: $id) -> Result<&mut T, Error> {
                self.0.get_mut(id.arena(), id.slot())
            }
            /// Resolve an index relative to this arena, retaining safe bounds checks.
            /// Call `get` when importing a full identity from another context.
            #[inline]
            pub fn get_slot(&self, slot: u32) -> Result<&T, Error> {
                self.0.get_slot(slot)
            }
            /// Mutably resolve an index relative to this arena. This does not
            /// establish that an externally supplied identity belongs here.
            #[inline]
            pub fn get_slot_mut(&mut self, slot: u32) -> Result<&mut T, Error> {
                self.0.get_slot_mut(slot)
            }
            pub fn iter(&self) -> impl Iterator<Item = ($id, &T)> {
                self.0
                    .values()
                    .enumerate()
                    .map(|(index, value)| ($id::new(self.0.id, index as u32 + 1), value))
            }
        }
        impl<T> std::fmt::Debug for $name<T> {
            fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                out.debug_struct(stringify!($name))
                    .field("id", &self.id())
                    .field("len", &self.len())
                    .finish_non_exhaustive()
            }
        }
    };
}
owned_arena!(OwnedArena, AuxId);
owned_arena!(SymbolArena, SymbolId);

impl<T> SymbolArena<T> {
    /// Storage numbered with an identity reserved by a `CheckerIdentity`, which
    /// hands each reservation out exactly once.
    pub(crate) fn with_id(id: ArenaId, counters: &Counters) -> Self {
        Self(Arena::with_id(id, counters))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_slots_retain_bounds_checks_and_values_across_growth() {
        let counters = Counters::new();
        let mut arena = OwnedArena::new(&counters);
        let first = arena.push(7);
        for value in 0..600 {
            arena.push(value);
        }
        *arena.get_slot_mut(first.slot()).unwrap() = 42;
        assert_eq!(arena.get(first), Ok(&42));
        assert_eq!(arena.get_slot(first.slot()), Ok(&42));
        assert_eq!(arena.get_slot(0), Err(Error::InvalidSlot));
        assert_eq!(arena.get_slot(u32::MAX), Err(Error::InvalidSlot));
        assert_eq!(arena.get_slot_mut(u32::MAX), Err(Error::InvalidSlot));
        assert_eq!(arena.get_slot(602), Err(Error::InvalidSlot));
    }

    #[test]
    fn owned_arenas_reject_foreign_and_unpublished_slots_and_never_recycle() {
        let counters = Counters::new();
        let before = counters.snapshot();
        let stale = {
            let mut first = SymbolArena::new(&counters);
            let id = first.push(1);
            assert_eq!(*first.get(id).unwrap(), 1);
            *first.get_mut(id).unwrap() = 2;
            assert_eq!(
                first.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
                [2]
            );
            let unpublished = SymbolId::new(first.id(), 2);
            assert!(matches!(first.get(unpublished), Err(Error::InvalidSlot)));
            let mut second = SymbolArena::new(&counters);
            second.push(3);
            assert!(matches!(second.get(id), Err(Error::WrongOwner)));
            assert!(matches!(second.get_mut(id), Err(Error::WrongOwner)));
            id
        };
        assert_eq!(counters.snapshot(), before);
        let mut replacement = SymbolArena::new(&counters);
        assert_ne!(replacement.push(4).arena(), stale.arena());
        assert!(matches!(replacement.get(stale), Err(Error::WrongOwner)));
        let mut aux = OwnedArena::new(&counters);
        let id = aux.push(5);
        assert_eq!(*aux.get(id).unwrap(), 5);
        let foreign = OwnedArena::<i32>::new(&counters);
        assert!(matches!(foreign.get(id), Err(Error::WrongOwner)));
    }
}
