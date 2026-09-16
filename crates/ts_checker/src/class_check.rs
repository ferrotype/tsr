//! Class declaration checks share type relations, override rules and body-flow.
//! Unported emit/decorator operations remain explicit boundaries.

use crate::{object_flags as of, type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, symbol_flags as sf, SyntaxKind as K};
use ts_diagnostics as messages;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkClassDeclaration
    // port: tsc/internal/checker/checker.go:Checker.checkClassLikeDeclaration
    pub(crate) fn check_class_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.ast(node)?.node(node)?;
        let modifiers = read.modifier_flags(self.ast(node)?)?;
        let ambient = read.flags() & nf::AMBIENT != 0;
        let name = read.name();
        if name.is_none()
            && modifiers & mf::DEFAULT == 0
            && self.ast(node)?.node(node)?.kind() == K::ClassDeclaration
        {
            self.grammar_error_first_token(
                node,
                messages::A_class_declaration_without_the_default_modifier_must_have_a_name,
                vec![],
            )?;
        }
        if !self.check_class_heritage_grammar(node)? {
            self.check_grammar_type_parameter_list(node)?;
        }
        for modifier in self.source_list(node, self.ast(node)?.node(node)?.modifiers())? {
            if self.ast(modifier)?.node(modifier)?.kind() == K::Decorator {
                return Err(Error::Unsupported("checkClassLikeDeclaration: decorators"));
            }
        }
        self.check_collisions_for_declaration_name(node)?;
        self.check_type_parameters(node)?;
        self.check_exports_on_merged_declarations(node)?;
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("class declaration symbol"))?;
        let class = self.get_declared_type_of_symbol(symbol)?;
        let this = self
            .types
            .interface(class)?
            .this_type
            .ok_or(Error::MissingLink("class this type"))?;
        let with_this = self.get_type_with_this_argument(class, this, false)?;
        let static_type = self.get_type_of_symbol(symbol)?;
        self.check_class_or_interface_type_parameters_identical(symbol)?;
        self.check_function_or_constructor_symbol(symbol)?;
        self.check_object_duplicate_declarations(node, true)?;
        let members = self.source_list(node, self.ast(node)?.node(node)?.member_list())?;
        for &member in &members {
            let member_read = self.ast(member)?.node(member)?;
            let flags = member_read.modifier_flags(self.ast(member)?)?;
            if !ambient
                && !self.program()?.host.options().use_define_for_class_fields()
                && flags & mf::STATIC != 0
            {
                if let Some(member_name) = self.ast(member)?.node(member)?.name() {
                    let Some(text) = self.effective_property_name(member_name)? else {
                        continue;
                    };
                    if matches!(
                        text.as_bytes(),
                        b"name" | b"length" | b"caller" | b"arguments"
                    ) {
                        let name = self.symbol_to_string(symbol)?;
                        self.error_at(Some(member_name), messages::Static_property_0_conflicts_with_built_in_property_Function_0_of_constructor_function_1, vec![text, name])?;
                    }
                }
            }
        }
        let bases = self.class_heritage_nodes(node, K::ExtendsKeyword)?;
        if let Some(&base_node) = bases.first() {
            for argument in self.source_list(
                base_node,
                self.ast(base_node)?.node(base_node)?.type_argument_list(),
            )? {
                self.check_source_element(argument)?;
            }
            let resolved = self.interface_base_types(class)?;
            if let Some(&base) = resolved.first() {
                self.check_jsdoc_class_extends(node, base_node, base)?;
                let constructor = self.class_base_constructor_type(class)?;
                let static_base = self.apparent_type(constructor)?;
                let signatures = self.signatures_of_type(static_base, true)?;
                if let Some((_, declaring_class)) =
                    self.constructor_accessibility_error(base_node, &signatures, mf::PRIVATE)?
                {
                    let symbol = self
                        .types
                        .get(declaring_class)?
                        .symbol
                        .ok_or(Error::MissingLink("private base class symbol"))?;
                    let name = self.fully_qualified_name(symbol, None)?;
                    self.error_at(
                        Some(base_node),
                        messages::Cannot_extend_a_class_0_Class_constructor_is_marked_as_private,
                        vec![name],
                    )?;
                }
                let expression = self
                    .ast(base_node)?
                    .node(base_node)?
                    .expression()
                    .ok_or(Error::MissingLink("class extends expression"))?;
                self.check_expression(expression)?;
                if !self
                    .source_list(
                        base_node,
                        self.ast(base_node)?.node(base_node)?.type_argument_list(),
                    )?
                    .is_empty()
                {
                    for argument in self.source_list(
                        base_node,
                        self.ast(base_node)?.node(base_node)?.type_argument_list(),
                    )? {
                        self.check_source_element(argument)?;
                    }
                    for signature in self.constructors_for_arguments(static_base, base_node)? {
                        let parameters = self
                            .signatures
                            .get(signature)?
                            .type_parameters
                            .clone()
                            .unwrap_or_default();
                        if !self.check_type_argument_constraints(base_node, &parameters)? {
                            break;
                        }
                    }
                }
                let base_with_this = self.get_type_with_this_argument(base, this, false)?;
                if self.is_type_related_to(with_this, base_with_this, RelationKind::Assignable)? {
                    let without_signatures = self.type_without_signatures(static_base)?;
                    let (_, diagnostic) = self.check_type_related_ex(static_type, without_signatures, RelationKind::Assignable, Some(name.unwrap_or(node)), Some(messages::Class_static_side_0_incorrectly_extends_base_class_static_side_1))?;
                    if let Some(diagnostic) = diagnostic {
                        self.add_diagnostic(diagnostic)?;
                    }
                } else {
                    self.issue_class_member_error(
                        node,
                        with_this,
                        base_with_this,
                        messages::Class_0_incorrectly_extends_base_class_1,
                    )?;
                }
                if self.types.flags(constructor)? & tf::TYPE_VARIABLE != 0 {
                    if self.is_mixin_constructor_type(static_type)? {
                        let mut abstract_base = false;
                        for signature in self.signatures_of_type(constructor, true)? {
                            if self.signatures.get(signature)?.flags
                                & crate::signature_flags::ABSTRACT
                                != 0
                            {
                                abstract_base = true;
                                break;
                            }
                        }
                        if abstract_base && modifiers & mf::ABSTRACT == 0 {
                            self.error_at(Some(name.unwrap_or(node)),messages::A_mixin_class_that_extends_from_a_type_variable_containing_an_abstract_construct_signature_must_also_be_declared_abstract,vec![])?;
                        }
                    } else {
                        self.error_at(Some(name.unwrap_or(node)),messages::A_mixin_class_must_have_a_constructor_with_a_single_rest_parameter_of_type_any,vec![])?;
                    }
                }
                let class_constructor = self
                    .types
                    .get(static_base)?
                    .symbol
                    .map(|symbol| self.symbol(symbol).map(|s| s.flags() & sf::CLASS != 0))
                    .transpose()?
                    .unwrap_or(false);
                if !class_constructor && self.types.flags(constructor)? & tf::TYPE_VARIABLE == 0 {
                    for signature in
                        self.instantiated_constructors_for_arguments(static_base, base_node)?
                    {
                        let result = self.return_type_of_signature(signature)?;
                        if !self.is_type_related_to(result, base, RelationKind::Identity)? {
                            self.error_at(
                                Some(expression),
                                messages::Base_constructors_must_all_have_the_same_return_type,
                                vec![],
                            )?;
                            break;
                        }
                    }
                }
                self.check_class_override_kinds(class, base)?;
            }
        }
        self.check_class_override_modifiers(node, class, with_this, static_type)?;
        for reference in self.class_heritage_nodes(node, K::ImplementsKeyword)? {
            if self.ast(reference)?.node(reference)?.kind() == K::ExpressionWithTypeArguments {
                let expression = self
                    .ast(reference)?
                    .node(reference)?
                    .expression()
                    .ok_or(Error::MissingLink("class implements expression"))?;
                if !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)?
                    || self.ast(expression)?.node(expression)?.flags() & nf::OPTIONAL_CHAIN != 0
                {
                    self.error_at(Some(expression), messages::A_class_can_only_implement_an_identifier_Slashqualified_name_with_optional_type_arguments, vec![])?;
                }
            }
            self.check_type_reference_node(reference)?;
            let ty = self.get_type_from_type_node(reference)?;
            let ty = self.get_reduced_type(ty)?;
            if self.is_error_type(ty)? {
                continue;
            }
            if self.is_valid_base_type(ty)? {
                let base = self.get_type_with_this_argument(ty, this, false)?;
                let class_base = match self.types.get(ty)?.symbol {
                    Some(symbol) => self.symbol(symbol)?.flags() & sf::CLASS != 0,
                    None => false,
                };
                if !self.is_type_related_to(with_this, base, RelationKind::Assignable)? {
                    self.issue_class_member_error(node, with_this, base, if class_base {
                        messages::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass
                    } else { messages::Class_0_incorrectly_implements_interface_1 })?;
                }
            } else {
                self.error_at(Some(reference), messages::A_class_can_only_implement_an_object_type_or_intersection_of_object_types_with_statically_known_members, vec![])?;
            }
        }
        self.check_source_index_constraints(class, node, false)?;
        self.check_source_index_constraints(static_type, node, true)?;
        self.check_class_property_initialization(node, &members)?;
        if self.ast(node)?.node(node)?.kind() != K::ClassExpression {
            for member in members {
                self.check_source_element(member)?;
            }
            self.register_for_unused_identifiers_check(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkJSDocAugmentsTagMatchesExtends
    fn check_jsdoc_class_extends(
        &mut self,
        node: NodeId,
        base_node: NodeId,
        base: TypeId,
    ) -> Result<(), Error> {
        if self.ast(node)?.node(node)?.flags() & nf::JAVA_SCRIPT_FILE == 0 {
            return Ok(());
        }
        // JS files carry lazily parsed JSDoc; the provider covers both forms.
        let docs: Vec<NodeId> = self
            .jsdoc_for_node(node)?
            .map(|docs| docs.iter().copied().collect())
            .unwrap_or_default();
        for doc in docs {
            let tags = self
                .ast(doc)?
                .node(doc)?
                .data_source()
                .as_js_doc()
                .ok_or(Error::MissingLink("class JSDoc"))?
                .tags();
            for tag in self.source_list(doc, tags)? {
                let read = self.ast(tag)?.node(tag)?;
                if read.kind() != K::JSDocAugmentsTag {
                    continue;
                }
                let data = read
                    .data_source()
                    .as_js_doc_augments_tag()
                    .ok_or(Error::MissingLink("class JSDoc extends tag"))?;
                let source = data
                    .class_name()
                    .ok_or(Error::MissingLink("JSDoc extends type"))?;
                let tag_name = data
                    .tag_name()
                    .ok_or(Error::MissingLink("JSDoc extends tag name"))?;
                let source_type = self.get_type_from_type_node(source)?;
                if self.is_type_related_to(source_type, base, RelationKind::Identity)? {
                    continue;
                }
                let source_expression = self
                    .ast(source)?
                    .node(source)?
                    .expression()
                    .ok_or(Error::MissingLink("JSDoc extends expression"))?;
                let target_expression = self
                    .ast(base_node)?
                    .node(base_node)?
                    .expression()
                    .ok_or(Error::MissingLink("class extends expression"))?;
                let identifier = |this: &Self, node: NodeId| -> Result<Option<NodeId>, Error> {
                    let read = this.ast(node)?.node(node)?;
                    Ok(match read.kind().known() {
                        Some(K::Identifier) => Some(node),
                        Some(K::PropertyAccessExpression) => read.name(),
                        _ => None,
                    })
                };
                if let (Some(source), Some(target)) = (
                    identifier(self, source_expression)?,
                    identifier(self, target_expression)?,
                ) {
                    let args = vec![
                        self.ast(tag_name)?.node_text(tag_name)?.into_js_string(),
                        self.ast(source)?.node_text(source)?.into_js_string(),
                        self.ast(target)?.node_text(target)?.into_js_string(),
                    ];
                    self.error_at(
                        Some(source),
                        messages::JSDoc_0_1_does_not_match_the_extends_2_clause,
                        args,
                    )?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeWithoutSignatures
    pub(crate) fn type_without_signatures(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::OBJECT != 0 {
            self.resolve_type_members(ty)?;
            let data = self.types.structured(ty)?;
            if data
                .signatures
                .as_ref()
                .is_some_and(|signatures| !signatures.is_empty())
            {
                let members = data.members;
                let properties = data.properties.clone();
                let result = self.new_object_type(of::ANONYMOUS, self.types.get(ty)?.symbol)?;
                self.types.get_mut(result)?.object_flags |= of::MEMBERS_RESOLVED;
                self.types.structured_mut(result)?.members = members;
                self.types.structured_mut(result)?.properties = properties;
                return Ok(result);
            }
        } else if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            let mut types = Vec::new();
            for &part in self.types.compound_types(ty)?.clone().iter() {
                types.push(self.type_without_signatures(part)?);
            }
            return self.get_intersection_type(&types);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.issueMemberSpecificError
    fn issue_class_member_error(
        &mut self,
        node: NodeId,
        ty: TypeId,
        base: TypeId,
        message: &'static ts_diagnostics::Message,
    ) -> Result<(), Error> {
        let mut issued = false;
        for member in self.source_list(node, self.ast(node)?.node(node)?.member_list())? {
            if ts_ast::utilities::is_static(self.ast(member)?, member)? {
                continue;
            }
            let Some(declared) = self.get_symbol_of_declaration(member)? else {
                continue;
            };
            let name = self.symbol(declared)?.name_to_owned();
            if name.as_bytes() == ts_ast::internal_symbol_names::COMPUTED {
                continue;
            }
            let property = self.constituent_property(ty, name.as_bytes(), false)?;
            let inherited = self.constituent_property(base, name.as_bytes(), false)?;
            if let (Some(property), Some(inherited)) = (property, inherited) {
                let source = self.get_type_of_symbol(property)?;
                let target = self.get_type_of_symbol(inherited)?;
                let name_node = self.ast(member)?.node(member)?.name().unwrap_or(member);
                let (related, diagnostic) = self.check_type_related_ex(
                    source,
                    target,
                    RelationKind::Assignable,
                    Some(name_node),
                    None,
                )?;
                if !related {
                    let diagnostic = diagnostic
                        .ok_or(Error::MissingLink("class property relation diagnostic"))?;
                    let name = self.symbol_to_string(declared)?;
                    let source = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                    let target = self.type_to_string(base, crate::type_display::DEFAULT_FLAGS)?;
                    self.add_diagnostic(ts_ast::Diagnostic::chain(Some(std::sync::Arc::new(diagnostic)), messages::Property_0_in_type_1_is_not_assignable_to_the_same_property_in_base_type_2, vec![name, source, target]))?;
                    issued = true;
                }
            }
        }
        if !issued {
            let location = self.ast(node)?.node(node)?.name().unwrap_or(node);
            let (_, diagnostic) = self.check_type_related_ex(
                ty,
                base,
                RelationKind::Assignable,
                Some(location),
                Some(message),
            )?;
            if let Some(diagnostic) = diagnostic {
                self.add_diagnostic(diagnostic)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkPropertyInitialization
    fn check_class_property_initialization(
        &mut self,
        node: NodeId,
        members: &[NodeId],
    ) -> Result<(), Error> {
        let options = self.program()?.host.options();
        if !self.options.strict_null_checks
            || !options.strict_option_value(options.strict_property_initialization)
            || self.ast(node)?.node(node)?.flags() & nf::AMBIENT != 0
        {
            return Ok(());
        }
        let mut constructor = None;
        for &member in members {
            if self.ast(member)?.node(member)?.kind() == K::Constructor
                && self.ast(member)?.node(member)?.body().is_some()
            {
                constructor = Some(member);
                break;
            }
        }
        for &member in members {
            let read = self.ast(member)?.node(member)?;
            if read.kind() != K::PropertyDeclaration
                || read.initializer().is_some()
                || read.modifier_flags(self.ast(member)?)?
                    & (mf::STATIC | mf::ABSTRACT | mf::AMBIENT)
                    != 0
            {
                continue;
            }
            let name = read
                .name()
                .ok_or(Error::MissingLink("class property name"))?;
            if let Some(token) = read.postfix_token() {
                if self.ast(token)?.node(token)?.kind() == K::ExclamationToken {
                    continue;
                }
            }
            if !matches!(
                self.ast(name)?.node(name)?.kind().known(),
                Some(K::Identifier | K::PrivateIdentifier | K::ComputedPropertyName)
            ) {
                continue;
            }
            let symbol = self
                .get_symbol_of_declaration(member)?
                .ok_or(Error::MissingLink("class property symbol"))?;
            let ty = self.get_type_of_symbol(symbol)?;
            if self.types.flags(ty)? & tf::ANY_OR_UNKNOWN != 0
                || self.class_type_contains_undefined(ty)?
            {
                continue;
            }
            if let Some(constructor) = constructor {
                if self.property_initialized_in_constructor(name, ty, constructor)? {
                    continue;
                }
            }
            let text = ts_scanner::declaration_name_to_string(self.ast(name)?, Some(name))?;
            self.error_at(Some(name), messages::Property_0_has_no_initializer_and_is_not_definitely_assigned_in_the_constructor, vec![text])?;
        }
        Ok(())
    }

    pub(crate) fn class_type_contains_undefined(&self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::UNDEFINED != 0 {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::UNION != 0 {
            for &part in self.types.types_of(ty)? {
                if self.class_type_contains_undefined(part)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkTypeParameterListsIdentical
    pub(crate) fn check_class_or_interface_type_parameters_identical(
        &mut self,
        symbol: ts_arena::SymbolId,
    ) -> Result<(), Error> {
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        if declarations.len() == 1 || !self.query.type_parameters_checked.insert(symbol) {
            return Ok(());
        }
        let result = (|| {
            let mut relevant = Vec::new();
            for declaration in declarations.into_iter().flatten() {
                if matches!(
                    self.ast(declaration)?.node(declaration)?.kind().known(),
                    Some(K::ClassDeclaration | K::InterfaceDeclaration)
                ) {
                    relevant.push(declaration);
                }
            }
            if relevant.len() <= 1 {
                return Ok(());
            }
            let ty = self.get_declared_type_of_symbol(symbol)?;
            let data = self.types.interface(ty)?;
            let parameters =
                data.type_parameters()[data.outer_type_parameter_count as usize..].to_vec();
            let minimum = self.min_type_argument_count(&parameters)?;
            let mut identical = true;
            for &declaration in &relevant {
                let sources = self.source_list(
                    declaration,
                    self.ast(declaration)?
                        .node(declaration)?
                        .type_parameter_list(),
                )?;
                if sources.len() < minimum || sources.len() > parameters.len() {
                    identical = false;
                    break;
                }
                for (&source, &target) in sources.iter().zip(&parameters) {
                    if !self.type_parameter_declarations_identical(&[source], target)? {
                        identical = false;
                        break;
                    }
                }
                if !identical {
                    break;
                }
            }
            if !identical {
                let name = self.symbol_to_string(symbol)?;
                for declaration in relevant {
                    self.error_at(
                        self.ast(declaration)?.node(declaration)?.name(),
                        messages::All_declarations_of_0_must_have_identical_type_parameters,
                        vec![name.clone()],
                    )?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.query.type_parameters_checked.remove(&symbol);
        }
        result
    }
}
