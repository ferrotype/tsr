//! Property/element value accesses share the symbol and flow operations used by
//! indexed types. Diagnostics retain the native source node and evaluation order.
use crate::flow_assignments::AssignmentKind;
use crate::{
    access_flags as af, object_flags as of, type_flags as tf, CheckerState, Error, TypeId,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as d;

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    pub(crate) fn access_receiver(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        Ok(match read.data_source().as_qualified_name() {
            Some(data) => data.left(),
            None => read.expression(),
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIndexedAccess
    // port: tsc/internal/checker/checker.go:Checker.checkElementAccessExpression
    pub(crate) fn check_element_access(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let chain = read.flags() & nf::OPTIONAL_CHAIN != 0;
        let left = required(read.expression(), "element receiver")?;
        let left_type = self.check_expression(left)?;
        let non_optional = if chain {
            self.optional_expression_type(left_type, left)?
        } else {
            left_type
        };
        let object = self.check_non_null_type(non_optional, left)?;
        let result = self.element_access_worker(node, object)?;
        if chain {
            self.propagate_optional_type_marker(result, node, non_optional != left_type)
        } else {
            Ok(result)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkElementAccessExpression
    fn element_access_worker(&mut self, node: NodeId, mut object: TypeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let index = required(
            read.data_source()
                .as_element_access_expression()
                .ok_or(Error::MissingLink("element access payload"))?
                .argument_expression(),
            "element index",
        )?;
        let assignment = self.assignment_target_kind(node)?;
        if assignment != AssignmentKind::None || self.access_is_call_target(node)? {
            object = self.widened_type(object)?;
        }
        let mut index_type = self.check_expression(index)?;
        if object == self.builtins.error_type || object == self.builtins.silent_never_type {
            return Ok(object);
        }
        let symbol = self.types.get(object)?.symbol;
        let constant_enum = symbol
            .map(|symbol| {
                self.symbol(symbol)
                    .map(|read| read.flags() & sf::CONST_ENUM != 0)
            })
            .transpose()?
            .unwrap_or(false);
        if constant_enum
            && !matches!(
                self.ast(index)?.node(index)?.kind().known(),
                Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
            )
        {
            self.error_at(
                Some(index),
                d::A_const_enum_member_can_only_be_accessed_using_a_string_literal,
                vec![],
            )?;
            return Ok(self.builtins.error_type);
        }
        if self.for_in_numeric_property_variable(index)? {
            index_type = self.builtins.number_type;
        }
        let flags = if assignment == AssignmentKind::None {
            af::EXPRESSION_POSITION
        } else {
            let this = self.types.flags(object)? & tf::TYPE_PARAMETER != 0
                && self.types.type_parameter(object)?.is_this_type;
            let generic = self.get_generic_object_flags(object)? & of::IS_GENERIC_OBJECT_TYPE != 0;
            af::WRITING
                | if assignment == AssignmentKind::Compound {
                    af::EXPRESSION_POSITION
                } else {
                    0
                }
                | if generic && !this {
                    af::NO_INDEX_SIGNATURES
                } else {
                    0
                }
        };
        let ty = self.get_indexed_access_type(object, index_type, flags, Some(node), None)?;
        let property = self.query.resolved_symbols.try_get(node).copied().flatten();
        let ty =
            self.flow_type_of_access_expression(node, property, ty, index, self.expression_mode)?;
        self.check_indexed_value_type(ty, node)
    }

    // port: tsc/internal/checker/checker.go:Checker.isForInVariableForNumericPropertyNames
    fn for_in_numeric_property_variable(&mut self, expression: NodeId) -> Result<bool, Error> {
        let mut node = expression;
        while self.ast(node)?.node(node)?.kind() == K::ParenthesizedExpression {
            node = required(self.access_receiver(node)?, "index parentheses")?;
        }
        if self.ast(node)?.node(node)?.kind() != K::Identifier {
            return Ok(false);
        }
        let symbol = self.resolved_value_symbol(node)?;
        if self.symbol(symbol)?.flags() & sf::VARIABLE == 0 {
            return Ok(false);
        }
        let mut child = expression;
        let mut parent = self.ast(expression)?.node(expression)?.parent();
        while let Some(node) = parent {
            let read = self.ast(node)?.node(node)?;
            if read.kind() == K::ForInStatement {
                let data = read
                    .data_source()
                    .as_for_in_or_of_statement()
                    .ok_or(Error::MissingLink("for-in payload"))?;
                if data.statement() == Some(child) {
                    let initializer = required(data.initializer(), "for-in initializer")?;
                    let expression = required(data.expression(), "for-in expression")?;
                    let read = self.ast(initializer)?.node(initializer)?;
                    let variable = if read.kind() == K::VariableDeclarationList {
                        let declarations = self.source_list(
                            initializer,
                            read.data_source()
                                .as_variable_declaration_list()
                                .ok_or(Error::MissingLink("for-in declaration list"))?
                                .declarations(),
                        )?;
                        match declarations.first() {
                            Some(&declaration) => {
                                let name = required(
                                    self.ast(declaration)?.node(declaration)?.name(),
                                    "for-in name",
                                )?;
                                if matches!(
                                    self.ast(name)?.node(name)?.kind().known(),
                                    Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                                ) {
                                    None
                                } else {
                                    self.get_symbol_of_declaration(declaration)?
                                }
                            }
                            None => None,
                        }
                    } else if read.kind() == K::Identifier {
                        Some(self.resolved_value_symbol(initializer)?)
                    } else {
                        None
                    };
                    if variable == Some(symbol) {
                        let ty = self.get_type_of_expression(expression)?;
                        let indexes = self.index_infos_of_type(ty)?;
                        if indexes.len() == 1
                            && self.signatures.index_info(indexes[0])?.key_type
                                == self.builtins.number_type
                        {
                            return Ok(true);
                        }
                    }
                }
            }
            child = node;
            parent = self.ast(node)?.node(node)?.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIndexedAccessIndexType
    pub(crate) fn check_indexed_value_type(
        &mut self,
        ty: TypeId,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::INDEXED_ACCESS == 0 {
            return Ok(ty);
        }
        let data = *self.types.indexed_access(ty)?;
        let mut remapped = false;
        if self.is_generic_mapped_type(data.object_type)? {
            if let Some(name) = self.mapped_name(data.object_type)? {
                let parameter = self.mapped_parameter(data.object_type)?;
                remapped =
                    !self.is_type_related_to(name, parameter, crate::RelationKind::Assignable)?;
            }
        }
        let key = if remapped {
            self.index_type_for_mapped(data.object_type, 0)?
        } else {
            self.get_index_type(data.object_type, 0)?
        };
        let mut number = false;
        for index in self.index_infos_of_type(data.object_type)? {
            number |= self.signatures.index_info(index)?.key_type == self.builtins.number_type;
        }
        let parts = if self.types.flags(data.index_type)? & tf::UNION != 0 {
            self.types.compound_types(data.index_type)?.to_vec()
        } else {
            vec![data.index_type]
        };
        let mut valid = true;
        for part in parts {
            if !(self.is_type_related_to(part, key, crate::RelationKind::Assignable)?
                || number && self.applicable_index_type(part, self.builtins.number_type)?)
            {
                valid = false;
                break;
            }
        }
        if valid {
            if self.ast(node)?.node(node)?.kind() == K::ElementAccessExpression
                && self.assignment_target_kind(node)? != AssignmentKind::None
                && self.types.object_flags(data.object_type)? & of::MAPPED != 0
                && self.mapped_modifiers(data.object_type)? & crate::mapped::INCLUDE_READONLY != 0
            {
                let display =
                    self.type_to_string(data.object_type, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(
                    Some(node),
                    d::Index_signature_in_type_0_only_permits_reading,
                    vec![display],
                )?;
            }
            return Ok(ty);
        }
        if self.get_generic_object_flags(data.object_type)? & of::IS_GENERIC_OBJECT_TYPE != 0 {
            if let Some(name) = self.index_property_name(data.index_type)? {
                let apparent = self.apparent_type(data.object_type)?;
                let parts = self.distributed_types(apparent)?;
                let mut property = None;
                for part in parts {
                    property = self.constituent_property(part, name.as_bytes(), false)?;
                    if property.is_some() {
                        break;
                    }
                }
                if let Some(property) = property {
                    if self.property_modifiers(property)? & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER
                        != 0
                    {
                        self.error_at(
                            Some(node),
                            d::Private_or_protected_member_0_cannot_be_accessed_on_a_type_parameter,
                            vec![name],
                        )?;
                        return Ok(self.builtins.error_type);
                    }
                }
            }
        }
        let index = self.type_to_string(data.index_type, crate::type_display::DEFAULT_FLAGS)?;
        let object = self.type_to_string(data.object_type, crate::type_display::DEFAULT_FLAGS)?;
        self.error_at(
            Some(node),
            d::Type_0_cannot_be_used_to_index_type_1,
            vec![index, object],
        )?;
        Ok(self.builtins.error_type)
    }

    // port: tsc/internal/checker/checker.go:Checker.isMethodAccessForCall
    pub(crate) fn access_is_call_target(&self, mut node: NodeId) -> Result<bool, Error> {
        while let Some(parent) = self.ast(node)?.node(node)?.parent() {
            let read = self.ast(parent)?.node(parent)?;
            if read.kind() == K::ParenthesizedExpression {
                node = parent;
                continue;
            }
            return Ok(matches!(
                read.kind().known(),
                Some(K::CallExpression | K::NewExpression)
            ) && read.expression() == Some(node));
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertyAccessExpression
    // port: tsc/internal/checker/checker.go:Checker.checkQualifiedName
    pub(crate) fn check_property_access(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_property_access_ex(node, self.expression_mode, false)
    }

    pub(crate) fn check_property_access_ex(
        &mut self,
        node: NodeId,
        mode: u32,
        write_only: bool,
    ) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let chain = read.flags() & nf::OPTIONAL_CHAIN != 0;
        let (left, right) = match read.data_source().as_qualified_name() {
            Some(data) => (data.left(), data.right()),
            None => (read.expression(), read.name()),
        };
        let left = required(left, "property receiver")?;
        let right = required(right, "property name")?;
        let ty = self.check_expression(left)?;
        let non_optional = if chain {
            self.optional_expression_type(ty, left)?
        } else {
            ty
        };
        let left_type = self.check_non_null_type(non_optional, left)?;
        let result =
            self.property_access_worker(node, left, left_type, right, mode, !chain && write_only)?;
        if chain {
            self.propagate_optional_type_marker(result, node, non_optional != ty)
        } else {
            Ok(result)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertyAccessExpressionOrQualifiedName
    fn property_access_worker(
        &mut self,
        node: NodeId,
        left: NodeId,
        left_type: TypeId,
        right: NodeId,
        mode: u32,
        write_only: bool,
    ) -> Result<TypeId, Error> {
        let assignment = self.assignment_target_kind(node)?;
        let widened = if assignment != AssignmentKind::None || self.access_is_call_target(node)? {
            self.widened_type(left_type)?
        } else {
            left_type
        };
        let apparent = self.apparent_type(widened)?;
        let any = self.types.flags(apparent)? & tf::ANY != 0
            || apparent == self.builtins.silent_never_type;
        let private = self.ast(right)?.node(right)?.kind() == K::PrivateIdentifier;
        let name = self.ast(right)?.node_text(right)?.into_js_string();
        let property = if private {
            match self.private_access_property(node, left_type, apparent, right, assignment)? {
                crate::private_access::PrivateAccessResult::Type(ty) => return Ok(ty),
                crate::private_access::PrivateAccessResult::Property(property) => property,
            }
        } else {
            if any {
                // markPropertyAliasReferenced also runs for an unresolved alias:
                // following `alias.member` must retain/check its import chain.
                if self.ast(left)?.node(left)?.kind() == K::Identifier
                    && self.ast(left)?.node_text(left)?.as_bytes() != b"this"
                {
                    let mut ancestor = Some(node);
                    let mut import_reference = false;
                    while let Some(current) = ancestor {
                        let read = self.ast(current)?.node(current)?;
                        if read.kind() == K::ImportEqualsDeclaration {
                            import_reference = true;
                            break;
                        }
                        if !matches!(
                            read.kind().known(),
                            Some(K::Identifier | K::QualifiedName | K::PropertyAccessExpression)
                        ) {
                            break;
                        }
                        ancestor = read.parent();
                    }
                    if !import_reference {
                        let parent = self.resolved_value_symbol(left)?;
                        if parent != self.builtins.unknown_symbol {
                            self.mark_alias_referenced_at(node, parent)?;
                        }
                    }
                }
                return Ok(if self.is_error_type(apparent)? {
                    self.builtins.error_type
                } else {
                    apparent
                });
            }
            let skip_augment = self.const_enum_object_type(apparent)?;
            let qualified = self.ast(node)?.node(node)?.kind() == K::QualifiedName;
            self.constituent_property_ex(apparent, name.as_bytes(), skip_augment, qualified)?
        };
        let ty = if let Some(property) = property {
            self.check_property_use_before_declaration(property, node, right)?;
            let parent = self.query.resolved_symbols.try_get(left).copied().flatten();
            self.mark_access_property_referenced(property, node, left, parent)?;
            *self.query.resolved_symbols.get_or_default(node) = Some(property);
            self.check_access_property_accessibility(
                node,
                self.ast(left)?.node(left)?.kind() == K::SuperKeyword,
                ts_ast::utilities::is_write_access(self.ast(node)?, node)?,
                apparent,
                property,
                Some(right),
            )?;
            if self.assignment_to_readonly_property(node, property, assignment)? {
                self.error_at(
                    Some(right),
                    d::Cannot_assign_to_0_because_it_is_a_read_only_property,
                    vec![name],
                )?;
                return Ok(self.builtins.error_type);
            }
            if self.this_property_access_in_constructor(node, property)? {
                self.builtins.auto_type
            } else if write_only || ts_ast::utilities::is_write_only_access(self.ast(node)?, node)?
            {
                self.write_type_of_symbol(property)?
            } else {
                self.get_type_of_symbol(property)?
            }
        } else {
            let this = self.types.flags(left_type)? & tf::TYPE_PARAMETER != 0
                && self.types.type_parameter(left_type)?.is_this_type;
            let generic =
                self.get_generic_object_flags(left_type)? & of::IS_GENERIC_OBJECT_TYPE != 0;
            let index = if !private && (assignment == AssignmentKind::None || !generic || this) {
                let key = self.get_string_literal_type(name.clone())?;
                self.applicable_index_info(apparent, key)?
            } else {
                None
            };
            let Some(index) = index else {
                let left_symbol = self.types.get(left_type)?.symbol;
                let unchecked_js =
                    self.is_unchecked_js_suggestion(Some(node), left_symbol, true)?;
                if !unchecked_js && self.is_js_literal_type(left_type)? {
                    return Ok(self.builtins.any_type);
                }
                if self.types.get(left_type)?.symbol == Some(self.builtins.global_this_symbol) {
                    let exports = self.symbol(self.builtins.global_this_symbol)?.exports();
                    let global = self.member_symbol(exports, name.as_bytes())?;
                    if global
                        .map(|symbol| {
                            self.symbol(symbol)
                                .map(|read| read.flags() & sf::BLOCK_SCOPED != 0)
                        })
                        .transpose()?
                        .unwrap_or(false)
                    {
                        let display =
                            self.type_to_string(left_type, crate::type_display::DEFAULT_FLAGS)?;
                        self.error_at(
                            Some(right),
                            d::Property_0_does_not_exist_on_type_1,
                            vec![name, display],
                        )?;
                    } else if self
                        .program()?
                        .host
                        .options()
                        .strict_option_value(self.program()?.host.options().no_implicit_any)
                    {
                        let display =
                            self.type_to_string(left_type, crate::type_display::DEFAULT_FLAGS)?;
                        self.error_at(Some(right),d::Element_implicitly_has_an_any_type_because_type_0_has_no_index_signature,vec![display])?;
                    }
                    return Ok(self.builtins.any_type);
                }
                if !name.as_bytes().is_empty() {
                    self.defer_missing_property_ex(
                        right,
                        if this { apparent } else { left_type },
                        unchecked_js,
                    );
                }
                return Ok(self.builtins.error_type);
            };
            let index = self.signatures.index_info(index)?.clone();
            if index.is_readonly
                && (assignment != AssignmentKind::None || self.access_is_delete_target(node)?)
            {
                let display = self.type_to_string(apparent, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(
                    Some(node),
                    d::Index_signature_in_type_0_only_permits_reading,
                    vec![display],
                )?;
            }
            let mut ty = index.value_type;
            if self
                .program()?
                .host
                .options()
                .no_unchecked_indexed_access
                .is_true()
                && assignment != AssignmentKind::Definite
            {
                ty = self.get_union_type(&[ty, self.builtins.missing_type])?;
            }
            if self
                .program()?
                .host
                .options()
                .no_property_access_from_index_signature
                .is_true()
                && self.ast(node)?.node(node)?.kind() == K::PropertyAccessExpression
            {
                self.error_at(
                    Some(right),
                    d::Property_0_comes_from_an_index_signature_so_it_must_be_accessed_with_0,
                    vec![name],
                )?;
            }
            ty
        };
        self.flow_type_of_access_expression(node, property, ty, right, mode)
    }

    // port: tsc/internal/checker/utilities.go:isDeleteTarget
    pub(crate) fn access_is_delete_target(&self, mut node: NodeId) -> Result<bool, Error> {
        while let Some(parent) = self.ast(node)?.node(node)?.parent() {
            let read = self.ast(parent)?.node(parent)?;
            if read.kind() == K::ParenthesizedExpression {
                node = parent;
                continue;
            }
            return Ok(read.kind() == K::DeleteExpression && read.expression() == Some(node));
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getFlowTypeOfAccessExpression
    pub(crate) fn flow_type_of_access_expression(
        &mut self,
        node: NodeId,
        property: Option<SymbolId>,
        mut ty: TypeId,
        error_node: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let assignment = self.assignment_target_kind(node)?;
        if assignment == AssignmentKind::Definite {
            let optional = property
                .map(|property| {
                    self.symbol(property)
                        .map(|read| read.flags() & sf::OPTIONAL != 0)
                })
                .transpose()?
                .unwrap_or(false);
            return self.remove_missing_type(ty, optional);
        }
        if let Some(property) = property {
            let flags = self.symbol(property)?.flags();
            if flags & (sf::VARIABLE | sf::PROPERTY | sf::ACCESSOR) == 0
                && !(flags & sf::METHOD != 0 && self.types.flags(ty)? & tf::UNION != 0)
            {
                return Ok(ty);
            }
        }
        if ty == self.builtins.auto_type {
            return self.flow_type_of_access_property(node, property);
        }
        ty = self.narrowable_reference_type(ty, node, mode)?;
        let mut assume = false;
        if self.options.strict_null_checks {
            if let Some(property) = property {
                if let Some(declaration) = self.symbol(property)?.value_declaration() {
                    let read = self.ast(declaration)?.node(declaration)?;
                    let options = self.program()?.host.options();
                    let left = self.access_receiver(node)?;
                    let is_this = left
                        .map(|left| {
                            self.ast(left)?
                                .node(left)
                                .map(|read| read.kind() == K::ThisKeyword)
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false);
                    let without_initializer = read.kind() == K::PropertyDeclaration
                        && read.modifier_flags(self.ast(declaration)?)?
                            & (mf::ABSTRACT | mf::STATIC)
                            == 0
                        && read.initializer().is_none()
                        && read
                            .postfix_token()
                            .map(|token| {
                                self.ast(token)?
                                    .node(token)
                                    .map(|read| read.kind() != K::ExclamationToken)
                                    .map_err(Error::from)
                            })
                            .transpose()?
                            .unwrap_or(true);
                    if options.strict_option_value(options.strict_property_initialization)
                        && is_this
                        && without_initializer
                    {
                        let container = self.control_flow_container(node)?;
                        assume = self.ast(container)?.node(container)?.kind() == K::Constructor
                            && self.ast(container)?.node(container)?.parent() == read.parent()
                            && read.flags() & nf::AMBIENT == 0;
                    } else if let Some(binary) = read.data_source().as_binary_expression() {
                        let left = binary
                            .left()
                            .ok_or(Error::MissingLink("assignment property left"))?;
                        assume = self.ast(left)?.node(left)?.kind() == K::PropertyAccessExpression
                            && self.control_flow_container(node)?
                                == self.control_flow_container(declaration)?;
                    }
                }
            }
        }
        let initial = self.add_type_optionality(ty, false, assume)?;
        let flow = self.flow_type_of_reference_with_container(node, ty, initial, None)?;
        if assume
            && !self.class_type_contains_undefined(ty)?
            && self.class_type_contains_undefined(flow)?
        {
            let property = required(property, "uninitialized property")?;
            let name = self.symbol_to_string(property)?;
            self.error_at(
                Some(error_node),
                d::Property_0_is_used_before_being_assigned,
                vec![name],
            )?;
            return Ok(ty);
        }
        if assignment == AssignmentKind::None {
            Ok(flow)
        } else {
            self.base_literal_type(flow)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getFlowTypeOfProperty
    fn flow_type_of_access_property(
        &mut self,
        node: NodeId,
        property: Option<SymbolId>,
    ) -> Result<TypeId, Error> {
        let mut initial = self.builtins.undefined_type;
        if let Some(property) = property {
            if let Some(declaration) = self.symbol(property)?.value_declaration() {
                if !self.auto_typed_property(property)?
                    || self
                        .ast(declaration)?
                        .node(declaration)?
                        .modifier_flags(self.ast(declaration)?)?
                        & mf::AMBIENT
                        != 0
                {
                    if let Some(base) = self.type_of_property_in_base_class(property)? {
                        initial = base;
                    }
                }
            }
        }
        self.flow_type_of_reference_with_container(node, self.builtins.auto_type, initial, None)
    }

    // port: tsc/internal/checker/checker.go:Checker.isAutoTypedProperty
    fn auto_typed_property(&self, property: SymbolId) -> Result<bool, Error> {
        let Some(declaration) = self.symbol(property)?.value_declaration() else {
            return Ok(false);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        Ok(read.kind() == K::PropertyDeclaration
            && read.type_node().is_none()
            && read.initializer().is_none()
            && self
                .program()?
                .host
                .options()
                .strict_option_value(self.program()?.host.options().no_implicit_any))
    }

    // port: tsc/internal/checker/checker.go:Checker.isThisPropertyAccessInConstructor
    pub(crate) fn this_property_access_in_constructor(
        &mut self,
        node: NodeId,
        property: SymbolId,
    ) -> Result<bool, Error> {
        let constructor =
            if let crate::assignment_declarations::ThisAssignment::Constructor(constructor) =
                self.constructor_this_assignment(property)?
            {
                Some(constructor)
            } else {
                let is_this = match self.access_receiver(node)? {
                    Some(left) => self.ast(left)?.node(left)?.kind() == K::ThisKeyword,
                    None => false,
                };
                if is_this && self.auto_typed_property(property)? {
                    self.declaring_constructor(property)?
                } else {
                    None
                }
            };
        Ok(constructor
            == Some(ts_ast::get_this_container(
                self.ast(node)?,
                node,
                true,
                false,
            )?))
    }

    // port: tsc/internal/checker/checker.go:Checker.isAssignmentToReadonlyEntity
    pub(crate) fn assignment_to_readonly_property(
        &mut self,
        node: NodeId,
        property: SymbolId,
        assignment: AssignmentKind,
    ) -> Result<bool, Error> {
        if assignment == AssignmentKind::None {
            return Ok(false);
        }
        let left = self.access_receiver(node)?;
        let mut receiver = left;
        while let Some(value) = receiver {
            if self.ast(value)?.node(value)?.kind() != K::ParenthesizedExpression {
                break;
            }
            receiver = self.ast(value)?.node(value)?.expression();
        }
        let receiver_symbol = match receiver {
            Some(receiver) if self.ast(receiver)?.node(receiver)?.kind() == K::Identifier => {
                Some(self.resolved_value_symbol(receiver)?)
            }
            _ => None,
        };
        if receiver_symbol
            .map(|symbol| {
                self.symbol(symbol)
                    .map(|read| read.flags() & sf::MODULE_EXPORTS != 0)
            })
            .transpose()?
            .unwrap_or(false)
        {
            return Ok(false);
        }
        if self.is_readonly_symbol(property)? {
            if self.symbol(property)?.flags() & sf::PROPERTY != 0
                && left
                    .map(|left| {
                        self.ast(left)?
                            .node(left)
                            .map(|read| read.kind() == K::ThisKeyword)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false)
            {
                let constructor = self.control_flow_container(node)?;
                if self.ast(constructor)?.node(constructor)?.kind() != K::Constructor {
                    return Ok(true);
                }
                if let Some(declaration) = self.symbol(property)?.value_declaration() {
                    let read = self.ast(declaration)?.node(declaration)?;
                    let class = self.ast(constructor)?.node(constructor)?.parent();
                    let local_declaration =
                        read.parent() == class || read.parent() == Some(constructor);
                    let local_assignment = if read.kind() == K::BinaryExpression {
                        let parent = self
                            .symbol(property)?
                            .parent()
                            .ok_or(Error::MissingLink("assignment property symbol parent"))?;
                        let declaration = self.symbol(parent)?.value_declaration();
                        declaration == class || declaration == Some(constructor)
                    } else {
                        false
                    };
                    return Ok(!local_declaration && !local_assignment);
                }
            }
            return Ok(true);
        }
        if receiver_symbol
            .map(|symbol| {
                self.symbol(symbol)
                    .map(|read| read.flags() & sf::ALIAS != 0)
            })
            .transpose()?
            .unwrap_or(false)
        {
            let symbol =
                receiver_symbol.ok_or(Error::MissingLink("namespace assignment receiver"))?;
            return self
                .alias_declaration_or_none(symbol)?
                .map(|declaration| {
                    Ok(self.ast(declaration)?.node(declaration)?.kind() == K::NamespaceImport)
                })
                .transpose()
                .map(|value| value.unwrap_or(false));
        }
        Ok(false)
    }
}
