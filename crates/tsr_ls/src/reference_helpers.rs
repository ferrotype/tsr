//! Shared symbol relationships for references, implementations and highlights.
use crate::{
    definition::{ancestor, object_literal_element},
    documentation::list,
    meaning,
    references::{EntryKind, SearchState},
    syntax::Syntax,
    Result,
};
use std::collections::HashSet;
use tsr_ast::{
    symbol_flags as sf, utilities as ast, utilities_class as class, utilities_positions as pos,
    AstView, NodeId, SyntaxKind as K,
};
use tsr_checker::{Operation, SymbolRef, TypeRef};

// port: tsc/internal/ls/findallreferences.go:getContextNodeForNodeEntry
pub(crate) fn entry_context(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>> {
    use crate::definition::context_node;
    let n = view.node(node)?;
    if tsr_ast::is_declaration(&n) {
        return context_node(view, node);
    }
    let Some(parent) = n.parent() else {
        return Ok(None);
    };
    let p = view.node(parent)?;
    if !tsr_ast::is_declaration(&p) && p.kind() != K::ExportAssignment {
        if ast::is_in_js_file(Some(&n)) {
            let assignment = if p.kind() == K::BinaryExpression {
                Some(parent)
            } else if ast::is_access_expression(&p) {
                p.parent().filter(|id| {
                    view.node(*id).is_ok_and(|n| {
                        n.data_source()
                            .as_binary_expression()
                            .is_some_and(|d| d.left() == Some(parent))
                    })
                })
            } else {
                None
            };
            if let Some(assignment) = assignment {
                if tsr_ast::get_assignment_declaration_kind(view, assignment)?
                    != tsr_ast::JSDeclarationKind::None
                {
                    return context_node(view, assignment);
                }
            }
        }
        match p.kind().known() {
            Some(K::JsxOpeningElement | K::JsxClosingElement) => return Ok(p.parent()),
            Some(
                K::JsxSelfClosingElement
                | K::LabeledStatement
                | K::BreakStatement
                | K::ContinueStatement,
            ) => return Ok(Some(parent)),
            _ => {}
        }
        if let Some(property) = ancestor(view, Some(node), |id| {
            Ok(view.node(id)?.kind() == K::ComputedPropertyName)
        })? {
            return view
                .node(property)?
                .parent()
                .map(|id| context_node(view, id))
                .transpose()
                .map(Option::flatten);
        }
        return Ok(None);
    }
    if p.name() == Some(node)
        || matches!(p.kind().known(), Some(K::Constructor | K::ExportAssignment))
        || (matches!(
            p.kind().known(),
            Some(K::ImportSpecifier | K::ExportSpecifier | K::BindingElement)
        ) && p.property_name() == Some(node))
        || n.kind() == K::DefaultKeyword
            && ast::has_syntactic_modifier(view, parent, tsr_ast::modifier_flags::EXPORT_DEFAULT)?
    {
        context_node(view, parent)
    } else {
        Ok(None)
    }
}

pub(crate) fn declarations(c: &Operation<'_>, symbol: SymbolRef) -> Result<Vec<NodeId>> {
    Ok(c.symbol_declarations(symbol)?.iter().flatten().collect())
}
pub(crate) fn parent_symbol(c: &Operation<'_>, symbol: SymbolRef) -> Result<Option<SymbolRef>> {
    c.symbol(symbol)?
        .parent()
        .map(|id| c.symbol_ref(id).map_err(Into::into))
        .transpose()
}
pub(crate) fn is_external(c: &Operation<'_>, symbol: SymbolRef) -> Result<bool> {
    Ok(c.symbol(symbol)?.flags() & sf::MODULE != 0
        && c.symbol(symbol)?.name_bytes().first() == Some(&b'"'))
}
// port: tsc/internal/ls/utilities.go:isTypeKeyword
pub(crate) fn type_keyword(kind: K) -> bool {
    matches!(
        kind,
        K::AnyKeyword
            | K::AssertsKeyword
            | K::BigIntKeyword
            | K::BooleanKeyword
            | K::FalseKeyword
            | K::InferKeyword
            | K::KeyOfKeyword
            | K::NeverKeyword
            | K::NullKeyword
            | K::NumberKeyword
            | K::ObjectKeyword
            | K::ReadonlyKeyword
            | K::StringKeyword
            | K::SymbolKeyword
            | K::TypeOfKeyword
            | K::TrueKeyword
            | K::VoidKeyword
            | K::UndefinedKeyword
            | K::UniqueKeyword
            | K::UnknownKeyword
    )
}
// port: tsc/internal/ls/findallreferences.go:getPossibleSymbolReferencePositions
pub(crate) fn positions(
    syntax: &Syntax<'_>,
    text: &[u8],
    container: Option<NodeId>,
) -> Result<Vec<usize>> {
    let mut result = Vec::new();
    if text.is_empty() {
        return Ok(result);
    }
    let source = syntax.file.text().as_bytes();
    let n = syntax.view.node(container.unwrap_or(syntax.source))?;
    // Preserve the pin's initial slice-relative search, including its known
    // Unicode-escape limitation. Subsequent searches use absolute offsets.
    let mut found = source[n.pos() as usize..]
        .windows(text.len())
        .position(|b| b == text);
    while let Some(position) = found.filter(|p| *p < n.end() as usize) {
        let end = position + text.len();
        if (position == 0 || !tsr_scanner::is_identifier_part(i32::from(source[position - 1])))
            && (end == source.len() || !tsr_scanner::is_identifier_part(i32::from(source[end])))
        {
            result.push(position);
        }
        let start = end + 1;
        if start > source.len() {
            break;
        }
        found = source[start..]
            .windows(text.len())
            .position(|b| b == text)
            .map(|p| p + start);
    }
    Ok(result)
}
pub(crate) fn candidates(
    syntax: &mut Syntax<'_>,
    text: &[u8],
    container: Option<NodeId>,
) -> Result<Vec<NodeId>> {
    let mut result = Vec::new();
    for position in positions(syntax, text, container)? {
        let node = syntax.nav().get_touching_property_name(position as i64)?;
        if node != syntax.source {
            result.push(node);
        }
    }
    Ok(result)
}
// port: tsc/internal/ls/utilities.go:isLiteralNameOfPropertyDeclarationOrIndexAccess
pub(crate) fn literal_property(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    let Some(parent) = n.parent() else {
        return Ok(false);
    };
    let p = view.node(parent)?;
    Ok(match p.kind().known() {
        Some(
            K::PropertyDeclaration
            | K::PropertySignature
            | K::PropertyAssignment
            | K::EnumMember
            | K::MethodDeclaration
            | K::MethodSignature
            | K::GetAccessor
            | K::SetAccessor
            | K::ModuleDeclaration,
        ) => p.name() == Some(node),
        Some(K::ElementAccessExpression) => {
            p.data_source()
                .as_element_access_expression()
                .unwrap()
                .argument_expression()
                == Some(node)
        }
        Some(K::ComputedPropertyName) => true,
        Some(K::LiteralType) => p
            .parent()
            .map(|p| view.node(p).map(|p| p.kind() == K::IndexedAccessType))
            .transpose()?
            .unwrap_or(false),
        _ => false,
    })
}
// port: tsc/internal/ls/findallreferences.go:isValidReferencePosition
pub(crate) fn valid_reference(view: AstView<'_>, node: NodeId, text: &[u8]) -> Result<bool> {
    let n = view.node(node)?;
    Ok(match n.kind().known() {
        Some(K::PrivateIdentifier | K::Identifier) => view.node_text(node)?.len() == text.len(),
        Some(K::DefaultKeyword) => text.len() == 7,
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
            if view.node_text(node)?.len() != text.len() {
                return Ok(false);
            }
            if literal_property(view, node)? {
                return Ok(true);
            }
            let Some(p) = n.parent() else {
                return Ok(false);
            };
            let p = view.node(p)?;
            matches!(
                p.kind().known(),
                Some(K::ExternalModuleReference | K::ImportSpecifier | K::ExportSpecifier)
            ) || (p.kind() == K::CallExpression
                && tsr_ast::is_bindable_object_define_property_call(view, n.parent().unwrap())?
                && list(view, p.argument_list())?.get(1) == Some(&node))
        }
        Some(K::NumericLiteral) => {
            literal_property(view, node)? && view.node_text(node)?.len() == text.len()
        }
        _ => false,
    })
}
// port: tsc/internal/ls/utilities.go:isObjectBindingElementWithoutPropertyName
pub(crate) fn binding_without_property(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    Ok(n.kind() == K::BindingElement
        && n.property_name().is_none()
        && n.parent()
            .map(|p| view.node(p).map(|p| p.kind() == K::ObjectBindingPattern))
            .transpose()?
            .unwrap_or(false)
        && n.name()
            .map(|p| view.node(p).map(|p| p.kind() == K::Identifier))
            .transpose()?
            .unwrap_or(false))
}
// port: tsc/internal/ls/utilities.go:getAllSuperTypeNodes
pub(crate) fn super_types(view: AstView<'_>, node: NodeId) -> Result<Vec<NodeId>> {
    let n = view.node(node)?;
    if n.kind() == K::InterfaceDeclaration {
        return Ok(class::get_heritage_elements(view, node, K::ExtendsKeyword)?);
    }
    if ast::is_class_like(&n) {
        let mut result = class::get_class_extends_heritage_element(view, node)?
            .into_iter()
            .collect::<Vec<_>>();
        result.extend(class::get_implements_heritage_clause_elements(view, node)?);
        return Ok(result);
    }
    Ok(Vec::new())
}
// port: tsc/internal/ls/utilities.go:isImplementation
pub(crate) fn implementation(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    if n.flags() & tsr_ast::node_flags::AMBIENT != 0 {
        return Ok(!matches!(
            n.kind().known(),
            Some(K::InterfaceDeclaration | K::TypeAliasDeclaration)
        ));
    }
    if tsr_ast::utilities_middle::is_variable_like(&n) {
        return Ok(tsr_ast::utilities_middle::has_initializer(&n));
    }
    if ast::is_function_like_declaration(Some(&n)) {
        return Ok(n.body().is_some());
    }
    Ok(ast::is_class_like(&n)
        || matches!(
            n.kind().known(),
            Some(K::ModuleDeclaration | K::EnumDeclaration)
        ))
}
// port: tsc/internal/ls/utilities.go:isImplementationExpression
pub(crate) fn implementation_expression(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let node = tsr_ast::skip_parentheses(view, node)?;
    Ok(matches!(
        view.node(node)?.kind().known(),
        Some(
            K::ArrowFunction
                | K::FunctionExpression
                | K::ObjectLiteralExpression
                | K::ClassExpression
                | K::ArrayLiteralExpression
        )
    ))
}
// port: tsc/internal/ls/utilities.go:getContainingNodeIfInHeritageClause
pub(crate) fn heritage_container(view: AstView<'_>, mut node: NodeId) -> Result<Option<NodeId>> {
    loop {
        let n = view.node(node)?;
        if matches!(
            n.kind().known(),
            Some(K::Identifier | K::QualifiedName | K::PropertyAccessExpression)
        ) {
            if let Some(parent) = n.parent() {
                node = parent;
                continue;
            }
        }
        if matches!(
            n.kind().known(),
            Some(K::ExpressionWithTypeArguments | K::TypeReference)
        ) {
            if let Some(p) = n.parent() {
                let p = view.node(p)?;
                if p.kind() == K::HeritageClause {
                    if let Some(parent) = p.parent() {
                        let n = view.node(parent)?;
                        if ast::is_class_like(&n) || n.kind() == K::InterfaceDeclaration {
                            return Ok(Some(parent));
                        }
                    }
                }
            }
        }
        return Ok(None);
    }
}
// port: tsc/internal/ls/utilities.go:getContainerNode
pub(crate) fn container(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>> {
    ancestor(view, view.node(node)?.parent(), |id| {
        Ok(matches!(
            view.node(id)?.kind().known(),
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
        ))
    })
}
impl SearchState<'_, '_, '_> {
    pub(crate) fn source_of(&self, node: NodeId) -> Result<NodeId> {
        Ok(
            ast::get_source_file_of_node(self.l.view(node)?, Some(node))?
                .ok_or(tsr_arena::Error::InvalidGraph)?,
        )
    }
    pub(crate) fn static_symbol(&self, symbol: SymbolRef) -> Result<bool> {
        let Some(node) = self.c.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        Ok(ast::has_syntactic_modifier(
            self.l.view(node)?,
            node,
            tsr_ast::modifier_flags::STATIC,
        )?)
    }
    pub(crate) fn global_exports(&self, node: Option<NodeId>) -> Result<bool> {
        let Some(node) = node else { return Ok(false) };
        if self.c.node(node)?.kind() != K::SourceFile {
            return Ok(false);
        }
        Ok(self
            .l
            .program
            .file_of_node(node)
            .ok_or(tsr_arena::Error::WrongOwner)?
            .bound()
            .view()
            .result()
            .global_exports()
            .is_some())
    }
    // port: tsc/internal/ls/findallreferences.go:getSymbolScope
    pub(crate) fn symbol_scope(&self, symbol: SymbolRef) -> Result<Option<NodeId>> {
        let s = self.c.symbol(symbol)?;
        if let Some(value) = s.value_declaration() {
            if matches!(
                self.c.node(value)?.kind().known(),
                Some(K::FunctionExpression | K::ClassExpression)
            ) {
                return Ok(Some(value));
            }
        }
        let decls = declarations(self.c, symbol)?;
        if decls.is_empty() {
            return Ok(None);
        }
        if s.flags() & (sf::PROPERTY | sf::METHOD) != 0 {
            for &decl in &decls {
                let view = self.l.view(decl)?;
                if ast::has_syntactic_modifier(view, decl, tsr_ast::modifier_flags::PRIVATE)?
                    || view.node(decl)?.name().is_some_and(|id| {
                        view.node(id)
                            .is_ok_and(|n| n.kind() == K::PrivateIdentifier)
                    })
                {
                    return ancestor(view, Some(decl), |id| {
                        Ok(view.node(id)?.kind() == K::ClassDeclaration)
                    });
                }
            }
            return Ok(None);
        }
        for &decl in &decls {
            if binding_without_property(self.l.view(decl)?, decl)? {
                return Ok(None);
            }
        }
        let exposed = s.parent().is_some() && s.flags() & sf::TYPE_PARAMETER == 0;
        if exposed {
            let parent = self.c.symbol_ref(s.parent().unwrap())?;
            if !is_external(self.c, parent)?
                || self.global_exports(self.c.symbol(parent)?.value_declaration())?
            {
                return Ok(None);
            }
        }
        let mut scope = None;
        for decl in decls {
            let view = self.l.view(decl)?;
            let Some(current) = container(view, decl)? else {
                return Ok(None);
            };
            if scope.is_some_and(|s| s != current) {
                return Ok(None);
            }
            if view.node(current)?.kind() == K::SourceFile {
                let file = view.source_file(current)?;
                if file.external_module_indicator.is_none()
                    && file.common_js_module_indicator().is_none()
                {
                    return Ok(None);
                }
            }
            scope = Some(current);
        }
        if exposed {
            return scope.map(|n| self.source_of(n)).transpose();
        }
        Ok(scope)
    }
    // port: tsc/internal/ls/utilities.go:getIntersectingMeaningFromDeclarations
    pub(crate) fn search_meaning(&self, node: Option<NodeId>, symbol: SymbolRef) -> Result<i32> {
        let Some(node) = node else {
            return Ok(pos::semantic_meaning::ALL);
        };
        let mut result = meaning::meaning(self.l.view(node)?, node, self.c)?;
        let decls = declarations(self.c, symbol)?;
        loop {
            let previous = result;
            for &decl in &decls {
                let m = pos::get_meaning_from_declaration(self.l.view(decl)?, decl)?;
                if m & result != 0 {
                    result |= m;
                }
            }
            if previous == result {
                break;
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/utilities.go:getPropertySymbolFromBindingElement
    pub(crate) fn binding_property(&mut self, element: NodeId) -> Result<Option<SymbolRef>> {
        let view = self.l.view(element)?;
        let n = view.node(element)?;
        let parent = n.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
        let name = view.node_text(n.name().ok_or(tsr_arena::Error::InvalidGraph)?)?;
        let ty = self.c.get_type_at_location(parent)?;
        Ok(self.c.get_property_of_type(ty, name.as_bytes())?)
    }
    // port: tsc/internal/ls/utilities.go:getPropertySymbolsFromBaseTypes
    pub(crate) fn base_properties(
        &mut self,
        symbol: SymbolRef,
        name: &[u8],
    ) -> Result<Vec<SymbolRef>> {
        enum Task {
            Symbol(SymbolRef),
            Type(NodeId),
        }
        let mut tasks = vec![Task::Symbol(symbol)];
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Symbol(s) => {
                    if self.c.symbol(s)?.flags() & (sf::CLASS | sf::INTERFACE) == 0
                        || !seen.insert(s)
                    {
                        continue;
                    }
                    let mut types = Vec::new();
                    for decl in declarations(self.c, s)? {
                        types.extend(super_types(self.l.view(decl)?, decl)?);
                    }
                    tasks.extend(types.into_iter().rev().map(Task::Type));
                }
                Task::Type(node) => {
                    let ty = self.c.get_type_at_location(node)?;
                    if let Some(s) = self.c.type_symbol(ty)? {
                        if let Some(property) = self.c.get_property_of_type(ty, name)? {
                            result.extend(self.c.get_root_symbols(property)?);
                        }
                        tasks.push(Task::Symbol(self.c.symbol_ref(s)?));
                    }
                }
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/utilities.go:getLocalSymbolForExportSpecifier
    pub(crate) fn export_local(
        &mut self,
        node: NodeId,
        symbol: SymbolRef,
        export: NodeId,
    ) -> Result<SymbolRef> {
        let view = self.l.view(node)?;
        let n = view.node(export)?;
        let is_alias = if let Some(property) = n.property_name() {
            property == node
        } else {
            let p = view
                .node(n.parent().ok_or(tsr_arena::Error::InvalidGraph)?)?
                .parent()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            view.node(p)?.module_specifier().is_none()
        };
        if is_alias {
            Ok(self
                .c
                .get_export_specifier_local_target_symbol(export)?
                .unwrap_or(symbol))
        } else {
            Ok(symbol)
        }
    }
    pub(crate) fn related_candidates(
        &mut self,
        symbol: SymbolRef,
        node: NodeId,
        bases: bool,
        parents: &[SymbolRef],
        wanted: Option<&[SymbolRef]>,
    ) -> Result<Vec<(SymbolRef, SymbolRef, EntryKind)>> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        let mut result = Vec::new();
        let populate_rename = wanted.is_none() && self.options.rename;
        let only_at_location = if wanted.is_none() {
            !(self.options.rename && self.options.aliases)
        } else {
            !self.options.rename || self.options.aliases
        };
        if let Some(element) = object_literal_element(view, node)? {
            if populate_rename {
                if let Some(value) = self.c.get_shorthand_assignment_value_symbol(n.parent())? {
                    result.push((value, value, EntryKind::LocalFoundProperty));
                    return Ok(result);
                }
            }
            if let Some(context) = self
                .c
                .get_contextual_type(view.node(element)?.parent().unwrap(), 0)?
            {
                for sym in self
                    .c
                    .get_property_symbols_from_contextual_type(element, context, true)?
                {
                    if self.related_roots(
                        sym,
                        bases,
                        parents,
                        wanted,
                        &mut result,
                        EntryKind::PropertyFoundLocal,
                    )? {
                        return Ok(result);
                    }
                }
            }
            if let Some(sym) = self
                .c
                .get_property_symbol_of_destructuring_assignment(node)?
            {
                if push_related(&mut result, wanted, sym, sym, EntryKind::PropertyFoundLocal) {
                    return Ok(result);
                }
            }
            if let Some(p) = n.parent() {
                if let Some(sym) = self.c.get_shorthand_assignment_value_symbol(Some(p))? {
                    if push_related(&mut result, wanted, sym, sym, EntryKind::LocalFoundProperty) {
                        return Ok(result);
                    }
                }
            }
        }
        if let Some(p) = n.parent() {
            if view.node(p)?.kind() == K::NamespaceExportDeclaration
                && self.c.symbol(symbol)?.flags() & sf::ALIAS != 0
            {
                let alias = self.c.get_aliased_symbol(symbol)?;
                if push_related(&mut result, wanted, alias, alias, EntryKind::Node) {
                    return Ok(result);
                }
            }
        }
        if self.related_roots(symbol, bases, parents, wanted, &mut result, EntryKind::Node)? {
            return Ok(result);
        }
        if let Some(decl) = self.c.symbol(symbol)?.value_declaration() {
            let view = self.l.view(decl)?;
            if view.node(decl)?.kind() == K::Parameter
                && ast::is_parameter_property_declaration(
                    view,
                    decl,
                    view.node(decl)?
                        .parent()
                        .ok_or(tsr_arena::Error::InvalidGraph)?,
                )?
            {
                let name = self.c.symbol(symbol)?.name_bytes().to_vec();
                let (param, member) = self
                    .c
                    .get_symbols_of_parameter_property_declaration(decl, &name)?;
                let other = if self.c.symbol(symbol)?.flags() & sf::FUNCTION_SCOPED_VARIABLE != 0 {
                    member
                } else {
                    param
                };
                self.related_roots(other, bases, parents, wanted, &mut result, EntryKind::Node)?;
                return Ok(result);
            }
        }
        for decl in declarations(self.c, symbol)? {
            if self.c.node(decl)?.kind() == K::ExportSpecifier
                && (!populate_rename || self.c.node(decl)?.property_name().is_none())
            {
                if let Some(local) = self.c.get_export_specifier_local_target_symbol(decl)? {
                    if push_related(&mut result, wanted, local, local, EntryKind::Node) {
                        return Ok(result);
                    }
                }
                break;
            }
        }
        let element = if !populate_rename && only_at_location {
            n.parent()
                .filter(|p| binding_without_property(view, *p).unwrap_or(false))
        } else if !populate_rename || only_at_location {
            let mut found = None;
            for decl in declarations(self.c, symbol)? {
                if binding_without_property(self.l.view(decl)?, decl)? {
                    found = Some(decl);
                    break;
                }
            }
            found
        } else {
            None
        };
        if let Some(element) = element {
            if let Some(sym) = self.binding_property(element)? {
                self.related_roots(
                    sym,
                    bases,
                    parents,
                    wanted,
                    &mut result,
                    EntryKind::PropertyFoundLocal,
                )?;
            }
        }
        Ok(result)
    }
    fn related_roots(
        &mut self,
        symbol: SymbolRef,
        bases: bool,
        parents: &[SymbolRef],
        wanted: Option<&[SymbolRef]>,
        out: &mut Vec<(SymbolRef, SymbolRef, EntryKind)>,
        kind: EntryKind,
    ) -> Result<bool> {
        for root in self.c.get_root_symbols(symbol)? {
            let found =
                if self.c.symbol(symbol)?.check_flags() & tsr_ast::check_flags::SYNTHETIC == 0 {
                    root
                } else {
                    symbol
                };
            if push_related(out, wanted, root, found, kind) {
                return Ok(true);
            }
            if bases {
                if let Some(parent) = parent_symbol(self.c, root)? {
                    if !parents.is_empty() {
                        let mut allowed = false;
                        for &base in parents {
                            if self.inherits(parent, base)? {
                                allowed = true;
                                break;
                            }
                        }
                        if !allowed {
                            continue;
                        }
                    }
                    let name = self.c.symbol(root)?.name_bytes().to_vec();
                    for base in self.base_properties(parent, &name)? {
                        if self.static_symbol(symbol)? == self.static_symbol(base)?
                            && push_related(out, wanted, base, found, kind)
                        {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }
    pub(crate) fn contextual_literal(&mut self, node: NodeId) -> Result<Option<TypeRef>> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        if n.flags() & tsr_ast::node_flags::JS_DOC != 0
            && n.flags() & tsr_ast::node_flags::JAVA_SCRIPT_FILE == 0
        {
            return Ok(None);
        }
        let mut parent = n.parent();
        while let Some(id) = parent {
            if view.node(id)?.kind() == K::ParenthesizedExpression {
                parent = view.node(id)?.parent();
            } else {
                break;
            }
        }
        let mut context = None;
        if let Some(parent) = parent {
            let p = view.node(parent)?;
            match p.kind().known() {
                Some(K::NewExpression) => context = self.c.get_contextual_type(parent, 0)?,
                Some(K::BinaryExpression) => {
                    let d = p.data_source().as_binary_expression().unwrap();
                    if d.operator_token()
                        .map(|id| {
                            view.node(id).map(|n| {
                                matches!(
                                    n.kind().known(),
                                    Some(
                                        K::EqualsEqualsToken
                                            | K::ExclamationEqualsToken
                                            | K::EqualsEqualsEqualsToken
                                            | K::ExclamationEqualsEqualsToken
                                    )
                                )
                            })
                        })
                        .transpose()?
                        .unwrap_or(false)
                    {
                        if let Some(other) = if d.right() == Some(node) {
                            d.left()
                        } else {
                            d.right()
                        } {
                            context = Some(self.c.get_type_at_location(other)?);
                        }
                    }
                }
                Some(K::CaseClause) => {
                    if let Some(block) = p.parent() {
                        if let Some(switch) = view.node(block)?.parent() {
                            if let Some(expression) = view.node(switch)?.expression() {
                                context = Some(self.c.get_type_at_location(expression)?);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if context.is_none() {
            context = self.c.get_contextual_type(node, 0)?;
        }
        if context.is_some() {
            return Ok(context);
        }
        let mut last = None;
        let mut current = Some(node);
        while let Some(id) = current {
            let n = view.node(id)?;
            if ast::is_type_node(&n) {
                last = Some(id);
            }
            let Some(parent) = n.parent() else { break };
            let p = view.node(parent)?;
            if p.kind() != K::QualifiedName && !ast::is_type_node(&p) && !ast::is_type_element(&p) {
                break;
            }
            current = Some(parent);
        }
        last.map(|n| self.c.get_type_at_location(n).map_err(Into::into))
            .transpose()
    }
}

// Finding a reference must stop at the first related symbol. Continuing through
// contextual/base symbols would unnecessarily resolve types after a direct hit.
fn push_related(
    out: &mut Vec<(SymbolRef, SymbolRef, EntryKind)>,
    wanted: Option<&[SymbolRef]>,
    test: SymbolRef,
    found: SymbolRef,
    kind: EntryKind,
) -> bool {
    if let Some(wanted) = wanted {
        if wanted.contains(&test) {
            out.push((test, found, kind));
            return true;
        }
    } else {
        out.push((test, found, kind));
    }
    false
}
