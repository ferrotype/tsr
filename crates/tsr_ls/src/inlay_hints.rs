use crate::{
    converters::Script, documentation::list, inlay_parts, syntax::Syntax, LanguageService, Result,
};
use tsr_ast::{span_map::FEATURE_INLAY_HINTS, utilities as ast, AstView, NodeId, SyntaxKind as K};
use tsr_checker::{Operation, SignatureRef, SymbolRef, TypePredicateRef, TypeRef};
use tsr_lsproto as lsp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParameterNameHints {
    #[default]
    None,
    Literals,
    All,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuotePreference {
    #[default]
    Auto,
    Single,
    Double,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlayHintsOptions {
    pub parameter_names: ParameterNameHints,
    pub parameter_names_when_matching: bool,
    pub parameter_types: bool,
    pub variable_types: bool,
    pub variable_types_when_matching: bool,
    pub property_types: bool,
    pub return_types: bool,
    pub enum_values: bool,
    pub quote: QuotePreference,
}
impl InlayHintsOptions {
    // port: tsc/internal/ls/inlay_hints.go:isAnyInlayHintEnabled
    pub fn enabled(self) -> bool {
        self.parameter_names != ParameterNameHints::None
            || self.parameter_types
            || self.variable_types
            || self.property_types
            || self.return_types
            || self.enum_values
    }
}
#[derive(Clone, Copy)]
enum HintType {
    Type(TypeRef),
    Predicate(TypePredicateRef),
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/inlay_hints.go:LanguageService.ProvideInlayHint
    pub fn inlay_hints(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::InlayHintParams,
        options: InlayHintsOptions,
    ) -> Result<lsp::InlayHintsOrNull> {
        if !options.enabled() {
            return Ok(lsp::InlayHintsOrNull::default());
        }
        let file = self.file(&params.text_document.uri)?;
        let single = single_quote(&Syntax::new(self.view(file)?, file)?, options.quote)?;
        let projections = self
            .converters
            .from_lsp_range_intersecting_for_source_file(
                self.program,
                file,
                &params.range,
                FEATURE_INLAY_HINTS,
            )?;
        let mut result = Vec::new();
        for projection in projections {
            let source = projection.script;
            let mut syntax = Syntax::new(self.view(source)?, source)?;
            let view = syntax.view;
            let span = projection.mapped.span;
            let mut stack = vec![source];
            while let Some(node) = stack.pop() {
                let n = view.node(node)?;
                if n.end() == n.pos() || n.flags() & tsr_ast::node_flags::REPARSED != 0 {
                    continue;
                }
                if matches!(
                    n.kind().known(),
                    Some(
                        K::ModuleDeclaration
                            | K::ClassDeclaration
                            | K::InterfaceDeclaration
                            | K::FunctionDeclaration
                            | K::ClassExpression
                            | K::FunctionExpression
                            | K::MethodDeclaration
                            | K::ArrowFunction
                    )
                ) {
                    self.check_canceled()?;
                }
                if !span.intersects(tsr_core::TextRange::new(
                    i64::from(n.pos()),
                    i64::from(n.end()),
                )) || ast::is_type_node(&n) && n.kind() != K::ExpressionWithTypeArguments
                {
                    continue;
                }
                if options.variable_types && n.kind() == K::VariableDeclaration
                    || options.property_types && n.kind() == K::PropertyDeclaration
                {
                    self.variable_hint(c, &mut syntax, node, options, single, &mut result)?;
                } else if options.enum_values && n.kind() == K::EnumMember {
                    if n.initializer().is_none() {
                        if let Some(value) = c.constant_value(node)? {
                            let text = match value {
                                tsr_printer::emit_resolver::ConstantValue::Number(n) => {
                                    n.to_string()
                                }
                                tsr_printer::emit_resolver::ConstantValue::String(s) => {
                                    String::from_utf8_lossy(s.as_bytes()).into_owned()
                                }
                            };
                            self.hint(
                                source,
                                n.end(),
                                lsp::InlayHint {
                                    label: lsp::StringOrInlayHintLabelParts {
                                        string: Some(Box::new(format!("= {text}"))),
                                        ..Default::default()
                                    },
                                    padding_left: Some(Box::new(true)),
                                    ..Default::default()
                                },
                                &mut result,
                            )?;
                        }
                    }
                } else if options.parameter_names != ParameterNameHints::None
                    && tsr_ast::utilities_middle::is_call_or_new_expression(&n)
                {
                    self.call_hints(c, &mut syntax, node, options, &mut result)?;
                } else {
                    if options.parameter_types
                        && ast::is_function_like_declaration(Some(&n))
                        && tsr_ast::utilities_containers::has_context_sensitive_parameters(
                            view, node,
                        )?
                    {
                        self.parameter_type_hints(c, &mut syntax, node, single, &mut result)?;
                    }
                    if options.return_types
                        && matches!(
                            n.kind().known(),
                            Some(
                                K::FunctionDeclaration
                                    | K::ArrowFunction
                                    | K::FunctionExpression
                                    | K::MethodDeclaration
                                    | K::GetAccessor
                            )
                        )
                    {
                        self.return_hint(c, &mut syntax, node, single, &mut result)?;
                    }
                }
                let mut children = syntax.children(node)?;
                children.reverse();
                stack.extend(children);
            }
        }
        Ok(lsp::InlayHintsOrNull {
            inlay_hints: Some(Box::new(result)),
        })
    }
    fn hint(
        &mut self,
        source: NodeId,
        position: i32,
        mut hint: lsp::InlayHint,
        out: &mut Vec<Option<Box<lsp::InlayHint>>>,
    ) -> Result<()> {
        let file = self.source(source)?;
        let original = file.original_file_name()?;
        let script = Script {
            file_name: file.file_name(),
            text: file.text().as_bytes(),
            original_file_name: original.as_bytes(),
            original_text: file.original_text(),
            span_map: file.span_map(),
        };
        let (position, fidelity) =
            self.converters
                .to_lsp_position_for_feature(&script, position, FEATURE_INLAY_HINTS);
        if !fidelity.is_none() {
            hint.position = position;
            out.push(Some(Box::new(hint)));
        }
        Ok(())
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.getNodeDisplayPart
    fn node_hint_part(&mut self, text: String, node: NodeId) -> Result<lsp::InlayHintLabelPart> {
        let view = self.view(node)?;
        let source = ast::get_source_file_of_node(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let mut syntax = Syntax::new(view, source)?;
        let (range, fidelity) = self.range(
            source,
            tsr_core::TextRange::new(syntax.start(node)?, i64::from(view.node(node)?.end())),
            FEATURE_INLAY_HINTS,
        )?;
        Ok(lsp::InlayHintLabelPart {
            value: text,
            location: fidelity.is_single_segment().then(|| {
                Box::new(lsp::Location {
                    uri: lsp::DocumentUri::from_file_name(
                        syntax
                            .file
                            .original_file_name()
                            .expect("validated source file")
                            .as_bytes(),
                    ),
                    range,
                })
            }),
            ..Default::default()
        })
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.typeToInlayHintParts
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.typePredicateToInlayHintParts
    fn type_hint_parts(
        &mut self,
        c: &mut Operation<'_>,
        ty: HintType,
        single: bool,
    ) -> Result<Vec<Option<Box<lsp::InlayHintLabelPart>>>> {
        let flags = tsr_nodebuilder::flags::IGNORE_ERRORS
            | tsr_nodebuilder::flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
            | tsr_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
        let mut builder = c.node_builder();
        let node = match ty {
            HintType::Type(ty) => builder.type_to_type_node(ty, None, flags, 0)?,
            HintType::Predicate(predicate) => builder.type_predicate_to_type_predicate_node(
                predicate,
                tsr_checker::BuilderRequest {
                    enclosing: None,
                    flags,
                    internal_flags: 0,
                },
            )?,
        }
        .ok_or(tsr_arena::Error::InvalidGraph)?;
        let symbols = builder.identifier_symbols().collect();
        let parts = inlay_parts::render(builder.view(), node, &symbols, single)?;
        drop(builder);
        let mut result = Vec::with_capacity(parts.len());
        for part in parts {
            let target = if let Some(id) = part.symbol {
                let symbol = c.symbol_ref(id)?;
                let declarations = c.symbol_declarations(symbol)?;
                let first = declarations.iter().flatten().next();
                match first {
                    Some(decl) => tsr_ast::get_name_of_declaration(self.view(decl)?, Some(decl))?,
                    None => None,
                }
            } else {
                None
            };
            let part = if let Some(target) = target {
                self.node_hint_part(part.value, target)?
            } else {
                lsp::InlayHintLabelPart {
                    value: part.value,
                    ..Default::default()
                }
            };
            result.push(Some(Box::new(part)));
        }
        Ok(result)
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.addTypeHints
    fn type_hint(
        &mut self,
        source: NodeId,
        position: i32,
        mut parts: Vec<Option<Box<lsp::InlayHintLabelPart>>>,
        out: &mut Vec<Option<Box<lsp::InlayHint>>>,
    ) -> Result<()> {
        parts.insert(
            0,
            Some(Box::new(lsp::InlayHintLabelPart {
                value: ": ".into(),
                ..Default::default()
            })),
        );
        self.hint(
            source,
            position,
            lsp::InlayHint {
                label: lsp::StringOrInlayHintLabelParts {
                    inlay_hint_label_parts: Some(Box::new(parts)),
                    ..Default::default()
                },
                kind: Some(Box::new(lsp::InlayHintKind::TYPE)),
                padding_left: Some(Box::new(true)),
                ..Default::default()
            },
            out,
        )
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.visitVariableLikeDeclaration
    fn variable_hint(
        &mut self,
        c: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        options: InlayHintsOptions,
        single: bool,
        out: &mut Vec<Option<Box<lsp::InlayHint>>>,
    ) -> Result<()> {
        let n = syntax.view.node(node)?;
        let Some(name) = n.name() else { return Ok(()) };
        if n.initializer().is_none() {
            if n.kind() != K::PropertyDeclaration {
                return Ok(());
            }
            let ty = c.get_type_at_location(node)?;
            if c.type_flags(ty)? & tsr_checker::type_flags::ANY != 0 {
                return Ok(());
            }
        }
        if ast::is_binding_pattern(&syntax.view.node(name)?)
            || n.kind() == K::VariableDeclaration && !hintable_declaration(syntax.view, node)?
        {
            return Ok(());
        }
        if n.type_node().is_some() {
            return Ok(());
        }
        let ty = c.get_type_at_location(node)?;
        if module_type(c, ty)? {
            return Ok(());
        }
        let parts = self.type_hint_parts(c, HintType::Type(ty), single)?;
        if !options.variable_types_when_matching
            && syntax.view.node(name)?.kind() != K::ComputedPropertyName
        {
            let text: String = parts.iter().flatten().map(|p| p.value.as_str()).collect();
            if tsr_jsstring::equal_fold(syntax.view.node_text(name)?.as_bytes(), text.as_bytes()) {
                return Ok(());
            }
        }
        self.type_hint(syntax.source, syntax.view.node(name)?.end(), parts, out)
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.visitFunctionDeclarationLikeForReturnType
    fn return_hint(
        &mut self,
        c: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        single: bool,
        out: &mut Vec<Option<Box<lsp::InlayHint>>>,
    ) -> Result<()> {
        let n = syntax.view.node(node)?;
        if n.kind() == K::ArrowFunction
            && syntax
                .nav()
                .find_child_of_kind(node, K::OpenParenToken)?
                .is_none()
            || n.type_node().is_some()
            || n.body().is_none()
        {
            return Ok(());
        }
        let signature = c.get_signature_from_declaration(node)?;
        let predicate = c.get_type_predicate_of_signature(signature)?;
        let ty = if let Some(predicate) =
            predicate.filter(|p| c.type_predicate_parts(*p).is_ok_and(|p| p.r#type.is_some()))
        {
            HintType::Predicate(predicate)
        } else {
            let ty = c.get_return_type_of_signature(signature)?;
            if module_type(c, ty)? {
                return Ok(());
            }
            HintType::Type(ty)
        };
        let parts = self.type_hint_parts(c, ty, single)?;
        let position =
            if let Some(close) = syntax.nav().find_child_of_kind(node, K::CloseParenToken)? {
                syntax.view.node(close)?.end()
            } else {
                n.parameter_list()
                    .map(|l| syntax.view.list(l).map(|l| l.loc().end() as i32))
                    .transpose()?
                    .ok_or(tsr_arena::Error::InvalidGraph)?
            };
        self.type_hint(syntax.source, position, parts, out)
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.visitFunctionLikeForParameterType
    fn parameter_type_hints(
        &mut self,
        c: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        single: bool,
        out: &mut Vec<Option<Box<lsp::InlayHint>>>,
    ) -> Result<()> {
        let signature = c.get_signature_from_declaration(node)?;
        let parameters = c.signature_parameters(signature)?;
        let mut pos = 0;
        for parameter in list(syntax.view, syntax.view.node(node)?.parameter_list())? {
            let n = syntax.view.node(parameter)?;
            let is_this = tsr_ast::utilities_class::is_this_parameter(syntax.view, parameter)?;
            if hintable_declaration(syntax.view, parameter)? {
                let symbol = if is_this {
                    c.signature_this_parameter(signature)?
                } else {
                    parameters.get(pos).copied()
                };
                if n.type_node().is_none() {
                    if let Some(symbol) = symbol {
                        if let Some(decl) = c.symbol(symbol)?.value_declaration() {
                            if c.node(decl)?.kind() == K::Parameter {
                                let ty = c.get_type_of_symbol_at_location(symbol, Some(decl))?;
                                if !module_type(c, ty)? {
                                    let parts =
                                        self.type_hint_parts(c, HintType::Type(ty), single)?;
                                    let end = n
                                        .question_token(syntax.view)?
                                        .or(n.name())
                                        .ok_or(tsr_arena::Error::InvalidGraph)?;
                                    self.type_hint(
                                        syntax.source,
                                        syntax.view.node(end)?.end(),
                                        parts,
                                        out,
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
            if !is_this {
                pos += 1;
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/inlay_hints.go:inlayHintState.visitCallOrNewExpression
    fn call_hints(
        &mut self,
        c: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        options: InlayHintsOptions,
        out: &mut Vec<Option<Box<lsp::InlayHint>>>,
    ) -> Result<()> {
        let args = list(syntax.view, syntax.view.node(node)?.argument_list())?;
        if args.is_empty() {
            return Ok(());
        }
        let signature = c.get_resolved_signature(node)?;
        let mut pos = 0;
        for original in args {
            let arg = tsr_ast::skip_parentheses(syntax.view, original)?;
            let n = syntax.view.node(arg)?;
            if options.parameter_names == ParameterNameHints::Literals
                && !hintable_literal(syntax.view, arg)?
            {
                pos += 1;
                continue;
            }
            let mut spread = 0;
            if n.kind() == K::SpreadElement {
                let expression = n.expression().ok_or(tsr_arena::Error::InvalidGraph)?;
                let ty = c.get_type_at_location(expression)?;
                if let Some((fixed, flags)) = c.tuple_element_flags(ty)? {
                    if fixed == 0 {
                        continue;
                    }
                    spread = flags
                        .iter()
                        .position(|f| f & tsr_checker::element_flags::REQUIRED == 0)
                        .unwrap_or(fixed as usize);
                }
            }
            let info = parameter_at(c, signature, pos)?;
            pos += spread.max(1);
            let Some((id, name, rest)) = info else {
                return Ok(());
            };
            let same = if n.kind() == K::Identifier {
                syntax.view.node_text(arg)?.as_bytes() == name.as_bytes()
            } else if n.kind() == K::PropertyAccessExpression {
                n.name()
                    .map(|id| {
                        syntax
                            .view
                            .node_text(id)
                            .map(|t| t.as_bytes() == name.as_bytes())
                    })
                    .transpose()?
                    .unwrap_or(false)
            } else {
                false
            };
            if !options.parameter_names_when_matching && same && !rest {
                continue;
            }
            if leading_parameter_comment(syntax, arg, &name)? {
                continue;
            }
            let part = self.node_hint_part(if rest { format!("...{name}") } else { name }, id)?;
            let parts = vec![
                Some(Box::new(part)),
                Some(Box::new(lsp::InlayHintLabelPart {
                    value: ":".into(),
                    ..Default::default()
                })),
            ];
            let position = syntax.start(original)? as i32;
            self.hint(
                syntax.source,
                position,
                lsp::InlayHint {
                    label: lsp::StringOrInlayHintLabelParts {
                        inlay_hint_label_parts: Some(Box::new(parts)),
                        ..Default::default()
                    },
                    kind: Some(Box::new(lsp::InlayHintKind::PARAMETER)),
                    padding_right: Some(Box::new(true)),
                    ..Default::default()
                },
                out,
            )?;
        }
        Ok(())
    }
}
// port: tsc/internal/ls/lsutil/utilities.go:GetQuotePreference
fn single_quote(syntax: &Syntax<'_>, preference: QuotePreference) -> Result<bool> {
    match preference {
        QuotePreference::Single => return Ok(true),
        QuotePreference::Double => return Ok(false),
        QuotePreference::Auto => {}
    }
    for &id in syntax.file.imports()?.iter().flatten() {
        let n = syntax.view.node(id)?;
        if n.kind() == K::StringLiteral
            && n.parent()
                .is_some_and(|p| syntax.view.node(p).is_ok_and(|p| p.pos() >= 0))
        {
            return Ok(n.data_source().as_string_literal().unwrap().token_flags()
                & tsr_ast::token_flags::SINGLE_QUOTE
                != 0);
        }
    }
    Ok(false)
}
// port: tsc/internal/ls/inlay_hints.go:isModuleReferenceType
fn module_type(c: &Operation<'_>, ty: TypeRef) -> Result<bool> {
    Ok(c.type_symbol(ty)?
        .map(|s| {
            c.symbol_ref(s).and_then(|s| {
                c.symbol(s)
                    .map(|s| s.flags() & tsr_ast::symbol_flags::MODULE != 0)
            })
        })
        .transpose()?
        .unwrap_or(false))
}
// port: tsc/internal/ls/inlay_hints.go:isHintableLiteral
fn hintable_literal(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    Ok(match n.kind().known() {
        Some(K::PrefixUnaryExpression) => {
            let operand = n
                .data_source()
                .as_prefix_unary_expression()
                .unwrap()
                .operand()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let r = view.node(operand)?;
            ast::is_literal_expression(&r)
                || r.kind() == K::Identifier
                    && matches!(view.node_text(operand)?.as_bytes(), b"Infinity" | b"NaN")
        }
        Some(
            K::TrueKeyword
            | K::FalseKeyword
            | K::NullKeyword
            | K::NoSubstitutionTemplateLiteral
            | K::TemplateExpression,
        ) => true,
        Some(K::Identifier) => matches!(
            view.node_text(node)?.as_bytes(),
            b"undefined" | b"Infinity" | b"NaN"
        ),
        _ => ast::is_literal_expression(&n),
    })
}
// port: tsc/internal/ls/inlay_hints.go:isHintableDeclaration
fn hintable_declaration(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    if (ast::is_part_of_parameter_declaration(view, node)?
        || n.kind() == K::VariableDeclaration && ast::is_var_const(view, node)?)
        && n.initializer().is_some()
    {
        let initial = tsr_ast::skip_parentheses(view, n.initializer().unwrap())?;
        let r = view.node(initial)?;
        return Ok(!(hintable_literal(view, initial)?
            || matches!(
                r.kind().known(),
                Some(K::NewExpression | K::ObjectLiteralExpression)
            )
            || ast::is_assertion_expression(&r)));
    }
    Ok(true)
}
// port: tsc/internal/ls/inlay_hints.go:getParameterDeclarationIdentifier
fn parameter_identifier(c: &Operation<'_>, s: SymbolRef) -> Result<Option<NodeId>> {
    let Some(decl) = c.symbol(s)?.value_declaration() else {
        return Ok(None);
    };
    let n = c.node(decl)?;
    if n.kind() != K::Parameter {
        return Ok(None);
    }
    Ok(n.name()
        .filter(|id| c.node(*id).is_ok_and(|n| n.kind() == K::Identifier)))
}
// port: tsc/internal/ls/inlay_hints.go:inlayHintState.getParameterIdentifierInfoAtPosition
fn parameter_at(
    c: &mut Operation<'_>,
    s: SignatureRef,
    pos: usize,
) -> Result<Option<(NodeId, String, bool)>> {
    let parameters = c.signature_parameters(s)?;
    let rest = c.signature_flags(s)? & tsr_checker::signature_flags::HAS_REST_PARAMETER != 0;
    let count = parameters.len() - usize::from(rest);
    if pos < count {
        return parameter_identifier(c, parameters[pos])?
            .map(|id| {
                Ok((
                    id,
                    String::from_utf8_lossy(
                        c.node(id)?.data_source().as_identifier().unwrap().text(),
                    )
                    .into_owned(),
                    false,
                ))
            })
            .transpose();
    }
    let Some(&rest_parameter) = parameters.get(count) else {
        return Ok(None);
    };
    let Some(rest_id) = parameter_identifier(c, rest_parameter)? else {
        return Ok(None);
    };
    let ty = c.get_type_of_symbol(rest_parameter)?;
    if c.is_tuple_type(ty)? {
        let elements = c.tuple_elements(ty)?;
        if let Some(decl) = elements
            .get(pos - count)
            .and_then(|e| e.labeled_declaration)
        {
            let n = c.node(decl)?;
            let id = n.name().ok_or(tsr_arena::Error::InvalidGraph)?;
            let rest = if let Some(d) = n.data_source().as_named_tuple_member() {
                d.dot_dot_dot_token().is_some()
            } else {
                n.data_source()
                    .as_parameter_declaration()
                    .ok_or(tsr_arena::Error::InvalidGraph)?
                    .dot_dot_dot_token()
                    .is_some()
            };
            return Ok(Some((
                id,
                String::from_utf8_lossy(c.node(id)?.data_source().as_identifier().unwrap().text())
                    .into_owned(),
                rest,
            )));
        }
        return Ok(None);
    }
    Ok((pos == count).then(|| {
        (
            rest_id,
            String::from_utf8_lossy(
                c.symbol(rest_parameter)
                    .expect("validated parameter")
                    .name_bytes(),
            )
            .into_owned(),
            true,
        )
    }))
}
// port: tsc/internal/ls/inlay_hints.go:inlayHintState.leadingCommentsContainsParameterName
fn leading_parameter_comment(syntax: &Syntax<'_>, node: NodeId, name: &str) -> Result<bool> {
    if !tsr_scanner::is_identifier_text(name.as_bytes(), syntax.file.language_variant) {
        return Ok(false);
    }
    if syntax.view.node(node)?.kind() == K::JsxText {
        return Ok(false);
    }
    for range in tsr_scanner::get_leading_comment_ranges(
        syntax.file.text().as_bytes(),
        i64::from(syntax.view.node(node)?.pos()),
    ) {
        let text =
            &syntax.file.text().as_bytes()[range.loc.pos() as usize..range.loc.end() as usize];
        // Rust and Go's Unicode whitespace sets agree here except U+001C..1F,
        // which Go does not classify as space (Rust doesn't either).
        if String::from_utf8_lossy(text)
            .trim_matches(|c: char| c.is_whitespace() || matches!(c, '/' | '*'))
            == name
        {
            return Ok(true);
        }
    }
    Ok(false)
}
