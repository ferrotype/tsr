//! The lazy-resolution cycle guard (`Checker.pushTypeResolution` and friends in
//! `tsc/internal/checker/checker.go`), kept as ADR 0008 requires.
//!
//! The guard keys on the entity *and* the property being resolved. On a cycle it
//! marks every resolution from the cycle start to the top as failed and pushes
//! nothing; the search stops early at any intermediate resolution whose property
//! has already been produced. A set of busy entities is not equivalent (plan
//! §4.2). Whether a property has been produced depends on the checker's links,
//! so the caller supplies that predicate; this module owns only the stack.

use crate::{SignatureId, TypeId};
use ts_arena::{NodeId, SymbolId};

/// `TypeSystemEntity` is `any` upstream; these are the four kinds it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeSystemEntity {
    Symbol(SymbolId),
    Type(TypeId),
    Signature(SignatureId),
    Node(NodeId),
}

/// `TypeSystemPropertyName`, in upstream declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum TypeSystemPropertyName {
    Type = 0,
    ResolvedBaseConstructorType,
    DeclaredType,
    ResolvedReturnType,
    ResolvedBaseConstraint,
    ResolvedTypeArguments,
    ResolvedBaseTypes,
    WriteType,
    InitializerIsUndefined,
    AliasTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeResolution {
    pub target: TypeSystemEntity,
    pub property_name: TypeSystemPropertyName,
    pub result: bool,
}

#[derive(Debug, Default)]
pub struct ResolutionStack {
    resolutions: Vec<TypeResolution>,
    resolution_start: usize,
}

impl ResolutionStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn depth(&self) -> usize {
        self.resolutions.len()
    }

    /// Sets the search floor and returns the previous one for restoration.
    pub fn set_resolution_start(&mut self, start: usize) -> usize {
        std::mem::replace(&mut self.resolution_start, start)
    }

    /// Returns `true` and records the resolution when no cycle exists. On a cycle
    /// every resolution from the cycle start upward is marked failed and nothing
    /// is pushed; the caller must not `pop` for a `false` result.
    // port: tsc/internal/checker/checker.go:Checker.pushTypeResolution
    pub fn push(
        &mut self,
        target: TypeSystemEntity,
        property_name: TypeSystemPropertyName,
        has_property: impl FnMut(&TypeResolution) -> bool,
    ) -> bool {
        let cycle_start =
            self.find_resolution_cycle_start_index(target, property_name, has_property);
        self.push_after_cycle_check(target, property_name, cycle_start)
    }

    /// Splits the read-only probe from mutation so the checker can consult all
    /// of its link stores without holding a mutable borrow of this stack.
    /// `cycle_start` must come from an immediately preceding probe of this stack.
    pub(crate) fn push_after_cycle_check(
        &mut self,
        target: TypeSystemEntity,
        property_name: TypeSystemPropertyName,
        cycle_start: Option<usize>,
    ) -> bool {
        if let Some(start) = cycle_start {
            for resolution in &mut self.resolutions[start..] {
                resolution.result = false;
            }
            return false;
        }
        self.resolutions.push(TypeResolution {
            target,
            property_name,
            result: true,
        });
        true
    }

    /// Searches from the top down to the resolution floor. Upstream returns -1 for
    /// no cycle; that is `None` here.
    // port: tsc/internal/checker/checker.go:Checker.findResolutionCycleStartIndex
    pub fn find_resolution_cycle_start_index(
        &self,
        target: TypeSystemEntity,
        property_name: TypeSystemPropertyName,
        mut has_property: impl FnMut(&TypeResolution) -> bool,
    ) -> Option<usize> {
        for index in (self.resolution_start..self.resolutions.len()).rev() {
            let resolution = &self.resolutions[index];
            if has_property(resolution) {
                return None;
            }
            if resolution.target == target && resolution.property_name == property_name {
                return Some(index);
            }
        }
        None
    }

    /// Pops the last resolution and reports whether it completed without a cycle.
    // port: tsc/internal/checker/checker.go:Checker.popTypeResolution
    pub fn pop(&mut self) -> bool {
        self.resolutions
            .pop()
            .expect("popTypeResolution pairs with a successful pushTypeResolution")
            .result
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl ResolutionStack {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.vec_capacity("resolution", &self.resolutions, self.resolutions.capacity());
    }
    pub(crate) fn census_entities(&self) -> impl Iterator<Item = TypeSystemEntity> + '_ {
        self.resolutions.iter().map(|entry| entry.target)
    }
}
