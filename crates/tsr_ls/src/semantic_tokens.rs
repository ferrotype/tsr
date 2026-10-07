use crate::{syntax::Syntax, LanguageService, Result};
use tsr_ast::{
    modifier_flags, node_flags, span_map::FEATURE_SEMANTIC_TOKENS, symbol_flags as sf, utilities,
    utilities_middle, utilities_positions, AstView, NodeId, SyntaxKind as K,
};
use tsr_checker::{type_flags as tf, Operation, SymbolRef};
use tsr_compiler::diagnostic_writer::DiagnosticSources;
use tsr_core::TextRange;
use tsr_lsproto as lsp;

pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "type",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "decorator",
    "event",
    "function",
    "method",
    "macro",
    "label",
    "comment",
    "string",
    "keyword",
    "number",
    "regexp",
    "operator",
];
pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
    "local",
];
const CLASS: usize = 1;
const INTERFACE: usize = 3;
const PARAMETER: usize = 7;
const VARIABLE: usize = 8;
const PROPERTY: usize = 9;
const FUNCTION: usize = 13;
const METHOD: usize = 14;

// port: tsc/internal/ls/semantictokens.go:tokenFromDeclarationMapping
fn from_declaration(kind: tsr_ast::NodeKind) -> Option<usize> {
    Some(match kind.known()? {
        K::VariableDeclaration => VARIABLE,
        K::Parameter => PARAMETER,
        K::PropertyDeclaration
        | K::PropertySignature
        | K::GetAccessor
        | K::SetAccessor
        | K::PropertyAssignment
        | K::ShorthandPropertyAssignment => PROPERTY,
        K::ModuleDeclaration => 0,
        K::EnumDeclaration => 2,
        K::EnumMember => 10,
        K::ClassDeclaration | K::ClassExpression => CLASS,
        K::MethodDeclaration | K::MethodSignature => METHOD,
        K::FunctionDeclaration | K::FunctionExpression => FUNCTION,
        K::InterfaceDeclaration => INTERFACE,
        K::TypeAliasDeclaration => 6,
        K::TypeParameter => 5,
        _ => return None,
    })
}
// port: tsc/internal/ls/semantictokens.go:getDeclarationForBindingElement
fn binding_declaration(view: AstView<'_>, mut node: NodeId) -> Result<NodeId> {
    loop {
        let Some(parent) = view.node(node)?.parent() else {
            return Ok(node);
        };
        if !utilities::is_binding_pattern(&view.node(parent)?) {
            return Ok(node);
        }
        let grandparent = view
            .node(parent)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        if view.node(grandparent)?.kind() != K::BindingElement {
            return Ok(grandparent);
        }
        node = grandparent;
    }
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/semantictokens.go:classifySymbol
    fn classify_token_symbol(
        &self,
        checker: &Operation<'_>,
        symbol: SymbolRef,
        meaning: i32,
    ) -> Result<Option<usize>> {
        let read = checker.symbol(symbol)?;
        let flags = read.flags();
        if flags & sf::CLASS != 0 {
            return Ok(Some(CLASS));
        }
        if flags & sf::ENUM != 0 {
            return Ok(Some(2));
        }
        if flags & sf::TYPE_ALIAS != 0 {
            return Ok(Some(6));
        }
        if flags & sf::INTERFACE != 0 && meaning & utilities_positions::semantic_meaning::TYPE != 0
        {
            return Ok(Some(INTERFACE));
        }
        if flags & sf::TYPE_PARAMETER != 0 {
            return Ok(Some(5));
        }
        let declaration = match read.value_declaration() {
            Some(id) => Some(id),
            None => checker.symbol_declarations(symbol)?.first().flatten(),
        };
        if let Some(mut declaration) = declaration {
            let view = self.view(declaration)?;
            if view.node(declaration)?.kind() == K::BindingElement {
                declaration = binding_declaration(view, declaration)?;
            }
            return Ok(from_declaration(view.node(declaration)?.kind()));
        }
        Ok(None)
    }
    // port: tsc/internal/ls/semantictokens.go:reclassifyByType
    fn reclassify_token(
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        node: NodeId,
        kind: usize,
    ) -> Result<usize> {
        if !matches!(kind, VARIABLE | PROPERTY | PARAMETER) {
            return Ok(kind);
        }
        let ty = checker.get_type_at_location(node)?;
        let mut types = vec![ty];
        if checker.type_flags(ty)? & tf::UNION != 0 {
            types.extend(checker.constituents(ty)?);
        }
        if kind != PARAMETER {
            for &ty in &types {
                if !checker.get_construct_signatures(ty)?.is_empty() {
                    return Ok(CLASS);
                }
            }
        }
        let mut callable = false;
        for &ty in &types {
            if !checker.get_call_signatures(ty)?.is_empty() {
                callable = true;
                break;
            }
        }
        if callable {
            let mut properties = false;
            for ty in types {
                if checker.type_flags(ty)? & tf::OBJECT != 0
                    && !checker.properties_of_type(ty)?.is_empty()
                {
                    properties = true;
                    break;
                }
            }
            let view = syntax.view;
            let mut expression = node;
            while utilities_middle::is_right_side_of_qualified_name_or_property_access(
                view, expression,
            )? {
                expression = view.node(expression)?.parent().unwrap();
            }
            let call = view
                .node(expression)?
                .parent()
                .map(|parent| view.node(parent))
                .transpose()?
                .is_some_and(|n| {
                    n.kind() == K::CallExpression && n.expression() == Some(expression)
                });
            if !properties || call {
                return Ok(if kind == PROPERTY { METHOD } else { FUNCTION });
            }
        }
        Ok(kind)
    }
    // port: tsc/internal/ls/semantictokens.go:isLocalDeclaration
    fn is_local_declaration(&self, mut declaration: NodeId, source: NodeId) -> Result<bool> {
        let view = self.view(declaration)?;
        if view.node(declaration)?.kind() == K::BindingElement {
            declaration = binding_declaration(view, declaration)?;
        }
        let read = view.node(declaration)?;
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        let parent_read = view.node(parent)?;
        let same_source = self
            .program
            .file_of_node(declaration)
            .is_some_and(|f| f.source() == source);
        if read.kind() == K::FunctionDeclaration {
            return Ok(parent_read.kind() != K::SourceFile && same_source);
        }
        if read.kind() == K::VariableDeclaration {
            if parent_read.kind() == K::CatchClause {
                return Ok(same_source);
            }
            if parent_read.kind() == K::VariableDeclarationList {
                if let Some(grand) = parent_read.parent() {
                    let grand = view.node(grand)?;
                    let great_is_source = grand
                        .parent()
                        .map(|p| view.node(p))
                        .transpose()?
                        .is_some_and(|p| p.kind() == K::SourceFile);
                    return Ok((!great_is_source || grand.kind() == K::CatchClause) && same_source);
                }
            }
        }
        Ok(false)
    }
    // port: tsc/internal/ls/semantictokens.go:LanguageService.ProvideSemanticTokens
    // port: tsc/internal/ls/semantictokens.go:LanguageService.ProvideSemanticTokensRange
    pub fn semantic_tokens(
        &mut self,
        checker: &mut Operation<'_>,
        uri: &lsp::DocumentUri,
        range: Option<&lsp::Range>,
        capabilities: Option<&lsp::SemanticTokensClientCapabilities>,
    ) -> Result<lsp::SemanticTokensOrNull> {
        let source = self.file(uri)?;
        let spans: Vec<_> = if let Some(range) = range {
            self.converters
                .from_lsp_range_intersecting_for_source_file(
                    self.program,
                    source,
                    range,
                    FEATURE_SEMANTIC_TOKENS,
                )?
                .into_iter()
                .map(|p| (p.script, p.mapped.span))
                .collect()
        } else {
            std::iter::once(source)
                .chain(self.program.supplemental_sources(source)?)
                .map(|id| {
                    let read = self.view(id)?.node(id)?;
                    Ok((
                        id,
                        TextRange::new(i64::from(read.pos()), i64::from(read.end())),
                    ))
                })
                .collect::<Result<_>>()?
        };
        let mut tokens = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (source, span) in spans {
            let view = self.view(source)?;
            let mut syntax = Syntax::new(view, source)?;
            // port: tsc/internal/ls/semantictokens.go:LanguageService.collectSemanticTokensInRange
            let mut stack = vec![(source, false)];
            while let Some((node, mut jsx)) = stack.pop() {
                self.check_canceled()?;
                let read = view.node(node)?;
                if read.flags() & node_flags::REPARSED != 0
                    || i64::from(read.pos()) >= span.end()
                    || i64::from(read.end()) <= span.pos()
                {
                    continue;
                }
                if matches!(
                    read.kind().known(),
                    Some(K::JsxElement | K::JsxSelfClosingElement)
                ) {
                    jsx = true;
                } else if read.kind() == K::JsxExpression {
                    jsx = false;
                }
                let in_import = read
                    .parent()
                    .map(|p| view.node(p))
                    .transpose()?
                    .is_some_and(|p| {
                        matches!(
                            p.kind().known(),
                            Some(K::ImportClause | K::ImportSpecifier | K::NamespaceImport)
                        )
                    });
                if read.kind() == K::Identifier
                    && !jsx
                    && !in_import
                    && !matches!(view.node_text(node)?.as_bytes(), b"" | b"Infinity" | b"NaN")
                {
                    if let Some(symbol) = checker.get_symbol_at_location(node)? {
                        let symbol = checker.skip_alias(symbol)?;
                        let meaning = crate::meaning::meaning(view, node, checker)?;
                        if let Some(mut kind) =
                            self.classify_token_symbol(checker, symbol, meaning)?
                        {
                            let mut modifiers = 0;
                            if let Some(parent) = read.parent() {
                                let parent = view.node(parent)?;
                                if (parent.kind() == K::BindingElement
                                    || from_declaration(parent.kind()) == Some(kind))
                                    && parent.name() == Some(node)
                                {
                                    modifiers |= 1;
                                }
                            }
                            if kind == PARAMETER && utilities_middle::is_right_side_of_qualified_name_or_property_access(view, node)? { kind = PROPERTY; }
                            kind = Self::reclassify_token(checker, &syntax, node, kind)?;
                            if let Some(decl) = checker.symbol(symbol)?.value_declaration() {
                                let decl_view = self.view(decl)?;
                                let flags =
                                    utilities::get_combined_modifier_flags(decl_view, decl)?;
                                let node_flags =
                                    utilities::get_combined_node_flags(decl_view, decl)?;
                                if flags & modifier_flags::STATIC != 0 {
                                    modifiers |= 1 << 3;
                                }
                                if flags & modifier_flags::ASYNC != 0 {
                                    modifiers |= 1 << 6;
                                }
                                if !matches!(kind, CLASS | INTERFACE)
                                    && (flags & modifier_flags::READONLY != 0
                                        || node_flags & node_flags::CONST != 0
                                        || checker.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0)
                                {
                                    modifiers |= 1 << 2;
                                }
                                if matches!(kind, VARIABLE | FUNCTION)
                                    && self.is_local_declaration(decl, source)?
                                {
                                    modifiers |= 1 << 10;
                                }
                                if let Some(owner) = self.program.file_of_node(decl) {
                                    let file = decl_view.source_file(owner.source())?;
                                    if self.program.is_lib(file.parse_options().path.as_bytes()) {
                                        modifiers |= 1 << 9;
                                    }
                                }
                            } else {
                                for decl in checker.symbol_declarations(symbol)?.iter().flatten() {
                                    let Some(file) = self.program.file_of_node(decl) else {
                                        continue;
                                    };
                                    if self.program.is_lib(
                                        file.bound()
                                            .view()
                                            .source_file()?
                                            .parse_options()
                                            .path
                                            .as_bytes(),
                                    ) {
                                        modifiers |= 1 << 9;
                                        break;
                                    }
                                }
                            }
                            if seen.insert((source, node, kind, modifiers)) {
                                let start = syntax.start(node)?;
                                let (range, fidelity) = self.range(
                                    source,
                                    TextRange::new(start, i64::from(read.end())),
                                    FEATURE_SEMANTIC_TOKENS,
                                )?;
                                tokens.push((
                                    range,
                                    fidelity,
                                    syntax.file.parse_options().path.clone(),
                                    read.pos(),
                                    kind,
                                    modifiers,
                                ));
                            }
                        }
                    }
                }
                stack.extend(syntax.children(node)?.into_iter().rev().map(|id| (id, jsx)));
            }
        }
        if tokens.is_empty() {
            return Ok(lsp::SemanticTokensOrNull::default());
        }
        tokens.sort_by(|a, b| {
            (a.0.start.line, a.0.start.character, a.2.as_bytes(), a.3).cmp(&(
                b.0.start.line,
                b.0.start.character,
                b.2.as_bytes(),
                b.3,
            ))
        });
        // port: tsc/internal/ls/semantictokens.go:encodeSemanticTokens
        let type_indices: Vec<_> = TOKEN_TYPES
            .iter()
            .enumerate()
            .filter(|(_, name)| {
                capabilities.is_some_and(|c| c.token_types.iter().any(|n| n == *name))
            })
            .map(|(index, _)| index)
            .collect();
        let modifier_indices: Vec<_> = TOKEN_MODIFIERS
            .iter()
            .enumerate()
            .filter(|(_, name)| {
                capabilities.is_some_and(|c| c.token_modifiers.iter().any(|n| n == *name))
            })
            .map(|(index, _)| index)
            .collect();
        let mut data = Vec::new();
        let (mut previous_line, mut previous_char) = (0, 0);
        for (range, fidelity, _, _, kind, modifiers) in tokens {
            let Some(index) = type_indices.iter().position(|&t| t == kind) else {
                continue;
            };
            if !fidelity.is_exact() {
                continue;
            }
            if range.start.line != range.end.line || range.end.character < range.start.character {
                return Err(tsr_astnav::Error::Assertion(
                    "semantic tokens: token spans multiple lines".into(),
                )
                .into());
            }
            let (line, character) = (range.start.line, range.start.character);
            if !data.is_empty() && line == previous_line && character == previous_char {
                continue;
            }
            if !data.is_empty() && (line, character) < (previous_line, previous_char) {
                return Err(tsr_astnav::Error::Assertion(
                    "semantic tokens: positions must be strictly increasing".into(),
                )
                .into());
            }
            let mask = modifier_indices
                .iter()
                .enumerate()
                .fold(0, |mask, (client, server)| {
                    if modifiers & (1 << server) != 0 {
                        mask | (1 << client)
                    } else {
                        mask
                    }
                });
            data.extend([
                line - previous_line,
                if line == previous_line {
                    character - previous_char
                } else {
                    character
                },
                range.end.character - range.start.character,
                index as u32,
                mask,
            ]);
            (previous_line, previous_char) = (line, character);
        }
        Ok(lsp::SemanticTokensOrNull {
            semantic_tokens: Some(Box::new(lsp::SemanticTokens {
                data,
                ..Default::default()
            })),
        })
    }
}
