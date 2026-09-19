//! Missing names run contextual meaning diagnostics before lexical spelling
//! suggestions. Resolution side effects are dispatched after the lexical borrow.
use crate::{type_flags as tf, CheckerState, Error};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, JsString, SyntaxKind as K};
use ts_diagnostics as d;
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getCannotFindNameDiagnosticForName
    pub(crate) fn cannot_find_name_diagnostic(
        &self,
        node: NodeId,
    ) -> Result<&'static d::Message, Error> {
        let text = self.node_text(node)?;
        let wildcard = self.program()?.host.options().uses_wildcard_types();
        Ok(match text.as_bytes() {
            b"document" | b"console" => d::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_include_dom,
            b"$" => if wildcard { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery } else { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery_and_then_add_jquery_to_the_types_field_in_your_tsconfig },
            b"beforeEach" | b"describe" | b"suite" | b"it" | b"test" => if wildcard { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha } else { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha_and_then_add_jest_or_mocha_to_the_types_field_in_your_tsconfig },
            b"process" | b"require" | b"Buffer" | b"module" | b"NodeJS" => if wildcard { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode } else { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode_and_then_add_node_to_the_types_field_in_your_tsconfig },
            b"Bun" => if wildcard { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_Bun_Try_npm_i_save_dev_types_Slashbun } else { d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_Bun_Try_npm_i_save_dev_types_Slashbun_and_then_add_bun_to_the_types_field_in_your_tsconfig },
            b"Map" | b"Set" | b"Promise" | b"ast.Symbol" | b"WeakMap" | b"WeakSet" | b"Iterator" | b"AsyncIterator" | b"SharedArrayBuffer" | b"Atomics" | b"AsyncIterable" | b"AsyncIterableIterator" | b"AsyncGenerator" | b"AsyncGeneratorFunction" | b"BigInt" | b"Reflect" | b"BigInt64Array" | b"BigUint64Array" => d::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_1_or_later,
            _ => {
                let parent = self.node(node)?.parent();
                let kind = parent.map(|parent| self.ast(parent)?.node(parent).map(|node|node.kind()).map_err(Error::from)).transpose()?;
                if text.as_bytes() == b"await" && kind == Some(K::CallExpression.into()) { d::Cannot_find_name_0_Did_you_mean_to_write_this_in_an_async_function }
                else if kind == Some(K::ShorthandPropertyAssignment.into()) { d::No_value_exists_in_scope_for_the_shorthand_property_0_Either_declare_one_or_provide_an_initializer }
                else { d::Cannot_find_name_0 }
            }
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.checkAndReportErrorForInvalidInitializer
    pub(crate) fn report_invalid_initializer(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        property: NodeId,
        result: Option<ts_arena::SymbolId>,
    ) -> Result<(), Error> {
        if let Some(location) = location {
            if result.is_none() && self.missing_name_prefix(location, name)? {
                return Ok(());
            }
        }
        let read = self.node(property)?;
        let property_name = read.name();
        let in_type = if let (Some(location), Some(annotation)) = (location, read.type_node()) {
            let range = self.node(annotation)?;
            let pos = self.node(location)?.pos();
            range.pos() <= pos && pos <= range.end()
        } else {
            false
        };
        let property_name =
            ts_scanner::declaration_name_to_string(self.ast(property)?, property_name)?;
        self.error_at(location,if in_type{d::Type_of_instance_member_variable_0_cannot_reference_identifier_1_declared_in_the_constructor}else{d::Initializer_of_instance_member_variable_0_cannot_reference_identifier_1_declared_in_the_constructor},vec![property_name,JsString::from_bytes(name)])?;
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.onSuccessfullyResolvedSymbol
    pub(crate) fn check_parameter_initializer_name(
        &mut self,
        location: NodeId,
        result: ts_arena::SymbolId,
        meaning: u32,
        declaration: NodeId,
    ) -> Result<(), Error> {
        let candidate = self.late_bound_symbol(result)?;
        let candidate = self.get_merged_symbol(candidate);
        let name = self.node(declaration)?.name();
        let name = ts_scanner::declaration_name_to_string(self.ast(declaration)?, name)?;
        if Some(candidate) == self.get_symbol_of_declaration(declaration)? {
            self.error_at(
                Some(location),
                d::Parameter_0_cannot_reference_itself,
                vec![name],
            )?;
        } else if let Some(value) = self.symbol(candidate)?.value_declaration() {
            if self.node(value)?.pos() > self.node(declaration)?.pos() {
                let root =
                    ts_ast::utilities::get_root_declaration(self.ast(declaration)?, declaration)?;
                let parent = self
                    .ast(root)?
                    .node(root)?
                    .parent()
                    .ok_or(Error::MissingLink("parameter root parent"))?;
                let locals = self
                    .program()?
                    .bound(parent)?
                    .node_binding(parent)?
                    .and_then(|binding| binding.locals);
                let candidate_name = self.symbol(candidate)?.name_to_owned();
                if locals.is_some()
                    && self.lookup_symbol_resolving(locals, candidate_name.as_bytes(), meaning)?
                        == Some(candidate)
                {
                    let reference = ts_scanner::declaration_name_to_string(
                        self.ast(location)?,
                        Some(location),
                    )?;
                    self.error_at(
                        Some(location),
                        d::Parameter_0_cannot_reference_identifier_1_declared_after_it,
                        vec![name, reference],
                    )?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.onFailedToResolveSymbol
    pub(crate) fn on_failed_source_name(
        &mut self,
        location: Option<NodeId>,
        name: &[u8],
        meaning: u32,
        message: &'static d::Message,
    ) -> Result<(), Error> {
        if let Some(location) = location {
            if let Some(parent) = self.node(location)?.parent() {
                if self.node(parent)?.kind() == K::JSDocLink {
                    return Ok(());
                }
                if name == b"const" && self.node(parent)?.kind() == K::TypeReference {
                    if let Some(assertion) = self.node(parent)?.parent() {
                        if matches!(
                            self.node(assertion)?.kind().known(),
                            Some(K::AsExpression | K::TypeAssertionExpression)
                        ) {
                            return Ok(());
                        }
                    }
                }
            }
            if self.missing_name_prefix(location, name)?
                || self.missing_name_meaning(location, name, meaning)?
            {
                return Ok(());
            }
        }
        let declaration = if let Some(location) = location {
            if self.node(location)?.kind() == K::Identifier
                && self.node_text(location)?.as_bytes() == name
            {
                ts_scanner::declaration_name_to_string(self.ast(location)?, Some(location))?
            } else {
                JsString::from_bytes(name)
            }
        } else {
            JsString::from_bytes(name)
        };
        if let Some(lib) = crate::name_resolution::suggested_library(name) {
            self.error_at(
                location,
                message,
                vec![declaration, JsString::from_bytes(lib)],
            )?;
            return Ok(());
        }
        if let Some(suggestion) = self.resolve_name_suggestion(location, name, meaning)? {
            let value = self.symbol(suggestion)?.value_declaration();
            let global = if let Some(value) = value {
                ts_ast::is_ambient_module(self.ast(value)?, value)?
                    && ts_ast::utilities::is_global_scope_augmentation(&self.node(value)?)
            } else {
                false
            };
            if !global {
                let text = self.symbol_to_string(suggestion)?;
                let unchecked_js =
                    self.is_unchecked_js_suggestion(location, Some(suggestion), false)?;
                let message = if meaning == sf::NAMESPACE {
                    d::Cannot_find_namespace_0_Did_you_mean_1
                } else if unchecked_js {
                    d::Could_not_find_name_0_Did_you_mean_1
                } else {
                    d::Cannot_find_name_0_Did_you_mean_1
                };
                let mut diagnostic =
                    self.diagnostic_for_node(location, message, vec![declaration, text.clone()])?;
                if let Some(value) = value {
                    diagnostic.related_information.push(std::sync::Arc::new(
                        self.diagnostic_for_node(Some(value), d::X_0_is_declared_here, vec![text])?,
                    ));
                }
                // port: tsc/internal/checker/checker.go:Checker.addErrorOrSuggestion
                if unchecked_js {
                    diagnostic.category = d::Category::Suggestion as i32;
                    self.add_suggestion_diagnostic(diagnostic)?;
                } else {
                    self.add_diagnostic(diagnostic)?;
                }
                return Ok(());
            }
        }
        self.error_at(location, message, vec![declaration])?;
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkAndReportErrorForMissingPrefix
    fn missing_name_prefix(&mut self, node: NodeId, name: &[u8]) -> Result<bool, Error> {
        let read = self.node(node)?;
        if read.kind() != K::Identifier || self.node_text(node)?.as_bytes() != name {
            return Ok(false);
        }
        if let Some(parent) = read.parent() {
            if self.node(parent)?.kind() == K::TypeReference {
                return Ok(false);
            }
        }
        let mut current = Some(node);
        while let Some(id) = current {
            let read = self.node(id)?;
            if read.kind() == K::TypeQuery {
                return Ok(false);
            }
            if !matches!(
                read.kind().known(),
                Some(K::Identifier | K::QualifiedName | K::PropertyAccessExpression)
            ) {
                break;
            }
            current = read.parent();
        }
        let container = ts_ast::get_this_container(self.ast(node)?, node, false, false)?;
        let mut location = container;
        while let Some(parent) = self.node(location)?.parent() {
            if matches!(
                self.node(parent)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) {
                let Some(symbol) = self.get_symbol_of_declaration(parent)? else {
                    break;
                };
                let constructor = self.get_type_of_symbol(symbol)?;
                if self
                    .constituent_property(constructor, name, false)?
                    .is_some()
                {
                    let text = self.symbol_to_string(symbol)?;
                    self.error_at(
                        Some(node),
                        d::Cannot_find_name_0_Did_you_mean_the_static_member_1_0,
                        vec![JsString::from_bytes(name), text],
                    )?;
                    return Ok(true);
                }
                if location == container
                    && self
                        .ast(location)?
                        .node(location)?
                        .modifier_flags(self.ast(location)?)?
                        & ts_ast::modifier_flags::STATIC
                        == 0
                {
                    let instance = self.get_declared_type_of_symbol(symbol)?;
                    let this = self
                        .types
                        .interface(instance)?
                        .this_type
                        .unwrap_or(instance);
                    if self.constituent_property(this, name, false)?.is_some() {
                        self.error_at(
                            Some(node),
                            d::Cannot_find_name_0_Did_you_mean_the_instance_member_this_0,
                            vec![JsString::from_bytes(name)],
                        )?;
                        return Ok(true);
                    }
                }
            }
            location = parent;
        }
        Ok(false)
    }
    fn missing_name_meaning(
        &mut self,
        node: NodeId,
        name: &[u8],
        meaning: u32,
    ) -> Result<bool, Error> {
        // The native extending-interface check precedes the other meanings.
        let mut current = node;
        loop {
            let read = self.node(current)?;
            match read.kind().known() {
                Some(K::Identifier | K::QualifiedName | K::PropertyAccessExpression) => {
                    if let Some(parent) = read.parent() {
                        current = parent;
                        continue;
                    }
                }
                Some(K::TypeReference | K::ExpressionWithTypeArguments) => {
                    let expression = if read.kind() == K::TypeReference {
                        read.data_source()
                            .as_type_reference_node()
                            .and_then(|d| d.type_name())
                    } else {
                        read.expression()
                    };
                    if let Some(expression) = expression {
                        if ts_ast::is_entity_name_expression(self.ast(expression)?, expression)?
                            && self
                                .resolve_entity_name(expression, sf::INTERFACE, true)?
                                .is_some()
                        {
                            let text =
                                ts_scanner::get_text_of_node(self.ast(expression)?, expression)?;
                            self.error_at(
                                Some(node),
                                d::Cannot_extend_an_interface_0_Did_you_mean_implements,
                                vec![text],
                            )?;
                            return Ok(true);
                        }
                    }
                }
                _ => {}
            }
            break;
        }
        let parent = self.node(node)?.parent();
        let primitive = matches!(
            name,
            b"any" | b"string" | b"number" | b"boolean" | b"never" | b"unknown"
        );
        if meaning == sf::NAMESPACE {
            let symbol =
                self.resolve_name(Some(node), name, sf::TYPE & !sf::NAMESPACE, None, false)?;
            if let Some(symbol) = self.resolve_module_symbol(symbol, false)? {
                if let Some(parent) = parent {
                    if self.node(parent)?.kind() == K::QualifiedName {
                        let right = self
                            .ast(parent)?
                            .node(parent)?
                            .data_source()
                            .as_qualified_name()
                            .and_then(|d| d.right())
                            .ok_or(Error::MissingLink("type namespace right"))?;
                        let prop = self.node_text(right)?.into_js_string();
                        let ty = self.get_declared_type_of_symbol(symbol)?;
                        if self
                            .constituent_property(ty, prop.as_bytes(), false)?
                            .is_some()
                        {
                            self.error_at(Some(parent),d::Cannot_access_0_1_because_0_is_a_type_but_not_a_namespace_Did_you_mean_to_retrieve_the_type_of_the_property_1_in_0_with_0_1,vec![JsString::from_bytes(name),prop])?;
                            return Ok(true);
                        }
                    }
                }
                self.error_at(
                    Some(node),
                    d::X_0_only_refers_to_a_type_but_is_being_used_as_a_namespace_here,
                    vec![JsString::from_bytes(name)],
                )?;
                return Ok(true);
            }
        }
        if primitive
            && parent
                .map(|p| {
                    self.ast(p)?
                        .node(p)
                        .map(|r| r.kind() == K::ExportSpecifier)
                        .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or(false)
        {
            self.error_at(
                Some(node),
                d::Cannot_export_0_Only_local_declarations_can_be_exported_from_a_module,
                vec![JsString::from_bytes(name)],
            )?;
            return Ok(true);
        }
        let export_assignment = parent
            .map(|p| {
                self.ast(p)?
                    .node(p)
                    .map(|r| r.kind() == K::ExportAssignment)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        if meaning & (sf::VALUE & !sf::TYPE) != 0 {
            let symbol = self.resolve_name(Some(node), name, sf::NAMESPACE_MODULE, None, false)?;
            if self.resolve_module_symbol(symbol, false)?.is_some() {
                if !export_assignment {
                    self.error_at(
                        Some(node),
                        d::Cannot_use_namespace_0_as_a_value,
                        vec![JsString::from_bytes(name)],
                    )?;
                }
                return Ok(true);
            }
        } else if meaning & (sf::TYPE & !sf::VALUE) != 0 {
            let symbol = self.resolve_name(Some(node), name, sf::MODULE, None, false)?;
            if self.resolve_module_symbol(symbol, false)?.is_some() {
                self.error_at(
                    Some(node),
                    d::Cannot_use_namespace_0_as_a_type,
                    vec![JsString::from_bytes(name)],
                )?;
                return Ok(true);
            }
        }
        if meaning & sf::VALUE != 0 {
            let symbol = if primitive {
                None
            } else {
                self.resolve_name(Some(node), name, sf::TYPE & !sf::VALUE, None, false)?
            };
            let symbol = self.resolve_module_symbol(symbol, false)?;
            let only_type = match symbol {
                Some(symbol) => self.module_symbol_flags(symbol, false, false)? & sf::VALUE == 0,
                None => false,
            };
            if primitive || only_type {
                if !primitive && export_assignment {
                    return Ok(true);
                }
                let mut message = d::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here;
                let mut args = vec![JsString::from_bytes(name)];
                if primitive {
                    if let Some(parent) = parent {
                        if let Some(grand) = self.node(parent)?.parent() {
                            if self.node(grand)?.kind() == K::HeritageClause {
                                let clause = self.node(grand)?;
                                let kind = clause
                                    .data_source()
                                    .as_heritage_clause()
                                    .ok_or(ts_arena::Error::InvalidGraph)?
                                    .token();
                                let owner = clause
                                    .parent()
                                    .ok_or(Error::MissingLink("heritage owner"))?;
                                message = if self.node(owner)?.kind() == K::InterfaceDeclaration
                                    && kind == K::ExtendsKeyword
                                {
                                    d::An_interface_cannot_extend_a_primitive_type_like_0_It_can_only_extend_other_named_object_types
                                } else if kind == K::ExtendsKeyword {
                                    d::A_class_cannot_extend_a_primitive_type_like_0_Classes_can_only_extend_constructable_values
                                } else {
                                    d::A_class_cannot_implement_a_primitive_type_like_0_It_can_only_implement_other_named_object_types
                                };
                            }
                        }
                    }
                } else if matches!(
                    name,
                    b"Promise" | b"Symbol" | b"Map" | b"WeakMap" | b"Set" | b"WeakSet"
                ) {
                    message=d::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_es2015_or_later;
                } else if let Some(symbol) = symbol {
                    if self.maybe_mapped_type(node, symbol)? {
                        message = d::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here_Did_you_mean_to_use_1_in_0;
                        args.push(JsString::from_bytes(if name == b"K" {
                            b"P".as_slice()
                        } else {
                            b"K".as_slice()
                        }));
                    }
                }
                self.error_at(Some(node), message, args)?;
                return Ok(true);
            }
        }
        if meaning & (sf::TYPE & !sf::NAMESPACE) != 0 {
            let symbol =
                self.resolve_name(Some(node), name, (!sf::TYPE) & sf::VALUE, None, false)?;
            if let Some(symbol) = self.resolve_module_symbol(symbol, false)? {
                if self.symbol(symbol)?.flags() & sf::NAMESPACE == 0 {
                    self.error_at(Some(node),d::X_0_refers_to_a_value_but_is_being_used_as_a_type_here_Did_you_mean_typeof_0,vec![JsString::from_bytes(name)])?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/utilities.go:Checker.isUncheckedJSSuggestion
    pub(crate) fn is_unchecked_js_suggestion(
        &mut self,
        node: Option<NodeId>,
        suggestion: Option<ts_arena::SymbolId>,
        exclude_classes: bool,
    ) -> Result<bool, Error> {
        let Some(node) = node else {
            return Ok(false);
        };
        let view = self.ast(node)?;
        let Some(file_id) = ts_ast::utilities::get_source_file_of_node(view, Some(node))? else {
            return Ok(false);
        };
        let file = view.source_file(file_id)?;
        if self.program()?.host.options().check_js != ts_core::Tristate::UNKNOWN
            || file.check_js_directive.is_some()
            || !matches!(
                file.script_kind,
                ts_core::ScriptKind::JS | ts_core::ScriptKind::JSX
            )
        {
            return Ok(false);
        }
        let mut declaration_file = None;
        let mut suggestion_has_no_extends_or_decorators = true;
        let mut suggestion_is_class = false;
        if let Some(suggestion) = suggestion {
            if let Some(first) = self.symbol_declarations(suggestion)?.first().flatten() {
                declaration_file =
                    ts_ast::utilities::get_source_file_of_node(self.ast(first)?, Some(first))?;
            }
            suggestion_is_class = self.symbol(suggestion)?.flags() & sf::CLASS != 0;
            if let Some(value) = self.symbol(suggestion)?.value_declaration() {
                let read = self.node(value)?;
                if ts_ast::utilities::is_class_like(&read) {
                    suggestion_has_no_extends_or_decorators = !self
                        .class_heritage_nodes(value, K::ExtendsKeyword)?
                        .is_empty()
                        || self.class_or_constructor_parameter_is_decorated(value)?;
                }
            }
        }
        let foreign_global = match declaration_file {
            Some(declaration_file) if declaration_file != file_id => {
                ts_ast::utilities_middle::is_global_source_file(
                    self.ast(declaration_file)?,
                    declaration_file,
                )?
            }
            _ => false,
        };
        let read = view.node(node)?;
        let this_access = read.kind() == K::PropertyAccessExpression
            && read
                .expression()
                .map(|expression| Ok::<_, Error>(view.node(expression)?.kind() == K::ThisKeyword))
                .transpose()?
                .unwrap_or(false);
        if foreign_global {
            return Ok(false);
        }
        if exclude_classes
            && suggestion_has_no_extends_or_decorators
            && (suggestion_is_class || this_access)
        {
            return Ok(false);
        }
        Ok(true)
    }

    // port: tsc/internal/ast/utilities.go:ClassOrConstructorParameterIsDecorated
    fn class_or_constructor_parameter_is_decorated(&self, class: NodeId) -> Result<bool, Error> {
        if self.has_decorators(class)? {
            return Ok(true);
        }
        for member in self.source_list(class, self.node(class)?.member_list())? {
            let read = self.node(member)?;
            if read.kind() == K::Constructor && read.body().is_some() {
                for parameter in self.source_list(member, read.parameter_list())? {
                    if self.has_decorators(parameter)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/ast/utilities.go:HasDecorators
    fn has_decorators(&self, node: NodeId) -> Result<bool, Error> {
        for modifier in self.source_list(node, self.node(node)?.modifiers())? {
            if self.node(modifier)?.kind() == K::Decorator {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.maybeMappedType
    fn maybe_mapped_type(
        &mut self,
        node: NodeId,
        symbol: ts_arena::SymbolId,
    ) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let mut current = node;
        loop {
            let Some(parent) = view.node(current)?.parent() else {
                return Ok(false);
            };
            current = parent;
            if !matches!(
                view.node(current)?.kind().known(),
                Some(K::ComputedPropertyName | K::PropertySignature)
            ) {
                break;
            }
        }
        let read = view.node(current)?;
        if read.kind() != K::TypeLiteral
            || self.source_list(current, read.member_list())?.len() != 1
        {
            return Ok(false);
        }
        let ty = self.get_declared_type_of_symbol(symbol)?;
        if self.types.flags(ty)? & tf::UNION == 0 {
            return Ok(false);
        }
        for &part in self.types.compound_types(ty)?.clone().iter() {
            if !self.type_assignable_to_kind_strict(part, tf::STRING_OR_NUMBER_LITERAL)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
