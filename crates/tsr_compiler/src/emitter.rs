//! `compiler/emitter.go`: the chain of script transformers the emitter runs
//! over one file, and the per-file emitter that transforms, prints and writes
//! its JavaScript, declaration and source-map outputs.
//!
//! The declaration transformers (`declarations.NewDeclarationTransformer` and
//! `NewSupplementalReferencesTransformer`) are not wired yet: a file with a
//! declaration output fails with `Error::Unsupported("declaration emit")`.
use crate::emit_host::EmitHost;
use crate::program_diagnostics::source_names;
use crate::program_emit::{
    push_emit_trace, EmitOnly, EmitResult, SourceMapEmitResult, WriteFile, WriteFileData,
};
use crate::{messages, Error, Program, ProgramFile, ProgramResolverHost};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::NodeId;
use tsr_ast::diagnostic_api::DiagnosticsCollection;
use tsr_ast::{AstBuilder, AstView, Diagnostic, JsString};
use tsr_binder::name_resolver::{ResolverHost, ResolverOptions};
use tsr_binder::reference_resolver::{
    NoReferenceResolverHooks, ReferenceResolver as BinderResolver,
};
use tsr_core::{CompilerOptions, LanguageVariant, ModuleKind, NewLineKind, Tristate};
use tsr_jsstring::line_map::compute_ecma_line_starts;
use tsr_jsstring::text::{add_utf8_byte_order_mark, encode_uri};
use tsr_jsstring::SourceText;
use tsr_printer::script_resolver::{ReferenceResolver, ResolverResult};
use tsr_printer::EmitContext;
use tsr_printer::{EmitTextWriter, Printer, PrinterOptions, SourceMapSource, TextWriter};
use tsr_transformers::{
    estransforms, inliners, jsxtransforms, moduletransforms, tstransforms, Failure,
    SharedEmitResolver, SharedReferenceResolver, TransformOptions, Transformer,
};
use tsr_tsoptions::output_paths::{get_source_file_path_in_new_dir, OutputPaths};
use tsr_tspath as path;

/// `binder.NewReferenceResolver(options, binder.ReferenceResolverHooks{})`
/// over the program's bound files: the reference resolver of a file whose
/// transforms ask the checker nothing.
struct BoundReferenceResolver<'a> {
    host: ProgramResolverHost<'a>,
    resolver: BinderResolver,
}

impl BoundReferenceResolver<'_> {
    /// Whether `node` belongs to storage the program does not retain: a node
    /// of a transform's factory. Upstream resolves such a node's name from
    /// the node itself; it has no parent, so no scope holds the name and the
    /// identifier queries below answer nil without reading it.
    fn is_transform_node(&self, node: NodeId) -> bool {
        matches!(self.host.ast(node), Err(tsr_arena::Error::WrongOwner))
    }
}

