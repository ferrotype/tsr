//! JavaScript assignment declarations use their whole declaration set, including
//! constructor flow, prototype writes and CommonJS export initialization order.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{JSDeclarationKind as J, SyntaxKind as K};

#[derive(Clone, Copy, Default)]
pub(crate) enum ThisAssignment {
    #[default]
    None,
    Typed(NodeId),
    Constructor(NodeId),
    Method,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isReadonlyAssignmentDeclaration
    pub(crate) fn is_readonly_assignment_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        if read.kind() != K::CallExpression {
            return Ok(false);
        }
        let args = read
            .argument_list()
            .ok_or(Error::MissingLink("property descriptor arguments"))?;
        let descriptor = view
            .node_slice(view.list(args)?.nodes())?
            .get(2)
            .flatten()
            .ok_or(Error::MissingLink("property descriptor"))?;
        let ty = self.check_expression_cached(descriptor)?;
        if self.property_type(ty, b"value")?.is_some() {
            let Some(writable) = self.constituent_property(ty, b"writable", false)? else {
                return Ok(true);
            };
            let initializer = if let Some(declaration) = self.symbol(writable)?.value_declaration()
            {
                let read = self.ast(declaration)?.node(declaration)?;
                if read.kind() == K::PropertyAssignment {
                    read.initializer()
                } else {
                    None
                }
            } else {
                None
            };
            let writable_type = if let Some(initializer) = initializer {
                self.check_expression(initializer)?
            } else {
                self.get_type_of_symbol(writable)?
            };
            return Ok(self.types.flags(writable_type)? & tf::BOOLEAN_LITERAL != 0
                && matches!(
                    self.types.literal(writable_type)?.value,
                    crate::LiteralValue::Boolean(false)
                ));
        }
        Ok(self.property_type(ty, b"set")?.is_none())
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaringConstructor
    pub(crate) fn declaring_constructor(&self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            let container =
                ts_ast::get_this_container(self.ast(declaration)?, declaration, false, false)?;
            if self.ast(container)?.node(container)?.kind() == K::Constructor {
                return Ok(Some(container));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.isConstructorDeclaredThisProperty
    pub(crate) fn constructor_this_assignment(
        &mut self,
        symbol: SymbolId,
    ) -> Result<ThisAssignment, Error> {
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(ThisAssignment::None);
        };
        if self.ast(declaration)?.node(declaration)?.kind() != K::BinaryExpression {
            return Ok(ThisAssignment::None);
        }
        if let Some(&cached) = self.query.this_assignments.get(&symbol) {
            return Ok(cached);
        }
        let mut all_this = true;
        let mut annotation = None;
        for declaration in self
            .symbol_declarations(symbol)?
            .to_vec()
            .into_iter()
            .flatten()
        {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() != K::BinaryExpression
                || ts_ast::get_assignment_declaration_kind(self.ast(declaration)?, declaration)?
                    != J::ThisProperty
            {
                all_this = false;
                break;
            }
            let data = read
                .data_source()
                .as_binary_expression()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            let left = data
                .left()
                .ok_or(Error::MissingLink("this declaration left"))?;
            if let Some(access) = self
                .ast(left)?
                .node(left)?
                .data_source()
                .as_element_access_expression()
            {
                let argument = access
                    .argument_expression()
                    .ok_or(Error::MissingLink("this declaration key"))?;
                if !matches!(
                    self.ast(argument)?.node(argument)?.kind().known(),
                    Some(K::StringLiteral | K::NumericLiteral | K::NoSubstitutionTemplateLiteral)
                ) {
                    all_this = false;
                    break;
                }
            }
            if read.type_node().is_some() {
                annotation = read.type_node();
            }
        }
        let result = if !all_this {
            ThisAssignment::None
        } else if let Some(annotation) = annotation {
            ThisAssignment::Typed(annotation)
        } else if let Some(constructor) = self.declaring_constructor(symbol)? {
            ThisAssignment::Constructor(constructor)
        } else {
            ThisAssignment::Method
        };
        self.query.this_assignments.insert(symbol, result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getWidenedTypeForAssignmentDeclaration
    pub(crate) fn widened_assignment_declaration_type(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        let kind = self.constructor_this_assignment(symbol)?;
        let mut ty = match kind {
            ThisAssignment::Typed(annotation) => Some(self.get_type_from_type_node(annotation)?),
            ThisAssignment::Constructor(constructor) => {
                self.infer_class_property_flow(symbol, &[constructor])?
            }
            ThisAssignment::Method => self.type_of_property_in_base_class(symbol)?,
            ThisAssignment::None => None,
        };
        if ty.is_none() {
            let declarations = self.symbol_declarations(symbol)?.to_vec();
            let mut types = Vec::new();
            for (index, declaration) in declarations.iter().copied().enumerate() {
                let Some(declaration) = declaration else {
                    continue;
                };
                let read = self.ast(declaration)?.node(declaration)?;
                if read.kind() == K::BinaryExpression {
                    if let Some(annotation) = read.type_node() {
                        ty = Some(self.get_type_from_type_node(annotation)?);
                        break;
                    }
                }
                if let Some(assigned) = self.assignment_declaration_initializer_type(declaration)? {
                    if (ts_ast::get_assignment_declaration_kind(
                        self.ast(declaration)?,
                        declaration,
                    )? != J::ExportsProperty
                        || index != 0
                        || declarations.len() == 1
                        || self.types.flags(assigned)? & tf::UNDEFINED == 0)
                        && !types.contains(&assigned)
                    {
                        types.push(assigned);
                    }
                }
            }
            if matches!(kind, ThisAssignment::Method)
                && !types.is_empty()
                && self.options.strict_null_checks
                && !types.contains(&self.builtins.undefined_or_missing_type)
            {
                types.push(self.builtins.undefined_or_missing_type);
            }
            if ty.is_none() {
                ty = Some(if types.is_empty() {
                    self.builtins.any_type
                } else {
                    self.get_union_type(&types)?
                });
            }
        }
        let ty = self.widened_type(ty.expect("assignment declaration type"))?;
        if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
            if self.ast(declaration)?.node(declaration)?.flags()
                & ts_ast::node_flags::JAVA_SCRIPT_FILE
                != 0
            {
                let filtered = self.filter_type(ty, &mut |checker, ty| {
                    Ok(checker.types.flags(ty)? & !tf::NULLABLE != 0)
                })?;
                if filtered == self.builtins.never_type {
                    self.report_implicit_any(declaration, self.builtins.any_type)?;
                    return Ok(self.builtins.any_type);
                }
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getAssignmentDeclarationInitializerType
    fn assignment_declaration_initializer_type(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        if let Some(binary) = read.data_source().as_binary_expression() {
            let left = binary
                .left()
                .ok_or(Error::MissingLink("assignment declaration left"))?;
            let mut right = binary
                .right()
                .ok_or(Error::MissingLink("assignment declaration right"))?;
            let kind = ts_ast::get_assignment_declaration_kind(self.ast(node)?, node)?;
            let ty = if matches!(kind, J::ModuleExports | J::ExportsProperty) {
                while let Some(binary) = self
                    .ast(right)?
                    .node(right)?
                    .data_source()
                    .as_binary_expression()
                {
                    let operator = binary
                        .operator_token()
                        .ok_or(Error::MissingLink("assigned operator"))?;
                    if self.ast(operator)?.node(operator)?.kind() != K::EqualsToken {
                        break;
                    }
                    right = binary.right().ok_or(Error::MissingLink("assigned right"))?;
                }
                let ty = self.check_expression_cached(right)?;
                self.get_regular_type_of_literal_type(ty)?
            } else {
                if kind == J::ThisProperty && self.contains_same_this_property(left, right)? {
                    return Ok(None);
                }
                let mode = std::mem::replace(&mut self.expression_mode, 0);
                let result = self.check_expression_for_mutable_location(right);
                self.expression_mode = mode;
                result?
            };
            if self.is_array_type(ty)? {
                let empty = if self.options.strict_null_checks {
                    self.builtins.implicit_never_type
                } else {
                    self.builtins.undefined_widening_type
                };
                if self.get_type_arguments(ty)?.first() == Some(&empty)
                    && !self.assignment_parent_has_annotation(node)?
                {
                    let array = self.any_array_type()?;
                    self.report_implicit_any(node, array)?;
                    return Ok(Some(array));
                }
            }
            return Ok(Some(ty));
        }
        if read.kind() == K::CallExpression {
            let arguments = self.source_list(node, read.argument_list())?;
            let descriptor = *arguments
                .get(2)
                .ok_or(Error::MissingLink("property descriptor argument"))?;
            return self.type_from_property_descriptor(descriptor).map(Some);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.hasParentWithTypeAnnotation
    fn assignment_parent_has_annotation(&mut self, node: NodeId) -> Result<bool, Error> {
        let Some(symbol) = self.raw_declaration_symbol(node)? else {
            return Ok(false);
        };
        let Some(parent) = self.symbol(symbol)?.parent() else {
            return Ok(false);
        };
        let Some(declaration) = self.symbol(parent)?.value_declaration() else {
            return Ok(false);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        if !matches!(
            read.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ) {
            return Ok(false);
        }
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        let Some(symbol) = self.raw_declaration_symbol(parent)? else {
            return Ok(false);
        };
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        Ok(self
            .ast(declaration)?
            .node(declaration)?
            .type_node()
            .is_some())
    }

    // port: tsc/internal/checker/checker.go:Checker.containsSameNamedThisProperty
    fn contains_same_this_property(
        &mut self,
        property: NodeId,
        expression: NodeId,
    ) -> Result<bool, Error> {
        let mut pending = vec![expression];
        while let Some(node) = pending.pop() {
            if self.matching_reference(property, node)? {
                return Ok(true);
            }
            if ts_ast::utilities::is_function_like(Some(&self.ast(node)?.node(node)?)) {
                continue;
            }
            pending.extend(self.source_children(node)?.into_iter().rev());
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromPropertyDescriptor
    fn type_from_property_descriptor(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let ty = self.check_expression_cached(node)?;
        if let Some(value) = self.property_type(ty, b"value")? {
            return Ok(value);
        }
        if let Some(getter) = self.property_type(ty, b"get")? {
            if let Some(signature) = self.call_single_signature(getter)? {
                return self.return_type_of_signature(signature);
            }
        }
        if let Some(setter) = self.property_type(ty, b"set")? {
            if let Some(signature) = self.call_single_signature(setter)? {
                return Ok(self
                    .parameter_type_at(signature, 0)?
                    .unwrap_or(self.builtins.never_type));
            }
        }
        Ok(self.builtins.any_type)
    }
}
