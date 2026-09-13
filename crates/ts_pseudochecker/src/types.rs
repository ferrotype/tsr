//! Native pseudo-type variants remain unnormalized and retain source nodes.
use std::sync::{Arc, OnceLock};
use ts_arena::NodeId;

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
pub enum PseudoTypeData {
    Direct {
        type_node: NodeId,
    },
    Inferred {
        expression: NodeId,
        error_nodes: Vec<NodeId>,
        is_signature_return: bool,
    },
    NoResult {
        declaration: NodeId,
    },
    MaybeConstLocation {
        node: NodeId,
        const_type: PseudoType,
        regular_type: PseudoType,
    },
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
    SingleCallSignature(PseudoSignature),
    Tuple {
        elements: Vec<PseudoType>,
    },
    ObjectLiteral {
        elements: Vec<PseudoObjectElement>,
    },
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
pub enum PseudoObjectElementData {
    Method(PseudoSignature),
    PropertyAssignment {
        readonly: bool,
        ty: PseudoType,
    },
    SetAccessor {
        signature: NodeId,
        parameter: PseudoParameter,
    },
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
    pub fn signature(&self) -> Option<NodeId> {
        match &self.data {
            PseudoObjectElementData::Method(data) => Some(data.signature),
            PseudoObjectElementData::SetAccessor { signature, .. }
            | PseudoObjectElementData::GetAccessor { signature, .. } => Some(*signature),
            PseudoObjectElementData::PropertyAssignment { .. } => None,
        }
    }
    pub fn property(readonly: bool, name: NodeId, optional: bool, ty: PseudoType) -> Self {
        Self {
            name,
            optional,
            data: PseudoObjectElementData::PropertyAssignment { readonly, ty },
        }
    }
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
        .map(Arc::new)
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
pub fn direct(type_node: NodeId) -> PseudoType {
    Arc::new(PseudoTypeData::Direct { type_node })
}
pub fn inferred(expression: NodeId, is_signature_return: bool) -> PseudoType {
    inferred_with_errors(expression, is_signature_return, Vec::new())
}
pub fn inferred_with_errors(
    expression: NodeId,
    is_signature_return: bool,
    error_nodes: Vec<NodeId>,
) -> PseudoType {
    Arc::new(PseudoTypeData::Inferred {
        expression,
        is_signature_return,
        error_nodes,
    })
}
pub fn no_result(declaration: NodeId) -> PseudoType {
    Arc::new(PseudoTypeData::NoResult { declaration })
}
pub fn maybe_const_location(
    node: NodeId,
    const_type: PseudoType,
    regular_type: PseudoType,
) -> PseudoType {
    Arc::new(PseudoTypeData::MaybeConstLocation {
        node,
        const_type,
        regular_type,
    })
}
pub fn union(types: Vec<PseudoType>) -> PseudoType {
    Arc::new(PseudoTypeData::Union { types })
}
pub fn single_call_signature(signature: PseudoSignature) -> PseudoType {
    Arc::new(PseudoTypeData::SingleCallSignature(signature))
}
pub fn tuple(elements: Vec<PseudoType>) -> PseudoType {
    Arc::new(PseudoTypeData::Tuple { elements })
}
pub fn object_literal(elements: Vec<PseudoObjectElement>) -> PseudoType {
    Arc::new(PseudoTypeData::ObjectLiteral { elements })
}
pub fn string_literal(node: NodeId) -> PseudoType {
    Arc::new(PseudoTypeData::StringLiteral { node })
}
pub fn numeric_literal(node: NodeId) -> PseudoType {
    Arc::new(PseudoTypeData::NumericLiteral { node })
}
pub fn bigint_literal(node: NodeId) -> PseudoType {
    Arc::new(PseudoTypeData::BigIntLiteral { node })
}
