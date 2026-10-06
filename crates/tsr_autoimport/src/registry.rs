use crate::index::{Index, Named};
use std::collections::HashSet;
use tsr_ast::{
    modifier_flags as mf, symbol_flags as sf, utilities as ast, NodeId, SyntaxKind as K,
};
use tsr_checker::{Error, Operation, SymbolRef};
use tsr_compiler::Program;
use tsr_jsstring::JsString;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExportId {
    pub module: JsString,
    pub name: JsString,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExportSyntax {
    #[default]
    None,
    Modifier,
    Named,
    DefaultModifier,
    DefaultDeclaration,
    Equals,
    Umd,
    Star,
    CommonJsModuleExports,
    CommonJsExportsProperty,
}
#[derive(Clone, Debug)]
pub struct Export {
    pub id: ExportId,
    pub target: Option<ExportId>,
    pub module_file_name: JsString,
    pub syntax: ExportSyntax,
    pub flags: u32,
    pub local_name: JsString,
    pub type_only: bool,
    pub path: JsString,
    pub package_name: JsString,
    pub entrypoints: std::sync::Arc<[tsr_module::ResolvedEntrypoint]>,
    pub completion_kind: Option<tsr_lsproto::CompletionItemKind>,
    pub modifiers: u32,
}
impl Named for Export {
    // port: tsc/internal/ls/autoimport/export.go:Export.Name
    fn name(&self) -> &[u8] {
        if !self.local_name.as_bytes().is_empty() {
            self.local_name.as_bytes()
        } else if self.id.name.as_bytes() == b"export=" {
            self.target.as_ref().map_or(b"", |t| t.name.as_bytes())
        } else {
            self.id.name.as_bytes()
        }
    }
}
impl Export {
    // port: tsc/internal/ls/autoimport/export.go:Export.IsRenameable
    pub fn is_renameable(&self) -> bool {
        matches!(self.id.name.as_bytes(), b"export=" | b"default")
    }
    // port: tsc/internal/ls/autoimport/export.go:Export.AmbientModuleName
    pub fn ambient_module_name(&self) -> &[u8] {
        if tsr_tspath::is_external_module_name_relative(self.id.module.as_bytes()) {
            b""
        } else {
            self.id.module.as_bytes()
        }
    }
    // port: tsc/internal/ls/autoimport/export.go:Export.IsUnresolvedAlias
    pub fn is_unresolved_alias(&self) -> bool {
        self.flags == sf::ALIAS
    }
}
/// One immutable program's export index. A new program gets a new registry;
/// concurrent requests on a retained snapshot share this one by Arc. All
/// identities below are module/name pairs, independent of checker leases.
#[derive(Default, Debug)]
pub struct Registry {
    pub index: Index<Export>,
    pub dependencies: crate::Dependencies,
    pub requested_file: Option<JsString>,
    pub(crate) build_key: crate::preferences::BuildKey,
    pub(crate) sources: std::collections::HashMap<JsString, NodeId>,
    pub(crate) package_imports: std::collections::BTreeSet<crate::package_names::Import>,
}
impl Registry {
    pub fn build(
        program: &Program,
        checker: &mut Operation<'_>,
        canceled: impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        Self::build_index(program, checker, false, canceled)
    }
    pub fn build_package(
        program: &Program,
        checker: &mut Operation<'_>,
        canceled: impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        Self::build_index(program, checker, true, canceled)
    }
    fn build_index(
        program: &Program,
        checker: &mut Operation<'_>,
        package: bool,
        canceled: impl Fn() -> bool,
    ) -> Result<Option<Self>, Error> {
        let mut index = Index::default();
        let symlinked = if package {
            HashSet::new()
        } else {
            symlinked_from_node_modules(program, checker)?
        };
        for file in program.files() {
            if canceled() {
                return Ok(None);
            }
            let view = file.bound().view().ast();
            let source = view.source_file(file.source())?;
            // Ordinary node_modules files, and files reached through a
            // node_modules symlink, are indexed with their packages.
            if source.is_content_mapper_supplemental()
                || program.default_lib_file(source.path()).is_some()
                || !package
                    && source.content_mapper().is_empty()
                    && (source
                        .file_name()
                        .windows(b"/node_modules/".len())
                        .any(|part| part == b"/node_modules/")
                        || symlinked.contains(source.path()))
            {
                continue;
            }
            let path = JsString::from_bytes(source.path());
            let filename = JsString::from_bytes(source.file_name());
            let mut modules = Vec::new();
            let mut augmentations = Vec::new();
            if let Some(module) = checker.bound_symbol_of_node(file.source())? {
                // Package files use their realpath as the module identity, so
                // a symlinked package matches imports resolved to real files.
                let module_id = if package {
                    real_module_path(program, source.file_name())
                } else {
                    path.clone()
                };
                modules.push((module, module_id, filename.clone()));
                augmentations = module_augmentations(program, file)?;
            } else {
                for statement in view
                    .node_slice(view.node(file.source())?.statements(view)?)?
                    .iter()
                    .flatten()
                {
                    let read = view.node(statement)?;
                    if read.kind() == K::ModuleDeclaration {
                        if let Some(name) = read
                            .name()
                            .filter(|&n| view.node(n).is_ok_and(|r| r.kind() == K::StringLiteral))
                        {
                            let name = view.node_text(name)?.into_js_string();
                            if name.as_bytes().contains(&b'*') {
                                continue;
                            }
                            if let Some(module) = checker.bound_symbol_of_node(statement)? {
                                modules.push((module, name, JsString::default()));
                            }
                        }
                    }
                }
            }
            for (module, module_id, module_file_name) in modules {
                let mut symbols = checker.get_exports_of_module(module)?;
                // The resolved exports table may omit the export= assignment.
                if let Some(symbol) = lookup_export(checker, module, b"export=")? {
                    if !symbols.contains(&symbol) {
                        symbols.push(symbol);
                    }
                }
                for symbol in symbols {
                    if canceled() {
                        return Ok(None);
                    }
                    let Some(mut export) = extract(
                        program,
                        checker,
                        symbol,
                        module_id.clone(),
                        module_file_name.clone(),
                        path.clone(),
                        None,
                    )?
                    else {
                        continue;
                    };
                    // GetExportsOfModule forwards export-star symbols unchanged.
                    // The pin assigns their original external-module parent as
                    // the target even when SkipAlias returns the same symbol.
                    if let Some(parent) = checker.symbol(symbol)?.parent() {
                        let parent = checker.symbol_ref(parent)?;
                        let parent = checker.get_merged_symbol(parent)?;
                        if checker.symbol(parent)?.is_external_module() {
                            if let Some(target) = export_id(program, checker, symbol, package)? {
                                if target.module != export.id.module {
                                    export.syntax = ExportSyntax::Star;
                                    export.target = Some(target);
                                }
                            }
                        }
                    }
                    let object_exports = export.syntax == ExportSyntax::CommonJsModuleExports
                        && export.target.is_none();
                    index.insert(export);
                    if object_exports {
                        for member in commonjs_object_members(program, checker, symbol)? {
                            if canceled() {
                                return Ok(None);
                            }
                            if let Some(export) = extract(
                                program,
                                checker,
                                member,
                                module_id.clone(),
                                module_file_name.clone(),
                                path.clone(),
                                Some(ExportSyntax::CommonJsModuleExports),
                            )? {
                                index.insert(export);
                            }
                        }
                    }
                }
            }
            for (declaration, module_id, module_file_name) in augmentations {
                let Some(module) = checker.bound_symbol_of_node(declaration)? else {
                    continue;
                };
                let Some(table) = checker.symbol(module)?.exports() else {
                    continue;
                };
                let symbols: Vec<_> = checker.symbol_table(table)?.symbols().flatten().collect();
                for symbol in symbols {
                    if canceled() {
                        return Ok(None);
                    }
                    let symbol = checker.symbol_ref(symbol)?;
                    if let Some(export) = extract(
                        program,
                        checker,
                        symbol,
                        module_id.clone(),
                        module_file_name.clone(),
                        path.clone(),
                        None,
                    )? {
                        index.insert(export);
                    }
                }
            }
        }
        Ok(Some(Self {
            index,
            dependencies: crate::Dependencies::default(),
            sources: crate::cache::sources(program),
            package_imports: crate::package_names::imports(program),
            requested_file: None,
            build_key: crate::preferences::BuildKey::default(),
        }))
    }
    pub fn set_build_preferences(&mut self, preferences: &crate::Preferences) {
        self.build_key = preferences.build_key();
    }
    pub fn search(&self, importing_path: &[u8], prefix: &[u8]) -> Vec<&Export> {
        self.index
            .search(prefix)
            .into_iter()
            .filter(|e| e.id.module.as_bytes() != importing_path)
            .collect()
    }
}
/// Program files outside the project directory that a symlink exposes under
/// node_modules.
// port: tsc/internal/ls/autoimport/registry.go:hasSymlinkToNodeModules
fn symlinked_from_node_modules(
    program: &Program,
    checker: &Operation<'_>,
) -> Result<HashSet<Vec<u8>>, Error> {
    let mut result = HashSet::new();
    let Some(symlinks) = checker.symlink_cache()? else {
        return Ok(result);
    };
    let root = tsr_tspath::Path::from(tsr_tspath::to_path(
        program.current_directory(),
        program.current_directory(),
        program.use_case_sensitive_file_names(),
    ));
    let through_node_modules = |links: &tsr_module::symlinks::SymlinkSet| {
        let mut found = false;
        links.range(|link| {
            found = link
                .windows(b"/node_modules/".len())
                .any(|part| part == b"/node_modules/");
            !found
        });
        found
    };
    for file in program.files() {
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        let path = tsr_tspath::Path::from_bytes(source.path());
        if root.contains_path(&path) {
            continue;
        }
        let mut found = symlinks
            .files_by_realpath()
            .load(&path)
            .is_some_and(|links| through_node_modules(&links));
        if !found {
            tsr_tspath::for_each_ancestor_directory_path(&path, |directory| {
                found = symlinks
                    .directories_by_realpath()
                    .load(&directory.ensure_trailing_directory_separator())
                    .is_some_and(|links| through_node_modules(&links));
                ((), found)
            });
        }
        if found {
            result.insert(source.path().to_vec());
        }
    }
    Ok(result)
}
/// A module file's non-global augmentations, each with the module ID and file
/// name of the module it augments. Relative names use the augmented file's
/// path; ambient names are their own IDs.
// port: tsc/internal/ls/autoimport/extract.go:exportExtractor.extractFromModule
pub fn module_augmentations(
    program: &Program,
    file: &tsr_compiler::ProgramFile,
) -> Result<Vec<(NodeId, JsString, JsString)>, Error> {
    let view = file.bound().view().ast();
    let source = view.source_file(file.source())?;
    let mut result = Vec::new();
    for &name in source.module_augmentations()?.iter().flatten() {
        let Some(declaration) = view.node(name)?.parent() else {
            continue;
        };
        if tsr_ast::utilities::is_global_scope_augmentation(&view.node(declaration)?) {
            continue;
        }
        let text = view.node_text(name)?.into_js_string();
        if !tsr_tspath::is_external_module_name_relative(text.as_bytes()) {
            result.push((declaration, text, JsString::default()));
            continue;
        }
        // The pin's extractor re-resolves the name in CommonJS mode; this uses
        // the program's retained resolution of the same augmentation name.
        let resolved = program
            .resolved_module_from_specifier(file, name)
            .map_err(|error| match error {
                tsr_compiler::Error::Checker(error) => error,
                tsr_compiler::Error::Ast(error) => Error::from(error),
                _ => Error::MissingLink("module augmentation resolution"),
            })?
            .filter(|module| module.is_resolved())
            .map(|module| module.resolved_file_name.clone());
        let file_name = resolved.unwrap_or_else(|| {
            JsString::from_bytes(tsr_tspath::normalize(&tsr_tspath::combine(
                &tsr_tspath::directory(source.file_name()),
                &[text.as_bytes()],
            )))
        });
        let module = tsr_tspath::to_path(
            file_name.as_bytes(),
            program.current_directory(),
            program.use_case_sensitive_file_names(),
        );
        result.push((declaration, module, file_name));
    }
    Ok(result)
}

/// The export a symbol is reached through, built without the index: a UMD
/// global is in no module's exports table.
// port: tsc/internal/ls/autoimport/export.go:SymbolToExport
pub fn symbol_to_export(
    program: &Program,
    checker: &mut Operation<'_>,
    symbol: SymbolRef,
) -> Result<Option<Export>, Error> {
    if let Some(parent) = checker.symbol(symbol)?.parent() {
        let parent = checker.symbol_ref(parent)?;
        if checker.symbol(parent)?.is_external_module() {
            let Some((module, file_name, path)) = module_of_symbol(program, checker, parent)?
            else {
                return Ok(None);
            };
            return extract(program, checker, symbol, module, file_name, path, None);
        }
    }
    let Some(declaration) = checker.symbol_declarations(symbol)?.iter().flatten().next() else {
        return Ok(None);
    };
    let Some(file) = program.file_of_node(declaration) else {
        return Ok(None);
    };
    let Some(module) = checker.bound_symbol_of_node(file.source())? else {
        return Ok(None);
    };
    let module = checker.get_merged_symbol(module)?;
    let source = file.bound().view().ast().source_file(file.source())?;
    let path = JsString::from_bytes(source.path());
    let file_name = JsString::from_bytes(source.file_name());
    let target = checker.skip_alias(symbol)?;
    let target = checker.get_merged_symbol(target)?;
    let name = JsString::from_bytes(checker.symbol(symbol)?.name_bytes());
    for name in [
        b"default".as_slice(),
        b"export=".as_slice(),
        name.as_bytes(),
    ] {
        if let Some(exported) = lookup_export(checker, module, name)? {
            let resolved = checker.skip_alias(exported)?;
            if checker.get_merged_symbol(resolved)? == target {
                return extract(
                    program,
                    checker,
                    exported,
                    path.clone(),
                    file_name,
                    path,
                    None,
                );
            }
        }
    }
    Ok(None)
}

/// A module symbol's ID, file name and declaring file path.
// port: tsc/internal/ls/autoimport/util.go:tryGetModuleIDAndFileNameOfModuleSymbol
fn module_of_symbol(
    program: &Program,
    checker: &mut Operation<'_>,
    module: SymbolRef,
) -> Result<Option<(JsString, JsString, JsString)>, Error> {
    for declaration in checker.symbol_declarations(module)?.iter().flatten() {
        let Some(file) = program.file_of_node(declaration) else {
            continue;
        };
        let view = file.bound().view().ast();
        if tsr_ast::utilities_modules::is_external_module_augmentation(view, declaration)?
            || tsr_ast::utilities::is_global_scope_augmentation(&view.node(declaration)?)
        {
            continue;
        }
        let source = view.source_file(file.source())?;
        let path = JsString::from_bytes(source.path());
        let read = view.node(declaration)?;
        if read.kind() == K::SourceFile {
            return Ok(Some((
                path.clone(),
                JsString::from_bytes(source.file_name()),
                path,
            )));
        }
        if let Some(name) = read
            .name()
            .filter(|&id| view.node(id).is_ok_and(|n| n.kind() == K::StringLiteral))
        {
            return Ok(Some((
                view.node_text(name)?.into_js_string(),
                JsString::default(),
                path,
            )));
        }
        return Ok(None);
    }
    Ok(None)
}

// port: tsc/internal/ls/autoimport/export.go:SymbolToExport
pub fn export_id_for_symbol(
    program: &Program,
    checker: &mut Operation<'_>,
    symbol: SymbolRef,
) -> Result<Option<ExportId>, Error> {
    export_id(program, checker, symbol, false)
}

/// The module identity of a package file: its realpath's canonical path.
// port: tsc/internal/ls/autoimport/extract.go:symbolExtractor.getModuleID
fn real_module_path(program: &Program, file_name: &[u8]) -> JsString {
    let real = program
        .host()
        .realpath(file_name)
        .unwrap_or_else(|_| JsString::from_bytes(file_name));
    tsr_tspath::to_path(
        real.as_bytes(),
        program.current_directory(),
        program.use_case_sensitive_file_names(),
    )
}

// port: tsc/internal/ls/autoimport/extract.go:symbolExtractor.getModuleIDForSymbol
fn export_id(
    program: &Program,
    checker: &mut Operation<'_>,
    symbol: SymbolRef,
    realpath: bool,
) -> Result<Option<ExportId>, Error> {
    let module_path = |source: &tsr_ast::SourceFileRead<'_>| {
        if realpath {
            real_module_path(program, source.file_name())
        } else {
            JsString::from_bytes(source.path())
        }
    };
    let read = checker.symbol(symbol)?;
    if let Some(parent) = read.parent() {
        let parent = checker.symbol_ref(parent)?;
        if checker.symbol(parent)?.is_external_module() {
            for declaration in checker.symbol_declarations(parent)?.iter().flatten() {
                let Some(file) = program.file_of_node(declaration) else {
                    continue;
                };
                let view = file.bound().view().ast();
                if tsr_ast::utilities_modules::is_external_module_augmentation(view, declaration)?
                    || tsr_ast::utilities::is_global_scope_augmentation(&view.node(declaration)?)
                {
                    continue;
                }
                let d = view.node(declaration)?;
                let module = if d.kind() == K::SourceFile {
                    module_path(&view.source_file(declaration)?)
                } else if let Some(name) = d
                    .name()
                    .filter(|&id| view.node(id).is_ok_and(|n| n.kind() == K::StringLiteral))
                {
                    view.node_text(name)?.into_js_string()
                } else {
                    return Ok(None);
                };
                return Ok(Some(ExportId {
                    module,
                    name: JsString::from_bytes(read.name_bytes()),
                }));
            }
            return Ok(None);
        }
    }
    let Some(declaration) = checker.symbol_declarations(symbol)?.iter().flatten().next() else {
        return Ok(None);
    };
    let Some(file) = program.file_of_node(declaration) else {
        return Ok(None);
    };
    let Some(module) = checker.bound_symbol_of_node(file.source())? else {
        return Ok(None);
    };
    let module = checker.get_merged_symbol(module)?;
    let target = checker.skip_alias(symbol)?;
    let target = checker.get_merged_symbol(target)?;
    let name = JsString::from_bytes(checker.symbol(symbol)?.name_bytes());
    for name in [
        b"default".as_slice(),
        b"export=".as_slice(),
        name.as_bytes(),
    ] {
        if let Some(exported) = lookup_export(checker, module, name)? {
            let exported = checker.skip_alias(exported)?;
            if checker.get_merged_symbol(exported)? == target {
                return Ok(Some(ExportId {
                    module: module_path(&file.bound().view().source_file()?),
                    name: JsString::from_bytes(name),
                }));
            }
        }
    }
    Ok(None)
}

