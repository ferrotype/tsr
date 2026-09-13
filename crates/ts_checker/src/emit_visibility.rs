//! Declaration visibility is checker-owned state. Marking an alias changes only
//! the resolver's links, never the published source tree.
use crate::{CheckerState, Error, LinkStore};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_printer::emit_resolver::SymbolAccessibilityResult;

#[derive(Default)]
pub(crate) struct EmitState {
    pub(crate) visible: LinkStore<NodeId, Option<bool>>,
    pub(crate) aliases_marked: LinkStore<NodeId, bool>,
}

impl CheckerState {
    pub(crate) fn emit_parent(&self, node: NodeId, count: usize) -> Result<Option<NodeId>, Error> {
        let mut result = Some(node);
        for _ in 0..count {
            result = result
                .map(|n| {
                    self.ast(n)?
                        .node(n)
                        .map(|n| n.parent())
                        .map_err(Error::from)
                })
                .transpose()?
                .flatten();
        }
        Ok(result)
    }

    pub(crate) fn emit_parse_node(&self, node: NodeId) -> Result<bool, Error> {
        Ok(self.ast(node)?.node(node)?.flags() & nf::SYNTHESIZED == 0)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.isDeclarationVisible
    pub(crate) fn emit_declaration_visible(&mut self, node: Option<NodeId>) -> Result<bool, Error> {
        let Some(node) = node else {
            return Ok(false);
        };
        if !self.emit_parse_node(node)? {
            return Ok(false);
        }
        if let Some(Some(value)) = self.emit.visible.try_get(node) {
            return Ok(*value);
        }
        let value = self.determine_declaration_visible(node)?;
        *self.emit.visible.get_or_default(node) = Some(value);
        Ok(value)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.determineIfDeclarationIsVisible
    fn determine_declaration_visible(&mut self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let parent = read.parent();
        match read.kind().known() {
            Some(K::JSDocCallbackTag | K::JSDocTypedefTag) => match self.emit_parent(node, 3)? {
                Some(n) => Ok(self.ast(n)?.node(n)?.kind() == K::SourceFile),
                None => Ok(false),
            },
            Some(K::BindingElement) => self.emit_declaration_visible(self.emit_parent(node, 2)?),
            Some(
                K::VariableDeclaration
                | K::ModuleDeclaration
                | K::ClassDeclaration
                | K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::FunctionDeclaration
                | K::EnumDeclaration
                | K::ImportEqualsDeclaration,
            ) => {
                if read.kind() == K::VariableDeclaration {
                    if let Some(name) = read.name() {
                        let name = view.node(name)?;
                        if ts_ast::utilities::is_binding_pattern(&name)
                            && name.elements(view)?.is_empty()
                        {
                            return Ok(false);
                        }
                    }
                }
                if ts_ast::is_ambient_module(view, node)?
                    && ts_ast::is_module_augmentation_external(view, node)?
                    || parent.is_some()
                        && ts_ast::is_implicitly_exported_js_doc_declaration(view, node)?
                {
                    return Ok(true);
                }
                let container = ts_ast::get_declaration_container(view, node)?
                    .ok_or(Error::MissingLink("declaration visibility container"))?;
                let parent_read = view.node(container)?;
                if ts_ast::utilities::get_combined_modifier_flags(view, node)? & mf::EXPORT == 0
                    && !(read.kind() != K::ImportEqualsDeclaration
                        && parent_read.kind() != K::SourceFile
                        && parent_read.flags() & nf::AMBIENT != 0)
                {
                    return Ok(ts_ast::utilities_middle::is_global_source_file(
                        view, container,
                    )?);
                }
                self.emit_declaration_visible(Some(container))
            }
            Some(
                K::PropertyDeclaration
                | K::PropertySignature
                | K::GetAccessor
                | K::SetAccessor
                | K::MethodDeclaration
                | K::MethodSignature,
            ) => {
                if self.effective_declaration_flags(node, mf::PRIVATE | mf::PROTECTED)? != 0 {
                    return Ok(false);
                }
                self.emit_declaration_visible(parent)
            }
            Some(
                K::Constructor
                | K::ConstructSignature
                | K::CallSignature
                | K::IndexSignature
                | K::Parameter
                | K::ModuleBlock
                | K::FunctionType
                | K::ConstructorType
                | K::TypeLiteral
                | K::TypeReference
                | K::ArrayType
                | K::TupleType
                | K::UnionType
                | K::IntersectionType
                | K::ParenthesizedType
                | K::NamedTupleMember,
            ) => self.emit_declaration_visible(parent),
            Some(K::TypeParameter | K::SourceFile | K::NamespaceExportDeclaration) => Ok(true),
            Some(K::ExportSpecifier) => {
                let Some(declaration) = self.emit_parent(node, 2)? else {
                    return Ok(false);
                };
                let read = self.ast(declaration)?.node(declaration)?;
                if read.kind() == K::ExportDeclaration
                    && read
                        .data_source()
                        .as_export_declaration()
                        .ok_or(ts_arena::Error::InvalidGraph)?
                        .module_specifier()
                        .is_none()
                {
                    let parent = read.parent();
                    return self.emit_declaration_visible(parent);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/emitresolver.go:isCommonJSModuleExports
    pub(crate) fn emit_common_js_exports(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        if view.node(node)?.kind() != K::BinaryExpression {
            return Ok(false);
        }
        let Some(statement) = self.emit_parent(node, 1)? else {
            return Ok(false);
        };
        if view.node(statement)?.kind() != K::ExpressionStatement {
            return Ok(false);
        }
        let Some(source) = self.emit_parent(node, 2)? else {
            return Ok(false);
        };
        if view.node(source)?.kind() != K::SourceFile
            || view
                .source_file(source)?
                .common_js_module_indicator()
                .is_none()
        {
            return Ok(false);
        }
        Ok(matches!(
            ts_ast::get_assignment_declaration_kind(view, node)?,
            ts_ast::JSDeclarationKind::ModuleExports | ts_ast::JSDeclarationKind::ExportsProperty
        ))
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.PrecalculateDeclarationEmitVisibility
    pub(crate) fn emit_precalculate_visibility(&mut self, source: NodeId) -> Result<(), Error> {
        if self
            .emit
            .aliases_marked
            .try_get(source)
            .copied()
            .unwrap_or(false)
        {
            return Ok(());
        }
        *self.emit.aliases_marked.get_or_default(source) = true;
        let result = (|| {
            let mut stack = self.source_children(source)?;
            stack.reverse();
            while let Some(node) = stack.pop() {
                let read = self.ast(node)?.node(node)?;
                let mark = match read.kind().known() {
                    Some(K::BinaryExpression) if self.emit_common_js_exports(node)? => read
                        .data_source()
                        .as_binary_expression()
                        .and_then(|n| n.right()),
                    Some(K::ExportAssignment) => read.expression(),
                    Some(K::ExportSpecifier) => read
                        .data_source()
                        .as_export_specifier()
                        .and_then(|n| n.property_name().or(n.name())),
                    _ => None,
                };
                if let Some(mark) = mark {
                    if self.ast(node)?.node(node)?.kind() == K::ExportSpecifier
                        || self.ast(mark)?.node(mark)?.kind() == K::Identifier
                    {
                        self.emit_mark_linked_aliases(mark)?;
                    }
                }
                let children = self.source_children(node)?;
                stack.extend(children.into_iter().rev());
            }
            Ok(())
        })();
        if result.is_err() {
            *self.emit.aliases_marked.get_or_default(source) = false;
        }
        result
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.markLinkedAliases
    fn emit_mark_linked_aliases(&mut self, node: NodeId) -> Result<(), Error> {
        let Some(parent) = self.emit_parent(node, 1)? else {
            return Ok(());
        };
        let meaning = sf::VALUE | sf::TYPE | sf::NAMESPACE | sf::ALIAS;
        let mut symbol = if self.ast(node)?.node(node)?.kind() != K::StringLiteral
            && (self.ast(parent)?.node(parent)?.kind() == K::ExportAssignment
                || self.emit_common_js_exports(parent)?)
        {
            let name = self.ast(node)?.node_text(node)?.into_js_string();
            self.resolve_name(Some(node), name.as_bytes(), meaning, None, false)?
        } else if self.ast(parent)?.node(parent)?.kind() == K::ExportSpecifier {
            // The normal alias resolver also marks type-only aliases. Resolve its
            // result here, because this native call requests dontResolveAlias=false.
            match self.target_of_alias_declaration(parent)? {
                Some(s) if self.symbol(s)?.flags() & sf::ALIAS != 0 => Some(self.resolve_alias(s)?),
                other => other,
            }
        } else {
            None
        };
        let mut visited = crate::types::Set::default();
        while let Some(current) = symbol {
            if !visited.insert(current) {
                break;
            }
            symbol = None;
            let declarations: Vec<_> = self
                .symbol_declarations(current)?
                .iter()
                .flatten()
                .collect();
            for declaration in declarations {
                *self.emit.visible.get_or_default(declaration) = Some(true);
                let read = self.ast(declaration)?.node(declaration)?;
                if let Some(data) = read.data_source().as_import_equals_declaration() {
                    if let Some(reference) = data.module_reference() {
                        if self.ast(reference)?.node(reference)?.kind()
                            != K::ExternalModuleReference
                        {
                            let first = ts_ast::utilities_middle::get_first_identifier(
                                self.ast(reference)?,
                                reference,
                            )?;
                            let name = self.ast(first)?.node_text(first)?.into_js_string();
                            symbol = self.resolve_name(
                                Some(declaration),
                                name.as_bytes(),
                                meaning,
                                None,
                                false,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/utilities.go:getAnyImportSyntax
    fn emit_any_import_syntax(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let levels = match self.ast(node)?.node(node)?.kind().known() {
            Some(K::ImportEqualsDeclaration) => 0,
            Some(K::ImportClause) => 1,
            Some(K::NamespaceImport) => 2,
            Some(K::ImportSpecifier) => 3,
            _ => return Ok(None),
        };
        self.emit_parent(node, levels)
    }

    fn emit_unexported_in_visible_parent(&mut self, node: NodeId) -> Result<bool, Error> {
        if ts_ast::utilities::has_syntactic_modifier(self.ast(node)?, node, mf::EXPORT)? {
            return Ok(false);
        }
        self.emit_declaration_visible(self.emit_parent(node, 1)?)
    }

    // port: tsc/internal/checker/emitresolver.go:EmitResolver.hasVisibleDeclarations
    pub(crate) fn emit_visible_declarations(
        &mut self,
        symbol: SymbolId,
        compute_aliases: bool,
    ) -> Result<Option<SymbolAccessibilityResult>, Error> {
        let declarations: Vec<_> = self.symbol_declarations(symbol)?.iter().flatten().collect();
        let mut aliases = crate::types::Map::default();
        for declaration in declarations {
            let kind = self.ast(declaration)?.node(declaration)?.kind();
            if kind == K::Identifier || self.emit_declaration_visible(Some(declaration))? {
                continue;
            }
            let mut statement = None;
            if let Some(import) = self.emit_any_import_syntax(declaration)? {
                if self.emit_unexported_in_visible_parent(import)? {
                    statement = Some(import);
                }
            }
            if statement.is_none() && kind == K::VariableDeclaration {
                if let Some(variable) = self.emit_parent(declaration, 2)? {
                    if self.ast(variable)?.node(variable)?.kind() == K::VariableStatement
                        && self.emit_unexported_in_visible_parent(variable)?
                    {
                        statement = Some(variable);
                    }
                }
            }
            if statement.is_none()
                && ts_ast::utilities_middle::is_late_visibility_painted_statement(
                    &self.ast(declaration)?.node(declaration)?,
                )
                && self.emit_unexported_in_visible_parent(declaration)?
            {
                statement = Some(declaration);
            }
            if statement.is_none() && kind == K::BindingElement {
                let flags = self.symbol(symbol)?.flags();
                if flags & sf::ALIAS != 0
                    && self.ast(declaration)?.node(declaration)?.flags() & nf::JAVA_SCRIPT_FILE != 0
                {
                    if let (Some(variable), Some(container)) = (
                        self.emit_parent(declaration, 2)?,
                        self.emit_parent(declaration, 4)?,
                    ) {
                        if self.ast(variable)?.node(variable)?.kind() == K::VariableDeclaration
                            && self.ast(container)?.node(container)?.kind() == K::VariableStatement
                            && self.emit_unexported_in_visible_parent(container)?
                        {
                            statement = Some(container);
                        }
                    }
                }
                if statement.is_none() && flags & sf::BLOCK_SCOPED_VARIABLE != 0 {
                    let root = ts_ast::utilities::walk_up_binding_elements_and_patterns(
                        self.ast(declaration)?,
                        declaration,
                    )?
                    .ok_or(Error::MissingLink("binding declaration root"))?;
                    if self.ast(root)?.node(root)?.kind() == K::Parameter {
                        return Ok(None);
                    }
                    let Some(variable) = self.emit_parent(root, 2)? else {
                        return Ok(None);
                    };
                    if self.ast(variable)?.node(variable)?.kind() != K::VariableStatement {
                        return Ok(None);
                    }
                    if ts_ast::utilities::has_syntactic_modifier(
                        self.ast(variable)?,
                        variable,
                        mf::EXPORT,
                    )? {
                        continue;
                    }
                    if !self.emit_declaration_visible(self.emit_parent(variable, 1)?)? {
                        return Ok(None);
                    }
                    statement = Some(variable);
                }
            }
            let Some(statement) = statement else {
                return Ok(None);
            };
            if compute_aliases {
                *self.emit.visible.get_or_default(declaration) = Some(true);
                aliases.insert(declaration, statement);
            }
        }
        let mut result = SymbolAccessibilityResult::accessible();
        result.aliases_to_make_visible = aliases.into_values().collect();
        Ok(Some(result))
    }
}
