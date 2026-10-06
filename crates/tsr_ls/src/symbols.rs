use crate::{syntax::Syntax, LanguageService, Result};
use std::collections::{HashMap, HashSet};
use tsr_ast::{
    modifier_flags, node_flags, source_file_tables, span_map::FEATURE_DOCUMENT_SYMBOLS, utilities,
    AstView, JSDeclarationKind as J, JsDocProvider, NodeId, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

type Symbols = Vec<Option<Box<lsp::DocumentSymbol>>>;
const MAX_LENGTH: usize = 150;

// port: tsc/internal/ls/symbols.go:getSymbolKindFromNode
pub(crate) fn symbol_kind(view: AstView<'_>, node: NodeId) -> Result<lsp::SymbolKind> {
    use lsp::SymbolKind as S;
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(K::SourceFile) => {
            if utilities::is_external_module(&*view.source_file(node)?) {
                S::MODULE
            } else {
                S::FILE
            }
        }
        Some(K::ModuleDeclaration) => S::NAMESPACE,
        Some(
            K::ClassDeclaration
            | K::ClassExpression
            | K::TypeAliasDeclaration
            | K::JSDocTypedefTag
            | K::JSDocCallbackTag,
        ) => S::CLASS,
        Some(K::InterfaceDeclaration) => S::INTERFACE,
        Some(K::EnumDeclaration) => S::ENUM,
        Some(K::ArrowFunction | K::FunctionDeclaration | K::FunctionExpression) => S::FUNCTION,
        Some(
            K::GetAccessor
            | K::SetAccessor
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::PropertyAssignment
            | K::ShorthandPropertyAssignment
            | K::SpreadAssignment
            | K::IndexSignature
            | K::StringLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::NumericLiteral,
        ) => S::PROPERTY,
        Some(K::MethodDeclaration | K::MethodSignature | K::CallSignature) => S::METHOD,
        Some(K::ConstructSignature | K::Constructor | K::ClassStaticBlockDeclaration) => {
            S::CONSTRUCTOR
        }
        Some(K::TypeParameter) => S::TYPE_PARAMETER,
        Some(K::EnumMember) => S::ENUM_MEMBER,
        Some(K::Parameter)
            if utilities::has_syntactic_modifier(
                view,
                node,
                modifier_flags::PARAMETER_PROPERTY_MODIFIER,
            )? =>
        {
            S::PROPERTY
        }
        Some(K::BinaryExpression | K::CallExpression)
            if matches!(
                tsr_ast::get_assignment_declaration_kind(view, node)?,
                J::ThisProperty | J::Property | J::ObjectDefinePropertyValue
            ) =>
        {
            S::PROPERTY
        }
        _ => S::VARIABLE,
    })
}

