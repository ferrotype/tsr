//! Phase 2 C5.5 services replay: read-only views of a checker's types and
//! signatures, the fields the pinned recorder overlay writes for a value at
//! first sight (`tools/phase2/services/overlay/checker_phase2_services.go`).
//! A view only reads stored fields. It never runs a lazy query, so describing
//! a result cannot change what a later replayed call observes.
use crate::types::{LiteralValue, TypeKind};
use crate::{Error, Operation, SignatureRef, SymbolRef, TypeRef};
use tsr_arena::NodeId;
use tsr_jsnum::{Number, PseudoBigInt};
use tsr_jsstring::JsString;

/// A type as the recorder describes it: its flags, the object flags, the Go
/// data struct its kind stores, and the fields that build it.
#[derive(Debug)]
pub struct TypeView {
    pub flags: u32,
    pub object_flags: u32,
    pub kind: &'static str,
    pub symbol: Option<SymbolRef>,
    pub alias: Option<(SymbolRef, Vec<TypeRef>)>,
    pub data: TypeViewData,
}

#[derive(Debug)]
pub enum LiteralView {
    String(JsString),
    Number(Number),
    Boolean(bool),
    BigInt(PseudoBigInt),
    Computed,
}

#[derive(Debug)]
pub enum TypeViewData {
    None,
    Intrinsic(JsString),
    Literal(LiteralView),
    Unique(JsString),
    Types(Vec<TypeRef>),
    TypeParameter {
        is_this_type: bool,
        target: Option<TypeRef>,
    },
    Index {
        target: TypeRef,
        flags: u32,
    },
    IndexedAccess {
        object: TypeRef,
        index: TypeRef,
    },
    Conditional {
        root: NodeId,
        check: TypeRef,
        extends: TypeRef,
    },
    Substitution {
        base: TypeRef,
        constraint: TypeRef,
    },
    StringMapping {
        target: TypeRef,
    },
    Template {
        texts: Vec<JsString>,
        types: Vec<TypeRef>,
    },
    Mapped {
        declaration: Option<NodeId>,
    },
    Tuple {
        element_flags: Vec<u32>,
        readonly: bool,
    },
    Reference {
        target: Option<TypeRef>,
        arguments: Option<Vec<TypeRef>>,
        node: Option<NodeId>,
    },
}

/// A signature's stored fields.
#[derive(Debug)]
pub struct SignatureView {
    pub flags: u32,
    pub min_argument_count: i32,
    pub declaration: Option<NodeId>,
    pub type_parameters: Option<Vec<TypeRef>>,
    pub parameters: Option<Vec<SymbolRef>>,
    pub this_parameter: Option<SymbolRef>,
    pub target: Option<SignatureRef>,
    pub composite: bool,
}

/// The Go data struct each kind stores (`Type.data`).
fn go_data_name(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Intrinsic => "IntrinsicType",
        TypeKind::Literal => "LiteralType",
        TypeKind::UniqueEsSymbol => "UniqueESSymbolType",
        TypeKind::Anonymous => "ObjectType",
        TypeKind::Reference => "TypeReference",
        TypeKind::Interface => "InterfaceType",
        TypeKind::Tuple => "TupleType",
        TypeKind::Mapped => "MappedType",
        TypeKind::ReverseMapped => "ReverseMappedType",
        TypeKind::EvolvingArray => "EvolvingArrayType",
        TypeKind::InstantiationExpression => "InstantiationExpressionType",
        TypeKind::Union => "UnionType",
        TypeKind::Intersection => "IntersectionType",
        TypeKind::TypeParameter => "TypeParameter",
        TypeKind::Index => "IndexType",
        TypeKind::IndexedAccess => "IndexedAccessType",
        TypeKind::TemplateLiteral => "TemplateLiteralType",
        TypeKind::StringMapping => "StringMappingType",
        TypeKind::Substitution => "SubstitutionType",
        TypeKind::Conditional => "ConditionalType",
    }
}

