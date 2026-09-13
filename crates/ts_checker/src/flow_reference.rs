//! Reference equivalence is symbol based, with source-qualified property paths.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};
fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    // port: tsc/internal/ast/utilities.go:IsThisInTypeQuery
    pub(crate) fn flow_this_type_query(&self, mut node: NodeId) -> Result<bool, Error> {
        if self.ast(node)?.node(node)?.kind() != K::Identifier
            || self.ast(node)?.node_text(node)?.as_bytes() != b"this"
        {
            return Ok(false);
        }
        loop {
            let parent = required(
                self.ast(node)?.node(node)?.parent(),
                "this type-query parent",
            )?;
            let read = self.ast(parent)?.node(parent)?;
            if let Some(qualified) = read.data_source().as_qualified_name() {
                if qualified.left() == Some(node) {
                    node = parent;
                    continue;
                }
            }
            return Ok(read.kind() == K::TypeQuery);
        }
    }

    // port: tsc/internal/checker/utilities.go:Checker.isConstantVariable
    pub(crate) fn is_constant_flow_variable(&self, symbol: SymbolId) -> Result<bool, Error> {
        if self.symbol(symbol)?.flags() & sf::VARIABLE == 0 {
            return Ok(false);
        }
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        Ok(
            ts_ast::utilities::get_combined_node_flags(self.ast(declaration)?, declaration)?
                & nf::CONSTANT
                != 0,
        )
    }

    // port: tsc/internal/checker/flow.go:Checker.optionalChainContainsReference
    pub(crate) fn optional_chain_contains_reference(
        &mut self,
        mut source: NodeId,
        target: NodeId,
    ) -> Result<bool, Error> {
        while self.ast(source)?.node(source)?.flags() & nf::OPTIONAL_CHAIN != 0 {
            source = required(
                self.ast(source)?.node(source)?.expression(),
                "optional-chain expression",
            )?;
            if self.matching_reference(source, target)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/flow.go:Checker.containsMatchingReference
    pub(crate) fn contains_flow_reference(
        &mut self,
        mut source: NodeId,
        target: NodeId,
    ) -> Result<bool, Error> {
        while matches!(
            self.ast(source)?.node(source)?.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            source = required(
                self.ast(source)?.node(source)?.expression(),
                "access expression",
            )?;
            if self.matching_reference(source, target)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    // port: tsc/internal/checker/flow.go:Checker.hasMatchingArgument
    pub(crate) fn flow_has_matching_argument(
        &mut self,
        call: NodeId,
        reference: NodeId,
    ) -> Result<bool, Error> {
        let read = self.ast(call)?.node(call)?;
        let expression = required(read.expression(), "call expression")?;
        let args = self.source_list(call, read.argument_list())?;
        for arg in args {
            if self.matching_reference(reference, arg)?
                || self.contains_flow_reference(reference, arg)?
                || self.optional_chain_contains_reference(arg, reference)?
            {
                return Ok(true);
            }
        }
        let read = self.ast(expression)?.node(expression)?;
        if read.kind() == K::PropertyAccessExpression {
            let target = required(read.expression(), "call access object")?;
            return Ok(self.matching_reference(reference, target)?
                || self.contains_flow_reference(reference, target)?);
        }
        Ok(false)
    }
    // port: tsc/internal/checker/flow.go:Checker.isConstantReference
    pub(crate) fn constant_flow_reference(&mut self, node: NodeId) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::ThisKeyword) => Ok(true),
                Some(K::Identifier) if !self.flow_this_type_query(node)? => {
                    let symbol = self.resolved_value_symbol(node)?;
                    if self.is_constant_flow_variable(symbol)?
                        || self.is_parameter_or_mutable_local_variable(symbol)?
                            && !self.is_symbol_assigned(symbol)?
                    {
                        return Ok(true);
                    }
                    Ok(match self.symbol(symbol)?.value_declaration() {
                        Some(decl) => self.ast(decl)?.node(decl)?.kind() == K::FunctionExpression,
                        None => false,
                    })
                }
                Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                    let expression = required(read.expression(), "constant access expression")?;
                    if !self.constant_flow_reference(expression)? {
                        return Ok(false);
                    }
                    match self.query.resolved_symbols.try_get(node).copied().flatten() {
                        Some(symbol) => self.is_readonly_symbol(symbol),
                        None => Ok(false),
                    }
                }
                Some(K::ObjectBindingPattern | K::ArrayBindingPattern) => {
                    let parent = required(read.parent(), "constant binding root")?;
                    let root = ts_ast::utilities::get_root_declaration(self.ast(parent)?, parent)?;
                    let read = self.ast(root)?.node(root)?;
                    let is_parameter = read.kind() == K::Parameter;
                    let is_variable = read.kind() == K::VariableDeclaration;
                    let is_catch = if let Some(parent) = read.parent() {
                        self.ast(parent)?.node(parent)?.kind() == K::CatchClause
                    } else {
                        false
                    };
                    if is_parameter || is_variable && is_catch {
                        return Ok(!self.some_binding_symbol_assigned(root)?);
                    }
                    Ok(is_variable && ts_ast::utilities::is_var_const_like(self.ast(root)?, root)?)
                }
                _ => Ok(false),
            }
        })
    }

    // port: tsc/internal/checker/flow.go:Checker.isMatchingReference
    pub(crate) fn matching_reference(
        &mut self,
        source: NodeId,
        target: NodeId,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.matching_reference_worker(source, target)
        })
    }
    fn matching_reference_worker(&mut self, source: NodeId, target: NodeId) -> Result<bool, Error> {
        let read = self.ast(target)?.node(target)?;
        if matches!(
            read.kind().known(),
            Some(K::ParenthesizedExpression | K::NonNullExpression)
        ) {
            return self.matching_reference(
                source,
                required(read.expression(), "target reference operand")?,
            );
        }
        if let Some(binary) = read.data_source().as_binary_expression() {
            let left = required(binary.left(), "target binary left")?;
            let right = required(binary.right(), "target binary right")?;
            let operator = self
                .ast(target)?
                .node(required(binary.operator_token(), "target binary operator")?)?
                .kind();
            return Ok(
                ts_ast::is_assignment_expression(self.ast(target)?, target, false)?
                    && self.matching_reference(source, left)?
                    || operator == K::CommaToken && self.matching_reference(source, right)?,
            );
        }
        let read = self.ast(source)?.node(source)?;
        let kind = read.kind();
        match kind.known() {
            Some(K::MetaProperty) => {
                let target_id = target;
                let target = self.ast(target)?.node(target)?;
                let Some(target_data) = target.data_source().as_meta_property() else {
                    return Ok(false);
                };
                let source_data = read
                    .data_source()
                    .as_meta_property()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                Ok(source_data.keyword_token() == target_data.keyword_token()
                    && self
                        .ast(source)?
                        .node_text(required(read.name(), "meta source name")?)?
                        .as_bytes()
                        == self
                            .ast(target_id)?
                            .node_text(required(target.name(), "meta target name")?)?
                            .as_bytes())
            }
            Some(K::Identifier | K::PrivateIdentifier) => {
                if self.flow_this_type_query(source)? {
                    return Ok(self.ast(target)?.node(target)?.kind() == K::ThisKeyword);
                }
                let symbol = self.resolved_value_symbol(source)?;
                let target_read = self.ast(target)?.node(target)?;
                if target_read.kind() == K::Identifier {
                    return Ok(symbol == self.resolved_value_symbol(target)?);
                }
                if matches!(
                    target_read.kind().known(),
                    Some(K::VariableDeclaration | K::BindingElement)
                ) {
                    let record = self.symbol(symbol)?;
                    let symbol = if record.flags() & sf::EXPORT_VALUE != 0 {
                        record.export_symbol().unwrap_or(symbol)
                    } else {
                        symbol
                    };
                    return Ok(Some(symbol) == self.get_symbol_of_declaration(target)?);
                }
                Ok(false)
            }
            Some(K::ThisKeyword | K::SuperKeyword) => {
                Ok(kind == self.ast(target)?.node(target)?.kind())
            }
            Some(K::ParenthesizedExpression | K::NonNullExpression | K::SatisfiesExpression) => {
                self.matching_reference(
                    required(read.expression(), "source reference operand")?,
                    target,
                )
            }
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                let base = required(read.expression(), "source property base")?;
                if let Some(name) = self.flow_property_name(source)? {
                    let target_read = self.ast(target)?.node(target)?;
                    if matches!(
                        target_read.kind().known(),
                        Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                    ) {
                        let target_base =
                            required(target_read.expression(), "target property base")?;
                        if let Some(target_name) = self.flow_property_name(target)? {
                            return Ok(name == target_name
                                && self.matching_reference(base, target_base)?);
                        }
                    }
                }
                let source_read = self.ast(source)?.node(source)?;
                let target_read = self.ast(target)?.node(target)?;
                if let (Some(source_data), Some(target_data)) = (
                    source_read.data_source().as_element_access_expression(),
                    target_read.data_source().as_element_access_expression(),
                ) {
                    let source_arg =
                        required(source_data.argument_expression(), "source element argument")?;
                    let target_arg =
                        required(target_data.argument_expression(), "target element argument")?;
                    let target_base = required(target_read.expression(), "target element base")?;
                    if self.ast(source_arg)?.node(source_arg)?.kind() == K::Identifier
                        && self.ast(target_arg)?.node(target_arg)?.kind() == K::Identifier
                    {
                        let symbol = self.resolved_value_symbol(source_arg)?;
                        if symbol == self.resolved_value_symbol(target_arg)?
                            && (self.is_constant_flow_variable(symbol)?
                                || self.is_parameter_or_mutable_local_variable(symbol)?
                                    && !self.is_symbol_assigned(symbol)?)
                        {
                            return self.matching_reference(base, target_base);
                        }
                    }
                }
                Ok(false)
            }
            Some(K::QualifiedName) => {
                let data = read
                    .data_source()
                    .as_qualified_name()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let left = required(data.left(), "qualified left")?;
                let right = required(data.right(), "qualified right")?;
                let target_read = self.ast(target)?.node(target)?;
                if matches!(
                    target_read.kind().known(),
                    Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                ) {
                    let target_base = required(target_read.expression(), "qualified target base")?;
                    if let Some(name) = self.flow_property_name(target)? {
                        return Ok(self.ast(right)?.node_text(right)?.as_bytes()
                            == name.as_bytes()
                            && self.matching_reference(left, target_base)?);
                    }
                }
                Ok(false)
            }
            Some(K::BinaryExpression) => {
                let binary = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let right = required(binary.right(), "source comma right")?;
                let operator = self
                    .ast(source)?
                    .node(required(binary.operator_token(), "source comma operator")?)?
                    .kind();
                Ok(operator == K::CommaToken && self.matching_reference(right, target)?)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/flow.go:Checker.getAccessedPropertyName
    // port: tsc/internal/checker/flow.go:Checker.tryGetElementAccessExpressionName
    pub(crate) fn flow_property_name(&mut self, access: NodeId) -> Result<Option<JsString>, Error> {
        let read = self.ast(access)?.node(access)?;
        match read.kind().known() {
            Some(K::PropertyAccessExpression) => {
                let name = required(read.name(), "accessed property name")?;
                Ok(Some(self.ast(name)?.node_text(name)?.into_js_string()))
            }
            Some(K::ElementAccessExpression) => {
                let data = read
                    .data_source()
                    .as_element_access_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let argument = required(data.argument_expression(), "element name argument")?;
                let read = self.ast(argument)?.node(argument)?;
                if matches!(
                    read.kind().known(),
                    Some(K::StringLiteral | K::NumericLiteral | K::NoSubstitutionTemplateLiteral)
                ) {
                    return Ok(Some(
                        self.ast(argument)?.node_text(argument)?.into_js_string(),
                    ));
                }
                if ts_ast::is_entity_name_expression(self.ast(argument)?, argument)? {
                    let Some(symbol) = self.resolve_entity_name(argument, sf::VALUE, true)? else {
                        return Ok(None);
                    };
                    if self.is_constant_flow_variable(symbol)?
                        || self.symbol(symbol)?.flags() & sf::ENUM_MEMBER != 0
                    {
                        return self.flow_name_from_entity(argument, symbol);
                    }
                }
                Ok(None)
            }
            Some(K::BindingElement) => self.destructuring_property_name(access),
            Some(K::Parameter) => {
                let parent = required(read.parent(), "parameter parent")?;
                let parameters =
                    self.source_list(parent, self.ast(parent)?.node(parent)?.parameter_list())?;
                let index = parameters
                    .iter()
                    .position(|&p| p == access)
                    .map_or(-1, |p| p as isize);
                Ok(Some(JsString::from_bytes(index.to_string().as_bytes())))
            }
            _ => Ok(None),
        }
    }
    // port: tsc/internal/checker/flow.go:Checker.tryGetNameFromEntityNameExpression
    fn flow_name_from_entity(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
    ) -> Result<Option<JsString>, Error> {
        let Some(declaration) = self.symbol(symbol)?.value_declaration() else {
            return Ok(None);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        if let Some(annotation) = read.type_node() {
            let ty = self.get_type_from_type_node(annotation)?;
            if let Some(name) = self.index_property_name(ty)? {
                return Ok(Some(name));
            }
        }
        let kind = self.ast(declaration)?.node(declaration)?.kind();
        let has_expression_initializer = matches!(
            kind.known(),
            Some(
                K::VariableDeclaration
                    | K::Parameter
                    | K::PropertyDeclaration
                    | K::EnumMember
                    | K::PropertyAssignment
                    | K::BindingElement
            )
        );
        if has_expression_initializer
            && kind != K::BindingElement
            && self.name_declared_before_use(declaration, node)?
        {
            let read = self.ast(declaration)?.node(declaration)?;
            if let Some(initializer) = read.initializer() {
                let ty = self.get_type_of_expression(initializer)?;
                return self.index_property_name(ty);
            }
            if read.kind() == K::EnumMember {
                let name = required(read.name(), "enum access name")?;
                let ty = self.literal_type_from_property_name(name)?;
                return self.index_property_name(ty);
            }
        }
        Ok(None)
    }
}
