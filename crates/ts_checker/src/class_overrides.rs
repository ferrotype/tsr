//! Inheritance diagnostics distinguish redeclarations, abstract requirements and
//! the syntactic override contract; assignability is checked by the caller.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    check_flags as cf, modifier_flags as mf, node_flags as nf, symbol_flags as sf, JsString,
    SyntaxKind as K,
};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/grammarchecks.go:Checker.isNonBindableDynamicName
    pub(crate) fn non_bindable_dynamic_name(&mut self, name: NodeId) -> Result<bool, Error> {
        if !ts_ast::is_dynamic_name(self.ast(name)?, name)? {
            return Ok(false);
        }
        let read = self.ast(name)?.node(name)?;
        let expression = if read.kind() == K::ComputedPropertyName {
            read.expression()
        } else {
            read.data_source()
                .as_element_access_expression()
                .and_then(|data| data.argument_expression())
        }
        .ok_or(Error::MissingLink("override dynamic name expression"))?;
        if !ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
            return Ok(true);
        }
        let ty = self.late_name_type(name)?;
        Ok(self.types.flags(ty)? & tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE == 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSuggestedSymbolForNonexistentClassMember
    fn suggested_override_member(
        &mut self,
        name: &[u8],
        base: TypeId,
    ) -> Result<Option<SymbolId>, Error> {
        let mut symbols = self.get_properties_of_type(base)?;
        self.sort_symbols(&mut symbols)?;
        let mut candidates = Vec::new();
        for (rank, symbol) in symbols.into_iter().enumerate() {
            let read = self.symbol(symbol)?;
            let name = read.name_to_owned();
            if name.is_empty() || matches!(name.as_bytes()[0], b'"' | 0xfe) {
                continue;
            }
            let flags = read.flags();
            let valid = if flags & sf::CLASS_MEMBER != 0 {
                true
            } else if flags & sf::ALIAS != 0 {
                let alias = self.resolve_alias(symbol)?;
                self.symbol(alias)?.flags() & sf::CLASS_MEMBER != 0
            } else {
                false
            };
            if valid {
                candidates.push((rank, symbol, name));
            }
        }
        Ok(ts_scanner::get_spelling_suggestion(
            name,
            candidates.iter(),
            |candidate| candidate.2.as_bytes(),
            |a, b| a.0.cmp(&b.0),
            0,
        )
        .map(|value| value.1))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkMembersForOverrideModifier
    pub(crate) fn check_class_override_modifiers(
        &mut self,
        node: NodeId,
        ty: TypeId,
        with_this: TypeId,
        static_type: TypeId,
    ) -> Result<(), Error> {
        let mut base_with_this = None;
        if self.class_base_type_node(ty)?.is_some() {
            if let Some(base) = self.interface_base_types(ty)?.first().copied() {
                let this = self
                    .types
                    .interface(ty)?
                    .this_type
                    .ok_or(Error::MissingLink("class this type"))?;
                base_with_this = Some(self.get_type_with_this_argument(base, this, false)?);
            }
        }
        let static_base = self.class_base_constructor_type(ty)?;
        for member in self.source_list(node, self.ast(node)?.node(node)?.member_list())? {
            if ts_ast::utilities::has_syntactic_modifier(self.ast(member)?, member, mf::AMBIENT)? {
                continue;
            }
            let members = if self.ast(member)?.node(member)?.kind() == K::Constructor {
                let mut parameters = vec![];
                for parameter in
                    self.source_list(member, self.ast(member)?.node(member)?.parameter_list())?
                {
                    if ts_ast::utilities::is_parameter_property_declaration(
                        self.ast(parameter)?,
                        parameter,
                        member,
                    )? {
                        parameters.push(parameter);
                    }
                }
                parameters
            } else {
                vec![member]
            };
            for member in members {
                if let Some(symbol) = self.get_symbol_of_declaration(member)? {
                    self.check_class_override_modifier(
                        node,
                        ty,
                        with_this,
                        static_type,
                        static_base,
                        base_with_this,
                        symbol,
                        member,
                    )?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkMemberForOverrideModifierWorker
    #[allow(clippy::too_many_arguments)]
    fn check_class_override_modifier(
        &mut self,
        node: NodeId,
        ty: TypeId,
        with_this: TypeId,
        static_type: TypeId,
        static_base: TypeId,
        base: Option<TypeId>,
        symbol: SymbolId,
        member: NodeId,
    ) -> Result<(), Error> {
        let view = self.ast(member)?;
        let modifiers = view.node(member)?.modifier_flags(view)?;
        let has_override = modifiers & mf::OVERRIDE != 0;
        let is_abstract = modifiers & mf::ABSTRACT != 0;
        let is_static = modifiers & mf::STATIC != 0;
        let parameter = view.node(member)?.kind() == K::Parameter;
        let javascript = self.ast(node)?.node(node)?.flags() & nf::JAVA_SCRIPT_FILE != 0;
        if has_override {
            if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                let read = self.ast(declaration)?.node(declaration)?;
                if ts_ast::utilities::is_class_element(&read) {
                    if let Some(name) = read.name() {
                        if self.non_bindable_dynamic_name(name)? {
                            self.error_at(Some(member), if javascript {d::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_its_name_is_dynamic} else {d::This_member_cannot_have_an_override_modifier_because_its_name_is_dynamic},vec![])?;
                            return Ok(());
                        }
                    }
                }
            }
        }
        let implicit = self
            .program()?
            .host
            .options()
            .no_implicit_override
            .is_true();
        if let Some(base) = base.filter(|_| has_override || implicit) {
            let name = self.symbol(symbol)?.name_to_owned();
            let own = self.constituent_property(
                if is_static { static_type } else { with_this },
                name.as_bytes(),
                false,
            )?;
            let original = self.constituent_property(
                if is_static { static_base } else { base },
                name.as_bytes(),
                false,
            )?;
            if own.is_some() && original.is_none() && has_override {
                let suggestion = self.suggested_override_member(
                    name.as_bytes(),
                    if is_static { static_base } else { base },
                )?;
                let display = self.type_to_string(base, crate::type_display::DEFAULT_FLAGS)?;
                let (message, args) = if let Some(suggestion) = suggestion {
                    (
                        if javascript {
                            d::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_it_is_not_declared_in_the_base_class_0_Did_you_mean_1
                        } else {
                            d::This_member_cannot_have_an_override_modifier_because_it_is_not_declared_in_the_base_class_0_Did_you_mean_1
                        },
                        vec![display, self.symbol_to_string(suggestion)?],
                    )
                } else {
                    (
                        if javascript {
                            d::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_it_is_not_declared_in_the_base_class_0
                        } else {
                            d::This_member_cannot_have_an_override_modifier_because_it_is_not_declared_in_the_base_class_0
                        },
                        vec![display],
                    )
                };
                self.error_at(Some(member), message, args)?;
                return Ok(());
            }
            let ambient = self.ast(node)?.node(node)?.flags() & nf::AMBIENT != 0;
            if let Some(original) = original.filter(|_| own.is_some() && implicit && !ambient) {
                let declarations = self.symbol_declarations(original)?.to_vec();
                if !declarations.is_empty() {
                    let mut base_abstract = false;
                    for declaration in declarations.into_iter().flatten() {
                        if ts_ast::utilities::has_syntactic_modifier(
                            self.ast(declaration)?,
                            declaration,
                            mf::ABSTRACT,
                        )? {
                            base_abstract = true;
                            break;
                        }
                    }
                    if has_override {
                        return Ok(());
                    }
                    let message = if !base_abstract {
                        Some(if parameter {
                            if javascript {
                                d::This_parameter_property_must_have_a_JSDoc_comment_with_an_override_tag_because_it_overrides_a_member_in_the_base_class_0
                            } else {
                                d::This_parameter_property_must_have_an_override_modifier_because_it_overrides_a_member_in_base_class_0
                            }
                        } else if javascript {
                            d::This_member_must_have_a_JSDoc_comment_with_an_override_tag_because_it_overrides_a_member_in_the_base_class_0
                        } else {
                            d::This_member_must_have_an_override_modifier_because_it_overrides_a_member_in_the_base_class_0
                        })
                    } else if is_abstract {
                        Some(d::This_member_must_have_an_override_modifier_because_it_overrides_an_abstract_method_that_is_declared_in_the_base_class_0)
                    } else {
                        None
                    };
                    if let Some(message) = message {
                        let display =
                            self.type_to_string(base, crate::type_display::DEFAULT_FLAGS)?;
                        self.error_at(Some(member), message, vec![display])?;
                    }
                }
            }
        } else if has_override {
            let display = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
            self.error_at(Some(member),if javascript {d::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_its_containing_class_0_does_not_extend_another_class} else {d::This_member_cannot_have_an_override_modifier_because_its_containing_class_0_does_not_extend_another_class},vec![display])?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.arePropertiesAbstractOrInterface
    fn override_base_is_abstract_or_interface(
        &self,
        symbol: SymbolId,
        flags: u32,
    ) -> Result<bool, Error> {
        let any = self.symbol(symbol)?.check_flags() & cf::SYNTHETIC != 0;
        for declaration in self.symbol_declarations(symbol)?.iter().flatten() {
            let view = self.ast(declaration)?;
            let read = view.node(declaration)?;
            let interface = read
                .parent()
                .map(|parent| {
                    view.node(parent)
                        .map(|read| read.kind() == K::InterfaceDeclaration)
                })
                .transpose()?
                .unwrap_or(false);
            let matches = interface
                || flags & mf::ABSTRACT != 0
                    && (read.kind() != K::PropertyDeclaration || read.initializer().is_none());
            if matches == any {
                return Ok(any);
            }
        }
        Ok(!any)
    }

    fn override_property_is_method(&self, symbol: SymbolId) -> Result<bool, Error> {
        let read = self.symbol(symbol)?;
        Ok(read.flags() & sf::METHOD != 0 || read.check_flags() & cf::SYNTHETIC_METHOD != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkKindsOfPropertyMemberOverrides
    pub(crate) fn check_class_override_kinds(
        &mut self,
        ty: TypeId,
        base_type: TypeId,
    ) -> Result<(), Error> {
        let class = self
            .types
            .get(ty)?
            .symbol
            .map(|s| self.class_declaration(s))
            .transpose()?
            .flatten();
        let mut missing = vec![];
        'property: for base_property in self.get_properties_of_type(base_type)? {
            let base = self.target_symbol(base_property)?;
            if self.symbol(base)?.flags() & sf::PROTOTYPE != 0 {
                continue;
            }
            let name = self.symbol(base)?.name_to_owned();
            let Some(derived) = self.constituent_property(ty, name.as_bytes(), true)? else {
                continue;
            };
            let derived = self.target_symbol(derived)?;
            let base_flags = self.property_modifiers(base)?;
            if derived == base {
                if base_flags & mf::ABSTRACT != 0 {
                    let abstract_class = class
                        .map(|class| {
                            ts_ast::utilities::has_syntactic_modifier(
                                self.ast(class)?,
                                class,
                                mf::ABSTRACT,
                            )
                            .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false);
                    if !abstract_class {
                        for &other in self.interface_base_types(ty)?.iter() {
                            if other == base_type {
                                continue;
                            }
                            if let Some(other) =
                                self.constituent_property(other, name.as_bytes(), true)?
                            {
                                if self.target_symbol(other)? != base {
                                    continue 'property;
                                }
                            }
                        }
                        missing.push(self.symbol_to_string(base_property)?);
                    }
                }
                continue;
            }
            let derived_flags = self.property_modifiers(derived)?;
            if (base_flags | derived_flags) & mf::PRIVATE != 0 {
                continue;
            }
            let base_kind = self.symbol(base)?.flags() & sf::PROPERTY_OR_ACCESSOR;
            let derived_kind = self.symbol(derived)?.flags() & sf::PROPERTY_OR_ACCESSOR;
            let declaration = self.symbol(derived)?.value_declaration();
            let location = declaration
                .map(|node| {
                    self.ast(node)
                        .and_then(|v| Ok(v.node(node)?.name().unwrap_or(node)))
                })
                .transpose()?;
            if base_kind != 0 && derived_kind != 0 {
                let assignment = declaration
                    .map(|node| {
                        self.ast(node)
                            .and_then(|v| Ok(v.node(node)?.kind() == K::BinaryExpression))
                    })
                    .transpose()?
                    .unwrap_or(false);
                if self.symbol(base)?.check_flags() & cf::MAPPED != 0
                    || assignment
                    || self.override_base_is_abstract_or_interface(base, base_flags)?
                {
                    continue;
                }
                let property_over_accessor =
                    base_kind != sf::PROPERTY && derived_kind == sf::PROPERTY;
                let accessor_over_property =
                    base_kind == sf::PROPERTY && derived_kind != sf::PROPERTY;
                if property_over_accessor || accessor_over_property {
                    let args = vec![
                        self.symbol_to_string(base)?,
                        self.type_to_string(base_type, crate::type_display::DEFAULT_FLAGS)?,
                        self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?,
                    ];
                    self.error_at(location,if property_over_accessor {d::X_0_is_defined_as_an_accessor_in_class_1_but_is_overridden_here_in_2_as_an_instance_property}else{d::X_0_is_defined_as_a_property_in_class_1_but_is_overridden_here_in_2_as_an_accessor},args)?;
                } else if self.program()?.host.options().use_define_for_class_fields() {
                    let declarations = self.symbol_declarations(derived)?.to_vec();
                    let mut uninitialized = None;
                    let mut ambient = false;
                    for declaration in declarations.into_iter().flatten() {
                        let read = self.ast(declaration)?.node(declaration)?;
                        ambient |= read.flags() & nf::AMBIENT != 0;
                        if uninitialized.is_none()
                            && read.kind() == K::PropertyDeclaration
                            && read.initializer().is_none()
                        {
                            uninitialized = Some(declaration);
                        }
                    }
                    let transient = self.symbol(derived)?.flags() & sf::TRANSIENT != 0;
                    if let Some(uninitialized) = uninitialized.filter(|_| {
                        !ambient && !transient && (base_flags | derived_flags) & mf::ABSTRACT == 0
                    }) {
                        let mut constructor = None;
                        if let Some(class) = class {
                            for member in self
                                .source_list(class, self.ast(class)?.node(class)?.member_list())?
                            {
                                let read = self.ast(member)?.node(member)?;
                                if read.kind() == K::Constructor && read.body().is_some() {
                                    constructor = Some(member);
                                    break;
                                }
                            }
                        }
                        let read = self.ast(uninitialized)?.node(uninitialized)?;
                        let name = read
                            .name()
                            .ok_or(Error::MissingLink("override property name"))?;
                        let exclamation = read
                            .postfix_token()
                            .map(|token| {
                                self.ast(token)
                                    .and_then(|v| Ok(v.node(token)?.kind() == K::ExclamationToken))
                            })
                            .transpose()?
                            .unwrap_or(false);
                        let initialized = if !exclamation
                            && self.options.strict_null_checks
                            && self.ast(name)?.node(name)?.kind() == K::Identifier
                        {
                            match constructor {
                                Some(constructor) => {
                                    self.property_initialized_in_constructor(name, ty, constructor)?
                                }
                                None => false,
                            }
                        } else {
                            false
                        };
                        if !initialized {
                            let args = vec![
                                self.symbol_to_string(base)?,
                                self.type_to_string(base_type, crate::type_display::DEFAULT_FLAGS)?,
                            ];
                            self.error_at(location,d::Property_0_will_overwrite_the_base_property_in_1_If_this_is_intentional_add_an_initializer_Otherwise_add_a_declare_modifier_or_remove_the_redundant_declaration,args)?;
                        }
                    }
                }
                continue;
            }
            let message = if self.override_property_is_method(base)? {
                if self.override_property_is_method(derived)?
                    || self.symbol(derived)?.flags() & sf::PROPERTY != 0
                {
                    continue;
                }
                d::Class_0_defines_instance_member_function_1_but_extended_class_2_defines_it_as_instance_member_accessor
            } else if self.symbol(base)?.flags() & sf::ACCESSOR != 0 {
                d::Class_0_defines_instance_member_accessor_1_but_extended_class_2_defines_it_as_instance_member_function
            } else {
                d::Class_0_defines_instance_member_property_1_but_extended_class_2_defines_it_as_instance_member_function
            };
            let args = vec![
                self.type_to_string(base_type, crate::type_display::DEFAULT_FLAGS)?,
                self.symbol_to_string(base)?,
                self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?,
            ];
            self.error_at(location, message, args)?;
        }
        if missing.is_empty() {
            return Ok(());
        }
        let expression = class
            .map(|node| {
                self.ast(node)
                    .and_then(|v| Ok(v.node(node)?.kind() == K::ClassExpression))
            })
            .transpose()?
            .unwrap_or(false);
        let base = self.type_to_string(base_type, crate::type_display::DEFAULT_FLAGS)?;
        let own = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
        if missing.len() == 1 {
            let (message, args) = if expression {
                (d::Non_abstract_class_expression_does_not_implement_inherited_abstract_member_0_from_class_1,vec![missing[0].clone(),base])
            } else {
                (d::Non_abstract_class_0_does_not_implement_inherited_abstract_member_1_from_class_2,vec![own,missing[0].clone(),base])
            };
            self.error_at(class, message, args)?;
        } else {
            let count = missing.len();
            let mut names = vec![];
            for (index, name) in missing
                .iter()
                .take(if count > 5 { 4 } else { count })
                .enumerate()
            {
                if index > 0 {
                    names.extend_from_slice(b", ");
                }
                names.push(b'\'');
                names.extend_from_slice(name.as_bytes());
                names.push(b'\'');
            }
            let names = JsString::from_bytes(names);
            let (message, mut args) = if expression {
                (
                    if count > 5 {
                        d::Non_abstract_class_expression_is_missing_implementations_for_the_following_members_of_0_Colon_1_and_2_more
                    } else {
                        d::Non_abstract_class_expression_is_missing_implementations_for_the_following_members_of_0_Colon_1
                    },
                    vec![base, names],
                )
            } else {
                (
                    if count > 5 {
                        d::Non_abstract_class_0_is_missing_implementations_for_the_following_members_of_1_Colon_2_and_3_more
                    } else {
                        d::Non_abstract_class_0_is_missing_implementations_for_the_following_members_of_1_Colon_2
                    },
                    vec![own, base, names],
                )
            };
            if count > 5 {
                args.push(JsString::from_bytes((count - 4).to_string().as_bytes()));
            }
            self.error_at(class, message, args)?;
        }
        Ok(())
    }
}