impl ReferenceResolver for BoundReferenceResolver<'_> {
    fn get_referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>> {
        if self.is_transform_node(node) {
            return Ok(None);
        }
        Ok(self.resolver.get_referenced_export_container(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
            prefix_locals,
        )?)
    }
    fn get_referenced_import_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        if self.is_transform_node(node) {
            return Ok(None);
        }
        Ok(self.resolver.get_referenced_import_declaration(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_referenced_value_declaration(&mut self, node: NodeId) -> ResolverResult<Option<NodeId>> {
        if self.is_transform_node(node) {
            return Ok(None);
        }
        Ok(self.resolver.get_referenced_value_declaration(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_referenced_value_declarations(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<Vec<NodeId>>> {
        if self.is_transform_node(node) {
            return Ok(None);
        }
        Ok(self.resolver.get_referenced_value_declarations(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_element_access_expression_name(
        &mut self,
        expression: NodeId,
    ) -> ResolverResult<JsString> {
        Ok(self
            .resolver
            .get_element_access_expression_name(&mut NoReferenceResolverHooks, Some(expression))?)
    }
    fn get_referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        Ok(self.resolver.get_referenced_member_value_declaration(
            &self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
}

impl Program {
    /// `Program.GetEmitModuleFormatOfFile` by file name.
    pub(crate) fn emit_module_format_of_file_name(
        &self,
        file_name: &[u8],
    ) -> Result<ModuleKind, tsr_arena::Error> {
        let file = self
            .source_file(file_name)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        let source = file.bound().view().source_file()?;
        let options = source.parse_options();
        let meta = self
            .metadata(options.path.as_bytes())
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        Ok(crate::metadata::emit_format(
            options.file_name.as_bytes(),
            self.options_for_file(options.path.as_bytes(), options.file_name.as_bytes()),
            meta,
        ))
    }
}

// port: tsc/internal/compiler/emitter.go:getModuleTransformer
fn get_module_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    match opts.compiler_options.emit_module_kind() {
        // `ESModuleTransformer` contains logic for preserving CJS input syntax in `--module preserve`
        ModuleKind::PRESERVE => moduletransforms::esmodule::new_es_module_transformer(opts),

        ModuleKind::ESNEXT
        | ModuleKind::ES2022
        | ModuleKind::ES2020
        | ModuleKind::ES2015
        | ModuleKind::NODE20
        | ModuleKind::NODE18
        | ModuleKind::NODE16
        | ModuleKind::NODE_NEXT
        | ModuleKind::COMMON_JS => {
            moduletransforms::impliedmodule::new_implied_module_transformer(opts)
        }

        _ => moduletransforms::commonjsmodule::new_common_js_module_transformer(opts),
    }
}

/// The options `getScriptTransformers` builds for `source_file`, a file of
/// `program`: the emit resolver is the reference resolver unless the file's
/// transforms need no checker, in which case references resolve from the
/// bound files alone.
pub fn script_transform_options<'a>(
    emit_context: &EmitContext,
    program: &'a Program,
    emit_resolver: SharedEmitResolver<'a>,
    source_file: NodeId,
    counters: &tsr_arena::Counters,
    failure: Failure,
) -> Result<TransformOptions<'a>, tsr_arena::Error> {
    let options = program.options();
    let (in_js_file, language_variant) = source_file_facts(program, source_file)?;

    // JS files don't use reference calculations as they don't do import elision, no need to calculate it
    let import_elision_enabled = !options.verbatim_module_syntax.is_true() && !in_js_file;
    let jsx_transform_enabled =
        options.jsx_transform_enabled() && language_variant == LanguageVariant::JSX;

    let resolver: SharedReferenceResolver<'a> = if import_elision_enabled
        || jsx_transform_enabled
        || !options.isolated_modules()
        || options.emit_decorator_metadata.is_true()
    {
        emit_resolver.clone()
    } else {
        Rc::new(RefCell::new(BoundReferenceResolver {
            host: program.resolver_host(counters),
            resolver: BinderResolver::new(ResolverOptions {
                emit_script_target: options.emit_script_target(),
                isolated_modules: options.isolated_modules(),
                verbatim_module_syntax: options.verbatim_module_syntax.is_true(),
                emit_standard_class_fields: options.emit_standard_class_fields(),
            }),
        }))
    };

    Ok(TransformOptions {
        context: emit_context.clone(),
        compiler_options: Arc::new(options.clone()),
        resolver,
        emit_resolver,
        get_emit_module_format_of_file: Rc::new(move |file_name: &[u8]| {
            Ok(program.emit_module_format_of_file_name(file_name)?)
        }),
        failure,
    })
}

/// Whether `source_file` is a JavaScript file, and its language variant.
fn source_file_facts(
    program: &Program,
    source_file: NodeId,
) -> Result<(bool, LanguageVariant), tsr_arena::Error> {
    let file = program
        .files()
        .iter()
        .find(|file| file.source() == source_file)
        .ok_or(tsr_arena::Error::WrongOwner)?;
    let view = file.bound().view().ast();
    let node = view.node(source_file)?;
    let state = file.bound().view().source_file()?;
    Ok((
        tsr_ast::utilities::is_in_js_file(Some(&node)),
        state.language_variant,
    ))
}

/// The transformers the emitter runs over `source_file`, in order.
// port: tsc/internal/compiler/emitter.go:getScriptTransformers
pub fn get_script_transformers<'a>(
    program: &Program,
    opts: &TransformOptions<'a>,
    source_file: NodeId,
) -> Result<Vec<Transformer<'a>>, tsr_arena::Error> {
    let mut tx = Vec::new();
    let options: &CompilerOptions = &opts.compiler_options;
    let (in_js_file, language_variant) = source_file_facts(program, source_file)?;

    // JS files don't use reference calculations as they don't do import elision, no need to calculate it
    let import_elision_enabled = !options.verbatim_module_syntax.is_true() && !in_js_file;
    let jsx_transform_enabled =
        options.jsx_transform_enabled() && language_variant == LanguageVariant::JSX;

    // transform TypeScript syntax
    {
        // use type nodes to add metadata decorators
        if options.emit_decorator_metadata.is_true() {
            tx.extend(tstransforms::metadata::new_metadata_transformer(opts));
        }

        // erase types
        tx.extend(tstransforms::typeeraser::new_type_eraser_transformer(opts));

        // elide imports
        if import_elision_enabled {
            tx.extend(tstransforms::importelision::new_import_elision_transformer(
                opts,
            ));
        }

        // transform `enum`, `namespace`, and parameter properties
        tx.extend(tstransforms::runtimesyntax::new_runtime_syntax_transformer(
            opts,
        ));

        if options.experimental_decorators.is_true() {
            tx.extend(tstransforms::legacydecorators::new_legacy_decorators_transformer(opts));
        }
    }

    if jsx_transform_enabled {
        tx.extend(jsxtransforms::jsx::new_jsx_transformer(opts));
    }

    tx.extend(estransforms::get_es_transformer(opts));

    tx.extend(estransforms::usestrict::new_use_strict_transformer(opts));

    // transform module syntax
    tx.extend(get_module_transformer(opts));

    // inlining (formerly done via substitutions)
    if !options.isolated_modules() {
        tx.extend(inliners::constenum::new_const_enum_inlining_transformer(
            opts,
        ));
    }
    Ok(tx)
}

/// A transformer by the name the native transform probe gives it
/// (`tools/phase3/probe`), for tests that run one transformer or a chain.
pub fn transformer_by_name<'a>(
    name: &str,
    opts: &TransformOptions<'a>,
) -> Option<Option<Transformer<'a>>> {
    use estransforms as es;
    Some(match name {
        "metadata" => tstransforms::metadata::new_metadata_transformer(opts),
        "typeeraser" => tstransforms::typeeraser::new_type_eraser_transformer(opts),
        "importelision" => tstransforms::importelision::new_import_elision_transformer(opts),
        "runtimesyntax" => tstransforms::runtimesyntax::new_runtime_syntax_transformer(opts),
        "legacydecorators" => {
            tstransforms::legacydecorators::new_legacy_decorators_transformer(opts)
        }
        "jsx" => jsxtransforms::jsx::new_jsx_transformer(opts),
        "es" => es::get_es_transformer(opts),
        "usestrict" => es::usestrict::new_use_strict_transformer(opts),
        "module" => get_module_transformer(opts),
        "commonjs" => moduletransforms::commonjsmodule::new_common_js_module_transformer(opts),
        "esmodule" => moduletransforms::esmodule::new_es_module_transformer(opts),
        "impliedmodule" => moduletransforms::impliedmodule::new_implied_module_transformer(opts),
        "constenum" => inliners::constenum::new_const_enum_inlining_transformer(opts),
        "using" => es::using::new_using_declaration_transformer(opts),
        "esdecorator" => es::esdecorator::new_es_decorator_transformer(opts),
        "classfields" => es::classfields::new_class_fields_transformer(opts),
        "logicalassignment" => es::logicalassignment::new_logical_assignment_transformer(opts),
        "nullishcoalescing" => es::nullishcoalescing::new_nullish_coalescing_transformer(opts),
        "optionalchain" => es::optionalchain::new_optional_chain_transformer(opts),
        "optionalcatch" => es::optionalcatch::new_optional_catch_transformer(opts),
        "objectrestspread" => es::objectrestspread::new_object_rest_spread_transformer(opts),
        "forawait" => es::forawait::new_for_await_transformer(opts),
        "taggedtemplate" => {
            es::taggedtemplate::new_tagged_template_lift_restriction_transformer(opts)
        }
        "async" => es::async_::new_async_transformer(opts),
        "exponentiation" => es::exponentiation::new_exponentiation_transformer(opts),
        _ => return None,
    })
}

/// The transformers `emitter.runScriptTransformers` runs over one file:
/// [`get_script_transformers`] for `Program.Emit`.
pub type ScriptTransformers = dyn for<'t> Fn(
        &Program,
        &TransformOptions<'t>,
        NodeId,
    ) -> Result<Vec<Transformer<'t>>, tsr_arena::Error>
    + Sync;

/// A transformation's failure as the program's: a transformer the port does
/// not have yet is `Unsupported` by its upstream name.
fn transform_error(error: tsr_transformers::Error) -> Error {
    match error {
        tsr_transformers::Error::Unsupported(name) => Error::Unsupported(name),
        tsr_transformers::Error::Arena(error) => Error::Ast(error),
        error => Error::Transform(error),
    }
}

fn printer_error(error: tsr_printer::Error) -> Error {
    match error {
        tsr_printer::Error::Unsupported(name) => Error::Unsupported(name),
        tsr_printer::Error::Arena(error) => Error::Ast(error),
        error => Error::Printer(error),
    }
}

/// The emitter's `ast.DiagnosticsCollection`: an equal diagnostic is kept
/// once, and the diagnostics are read sorted.
#[derive(Default)]
pub(crate) struct EmitterDiagnostics {
    added: Vec<Arc<Diagnostic>>,
    collection: DiagnosticsCollection,
}

impl EmitterDiagnostics {
    fn add(&mut self, program: &Program, diagnostic: Diagnostic) -> Result<(), Error> {
        let file_name = |id| source_names(program, id).map(|(name, _)| name);
        for existing in &self.added {
            if tsr_ast::equal_diagnostics(existing, &diagnostic, &file_name)? {
                return Ok(());
            }
        }
        let path = match diagnostic.file {
            Some(file) => Some(source_names(program, file)?.1.to_vec()),
            None => None,
        };
        let diagnostic = Arc::new(diagnostic);
        self.added.push(diagnostic.clone());
        self.collection.add(diagnostic, path.as_deref());
        Ok(())
    }

    fn get_diagnostics(&self, program: &Program) -> Result<Vec<Diagnostic>, Error> {
        let file_name = |id| source_names(program, id).map(|(name, _)| name);
        Ok(self
            .collection
            .get_diagnostics(&file_name)?
            .iter()
            .map(|diagnostic| (**diagnostic).clone())
            .collect())
    }
}

/// `emitter`: one file's emit. The writer is the one `Program.Emit` takes
/// from its pool for the file.
#[allow(
    clippy::struct_field_names,
    reason = "the fields keep the names of Go's emitter fields"
)]
pub(crate) struct Emitter<'e, 'a> {
    pub host: &'e EmitHost<'a>,
    pub emit_only: EmitOnly,
    pub emitter_diagnostics: EmitterDiagnostics,
    pub writer: &'e mut TextWriter,
    pub paths: OutputPaths,
    pub source_file: &'e ProgramFile,
    pub emit_result: EmitResult,
    pub force_emit: bool,
    pub write_file: Option<&'e WriteFile<'e>>,
    pub tr: Option<&'e Arc<dyn tsr_checker::TraceSink>>,
    pub script_transformers: &'e ScriptTransformers,
}

