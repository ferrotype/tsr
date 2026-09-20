//! Rule contexts (`format/rulecontext.go`): the filters that narrow where a
//! rule applies. Each reads the formatting context the span worker has set up
//! for one pair of adjacent tokens.

// Every predicate has the one signature the rule table stores, and most of them
// read the tree, which can fail. The few that only look at token kinds keep the
// `Result` for that reason.
#![allow(clippy::unnecessary_wraps)]

use crate::{
    context::{FormatRequestKind, FormattingContext},
    lsutil,
    scanner::TextRangeWithKind,
    Error,
};
use tsr_arena::NodeId;
use tsr_ast::{utilities, utilities_middle, SyntaxKind as K};

type Context<'c, 'a, 'p, 'f> = &'c mut FormattingContext<'a, 'p, 'f>;
type Predicate = Result<bool, Error>;

/// Upstream dereferences these without a check: they are set by
/// `UpdateContext` before any rule is asked.
fn required(node: Option<NodeId>) -> Result<NodeId, Error> {
    node.ok_or_else(|| Error::Assertion("the formatting context has not been updated".into()))
}

fn kind_of(context: &FormattingContext<'_, '_, '_>, node: NodeId) -> Result<Option<K>, Error> {
    Ok(context.file.node(node)?.kind().known())
}

fn context_kind(context: &FormattingContext<'_, '_, '_>) -> Result<Option<K>, Error> {
    kind_of(context, required(context.context_node)?)
}

fn current_parent_kind(context: &FormattingContext<'_, '_, '_>) -> Result<Option<K>, Error> {
    kind_of(context, required(context.current_token_parent)?)
}

fn next_parent_kind(context: &FormattingContext<'_, '_, '_>) -> Result<Option<K>, Error> {
    kind_of(context, required(context.next_token_parent)?)
}

// port: tsc/internal/format/rulecontext.go:isForContext
pub(crate) fn is_for_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::ForStatement))
}

// port: tsc/internal/format/rulecontext.go:isNotForContext
pub(crate) fn is_not_for_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_for_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isBinaryOpContext
pub(crate) fn is_binary_op_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    let (current, next) = (
        context.current_token_span.kind,
        context.next_token_span.kind,
    );
    Ok(match context_kind(context)? {
        Some(K::BinaryExpression) => {
            let node = context.file.node(required(context.context_node)?)?;
            let operator = node
                .data_source()
                .as_binary_expression()
                .and_then(|data| data.operator_token())
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            context.file.node(operator)?.kind() != K::CommaToken
        }
        Some(
            K::ConditionalExpression
            | K::ConditionalType
            | K::AsExpression
            | K::ExportSpecifier
            | K::ImportSpecifier
            | K::TypePredicate
            | K::UnionType
            | K::IntersectionType
            | K::SatisfiesExpression,
        ) => true,
        // The equals sign of a binding element, a type alias, an import or
        // export assignment, a declaration, a parameter or a member.
        Some(
            K::BindingElement
            | K::TypeAliasDeclaration
            | K::ImportEqualsDeclaration
            | K::ExportAssignment
            | K::VariableDeclaration
            | K::Parameter
            | K::EnumMember
            | K::PropertyDeclaration
            | K::PropertySignature,
        ) => current == K::EqualsToken || next == K::EqualsToken,
        // `in` in `for (let x in [])` and in `[P in keyof T]`.
        Some(K::ForInStatement | K::TypeParameter) => {
            current == K::InKeyword
                || next == K::InKeyword
                || current == K::EqualsToken
                || next == K::EqualsToken
        }
        // `of` is not a binary operator, but it is formatted like `in`.
        Some(K::ForOfStatement) => current == K::OfKeyword || next == K::OfKeyword,
        _ => false,
    })
}

// port: tsc/internal/format/rulecontext.go:isNotBinaryOpContext
pub(crate) fn is_not_binary_op_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_binary_op_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isNotTypeAnnotationContext
pub(crate) fn is_not_type_annotation_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_type_annotation_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isTypeAnnotationContext
pub(crate) fn is_type_annotation_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    let node = context.file.node(required(context.context_node)?)?;
    Ok(matches!(
        node.kind().known(),
        Some(K::PropertyDeclaration | K::PropertySignature | K::Parameter | K::VariableDeclaration)
    ) || utilities::is_function_like_kind(node.kind()))
}

// port: tsc/internal/format/rulecontext.go:isOptionalPropertyContext
fn is_optional_property_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    let node = context.file.node(required(context.context_node)?)?;
    Ok(node.kind() == K::PropertyDeclaration
        && utilities_middle::has_question_token(context.file.view, &node)?)
}

