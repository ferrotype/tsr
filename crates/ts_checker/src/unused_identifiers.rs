//! Unused declaration diagnostics (`checkUnusedIdentifiers` and its helpers in
//! `tsc/internal/checker/checker.go`). Reference kinds are recorded by the name
//! resolver (`symbolReferenced`) and by `markPropertyAsReferenced` into
//! `query.references`; the pass itself runs once per source file after the type
//! check, as `checkSourceFile` does in Go.

use crate::{CheckerState, Error};
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    modifier_flags as mf, node_flags as nf, symbol_flags as sf, Diagnostic, SymbolFlags,
    SyntaxKind as K,
};
use ts_core::TextRange;
use ts_diagnostics as d;
use ts_jsstring::JsString;

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

/// `UnusedKind` in checker.go: which option decides between error and suggestion.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UnusedKind {
    Local,
    Parameter,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.registerForUnusedIdentifiersCheck
    pub(crate) fn register_for_unused_identifiers_check(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let file = required(
            ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?,
            "unused identifier source file",
        )?;
        self.query
            .identifier_check_nodes
            .entry(file)
            .or_default()
            .push(node);
        Ok(())
    }

    /// Registers when the node owns a locals table (`node.Locals() != nil`).
    pub(crate) fn register_locals_for_unused_check(&mut self, node: NodeId) -> Result<(), Error> {
        if self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|binding| binding.locals)
            .is_some()
        {
            self.register_for_unused_identifiers_check(node)?;
        }
        Ok(())
    }

    fn reference_kinds(&self, symbol: SymbolId) -> SymbolFlags {
        self.query.references.try_get(symbol).copied().unwrap_or(0)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkUnusedIdentifiers
    pub(crate) fn check_unused_identifiers(&mut self, nodes: Vec<NodeId>) -> Result<(), Error> {
        for node in nodes {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::ClassDeclaration | K::ClassExpression) => {
                    self.check_unused_class_members(node)?;
                    self.check_unused_type_parameters(node)?;
                }
                Some(
                    K::SourceFile
                    | K::ModuleDeclaration
                    | K::Block
                    | K::CaseBlock
                    | K::ForStatement
                    | K::ForInStatement
                    | K::ForOfStatement,
                ) => self.check_unused_locals_and_parameters(node)?,
                Some(
                    K::Constructor
                    | K::FunctionExpression
                    | K::FunctionDeclaration
                    | K::ArrowFunction
                    | K::MethodDeclaration
                    | K::GetAccessor
                    | K::SetAccessor,
                ) => {
                    // Only report unused parameters on the implementation, not overloads.
                    if read.body().is_some() {
                        self.check_unused_locals_and_parameters(node)?;
                    }
                    self.check_unused_type_parameters(node)?;
                }
                Some(
                    K::MethodSignature
                    | K::CallSignature
                    | K::ConstructSignature
                    | K::FunctionType
                    | K::ConstructorType
                    | K::TypeAliasDeclaration
                    | K::JSTypeAliasDeclaration
                    | K::InterfaceDeclaration,
                ) => self.check_unused_type_parameters(node)?,
                Some(K::InferType) => self.check_unused_infer_type_parameter(node)?,
                _ => {
                    return Err(Error::Unsupported(
                        "checkUnusedIdentifiers: unhandled registered node kind",
                    ))
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isReferenced
    fn is_referenced(&self, symbol: SymbolId) -> bool {
        self.reference_kinds(symbol) != 0
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedVariable
    fn report_unused_variable(
        &mut self,
        mut location: NodeId,
        diagnostic: Diagnostic,
    ) -> Result<(), Error> {
        loop {
            let read = self.ast(location)?.node(location)?;
            if read.kind() == K::BindingElement || self.is_binding_pattern(location)? {
                location = required(read.parent(), "binding element parent")?;
            } else {
                break;
            }
        }
        let kind = if self.ast(location)?.node(location)?.kind() == K::Parameter {
            UnusedKind::Parameter
        } else {
            UnusedKind::Local
        };
        self.report_unused(location, kind, diagnostic)
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnused
    fn report_unused(
        &mut self,
        location: NodeId,
        kind: UnusedKind,
        mut diagnostic: Diagnostic,
    ) -> Result<(), Error> {
        let flags = self.ast(location)?.node(location)?.flags();
        if flags & (nf::AMBIENT | nf::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR) != 0 {
            return Ok(());
        }
        if self.unused_is_error(kind)? {
            self.add_diagnostic(diagnostic)?;
        } else {
            diagnostic.category = ts_diagnostics::Category::Suggestion as i32;
            self.add_suggestion_diagnostic(diagnostic)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.unusedIsError
    fn unused_is_error(&self, kind: UnusedKind) -> Result<bool, Error> {
        let options = self.program()?.host.options();
        Ok(match kind {
            UnusedKind::Local => options.no_unused_locals.is_true(),
            UnusedKind::Parameter => options.no_unused_parameters.is_true(),
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.checkUnusedClassMembers
    fn check_unused_class_members(&mut self, node: NodeId) -> Result<(), Error> {
        for member in self.source_list(node, self.ast(node)?.node(node)?.member_list())? {
            let read = self.ast(member)?.node(member)?;
            match read.kind().known() {
                Some(
                    K::MethodDeclaration | K::PropertyDeclaration | K::GetAccessor | K::SetAccessor,
                ) => {
                    if read.kind() == K::SetAccessor {
                        if let Some(raw) = self.raw_declaration_symbol(member)? {
                            if self.symbol(raw)?.flags() & sf::GET_ACCESSOR != 0 {
                                // Already would have reported an error on the getter.
                                continue;
                            }
                        }
                    }
                    let name = read.name();
                    let member_flags = read.flags();
                    let private = read.modifier_flags(self.ast(member)?)? & mf::PRIVATE != 0
                        || name
                            .map(|name| {
                                Ok::<_, Error>(
                                    self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier,
                                )
                            })
                            .transpose()?
                            .unwrap_or(false);
                    let symbol = required(
                        self.get_symbol_of_declaration(member)?,
                        "class member symbol",
                    )?;
                    if !self.is_referenced(symbol) && private && member_flags & nf::AMBIENT == 0 {
                        let text = self.symbol_to_string(symbol)?;
                        let diagnostic = self.diagnostic_for_node(
                            name,
                            d::X_0_is_declared_but_its_value_is_never_read,
                            vec![text],
                        )?;
                        self.report_unused(member, UnusedKind::Local, diagnostic)?;
                    }
                }
                Some(K::Constructor) => {
                    for parameter in self.source_list(member, read.parameter_list())? {
                        let Some(symbol) = self.raw_declaration_symbol(parameter)? else {
                            continue;
                        };
                        if !self.is_referenced(symbol)
                            && ts_ast::utilities::has_syntactic_modifier(
                                self.ast(parameter)?,
                                parameter,
                                mf::PRIVATE,
                            )?
                        {
                            let text = self.ast_symbol_name(symbol)?;
                            let diagnostic = self.diagnostic_for_node(
                                self.ast(parameter)?.node(parameter)?.name(),
                                d::Property_0_is_declared_but_its_value_is_never_read,
                                vec![text],
                            )?;
                            self.report_unused(parameter, UnusedKind::Local, diagnostic)?;
                        }
                    }
                }
                Some(
                    K::IndexSignature
                    | K::SemicolonClassElement
                    | K::ClassStaticBlockDeclaration
                    | K::JSTypeAliasDeclaration,
                ) => {
                    // Can't be private
                }
                _ => {
                    return Err(Error::Unsupported(
                        "checkUnusedClassMembers: unhandled member kind",
                    ))
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/ast/symbol.go:SymbolName
    pub(crate) fn ast_symbol_name(&self, symbol: SymbolId) -> Result<JsString, Error> {
        let read = self.symbol(symbol)?;
        if let Some(declaration) = read.value_declaration() {
            let view = self.ast(declaration)?;
            if ts_ast::utilities::is_private_identifier_class_element_declaration(
                view,
                declaration,
            )? {
                let name = required(view.node(declaration)?.name(), "private member name")?;
                return Ok(view.node_text(name)?.into_js_string());
            }
        }
        Ok(read.name_to_owned())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkUnusedLocalsAndParameters
    fn check_unused_locals_and_parameters(&mut self, node: NodeId) -> Result<(), Error> {
        let Some(table) = self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|binding| binding.locals)
        else {
            return Ok(());
        };
        let locals: Vec<SymbolId> = self.table(table)?.iter().filter_map(|(_, s)| s).collect();
        let mut variable_parents: Vec<NodeId> = Vec::new();
        let mut import_clauses: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
        for local in locals {
            let read = self.symbol(local)?;
            let flags = read.flags();
            let reference_kinds = self.reference_kinds(local);
            if flags & sf::TYPE_PARAMETER != 0
                && (flags & sf::VARIABLE == 0 || reference_kinds & sf::VARIABLE != 0)
                || flags & sf::TYPE_PARAMETER == 0
                    && (reference_kinds != 0
                        || read.export_symbol().is_some()
                        || flags & sf::MODULE_EXPORTS != 0)
            {
                continue;
            }
            let name = read.name_to_owned();
            for declaration in self
                .symbol_declarations(local)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                let view = self.ast(declaration)?;
                let kind = view.node(declaration)?.kind();
                match kind.known() {
                    Some(K::VariableDeclaration | K::Parameter | K::BindingElement) => {
                        let root = ts_ast::utilities::get_root_declaration(view, declaration)?;
                        let parent =
                            required(view.node(root)?.parent(), "root declaration parent")?;
                        if !variable_parents.contains(&parent) {
                            variable_parents.push(parent);
                        }
                    }
                    Some(K::ImportClause | K::ImportSpecifier | K::NamespaceImport) => {
                        let declared = view.node(declaration)?.name();
                        if !self.is_identifier_that_starts_with_underscore(declared)? {
                            let clause = self.import_clause_from_imported(declaration)?;
                            if let Some(entry) =
                                import_clauses.iter_mut().find(|(c, _)| *c == clause)
                            {
                                entry.1.push(declaration);
                            } else {
                                import_clauses.push((clause, vec![declaration]));
                            }
                        }
                    }
                    _ => {
                        if kind != K::TypeParameter
                            && !ts_ast::is_ambient_module(view, declaration)?
                        {
                            self.report_unused_local(declaration, name.clone())?;
                        }
                    }
                }
            }
        }
        for declaration in variable_parents {
            if self.ast(declaration)?.node(declaration)?.kind() == K::VariableDeclarationList {
                self.report_unused_variables(declaration)?;
            } else {
                self.report_unused_parameters(declaration)?;
            }
        }
        for (clause, unuseds) in import_clauses {
            self.report_unused_imports(clause, &unuseds)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedLocal
    fn report_unused_local(&mut self, node: NodeId, name: JsString) -> Result<(), Error> {
        let message = if self.is_type_declaration(node)? {
            d::X_0_is_declared_but_never_used
        } else {
            d::X_0_is_declared_but_its_value_is_never_read
        };
        let target = self.ast(node)?.node(node)?.name().unwrap_or(node);
        let diagnostic = self.diagnostic_for_node(Some(target), message, vec![name])?;
        self.report_unused(node, UnusedKind::Local, diagnostic)
    }

    fn variable_list_declarations(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let list = read
            .data_source()
            .as_variable_declaration_list()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .declarations();
        self.source_list(node, list)
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedVariables
    fn report_unused_variables(&mut self, node: NodeId) -> Result<(), Error> {
        let declarations = self.variable_list_declarations(node)?;
        if declarations.len() > 1 && self.all_unreferenced_variable_declarations(&declarations)? {
            let diagnostic =
                self.diagnostic_for_node(Some(node), d::All_variables_are_unused, vec![])?;
            self.report_unused_variable(node, diagnostic)
        } else {
            self.report_unused_variable_declarations(&declarations)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedParameters
    fn report_unused_parameters(&mut self, node: NodeId) -> Result<(), Error> {
        let parameters = self.source_list(node, self.ast(node)?.node(node)?.parameter_list())?;
        self.report_unused_variable_declarations(&parameters)
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedBindingElements
    fn report_unused_binding_elements(&mut self, node: NodeId) -> Result<(), Error> {
        let declarations = self.source_list(node, self.ast(node)?.node(node)?.element_list())?;
        if declarations.len() > 1 && self.all_unreferenced_variable_declarations(&declarations)? {
            let diagnostic = self.diagnostic_for_node(
                Some(node),
                d::All_destructured_elements_are_unused,
                vec![],
            )?;
            self.report_unused_variable(node, diagnostic)
        } else {
            self.report_unused_variable_declarations(&declarations)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedVariableDeclarations
    fn report_unused_variable_declarations(
        &mut self,
        declarations: &[NodeId],
    ) -> Result<(), Error> {
        for &declaration in declarations {
            let view = self.ast(declaration)?;
            let read = view.node(declaration)?;
            let Some(name) = read.name() else {
                continue;
            };
            let parent = required(read.parent(), "declaration parent")?;
            if ts_ast::utilities::is_parameter_property_declaration(view, declaration, parent)?
                || self.is_this_parameter(declaration)?
            {
                continue;
            }
            if self.is_binding_pattern(name)? {
                self.report_unused_binding_elements(name)?;
            } else if self.is_unreferenced_variable_declaration(declaration)? {
                let text = self.ast(name)?.node_text(name)?.into_js_string();
                let diagnostic = self.diagnostic_for_node(
                    Some(name),
                    d::X_0_is_declared_but_its_value_is_never_read,
                    vec![text],
                )?;
                self.report_unused_variable(declaration, diagnostic)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/ast/utilities.go:IsThisParameter
    fn is_this_parameter(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        if read.kind() != K::Parameter {
            return Ok(false);
        }
        let Some(name) = read.name() else {
            return Ok(false);
        };
        Ok(view.node(name)?.kind() == K::Identifier
            && view.node_text(name)?.into_js_string().as_bytes() == b"this")
    }

    fn all_unreferenced_variable_declarations(
        &mut self,
        declarations: &[NodeId],
    ) -> Result<bool, Error> {
        for &declaration in declarations {
            if !self.is_unreferenced_variable_declaration(declaration)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.isUnreferencedVariableDeclaration
    fn is_unreferenced_variable_declaration(&mut self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let Some(name) = read.name() else {
            return Ok(true);
        };
        if self.is_binding_pattern(name)? {
            let elements = self.source_list(name, view.node(name)?.element_list())?;
            return self.all_unreferenced_variable_declarations(&elements);
        }
        if let Some(symbol) = self.get_symbol_of_declaration(node)? {
            if self.reference_kinds(symbol) & sf::VARIABLE != 0 {
                return Ok(false);
            }
        }
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let parent = required(read.parent(), "declaration parent")?;
        let parent_kind = view.node(parent)?.kind();
        if read.kind() == K::BindingElement && parent_kind == K::ObjectBindingPattern {
            // In `{ a, ...b }, `a` is considered used since it removes a property from `b`. `b` may still be unused though.
            let elements = self.source_list(parent, view.node(parent)?.element_list())?;
            if let Some(&last) = elements.last() {
                if node != last && self.binding_is_rest(last)? {
                    return Ok(false);
                }
            }
        }
        let grandparent = view.node(parent)?.parent();
        let for_in_or_of = grandparent
            .map(|g| {
                Ok::<_, Error>(ts_ast::utilities::is_for_in_or_of_statement(Some(
                    &view.node(g)?,
                )))
            })
            .transpose()?
            .unwrap_or(false);
        let candidate = read.kind() == K::Parameter
            || read.kind() == K::VariableDeclaration
                && (for_in_or_of
                    || ts_ast::utilities::get_combined_node_flags(view, node)? & nf::USING != 0)
            || read.kind() == K::BindingElement
                && !(parent_kind == K::ObjectBindingPattern && read.property_name().is_none());
        if candidate && self.is_identifier_that_starts_with_underscore(Some(name))? {
            return Ok(false);
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.reportUnusedImports
    fn report_unused_imports(&mut self, node: NodeId, unuseds: &[NodeId]) -> Result<(), Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let mut declaration_count = usize::from(read.name().is_some());
        let named_bindings = read
            .data_source()
            .as_import_clause()
            .ok_or(ts_arena::Error::InvalidGraph)?
            .named_bindings();
        if let Some(bindings) = named_bindings {
            let bindings_read = view.node(bindings)?;
            if bindings_read.kind() == K::NamespaceImport {
                declaration_count += 1;
            } else {
                declaration_count += self
                    .source_list(bindings, bindings_read.element_list())?
                    .len();
            }
        }
        if declaration_count > 1 && declaration_count == unuseds.len() {
            let parent = required(read.parent(), "import clause parent")?;
            let diagnostic = self.diagnostic_for_node(
                Some(parent),
                d::All_imports_in_import_declaration_are_unused,
                vec![],
            )?;
            self.report_unused(node, UnusedKind::Local, diagnostic)
        } else {
            for &unused in unuseds {
                let name = required(self.ast(unused)?.node(unused)?.name(), "import name")?;
                let text = self.ast(name)?.node_text(name)?.into_js_string();
                self.report_unused_local(unused, text)?;
            }
            Ok(())
        }
    }

    // port: tsc/internal/checker/checker.go:isIdentifierThatStartsWithUnderscore
    fn is_identifier_that_starts_with_underscore(
        &self,
        node: Option<NodeId>,
    ) -> Result<bool, Error> {
        let Some(node) = node else {
            return Ok(false);
        };
        let view = self.ast(node)?;
        if view.node(node)?.kind() != K::Identifier {
            return Ok(false);
        }
        Ok(view.node_text(node)?.into_js_string().as_bytes().first() == Some(&b'_'))
    }

    // port: tsc/internal/checker/checker.go:importClauseFromImported
    fn import_clause_from_imported(&self, node: NodeId) -> Result<NodeId, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        match read.kind().known() {
            Some(K::ImportClause) => Ok(node),
            Some(K::NamespaceImport) => required(read.parent(), "namespace import parent"),
            _ => {
                let parent = required(read.parent(), "import specifier parent")?;
                required(view.node(parent)?.parent(), "named imports parent")
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkUnusedInferTypeParameter
    fn check_unused_infer_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let parameter = required(
            self.ast(node)?
                .node(node)?
                .data_source()
                .as_infer_type_node()
                .and_then(|data| data.type_parameter()),
            "infer type parameter",
        )?;
        if self.is_unreferenced_type_parameter(parameter)? {
            let name = required(self.ast(parameter)?.node(parameter)?.name(), "infer name")?;
            let text = self.ast(name)?.node_text(name)?.into_js_string();
            let diagnostic = self.diagnostic_for_node(
                Some(name),
                d::X_0_is_declared_but_never_used,
                vec![text],
            )?;
            self.report_unused(node, UnusedKind::Parameter, diagnostic)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkUnusedTypeParameters
    fn check_unused_type_parameters(&mut self, node: NodeId) -> Result<(), Error> {
        let symbol = required(
            self.get_symbol_of_declaration(node)?,
            "type parameter owner symbol",
        )?;
        if !self.all_declarations_in_same_source_file(symbol)? {
            return Ok(());
        }
        let view = self.ast(node)?;
        let Some(list) = view.node(node)?.type_parameter_list() else {
            return Ok(());
        };
        let parameters = self.source_list(node, Some(list))?;
        let mut all_unreferenced = parameters.len() > 1;
        if all_unreferenced {
            for &parameter in &parameters {
                if !self.is_unreferenced_type_parameter(parameter)? {
                    all_unreferenced = false;
                    break;
                }
            }
        }
        if all_unreferenced {
            let file = required(
                ts_ast::utilities::get_source_file_of_node(view, Some(node))?,
                "type parameter source file",
            )?;
            let loc = self.range_of_type_parameters(file, view.list(list)?.loc())?;
            let diagnostic =
                Diagnostic::new(Some(file), loc, d::All_type_parameters_are_unused, vec![]);
            self.report_unused(node, UnusedKind::Parameter, diagnostic)
        } else {
            for parameter in parameters {
                if self.is_unreferenced_type_parameter(parameter)? {
                    let name = required(
                        self.ast(parameter)?.node(parameter)?.name(),
                        "type parameter name",
                    )?;
                    let text = self.ast(name)?.node_text(name)?.into_js_string();
                    let diagnostic = self.diagnostic_for_node(
                        Some(parameter),
                        d::X_0_is_declared_but_never_used,
                        vec![text],
                    )?;
                    self.report_unused(node, UnusedKind::Parameter, diagnostic)?;
                }
            }
            Ok(())
        }
    }

    // port: tsc/internal/checker/utilities.go:rangeOfTypeParameters
    fn range_of_type_parameters(&self, file: NodeId, list: TextRange) -> Result<TextRange, Error> {
        let view = self.ast(file)?;
        let text = view.source().as_bytes();
        let end = ts_scanner::skip_trivia(text, list.end()) + 1;
        Ok(TextRange::new(list.pos() - 1, end.min(text.len() as i64)))
    }

    // port: tsc/internal/checker/utilities.go:allDeclarationsInSameSourceFile
    fn all_declarations_in_same_source_file(&self, symbol: SymbolId) -> Result<bool, Error> {
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let mut file = None;
        for declaration in declarations.into_iter().flatten() {
            let current = ts_ast::utilities::get_source_file_of_node(
                self.ast(declaration)?,
                Some(declaration),
            )?;
            match file {
                None => file = Some(current),
                Some(first) if first != current => return Ok(false),
                Some(_) => {}
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.isUnreferencedTypeParameter
    fn is_unreferenced_type_parameter(&self, parameter: NodeId) -> Result<bool, Error> {
        let Some(raw) = self.raw_declaration_symbol(parameter)? else {
            return Ok(false);
        };
        let symbol = self.get_merged_symbol(raw);
        Ok(self.reference_kinds(symbol) & sf::TYPE_PARAMETER == 0
            && !self.is_identifier_that_starts_with_underscore(
                self.ast(parameter)?.node(parameter)?.name(),
            )?)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkUnusedRenamedBindingElements
    pub(crate) fn check_unused_renamed_binding_elements(&mut self) -> Result<(), Error> {
        let nodes = std::mem::take(&mut self.query.renamed_binding_elements_in_types);
        for node in nodes.iter().copied() {
            let Some(symbol) = self.get_symbol_of_declaration(node)? else {
                continue;
            };
            if self.reference_kinds(symbol) != 0 {
                continue;
            }
            let view = self.ast(node)?;
            let wrapping = required(
                ts_ast::utilities::walk_up_binding_elements_and_patterns(view, node)?,
                "renamed binding wrapper",
            )?;
            if !ts_ast::utilities::is_part_of_parameter_declaration(view, wrapping)? {
                return Err(Error::MissingLink(
                    "Only parameter declaration should be checked here",
                ));
            }
            let read = view.node(node)?;
            let name = read.name();
            let property_name = read.property_name();
            let mut diagnostic = self.diagnostic_for_node(
                name,
                d::X_0_is_an_unused_renaming_of_1_Did_you_intend_to_use_it_as_a_type_annotation,
                vec![
                    ts_scanner::declaration_name_to_string(view, name)?,
                    ts_scanner::declaration_name_to_string(view, property_name)?,
                ],
            )?;
            let wrapping_read = view.node(wrapping)?;
            if wrapping_read.type_node().is_none() {
                // entire parameter does not have type annotation, suggest adding an annotation
                let file = ts_ast::utilities::get_source_file_of_node(view, Some(wrapping))?;
                let end = wrapping_read.end();
                diagnostic.related_information.push(Arc::new(Diagnostic::new(
                    file,
                    TextRange::new(i64::from(end), i64::from(end)),
                    d::We_can_only_write_a_type_for_0_by_adding_a_type_for_the_entire_parameter_here,
                    vec![ts_scanner::declaration_name_to_string(view, property_name)?],
                )));
            }
            self.add_diagnostic(diagnostic)?;
        }
        Ok(())
    }
}
