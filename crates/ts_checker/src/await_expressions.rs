//! Await diagnostics share the promise adoption algorithm with async returns.
use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{modifier_flags as mf, node_flags as nf, SyntaxKind as K};
use ts_core::{ModuleKind as M, ScriptTarget};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkAwaitExpression
    pub(crate) fn check_await_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_grammar_await_or_await_using(node)?;
        let operand = self
            .ast(node)?
            .node(node)?
            .expression()
            .ok_or(Error::MissingLink("await operand"))?;
        let ty = self.check_expression(operand)?;
        let result=self.awaited_type_no_alias_ex(ty,Some(node),Some(d::Type_of_await_operand_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member),&[])?;
        let awaited = match result {
            Some(ty) => self.create_awaited_type_if_needed(ty)?,
            None => self.builtins.error_type,
        };
        if awaited == ty
            && !self.is_error_type(awaited)?
            && self.types.flags(ty)? & tf::ANY_OR_UNKNOWN == 0
        {
            let mut diagnostic = self.diagnostic_for_node(
                Some(node),
                d::X_await_has_no_effect_on_the_type_of_this_expression,
                vec![],
            )?;
            diagnostic.category = d::Category::Suggestion as i32;
            self.add_suggestion_diagnostic(diagnostic)?;
        }
        Ok(awaited)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarAwaitOrAwaitUsing
    pub(crate) fn check_grammar_await_or_await_using(
        &mut self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let await_expression = self.ast(node)?.node(node)?.kind() == K::AwaitExpression;
        let mut has_error = false;
        let container = self.containing_function_or_static_block(node)?;
        let static_block = container
            .map(|container| {
                self.ast(container)?
                    .node(container)
                    .map(|read| read.kind() == K::ClassStaticBlockDeclaration)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        if static_block {
            // NOTE: We report this regardless as to whether there are parse diagnostics.
            self.error_at(
                Some(node),
                if await_expression {
                    d::X_await_expression_cannot_be_used_inside_a_class_static_block
                } else {
                    d::X_await_using_statements_cannot_be_used_inside_a_class_static_block
                },
                vec![],
            )?;
            has_error = true;
        } else if self.ast(node)?.node(node)?.flags() & nf::AWAIT_CONTEXT == 0 {
            let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
                .ok_or(Error::MissingLink("await source"))?;
            let file = self.ast(source)?.source_file(source)?;
            if file.diagnostics().is_empty() {
                let range = ts_scanner::get_range_of_token_at_position(
                    self.ast(source)?,
                    source,
                    i64::from(self.ast(node)?.node(node)?.pos()),
                )?;
                if ts_ast::is_in_top_level_context(self.ast(node)?, node)? {
                    let module = self.program()?.host.options().emit_module_kind();
                    let target = self.program()?.host.options().emit_script_target();
                    let file = self.ast(source)?.source_file(source)?;
                    let external = file.external_module_indicator.is_some()
                        || (module == M::COMMON_JS
                            || M::NODE16 <= module && module <= M::NODE_NEXT)
                            && file.common_js_module_indicator().is_some();
                    if !external {
                        let message = if await_expression {
                            d::X_await_expressions_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module
                        } else {
                            d::X_await_using_statements_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module
                        };
                        self.add_diagnostic(ts_ast::Diagnostic::new(
                            Some(source),
                            range,
                            message,
                            vec![],
                        ))?;
                        has_error = true;
                    }
                    let node_module =
                        matches!(module, M::NODE16 | M::NODE18 | M::NODE20 | M::NODE_NEXT);
                    let common_js = if node_module {
                        let file = self.ast(source)?.source_file(source)?;
                        self.program()?
                            .host
                            .get_source_file_meta_data(file.parse_options().file_name.as_bytes())?
                            .implied_node_format
                            == M::COMMON_JS
                    } else {
                        false
                    };
                    if common_js {
                        self.add_diagnostic(ts_ast::Diagnostic::new(Some(source),range,d::The_current_file_is_a_CommonJS_module_and_cannot_use_await_at_the_top_level,vec![]))?;
                        has_error = true;
                    } else if !(node_module
                        || matches!(module, M::ES2022 | M::ESNEXT | M::PRESERVE | M::SYSTEM))
                        || target < ScriptTarget::ES2017
                    {
                        let message = if await_expression {
                            d::Top_level_await_expressions_are_only_allowed_when_the_module_option_is_set_to_es2022_esnext_system_node16_node18_node20_nodenext_or_preserve_and_the_target_option_is_set_to_es2017_or_higher
                        } else {
                            d::Top_level_await_using_statements_are_only_allowed_when_the_module_option_is_set_to_es2022_esnext_system_node16_node18_node20_nodenext_or_preserve_and_the_target_option_is_set_to_es2017_or_higher
                        };
                        self.add_diagnostic(ts_ast::Diagnostic::new(
                            Some(source),
                            range,
                            message,
                            vec![],
                        ))?;
                        has_error = true;
                    }
                } else {
                    let message = if await_expression {
                        d::X_await_expressions_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules
                    } else {
                        d::X_await_using_statements_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules
                    };
                    let mut diagnostic =
                        ts_ast::Diagnostic::new(Some(source), range, message, vec![]);
                    has_error = true;
                    if let Some(container) = container {
                        let read = self.ast(container)?.node(container)?;
                        if read.kind() != K::Constructor
                            && read.modifier_flags(self.ast(container)?)? & mf::ASYNC == 0
                        {
                            diagnostic.related_information.push(std::sync::Arc::new(
                                self.diagnostic_for_node(
                                    Some(container),
                                    d::Did_you_mean_to_mark_this_function_as_async,
                                    vec![],
                                )?,
                            ));
                        }
                    }
                    self.add_diagnostic(diagnostic)?;
                }
            }
        }
        if await_expression && self.in_parameter_initializer_before_function(node)? {
            // NOTE: We report this regardless as to whether there are parse diagnostics.
            self.error_at(
                Some(node),
                d::X_await_expressions_cannot_be_used_in_a_parameter_initializer,
                vec![],
            )?;
            has_error = true;
        }
        Ok(has_error)
    }
}