impl Emitter<'_, '_> {
    // port: tsc/internal/compiler/emitter.go:emitter.emit
    pub(crate) fn emit(&mut self) -> Result<(), Error> {
        let path = self
            .source_file
            .bound()
            .view()
            .source_file()?
            .path()
            .to_vec();
        let _span = push_emit_trace(self.tr, "emit", Some(("path", &path)), true);
        let paths = self.paths.clone();
        self.emit_js_file(
            self.source_file,
            paths.js_file_path(),
            paths.source_map_file_path(),
        )?;
        self.emit_declaration_file(
            self.source_file,
            paths.declaration_file_path(),
            paths.declaration_map_path(),
        )?;
        self.emit_result.diagnostics = self
            .emitter_diagnostics
            .get_diagnostics(self.host.program())?;
        Ok(())
    }

    // port: tsc/internal/compiler/emitter.go:emitter.runScriptTransformers
    fn run_script_transformers(
        &self,
        emit_context: &EmitContext,
        output: &mut AstBuilder,
        counters: &tsr_arena::Counters,
        source_file: &ProgramFile,
    ) -> Result<NodeId, Error> {
        let path = source_file.bound().view().source_file()?.path().to_vec();
        let _span = push_emit_trace(self.tr, "transformNodes", Some(("path", &path)), false);
        let program = self.host.program();
        let failure = Failure::default();
        let opts = script_transform_options(
            emit_context,
            program,
            self.host.get_emit_resolver(),
            source_file.source(),
            counters,
            failure,
        )?;
        let mut file = source_file.source();
        for transformer in (self.script_transformers)(program, &opts, file)? {
            file = transformer
                .transform_source_file(output, file)
                .map_err(transform_error)?;
        }
        Ok(file)
    }

