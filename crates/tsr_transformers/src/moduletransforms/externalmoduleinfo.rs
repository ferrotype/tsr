//! `transformers/moduletransforms/externalmoduleinfo.go`: the imports, exports
//! and export bindings of a module, collected for the module transformers, and
//! the `tslib` import of `--importHelpers`.
use crate::transformer::{Error, SharedReferenceResolver};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use tsr_ast::{
    modifier_flags, token_flags, AstView, FactoryMethods, JsString, NodeId, RuntimeFactory,
    SyntaxKind as K,
};
use tsr_core::collections::OrderedSet;
use tsr_core::{CompilerOptions, ModuleKind, TextRange};
use tsr_printer::{emit_flags, EmitContext, EmitHelper};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `collections.MultiMap`: values per key in insertion order.
#[derive(Debug)]
pub struct MultiMap<K, V> {
    entries: HashMap<K, Vec<V>>,
}

impl<K, V> Default for MultiMap<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash, V> MultiMap<K, V> {
    pub fn add(&mut self, key: K, value: V) {
        self.entries.entry(key).or_default().push(value);
    }
    pub fn get(&self, key: &K) -> &[V] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }
}

/// `externalModuleInfo`.
#[derive(Debug, Default)]
pub struct ExternalModuleInfo {
    /// ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration. imports
    /// and reexports of other external modules
    pub external_imports: Vec<NodeId>,
    /// Maps local names to their associated export specifiers (excludes reexports)
    pub export_specifiers: MultiMap<JsString, NodeId>,
    /// Maps local declarations to their associated export aliases
    pub exported_bindings: MultiMap<NodeId, NodeId>,
    /// all exported names in the module, both local and re-exported, excluding
    /// the names of locally exported function declarations
    pub exported_names: Vec<NodeId>,
    /// all of the top-level exported function declarations
    pub exported_functions: OrderedSet<NodeId>,
    /// an export=/module.exports= declaration if one was present
    pub export_equals: Option<NodeId>,
    /// whether this module contains export*
    pub has_export_stars_to_export_values: bool,
}

/// `externalModuleInfoCollector`. The factory is the builder the source file
/// and the generated names live in.
struct ExternalModuleInfoCollector<'c, 'a> {
    factory: &'c mut dyn RuntimeFactory,
    source_file: NodeId,
    emit_context: EmitContext,
    resolver: &'c SharedReferenceResolver<'a>,
    unique_exports: HashSet<JsString>,
    has_export_default: bool,
    output: ExternalModuleInfo,
}

/// A failed read or resolver query is the error; upstream's collection
/// cannot fail.
// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:collectExternalModuleInfo
pub fn collect_external_module_info(
    factory: &mut dyn RuntimeFactory,
    source_file: NodeId,
    _compiler_options: &CompilerOptions,
    emit_context: &EmitContext,
    resolver: &SharedReferenceResolver<'_>,
) -> Result<ExternalModuleInfo, Error> {
    let c = ExternalModuleInfoCollector {
        factory,
        source_file,
        emit_context: emit_context.clone(),
        resolver,
        unique_exports: HashSet::new(),
        has_export_default: false,
        output: ExternalModuleInfo::default(),
    };
    c.collect()
}

fn view(factory: &dyn RuntimeFactory) -> AstView<'_> {
    factory.ast_view().expect(NIL)
}

/// The nodes of `list`, upstream's `NodeList.Nodes`.
fn list_nodes(factory: &dyn RuntimeFactory, list: Option<tsr_ast::NodeListId>) -> Vec<NodeId> {
    let list = factory.read_list(list.expect(NIL));
    factory
        .read_nodes(list.nodes())
        .iter()
        .map(|node| node.expect(NIL))
        .collect()
}

/// `node.Text()` of an identifier or string literal.
fn text(factory: &dyn RuntimeFactory, node: NodeId) -> Result<JsString, Error> {
    Ok(view(factory).node_text(node)?.into_js_string())
}