impl Operation<'_> {
    pub fn replay_type_view(&self, t: TypeRef) -> Result<TypeView, Error> {
        let id = self.check_type(t)?;
        let state = self.state();
        let types = &state.types;
        let record = *types.get(id)?;
        let symbol = record.symbol.map(|s| self.symbol_ref(s)).transpose()?;
        let alias = match types.alias_of(id)? {
            Some(alias) => Some((
                self.symbol_ref(alias.symbol)?,
                self.type_refs(&alias.type_arguments),
            )),
            None => None,
        };
        let kind = record.kind();
        let data = match kind {
            TypeKind::Intrinsic => TypeViewData::Intrinsic(types.intrinsic(id)?.name.clone()),
            TypeKind::Literal => TypeViewData::Literal(match &types.literal(id)?.value {
                LiteralValue::String(value) => LiteralView::String(value.clone()),
                LiteralValue::Number(value) => LiteralView::Number(*value),
                LiteralValue::Boolean(value) => LiteralView::Boolean(*value),
                LiteralValue::BigInt(value) => LiteralView::BigInt(value.clone()),
                LiteralValue::ComputedEnum => LiteralView::Computed,
            }),
            TypeKind::UniqueEsSymbol => TypeViewData::Unique(types.unique_symbol(id)?.name.clone()),
            TypeKind::Union => TypeViewData::Types(self.type_refs(&types.union(id)?.types)),
            TypeKind::Intersection => {
                TypeViewData::Types(self.type_refs(&types.intersection(id)?.types))
            }
            TypeKind::TypeParameter => {
                let data = types.type_parameter(id)?;
                TypeViewData::TypeParameter {
                    is_this_type: data.is_this_type,
                    target: data.target.map(|t| self.type_ref(t)),
                }
            }
            TypeKind::Index => {
                let data = types.index_type(id)?;
                TypeViewData::Index {
                    target: self.type_ref(data.target),
                    flags: data.index_flags,
                }
            }
            TypeKind::IndexedAccess => {
                let data = types.indexed_access(id)?;
                TypeViewData::IndexedAccess {
                    object: self.type_ref(data.object_type),
                    index: self.type_ref(data.index_type),
                }
            }
            TypeKind::Conditional => {
                let data = types.conditional(id)?;
                TypeViewData::Conditional {
                    root: state.conditional_root(data.root)?.node,
                    check: self.type_ref(data.check_type),
                    extends: self.type_ref(data.extends_type),
                }
            }
            TypeKind::Substitution => {
                let data = types.substitution(id)?;
                TypeViewData::Substitution {
                    base: self.type_ref(data.base),
                    constraint: self.type_ref(data.constraint),
                }
            }
            TypeKind::StringMapping => TypeViewData::StringMapping {
                target: self.type_ref(types.string_mapping(id)?.target),
            },
            TypeKind::TemplateLiteral => {
                let data = types.template_literal(id)?;
                TypeViewData::Template {
                    texts: data.texts.to_vec(),
                    types: self.type_refs(&data.types),
                }
            }
            TypeKind::Mapped => TypeViewData::Mapped {
                declaration: types.mapped(id)?.declaration,
            },
            TypeKind::Tuple => {
                let data = types.tuple(id)?;
                TypeViewData::Tuple {
                    element_flags: data.element_infos.iter().map(|info| info.flags).collect(),
                    readonly: data.readonly,
                }
            }
            TypeKind::Reference => {
                let data = types.type_reference(id)?;
                TypeViewData::Reference {
                    target: data.object.target.map(|t| self.type_ref(t)),
                    arguments: data
                        .resolved_type_arguments
                        .as_ref()
                        .map(|list| self.type_refs(list)),
                    node: data.node,
                }
            }
            TypeKind::Anonymous
            | TypeKind::Interface
            | TypeKind::ReverseMapped
            | TypeKind::EvolvingArray
            | TypeKind::InstantiationExpression => TypeViewData::None,
        };
        Ok(TypeView {
            flags: record.flags,
            object_flags: record.object_flags,
            kind: go_data_name(kind),
            symbol,
            alias,
            data,
        })
    }

    pub fn replay_signature_view(&self, s: SignatureRef) -> Result<SignatureView, Error> {
        let id = self.check_signature(s)?;
        let signature = self.state().signatures.get(id)?;
        Ok(SignatureView {
            flags: signature.flags,
            min_argument_count: signature.min_argument_count,
            declaration: signature.declaration,
            type_parameters: signature
                .type_parameters
                .as_ref()
                .map(|list| self.type_refs(list)),
            parameters: signature
                .parameters
                .as_ref()
                .map(|list| self.symbol_refs(list))
                .transpose()?,
            this_parameter: signature
                .this_parameter
                .map(|s| self.symbol_ref(s))
                .transpose()?,
            target: signature.target.map(|s| self.signature_ref(s)),
            composite: signature.composite.is_some(),
        })
    }

    /// The checker's own symbols the recorder names instead of describing.
    pub fn replay_builtin_symbol(&self, name: &str) -> Result<Option<SymbolRef>, Error> {
        let builtins = &self.state().builtins;
        let id = match name {
            "unknown" => builtins.unknown_symbol,
            "undefined" => builtins.undefined_symbol,
            "arguments" => builtins.arguments_symbol,
            "require" => builtins.require_symbol,
            "globalThis" => builtins.global_this_symbol,
            _ => return Ok(None),
        };
        self.symbol_ref(id).map(Some)
    }

    /// The builtin name of `symbol`, if it is one of the checker's own.
    pub fn replay_builtin_name(&self, symbol: SymbolRef) -> Result<Option<&'static str>, Error> {
        let id = self.check_symbol_ref(symbol)?;
        let builtins = &self.state().builtins;
        Ok(if id == builtins.unknown_symbol {
            Some("unknown")
        } else if id == builtins.undefined_symbol {
            Some("undefined")
        } else if id == builtins.arguments_symbol {
            Some("arguments")
        } else if id == builtins.require_symbol {
            Some("require")
        } else if id == builtins.global_this_symbol {
            Some("globalThis")
        } else {
            None
        })
    }
}
