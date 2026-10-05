//! Native reference groups retain their symbol identity until protocol conversion.
//! The same engine serves references, highlights and the implementation worklist.
use crate::{
    definition::ancestor, documentation::list, meaning, reference_helpers as h, syntax::Syntax,
    LanguageService, Result,
};
use std::collections::{HashMap, HashSet, VecDeque};
use tsr_ast::{
    span_map::{FEATURE_IMPLEMENTATION, FEATURE_REFERENCES},
    symbol_flags as sf, utilities as ast, utilities_positions as pos, NodeId, SyntaxKind as K,
};
use tsr_checker::{Operation, SymbolRef};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefinitionKind {
    Symbol,
    Label,
    Keyword,
    This,
    String,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum EntryKind {
    #[default]
    Node,
    LocalFoundProperty,
    PropertyFoundLocal,
}
#[derive(Clone)]
pub(crate) struct ReferenceEntry {
    pub kind: EntryKind,
    pub node: Option<NodeId>,
    pub context: Option<NodeId>,
    pub source: NodeId,
    pub range: Option<TextRange>,
}
#[derive(Clone)]
pub(crate) struct ReferenceGroup {
    pub kind: DefinitionKind,
    pub symbol: Option<SymbolRef>,
    pub node: Option<NodeId>,
    pub entries: Vec<ReferenceEntry>,
}
#[derive(Clone, Copy, Default)]
pub(crate) struct ReferenceOptions {
    pub implementations: bool,
    pub adjust: bool,
    pub rename: bool,
    pub aliases: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum From {
    Unknown,
    Import,
    Export,
}
#[derive(Clone)]
pub(crate) struct Search {
    pub symbol: SymbolRef,
    pub text: Vec<u8>,
    pub symbols: Vec<SymbolRef>,
    pub from: From,
    pub parents: Vec<SymbolRef>,
}
pub(crate) struct SearchState<'a, 'p, 'o> {
    pub l: &'a mut LanguageService<'p>,
    pub c: &'a mut Operation<'o>,
    pub files: Vec<NodeId>,
    pub options: ReferenceOptions,
    pub result: Vec<ReferenceGroup>,
    pub meaning: i32,
    pub special: &'static str,
    pub seen: HashMap<NodeId, HashSet<SymbolRef>>,
    pub seen_exports: HashSet<NodeId>,
    pub seen_types: HashSet<NodeId>,
    pub imports: Option<HashMap<SymbolRef, Vec<NodeId>>>,
    pub inheritance: HashMap<(SymbolRef, SymbolRef), bool>,
}
impl<'a, 'p, 'o> SearchState<'a, 'p, 'o> {
    pub fn new(
        l: &'a mut LanguageService<'p>,
        c: &'a mut Operation<'o>,
        files: Vec<NodeId>,
        options: ReferenceOptions,
    ) -> Self {
        Self {
            l,
            c,
            files,
            options,
            result: Vec::new(),
            meaning: pos::semantic_meaning::ALL,
            special: "none",
            seen: HashMap::new(),
            seen_exports: HashSet::new(),
            seen_types: HashSet::new(),
            imports: None,
            inheritance: HashMap::new(),
        }
    }
    pub fn entry(&self, node: NodeId) -> Result<ReferenceEntry> {
        let n = self.c.node(node)?;
        let context = h::entry_context(self.l.view(node)?, node)?;
        let node = n.name().unwrap_or(node);
        Ok(ReferenceEntry {
            kind: EntryKind::Node,
            node: Some(node),
            context,
            source: self.source_of(node)?,
            range: None,
        })
    }
    pub fn group(
        &self,
        kind: DefinitionKind,
        node: Option<NodeId>,
        symbol: Option<SymbolRef>,
        nodes: Vec<NodeId>,
    ) -> Result<ReferenceGroup> {
        Ok(ReferenceGroup {
            kind,
            node,
            symbol,
            entries: nodes
                .into_iter()
                .map(|n| self.entry(n))
                .collect::<Result<_>>()?,
        })
    }
    pub fn includes(&self, node: NodeId) -> Result<bool> {
        Ok(self.files.contains(&self.source_of(node)?))
    }
    // port: tsc/internal/ls/findallreferences.go:getReferencedSymbolsForSymbol
    pub fn search_symbol(&mut self, symbol: SymbolRef, node: Option<NodeId>) -> Result<()> {
        self.meaning = if self.options.rename {
            pos::semantic_meaning::ALL
        } else {
            self.search_meaning(node, symbol)?
        };
        self.special = if let Some(node) = node {
            let view = self.l.view(node)?;
            let n = view.node(node)?;
            if matches!(
                n.kind().known(),
                Some(K::Constructor | K::ConstructorKeyword)
            ) {
                "constructor"
            } else if n.kind() == K::Identifier
                && n.parent().is_some_and(|p| {
                    view.node(p)
                        .is_ok_and(|p| ast::is_class_like(&p) && p.name() == Some(node))
                })
            {
                "class"
            } else {
                "none"
            }
        } else {
            "none"
        };
        let mut target = symbol;
        if let Some(node) = node {
            let view = self.l.view(node)?;
            if let Some(parent) = view
                .node(node)?
                .parent()
                .filter(|p| view.node(*p).is_ok_and(|p| p.kind() == K::ExportSpecifier))
            {
                if !(self.options.rename && self.options.aliases) {
                    target = self.export_local(node, symbol, parent)?;
                }
            } else {
                for decl in h::declarations(self.c, symbol)? {
                    let v = self.l.view(decl)?;
                    if let Some(p) = v
                        .node(decl)?
                        .parent()
                        .filter(|p| v.node(*p).is_ok_and(|p| p.kind() == K::TypeLiteral))
                    {
                        if let Some(union) = v
                            .node(p)?
                            .parent()
                            .filter(|p| v.node(*p).is_ok_and(|p| p.kind() == K::UnionType))
                        {
                            let ty = self.c.get_type_from_type_node(union)?;
                            let name = self.c.symbol(symbol)?.name_bytes().to_vec();
                            if let Some(s) = self.c.get_property_of_type(ty, &name)? {
                                target = s;
                                break;
                            }
                        }
                    }
                }
            }
        }
        if self.options.rename && self.options.aliases {
            for declaration in h::declarations(self.c, target)? {
                if self.c.node(declaration)?.kind() == K::ExportSpecifier {
                    let name = self
                        .c
                        .node(declaration)?
                        .name()
                        .ok_or(tsr_arena::Error::InvalidGraph)?;
                    let search =
                        self.create_search(symbol, node, From::Unknown, None, Vec::new())?;
                    return self.at_export(name, target, declaration, &search, true, true);
                }
            }
        }
        if let Some(node) = node {
            if self.c.node(node)?.kind() == K::DefaultKeyword
                && self.c.symbol(target)?.name_bytes() == b"default"
            {
                if let Some(parent) = h::parent_symbol(self.c, target)? {
                    self.add(node, target)?;
                    return self.imports_of_export(
                        target,
                        crate::import_tracker::ExportInfo {
                            module: parent,
                            kind: crate::import_tracker::ExportKind::Default,
                        },
                    );
                }
            }
        }
        let symbols = if let Some(node) = node {
            self.related_candidates(target, node, !self.options.implementations, &[], None)?
                .into_iter()
                .map(|(s, _, _)| s)
                .collect()
        } else {
            vec![target]
        };
        let search = self.create_search(target, node, From::Unknown, None, symbols)?;
        self.in_scope(target, &search)
    }
    // port: tsc/internal/ls/findallreferences.go:refState.createSearch
    pub(crate) fn create_search(
        &mut self,
        symbol: SymbolRef,
        location: Option<NodeId>,
        from: From,
        text: Option<Vec<u8>>,
        mut symbols: Vec<SymbolRef>,
    ) -> Result<Search> {
        let text = if let Some(text) = text {
            text
        } else {
            let mut named = symbol;
            let decls = h::declarations(self.c, symbol)?;
            // binder.GetLocalSymbolForExportDefault and merged non-module symbol.
            if self.c.symbol(symbol)?.name_bytes() == b"default" {
                if let Some(&decl) = decls.first() {
                    if let Some(id) = self
                        .l
                        .program
                        .file_of_node(decl)
                        .ok_or(tsr_arena::Error::WrongOwner)?
                        .bound()
                        .view()
                        .node_binding(decl)?
                        .and_then(|b| b.local_symbol)
                    {
                        named = self.c.symbol_ref(id)?;
                    }
                }
            } else if self.c.symbol(symbol)?.flags() & (sf::MODULE | sf::TRANSIENT) != 0 {
                for decl in decls {
                    if !matches!(
                        self.c.node(decl)?.kind().known(),
                        Some(K::SourceFile | K::ModuleDeclaration)
                    ) {
                        if let Some(s) = self.c.bound_symbol_of_node(decl)? {
                            named = s;
                        }
                        break;
                    }
                }
            }
            let mut bytes = self.c.symbol_display_name(named)?.as_bytes().to_vec();
            if bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"') {
                bytes = bytes[1..bytes.len() - 1].to_vec();
            }
            bytes
        };
        if symbols.is_empty() {
            symbols.push(symbol);
        }
        let mut parents = Vec::new();
        if self.options.implementations {
            if let Some(node) = location {
                let view = self.l.view(node)?;
                let n = view.node(node)?;
                if let Some(parent) = n.parent() {
                    let p = view.node(parent)?;
                    if p.kind() == K::PropertyAccessExpression && p.name() == Some(node) {
                        let ty = self.c.get_type_at_location(
                            p.expression().ok_or(tsr_arena::Error::InvalidGraph)?,
                        )?;
                        let types = if self.c.type_flags(ty)?
                            & tsr_checker::type_flags::UNION_OR_INTERSECTION
                            != 0
                        {
                            self.c.constituents(ty)?
                        } else if self.c.type_symbol(ty)? != self.c.symbol(symbol)?.parent() {
                            vec![ty]
                        } else {
                            Vec::new()
                        };
                        for ty in types {
                            if let Some(id) = self.c.type_symbol(ty)? {
                                let s = self.c.symbol_ref(id)?;
                                if self.c.symbol(s)?.flags() & (sf::CLASS | sf::INTERFACE) != 0 {
                                    parents.push(s);
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(Search {
            symbol,
            text,
            symbols,
            from,
            parents,
        })
    }
    // port: tsc/internal/ls/findallreferences.go:refState.referenceAdder
    pub(crate) fn append(&mut self, node: NodeId, symbol: SymbolRef) -> Result<()> {
        self.append_kind(node, symbol, EntryKind::Node)
    }
    fn reference_group(&mut self, symbol: SymbolRef) -> &mut ReferenceGroup {
        let index = self
            .result
            .iter()
            .position(|group| group.kind == DefinitionKind::Symbol && group.symbol == Some(symbol))
            .unwrap_or_else(|| {
                self.result.push(ReferenceGroup {
                    kind: DefinitionKind::Symbol,
                    symbol: Some(symbol),
                    node: None,
                    entries: Vec::new(),
                });
                self.result.len() - 1
            });
        &mut self.result[index]
    }
    fn append_kind(&mut self, node: NodeId, symbol: SymbolRef, kind: EntryKind) -> Result<()> {
        let mut entry = self.entry(node)?;
        entry.kind = kind;
        self.reference_group(symbol).entries.push(entry);
        Ok(())
    }
    // port: tsc/internal/ls/findallreferences.go:refState.addReference
    pub(crate) fn add(&mut self, node: NodeId, symbol: SymbolRef) -> Result<()> {
        self.add_kind(node, symbol, EntryKind::Node)
    }
    fn add_kind(&mut self, node: NodeId, symbol: SymbolRef, kind: EntryKind) -> Result<()> {
        if self.options.implementations {
            // The definition survives even when this project has no local
            // implementation. Cross-project discovery still needs that symbol.
            self.reference_group(symbol);
            for n in self.implementation_nodes(node)? {
                self.append_kind(n, symbol, kind)?;
            }
        } else {
            self.append_kind(node, symbol, kind)?;
        }
        Ok(())
    }
    // port: tsc/internal/ls/findallreferences.go:refState.getReferencesInContainerOrFiles
    pub(crate) fn in_scope(&mut self, symbol: SymbolRef, search: &Search) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            if let Some(scope) = self.symbol_scope(symbol)? {
                let source = self.source_of(scope)?;
                let add =
                    self.c.node(scope)?.kind() != K::SourceFile || self.files.contains(&scope);
                self.in_container(scope, source, search, add)
            } else {
                for source in self.files.clone() {
                    self.in_container(source, source, search, true)?;
                }
                Ok(())
            }
        })
    }
    // port: tsc/internal/ls/findallreferences.go:refState.getReferencesInContainer
    pub(crate) fn in_container(
        &mut self,
        container: NodeId,
        source: NodeId,
        search: &Search,
        add: bool,
    ) -> Result<()> {
        self.l.check_canceled()?;
        let seen = self.seen.entry(source).or_default();
        let mut any = false;
        for &s in &search.symbols {
            any |= seen.insert(s);
        }
        if !any {
            return Ok(());
        }
        let mut syntax = Syntax::new(self.l.view(source)?, source)?;
        for position in h::positions(&syntax, &search.text, Some(container))? {
            let node = syntax.nav().get_touching_property_name(position as i64)?;
            if h::valid_reference(syntax.view, node, &search.text)? {
                self.at_location(node, search, add)?;
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/findallreferences.go:refState.getReferencesAtLocation
    fn at_location(&mut self, node: NodeId, search: &Search, add: bool) -> Result<()> {
        let view = self.l.view(node)?;
        if self.options.rename && view.node(node)?.kind() == K::DefaultKeyword {
            return Ok(());
        }
        if meaning::meaning(view, node, self.c)? & self.meaning == 0 {
            return Ok(());
        }
        let Some(mut symbol) = self.c.get_symbol_at_location(node)? else {
            return Ok(());
        };
        let Some(parent) = view.node(node)?.parent() else {
            return Ok(());
        };
        let p = view.node(parent)?;
        if p.kind() == K::ImportSpecifier && p.property_name() == Some(node) {
            return Ok(());
        }
        if p.kind() == K::ExportSpecifier {
            return self.at_export(node, symbol, parent, search, add, false);
        }
        let mut related = None;
        let candidates =
            self.related_candidates(symbol, node, true, &search.parents, Some(&search.symbols))?;
        for (test, found, kind) in candidates {
            if search.symbols.contains(&test) {
                related = Some((found, kind));
                break;
            }
        }
        let Some((related, kind)) = related else {
            if self.c.symbol(symbol)?.flags() & sf::TRANSIENT == 0 {
                if let Some(decl) = self.c.symbol(symbol)?.value_declaration() {
                    if let Some(s) = self.c.get_shorthand_assignment_value_symbol(Some(decl))? {
                        if search.symbols.contains(&s) {
                            if let Some(name) = self.c.node(decl)?.name() {
                                self.add(name, s)?;
                            }
                        }
                    }
                }
            }
            return Ok(());
        };
        match self.special {
            "constructor" => self.constructor_references(node, related, search, add)?,
            "class" => self.class_references(node, related, search, add)?,
            _ => {
                if add {
                    self.add_kind(node, related, kind)?;
                }
            }
        }
        if p.kind() == K::BindingElement && ast::is_in_js_file(Some(&view.node(node)?)) {
            if let Some(pattern) = p.parent() {
                if let Some(decl) = view.node(pattern)?.parent() {
                    if tsr_ast::is_variable_declaration_initialized_to_bare_or_accessed_require(
                        view, decl,
                    )? {
                        let Some(s) = self.c.bound_symbol_of_node(parent)? else {
                            return Ok(());
                        };
                        symbol = s;
                    }
                }
            }
        }
        self.import_export_references(node, symbol, search)
    }
    // port: tsc/internal/ls/findallreferences.go:refState.explicitlyInheritsFrom
    pub(crate) fn inherits(&mut self, symbol: SymbolRef, parent: SymbolRef) -> Result<bool> {
        if symbol == parent {
            return Ok(true);
        }
        let key = (symbol, parent);
        if let Some(&answer) = self.inheritance.get(&key) {
            return Ok(answer);
        }
        self.inheritance.insert(key, false);
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || -> Result<bool> {
            for decl in h::declarations(self.c, symbol)? {
                for node in h::super_types(self.l.view(decl)?, decl)? {
                    let ty = self.c.get_type_at_location(node)?;
                    if let Some(id) = self.c.type_symbol(ty)? {
                        if self.inherits(self.c.symbol_ref(id)?, parent)? {
                            return Ok(true);
                        }
                    }
                }
            }
            Ok(false)
        })?;
        self.inheritance.insert(key, result);
        Ok(result)
    }
    // port: tsc/internal/ls/findallreferences.go:refState.addImplementationReferences
    fn implementation_nodes(&mut self, node: NodeId) -> Result<Vec<NodeId>> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(Vec::new());
        };
        if pos::is_declaration_name(view, node)? && h::implementation(view, parent)? {
            return Ok(vec![node]);
        }
        if n.kind() != K::Identifier {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        if view.node(parent)?.kind() == K::ShorthandPropertyAssignment {
            if let Some(s) = self.c.get_symbol_at_location(node)? {
                if let Some(decl) = self.c.symbol(s)?.value_declaration() {
                    if let Some(value) = self.c.get_shorthand_assignment_value_symbol(Some(decl))? {
                        for decl in h::declarations(self.c, value)? {
                            if pos::get_meaning_from_declaration(self.l.view(decl)?, decl)?
                                & pos::semantic_meaning::VALUE
                                != 0
                            {
                                result.push(decl);
                            }
                        }
                    }
                }
            }
        }
        if let Some(container) = h::heritage_container(view, node)? {
            result.push(container);
            return Ok(result);
        }
        let type_node = ancestor(view, Some(node), |id| {
            let Some(p) = view.node(id)?.parent() else {
                return Ok(true);
            };
            let p = view.node(p)?;
            Ok(p.kind() != K::QualifiedName && !ast::is_type_node(&p) && !ast::is_type_element(&p))
        })?;
        let Some(type_node) = type_node else {
            return Ok(result);
        };
        let Some(having) = view.node(type_node)?.parent() else {
            return Ok(result);
        };
        let having_node = view.node(having)?;
        if having_node.type_node() != Some(type_node) || !self.seen_types.insert(having) {
            return Ok(result);
        }
        let mut expressions = Vec::new();
        if tsr_ast::utilities_middle::has_initializer(&having_node) {
            expressions.extend(having_node.initializer());
        } else if ast::is_function_like(Some(&having_node)) {
            if let Some(body) = having_node.body() {
                if view.node(body)?.kind() == K::Block {
                    let syntax = Syntax::new(view, self.source_of(node)?)?;
                    let mut stack = vec![body];
                    while let Some(id) = stack.pop() {
                        let n = view.node(id)?;
                        if n.kind() == K::ReturnStatement {
                            expressions.extend(n.expression());
                        } else if !ast::is_function_like(Some(&n)) {
                            let children = syntax.children(id)?;
                            stack.extend(children.into_iter().rev());
                        }
                    }
                } else {
                    expressions.push(body);
                }
            }
        } else if ast::is_assertion_expression(&having_node)
            || having_node.kind() == K::SatisfiesExpression
        {
            expressions.extend(having_node.expression());
        }
        for expression in expressions {
            if h::implementation_expression(view, expression)? {
                result.push(expression);
            }
        }
        Ok(result)
    }
    fn class_references(
        &mut self,
        node: NodeId,
        symbol: SymbolRef,
        search: &Search,
        add: bool,
    ) -> Result<()> {
        if add {
            self.add(node, symbol)?;
        }
        if self.options.rename {
            return Ok(());
        }
        let view = self.l.view(node)?;
        let Some(class) = view
            .node(node)?
            .parent()
            .filter(|p| view.node(*p).is_ok_and(|p| ast::is_class_like(&p)))
        else {
            return Ok(());
        };
        let syntax = Syntax::new(view, self.source_of(node)?)?;
        for member in list(view, view.node(class)?.member_list())? {
            let n = view.node(member)?;
            if !matches!(
                n.kind().known(),
                Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor)
            ) || !ast::has_syntactic_modifier(view, member, tsr_ast::modifier_flags::STATIC)?
            {
                continue;
            }
            let mut stack = n.body().into_iter().collect::<Vec<_>>();
            while let Some(id) = stack.pop() {
                let n = view.node(id)?;
                if n.kind() == K::ThisKeyword {
                    self.append(id, search.symbol)?;
                } else if !ast::is_function_like(Some(&n)) && !ast::is_class_like(&n) {
                    stack.extend(syntax.children(id)?.into_iter().rev());
                }
            }
        }
        Ok(())
    }
    fn constructor_references(
        &mut self,
        node: NodeId,
        symbol: SymbolRef,
        search: &Search,
        add: bool,
    ) -> Result<()> {
        let view = self.l.view(node)?;
        if tsr_ast::utilities_targets::is_new_expression_target(view, node, false, false)? && add {
            self.add(node, symbol)?;
        }
        let Some(parent) = view.node(node)?.parent() else {
            return Ok(());
        };
        let class = if ast::is_class_like(&view.node(parent)?) {
            Some(parent)
        } else {
            h::heritage_container(view, node)?
        };
        let Some(class) = class.filter(|id| view.node(*id).is_ok_and(|n| ast::is_class_like(&n)))
        else {
            return Ok(());
        };
        let own = parent == class;
        let mut syntax = Syntax::new(view, self.source_of(class)?)?;
        let constructors = list(view, view.node(class)?.member_list())?
            .into_iter()
            .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::Constructor))
            .collect::<Vec<_>>();
        for &decl in &constructors {
            if own {
                if let Some(keyword) = syntax
                    .nav()
                    .find_child_of_kind(decl, K::ConstructorKeyword)?
                {
                    self.append(keyword, search.symbol)?;
                }
            } else {
                let mut stack = view.node(decl)?.body().into_iter().collect::<Vec<_>>();
                while let Some(id) = stack.pop() {
                    let n = view.node(id)?;
                    if n.kind() == K::SuperKeyword
                        && tsr_ast::utilities_targets::is_call_expression_target(
                            view, id, false, false,
                        )?
                    {
                        self.append(id, search.symbol)?;
                    }
                    stack.extend(syntax.children(id)?.into_iter().rev());
                }
            }
        }
        if !own && constructors.is_empty() {
            if let Some(symbol) = self.c.bound_symbol_of_node(class)? {
                let next = self.create_search(symbol, None, From::Unknown, None, Vec::new())?;
                self.in_scope(symbol, &next)?;
            }
        }
        if own {
            for member in list(view, view.node(class)?.member_list())? {
                if view.node(member)?.kind() == K::MethodDeclaration
                    && ast::has_syntactic_modifier(view, member, tsr_ast::modifier_flags::STATIC)?
                {
                    let mut stack = view.node(member)?.body().into_iter().collect::<Vec<_>>();
                    while let Some(id) = stack.pop() {
                        if view.node(id)?.kind() == K::ThisKeyword
                            && tsr_ast::utilities_targets::is_new_expression_target(
                                view, id, false, false,
                            )?
                        {
                            self.append(id, search.symbol)?;
                        }
                        stack.extend(syntax.children(id)?.into_iter().rev());
                    }
                }
            }
        }
        Ok(())
    }
}
impl LanguageService<'_> {
    pub(crate) fn entry_write(&self, entry: &ReferenceEntry) -> Result<bool> {
        let Some(node) = entry.node else {
            return Ok(false);
        };
        let bound = self
            .program
            .file_of_node(node)
            .ok_or(tsr_arena::Error::WrongOwner)?
            .bound()
            .view();
        Ok(pos::is_write_access_for_reference(
            bound.ast(),
            Some(bound),
            node,
        )?)
    }
    pub(crate) fn entry_range(&mut self, entry: &ReferenceEntry) -> Result<TextRange> {
        if let Some(range) = entry.range {
            return Ok(range);
        }
        Syntax::new(self.view(entry.source)?, entry.source)?
            .reference_range(entry.node.ok_or(tsr_arena::Error::InvalidGraph)?, None)
    }
    pub(crate) fn entry_location(
        &mut self,
        entry: &ReferenceEntry,
        feature: i32,
    ) -> Result<Option<lsp::Location>> {
        let range = self.entry_range(entry)?;
        let (location, fidelity) =
            self.file_location(&self.source(entry.source)?, range, Some(feature))?;
        Ok(fidelity.is_single_segment().then_some(location))
    }
    // port: tsc/internal/ls/findallreferences.go:LanguageService.ProvideReferences
    pub fn references(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::ReferenceParams,
    ) -> Result<lsp::LocationsOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let mapped = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_REFERENCES,
        )?;
        let files = self
            .program
            .files()
            .iter()
            .map(|f| f.source())
            .collect::<Vec<_>>();
        let mut locations = Vec::new();
        let mut seen = HashSet::new();
        for mapped in mapped {
            if !mapped.mapped.fidelity.is_single_segment() {
                continue;
            }
            let mut syntax = Syntax::new(self.view(mapped.script)?, mapped.script)?;
            let node = syntax
                .nav()
                .get_touching_property_name(i64::from(mapped.mapped.position))?;
            let mut state = SearchState::new(
                self,
                c,
                files.clone(),
                ReferenceOptions {
                    adjust: true,
                    ..Default::default()
                },
            );
            let groups = state.for_node(node, i64::from(mapped.mapped.position))?;
            for group in groups {
                self.record_cross_project_group(c, &group)?;
                for entry in group.entries {
                    if !params
                        .context
                        .as_deref()
                        .is_some_and(|c| c.include_declaration)
                    {
                        if let (Some(node), Some(symbol)) = (entry.node, group.symbol) {
                            if is_declaration(
                                self.program
                                    .file_of_node(node)
                                    .ok_or(tsr_arena::Error::WrongOwner)?
                                    .bound()
                                    .view(),
                                c,
                                node,
                                symbol,
                            )? {
                                continue;
                            }
                        }
                    }
                    if let Some(location) = self.entry_location(&entry, FEATURE_REFERENCES)? {
                        if seen.insert((
                            location.uri.0.clone(),
                            location.range.start.line,
                            location.range.start.character,
                            location.range.end.line,
                            location.range.end.character,
                        )) {
                            locations.push(location);
                        }
                    }
                }
            }
        }
        Ok(lsp::LocationsOrNull {
            locations: Some(Box::new(locations)),
        })
    }
    pub(crate) fn implementation_entries(
        &mut self,
        c: &mut Operation<'_>,
        node: NodeId,
        position: i64,
    ) -> Result<Vec<ReferenceEntry>> {
        let files = self
            .program
            .files()
            .iter()
            .map(|f| f.source())
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([(node, position)]);
        while let Some((node, pos)) = queue.pop_front() {
            self.check_canceled()?;
            let mut state = SearchState::new(
                self,
                c,
                files.clone(),
                ReferenceOptions {
                    implementations: true,
                    adjust: true,
                    ..Default::default()
                },
            );
            for group in state.for_node(node, pos)? {
                self.record_cross_project_group(c, &group)?;
                for entry in group.entries {
                    if let Some(node) = entry.node {
                        if seen.insert(node) {
                            queue.push_back((node, i64::from(c.node(node)?.pos())));
                            entries.push(entry);
                        }
                    }
                }
            }
        }
        Ok(entries)
    }
    // port: tsc/internal/ls/findallreferences.go:LanguageService.ProvideImplementations
    pub fn implementations(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::ImplementationParams,
        links: bool,
    ) -> Result<lsp::LocationOrLocationsOrDefinitionLinksOrNull> {
        self.implementations_with_options(c, params, links, false)
    }

    pub fn implementations_with_options(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::ImplementationParams,
        links: bool,
        drop_origin: bool,
    ) -> Result<lsp::LocationOrLocationsOrDefinitionLinksOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let mapped = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_IMPLEMENTATION,
        )?;
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        for mapped in mapped {
            if !mapped.mapped.fidelity.is_single_segment() {
                continue;
            }
            let mut syntax = Syntax::new(self.view(mapped.script)?, mapped.script)?;
            let node = syntax
                .nav()
                .get_touching_property_name(i64::from(mapped.mapped.position))?;
            if node == mapped.script {
                continue;
            }
            for entry in self.implementation_entries(c, node, i64::from(mapped.mapped.position))? {
                if let Some(node) = entry.node {
                    if !seen.insert(node) {
                        continue;
                    }
                    if drop_origin {
                        let location = c.node(node)?;
                        if location.pos() <= mapped.mapped.position
                            && mapped.mapped.position <= location.end()
                        {
                            continue;
                        }
                    }
                    entries.push(entry);
                }
            }
        }

        let mut result = lsp::LocationOrLocationsOrDefinitionLinksOrNull::default();
        if links {
            let mut out = Vec::new();
            for entry in entries {
                let Some(loc) = self.entry_location(&entry, FEATURE_IMPLEMENTATION)? else {
                    continue;
                };
                let mut range = loc.range.clone();
                if let Some(context) = entry.context {
                    let view = self.view(context)?;
                    let mut syntax = Syntax::new(view, entry.source)?;
                    let context_range = syntax.reference_range(context, None)?;
                    let (location, fidelity) = self.file_location(
                        &syntax.file,
                        context_range,
                        Some(FEATURE_IMPLEMENTATION),
                    )?;
                    if !fidelity.is_none() && location.uri == loc.uri {
                        range = location.range;
                    }
                }
                out.push(Some(Box::new(lsp::LocationLink {
                    // The pin keeps the original entry URI on implementation
                    // links even when its selection follows a declaration map.
                    target_uri: lsp::DocumentUri::from_file_name(
                        self.source(entry.source)?.original_file_name()?.as_bytes(),
                    ),
                    target_selection_range: loc.range,
                    target_range: range,
                    ..Default::default()
                })));
            }
            result.definition_links = Some(Box::new(out));
        } else {
            let mut out = Vec::new();
            for e in entries {
                if let Some(l) = self.entry_location(&e, FEATURE_IMPLEMENTATION)? {
                    out.push(l);
                }
            }
            result.locations = Some(Box::new(out));
        }
        Ok(result)
    }
}
// port: tsc/internal/ls/findallreferences.go:isDeclarationOfSymbol
pub(crate) fn is_declaration(
    bound: tsr_ast::BoundView<'_>,
    c: &Operation<'_>,
    node: NodeId,
    symbol: SymbolRef,
) -> Result<bool> {
    let view = bound.ast();
    let n = view.node(node)?;
    let mut source = pos::get_declaration_from_name(view, Some(bound), Some(node))?;
    if source.is_none() {
        match n.kind().known() {
            Some(K::DefaultKeyword) => source = n.parent(),
            Some(K::ConstructorKeyword) => {
                if let Some(p) = n.parent() {
                    if view.node(p)?.kind() == K::Constructor {
                        source = view.node(p)?.parent();
                    }
                }
            }
            _ => {
                if let Some(p) = n.parent() {
                    if view.node(p)?.kind() == K::ComputedPropertyName {
                        source = view.node(p)?.parent();
                    }
                }
            }
        }
    }
    Ok(source.is_some_and(|s| {
        c.symbol_declarations(symbol)
            .is_ok_and(|d| d.iter().flatten().any(|n| n == s))
    }))
}
