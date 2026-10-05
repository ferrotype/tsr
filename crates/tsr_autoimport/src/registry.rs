use crate::index::{Index, Named};
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
        for file in program.files() {
            if canceled() {
                return Ok(None);
            }
            let view = file.bound().view().ast();
            let source = view.source_file(file.source())?;
            if source.is_content_mapper_supplemental()
                || program.default_lib_file(source.path()).is_some()
                || !package
                    && source.content_mapper().is_empty()
                    && source
                        .file_name()
                        .windows(b"/node_modules/".len())
                        .any(|part| part == b"/node_modules/")
            {
                continue;
            }
            let path = JsString::from_bytes(source.path());
            let filename = JsString::from_bytes(source.file_name());
            let mut modules = Vec::new();
            if let Some(module) = checker.bound_symbol_of_node(file.source())? {
                modules.push((module, path.clone(), filename.clone()));
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
                for symbol in checker.get_exports_of_module(module)? {
                    if canceled() {
                        return Ok(None);
                    }
                    if let Some(export) = extract(
                        program,
                        checker,
                        symbol,
                        module_id.clone(),
                        module_file_name.clone(),
                        path.clone(),
                    )? {
                        index.insert(export);
                    }
                }
                // export= is not included by GetExportsOfModule for value-only
                // targets. Keep the namespace/default-like import as well.
                if let Some(symbol) = lookup_export(checker, module, b"export=")? {
                    if let Some(export) = extract(
                        program,
                        checker,
                        symbol,
                        module_id.clone(),
                        module_file_name.clone(),
                        path.clone(),
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
// port: tsc/internal/ls/autoimport/export.go:SymbolToExport
pub fn export_id_for_symbol(
    program: &Program,
    checker: &mut Operation<'_>,
    symbol: SymbolRef,
) -> Result<Option<ExportId>, Error> {
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
                    JsString::from_bytes(view.source_file(declaration)?.path())
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
                    module: JsString::from_bytes(file.bound().view().source_file()?.path()),
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
    let syntax = syntax(program, &declarations)?;
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
    let mut local_name = JsString::default();
    if matches!(name.as_bytes(), b"default" | b"export=") {
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
            let file = target_id
                .as_ref()
                .map_or(module_file_name.as_bytes(), |id| id.module.as_bytes());
            let file = if file.is_empty() {
                module.as_bytes()
            } else {
                file
            };
            local_name = JsString::from_bytes(module_identifier(file));
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
