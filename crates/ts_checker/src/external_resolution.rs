//! External names consume the program's already-published, mode-specific
//! resolution. Checker diagnostics and ambient declarations remain checker-owned.
use crate::{CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, JsString, SyntaxKind as K};
use ts_core::{ModuleKind, ModuleResolutionKind};
use ts_diagnostics::{self as d, Message};
use ts_tspath as path;

#[path = "external_resolution_context.rs"]
mod context;

struct ExternalModuleReference {
    location: NodeId,
    module_reference: JsString,
    error_node: Option<NodeId>,
    message: Option<&'static Message>,
    augmentation: bool,
    mode: ModuleKind,
    attributes: Option<crate::TypeId>,
}

impl CheckerState {
    pub(crate) fn module_source(&self, node: NodeId) -> Result<(NodeId, JsString), Error> {
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("external module source file"))?;
        Ok((
            source,
            self.ast(source)?
                .source_file(source)?
                .parse_options()
                .file_name
                .clone(),
        ))
    }

    // port: tsc/internal/checker/checker.go:Checker.getModuleSpecifierForImportOrExport
    pub(crate) fn module_specifier(&self, mut node: NodeId) -> Result<Option<NodeId>, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                    return Ok(read
                        .data_source()
                        .as_import_declaration()
                        .and_then(|data| data.module_specifier()))
                }
                Some(K::ExportDeclaration) => {
                    return Ok(read
                        .data_source()
                        .as_export_declaration()
                        .and_then(|data| data.module_specifier()))
                }
                Some(K::ImportEqualsDeclaration) => {
                    let reference = read
                        .data_source()
                        .as_import_equals_declaration()
                        .and_then(|data| data.module_reference())
                        .ok_or(Error::MissingLink("import equals module reference"))?;
                    let read = self.ast(reference)?.node(reference)?;
                    return Ok((read.kind() == K::ExternalModuleReference)
                        .then(|| read.expression())
                        .flatten());
                }
                Some(K::ModuleDeclaration) => return Ok(read.name()),
                // `const x = require("m")` aliases name their module through the call.
                Some(K::VariableDeclaration) => {
                    let Some(initializer) = read.initializer() else {
                        return Ok(None);
                    };
                    let view = self.ast(initializer)?;
                    let call = view.node(initializer)?;
                    if !ts_ast::utilities_middle::is_require_call(view, &call, true)? {
                        return Ok(None);
                    }
                    let arguments = call.argument_list();
                    return Ok(self.source_list(initializer, arguments)?.first().copied());
                }
                Some(K::SourceFile) => return Ok(None),
                _ => {
                    node = match read.parent() {
                        Some(parent) => parent,
                        None => return Ok(None),
                    }
                }
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.tryFindAmbientModule
    fn try_find_ambient_module(&mut self, name: &[u8]) -> Result<Option<SymbolId>, Error> {
        if ts_module::is_relative(name) {
            return Ok(None);
        }
        let mut key = Vec::with_capacity(name.len() + 2);
        key.push(b'"');
        key.extend_from_slice(name);
        key.push(b'"');
        let Some(globals) = self.builtins.globals else {
            return Ok(None);
        };
        let Some(symbol) = self.table(globals)?.get(&key).flatten() else {
            return Ok(None);
        };
        let symbol = self.get_merged_symbol(symbol);
        Ok((self.symbol(symbol)?.flags() & sf::VALUE_MODULE != 0).then_some(symbol))
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModuleName
    pub(crate) fn resolve_external_module_name(
        &mut self,
        location: NodeId,
        specifier: NodeId,
        ignore_errors: bool,
    ) -> Result<Option<SymbolId>, Error> {
        let mut message = d::Cannot_find_module_0_or_its_corresponding_type_declarations;
        if self.ast(specifier)?.node(specifier)?.kind() == K::StringLiteral
            && node_core_module(self.ast(specifier)?.node_text(specifier)?.as_bytes())
        {
            message = if self.program()?.host.options().uses_wildcard_types() {
                d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode
            } else {
                d::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode_and_then_add_node_to_the_types_field_in_your_tsconfig
            };
        }
        self.resolve_external_module_name_with_error(
            location,
            specifier,
            ignore_errors,
            Some(message),
            false,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModuleNameWorker
    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModule
    pub(crate) fn resolve_external_module_name_with_error(
        &mut self,
        location: NodeId,
        specifier: NodeId,
        ignore_errors: bool,
        message: Option<&'static Message>,
        augmentation: bool,
    ) -> Result<Option<SymbolId>, Error> {
        if !matches!(
            self.ast(specifier)?.node(specifier)?.kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        ) {
            return Ok(None);
        }
        let module_reference = self.ast(specifier)?.node_text(specifier)?.into_js_string();
        let host = self.program()?.host.clone();
        let (_, file_name) = self.module_source(location)?;
        let mode = host.get_mode_for_usage_location(file_name.as_bytes(), specifier)?;
        let error_node =
            (!ignore_errors && !host.options().no_check.is_true()).then_some(specifier);
        let attributes = self.import_attributes_type_for_specifier(specifier)?;
        self.resolve_external_module_reference(ExternalModuleReference {
            location,
            module_reference,
            error_node,
            message,
            augmentation,
            mode,
            attributes,
        })
    }

    // The loader represents this synthetic import as a side-effect import with
    // no attributes. Its mode is the host's ordinary import resolution mode and its
    // resolution is retained under `tslib`, exactly as for ordinary imports.
    // port: tsc/internal/checker/checker.go:Checker.resolveHelpersModule
    pub(crate) fn resolve_external_helpers_module(
        &mut self,
        source: NodeId,
        error_node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let (_, file_name) = self.module_source(source)?;
        let mode = self
            .program()?
            .host
            .get_import_helpers_resolution_mode(file_name.as_bytes())?;
        self.resolve_external_module_reference(ExternalModuleReference {
            location: source,
            module_reference: JsString::from_bytes(b"tslib".as_slice()),
            error_node: Some(error_node),
            message: Some(d::This_syntax_requires_an_imported_helper_but_module_0_cannot_be_found),
            augmentation: false,
            mode,
            attributes: None,
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModule
    fn resolve_external_module_reference(
        &mut self,
        site: ExternalModuleReference,
    ) -> Result<Option<SymbolId>, Error> {
        let ExternalModuleReference {
            location,
            module_reference,
            error_node,
            message,
            augmentation,
            mode,
            attributes,
        } = site;
        let host = self.program()?.host.clone();
        let options = host.options();
        // Native side-effect checks inspect the diagnostic node, not the
        // synthetic import whose location supplied the resolution mode.
        let side_effect = error_node
            .map(|node| self.side_effect_import(node))
            .transpose()?
            .unwrap_or(false);
        if let Some(error) = error_node {
            if let Some(without) = module_reference.as_bytes().strip_prefix(b"@types/") {
                self.error_at(
                    Some(error),
                    d::Cannot_import_type_declaration_files_Consider_importing_0_instead_of_1,
                    vec![JsString::from_bytes(without), module_reference.clone()],
                )?;
            }
        }
        let attributes = attributes.unwrap_or(self.builtins.empty_object_type);
        if let Some(ambient) = self.try_find_ambient_module(module_reference.as_bytes())? {
            return self.resolve_attributed_pattern_module(
                Some(ambient),
                module_reference.as_bytes(),
                attributes,
            );
        }
        let (source, file_name) = self.module_source(location)?;
        let resolved =
            host.get_resolved_module(file_name.as_bytes(), module_reference.as_bytes(), mode)?;
        let is_declaration_file = self.ast(source)?.source_file(source)?.is_declaration_file;
        let diagnostic = if error_node.is_some() {
            resolved
                .filter(|resolved| resolved.is_resolved())
                .and_then(|resolved| {
                    ts_module::resolution_diagnostic(options, resolved, is_declaration_file)
                })
        } else {
            None
        };
        let target = resolved
            .filter(|resolved| {
                resolved.is_resolved()
                    && (diagnostic.is_none()
                        || diagnostic == Some(d::Module_0_was_resolved_to_1_but_jsx_is_not_set))
            })
            .and_then(|resolved| {
                host.get_source_file_for_resolved_module(resolved.resolved_file_name.as_bytes())
            });
        if let Some(file) = target {
            let resolved = resolved.ok_or(Error::MissingLink("resolved module target"))?;
            if let Some(diagnostic) = diagnostic {
                self.error_at(
                    error_node,
                    diagnostic,
                    vec![
                        module_reference.clone(),
                        resolved.resolved_file_name.clone(),
                    ],
                )?;
            }
            if let Some(error) = error_node {
                self.check_resolved_ts_extension(
                    location,
                    error,
                    &module_reference,
                    resolved,
                    mode,
                    file,
                )?;
            }
            if let Some(symbol) = file
                .view()
                .node_binding(file.source())?
                .and_then(|binding| binding.symbol)
            {
                if let Some(error) = error_node {
                    if resolved.is_external_library_import
                        && !ts_or_json_extension(resolved.extension.as_bytes())
                    {
                        self.external_implicit_any_module(
                            false,
                            error,
                            resolved,
                            &module_reference,
                        )?;
                    }
                    if matches!(
                        options.emit_module_kind(),
                        ModuleKind::NODE16 | ModuleKind::NODE18
                    ) {
                        let target_name =
                            file.view().source_file()?.parse_options().file_name.clone();
                        self.check_module_kind_mismatch(
                            location,
                            error,
                            source,
                            &file_name,
                            &target_name,
                            &module_reference,
                        )?;
                    }
                }
                return self.resolve_attributed_pattern_module(
                    Some(self.get_merged_symbol(symbol)),
                    module_reference.as_bytes(),
                    attributes,
                );
            }
            if let Some(pattern) = self.resolve_attributed_pattern_module(
                None,
                module_reference.as_bytes(),
                attributes,
            )? {
                return Ok(Some(pattern));
            }
            if error_node.is_some() && message.is_some() && !side_effect {
                self.error_at(
                    error_node,
                    d::File_0_is_not_a_module,
                    vec![resolved.resolved_file_name.clone()],
                )?;
            }
            return Ok(None);
        }
        if let Some(pattern) =
            self.resolve_attributed_pattern_module(None, module_reference.as_bytes(), attributes)?
        {
            return Ok(Some(pattern));
        }
        let Some(error) = error_node else {
            return Ok(None);
        };
        if let Some(resolved) = resolved {
            if resolved.is_resolved() && !ts_or_json_extension(resolved.extension.as_bytes()) && diagnostic.is_none()
                || diagnostic==Some(d::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type) {
                if augmentation {
                    self.error_at(Some(error),d::Invalid_module_name_in_augmentation_Module_0_resolves_to_an_untyped_module_at_1_which_cannot_be_augmented,vec![module_reference,resolved.resolved_file_name.clone()])?;
                } else {
                    self.external_implicit_any_module(options.strict_option_value(options.no_implicit_any) && message.is_some(),error,resolved,&module_reference)?;
                }
                return Ok(None);
            }
        }
        if let Some(message) = message {
            if let Some(resolved) = resolved.filter(|resolved| resolved.is_resolved()) {
                if host
                    .get_project_reference_from_source(resolved.resolved_file_name.as_bytes())?
                    .is_some()
                {
                    return Err(Error::Unsupported(
                        "resolveExternalModule: project reference output",
                    ));
                }
            }
            if let Some(diagnostic) = diagnostic {
                self.error_at(
                    Some(error),
                    diagnostic,
                    vec![
                        module_reference,
                        resolved
                            .ok_or(Error::MissingLink("resolution diagnostic result"))?
                            .resolved_file_name
                            .clone(),
                    ],
                )?;
            } else if !options.resolve_json_module()
                && module_reference.as_bytes().ends_with(b".json")
            {
                self.error_at(Some(error),d::Cannot_find_module_0_Consider_using_resolveJsonModule_to_import_module_with_json_extension,vec![module_reference])?;
            } else if mode == ModuleKind::ESNEXT
                && matches!(
                    options.module_resolution_kind(),
                    ModuleResolutionKind::NODE16 | ModuleResolutionKind::NODE_NEXT
                )
                && path::is_relative(module_reference.as_bytes())
                && !path::has_extension(module_reference.as_bytes())
            {
                let absolute = path::absolute(
                    module_reference.as_bytes(),
                    &path::directory(file_name.as_bytes()),
                );
                if let Some(extension) = self.suggested_import_extension(&absolute)? {
                    let mut name = module_reference.as_bytes().to_vec();
                    name.extend_from_slice(extension);
                    self.error_at(Some(error),d::Relative_import_paths_need_explicit_file_extensions_in_ECMAScript_imports_when_moduleResolution_is_node16_or_nodenext_Did_you_mean_0,vec![JsString::from_bytes(name)])?;
                } else {
                    self.error_at(Some(error),d::Relative_import_paths_need_explicit_file_extensions_in_ECMAScript_imports_when_moduleResolution_is_node16_or_nodenext_Consider_adding_an_extension_to_the_import_path,vec![])?;
                }
            } else if resolved.is_some_and(|resolved| !resolved.alternate_result.is_empty()) {
                return Err(Error::Unsupported(
                    "resolveExternalModule: alternate package resolution diagnostic chain",
                ));
            } else {
                self.error_at(Some(error), message, vec![module_reference])?;
            }
        }
        Ok(None)
    }

    fn side_effect_import(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.ast(node)?.node(node)?.parent() else {
            return Ok(false);
        };
        Ok(self
            .ast(parent)?
            .node(parent)?
            .data_source()
            .as_import_declaration()
            .is_some_and(|data| data.import_clause().is_none()))
    }
    fn external_implicit_any_module(
        &mut self,
        is_error: bool,
        node: NodeId,
        resolved: &ts_module::ResolvedModule,
        name: &JsString,
    ) -> Result<(), Error> {
        if self.side_effect_import(node)? {
            return Ok(());
        }
        let chain =
            if !ts_module::is_relative(name.as_bytes()) && !resolved.package_id.name.is_empty() {
                Some(self.module_not_found_chain(node, resolved, name)?)
            } else {
                None
            };
        let args = vec![name.clone(), resolved.resolved_file_name.clone()];
        let diagnostic = if let Some(chain) = chain {
            ts_ast::Diagnostic::chain(
                Some(std::sync::Arc::new(chain)),
                d::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type,
                args,
            )
        } else {
            self.diagnostic_for_node(
                Some(node),
                d::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type,
                args,
            )?
        };
        if is_error {
            self.add_diagnostic(diagnostic)?;
        } else {
            self.add_suggestion_diagnostic(diagnostic)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.getSuggestedImportExtension
    fn suggested_import_extension(&self, path: &[u8]) -> Result<Option<&'static [u8]>, Error> {
        for (extension, suggestion) in [
            (b".mts".as_slice(), b".mjs".as_slice()),
            (b".ts", b".js"),
            (b".cts", b".cjs"),
            (b".mjs", b".mjs"),
            (b".js", b".js"),
            (b".cjs", b".cjs"),
            (
                b".tsx",
                if self.program()?.host.options().jsx == ts_core::JsxEmit::PRESERVE {
                    b".jsx"
                } else {
                    b".js"
                },
            ),
            (b".jsx", b".jsx"),
            (b".json", b".json"),
        ] {
            let mut name = path.to_vec();
            name.extend_from_slice(extension);
            if self.program()?.host.file_exists(&name).map_err(|_| {
                Error::Unsupported("getSuggestedImportExtension: file existence failure")
            })? {
                return Ok(Some(suggestion));
            }
        }
        Ok(None)
    }
    fn check_resolved_ts_extension(
        &mut self,
        location: NodeId,
        error: NodeId,
        name: &JsString,
        resolved: &ts_module::ResolvedModule,
        mode: ModuleKind,
        target: &ts_ast::CompletedFile,
    ) -> Result<(), Error> {
        let (_, source_name) = self.module_source(location)?;
        let emittable = self.module_import_emittable(location)?;
        if resolved.resolved_using_ts_extension
            && path::is_declaration_file_name(name.as_bytes())
            && emittable
        {
            let extension = ts_extension(name.as_bytes())
                .ok_or(Error::MissingLink("resolved declaration extension"))?;
            let options = self.program()?.host.options();
            let mut suggestion = name.as_bytes()[..name.len() - extension.len()].to_vec();
            if options.emit_module_kind().is_non_node_esm() || mode == ModuleKind::ESNEXT {
                let prefer_ts = options.allow_importing_ts_extensions();
                suggestion.extend_from_slice(match extension {
                    b".d.mts" | b".mts" => {
                        if prefer_ts {
                            b".mts"
                        } else {
                            b".mjs"
                        }
                    }
                    b".d.cts" | b".cts" => {
                        if prefer_ts {
                            b".cts"
                        } else {
                            b".cjs"
                        }
                    }
                    _ => {
                        if prefer_ts {
                            b".ts"
                        } else {
                            b".js"
                        }
                    }
                });
            }
            self.error_at(Some(error),d::A_declaration_file_cannot_be_imported_without_import_type_Did_you_mean_to_import_an_implementation_file_0_instead,vec![JsString::from_bytes(suggestion)])?;
        } else if resolved.resolved_using_ts_extension
            && !self
                .program()?
                .host
                .options()
                .allow_importing_ts_extensions_from(source_name.as_bytes())
            && emittable
        {
            let extension = ts_extension(name.as_bytes())
                .or_else(|| {
                    TS_EXTENSIONS.iter().copied().find(|extension| {
                        name.as_bytes()
                            .windows(extension.len())
                            .any(|part| part == *extension)
                    })
                })
                .ok_or(Error::MissingLink("resolved TS extension"))?;
            self.error_at(Some(error),d::An_import_path_can_only_end_with_a_0_extension_when_allowImportingTsExtensions_is_enabled,vec![JsString::from_bytes(extension)])?;
        } else if self
            .program()?
            .host
            .options()
            .rewrite_relative_import_extensions
            .is_true()
            && self.ast(location)?.node(location)?.flags() & ts_ast::node_flags::AMBIENT == 0
            && !path::is_declaration_file_name(name.as_bytes())
            && !self.is_literal_import_type_node(location)?
            && !self.is_part_of_type_only_import_or_export_declaration(location)?
        {
            // port: tsc/internal/core/core.go:ShouldRewriteModuleSpecifier
            let should_rewrite =
                specifier_is_relative(name.as_bytes()) && has_ts_file_extension(name.as_bytes());
            let host = self.program()?.host.clone();
            if !resolved.resolved_using_ts_extension && should_rewrite {
                let cwd = host.get_current_directory();
                let from = path::absolute(source_name.as_bytes(), cwd);
                let relative = path::relative_from_file(
                    &from,
                    resolved.resolved_file_name.as_bytes(),
                    cwd,
                    host.use_case_sensitive_file_names(),
                );
                self.error_at(
                    Some(error),
                    d::This_relative_import_path_is_unsafe_to_rewrite_because_it_looks_like_a_file_name_but_actually_resolves_to_0,
                    vec![JsString::from_bytes(relative.as_slice())],
                )?;
            } else if resolved.resolved_using_ts_extension
                && !should_rewrite
                && host.source_file_may_be_emitted(target, false)?
            {
                self.error_at(
                    Some(error),
                    d::This_import_uses_a_0_extension_to_resolve_to_an_input_TypeScript_file_but_will_not_be_rewritten_during_emit_because_it_is_not_a_relative_path,
                    vec![JsString::from_bytes(any_extension(name.as_bytes()))],
                )?;
            } else if resolved.resolved_using_ts_extension && should_rewrite {
                // Loading rejects project references, so the redirect comparison
                // upstream performs here has nothing to compare.
                let target_name = target
                    .view()
                    .source_file()?
                    .parse_options()
                    .file_name
                    .clone();
                if host
                    .get_redirect_for_resolution(target_name.as_bytes())?
                    .is_some()
                {
                    return Err(Error::Unsupported(
                        "resolveExternalModule: rewrite across project references",
                    ));
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/ast/utilities.go:IsLiteralImportTypeNode
    fn is_literal_import_type_node(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.ast(node)?;
        let read = view.node(node)?;
        let Some(data) = read.data_source().as_import_type_node() else {
            return Ok(false);
        };
        let Some(argument) = data.argument() else {
            return Ok(false);
        };
        let argument_read = view.node(argument)?;
        let Some(literal) = argument_read
            .data_source()
            .as_literal_type_node()
            .and_then(|data| data.literal())
        else {
            return Ok(false);
        };
        Ok(view.node(literal)?.kind() == K::StringLiteral)
    }

    // port: tsc/internal/ast/utilities.go:IsPartOfTypeOnlyImportOrExportDeclaration
    fn is_part_of_type_only_import_or_export_declaration(
        &self,
        node: NodeId,
    ) -> Result<bool, Error> {
        let view = self.ast(node)?;
        Ok(
            ts_ast::utilities::find_ancestor(view, Some(node), |candidate| {
                type_only_import_or_export_declaration(view, candidate).unwrap_or(false)
            })?
            .is_some(),
        )
    }

    // port: tsc/internal/ast/utilities.go:IsEmittableImport
    #[allow(
        clippy::match_same_arms,
        reason = "Keep the pinned upstream per-kind dispatch auditable when individual syntax cases change"
    )]
    fn module_import_emittable(&self, mut node: NodeId) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::ImportDeclaration | K::JSImportDeclaration) => {
                    let clause = read
                        .data_source()
                        .as_import_declaration()
                        .and_then(|data| data.import_clause());
                    return Ok(match clause {
                        Some(clause) => !self.ast(clause)?.node(clause)?.is_type_only(),
                        None => false,
                    });
                }
                Some(K::ImportEqualsDeclaration | K::ExportDeclaration) => {
                    return Ok(!read.is_type_only())
                }
                Some(K::ImportType) => return Ok(false),
                Some(K::SourceFile) => return Ok(false),
                _ => {
                    node = match read.parent() {
                        Some(parent) => parent,
                        None => return Ok(false),
                    }
                }
            }
        }
    }
}
const TS_EXTENSIONS: &[&[u8]] = &[
    b".d.ts", b".ts", b".tsx", b".d.mts", b".mts", b".d.cts", b".cts",
];
fn ts_extension(name: &[u8]) -> Option<&'static [u8]> {
    TS_EXTENSIONS
        .iter()
        .copied()
        .find(|extension| name.ends_with(extension))
}
fn ts_or_json_extension(extension: &[u8]) -> bool {
    TS_EXTENSIONS.contains(&extension) || extension == b".json"
}

// Source: tsc/internal/core/nodemodules.go:NodeCoreModules
fn node_core_module(name: &[u8]) -> bool {
    matches!(
        name,
        b"node:quic" | b"node:sea" | b"node:sqlite" | b"node:test" | b"node:test/reporters"
    ) || matches!(
        name.strip_prefix(b"node:").unwrap_or(name),
        b"assert"
            | b"assert/strict"
            | b"async_hooks"
            | b"buffer"
            | b"child_process"
            | b"cluster"
            | b"console"
            | b"constants"
            | b"crypto"
            | b"dgram"
            | b"diagnostics_channel"
            | b"dns"
            | b"dns/promises"
            | b"domain"
            | b"events"
            | b"fs"
            | b"fs/promises"
            | b"http"
            | b"http2"
            | b"https"
            | b"inspector"
            | b"inspector/promises"
            | b"module"
            | b"net"
            | b"os"
            | b"path"
            | b"path/posix"
            | b"path/win32"
            | b"perf_hooks"
            | b"process"
            | b"punycode"
            | b"querystring"
            | b"readline"
            | b"readline/promises"
            | b"repl"
            | b"stream"
            | b"stream/consumers"
            | b"stream/promises"
            | b"stream/web"
            | b"string_decoder"
            | b"sys"
            | b"timers"
            | b"timers/promises"
            | b"tls"
            | b"trace_events"
            | b"tty"
            | b"url"
            | b"util"
            | b"util/types"
            | b"v8"
            | b"vm"
            | b"wasi"
            | b"worker_threads"
            | b"zlib"
    )
}

impl CheckerState {
    /// The Node16/Node18 branch of `resolveExternalModule`: a synchronous import
    /// from a CommonJS file cannot load an ECMAScript module.
    // port: tsc/internal/checker/checker.go:Checker.resolveExternalModule
    fn check_module_kind_mismatch(
        &mut self,
        location: NodeId,
        error_node: NodeId,
        importing_source: NodeId,
        importing_file_name: &JsString,
        target_file_name: &JsString,
        module_reference: &JsString,
    ) -> Result<(), Error> {
        let host = self.program()?.host.clone();
        let view = self.ast(location)?;
        let in_import_call = ts_ast::utilities::find_ancestor(view, Some(location), |node| {
            is_import_call(view, node).unwrap_or(false)
        })?
        .is_some();
        let import_equals = ts_ast::utilities::find_ancestor(view, Some(location), |node| {
            node.kind() == K::ImportEqualsDeclaration
        })?;
        let sync = host.get_default_resolution_mode_for_file(importing_file_name.as_bytes())?
            == ModuleKind::COMMON_JS
            && !in_import_call
            || import_equals.is_some();
        let override_host = ts_ast::utilities::find_ancestor(view, Some(location), |node| {
            ts_ast::utilities_middle::is_resolution_mode_override_host(Some(node))
        })?;
        let has_override = override_host
            .map(|host| self.has_resolution_mode_override(host))
            .transpose()?
            .unwrap_or(false);
        if !(sync
            && host.get_default_resolution_mode_for_file(target_file_name.as_bytes())?
                == ModuleKind::ESNEXT
            && !has_override)
        {
            return Ok(());
        }
        if import_equals.is_some() {
            // ImportEquals in an ESM file resolving to another ESM file
            self.error_at(
                Some(error_node),
                d::Module_0_cannot_be_imported_using_this_construct_The_specifier_only_resolves_to_an_ES_module_which_cannot_be_imported_with_require_Use_an_ECMAScript_import_instead,
                vec![module_reference.clone()],
            )?;
            return Ok(());
        }
        // CJS file resolving to an ESM file
        let details = match try_get_extension_from_path(importing_file_name.as_bytes()) {
            Some(b".ts" | b".js" | b".tsx" | b".jsx") => {
                Some(self.mode_mismatch_details(importing_source, error_node)?)
            }
            _ => None,
        };
        let mut message = d::The_current_file_is_a_CommonJS_module_whose_imports_will_produce_require_calls_however_the_referenced_file_is_an_ECMAScript_module_and_cannot_be_imported_with_require_Consider_writing_a_dynamic_import_0_call_instead;
        if let Some(host_node) = override_host {
            let read = self.ast(host_node)?.node(host_node)?;
            match read.kind().known() {
                Some(K::ImportDeclaration) => {
                    let type_only = read
                        .data_source()
                        .as_import_declaration()
                        .and_then(|data| data.import_clause())
                        .map(|clause| {
                            Ok::<_, Error>(self.ast(clause)?.node(clause)?.is_type_only())
                        })
                        .transpose()?
                        .unwrap_or(false);
                    if type_only {
                        message = d::Type_only_import_of_an_ECMAScript_module_from_a_CommonJS_module_must_have_a_resolution_mode_attribute;
                    }
                }
                Some(K::ImportType) => {
                    message = d::Type_import_of_an_ECMAScript_module_from_a_CommonJS_module_must_have_a_resolution_mode_attribute;
                }
                _ => {}
            }
        }
        let args = vec![module_reference.clone()];
        let diagnostic = if let Some(details) = details {
            ts_ast::Diagnostic::chain(Some(std::sync::Arc::new(details)), message, args)
        } else {
            self.diagnostic_for_node(Some(error_node), message, args)?
        };
        self.add_diagnostic(diagnostic)?;
        Ok(())
    }

    // port: tsc/internal/ast/utilities.go:HasResolutionModeOverride
    fn has_resolution_mode_override(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.ast(node)?.node(node)?;
        let attributes = match read.kind().known() {
            Some(
                K::ImportType
                | K::ImportDeclaration
                | K::JSImportDeclaration
                | K::ExportDeclaration,
            ) => self.import_attributes(node)?,
            _ => None,
        };
        let (mode, _) =
            ts_ast::utilities_middle::import_attributes_resolution_mode_with_invalid_value(
                self.ast(node)?,
                attributes,
            )?;
        Ok(mode.is_some())
    }

    /// The repopulate marker Go attaches for incremental builds has no Rust
    /// counterpart; the message and arguments are the observable payload.
    // port: tsc/internal/checker/checker.go:Checker.createModeMismatchDetails
    // port: tsc/internal/checker/utilities.go:CreateModeMismatchDetails
    fn mode_mismatch_details(
        &mut self,
        source: NodeId,
        error_node: NodeId,
    ) -> Result<ts_ast::Diagnostic, Error> {
        let file_name = self
            .ast(source)?
            .source_file(source)?
            .parse_options()
            .file_name
            .clone();
        let target_extension: &[u8] = match try_get_extension_from_path(file_name.as_bytes()) {
            Some(b".ts") => b".mts",
            Some(b".js") => b".mjs",
            _ => b"",
        };
        let path = self
            .ast(source)?
            .source_file(source)?
            .parse_options()
            .path
            .clone();
        let meta = self
            .program()?
            .host
            .get_source_file_meta_data(path.as_bytes())?;
        let package_json_type = meta.package_json_type.clone();
        let package_json_directory = meta.package_json_directory.clone();
        let package_json = || {
            JsString::from_bytes(
                path::combine(
                    package_json_directory.as_bytes(),
                    &[b"package.json".as_slice()],
                )
                .as_slice(),
            )
        };
        let (message, args): (&'static Message, Vec<JsString>) = if package_json_directory
            .is_empty()
            || !package_json_type.is_empty()
        {
            if target_extension.is_empty() {
                (
                        d::To_convert_this_file_to_an_ECMAScript_module_create_a_local_package_json_file_with_type_Colon_module,
                        vec![],
                    )
            } else {
                (
                        d::To_convert_this_file_to_an_ECMAScript_module_change_its_file_extension_to_0_or_create_a_local_package_json_file_with_type_Colon_module,
                        vec![JsString::from_bytes(target_extension)],
                    )
            }
        } else if target_extension.is_empty() {
            (
                    d::To_convert_this_file_to_an_ECMAScript_module_add_the_field_type_Colon_module_to_0,
                    vec![package_json()],
                )
        } else {
            (
                    d::To_convert_this_file_to_an_ECMAScript_module_change_its_file_extension_to_0_or_add_the_field_type_Colon_module_to_1,
                    vec![JsString::from_bytes(target_extension), package_json()],
                )
        };
        self.diagnostic_for_node(Some(error_node), message, args)
    }
}

// port: tsc/internal/ast/utilities.go:IsImportCall
pub(crate) fn is_import_call(
    view: ts_ast::AstView<'_>,
    node: &ts_ast::NodeRead<'_>,
) -> Result<bool, Error> {
    if node.kind() != K::CallExpression {
        return Ok(false);
    }
    let Some(expression) = node.expression() else {
        return Ok(false);
    };
    let read = view.node(expression)?;
    if read.kind() == K::ImportKeyword {
        return Ok(true);
    }
    let Some(meta) = read.data_source().as_meta_property() else {
        return Ok(false);
    };
    Ok(meta.keyword_token() == K::ImportKeyword
        && meta
            .name()
            .map(|name| Ok::<_, Error>(view.node_text(name)?.as_bytes() == b"defer"))
            .transpose()?
            .unwrap_or(false))
}

// port: tsc/internal/tspath/extension.go:TryGetExtensionFromPath
fn try_get_extension_from_path(path: &[u8]) -> Option<&'static [u8]> {
    const EXTENSIONS_TO_REMOVE: [&[u8]; 12] = [
        b".d.ts", b".d.mts", b".d.cts", b".mjs", b".mts", b".cjs", b".cts", b".ts", b".js",
        b".tsx", b".jsx", b".json",
    ];
    EXTENSIONS_TO_REMOVE
        .into_iter()
        .find(|extension| path.ends_with(extension))
}

impl CheckerState {
    /// The repopulate marker Go attaches for incremental builds has no Rust
    /// counterpart; the message and arguments are the observable payload.
    // port: tsc/internal/checker/checker.go:Checker.createModuleNotFoundChain
    // port: tsc/internal/checker/utilities.go:CreateModuleNotFoundChain
    fn module_not_found_chain(
        &mut self,
        error_node: NodeId,
        resolved: &ts_module::ResolvedModule,
        module_reference: &JsString,
    ) -> Result<ts_ast::Diagnostic, Error> {
        let mut package_name = resolved.package_id.name.as_bytes().to_vec();
        let (message, args): (&'static Message, Vec<JsString>) = if resolved
            .alternate_result
            .is_empty()
        {
            let host = self.program()?.host.clone();
            let types_name = ts_module::get_types_package_name(&package_name);
            let mangled = JsString::from_bytes(
                ts_module::mangle_scoped_package_name(&package_name).as_slice(),
            );
            if host.package_bundles_types(&types_name)?.is_some() {
                (
                    d::If_the_0_package_actually_exposes_this_module_consider_sending_a_pull_request_to_amend_https_Colon_Slash_Slashgithub_com_SlashDefinitelyTyped_SlashDefinitelyTyped_Slashtree_Slashmaster_Slashtypes_Slash_1,
                    vec![JsString::from_bytes(package_name.as_slice()), mangled],
                )
            } else if host.package_bundles_types(&package_name)? == Some(true) {
                (
                    d::If_the_0_package_actually_exposes_this_module_try_adding_a_new_declaration_d_ts_file_containing_declare_module_1,
                    vec![
                        JsString::from_bytes(package_name.as_slice()),
                        module_reference.clone(),
                    ],
                )
            } else {
                (
                    d::Try_npm_i_save_dev_types_Slash_1_if_it_exists_or_add_a_new_declaration_d_ts_file_containing_declare_module_0,
                    vec![module_reference.clone(), mangled],
                )
            }
        } else {
            if resolved
                .alternate_result
                .as_bytes()
                .windows(b"/node_modules/@types/".len())
                .any(|window| window == b"/node_modules/@types/")
            {
                package_name = ts_module::get_types_package_name(&package_name);
            }
            (
                d::There_are_types_at_0_but_this_result_could_not_be_resolved_when_respecting_package_json_exports_The_1_library_may_need_to_update_its_package_json_or_typings,
                vec![
                    resolved.alternate_result.clone(),
                    JsString::from_bytes(package_name.as_slice()),
                ],
            )
        };
        self.diagnostic_for_node(Some(error_node), message, args)
    }
}

// port: tsc/internal/ast/utilities.go:IsTypeOnlyImportOrExportDeclaration
pub(crate) fn type_only_import_or_export_declaration(
    view: ts_ast::AstView<'_>,
    node: &ts_ast::NodeRead<'_>,
) -> Result<bool, Error> {
    let parent_type_only = |depth: usize| -> Result<bool, Error> {
        let mut current = node.parent();
        for _ in 1..depth {
            current = match current {
                Some(id) => view.node(id)?.parent(),
                None => None,
            };
        }
        Ok(match current {
            Some(id) => view.node(id)?.is_type_only(),
            None => false,
        })
    };
    Ok(match node.kind().known() {
        Some(K::ImportSpecifier | K::ExportSpecifier) => {
            node.is_type_only() || parent_type_only(2)?
        }
        Some(K::NamespaceImport | K::NamespaceExport) => parent_type_only(1)?,
        Some(K::ImportClause | K::ImportEqualsDeclaration) => node.is_type_only(),
        Some(K::ExportDeclaration) => {
            let data = node
                .data_source()
                .as_export_declaration()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            data.is_type_only()
                && data.module_specifier().is_some()
                && data.export_clause().is_none()
        }
        _ => false,
    })
}

// port: tsc/internal/tspath/path.go:PathIsRelative
fn specifier_is_relative(path: &[u8]) -> bool {
    path == b"."
        || path == b".."
        || path.len() >= 2 && path[0] == b'.' && matches!(path[1], b'/' | b'\\')
        || path.len() >= 3 && path[0] == b'.' && path[1] == b'.' && matches!(path[2], b'/' | b'\\')
}

// port: tsc/internal/tspath/extension.go:HasTSFileExtension
fn has_ts_file_extension(path: &[u8]) -> bool {
    TS_EXTENSIONS
        .iter()
        .any(|extension| path.ends_with(extension))
}

// port: tsc/internal/tspath/path.go:GetAnyExtensionFromPath
fn any_extension(path: &[u8]) -> &[u8] {
    let base = path::base_name(path);
    match base.iter().rposition(|&byte| byte == b'.') {
        Some(index) => &base[index..],
        None => b"",
    }
}
