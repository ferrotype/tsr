use crate::{
    completion_context::Context,
    signature_arguments::{ArgumentInfo, Invocation},
    syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{utilities as ast, NodeId, SyntaxKind as K};
use tsr_checker::{context_flags as cf, type_flags as tf, Operation, TypeRef};
use tsr_lsproto as lsp;

fn equality(kind: tsr_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::EqualsEqualsToken
                | K::EqualsEqualsEqualsToken
                | K::ExclamationEqualsToken
                | K::ExclamationEqualsEqualsToken
        )
    )
}
impl Syntax<'_> {
    // port: tsc/internal/ls/completions.go:getArgumentInfoForCompletions
    pub(crate) fn completion_argument(
        &mut self,
        node: NodeId,
        position: i64,
        checker: &mut Operation<'_>,
    ) -> Result<Option<ArgumentInfo>> {
        Ok(self
            .immediate_argument(node, position, checker)?
            .filter(|info| !info.type_arguments && matches!(info.invocation, Invocation::Call(_))))
    }
    // port: tsc/internal/ls/utilities.go:getContextualTypeFromParent
    pub(crate) fn contextual_type_from_parent(
        &self,
        node: NodeId,
        checker: &mut Operation<'_>,
        flags: u32,
    ) -> Result<Option<TypeRef>> {
        let Some(mut parent) = self.view.node(node)?.parent() else {
            return Ok(None);
        };
        while self.view.node(parent)?.kind() == K::ParenthesizedExpression {
            let Some(next) = self.view.node(parent)?.parent() else {
                return Ok(None);
            };
            parent = next;
        }
        let read = self.view.node(parent)?;
        Ok(match read.kind().known() {
            Some(K::NewExpression) => checker.get_contextual_type(parent, flags)?,
            Some(K::BinaryExpression) => {
                let data = read.data_source().as_binary_expression().unwrap();
                if data
                    .operator_token()
                    .is_some_and(|n| self.view.node(n).is_ok_and(|n| equality(n.kind())))
                {
                    let other = if data.right() == Some(node) {
                        data.left()
                    } else {
                        data.right()
                    };
                    other.map(|n| checker.get_type_at_location(n)).transpose()?
                } else {
                    checker.get_contextual_type(node, flags)?
                }
            }
            Some(K::CaseClause) => self.switched_type(parent, checker)?,
            _ => checker.get_contextual_type(node, flags)?,
        })
    }
    pub(crate) fn switched_type(
        &self,
        clause: NodeId,
        checker: &mut Operation<'_>,
    ) -> Result<Option<TypeRef>> {
        let block = self
            .view
            .node(clause)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let switch = self
            .view
            .node(block)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        Ok(self
            .view
            .node(switch)?
            .expression()
            .map(|expr| checker.get_type_at_location(expr))
            .transpose()?)
    }
    // port: tsc/internal/ls/completions.go:getContextualType
    pub(crate) fn completion_context_type(
        &mut self,
        previous: NodeId,
        position: i64,
        checker: &mut Operation<'_>,
    ) -> Result<Option<TypeRef>> {
        let read = self.view.node(previous)?;
        let Some(parent) = read.parent() else {
            return Ok(None);
        };
        let pr = self.view.node(parent)?;
        match read.kind().known() {
            Some(K::Identifier) => {
                return self.contextual_type_from_parent(previous, checker, cf::NONE)
            }
            Some(K::EqualsToken) => {
                return Ok(match pr.kind().known() {
                    Some(K::VariableDeclaration) => {
                        if let Some(init) = pr.initializer() {
                            checker.get_contextual_type(init, cf::NONE)?
                        } else {
                            None
                        }
                    }
                    Some(K::BinaryExpression) => pr
                        .data_source()
                        .as_binary_expression()
                        .and_then(|d| d.left())
                        .map(|n| checker.get_type_at_location(n))
                        .transpose()?,
                    Some(K::JsxAttribute) => {
                        checker.get_contextual_type_for_jsx_attribute(parent)?
                    }
                    _ => None,
                })
            }
            Some(K::NewKeyword) => return Ok(checker.get_contextual_type(parent, cf::NONE)?),
            Some(K::CaseKeyword) => {
                return if pr.kind() == K::CaseClause {
                    self.switched_type(parent, checker)
                } else {
                    Ok(None)
                }
            }
            Some(K::OpenBraceToken) => {
                return Ok(if pr.kind() == K::JsxExpression {
                    if let Some(attribute) = pr.parent().filter(|&n| {
                        self.view.node(n).is_ok_and(|n| {
                            !matches!(n.kind().known(), Some(K::JsxElement | K::JsxFragment))
                        })
                    }) {
                        checker.get_contextual_type_for_jsx_attribute(attribute)?
                    } else {
                        None
                    }
                } else {
                    None
                })
            }
            Some(K::OpenBracketToken | K::CommaToken) if pr.kind() == K::ArrayLiteralExpression => {
                let ty = checker.get_contextual_type(parent, cf::NONE)?;
                return Ok(checker
                    .get_contextual_type_for_array_literal_at_position(ty, parent, position)?);
            }
            Some(K::QuestionToken | K::ColonToken) if pr.kind() == K::ConditionalExpression => {
                if let Some(info) = self.completion_argument(parent, position, checker)? {
                    return Ok(checker.get_contextual_type_for_argument_at_index(
                        info.invocation.enclosing(),
                        info.index,
                    )?);
                }
                return Ok(
                    match checker.get_contextual_type(parent, cf::IGNORE_NODE_INFERENCES)? {
                        Some(ty) => Some(ty),
                        None => checker.get_contextual_type(parent, cf::NONE)?,
                    },
                );
            }
            Some(K::OpenBracketToken | K::CloseBracketToken | K::QuestionToken) => return Ok(None),
            _ => {}
        }
        if let Some(info) = self.completion_argument(previous, position, checker)? {
            return Ok(checker.get_contextual_type_for_argument_at_index(
                info.invocation.enclosing(),
                info.index,
            )?);
        }
        if equality(read.kind()) && pr.kind() == K::BinaryExpression {
            if let Some(left) = pr
                .data_source()
                .as_binary_expression()
                .and_then(|d| d.left())
            {
                return Ok(Some(checker.get_type_at_location(left)?));
            }
        }
        Ok(
            match checker.get_contextual_type(previous, cf::IGNORE_NODE_INFERENCES)? {
                Some(ty) => Some(ty),
                None => checker.get_contextual_type(previous, cf::NONE)?,
            },
        )
    }
}
impl LanguageService<'_> {
    pub(crate) fn literal_completions(
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Vec<Option<Box<lsp::CompletionItem>>>> {
        let Some(previous) = context.previous else {
            return Ok(Vec::new());
        };
        if ast::is_string_literal_like(&syntax.view.node(previous)?) || context.member.is_some() {
            return Ok(Vec::new());
        }
        let Some(ty) = syntax.completion_context_type(previous, position, checker)? else {
            return Ok(Vec::new());
        };
        let types = if checker.type_flags(ty)? & tf::UNION != 0 {
            checker.constituents(ty)?
        } else {
            vec![ty]
        };
        let single = crate::inlay_hints::single_quote(syntax, options.quote)?;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for ty in types {
            let flags = checker.type_flags(ty)?;
            if flags & (tf::STRING_LITERAL | tf::NUMBER_LITERAL | tf::BIG_INT_LITERAL) == 0
                || flags & tf::ENUM_LITERAL != 0
            {
                continue;
            }
            let label = if flags & tf::STRING_LITERAL != 0 {
                let value = checker.string_literal_value(ty)?;
                let ch = if single {
                    tsr_jsstring::QuoteChar::Single
                } else {
                    tsr_jsstring::QuoteChar::Double
                };
                let quote = if single { "'" } else { "\"" };
                format!(
                    "{quote}{}{quote}",
                    String::from_utf8_lossy(&tsr_jsstring::escape::escape_string(
                        value.as_bytes(),
                        ch
                    ))
                )
            } else {
                String::from_utf8_lossy(checker.literal_value_text(ty)?.as_bytes()).into_owned()
            };
            if seen.insert(label.clone()) {
                items.push(Some(Box::new(lsp::CompletionItem {
                    label,
                    kind: Some(Box::new(lsp::CompletionItemKind::CONSTANT)),
                    sort_text: Some(Box::new("11".into())),
                    commit_characters: Some(Box::new(Vec::new())),
                    ..Default::default()
                })));
            }
        }
        Ok(items)
    }
}