// port: tsc/internal/ls/symbols.go:getTextOfName
fn text_of_name(syntax: &Syntax<'_>, mut name: NodeId) -> Result<String> {
    let view = syntax.view;
    let read = view.node(name)?;
    if read.kind() == K::ComputedPropertyName {
        if let Some(expression) = read.expression() {
            if utilities::is_string_or_numeric_literal_like(&view.node(expression)?) {
                name = expression;
            }
        }
    }
    let kind = view.node(name)?.kind();
    let bytes = if matches!(
        kind.known(),
        Some(K::Identifier | K::PrivateIdentifier | K::NumericLiteral)
    ) {
        view.node_text(name)?.as_bytes().to_vec()
    } else if matches!(
        kind.known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
    ) {
        let (quote, character) = if kind == K::StringLiteral {
            (tsr_jsstring::QuoteChar::Double, b'"')
        } else {
            (tsr_jsstring::QuoteChar::Backtick, b'`')
        };
        let mut bytes = vec![character];
        bytes.extend(tsr_jsstring::escape::escape_string(
            view.node_text(name)?.as_bytes(),
            quote,
        ));
        bytes.push(character);
        bytes
    } else {
        tsr_scanner::get_text_of_node(view, name)?
            .as_bytes()
            .to_vec()
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
// port: tsc/internal/ls/symbols.go:isAnonymousName
fn anonymous(name: &str) -> bool {
    matches!(
        name,
        "<function>" | "<class>" | "export=" | "default" | "constructor" | "()" | "new()" | "[]"
    ) || name.ends_with(") callback")
}
fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_LENGTH) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.into(),
    }
}
// port: tsc/internal/ls/symbols.go:cleanCallbackText
fn callback_text(text: &str) -> String {
    truncate(text).replace(['\r', '\n', '\u{2028}', '\u{2029}'], "")
}
// port: tsc/internal/ls/symbols.go:getCallExpressionName
fn call_name(view: AstView<'_>, mut node: NodeId) -> Result<String> {
    let mut names = Vec::new();
    loop {
        let read = view.node(node)?;
        match read.kind().known() {
            Some(K::Identifier | K::PrivateIdentifier) => {
                names.push(String::from_utf8_lossy(view.node_text(node)?.as_bytes()).into_owned());
                break;
            }
            Some(K::PropertyAccessExpression) => {
                names.push(
                    String::from_utf8_lossy(
                        view.node_text(read.name().ok_or(tsr_arena::Error::InvalidGraph)?)?
                            .as_bytes(),
                    )
                    .into_owned(),
                );
                node = read.expression().ok_or(tsr_arena::Error::InvalidGraph)?;
            }
            _ => break,
        }
    }
    names.reverse();
    Ok(names.join("."))
}
// port: tsc/internal/ls/symbols.go:getUnnamedNodeLabel
fn unnamed(syntax: &Syntax<'_>, node: NodeId) -> Result<String> {
    let view = syntax.view;
    let read = view.node(node)?;
    let mut parent = read.parent();
    while let Some(id) = parent.filter(|id| {
        view.node(*id)
            .is_ok_and(|n| n.kind() == K::ParenthesizedExpression)
    }) {
        parent = view.node(id)?.parent();
    }
    if let Some(parent) = parent {
        if let Some(export) = view.node(parent)?.data_source().as_export_assignment() {
            return Ok(if export.is_export_equals() {
                "export="
            } else {
                "default"
            }
            .into());
        }
    }
    Ok(match read.kind().known() {
        Some(K::FunctionDeclaration | K::FunctionExpression | K::ArrowFunction) => {
            if read.modifier_flags(view)? & modifier_flags::DEFAULT != 0 {
                "default".into()
            } else if let Some(parent) = read
                .parent()
                .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::CallExpression))
            {
                let call = view.node(parent)?;
                let name = callback_text(&call_name(
                    view,
                    call.expression().ok_or(tsr_arena::Error::InvalidGraph)?,
                )?);
                if name.is_empty() {
                    "<function>".into()
                } else if name.len() > MAX_LENGTH {
                    format!("{name} callback")
                } else {
                    // port: tsc/internal/ls/symbols.go:getCallExpressionLiteralArgs
                    let mut args = Vec::new();
                    for arg in view.node_slice(call.arguments(view)?)?.iter().flatten() {
                        let read = view.node(arg)?;
                        if utilities::is_string_literal_like(&read)
                            || read.kind() == K::TemplateExpression
                        {
                            args.push(
                                String::from_utf8_lossy(
                                    tsr_scanner::get_text_of_node(view, arg)?.as_bytes(),
                                )
                                .into_owned(),
                            );
                        }
                    }
                    format!("{name}({}) callback", callback_text(&args.join(", ")))
                }
            } else {
                "<function>".into()
            }
        }
        Some(K::ClassDeclaration | K::ClassExpression) => {
            if read.modifier_flags(view)? & modifier_flags::DEFAULT != 0 {
                "default"
            } else {
                "<class>"
            }
            .into()
        }
        Some(K::Constructor) => "constructor".into(),
        Some(K::CallSignature) => "()".into(),
        Some(K::ConstructSignature) => "new()".into(),
        Some(K::IndexSignature) => "[]".into(),
        _ => String::new(),
    })
}
// port: tsc/internal/ls/symbols.go:getInteriorModule
fn interior_module(view: AstView<'_>, mut node: NodeId) -> Result<NodeId> {
    while let Some(body) = view.node(node)?.body().filter(|id| {
        view.node(*id)
            .is_ok_and(|n| n.kind() == K::ModuleDeclaration)
    }) {
        node = body;
    }
    Ok(node)
}
// port: tsc/internal/ls/symbols.go:getModuleName
fn module_name(view: AstView<'_>, mut node: NodeId) -> Result<String> {
    let mut names = Vec::new();
    loop {
        names.push(
            String::from_utf8_lossy(
                view.node_text(
                    view.node(node)?
                        .name()
                        .ok_or(tsr_arena::Error::InvalidGraph)?,
                )?
                .as_bytes(),
            )
            .into_owned(),
        );
        let Some(body) = view.node(node)?.body().filter(|id| {
            view.node(*id)
                .is_ok_and(|n| n.kind() == K::ModuleDeclaration)
        }) else {
            break;
        };
        node = body;
    }
    Ok(names.join("."))
}
// port: tsc/internal/ls/symbols.go:flattenDocumentSymbols
fn flatten(
    symbols: &[Option<Box<lsp::DocumentSymbol>>],
    uri: &lsp::DocumentUri,
) -> Vec<Option<Box<lsp::SymbolInformation>>> {
    let mut result = Vec::new();
    let mut stack: Vec<_> = symbols
        .iter()
        .flatten()
        .rev()
        .map(|s| (s.as_ref(), None))
        .collect();
    while let Some((symbol, container)) = stack.pop() {
        result.push(Some(Box::new(lsp::SymbolInformation {
            name: symbol.name.clone(),
            kind: symbol.kind,
            location: lsp::Location {
                uri: uri.clone(),
                range: symbol.range.clone(),
            },
            container_name: container,
            tags: symbol.tags.clone(),
            deprecated: symbol.deprecated.clone(),
        })));
        if let Some(children) = symbol.children.as_deref() {
            stack.extend(
                children
                    .iter()
                    .flatten()
                    .rev()
                    .map(|s| (s.as_ref(), Some(Box::new(symbol.name.clone())))),
            );
        }
    }
    result
}
// port: tsc/internal/ls/symbols.go:mergeExpandos
fn merge_expandos(mut symbols: Symbols) -> Symbols {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
        let mut targets: HashMap<String, Vec<usize>> = HashMap::new();
        let mut namespaces = HashMap::new();
        for (index, symbol) in symbols
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i, s)))
        {
            if anonymous(&symbol.name) {
                continue;
            }
            if matches!(
                symbol.kind,
                lsp::SymbolKind::CLASS | lsp::SymbolKind::FUNCTION | lsp::SymbolKind::VARIABLE
            ) {
                targets.entry(symbol.name.clone()).or_default().push(index);
            }
            if symbol.kind == lsp::SymbolKind::NAMESPACE {
                namespaces.entry(symbol.name.clone()).or_insert(index);
            }
        }
        for index in 0..symbols.len() {
            let Some(mut symbol) = symbols[index].take() else {
                continue;
            };
            if let Some(children) = symbol.children.take() {
                symbol.children = Some(Box::new(merge_expandos(*children)));
            }
            let mut merged = false;
            if !anonymous(&symbol.name) {
                let dest = if symbol.kind == lsp::SymbolKind::PROPERTY {
                    targets.get(&symbol.name).cloned().unwrap_or_default()
                } else if symbol.kind == lsp::SymbolKind::NAMESPACE {
                    namespaces
                        .get(&symbol.name)
                        .copied()
                        .filter(|&i| i != index)
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                };
                for dest in dest.into_iter().rev() {
                    if let Some(target) = symbols[dest].as_mut() {
                        // port: tsc/internal/ls/symbols.go:mergeChildren
                        if let Some(source) = symbol.children.as_deref() {
                            match target.children.take() {
                                None => target.children = Some(Box::new(source.clone())),
                                Some(children) => {
                                    let mut children = merge_expandos(
                                        (*children)
                                            .into_iter()
                                            .chain(source.iter().cloned())
                                            .collect(),
                                    );
                                    children.sort_by_key(|s| {
                                        let r = &s.as_ref().unwrap().range;
                                        (
                                            r.start.line,
                                            r.start.character,
                                            r.end.line,
                                            r.end.character,
                                        )
                                    });
                                    target.children = Some(Box::new(children));
                                }
                            }
                        }
                        merged = true;
                    }
                }
            }
            if !merged {
                symbols[index] = Some(symbol);
            }
        }
        symbols.into_iter().flatten().map(Some).collect()
    })
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/symbols.go:LanguageService.ProvideDocumentSymbols
    pub fn document_symbols(
        &mut self,
        uri: &lsp::DocumentUri,
        hierarchical: bool,
    ) -> Result<lsp::SymbolInformationsOrDocumentSymbolsOrNull> {
        let source = self.file(uri)?;
        let file = self.source(source)?;
        let files: Vec<_> = std::iter::once(source)
            .chain(file.supplemental_source_files()?.iter().flatten().copied())
            .collect();
        let mut symbols = Vec::new();
        let mut seen = HashSet::new();
        for source in files {
            let mut syntax = Syntax::new(self.view(source)?, source)?;
            for symbol in self
                .symbols_for_children(&mut syntax, Some(source))?
                .into_iter()
                .flatten()
            {
                let r = &symbol.range;
                if seen.insert((
                    symbol.name.clone(),
                    symbol.kind.0,
                    r.start.line,
                    r.start.character,
                    r.end.line,
                    r.end.character,
                )) {
                    symbols.push(Some(symbol));
                }
            }
        }
        Ok(if hierarchical {
            lsp::SymbolInformationsOrDocumentSymbolsOrNull {
                document_symbols: Some(Box::new(symbols)),
                ..Default::default()
            }
        } else {
            lsp::SymbolInformationsOrDocumentSymbolsOrNull {
                symbol_informations: Some(Box::new(flatten(&symbols, uri))),
                ..Default::default()
            }
        })
    }
    // port: tsc/internal/ls/symbols.go:LanguageService.newDocumentSymbol
    fn add_document_symbol(
        &mut self,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        name: Option<NodeId>,
        children: Symbols,
        output: &mut Symbols,
    ) -> Result<()> {
        let view = syntax.view;
        let read = view.node(node)?;
        if read.flags() & node_flags::REPARSED != 0 {
            return Ok(());
        }
        let node_start =
            tsr_scanner::skip_trivia(syntax.file.text().as_bytes(), i64::from(read.pos()));
        let name = match name {
            Some(name) => Some(name),
            None => tsr_ast::get_name_of_declaration(view, Some(node))?,
        };
        let (text, a, b) =
            if read.kind() == K::ModuleDeclaration && !tsr_ast::is_ambient_module(view, node)? {
                let innermost = interior_module(view, node)?;
                (
                    module_name(view, node)?,
                    syntax.start(name.ok_or(tsr_arena::Error::InvalidGraph)?)?,
                    i64::from(
                        view.node(
                            view.node(innermost)?
                                .name()
                                .ok_or(tsr_arena::Error::InvalidGraph)?,
                        )?
                        .end(),
                    ),
                )
            } else if read.kind() == K::ExportAssignment
                && read
                    .data_source()
                    .as_export_assignment()
                    .is_some_and(|e| e.is_export_equals())
            {
                let (a, b) = if let Some(name) =
                    name.filter(|id| view.node(*id).is_ok_and(|n| n.pos() != n.end()))
                {
                    (syntax.start(name)?, i64::from(view.node(name)?.end()))
                } else {
                    (node_start, i64::from(read.end()))
                };
                ("export=".into(), a, b)
            } else if let Some(name) = name {
                (
                    text_of_name(syntax, name)?,
                    syntax.start(name)?.max(node_start),
                    i64::from(view.node(name)?.end()).max(node_start),
                )
            } else {
                (unnamed(syntax, node)?, node_start, node_start)
            };
        if text.is_empty() {
            return Ok(());
        }
        let (selection, fidelity) = self.range(
            syntax.source,
            TextRange::new(a, b),
            FEATURE_DOCUMENT_SYMBOLS,
        )?;
        if !fidelity.is_single_segment() {
            return Ok(());
        }
        let (mut range, fidelity) = self.range(
            syntax.source,
            TextRange::new(node_start, i64::from(read.end())),
            FEATURE_DOCUMENT_SYMBOLS,
        )?;
        if fidelity.is_none() {
            range = selection.clone();
        }
        output.push(Some(Box::new(lsp::DocumentSymbol {
            name: truncate(&text),
            kind: symbol_kind(view, node)?,
            range,
            selection_range: selection,
            children: Some(Box::new(children)),
            ..Default::default()
        })));
        Ok(())
    }
    fn symbols_for_children(
        &mut self,
        syntax: &mut Syntax<'_>,
        node: Option<NodeId>,
    ) -> Result<Symbols> {
        let mut output = Vec::new();
        let mut expandos = HashSet::new();
        if let Some(node) = node {
            for child in syntax.children(node)? {
                self.visit_document_symbol(syntax, child, &mut expandos, &mut output)?;
            }
        }
        Ok(merge_expandos(output))
    }
    // port: tsc/internal/ls/symbols.go:LanguageService.getDocumentSymbolsForChildren
    fn visit_document_symbol(
        &mut self,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        expandos: &mut HashSet<Vec<u8>>,
        output: &mut Symbols,
    ) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.visit_document_symbol_worker(syntax, node, expandos, output)
        })
    }
    fn visit_document_symbol_worker(
        &mut self,
        syntax: &mut Syntax<'_>,
        node: NodeId,
        expandos: &mut HashSet<Vec<u8>>,
        output: &mut Symbols,
    ) -> Result<()> {
        self.check_canceled()?;
        let view = syntax.view;
        let read = view.node(node)?;
        if read.flags() & node_flags::REPARSED == 0 {
            for jsdoc in syntax
                .docs
                .jsdoc(view, syntax.source, node)?
                .iter()
                .copied()
            {
                if let Some(tags) = view
                    .node(jsdoc)?
                    .data_source()
                    .as_js_doc()
                    .and_then(|d| d.tags())
                {
                    for tag in view.node_slice(view.list(tags)?.nodes())?.iter().flatten() {
                        if matches!(
                            view.node(tag)?.kind().known(),
                            Some(K::JSDocTypedefTag | K::JSDocCallbackTag)
                        ) {
                            self.add_document_symbol(syntax, tag, None, Vec::new(), output)?;
                        }
                    }
                }
            }
        }
        match read.kind().known() {
            Some(
                K::ClassDeclaration
                | K::ClassExpression
                | K::InterfaceDeclaration
                | K::EnumDeclaration,
            ) => {
                let name = source_file_tables::get_declaration_name(view, node)?;
                if utilities::is_class_like(&read) && !name.is_empty() {
                    expandos.insert(name);
                }
                let children = self.symbols_for_children(syntax, Some(node))?;
                self.add_document_symbol(syntax, node, None, children, output)?;
            }
            Some(K::ModuleDeclaration) => {
                let children =
                    self.symbols_for_children(syntax, Some(interior_module(view, node)?))?;
                self.add_document_symbol(syntax, node, None, children, output)?;
            }
            Some(K::Constructor) => {
                let children = self.symbols_for_children(syntax, read.body())?;
                self.add_document_symbol(syntax, node, None, children, output)?;
                for param in view.node_slice(read.parameters(view)?)?.iter().flatten() {
                    if utilities::is_parameter_property_declaration(view, param, node)? {
                        self.add_document_symbol(syntax, param, None, Vec::new(), output)?;
                    }
                }
            }
            Some(
                K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor,
            ) => {
                let name = source_file_tables::get_declaration_name(view, node)?;
                if !name.is_empty() {
                    expandos.insert(name);
                }
                let children = self.symbols_for_children(syntax, read.body())?;
                self.add_document_symbol(syntax, node, None, children, output)?;
            }
            Some(
                K::VariableDeclaration
                | K::BindingElement
                | K::PropertyAssignment
                | K::PropertyDeclaration,
            ) => {
                if let Some(name) = read.name() {
                    if utilities::is_binding_pattern(&view.node(name)?) {
                        self.visit_document_symbol(syntax, name, expandos, output)?;
                    } else {
                        let children = self.symbols_for_children(syntax, read.initializer())?;
                        self.add_document_symbol(syntax, node, None, children, output)?;
                    }
                }
            }
            Some(K::SpreadAssignment) => {
                self.add_document_symbol(syntax, node, read.expression(), Vec::new(), output)?;
            }
            Some(
                K::MethodSignature
                | K::PropertySignature
                | K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::EnumMember
                | K::ShorthandPropertyAssignment
                | K::TypeAliasDeclaration
                | K::ImportEqualsDeclaration
                | K::ExportSpecifier,
            ) => self.add_document_symbol(syntax, node, None, Vec::new(), output)?,
            Some(K::ImportClause) => {
                if let Some(name) = read.name() {
                    self.add_document_symbol(syntax, name, Some(name), Vec::new(), output)?;
                }
                if let Some(bindings) = read
                    .data_source()
                    .as_import_clause()
                    .unwrap()
                    .named_bindings()
                {
                    if view.node(bindings)?.kind() == K::NamespaceImport {
                        self.add_document_symbol(syntax, bindings, None, Vec::new(), output)?;
                    } else {
                        for element in view
                            .node_slice(view.node(bindings)?.elements(view)?)?
                            .iter()
                            .flatten()
                        {
                            self.add_document_symbol(syntax, element, None, Vec::new(), output)?;
                        }
                    }
                }
            }
            Some(K::BinaryExpression | K::CallExpression)
                if matches!(
                    tsr_ast::get_assignment_declaration_kind(view, node)?,
                    J::Property | J::ObjectDefinePropertyValue
                ) =>
            {
                let (target, mut function, definition, property) =
                    if let Some(binary) = read.data_source().as_binary_expression() {
                        let target = binary.left().ok_or(tsr_arena::Error::InvalidGraph)?;
                        let target_read = view.node(target)?;
                        let property = tsr_ast::get_element_or_property_access_name(view, target)?
                            .ok_or(tsr_arena::Error::InvalidGraph)?;
                        (
                            target,
                            target_read
                                .expression()
                                .ok_or(tsr_arena::Error::InvalidGraph)?,
                            binary.right().ok_or(tsr_arena::Error::InvalidGraph)?,
                            property,
                        )
                    } else {
                        let args = view.node_slice(read.arguments(view)?)?;
                        (
                            args.at(1).unwrap(),
                            args.at(0).unwrap(),
                            args.at(2).unwrap(),
                            args.at(1).unwrap(),
                        )
                    };
                // port: tsc/internal/ls/symbols.go:isPrototypeExpando
                let access_name = if tsr_ast::utilities::is_access_expression(&view.node(function)?)
                {
                    tsr_ast::get_element_or_property_access_name(view, function)?
                } else {
                    None
                };
                if let Some(name) = access_name {
                    if view.node_text(name)?.as_bytes() == b"prototype" {
                        function = view
                            .node(function)?
                            .expression()
                            .ok_or(tsr_arena::Error::InvalidGraph)?;
                        if view.node(function)?.kind() == K::Identifier {
                            expandos.insert(view.node_text(function)?.as_bytes().to_vec());
                        }
                    }
                }
                if view.node(function)?.kind() == K::Identifier
                    && expandos.contains(view.node_text(function)?.as_bytes())
                {
                    let mut children = Vec::new();
                    let mut inner = HashSet::new();
                    self.visit_document_symbol(syntax, definition, &mut inner, &mut children)?;
                    let mut member = Vec::new();
                    self.add_document_symbol(
                        syntax,
                        target,
                        Some(property),
                        children,
                        &mut member,
                    )?;
                    self.add_document_symbol(syntax, node, Some(function), member, output)?;
                } else {
                    for child in syntax.children(node)? {
                        self.visit_document_symbol(syntax, child, expandos, output)?;
                    }
                }
            }
            Some(K::ExportAssignment)
                if read
                    .data_source()
                    .as_export_assignment()
                    .unwrap()
                    .is_export_equals() =>
            {
                let mut children = Vec::new();
                if let Some(expression) = read.expression() {
                    self.visit_document_symbol(syntax, expression, expandos, &mut children)?;
                }
                self.add_document_symbol(syntax, node, None, children, output)?;
            }
            _ => {
                for child in syntax.children(node)? {
                    self.visit_document_symbol(syntax, child, expandos, output)?;
                }
            }
        }
        Ok(())
    }
}