// port: tsc/internal/format/rulecontext.go:isNonOptionalPropertyContext
pub(crate) fn is_non_optional_property_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_optional_property_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isConditionalOperatorContext
pub(crate) fn is_conditional_operator_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(K::ConditionalExpression | K::ConditionalType)
    ))
}

// port: tsc/internal/format/rulecontext.go:isSameLineTokenOrBeforeBlockContext
pub(crate) fn is_same_line_token_or_before_block_context(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(context.tokens_are_on_same_line()? || is_before_block_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isBraceWrappedContext
pub(crate) fn is_brace_wrapped_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(K::ObjectBindingPattern | K::MappedType)
    ) || is_single_line_block_context(context)?)
}

/// Asked before an open brace in a control construct, a function or a
/// TypeScript block declaration.
// port: tsc/internal/format/rulecontext.go:isBeforeMultilineBlockContext
pub(crate) fn is_before_multiline_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(is_before_block_context(context)?
        && !(context.next_node_all_on_same_line()? || context.next_node_block_is_on_one_line()?))
}

// port: tsc/internal/format/rulecontext.go:isMultilineBlockContext
pub(crate) fn is_multiline_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(is_block_context(context)?
        && !(context.context_node_all_on_same_line()?
            || context.context_node_block_is_on_one_line()?))
}

// port: tsc/internal/format/rulecontext.go:isSingleLineBlockContext
fn is_single_line_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(is_block_context(context)?
        && (context.context_node_all_on_same_line()?
            || context.context_node_block_is_on_one_line()?))
}

// port: tsc/internal/format/rulecontext.go:isBlockContext
fn is_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(node_is_block_context(context_kind(context)?))
}

// port: tsc/internal/format/rulecontext.go:isBeforeBlockContext
pub(crate) fn is_before_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(node_is_block_context(next_parent_kind(context)?))
}

/// True only for nodes that have their open and close braces as immediate
/// children.
// port: tsc/internal/format/rulecontext.go:nodeIsBlockContext
fn node_is_block_context(kind: Option<K>) -> bool {
    // A class, module, enum or object type looks like a block to the user but
    // is not one in the grammar.
    node_is_typescript_decl_with_block_context(kind)
        || matches!(
            kind,
            Some(K::Block | K::CaseBlock | K::ObjectLiteralExpression | K::ModuleBlock)
        )
}

// port: tsc/internal/format/rulecontext.go:isFunctionDeclContext
pub(crate) fn is_function_decl_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(
            K::FunctionDeclaration
                | K::MethodDeclaration
                | K::MethodSignature
                | K::GetAccessor
                | K::SetAccessor
                | K::CallSignature
                | K::FunctionExpression
                | K::Constructor
                | K::ArrowFunction
                // Not truly a function, but it formats like one.
                | K::InterfaceDeclaration
        )
    ))
}

// port: tsc/internal/format/rulecontext.go:isNotFunctionDeclContext
pub(crate) fn is_not_function_decl_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_function_decl_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isFunctionDeclarationOrFunctionExpressionContext
pub(crate) fn is_function_declaration_or_function_expression_context(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(K::FunctionDeclaration | K::FunctionExpression)
    ))
}

// port: tsc/internal/format/rulecontext.go:isTypeScriptDeclWithBlockContext
pub(crate) fn is_typescript_decl_with_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(node_is_typescript_decl_with_block_context(context_kind(
        context,
    )?))
}

// port: tsc/internal/format/rulecontext.go:nodeIsTypeScriptDeclWithBlockContext
fn node_is_typescript_decl_with_block_context(kind: Option<K>) -> bool {
    matches!(
        kind,
        Some(
            K::ClassDeclaration
                | K::ClassExpression
                | K::InterfaceDeclaration
                | K::EnumDeclaration
                | K::TypeLiteral
                | K::ModuleDeclaration
                | K::ExportDeclaration
                | K::NamedExports
                | K::ImportDeclaration
                | K::NamedImports
        )
    )
}

// port: tsc/internal/format/rulecontext.go:isAfterCodeBlockContext
pub(crate) fn is_after_code_block_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(match current_parent_kind(context)? {
        Some(
            K::ClassDeclaration
            | K::ModuleDeclaration
            | K::EnumDeclaration
            | K::CatchClause
            | K::ModuleBlock
            | K::SwitchStatement,
        ) => true,
        Some(K::Block) => {
            // In a codefix the parents may not be set, so a block without one
            // counts.
            let block = context.file.node(required(context.current_token_parent)?)?;
            match block.parent() {
                None => true,
                Some(parent) => !matches!(
                    kind_of(context, parent)?,
                    Some(K::ArrowFunction | K::FunctionExpression)
                ),
            }
        }
        _ => false,
    })
}

