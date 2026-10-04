//! Definition navigation uses the requesting program's checker and resolves
//! output locations against the retained source owners, including mapped files.
use crate::{syntax::Syntax, LanguageService, Result};
use tsr_ast::{
    span_map::{FEATURE_DEFINITION, FEATURE_TYPE_DEFINITION},
    symbol_flags as sf, utilities as ast, utilities_middle as middle,
    utilities_positions as positions, AstView, NodeId, SyntaxKind as K,
};
use tsr_checker::{context_flags, type_flags as tf, Operation, SymbolRef, TypeRef};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

pub(crate) fn ancestor(
    view: AstView<'_>,
    mut node: Option<NodeId>,
    mut predicate: impl FnMut(NodeId) -> Result<bool>,
) -> Result<Option<NodeId>> {
    while let Some(id) = node {
        if predicate(id)? {
            return Ok(Some(id));
        }
        node = view.node(id)?.parent();
    }
    Ok(None)
}
// port: tsc/internal/ls/definition.go:getDeclarationNameForKeyword
pub(crate) fn declaration_name_for_keyword(view: AstView<'_>, node: NodeId) -> Result<NodeId> {
    let read = view.node(node)?;
    if tsr_ast::utilities_tail::is_keyword(read.kind()) {
        if let Some(parent) = read.parent() {
            let parent = view.node(parent)?;
            if parent.kind() == K::VariableDeclarationList {
                if let Some(decl) = view
                    .node_slice(
                        view.list(
                            parent
                                .data_source()
                                .as_variable_declaration_list()
                                .unwrap()
                                .declarations()
                                .ok_or(tsr_arena::Error::InvalidGraph)?,
                        )?
                        .nodes(),
                    )?
                    .iter()
                    .flatten()
                    .next()
                {
                    if let Some(name) = view.node(decl)?.name() {
                        return Ok(name);
                    }
                }
            } else if tsr_ast::is_declaration(&parent) {
                if let Some(name) = parent.name() {
                    if read.pos() < view.node(name)?.pos() {
                        return Ok(name);
                    }
                }
            }
        }
    }
    Ok(node)
}
// port: tsc/internal/ls/utilities.go:getTargetLabel
pub(crate) fn target_label(
    view: AstView<'_>,
    reference: NodeId,
    name: &[u8],
) -> Result<Option<NodeId>> {
    let mut node = Some(reference);
    while let Some(id) = node {
        let read = view.node(id)?;
        if read.kind() == K::LabeledStatement {
            let label = read
                .data_source()
                .as_labeled_statement()
                .unwrap()
                .label()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            if view.node_text(label)?.as_bytes() == name {
                return Ok(Some(label));
            }
        }
        node = read.parent();
    }
    Ok(None)
}
// port: tsc/internal/ls/findallreferences.go:getContextNode
pub(crate) fn context_node(view: AstView<'_>, mut node: NodeId) -> Result<Option<NodeId>> {
    loop {
        let read = view.node(node)?;
        let Some(parent_id) = read.parent() else {
            return Ok(Some(node));
        };
        let parent = view.node(parent_id)?;
        return Ok(match read.kind().known() {
            Some(K::VariableDeclaration) => {
                if parent.kind() != K::VariableDeclarationList
                    || view
                        .node_slice(
                            view.list(
                                parent
                                    .data_source()
                                    .as_variable_declaration_list()
                                    .unwrap()
                                    .declarations()
                                    .ok_or(tsr_arena::Error::InvalidGraph)?,
                            )?
                            .nodes(),
                        )?
                        .len()
                        != 1
                {
                    Some(node)
                } else if let Some(grand) = parent.parent() {
                    match view.node(grand)?.kind().known() {
                        Some(K::VariableStatement) => Some(grand),
                        Some(K::ForInStatement | K::ForOfStatement) => None,
                        _ => Some(parent_id),
                    }
                } else {
                    Some(parent_id)
                }
            }
            Some(K::BindingElement) => {
                node = parent.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
                continue;
            }
            Some(K::ImportSpecifier) => parent
                .parent()
                .map(|p| view.node(p).map(|n| n.parent()))
                .transpose()?
                .flatten(),
            Some(K::ExportSpecifier | K::NamespaceImport) => parent.parent(),
            Some(K::ImportClause | K::NamespaceExport) => Some(parent_id),
            Some(K::BinaryExpression) => Some(if parent.kind() == K::ExpressionStatement {
                parent_id
            } else {
                node
            }),
            Some(K::ForInStatement | K::ForOfStatement | K::SwitchStatement) => None,
            Some(K::PropertyAssignment | K::ShorthandPropertyAssignment)
                if positions::is_array_literal_or_object_literal_destructuring_pattern(
                    view, parent_id,
                )? =>
            {
                if let Some(found) = ancestor(view, Some(parent_id), |id| {
                    Ok(matches!(
                        view.node(id)?.kind().known(),
                        Some(K::BinaryExpression | K::ForInStatement | K::ForOfStatement)
                    ))
                })? {
                    node = found;
                    continue;
                }
                None
            }
            _ => Some(node),
        });
    }
}

