use crate::{
    change_nodes::Leading,
    code_actions::localized,
    codefix_isolated::{const_assertion, Fixer},
    Result,
};
use tsr_ast::{modifier_flags as mf, FactoryMethods, NodeId, SyntaxKind as K};
impl Fixer<'_, '_, '_> {
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.transformDestructuringPatterns
    pub(super) fn destructure(&mut self, pattern: NodeId) -> Result<String> {
        let view = self.syntax.view;
        let Some(declaration) = view.node(pattern)?.parent().filter(|&n| {
            view.node(n)
                .is_ok_and(|n| n.kind() == K::VariableDeclaration)
        }) else {
            return Ok(String::new());
        };
        let list = view.node(declaration)?.parent().expect("variable list");
        let Some(statement) = view
            .node(list)?
            .parent()
            .filter(|&n| view.node(n).is_ok_and(|n| n.kind() == K::VariableStatement))
        else {
            return Ok(String::new());
        };
        let Some(init) = view.node(declaration)?.initializer() else {
            return Ok(String::new());
        };
        let mut nodes = Vec::new();
        let base = if view.node(init)?.kind() == K::Identifier {
            self.id(view.node_text(init)?.as_bytes())
        } else {
            let name = self.unique(b"dest", true);
            let init = self.clone_node(init);
            nodes.push(self.variable(name, None, Some(init), false)?);
            name
        };
        let export = tsr_ast::utilities::has_syntactic_modifier(view, statement, mf::EXPORT)?;
        self.binding_elements(pattern, base, &mut nodes, export)?;
        if nodes.is_empty() {
            return Ok(String::new());
        }
        let declarations: Vec<_> = view
            .node_slice(
                view.list(
                    view.node(list)?
                        .data_source()
                        .as_variable_declaration_list()
                        .and_then(|d| d.declarations())
                        .expect("variable declarations"),
                )?
                .nodes(),
            )?
            .iter()
            .flatten()
            .collect();
        if declarations.len() > 1 {
            let remaining: Vec<_> = declarations
                .into_iter()
                .filter(|&d| d != declaration)
                .collect();
            let new_list = self.list(&remaining)?;
            let new_list = self.tracker.ast.update_variable_declaration_list(
                list,
                Some(new_list),
                view.node(list)?.flags(),
            );
            let remaining = self.tracker.ast.update_variable_statement(
                statement,
                view.node(statement)?.modifiers(),
                Some(new_list),
            );
            nodes.push(remaining);
        }
        self.tracker
            .replace_node_with_nodes(self.syntax.source, statement, nodes, None)?;
        Ok(localized(
            tsr_diagnostics::Extract_binding_expressions_to_variable,
            self.locale,
            &[],
        ))
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.extractBindingElements
    fn binding_elements(
        &mut self,
        pattern: NodeId,
        base: NodeId,
        nodes: &mut Vec<NodeId>,
        export: bool,
    ) -> Result<()> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            self.service.check_canceled()?;
            let view = self.syntax.view;
            let object = view.node(pattern)?.kind() == K::ObjectBindingPattern;
            let elements: Vec<_> = view
                .node_slice(view.node(pattern)?.elements(view)?)?
                .iter()
                .flatten()
                .collect();
            for (index, element) in elements.into_iter().enumerate() {
                let read = view.node(element)?;
                if read.kind() == K::OmittedExpression {
                    continue;
                }
                let data = read.data_source();
                let data = data.as_binding_element().expect("binding element");
                let Some(name) = data.name() else {
                    continue;
                };
                let access = if object {
                    if let Some(property) = data.property_name() {
                        if view.node(property)?.kind() == K::ComputedPropertyName {
                            let expression = view
                                .node(property)?
                                .expression()
                                .expect("computed expression");
                            let id = self
                                .tracker
                                .emit
                                .new_generated_name_for_node(&mut self.tracker.ast, expression);
                            nodes.push(self.variable(id, None, Some(expression), false)?);
                            self.tracker.ast.new_element_access_expression(
                                Some(base),
                                None,
                                Some(id),
                                0,
                            )
                        } else {
                            let id = self.id(view.node_text(property)?.as_bytes());
                            self.tracker.ast.new_property_access_expression(
                                Some(base),
                                None,
                                Some(id),
                                0,
                            )
                        }
                    } else if view.node(name)?.kind() == K::Identifier {
                        let id = self.id(view.node_text(name)?.as_bytes());
                        self.tracker.ast.new_property_access_expression(
                            Some(base),
                            None,
                            Some(id),
                            0,
                        )
                    } else {
                        continue;
                    }
                } else {
                    let number = self.tracker.ast.new_numeric_literal(
                        tsr_ast::JsString::from_bytes(index.to_string().into_bytes()),
                        0,
                    );
                    self.tracker.ast.new_element_access_expression(
                        Some(base),
                        None,
                        Some(number),
                        0,
                    )
                };
                if matches!(
                    view.node(name)?.kind().known(),
                    Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                ) {
                    self.binding_elements(name, access, nodes, export)?;
                    continue;
                }
                let ty = self.infer(name, None)?;
                let mut init = access;
                if let Some(default) = data.initializer() {
                    let base_name = data
                        .property_name()
                        .filter(|&p| view.node(p).is_ok_and(|p| p.kind() == K::Identifier))
                        .map(|p| view.node_text(p).map(|s| s.as_bytes().to_vec()))
                        .transpose()?
                        .unwrap_or_else(|| b"temp".to_vec());
                    let temp = self.unique(&base_name, true);
                    nodes.push(self.variable(temp, None, Some(access), false)?);
                    let undef = self.id(b"undefined");
                    let equal = self
                        .tracker
                        .ast
                        .new_token(K::EqualsEqualsEqualsToken.into());
                    let condition = self.tracker.ast.new_binary_expression(
                        None,
                        Some(temp),
                        None,
                        Some(equal),
                        Some(undef),
                    );
                    let question = self.tracker.ast.new_token(K::QuestionToken.into());
                    let colon = self.tracker.ast.new_token(K::ColonToken.into());
                    init = self.tracker.ast.new_conditional_expression(
                        Some(condition),
                        Some(question),
                        Some(default),
                        Some(colon),
                        Some(access),
                    );
                }
                let name = self.id(view.node_text(name)?.as_bytes());
                nodes.push(self.variable(name, ty, Some(init), export)?);
            }
            Ok(())
        })
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.typeFromSpreads
    pub(super) fn spreads(&mut self, node: NodeId, name: &[u8]) -> Result<Option<NodeId>> {
        let view = self.syntax.view;
        let array = view.node(node)?.kind() == K::ArrayLiteralExpression;
        let mut current = Some(node);
        let mut constant = false;
        while let Some(n) = current {
            if const_assertion(view, n)? {
                constant = true;
                break;
            }
            current = view.node(n)?.parent();
        }
        if array && !constant {
            return Ok(None);
        }
        let name = if name.is_empty() {
            b"temp".as_slice()
        } else {
            name
        };
        let statement = self.statement(node)?;
        let children: Vec<_> = view
            .node_slice(if array {
                view.node(node)?.elements(view)?
            } else {
                view.node(node)?.properties(view)?
            })?
            .iter()
            .flatten()
            .collect();
        let mut types = Vec::new();
        let mut spreads = Vec::new();
        let mut collected = Vec::new();
        for child in children {
            if view.node(child)?.kind()
                == if array {
                    K::SpreadElement
                } else {
                    K::SpreadAssignment
                }
            {
                self.finalize_spread_part(
                    name,
                    constant,
                    statement,
                    array,
                    &mut collected,
                    &mut types,
                    &mut spreads,
                )?;
                let expr = view.node(child)?.expression().expect("spread expression");
                if tsr_ast::is_entity_name_expression(view, expr)? {
                    types.push(self.typeof_node(expr));
                    spreads.push(child);
                } else {
                    self.spread_variable(
                        name,
                        constant,
                        statement,
                        array,
                        expr,
                        &mut types,
                        &mut spreads,
                    )?;
                }
            } else {
                collected.push(child);
            }
        }
        if spreads.is_empty() {
            return Ok(None);
        }
        self.finalize_spread_part(
            name,
            constant,
            statement,
            array,
            &mut collected,
            &mut types,
            &mut spreads,
        )?;
        let new = self.literal(array, &spreads)?;
        self.tracker
            .replace_node(self.syntax.source, node, new, None)?;
        self.mutated = true;
        if array {
            let rest: Vec<_> = types
                .into_iter()
                .map(|t| self.tracker.ast.new_rest_type_node(Some(t)))
                .collect();
            let list = self.list(&rest)?;
            Ok(Some(self.tracker.ast.new_tuple_type_node(Some(list))))
        } else {
            let list = self.list(&types)?;
            Ok(Some(
                self.tracker.ast.new_intersection_type_node(Some(list)),
            ))
        }
    }
    fn literal(&mut self, array: bool, children: &[NodeId]) -> Result<NodeId> {
        let list = self.list(children)?;
        Ok(if array {
            self.tracker
                .ast
                .new_array_literal_expression(Some(list), true)
        } else {
            self.tracker
                .ast
                .new_object_literal_expression(Some(list), true)
        })
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Spread decomposition keeps its two accumulating node lists and original inference policy explicit"
    )]
    fn finalize_spread_part(
        &mut self,
        name: &[u8],
        constant: bool,
        statement: Option<NodeId>,
        array: bool,
        collected: &mut Vec<NodeId>,
        types: &mut Vec<NodeId>,
        spreads: &mut Vec<NodeId>,
    ) -> Result<()> {
        if !collected.is_empty() {
            let expression = self.literal(array, collected)?;
            self.spread_variable(name, constant, statement, array, expression, types, spreads)?;
            collected.clear();
        }
        Ok(())
    }
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:isolatedDeclarationsFixer.makeSpreadVariable
    #[allow(
        clippy::too_many_arguments,
        reason = "Spread decomposition keeps its two accumulating node lists and original inference policy explicit"
    )]
    fn spread_variable(
        &mut self,
        name: &[u8],
        constant: bool,
        statement: Option<NodeId>,
        array: bool,
        expression: NodeId,
        types: &mut Vec<NodeId>,
        spreads: &mut Vec<NodeId>,
    ) -> Result<()> {
        let mut base = name.to_vec();
        base.extend_from_slice(format!("_Part{}", spreads.len() + 1).as_bytes());
        let name = self.unique(&base, true);
        let mut expression = self.clone_node(expression);
        if constant {
            let id = self.id(b"const");
            let ty = self.tracker.ast.new_type_reference_node(Some(id), None);
            expression = self
                .tracker
                .ast
                .new_as_expression(Some(expression), Some(ty));
        }
        let declaration = self.variable(name, None, Some(expression), false)?;
        if let Some(statement) = statement {
            self.tracker.insert_before(
                self.syntax.source,
                statement,
                declaration,
                false,
                Leading::None,
            )?;
        }
        types.push(self.typeof_node(name));
        spreads.push(if array {
            self.tracker.ast.new_spread_element(Some(name))
        } else {
            self.tracker.ast.new_spread_assignment(Some(name))
        });
        Ok(())
    }
}
