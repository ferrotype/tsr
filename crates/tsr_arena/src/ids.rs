use crate::Error;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicU64, Ordering};

// Counter values beyond u32::MAX remain an exhausted sentinel, never a recycled id.
static NEXT_ARENA: AtomicU64 = AtomicU64::new(1);

pub(crate) fn checked_arena(counter: &AtomicU64) -> Result<ArenaId, Error> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            if next == 0 || next > u64::from(u32::MAX) {
                None
            } else {
                Some(next + 1)
            }
        })
        .map_err(|_| Error::InvalidId)
        .and_then(|id| ArenaId::from_raw(id as u32))
}

pub(crate) fn next_arena() -> ArenaId {
    checked_arena(&NEXT_ARENA).unwrap_or_else(|_| {
        eprintln!("ts_arena: process arena-id space exhausted; refusing id reuse");
        std::process::abort()
    })
}

/// A process-unique arena identity; zero is never live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ArenaId(NonZeroU32);
impl ArenaId {
    pub fn get(self) -> u32 {
        self.0.get()
    }
    fn from_raw(value: u32) -> Result<Self, Error> {
        NonZeroU32::new(value).map(Self).ok_or(Error::InvalidId)
    }
}

/// File identity is its core node arena identity; it does not retain the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub(crate) ArenaId);
impl FileId {
    pub fn arena(self) -> ArenaId {
        self.0
    }
}

macro_rules! packed_id {
    ($name:ident) => {
        #[doc = "A non-owning packed arena/slot identity. Both full 32-bit fields are nonzero."]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(NonZeroU64);
        impl $name {
            /// Reconstruct a non-owning identity in an existing namespace.
            /// This checks the nonzero slot, not liveness, bounds or retention.
            pub fn from_parts(arena: ArenaId, slot: u32) -> Result<Self, Error> {
                Self::from_bits((u64::from(arena.get()) << 32) | u64::from(slot))
            }
            /// Decode a storage identity, not an ownership proof or an API wire handle.
            pub(crate) fn from_bits(bits: u64) -> Result<Self, Error> {
                if bits >> 32 == 0 || bits as u32 == 0 {
                    return Err(Error::InvalidId);
                }
                NonZeroU64::new(bits).map(Self).ok_or(Error::InvalidId)
            }
            pub fn bits(self) -> u64 {
                self.0.get()
            }
            pub fn arena(self) -> ArenaId {
                ArenaId::from_raw((self.bits() >> 32) as u32).expect("nonzero arena")
            }
            pub fn slot(self) -> u32 {
                self.bits() as u32
            }
            pub(crate) fn new(arena: ArenaId, slot: u32) -> Self {
                Self::from_bits((u64::from(arena.get()) << 32) | u64::from(slot))
                    .expect("nonzero slot")
            }
        }
    };
}
packed_id!(NodeId);
packed_id!(SymbolId);
packed_id!(AuxId);

pub(crate) fn next_slot(len: usize) -> Result<u32, Error> {
    u32::try_from(len)
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or(Error::InvalidSlot)
}

pub(crate) fn allocate_slot(len: usize) -> u32 {
    next_slot(len).unwrap_or_else(|_| panic!("ts_arena: arena slot space exhausted before reuse"))
}