// port: tsc/internal/ls/utilities.go:getContainingObjectLiteralElement
pub(crate) fn object_literal_element(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>> {
    let read = view.node(node)?;
    let Some(parent_id) = read.parent() else {
        return Ok(None);
    };
    let parent = view.node(parent_id)?;
    let mut element = None;
    match read.kind().known() {
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral)
            if parent.kind() == K::ComputedPropertyName =>
        {
            element = parent.parent();
        }
        Some(
            K::StringLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::NumericLiteral
            | K::Identifier
            | K::JsxNamespacedName,
        ) if parent.name() == Some(node) => element = Some(parent_id),
        _ => {}
    }
    let Some(element) = element else {
        return Ok(None);
    };
    let read = view.node(element)?;
    if ast::is_object_literal_element(&read)
        || matches!(
            read.kind().known(),
            Some(K::JsxAttribute | K::JsxSpreadAttribute)
        )
    {
        if let Some(parent) = read.parent() {
            if matches!(
                view.node(parent)?.kind().known(),
                Some(K::ObjectLiteralExpression | K::JsxAttributes)
            ) {
                return Ok(Some(element));
            }
        }
    }
    Ok(None)
}

impl LanguageService<'_> {
    pub(crate) fn bound_symbol(
        &self,
        checker: &Operation<'_>,
        node: NodeId,
    ) -> Result<Option<SymbolRef>> {
        let file = self
            .program
            .file_of_node(node)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        file.bound()
            .view()
            .node_binding(node)?
            .and_then(|b| b.symbol)
            .map(|s| checker.symbol_ref(s).map_err(Into::into))
            .transpose()
    }
    // port: tsc/internal/ls/definition.go:getDeclarationsFromObjectLiteralElement
    fn object_literal_declarations(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<Vec<NodeId>> {
        let view = self.view(node)?;
        let Some(element) = object_literal_element(view, node)? else {
            return Ok(Vec::new());
        };
        let parent = view
            .node(element)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let Some(context) = checker.get_contextual_type(parent, context_flags::NONE)? else {
            return Ok(Vec::new());
        };
        let mut properties =
            checker.get_property_symbols_from_contextual_type(element, context, false)?;
        let mut inferred = false;
        for &symbol in &properties {
            if let Some(declaration) = checker.symbol(symbol)?.value_declaration() {
                let view = self.view(declaration)?;
                let read = view.node(declaration)?;
                if ast::is_object_literal_element(&read)
                    && read.name() == Some(node)
                    && read.parent().is_some_and(|id| {
                        view.node(id)
                            .is_ok_and(|n| n.kind() == K::ObjectLiteralExpression)
                    })
                {
                    inferred = true;
                    break;
                }
            }
        }
        if inferred {
            if let Some(context) =
                checker.get_contextual_type(parent, context_flags::IGNORE_NODE_INFERENCES)?
            {
                let without =
                    checker.get_property_symbols_from_contextual_type(element, context, false)?;
                if !without.is_empty() {
                    properties = without;
                }
            }
        }
        let mut result = Vec::new();
        for symbol in properties {
            result.extend(checker.symbol_declarations(symbol)?.iter().flatten());
        }
        Ok(result)
    }
    // port: tsc/internal/ls/definition.go:getDeclarationsFromLocation
    pub(crate) fn declarations_at(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<Vec<NodeId>> {
        let view = self.view(node)?;
        let read = view.node(node)?;
        if let Some(parent_id) = read.parent() {
            let parent = view.node(parent_id)?;
            if read.kind() == K::Identifier && parent.kind() == K::ShorthandPropertyAssignment {
                let symbol = checker.get_resolved_symbol(node)?;
                let mut declarations: Vec<_> = checker
                    .symbol_declarations(symbol)?
                    .iter()
                    .flatten()
                    .collect();
                declarations.extend(self.object_literal_declarations(checker, node)?);
                return Ok(declarations);
            }
            if ast::is_property_name(&read) && parent.kind() == K::BindingElement {
                if let Some(grand) = parent.parent().filter(|id| {
                    view.node(*id)
                        .is_ok_and(|n| n.kind() == K::ObjectBindingPattern)
                }) {
                    let data = parent.data_source().as_binding_element().unwrap();
                    if data.dot_dot_dot_token().is_none()
                        && data.property_name().or(parent.name()) == Some(node)
                    {
                        if let Some(name) =
                            tsr_ast::utilities_targets::try_get_text_of_property_name(view, node)?
                        {
                            let ty = checker.get_type_at_location(grand)?;
                            let types = if checker.type_flags(ty)? & tf::UNION != 0 {
                                checker.constituents(ty)?
                            } else {
                                vec![ty]
                            };
                            let mut declarations = Vec::new();
                            for ty in types {
                                if let Some(prop) = checker.get_property_of_type(ty, &name)? {
                                    declarations.extend(
                                        checker.symbol_declarations(prop)?.iter().flatten(),
                                    );
                                }
                            }
                            return Ok(declarations);
                        }
                    }
                }
            }
        }
        let node = declaration_name_for_keyword(view, node)?;
        if let Some(mut symbol) = checker.get_symbol_at_location(node)? {
            let read = checker.symbol(symbol)?;
            if read.flags() & sf::CLASS != 0
                && read.flags() & (sf::FUNCTION | sf::VARIABLE) == 0
                && view.node(node)?.kind() == K::ConstructorKeyword
            {
                if let Some(members) = read.members() {
                    if let Some(constructor) = checker
                        .symbol_table(members)?
                        .get(tsr_ast::internal_symbol_names::CONSTRUCTOR)
                        .flatten()
                    {
                        symbol = checker.symbol_ref(constructor)?;
                    }
                }
            }
            if checker.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                let (resolved, ok) = checker.resolve_alias(symbol)?;
                if ok {
                    symbol = resolved;
                }
            }
            let contextual = self.object_literal_declarations(checker, node)?;
            if !contextual.is_empty() {
                return Ok(contextual);
            }
            let declarations: Vec<_> = checker
                .symbol_declarations(symbol)?
                .iter()
                .flatten()
                .collect();
            if !declarations.is_empty() {
                return Ok(declarations);
            }
        }
        Ok(checker.get_index_signatures_at_location(node)?)
    }
    // port: tsc/internal/ls/definition.go:getAncestorCallLikeExpression
    pub(crate) fn ancestor_call(&self, node: NodeId) -> Result<Option<NodeId>> {
        let view = self.view(node)?;
        let mut target = node;
        while middle::is_right_side_of_property_access(view, target)? {
            target = view
                .node(target)?
                .parent()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
        }
        let Some(call) = view.node(target)?.parent() else {
            return Ok(None);
        };
        Ok((middle::is_call_like_expression(view, &view.node(call)?)?
            && middle::get_invoked_expression(view, call)? == Some(target))
        .then_some(call))
    }
    // port: tsc/internal/ls/definition.go:tryGetSignatureDeclaration
    pub(crate) fn called_declaration(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<Option<NodeId>> {
        if let Some(call) = self.ancestor_call(node)? {
            let signature = checker.get_resolved_signature(call)?;
            if let Some(declaration) = checker.signature_declaration(signature)? {
                let id = declaration.id();
                let view = self.view(id)?;
                let read = view.node(id)?;
                if ast::is_function_like(Some(&read)) && read.kind() != K::FunctionType {
                    return Ok(Some(id));
                }
            }
        }
        Ok(None)
    }
    // port: tsc/internal/ls/definition.go:symbolMatchesSignature
    fn symbol_matches_signature(
        &self,
        checker: &Operation<'_>,
        symbol: SymbolRef,
        declaration: NodeId,
    ) -> Result<bool> {
        if let Some(called) = self.bound_symbol(checker, declaration)? {
            if symbol == called || checker.symbol(called)?.parent() == Some(symbol.id()) {
                return Ok(true);
            }
        }
        let view = self.view(declaration)?;
        if let Some(parent) = view.node(declaration)?.parent() {
            if tsr_ast::is_assignment_expression(view, parent, false)?
                || !middle::is_call_like_expression(view, &view.node(parent)?)?
                    && self.bound_symbol(checker, parent)? == Some(symbol)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    // port: tsc/internal/ls/definition.go:getSymbolForOverriddenMember
    fn overridden_symbol(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<Option<SymbolRef>> {
        let view = self.view(node)?;
        let Some(element) = ancestor(view, Some(node), |id| {
            Ok(ast::is_class_element(&view.node(id)?))
        })?
        else {
            return Ok(None);
        };
        let Some(name) = view.node(element)?.name() else {
            return Ok(None);
        };
        let Some(class) = ancestor(view, Some(element), |id| {
            Ok(matches!(
                view.node(id)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ))
        })?
        else {
            return Ok(None);
        };
        let Some(extends) =
            tsr_ast::utilities_class::get_class_extends_heritage_element(view, class)?
        else {
            return Ok(None);
        };
        let expression = tsr_ast::skip_parentheses(
            view,
            view.node(extends)?
                .expression()
                .ok_or(tsr_arena::Error::InvalidGraph)?,
        )?;
        let symbol = if view.node(expression)?.kind() == K::ClassExpression {
            self.bound_symbol(checker, expression)?
        } else {
            checker.get_symbol_at_location(expression)?
        };
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        let ty = if ast::has_static_modifier(view, element)? {
            checker.get_type_of_symbol(symbol)?
        } else {
            checker.get_declared_type_of_symbol(symbol)?
        };
        Ok(checker.get_property_of_type(
            ty,
            &tsr_ast::utilities_targets::get_text_of_property_name(view, name)?,
        )?)
    }
    // port: tsc/internal/ls/definition.go:getTypeOfSymbolAtLocation
    fn definition_type(
        &self,
        checker: &mut Operation<'_>,
        symbol: SymbolRef,
        node: NodeId,
    ) -> Result<TypeRef> {
        let ty = checker.get_type_of_symbol_at_location(symbol, Some(node))?;
        let type_symbol = checker.type_symbol(ty)?;
        let mut inferred = type_symbol == Some(symbol.id());
        if !inferred {
            if let (Some(type_symbol), Some(value)) =
                (type_symbol, checker.symbol(symbol)?.value_declaration())
            {
                let view = self.view(value)?;
                inferred = view.node(value)?.kind() == K::VariableDeclaration
                    && view.node(value)?.initializer()
                        == checker
                            .symbol(checker.symbol_ref(type_symbol)?)?
                            .value_declaration();
            }
        }
        if inferred {
            let signatures = checker.get_call_signatures(ty)?;
            if signatures.len() == 1 {
                return Ok(checker.get_return_type_of_signature(signatures[0])?);
            }
        }
        Ok(ty)
    }
    // port: tsc/internal/ls/definition.go:getDeclarationsFromType
    fn type_declarations(checker: &Operation<'_>, ty: TypeRef) -> Result<Vec<NodeId>> {
        let flags = checker.type_flags(ty)?;
        let types = if flags & tf::UNION != 0 {
            checker.constituents(ty)?
        } else if flags & tf::NEVER != 0 {
            Vec::new()
        } else {
            vec![ty]
        };
        let mut result = Vec::new();
        for ty in types {
            if let Some(symbol) = checker.type_symbol(ty)? {
                for declaration in checker
                    .symbol_declarations(checker.symbol_ref(symbol)?)?
                    .iter()
                    .flatten()
                {
                    if !result.contains(&declaration) {
                        result.push(declaration);
                    }
                }
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/definition.go:LanguageService.createDefinitionLocations
    pub(crate) fn definition_locations(
        &mut self,
        origin: &lsp::Range,
        declarations: &[NodeId],
        reference: Option<&[u8]>,
        feature: i32,
    ) -> Result<Vec<Option<Box<lsp::LocationLink>>>> {
        let mut links = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Some(file_name) = reference {
            links.push(Some(Box::new(lsp::LocationLink {
                origin_selection_range: Some(Box::new(origin.clone())),
                target_uri: lsp::DocumentUri::from_file_name(file_name),
                ..Default::default()
            })));
        }
        for &declaration in declarations {
            let file = self
                .program
                .file_of_node(declaration)
                .ok_or(tsr_arena::Error::WrongOwner)?;
            let view = file.bound().view().ast();
            let mut syntax = Syntax::new(view, file.source())?;
            let name =
                tsr_ast::get_name_of_declaration(view, Some(declaration))?.unwrap_or(declaration);
            let name_read = view.node(name)?;
            let name_range = if name_read.kind() == K::EmptyStatement {
                TextRange::new(i64::from(name_read.pos()), i64::from(name_read.pos()))
            } else {
                TextRange::new(syntax.start(name)?, i64::from(name_read.end()))
            };
            if !seen.insert((file.source(), name_range.pos(), name_range.end())) {
                continue;
            }
            let context = context_node(view, declaration)?.unwrap_or(declaration);
            let context_range = TextRange::new(
                syntax.start(context)?.min(name_range.pos()),
                i64::from(view.node(context)?.end()).max(name_range.end()),
            );
            let (selection, fidelity) = self.range(file.source(), name_range, feature)?;
            if !fidelity.is_single_segment() {
                continue;
            }
            let (range, context_fidelity) =
                self.unrestricted_range(file.source(), context_range)?;
            let range = if context_fidelity.is_none() || !range_contains(&range, &selection) {
                selection.clone()
            } else {
                range
            };
            links.push(Some(Box::new(lsp::LocationLink {
                origin_selection_range: Some(Box::new(origin.clone())),
                target_uri: lsp::DocumentUri::from_file_name(
                    syntax.file.original_file_name()?.as_bytes(),
                ),
                target_range: range,
                target_selection_range: selection,
            })));
        }
        Ok(links)
    }
    // port: tsc/internal/ls/utilities.go:getReferenceAtPosition
    pub(crate) fn reference_at(
        &self,
        syntax: &mut Syntax<'_>,
        position: i64,
    ) -> Result<Option<Vec<u8>>> {
        let file = self
            .program
            .file_of_node(syntax.source)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        for reference in syntax.file.referenced_files()?.iter() {
            if reference.loc.pos() <= position && position <= reference.loc.end() {
                return Ok(self
                    .program
                    .source_file_from_reference(file, reference)
                    .map(|f| {
                        f.bound()
                            .view()
                            .source_file()
                            .map(|s| s.file_name().to_vec())
                    })
                    .transpose()?);
            }
        }
        for reference in syntax.file.type_reference_directives()?.iter() {
            if reference.loc.pos() <= position && position <= reference.loc.end() {
                return Ok(self
                    .program
                    .resolved_type_reference_from_directive(file, reference)?
                    .and_then(|r| self.program.source_file(r.resolved_file_name.as_bytes()))
                    .map(|f| {
                        f.bound()
                            .view()
                            .source_file()
                            .map(|s| s.file_name().to_vec())
                    })
                    .transpose()?);
            }
        }
        for reference in syntax.file.lib_reference_directives()?.iter() {
            if reference.loc.pos() <= position && position <= reference.loc.end() {
                return Ok(self
                    .program
                    .lib_file_from_reference(reference)
                    .map(|f| {
                        f.bound()
                            .view()
                            .source_file()
                            .map(|s| s.file_name().to_vec())
                    })
                    .transpose()?);
            }
        }
        if syntax.file.imports()?.is_empty() && syntax.file.module_augmentations()?.is_empty() {
            return Ok(None);
        }
        let node = syntax.nav().get_touching_token(position)?;
        let view = syntax.view;
        let read = view.node(node)?;
        if !ast::is_string_literal_like(&read)
            || !tsr_tspath::is_external_module_name_relative(view.node_text(node)?.as_bytes())
        {
            return Ok(None);
        }
        let Some(parent_id) = read.parent() else {
            return Ok(None);
        };
        let parent = view.node(parent_id)?;
        let specifier = matches!(
            parent.kind().known(),
            Some(K::ExternalModuleReference | K::ImportDeclaration | K::JSImportDeclaration)
        ) || parent.kind() == K::CallExpression
            && (middle::is_require_call(view, &parent, false)?
                || positions::is_import_call(view, parent_id)?)
            && view.node_slice(parent.arguments(view)?)?.at(0) == Some(node);
        if !specifier {
            return Ok(None);
        }
        let Some(resolution) = self.program.resolved_module_from_specifier(file, node)? else {
            return Ok(None);
        };
        let name = if resolution.resolved_file_name.is_empty() {
            tsr_tspath::resolve(
                &tsr_tspath::directory(syntax.file.file_name()),
                &[view.node_text(node)?.as_bytes()],
            )
        } else {
            resolution.resolved_file_name.as_bytes().to_vec()
        };
        // The pin carries an unresolved path through to createDefinitionLocations
        // as well; only a loaded reference takes the early return.
        Ok(Some(name))
    }
    // port: tsc/internal/ls/definition.go:LanguageService.ProvideDefinition
    // port: tsc/internal/ls/definition.go:LanguageService.ProvideTypeDefinition
    pub fn definition(
        &mut self,
        checker: &mut Operation<'_>,
        uri: &lsp::DocumentUri,
        position: &lsp::Position,
        type_definition: bool,
        links: bool,
    ) -> Result<lsp::LocationOrLocationsOrDefinitionLinksOrNull> {
        let feature = if type_definition {
            FEATURE_TYPE_DEFINITION
        } else {
            FEATURE_DEFINITION
        };
        let file = self.file(uri)?;
        let projections = self.converters.from_lsp_position_for_source_file(
            self.program,
            file,
            position,
            feature,
        )?;
        let mut all = Vec::new();
        for projection in projections {
            self.check_canceled()?;
            if !projection.mapped.fidelity.is_single_segment() {
                continue;
            }
            let source = projection.script;
            let view = self.view(source)?;
            let mut syntax = Syntax::new(view, source)?;
            let node = syntax
                .nav()
                .get_touching_property_name(i64::from(projection.mapped.position))?;
            let read = view.node(node)?;
            if read.kind() == K::SourceFile {
                continue;
            }
            let origin = self
                .unrestricted_range(
                    source,
                    TextRange::new(syntax.start(node)?, i64::from(read.end())),
                )?
                .0;
            if type_definition {
                let node = declaration_name_for_keyword(view, node)?;
                if let Some(symbol) = checker.get_symbol_at_location(node)? {
                    let ty = self.definition_type(checker, symbol, node)?;
                    let mut declarations = Self::type_declarations(checker, ty)?;
                    if let Some(argument) = checker.get_first_type_argument_from_known_type(ty)? {
                        let mut from_argument = Self::type_declarations(checker, argument)?;
                        from_argument.extend(declarations);
                        declarations = from_argument;
                    }
                    if declarations.is_empty()
                        && checker.symbol(symbol)?.flags() & sf::VALUE == 0
                        && checker.symbol(symbol)?.flags() & sf::TYPE != 0
                    {
                        declarations.extend(checker.symbol_declarations(symbol)?.iter().flatten());
                    }
                    all.extend(self.definition_locations(&origin, &declarations, None, feature)?);
                }
                continue;
            }
            let reference =
                self.reference_at(&mut syntax, i64::from(projection.mapped.position))?;
            if let Some(name) = reference
                .as_deref()
                .filter(|name| self.program.source_file(name).is_some())
            {
                all.extend(self.definition_locations(&origin, &[], Some(name), feature)?);
                continue;
            }
            if read.kind() == K::OverrideKeyword {
                if let Some(symbol) = self.overridden_symbol(checker, node)? {
                    let declarations: Vec<_> = checker
                        .symbol_declarations(symbol)?
                        .iter()
                        .flatten()
                        .collect();
                    all.extend(self.definition_locations(&origin, &declarations, None, feature)?);
                    continue;
                }
            }
            if middle::is_jump_statement_target(view, node)? {
                if let Some(label) = target_label(
                    view,
                    read.parent().ok_or(tsr_arena::Error::InvalidGraph)?,
                    view.node_text(node)?.as_bytes(),
                )? {
                    all.extend(self.definition_locations(&origin, &[label], None, feature)?);
                    continue;
                }
            }
            if read.kind() == K::CaseKeyword
                || read.kind() == K::DefaultKeyword
                    && read
                        .parent()
                        .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::DefaultClause))
            {
                if let Some(switch) = ancestor(view, read.parent(), |id| {
                    Ok(view.node(id)?.kind() == K::SwitchStatement)
                })? {
                    let start = syntax.start(switch)?;
                    let (range, _) =
                        self.range(source, TextRange::new(start, start + 6), feature)?;
                    all.push(Some(Box::new(lsp::LocationLink {
                        target_uri: lsp::DocumentUri::from_file_name(
                            syntax.file.original_file_name()?.as_bytes(),
                        ),
                        target_range: range.clone(),
                        target_selection_range: range,
                        ..Default::default()
                    })));
                    continue;
                }
            }
            if matches!(
                read.kind().known(),
                Some(K::ReturnKeyword | K::YieldKeyword | K::AwaitKeyword)
            ) {
                if let Some(function) = ancestor(view, Some(node), |id| {
                    Ok(ast::is_function_like_declaration(Some(&view.node(id)?)))
                })? {
                    all.extend(self.definition_locations(&origin, &[function], None, feature)?);
                    continue;
                }
            }
            let mut declarations = self.declarations_at(checker, node)?;
            if let Some(called) = self.called_declaration(checker, node)? {
                let kind = self.view(called)?.node(called)?.kind();
                let jsx_constructor = read.parent().is_some_and(|id| {
                    view.node(id)
                        .is_ok_and(|n| middle::is_jsx_opening_like_element(&n))
                }) && matches!(
                    kind.known(),
                    Some(
                        K::Constructor
                            | K::ConstructorType
                            | K::CallSignature
                            | K::ConstructSignature
                    )
                );
                if !jsx_constructor {
                    let mut matches = false;
                    if let Some(symbol) =
                        checker.get_symbol_at_location(declaration_name_for_keyword(view, node)?)?
                    {
                        for symbol in checker.get_root_symbols(symbol)? {
                            if self.symbol_matches_signature(checker, symbol, called)? {
                                matches = true;
                                break;
                            }
                        }
                    }
                    if matches && kind != K::Constructor {
                        declarations.clear();
                    } else {
                        let mut kept = Vec::new();
                        for id in declarations {
                            if id != called
                                && (!matches
                                    || matches!(
                                        self.view(id)?.node(id)?.kind().known(),
                                        Some(K::ClassDeclaration | K::ClassExpression)
                                    ))
                            {
                                kept.push(id);
                            }
                        }
                        declarations = kept;
                    }
                    declarations.push(called);
                }
            }
            all.extend(self.definition_locations(
                &origin,
                &declarations,
                reference.as_deref(),
                feature,
            )?);
        }
        // port: tsc/internal/ls/definition.go:combineDefinitionResponses
        let mut seen = std::collections::HashSet::new();
        all.retain(|link| {
            link.as_ref().is_some_and(|l| {
                seen.insert((
                    l.target_uri.clone(),
                    l.target_selection_range.start.line,
                    l.target_selection_range.start.character,
                    l.target_selection_range.end.line,
                    l.target_selection_range.end.character,
                ))
            })
        });
        Ok(if links {
            lsp::LocationOrLocationsOrDefinitionLinksOrNull {
                definition_links: Some(Box::new(all)),
                ..Default::default()
            }
        } else {
            lsp::LocationOrLocationsOrDefinitionLinksOrNull {
                locations: Some(Box::new(
                    all.into_iter()
                        .flatten()
                        .map(|l| lsp::Location {
                            uri: l.target_uri,
                            range: l.target_selection_range,
                        })
                        .collect(),
                )),
                ..Default::default()
            }
        })
    }
}
// port: tsc/internal/ls/definition.go:lspRangeContains
pub(crate) fn range_contains(outer: &lsp::Range, inner: &lsp::Range) -> bool {
    (outer.start.line, outer.start.character) <= (inner.start.line, inner.start.character)
        && (inner.end.line, inner.end.character) <= (outer.end.line, outer.end.character)
}
