use crate::{index::Named, Export, ExportSyntax};
use tsr_ast::{
    symbol_flags as sf, utilities_modules::try_get_import_from_module_specifier, NodeId,
    SyntaxKind as K,
};
use tsr_checker::{Error, Operation};
use tsr_compiler::Program;
use tsr_lsproto as lsp;

// port: tsc/internal/ls/autoimport/fix.go:getAddAsTypeOnly
#[allow(
    clippy::nonminimal_bool,
    reason = "Keep the pinned verbatim-syntax and type-only export branches recognizable"
)]
pub fn add_as_type_only(
    type_location: bool,
    export: &Export,
    options: &tsr_core::CompilerOptions,
) -> lsp::AddAsTypeOnly {
    if !type_location {
        return lsp::AddAsTypeOnly::NOT_ALLOWED;
    }
    if options.verbatim_module_syntax.is_true()
        && (export.type_only || export.flags & sf::VALUE == 0)
        || export.type_only && export.flags & sf::VALUE != 0
    {
        lsp::AddAsTypeOnly::REQUIRED
    } else {
        lsp::AddAsTypeOnly::ALLOWED
    }
}
// port: tsc/internal/ls/autoimport/fix.go:getImportKind
pub fn import_kind(
    program: &Program,
    checker: &Operation<'_>,
    source: NodeId,
    export: &Export,
    force_import: bool,
) -> Result<lsp::ImportKind, Error> {
    let file = program
        .file_of_node(source)
        .ok_or(Error::MissingLink("auto-import source"))?;
    let view = file.bound().view().ast();
    let file = view.source_file(source)?;
    if program.options().verbatim_module_syntax.is_true()
        && checker.import_file_module_formats(source)?.0 == tsr_core::ModuleKind::COMMON_JS
    {
        return Ok(lsp::ImportKind::COMMON_JS);
    }
    Ok(match export.syntax {
        ExportSyntax::DefaultModifier | ExportSyntax::DefaultDeclaration => {
            lsp::ImportKind::DEFAULT
        }
        ExportSyntax::Named if export.id.name.as_bytes() == b"default" => lsp::ImportKind::DEFAULT,
        ExportSyntax::Modifier
        | ExportSyntax::Named
        | ExportSyntax::Star
        | ExportSyntax::CommonJsExportsProperty => lsp::ImportKind::NAMED,
        ExportSyntax::Equals | ExportSyntax::CommonJsModuleExports | ExportSyntax::Umd => {
            if export.id.name.as_bytes() != b"export=" {
                return Ok(lsp::ImportKind::NAMED);
            }
            for statement in view
                .node_slice(view.node(source)?.statements(view)?)?
                .iter()
                .flatten()
            {
                let read = view.node(statement)?;
                if let Some(reference) = read
                    .data_source()
                    .as_import_equals_declaration()
                    .and_then(|d| d.module_reference())
                {
                    if !tsr_ast::node_is_missing(Some(&view.node(reference)?)) {
                        return Ok(lsp::ImportKind::COMMON_JS);
                    }
                }
            }
            if file.external_module_indicator.is_some() || force_import || !file.is_js() {
                lsp::ImportKind::DEFAULT
            } else {
                lsp::ImportKind::COMMON_JS
            }
        }
        ExportSyntax::None => return Err(Error::MissingLink("auto-import export syntax")),
    })
}
fn syntax_indicators(program: &Program, source: NodeId) -> Result<(bool, bool), Error> {
    let file = program
        .file_of_node(source)
        .ok_or(Error::MissingLink("auto-import source"))?;
    let view = file.bound().view().ast();
    let file = view.source_file(source)?;
    let cjs = file.common_js_module_indicator().is_some();
    let esm = file.external_module_indicator;
    if program.options().emit_module_detection_kind() != tsr_core::ModuleDetectionKind::FORCE {
        return Ok((esm.is_some(), cjs));
    }
    if esm.is_some_and(|n| n != source) {
        return Ok((true, cjs));
    }
    for &id in file.imports()?.iter().flatten() {
        let read = view.node(id)?;
        if read.flags() & tsr_ast::node_flags::SYNTHESIZED != 0 {
            continue;
        }
        if let Some(parent) = read.parent() {
            if matches!(
                view.node(parent)?.kind().known(),
                Some(
                    K::ImportDeclaration
                        | K::JSImportDeclaration
                        | K::ExportDeclaration
                        | K::ExternalModuleReference
                )
            ) {
                return Ok((true, cjs));
            }
        }
    }
    // Under Force, a SourceFile indicator marks module treatment, not ESM
    // syntax. Only the declarations found above establish an ESM preference.
    Ok((false, cjs))
}
// port: tsc/internal/ls/autoimport/fix.go:View.computeShouldUseRequire
pub fn use_require(
    program: &Program,
    checker: &Operation<'_>,
    source: NodeId,
) -> Result<bool, Error> {
    let file = program
        .file_of_node(source)
        .ok_or(Error::MissingLink("auto-import source"))?;
    let source_file = file.bound().view().ast().source_file(source)?;
    if !tsr_tspath::has_js_file_extension(source_file.file_name()) {
        return Ok(false);
    }
    match syntax_indicators(program, source)? {
        (true, false) => return Ok(false),
        (false, true) => return Ok(true),
        _ => {}
    }
    match checker.import_file_module_formats(source)?.1 {
        tsr_core::ModuleKind::COMMON_JS => return Ok(true),
        tsr_core::ModuleKind::ESNEXT => return Ok(false),
        _ => {}
    }
    if !program.options().config_file_path.as_bytes().is_empty() {
        return Ok(program.options().emit_module_kind() < tsr_core::ModuleKind::ES2015);
    }
    for other in program.files() {
        if other.source() == source {
            continue;
        }
        let read = other.bound().view().ast().source_file(other.source())?;
        if !read.is_js()
            || read
                .file_name()
                .windows(b"/node_modules/".len())
                .any(|w| w == b"/node_modules/")
        {
            continue;
        }
        match syntax_indicators(program, other.source())? {
            (true, false) => return Ok(false),
            (false, true) => return Ok(true),
            _ => {}
        }
    }
    Ok(true)
}
#[derive(Default)]
pub struct Usage {
    pub type_only: bool,
    pub jsx: bool,
    pub position: Option<lsp::Position>,
}

