//! Retained syntax and binder facts for the independent reference frontend.
//!
//! Reads borrow completed files; no read clones an owner or consults a checker.
//! Cross-file symbols remain groups of binder identities. Combining their
//! declarations into a type is the frontend's responsibility.

use std::collections::{HashMap, HashSet};
use ts_arena::{ArenaId, Error as ArenaError, SymbolId};
use ts_ast::{
    symbol_flags as flags, AstView, CompletedFile, DeclarationRead, JsString, NodeBinding,
    NodeDataRead, NodeId, NodeListId, NodeListRead, NodeRead, NodeSlice, NodeSliceRead, NodeText,
    SourceFileRead, SymbolFlags, SymbolRead, SymbolRef, SymbolTableId, SymbolTableRead,
    SyntaxKind as K,
};
use ts_binder::name_resolver::{NameResolver, NoNameResolverHooks, ResolverHost, ResolverOptions};
use ts_core::{CompilerOptions, ResolutionMode, Tristate};

#[derive(Clone, Copy, Debug)]
pub struct BoundInputOptions {
    pub strict_null_checks: bool,
    pub strict_bind_call_apply: bool,
    pub exact_optional_property_types: bool,
    pub resolver: ResolverOptions,
}

impl From<&CompilerOptions> for BoundInputOptions {
    fn from(options: &CompilerOptions) -> Self {
        Self {
            strict_null_checks: options.strict_option_value(options.strict_null_checks),
            strict_bind_call_apply: options.strict_option_value(options.strict_bind_call_apply),
            exact_optional_property_types: options.exact_optional_property_types == Tristate::TRUE,
            resolver: ResolverOptions {
                emit_script_target: options.emit_script_target(),
                isolated_modules: options.isolated_modules(),
                verbatim_module_syntax: options.verbatim_module_syntax == Tristate::TRUE,
                emit_standard_class_fields: options.emit_standard_class_fields(),
            },
        }
    }
}

impl Default for BoundInputOptions {
    fn default() -> Self {
        Self::from(&CompilerOptions::default())
    }
}

/// An already-completed loader decision, without resolved checker types.
#[derive(Clone, Debug)]
pub struct ModuleResolution {
    pub containing_source: NodeId,
    pub specifier: Vec<u8>,
    pub mode: ResolutionMode,
    pub resolved_source: NodeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolGroup {
    pub symbols: Vec<SymbolId>,
}

impl SymbolGroup {
    fn one(symbol: SymbolId) -> Self {
        Self {
            symbols: vec![symbol],
        }
    }

    fn push_unique(&mut self, symbol: SymbolId) {
        if !self.symbols.contains(&symbol) {
            self.symbols.push(symbol);
        }
    }
}

#[derive(Debug)]
pub enum InputError {
    Ast(ArenaError),
    Unsupported(String),
}

impl From<ArenaError> for InputError {
    fn from(error: ArenaError) -> Self {
        Self::Ast(error)
    }
}

impl std::fmt::Display for InputError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ast(error) => write!(out, "bound input: {error}"),
            Self::Unsupported(message) => write!(out, "bound input: {message}"),
        }
    }
}

impl std::error::Error for InputError {}

#[derive(Debug)]
pub struct BoundInput {
    files: Vec<CompletedFile>,
    options: BoundInputOptions,
    nodes: HashMap<ArenaId, usize>,
    symbols: HashMap<ArenaId, usize>,
    tables: HashMap<ArenaId, usize>,
    sources: HashMap<NodeId, usize>,
    globals: HashMap<Vec<u8>, SymbolGroup>,
    modules: Vec<ModuleResolution>,
}