// port: tsc/internal/format/rulecontext.go:isControlDeclContext
pub(crate) fn is_control_decl_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(
            K::IfStatement
                | K::SwitchStatement
                | K::ForStatement
                | K::ForInStatement
                | K::ForOfStatement
                | K::WhileStatement
                | K::TryStatement
                | K::DoStatement
                | K::WithStatement
                | K::CatchClause
        )
    ))
}

// port: tsc/internal/format/rulecontext.go:isObjectContext
pub(crate) fn is_object_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::ObjectLiteralExpression))
}

// port: tsc/internal/format/rulecontext.go:isFunctionCallContext
fn is_function_call_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::CallExpression))
}

// port: tsc/internal/format/rulecontext.go:isNewContext
fn is_new_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::NewExpression))
}

// port: tsc/internal/format/rulecontext.go:isFunctionCallOrNewContext
pub(crate) fn is_function_call_or_new_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(is_function_call_context(context)? || is_new_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isPreviousTokenNotComma
pub(crate) fn is_previous_token_not_comma(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context.current_token_span.kind != K::CommaToken)
}

// port: tsc/internal/format/rulecontext.go:isNextTokenNotCloseBracket
pub(crate) fn is_next_token_not_close_bracket(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context.next_token_span.kind != K::CloseBracketToken)
}

// port: tsc/internal/format/rulecontext.go:isNextTokenNotCloseParen
pub(crate) fn is_next_token_not_close_paren(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context.next_token_span.kind != K::CloseParenToken)
}

// port: tsc/internal/format/rulecontext.go:isArrowFunctionContext
pub(crate) fn is_arrow_function_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::ArrowFunction))
}

// port: tsc/internal/format/rulecontext.go:isImportTypeContext
pub(crate) fn is_import_type_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::ImportType))
}

// port: tsc/internal/format/rulecontext.go:isNonJsxSameLineTokenContext
pub(crate) fn is_non_jsx_same_line_token_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context.tokens_are_on_same_line()? && context_kind(context)? != Some(K::JsxText))
}

// port: tsc/internal/format/rulecontext.go:isNonJsxTextContext
pub(crate) fn is_non_jsx_text_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? != Some(K::JsxText))
}

// port: tsc/internal/format/rulecontext.go:isNonJsxElementOrFragmentContext
pub(crate) fn is_non_jsx_element_or_fragment_context(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(!matches!(
        context_kind(context)?,
        Some(K::JsxElement | K::JsxFragment)
    ))
}

// port: tsc/internal/format/rulecontext.go:isJsxExpressionContext
pub(crate) fn is_jsx_expression_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(K::JsxExpression | K::JsxSpreadAttribute)
    ))
}

// port: tsc/internal/format/rulecontext.go:isNextTokenParentJsxAttribute
pub(crate) fn is_next_token_parent_jsx_attribute(context: Context<'_, '_, '_, '_>) -> Predicate {
    let parent = context.file.node(required(context.next_token_parent)?)?;
    if parent.kind() == K::JsxAttribute {
        return Ok(true);
    }
    if parent.kind() != K::JsxNamespacedName {
        return Ok(false);
    }
    // Upstream dereferences the namespaced name's parent without a check.
    let grandparent = parent.parent().ok_or_else(|| {
        Error::Assertion("runtime error: invalid memory address or nil pointer dereference".into())
    })?;
    Ok(kind_of(context, grandparent)? == Some(K::JsxAttribute))
}

// port: tsc/internal/format/rulecontext.go:isJsxAttributeContext
pub(crate) fn is_jsx_attribute_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::JsxAttribute))
}

// port: tsc/internal/format/rulecontext.go:isNextTokenParentNotJsxNamespacedName
pub(crate) fn is_next_token_parent_not_jsx_namespaced_name(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(next_parent_kind(context)? != Some(K::JsxNamespacedName))
}

// port: tsc/internal/format/rulecontext.go:isNextTokenParentJsxNamespacedName
pub(crate) fn is_next_token_parent_jsx_namespaced_name(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(next_parent_kind(context)? == Some(K::JsxNamespacedName))
}