/// Produces both namespace qualification and import fixes. The caller ranks
/// equivalent exports, so an existing namespace does not suppress AddNew.
pub fn fixes(
    program: &Program,
    checker: &mut Operation<'_>,
    source: NodeId,
    export: &Export,
    usage: Usage,
    preferences: &crate::Preferences,
) -> Result<Vec<lsp::AutoImportFix>, Error> {
    let file = program
        .file_of_node(source)
        .ok_or(Error::MissingLink("auto-import source"))?;
    let view = file.bound().view().ast();
    let source_file = view.source_file(source)?;
    let kind = import_kind(program, checker, source, export, false)?;
    let as_type = add_as_type_only(usage.type_only, export, program.options());
    let for_jsx = usage.jsx;
    let usage = usage.position;
    let name = String::from_utf8_lossy(export.name()).into_owned();
    let mut result = Vec::new();
    let mut best = None;
    for (index, literal) in source_file.imports()?.iter().enumerate() {
        let Some(literal) = *literal else {
            continue;
        };
        let Some(mut declaration) = try_get_import_from_module_specifier(view, literal)? else {
            continue;
        };
        if let Some(parent) = view.node(declaration)?.parent() {
            if tsr_ast::is_variable_declaration_initialized_to_require(view, parent)? {
                declaration = parent;
            }
        }
        let read = view.node(declaration)?;
        if !matches!(
            read.kind().known(),
            Some(
                K::VariableDeclaration
                    | K::ImportDeclaration
                    | K::ImportEqualsDeclaration
                    | K::JSDocImportTag
            )
        ) {
            continue;
        }
        let Some(module) = checker.get_symbol_at_location(literal)? else {
            continue;
        };
        let declarations: Vec<_> = checker
            .symbol_declarations(module)?
            .iter()
            .flatten()
            .collect();
        let matches = declarations.iter().any(|&d| {
            if let Some(file) = program.file_of_node(d) {
                let view = file.bound().view().ast();
                let Ok(read) = view.node(d) else {
                    return false;
                };
                if read.kind() == K::SourceFile {
                    return view
                        .source_file(d)
                        .is_ok_and(|s| s.path() == export.id.module.as_bytes());
                }
                if read.kind() == K::ModuleDeclaration {
                    return read.name().is_some_and(|n| {
                        view.node_text(n)
                            .is_ok_and(|n| n.as_bytes() == export.id.module.as_bytes())
                    });
                }
            }
            false
        });
        if !matches {
            continue;
        }
        let clause = if matches!(
            read.kind().known(),
            Some(K::ImportDeclaration | K::JSDocImportTag)
        ) {
            read.import_clause()
        } else {
            None
        };
        let binding = clause.and_then(|c| view.node(c).ok()).and_then(|n| {
            n.data_source()
                .as_import_clause()
                .and_then(|d| d.named_bindings())
        });
        let namespace =
            if read.kind() == K::ImportEqualsDeclaration || read.kind() == K::VariableDeclaration {
                read.name()
                    .filter(|&n| view.node(n).is_ok_and(|r| r.kind() == K::Identifier))
            } else {
                binding
                    .filter(|&b| view.node(b).is_ok_and(|b| b.kind() == K::NamespaceImport))
                    .and_then(|b| view.node(b).ok().and_then(|n| n.name()))
            };
        let specifier = String::from_utf8_lossy(view.node_text(literal)?.as_bytes()).into_owned();
        if kind == lsp::ImportKind::NAMED && result.is_empty() && usage.is_some() {
            if let Some(namespace) = namespace {
                result.push(lsp::AutoImportFix {
                    kind: lsp::AutoImportFixKind::USE_NAMESPACE,
                    name: name.clone(),
                    import_kind: lsp::ImportKind::NAMESPACE,
                    module_specifier: specifier.clone(),
                    add_as_type_only: lsp::AddAsTypeOnly::ALLOWED,
                    import_index: index as i32,
                    usage_position: usage.clone().map(Box::new),
                    namespace_prefix: String::from_utf8_lossy(
                        view.node_text(namespace)?.as_bytes(),
                    )
                    .into_owned(),
                    ..Default::default()
                });
            }
        }
        if source_file.is_js() && export.flags & sf::VALUE == 0 && read.kind() != K::JSDocImportTag
        {
            continue;
        }
        if !matches!(kind, lsp::ImportKind::NAMED | lsp::ImportKind::DEFAULT)
            || read.kind() == K::ImportEqualsDeclaration
        {
            continue;
        }
        if read.kind() == K::VariableDeclaration {
            if read.name().is_some_and(|n| {
                view.node(n)
                    .is_ok_and(|r| r.kind() == K::ObjectBindingPattern)
            }) {
                let fix = lsp::AutoImportFix {
                    kind: lsp::AutoImportFixKind::ADD_TO_EXISTING,
                    name: name.clone(),
                    import_kind: kind,
                    add_as_type_only: as_type,
                    module_specifier: specifier,
                    import_index: index as i32,
                    ..Default::default()
                };
                if as_type == lsp::AddAsTypeOnly::NOT_ALLOWED {
                    result.push(fix);
                    return Ok(result);
                }
                best.get_or_insert(fix);
            }
            continue;
        }
        let Some(clause) = clause else {
            continue;
        };
        let clause_read = view.node(clause)?;
        let type_only = clause_read
            .data_source()
            .as_import_clause()
            .is_some_and(|d| d.phase_modifier() == K::TypeKeyword);
        if type_only && !(kind == lsp::ImportKind::NAMED && binding.is_some()) {
            continue;
        }
        if kind == lsp::ImportKind::DEFAULT
            && (clause_read.name().is_some()
                || as_type == lsp::AddAsTypeOnly::REQUIRED && binding.is_some())
        {
            continue;
        }
        if kind == lsp::ImportKind::NAMED && namespace.is_some() {
            continue;
        }
        let fix = lsp::AutoImportFix {
            kind: lsp::AutoImportFixKind::ADD_TO_EXISTING,
            name: name.clone(),
            import_kind: kind,
            add_as_type_only: as_type,
            module_specifier: specifier,
            import_index: index as i32,
            ..Default::default()
        };
        if (as_type != lsp::AddAsTypeOnly::NOT_ALLOWED && type_only)
            || (as_type == lsp::AddAsTypeOnly::NOT_ALLOWED && !type_only)
        {
            result.push(fix);
            return Ok(result);
        }
        best.get_or_insert(fix);
    }
    if let Some(fix) = best {
        result.push(fix);
        return Ok(result);
    }
    let specifier = if !export.ambient_module_name().is_empty() {
        tsr_jsstring::JsString::from_bytes(export.ambient_module_name())
    } else if !export.package_name.is_empty() {
        let Some(specifier) = crate::specifiers::for_package(
            export,
            checker,
            source,
            program.options(),
            preferences,
        )?
        else {
            return Ok(result);
        };
        specifier
    } else {
        let Some(specifier) = checker.module_specifier_for_auto_import(
            source,
            export.module_file_name.as_bytes(),
            preferences.module_specifier.as_deref(),
            preferences.ending.as_deref(),
            &|s| preferences.excludes(s),
        )?
        else {
            return Ok(result);
        };
        specifier
    };
    if preferences.excludes(specifier.as_bytes()) {
        return Ok(result);
    }
    if source_file.is_js()
        && export.flags & sf::VALUE == 0
        && !export.is_unresolved_alias()
        && usage.is_some()
    {
        return Ok(vec![lsp::AutoImportFix {
            kind: lsp::AutoImportFixKind::JSDOC_TYPE_IMPORT,
            name,
            module_specifier: String::from_utf8_lossy(specifier.as_bytes()).into_owned(),
            usage_position: usage.map(Box::new),
            ..Default::default()
        }]);
    }
    let mut name = name;
    if for_jsx && !crate::unicode::is_upper(i32::from(export.name()[0])) {
        if !export.is_renameable() {
            return Ok(Vec::new());
        }
        // The pin reads one byte here, rather than decoding a UTF-8 rune.
        let upper = tsr_jsstring::helpers::simple_upper_go(i32::from(export.name()[0]));
        name = format!(
            "{}{}",
            char::from_u32(upper as u32).unwrap(),
            String::from_utf8_lossy(&export.name()[1..])
        );
    }
    result.push(lsp::AutoImportFix {
        kind: lsp::AutoImportFixKind::ADD_NEW,
        name,
        import_kind: kind,
        add_as_type_only: as_type,
        module_specifier: String::from_utf8_lossy(specifier.as_bytes()).into_owned(),
        use_require: use_require(program, checker, source)?,
        ..Default::default()
    });
    Ok(result)
}
