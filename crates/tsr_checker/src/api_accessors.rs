//! Production readers of type internals for the API server's response
//! constructors: the alias of a type, its literal value, the parts of
//! indexed-access, conditional, substitution, template-literal, index and
//! string-mapping types, the type parameters of an interface and the shape
//! of a tuple. Each validates the reference's owner through the operation
//! before reading, as the other type readers do.
//! port: tsc/internal/api/proto.go:newTypeResponse
use crate::types::{LiteralValue, TupleElementInfo};
use crate::{ElementFlags, Error};
use crate::{Operation, TypeRef};
use tsr_arena::SymbolId;
use tsr_ast::{AstView, JsString, NodeId};

/// The type parameters of a class or interface type, in the pin's layout:
/// outer parameters first, local ones after them, the `this` type last.
#[derive(Clone, Debug)]
pub struct InterfaceTypeParameters {
    pub all: Vec<TypeRef>,
    pub outer_count: usize,
    pub has_this_type: bool,
}

impl InterfaceTypeParameters {
    /// `InterfaceType.TypeParameters`: everything but the `this` type.
    pub fn type_parameters(&self) -> &[TypeRef] {
        let end = self.all.len() - usize::from(self.has_this_type && !self.all.is_empty());
        &self.all[..end]
    }
    /// `InterfaceType.OuterTypeParameters`
    pub fn outer(&self) -> &[TypeRef] {
        &self.all[..self.outer_count.min(self.all.len())]
    }
    /// `InterfaceType.LocalTypeParameters`
    pub fn local(&self) -> &[TypeRef] {
        let parameters = self.type_parameters();
        &parameters[self.outer_count.min(parameters.len())..]
    }
}

/// A tuple target's shape.
#[derive(Clone, Debug)]
pub struct TupleShape {
    pub element_infos: Vec<TupleElementInfo>,
    pub fixed_length: u32,
    pub readonly: bool,
}

impl TupleShape {
    pub fn element_flags(&self) -> Vec<ElementFlags> {
        self.element_infos.iter().map(|info| info.flags).collect()
    }
}

impl Operation<'_> {
    /// The view that owns `node`: a retained program file or this checker's
    /// own arena. Reads through it cannot outlive the operation.
    pub fn ast_view(&self, node: NodeId) -> Result<AstView<'_>, Error> {
        self.state().ast(node)
    }

    /// The alias a type was declared through, with its type arguments.
    /// port: tsc/internal/checker/types.go:Type.Alias
    pub fn type_alias(&self, t: TypeRef) -> Result<Option<(SymbolId, Vec<TypeRef>)>, Error> {
        let id = self.check_type(t)?;
        Ok(self.state().types.alias_of(id)?.map(|alias| {
            (
                alias.symbol,
                alias
                    .type_arguments
                    .iter()
                    .map(|argument| self.type_ref(*argument))
                    .collect(),
            )
        }))
    }

    /// The value of a literal type.
    /// port: tsc/internal/checker/types.go:LiteralType.Value
    pub fn literal_value(&self, t: TypeRef) -> Result<LiteralValue, Error> {
        let id = self.check_type(t)?;
        Ok(self.state().types.literal(id)?.value.clone())
    }

    /// The target of a type reference, index type or string mapping, when
    /// the kind has one.
    /// port: tsc/internal/checker/types.go:Type.Target
    pub fn type_target(&self, t: TypeRef) -> Result<Option<TypeRef>, Error> {
        let id = self.check_type(t)?;
        Ok(self
            .state()
            .types
            .target(id)
            .ok()
            .map(|target| self.type_ref(target)))
    }

    /// The type parameters of a class or interface type.
    /// port: tsc/internal/checker/types.go:InterfaceType.TypeParameters
    pub fn interface_type_parameters(
        &self,
        t: TypeRef,
    ) -> Result<Option<InterfaceTypeParameters>, Error> {
        let id = self.check_type(t)?;
        let Ok(interface) = self.state().types.interface(id) else {
            return Ok(None);
        };
        let all = interface
            .all_type_parameters
            .as_ref()
            .map(|list| list.iter().map(|id| self.type_ref(*id)).collect())
            .unwrap_or_default();
        Ok(Some(InterfaceTypeParameters {
            all,
            outer_count: interface.outer_type_parameter_count as usize,
            has_this_type: interface.this_type.is_some(),
        }))
    }

    /// The shape of a tuple type's target: `None` for other types.
    /// port: tsc/internal/checker/types.go:TupleType.ElementFlags
    pub fn tuple_shape(&self, t: TypeRef) -> Result<Option<TupleShape>, Error> {
        let id = self.check_type(t)?;
        let state = self.state();
        if !state.is_tuple_type(id)? {
            return Ok(None);
        }
        let tuple = state.types.tuple(state.types.target(id)?)?;
        Ok(Some(TupleShape {
            element_infos: tuple.element_infos.to_vec(),
            fixed_length: tuple.fixed_length,
            readonly: tuple.readonly,
        }))
    }

    /// port: tsc/internal/checker/types.go:IndexedAccessType.ObjectType
    pub fn indexed_access_parts(&self, t: TypeRef) -> Result<Option<(TypeRef, TypeRef)>, Error> {
        let id = self.check_type(t)?;
        Ok(self.state().types.indexed_access(id).ok().map(|data| {
            (
                self.type_ref(data.object_type),
                self.type_ref(data.index_type),
            )
        }))
    }

    /// port: tsc/internal/checker/types.go:ConditionalType.CheckType
    pub fn conditional_parts(&self, t: TypeRef) -> Result<Option<(TypeRef, TypeRef)>, Error> {
        let id = self.check_type(t)?;
        Ok(self.state().types.conditional(id).ok().map(|data| {
            (
                self.type_ref(data.check_type),
                self.type_ref(data.extends_type),
            )
        }))
    }

    /// port: tsc/internal/checker/types.go:SubstitutionType.BaseType
    pub fn substitution_parts(&self, t: TypeRef) -> Result<Option<(TypeRef, TypeRef)>, Error> {
        let id = self.check_type(t)?;
        Ok(self
            .state()
            .types
            .substitution(id)
            .ok()
            .map(|data| (self.type_ref(data.base), self.type_ref(data.constraint))))
    }

    /// port: tsc/internal/checker/types.go:TemplateLiteralType.Texts
    pub fn template_literal_texts(&self, t: TypeRef) -> Result<Option<Vec<JsString>>, Error> {
        let id = self.check_type(t)?;
        Ok(self
            .state()
            .types
            .template_literal(id)
            .ok()
            .map(|data| data.texts.to_vec()))
    }

    /// The parts of a union, intersection or template literal type: the
    /// pin's `Type.Types`, which the API's `getTypes` reads.
    /// port: tsc/internal/checker/types.go:Type.Types
    pub fn type_parts(&self, t: TypeRef) -> Result<Vec<TypeRef>, Error> {
        let id = self.check_type(t)?;
        let state = self.state();
        if let Ok(data) = state.types.template_literal(id) {
            return Ok(data.types.iter().map(|id| self.type_ref(*id)).collect());
        }
        Ok(state
            .types
            .types_of(id)?
            .iter()
            .map(|id| self.type_ref(*id))
            .collect())
    }

    /// port: tsc/internal/checker/types.go:TypeParameter.IsThisType
    pub fn type_parameter_is_this(&self, t: TypeRef) -> Result<bool, Error> {
        let id = self.check_type(t)?;
        Ok(self
            .state()
            .types
            .type_parameter(id)
            .ok()
            .is_some_and(|data| data.is_this_type))
    }
}
