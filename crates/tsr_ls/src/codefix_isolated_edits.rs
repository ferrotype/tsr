use crate::{
    change_nodes::{Leading, NodeOptions},
    code_actions::localized,
    codefix_isolated::{const_assertion, named_declaration, value_signature, Fixer},
    Result,
};
use tsr_ast::{modifier_flags as mf, Factory, FactoryMethods, NodeId, SyntaxKind as K};
use tsr_checker::type_flags as tf;
use tsr_core::TextRange;
impl Fixer<'_, '_, '_> {
    fn assertion(&mut self, node: NodeId, ty: NodeId) -> Result<NodeId> {
        let mut node = node;
        let view = self.tracker.ast.view();
        if !tsr_ast::is_entity_name_expression(view, node)?
            && !matches!(
                view.node(node)?.kind().known(),
                Some(K::CallExpression | K::ObjectLiteralExpression | K::ArrayLiteralExpression)
            )
        {
            node = self.tracker.ast.new_parenthesized_expression(Some(node));
        }
        Ok(self.tracker.ast.new_as_expression(Some(node), Some(ty)))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.addInlineAssertion
    pub(super) fn inline(&mut self, span: TextRange) -> Result<String> {
        let token = self.syntax.nav().get_token_at_position(span.pos())?;
        if self.expando(token)?.is_some() {
            return Ok(String::new());
        }
        let node = self.best(span)?;
        let view = self.syntax.view;
        let read = view.node(node)?;
        if value_signature(read.kind())
            || read
                .parent()
                .is_some_and(|p| view.node(p).is_ok_and(|p| value_signature(p.kind())))
        {
            return Ok(String::new());
        }
        let shorthand = read.kind() == K::ShorthandPropertyAssignment;
        if !shorthand && named_declaration(read.kind())
            || self
                .ancestor(node, |n| {
                    matches!(
                        n.kind().known(),
                        Some(K::ObjectBindingPattern | K::ArrayBindingPattern | K::EnumMember)
                    )
                })?
                .is_some()
        {
            return Ok(String::new());
        }
        let expression = tsr_ast::utilities_positions::is_expression(view, node)?;
        if expression
            && self
                .ancestor(node, |n| {
                    n.kind() == K::HeritageClause || tsr_ast::utilities::is_type_node(n)
                })?
                .is_some()
            || read.kind() == K::SpreadElement
        {
            return Ok(String::new());
        }
        let variable = self.ancestor(node, |n| n.kind() == K::VariableDeclaration)?;
        let variable_type = variable
            .map(|n| self.checker.get_type_at_location(n))
            .transpose()?;
        if variable_type.is_some_and(|t| {
            self.checker
                .type_flags(t)
                .is_ok_and(|f| f & tf::UNIQUE_ES_SYMBOL != 0)
        }) || !expression && !shorthand
        {
            return Ok(String::new());
        }
        let Some(ty) = self.infer(node, variable_type)? else {
            return Ok(String::new());
        };
        if self.mutated {
            return Ok(String::new());
        }
        if shorthand {
            let name = self.clone_node(read.name().expect("shorthand name"));
            let expr = self.assertion(name, ty)?;
            self.tracker.insert_node(
                self.syntax.source,
                i64::from(read.end()),
                expr,
                NodeOptions {
                    prefix: ": ".into(),
                    ..Default::default()
                },
            );
        } else {
            let mut cloned = self.clone_node(node);
            if !tsr_ast::is_entity_name_expression(view, node)?
                && !matches!(
                    read.kind().known(),
                    Some(
                        K::CallExpression | K::ObjectLiteralExpression | K::ArrayLiteralExpression
                    )
                )
            {
                cloned = self.tracker.ast.new_parenthesized_expression(Some(cloned));
            }
            let cloned_type = self.clone_node(ty);
            let satisfies = self
                .tracker
                .ast
                .new_satisfies_expression(Some(cloned), Some(cloned_type));
            let new = self
                .tracker
                .ast
                .new_as_expression(Some(satisfies), Some(ty));
            self.tracker
                .replace_node(self.syntax.source, node, new, None)?;
        }
        let text = self.display(ty)?;
        Ok(localized(
            tsr_diagnostics::Add_satisfies_and_an_inline_type_assertion_with_0,
            self.locale,
            &[&text],
        ))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.extractAsVariable
    pub(super) fn extract(&mut self, span: TextRange) -> Result<String> {
        let node = self.best(span)?;
        let view = self.syntax.view;
        let read = view.node(node)?;
        if value_signature(read.kind())
            || read
                .parent()
                .is_some_and(|p| view.node(p).is_ok_and(|p| value_signature(p.kind())))
            || !tsr_ast::utilities_positions::is_expression(view, node)?
        {
            return Ok(String::new());
        }
        if read.kind() == K::ArrayLiteralExpression {
            let name = self.id(b"const");
            let ty = self.tracker.ast.new_type_reference_node(Some(name), None);
            let cloned = self.clone_node(node);
            let new = self.assertion(cloned, ty)?;
            self.tracker
                .replace_node(self.syntax.source, node, new, None)?;
            return Ok(localized(
                tsr_diagnostics::Mark_array_literal_as_const,
                self.locale,
                &[],
            ));
        }
        let Some(property) = self.ancestor(node, |n| n.kind() == K::PropertyAssignment)? else {
            return Ok(String::new());
        };
        if read.parent() == Some(property) && tsr_ast::is_entity_name_expression(view, node)? {
            return Ok(String::new());
        }
        let base = if read.kind() == K::PropertyAccessExpression {
            read.name()
                .filter(|&n| view.node(n).is_ok_and(|n| n.kind() == K::Identifier))
                .map(|n| view.node_text(n).map(|s| s.as_bytes().to_vec()))
                .transpose()?
                .filter(|s| tsr_scanner::string_to_token(s) == K::Unknown)
                .unwrap_or_else(|| b"newLocal".to_vec())
        } else {
            b"newLocal".to_vec()
        };
        let name = self.unique(&base, true);
        let mut target = node;
        let mut init = node;
        if read.kind() == K::SpreadElement {
            target = read.parent().expect("spread container");
            while let Some(parent) = view.node(target)?.parent().filter(|&p| {
                view.node(p)
                    .is_ok_and(|p| p.kind() == K::ParenthesizedExpression)
            }) {
                target = parent;
            }
            if let Some(parent) = view
                .node(target)?
                .parent()
                .filter(|&p| const_assertion(view, p).unwrap_or(false))
            {
                target = parent;
                init = target;
            } else {
                let constant = self.id(b"const");
                let constant = self
                    .tracker
                    .ast
                    .new_type_reference_node(Some(constant), None);
                let cloned = self.clone_node(target);
                init = self.assertion(cloned, constant)?;
            }
        }
        if tsr_ast::is_entity_name_expression(view, target)? {
            return Ok(String::new());
        }
        let cloned = self.clone_node(init);
        let statement = self.variable(name, None, Some(cloned), false)?;
        let Some(before) = self.statement(node)? else {
            return Ok(String::new());
        };
        self.tracker
            .insert_before(self.syntax.source, before, statement, false, Leading::None)?;
        let query = self.tracker.ast.new_type_query_node(Some(name), None);
        let new = self.tracker.ast.new_as_expression(Some(name), Some(query));
        self.tracker
            .replace_node(self.syntax.source, target, new, None)?;
        let text = self.display(name)?;
        Ok(localized(
            tsr_diagnostics::Extract_to_variable_and_replace_with_0_as_typeof_0,
            self.locale,
            &[&text],
        ))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.transformExportAssignment
    pub(super) fn export_assignment(&mut self, node: NodeId) -> Result<String> {
        let view = self.syntax.view;
        let read = view.node(node)?;
        let data = read.data_source();
        let data = data.as_export_assignment().unwrap();
        if data.is_export_equals() {
            return Ok(String::new());
        }
        let expression = data.expression().expect("export expression");
        let Some(ty) = self.infer(expression, None)? else {
            return Ok(String::new());
        };
        let name = self.unique(b"_default", false);
        let init = self.clone_node(expression);
        let statement = self.variable(name, Some(ty), Some(init), false)?;
        let export = self.tracker.ast.update_export_assignment(
            node,
            read.modifiers(),
            false,
            None,
            Some(name),
        );
        self.tracker.replace_node_with_nodes(
            self.syntax.source,
            node,
            vec![statement, export],
            None,
        )?;
        Ok(localized(
            tsr_diagnostics::Extract_default_export_to_variable,
            self.locale,
            &[],
        ))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.transformExtendsClauseWithExpression
    pub(super) fn extends_clause(&mut self, node: NodeId) -> Result<String> {
        let view = self.syntax.view;
        let Some(heritage) =
            tsr_ast::utilities_class::get_class_extends_heritage_element(view, node)?
        else {
            return Ok(String::new());
        };
        let Some(expression) = view.node(heritage)?.expression() else {
            return Ok(String::new());
        };
        let Some(ty) = self.infer(expression, None)? else {
            return Ok(String::new());
        };
        let base = if let Some(name) = view.node(node)?.name() {
            let mut name = view.node_text(name)?.as_bytes().to_vec();
            name.extend_from_slice(b"Base");
            name
        } else {
            b"Anonymous".to_vec()
        };
        let name = self.unique(&base, true);
        let init = self.clone_node(expression);
        let statement = self.variable(name, Some(ty), Some(init), false)?;
        self.tracker
            .insert_before(self.syntax.source, node, statement, false, Leading::None)?;
        let new = self
            .tracker
            .ast
            .new_expression_with_type_arguments(Some(name), None);
        self.tracker
            .replace_node(self.syntax.source, heritage, new, None)?;
        Ok(localized(
            tsr_diagnostics::Extract_base_class_to_variable,
            self.locale,
            &[],
        ))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.createNamespaceForExpandoProperties
    pub(super) fn expando_namespace(&mut self, node: NodeId) -> Result<String> {
        let view = self.syntax.view;
        let Some(name) = view.node(node)?.name() else {
            return Ok(String::new());
        };
        let ty = self.checker.get_type_at_location(node)?;
        let mut statements = Vec::new();
        for symbol in self.checker.get_properties_of_type(ty)? {
            let read = self.checker.symbol(symbol)?;
            let name = read.name_bytes().to_vec();
            let value = read.value_declaration();
            if !tsr_scanner::is_identifier_text(&name, tsr_core::LanguageVariant::STANDARD)
                || value.is_some_and(|d| {
                    self.checker
                        .node(d)
                        .is_ok_and(|d| d.kind() == K::VariableDeclaration)
                })
            {
                continue;
            }
            let ty = self.checker.get_type_of_symbol(symbol)?;
            let Some(ty) = self.minimized(ty, node, super::codefix_isolated::DECLARATION_FLAGS)?
            else {
                continue;
            };
            let name = self.id(&name);
            let declaration =
                self.tracker
                    .ast
                    .new_variable_declaration(Some(name), None, Some(ty), None);
            let list = self.list(&[declaration])?;
            let declarations = self
                .tracker
                .ast
                .new_variable_declaration_list(Some(list), 0);
            let export = self.tracker.ast.new_modifier(K::ExportKeyword.into());
            let modifiers = self.list(&[export])?;
            statements.push(
                self.tracker
                    .ast
                    .new_variable_statement(Some(modifiers), Some(declarations)),
            );
        }
        if statements.is_empty() {
            return Ok(String::new());
        }
        let mut modifiers = Vec::new();
        if tsr_ast::utilities::has_syntactic_modifier(view, node, mf::EXPORT)? {
            modifiers.push(self.tracker.ast.new_modifier(K::ExportKeyword.into()));
        }
        modifiers.push(self.tracker.ast.new_modifier(K::DeclareKeyword.into()));
        let modifiers = self.list(&modifiers)?;
        let statements = self.list(&statements)?;
        let body = self.tracker.ast.new_module_block(Some(statements));
        let name = self.id(view.node_text(name)?.as_bytes());
        let module = self.tracker.ast.new_module_declaration(
            Some(modifiers),
            K::NamespaceKeyword.into(),
            Some(name),
            None,
            Some(body),
        );
        self.tracker.ast.node_mut(module)?.set_flags(
            tsr_ast::node_flags::AMBIENT
                | tsr_ast::node_flags::EXPORT_CONTEXT
                | tsr_ast::node_flags::CONTEXT_FLAGS,
        );
        self.tracker
            .insert_after(self.syntax.source, node, module)?;
        Ok(localized(
            tsr_diagnostics::Annotate_types_of_properties_expando_function_in_a_namespace,
            self.locale,
            &[],
        ))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.relativeType
    pub(super) fn relative(&mut self, node: NodeId) -> Result<Option<NodeId>> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.relative_worker(node))
    }
    fn relative_worker(&mut self, node: NodeId) -> Result<Option<NodeId>> {
        self.service.check_canceled()?;
        let view = self.syntax.view;
        let read = view.node(node)?;
        if read.kind() == K::Parameter {
            return Ok(None);
        }
        if read.kind() == K::ShorthandPropertyAssignment {
            return Ok(Some(self.typeof_node(read.name().expect("shorthand name"))));
        }
        if tsr_ast::is_entity_name_expression(view, node)? {
            return Ok(Some(self.typeof_node(node)));
        }
        if const_assertion(view, node)? {
            return self.relative(read.expression().expect("assertion operand"));
        }
        match read.kind().known() {
            Some(K::ArrayLiteralExpression | K::ObjectLiteralExpression) => {
                let variable = self.ancestor(node, |n| n.kind() == K::VariableDeclaration)?;
                let name = variable
                    .and_then(|d| view.node(d).ok().and_then(|d| d.name()))
                    .filter(|&n| view.node(n).is_ok_and(|n| n.kind() == K::Identifier))
                    .map(|n| view.node_text(n).map(|s| s.as_bytes().to_vec()))
                    .transpose()?
                    .unwrap_or_default();
                self.spreads(node, &name)
            }
            Some(K::VariableDeclaration) => {
                if let Some(init) = read.initializer() {
                    self.relative(init)
                } else {
                    Ok(None)
                }
            }
            Some(K::ConditionalExpression) => {
                let data = read.data_source();
                let data = data.as_conditional_expression().unwrap();
                let Some(a) = self.relative(data.when_true().expect("true branch"))? else {
                    return Ok(None);
                };
                let changed = self.mutated;
                let Some(b) = self.relative(data.when_false().expect("false branch"))? else {
                    return Ok(None);
                };
                self.mutated |= changed;
                let list = self.list(&[a, b])?;
                Ok(Some(self.tracker.ast.new_union_type_node(Some(list))))
            }
            _ => Ok(None),
        }
    }
    pub fn typeof_node(&mut self, node: NodeId) -> NodeId {
        let node = self.clone_node(node);
        self.tracker.ast.new_type_query_node(Some(node), None)
    }
}