// port: tsc/internal/ls/symbols.go:getMatchScore
fn match_score(name: &[u8], pattern: &str) -> Option<usize> {
    let mut remaining = name;
    let mut score = 0;
    for pattern in pattern.chars() {
        let exact = pattern.is_uppercase();
        loop {
            let (rune, size) = tsr_jsstring::wtf8::decode_utf8(remaining);
            if size == 0 {
                return None;
            }
            remaining = &remaining[size..];
            if exact && rune == pattern as i32
                || !exact
                    && tsr_jsstring::helpers::simple_lower_go(rune)
                        == tsr_jsstring::helpers::simple_lower_go(pattern as i32)
            {
                break;
            }
            score += 1;
        }
    }
    Some(score)
}

// port: tsc/internal/ls/utilities.go:getContainerNode
pub(crate) fn container_node(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>> {
    let mut parent = view.node(node)?.parent();
    while let Some(node) = parent {
        let read = view.node(node)?;
        if matches!(
            read.kind().known(),
            Some(
                K::SourceFile
                    | K::MethodDeclaration
                    | K::MethodSignature
                    | K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::GetAccessor
                    | K::SetAccessor
                    | K::ClassDeclaration
                    | K::InterfaceDeclaration
                    | K::EnumDeclaration
                    | K::ModuleDeclaration
            )
        ) {
            return Ok(Some(node));
        }
        parent = read.parent();
    }
    Ok(None)
}

// port: tsc/internal/ls/symbols.go:ProvideWorkspaceSymbols
pub fn workspace_symbols(
    programs: &[&tsr_compiler::Program],
    encoding: tsr_jsstring::PositionEncoding,
    cancellation: &tsr_core::CancellationToken,
    query: &str,
    exclude_libraries: bool,
) -> Result<lsp::SymbolInformationsOrWorkspaceSymbolsOrNull> {
    let mut files = HashMap::new();
    for &program in programs {
        let mut has_ts = false;
        for file in program.files() {
            has_ts |= tsr_tspath::has_implementation_ts_file_extension(
                file.bound().view().source_file()?.file_name(),
            );
        }
        for file in program.files() {
            let source = file.bound().view().source_file()?;
            let path = source.parse_options().path.as_bytes();
            // port: tsc/internal/ls/symbols.go:shouldExcludeFile
            let excluded = exclude_libraries
                && (source
                    .file_name()
                    .windows(b"/node_modules/".len())
                    .any(|s| s == b"/node_modules/")
                    || program.is_lib(path));
            if (has_ts || !source.is_declaration_file) && !excluded {
                files.insert(path.to_vec(), (program, file));
            }
        }
    }
    let mut infos = Vec::new();
    for (path, (program, file)) in files {
        if cancellation.is_canceled() {
            return Ok(lsp::SymbolInformationsOrWorkspaceSymbolsOrNull::default());
        }
        let bound = file.bound().view();
        for (name, declarations) in
            source_file_tables::get_declaration_map(bound.ast(), Some(bound), file.source())?
        {
            if let Some(score) = match_score(&name, query) {
                for declaration in declarations {
                    infos.push((
                        score,
                        name.clone(),
                        path.clone(),
                        bound.ast().node(declaration)?.pos(),
                        program,
                        file,
                        declaration,
                    ));
                }
            }
        }
    }
    // port: tsc/internal/ls/symbols.go:compareDeclarationInfos
    infos.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| tsr_jsstring::compare::compare_case_insensitive(&a.1, &b.1))
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut result = Vec::new();
    for (_, name, _, _, program, file, node) in infos.into_iter().take(256) {
        let view = file.bound().view().ast();
        let mut service = LanguageService::new(program, encoding, cancellation.clone());
        let mut syntax = Syntax::new(view, file.source())?;
        let name_node = tsr_ast::get_name_of_declaration(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let (range, fidelity) = service.range(
            file.source(),
            TextRange::new(
                syntax.start(name_node)?,
                i64::from(view.node(name_node)?.end()),
            ),
            FEATURE_DOCUMENT_SYMBOLS,
        )?;
        if !fidelity.is_single_segment() {
            continue;
        }
        let container_name = container_node(view, node)?
            .map(|id| source_file_tables::get_declaration_name(view, id))
            .transpose()?
            .filter(|name| !name.is_empty())
            .map(|name| Box::new(String::from_utf8_lossy(&name).into_owned()));
        result.push(Some(Box::new(lsp::SymbolInformation {
            name: String::from_utf8_lossy(&name).into_owned(),
            kind: symbol_kind(view, node)?,
            location: lsp::Location {
                uri: lsp::DocumentUri::from_file_name(
                    file.bound()
                        .view()
                        .source_file()?
                        .original_file_name()?
                        .as_bytes(),
                ),
                range,
            },
            container_name,
            ..Default::default()
        })));
    }
    Ok(lsp::SymbolInformationsOrWorkspaceSymbolsOrNull {
        symbol_informations: Some(Box::new(result)),
        ..Default::default()
    })
}
