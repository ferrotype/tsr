//! Native pseudo-type variants remain unnormalized and retain source nodes.
use std::sync::{Arc, OnceLock};
use tsr_arena::NodeId;

pub type PseudoType = Arc<PseudoTypeData>;

// Source: tsc/internal/pseudochecker/type.go:PseudoTypeKind
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i16)]
pub enum PseudoTypeKind {
    Direct,
    Inferred,
    NoResult,
    MaybeConstLocation,
    Union,
    Undefined,
    Null,
    Any,
    String,
    Number,
    BigInt,
    Boolean,
    False,
    True,
    SingleCallSignature,
    Tuple,
    ObjectLiteral,
    StringLiteral,
    NumericLiteral,
    BigIntLiteral,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PseudoSignature {
    pub signature: NodeId,
    pub parameters: Vec<PseudoParameter>,
    pub type_parameters: Vec<NodeId>,
    pub return_type: PseudoType,
}

// Source: tsc/internal/pseudochecker/type.go:PseudoType
#[derive(Clone, Debug, Eq, PartialEq)]
///
/// Go's `As*` accessors assert the concrete payload struct and panic on any
/// other kind; in Rust the variant is destructured by `match` or `let ... else`,
/// so each accessor is marked on the variant that plays its role.
pub enum PseudoTypeData {
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeDirect
    Direct {
        type_node: NodeId,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeInferred
    Inferred {
        expression: NodeId,
        error_nodes: Vec<NodeId>,
        is_signature_return: bool,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeNoResult
    NoResult {
        declaration: NodeId,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeMaybeConstLocation
    MaybeConstLocation {
        node: NodeId,
        const_type: PseudoType,
        regular_type: PseudoType,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeUnion
    Union {
        types: Vec<PseudoType>,
    },
    Undefined,
    Null,
    Any,
    String,
    Number,
    BigInt,
    Boolean,
    False,
    True,
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeSingleCallSignature
    SingleCallSignature(PseudoSignature),
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeTuple
    Tuple {
        elements: Vec<PseudoType>,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeObjectLiteral
    ObjectLiteral {
        elements: Vec<PseudoObjectElement>,
    },
    // Go's single `PseudoTypeLiteral` payload backs the three literal kinds;
    // its accessor is the `node` field of whichever of them is matched.
    // port: tsc/internal/pseudochecker/type.go:PseudoType.AsPseudoTypeLiteral
    StringLiteral {
        node: NodeId,
    },
    NumericLiteral {
        node: NodeId,
    },
    BigIntLiteral {
        node: NodeId,
    },
}
impl PseudoTypeData {
    pub fn kind(&self) -> PseudoTypeKind {
        use PseudoTypeKind as K;
        match self {
            Self::Direct { .. } => K::Direct,
            Self::Inferred { .. } => K::Inferred,
            Self::NoResult { .. } => K::NoResult,
            Self::MaybeConstLocation { .. } => K::MaybeConstLocation,
            Self::Union { .. } => K::Union,
            Self::Undefined => K::Undefined,
            Self::Null => K::Null,
            Self::Any => K::Any,
            Self::String => K::String,
            Self::Number => K::Number,
            Self::BigInt => K::BigInt,
            Self::Boolean => K::Boolean,
            Self::False => K::False,
            Self::True => K::True,
            Self::SingleCallSignature(_) => K::SingleCallSignature,
            Self::Tuple { .. } => K::Tuple,
            Self::ObjectLiteral { .. } => K::ObjectLiteral,
            Self::StringLiteral { .. } => K::StringLiteral,
            Self::NumericLiteral { .. } => K::NumericLiteral,
            Self::BigIntLiteral { .. } => K::BigIntLiteral,
        }
    }
}

// The native collector releases arbitrarily deep skeletons without consuming
// the caller's stack. Drain only uniquely owned children; shared roots retain
// their ordinary Arc identity and lifetime.
impl Drop for PseudoTypeData {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.drain_children(&mut pending);
        while let Some(child) = pending.pop() {
            if let Ok(mut child) = Arc::try_unwrap(child) {
                child.drain_children(&mut pending);
            }
        }
    }
}
impl PseudoTypeData {
    fn drain_children(&mut self, pending: &mut Vec<PseudoType>) {
        fn signature(signature: &mut PseudoSignature, pending: &mut Vec<PseudoType>) {
            pending.push(std::mem::replace(&mut signature.return_type, any()));
            pending.extend(
                std::mem::take(&mut signature.parameters)
                    .into_iter()
                    .map(|p| p.ty),
            );
        }
        match self {
            Self::MaybeConstLocation {
                const_type,
                regular_type,
                ..
            } => {
                pending.push(std::mem::replace(const_type, any()));
                pending.push(std::mem::replace(regular_type, any()));
            }
            Self::Union { types } | Self::Tuple { elements: types } => {
                pending.extend(std::mem::take(types));
            }
            Self::SingleCallSignature(data) => signature(data, pending),
            Self::ObjectLiteral { elements } => {
                for element in std::mem::take(elements) {
                    match element.data {
                        PseudoObjectElementData::Method(mut data) => signature(&mut data, pending),
                        PseudoObjectElementData::PropertyAssignment { ty, .. }
                        | PseudoObjectElementData::GetAccessor { ty, .. } => pending.push(ty),
                        PseudoObjectElementData::SetAccessor { parameter, .. } => {
                            pending.push(parameter.ty);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PseudoParameter {
    pub rest: bool,
    pub name: NodeId,
    pub optional: bool,
    pub ty: PseudoType,
}
impl PseudoParameter {
    // port: tsc/internal/pseudochecker/type.go:NewPseudoParameter
    pub fn new(rest: bool, name: NodeId, optional: bool, ty: PseudoType) -> Self {
        Self {
            rest,
            name,
            optional,
            ty,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i8)]
pub enum PseudoObjectElementKind {
    Method,
    PropertyAssignment,
    SetAccessor,
    GetAccessor,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PseudoObjectElement {
    pub name: NodeId,
    pub optional: bool,
    pub data: PseudoObjectElementData,
}
#[derive(Clone, Debug, Eq, PartialEq)]
///
/// As with [`PseudoTypeData`], Go's `As*` accessors are the variant
/// destructurings.
pub enum PseudoObjectElementData {
    // port: tsc/internal/pseudochecker/type.go:PseudoObjectElement.AsPseudoObjectMethod
    Method(PseudoSignature),
    // port: tsc/internal/pseudochecker/type.go:PseudoObjectElement.AsPseudoPropertyAssignment
    PropertyAssignment {
        readonly: bool,
        ty: PseudoType,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoObjectElement.AsPseudoSetAccessor
    SetAccessor {
        signature: NodeId,
        parameter: PseudoParameter,
    },
    // port: tsc/internal/pseudochecker/type.go:PseudoObjectElement.AsPseudoGetAccessor
    GetAccessor {
        signature: NodeId,
        ty: PseudoType,
    },
}
impl PseudoObjectElement {
    pub fn kind(&self) -> PseudoObjectElementKind {
        match &self.data {
            PseudoObjectElementData::Method(_) => PseudoObjectElementKind::Method,
            PseudoObjectElementData::PropertyAssignment { .. } => {
                PseudoObjectElementKind::PropertyAssignment
            }
            PseudoObjectElementData::SetAccessor { .. } => PseudoObjectElementKind::SetAccessor,
            PseudoObjectElementData::GetAccessor { .. } => PseudoObjectElementKind::GetAccessor,
        }
    }
    // port: tsc/internal/pseudochecker/type.go:PseudoObjectElement.Signature
    pub fn signature(&self) -> Option<NodeId> {
        match &self.data {
            PseudoObjectElementData::Method(data) => Some(data.signature),
            PseudoObjectElementData::SetAccessor { signature, .. }
            | PseudoObjectElementData::GetAccessor { signature, .. } => Some(*signature),
            PseudoObjectElementData::PropertyAssignment { .. } => None,
        }
    }
    // port: tsc/internal/pseudochecker/type.go:NewPseudoObjectMethod
    pub fn method(
        signature: NodeId,
        name: NodeId,
        optional: bool,
        type_parameters: Vec<NodeId>,
        parameters: Vec<PseudoParameter>,
        return_type: PseudoType,
    ) -> Self {
        new_pseudo_object_element(
            name,
            optional,
            PseudoObjectElementData::Method(PseudoSignature {
                signature,
                parameters,
                type_parameters,
                return_type,
            }),
        )
    }
    // port: tsc/internal/pseudochecker/type.go:NewPseudoPropertyAssignment
    pub fn property(readonly: bool, name: NodeId, optional: bool, ty: PseudoType) -> Self {
        new_pseudo_object_element(
            name,
            optional,
            PseudoObjectElementData::PropertyAssignment { readonly, ty },
        )
    }
    // port: tsc/internal/pseudochecker/type.go:NewPseudoSetAccessor
    pub fn set_accessor(
        signature: NodeId,
        name: NodeId,
        optional: bool,
        parameter: PseudoParameter,
    ) -> Self {
        new_pseudo_object_element(
            name,
            optional,
            PseudoObjectElementData::SetAccessor {
                signature,
                parameter,
            },
        )
    }
    // port: tsc/internal/pseudochecker/type.go:NewPseudoGetAccessor
    pub fn get_accessor(signature: NodeId, name: NodeId, optional: bool, ty: PseudoType) -> Self {
        new_pseudo_object_element(
            name,
            optional,
            PseudoObjectElementData::GetAccessor { signature, ty },
        )
    }
}

// Go stores the kind beside the payload and reaches the embedded header through
// `AsPseudoObjectElement`; here the kind is derived from the payload variant
// (`PseudoObjectElement::kind`), so the header is just the two shared fields.
// port: tsc/internal/pseudochecker/type.go:newPseudoObjectElement
fn new_pseudo_object_element(
    name: NodeId,
    optional: bool,
    data: PseudoObjectElementData,
) -> PseudoObjectElement {
    PseudoObjectElement {
        name,
        optional,
        data,
    }
}

// Go stores the kind beside the payload and reaches the embedded header through
// `PseudoTypeDefault.AsPseudoType`; here the kind is derived from the payload
// variant (`PseudoTypeData::kind`), so construction is the shared allocation.
// port: tsc/internal/pseudochecker/type.go:newPseudoType
fn new_pseudo_type(data: PseudoTypeData) -> PseudoType {
    Arc::new(data)
}

fn primitives() -> &'static [PseudoType; 9] {
    static VALUES: OnceLock<[PseudoType; 9]> = OnceLock::new();
    VALUES.get_or_init(|| {
        [
            PseudoTypeData::Undefined,
            PseudoTypeData::Null,
            PseudoTypeData::Any,
            PseudoTypeData::String,
            PseudoTypeData::Number,
            PseudoTypeData::BigInt,
            PseudoTypeData::Boolean,
            PseudoTypeData::False,
            PseudoTypeData::True,
        ]
        .map(new_pseudo_type)
    })
}
pub fn undefined() -> PseudoType {
    primitives()[0].clone()
}
pub fn null() -> PseudoType {
    primitives()[1].clone()
}
pub fn any() -> PseudoType {
    primitives()[2].clone()
}
pub fn string() -> PseudoType {
    primitives()[3].clone()
}
pub fn number() -> PseudoType {
    primitives()[4].clone()
}
pub fn bigint() -> PseudoType {
    primitives()[5].clone()
}
pub fn boolean() -> PseudoType {
    primitives()[6].clone()
}
pub fn false_type() -> PseudoType {
    primitives()[7].clone()
}
pub fn true_type() -> PseudoType {
    primitives()[8].clone()
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeDirect
pub fn direct(type_node: NodeId) -> PseudoType {
    new_pseudo_type(PseudoTypeData::Direct { type_node })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeInferred
pub fn inferred(expression: NodeId, is_signature_return: bool) -> PseudoType {
    new_pseudo_type(PseudoTypeData::Inferred {
        expression,
        error_nodes: Vec::new(),
        is_signature_return,
    })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeInferredWithErrors
pub fn inferred_with_errors(
    expression: NodeId,
    is_signature_return: bool,
    error_nodes: Vec<NodeId>,
) -> PseudoType {
    new_pseudo_type(PseudoTypeData::Inferred {
        expression,
        error_nodes,
        is_signature_return,
    })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeNoResult
pub fn no_result(declaration: NodeId) -> PseudoType {
    new_pseudo_type(PseudoTypeData::NoResult { declaration })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeMaybeConstLocation
pub fn maybe_const_location(
    node: NodeId,
    const_type: PseudoType,
    regular_type: PseudoType,
) -> PseudoType {
    new_pseudo_type(PseudoTypeData::MaybeConstLocation {
        node,
        const_type,
        regular_type,
    })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeUnion
pub fn union(types: Vec<PseudoType>) -> PseudoType {
    new_pseudo_type(PseudoTypeData::Union { types })
}
/// Go passes the signature node, parameters, type parameters and return type
/// as four arguments; the established Rust API takes them as one
/// [`PseudoSignature`], which is also the payload of an object method.
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeSingleCallSignature
pub fn single_call_signature(signature: PseudoSignature) -> PseudoType {
    new_pseudo_type(PseudoTypeData::SingleCallSignature(signature))
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeTuple
pub fn tuple(elements: Vec<PseudoType>) -> PseudoType {
    new_pseudo_type(PseudoTypeData::Tuple { elements })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeObjectLiteral
pub fn object_literal(elements: Vec<PseudoObjectElement>) -> PseudoType {
    new_pseudo_type(PseudoTypeData::ObjectLiteral { elements })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeStringLiteral
pub fn string_literal(node: NodeId) -> PseudoType {
    new_pseudo_type(PseudoTypeData::StringLiteral { node })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeNumericLiteral
pub fn numeric_literal(node: NodeId) -> PseudoType {
    new_pseudo_type(PseudoTypeData::NumericLiteral { node })
}
// port: tsc/internal/pseudochecker/type.go:NewPseudoTypeBigIntLiteral
pub fn bigint_literal(node: NodeId) -> PseudoType {
    new_pseudo_type(PseudoTypeData::BigIntLiteral { node })
}