impl BoundInput {
    /// Transfers completed-file retention into this input. Initialization indexes
    /// source locals once. A failed construction publishes no partial input.
    pub fn new(
        files: Vec<CompletedFile>,
        options: BoundInputOptions,
        modules: Vec<ModuleResolution>,
    ) -> Result<Self, InputError> {
        let mut input = Self {
            files,
            options,
            nodes: HashMap::new(),
            symbols: HashMap::new(),
            tables: HashMap::new(),
            sources: HashMap::new(),
            globals: HashMap::new(),
            modules,
        };
        let mut augmentations = Vec::new();
        for (index, file) in input.files.iter().enumerate() {
            let source = file.source();
            let view = file.view();
            view.source_file()?;
            if input.sources.insert(source, index).is_some() {
                return Err(InputError::Unsupported("duplicate completed source".into()));
            }
            input.nodes.entry(source.arena()).or_insert(index);
            input.symbols.insert(view.result().symbols().id(), index);
            input.tables.insert(view.result().tables().id(), index);
            if !view.result().pattern_ambient_modules().is_empty() {
                return Err(InputError::Unsupported("pattern ambient modules".into()));
            }
            augmentations.extend(
                view.source_file()?
                    .module_augmentations()?
                    .iter()
                    .flatten()
                    .copied(),
            );
            if ts_ast::utilities_middle::is_global_source_file(view.ast(), source)? {
                if let Some(table) = view.node_binding(source)?.and_then(|b| b.locals) {
                    for (name, symbol) in view.result().tables().get(table)? {
                        if let Some(symbol) = symbol {
                            input
                                .globals
                                .entry(name.to_vec())
                                .or_insert_with(|| SymbolGroup {
                                    symbols: Vec::new(),
                                })
                                .push_unique(symbol);
                        }
                    }
                }
            }
        }
        for name in augmentations {
            let declaration = input.node(name)?.parent().ok_or(ArenaError::InvalidGraph)?;
            if !ts_ast::utilities::is_global_scope_augmentation(&input.node(declaration)?) {
                return Err(InputError::Unsupported(
                    "external module augmentation".into(),
                ));
            }
            let symbol = input
                .binding(declaration)?
                .and_then(|binding| binding.symbol)
                .ok_or(ArenaError::InvalidGraph)?;
            if input.declarations(symbol)?.get(0).flatten() != Some(declaration) {
                continue;
            }
            if let Some(exports) = input.symbol(symbol)?.exports() {
                let entries = input
                    .table(exports)?
                    .iter()
                    .filter_map(|(name, symbol)| symbol.map(|symbol| (name.to_vec(), symbol)))
                    .collect::<Vec<_>>();
                for (name, symbol) in entries {
                    input
                        .globals
                        .entry(name)
                        .or_insert_with(|| SymbolGroup {
                            symbols: Vec::new(),
                        })
                        .push_unique(symbol);
                }
            }
        }
        let mut module_keys = HashSet::new();
        for module in &input.modules {
            input.require_source(module.containing_source)?;
            input.require_source(module.resolved_source)?;
            if !module_keys.insert((
                module.containing_source,
                module.specifier.as_slice(),
                module.mode,
            )) {
                return Err(InputError::Unsupported(
                    "duplicate module-resolution fact".into(),
                ));
            }
        }
        Ok(input)
    }

    pub fn files(&self) -> &[CompletedFile] {
        &self.files
    }

    pub fn options(&self) -> &BoundInputOptions {
        &self.options
    }

    fn require_source(&self, source: NodeId) -> Result<usize, ArenaError> {
        self.sources
            .get(&source)
            .copied()
            .ok_or(ArenaError::WrongOwner)
    }

    fn node_file(&self, node: NodeId) -> Result<&CompletedFile, ArenaError> {
        if let Some(&index) = self.nodes.get(&node.arena()) {
            return Ok(&self.files[index]);
        }
        // Lazy/imported arenas are not necessarily a source's core arena. This
        // uncommon route still checks the ID through a complete retained root.
        self.files
            .iter()
            .find(|file| file.view().node(node).is_ok())
            .ok_or(ArenaError::WrongOwner)
    }

