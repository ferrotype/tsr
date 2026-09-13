//! Assignment declarations obtain context only from independent annotations;
//! asking their inferred left-hand type would reenter the same initializer.
use crate::{CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/ast/precedence.go:GetLeftmostExpression
    pub(crate) fn leftmost_context_expression(&self, mut node: NodeId) -> Result<NodeId, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            let next = match read.kind().known() {
                Some(K::PostfixUnaryExpression) => read
                    .data_source()
                    .as_postfix_unary_expression()
                    .and_then(|data| data.operand()),
                Some(K::BinaryExpression) => read
                    .data_source()
                    .as_binary_expression()
                    .and_then(|data| data.left()),
                Some(K::ConditionalExpression) => read
                    .data_source()
                    .as_conditional_expression()
                    .and_then(|data| data.condition()),
                Some(K::TaggedTemplateExpression) => read
                    .data_source()
                    .as_tagged_template_expression()
                    .and_then(|data| data.tag()),
                Some(
                    K::CallExpression
                    | K::AsExpression
                    | K::ElementAccessExpression
                    | K::PropertyAccessExpression
                    | K::NonNullExpression
                    | K::PartiallyEmittedExpression
                    | K::SatisfiesExpression,
                ) => read.expression(),
                _ => return Ok(node),
            };
            node = next.ok_or(Error::MissingLink("leftmost context expression"))?;
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextualTypeForAssignmentExpression
    pub(crate) fn contextual_assignment_expression(
        &mut self,
        binary: NodeId,
        left: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        if !matches!(
            self.ast(left)?.node(left)?.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return self.get_type_of_expression(left).map(Some);
        }
        let Some(receiver) = self.access_receiver(left)? else {
            return self.get_type_of_expression(left).map(Some);
        };
        let declaration_symbol = self.raw_declaration_symbol(binary)?;
        match self.ast(receiver)?.node(receiver)?.kind().known() {
            Some(K::Identifier) => {
                let local = self.resolved_value_symbol(receiver)?;
                let symbol = self.get_export_symbol_of_value_symbol_if_exported(local)?;
                if self.symbol(symbol)?.flags() & sf::MODULE_EXPORTS != 0 {
                    return Ok(None);
                }
                if declaration_symbol.is_some() {
                    if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                        let read = self.ast(declaration)?.node(declaration)?;
                        if read.kind() == K::VariableDeclaration {
                            if let Some(annotation) = read.type_node() {
                                let read = self.ast(left)?.node(left)?;
                                if read.kind() == K::PropertyAccessExpression {
                                    let name = read
                                        .name()
                                        .ok_or(Error::MissingLink("contextual assignment name"))?;
                                    let name = self.ast(name)?.node_text(name)?.into_js_string();
                                    let ty = self.get_type_from_type_node(annotation)?;
                                    return self
                                        .type_of_property_of_contextual_type(ty, name.as_bytes());
                                }
                                let argument = read
                                    .data_source()
                                    .as_element_access_expression()
                                    .and_then(|data| data.argument_expression())
                                    .ok_or(Error::MissingLink("contextual assignment index"))?;
                                let key = self.check_expression_cached(argument)?;
                                if let Some(name) = self.index_property_name(key)? {
                                    let ty = self.get_type_from_type_node(annotation)?;
                                    return self.contextual_property_type_ex(
                                        ty,
                                        name.as_bytes(),
                                        Some(key),
                                    );
                                }
                                return self.get_type_of_expression(left).map(Some);
                            }
                        }
                    }
                    return Ok(None);
                }
            }
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                if declaration_symbol.is_some() =>
            {
                return Ok(None)
            }
            Some(K::ThisKeyword) => {
                let this_type = self.get_type_of_expression(receiver)?;
                let read = self.ast(left)?.node(left)?;
                let name = if read.kind() == K::PropertyAccessExpression {
                    let name = read
                        .name()
                        .ok_or(Error::MissingLink("this assignment name"))?;
                    let text = self.ast(name)?.node_text(name)?.into_js_string();
                    if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier {
                        self.types
                            .get(this_type)?
                            .symbol
                            .map(|symbol| {
                                self.symbol(symbol).map(|symbol| {
                                    ts_binder::get_symbol_name_for_private_identifier(
                                        &symbol,
                                        text.as_bytes(),
                                    )
                                })
                            })
                            .transpose()?
                    } else {
                        Some(text)
                    }
                } else {
                    let argument = read
                        .data_source()
                        .as_element_access_expression()
                        .and_then(|data| data.argument_expression())
                        .ok_or(Error::MissingLink("this assignment index"))?;
                    let key = self.check_expression_cached(argument)?;
                    self.index_property_name(key)?
                };
                if let Some(name) = name {
                    if let Some(symbol) =
                        self.constituent_property(this_type, name.as_bytes(), false)?
                    {
                        if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                            let read = self.ast(declaration)?.node(declaration)?;
                            if matches!(
                                read.kind().known(),
                                Some(K::PropertyDeclaration | K::PropertySignature)
                            ) && read.type_node().is_none()
                                && read.initializer().is_none()
                            {
                                return Ok(None);
                            }
                        }
                    }
                }
                if let Some(symbol) = declaration_symbol {
                    if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                        // The pin also returns nil for object-literal methods here;
                        // its contextual-this implementation remains a native TODO.
                        if self
                            .ast(declaration)?
                            .node(declaration)?
                            .type_node()
                            .is_none()
                        {
                            return Ok(None);
                        }
                    }
                }
            }
            _ => {}
        }
        self.get_type_of_expression(left).map(Some)
    }
}