    /// The declaration transform and the supplemental references transform
    /// over `source_file`, with their diagnostics. Not wired yet.
    // TODO(transformers/declarations/transform.go): NewDeclarationTransformer
    // TODO(transformers/declarations/supplementalreferences.go): NewSupplementalReferencesTransformer
    #[allow(clippy::unused_self)]
    fn run_declaration_transformers(
        &self,
        _emit_context: &EmitContext,
        _output: &mut AstBuilder,
        _source_file: &ProgramFile,
        _declaration_file_path: &[u8],
        _declaration_map_path: &[u8],
    ) -> Result<(NodeId, Vec<Diagnostic>), Error> {
        Err(Error::Unsupported("declaration emit"))
    }

    // port: tsc/internal/compiler/emitter.go:emitter.emitJSFile
    fn emit_js_file(
        &mut self,
        source_file: &ProgramFile,
        js_file_path: &[u8],
        source_map_file_path: &[u8],
    ) -> Result<(), Error> {
        let options = self.host.options();

        if (self.emit_only != EmitOnly::All && self.emit_only != EmitOnly::Js)
            || js_file_path.is_empty()
        {
            return Ok(());
        }

        if !self.force_emit
            && (options.no_emit == Tristate::TRUE || self.host.is_emit_blocked(js_file_path))
        {
            self.emit_result.emit_skipped = true;
            return Ok(());
        }

        let _span = push_emit_trace(
            self.tr,
            "emitJsFileOrBundle",
            Some(("jsFilePath", js_file_path)),
            true,
        );

        let emit_context = EmitContext::new();
        let counters = tsr_arena::Counters::new();
        let mut output = AstBuilder::with_hooks(
            SourceText::default(),
            &counters,
            emit_context.factory_hooks(),
        );
        // A transform may read any file its resolver answers with.
        for file in self.host.source_files() {
            output.retain_completed(file.bound());
        }

        let transformed =
            self.run_script_transformers(&emit_context, &mut output, &counters, source_file)?;

        let printer_options = PrinterOptions {
            remove_comments: options.remove_comments.is_true(),
            new_line: options.new_line,
            no_emit_helpers: options.no_emit_helpers.is_true(),
            source_map: options.source_map.is_true(),
            inline_source_map: options.inline_source_map.is_true(),
            inline_sources: options.inline_sources.is_true(),
            target: options.target,
            ..PrinterOptions::default()
        };

        // Generated names for namespaces and enums read the file's binder
        // state, as the pin reads it from the bound nodes.
        let bindings = source_file.bound().view();
        // create a printer to print the nodes
        let mut printer = Printer::new(printer_options, &emit_context);
        printer.bindings = Some(&bindings);

        let source = bindings.source_file()?;
        let file_name = source.file_name();
        self.print_source_file(
            js_file_path,
            source_map_file_path,
            output.view(),
            transformed,
            file_name,
            &printer,
            options,
            should_emit_source_maps(options, file_name),
        )
    }