    pub fn ast(&self, node: NodeId) -> Result<AstView<'_>, ArenaError> {
        let view = self.node_file(node)?.view().ast();
        view.node(node)?;
        Ok(view)
    }

    pub fn node(&self, node: NodeId) -> Result<NodeRead<'_>, ArenaError> {
        self.node_file(node)?.view().node(node)
    }

    pub fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, ArenaError> {
        self.node_file(node)?.view().node_binding(node)
    }

    pub fn symbol(&self, symbol: SymbolId) -> Result<SymbolRead<'_>, ArenaError> {
        let &index = self
            .symbols
            .get(&symbol.arena())
            .ok_or(ArenaError::WrongOwner)?;
        self.files[index].view().symbol(symbol)
    }

    pub fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, ArenaError> {
        let &index = self
            .tables
            .get(&table.arena())
            .ok_or(ArenaError::WrongOwner)?;
        self.files[index].view().result().tables().get(table)
    }

    pub fn declarations(&self, symbol: SymbolId) -> Result<DeclarationRead<'_>, ArenaError> {
        let &index = self
            .symbols
            .get(&symbol.arena())
            .ok_or(ArenaError::WrongOwner)?;
        let view = self.files[index].view();
        view.result()
            .declarations()
            .get(view.symbol(symbol)?.declarations())
    }

    pub fn list(&self, owner: NodeId, list: NodeListId) -> Result<NodeListRead<'_>, ArenaError> {
        self.ast(owner)?.list(list)
    }

    pub fn nodes(&self, owner: NodeId, nodes: NodeSlice) -> Result<NodeSliceRead<'_>, ArenaError> {
        self.ast(owner)?.node_slice(nodes)
    }

    pub fn text(&self, node: NodeId) -> Result<NodeText<'_>, ArenaError> {
        self.ast(node)?.node_text(node)
    }

    pub fn source(&self, node: NodeId) -> Result<NodeId, ArenaError> {
        let mut current = node;
        let mut seen = HashSet::new();
        loop {
            if self.sources.contains_key(&current) {
                return Ok(current);
            }
            if !seen.insert(current) {
                return Err(ArenaError::InvalidGraph);
            }
            current = self
                .node(current)?
                .parent()
                .ok_or(ArenaError::InvalidGraph)?;
        }
    }

    pub fn source_file(&self, node: NodeId) -> Result<SourceFileRead<'_>, ArenaError> {
        let source = self.source(node)?;
        self.files[self.require_source(source)?]
            .view()
            .source_file()
    }

    /// Direct top-level declarations only; the caller chooses the fixture file.
    /// Returning a declaration from another global source would lose provenance.
    pub fn declaration_by_name(
        &self,
        source: NodeId,
        name: &[u8],
    ) -> Result<Option<NodeId>, InputError> {
        self.require_source(source)?;
        let Some(table) = self.binding(source)?.and_then(|b| b.locals) else {
            return Ok(None);
        };
        let Some(symbol) = self.table(table)?.get(name).flatten() else {
            return Ok(None);
        };
        for declaration in self.declarations(symbol)?.iter().flatten() {
            if self.source(declaration)? == source {
                return Ok(Some(declaration));
            }
        }
        Ok(None)
    }

    /// Byte offsets of the declaration name, excluding leading trivia.
    pub fn declaration_name_span(&self, declaration: NodeId) -> Result<(i32, i32), InputError> {
        let node = self.node(declaration)?;
        let name = node
            .name()
            .ok_or_else(|| InputError::Unsupported("declaration has no name".into()))?;
        let name = self.node(name)?;
        let source = self.source_file(declaration)?;
        let start = ts_scanner::skip_trivia(source.text().as_bytes(), i64::from(name.pos()));
        let start = i32::try_from(start).map_err(|_| ArenaError::InvalidSlot)?;
        Ok((start, name.end()))
    }

    /// Lexical rules are the pinned binder resolver, with its checker hooks
    /// absent. Aliases are returned only when `meaning` includes `ALIAS`.
    /// Global declaration merging is represented explicitly as a SymbolGroup.
    pub fn resolve_name(
        &self,
        location: NodeId,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolGroup>, InputError> {
        self.node(location)?;
        let mut resolver = NameResolver::new(self.options.resolver);
        let mut host = BorrowedResolverHost {
            input: self,
            requested_transient: false,
        };
        let resolved = resolver.resolve(
            &mut host,
            &mut NoNameResolverHooks,
            Some(location),
            name,
            meaning,
            None,
            false,
            true,
        );
        if host.requested_transient {
            return Err(InputError::Unsupported(
                "synthesized binder value symbol in reference input".into(),
            ));
        }
        if let Some(symbol) = resolved? {
            return Ok(Some(SymbolGroup::one(symbol)));
        }
        self.resolve_global(name, meaning)
    }

    pub fn resolve_global(
        &self,
        name: &[u8],
        meaning: SymbolFlags,
    ) -> Result<Option<SymbolGroup>, InputError> {
        let Some(global) = self.globals.get(name) else {
            return Ok(None);
        };
        let mut group = SymbolGroup {
            symbols: Vec::new(),
        };
        for &symbol in &global.symbols {
            if self.symbol(symbol)?.flags() & meaning != 0 {
                group.push_unique(symbol);
            }
        }
        Ok((!group.symbols.is_empty()).then_some(group))
    }

    pub fn resolve_type_name(&self, name: NodeId) -> Result<Option<SymbolGroup>, InputError> {
        self.resolve_entity_name(name, &mut HashSet::new())
    }

    pub fn resolve_value_name(&self, name: NodeId) -> Result<Option<SymbolGroup>, InputError> {
        let node = self.node(name)?;
        let mut aliases = HashSet::new();
        let group = match node.data() {
            NodeDataRead::Identifier(_) => self.resolve_name(
                name,
                self.text(name)?.as_bytes(),
                flags::VALUE | flags::ALIAS,
            )?,
            NodeDataRead::QualifiedName(data) => {
                let Some(left) =
                    self.resolve_value_name(data.left().ok_or(ArenaError::InvalidGraph)?)?
                else {
                    return Ok(None);
                };
                self.export_named(
                    &left,
                    self.text(data.right().ok_or(ArenaError::InvalidGraph)?)?
                        .as_bytes(),
                    &mut aliases,
                )?
            }
            _ => {
                return Err(InputError::Unsupported(format!(
                    "value name syntax {:?}",
                    node.kind()
                )));
            }
        };
        group
            .map(|group| self.follow_aliases(group, &mut aliases))
            .transpose()
    }

    fn resolve_entity_name(
        &self,
        name: NodeId,
        aliases: &mut HashSet<SymbolId>,
    ) -> Result<Option<SymbolGroup>, InputError> {
        let node = self.node(name)?;
        let group = match node.data() {
            NodeDataRead::Identifier(_) => self.resolve_name(
                name,
                self.text(name)?.as_bytes(),
                flags::TYPE | flags::NAMESPACE | flags::ALIAS,
            )?,
            NodeDataRead::QualifiedName(data) => {
                let left = data.left().ok_or(ArenaError::InvalidGraph)?;
                let right = data.right().ok_or(ArenaError::InvalidGraph)?;
                let Some(left) = self.resolve_entity_name(left, aliases)? else {
                    return Ok(None);
                };
                self.export_named(&left, self.text(right)?.as_bytes(), aliases)?
            }
            NodeDataRead::PropertyAccessExpression(data) => {
                let left = data.expression().ok_or(ArenaError::InvalidGraph)?;
                let right = data.name().ok_or(ArenaError::InvalidGraph)?;
                let Some(left) = self.resolve_entity_name(left, aliases)? else {
                    return Ok(None);
                };
                self.export_named(&left, self.text(right)?.as_bytes(), aliases)?
            }
            _ => {
                return Err(InputError::Unsupported(format!(
                    "entity name syntax {:?}",
                    node.kind()
                )));
            }
        };
        group
            .map(|group| self.follow_aliases(group, aliases))
            .transpose()
    }

    /// Exact loader facts are available when the caller has the usage mode.
    pub fn module_source(
        &self,
        containing_source: NodeId,
        specifier: &[u8],
        mode: ResolutionMode,
    ) -> Result<Option<NodeId>, InputError> {
        self.require_source(containing_source)?;
        Ok(self
            .modules
            .iter()
            .find(|entry| {
                entry.containing_source == containing_source
                    && entry.specifier == specifier
                    && entry.mode == mode
            })
            .map(|entry| entry.resolved_source))
    }

    fn module_group(&self, usage: NodeId, specifier: NodeId) -> Result<SymbolGroup, InputError> {
        let source = self.source(usage)?;
        let text = self.text(specifier)?;
        let mut target = None;
        for entry in self
            .modules
            .iter()
            .filter(|entry| entry.containing_source == source && entry.specifier == text.as_bytes())
        {
            if target.is_some_and(|previous| previous != entry.resolved_source) {
                return Err(InputError::Unsupported(format!(
                    "module {:?} needs a usage-specific resolution mode",
                    String::from_utf8_lossy(text.as_bytes())
                )));
            }
            target = Some(entry.resolved_source);
        }
        if target.is_none() {
            let mut ambient_name = Vec::with_capacity(text.len() + 2);
            ambient_name.push(b'"');
            ambient_name.extend_from_slice(text.as_bytes());
            ambient_name.push(b'"');
            if let Some(ambient) = self.resolve_global(&ambient_name, flags::MODULE)? {
                return Ok(ambient);
            }
        }
        let target = target.ok_or_else(|| {
            InputError::Unsupported(format!(
                "missing loader resolution for {:?}",
                String::from_utf8_lossy(text.as_bytes())
            ))
        })?;
        let symbol = self
            .binding(target)?
            .and_then(|binding| binding.symbol)
            .ok_or_else(|| {
                InputError::Unsupported("resolved source has no external-module symbol".into())
            })?;
        Ok(SymbolGroup::one(symbol))
    }

    fn export_named(
        &self,
        module: &SymbolGroup,
        name: &[u8],
        aliases: &mut HashSet<SymbolId>,
    ) -> Result<Option<SymbolGroup>, InputError> {
        let mut found = SymbolGroup {
            symbols: Vec::new(),
        };
        for &symbol in &module.symbols {
            if let Some(table) = self.symbol(symbol)?.exports() {
                if let Some(symbol) = self.table(table)?.get(name).flatten() {
                    found.push_unique(symbol);
                }
                if self
                    .table(table)?
                    .contains_key(ts_ast::internal_symbol_names::EXPORT_STAR)
                {
                    return Err(InputError::Unsupported(
                        "export-star resolution in reference input".into(),
                    ));
                }
            }
        }
        if found.symbols.is_empty() {
            Ok(None)
        } else {
            self.follow_aliases(found, aliases).map(Some)
        }
    }

    fn follow_aliases(
        &self,
        group: SymbolGroup,
        aliases: &mut HashSet<SymbolId>,
    ) -> Result<SymbolGroup, InputError> {
        let mut result = SymbolGroup {
            symbols: Vec::new(),
        };
        for symbol in group.symbols {
            if self.symbol(symbol)?.flags() & flags::ALIAS == 0 {
                result.push_unique(symbol);
                continue;
            }
            if !aliases.insert(symbol) {
                return Err(InputError::Unsupported("cyclic import/export alias".into()));
            }
            let declarations = self.declarations(symbol)?;
            if declarations.len() != 1 {
                return Err(InputError::Unsupported(
                    "alias without one bound declaration".into(),
                ));
            }
            let declaration = declarations
                .get(0)
                .flatten()
                .ok_or(ArenaError::InvalidGraph)?;
            let target = self.alias_target(declaration, aliases)?;
            for target in target.symbols {
                result.push_unique(target);
            }
            aliases.remove(&symbol);
        }
        Ok(result)
    }

    fn alias_target(
        &self,
        declaration: NodeId,
        aliases: &mut HashSet<SymbolId>,
    ) -> Result<SymbolGroup, InputError> {
        let node = self.node(declaration)?;
        match node.data() {
            NodeDataRead::ImportEqualsDeclaration(data) => {
                let reference = data.module_reference().ok_or(ArenaError::InvalidGraph)?;
                let reference_node = self.node(reference)?;
                if let NodeDataRead::ExternalModuleReference(data) = reference_node.data() {
                    let module = self.module_group(
                        declaration,
                        data.expression().ok_or(ArenaError::InvalidGraph)?,
                    )?;
                    return Ok(self
                        .export_named(&module, b"export=", aliases)?
                        .unwrap_or(module));
                }
                self.resolve_entity_name(reference, aliases)?
                    .ok_or_else(|| InputError::Unsupported("unresolved import-equals name".into()))
            }
            NodeDataRead::ImportSpecifier(data) => {
                let name = data
                    .property_name()
                    .or(data.name())
                    .ok_or(ArenaError::InvalidGraph)?;
                let module = self.enclosing_import_module(declaration)?;
                self.required_export(&module, self.text(name)?.as_bytes(), aliases)
            }
            NodeDataRead::ImportClause(_) => {
                let module = self.enclosing_import_module(declaration)?;
                self.required_export(&module, b"default", aliases)
            }
            NodeDataRead::NamespaceImport(_) => self.enclosing_import_module(declaration),
            NodeDataRead::ExportSpecifier(data) => {
                let name = data
                    .property_name()
                    .or(data.name())
                    .ok_or(ArenaError::InvalidGraph)?;
                let export = node
                    .parent()
                    .and_then(|parent| self.node(parent).ok()?.parent())
                    .ok_or(ArenaError::InvalidGraph)?;
                let export_node = self.node(export)?;
                let NodeDataRead::ExportDeclaration(data) = export_node.data() else {
                    return Err(ArenaError::InvalidGraph.into());
                };
                if let Some(specifier) = data.module_specifier() {
                    let module = self.module_group(export, specifier)?;
                    self.required_export(&module, self.text(name)?.as_bytes(), aliases)
                } else {
                    self.resolve_entity_name(name, aliases)?.ok_or_else(|| {
                        InputError::Unsupported("unresolved local export alias".into())
                    })
                }
            }
            _ => Err(InputError::Unsupported(format!(
                "alias declaration syntax {:?}",
                node.kind()
            ))),
        }
    }

    fn required_export(
        &self,
        module: &SymbolGroup,
        name: &[u8],
        aliases: &mut HashSet<SymbolId>,
    ) -> Result<SymbolGroup, InputError> {
        self.export_named(module, name, aliases)?.ok_or_else(|| {
            InputError::Unsupported(format!(
                "missing module export {:?}",
                String::from_utf8_lossy(name)
            ))
        })
    }

    fn enclosing_import_module(&self, declaration: NodeId) -> Result<SymbolGroup, InputError> {
        let mut current = declaration;
        while let Some(parent) = self.node(current)?.parent() {
            let node = self.node(parent)?;
            if let NodeDataRead::ImportDeclaration(data) = node.data() {
                return self.module_group(
                    parent,
                    data.module_specifier().ok_or(ArenaError::InvalidGraph)?,
                );
            }
            if node.kind() == K::SourceFile {
                break;
            }
            current = parent;
        }
        Err(InputError::Unsupported(
            "import alias has no import declaration".into(),
        ))
    }
}

