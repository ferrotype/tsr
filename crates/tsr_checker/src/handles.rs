//! Owner-bound handles: the public face of a checker (design note
//! `symbols.md` §2.4; plan §4.1).
//!
//! Inside an [`Operation`] the caller works with copyable refs (`TypeRef`,
//! `SignatureRef`, `NodeRef`) that name the exact checker they belong to; every
//! use validates that identity against the operation's lease, so an in-range
//! handle from another checker is rejected before any read. A ref never keeps
//! storage alive. To keep a result past the operation, `retain_*` pays the
//! owner reference increment; a `Retained*` value keeps the `CheckerOwner`, its
//! type universe and its synthetic AST alive, and `import_*` brings it back into
//! an operation of that same owner after revalidating identity and generation.
//! Internal caches never hold retained handles, so a checker cannot retain itself.

use crate::{
    element_flags, CheckerOwner, ElementFlags, Error, ObjectFlags, Operation, SignatureId,
    TupleElementInfo, TypeFlags, TypeId, TypeKind, TypeList, UnionReduction,
};
use std::collections::HashMap;
use std::sync::Arc;
use tsr_arena::{ArenaId, NodeId, SymbolId};
use tsr_ast::{CheckFlags, JsString, NodeKind, SymbolFlags};
use tsr_jsnum::{Number, PseudoBigInt};

#[path = "handles_display.rs"]
mod display;
pub use display::TypeNodeBuilder;

/// A type of one checker, usable inside an operation on that checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeRef {
    owner: ArenaId,
    id: TypeId,
}

impl TypeRef {
    /// The upstream numeric id (`Type.Id()`), for display and diagnostics only.
    pub fn id(self) -> u32 {
        self.id.get()
    }
}

/// A signature of one checker, usable inside an operation on that checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignatureRef {
    owner: ArenaId,
    id: SignatureId,
}

/// A source or transient symbol interpreted by one exact checker. The raw AST
/// identity alone cannot distinguish two checkers' merged interpretations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymbolRef {
    owner: ArenaId,
    id: SymbolId,
}
impl SymbolRef {
    pub fn id(self) -> SymbolId {
        self.id
    }
}

impl SignatureRef {
    pub fn id(self) -> u32 {
        self.id.get()
    }
}

/// A node the checker created in its own synthetic AST arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeRef {
    owner: ArenaId,
    id: NodeId,
}

impl NodeRef {
    pub fn id(self) -> NodeId {
        self.id
    }
}

macro_rules! retained {
    ($name:ident, $id:ty, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone)]
        pub struct $name {
            owner: Arc<CheckerOwner>,
            id: $id,
        }

        impl $name {
            /// The owner this result keeps alive.
            pub fn owner(&self) -> &Arc<CheckerOwner> {
                &self.owner
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                output
                    .debug_struct(stringify!($name))
                    .field("checker", &self.owner.identity().id())
                    .field("id", &self.id)
                    .finish()
            }
        }
    };
}

retained!(
    RetainedType,
    TypeId,
    "A type kept past its operation; it keeps the whole checker alive."
);
retained!(
    RetainedSymbol,
    SymbolId,
    "A checker-created symbol kept past its operation."
);
retained!(
    RetainedSignature,
    SignatureId,
    "A signature kept past its operation."
);
retained!(
    RetainedNode,
    NodeId,
    "A checker-created node kept past its operation."
);
retained!(
    RetainedTypeList,
    TypeList,
    "An immutable type list coupled to its owning checker."
);

impl RetainedType {
    pub fn id(&self) -> u32 {
        self.id.get()
    }
}

impl RetainedSignature {
    pub fn id(&self) -> u32 {
        self.id.get()
    }
}

impl RetainedTypeList {
    pub fn len(&self) -> usize {
        self.id.len()
    }
    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }
}

/// One member of an anonymous object type built through the operation API.
#[derive(Clone, Copy, Debug)]
pub struct MemberSpec<'a> {
    pub name: &'a [u8],
    pub r#type: TypeRef,
    pub optional: bool,
    pub readonly: bool,
}