    // port: tsc/internal/compiler/emitter.go:emitter.emitDeclarationFile
    fn emit_declaration_file(
        &mut self,
        source_file: &ProgramFile,
        declaration_file_path: &[u8],
        declaration_map_path: &[u8],
    ) -> Result<(), Error> {
        let options = self.host.options();

        if self.emit_only == EmitOnly::Js || declaration_file_path.is_empty() {
            return Ok(());
        }
        let emit_declaration_map =
            self.emit_only != EmitOnly::BuilderSignature && options.declaration_map.is_true();
        let content_mapped_source = source_file.bound().view().source_file()?;

        let _span = push_emit_trace(
            self.tr,
            "emitDeclarationFileOrBundle",
            Some(("declarationFilePath", declaration_file_path)),
            true,
        );

        let emit_context = EmitContext::new();
        let counters = tsr_arena::Counters::new();
        let mut output = AstBuilder::with_hooks(
            SourceText::default(),
            &counters,
            emit_context.factory_hooks(),
        );
        output.retain_completed(source_file.bound());
        let (transformed, diags) = self.run_declaration_transformers(
            &emit_context,
            &mut output,
            source_file,
            declaration_file_path,
            declaration_map_path,
        )?;

        for elem in &diags {
            // Add declaration transform diagnostics to emit diagnostics
            self.emitter_diagnostics
                .add(self.host.program(), elem.clone())?;
        }

        if !self.force_emit
            && self.emit_only != EmitOnly::BuilderSignature
            && (options.no_emit == Tristate::TRUE
                || self.host.is_emit_blocked(declaration_file_path))
        {
            self.emit_result.emit_skipped = true;
            return Ok(());
        }

        let decl_blocked =
            !diags.is_empty() && !self.force_emit && self.emit_only != EmitOnly::BuilderSignature;
        if decl_blocked {
            self.emit_result.emit_skipped = true;
            return Ok(());
        }

        let printer_options = PrinterOptions {
            remove_comments: options.remove_comments.is_true(),
            new_line: options.new_line,
            no_emit_helpers: true,
            target: options.emit_script_target(),
            source_map: emit_declaration_map,
            inline_source_map: options.inline_source_map.is_true(),
            only_print_js_doc_style: true,
            omit_brace_source_map_positions: true,
            ..PrinterOptions::default()
        };

        // create a printer to print the nodes
        let mut printer = Printer::new(printer_options, &emit_context);
        if let Some(span_map) = content_mapped_source
            .span_map()
            .filter(|_| emit_declaration_map)
        {
            let original_source: Rc<dyn tsr_sourcemap::Source> =
                Rc::new(new_declaration_map_source(&content_mapped_source)?);
            let content_mapped_file_name = content_mapped_source.file_name();
            printer.map_source_position = Some(Box::new(move |source, text, pos| {
                if text.file_name() != content_mapped_file_name {
                    return Some((source.clone(), pos));
                }
                let (mapped, ok) = span_map.virtual_to_original_position_exact(
                    i32::try_from(pos).expect("a source position fits a text position"),
                );
                if !ok {
                    return None;
                }
                Some((
                    SourceMapSource::Mapped(original_source.clone()),
                    i64::from(mapped),
                ))
            }));
        }

        let declaration_map_options = CompilerOptions {
            source_map: if emit_declaration_map {
                Tristate::TRUE
            } else {
                Tristate::FALSE
            },
            source_root: options.source_root.clone(),
            map_root: options.map_root.clone(),
            // Explicitly do not pass through either inline option.
            ..CompilerOptions::default()
        };
        let file_name = content_mapped_source.file_name();
        self.print_source_file(
            declaration_file_path,
            declaration_map_path,
            output.view(),
            transformed,
            file_name,
            &printer,
            &declaration_map_options,
            should_emit_source_maps(&declaration_map_options, file_name),
        )
    }

