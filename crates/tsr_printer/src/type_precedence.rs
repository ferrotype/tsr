//! Type-node precedence (`TypePrecedence` and `GetTypeNodePrecedence` in
//! `tsc/internal/ast/precedence.go`). The printer parenthesizes a type node whose
//! precedence is lower than the position it is printed in. Upstream keeps this
//! in `ast`; it lives here until a second consumer needs it.

use crate::Error;
use ts_ast::{AstView, NodeId, SyntaxKind as K};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum TypePrecedence {
    /// `T extends U ? X : Y` (lowest).
    Conditional = 0,
    /// `...T` and `T=` JSDoc types.
    JsDoc,
    /// Function and constructor types.
    Function,
    Union,
    Intersection,
    /// `keyof T`, `readonly T`, `unique symbol`, `infer T`, `typeof x`.
    TypeOperator,
    /// `T[]`, `T[K]`, `T?`.
    Postfix,
    /// Everything atomic (highest).
    NonArray,
}

impl TypePrecedence {
    pub const LOWEST: Self = Self::Conditional;
    pub const HIGHEST: Self = Self::NonArray;
}

/// `None` for a node that is not a type node; upstream panics there.
// port: tsc/internal/ast/precedence.go:GetTypeNodePrecedence
pub fn get_type_node_precedence(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<TypePrecedence>, Error> {
    let read = view.node(node)?;
    let Some(kind) = read.kind().known() else {
        return Ok(None);
    };
    Ok(Some(match kind {
        K::ConditionalType => TypePrecedence::Conditional,
        K::JSDocOptionalType | K::JSDocVariadicType => TypePrecedence::JsDoc,
        K::FunctionType | K::ConstructorType => TypePrecedence::Function,
        K::UnionType => TypePrecedence::Union,
        K::IntersectionType => TypePrecedence::Intersection,
        K::InferType => {
            // `infer T extends U` eagerly consumes the type after `extends`, so it
            // takes function precedence.
            let type_parameter = read
                .data_source()
                .as_infer_type_node()
                .and_then(|infer| infer.type_parameter());
            let constrained = match type_parameter {
                Some(parameter) => view
                    .node(parameter)?
                    .data_source()
                    .as_type_parameter_declaration()
                    .is_some_and(|declaration| declaration.constraint().is_some()),
                None => false,
            };
            if constrained {
                TypePrecedence::Function
            } else {
                TypePrecedence::TypeOperator
            }
        }
        K::IndexedAccessType | K::ArrayType | K::OptionalType => TypePrecedence::Postfix,
        // A type query is a non-array type but is parenthesized in postfix
        // positions like a type operator, so `(typeof C)[]` never prints as
        // `typeof C[]`.
        K::TypeOperator | K::TypeQuery => TypePrecedence::TypeOperator,
        K::AnyKeyword
        | K::UnknownKeyword
        | K::StringKeyword
        | K::NumberKeyword
        | K::BigIntKeyword
        | K::SymbolKeyword
        | K::BooleanKeyword
        | K::UndefinedKeyword
        | K::NeverKeyword
        | K::ObjectKeyword
        | K::IntrinsicKeyword
        | K::VoidKeyword
        | K::JSDocAllType
        | K::JSDocNullableType
        | K::JSDocNonNullableType
        | K::LiteralType
        | K::TypePredicate
        | K::TypeReference
        | K::TypeLiteral
        | K::TupleType
        | K::RestType
        | K::ParenthesizedType
        | K::ThisType
        | K::MappedType
        | K::NamedTupleMember
        | K::TemplateLiteralType
        | K::ImportType
        | K::PropertyAccessExpression
        | K::ExpressionWithTypeArguments => TypePrecedence::NonArray,
        _ => return Ok(None),
    }))
}
