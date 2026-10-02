//! `transformers/moduletransforms/utilities.go`: the helpers the module
//! transformers share. Upstream's `host any /*EmitHost*/` parameters are
//! placeholders nothing reads; the port omits them.
use crate::transformer::{Error, SharedEmitResolver};
use tsr_ast::{
    token_flags, Factory, FactoryMethods, JsString, NodeId, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::{CompilerOptions, TextRange};
use tsr_printer::{EmitContext, GeneratedIdentifierFlagsExt};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

// port: tsc/internal/transformers/moduletransforms/utilities.go:isDeclarationNameOfEnumOrNamespace
pub fn is_declaration_name_of_enum_or_namespace(
    emit_context: &EmitContext,
    factory: &dyn Factory,
    node: NodeId,
) -> bool {
    let original = emit_context.most_original(node);
    if let Some(parent) = factory.node(original).parent() {
        let parent = factory.node(parent);
        if matches!(
            parent.kind().known(),
            Some(K::EnumDeclaration | K::ModuleDeclaration)
        ) {
            return Some(original) == parent.name();
        }
    }
    false
}

// port: tsc/internal/transformers/moduletransforms/utilities.go:rewriteModuleSpecifier
pub fn rewrite_module_specifier(
    emit_context: &mut EmitContext,
    factory: &mut dyn Factory,
    node: Option<NodeId>,
    compiler_options: &CompilerOptions,
) -> Option<NodeId> {
    let Some(id) = node else {
        return node;
    };
    let read = factory.node(id);
    let Some(literal) = read.as_string_literal() else {
        return node;
    };
    let text = literal.text().to_vec();
    let flags = literal.token_flags();
    drop(read);
    if !tsr_tspath::should_rewrite_module_specifier(&text, compiler_options) {
        return node;
    }
    let updated_text = tsr_tspath::change_extension(
        &text,
        tsr_tsoptions::output_paths::get_output_extension(&text, compiler_options.jsx),
    );
    if updated_text != text {
        let updated = factory.new_string_literal(JsString::from_bytes(updated_text), flags);
        emit_context.set_original(updated, id);
        emit_context.assign_comment_and_source_map_ranges(factory, updated, id);
        return Some(updated);
    }
    node
}

// port: tsc/internal/transformers/moduletransforms/utilities.go:createEmptyImports
pub fn create_empty_imports(factory: &mut dyn RuntimeFactory) -> NodeId {
    let nodes = factory.alloc_nodes(Vec::new());
    let list = factory.alloc_list(TextRange::new(-1, -1), nodes);
    let exports = factory.new_named_exports(Some(list));
    factory.new_export_declaration(
        None,  /*modifiers*/
        false, /*isTypeOnly*/
        Some(exports),
        None, /*moduleSpecifier*/
        None, /*attributes*/
    )
}

/// Get the name of a target module from an import/export declaration as should
/// be written in the emitted output. The emitted output name can be different
/// from the input if:
///  1. The module has a `/// <amd-module name="<new name>" />`
///  2. `--out` or `--outFile` is used, making the name relative to the rootDir
///  3. The containing SourceFile has an entry in renamedDependencies for the
///     import as requested by some module loaders (e.g. System).
///
/// Otherwise, a new StringLiteral node representing the module name will be
/// returned. A `None` resolver is upstream's nil `EmitResolver`.
// port: tsc/internal/transformers/moduletransforms/utilities.go:getExternalModuleNameLiteral
pub fn get_external_module_name_literal(
    factory: &mut dyn RuntimeFactory,
    import_node: NodeId, /*ImportDeclaration | ExportDeclaration | ImportEqualsDeclaration | ImportCall*/
    source_file: NodeId,
    resolver: Option<&SharedEmitResolver<'_>>,
    compiler_options: &CompilerOptions,
) -> Result<Option<NodeId>, Error> {
    let view = factory.ast_view().expect(NIL);
    let module_name = tsr_ast::utilities_modules::get_external_module_name(view, import_node)?;
    if let Some(module_name) = module_name {
        if tsr_ast::is_string_literal(&factory.node(module_name)) {
            let mut name = try_get_module_name_from_declaration(
                import_node,
                factory,
                resolver,
                compiler_options,
            )?;
            if name.is_none() {
                name = try_rename_external_module(factory, module_name, source_file);
            }
            if name.is_none() {
                // !!! propagate token flags (will produce new diffs)
                let text = factory
                    .node(module_name)
                    .as_string_literal()
                    .expect("StringLiteral payload")
                    .text()
                    .to_vec();
                name =
                    Some(factory.new_string_literal(JsString::from_bytes(text), token_flags::NONE));
            }
            return Ok(name);
        }
    }
    Ok(None)
}

/// Get the name of a module as should be written in the emitted output. The
/// emitted output name can be different from the input if:
///  1. The module has a `/// <amd-module name="<new name>" />`
///  2. `--out` or `--outFile` is used, making the name relative to the rootDir
///
/// Otherwise, a new StringLiteral node representing the module name will be
/// returned.
// port: tsc/internal/transformers/moduletransforms/utilities.go:tryGetModuleNameFromFile
pub fn try_get_module_name_from_file(
    _factory: &mut dyn RuntimeFactory,
    file: Option<NodeId>,
    _options: &CompilerOptions,
) -> Option<NodeId> {
    file?;
    // !!!
    // if file.moduleName {
    // 	return factory.createStringLiteral(file.moduleName)
    // }
    None
}

// port: tsc/internal/transformers/moduletransforms/utilities.go:tryGetModuleNameFromDeclaration
pub fn try_get_module_name_from_declaration(
    declaration: NodeId, /*ImportEqualsDeclaration | ImportDeclaration | ExportDeclaration | ImportCall*/
    factory: &mut dyn RuntimeFactory,
    resolver: Option<&SharedEmitResolver<'_>>,
    compiler_options: &CompilerOptions,
) -> Result<Option<NodeId>, Error> {
    let Some(resolver) = resolver else {
        return Ok(None);
    };
    let file = resolver
        .borrow_mut()
        .get_external_module_file_from_declaration(declaration)?;
    Ok(try_get_module_name_from_file(
        factory,
        file,
        compiler_options,
    ))
}

/// Resolves a local path to a path which is absolute to the base of the emit.
// port: tsc/internal/transformers/moduletransforms/utilities.go:getExternalModuleNameFromPath
pub fn get_external_module_name_from_path(_file_name: &[u8], _reference_path: &[u8]) -> JsString {
    // !!!
    JsString::default()
}

/// Some bundlers (SystemJS builder) sometimes want to rename dependencies.
/// Here we check if alternative name was provided for a given moduleName and
/// return it if possible.
// port: tsc/internal/transformers/moduletransforms/utilities.go:tryRenameExternalModule
pub fn try_rename_external_module(
    _factory: &mut dyn RuntimeFactory,
    _module_name: NodeId,
    _source_file: NodeId,
) -> Option<NodeId> {
    // !!!
    None
}

// port: tsc/internal/transformers/moduletransforms/utilities.go:isFileLevelReservedGeneratedIdentifier
pub fn is_file_level_reserved_generated_identifier(
    emit_context: &EmitContext,
    name: NodeId,
) -> bool {
    emit_context.auto_generate_info(name).is_some_and(|info| {
        info.flags.is_file_level()
            && info.flags.is_optimistic()
            && info.flags.is_reserved_in_nested_scopes()
    })
}

/// A simple inlinable expression is an expression which can be copied into
/// multiple locations without risk of repeating any sideeffects and whose
/// value could not possibly change between any such locations.
// port: tsc/internal/transformers/moduletransforms/utilities.go:isSimpleInlineableExpression
pub fn is_simple_inlineable_expression(factory: &dyn Factory, expression: NodeId) -> bool {
    !tsr_ast::is_identifier(&factory.node(expression))
        && crate::utilities::is_simple_copiable_expression(factory, expression)
}