impl ExternalModuleInfoCollector<'_, '_> {
    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.collect
    fn collect(mut self) -> Result<ExternalModuleInfo, Error> {
        let mut has_import_star = false;
        let mut has_import_default = false;
        let statements = self.factory.node(self.source_file).statement_list();
        for node in list_nodes(self.factory, statements) {
            // Look through NotEmittedStatement to find elided export= declarations
            // (e.g., `declare export = x` is elided by the type eraser but must still be collected)
            if tsr_ast::is_not_emitted_statement(&self.factory.node(node)) {
                let original = self.emit_context.most_original(node);
                let original = self.factory.node(original);
                if let Some(n) = original.as_export_assignment() {
                    if n.is_export_equals() && self.output.export_equals.is_none() {
                        self.output.export_equals = Some(original.id());
                    }
                }
                continue;
            }
            let kind = self.factory.node(node).kind();
            match kind.known() {
                Some(K::ImportDeclaration) => {
                    // import "mod"
                    // import x from "mod"
                    // import * as x from "mod"
                    // import { x, y } from "mod"
                    self.add_external_import(node);
                    if !has_import_star && get_import_needs_import_star_helper(self.factory, node)?
                    {
                        has_import_star = true;
                    }
                    if !has_import_default
                        && get_import_needs_import_default_helper(self.factory, node)?
                    {
                        has_import_default = true;
                    }
                }

                Some(K::ImportEqualsDeclaration) => {
                    let read = self.factory.node(node);
                    let module_reference = read
                        .data_source()
                        .as_import_equals_declaration()
                        .expect("ImportEqualsDeclaration payload")
                        .module_reference()
                        .expect(NIL);
                    drop(read);
                    if tsr_ast::is_external_module_reference(&self.factory.node(module_reference)) {
                        // import x = require("mod")
                        self.add_external_import(node);
                    }
                }

                Some(K::ExportDeclaration) => {
                    let read = self.factory.node(node);
                    let n = read
                        .data_source()
                        .as_export_declaration()
                        .expect("ExportDeclaration payload");
                    let (module_specifier, export_clause) =
                        (n.module_specifier(), n.export_clause());
                    drop(read);
                    if module_specifier.is_some() {
                        // export * from "mod"
                        // export * as ns from "mod"
                        // export { x, y } from "mod"
                        self.add_external_import(node);
                        if let Some(export_clause) = export_clause {
                            if tsr_ast::is_named_exports(&self.factory.node(export_clause)) {
                                // export { x, y } from "mod"
                                self.add_exported_names_for_export_declaration(node)?;
                                if !has_import_default {
                                    has_import_default = contains_default_reference(
                                        self.factory,
                                        Some(export_clause),
                                    )?;
                                }
                            } else {
                                // export * as ns from "mod"
                                let name = self
                                    .factory
                                    .node(export_clause)
                                    .data_source()
                                    .as_namespace_export()
                                    .expect("interface conversion: NamespaceExport payload")
                                    .name()
                                    .expect(NIL);
                                let name_text = text(self.factory, name)?;
                                if self.add_unique_export(name_text) {
                                    self.add_exported_binding(node, name);
                                    self.add_exported_name(name);
                                }
                                // we use the same helpers for `export * as ns` as we do for `import * as ns`
                                has_import_star = true;
                            }
                        } else {
                            // export * from "mod"
                            self.output.has_export_stars_to_export_values = true;
                        }
                    } else {
                        // export { x, y }
                        self.add_exported_names_for_export_declaration(node)?;
                    }
                }

                Some(K::ExportAssignment) => {
                    let is_export_equals = self
                        .factory
                        .node(node)
                        .data_source()
                        .as_export_assignment()
                        .expect("ExportAssignment payload")
                        .is_export_equals();
                    if is_export_equals && self.output.export_equals.is_none() {
                        // export = x
                        self.output.export_equals = Some(node);
                    }
                }

                Some(K::VariableStatement) => {
                    if tsr_ast::utilities::has_syntactic_modifier(
                        view(self.factory),
                        node,
                        modifier_flags::EXPORT,
                    )? {
                        let declaration_list = self
                            .factory
                            .node(node)
                            .data_source()
                            .as_variable_statement()
                            .expect("VariableStatement payload")
                            .declaration_list()
                            .expect(NIL);
                        let declarations = self
                            .factory
                            .node(declaration_list)
                            .data_source()
                            .as_variable_declaration_list()
                            .expect("interface conversion: VariableDeclarationList payload")
                            .declarations();
                        for decl in list_nodes(self.factory, declarations) {
                            self.collect_exported_variable_info(decl)?;
                        }
                    }
                }

                Some(K::FunctionDeclaration) => {
                    let view = view(self.factory);
                    if tsr_ast::utilities::has_syntactic_modifier(
                        view,
                        node,
                        modifier_flags::EXPORT,
                    )? {
                        let is_default = tsr_ast::utilities::has_syntactic_modifier(
                            view,
                            node,
                            modifier_flags::DEFAULT,
                        )?;
                        self.add_exported_function_declaration(
                            node, None, /*name*/
                            is_default,
                        )?;
                    }
                }

                Some(K::ClassDeclaration) => {
                    let view = view(self.factory);
                    if tsr_ast::utilities::has_syntactic_modifier(
                        view,
                        node,
                        modifier_flags::EXPORT,
                    )? {
                        if tsr_ast::utilities::has_syntactic_modifier(
                            view,
                            node,
                            modifier_flags::DEFAULT,
                        )? {
                            // export default class { }
                            if !self.has_export_default {
                                let name = match self.factory.node(node).name() {
                                    Some(name) => name,
                                    None => self
                                        .emit_context
                                        .new_generated_name_for_node(self.factory, node),
                                };
                                self.add_exported_binding(node, name);
                                self.has_export_default = true;
                            }
                        } else {
                            // export class x { }
                            if let Some(name) = self.factory.node(node).name() {
                                if self.add_unique_export(text(self.factory, name)?) {
                                    self.add_exported_binding(node, name);
                                    self.add_exported_name(name);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(self.output)
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.addUniqueExport
    fn add_unique_export(&mut self, name: JsString) -> bool {
        if !self.unique_exports.contains(&name) {
            self.unique_exports.insert(name);
            return true;
        }
        false
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.addExportedBinding
    fn add_exported_binding(&mut self, decl: NodeId, name: NodeId /*ModuleExportName*/) {
        let decl = self.emit_context.most_original(decl);
        self.output.exported_bindings.add(decl, name);
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.addExternalImport
    fn add_external_import(
        &mut self,
        node: NodeId, /*ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration*/
    ) {
        self.output.external_imports.push(node);
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.addExportedName
    fn add_exported_name(&mut self, name: NodeId /*ModuleExportName*/) {
        self.output.exported_names.push(name);
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.addExportedNamesForExportDeclaration
    fn add_exported_names_for_export_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.factory.node(node);
        let n = read
            .data_source()
            .as_export_declaration()
            .expect("ExportDeclaration payload");
        let (module_specifier, export_clause) = (n.module_specifier(), n.export_clause());
        drop(read);
        let elements = self.factory.node(export_clause.expect(NIL)).element_list();
        for specifier in list_nodes(self.factory, elements) {
            let read = self.factory.node(specifier);
            let specifier_name = read.name().expect(NIL);
            let name = read.property_name_or_name().expect(NIL);
            drop(read);
            let specifier_name_text = text(self.factory, specifier_name)?;
            if self.add_unique_export(specifier_name_text.clone()) {
                if self.factory.node(name).kind() != K::StringLiteral {
                    if module_specifier.is_none() {
                        let name_text = text(self.factory, name)?;
                        self.output.export_specifiers.add(name_text, specifier);
                    }

                    let original = self.emit_context.most_original(name);
                    let mut decl = self
                        .resolver
                        .borrow_mut()
                        .get_referenced_import_declaration(original)?;
                    if decl.is_none() {
                        let original = self.emit_context.most_original(name);
                        decl = self
                            .resolver
                            .borrow_mut()
                            .get_referenced_value_declaration(original)?;
                    }
                    if let Some(decl) = decl {
                        if self.factory.node(decl).kind() == K::FunctionDeclaration {
                            self.unique_exports.remove(&specifier_name_text);
                            let is_default =
                                tsr_ast::utilities_middle::module_export_name_is_default(
                                    view(self.factory),
                                    specifier_name,
                                )?;
                            self.add_exported_function_declaration(
                                decl,
                                Some(specifier_name),
                                is_default,
                            )?;
                            continue;
                        }
                        self.add_exported_binding(decl, specifier_name);
                    }
                }

                self.add_exported_name(specifier_name);
            }
        }
        Ok(())
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.addExportedFunctionDeclaration
    fn add_exported_function_declaration(
        &mut self,
        node: NodeId,
        mut name: Option<NodeId>, /*ModuleExportName*/
        is_default: bool,
    ) -> Result<(), Error> {
        let original = self.emit_context.most_original(node);
        self.output.exported_functions.insert(original);
        if is_default {
            // export default function() { }
            // function x() { } + export { x as default };
            if !self.has_export_default {
                let name = match name {
                    Some(name) => name,
                    None => self
                        .emit_context
                        .new_generated_name_for_node(self.factory, node),
                };
                self.add_exported_binding(node, name);
                self.has_export_default = true;
            }
        } else {
            // export function x() { }
            // function x() { } + export { x }
            if name.is_none() {
                name = self.factory.node(node).name();
            }
            let name = name.expect(NIL);
            let name_text = text(self.factory, name)?;
            if self.add_unique_export(name_text) {
                self.add_exported_binding(node, name);
            }
        }
        Ok(())
    }

    // port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:externalModuleInfoCollector.collectExportedVariableInfo
    fn collect_exported_variable_info(
        &mut self,
        decl: NodeId, /*VariableDeclaration | BindingElement*/
    ) -> Result<(), Error> {
        let name = self.factory.node(decl).name().expect(NIL);
        if tsr_ast::utilities::is_binding_pattern(&self.factory.node(name)) {
            let elements = self.factory.node(name).element_list();
            for element in list_nodes(self.factory, elements) {
                let e = self.factory.node(element);
                let e = e.data_source().as_binding_element().unwrap_or_else(|| {
                    panic!(
                        "interface conversion: ast.nodeData is {}, not *ast.BindingElement",
                        self.factory.node(element).kind()
                    )
                });
                if e.name().is_some() {
                    self.collect_exported_variable_info(element)?;
                }
            }
        } else if !self.emit_context.has_auto_generate_info(name) {
            let name_text = text(self.factory, name)?;
            if self.add_unique_export(name_text) {
                self.add_exported_name(name);
                if is_local_name(&self.emit_context, name) {
                    self.add_exported_binding(decl, name);
                }
            }
        }
        Ok(())
    }
}

// TODO(h1): transformers.IsLocalName
fn is_local_name(emit_context: &EmitContext, name: NodeId) -> bool {
    emit_context.emit_flags(name) & emit_flags::LOCAL_NAME != 0
}

const EXTERNAL_HELPERS_MODULE_NAME_TEXT: &[u8] = b"tslib";

/// `tsr_printer` errors other than storage reads are upstream panics.
fn printer_error(error: tsr_printer::Error) -> Error {
    match error {
        tsr_printer::Error::Arena(error) => Error::Arena(error),
        error => panic!("{error:?}"),
    }
}

/// A failed read is the error; upstream's construction cannot fail.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:createExternalHelpersImportDeclarationIfNeeded
pub fn create_external_helpers_import_declaration_if_needed(
    emit_context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    source_file: NodeId,
    compiler_options: &CompilerOptions,
    file_module_kind: ModuleKind,
    has_export_stars_to_export_values: bool,
    has_import_star: bool,
    has_import_default: bool,
) -> Result<Option<NodeId> /*ImportDeclaration | ImportEqualsDeclaration*/, Error> {
    if compiler_options.import_helpers.is_true()
        && tsr_ast::utilities_modules::is_effective_external_module(
            &factory.read_source_file(source_file)?,
            compiler_options,
        )
    {
        let module_kind = compiler_options.emit_module_kind();
        let helpers = get_imported_helpers(emit_context, source_file);
        if file_module_kind == ModuleKind::COMMON_JS
            || file_module_kind == ModuleKind::NONE && module_kind == ModuleKind::COMMON_JS
        {
            // When we emit to a non-ES module, generate a synthetic `import tslib = require("tslib")` to be further transformed.
            let external_helpers_module_name = get_or_create_external_helpers_module_name_if_needed(
                emit_context,
                factory,
                source_file,
                compiler_options,
                &helpers,
                has_export_stars_to_export_values,
                has_import_star || has_import_default,
                file_module_kind,
            );
            if let Some(external_helpers_module_name) = external_helpers_module_name {
                let literal = factory.new_string_literal(
                    JsString::from_bytes(EXTERNAL_HELPERS_MODULE_NAME_TEXT),
                    token_flags::NONE,
                );
                let reference = factory.new_external_module_reference(Some(literal));
                let external_helpers_import_declaration = factory.new_import_equals_declaration(
                    None,  /*modifiers*/
                    false, /*isTypeOnly*/
                    Some(external_helpers_module_name),
                    Some(reference),
                );
                emit_context.add_emit_flags(
                    external_helpers_import_declaration,
                    emit_flags::CUSTOM_PROLOGUE,
                );
                return Ok(Some(external_helpers_import_declaration));
            }
        } else {
            // When we emit as an ES module, generate an `import` declaration that uses named imports for helpers.
            // If we cannot determine the implied module kind under `module: preserve` we assume ESM.
            let mut helper_names: Vec<&'static [u8]> = Vec::new();
            for helper in &helpers {
                let import_name = helper.import_name;
                if !import_name.is_empty() && !helper_names.contains(&import_name) {
                    helper_names.push(import_name);
                }
            }
            if !helper_names.is_empty() {
                helper_names.sort_unstable();
                // Alias the imports if the names are used somewhere in the file.
                // NOTE: We don't need to care about global import collisions as this is a module.

                let mut import_specifiers = Vec::with_capacity(helper_names.len());
                for name in helper_names {
                    let unique = emit_context
                        .is_file_level_unique_name(
                            view(factory),
                            source_file,
                            name,
                            None, /*hasGlobalName*/
                        )
                        .map_err(printer_error)?;
                    let specifier = if unique {
                        let identifier = factory.new_identifier(JsString::from_bytes(name));
                        factory.new_import_specifier(
                            false, /*isTypeOnly*/
                            None,  /*propertyName*/
                            Some(identifier),
                        )
                    } else {
                        let identifier = factory.new_identifier(JsString::from_bytes(name));
                        let helper_name = emit_context.new_unscoped_helper_name(factory, name);
                        factory.new_import_specifier(
                            false, /*isTypeOnly*/
                            Some(identifier),
                            Some(helper_name),
                        )
                    };
                    import_specifiers.push(specifier);
                }
                let nodes = factory.alloc_nodes(import_specifiers.into_iter().map(Some).collect());
                let list = factory.alloc_list(TextRange::new(-1, -1), nodes);
                let named_bindings = factory.new_named_imports(Some(list));
                let parse_node = emit_context.most_original(source_file);
                emit_context.add_emit_flags(parse_node, emit_flags::EXTERNAL_HELPERS);

                let import_clause = factory.new_import_clause(
                    K::Unknown.into(), /*phaseModifier*/
                    None,              /*name*/
                    Some(named_bindings),
                );
                let literal = factory.new_string_literal(
                    JsString::from_bytes(EXTERNAL_HELPERS_MODULE_NAME_TEXT),
                    token_flags::NONE,
                );
                let external_helpers_import_declaration = factory.new_import_declaration(
                    None, /*modifiers*/
                    Some(import_clause),
                    Some(literal),
                    None, /*attributes*/
                );

                emit_context.add_emit_flags(
                    external_helpers_import_declaration,
                    emit_flags::CUSTOM_PROLOGUE,
                );
                return Ok(Some(external_helpers_import_declaration));
            }
        }
    }
    Ok(None)
}

// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:getImportedHelpers
pub fn get_imported_helpers(
    emit_context: &EmitContext,
    source_file: NodeId,
) -> Vec<&'static EmitHelper> {
    let mut helpers = Vec::new();
    for helper in emit_context.get_emit_helpers(source_file) {
        if !helper.scoped {
            helpers.push(helper);
        }
    }
    helpers
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:getOrCreateExternalHelpersModuleNameIfNeeded
pub fn get_or_create_external_helpers_module_name_if_needed(
    emit_context: &mut EmitContext,
    factory: &mut dyn RuntimeFactory,
    node: NodeId,
    _compiler_options: &CompilerOptions,
    helpers: &[&'static EmitHelper],
    has_export_stars_to_export_values: bool,
    has_import_star_or_import_default: bool,
    file_module_kind: ModuleKind,
) -> Option<NodeId> {
    let mut external_helpers_module_name =
        emit_context.get_external_helpers_module_name(factory, node);
    if external_helpers_module_name.is_some() {
        return external_helpers_module_name;
    }

    let create = !helpers.is_empty()
        || (has_export_stars_to_export_values || has_import_star_or_import_default)
            && file_module_kind < ModuleKind::SYSTEM;

    if create {
        let name = emit_context.new_unique_name(
            factory,
            JsString::from_bytes(EXTERNAL_HELPERS_MODULE_NAME_TEXT),
        );
        emit_context.set_external_helpers_module_name(factory, node, Some(name));
        external_helpers_module_name = Some(name);
    }

    external_helpers_module_name
}

// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:isNamedDefaultReference
pub fn is_named_default_reference(
    factory: &dyn RuntimeFactory,
    e: NodeId, /*ImportSpecifier | ExportSpecifier*/
) -> Result<bool, Error> {
    let name = factory.node(e).property_name_or_name().expect(NIL);
    Ok(tsr_ast::utilities_middle::module_export_name_is_default(
        view(factory),
        name,
    )?)
}

// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:containsDefaultReference
pub fn contains_default_reference(
    factory: &dyn RuntimeFactory,
    node: Option<NodeId>, /*NamedImportBindings | NamedExportBindings*/
) -> Result<bool, Error> {
    let Some(node) = node else {
        return Ok(false);
    };
    let read = factory.node(node);
    if !(tsr_ast::is_named_imports(&read) || tsr_ast::is_named_exports(&read)) {
        return Ok(false);
    }
    let elements = read.element_list();
    drop(read);
    for element in list_nodes(factory, elements) {
        if is_named_default_reference(factory, element)? {
            return Ok(true);
        }
    }
    Ok(false)
}

// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:getExportNeedsImportStarHelper
pub fn get_export_needs_import_star_helper(
    factory: &dyn RuntimeFactory,
    node: NodeId, /*ExportDeclaration*/
) -> Result<bool, Error> {
    Ok(tsr_ast::utilities_middle::get_namespace_declaration_node(view(factory), node)?.is_some())
}

/// The import clause's named bindings, `node.ImportClause.AsImportClause().NamedBindings`.
fn named_bindings(factory: &dyn RuntimeFactory, import_clause: NodeId) -> Option<NodeId> {
    factory
        .node(import_clause)
        .data_source()
        .as_import_clause()
        .expect("interface conversion: ImportClause payload")
        .named_bindings()
}

// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:getImportNeedsImportStarHelper
pub fn get_import_needs_import_star_helper(
    factory: &dyn RuntimeFactory,
    node: NodeId, /*ImportDeclaration*/
) -> Result<bool, Error> {
    let view = view(factory);
    if tsr_ast::utilities_middle::get_namespace_declaration_node(view, node)?.is_some() {
        return Ok(true);
    }
    let Some(import_clause) = factory.node(node).import_clause() else {
        return Ok(false);
    };
    let Some(bindings) = named_bindings(factory, import_clause) else {
        return Ok(false);
    };
    if !tsr_ast::is_named_imports(&factory.node(bindings)) {
        return Ok(false);
    }
    let elements = list_nodes(factory, factory.node(bindings).element_list());
    let mut default_ref_count = 0;
    for &binding in &elements {
        if is_named_default_reference(factory, binding)? {
            default_ref_count += 1;
        }
    }
    // Import star is required if there's default named refs mixed with non-default refs, or if theres non-default refs and it has a default import
    Ok(
        (default_ref_count > 0 && default_ref_count != elements.len())
            || (elements.len() - default_ref_count != 0
                && tsr_ast::utilities_middle::is_default_import(view, &factory.node(node))?),
    )
}

// port: tsc/internal/transformers/moduletransforms/externalmoduleinfo.go:getImportNeedsImportDefaultHelper
pub fn get_import_needs_import_default_helper(
    factory: &dyn RuntimeFactory,
    node: NodeId, /*ImportDeclaration*/
) -> Result<bool, Error> {
    // Import default is needed if there's a default import or a default ref and no other refs (meaning an import star helper wasn't requested)
    if get_import_needs_import_star_helper(factory, node)? {
        return Ok(false);
    }
    if tsr_ast::utilities_middle::is_default_import(view(factory), &factory.node(node))? {
        return Ok(true);
    }
    let Some(import_clause) = factory.node(node).import_clause() else {
        return Ok(false);
    };
    let bindings = named_bindings(factory, import_clause).expect(NIL);
    if !tsr_ast::is_named_imports(&factory.node(bindings)) {
        return Ok(false);
    }
    contains_default_reference(factory, Some(bindings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multimap_keeps_values_in_insertion_order() {
        let mut map: MultiMap<u8, u8> = MultiMap::default();
        map.add(1, 3);
        map.add(1, 2);
        map.add(2, 1);
        assert_eq!(map.get(&1), &[3, 2]);
        assert_eq!(map.get(&3), &[] as &[u8]);
        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
        assert_eq!(map.keys().count(), 2);
    }
}
