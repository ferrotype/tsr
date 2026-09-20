//! Checker-local identities. Upstream numbers types and signatures per checker
//! from 1 (`Checker.TypeCount`, `SignatureCount`); the numbers are not unique
//! across checkers, which is why they never leave an operation scope on their
//! own (ADR 0007, plan §4.1). Retention and cross-checker validation are the
//! owner's job; these values only index this checker's stores.

use crate::Error;
use std::num::NonZeroU32;

macro_rules! local_id {
    ($name:ident, $doc:literal $(, #[$index_attr:meta])*) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(transparent)]
        pub struct $name(NonZeroU32);
        impl $name {
            pub(crate) fn new(value: u32) -> Option<Self> {
                NonZeroU32::new(value).map(Self)
            }
            /// The next id for a store holding `len` records above `base`.
            /// Exhaustion is an error before any record is written.
            pub(crate) fn next(base: u32, len: usize) -> Result<Self, Error> {
                u32::try_from(len)
                    .ok()
                    .and_then(|len| len.checked_add(base))
                    .and_then(|value| value.checked_add(1))
                    .and_then(Self::new)
                    .ok_or(Error::IdExhausted)
            }
            /// The record index for a store whose ids start above `base`.
            $(#[$index_attr])*
            pub(crate) fn index(self, base: u32) -> Option<usize> {
                self.0
                    .get()
                    .checked_sub(base)
                    .and_then(|value| value.checked_sub(1))
                    .map(|value| value as usize)
            }
        }
    };
}

local_id!(
    TypeId,
    "A type in one checker's type store (`checker.TypeId`)."
);
local_id!(
    SignatureId,
    "A signature in one checker's signature store (`checker.SignatureId`)."
);
local_id!(AliasId, "A `TypeAlias` record in one checker's type store.");
local_id!(
    InferenceId,
    "An inference context retained by checker-local mappers."
);
local_id!(
    RelationFrameId,
    "A checker-local relation continuation used by inference comparers."
);
local_id!(
    ConditionalRootId,
    "A conditional type root owned by one checker."
);
local_id!(MapperId, "A type mapper owned by one checker.");
local_id!(
    IndexInfoId,
    "An `IndexInfo` record in one checker's signature store."
);
local_id!(
    TypePredicateId,
    "A `TypePredicate` record in one checker's signature store."
);

impl TypeId {
    /// The upstream numeric id, never a cross-checker ownership handle.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}
impl SignatureId {
    /// The upstream numeric id, never a cross-checker ownership handle.
    pub fn get(self) -> u32 {
        self.0.get()
    }
}
