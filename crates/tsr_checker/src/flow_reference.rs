//! Reference equivalence is symbol based, with source-qualified property paths.
use crate::symbols::NameBuf;
use crate::{CheckerState, Error};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};
fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    // Inline copy of `IsThisInTypeQuery`; its Phase 1 home is in tsr_ast (table group positions).
    pub(crate) fn flow_this_type_query(&mut self, node: NodeId) -> Result<bool, Error> {
        if let Some(&result) = self.flow.this_type_queries.get(&node) {
            return Ok(result);
        }
        let result = self.flow_this_type_query_worker(node)?;
        self.flow.this_type_queries.insert(node, result);
        Ok(result)
    }
    fn flow_this_type_query_worker(&self, mut node: NodeId) -> Result<bool, Error> {
        // Every ancestor is in the node's file: one view serves the walk.
        let view = self.ast(node)?;
        if view.node(node)?.kind() != K::Identifier || view.node_text(node)?.as_bytes() != b"this" {
            return Ok(false);
        }
        loop {
            let parent = required(view.node(node)?.parent(), "this type-query parent")?;
            let read = view.node(parent)?;
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
            tsr_ast::utilities::get_combined_node_flags(self.ast(declaration)?, declaration)?
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
        while self.node(source)?.flags() & nf::OPTIONAL_CHAIN != 0 {
            source = required(self.node(source)?.expression(), "optional-chain expression")?;
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
            self.node(source)?.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            source = required(self.node(source)?.expression(), "access expression")?;
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
        let read = self.node(call)?;
        let expression = required(read.expression(), "call expression")?;
        let args = self.source_list(call, read.argument_list())?;
        for arg in args {
            // port: tsc/internal/checker/flow.go:Checker.isOrContainsMatchingReference
            if self.matching_reference(reference, arg)?
                || self.contains_flow_reference(reference, arg)?
                || self.optional_chain_contains_reference(arg, reference)?
            {
                return Ok(true);
            }
        }
        let read = self.node(expression)?;
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
            let (kind, expression, node_parent) = {
                let read = self.node(node)?;
                let kind = read.kind();
                // `expression` exists only on access expressions; reading it on
                // another kind is a payload error.
                let expression = if matches!(
                    kind.known(),
                    Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                ) {
                    read.expression()
                } else {
                    None
                };
                (kind, expression, read.parent())
            };
            match kind.known() {
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
                        Some(decl) => self.node(decl)?.kind() == K::FunctionExpression,
                        None => false,
                    })
                }
                Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                    let expression = required(expression, "constant access expression")?;
                    if !self.constant_flow_reference(expression)? {
                        return Ok(false);
                    }
                    match self.query.resolved_symbols.try_get(node).copied().flatten() {
                        Some(symbol) => self.is_readonly_symbol(symbol),
                        None => Ok(false),
                    }
                }
                Some(K::ObjectBindingPattern | K::ArrayBindingPattern) => {
                    let parent = required(node_parent, "constant binding root")?;
                    let root = tsr_ast::utilities::get_root_declaration(self.ast(parent)?, parent)?;
                    let read = self.node(root)?;
                    let is_parameter = read.kind() == K::Parameter;
                    let is_variable = read.kind() == K::VariableDeclaration;
                    let is_catch = if let Some(parent) = read.parent() {
                        self.node(parent)?.kind() == K::CatchClause
                    } else {
                        false
                    };
                    if is_parameter || is_variable && is_catch {
                        return Ok(!self.some_binding_symbol_assigned(root)?);
                    }
                    Ok(
                        is_variable
                            && tsr_ast::utilities::is_var_const_like(self.ast(root)?, root)?,
                    )
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
        let read = self.node(target)?;
        let target_kind = read.kind();
        if matches!(
            target_kind.known(),
            Some(K::ParenthesizedExpression | K::NonNullExpression)
        ) {
            return self.matching_reference(
                source,
                required(read.expression(), "target reference operand")?,
            );
        }
        if target_kind == K::BinaryExpression {
            let binary = read
                .data_source()
                .as_binary_expression()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let left = required(binary.left(), "target binary left")?;
            let right = required(binary.right(), "target binary right")?;
            let operator = self
                .ast(target)?
                .node(required(binary.operator_token(), "target binary operator")?)?
                .kind();
            return Ok(
                tsr_ast::is_assignment_expression(self.ast(target)?, target, false)?
                    && self.matching_reference(source, left)?
                    || operator == K::CommaToken && self.matching_reference(source, right)?,
            );
        }
        let read = self.node(source)?;
        let kind = read.kind();
        match kind.known() {
            Some(K::MetaProperty) => {
                let target_id = target;
                let target = self.node(target)?;
                let Some(target_data) = target.data_source().as_meta_property() else {
                    return Ok(false);
                };
                let source_data = read
                    .data_source()
                    .as_meta_property()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
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
                    return Ok(target_kind == K::ThisKeyword);
                }
                let symbol = self.resolved_value_symbol(source)?;
                if target_kind == K::Identifier {
                    return Ok(symbol == self.resolved_value_symbol(target)?);
                }
                if matches!(
                    target_kind.known(),
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
            Some(K::ThisKeyword | K::SuperKeyword) => Ok(kind == target_kind),
            Some(K::ParenthesizedExpression | K::NonNullExpression | K::SatisfiesExpression) => {
                self.matching_reference(
                    required(read.expression(), "source reference operand")?,
                    target,
                )
            }
            Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                let base = required(read.expression(), "source property base")?;
                if let Some(name) = self.flow_property_name(source)? {
                    let target_read = self.node(target)?;
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
                let source_read = self.node(source)?;
                let target_read = self.node(target)?;
                if let (Some(source_data), Some(target_data)) = (
                    source_read.data_source().as_element_access_expression(),
                    target_read.data_source().as_element_access_expression(),
                ) {
                    let source_arg =
                        required(source_data.argument_expression(), "source element argument")?;
                    let target_arg =
                        required(target_data.argument_expression(), "target element argument")?;
                    let target_base = required(target_read.expression(), "target element base")?;
                    if self.node(source_arg)?.kind() == K::Identifier
                        && self.node(target_arg)?.kind() == K::Identifier
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
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let left = required(data.left(), "qualified left")?;
                let right = required(data.right(), "qualified right")?;
                let target_read = self.node(target)?;
                if matches!(
                    target_read.kind().known(),
                    Some(K::PropertyAccessExpression | K::ElementAccessExpression)
                ) {
                    let target_base = required(target_read.expression(), "qualified target base")?;
                    if let Some(name) = self.flow_property_name(target)? {
                        return Ok(self.node_text(right)?.as_bytes() == name.as_bytes()
                            && self.matching_reference(left, target_base)?);
                    }
                }
                Ok(false)
            }
            Some(K::BinaryExpression) => {
                let binary = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
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
        let read = self.node(access)?;
        match read.kind().known() {
            Some(K::PropertyAccessExpression) => {
                let name = required(read.name(), "accessed property name")?;
                Ok(Some(self.node_text(name)?.into_js_string()))
            }
            Some(K::ElementAccessExpression) => {
                let data = read
                    .data_source()
                    .as_element_access_expression()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let argument = required(data.argument_expression(), "element name argument")?;
                let read = self.node(argument)?;
                if matches!(
                    read.kind().known(),
                    Some(K::StringLiteral | K::NumericLiteral | K::NoSubstitutionTemplateLiteral)
                ) {
                    return Ok(Some(self.node_text(argument)?.into_js_string()));
                }
                if tsr_ast::is_entity_name_expression(self.ast(argument)?, argument)? {
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
                let parameters = self.source_list(parent, self.node(parent)?.parameter_list())?;
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
        let read = self.node(declaration)?;
        if let Some(annotation) = read.type_node() {
            let ty = self.get_type_from_type_node(annotation)?;
            if let Some(name) = self.index_property_name(ty)? {
                return Ok(Some(name));
            }
        }
        let kind = self.node(declaration)?.kind();
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
            let read = self.node(declaration)?;
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

/// A flow walk's reference read once: what `matching_reference` reads from
/// its source on every comparison, so each flow step reads only the target.
/// Built at the walk's first comparison (`FlowQuery::shape`), where the
/// first comparison would have read the reference.
pub(crate) struct ReferenceShape {
    /// The accesses from the reference inward, after the source-side peels
    /// (parentheses, non-null and satisfies expressions, comma operands).
    accesses: Vec<AccessStep>,
    root: ReferenceRoot,
    /// How many leading accesses `contains_flow_reference` would peel: the
    /// raw chain of access expressions from the reference, stopped by any
    /// other node, as its loop is.
    contains_candidates: usize,
}
struct AccessStep {
    /// The accessed property name (`getAccessedPropertyName`), when one.
    name: Option<NameBuf>,
    /// An element access's identifier argument, for the pin's second rule
    /// (same resolved symbol, constant or unassigned); resolved at the
    /// comparison, as the pin resolves it.
    element_argument: Option<NodeId>,
}
enum ReferenceRoot {
    /// `this` in a type query: only a `this` target matches.
    ThisTypeQuery,
    Identifier {
        symbol: SymbolId,
        exported: SymbolId,
    },
    This,
    Super,
    MetaProperty {
        keyword: tsr_ast::NodeKind,
        name: NameBuf,
    },
    /// A source kind the pin never matches.
    Unmatched,
}

impl CheckerState {
    /// The shape of `reference`: `matching_reference_worker`'s reading of its
    /// source, done once.
    // port: tsc/internal/checker/flow.go:Checker.isMatchingReference
    pub(crate) fn reference_shape(&mut self, reference: NodeId) -> Result<ReferenceShape, Error> {
        // The raw access chain `contains_flow_reference` peels.
        let mut contains_candidates = 0;
        let mut raw = reference;
        loop {
            let read = self.node(raw)?;
            if !matches!(
                read.kind().known(),
                Some(K::PropertyAccessExpression | K::ElementAccessExpression)
            ) {
                break;
            }
            contains_candidates += 1;
            raw = required(read.expression(), "access expression")?;
        }
        let mut accesses = Vec::new();
        let mut source = reference;
        let root = loop {
            let read = self.node(source)?;
            let kind = read.kind();
            match kind.known() {
                Some(
                    K::ParenthesizedExpression | K::NonNullExpression | K::SatisfiesExpression,
                ) => {
                    source = required(read.expression(), "source reference operand")?;
                }
                Some(K::BinaryExpression) => {
                    let binary = read
                        .data_source()
                        .as_binary_expression()
                        .ok_or(tsr_arena::Error::InvalidGraph)?;
                    let right = required(binary.right(), "source comma right")?;
                    let operator = self
                        .ast(source)?
                        .node(required(binary.operator_token(), "source comma operator")?)?
                        .kind();
                    if operator != K::CommaToken {
                        break ReferenceRoot::Unmatched;
                    }
                    source = right;
                }
                Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                    let base = required(read.expression(), "source property base")?;
                    let element_argument = read
                        .data_source()
                        .as_element_access_expression()
                        .and_then(|data| data.argument_expression());
                    let element_argument = match element_argument {
                        Some(argument) if self.node(argument)?.kind() == K::Identifier => {
                            Some(argument)
                        }
                        _ => None,
                    };
                    let name = self
                        .flow_property_name(source)?
                        .map(|name| NameBuf::new(name.as_bytes()));
                    accesses.push(AccessStep {
                        name,
                        element_argument,
                    });
                    source = base;
                }
                Some(K::QualifiedName) => {
                    let data = read
                        .data_source()
                        .as_qualified_name()
                        .ok_or(tsr_arena::Error::InvalidGraph)?;
                    let left = required(data.left(), "qualified left")?;
                    let right = required(data.right(), "qualified right")?;
                    let name = NameBuf::new(self.node_text(right)?.as_bytes());
                    accesses.push(AccessStep {
                        name: Some(name),
                        element_argument: None,
                    });
                    source = left;
                }
                Some(K::MetaProperty) => {
                    let data = read
                        .data_source()
                        .as_meta_property()
                        .ok_or(tsr_arena::Error::InvalidGraph)?;
                    let keyword = data.keyword_token();
                    let name = required(read.name(), "meta source name")?;
                    let name = NameBuf::new(self.ast(source)?.node_text(name)?.as_bytes());
                    break ReferenceRoot::MetaProperty { keyword, name };
                }
                Some(K::Identifier | K::PrivateIdentifier) => {
                    if self.flow_this_type_query(source)? {
                        break ReferenceRoot::ThisTypeQuery;
                    }
                    let symbol = self.resolved_value_symbol(source)?;
                    let record = self.symbol(symbol)?;
                    let exported = if record.flags() & sf::EXPORT_VALUE != 0 {
                        record.export_symbol().unwrap_or(symbol)
                    } else {
                        symbol
                    };
                    break ReferenceRoot::Identifier { symbol, exported };
                }
                Some(K::ThisKeyword) => break ReferenceRoot::This,
                Some(K::SuperKeyword) => break ReferenceRoot::Super,
                _ => break ReferenceRoot::Unmatched,
            }
        };
        Ok(ReferenceShape {
            accesses,
            root,
            contains_candidates,
        })
    }

    /// `matching_reference(reference, target)` with the reference's shape.
    pub(crate) fn matching_shape(
        &mut self,
        shape: &ReferenceShape,
        target: NodeId,
    ) -> Result<bool, Error> {
        self.matching_shape_at(shape, 0, target)
    }

    /// `contains_flow_reference(reference, target)` with the reference's shape.
    pub(crate) fn shape_contains(
        &mut self,
        shape: &ReferenceShape,
        target: NodeId,
    ) -> Result<bool, Error> {
        for peeled in 1..=shape.contains_candidates {
            if self.matching_shape_at(shape, peeled, target)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Match `target` against the reference with its first `index` accesses
    /// peeled: the worker's target-side cases, with the source's reads
    /// replaced by the shape.
    fn matching_shape_at(
        &mut self,
        shape: &ReferenceShape,
        index: usize,
        target: NodeId,
    ) -> Result<bool, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.matching_shape_worker(shape, index, target)
        })
    }
    fn matching_shape_worker(
        &mut self,
        shape: &ReferenceShape,
        index: usize,
        target: NodeId,
    ) -> Result<bool, Error> {
        let read = self.node(target)?;
        let target_kind = read.kind();
        if matches!(
            target_kind.known(),
            Some(K::ParenthesizedExpression | K::NonNullExpression)
        ) {
            return self.matching_shape_at(
                shape,
                index,
                required(read.expression(), "target reference operand")?,
            );
        }
        if target_kind == K::BinaryExpression {
            let binary = read
                .data_source()
                .as_binary_expression()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let left = required(binary.left(), "target binary left")?;
            let right = required(binary.right(), "target binary right")?;
            let operator = self
                .ast(target)?
                .node(required(binary.operator_token(), "target binary operator")?)?
                .kind();
            return Ok(
                tsr_ast::is_assignment_expression(self.ast(target)?, target, false)?
                    && self.matching_shape_at(shape, index, left)?
                    || operator == K::CommaToken && self.matching_shape_at(shape, index, right)?,
            );
        }
        let Some(step) = shape.accesses.get(index) else {
            return Ok(match &shape.root {
                ReferenceRoot::ThisTypeQuery | ReferenceRoot::This => target_kind == K::ThisKeyword,
                ReferenceRoot::Identifier { symbol, exported } => {
                    if target_kind == K::Identifier {
                        *symbol == self.resolved_value_symbol(target)?
                    } else if matches!(
                        target_kind.known(),
                        Some(K::VariableDeclaration | K::BindingElement)
                    ) {
                        Some(*exported) == self.get_symbol_of_declaration(target)?
                    } else {
                        false
                    }
                }
                ReferenceRoot::Super => target_kind == K::SuperKeyword,
                ReferenceRoot::MetaProperty { keyword, name } => {
                    match read.data_source().as_meta_property() {
                        Some(data) if data.keyword_token() == *keyword => {
                            let target_name = required(read.name(), "meta target name")?;
                            self.ast(target)?.node_text(target_name)?.as_bytes() == name.as_bytes()
                        }
                        _ => false,
                    }
                }
                ReferenceRoot::Unmatched => false,
            });
        };
        if !matches!(
            target_kind.known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return Ok(false);
        }
        let target_base = required(read.expression(), "target property base")?;
        if let Some(name) = &step.name {
            if let Some(target_name) = self.flow_property_name(target)? {
                return Ok(name.as_bytes() == target_name.as_bytes()
                    && self.matching_shape_at(shape, index + 1, target_base)?);
            }
        }
        if let Some(source_arg) = step.element_argument {
            let target_read = self.node(target)?;
            if let Some(target_data) = target_read.data_source().as_element_access_expression() {
                let target_arg =
                    required(target_data.argument_expression(), "target element argument")?;
                if self.node(target_arg)?.kind() == K::Identifier {
                    let symbol = self.resolved_value_symbol(source_arg)?;
                    if symbol == self.resolved_value_symbol(target_arg)?
                        && (self.is_constant_flow_variable(symbol)?
                            || self.is_parameter_or_mutable_local_variable(symbol)?
                                && !self.is_symbol_assigned(symbol)?)
                    {
                        return self.matching_shape_at(shape, index + 1, target_base);
                    }
                }
            }
        }
        Ok(false)
    }
}
