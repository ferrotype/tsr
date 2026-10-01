//! `compiler/emitter.go`. Ported so far: the chain of script transformers the
//! emitter runs over one file. The emitter itself is Phase 3 T8.
use crate::{Program, ProgramResolverHost};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::NodeId;
use tsr_ast::JsString;
use tsr_binder::name_resolver::ResolverOptions;
use tsr_binder::reference_resolver::{
    NoReferenceResolverHooks, ReferenceResolver as BinderResolver,
};
use tsr_core::{CompilerOptions, LanguageVariant, ModuleKind};
use tsr_printer::script_resolver::{ReferenceResolver, ResolverResult};
use tsr_printer::EmitContext;
use tsr_transformers::{
    estransforms, inliners, jsxtransforms, moduletransforms, tstransforms, Failure,
    SharedEmitResolver, SharedReferenceResolver, TransformOptions, Transformer,
};

/// `binder.NewReferenceResolver(options, binder.ReferenceResolverHooks{})`
/// over the program's bound files: the reference resolver of a file whose
/// transforms ask the checker nothing.
struct BoundReferenceResolver<'a> {
    host: ProgramResolverHost<'a>,
    resolver: BinderResolver,
}

impl ReferenceResolver for BoundReferenceResolver<'_> {
    fn get_referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>> {
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
        Ok(self.resolver.get_referenced_import_declaration(
            &mut self.host,
            &mut NoReferenceResolverHooks,
            node,
        )?)
    }
    fn get_referenced_value_declaration(&mut self, node: NodeId) -> ResolverResult<Option<NodeId>> {
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