struct BorrowedResolverHost<'a> {
    input: &'a BoundInput,
    requested_transient: bool,
}

impl ResolverHost for BorrowedResolverHost<'_> {
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, ArenaError> {
        self.input.ast(node)
    }
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, ArenaError> {
        self.input.binding(node)
    }
    fn symbol(&self, symbol: SymbolId) -> Result<SymbolRef<'_>, ArenaError> {
        self.input.symbol(symbol).map(SymbolRef::Stored)
    }
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, ArenaError> {
        self.input.table(table)
    }
    fn declarations(&self, symbol: SymbolId) -> Result<DeclarationRead<'_>, ArenaError> {
        self.input.declarations(symbol)
    }
    fn new_transient_symbol(
        &mut self,
        _: SymbolFlags,
        _: JsString,
    ) -> Result<SymbolId, ArenaError> {
        self.requested_transient = true;
        Err(ArenaError::InvalidGraph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_ast::SourceFileParseOptions;
    use ts_core::{ModuleKind, ScriptKind};
    use ts_jsstring::SourceText;

    fn bind(path: &[u8], text: &[u8]) -> CompletedFile {
        ts_binder::bind_parsed_file(ts_parser::parse_source_file(
            SourceText::from_loaded_bytes(text),
            ScriptKind::TS,
            SourceFileParseOptions {
                file_name: JsString::from_bytes(path),
                ..Default::default()
            },
        ))
        .unwrap()
    }

    fn type_name(input: &BoundInput, source: NodeId, name: &[u8]) -> NodeId {
        let declaration = input.declaration_by_name(source, name).unwrap().unwrap();
        let ty = input.node(declaration).unwrap().type_node().unwrap();
        match input.node(ty).unwrap().data() {
            NodeDataRead::TypeReferenceNode(data) => data.type_name().unwrap(),
            other => panic!("expected reference, got {other:?}"),
        }
    }

    #[test]
    fn retains_files_groups_globals_and_respects_type_parameter_scope() {
        let library_a = bind(b"/lib.a.d.ts", b"interface Collection<T> { value: T }");
        let library_b = bind(
            b"/lib.b.d.ts",
            b"interface Collection<T> { length: number }",
        );
        let fixture = bind(
            b"/fixture.ts",
            b"type A = Collection<string>; type B<Collection> = Collection;",
        );
        let source = fixture.source();
        let input = BoundInput::new(
            vec![library_a, library_b, fixture.clone()],
            BoundInputOptions::default(),
            Vec::new(),
        )
        .unwrap();
        drop(fixture);
        let global = input
            .resolve_type_name(type_name(&input, source, b"A"))
            .unwrap()
            .unwrap();
        assert_eq!(global.symbols.len(), 2);
        assert_eq!(
            global
                .symbols
                .iter()
                .map(|&symbol| input.declarations(symbol).unwrap().len())
                .sum::<usize>(),
            2
        );
        let local = input
            .resolve_type_name(type_name(&input, source, b"B"))
            .unwrap()
            .unwrap();
        assert_eq!(local.symbols.len(), 1);
        assert_ne!(
            input.symbol(local.symbols[0]).unwrap().flags() & flags::TYPE_PARAMETER,
            0
        );
        let declaration = input.declaration_by_name(source, b"A").unwrap().unwrap();
        assert_eq!(input.declaration_name_span(declaration).unwrap(), (5, 6));
        let foreign = bind(b"/foreign.ts", b"type Foreign = number;");
        assert!(matches!(
            input.node(foreign.source()),
            Err(ArenaError::WrongOwner)
        ));
    }

    #[test]
    fn follows_named_and_namespace_imports_using_loader_facts() {
        let module = bind(b"/module.ts", b"export interface Shape { x: number }");
        let module_source = module.source();
        let fixture = bind(b"/fixture.ts", b"import { Shape as Imported } from './module'; import * as ns from './module'; type A = Imported; type B = ns.Shape;");
        let source = fixture.source();
        let input = BoundInput::new(
            vec![module, fixture],
            BoundInputOptions::default(),
            vec![ModuleResolution {
                containing_source: source,
                specifier: b"./module".to_vec(),
                mode: ModuleKind::ESNEXT,
                resolved_source: module_source,
            }],
        )
        .unwrap();
        let named = input
            .resolve_type_name(type_name(&input, source, b"A"))
            .unwrap()
            .unwrap();
        let namespaced = input
            .resolve_type_name(type_name(&input, source, b"B"))
            .unwrap()
            .unwrap();
        assert_eq!(named, namespaced);
        let declaration = input
            .declarations(named.symbols[0])
            .unwrap()
            .get(0)
            .flatten()
            .unwrap();
        assert_eq!(input.source(declaration).unwrap(), module_source);
        assert_eq!(
            input.node(declaration).unwrap().kind(),
            K::InterfaceDeclaration
        );
    }

    #[test]
    fn missing_and_duplicate_module_facts_fail_explicitly() {
        let module = bind(b"/module.ts", b"export type Shape = number;");
        let fixture = bind(
            b"/fixture.ts",
            b"import { Shape } from './module'; type A = Shape;",
        );
        let source = fixture.source();
        let no_facts = BoundInput::new(
            vec![module.clone(), fixture.clone()],
            BoundInputOptions::default(),
            Vec::new(),
        )
        .unwrap();
        let error = no_facts
            .resolve_type_name(type_name(&no_facts, source, b"A"))
            .unwrap_err();
        assert!(error.to_string().contains("missing loader resolution"));
        let fact = ModuleResolution {
            containing_source: source,
            specifier: b"./module".to_vec(),
            mode: ModuleKind::ESNEXT,
            resolved_source: module.source(),
        };
        let error = BoundInput::new(
            vec![module, fixture],
            BoundInputOptions::default(),
            vec![fact.clone(), fact],
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("duplicate module-resolution fact"));
    }
}