/// Auto-imports retain the `export=` alias itself. The checker's resolved
/// module-export table instead contains the target namespace's members, and
/// is empty for a function-only target.
pub fn lookup_export(
    checker: &mut Operation<'_>,
    module: SymbolRef,
    name: &[u8],
) -> Result<Option<SymbolRef>, Error> {
    if name == b"export=" {
        let Some(table) = checker.symbol(module)?.exports() else {
            return Ok(None);
        };
        return checker
            .symbol_table(table)?
            .get(name)
            .flatten()
            .map(|symbol| checker.symbol_ref(symbol))
            .transpose();
    }
    checker.try_get_member_in_module_exports_and_properties(name, module)
}

fn unusable_name(name: &[u8]) -> bool {
    matches!(
        name,
        b"" | b"_default" | b"__export" | b"default" | b"export="
    )
}
fn extract(
    program: &Program,
    checker: &mut Operation<'_>,
    symbol: SymbolRef,
    module: JsString,
    module_file_name: JsString,
    path: JsString,
    inherited_syntax: Option<ExportSyntax>,
) -> Result<Option<Export>, Error> {
    if checker.symbol(symbol)?.flags() & sf::PROTOTYPE != 0 {
        return Ok(None);
    }
    let name = JsString::from_bytes(checker.symbol(symbol)?.name_bytes());
    let declarations: Vec<_> = checker
        .symbol_declarations(symbol)?
        .iter()
        .flatten()
        .collect();
    let syntax = if let Some(syntax) = inherited_syntax {
        syntax
    } else {
        syntax(program, &declarations)?
    };
    // The native extractor first uses the binder's non-reporting resolver.
    // Preserve that result's flags (including an export-value local) instead
    // of eagerly replacing it with the checker's exported target.
    let local = local_alias_target(program, checker, symbol, syntax, &declarations)?;
    let target = if let Some(local) = local {
        local
    } else {
        checker.skip_alias(symbol)?
    };
    let flags = if local.is_some() {
        checker.symbol(target)?.flags()
    } else {
        checker.get_symbol_flags(target)?
    };
    let target_id = if target == symbol {
        None
    } else {
        let declarations: Vec<_> = checker
            .symbol_declarations(target)?
            .iter()
            .flatten()
            .collect();
        declarations
            .first()
            .and_then(|&d| program.file_of_node(d))
            .map(|file| ExportId {
                module: JsString::from_bytes(
                    file.bound()
                        .view()
                        .ast()
                        .source_file(file.source())
                        .expect("source file owner")
                        .path(),
                ),
                name: JsString::from_bytes(
                    checker
                        .symbol(target)
                        .expect("validated symbol")
                        .name_bytes(),
                ),
            })
    };
    // A UMD global is imported as its module's export=, under the global name.
    let (name, mut local_name) = if syntax == ExportSyntax::Umd {
        (JsString::from_bytes(b"export=".as_slice()), name)
    } else {
        (name, JsString::default())
    };
    if syntax != ExportSyntax::Umd && matches!(name.as_bytes(), b"default" | b"export=") {
        local_name = declaration_name(program, &declarations)?;
        if unusable_name(local_name.as_bytes()) {
            if let Some(target) = &target_id {
                local_name = target.name.clone();
            }
        }
        if unusable_name(local_name.as_bytes()) {
            let declarations: Vec<_> = checker
                .symbol_declarations(target)?
                .iter()
                .flatten()
                .collect();
            local_name = declaration_name(program, &declarations)?;
        }
        if unusable_name(local_name.as_bytes()) {
            // The target's file name keeps the casing a folded path loses.
            // port: tsc/internal/ls/autoimport/extract.go:fileNameForDefaultExportName
            let target_file = if target == symbol {
                None
            } else {
                checker
                    .symbol_declarations(target)?
                    .iter()
                    .flatten()
                    .next()
                    .and_then(|declaration| program.file_of_node(declaration))
                    .map(|file| {
                        let view = file.bound().view().ast();
                        view.source_file(file.source())
                            .map(|source| JsString::from_bytes(source.file_name()))
                    })
                    .transpose()?
                    .filter(|name| !name.is_empty())
            };
            let file = target_file.unwrap_or_else(|| {
                if module_file_name.is_empty() {
                    module.clone()
                } else {
                    module_file_name.clone()
                }
            });
            local_name = JsString::from_bytes(module_identifier(file.as_bytes()));
        }
    }
    let type_only = checker.get_type_only_alias_declaration(symbol)?.is_some();
    let export = Export {
        id: ExportId { module, name },
        target: target_id,
        module_file_name,
        syntax,
        flags,
        local_name,
        type_only,
        path,
        package_name: JsString::default(),
        entrypoints: std::sync::Arc::from([]),
        completion_kind: None,
        modifiers: 0,
    };
    Ok((!unusable_name(export.name())).then_some(export))
}
// port: tsc/internal/ls/autoimport/extract.go:symbolExtractor.tryResolveSymbol
fn local_alias_target(
    program: &Program,
    checker: &Operation<'_>,
    symbol: SymbolRef,
    syntax: ExportSyntax,
    declarations: &[NodeId],
) -> Result<Option<SymbolRef>, Error> {
    if checker.symbol(symbol)?.flags() & sf::ALIAS == 0 {
        return Ok(None);
    }
    for &decl in declarations {
        let read = checker.node(decl)?;
        let name = match syntax {
            ExportSyntax::DefaultDeclaration | ExportSyntax::Equals
                if read.kind() == K::ExportAssignment =>
            {
                read.expression()
            }
            ExportSyntax::Named if read.kind() == K::ExportSpecifier => {
                let named = read.parent().and_then(|id| checker.node(id).ok()?.parent());
                if named.is_some_and(|id| {
                    checker
                        .node(id)
                        .is_ok_and(|n| n.module_specifier().is_some())
                }) {
                    None
                } else {
                    read.name().or(read.property_name())
                }
            }
            _ => None,
        };
        let Some(name) =
            name.filter(|&id| checker.node(id).is_ok_and(|n| n.kind() == K::Identifier))
        else {
            continue;
        };
        let text = checker
            .node(name)?
            .data_source()
            .as_identifier()
            .expect("alias identifier")
            .text()
            .to_vec();
        let mut host = program.resolver_host(&tsr_arena::Counters::new());
        let mut resolver = tsr_binder::name_resolver::NameResolver::new(
            tsr_binder::name_resolver::ResolverOptions {
                emit_script_target: tsr_core::CompilerOptions::default().emit_script_target(),
                isolated_modules: false,
                verbatim_module_syntax: false,
                emit_standard_class_fields: tsr_core::CompilerOptions::default()
                    .emit_standard_class_fields(),
            },
        );
        let mut hooks = tsr_binder::name_resolver::NoNameResolverHooks;
        if let Some(local) = resolver.resolve(
            &mut host,
            &mut hooks,
            Some(name),
            &text,
            sf::ALL,
            None,
            false,
            false,
        )? {
            let local = checker.symbol_ref(local)?;
            if checker.symbol(local)?.flags() & sf::ALIAS == 0 {
                return Ok(Some(local));
            }
        }
    }
    Ok(None)
}
pub(crate) fn declaration_name(
    program: &Program,
    declarations: &[NodeId],
) -> Result<JsString, Error> {
    for &decl in declarations {
        let Some(file) = program.file_of_node(decl) else {
            continue;
        };
        let view = file.bound().view().ast();
        let read = view.node(decl)?;
        let name = match read.kind().known() {
            Some(K::ExportAssignment) => read
                .expression()
                .map(|expression| {
                    ast::skip_outer_expressions(view, expression, ast::outer_expression_kinds::ALL)
                })
                .transpose()?,
            Some(K::ExportSpecifier) => read.property_name(),
            _ => tsr_ast::get_name_of_declaration(view, Some(decl))?,
        };
        if let Some(name) = name.filter(|&n| view.node(n).is_ok_and(|r| r.kind() == K::Identifier))
        {
            return Ok(view.node_text(name)?.into_js_string());
        }
    }
    Ok(JsString::default())
}
// port: tsc/internal/ls/autoimport/extract.go:getSyntax
fn syntax(program: &Program, declarations: &[NodeId]) -> Result<ExportSyntax, Error> {
    for &decl in declarations {
        let Some(file) = program.file_of_node(decl) else {
            continue;
        };
        let view = file.bound().view().ast();
        let read = view.node(decl)?;
        match read.kind().known() {
            Some(K::ExportSpecifier) => return Ok(ExportSyntax::Named),
            Some(K::ExportAssignment) => {
                return Ok(
                    if read
                        .data_source()
                        .as_export_assignment()
                        .is_some_and(|d| d.is_export_equals())
                    {
                        ExportSyntax::Equals
                    } else {
                        ExportSyntax::DefaultDeclaration
                    },
                )
            }
            Some(K::NamespaceExportDeclaration) => return Ok(ExportSyntax::Umd),
            Some(K::BinaryExpression) => {
                match tsr_ast::get_assignment_declaration_kind(view, decl)? {
                    tsr_ast::JSDeclarationKind::ModuleExports => {
                        return Ok(ExportSyntax::CommonJsModuleExports)
                    }
                    tsr_ast::JSDeclarationKind::ExportsProperty => {
                        return Ok(ExportSyntax::CommonJsExportsProperty)
                    }
                    _ => {}
                }
            }
            _ => {
                return Ok(
                    if ast::get_combined_modifier_flags(view, decl)? & mf::DEFAULT != 0 {
                        ExportSyntax::DefaultModifier
                    } else {
                        ExportSyntax::Modifier
                    },
                )
            }
        }
    }
    Ok(ExportSyntax::None)
}
// port: tsc/internal/ls/lsutil/utilities.go:ModuleSpecifierToValidIdentifier
pub fn module_identifier(file: &[u8]) -> Vec<u8> {
    let stem = tsr_tspath::remove_any_file_extension(file);
    let stem = stem.strip_suffix(b"/index").unwrap_or(stem);
    let stem = tsr_tspath::base_name(stem);
    let mut result = Vec::new();
    let mut last_valid = true;
    let mut pos = 0;
    while pos < stem.len() {
        let (ch, size) = tsr_jsstring::wtf8::decode_utf8(&stem[pos..]);
        let valid = if pos == 0 {
            tsr_scanner::is_identifier_start(ch)
        } else {
            tsr_scanner::is_identifier_part(ch)
        };
        if valid {
            result.extend(tsr_jsstring::wtf8::encode_rune(if last_valid {
                ch
            } else {
                tsr_jsstring::helpers::simple_upper_go(ch)
            }));
        }
        last_valid = valid;
        pos += size;
    }
    let token = tsr_scanner::string_to_token(&result);
    if result.is_empty() || tsr_ast::utilities_tail::is_non_contextual_keyword(token.into()) {
        result.insert(0, b'_');
    }
    result
}