impl Operation<'_> {
    /// Compares two results owned by this checker in the selected production
    /// relation. Types retained from another checker are rejected first.
    pub fn is_type_related_to(
        &mut self,
        source: TypeRef,
        target: TypeRef,
        mode: crate::RelationKind,
    ) -> Result<bool, Error> {
        let source = self.check_type(source)?;
        let target = self.check_type(target)?;
        self.state_mut().is_type_related_to(source, target, mode)
    }

    /// Rust's Ordering preserves the sign of Go's integer comparator. Both
    /// non-null handles are validated before comparing, including equal refs.
    pub fn compare_type_order(
        &self,
        source: Option<TypeRef>,
        target: Option<TypeRef>,
    ) -> Result<std::cmp::Ordering, Error> {
        let source = source.map(|t| self.check_type(t)).transpose()?;
        let target = target.map(|t| self.check_type(t)).transpose()?;
        match (source, target) {
            (None, None) => Ok(std::cmp::Ordering::Equal),
            (None, Some(_)) => Ok(std::cmp::Ordering::Less),
            (Some(_), None) => Ok(std::cmp::Ordering::Greater),
            (Some(source), Some(target)) => self.state().compare_types(source, target),
        }
    }

    /// Constructs only the supplemental comparator-domain records described
    /// by P0; every result comes from the production comparator.
    #[cfg(feature = "relation-probe")]
    pub fn observe_residual_comparators(
        &mut self,
        first: NodeId,
        second: NodeId,
    ) -> Result<serde_json::Value, Error> {
        self.state().ast(first)?.node(first)?;
        self.state().ast(second)?.node(second)?;
        self.state_mut().residual_comparators(first, second)
    }

    /// Diagnostic-only access to the frozen relation observation contract.
    #[cfg(feature = "relation-probe")]
    pub fn observe_type_relation(
        &mut self,
        source: TypeRef,
        target: TypeRef,
        mode: crate::RelationKind,
        error_node: Option<NodeId>,
    ) -> Result<(bool, Vec<crate::Ternary>, Option<tsr_ast::Diagnostic>), Error> {
        let source = self.check_type(source)?;
        let target = self.check_type(target)?;
        let state = self.state_mut();
        if state.relations.observer.is_some() {
            return Err(Error::MissingLink("relation observer already installed"));
        }
        if let Some(node) = error_node {
            state.node(node)?;
        }
        state.relations.observer = Some(Vec::new());
        let result = state.check_type_related_ex(source, target, mode, error_node, None);
        let calls = state
            .relations
            .observer
            .take()
            .expect("observer installed above");
        result.map(|(result, diagnostic)| (result, calls, diagnostic))
    }

    /// Install contract-only observation without changing any semantic limit.
    /// A configured panic happens inside the real recursive relation, after
    /// stack growth. A panic retires the operation's generation as usual.
    #[cfg(feature = "recursion-probe")]
    pub fn begin_recursion_probe(&mut self, panic_at_depth: Option<usize>) -> Result<(), Error> {
        let state = self.state_mut();
        if state.relations.recursion_probe.is_some() {
            return Err(Error::MissingLink("recursion probe already installed"));
        }
        state.relations.recursion_probe = Some(crate::relater::RecursionProbe {
            panic_at_depth,
            ..Default::default()
        });
        Ok(())
    }

    /// Remove the contract observer and return only its executed-path counts.
    #[cfg(feature = "recursion-probe")]
    pub fn take_recursion_probe(&mut self) -> Result<serde_json::Value, Error> {
        let probe = self
            .state_mut()
            .relations
            .recursion_probe
            .take()
            .ok_or(Error::MissingLink("recursion probe not installed"))?;
        Ok(serde_json::json!({
            "calls": probe.calls,
            "maximum_depth": probe.maximum_depth,
            "maximum_remaining_stack": probe.maximum_remaining_stack,
            "depth_limit_hits": probe.depth_limit_hits,
        }))
    }

    /// Owned counter/cache snapshot. It cannot warm a type or relation cache.
    #[cfg(feature = "relation-probe")]
    pub fn relation_state(&self) -> serde_json::Value {
        let state = self.state();
        let mut caches = serde_json::Map::new();
        for (index, name) in [
            "identity",
            "assignable",
            "subtype",
            "strict_subtype",
            "comparable",
        ]
        .iter()
        .enumerate()
        {
            let cache = &state.relations.caches[index];
            let mut flags = cache.values().copied().collect::<Vec<_>>();
            flags.sort_unstable();
            caches.insert(
                (*name).to_string(),
                serde_json::json!({"entries":cache.len(),"result_flags":flags}),
            );
        }
        serde_json::json!({"types_created":state.types.len(),"signatures_created":state.signatures.len(),"instantiations":state.instantiation.total_count,"caches":caches})
    }

    /// Reads the alias cache without resolving its declaration or creating links.
    #[cfg(feature = "relation-probe")]
    pub fn alias_instantiation_cache_entries(&self, symbol: SymbolRef) -> Result<usize, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        Ok(self
            .state()
            .query
            .type_aliases
            .try_get(symbol)
            .map_or(0, |links| links.instantiations.len()))
    }

    /// Imports the exact identity only if the checker retains its symbol store.
    /// This preserves raw symbol observation, like Go's symbol-taking APIs;
    /// it does not substitute a merged clone. Source name/location queries
    /// return this checker's merged symbol when one exists.
    pub fn symbol_ref(&self, id: SymbolId) -> Result<SymbolRef, Error> {
        self.lease().validate_identity(self.checker())?;
        self.state().symbol(id)?;
        Ok(SymbolRef {
            owner: self.checker(),
            id,
        })
    }

    fn check_symbol_ref(&self, symbol: SymbolRef) -> Result<SymbolId, Error> {
        self.lease().validate_identity(symbol.owner)?;
        self.state().symbol(symbol.id)?;
        Ok(symbol.id)
    }

    pub fn get_symbol_at_location(&mut self, node: NodeId) -> Result<Option<SymbolRef>, Error> {
        let symbol = self.state_mut().get_symbol_at_location(node)?;
        symbol.map(|symbol| self.symbol_ref(symbol)).transpose()
    }

    pub fn get_type_at_location(&mut self, node: NodeId) -> Result<TypeRef, Error> {
        let ty = self.state_mut().get_type_at_location(node)?;
        Ok(self.type_ref(ty))
    }

    /// Whether this source node participates in an expression query. The
    /// contextual cases share the classifier used by checker name resolution.
    pub fn is_expression_node(&self, node: NodeId) -> Result<bool, Error> {
        crate::query::is_expression_node(self.state().ast(node)?, node)
    }

    /// Native `IsPartOfTypeNode`, including qualified names and heritage nodes.
    pub fn is_part_of_type_node(&self, node: NodeId) -> Result<bool, Error> {
        self.state().is_part_of_type_node(node)
    }

    /// The intrinsic spelling is distinct from printed type syntax. In
    /// particular the native baseline walker bypasses the builder for `any`.
    pub fn intrinsic_type_name(&self, ty: TypeRef) -> Result<JsString, Error> {
        let ty = self.check_type(ty)?;
        Ok(self.state().types.intrinsic(ty)?.name.clone())
    }

    pub fn get_declared_type_of_symbol(&mut self, symbol: SymbolRef) -> Result<TypeRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let ty = self.state_mut().get_declared_type_of_symbol(symbol)?;
        Ok(self.type_ref(ty))
    }

    pub fn get_type_of_symbol(&mut self, symbol: SymbolRef) -> Result<TypeRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let ty = self.state_mut().get_type_of_symbol(symbol)?;
        Ok(self.type_ref(ty))
    }

    pub fn symbol(&self, symbol: SymbolRef) -> Result<tsr_ast::SymbolRef<'_>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state().symbol(symbol)
    }

    pub fn symbol_declarations(
        &self,
        symbol: SymbolRef,
    ) -> Result<tsr_ast::DeclarationRead<'_>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state().symbol_declarations(symbol)
    }

    pub fn symbol_table(
        &self,
        table: tsr_ast::SymbolTableId,
    ) -> Result<tsr_ast::SymbolTableRead<'_>, Error> {
        self.state().table(table)
    }

    pub fn properties_of_type(&mut self, ty: TypeRef) -> Result<Vec<SymbolRef>, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut()
            .get_properties_of_type(ty)?
            .into_iter()
            .map(|id| self.symbol_ref(id))
            .collect()
    }

    /// Context-free `TypeToStringEx`: flags are explicit and there is no
    /// enclosing declaration. Go's `TypeToString` defaults are
    /// `ALLOW_UNIQUE_ES_SYMBOL_TYPE | USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE`.
    /// Use `type_to_string_at` for qualification and annotation reuse in an
    /// enclosing declaration.
    pub fn type_to_string(
        &mut self,
        ty: TypeRef,
        flags: crate::TypeFormatFlags,
    ) -> Result<JsString, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut().type_to_string(ty, flags)
    }

    pub fn global_diagnostics(&mut self) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        Ok(self
            .state_mut()
            .diagnostics_for_file(None)?
            .into_iter()
            .cloned()
            .collect())
    }

    pub fn semantic_diagnostics(
        &mut self,
        source: NodeId,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        self.state_mut().check_source_file(source)?;
        Ok(self
            .state_mut()
            .diagnostics_for_file(Some(source))?
            .into_iter()
            .cloned()
            .collect())
    }

    /// Suggestions produced by queries and checking, including the
    /// unused-identifier pass Go's GetSuggestionDiagnostics requests.
    pub fn recorded_suggestions(
        &mut self,
        source: NodeId,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        self.state_mut().check_source_file_ex(source, true)?;
        Ok(self
            .state_mut()
            .suggestions_for_file(Some(source))?
            .into_iter()
            .cloned()
            .collect())
    }

    fn checker(&self) -> ArenaId {
        self.owner().identity().id()
    }

    fn type_ref(&self, id: TypeId) -> TypeRef {
        TypeRef {
            owner: self.checker(),
            id,
        }
    }

    fn signature_ref(&self, id: SignatureId) -> SignatureRef {
        SignatureRef {
            owner: self.checker(),
            id,
        }
    }

    fn node_ref(&self, id: NodeId) -> NodeRef {
        NodeRef {
            owner: self.checker(),
            id,
        }
    }

    /// Validates a type ref against this operation: exact checker, live
    /// generation, published slot.
    fn check_type(&self, t: TypeRef) -> Result<TypeId, Error> {
        self.lease().validate_identity(t.owner)?;
        self.state().types.get(t.id)?;
        Ok(t.id)
    }

    fn check_types(&self, types: &[TypeRef]) -> Result<Vec<TypeId>, Error> {
        types.iter().map(|t| self.check_type(*t)).collect()
    }

    fn check_signature(&self, s: SignatureRef) -> Result<SignatureId, Error> {
        self.lease().validate_identity(s.owner)?;
        self.state().signatures.get(s.id)?;
        Ok(s.id)
    }

    fn check_symbol(&self, s: SymbolId) -> Result<SymbolId, Error> {
        self.lease().validate_identity(s.arena())?;
        self.state().symbols.get(s)?;
        Ok(s)
    }

    fn check_node(&self, n: NodeRef) -> Result<NodeId, Error> {
        self.lease().validate_identity(n.owner)?;
        if n.id.arena() != self.state().factory.id().arena() {
            return Err(Error::Arena(tsr_arena::Error::WrongOwner));
        }
        self.state().factory.view().node(n.id)?;
        Ok(n.id)
    }

    /// A type `NewChecker` creates, by its upstream field name (`"stringType"`).
    pub fn builtin_type(&self, name: &str) -> Option<TypeRef> {
        self.state()
            .builtins
            .type_by_name(name)
            .map(|id| self.type_ref(id))
    }

    /// The structural storage census of this checker with `roots` as the
    /// retained results (`data/s08/type-footprint.json`). Read-only: no
    /// resolution, links or caches are created.
    #[cfg(feature = "storage-pilot")]
    pub fn census(&self, roots: &[TypeRef]) -> Result<serde_json::Value, Error> {
        let roots = self.check_types(roots)?;
        self.state().census(&roots)
    }

    /// `Checker.TypeCount`.
    pub fn type_count(&self) -> usize {
        self.state().types.len()
    }

    /// `Checker.SymbolCount`.
    pub fn symbol_count(&self) -> u32 {
        self.state().symbol_count
    }

    /// `Checker.SignatureCount`.
    pub fn signature_count(&self) -> usize {
        self.state().signatures.len()
    }

    pub fn type_flags(&self, t: TypeRef) -> Result<TypeFlags, Error> {
        let id = self.check_type(t)?;
        self.state().types.flags(id)
    }

    pub fn type_object_flags(&self, t: TypeRef) -> Result<ObjectFlags, Error> {
        let id = self.check_type(t)?;
        self.state().types.object_flags(id)
    }

    pub fn type_kind(&self, t: TypeRef) -> Result<TypeKind, Error> {
        let id = self.check_type(t)?;
        self.state().kind(id)
    }

    /// Existing computed-name identity, without creating links or resolving a type.
    /// The operation validates the symbol owner before observing its name type;
    /// this read is suitable for snapshots that must not warm checker queries.
    pub fn symbol_name_type(&self, symbol: SymbolRef) -> Result<Option<TypeRef>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        Ok(self
            .state()
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.name_type)
            .map(|ty| self.type_ref(ty)))
    }

    /// The symbol of a type, if it has one.
    pub fn type_symbol(&self, t: TypeRef) -> Result<Option<SymbolId>, Error> {
        let id = self.check_type(t)?;
        Ok(self.state().types.get(id)?.symbol)
    }

    /// The checker's interned union types (`Checker.unionTypes` values), in
    /// cache order; the runner's union-ordering check does not depend on it.
    // port: tsc/internal/checker/checker.go:Checker.UnionTypes
    pub fn union_types(&self) -> Vec<TypeRef> {
        self.state()
            .types
            .caches
            .union_types
            .values()
            .map(|id| self.type_ref(*id))
            .collect()
    }

    /// Constituents of a union or intersection, in stored order.
    pub fn constituents(&self, t: TypeRef) -> Result<Vec<TypeRef>, Error> {
        let id = self.check_type(t)?;
        Ok(self
            .state()
            .types
            .types_of(id)?
            .iter()
            .map(|id| self.type_ref(*id))
            .collect())
    }

    pub fn string_literal_type(&mut self, value: &[u8]) -> Result<TypeRef, Error> {
        let id = self
            .state_mut()
            .get_string_literal_type(JsString::from_bytes(value))?;
        Ok(self.type_ref(id))
    }

    pub fn number_literal_type(&mut self, value: f64) -> Result<TypeRef, Error> {
        let id = self
            .state_mut()
            .get_number_literal_type(Number::new(value))?;
        Ok(self.type_ref(id))
    }

    pub fn big_int_literal_type(
        &mut self,
        negative: bool,
        digits: &[u8],
    ) -> Result<TypeRef, Error> {
        let id = self
            .state_mut()
            .get_big_int_literal_type(PseudoBigInt::new(digits, negative))?;
        Ok(self.type_ref(id))
    }

    pub fn fresh_type_of_literal_type(&mut self, t: TypeRef) -> Result<TypeRef, Error> {
        let id = self.check_type(t)?;
        let fresh = self.state_mut().get_fresh_type_of_literal_type(id)?;
        Ok(self.type_ref(fresh))
    }

    pub fn regular_type_of_literal_type(&mut self, t: TypeRef) -> Result<TypeRef, Error> {
        let id = self.check_type(t)?;
        let regular = self.state_mut().get_regular_type_of_literal_type(id)?;
        Ok(self.type_ref(regular))
    }

    /// `getUnionType`: literal reduction.
    pub fn union_type(&mut self, types: &[TypeRef]) -> Result<TypeRef, Error> {
        self.union_type_with(types, UnionReduction::Literal)
    }

    pub fn union_type_with(
        &mut self,
        types: &[TypeRef],
        reduction: UnionReduction,
    ) -> Result<TypeRef, Error> {
        let ids = self.check_types(types)?;
        let id = self
            .state_mut()
            .get_union_type_ex(&ids, reduction, None, None)?;
        Ok(self.type_ref(id))
    }

    pub fn type_parameter(&mut self, symbol: Option<SymbolId>) -> Result<TypeRef, Error> {
        let symbol = symbol.map(|s| self.check_symbol(s)).transpose()?;
        let id = self.state_mut().new_type_parameter(symbol)?;
        Ok(self.type_ref(id))
    }

    /// A transient symbol of this checker (`newSymbolEx`).
    pub fn new_symbol(
        &mut self,
        flags: SymbolFlags,
        name: &[u8],
        check_flags: CheckFlags,
    ) -> Result<SymbolId, Error> {
        self.state_mut()
            .new_symbol_ex(flags, JsString::from_bytes(name), check_flags)
    }

    /// A tuple target for the given element flags (`getTupleTargetType`).
    pub fn tuple_target_type(
        &mut self,
        elements: &[ElementFlags],
        readonly: bool,
    ) -> Result<TypeRef, Error> {
        let infos: Vec<TupleElementInfo> = elements
            .iter()
            .map(|flags| TupleElementInfo {
                flags: if *flags == element_flags::NONE {
                    element_flags::REQUIRED
                } else {
                    *flags
                },
                labeled_declaration: None,
            })
            .collect();
        let id = self.state_mut().get_tuple_target_type(&infos, readonly)?;
        Ok(self.type_ref(id))
    }

    /// `createTupleType`: a tuple of required elements.
    pub fn tuple_type(&mut self, element_types: &[TypeRef]) -> Result<TypeRef, Error> {
        let ids = self.check_types(element_types)?;
        let id = self.state_mut().create_tuple_type(&ids)?;
        Ok(self.type_ref(id))
    }

    /// `createTypeReference`.
    pub fn type_reference(
        &mut self,
        target: TypeRef,
        type_arguments: &[TypeRef],
    ) -> Result<TypeRef, Error> {
        let target = self.check_type(target)?;
        let ids = self.check_types(type_arguments)?;
        let id = self.state_mut().create_type_reference(target, &ids)?;
        Ok(self.type_ref(id))
    }

    /// An anonymous object type with property members whose types are known
    /// (`newAnonymousType` over `newSymbolEx` members with resolved value links).
    pub fn anonymous_type(
        &mut self,
        symbol: Option<SymbolId>,
        members: &[MemberSpec<'_>],
    ) -> Result<TypeRef, Error> {
        let symbol = symbol.map(|s| self.check_symbol(s)).transpose()?;
        let mut table = HashMap::with_capacity(members.len());
        for member in members {
            let t = self.check_type(member.r#type)?;
            let flags = tsr_ast::symbol_flags::PROPERTY
                | if member.optional {
                    tsr_ast::symbol_flags::OPTIONAL
                } else {
                    0
                };
            let check_flags = if member.readonly {
                tsr_ast::check_flags::READONLY
            } else {
                0
            };
            let property = self.state_mut().new_symbol_ex(
                flags,
                JsString::from_bytes(member.name),
                check_flags,
            )?;
            self.state_mut()
                .value_symbol_links
                .get_or_default(property)
                .resolved_type = Some(t);
            table.insert(JsString::from_bytes(member.name), Some(property));
        }
        let members = if table.is_empty() {
            None
        } else {
            Some(self.state_mut().alloc_symbol_table(table))
        };
        let id = self
            .state_mut()
            .new_anonymous_type(symbol, members, &[], &[], &[])?;
        Ok(self.type_ref(id))
    }

    /// The type's named properties in upstream order, with their resolved types.
    pub fn properties(&self, t: TypeRef) -> Result<Vec<(SymbolId, Option<TypeRef>)>, Error> {
        let id = self.check_type(t)?;
        let state = self.state();
        let properties = state.types.structured(id)?.properties.clone();
        Ok(properties
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|symbol| {
                let resolved = state
                    .value_symbol_links
                    .try_get(*symbol)
                    .and_then(|links| links.resolved_type)
                    .map(|id| self.type_ref(id));
                (*symbol, resolved)
            })
            .collect())
    }

    /// `getTemplateLiteralType`.
    pub fn template_literal_type(
        &mut self,
        texts: &[&[u8]],
        types: &[TypeRef],
    ) -> Result<TypeRef, Error> {
        let ids = self.check_types(types)?;
        let texts: Vec<JsString> = texts
            .iter()
            .map(|text| JsString::from_bytes(*text))
            .collect();
        let id = self.state_mut().get_template_literal_type(&texts, &ids)?;
        Ok(self.type_ref(id))
    }

    /// `newCallSignature`: a signature with a checker-created declaration.
    pub fn call_signature(
        &mut self,
        parameters: &[SymbolId],
        return_type: TypeRef,
    ) -> Result<SignatureRef, Error> {
        let return_type = self.check_type(return_type)?;
        let parameters: Vec<SymbolId> = parameters
            .iter()
            .map(|s| self.check_symbol(*s))
            .collect::<Result<_, _>>()?;
        let parameters = if parameters.is_empty() {
            None
        } else {
            Some(Arc::from(parameters))
        };
        let id = self
            .state_mut()
            .new_call_signature(None, None, parameters, return_type)?;
        Ok(self.signature_ref(id))
    }

    pub fn signature_return_type(&self, s: SignatureRef) -> Result<Option<TypeRef>, Error> {
        let id = self.check_signature(s)?;
        Ok(self
            .state()
            .signatures
            .get(id)?
            .resolved_return_type
            .map(|id| self.type_ref(id)))
    }

    /// The synthetic declaration of a checker-created signature.
    pub fn signature_declaration(&self, s: SignatureRef) -> Result<Option<NodeRef>, Error> {
        let id = self.check_signature(s)?;
        Ok(self
            .state()
            .signatures
            .get(id)?
            .declaration
            .map(|id| self.node_ref(id)))
    }

    /// A checker-created expression node embedding a type.
    pub fn synthetic_expression(&mut self, t: TypeRef) -> Result<NodeRef, Error> {
        let id = self.check_type(t)?;
        let node = self.state_mut().new_synthetic_expression(id, false, None)?;
        Ok(self.node_ref(node))
    }

    /// `createSyntheticExpression` through its port in `call_spread.rs`: a
    /// checker-created expression whose parent and range come from a node of
    /// the checker's program. The parent stays an id into a retained file; a
    /// node this checker's file set does not hold is rejected before anything
    /// is created.
    pub fn synthetic_expression_at(
        &mut self,
        parent: NodeId,
        t: TypeRef,
        is_spread: bool,
    ) -> Result<NodeRef, Error> {
        let id = self.check_type(t)?;
        let node = self
            .state_mut()
            .synthetic_call_argument(parent, id, is_spread, None)?;
        Ok(self.node_ref(node))
    }

    /// The parent of a checker-created node and its kind, resolved through the
    /// checker's own arena or through its retained file set.
    pub fn node_parent(&self, node: NodeRef) -> Result<Option<(NodeId, NodeKind)>, Error> {
        let id = self.check_node(node)?;
        let Some(parent) = self.state().factory.view().node(id)?.parent() else {
            return Ok(None);
        };
        let kind = self.state().node(parent)?.kind();
        Ok(Some((parent, kind)))
    }

    /// The type a synthetic expression embeds.
    pub fn synthetic_expression_type(&self, node: NodeRef) -> Result<TypeRef, Error> {
        let id = self.check_node(node)?;
        self.state()
            .synthetic_expression_types
            .get(&id)
            .map(|t| self.type_ref(*t))
            .ok_or(Error::MissingLink("SyntheticExpression.Type"))
    }

    pub fn node_kind(&self, node: NodeRef) -> Result<NodeKind, Error> {
        let id = self.check_node(node)?;
        Ok(self.state().factory.view().node(id)?.kind())
    }

    pub fn retain_type(&self, t: TypeRef) -> Result<RetainedType, Error> {
        let id = self.check_type(t)?;
        Ok(RetainedType {
            owner: self.owner().clone(),
            id,
        })
    }

    /// Brings a retained type back into an operation of its own checker;
    /// another checker's operation rejects it before any read.
    pub fn import_type(&self, retained: &RetainedType) -> Result<TypeRef, Error> {
        self.check_import(&retained.owner)?;
        self.state().types.get(retained.id)?;
        Ok(self.type_ref(retained.id))
    }

    pub fn retain_symbol(&self, symbol: SymbolId) -> Result<RetainedSymbol, Error> {
        let id = self.check_symbol(symbol)?;
        Ok(RetainedSymbol {
            owner: self.owner().clone(),
            id,
        })
    }

    pub fn import_symbol(&self, retained: &RetainedSymbol) -> Result<SymbolId, Error> {
        self.check_import(&retained.owner)?;
        self.check_symbol(retained.id)
    }

    /// Retains a symbol this checker returned from a query. It may live in a
    /// bound file rather than in the checker's own arena; the owner retains
    /// that file set, so the result keeps the file alive too.
    pub fn retain_symbol_ref(&self, symbol: SymbolRef) -> Result<RetainedSymbol, Error> {
        let id = self.check_symbol_ref(symbol)?;
        Ok(RetainedSymbol {
            owner: self.owner().clone(),
            id,
        })
    }

    /// The exact-checker reference of a retained symbol, for the queries that
    /// take one. Another checker's operation rejects it before any read, even
    /// when both checkers share the bound file the symbol lives in.
    pub fn import_symbol_ref(&self, retained: &RetainedSymbol) -> Result<SymbolRef, Error> {
        self.check_import(&retained.owner)?;
        self.symbol_ref(retained.id)
    }

    pub fn retain_signature(&self, s: SignatureRef) -> Result<RetainedSignature, Error> {
        let id = self.check_signature(s)?;
        Ok(RetainedSignature {
            owner: self.owner().clone(),
            id,
        })
    }

    pub fn import_signature(&self, retained: &RetainedSignature) -> Result<SignatureRef, Error> {
        self.check_import(&retained.owner)?;
        self.state().signatures.get(retained.id)?;
        Ok(self.signature_ref(retained.id))
    }

    pub fn retain_node(&self, node: NodeRef) -> Result<RetainedNode, Error> {
        let id = self.check_node(node)?;
        Ok(RetainedNode {
            owner: self.owner().clone(),
            id,
        })
    }

    pub fn import_node(&self, retained: &RetainedNode) -> Result<NodeRef, Error> {
        self.check_import(&retained.owner)?;
        self.check_node(self.node_ref(retained.id))
            .map(|id| self.node_ref(id))
    }

    pub fn retain_type_list(&self, types: &[TypeRef]) -> Result<RetainedTypeList, Error> {
        let ids = self.check_types(types)?;
        Ok(RetainedTypeList {
            owner: self.owner().clone(),
            id: Arc::from(ids),
        })
    }

    pub fn import_type_list(&self, retained: &RetainedTypeList) -> Result<Vec<TypeRef>, Error> {
        self.check_import(&retained.owner)?;
        retained
            .id
            .iter()
            .map(|id| {
                self.state().types.get(*id)?;
                Ok(self.type_ref(*id))
            })
            .collect()
    }

    /// Exact owner, then live generation. A pool-generation match alone never
    /// permits mixing checkers.
    fn check_import(&self, owner: &Arc<CheckerOwner>) -> Result<(), Error> {
        if !Arc::ptr_eq(owner, self.owner()) {
            return Err(Error::Arena(tsr_arena::Error::WrongOwner));
        }
        self.lease().validate_identity(owner.identity().id())?;
        Ok(())
    }
}