// port: tsc/internal/format/rulecontext.go:isJsxSelfClosingElementContext
pub(crate) fn is_jsx_self_closing_element_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::JsxSelfClosingElement))
}

// port: tsc/internal/format/rulecontext.go:isNotBeforeBlockInFunctionDeclarationContext
pub(crate) fn is_not_before_block_in_function_declaration_context(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(!is_function_decl_context(context)? && !is_before_block_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isEndOfDecoratorContextOnSameLine
pub(crate) fn is_end_of_decorator_context_on_same_line(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    if !context.tokens_are_on_same_line()? {
        return Ok(false);
    }
    let node = context.file.node(required(context.context_node)?)?;
    if !utilities_middle::has_decorators(context.file.view, &node)? {
        return Ok(false);
    }
    Ok(
        node_is_in_decorator_context(context, context.current_token_parent)?
            && !node_is_in_decorator_context(context, context.next_token_parent)?,
    )
}

// port: tsc/internal/format/rulecontext.go:nodeIsInDecoratorContext
fn node_is_in_decorator_context(
    context: &FormattingContext<'_, '_, '_>,
    mut node: Option<NodeId>,
) -> Predicate {
    while let Some(id) = node {
        let skipped = tsr_ast::skip_partially_emitted_expressions(context.file.view, id)?;
        if !utilities::is_expression_kind(context.file.node(skipped)?.kind()) {
            break;
        }
        node = context.file.node(id)?.parent();
    }
    Ok(match node {
        Some(id) => kind_of(context, id)? == Some(K::Decorator),
        None => false,
    })
}

// port: tsc/internal/format/rulecontext.go:isStartOfVariableDeclarationList
pub(crate) fn is_start_of_variable_declaration_list(context: Context<'_, '_, '_, '_>) -> Predicate {
    let parent = required(context.current_token_parent)?;
    Ok(
        kind_of(context, parent)? == Some(K::VariableDeclarationList)
            && context.file.token_pos(parent)? == context.current_token_span.loc.pos(),
    )
}

// port: tsc/internal/format/rulecontext.go:isNotFormatOnEnter
pub(crate) fn is_not_format_on_enter(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context.formatting_request_kind != FormatRequestKind::FormatOnEnter)
}

// port: tsc/internal/format/rulecontext.go:isModuleDeclContext
pub(crate) fn is_module_decl_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::ModuleDeclaration))
}

// port: tsc/internal/format/rulecontext.go:isObjectTypeContext
pub(crate) fn is_object_type_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::TypeLiteral))
}

// port: tsc/internal/format/rulecontext.go:isConstructorSignatureContext
pub(crate) fn is_constructor_signature_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::ConstructSignature))
}

// port: tsc/internal/format/rulecontext.go:isTypeArgumentOrParameterOrAssertion
fn is_type_argument_or_parameter_or_assertion(token: TextRangeWithKind, parent: Option<K>) -> bool {
    if token.kind != K::LessThanToken && token.kind != K::GreaterThanToken {
        return false;
    }
    matches!(
        parent,
        Some(
            K::TypeReference
                | K::TypeAssertionExpression
                | K::TypeAliasDeclaration
                | K::ClassDeclaration
                | K::ClassExpression
                | K::InterfaceDeclaration
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::MethodSignature
                | K::CallSignature
                | K::ConstructSignature
                | K::CallExpression
                | K::NewExpression
                | K::ExpressionWithTypeArguments
        )
    )
}

// port: tsc/internal/format/rulecontext.go:isTypeArgumentOrParameterOrAssertionContext
pub(crate) fn is_type_argument_or_parameter_or_assertion_context(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    Ok(is_type_argument_or_parameter_or_assertion(
        context.current_token_span,
        current_parent_kind(context)?,
    ) || is_type_argument_or_parameter_or_assertion(
        context.next_token_span,
        next_parent_kind(context)?,
    ))
}

// port: tsc/internal/format/rulecontext.go:isTypeAssertionContext
pub(crate) fn is_type_assertion_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::TypeAssertionExpression))
}

// port: tsc/internal/format/rulecontext.go:isNonTypeAssertionContext
pub(crate) fn is_non_type_assertion_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_type_assertion_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isVoidOpContext
pub(crate) fn is_void_op_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context.current_token_span.kind == K::VoidKeyword
        && current_parent_kind(context)? == Some(K::VoidExpression))
}