// Pin symbolExtractor.extractInto's CommonJSModuleExports object-literal branch.
fn commonjs_object_members(
    program: &Program,
    checker: &Operation<'_>,
    symbol: SymbolRef,
) -> Result<Vec<SymbolRef>, Error> {
    let Some(declaration) = checker.symbol_declarations(symbol)?.iter().flatten().next() else {
        return Ok(Vec::new());
    };
    let Some(expression) = checker
        .node(declaration)?
        .data_source()
        .as_binary_expression()
        .and_then(|read| read.right())
    else {
        return Ok(Vec::new());
    };
    let file = program
        .file_of_node(expression)
        .ok_or(Error::MissingLink("CommonJS export object owner"))?;
    let view = file.bound().view().ast();
    let read = view.node(expression)?;
    if read.kind() != K::ObjectLiteralExpression {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for property in view.node_slice(read.properties(view)?)?.iter().flatten() {
        let read = view.node(property)?;
        let Some(name) = read.name() else { continue };
        if read.kind() == K::ShorthandPropertyAssignment
            || read.kind() == K::PropertyAssignment && view.node(name)?.kind() == K::Identifier
        {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let object = checker
        .bound_symbol_of_node(expression)?
        .ok_or(Error::MissingLink("CommonJS export object symbol"))?;
    let members = checker
        .symbol(object)?
        .members()
        .ok_or(Error::MissingLink("CommonJS export object members"))?;
    let members = checker.symbol_table(members)?;
    let mut result = Vec::with_capacity(names.len());
    for name in names {
        let member = members
            .get(view.node_text(name)?.as_bytes())
            .flatten()
            .ok_or(Error::MissingLink("CommonJS export property member"))?;
        result.push(checker.symbol_ref(member)?);
    }
    Ok(result)
}

#[cfg(test)]
#[path = "registry_commonjs_tests.rs"]
mod commonjs_tests;