    // port: tsc/internal/compiler/emitter.go:emitter.printSourceFile
    #[allow(clippy::too_many_arguments)]
    fn print_source_file(
        &mut self,
        js_file_path: &[u8],
        source_map_file_path: &[u8],
        view: AstView<'_>,
        source_file: NodeId,
        source_file_name: &[u8],
        printer: &Printer<'_>,
        map_options: &CompilerOptions,
        should_emit_source_maps: bool,
    ) -> Result<(), Error> {
        let options = self.host.options();
        let mut source_map_generator = None;
        if should_emit_source_maps {
            source_map_generator = Some(tsr_sourcemap::new_generator(
                JsString::from_bytes(path::base_name(&path::normalize_slashes(js_file_path))),
                JsString::from_bytes(get_source_root(map_options)),
                JsString::from_bytes(self.get_source_map_directory(
                    map_options,
                    js_file_path,
                    Some(source_file_name),
                )),
                JsString::from_bytes(self.host.get_current_directory()),
                self.host.use_case_sensitive_file_names(),
            ));
        }

        printer
            .write(
                view,
                source_file,
                Some(source_file),
                &mut *self.writer,
                source_map_generator.as_mut(),
            )
            .map_err(printer_error)?;

        let mut source_map_url_pos: isize = -1;
        if let Some(source_map_generator) = source_map_generator.as_mut() {
            if map_options.source_map.is_true() || map_options.inline_source_map.is_true() {
                self.emit_result.source_maps.push(SourceMapEmitResult {
                    input_source_file_names: source_map_generator.sources().to_vec(),
                    source_map: source_map_generator.raw_source_map(),
                    generated_file: JsString::from_bytes(js_file_path),
                });
            }

            let source_mapping_url = self.get_source_mapping_url(
                map_options,
                source_map_generator,
                js_file_path,
                source_map_file_path,
                Some(source_file_name),
            );

            if !source_mapping_url.is_empty() {
                if !self.writer.is_at_start_of_line() {
                    self.writer
                        .raw_write(if options.new_line == NewLineKind::CRLF {
                            b"\r\n"
                        } else {
                            b"\n"
                        });
                }
                source_map_url_pos = isize::try_from(self.writer.get_text_pos())
                    .expect("a text position fits a Go int");
                self.writer.write_comment(b"//# sourceMappingURL=");
                self.writer.write_comment(&source_mapping_url);
            }

            // Write the source map
            if !source_map_file_path.is_empty() {
                let source_map = source_map_generator.string();
                let err = self.write_text(
                    source_map_file_path,
                    source_map.as_bytes(),
                    &mut WriteFileData {
                        source_file: Some(self.source_file.source()),
                        ..WriteFileData::default()
                    },
                );
                match err {
                    Err(err) => self.emitter_diagnostics.add(
                        self.host.program(),
                        Diagnostic::compiler(
                            messages::Could_not_write_file_0_Colon_1,
                            vec![
                                JsString::from_bytes(js_file_path),
                                JsString::from_bytes(err.to_string().into_bytes()),
                            ],
                        ),
                    )?,
                    Ok(()) => self
                        .emit_result
                        .emitted_files
                        .push(JsString::from_bytes(source_map_file_path)),
                }
            }
        } else {
            self.writer.write_line();
        }

        // Write the output file
        let mut text = self.writer.text().to_vec();
        if options.emit_bom.is_true() {
            text = add_utf8_byte_order_mark(&text).into_owned();
        }
        let mut data = WriteFileData {
            source_map_url_pos,
            diagnostics: self
                .emitter_diagnostics
                .get_diagnostics(self.host.program())?,
            source_file: Some(self.source_file.source()),
            ..WriteFileData::default()
        };
        let err = self.write_text(js_file_path, &text, &mut data);
        let skipped_dts_write = data.skipped_dts_write;
        if let Err(err) = err {
            self.emitter_diagnostics.add(
                self.host.program(),
                Diagnostic::compiler(
                    messages::Could_not_write_file_0_Colon_1,
                    vec![
                        JsString::from_bytes(js_file_path),
                        JsString::from_bytes(err.to_string().into_bytes()),
                    ],
                ),
            )?;
        } else if !skipped_dts_write {
            self.emit_result
                .emitted_files
                .push(JsString::from_bytes(js_file_path));
        }

        // Reset state
        self.writer.clear();
        Ok(())
    }

