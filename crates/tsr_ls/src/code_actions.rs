use crate::{CompletionOptions, LanguageService, OrganizeMode, OrganizeOptions, Result};
use tsr_ast::NodeId;
use tsr_checker::Operation;
use tsr_compiler::diagnostic_writer::DiagnosticSources;
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    // port: tsc/internal/ls/codeactions.go:LanguageService.ProvideCodeActions
    pub fn code_actions(
        &mut self,
        checker: &mut Operation<'_>,
        params: &lsp::CodeActionParams,
        organize: &OrganizeOptions,
        options: &CompletionOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<lsp::CommandOrCodeActionArrayOrNull> {
        self.check_canceled()?;
        let source = self.file(&params.text_document.uri)?;
        let mut actions = Vec::new();
        if let Some(only) = params.context.as_deref().and_then(|c| c.only.as_deref()) {
            for requested in only {
                for (kind, mode, title) in [
                    (
                        "source.organizeImports.ts",
                        OrganizeMode::Organize,
                        tsr_diagnostics::Organize_Imports,
                    ),
                    (
                        "source.removeUnusedImports.ts",
                        OrganizeMode::RemoveUnused,
                        tsr_diagnostics::Remove_Unused_Imports,
                    ),
                    (
                        "source.sortImports.ts",
                        OrganizeMode::Sort,
                        tsr_diagnostics::Sort_Imports,
                    ),
                ] {
                    if !contains(&requested.0, kind) {
                        continue;
                    }
                    let edits =
                        self.organize_imports(checker, source, mode, &options.format, organize)?;
                    actions.push(lsp::CommandOrCodeAction {
                        code_action: Some(Box::new(lsp::CodeAction {
                            title: String::from_utf8_lossy(&title.localize(locale, &[]))
                                .into_owned(),
                            kind: Some(Box::new(lsp::CodeActionKind(kind.into()))),
                            edit: Some(Box::new(lsp::WorkspaceEdit {
                                changes: Some(Box::new(
                                    edits
                                        .into_iter()
                                        .map(|(k, v)| {
                                            (k, v.into_iter().map(|e| Some(Box::new(e))).collect())
                                        })
                                        .collect(),
                                )),
                                ..Default::default()
                            })),
                            ..Default::default()
                        })),
                        ..Default::default()
                    });
                }
                if contains(&requested.0, "source.fixAll.ts") {
                    let mut edits = Vec::new();
                    for provider in PROVIDERS {
                        if let Some(fix) =
                            self.all_fixes(checker, source, provider, options, organize, locale)?
                        {
                            edits.extend(fix.edits);
                        }
                    }
                    if !edits.is_empty() {
                        actions.push(action(
                            localized(tsr_diagnostics::Fix_All, locale, &[]),
                            "source.fixAll.ts",
                            &params.text_document.uri,
                            edits,
                            None,
                        ));
                    }
                }
            }
        }
        if let Some(context) = params.context.as_deref().filter(|context| {
            context.only.as_deref().is_none_or(|only| {
                only.is_empty() || only.iter().any(|kind| contains(&kind.0, "quickfix"))
            })
        }) {
            let mut seen: Vec<Fix> = Vec::new();
            let mut matched = [false; 3];
            for diagnostic in context.diagnostics.iter().flatten() {
                let Some(code) = diagnostic
                    .code
                    .as_deref()
                    .and_then(|code| code.integer.as_deref())
                    .copied()
                else {
                    continue;
                };
                if diagnostic
                    .source
                    .as_deref()
                    .is_some_and(|s| s.as_str() != "ts")
                {
                    continue;
                }
                for (index, provider) in PROVIDERS.into_iter().enumerate() {
                    if !provider.codes().contains(&code) {
                        continue;
                    }
                    let spans = self.converters.from_lsp_range_for_source_file(
                        self.program,
                        source,
                        &diagnostic.range,
                        tsr_ast::span_map::FEATURE_CODE_ACTIONS,
                    )?;
                    for span in spans {
                        let fixes = match provider {
                            Provider::Import => self.import_fixes(
                                checker,
                                span.script,
                                span.mapped.span,
                                code,
                                Some(diagnostic),
                                options,
                                organize,
                                locale,
                            )?,
                            Provider::Isolated => self.isolated_fixes(
                                checker,
                                span.script,
                                span.mapped.span,
                                options,
                                locale,
                            )?,
                            Provider::Class => self.class_fixes(
                                checker,
                                span.script,
                                span.mapped.span,
                                options,
                                locale,
                            )?,
                        };
                        for fix in fixes {
                            if seen
                                .iter()
                                .any(|old| old.title == fix.title && old.edits == fix.edits)
                            {
                                continue;
                            }
                            actions.push(action(
                                fix.title.clone(),
                                "quickfix",
                                &params.text_document.uri,
                                fix.edits.clone(),
                                Some(diagnostic.clone()),
                            ));
                            seen.push(fix);
                            matched[index] = true;
                        }
                    }
                }
            }
            if matched.iter().any(|&b| b) {
                let diagnostics = self.action_diagnostics(checker, source)?;
                for (index, provider) in PROVIDERS.into_iter().enumerate() {
                    if !matched[index]
                        || diagnostics
                            .iter()
                            .filter(|d| d.source.is_empty() && provider.codes().contains(&d.code))
                            .take(2)
                            .count()
                            < 2
                    {
                        continue;
                    }
                    if let Some(fix) =
                        self.all_fixes(checker, source, provider, options, organize, locale)?
                    {
                        actions.push(action(
                            fix.title,
                            "quickfix",
                            &params.text_document.uri,
                            fix.edits,
                            None,
                        ));
                    }
                }
            }
        }
        Ok(lsp::CommandOrCodeActionArrayOrNull {
            command_or_code_action_array: Some(Box::new(actions)),
        })
    }
    fn all_fixes(
        &mut self,
        checker: &mut Operation<'_>,
        source: NodeId,
        provider: Provider,
        options: &CompletionOptions,
        organize: &OrganizeOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<Fix>> {
        match provider {
            Provider::Import => self.all_import_fixes(checker, source, options, organize, locale),
            Provider::Isolated => self.all_isolated_fixes(checker, source, options, locale),
            Provider::Class => self.all_class_fixes(checker, source, options, locale),
        }
    }
    // port: tsc/internal/ls/diagnostics.go:getAllDiagnostics
    pub(crate) fn action_diagnostics(
        &self,
        checker: &mut Operation<'_>,
        source: NodeId,
    ) -> Result<Vec<tsr_ast::Diagnostic>> {
        let mut diagnostics = Vec::new();
        for id in std::iter::once(source).chain(self.program.supplemental_sources(source)?) {
            self.check_canceled()?;
            let file = self
                .program
                .file_of_node(id)
                .ok_or(tsr_arena::Error::WrongOwner)?;
            diagnostics.extend(self.program.syntactic_diagnostics(Some(file))?);
            diagnostics.extend(
                self.program.filter_and_sort_diagnostics(
                    &self.program.semantic_diagnostics_in(
                        checker,
                        file,
                        Some(&self.cancellation),
                    )?,
                )?,
            );
            diagnostics.extend(
                self.program.filter_and_sort_diagnostics(
                    &self.program.suggestion_diagnostics_in(
                        checker,
                        file,
                        Some(&self.cancellation),
                    )?,
                )?,
            );
            if self.program.options().emit_declarations() {
                diagnostics.extend(self.program.declaration_diagnostics(checker, Some(file))?);
            }
        }
        Ok(diagnostics)
    }
}
pub(crate) struct Fix {
    pub title: String,
    pub edits: Vec<lsp::TextEdit>,
}
#[derive(Clone, Copy)]
pub(crate) enum Provider {
    Import,
    Isolated,
    Class,
}
const PROVIDERS: [Provider; 3] = [Provider::Import, Provider::Isolated, Provider::Class];
pub(crate) fn localized(
    message: &'static tsr_diagnostics::Message,
    locale: &tsr_locale::Locale,
    args: &[&[u8]],
) -> String {
    let args: Vec<_> = args
        .iter()
        .map(|s| tsr_diagnostics::Argument::Bytes(s.to_vec()))
        .collect();
    String::from_utf8_lossy(&message.localize(locale, &args)).into_owned()
}
fn action(
    title: String,
    kind: &str,
    uri: &lsp::DocumentUri,
    edits: Vec<lsp::TextEdit>,
    diagnostic: Option<Box<lsp::Diagnostic>>,
) -> lsp::CommandOrCodeAction {
    lsp::CommandOrCodeAction {
        code_action: Some(Box::new(lsp::CodeAction {
            title,
            kind: Some(Box::new(lsp::CodeActionKind(kind.into()))),
            edit: Some(Box::new(lsp::WorkspaceEdit {
                changes: Some(Box::new(std::collections::HashMap::from([(
                    uri.clone(),
                    edits.into_iter().map(|edit| Some(Box::new(edit))).collect(),
                )]))),
                ..Default::default()
            })),
            diagnostics: diagnostic.map(|d| Box::new(vec![Some(d)])),
            ..Default::default()
        })),
        ..Default::default()
    }
}
fn contains(parent: &str, child: &str) -> bool {
    parent.is_empty()
        || parent == child
        || child
            .strip_prefix(parent)
            .is_some_and(|tail| tail.starts_with('.'))
}

impl Provider {
    pub(crate) fn codes(self) -> Vec<i32> {
        match self {
            Self::Import => vec![
                tsr_diagnostics::Cannot_find_name_0.code,
                tsr_diagnostics::Cannot_find_name_0_Did_you_mean_1.code,
                tsr_diagnostics::Cannot_find_name_0_Did_you_mean_the_instance_member_this_0.code,
                tsr_diagnostics::Cannot_find_name_0_Did_you_mean_the_static_member_1_0.code,
                tsr_diagnostics::Cannot_find_namespace_0.code,
                tsr_diagnostics::X_0_refers_to_a_UMD_global_but_the_current_file_is_a_module_Consider_adding_an_import_instead.code,
                tsr_diagnostics::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here.code,
                tsr_diagnostics::No_value_exists_in_scope_for_the_shorthand_property_0_Either_declare_one_or_provide_an_initializer.code,
                tsr_diagnostics::X_0_cannot_be_used_as_a_value_because_it_was_imported_using_import_type.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_1_or_later.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_include_dom.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha_and_then_add_jest_or_mocha_to_the_types_field_in_your_tsconfig.code,
                tsr_diagnostics::Cannot_find_name_0_Did_you_mean_to_write_this_in_an_async_function.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery_and_then_add_jquery_to_the_types_field_in_your_tsconfig.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode.code,
                tsr_diagnostics::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode_and_then_add_node_to_the_types_field_in_your_tsconfig.code,
                tsr_diagnostics::Cannot_find_namespace_0_Did_you_mean_1.code,
                tsr_diagnostics::Cannot_extend_an_interface_0_Did_you_mean_implements.code,
                tsr_diagnostics::This_JSX_tag_requires_0_to_be_in_scope_but_it_could_not_be_found.code,
            ],
            Self::Isolated => vec![
                tsr_diagnostics::Function_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations.code,
                tsr_diagnostics::Method_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations.code,
                tsr_diagnostics::At_least_one_accessor_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code,
                tsr_diagnostics::Variable_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code,
                tsr_diagnostics::Parameter_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code,
                tsr_diagnostics::Property_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code,
                tsr_diagnostics::Expression_type_can_t_be_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Binding_elements_with_initializers_can_t_be_exported_directly_with_isolatedDeclarations.code,
                tsr_diagnostics::Computed_property_names_on_class_or_object_literals_cannot_be_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Computed_properties_must_be_number_or_string_literals_variables_or_dotted_expressions_with_isolatedDeclarations.code,
                tsr_diagnostics::Enum_member_initializers_must_be_computable_without_references_to_external_symbols_with_isolatedDeclarations.code,
                tsr_diagnostics::Extends_clause_can_t_contain_an_expression_with_isolatedDeclarations.code,
                tsr_diagnostics::Objects_that_contain_shorthand_properties_can_t_be_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Objects_that_contain_spread_assignments_can_t_be_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Arrays_with_spread_elements_can_t_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Default_exports_can_t_be_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Only_const_arrays_can_be_inferred_with_isolatedDeclarations.code,
                tsr_diagnostics::Assigning_properties_to_functions_without_declaring_them_is_not_supported_with_isolatedDeclarations_Add_an_explicit_declaration_for_the_properties_assigned_to_this_function.code,
                tsr_diagnostics::Declaration_emit_for_this_parameter_requires_implicitly_adding_undefined_to_its_type_This_is_not_supported_with_isolatedDeclarations.code,
                tsr_diagnostics::Type_containing_private_name_0_can_t_be_used_with_isolatedDeclarations.code,
                tsr_diagnostics::Add_satisfies_and_a_type_assertion_to_this_expression_satisfies_T_as_T_to_make_the_type_explicit.code,
            ],
            Self::Class => vec![
                tsr_diagnostics::Class_0_incorrectly_implements_interface_1.code,
                tsr_diagnostics::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass.code,
            ],
        }
    }
}
