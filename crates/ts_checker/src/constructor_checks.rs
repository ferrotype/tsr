//! Constructor-specific super-call ordering after signature, body and overload
//! checking. These traversals intentionally exclude nested function bodies.
use crate::{CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkConstructorDeclaration
    pub(crate) fn check_constructor_super_calls(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let Some(body) = read.body() else {
            return Ok(());
        };
        if !ts_ast::node_is_present(Some(&self.ast(body)?.node(body)?)) {
            return Ok(());
        }
        let class = read
            .parent()
            .ok_or(Error::MissingLink("constructor class"))?;
        let parameters = read.parameter_list();
        if self
            .class_heritage_nodes(class, K::ExtendsKeyword)?
            .is_empty()
        {
            return Ok(());
        }
        let extends_null = self.class_extends_null(class)?;
        let Some(call) = self.first_constructor_super_call(body)? else {
            if !extends_null {
                self.error_at(
                    Some(node),
                    d::Constructors_for_derived_classes_must_contain_a_super_call,
                    vec![],
                )?;
            }
            return Ok(());
        };
        if extends_null {
            self.error_at(
                Some(call),
                d::A_constructor_cannot_contain_a_super_call_when_its_class_extends_null,
                vec![],
            )?;
        }
        let mut requires_root = false;
        if !self.program()?.host.options().emit_standard_class_fields() {
            let members = self.source_list(class, self.ast(class)?.node(class)?.member_list())?;
            for member in members {
                let read = self.ast(member)?.node(member)?;
                if ts_ast::utilities::is_private_identifier_class_element_declaration(
                    self.ast(member)?,
                    member,
                )? || read.kind() == K::PropertyDeclaration
                    && read.modifier_flags(self.ast(member)?)? & mf::STATIC == 0
                    && read.initializer().is_some()
                {
                    requires_root = true;
                    break;
                }
            }
            if !requires_root {
                for parameter in self.source_list(node, parameters)? {
                    if self
                        .ast(parameter)?
                        .node(parameter)?
                        .modifier_flags(self.ast(parameter)?)?
                        & mf::PARAMETER_PROPERTY_MODIFIER
                        != 0
                    {
                        requires_root = true;
                        break;
                    }
                }
            }
        }
        if !requires_root {
            return Ok(());
        }
        let parent = ts_ast::utilities::walk_up_parenthesized_expressions(
            self.ast(call)?,
            self.ast(call)?.node(call)?.parent(),
        )?;
        let root = match parent {
            Some(parent) => {
                self.ast(parent)?.node(parent)?.kind() == K::ExpressionStatement
                    && self.ast(parent)?.node(parent)?.parent() == Some(body)
            }
            None => false,
        };
        if !root {
            self.error_at(Some(call), d::A_super_call_must_be_a_root_level_statement_within_a_constructor_of_a_derived_class_that_contains_initialized_properties_parameter_properties_or_private_identifiers, vec![])?;
            return Ok(());
        }
        let mut found = false;
        for statement in self.source_list(body, self.ast(body)?.node(body)?.statement_list())? {
            let read = self.ast(statement)?.node(statement)?;
            if read.kind() == K::ExpressionStatement {
                let mut expression = read
                    .expression()
                    .ok_or(Error::MissingLink("constructor statement expression"))?;
                loop {
                    let read = self.ast(expression)?.node(expression)?;
                    if !matches!(
                        read.kind().known(),
                        Some(
                            K::ParenthesizedExpression
                                | K::TypeAssertionExpression
                                | K::AsExpression
                                | K::NonNullExpression
                                | K::SatisfiesExpression
                                | K::PartiallyEmittedExpression
                                | K::ExpressionWithTypeArguments
                        )
                    ) {
                        break;
                    }
                    expression = read
                        .expression()
                        .ok_or(Error::MissingLink("outer super-call expression"))?;
                }
                if self.constructor_super_call(expression)? {
                    found = true;
                    break;
                }
            }
            if self.immediately_references_super_or_this(statement)? {
                break;
            }
        }
        if !found {
            self.error_at(Some(node), d::A_super_call_must_be_the_first_statement_in_the_constructor_to_refer_to_super_or_this_when_a_derived_class_contains_initialized_properties_parameter_properties_or_private_identifiers, vec![])?;
        }
        Ok(())
    }

    fn constructor_super_call(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.kind() != K::CallExpression {
            return Ok(false);
        }
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("super-call expression"))?;
        Ok(self.ast(expression)?.node(expression)?.kind() == K::SuperKeyword)
    }

    // port: tsc/internal/checker/checker.go:Checker.findFirstSuperCall
    fn first_constructor_super_call(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            if self.constructor_super_call(node)? {
                return Ok(Some(node));
            }
            if ts_ast::utilities::is_function_like(Some(&self.ast(node)?.node(node)?)) {
                continue;
            }
            pending.extend(self.source_children(node)?.into_iter().rev());
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:nodeImmediatelyReferencesSuperOrThis
    fn immediately_references_super_or_this(&self, node: NodeId) -> Result<bool, Error> {
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::SuperKeyword | K::ThisKeyword) => return Ok(true),
                Some(
                    K::ArrowFunction
                    | K::FunctionDeclaration
                    | K::FunctionExpression
                    | K::PropertyDeclaration,
                ) => continue,
                Some(K::Block) => {
                    if let Some(parent) = read.parent() {
                        if matches!(
                            self.ast(parent)?.node(parent)?.kind().known(),
                            Some(
                                K::Constructor
                                    | K::MethodDeclaration
                                    | K::GetAccessor
                                    | K::SetAccessor
                            )
                        ) {
                            continue;
                        }
                    }
                }
                _ => {}
            }
            pending.extend(self.source_children(node)?.into_iter().rev());
        }
        Ok(false)
    }
}
