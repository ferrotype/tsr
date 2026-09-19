use crate::{NodeKind, SyntaxKind};

/// Pinned Go operator precedence values, including the invalid sentinel.
pub mod operator_precedence {
    pub const COMMA: i32 = 0;
    pub const SPREAD: i32 = 1;
    pub const YIELD: i32 = 2;
    pub const ASSIGNMENT: i32 = 3;
    pub const CONDITIONAL: i32 = 4;
    pub const LOGICAL_OR: i32 = 5;
    pub const LOGICAL_AND: i32 = 6;
    pub const BITWISE_OR: i32 = 7;
    pub const BITWISE_XOR: i32 = 8;
    pub const BITWISE_AND: i32 = 9;
    pub const EQUALITY: i32 = 10;
    pub const RELATIONAL: i32 = 11;
    pub const SHIFT: i32 = 12;
    pub const ADDITIVE: i32 = 13;
    pub const MULTIPLICATIVE: i32 = 14;
    pub const EXPONENTIATION: i32 = 15;
    pub const UNARY: i32 = 16;
    pub const UPDATE: i32 = 17;
    pub const LEFT_HAND_SIDE: i32 = 18;
    pub const OPTIONAL_CHAIN: i32 = 19;
    pub const MEMBER: i32 = 20;
    pub const PRIMARY: i32 = 21;
    pub const PARENTHESES: i32 = 22;
    pub const LOWEST: i32 = COMMA;
    pub const HIGHEST: i32 = PARENTHESES;
    pub const DISALLOW_COMMA: i32 = YIELD;
    pub const COALESCE: i32 = LOGICAL_OR;
    pub const INVALID: i32 = -1;
}

// port: tsc/internal/ast/precedence.go:GetBinaryOperatorPrecedence
pub fn get_binary_operator_precedence(operator: NodeKind) -> i32 {
    use operator_precedence as p;
    use SyntaxKind as K;
    match operator.known() {
        Some(K::QuestionQuestionToken) => p::COALESCE,
        Some(K::BarBarToken) => p::LOGICAL_OR,
        Some(K::AmpersandAmpersandToken) => p::LOGICAL_AND,
        Some(K::BarToken) => p::BITWISE_OR,
        Some(K::CaretToken) => p::BITWISE_XOR,
        Some(K::AmpersandToken) => p::BITWISE_AND,
        Some(
            K::EqualsEqualsToken
            | K::ExclamationEqualsToken
            | K::EqualsEqualsEqualsToken
            | K::ExclamationEqualsEqualsToken,
        ) => p::EQUALITY,
        Some(
            K::LessThanToken
            | K::GreaterThanToken
            | K::LessThanEqualsToken
            | K::GreaterThanEqualsToken
            | K::InstanceOfKeyword
            | K::InKeyword
            | K::AsKeyword
            | K::SatisfiesKeyword,
        ) => p::RELATIONAL,
        Some(
            K::LessThanLessThanToken
            | K::GreaterThanGreaterThanToken
            | K::GreaterThanGreaterThanGreaterThanToken,
        ) => p::SHIFT,
        Some(K::PlusToken | K::MinusToken) => p::ADDITIVE,
        Some(K::AsteriskToken | K::SlashToken | K::PercentToken) => p::MULTIPLICATIVE,
        Some(K::AsteriskAsteriskToken) => p::EXPONENTIATION,
        _ => p::INVALID,
    }
}

/// `OperatorPrecedenceFlags`.
pub mod operator_precedence_flags {
    pub const NONE: u32 = 0;
    pub const NEW_WITHOUT_ARGUMENTS: u32 = 1 << 0;
    pub const OPTIONAL_CHAIN: u32 = 1 << 1;
}

// port: tsc/internal/ast/precedence.go:getOperator
fn get_operator(
    view: crate::AstView<'_>,
    node: &crate::NodeRead<'_>,
) -> Result<NodeKind, ts_arena::Error> {
    let data = node.data_source();
    if let Some(binary) = data.as_binary_expression() {
        let token = binary
            .operator_token()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        return Ok(view.node(token)?.kind());
    }
    if let Some(prefix) = data.as_prefix_unary_expression() {
        return Ok(prefix.operator());
    }
    if let Some(postfix) = data.as_postfix_unary_expression() {
        return Ok(postfix.operator());
    }
    Ok(node.kind())
}

// port: tsc/internal/ast/precedence.go:GetExpressionPrecedence
pub fn get_expression_precedence(
    view: crate::AstView<'_>,
    node: &crate::NodeRead<'_>,
) -> Result<i32, ts_arena::Error> {
    let operator = get_operator(view, node)?;
    let flags = if node.kind() == SyntaxKind::NewExpression && node.argument_list().is_none() {
        operator_precedence_flags::NEW_WITHOUT_ARGUMENTS
    } else if crate::utilities::is_optional_chain(node) {
        operator_precedence_flags::OPTIONAL_CHAIN
    } else {
        operator_precedence_flags::NONE
    };
    Ok(get_operator_precedence(node.kind(), operator, flags))
}

