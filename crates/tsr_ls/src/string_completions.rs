use crate::{
    completion_context::{Context, ALL},
    completion_items,
    completions::{Candidate, CompletionOptions},
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{
    span_map::FEATURE_COMPLETION, utilities as ast, utilities_middle as middle, NodeId,
    SyntaxKind as K,
};
use tsr_checker::{context_flags as cf, type_flags as tf, Operation, SymbolRef, TypeRef};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

// port: tsc/internal/ls/utilities.go:IsInString
pub(crate) fn in_string(
    syntax: &mut Syntax<'_>,
    token: Option<NodeId>,
    position: i64,
) -> Result<Option<NodeId>> {
    let Some(token) = token else {
        return Ok(None);
    };
    let read = syntax.view.node(token)?;
    if !ast::is_string_literal_like(&read) {
        return Ok(None);
    }
    Ok((syntax.start(token)? < position
        && (position < i64::from(read.end())
            || middle::is_unterminated_literal(&read) && position == i64::from(read.end())))
    .then_some(token))
}

fn string_types(
    checker: &mut Operation<'_>,
    ty: TypeRef,
    seen: &mut HashSet<Vec<u8>>,
    result: &mut Vec<String>,
) -> Result<()> {
    let mut pending = vec![ty];
    let mut visited = HashSet::new();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        let flags = checker.type_flags(ty)?;
        if flags & tf::TYPE_PARAMETER != 0 {
            if let Some(constraint) = checker.get_constraint_of_type_parameter(ty)? {
                pending.push(constraint);
            }
        } else if flags & tf::UNION != 0 {
            pending.extend(checker.constituents(ty)?.into_iter().rev());
        } else if flags & tf::STRING_LITERAL != 0 && flags & tf::ENUM_LITERAL == 0 {
            let value = checker.string_literal_value(ty)?;
            if seen.insert(value.as_bytes().to_vec()) {
                result.push(String::from_utf8_lossy(value.as_bytes()).into_owned());
            }
        }
    }
    Ok(())
}
impl LanguageService<'_> {
    pub(crate) fn string_completion_symbols(
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        literal: NodeId,
    ) -> Result<Vec<SymbolRef>> {
        let view = syntax.view;
        let Some(parent) = view.node(literal)?.parent() else {
            return Ok(Vec::new());
        };
        let pr = view.node(parent)?;
        let mut ty = None;
        match pr.kind().known() {
            Some(K::ElementAccessExpression) => {
                if let Some(expression) = pr.expression() {
                    ty = Some(checker.get_type_at_location(expression)?);
                }
            }
            Some(K::PropertyAssignment) if pr.name() == Some(literal) => {
                if let Some(object) = pr.parent() {
                    ty = checker.get_contextual_type(object, cf::NONE)?;
                }
            }
            Some(K::BinaryExpression) => {
                let data = pr.data_source().as_binary_expression().unwrap();
                if data
                    .operator_token()
                    .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::InKeyword))
                {
                    if let Some(right) = data.right() {
                        ty = Some(checker.get_type_at_location(right)?);
                    }
                }
            }
            Some(K::LiteralType) => {
                if let Some(grandparent) = pr.parent() {
                    if let Some(data) = view
                        .node(grandparent)?
                        .data_source()
                        .as_indexed_access_type_node()
                    {
                        if let Some(object) = data.object_type() {
                            ty = Some(checker.get_type_from_type_node(object)?);
                        }
                    }
                }
            }
            _ => {}
        }
        ty.map_or_else(
            || Ok(Vec::new()),
            |ty| crate::completions::properties(checker, ty),
        )
    }
    #[allow(
        clippy::if_not_else,
        reason = "Keep property completions before type literals, matching the pinned conversion dispatch"
    )]
    pub(crate) fn string_completions(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        literal: NodeId,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionList>> {
        if crate::completion_paths::module_literal(syntax, literal)? {
            return self
                .module_path_completions(checker, syntax, literal, position, options)
                .map(Some);
        }
        let view = syntax.view;
        let symbols = Self::string_completion_symbols(checker, syntax, literal)?;
        let from_properties = !symbols.is_empty();
        let read = view.node(literal)?;
        let mut list = lsp::CompletionList::default();
        let mut end = i64::from(read.end()) - 1;
        let start = syntax.start(literal)?;
        let unterminated = middle::is_unterminated_literal(&read);
        let replacement = if unterminated && start == end {
            None
        } else {
            if unterminated {
                end = position.min(i64::from(read.end()));
            }
            let (range, fidelity) = self.range(
                syntax.source,
                TextRange::new(start + 1, end),
                FEATURE_COMPLETION,
            )?;
            fidelity.is_exact().then_some(range)
        };
        if !symbols.is_empty() {
            let context = Context {
                location: syntax.source,
                token: Some(literal),
                previous: Some(literal),
                member: None,
                container: None,
                type_only: false,
                filter: crate::completion_keywords::Filter::None,
                new_identifier: false,
                commit: ALL,
            };
            for symbol in symbols {
                if let Some(mut item) = self.completion_symbol_item(
                    checker,
                    syntax,
                    &context,
                    &Candidate {
                        symbol,
                        sort: "11",
                        nullable: false,
                        this_member: false,
                        promise: false,
                    },
                    position,
                    options,
                )? {
                    if let Some(range) = &replacement {
                        item.text_edit = Some(Box::new(lsp::TextEditOrInsertReplaceEdit {
                            text_edit: Some(Box::new(lsp::TextEdit {
                                range: range.clone(),
                                new_text: item.label.clone(),
                            })),
                            ..Default::default()
                        }));
                    }
                    list.items.push(Some(Box::new(item)));
                }
            }
        } else {
            let mut types = Vec::new();
            let mut seen = HashSet::new();
            if let Some(parent) = read.parent() {
                let pr = view.node(parent)?;
                if matches!(
                    pr.kind().known(),
                    Some(K::CallExpression | K::NewExpression)
                ) {
                    let args: Vec<_> = view
                        .node_slice(pr.arguments(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    if let Some(index) = args.iter().position(|&n| n == literal) {
                        for sig in checker.get_candidate_signatures_for_string_literal_completions(
                            parent, literal,
                        )? {
                            let ty = checker.get_type_parameter_at_position(sig, index)?;
                            string_types(checker, ty, &mut seen, &mut types)?;
                        }
                    }
                }
                if pr.kind() == K::LiteralType {
                    if let Some(ty) = checker.get_type_argument_constraint(parent)? {
                        string_types(checker, ty, &mut seen, &mut types)?;
                    }
                }
            }
            for flags in [cf::IGNORE_NODE_INFERENCES, cf::NONE] {
                if let Some(ty) = syntax.contextual_type_from_parent(literal, checker, flags)? {
                    string_types(checker, ty, &mut seen, &mut types)?;
                }
            }
            if types.is_empty() {
                return Ok(None);
            }
            // The pin tests the decoded literal text, rather than the source's opening quote.
            let quote = if read.kind() == K::NoSubstitutionTemplateLiteral {
                tsr_jsstring::QuoteChar::Backtick
            } else if view.node_text(literal)?.as_bytes().starts_with(b"'") {
                tsr_jsstring::QuoteChar::Single
            } else {
                tsr_jsstring::QuoteChar::Double
            };
            for value in types {
                let name = String::from_utf8_lossy(&tsr_jsstring::escape::escape_string(
                    value.as_bytes(),
                    quote,
                ))
                .into_owned();
                list.items.push(Some(Box::new(lsp::CompletionItem {
                    label: name.clone(),
                    kind: Some(Box::new(lsp::CompletionItemKind::CONSTANT)),
                    sort_text: Some(Box::new("11".into())),
                    text_edit: replacement.as_ref().map(|range| {
                        Box::new(lsp::TextEditOrInsertReplaceEdit {
                            text_edit: Some(Box::new(lsp::TextEdit {
                                range: range.clone(),
                                new_text: name,
                            })),
                            ..Default::default()
                        })
                    }),
                    ..Default::default()
                })));
            }
        }
        let (cursor, _) = self.range(
            syntax.source,
            TextRange::new(position, position),
            FEATURE_COMPLETION,
        )?;
        completion_items::defaults(
            &mut list,
            options,
            &cursor.start,
            if from_properties { replacement } else { None },
            ALL,
        );
        self.completion_data(syntax.source, position, &mut list)?;
        Ok(Some(list))
    }
}
