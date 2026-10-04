//! Import/export traversal shared by the reference search. It follows aliases
//! through re-exports without folding distinct local import symbols together.
use crate::{
    documentation::list,
    reference_helpers as h,
    references::{From, Search, SearchState},
    Result,
};
use std::collections::{HashMap, HashSet};
use tsr_ast::{
    symbol_flags as sf, utilities as ast, utilities_modules as modules, NodeId, SymbolTableId,
    SyntaxKind as K,
};
use tsr_checker::SymbolRef;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Named,
    Default,
    Equals,
}
#[derive(Clone, Copy)]
pub(crate) struct ExportInfo {
    pub module: SymbolRef,
    pub kind: ExportKind,
}
#[derive(Default)]
struct Importers {
    direct: Vec<NodeId>,
    indirect: Vec<NodeId>,
    seen_direct: HashSet<NodeId>,
    seen_indirect: HashSet<NodeId>,
    global: bool,
}
impl SearchState<'_, '_, '_> {
    pub(crate) fn table_entry(
        &self,
        table: Option<SymbolTableId>,
        name: &[u8],
    ) -> Result<Option<SymbolRef>> {
        let Some(table) = table else { return Ok(None) };
        self.c
            .symbol_table(table)?
            .get(name)
            .flatten()
            .map(|s| self.c.symbol_ref(s).map_err(Into::into))
            .transpose()
    }
    fn source_like(&self, node: NodeId) -> Result<NodeId> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        if matches!(
            n.kind().known(),
            Some(K::CallExpression | K::JSDocImportTag)
        ) {
            return self.source_of(node);
        }
        let parent = n.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
        if view.node(parent)?.kind() == K::SourceFile {
            return Ok(parent);
        }
        Ok(view
            .node(parent)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?)
    }
    fn ambient(&self, node: NodeId) -> Result<bool> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        Ok(n.kind() == K::ModuleDeclaration
            && n.name()
                .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::StringLiteral)))
    }
    fn import_statements(&self, source: NodeId) -> Result<Vec<NodeId>> {
        let view = self.l.view(source)?;
        let n = view.node(source)?;
        let root = if n.kind() == K::SourceFile {
            source
        } else {
            let Some(body) = n.body() else {
                return Ok(Vec::new());
            };
            body
        };
        let mut pending = list(view, view.node(root)?.statement_list())?;
        pending.reverse();
        let mut result = Vec::new();
        while let Some(node) = pending.pop() {
            result.push(node);
            if self.ambient(node)? {
                if let Some(body) = view.node(node)?.body() {
                    pending.extend(
                        list(view, view.node(body)?.statement_list())?
                            .into_iter()
                            .rev(),
                    );
                }
            }
        }
        Ok(result)
    }
    // port: tsc/internal/ls/importTracker.go:forEachImport
    pub(crate) fn file_imports(&self, source: NodeId) -> Result<Vec<(NodeId, NodeId)>> {
        let view = self.l.view(source)?;
        let file = view.source_file(source)?;
        let imports = file.imports()?;
        let mut result = Vec::new();
        if file.external_module_indicator.is_some() || !imports.is_empty() {
            for &node in imports.iter().flatten() {
                if let Some(import) = modules::import_from_module_specifier(view, node)? {
                    result.push((import, node));
                }
            }
        } else {
            for node in self.import_statements(source)? {
                let n = view.node(node)?;
                if matches!(
                    n.kind().known(),
                    Some(K::ExportDeclaration | K::ImportDeclaration | K::JSImportDeclaration)
                ) {
                    if let Some(specifier) = n
                        .module_specifier()
                        .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::StringLiteral))
                    {
                        result.push((node, specifier));
                    }
                } else if n.kind() == K::ImportEqualsDeclaration {
                    if let Some(specifier) = self.external_equals(node)? {
                        result.push((node, specifier));
                    }
                }
            }
        }
        Ok(result)
    }
    fn external_equals(&self, node: NodeId) -> Result<Option<NodeId>> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        let Some(module) = n
            .data_source()
            .as_import_equals_declaration()
            .and_then(|d| d.module_reference())
        else {
            return Ok(None);
        };
        let n = view.node(module)?;
        Ok(if n.kind() == K::ExternalModuleReference {
            n.expression()
                .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::StringLiteral))
        } else {
            None
        })
    }
    // port: tsc/internal/ls/importTracker.go:getDirectImportsMap
    fn prepare_imports(&mut self) -> Result<()> {
        if self.imports.is_some() {
            return Ok(());
        }
        let mut imports: HashMap<SymbolRef, Vec<NodeId>> = HashMap::new();
        for source in self.files.clone() {
            self.l.check_canceled()?;
            for (import, literal) in self.file_imports(source)? {
                if let Some(symbol) = self.c.get_symbol_at_location(literal)? {
                    imports.entry(symbol).or_default().push(import);
                }
            }
        }
        self.imports = Some(imports);
        Ok(())
    }
    fn module_symbol(&mut self, node: NodeId) -> Result<Option<SymbolRef>> {
        let source = self.source_like(node)?;
        self.c
            .bound_symbol_of_node(source)?
            .map(|s| self.c.get_merged_symbol(s).map_err(Into::into))
            .transpose()
    }
    fn exported(&self, mut node: NodeId, stop_at_ambient: bool) -> Result<bool> {
        let view = self.l.view(node)?;
        loop {
            if stop_at_ambient && self.ambient(node)? {
                return Ok(false);
            }
            if ast::has_syntactic_modifier(view, node, tsr_ast::modifier_flags::EXPORT)? {
                return Ok(true);
            }
            let Some(p) = view.node(node)?.parent() else {
                return Ok(false);
            };
            node = p;
        }
    }
    // port: tsc/internal/ls/importTracker.go:findNamespaceReExports
    fn namespace_reexports(&mut self, source: NodeId, name: NodeId) -> Result<bool> {
        let symbol = self.c.get_symbol_at_location(name)?;
        let view = self.l.view(source)?;
        for node in self.import_statements(source)? {
            let n = view.node(node)?;
            if let Some(d) = n.data_source().as_export_declaration() {
                if d.module_specifier().is_none() {
                    if let Some(clause) = d
                        .export_clause()
                        .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::NamedExports))
                    {
                        for element in list(view, view.node(clause)?.element_list())? {
                            if self.c.get_export_specifier_local_target_symbol(element)? == symbol {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
        Ok(false)
    }
    fn indirect_user(
        &mut self,
        out: &mut Importers,
        source: NodeId,
        transitive: bool,
    ) -> Result<()> {
        if out.global || !out.seen_indirect.insert(source) {
            return Ok(());
        }
        out.indirect.push(source);
        if !transitive {
            return Ok(());
        }
        if let Some(symbol) = self.c.bound_symbol_of_node(source)? {
            let symbol = self.c.get_merged_symbol(symbol)?;
            for direct in self
                .imports
                .as_ref()
                .and_then(|m| m.get(&symbol))
                .cloned()
                .unwrap_or_default()
            {
                if self.c.node(direct)?.kind() != K::ImportType {
                    let next = self.source_like(direct)?;
                    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                        self.indirect_user(out, next, true)
                    })?;
                }
            }
        }
        Ok(())
    }
    fn namespace_import(
        &mut self,
        out: &mut Importers,
        info: ExportInfo,
        decl: NodeId,
        name: NodeId,
        reexport: bool,
        added: bool,
    ) -> Result<()> {
        if info.kind == ExportKind::Equals {
            if !added {
                out.direct.push(decl);
            }
            return Ok(());
        }
        if !out.global {
            let source = self.source_like(decl)?;
            let transitive = reexport || self.namespace_reexports(source, name)?;
            self.indirect_user(out, source, transitive)?;
        }
        Ok(())
    }
    fn direct_imports(
        &mut self,
        out: &mut Importers,
        info: ExportInfo,
        module: SymbolRef,
    ) -> Result<()> {
        for direct in self
            .imports
            .as_ref()
            .and_then(|m| m.get(&module))
            .cloned()
            .unwrap_or_default()
        {
            if !out.seen_direct.insert(direct) {
                continue;
            }
            let view = self.l.view(direct)?;
            let n = view.node(direct)?;
            match n.kind().known() {
                Some(K::CallExpression) => {
                    if tsr_ast::utilities_positions::is_import_call(view, direct)? {
                        let mut top = None;
                        let mut current = Some(direct);
                        while let Some(id) = current {
                            if self.ambient(id)? {
                                top = Some(id);
                                break;
                            }
                            current = view.node(id)?.parent();
                        }
                        let top = top.unwrap_or(self.source_of(direct)?);
                        self.indirect_user(out, top, self.exported(direct, true)?)?;
                    } else if !out.global && info.kind == ExportKind::Equals {
                        if let Some(parent) = n.parent() {
                            let p = view.node(parent)?;
                            if p.kind() == K::VariableDeclaration {
                                if let Some(name) = p.name().filter(|id| {
                                    view.node(*id).is_ok_and(|n| n.kind() == K::Identifier)
                                }) {
                                    out.direct.push(name);
                                }
                            }
                        }
                    }
                }
                Some(K::Identifier) => {}
                Some(K::ImportEqualsDeclaration) => {
                    if let Some(name) = n.name() {
                        self.namespace_import(
                            out,
                            info,
                            direct,
                            name,
                            ast::has_syntactic_modifier(
                                view,
                                direct,
                                tsr_ast::modifier_flags::EXPORT,
                            )?,
                            false,
                        )?;
                    }
                }
                Some(K::ImportDeclaration | K::JSImportDeclaration | K::JSDocImportTag) => {
                    out.direct.push(direct);
                    if let Some(clause) = n.import_clause() {
                        let c = view.node(clause)?;
                        let data = c.data_source().as_import_clause().unwrap();
                        if let Some(bindings) = data.named_bindings().filter(|id| {
                            view.node(*id).is_ok_and(|n| n.kind() == K::NamespaceImport)
                        }) {
                            if let Some(name) = view.node(bindings)?.name() {
                                self.namespace_import(out, info, direct, name, false, true)?;
                            }
                            continue;
                        }
                        if !out.global && c.name().is_some() {
                            let source = self.source_like(direct)?;
                            self.indirect_user(out, source, false)?;
                        }
                    }
                }
                Some(K::ExportDeclaration) => {
                    let clause = n
                        .data_source()
                        .as_export_declaration()
                        .unwrap()
                        .export_clause();
                    if let Some(clause) = clause {
                        if view.node(clause)?.kind() == K::NamespaceExport {
                            let source = self.source_like(direct)?;
                            self.indirect_user(out, source, true)?;
                        } else {
                            out.direct.push(direct);
                        }
                    } else if let Some(module) = self.module_symbol(direct)? {
                        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                            self.direct_imports(out, info, module)
                        })?;
                    }
                }
                Some(K::ImportType) => {
                    let d = n.data_source().as_import_type_node().unwrap();
                    if !out.global
                        && d.is_type_of()
                        && d.qualifier().is_none()
                        && self.exported(direct, false)?
                    {
                        self.indirect_user(out, self.source_of(direct)?, true)?;
                    }
                    out.direct.push(direct);
                }
                _ => {
                    return Err(tsr_astnav::Error::Assertion(format!(
                        "Unexpected import kind: {:?}",
                        n.kind()
                    ))
                    .into())
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/importTracker.go:getImportersForExport
    fn importers(&mut self, info: ExportInfo) -> Result<Importers> {
        self.prepare_imports()?;
        let mut out = Importers {
            global: self.global_exports(self.c.symbol(info.module)?.value_declaration())?,
            ..Default::default()
        };
        self.direct_imports(&mut out, info, info.module)?;
        if out.global {
            out.indirect.clone_from(&self.files);
        } else {
            for decl in h::declarations(self.c, info.module)? {
                let view = self.l.view(decl)?;
                if modules::is_external_module_augmentation(view, decl)? && self.includes(decl)? {
                    self.indirect_user(&mut out, decl, false)?;
                }
            }
            out.indirect = out
                .indirect
                .into_iter()
                .map(|n| self.source_of(n))
                .collect::<Result<_>>()?;
        }
        Ok(out)
    }
    // port: tsc/internal/ls/findallreferences.go:refState.searchForImportsOfExport
    pub(crate) fn imports_of_export(&mut self, symbol: SymbolRef, info: ExportInfo) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let imports = self.importers(info)?;
            let mut searches = Vec::new();
            let mut singles = Vec::new();
            let name = self.c.symbol(symbol)?.name_bytes().to_vec();
            // getSearchesFromDirectImports: collect both lists before executing
            // any search, since aliases may recurse back into this tracker.
            for direct in imports.direct {
                let view = self.l.view(direct)?;
                let n = view.node(direct)?;
                if n.kind() == K::ImportEqualsDeclaration || n.kind() == K::Identifier {
                    if info.kind == ExportKind::Equals
                        && (n.kind() == K::Identifier || self.external_equals(direct)?.is_some())
                    {
                        if let Some(location) = if n.kind() == K::Identifier {
                            Some(direct)
                        } else {
                            n.name()
                        } {
                            if let Some(symbol) = self.c.get_symbol_at_location(location)? {
                                searches.push((location, symbol));
                            }
                        }
                    }
                    continue;
                }
                if n.kind() == K::ImportType {
                    let d = n.data_source().as_import_type_node().unwrap();
                    if let Some(mut qualifier) = d.qualifier() {
                        while let Some(left) = view
                            .node(qualifier)?
                            .data_source()
                            .as_qualified_name()
                            .and_then(|d| d.left())
                        {
                            qualifier = left;
                        }
                        if view.node_text(qualifier)?.as_bytes() == name {
                            singles.push(qualifier);
                        }
                    } else if info.kind == ExportKind::Equals {
                        if let Some(argument) = d.argument() {
                            if let Some(literal) = view
                                .node(argument)?
                                .data_source()
                                .as_literal_type_node()
                                .and_then(|d| d.literal())
                            {
                                singles.push(literal);
                            }
                        }
                    }
                    continue;
                }
                if !n
                    .module_specifier()
                    .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::StringLiteral))
                {
                    continue;
                }
                let mut bindings = None;
                let mut default_import = None;
                if let Some(d) = n.data_source().as_export_declaration() {
                    bindings = d
                        .export_clause()
                        .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::NamedExports));
                } else if let Some(clause) = n.import_clause() {
                    let c = view.node(clause)?;
                    let d = c.data_source().as_import_clause().unwrap();
                    if let Some(b) = d.named_bindings() {
                        match view.node(b)?.kind().known() {
                            Some(K::NamespaceImport) if info.kind == ExportKind::Equals => {
                                if let Some(location) = view.node(b)?.name() {
                                    if let Some(symbol) = self.c.get_symbol_at_location(location)? {
                                        searches.push((location, symbol));
                                    }
                                }
                            }
                            Some(K::NamedImports) if info.kind != ExportKind::Equals => {
                                bindings = Some(b);
                            }
                            _ => {}
                        }
                    }
                    if info.kind != ExportKind::Named {
                        default_import = c.name();
                    }
                }
                if let Some(bindings) = bindings {
                    for element in list(view, view.node(bindings)?.element_list())? {
                        let e = view.node(element)?;
                        let Some(location) = e.name() else { continue };
                        let original = e.property_name().unwrap_or(location);
                        let text = view.node_text(original)?;
                        if text.as_bytes() == name
                            || info.kind != ExportKind::Named && text.as_bytes() == b"default"
                        {
                            if e.property_name().is_some() {
                                singles.push(original);
                            }
                            if let Some(symbol) = self.c.get_symbol_at_location(location)? {
                                searches.push((location, symbol));
                            }
                        }
                    }
                }
                if let Some(location) = default_import {
                    if let Some(symbol) = self.c.get_symbol_at_location(location)? {
                        searches.push((location, symbol));
                    }
                }
            }
            for node in singles {
                if crate::meaning::meaning(self.l.view(node)?, node, self.c)? & self.meaning != 0 {
                    self.append_single(node, symbol)?;
                }
            }
            for (node, symbol) in searches {
                let search =
                    self.create_search(symbol, Some(node), From::Export, None, Vec::new())?;
                let source = self.source_of(node)?;
                self.in_container(source, source, &search, true)?;
            }
            if info.kind != ExportKind::Equals {
                let text = (info.kind == ExportKind::Default).then(|| b"default".to_vec());
                let search = self.create_search(symbol, None, From::Export, text, Vec::new())?;
                for source in imports.indirect {
                    self.in_container(source, source, &search, true)?;
                }
            }
            Ok(())
        })
    }
    fn append_single(&mut self, node: NodeId, symbol: SymbolRef) -> Result<()> {
        self.append(node, symbol)
    }
    // port: tsc/internal/ls/importTracker.go:getExportInfo
    fn export_info(&self, symbol: SymbolRef, kind: ExportKind) -> Result<Option<ExportInfo>> {
        let Some(parent) = h::parent_symbol(self.c, symbol)? else {
            return Ok(None);
        };
        let module = self.c.get_merged_symbol(parent)?;
        Ok(h::is_external(self.c, module)?.then_some(ExportInfo { module, kind }))
    }
    // port: tsc/internal/ls/findallreferences.go:refState.getReferencesAtExportSpecifier
    pub(crate) fn at_export(
        &mut self,
        node: NodeId,
        symbol: SymbolRef,
        specifier: NodeId,
        search: &Search,
        add: bool,
    ) -> Result<()> {
        let view = self.l.view(node)?;
        let n = view.node(specifier)?;
        let name = n.name().ok_or(tsr_arena::Error::InvalidGraph)?;
        let decl = view
            .node(n.parent().ok_or(tsr_arena::Error::InvalidGraph)?)?
            .parent()
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let module = view.node(decl)?.module_specifier();
        let local = self.export_local(node, symbol, specifier)?;
        if !search.symbols.contains(&local) {
            return Ok(());
        }
        if n.property_name().is_none() {
            if add {
                self.add(node, local)?;
            }
        } else if n.property_name() == Some(node) {
            if add && module.is_none() {
                self.add(node, local)?;
            }
            if add && self.seen_exports.insert(name) {
                if let Some(exported) = self.c.bound_symbol_of_node(specifier)? {
                    self.add(name, exported)?;
                }
            }
        } else if self.seen_exports.insert(node) && add {
            self.add(node, local)?;
        }
        let kind = if view.node_text(node)?.as_bytes() == b"default"
            || view.node_text(name)?.as_bytes() == b"default"
        {
            ExportKind::Default
        } else {
            ExportKind::Named
        };
        if let Some(exported) = self.c.bound_symbol_of_node(specifier)? {
            if let Some(info) = self.export_info(exported, kind)? {
                self.imports_of_export(exported, info)?;
            }
        }
        if search.from != From::Export && module.is_some() && n.property_name().is_none() {
            if let Some(imported) = self.c.get_export_specifier_local_target_symbol(specifier)? {
                self.search_imported(imported)?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/findallreferences.go:refState.searchForImportedSymbol
    fn search_imported(&mut self, symbol: SymbolRef) -> Result<()> {
        for decl in h::declarations(self.c, symbol)? {
            let source = self.source_of(decl)?;
            let search = self.create_search(symbol, Some(decl), From::Import, None, Vec::new())?;
            self.in_container(source, source, &search, self.files.contains(&source))?;
        }
        Ok(())
    }
    fn export_declaration(&self, mut parent: NodeId, node: NodeId) -> Result<Option<NodeId>> {
        let view = self.l.view(parent)?;
        let n = view.node(parent)?;
        if matches!(
            n.kind().known(),
            Some(K::VariableDeclaration | K::BindingElement)
        ) {
            if n.name() != Some(node) {
                return Ok(None);
            }
            while view.node(parent)?.kind() == K::BindingElement {
                let Some(p) = view.node(parent)?.parent() else {
                    return Ok(None);
                };
                let Some(grand) = view.node(p)?.parent() else {
                    return Ok(None);
                };
                parent = grand;
            }
            let Some(p) = view.node(parent)?.parent() else {
                return Ok(None);
            };
            if view.node(p)?.kind() == K::CatchClause {
                return Ok(None);
            }
            return Ok(view.node(p)?.parent().filter(|id| {
                view.node(*id)
                    .is_ok_and(|n| n.kind() == K::VariableStatement)
            }));
        }
        Ok(Some(parent))
    }
    fn export_assignment(&self, node: NodeId) -> Result<Option<ExportInfo>> {
        let Some(s) = self.c.bound_symbol_of_node(node)? else {
            return Ok(None);
        };
        let Some(module) = h::parent_symbol(self.c, s)? else {
            return Ok(None);
        };
        let kind = if self
            .c
            .node(node)?
            .data_source()
            .as_export_assignment()
            .unwrap()
            .is_export_equals()
        {
            ExportKind::Equals
        } else {
            ExportKind::Default
        };
        Ok(Some(ExportInfo { module, kind }))
    }
    fn special_export(
        &self,
        node: NodeId,
        symbol: SymbolRef,
        lhs: bool,
    ) -> Result<Option<(SymbolRef, ExportInfo)>> {
        let view = self.l.view(node)?;
        let kind = match tsr_ast::get_assignment_declaration_kind(view, node)? {
            tsr_ast::JSDeclarationKind::ExportsProperty => ExportKind::Named,
            tsr_ast::JSDeclarationKind::ModuleExports => ExportKind::Equals,
            _ => return Ok(None),
        };
        let symbol = if lhs {
            let Some(s) = self.c.bound_symbol_of_node(node)? else {
                return Ok(None);
            };
            s
        } else {
            symbol
        };
        Ok(self.export_info(symbol, kind)?.map(|i| (symbol, i)))
    }
    // port: tsc/internal/ls/importTracker.go:getImportOrExportSymbol
    pub(crate) fn import_export_references(
        &mut self,
        node: NodeId,
        symbol: SymbolRef,
        search: &Search,
    ) -> Result<()> {
        let view = self.l.view(node)?;
        let Some(parent) = view.node(node)?.parent() else {
            return Ok(());
        };
        let p = view.node(parent)?;
        let grand = p.parent();
        let kind = if ast::has_syntactic_modifier(view, parent, tsr_ast::modifier_flags::DEFAULT)? {
            ExportKind::Default
        } else {
            ExportKind::Named
        };
        let mut export = None;
        if let Some(id) = self.c.symbol(symbol)?.export_symbol() {
            if p.kind() == K::PropertyAccessExpression {
                if let Some(grand) = grand.filter(|id| {
                    view.node(*id)
                        .is_ok_and(|n| n.kind() == K::BinaryExpression)
                }) {
                    if h::declarations(self.c, symbol)?.contains(&parent) {
                        export = self.special_export(grand, symbol, false)?;
                    }
                }
            } else {
                let s = self.c.symbol_ref(id)?;
                export = self.export_info(s, kind)?.map(|i| (s, i));
            }
        } else {
            let decl = self.export_declaration(parent, node)?;
            if let Some(decl) = decl.filter(|id| {
                ast::has_syntactic_modifier(view, *id, tsr_ast::modifier_flags::EXPORT)
                    .unwrap_or(false)
                    || tsr_ast::is_implicitly_exported_js_doc_declaration(view, *id)
                        .unwrap_or(false)
            }) {
                if view.node(decl)?.kind() == K::ImportEqualsDeclaration
                    && view
                        .node(decl)?
                        .data_source()
                        .as_import_equals_declaration()
                        .unwrap()
                        .module_reference()
                        == Some(node)
                {
                    if search.from != From::Export {
                        if let Some(name) = view.node(decl)?.name() {
                            if let Some(symbol) = self.c.get_symbol_at_location(name)? {
                                self.search_imported(symbol)?;
                            }
                        }
                    }
                    return Ok(());
                }
                let kind =
                    if ast::has_syntactic_modifier(view, decl, tsr_ast::modifier_flags::DEFAULT)? {
                        ExportKind::Default
                    } else {
                        ExportKind::Named
                    };
                export = self.export_info(symbol, kind)?.map(|i| (symbol, i));
            } else if p.kind() == K::NamespaceExport {
                export = self
                    .export_info(symbol, ExportKind::Named)?
                    .map(|i| (symbol, i));
            } else if p.kind() == K::ExportAssignment {
                export = self.export_assignment(parent)?.map(|i| (symbol, i));
            } else if let Some(grand) = grand.filter(|id| {
                view.node(*id)
                    .is_ok_and(|n| n.kind() == K::ExportAssignment)
            }) {
                export = self.export_assignment(grand)?.map(|i| (symbol, i));
            } else if p.kind() == K::BinaryExpression {
                export = self.special_export(parent, symbol, true)?;
            } else if let Some(grand) = grand.filter(|id| {
                view.node(*id)
                    .is_ok_and(|n| n.kind() == K::BinaryExpression)
            }) {
                export = self.special_export(grand, symbol, true)?;
            } else if matches!(
                p.kind().known(),
                Some(K::JSDocTypedefTag | K::JSDocCallbackTag)
            ) {
                export = self
                    .export_info(symbol, ExportKind::Named)?
                    .map(|i| (symbol, i));
            }
        }
        if let Some((symbol, info)) = export {
            return self.imports_of_export(symbol, info);
        }
        if search.from == From::Export {
            return Ok(());
        }
        let is_import = match p.kind().known() {
            Some(K::ImportEqualsDeclaration) => {
                p.name() == Some(node) && self.external_equals(parent)?.is_some()
            }
            Some(K::ImportSpecifier) => p.property_name().is_none(),
            Some(K::ImportClause | K::NamespaceImport) => true,
            Some(K::BindingElement) if ast::is_in_js_file(Some(&view.node(node)?)) => {
                if let Some(pattern) = p.parent() {
                    if let Some(decl) = view.node(pattern)?.parent() {
                        tsr_ast::is_variable_declaration_initialized_to_bare_or_accessed_require(
                            view, decl,
                        )?
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false,
        };
        if !is_import {
            return Ok(());
        }
        let imported = if self.c.symbol(symbol)?.flags() & sf::ALIAS != 0 {
            self.c.get_immediate_aliased_symbol(symbol)?
        } else {
            let mut found = None;
            for decl in h::declarations(self.c, symbol)? {
                if h::binding_without_property(self.l.view(decl)?, decl)? {
                    found = self.binding_property(decl)?;
                    break;
                }
            }
            found
        };
        let Some(mut imported) = imported else {
            return Ok(());
        };
        for decl in h::declarations(self.c, imported)? {
            let v = self.l.view(decl)?;
            let n = v.node(decl)?;
            if n.kind() == K::ExportSpecifier && n.property_name().is_none() {
                if let Some(p) = n.parent() {
                    if let Some(export) = v.node(p)?.parent() {
                        if v.node(export)?.module_specifier().is_none() {
                            imported = self
                                .c
                                .get_export_specifier_local_target_symbol(decl)?
                                .unwrap_or(imported);
                            break;
                        }
                    }
                }
            } else if n.kind() == K::PropertyAccessExpression {
                if let Some(expression) = n.expression() {
                    if tsr_ast::is_module_exports_access_expression(v, expression)?
                        && n.name().is_some_and(|id| {
                            v.node(id).is_ok_and(|n| n.kind() != K::PrivateIdentifier)
                        })
                    {
                        let Some(target) = self.c.get_symbol_at_location(decl)? else {
                            return Ok(());
                        };
                        imported = target;
                        break;
                    }
                }
            } else if n.kind() == K::ShorthandPropertyAssignment {
                if let Some(object) = n.parent() {
                    if let Some(assignment) = v.node(object)?.parent() {
                        if v.node(assignment)?.kind() == K::BinaryExpression
                            && tsr_ast::get_assignment_declaration_kind(v, assignment)?
                                == tsr_ast::JSDeclarationKind::ModuleExports
                        {
                            let Some(name) = n.name() else { return Ok(()) };
                            let Some(target) =
                                self.c.get_export_specifier_local_target_symbol(name)?
                            else {
                                return Ok(());
                            };
                            imported = target;
                            break;
                        }
                    }
                }
            }
        }
        if self.c.symbol(imported)?.name_bytes() == b"export=" {
            if self.c.symbol(imported)?.flags() & sf::ALIAS != 0 {
                let Some(s) = self.c.get_immediate_aliased_symbol(imported)? else {
                    return Ok(());
                };
                imported = s;
            } else {
                let Some(decl) = self.c.symbol(imported)?.value_declaration() else {
                    return Ok(());
                };
                let n = self.c.node(decl)?;
                let node = match n.kind().known() {
                    Some(K::ExportAssignment) => n.expression(),
                    Some(K::BinaryExpression) => {
                        n.data_source().as_binary_expression().unwrap().right()
                    }
                    Some(K::SourceFile) => Some(decl),
                    _ => None,
                };
                let Some(node) = node else { return Ok(()) };
                let Some(s) = self.c.bound_symbol_of_node(node)? else {
                    return Ok(());
                };
                imported = s;
            }
        }
        let mut name = self.c.symbol(imported)?.name_bytes().to_vec();
        if name == b"default" {
            name.clear();
            for decl in h::declarations(self.c, imported)? {
                let v = self.l.view(decl)?;
                if let Some(n) = v
                    .node(decl)?
                    .name()
                    .filter(|id| v.node(*id).is_ok_and(|n| n.kind() == K::Identifier))
                {
                    name = v.node_text(n)?.as_bytes().to_vec();
                    break;
                }
            }
        }
        if name.is_empty() || name == b"default" || name == self.c.symbol(symbol)?.name_bytes() {
            self.search_imported(imported)?;
        }
        Ok(())
    }
}
