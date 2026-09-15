//! Private names resolve in the lexical class first, then against the receiver
//! type. Equal spelling alone never identifies a private property.
use crate::flow_assignments::AssignmentKind;
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;

pub(crate) enum PrivateAccessResult {
    Type(TypeId),
    Property(Option<SymbolId>),
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkPropertyAccessExpressionOrQualifiedName
    pub(crate) fn private_access_property(
        &mut self,
        node: NodeId,
        left_type: TypeId,
        apparent: TypeId,
        right: NodeId,
        assignment: AssignmentKind,
    ) -> Result<PrivateAccessResult, Error> {
        if self.private_elements_need_transform()? {
            if assignment != AssignmentKind::None {
                self.check_external_emit_helpers(
                    node,
                    crate::external_emit_helpers::CLASS_PRIVATE_FIELD_SET,
                )?;
            }
            if assignment != AssignmentKind::Definite {
                self.check_external_emit_helpers(
                    node,
                    crate::external_emit_helpers::CLASS_PRIVATE_FIELD_GET,
                )?;
            }
        }
        let name = self.ast(right)?.node_text(right)?.into_js_string();
        let lexical = self.lookup_private_identifier_declaration(name.as_bytes(), right)?;
        if assignment != AssignmentKind::None {
            if let Some(symbol) = lexical {
                if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                    if self.ast(declaration)?.node(declaration)?.kind() == K::MethodDeclaration {
                        self.grammar_error_node(
                            right,
                            d::Cannot_assign_to_private_method_0_Private_methods_are_not_writable,
                            vec![name.clone()],
                        )?;
                    }
                }
            }
        }
        if self.types.flags(apparent)? & tf::ANY != 0 || apparent == self.builtins.silent_never_type
        {
            if lexical.is_some() {
                return Ok(PrivateAccessResult::Type(
                    if self.is_error_type(apparent)? {
                        self.builtins.error_type
                    } else {
                        apparent
                    },
                ));
            }
            if self.private_containing_class(right)?.is_none() {
                self.grammar_error_node(
                    right,
                    d::Private_identifiers_are_not_allowed_outside_class_bodies,
                    vec![],
                )?;
                return Ok(PrivateAccessResult::Type(self.builtins.any_type));
            }
        }
        let property = match lexical {
            Some(symbol) => {
                let name = self.symbol(symbol)?.name_to_owned();
                self.constituent_property(left_type, name.as_bytes(), false)?
            }
            None => None,
        };
        if property.is_none() {
            if self.private_identifier_access_error(left_type, right, lexical)? {
                return Ok(PrivateAccessResult::Type(self.builtins.error_type));
            }
            if let Some(class) = self.private_containing_class(right)? {
                if self.is_plain_js_node(class)? {
                    let text = self.ast(right)?.node_text(right)?.into_js_string();
                    self.grammar_error_node(
                        right,
                        d::Private_field_0_must_be_declared_in_an_enclosing_class,
                        vec![text],
                    )?;
                }
            }
        } else if let Some(property) = property {
            let flags = self.symbol(property)?.flags();
            if flags & sf::SET_ACCESSOR != 0
                && flags & sf::GET_ACCESSOR == 0
                && assignment != AssignmentKind::Definite
            {
                self.error_at(
                    Some(node),
                    d::Private_accessor_was_defined_without_a_getter,
                    vec![],
                )?;
            }
        }
        Ok(PrivateAccessResult::Property(property))
    }

    // port: tsc/internal/checker/utilities.go:getContainingClassExcludingClassDecorators
    pub(crate) fn private_containing_class(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let mut current = self.ast(node)?.node(node)?.parent();
        while let Some(ancestor) = current {
            let read = self.ast(ancestor)?.node(ancestor)?;
            if matches!(
                read.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) {
                break;
            }
            if read.kind() == K::Decorator {
                let parent = read
                    .parent()
                    .ok_or(Error::MissingLink("decorator parent"))?;
                if matches!(
                    self.ast(parent)?.node(parent)?.kind().known(),
                    Some(K::ClassDeclaration | K::ClassExpression)
                ) {
                    return Ok(ts_ast::utilities::get_containing_class(
                        self.ast(parent)?,
                        parent,
                    )?);
                }
                return Ok(ts_ast::utilities::get_containing_class(
                    self.ast(ancestor)?,
                    ancestor,
                )?);
            }
            current = read.parent();
        }
        Ok(ts_ast::utilities::get_containing_class(
            self.ast(node)?,
            node,
        )?)
    }

    // port: tsc/internal/checker/checker.go:Checker.lookupSymbolForPrivateIdentifierDeclaration
    pub(crate) fn lookup_private_identifier_declaration(
        &self,
        name: &[u8],
        location: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let mut class = self.private_containing_class(location)?;
        while let Some(node) = class {
            let symbol = self
                .raw_declaration_symbol(node)?
                .ok_or(Error::MissingLink("private class symbol"))?;
            let read = self.symbol(symbol)?;
            let name = ts_binder::get_symbol_name_for_private_identifier(&read, name);
            if let Some(property) = self.member_symbol(read.members(), name.as_bytes())? {
                return Ok(Some(property));
            }
            if let Some(property) = self.member_symbol(read.exports(), name.as_bytes())? {
                return Ok(Some(property));
            }
            class = ts_ast::utilities::get_containing_class(self.ast(node)?, node)?;
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPrivateIdentifierPropertyAccess
    fn private_identifier_access_error(
        &mut self,
        ty: TypeId,
        right: NodeId,
        lexical: Option<SymbolId>,
    ) -> Result<bool, Error> {
        let spelling = self.ast(right)?.node_text(right)?.into_js_string();
        let mut property = None;
        for candidate in self.get_properties_of_type(ty)? {
            if let Some(declaration) = self.symbol(candidate)?.value_declaration() {
                if let Some(name) = self.ast(declaration)?.node(declaration)?.name() {
                    if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier
                        && self.ast(name)?.node_text(name)?.as_bytes() == spelling.as_bytes()
                    {
                        property = Some(candidate);
                        break;
                    }
                }
            }
        }
        let Some(property) = property else {
            return Ok(false);
        };
        let declaration = self
            .symbol(property)?
            .value_declaration()
            .ok_or(Error::MissingLink("private property declaration"))?;
        let class = ts_ast::utilities::get_containing_class(self.ast(declaration)?, declaration)?
            .ok_or(Error::MissingLink("private property class"))?;
        let name = ts_scanner::declaration_name_to_string(self.ast(right)?, Some(right))?;
        if let Some(lexical) = lexical {
            if let Some(lexical_declaration) = self.symbol(lexical)?.value_declaration() {
                let lexical_class = ts_ast::utilities::get_containing_class(
                    self.ast(lexical_declaration)?,
                    lexical_declaration,
                )?
                .ok_or(Error::MissingLink("lexical private class"))?;
                let mut ancestor = Some(lexical_class);
                let mut shadows = false;
                while let Some(node) = ancestor {
                    if node == class {
                        shadows = true;
                        break;
                    }
                    ancestor = self.ast(node)?.node(node)?.parent();
                }
                if shadows {
                    let display = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                    if let Some(error)=self.error_at(Some(right),d::The_property_0_cannot_be_accessed_on_type_1_within_this_class_because_it_is_shadowed_by_another_private_identifier_with_the_same_spelling,vec![name.clone(),display])? {
                        let related=self.diagnostic_for_node(Some(lexical_declaration),d::The_shadowing_declaration_of_0_is_defined_here,vec![name.clone()])?;
                        self.add_related_diagnostic(error,related)?;
                        let related=self.diagnostic_for_node(Some(declaration),d::The_declaration_of_0_that_you_probably_intended_to_use_is_defined_here,vec![name])?;
                        self.add_related_diagnostic(error,related)?;
                    }
                    return Ok(true);
                }
            }
        }
        let class_symbol = self
            .raw_declaration_symbol(class)?
            .ok_or(Error::MissingLink("private property class symbol"))?;
        let class_name = self.symbol_to_string(class_symbol)?;
        self.error_at(
            Some(right),
            d::Property_0_is_not_accessible_outside_class_1_because_it_has_a_private_identifier,
            vec![name, class_name],
        )?;
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSymbolForPrivateIdentifierExpression
    pub(crate) fn private_identifier_expression_symbol(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = self.query.resolved_symbols.try_get(node).copied().flatten() {
            return Ok(Some(symbol));
        }
        let name = self.ast(node)?.node_text(node)?.into_js_string();
        let symbol = self.lookup_private_identifier_declaration(name.as_bytes(), node)?;
        *self.query.resolved_symbols.get_or_default(node) = symbol;
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPrivateIdentifierExpression
    pub(crate) fn check_private_identifier_expression(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        self.check_private_identifier_grammar(node)?;
        if let Some(symbol) = self.private_identifier_expression_symbol(node)? {
            if self.symbol(symbol)?.flags() & sf::CLASS_MEMBER != 0
                && self.symbol(symbol)?.value_declaration().is_some()
            {
                let target = self.target_symbol(symbol)?;
                *self.query.references.get_or_default(target) |= sf::ALL;
            }
        }
        Ok(self.builtins.any_type)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarPrivateIdentifierExpression
    fn check_private_identifier_grammar(&mut self, node: NodeId) -> Result<bool, Error> {
        if ts_ast::utilities::get_containing_class(self.ast(node)?, node)?.is_none() {
            return self.grammar_error_node(
                node,
                d::Private_identifiers_are_not_allowed_outside_class_bodies,
                vec![],
            );
        }
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("private expression parent"))?;
        let read = self.ast(parent)?.node(parent)?;
        if read.kind() != K::ForInStatement {
            if !self.expression_node(node)? {
                return self.grammar_error_node(node,d::Private_identifiers_are_only_allowed_in_class_bodies_and_may_only_be_used_as_part_of_a_class_member_declaration_property_access_or_on_the_left_hand_side_of_an_in_expression,vec![]);
            }
            let in_operation = if let Some(data) = read.data_source().as_binary_expression() {
                let token = data
                    .operator_token()
                    .ok_or(Error::MissingLink("private expression operator"))?;
                self.ast(token)?.node(token)?.kind() == K::InKeyword
            } else {
                false
            };
            if self.private_identifier_expression_symbol(node)?.is_none() && !in_operation {
                let name = self.ast(node)?.node_text(node)?.into_js_string();
                return self.grammar_error_node(node, d::Cannot_find_name_0, vec![name]);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkInExpression
    pub(crate) fn check_private_in_operand(
        &mut self,
        left: NodeId,
        right: TypeId,
    ) -> Result<(), Error> {
        if self.private_elements_need_transform()? {
            self.check_external_emit_helpers(
                left,
                crate::external_emit_helpers::CLASS_PRIVATE_FIELD_IN,
            )?;
        }
        if self
            .query
            .resolved_symbols
            .try_get(left)
            .copied()
            .flatten()
            .is_none()
            && ts_ast::utilities::get_containing_class(self.ast(left)?, left)?.is_some()
        {
            let right_symbol = self.types.get(right)?.symbol;
            let unchecked_js = self.is_unchecked_js_suggestion(Some(left), right_symbol, true)?;
            self.defer_missing_property_ex(left, right, unchecked_js);
        }
        Ok(())
    }

    /// `IsPlainJSFile(GetSourceFileOfNode(node), checkJs)`.
    // port: tsc/internal/ast/utilities.go:IsPlainJSFile
    pub(crate) fn is_plain_js_node(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let Some(source) = ts_ast::utilities::get_source_file_of_node(view, Some(node))? else {
            return Ok(false);
        };
        let file = view.source_file(source)?;
        Ok(matches!(
            file.script_kind,
            ts_core::ScriptKind::JS | ts_core::ScriptKind::JSX
        ) && file.check_js_directive.is_none()
            && self.program()?.host.options().check_js == ts_core::Tristate::UNKNOWN)
    }
}
