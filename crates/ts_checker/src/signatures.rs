//! Signatures, index infos and type predicates (`Signature`, `IndexInfo`,
//! `TypePredicate` in `tsc/internal/checker/types.go`; constructors in
//! `checker.go`). Upstream allocates signatures and index infos from per-checker
//! arenas and numbers signatures from 1; the store does the same.

use crate::{
    Error, IndexInfoId, SignatureFlags, SignatureId, SymbolList, TypeId, TypeList, TypePredicateId,
    TypePredicateKind,
};
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::JsString;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypePredicate {
    pub kind: TypePredicateKind,
    pub parameter_index: i32,
    pub parameter_name: JsString,
    pub t: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompositeSignature {
    pub is_union: bool,
    pub signatures: Arc<[SignatureId]>,
}

/// One signature, including lazy instantiated and composite results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub flags: SignatureFlags,
    pub min_argument_count: i32,
    pub resolved_min_argument_count: i32,
    pub declaration: Option<NodeId>,
    pub type_parameters: Option<TypeList>,
    pub parameters: Option<SymbolList>,
    pub this_parameter: Option<SymbolId>,
    pub resolved_return_type: Option<TypeId>,
    pub resolved_type_predicate: Option<TypePredicateId>,
    pub target: Option<SignatureId>,
    pub(crate) composite: Option<CompositeSignature>,
    pub mapper: Option<crate::MapperId>,
    pub erased: Option<SignatureId>,
    pub base: Option<SignatureId>,
    pub isolated_signature_type: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    pub key_type: TypeId,
    pub value_type: TypeId,
    pub is_readonly: bool,
    /// IndexSignatureDeclaration.
    pub declaration: Option<NodeId>,
    /// Synthetic property symbol for this index signature.
    pub index_symbol: Option<SymbolId>,
    /// ElementWithComputedPropertyName nodes.
    pub components: Option<Arc<[NodeId]>>,
}

#[derive(Debug, Default)]
pub struct SignatureStore {
    signatures: Vec<Signature>,
    index_infos: Vec<IndexInfo>,
    predicates: Vec<TypePredicate>,
    pub(crate) instantiations: crate::types::Map<(SignatureId, crate::CacheKey), SignatureId>,
}

impl SignatureStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Signatures created so far (`Checker.SignatureCount`).
    pub fn len(&self) -> usize {
        self.signatures.len()
    }

    #[cfg(any(test, feature = "storage-pilot"))]
    pub fn index_info_count(&self) -> usize {
        self.index_infos.len()
    }

    #[cfg(any(test, feature = "storage-pilot"))]
    pub fn predicate_count(&self) -> usize {
        self.predicates.len()
    }

    // port: tsc/internal/checker/checker.go:Checker.newSignature
    #[allow(
        clippy::too_many_arguments,
        reason = "upstream's parameter list, kept for traceability"
    )]
    pub(crate) fn new_signature(
        &mut self,
        flags: SignatureFlags,
        declaration: Option<NodeId>,
        type_parameters: Option<TypeList>,
        this_parameter: Option<SymbolId>,
        parameters: Option<SymbolList>,
        resolved_return_type: Option<TypeId>,
        resolved_type_predicate: Option<TypePredicateId>,
        min_argument_count: i32,
    ) -> Result<SignatureId, Error> {
        let id = SignatureId::next(0, self.signatures.len())?;
        self.signatures.push(Signature {
            flags,
            min_argument_count,
            resolved_min_argument_count: -1,
            declaration,
            type_parameters,
            parameters,
            this_parameter,
            resolved_return_type,
            resolved_type_predicate,
            target: None,
            composite: None,
            mapper: None,
            erased: None,
            base: None,
            isolated_signature_type: None,
        });
        Ok(id)
    }

    // port: tsc/internal/checker/checker.go:Checker.newIndexInfo
    pub(crate) fn new_index_info(
        &mut self,
        key_type: TypeId,
        value_type: TypeId,
        is_readonly: bool,
        declaration: Option<NodeId>,
        components: Option<Arc<[NodeId]>>,
    ) -> Result<IndexInfoId, Error> {
        let id = IndexInfoId::next(0, self.index_infos.len())?;
        self.index_infos.push(IndexInfo {
            key_type,
            value_type,
            is_readonly,
            declaration,
            index_symbol: None,
            components,
        });
        Ok(id)
    }

    pub(crate) fn new_type_predicate(
        &mut self,
        predicate: TypePredicate,
    ) -> Result<TypePredicateId, Error> {
        let id = TypePredicateId::next(0, self.predicates.len())?;
        self.predicates.push(predicate);
        Ok(id)
    }

    pub fn get(&self, id: SignatureId) -> Result<&Signature, Error> {
        id.index(0)
            .and_then(|index| self.signatures.get(index))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }

    pub(crate) fn get_mut(&mut self, id: SignatureId) -> Result<&mut Signature, Error> {
        id.index(0)
            .and_then(|index| self.signatures.get_mut(index))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }

    pub fn index_info(&self, id: IndexInfoId) -> Result<&IndexInfo, Error> {
        id.index(0)
            .and_then(|index| self.index_infos.get(index))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }

    pub(crate) fn index_info_mut(&mut self, id: IndexInfoId) -> Result<&mut IndexInfo, Error> {
        id.index(0)
            .and_then(|index| self.index_infos.get_mut(index))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }

    pub fn predicate(&self, id: TypePredicateId) -> Result<&TypePredicate, Error> {
        id.index(0)
            .and_then(|index| self.predicates.get(index))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }

    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn capacities(&self) -> (usize, usize, usize) {
        (
            self.signatures.capacity(),
            self.index_infos.capacity(),
            self.predicates.capacity(),
        )
    }
}
