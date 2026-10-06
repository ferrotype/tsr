//! Case values, exhaustive clauses and contextual symbol recommendations.
use crate::{
    completion_context::Context, syntax::Syntax, CompletionOptions, LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{
    symbol_flags as sf, utilities as ast, AstBuilder, FactoryMethods, NodeId, SyntaxKind as K,
};
use tsr_checker::{type_flags as tf, Operation, SymbolRef, TypeRef};
use tsr_lsproto as lsp;
use tsr_printer::emit_resolver::ConstantValue;

#[derive(Default)]
pub(crate) struct CaseValues {
    strings: HashSet<tsr_jsstring::JsString>,
    numbers: HashSet<String>,
    bigints: HashSet<tsr_jsnum::PseudoBigInt>,
}
impl CaseValues {
    fn add(&mut self, value: ConstantValue) {
        match value {
            ConstantValue::String(value) => {
                self.strings.insert(value);
            }
            ConstantValue::Number(value) => {
                self.numbers.insert(value.to_string());
            }
        }
    }
    pub(crate) fn contains_value(&self, value: &ConstantValue) -> bool {
        match value {
            ConstantValue::String(value) => self.strings.contains(value),
            ConstantValue::Number(value) => self.numbers.contains(&value.to_string()),
        }
    }
    pub(crate) fn contains(&self, checker: &Operation<'_>, ty: TypeRef) -> Result<bool> {
        let flags = checker.type_flags(ty)?;
        Ok(if flags & tf::STRING_LITERAL != 0 {
            self.strings.contains(&checker.string_literal_value(ty)?)
        } else if flags & tf::NUMBER_LITERAL != 0 {
            self.numbers.contains(
                &String::from_utf8_lossy(checker.literal_value_text(ty)?.as_bytes()).into_owned(),
            )
        } else if flags & tf::BIG_INT_LITERAL != 0 {
            self.bigints.contains(&tsr_jsnum::parse_valid_big_int(
                checker.literal_value_text(ty)?.as_bytes(),
            ))
        } else {
            false
        })
    }
    // port: tsc/internal/ls/utilities.go:newCaseClauseTracker
    pub(crate) fn new(
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        block: NodeId,
    ) -> Result<Self> {
        let mut result = Self::default();
        for clause in syntax.children(block)? {
            let read = syntax.view.node(clause)?;
            if read.kind() != K::CaseClause {
                continue;
            }
            let Some(mut expression) = read.expression() else {
                continue;
            };
            while syntax.view.node(expression)?.kind() == K::ParenthesizedExpression {
                expression = syntax
                    .view
                    .node(expression)?
                    .expression()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
            }
            match syntax.view.node(expression)?.kind().known() {
                Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                    result.strings.insert(tsr_jsstring::JsString::from_bytes(
                        syntax.view.node_text(expression)?.as_bytes(),
                    ));
                }
                Some(K::NumericLiteral) => {
                    result.numbers.insert(
                        tsr_jsnum::from_string(syntax.view.node_text(expression)?.as_bytes())
                            .to_string(),
                    );
                }
                Some(K::BigIntLiteral) => {
                    result.bigints.insert(tsr_jsnum::parse_valid_big_int(
                        syntax.view.node_text(expression)?.as_bytes(),
                    ));
                }
                _ => {
                    if let Some(symbol) =
                        checker.get_symbol_at_location(read.expression().unwrap())?
                    {
                        if checker.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0 {
                            if let Some(declaration) = checker.symbol(symbol)?.value_declaration() {
                                if let Some(value) = checker.constant_value(declaration)? {
                                    result.add(value);
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(result)
    }
}

pub(crate) fn case_block(syntax: &Syntax<'_>, mut node: Option<NodeId>) -> Result<Option<NodeId>> {
    while let Some(id) = node {
        let read = syntax.view.node(id)?;
        if read.kind() == K::CaseBlock {
            return Ok(Some(id));
        }
        node = read.parent();
    }
    Ok(None)
}

pub(crate) fn expression_case_values(
    checker: &mut Operation<'_>,
    syntax: &Syntax<'_>,
    token: Option<NodeId>,
) -> Result<Option<CaseValues>> {
    let Some(token) = token else {
        return Ok(None);
    };
    let mut node = token;
    while let Some(parent) = syntax.view.node(node)?.parent() {
        let read = syntax.view.node(parent)?;
        if read.kind() == K::CaseClause {
            if read.expression() == Some(node) || syntax.view.node(token)?.kind() == K::CaseKeyword
            {
                return CaseValues::new(
                    checker,
                    syntax,
                    read.parent().ok_or(tsr_arena::Error::InvalidGraph)?,
                )
                .map(Some);
            }
            return Ok(None);
        }
        node = parent;
    }
    Ok(None)
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:getRecommendedCompletion
    pub(crate) fn recommended_completion(
        &self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: i64,
    ) -> Result<Option<SymbolRef>> {
        let Some(previous) = context.previous else {
            return Ok(None);
        };
        let Some(ty) = syntax.completion_context_type(previous, position, checker)? else {
            return Ok(None);
        };
        let types = if checker.type_flags(ty)? & tf::UNION != 0 {
            checker.constituents(ty)?
        } else {
            vec![ty]
        };
        for ty in types {
            let Some(symbol) = checker.type_symbol(ty)? else {
                continue;
            };
            let symbol = checker.symbol_ref(symbol)?;
            let read = checker.symbol(symbol)?;
            if read.flags() & (sf::ENUM_MEMBER | sf::ENUM | sf::CLASS) == 0 {
                continue;
            }
            if read.flags() & sf::CLASS != 0 {
                let mut is_abstract = false;
                for declaration in checker.symbol_declarations(symbol)?.iter().flatten() {
                    if let Some(file) = self.program.file_of_node(declaration) {
                        let view = file.bound().view().ast();
                        if ast::is_class_like(&view.node(declaration)?)
                            && ast::has_syntactic_modifier(
                                view,
                                declaration,
                                tsr_ast::modifier_flags::ABSTRACT,
                            )?
                        {
                            is_abstract = true;
                            break;
                        }
                    }
                }
                if is_abstract {
                    continue;
                }
            }
            if let Some(first) = self.first_symbol_in_chain(checker, symbol, previous)? {
                return Ok(Some(first));
            }
        }
        Ok(None)
    }
    // port: tsc/internal/ls/completions.go:getFirstSymbolInChain
    fn first_symbol_in_chain(
        &self,
        checker: &mut Operation<'_>,
        mut symbol: SymbolRef,
        enclosing: NodeId,
    ) -> Result<Option<SymbolRef>> {
        loop {
            let chain =
                checker.get_accessible_symbol_chain(symbol, Some(enclosing), sf::ALL, false)?;
            if let Some(&first) = chain.first() {
                return Ok(Some(first));
            }
            let Some(parent) = checker.symbol(symbol)?.parent() else {
                return Ok(None);
            };
            let parent = checker.symbol_ref(parent)?;
            for declaration in checker.symbol_declarations(parent)?.iter().flatten() {
                if self
                    .program
                    .file_of_node(declaration)
                    .is_some_and(|f| f.source() == declaration)
                {
                    return Ok(Some(symbol));
                }
            }
            symbol = parent;
        }
    }
    // port: tsc/internal/ls/completions.go:LanguageService.getExhaustiveCaseSnippets
    pub(crate) fn switch_case_completion(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionItem>> {
        if context.member.is_some() || crate::completion_jsx::open_tag(syntax, context)? {
            return Ok(None);
        }
        let Some(block) = case_block(syntax, context.token)? else {
            return Ok(None);
        };
        let switch = syntax
            .view
            .node(block)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let expression = syntax
            .view
            .node(switch)?
            .expression()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let ty = checker.get_type_at_location(expression)?;
        if checker.type_flags(ty)? & tf::UNION == 0 {
            return Ok(None);
        }
        let types = checker.constituents(ty)?;
        for &ty in &types {
            if checker.type_flags(ty)?
                & (tf::STRING_LITERAL | tf::NUMBER_LITERAL | tf::BIG_INT_LITERAL)
                == 0
            {
                return Ok(None);
            }
        }
        let mut used = CaseValues::new(checker, syntax, block)?;
        let mut clauses = Vec::new();
        let mut adder = tsr_autoimport::ImportAdder::default();
        for ty in types {
            self.check_canceled()?;
            if used.contains(checker, ty)? {
                continue;
            }
            let text = if checker.type_flags(ty)? & tf::ENUM_LITERAL != 0 {
                let symbol = checker.symbol_ref(
                    checker
                        .type_symbol(ty)?
                        .ok_or(tsr_arena::Error::InvalidGraph)?,
                )?;
                if let Some(decl) = checker.symbol(symbol)?.value_declaration() {
                    if let Some(value) = checker.constant_value(decl)? {
                        used.add(value);
                    }
                }
                let mut builder = checker.node_builder();
                let Some(node) = builder.type_to_type_node(ty, Some(block), 0, 0)? else {
                    return Ok(None);
                };
                let mut nodes = builder.into_syntax();
                let mut roots = [node];
                self.import_generated_types(
                    checker, syntax, &mut nodes, &mut roots, options, &mut adder,
                )?;
                let single_quote = crate::inlay_hints::single_quote(syntax, options.quote)?;
                let Some(expression) =
                    type_node_to_expression(&mut nodes.ast, roots[0], single_quote)?
                else {
                    return Ok(None);
                };
                let mut writer = tsr_printer::TextWriter::new(b"", 0);
                tsr_printer::Printer::new(
                    tsr_printer::PrinterOptions {
                        remove_comments: true,
                        ..Default::default()
                    },
                    &nodes.emit,
                )
                .write(nodes.ast.view(), expression, None, &mut writer, None)?;
                String::from_utf8_lossy(tsr_printer::EmitTextWriter::text(&writer)).into_owned()
            } else {
                Self::completion_literal_label(checker, ty, syntax, options)?
            };
            clauses.push(format!("case {text}:"));
        }
        let Some(first) = clauses.first() else {
            return Ok(None);
        };
        let label = format!("{first} ...");
        let newline = options.newline.as_deref().unwrap_or("\n");
        let text = clauses
            .iter()
            .enumerate()
            .map(|(i, clause)| {
                if options.snippets {
                    format!("{}${}", clause.replace('$', "\\$"), i + 1)
                } else {
                    clause.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(newline);
        let edits = self.import_adder_edits(syntax, options, &adder)?;
        Ok(Some(lsp::CompletionItem {
            additional_text_edits: (!edits.is_empty()).then(|| Box::new(edits)),
            label: label.clone(),
            kind: Some(Box::new(lsp::CompletionItemKind::SNIPPET)),
            sort_text: Some(Box::new("15".into())),
            insert_text: Some(Box::new(text)),
            insert_text_format: options
                .snippets
                .then(|| Box::new(lsp::InsertTextFormat::SNIPPET)),
            data: Some(Box::new(lsp::CompletionItemData {
                supplemental_file_index: self.completion_source_index(syntax.source)?,
                file_name: String::from_utf8_lossy(syntax.file.original_file_name()?.as_bytes())
                    .into_owned(),
                position: position as i32,
                name: label,
                source: "SwitchCases/".into(),
                ..Default::default()
            })),
            ..Default::default()
        }))
    }
}

// port: tsc/internal/ls/completions.go:typeNodeToExpression
fn type_node_to_expression(
    ast: &mut AstBuilder,
    node: NodeId,
    single_quote: bool,
) -> Result<Option<NodeId>> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let read = ast.view().node(node)?;
        let data = read.data_source();
        Ok(match read.kind().known() {
            Some(K::TypeReference) => data
                .as_type_reference_node()
                .and_then(|d| d.type_name())
                .map(|n| entity_expression(ast, n))
                .transpose()?,
            Some(K::TypeQuery) => data
                .as_type_query_node()
                .and_then(|d| d.expr_name())
                .map(|n| entity_expression(ast, n))
                .transpose()?,
            Some(K::IndexedAccessType) => {
                let d = data.as_indexed_access_type_node().unwrap();
                let object = d.object_type().ok_or(tsr_arena::Error::InvalidGraph)?;
                let index = d.index_type().ok_or(tsr_arena::Error::InvalidGraph)?;
                match (
                    type_node_to_expression(ast, object, single_quote)?,
                    type_node_to_expression(ast, index, single_quote)?,
                ) {
                    (Some(a), Some(b)) => {
                        Some(ast.new_element_access_expression(Some(a), None, Some(b), 0))
                    }
                    _ => None,
                }
            }
            Some(K::LiteralType) => {
                let literal = data
                    .as_literal_type_node()
                    .and_then(|d| d.literal())
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let read = ast.view().node(literal)?;
                let text = ast.view().node_text(literal)?.into_js_string();
                match read.kind().known() {
                    Some(K::StringLiteral) => Some(ast.new_string_literal(
                        text,
                        if single_quote {
                            tsr_ast::token_flags::SINGLE_QUOTE
                        } else {
                            0
                        },
                    )),
                    Some(K::NumericLiteral) => {
                        let flags = read
                            .data_source()
                            .as_numeric_literal()
                            .unwrap()
                            .token_flags();
                        Some(ast.new_numeric_literal(text, flags))
                    }
                    _ => None,
                }
            }
            Some(K::ParenthesizedType) => {
                let inner = data
                    .as_parenthesized_type_node()
                    .and_then(|d| d.r#type())
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                type_node_to_expression(ast, inner, single_quote)?.map(|expr| {
                    if ast
                        .view()
                        .node(expr)
                        .is_ok_and(|r| r.kind() == K::Identifier)
                    {
                        expr
                    } else {
                        ast.new_parenthesized_expression(Some(expr))
                    }
                })
            }
            Some(K::ImportType) => panic!("import type remained after auto-import conversion"),
            _ => None,
        })
    })
}
// port: tsc/internal/ls/completions.go:entityNameToExpression
fn entity_expression(ast: &mut AstBuilder, mut name: NodeId) -> Result<NodeId> {
    let mut rights = Vec::new();
    while ast.view().node(name)?.kind() != K::Identifier {
        let read = ast.view().node(name)?;
        let d = read.data_source();
        let d = d
            .as_qualified_name()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        rights.push(d.right());
        name = d.left().ok_or(tsr_arena::Error::InvalidGraph)?;
    }
    for right in rights.into_iter().rev() {
        name = ast.new_property_access_expression(Some(name), None, right, 0);
    }
    Ok(name)
}