/// One signature's shape for the S08 relater prototype's translation
/// (`docs/S08-P7.md`): the prototype constructs its own graph from these
/// read-only shapes and never delegates a relation to this checker.
#[cfg(feature = "relation-probe")]
#[derive(Clone, Debug)]
pub struct SignatureShape {
    pub parameters: Vec<TypeRef>,
    pub min_argument_count: usize,
    pub has_rest_parameter: bool,
    pub type_parameters: usize,
    pub this_type: Option<TypeRef>,
    pub return_type: TypeRef,
    pub is_abstract: bool,
    /// Raw `SyntaxKind` of the declaration, when there is one.
    pub declaration_kind: Option<i16>,
}

/// A literal type's value, for the same translation.
#[cfg(feature = "relation-probe")]
#[derive(Clone, Debug, PartialEq)]
pub enum LiteralShape {
    String(Vec<u8>),
    Number(f64),
    Boolean(bool),
    BigInt { negative: bool, digits: Vec<u8> },
    Unknown,
}

#[cfg(feature = "relation-probe")]
impl Operation<'_> {
    /// Resolved index signatures as (key type, value type, readonly).
    pub fn index_infos(&mut self, t: TypeRef) -> Result<Vec<(TypeRef, TypeRef, bool)>, Error> {
        let id = self.check_type(t)?;
        let infos = self.state_mut().index_infos_of_type(id)?;
        let state = self.state();
        let mut result = Vec::with_capacity(infos.len());
        for info in infos {
            let info = state.signatures.index_info(info)?;
            result.push((
                self.type_ref(info.key_type),
                self.type_ref(info.value_type),
                info.is_readonly,
            ));
        }
        Ok(result)
    }

    /// Resolved call (or construct) signatures in upstream order.
    pub fn signatures_of_type(
        &mut self,
        t: TypeRef,
        construct: bool,
    ) -> Result<Vec<SignatureRef>, Error> {
        let id = self.check_type(t)?;
        Ok(self
            .state_mut()
            .signatures_of_type(id, construct)?
            .into_iter()
            .map(|id| self.signature_ref(id))
            .collect())
    }

    pub fn signature_shape(&mut self, s: SignatureRef) -> Result<SignatureShape, Error> {
        let id = self.check_signature(s)?;
        let (parameters, this_parameter, flags, type_parameters, declaration) = {
            let sig = self.state().signatures.get(id)?;
            (
                sig.parameters.as_deref().unwrap_or(&[]).to_vec(),
                sig.this_parameter,
                sig.flags,
                sig.type_parameters.as_ref().map_or(0, |list| list.len()),
                sig.declaration,
            )
        };
        let mut parameter_types = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            let ty = self.state_mut().type_of_parameter(parameter)?;
            parameter_types.push(self.type_ref(ty));
        }
        let this_type = match this_parameter {
            Some(symbol) => {
                let ty = self.state_mut().get_type_of_symbol(symbol)?;
                Some(self.type_ref(ty))
            }
            None => None,
        };
        let return_type = self.state_mut().return_type_of_signature(id)?;
        let min_argument_count = self.state_mut().min_argument_count(id)?;
        let declaration_kind = match declaration {
            Some(node) => Some(self.state().ast(node)?.node(node)?.kind().raw()),
            None => None,
        };
        Ok(SignatureShape {
            parameters: parameter_types,
            min_argument_count,
            has_rest_parameter: flags & crate::signature_flags::HAS_REST_PARAMETER != 0,
            type_parameters,
            this_type,
            return_type: self.type_ref(return_type),
            is_abstract: flags & crate::signature_flags::ABSTRACT != 0,
            declaration_kind,
        })
    }

    /// A literal type's value and whether this is the fresh form.
    pub fn literal_shape(&self, t: TypeRef) -> Result<(LiteralShape, bool), Error> {
        let id = self.check_type(t)?;
        let state = self.state();
        let data = state.types.literal(id)?;
        let value = match &data.value {
            crate::LiteralValue::String(text) => LiteralShape::String(text.as_bytes().to_vec()),
            crate::LiteralValue::Number(value) => LiteralShape::Number(value.value()),
            crate::LiteralValue::Boolean(value) => LiteralShape::Boolean(*value),
            crate::LiteralValue::BigInt(value) => LiteralShape::BigInt {
                negative: value.negative,
                digits: value.base10_value.clone(),
            },
            crate::LiteralValue::ComputedEnum => LiteralShape::Unknown,
        };
        Ok((value, state.is_fresh_literal_type(id)?))
    }

    pub fn is_readonly_symbol(&mut self, symbol: SymbolRef) -> Result<bool, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().is_readonly_symbol(symbol)
    }

    /// The alias symbol a type displays through, if any.
    pub fn alias_symbol(&self, t: TypeRef) -> Result<Option<SymbolId>, Error> {
        let id = self.check_type(t)?;
        Ok(self.state().types.alias_of(id)?.map(|alias| alias.symbol))
    }
}
