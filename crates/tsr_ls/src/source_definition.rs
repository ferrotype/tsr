//! Source-definition resolution uses a private NoDts resolver and bound files.
//! It never inserts navigation-only files into the program or its checker.
use crate::{
    source_declarations::{self as decl, Declaration},
    syntax::Syntax,
    LanguageService, Result,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tsr_ast::{
    span_map::FEATURE_DEFINITION, utilities as ast, utilities_positions as pos, AstView, NodeId,
    SyntaxKind as K,
};
use tsr_checker::Operation;
use tsr_compiler::{Program, ProgramFile};
use tsr_core::{ModuleKind, TextRange};
use tsr_jsstring::JsString;
use tsr_lsproto as lsp;

struct Resolver<'a> {
    program: &'a Program,
    modules: tsr_module::Resolver,
    from: Vec<u8>,
    files: HashMap<Vec<u8>, Option<Arc<ProgramFile>>>,
    cancellation: tsr_core::CancellationToken,
}
impl<'a> Resolver<'a> {
    // port: tsc/internal/ls/sourcedefinition.go:LanguageService.newSourceDefResolver
    fn new(
        program: &'a Program,
        from: &[u8],
        cancellation: tsr_core::CancellationToken,
    ) -> Result<Self> {
        Ok(Self {
            program,
            modules: program.source_definition_resolver()?,
            from: from.to_vec(),
            files: HashMap::new(),
            cancellation,
        })
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.getOrParseSourceFile
    fn file(&mut self, name: &[u8]) -> Result<Option<Arc<ProgramFile>>> {
        if self.cancellation.is_canceled() {
            return Err(crate::Error::Canceled);
        }
        if name.is_empty() {
            return Ok(None);
        }
        if let Some(file) = self.program.source_file(name) {
            return Ok(self.program.file_of_node(file.source()).cloned());
        }
        if let Some(file) = self.files.get(name) {
            return Ok(file.clone());
        }
        let file = if let Some(content) = self.program.host().read_file(name).ok().flatten() {
            Some(ProgramFile::parse_and_bind(
                content.text,
                tsr_core::ScriptKind::ensure_from_file_name(name),
                tsr_ast::SourceFileParseOptions {
                    file_name: JsString::from_bytes(name),
                    path: tsr_tspath::to_path(
                        name,
                        self.program.current_directory(),
                        self.program.use_case_sensitive_file_names(),
                    ),
                    ..Default::default()
                },
                &tsr_arena::Counters::new(),
                None,
                None,
            )?)
        } else {
            None
        };
        self.files.insert(name.to_vec(), file.clone());
        Ok(file)
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.inferImpliedNodeFormat
    fn mode(&mut self, name: &[u8]) -> Result<ModuleKind> {
        let package = self
            .modules
            .package_scope(&tsr_tspath::directory(name))
            .map_err(tsr_compiler::Error::from)?;
        Ok(tsr_compiler::navigation_module_format(
            name,
            package
                .as_ref()
                .and_then(|p| p.string("type"))
                .unwrap_or_default(),
        ))
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.resolveImplementationFrom
    fn implementation_from(
        &mut self,
        name: &[u8],
        from: &[u8],
        preferred: ModuleKind,
    ) -> Result<Vec<u8>> {
        let mut modes = vec![preferred];
        if preferred != ModuleKind::ESNEXT {
            modes.push(ModuleKind::ESNEXT);
        }
        if preferred != ModuleKind::COMMON_JS {
            modes.push(ModuleKind::COMMON_JS);
        }
        for mode in modes {
            let result = self
                .modules
                .resolve(name, from, mode)
                .map_err(tsr_compiler::Error::from)?;
            let file = result.resolved_file_name.as_bytes();
            if !file.is_empty() && !tsr_tspath::is_declaration_file_name(file) {
                return Ok(file.to_vec());
            }
        }
        Ok(Vec::new())
    }
    fn implementation(&mut self, name: &[u8], mode: ModuleKind) -> Result<Vec<u8>> {
        self.implementation_from(name, &self.from.clone(), mode)
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.findImplementationFileFromDtsFileName
    fn implementation_for_declaration_file(
        &mut self,
        name: &[u8],
        mode: ModuleKind,
    ) -> Result<Vec<u8>> {
        let ext = tsr_module::js_extension_for_file(name, self.program.options());
        if !ext.is_empty() {
            let candidate = tsr_tspath::change_extension(name, ext);
            if self
                .program
                .host()
                .file_exists(&candidate)
                .map_err(tsr_compiler::Error::from)?
            {
                return Ok(candidate);
            }
        }
        let mut segments = name
            .windows(14)
            .enumerate()
            .filter(|(_, b)| *b == b"/node_modules/");
        let Some((start, _)) = segments.next() else {
            return Ok(Vec::new());
        };
        if segments.next().is_some() {
            return Ok(Vec::new());
        }
        let root = tsr_module::parse_node_module_from_path(name, false);
        if root.len() <= start + 14 {
            return Ok(Vec::new());
        }
        let package = tsr_module::package_name_from_types_package_name(
            &tsr_module::unmangle_scoped_package_name(&name[start + 14..root.len()]),
        );
        if package.is_empty() {
            return Ok(Vec::new());
        }
        let tail = name.get(root.len() + 1..).unwrap_or_default();
        if !tail.is_empty() {
            let spec = [
                package.as_slice(),
                b"/",
                tsr_tspath::remove_file_extension(tail),
            ]
            .concat();
            let file = self.implementation(&spec, mode)?;
            if !file.is_empty() {
                return Ok(file);
            }
        }
        self.implementation(&package, mode)
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.getForwardedImplementationFiles
    fn forwards(&mut self, file: &ProgramFile) -> Result<Vec<Vec<u8>>> {
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        let name = source.file_name();
        let mode = self.mode(name)?;
        let mut result = Vec::new();
        for import in source.imports()?.iter().flatten() {
            let target =
                self.implementation_from(view.node_text(*import)?.as_bytes(), name, mode)?;
            if !target.is_empty() && !result.contains(&target) {
                result.push(target);
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.findDeclarationsInFile
    fn find(
        &mut self,
        name: &[u8],
        names: &[Vec<u8>],
        seen: &mut HashSet<Vec<u8>>,
    ) -> Result<Vec<Declaration>> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            if name.is_empty() || names.is_empty() || !seen.insert(name.to_vec()) {
                return Ok(Vec::new());
            }
            let Some(file) = self.file(name)? else {
                return Ok(Vec::new());
            };
            let mut declarations = decl::find(&file, names, &self.cancellation)?;
            if !declarations.is_empty() && decl::has_concrete(&declarations)? {
                return Ok(declarations);
            }
            let mut forwarded = Vec::new();
            for file in self.forwards(&file)? {
                forwarded.extend(self.find(&file, names, seen)?);
            }
            if forwarded.is_empty() {
                return Ok(declarations);
            }
            if decl::has_concrete(&forwarded)? {
                return decl::unique(forwarded);
            }
            declarations.extend(forwarded);
            decl::unique(declarations)
        })
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.searchImplementationFile
    fn search(
        &mut self,
        view: AstView<'_>,
        node: NodeId,
        name: &[u8],
        names: &[Vec<u8>],
    ) -> Result<Vec<Declaration>> {
        if name.is_empty() {
            return Ok(Vec::new());
        }
        let Some(file) = self.file(name)? else {
            return Ok(Vec::new());
        };
        if decl::default_import(view, node)? {
            let found = self.find(name, &[b"default".to_vec()], &mut HashSet::new())?;
            return if found.is_empty() {
                Ok(vec![decl::entry(file)?])
            } else {
                decl::preferred(view, node, found)
            };
        }
        let found = self.find(name, names, &mut HashSet::new())?;
        decl::preferred(view, node, found)
    }
    // port: tsc/internal/ls/sourcedefinition.go:sourceDefResolver.mapDeclarationToSource
    fn map_declaration(
        &mut self,
        l: &mut LanguageService<'_>,
        view: AstView<'_>,
        node: NodeId,
        d: Declaration,
        implementation: &[u8],
    ) -> Result<Vec<Declaration>> {
        let name = d.view().source_file(d.file.source())?.file_name().to_vec();
        let declaration_name =
            tsr_ast::get_name_of_declaration(d.view(), Some(d.node))?.unwrap_or(d.node);
        let start = Syntax::new(d.view(), d.file.source())?
            .reference_range(declaration_name, None)?
            .pos();
        if let Some(mapped) = l.source_position(&name, start) {
            if let Some(file) = self.file(mapped.file_name.as_bytes())? {
                return Ok(vec![decl::closest(file, mapped.pos as i64)?]);
            }
        }
        if !tsr_tspath::is_declaration_file_name(&name) {
            return Ok(vec![d]);
        }
        let implementation = if implementation.is_empty() {
            let mode = self.mode(&name)?;
            self.implementation_for_declaration_file(&name, mode)?
        } else {
            implementation.to_vec()
        };
        self.search(
            view,
            node,
            &implementation,
            &decl::names(view, node, Some(&d))?,
        )
    }
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/sourcedefinition.go:getSourceDefCheckerInfo
    fn source_checker_info(
        &self,
        c: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<(Vec<Declaration>, Vec<u8>)> {
        let view = self.view(node)?;
        let n = view.node(node)?;
        let property = n.parent().filter(|p| {
            view.node(*p)
                .is_ok_and(|p| ast::is_access_expression(&p) && p.name() == Some(node))
        });
        let mut declarations = self.declarations_at(c, node)?;
        if declarations.is_empty() {
            if let Some(parent) = property {
                if let Some(left) = view.node(parent)?.expression() {
                    let ty = c.get_type_at_location(left)?;
                    if let Some(prop) =
                        c.get_property_of_type(ty, view.node_text(node)?.as_bytes())?
                    {
                        declarations.extend(c.symbol_declarations(prop)?.iter().flatten());
                    }
                }
            }
        }
        if let Some(called) = self.called_declaration(c, node)? {
            let mut keep = Vec::new();
            for d in declarations {
                if !ast::is_function_like(Some(&self.view(d)?.node(d)?)) {
                    keep.push(d);
                }
            }
            keep.push(called);
            declarations = keep;
        }
        let mut resolve = node;
        if let Some(parent) = property {
            if let Some(mut expr) = view.node(parent)?.expression() {
                while ast::is_access_expression(&view.node(expr)?) {
                    let Some(next) = view.node(expr)?.expression() else {
                        break;
                    };
                    expr = next;
                }
                resolve = expr;
            }
        }
        let mut specifier = Vec::new();
        if let Some(symbol) = c.get_symbol_at_location(resolve)? {
            for d in c.symbol_declarations(symbol)?.iter().flatten() {
                let dv = self.view(d)?;
                if !matches!(
                    dv.node(d)?.kind().known(),
                    Some(
                        K::ImportSpecifier
                            | K::ImportClause
                            | K::NamespaceImport
                            | K::ImportEqualsDeclaration
                    )
                ) {
                    continue;
                }
                if let Some(spec) = decl::module_specifier(dv, d)? {
                    specifier = dv.node_text(spec)?.as_bytes().to_vec();
                    break;
                }
            }
        }
        let declarations = declarations
            .into_iter()
            .map(|node| {
                self.program
                    .file_of_node(node)
                    .cloned()
                    .map(|file| Declaration { file, node })
                    .ok_or_else(|| tsr_arena::Error::WrongOwner.into())
            })
            .collect::<Result<_>>()?;
        Ok((declarations, specifier))
    }
    // port: tsc/internal/ls/sourcedefinition.go:LanguageService.ProvideSourceDefinition
    pub fn source_definition(
        &mut self,
        c: &mut impl crate::QueryChecker,
        uri: &lsp::DocumentUri,
        position: &lsp::Position,
        links: bool,
    ) -> Result<lsp::LocationOrLocationsOrDefinitionLinksOrNull> {
        let source = self.file(uri)?;
        let projections = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            position,
            FEATURE_DEFINITION,
        )?;
        let mut all = Vec::new();
        for projection in projections {
            self.check_canceled()?;
            if !projection.mapped.fidelity.is_single_segment() {
                continue;
            }
            let file = self
                .program
                .file_of_node(projection.script)
                .ok_or(tsr_arena::Error::WrongOwner)?
                .clone();
            let view = file.bound().view().ast();
            let mut syntax = Syntax::new(view, projection.script)?;
            let at = i64::from(projection.mapped.position);
            let node = syntax.nav().get_touching_property_name(at)?;
            let mut resolver = Resolver::new(
                self.program,
                syntax.file.file_name(),
                self.cancellation.clone(),
            )?;
            let (origin, declarations) = if node == projection.script {
                let reference = syntax
                    .file
                    .referenced_files()?
                    .iter()
                    .chain(syntax.file.type_reference_directives()?.iter())
                    .chain(syntax.file.lib_reference_directives()?.iter())
                    .find(|r| r.loc.pos() <= at && at <= r.loc.end())
                    .map(|r| r.loc);
                let Some(range) = reference else { continue };
                let Some(name) = self.reference_at(&mut syntax, at)? else {
                    continue;
                };
                let implementation = if tsr_tspath::is_declaration_file_name(&name) {
                    let mode = resolver.mode(&name)?;
                    resolver.implementation_for_declaration_file(&name, mode)?
                } else {
                    name
                };
                let Some(file) = resolver.file(&implementation)? else {
                    continue;
                };
                (
                    self.unrestricted_range(projection.script, range)?.0,
                    vec![decl::entry(file)?],
                )
            } else {
                let origin = self
                    .unrestricted_range(
                        projection.script,
                        TextRange::new(syntax.start(node)?, i64::from(view.node(node)?.end())),
                    )?
                    .0;
                let module = decl::module_specifier(view, node)?;
                let mut implementation = if let Some(spec) = module {
                    resolver.implementation(
                        view.node_text(spec)?.as_bytes(),
                        self.program.usage_resolution_mode(&file, spec)?,
                    )?
                } else {
                    Vec::new()
                };
                let mut found = Vec::new();
                if module == Some(node) && !implementation.is_empty() {
                    if let Some(file) = resolver.file(&implementation)? {
                        found.push(decl::entry(file)?);
                    }
                } else if !implementation.is_empty() {
                    let results = resolver.search(
                        view,
                        node,
                        &implementation,
                        &decl::names(view, node, None)?,
                    )?;
                    if (!pos::is_part_of_type_node(view, node)?
                        && !tsr_ast::utilities_modules::is_part_of_type_only_import_or_export_declaration(view, node)?)
                        || decl::has_concrete(&results)? {
                        found = results;
                    }
                }
                if found.is_empty() && module != Some(node) {
                    let (checked, spec) =
                        c.with_checker(projection.script, |c| self.source_checker_info(c, node))?;
                    if implementation.is_empty() && !spec.is_empty() {
                        let mode = resolver.mode(syntax.file.file_name())?;
                        implementation = resolver.implementation(&spec, mode)?;
                    }
                    if checked.is_empty() && !implementation.is_empty() {
                        found = resolver.search(
                            view,
                            node,
                            &implementation,
                            &decl::names(view, node, None)?,
                        )?;
                    } else {
                        for d in &checked {
                            found.extend(resolver.map_declaration(
                                self,
                                view,
                                node,
                                d.clone(),
                                &implementation,
                            )?);
                        }
                        found = decl::unique(found)?;
                        if !decl::has_concrete(&found)? {
                            found.clear();
                        }
                    }
                    if found.is_empty()
                        && module.is_some()
                        && !implementation.is_empty()
                        && !decl::has_concrete(&checked)?
                    {
                        if let Some(file) = resolver.file(&implementation)? {
                            found.push(decl::entry(file)?);
                        }
                    }
                }
                if found.is_empty() {
                    all.extend(c.with_checker(projection.script, |c| {
                        self.definition_at_position(c, projection.script, at, false)
                    })?);
                    continue;
                }
                (origin, decl::unique(found)?)
            };
            for d in declarations {
                if let Some(link) =
                    self.definition_location_in_file(&origin, &d.file, d.node, FEATURE_DEFINITION)?
                {
                    all.push(Some(Box::new(link)));
                }
            }
        }
        let mut seen = HashSet::new();
        all.retain(|l| {
            l.as_ref().is_some_and(|l| {
                seen.insert((
                    l.target_uri.clone(),
                    l.target_selection_range.start.line,
                    l.target_selection_range.start.character,
                    l.target_selection_range.end.line,
                    l.target_selection_range.end.character,
                ))
            })
        });
        Ok(if links {
            lsp::LocationOrLocationsOrDefinitionLinksOrNull {
                definition_links: Some(Box::new(all)),
                ..Default::default()
            }
        } else {
            lsp::LocationOrLocationsOrDefinitionLinksOrNull {
                locations: Some(Box::new(
                    all.into_iter()
                        .flatten()
                        .map(|l| lsp::Location {
                            uri: l.target_uri,
                            range: l.target_selection_range,
                        })
                        .collect(),
                )),
                ..Default::default()
            }
        })
    }
}