/// Three entries differ from Strada on purpose, to line up with the
/// parenthesizer rules: arrow functions, member access and `new`.
// port: tsc/internal/ast/precedence.go:GetOperatorPrecedence
pub fn get_operator_precedence(node_kind: NodeKind, operator_kind: NodeKind, flags: u32) -> i32 {
    use operator_precedence as p;
    use SyntaxKind as K;
    match node_kind.known() {
        Some(K::SpreadElement) => p::SPREAD,
        Some(K::YieldExpression) => p::YIELD,
        Some(K::ArrowFunction) => p::ASSIGNMENT,
        Some(K::ConditionalExpression) => p::CONDITIONAL,
        Some(K::BinaryExpression) => match operator_kind.known() {
            Some(K::CommaToken) => p::COMMA,
            Some(
                K::EqualsToken
                | K::PlusEqualsToken
                | K::MinusEqualsToken
                | K::AsteriskAsteriskEqualsToken
                | K::AsteriskEqualsToken
                | K::SlashEqualsToken
                | K::PercentEqualsToken
                | K::LessThanLessThanEqualsToken
                | K::GreaterThanGreaterThanEqualsToken
                | K::GreaterThanGreaterThanGreaterThanEqualsToken
                | K::AmpersandEqualsToken
                | K::CaretEqualsToken
                | K::BarEqualsToken
                | K::BarBarEqualsToken
                | K::AmpersandAmpersandEqualsToken
                | K::QuestionQuestionEqualsToken,
            ) => p::ASSIGNMENT,
            _ => get_binary_operator_precedence(operator_kind),
        },
        // Upstream asks whether prefix `++` and `--` belong with `Update`.
        Some(
            K::TypeAssertionExpression
            | K::NonNullExpression
            | K::PrefixUnaryExpression
            | K::TypeOfExpression
            | K::VoidExpression
            | K::DeleteExpression
            | K::AwaitExpression,
        ) => p::UNARY,
        Some(K::PostfixUnaryExpression) => p::UPDATE,
        Some(K::PropertyAccessExpression | K::ElementAccessExpression | K::CallExpression) => {
            if flags & operator_precedence_flags::OPTIONAL_CHAIN != 0 {
                p::OPTIONAL_CHAIN
            } else {
                p::MEMBER
            }
        }
        Some(K::NewExpression) => {
            if flags & operator_precedence_flags::NEW_WITHOUT_ARGUMENTS != 0 {
                p::LEFT_HAND_SIDE
            } else {
                p::MEMBER
            }
        }
        Some(K::TaggedTemplateExpression | K::MetaProperty | K::ExpressionWithTypeArguments) => {
            p::MEMBER
        }
        Some(K::AsExpression | K::SatisfiesExpression) => p::RELATIONAL,
        Some(
            K::ThisKeyword
            | K::SuperKeyword
            | K::ImportKeyword
            | K::Identifier
            | K::PrivateIdentifier
            | K::NullKeyword
            | K::TrueKeyword
            | K::FalseKeyword
            | K::NumericLiteral
            | K::BigIntLiteral
            | K::StringLiteral
            | K::ArrayLiteralExpression
            | K::ObjectLiteralExpression
            | K::FunctionExpression
            | K::ClassExpression
            | K::RegularExpressionLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::TemplateExpression
            | K::OmittedExpression
            | K::JsxElement
            | K::JsxSelfClosingElement
            | K::JsxFragment
            | K::MissingDeclaration,
        ) => p::PRIMARY,
        Some(K::ParenthesizedExpression) => p::PARENTHESES,
        _ => p::INVALID,
    }
}

// port: tsc/internal/ast/precedence.go:GetLeftmostExpression
pub fn get_leftmost_expression(
    view: crate::AstView<'_>,
    mut node: crate::NodeId,
    stop_at_call_expressions: bool,
) -> Result<crate::NodeId, ts_arena::Error> {
    use SyntaxKind as K;
    loop {
        let read = view.node(node)?;
        let data = read.data_source();
        let next = match read.kind().known() {
            Some(K::PostfixUnaryExpression) => data
                .as_postfix_unary_expression()
                .and_then(|postfix| postfix.operand()),
            Some(K::BinaryExpression) => data.as_binary_expression().and_then(|b| b.left()),
            Some(K::ConditionalExpression) => data
                .as_conditional_expression()
                .and_then(|conditional| conditional.condition()),
            Some(K::TaggedTemplateExpression) => data
                .as_tagged_template_expression()
                .and_then(|tagged| tagged.tag()),
            Some(K::CallExpression) if stop_at_call_expressions => return Ok(node),
            Some(
                K::CallExpression
                | K::AsExpression
                | K::ElementAccessExpression
                | K::PropertyAccessExpression
                | K::NonNullExpression
                | K::PartiallyEmittedExpression
                | K::SatisfiesExpression,
            ) => read.expression(),
            _ => return Ok(node),
        };
        // Upstream dereferences the child; without one the walk ends here.
        match next {
            Some(next) => node = next,
            None => return Ok(node),
        }
    }
}
