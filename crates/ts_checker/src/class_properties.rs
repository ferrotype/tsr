//! Class property inference and accessor read/write types. Accessor caches use
//! the native resolution stack and preserve the separate setter write type.

use crate::{CheckerState, Error, TypeId, TypeSystemPropertyName};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};
use ts_diagnostics as messages;
use ts_jsstring::JsString;

impl CheckerState {
    pub(crate) fn declaration_of_kind(
        &self,
        symbol: SymbolId,
        kind: K,
    ) -> Result<Option<NodeId>, Error> {
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            if self.ast(declaration)?.node(declaration)?.kind() == kind {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/utilities.go:isPrivateWithinAmbient
    fn private_within_ambient(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::AMBIENT == 0 {
            return Ok(false);
        }
        let private_name = read
            .name()
            .map(|name| {
                self.ast(name)?
                    .node(name)
                    .map(|read| read.kind() == K::PrivateIdentifier)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        Ok(private_name || read.modifier_flags(self.ast(node)?)? & mf::PRIVATE != 0)
    }

    pub(crate) fn set_accessor_value_parameter(
        &self,
        node: NodeId,
    ) -> Result<Option<NodeId>, Error> {
        let parameters = self.source_list(node, self.ast(node)?.node(node)?.parameter_list())?;
        let Some(&first) = parameters.first() else {
            return Ok(None);
        };
        let is_this = if let Some(name) = self.ast(first)?.node(first)?.name() {
            self.ast(name)?.node(name)?.kind() == K::Identifier
                && self.ast(name)?.node_text(name)?.as_bytes() == b"this"
        } else {
            false
        };
        Ok(parameters.get(usize::from(is_this)).copied())
    }

    // port: tsc/internal/checker/checker.go:Checker.getAnnotatedAccessorTypeNode
    pub(crate) fn annotated_accessor_type_node(
        &self,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        let Some(node) = node else {
            return Ok(None);
        };
        let read = self.ast(node)?.node(node)?;
        match read.kind().known() {
            Some(K::GetAccessor | K::PropertyDeclaration) => Ok(read.type_node()),
            Some(K::SetAccessor) => self
                .set_accessor_value_parameter(node)?
                .map(|parameter| {
                    self.ast(parameter)?
                        .node(parameter)
                        .map(|read| read.type_node())
                        .map_err(Error::from)
                })
                .transpose()
                .map(Option::flatten),
            _ => Ok(None),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getAnnotatedAccessorType
    pub(crate) fn annotated_accessor_type(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<TypeId>, Error> {
        self.annotated_accessor_type_node(node)?
            .map(|annotation| self.get_type_from_type_node(annotation))
            .transpose()
    }

    fn auto_accessor_declaration(&self, symbol: SymbolId) -> Result<Option<NodeId>, Error> {
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            let read = self.ast(declaration)?.node(declaration)?;
            if read.kind() == K::PropertyDeclaration
                && read.modifier_flags(self.ast(declaration)?)? & mf::ACCESSOR != 0
            {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfAccessors
    pub(crate) fn type_of_accessors(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.resolved_type)
        {
            return Ok(ty);
        }
        if !self.push_source_resolution(symbol, TypeSystemPropertyName::Type) {
            return Ok(self.builtins.error_type);
        }
        let result: Result<_, Error> = (|| {
            let getter = self.declaration_of_kind(symbol, K::GetAccessor)?;
            let setter = self.declaration_of_kind(symbol, K::SetAccessor)?;
            let accessor = self.auto_accessor_declaration(symbol)?;
            let mut ty = self.annotated_accessor_type(getter)?;
            if ty.is_none() {
                ty = self.annotated_accessor_type(setter)?;
            }
            if ty.is_none() {
                ty = self.annotated_accessor_type(accessor)?;
            }
            if ty.is_none() {
                if let Some(getter) = getter {
                    if self.ast(getter)?.node(getter)?.body().is_some() {
                        ty = Some(self.return_type_from_body(getter)?);
                    }
                }
            }
            if ty.is_none() {
                if let Some(accessor) = accessor {
                    ty = Some(self.type_of_class_property(accessor)?);
                }
            }
            if ty.is_none() {
                for (declaration, diagnostic, member) in [
                    (setter, messages::Property_0_implicitly_has_type_any_because_its_set_accessor_lacks_a_parameter_type_annotation, false),
                    (getter, messages::Property_0_implicitly_has_type_any_because_its_get_accessor_lacks_a_return_type_annotation, false),
                    (accessor, messages::Member_0_implicitly_has_an_1_type, true),
                ] {
                    if let Some(declaration) = declaration {
                        if !self.private_within_ambient(declaration)? {
                            let mut args = vec![self.symbol_to_string(symbol)?];
                            if member { args.push(JsString::from_bytes(b"any".as_slice())); }
                            let diagnostic = self.diagnostic_for_node(Some(declaration), diagnostic, args)?;
                            let options = self.program()?.host.options();
                            if options.strict_option_value(options.no_implicit_any) { self.add_diagnostic(diagnostic)?; }
                            else { self.add_suggestion_diagnostic(diagnostic)?; }
                            break;
                        }
                    }
                }
            }
            Ok((
                ty.unwrap_or(self.builtins.any_type),
                getter,
                setter,
                accessor,
            ))
        })();
        let complete = self.resolution.pop();
        let (mut ty, getter, setter, accessor) = result?;
        if !complete {
            let annotated = if self.annotated_accessor_type_node(getter)?.is_some() {
                getter
            } else if self.annotated_accessor_type_node(setter)?.is_some() {
                setter
            } else if self.annotated_accessor_type_node(accessor)?.is_some() {
                // The pinned branch reports on `setter`, even for an auto
                // accessor. Do not invent a different diagnostic location.
                return Err(Error::Unsupported(
                    "getTypeOfAccessors: circular auto-accessor diagnostic path",
                ));
            } else {
                None
            };
            if let Some(declaration) = annotated {
                let name = self.symbol_to_string(symbol)?;
                self.error_at(
                    Some(declaration),
                    messages::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                    vec![name],
                )?;
            } else if let Some(getter) = getter {
                let options = self.program()?.host.options();
                if options.strict_option_value(options.no_implicit_any) {
                    let name = self.symbol_to_string(symbol)?;
                    self.error_at(Some(getter), messages::X_0_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions, vec![name])?;
                }
            }
            ty = self.builtins.any_type;
        }
        Ok(*self
            .value_symbol_links
            .get_or_default(symbol)
            .resolved_type
            .get_or_insert(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.getWriteTypeOfAccessors
    pub(crate) fn write_type_of_accessors(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.write_type)
        {
            return Ok(ty);
        }
        if !self.push_source_resolution(symbol, TypeSystemPropertyName::WriteType) {
            return Ok(self.builtins.error_type);
        }
        let result: Result<_, Error> = (|| {
            let setter = self.declaration_of_kind(symbol, K::SetAccessor)?;
            let setter = match setter {
                Some(_) => setter,
                None => self.auto_accessor_declaration(symbol)?,
            };
            Ok((self.annotated_accessor_type(setter)?, setter))
        })();
        let complete = self.resolution.pop();
        let (mut ty, setter) = result?;
        if !complete {
            if self.annotated_accessor_type_node(setter)?.is_some() {
                let name = self.symbol_to_string(symbol)?;
                self.error_at(
                    setter,
                    messages::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                    vec![name],
                )?;
            }
            ty = Some(self.builtins.any_type);
        }
        if let Some(ty) = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.write_type)
        {
            return Ok(ty);
        }
        let ty = match ty {
            Some(ty) => ty,
            None => self.type_of_accessors(symbol)?,
        };
        self.value_symbol_links.get_or_default(symbol).write_type = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeForVariableLikeDeclaration
    pub(crate) fn type_for_class_property_raw(
        &mut self,
        node: NodeId,
        include_optionality: bool,
        mode: u32,
    ) -> Result<Option<TypeId>, Error> {
        let read = self.ast(node)?.node(node)?;
        let modifiers = read.modifier_flags(self.ast(node)?)?;
        let optional = include_optionality && read.question_token(self.ast(node)?)?.is_some();
        let initializer = read.initializer();
        let mut ty = if let Some(annotation) = read.type_node() {
            Some(self.get_type_from_type_node(annotation)?)
        } else if initializer.is_some() {
            let ty = self.check_declaration_initializer(node, mode, None)?;
            Some(self.widen_type_inferred_from_initializer(node, ty)?)
        } else {
            None
        };
        if ty.is_none() {
            let options = self.program()?.host.options();
            if options.strict_option_value(options.no_implicit_any) {
                let class = self
                    .ast(node)?
                    .node(node)?
                    .parent()
                    .ok_or(Error::MissingLink("property class"))?;
                let members =
                    self.source_list(class, self.ast(class)?.node(class)?.member_list())?;
                let mut containers = Vec::new();
                for member in members {
                    let read = self.ast(member)?.node(member)?;
                    if modifiers & mf::STATIC == 0
                        && read.kind() == K::Constructor
                        && read.body().is_some()
                    {
                        containers.push(member);
                        break;
                    }
                    if modifiers & mf::STATIC != 0 && read.kind() == K::ClassStaticBlockDeclaration
                    {
                        containers.push(member);
                    }
                }
                if !containers.is_empty() {
                    let symbol = self
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("property inference symbol"))?;
                    ty = self.infer_class_property_flow(symbol, &containers)?;
                    return ty
                        .map(|ty| self.add_type_optionality(ty, true, optional))
                        .transpose();
                }
                if modifiers & mf::AMBIENT != 0 {
                    let symbol = self
                        .get_symbol_of_declaration(node)?
                        .ok_or(Error::MissingLink("property symbol"))?;
                    ty = self.type_of_property_in_base_class(symbol)?;
                }
            }
        }
        let Some(mut ty) = ty else {
            return Ok(None);
        };
        if optional && self.options.strict_null_checks {
            let absent = if modifiers & mf::ACCESSOR == 0 {
                self.builtins.undefined_or_missing_type
            } else {
                self.builtins.undefined_type
            };
            ty = self.get_union_type(&[ty, absent])?;
        }
        Ok(Some(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.widenTypeForVariableLikeDeclaration
    pub(crate) fn type_of_class_property(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let ty = self.type_for_class_property_raw(node, true, 0)?;
        self.widen_type_for_variable_like(node, ty, true)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfPropertyInBaseClass
    pub(crate) fn type_of_property_in_base_class(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Option<TypeId>, Error> {
        let Some(parent) = self.parent_of_symbol(symbol)? else {
            return Ok(None);
        };
        if self.symbol(parent)?.flags() & ts_ast::symbol_flags::CLASS == 0 {
            return Ok(None);
        }
        let class = self.get_declared_type_of_symbol(parent)?;
        let bases = self.interface_base_types(class)?;
        if let Some(&base) = bases.first() {
            let name = self.symbol(symbol)?.name_to_owned();
            self.property_type(base, name.as_bytes())
        } else {
            Ok(None)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertySignature
    pub(crate) fn check_property_signature(&mut self, node: NodeId) -> Result<(), Error> {
        if let Some(name) = self.ast(node)?.node(node)?.name() {
            if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier {
                self.error_at(
                    Some(node),
                    messages::Private_identifiers_are_not_allowed_outside_class_bodies,
                    vec![],
                )?;
            }
        }
        self.check_class_property(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertyDeclaration
    pub(crate) fn check_class_property(&mut self, node: NodeId) -> Result<(), Error> {
        let grammar_error = self.check_grammar_modifiers(node)?;
        let read = self.ast(node)?.node(node)?;
        let modifiers = read.modifier_flags(self.ast(node)?)?;
        let name = read.name().ok_or(Error::MissingLink("property name"))?;
        let initializer = read.initializer();
        if !grammar_error && !self.check_class_property_grammar(node)? {
            self.check_grammar_computed_property_name(name)?;
        }
        self.check_variable_initializer(node)?;
        if self.ast(name)?.node(name)?.kind() == K::PrivateIdentifier {
            self.set_node_links_for_private_identifier_scope(node)?;
        }
        if modifiers & mf::ABSTRACT != 0 && initializer.is_some() {
            let name = ts_scanner::declaration_name_to_string(self.ast(node)?, Some(name))?;
            self.error_at(
                Some(node),
                messages::Property_0_cannot_have_an_initializer_because_it_is_marked_abstract,
                vec![name],
            )?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarComputedPropertyName
    pub(crate) fn check_grammar_computed_property_name(
        &mut self,
        name: NodeId,
    ) -> Result<bool, Error> {
        if self.ast(name)?.node(name)?.kind() != K::ComputedPropertyName {
            return Ok(false);
        }
        let expression = self
            .ast(name)?
            .node(name)?
            .expression()
            .ok_or(Error::MissingLink("computed name expression"))?;
        if let Some(binary) = self
            .ast(expression)?
            .node(expression)?
            .data_source()
            .as_binary_expression()
        {
            let operator = binary
                .operator_token()
                .ok_or(Error::MissingLink("computed grammar operator"))?;
            if self.ast(operator)?.node(operator)?.kind() == K::CommaToken {
                return self.grammar_error_node(
                    expression,
                    messages::A_comma_expression_is_not_allowed_in_a_computed_property_name,
                    vec![],
                );
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarForInvalidDynamicName
    pub(crate) fn check_grammar_invalid_dynamic_name(
        &mut self,
        name: NodeId,
        message: &'static messages::Message,
    ) -> Result<bool, Error> {
        if !ts_ast::is_dynamic_name(self.ast(name)?, name)? {
            return Ok(false);
        }
        let read = self.ast(name)?.node(name)?;
        let computed = read.kind() == K::ComputedPropertyName;
        let expression = if computed {
            read.expression()
        } else {
            read.data_source()
                .as_element_access_expression()
                .and_then(|data| data.argument_expression())
        }
        .ok_or(Error::MissingLink("dynamic name expression"))?;
        // isLateBindableName evaluates only syntactic entity names.
        if ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
            let ty = self.late_name_type(name)?;
            if self.types.flags(ty)?
                & (crate::type_flags::STRING_OR_NUMBER_LITERAL
                    | crate::type_flags::UNIQUE_ES_SYMBOL)
                != 0
            {
                return Ok(false);
            }
        }
        let expression = if computed {
            expression
        } else {
            ts_ast::skip_parentheses(self.ast(expression)?, expression)?
        };
        if !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
            return self.grammar_error_node(name, message, vec![]);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarProperty
    fn check_class_property_grammar(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let name = read.name().ok_or(Error::MissingLink("property name"))?;
        let postfix = read.postfix_token();
        let initializer = read.initializer();
        let annotation = read.type_node();
        let ambient = read.flags() & nf::AMBIENT != 0;
        let modifiers = read.modifier_flags(self.ast(node)?)?;
        let parent = read.parent().ok_or(Error::MissingLink("property parent"))?;
        let class = matches!(
            self.ast(parent)?.node(parent)?.kind().known(),
            Some(K::ClassDeclaration | K::ClassExpression)
        );
        let name_read = self.ast(name)?.node(name)?;
        if name_read.kind() == K::ComputedPropertyName {
            if let Some(expression) = name_read.expression() {
                if let Some(binary) = self
                    .ast(expression)?
                    .node(expression)?
                    .data_source()
                    .as_binary_expression()
                {
                    let operator = binary
                        .operator_token()
                        .ok_or(Error::MissingLink("computed grammar operator"))?;
                    if self.ast(operator)?.node(operator)?.kind() == K::InKeyword {
                        let members = self
                            .source_list(parent, self.ast(parent)?.node(parent)?.member_list())?;
                        return self.grammar_error_node(
                            *members
                                .first()
                                .ok_or(Error::MissingLink("mapped grammar member"))?,
                            messages::A_mapped_type_may_not_declare_properties_or_methods,
                            vec![],
                        );
                    }
                }
            }
        }
        if class
            && self.ast(name)?.node(name)?.kind() == K::StringLiteral
            && self.ast(name)?.node_text(name)?.as_bytes() == b"constructor"
        {
            return self.grammar_error_node(
                name,
                messages::Classes_may_not_have_a_field_named_constructor,
                vec![],
            );
        }
        let dynamic_message = if class {
            Some(messages::A_computed_property_name_in_a_class_property_declaration_must_have_a_simple_literal_type_or_a_unique_symbol_type)
        } else if self.ast(parent)?.node(parent)?.kind() == K::InterfaceDeclaration {
            Some(messages::A_computed_property_name_in_an_interface_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type)
        } else if self.ast(parent)?.node(parent)?.kind() == K::TypeLiteral {
            Some(messages::A_computed_property_name_in_a_type_literal_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type)
        } else {
            None
        };
        if let Some(message) = dynamic_message {
            if self.check_grammar_invalid_dynamic_name(name, message)? {
                return Ok(true);
            }
        }
        if !class {
            if let Some(initializer) = initializer {
                let message = match self.ast(parent)?.node(parent)?.kind().known() {
                    Some(K::InterfaceDeclaration) => {
                        Some(messages::An_interface_property_cannot_have_an_initializer)
                    }
                    Some(K::TypeLiteral) => {
                        Some(messages::A_type_literal_property_cannot_have_an_initializer)
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    return self.grammar_error_node(initializer, message, vec![]);
                }
            }
        }
        if class && modifiers & mf::ACCESSOR != 0 {
            if let Some(token) = postfix {
                if self.ast(token)?.node(token)?.kind() == K::QuestionToken {
                    return self.grammar_error_node(
                        token,
                        messages::An_accessor_property_cannot_be_declared_optional,
                        vec![],
                    );
                }
            }
        }
        if ambient {
            self.check_ambient_initializer(node)?;
        }
        if let Some(token) = postfix {
            if self.ast(token)?.node(token)?.kind() == K::ExclamationToken {
                let message = if initializer.is_some() {
                    Some(messages::Declarations_with_initializers_cannot_also_have_definite_assignment_assertions)
                } else if annotation.is_none() {
                    Some(messages::Declarations_with_definite_assignment_assertions_must_also_have_type_annotations)
                } else if !class || ambient || modifiers & (mf::STATIC | mf::ABSTRACT) != 0 {
                    Some(messages::A_definite_assignment_assertion_is_not_permitted_in_this_context)
                } else {
                    None
                };
                if let Some(message) = message {
                    return self.grammar_error_node(token, message, vec![]);
                }
            }
        }
        Ok(false)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkAccessorDeclaration
    pub(crate) fn check_class_accessor(&mut self, node: NodeId) -> Result<(), Error> {
        if !self.check_grammar_function_like(node)? && !self.check_accessor_grammar(node)? {
            let name = self
                .ast(node)?
                .node(node)?
                .name()
                .ok_or(Error::MissingLink("accessor name"))?;
            self.check_grammar_computed_property_name(name)?;
        }
        let read = self.ast(node)?.node(node)?;
        let name = read.name().ok_or(Error::MissingLink("accessor name"))?;
        let getter_kind = read.kind() == K::GetAccessor;
        let flags = read.flags();
        let body = read.body();
        if self.ast(name)?.node(name)?.kind() == K::Identifier
            && self.ast(name)?.node_text(name)?.as_bytes() == b"constructor"
        {
            let parent = self
                .ast(node)?
                .node(node)?
                .parent()
                .ok_or(Error::MissingLink("accessor parent"))?;
            if matches!(
                self.ast(parent)?.node(parent)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) {
                self.error_at(
                    Some(name),
                    messages::Class_constructor_may_not_be_an_accessor,
                    vec![],
                )?;
            }
        }
        self.check_signature_syntax(node)?;
        if getter_kind
            && flags & nf::AMBIENT == 0
            && body.is_some()
            && flags & nf::HAS_IMPLICIT_RETURN != 0
            && flags & nf::HAS_EXPLICIT_RETURN == 0
        {
            self.error_at(
                Some(name),
                messages::A_get_accessor_must_return_a_value,
                vec![],
            )?;
        }
        if self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName {
            self.check_computed_property_name(name)?;
        }
        let bindable = !self.non_bindable_dynamic_name(name)?;
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("accessor symbol"))?;
        let getter = self.declaration_of_kind(symbol, K::GetAccessor)?;
        let setter = self.declaration_of_kind(symbol, K::SetAccessor)?;
        if let (Some(getter), Some(setter)) = (getter.filter(|_| bindable), setter) {
            if self.query.accessor_pairs_checked.insert(getter) {
                let getter_flags = self
                    .ast(getter)?
                    .node(getter)?
                    .modifier_flags(self.ast(getter)?)?;
                let setter_flags = self
                    .ast(setter)?
                    .node(setter)?
                    .modifier_flags(self.ast(setter)?)?;
                let getter_name = self.ast(getter)?.node(getter)?.name();
                let setter_name = self.ast(setter)?.node(setter)?.name();
                if getter_flags & mf::ABSTRACT != setter_flags & mf::ABSTRACT {
                    self.error_at(
                        getter_name,
                        messages::Accessors_must_both_be_abstract_or_non_abstract,
                        vec![],
                    )?;
                    self.error_at(
                        setter_name,
                        messages::Accessors_must_both_be_abstract_or_non_abstract,
                        vec![],
                    )?;
                }
                if getter_flags & mf::PROTECTED != 0
                    && setter_flags & (mf::PROTECTED | mf::PRIVATE) == 0
                    || getter_flags & mf::PRIVATE != 0 && setter_flags & mf::PRIVATE == 0
                {
                    self.error_at(
                        getter_name,
                        messages::A_get_accessor_must_be_at_least_as_accessible_as_the_setter,
                        vec![],
                    )?;
                    self.error_at(
                        setter_name,
                        messages::A_get_accessor_must_be_at_least_as_accessible_as_the_setter,
                        vec![],
                    )?;
                }
            }
        }
        let return_type = self.type_of_accessors(symbol)?;
        if getter_kind {
            self.check_function_return_paths(node, Some(return_type))?;
        }
        if let Some(body) = body {
            self.check_source_element(body)?;
        }
        self.set_node_links_for_private_identifier_scope(node)?;
        Ok(())
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarAccessor
    fn check_accessor_grammar(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let name = read.name().ok_or(Error::MissingLink("accessor name"))?;
        let body = read.body();
        let modifiers = read.modifier_flags(self.ast(node)?)?;
        let parent = read.parent().ok_or(Error::MissingLink("accessor parent"))?;
        let interface = matches!(
            self.ast(parent)?.node(parent)?.kind().known(),
            Some(K::TypeLiteral | K::InterfaceDeclaration)
        );
        if read.flags() & nf::AMBIENT == 0
            && !interface
            && body.is_none()
            && modifiers & mf::ABSTRACT == 0
        {
            let end = i64::from(read.end());
            return self.grammar_error_range_with_args(
                node,
                end - 1,
                end,
                messages::X_0_expected,
                vec![JsString::from_bytes(b"{".as_slice())],
            );
        }
        if let Some(body) = body {
            if modifiers & mf::ABSTRACT != 0 {
                return self.grammar_error_node(
                    node,
                    messages::An_abstract_accessor_cannot_have_an_implementation,
                    vec![],
                );
            }
            if interface {
                return self.grammar_error_node(
                    body,
                    messages::An_implementation_cannot_be_declared_in_ambient_contexts,
                    vec![],
                );
            }
        }
        if read.type_parameter_list().is_some() {
            return self.grammar_error_node(
                name,
                messages::An_accessor_cannot_have_type_parameters,
                vec![],
            );
        }
        let getter = read.kind() == K::GetAccessor;
        let parameters = self.source_list(node, read.parameter_list())?;
        let this = if parameters.len() == if getter { 1 } else { 2 } {
            let parameter = parameters[0];
            match self.ast(parameter)?.node(parameter)?.name() {
                Some(name) => {
                    self.ast(name)?.node(name)?.kind() == K::Identifier
                        && self.ast(name)?.node_text(name)?.as_bytes() == b"this"
                }
                None => false,
            }
        } else {
            false
        };
        if !this && parameters.len() != usize::from(!getter) {
            return self.grammar_error_node(
                name,
                if getter {
                    messages::A_get_accessor_cannot_have_parameters
                } else {
                    messages::A_set_accessor_must_have_exactly_one_parameter
                },
                vec![],
            );
        }
        if !getter {
            if read.type_node().is_some() {
                return self.grammar_error_node(
                    name,
                    messages::A_set_accessor_cannot_have_a_return_type_annotation,
                    vec![],
                );
            }
            let parameter = self
                .set_accessor_value_parameter(node)?
                .ok_or(Error::MissingLink("setter parameter after arity check"))?;
            let parameter_read = self.ast(parameter)?.node(parameter)?;
            let data = parameter_read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("setter parameter"))?;
            let rest = data.dot_dot_dot_token();
            let optional = data.question_token();
            let initializer = data.initializer();
            if let Some(rest) = rest {
                return self.grammar_error_node(
                    rest,
                    messages::A_set_accessor_cannot_have_rest_parameter,
                    vec![],
                );
            }
            if let Some(optional) = optional {
                return self.grammar_error_node(
                    optional,
                    messages::A_set_accessor_cannot_have_an_optional_parameter,
                    vec![],
                );
            }
            if initializer.is_some() {
                return self.grammar_error_node(
                    name,
                    messages::A_set_accessor_parameter_cannot_have_an_initializer,
                    vec![],
                );
            }
        }
        Ok(false)
    }
}