    // port: tsc/internal/compiler/emitter.go:emitter.writeText
    fn write_text(
        &self,
        file_name: &[u8],
        text: &[u8],
        data: &mut WriteFileData,
    ) -> Result<(), tsr_vfs::Error> {
        if let Some(write_file) = self.write_file {
            return write_file(file_name, text, data);
        }
        self.host.write_file(file_name, text)
    }

    // port: tsc/internal/compiler/emitter.go:emitter.getSourceMapDirectory
    fn get_source_map_directory(
        &self,
        map_options: &CompilerOptions,
        file_path: &[u8],
        source_file_name: Option<&[u8]>,
    ) -> Vec<u8> {
        if !map_options.source_root.is_empty() {
            return self.host.common_source_directory().to_vec();
        }
        if !map_options.map_root.is_empty() {
            let mut source_map_dir =
                path::normalize_slashes(map_options.map_root.as_bytes()).into_owned();
            if let Some(source_file_name) = source_file_name {
                // For modules or multiple emit files the mapRoot will have directory structure like the sources
                // So if src\a.ts and src\lib\b.ts are compiled together user would be moving the maps into mapRoot\a.js.map and mapRoot\lib\b.js.map
                source_map_dir = path::directory(&get_source_file_path_in_new_dir(
                    source_file_name,
                    &source_map_dir,
                    self.host.get_current_directory(),
                    self.host.common_source_directory(),
                    self.host.use_case_sensitive_file_names(),
                ));
            }
            if path::root_length(&source_map_dir) == 0 {
                // The relative paths are relative to the common directory
                source_map_dir =
                    path::combine(self.host.common_source_directory(), &[&source_map_dir]);
            }
            return source_map_dir;
        }
        path::directory(&path::normalize(file_path))
    }