// port: tsc/internal/format/rulecontext.go:isYieldOrYieldStarWithOperand
pub(crate) fn is_yield_or_yield_star_with_operand(context: Context<'_, '_, '_, '_>) -> Predicate {
    let node = context.file.node(required(context.context_node)?)?;
    Ok(node.kind() == K::YieldExpression && node.expression().is_some())
}

// port: tsc/internal/format/rulecontext.go:isNonNullAssertionContext
pub(crate) fn is_non_null_assertion_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(context_kind(context)? == Some(K::NonNullExpression))
}

// port: tsc/internal/format/rulecontext.go:isNotStatementConditionContext
pub(crate) fn is_not_statement_condition_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(!is_statement_condition_context(context)?)
}

// port: tsc/internal/format/rulecontext.go:isStatementConditionContext
fn is_statement_condition_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    Ok(matches!(
        context_kind(context)?,
        Some(
            K::IfStatement
                | K::ForStatement
                | K::ForInStatement
                | K::ForOfStatement
                | K::DoStatement
                | K::WhileStatement
        )
    ))
}

// port: tsc/internal/format/rulecontext.go:isSemicolonDeletionContext
pub(crate) fn is_semicolon_deletion_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    let mut next_kind = context.next_token_span.kind;
    let mut next_start = context.next_token_span.loc.pos();
    let (current_parent, next_parent) = (
        required(context.current_token_parent)?,
        required(context.next_token_parent)?,
    );
    if utilities_middle::is_trivia(next_kind.into()) {
        // Upstream notes this differs from the original, whose search for an
        // ancestor without a parent always found the source file.
        let next_real = if next_parent == current_parent {
            let source = context.file.source;
            context
                .file
                .navigator()
                .find_next_token(next_parent, source)?
        } else {
            lsutil::get_first_token(context.file, next_parent)?
        };
        let Some(next_real) = next_real else {
            return Ok(true);
        };
        // A token's kind is always a known kind.
        next_kind = context
            .file
            .node(next_real)?
            .kind()
            .known()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        next_start = context.file.token_pos(next_real)?;
    }

    let start_line = context.file.line_of(context.current_token_span.loc.pos())?;
    let end_line = context.file.line_of(next_start)?;
    if start_line == end_line {
        return Ok(next_kind == K::CloseBraceToken || next_kind == K::EndOfFile);
    }
    if next_kind == K::SemicolonToken && context.current_token_span.kind == K::SemicolonToken {
        return Ok(true);
    }
    if next_kind == K::SemicolonClassElement || next_kind == K::SemicolonToken {
        return Ok(false);
    }
    let current = context.file.node(current_parent)?;
    if matches!(
        context_kind(context)?,
        Some(K::InterfaceDeclaration | K::TypeAliasDeclaration)
    ) {
        // A semicolon after `foo` cannot go when `()` follows on the next line:
        // the two would parse as a method declaration.
        return Ok(current.kind() != K::PropertySignature
            || current.type_node().is_some()
            || next_kind != K::OpenParenToken);
    }
    if current.kind() == K::PropertyDeclaration {
        return Ok(current.initializer().is_none());
    }
    Ok(!matches!(
        current.kind().known(),
        Some(K::ForStatement | K::EmptyStatement | K::SemicolonClassElement)
    ) && !matches!(
        next_kind,
        K::OpenBracketToken
            | K::OpenParenToken
            | K::PlusToken
            | K::MinusToken
            | K::SlashToken
            | K::RegularExpressionLiteral
            | K::CommaToken
            | K::TemplateExpression
            | K::TemplateHead
            | K::NoSubstitutionTemplateLiteral
            | K::DotToken
    ))
}

// port: tsc/internal/format/rulecontext.go:isSemicolonInsertionContext
pub(crate) fn is_semicolon_insertion_context(context: Context<'_, '_, '_, '_>) -> Predicate {
    let parent = required(context.current_token_parent)?;
    lsutil::position_is_asi_candidate(context.current_token_span.loc.end(), parent, context.file)
}

// port: tsc/internal/format/rulecontext.go:isNotPropertyAccessOnIntegerLiteral
pub(crate) fn is_not_property_access_on_integer_literal(
    context: Context<'_, '_, '_, '_>,
) -> Predicate {
    let node = context.file.node(required(context.context_node)?)?;
    if node.kind() != K::PropertyAccessExpression {
        return Ok(true);
    }
    let Some(expression) = node.expression() else {
        return Ok(true);
    };
    let expression = context.file.node(expression)?;
    let Some(literal) = expression.data_source().as_numeric_literal() else {
        return Ok(true);
    };
    Ok(literal.text().contains(&b'.'))
}
