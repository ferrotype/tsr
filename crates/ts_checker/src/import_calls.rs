//! Dynamic imports check their own arguments and return a promise of the
//! resolved module namespace. They never use ordinary call-signature resolution.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;
use ts_core::ModuleKind;
use ts_diagnostics as d;

impl CheckerState {
    // Native NewChecker has separate reporting and non-reporting global resolvers.
    pub(crate) fn global_import_call_options_type(
        &mut self,
        report: bool,
    ) -> Result<TypeId, Error> {
        let index = usize::from(report);
        if let Some(ty) = self.module_aliases.global_import_call_options[index] {
            return Ok(ty);
        }
        let ty = self.get_global_type("ImportCallOptions", 0, report)?;
        self.module_aliases.global_import_call_options[index] = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkImportCallExpression
    pub(crate) fn check_import_call_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_grammar_import_call_expression(node)?;
        let args = self.source_list(node, self.ast(node)?.node(node)?.argument_list())?;
        let Some(&specifier) = args.first() else {
            return self.create_promise_return_type(node, self.builtins.any_type);
        };
        let specifier_type = self.check_expression_cached(specifier)?;
        let options_type = args
            .get(1)
            .map(|&arg| self.check_expression_cached(arg))
            .transpose()?;
        // Grammar failure does not skip semantic checking of excess arguments.
        for &argument in args.iter().skip(2) {
            self.check_expression_cached(argument)?;
        }
        if self.types.flags(specifier_type)? & tf::NULLABLE != 0
            || !self.is_type_related_to(
                specifier_type,
                self.builtins.string_type,
                crate::RelationKind::Assignable,
            )?
        {
            let text = self.type_to_string(
                specifier_type,
                crate::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                    | crate::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            )?;
            self.error_at(
                Some(specifier),
                d::Dynamic_import_s_specifier_must_be_of_type_string_but_here_has_type_0,
                vec![text],
            )?;
        }
        if let Some(options_type) = options_type {
            let global = self.global_import_call_options_type(true)?;
            if global != self.builtins.empty_object_type {
                let target = self.nullable_type(global, tf::UNDEFINED)?;
                self.check_assignable_at(options_type, target, args[1])?;
            }
            let read = self.ast(args[1])?.node(args[1])?;
            if read.kind() == K::ObjectLiteralExpression {
                for property in self.source_list(args[1], read.property_list())? {
                    let read = self.ast(property)?.node(property)?;
                    if read.kind() == K::PropertyAssignment {
                        if let Some(name) = read.name() {
                            if self.ast(name)?.node(name)?.kind() == K::Identifier
                                && self.ast(name)?.node_text(name)?.as_bytes() == b"assert"
                            {
                                self.error_at(Some(name), d::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert, vec![])?;
                                break;
                            }
                        }
                    }
                }
            }
        }
        // The resolver reads `with` from the already checked/cached options.
        // Nonliteral specifiers deliberately resolve to no module.
        if let Some(module) = self.resolve_external_module_name(node, specifier, false)? {
            if let Some(symbol) = self.resolve_external_module_symbol(Some(module), true)? {
                let ty = self.get_type_of_symbol(symbol)?;
                let ty = match self.module_default_only_type(ty, symbol, module, specifier)? {
                    Some(ty) => ty,
                    None => self.module_synthetic_default_type(ty, symbol, module, specifier)?,
                };
                return self.create_promise_return_type(node, ty);
            }
        }
        self.create_promise_return_type(node, self.builtins.any_type)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarImportCallExpression
    fn check_grammar_import_call_expression(&mut self, node: NodeId) -> Result<bool, Error> {
        let options = self.program()?.host.options();
        let kind = options.emit_module_kind();
        if options.verbatim_module_syntax.is_true() && kind == ModuleKind::COMMON_JS {
            let (_, file) = self.module_source(node)?;
            let message = if file.as_bytes().ends_with(b".cts")
                || file.as_bytes().ends_with(b".cjs")
            {
                d::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax
            } else {
                d::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax_Adjust_the_type_field_in_the_nearest_package_json_to_make_this_file_an_ECMAScript_module_or_adjust_your_verbatimModuleSyntax_module_and_moduleResolution_settings_in_TypeScript
            };
            return self.grammar_error_node(node, message, vec![]);
        }
        let read = self.ast(node)?.node(node)?;
        let expression = read
            .expression()
            .ok_or(Error::MissingLink("import callee"))?;
        if self.ast(expression)?.node(expression)?.kind() == K::MetaProperty {
            if kind != ModuleKind::ESNEXT && kind != ModuleKind::PRESERVE {
                return self.grammar_error_node(node, d::Deferred_imports_are_only_supported_when_the_module_flag_is_set_to_esnext_or_preserve, vec![]);
            }
        } else if kind == ModuleKind::ES2015 {
            return self.grammar_error_node(node, d::Dynamic_imports_are_only_supported_when_the_module_flag_is_set_to_es2020_es2022_esnext_commonjs_amd_system_umd_node16_node18_node20_or_nodenext, vec![]);
        }
        if read.type_argument_list().is_some() {
            return self.grammar_error_node(node, d::This_use_of_import_is_invalid_import_calls_can_be_written_but_they_must_have_parentheses_and_cannot_have_type_arguments, vec![]);
        }
        let list = read.argument_list();
        let args = self.source_list(node, list)?;
        if !(ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&kind)
            && kind != ModuleKind::ESNEXT
            && kind != ModuleKind::PRESERVE
        {
            if let Some(list) = list {
                self.check_grammar_trailing_comma(node, list, d::Trailing_comma_not_allowed)?;
            }
            if let Some(&options) = args.get(1) {
                return self.grammar_error_node(options, d::Dynamic_imports_only_support_a_second_argument_when_the_module_option_is_set_to_esnext_node16_node18_node20_nodenext_or_preserve, vec![]);
            }
        }
        if args.is_empty() || args.len() > 2 {
            return self.grammar_error_node(node, d::Dynamic_imports_can_only_accept_a_module_specifier_and_an_optional_set_of_attributes_as_arguments, vec![]);
        }
        for argument in args {
            if self.ast(argument)?.node(argument)?.kind() == K::SpreadElement {
                return self.grammar_error_node(
                    argument,
                    d::Argument_of_dynamic_import_cannot_be_spread_element,
                    vec![],
                );
            }
        }
        Ok(false)
    }
}