    // port: tsc/internal/compiler/emitter.go:emitter.getSourceMappingURL
    fn get_source_mapping_url(
        &self,
        map_options: &CompilerOptions,
        source_map_generator: &mut tsr_sourcemap::Generator,
        file_path: &[u8],
        source_map_file_path: &[u8],
        source_file_name: Option<&[u8]>,
    ) -> Vec<u8> {
        if map_options.inline_source_map.is_true() {
            // Encode the sourceMap into the sourceMap url
            return source_map_generator.base64_data_url().as_bytes().to_vec();
        }

        let source_map_file =
            path::base_name(&path::normalize_slashes(source_map_file_path)).to_vec();
        if !map_options.map_root.is_empty() {
            let mut source_map_dir =
                path::normalize_slashes(map_options.map_root.as_bytes()).into_owned();
            if let Some(source_file_name) = source_file_name {
                // For modules or multiple emit files the mapRoot will have directory structure like the sources
                // So if src\a.ts and src\lib\b.ts are compiled together user would be moving the maps into mapRoot\a.js.map and mapRoot\lib\b.js.map
                source_map_dir = path::directory(&get_source_file_path_in_new_dir(
                    source_file_name,
                    &source_map_dir,
                    self.host.get_current_directory(),
                    self.host.common_source_directory(),
                    self.host.use_case_sensitive_file_names(),
                ));
            }
            if path::root_length(&source_map_dir) == 0 {
                // The relative paths are relative to the common directory
                source_map_dir =
                    path::combine(self.host.common_source_directory(), &[&source_map_dir]);
                return encode_uri(&path::relative_to_directory_or_url(
                    // get the relative sourceMapDir path based on jsFilePath
                    &path::directory(&path::normalize(file_path)),
                    // this is where user expects to see sourceMap
                    &path::combine(&source_map_dir, &[&source_map_file]),
                    /*isAbsolutePathAnUrl*/ true,
                    self.host.get_current_directory(),
                    self.host.use_case_sensitive_file_names(),
                ))
                .into_owned();
            }
            return encode_uri(&path::combine(&source_map_dir, &[&source_map_file])).into_owned();
        }
        encode_uri(&source_map_file).into_owned()
    }
}

/// `declarationMapSource`: the original text of a content-mapped file.
struct DeclarationMapSource {
    file_name: Vec<u8>,
    text: Vec<u8>,
    line_map: Vec<i32>,
}

// port: tsc/internal/compiler/emitter.go:newDeclarationMapSource
fn new_declaration_map_source(
    source_file: &tsr_ast::SourceFileRead<'_>,
) -> Result<DeclarationMapSource, Error> {
    let text = source_file.original_text().to_vec();
    Ok(DeclarationMapSource {
        file_name: source_file.original_file_name()?.as_bytes().to_vec(),
        line_map: compute_ecma_line_starts(&text),
        text,
    })
}

impl tsr_sourcemap::Source for DeclarationMapSource {
    // port: tsc/internal/compiler/emitter.go:declarationMapSource.FileName
    fn file_name(&self) -> &[u8] {
        &self.file_name
    }
    // port: tsc/internal/compiler/emitter.go:declarationMapSource.Text
    fn text(&self) -> &[u8] {
        &self.text
    }
    // port: tsc/internal/compiler/emitter.go:declarationMapSource.ECMALineMap
    fn ecma_line_map(&self) -> &[i32] {
        &self.line_map
    }
}

/// `sourceFile` by its file name.
// port: tsc/internal/compiler/emitter.go:shouldEmitSourceMaps
fn should_emit_source_maps(map_options: &CompilerOptions, source_file_name: &[u8]) -> bool {
    (map_options.source_map.is_true() || map_options.inline_source_map.is_true())
        && !path::file_extension_is(source_file_name, b".json")
}

// port: tsc/internal/compiler/emitter.go:getSourceRoot
fn get_source_root(map_options: &CompilerOptions) -> Vec<u8> {
    // Normalize source root and make sure it has trailing "/" so that it can be used to combine paths with the
    // relative paths of the sources list in the sourcemap
    let mut source_root = path::normalize_slashes(map_options.source_root.as_bytes()).into_owned();
    if !source_root.is_empty() {
        source_root = path::ensure_trailing_directory_separator(&source_root).into_owned();
    }
    source_root
}
