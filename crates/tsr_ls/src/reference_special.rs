//! Keyword, literal, label and module reference searches.
use crate::{
    definition::target_label,
    documentation::list,
    reference_helpers as h,
    references::{DefinitionKind, ReferenceEntry, ReferenceGroup, SearchState},
    syntax::Syntax,
    Result,
};
use tsr_ast::{
    symbol_flags as sf, utilities as ast, utilities_containers as containers, NodeId,
    SyntaxKind as K,
};
use tsr_checker::SymbolRef;

impl SearchState<'_, '_, '_> {
    // port: tsc/internal/ls/findallreferences.go:LanguageService.getReferencedSymbolsForNode
    pub(crate) fn for_node(
        &mut self,
        mut node: NodeId,
        position: i64,
    ) -> Result<Vec<ReferenceGroup>> {
        self.l.check_canceled()?;
        if self.options.adjust {
            node = crate::meaning::adjusted_location(self.l.view(node)?, node, false)?;
        }
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        if n.kind() == K::SourceFile {
            let mut syntax = Syntax::new(view, node)?;
            if let Some(name) = self.l.reference_at(&mut syntax, position)? {
                if let Some(file) = self.l.program.source_file(&name) {
                    if let Some(symbol) = self.c.bound_symbol_of_node(file.source())? {
                        let symbol = self.c.get_merged_symbol(symbol)?;
                        return self.module_references(symbol, false);
                    }
                }
            }
            return Ok(Vec::new());
        }
        if !self.options.implementations {
            if let Some(groups) = self.special_references(node)? {
                return Ok(groups);
            }
        }
        let target = if n.kind() == K::Constructor {
            n.parent()
                .and_then(|p| view.node(p).ok().and_then(|p| p.name()))
                .unwrap_or(node)
        } else {
            node
        };
        let Some(symbol) = self.c.get_symbol_at_location(target)? else {
            return if !self.options.implementations && ast::is_string_literal_like(&n) {
                self.string_references(node)
            } else {
                Ok(Vec::new())
            };
        };
        if self.c.symbol(symbol)?.name_bytes() == b"export=" {
            return match h::parent_symbol(self.c, symbol)? {
                Some(parent) => self.module_references(parent, false),
                None => Ok(Vec::new()),
            };
        }
        let mut groups = self.module_if_source(symbol)?;
        if !groups.is_empty() && self.c.symbol(symbol)?.flags() & sf::TRANSIENT == 0 {
            return Ok(groups);
        }
        let alias = if n.parent().is_some_and(|p| {
            view.node(p)
                .is_ok_and(|n| n.kind() == K::NamespaceExportDeclaration)
        }) && self.c.symbol(symbol)?.flags() & sf::ALIAS != 0
        {
            Some(self.c.get_aliased_symbol(symbol)?)
        } else {
            None
        };
        self.search_symbol(symbol, Some(node))?;
        let result = std::mem::take(&mut self.result);
        self.merge_groups(&mut groups, result)?;
        if let Some(alias) = alias {
            let result = self.module_if_source(alias)?;
            self.merge_groups(&mut groups, result)?;
        }
        Ok(groups)
    }
    fn merge_groups(
        &mut self,
        out: &mut Vec<ReferenceGroup>,
        groups: Vec<ReferenceGroup>,
    ) -> Result<()> {
        if out.is_empty() {
            *out = groups;
            return Ok(());
        }
        for mut group in groups {
            if group.kind == DefinitionKind::Symbol {
                if let Some(existing) = out
                    .iter_mut()
                    .find(|g| g.kind == DefinitionKind::Symbol && g.symbol == group.symbol)
                {
                    existing.entries.append(&mut group.entries);
                    let mut with_keys = Vec::new();
                    for entry in std::mem::take(&mut existing.entries) {
                        let range = self.l.entry_range(&entry)?;
                        let index = self
                            .l
                            .program
                            .files()
                            .iter()
                            .position(|f| f.source() == entry.source);
                        with_keys.push(((index, range.pos(), range.end()), entry));
                    }
                    with_keys.sort_by_key(|(key, _)| *key);
                    existing.entries = with_keys.into_iter().map(|(_, entry)| entry).collect();
                    continue;
                }
            }
            out.push(group);
        }
        Ok(())
    }
    fn module_if_source(&mut self, symbol: SymbolRef) -> Result<Vec<ReferenceGroup>> {
        if self.c.symbol(symbol)?.flags() & sf::MODULE == 0 {
            return Ok(Vec::new());
        }
        let mut source = None;
        for decl in h::declarations(self.c, symbol)? {
            if self.c.node(decl)?.kind() == K::SourceFile {
                source = Some(decl);
                break;
            }
        }
        let Some(source) = source else {
            return Ok(Vec::new());
        };
        let exported = self.table_entry(self.c.symbol(symbol)?.exports(), b"export=")?;
        let mut groups = self.module_references(symbol, exported.is_some())?;
        if let Some(exported) = exported {
            if self.c.symbol(exported)?.flags() & sf::ALIAS != 0 && self.files.contains(&source) {
                let alias = self.c.get_aliased_symbol(exported)?;
                self.search_symbol(alias, None)?;
                let result = std::mem::take(&mut self.result);
                self.merge_groups(&mut groups, result)?;
            }
        }
        Ok(groups)
    }
    // port: tsc/internal/ls/findallreferences.go:LanguageService.getReferencesForStringLiteral
    fn string_references(&mut self, node: NodeId) -> Result<Vec<ReferenceGroup>> {
        let context = self.contextual_literal(node)?;
        let text = self.l.view(node)?.node_text(node)?.as_bytes().to_vec();
        let mut found = Vec::new();
        for source in self.files.clone() {
            self.l.check_canceled()?;
            let view = self.l.view(source)?;
            let mut syntax = Syntax::new(view, source)?;
            for candidate in h::candidates(&mut syntax, &text, None)? {
                let n = view.node(candidate)?;
                if !ast::is_string_literal_like(&n) || view.node_text(candidate)?.as_bytes() != text
                {
                    continue;
                }
                if let Some(context) = context {
                    let other = self.contextual_literal(candidate)?;
                    let string = self.c.get_string_type();
                    let mut property = false;
                    if let Some(parent) = n.parent().filter(|p| {
                        view.node(*p)
                            .is_ok_and(|n| n.kind() == K::PropertySignature)
                    }) {
                        if let Some(literal) = view.node(parent)?.parent() {
                            let ty = self.c.get_type_at_location(literal)?;
                            property = self.c.get_property_of_type(ty, &text)?.is_some();
                        }
                    }
                    if context != string && (other == Some(context) || property) {
                        found.push(candidate);
                    }
                } else if n.kind() != K::NoSubstitutionTemplateLiteral
                    || syntax.same_line(i64::from(n.pos()), i64::from(n.end()))
                {
                    found.push(candidate);
                }
            }
        }
        Ok(vec![self.group(
            DefinitionKind::String,
            Some(node),
            None,
            found,
        )?])
    }
    // port: tsc/internal/ls/findallreferences.go:getReferencedSymbolsSpecial
    fn special_references(&mut self, node: NodeId) -> Result<Option<Vec<ReferenceGroup>>> {
        let view = self.l.view(node)?;
        let n = view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(None);
        };
        let p = view.node(parent)?;
        if n.kind().known().is_some_and(h::type_keyword) {
            if n.kind() == K::VoidKeyword && p.kind() == K::VoidExpression
                || n.kind() == K::ReadonlyKeyword && p.kind() != K::TypeOperator
            {
                return Ok(None);
            }
            let text = tsr_scanner::token_to_string(n.kind().known().unwrap()).as_bytes();
            let mut found = Vec::new();
            for source in self.files.clone() {
                self.l.check_canceled()?;
                let v = self.l.view(source)?;
                let mut syntax = Syntax::new(v, source)?;
                for candidate in h::candidates(&mut syntax, text, None)? {
                    let c = v.node(candidate)?;
                    if c.kind() == n.kind()
                        && (n.kind() != K::ReadonlyKeyword
                            || c.parent().is_some_and(|p| {
                                v.node(p).is_ok_and(|n| n.kind() == K::TypeOperator)
                            }))
                    {
                        found.push(candidate);
                    }
                }
            }
            return Ok(if found.is_empty() {
                None
            } else {
                Some(vec![self.group(
                    DefinitionKind::Keyword,
                    found.first().copied(),
                    None,
                    found,
                )?])
            });
        }
        if p.kind() == K::MetaProperty
            && p.data_source().as_meta_property().unwrap().keyword_token() == K::ImportKeyword
            && p.name() == Some(node)
        {
            let mut found = Vec::new();
            for source in self.files.clone() {
                let v = self.l.view(source)?;
                let mut syntax = Syntax::new(v, source)?;
                for candidate in h::candidates(&mut syntax, b"meta", None)? {
                    if let Some(parent) = v.node(candidate)?.parent() {
                        let p = v.node(parent)?;
                        if p.kind() == K::MetaProperty
                            && p.data_source().as_meta_property().unwrap().keyword_token()
                                == K::ImportKeyword
                        {
                            found.push(parent);
                        }
                    }
                }
            }
            return Ok(if found.is_empty() {
                None
            } else {
                Some(vec![self.group(
                    DefinitionKind::Keyword,
                    found.first().copied(),
                    None,
                    found,
                )?])
            });
        }
        if n.kind() == K::StaticKeyword && p.kind() == K::ClassStaticBlockDeclaration {
            return Ok(Some(vec![self.group(
                DefinitionKind::Keyword,
                Some(node),
                None,
                vec![node],
            )?]));
        }
        if n.kind() == K::Identifier
            && matches!(
                p.kind().known(),
                Some(K::LabeledStatement | K::BreakStatement | K::ContinueStatement)
            )
        {
            let name = view.node_text(node)?;
            let label = if p.kind() == K::LabeledStatement {
                Some(node)
            } else {
                target_label(view, parent, name.as_bytes())?
            };
            let Some(label) = label else { return Ok(None) };
            let source = self.source_of(node)?;
            let mut syntax = Syntax::new(view, source)?;
            let mut found = Vec::new();
            for candidate in
                h::candidates(&mut syntax, name.as_bytes(), view.node(label)?.parent())?
            {
                let c = view.node(candidate)?;
                if candidate == label
                    || c.parent().is_some_and(|p| {
                        view.node(p).is_ok_and(|p| {
                            matches!(
                                p.kind().known(),
                                Some(K::BreakStatement | K::ContinueStatement)
                            )
                        })
                    }) && target_label(view, candidate, name.as_bytes())? == Some(label)
                {
                    found.push(candidate);
                }
            }
            return Ok(Some(vec![self.group(
                DefinitionKind::Label,
                Some(label),
                None,
                found,
            )?]));
        }
        if n.kind() == K::ThisKeyword
            || n.kind() == K::Identifier
                && p.kind() == K::Parameter
                && view.node_text(node)?.as_bytes() == b"this"
        {
            return self.this_references(node).map(Some);
        }
        if n.kind() == K::SuperKeyword {
            return self.super_references(node).map(Some);
        }
        Ok(None)
    }
    // port: tsc/internal/ls/findallreferences.go:getReferencesForThisKeyword
    fn this_references(&mut self, node: NodeId) -> Result<Vec<ReferenceGroup>> {
        let view = self.l.view(node)?;
        let mut space = tsr_ast::get_this_container(view, node, false, false)?;
        let mut is_static = true;
        let n = view.node(space)?;
        match n.kind().known() {
            Some(
                K::MethodDeclaration
                | K::MethodSignature
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor,
            ) => {
                is_static =
                    ast::has_syntactic_modifier(view, space, tsr_ast::modifier_flags::STATIC)?;
                space = n.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
            }
            Some(K::SourceFile) => {
                if view.source_file(space)?.external_module_indicator.is_some()
                    || view.node(node)?.kind() == K::Identifier
                {
                    return Ok(Vec::new());
                }
            }
            Some(K::FunctionDeclaration | K::FunctionExpression) => {}
            _ => return Ok(Vec::new()),
        }
        let symbol = self.c.bound_symbol_of_node(space)?;
        let kind = view.node(space)?.kind();
        let files = if kind == K::SourceFile {
            self.files.clone()
        } else {
            vec![self.source_of(space)?]
        };
        let mut found = Vec::new();
        for source in files {
            let v = self.l.view(source)?;
            let mut syntax = Syntax::new(v, source)?;
            for candidate in h::candidates(
                &mut syntax,
                b"this",
                Some(if kind == K::SourceFile { source } else { space }),
            )? {
                let c = v.node(candidate)?;
                let parameter = c.kind() == K::Identifier
                    && c.parent().is_some_and(|p| {
                        v.node(p)
                            .is_ok_and(|p| p.kind() == K::Parameter && p.name() == Some(candidate))
                    });
                if c.kind() != K::ThisKeyword && !parameter {
                    continue;
                }
                let container = tsr_ast::get_this_container(v, candidate, false, false)?;
                let accept = match kind.known() {
                    Some(K::FunctionExpression | K::FunctionDeclaration) => {
                        self.c.bound_symbol_of_node(container)? == symbol
                    }
                    Some(K::ClassExpression | K::ClassDeclaration | K::ObjectLiteralExpression) => {
                        if let Some(p) = v.node(container)?.parent() {
                            self.c.bound_symbol_of_node(p)? == symbol
                                && ast::has_syntactic_modifier(
                                    v,
                                    container,
                                    tsr_ast::modifier_flags::STATIC,
                                )? == is_static
                        } else {
                            false
                        }
                    }
                    Some(K::SourceFile) => {
                        v.node(container)?.kind() == K::SourceFile
                            && v.source_file(container)?
                                .external_module_indicator
                                .is_none()
                            && !parameter
                    }
                    _ => false,
                };
                if accept {
                    found.push(candidate);
                }
            }
        }
        let parameter = found
            .iter()
            .copied()
            .find(|&n| {
                self.c.node(n).is_ok_and(|n| {
                    n.parent()
                        .is_some_and(|p| self.c.node(p).is_ok_and(|p| p.kind() == K::Parameter))
                })
            })
            .unwrap_or(node);
        Ok(vec![self.group(
            DefinitionKind::This,
            Some(parameter),
            symbol,
            found,
        )?])
    }
    // port: tsc/internal/ls/findallreferences.go:getReferencesForSuperKeyword
    fn super_references(&mut self, node: NodeId) -> Result<Vec<ReferenceGroup>> {
        let view = self.l.view(node)?;
        let Some(container) = containers::get_super_container(view, node, false)? else {
            return Ok(Vec::new());
        };
        let n = view.node(container)?;
        if !matches!(
            n.kind().known(),
            Some(
                K::PropertyDeclaration
                    | K::PropertySignature
                    | K::MethodDeclaration
                    | K::MethodSignature
                    | K::Constructor
                    | K::GetAccessor
                    | K::SetAccessor
            )
        ) {
            return Ok(Vec::new());
        }
        let space = n.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
        let symbol = self.c.bound_symbol_of_node(space)?;
        let is_static =
            ast::has_syntactic_modifier(view, container, tsr_ast::modifier_flags::STATIC)?;
        let mut syntax = Syntax::new(view, self.source_of(node)?)?;
        let mut found = Vec::new();
        for candidate in h::candidates(&mut syntax, b"super", Some(space))? {
            if view.node(candidate)?.kind() != K::SuperKeyword {
                continue;
            }
            if let Some(container) = containers::get_super_container(view, candidate, false)? {
                if ast::has_syntactic_modifier(view, container, tsr_ast::modifier_flags::STATIC)?
                    == is_static
                {
                    if let Some(p) = view.node(container)?.parent() {
                        if self.c.bound_symbol_of_node(p)? == symbol {
                            found.push(candidate);
                        }
                    }
                }
            }
        }
        Ok(vec![self.group(
            DefinitionKind::Symbol,
            None,
            symbol,
            found,
        )?])
    }
    // port: tsc/internal/ls/findallreferences.go:LanguageService.getReferencedSymbolsForModule
    pub(crate) fn module_references(
        &mut self,
        symbol: SymbolRef,
        exclude_import_type: bool,
    ) -> Result<Vec<ReferenceGroup>> {
        let mut entries = Vec::new();
        let target = self.c.symbol(symbol)?.value_declaration();
        for source in self.files.clone() {
            let file = self
                .l
                .program
                .file_of_node(source)
                .ok_or(tsr_arena::Error::WrongOwner)?;
            let view = self.l.view(source)?;
            let sf = view.source_file(source)?;
            if target.is_some_and(|n| self.c.node(n).is_ok_and(|n| n.kind() == K::SourceFile)) {
                for reference in sf.referenced_files()?.iter() {
                    if self
                        .l
                        .program
                        .source_file_from_reference(file, reference)
                        .map(tsr_compiler::ProgramFile::source)
                        == target
                    {
                        entries.push(ReferenceEntry {
                            node: None,
                            context: None,
                            source,
                            range: Some(reference.loc),
                        });
                    }
                }
                for reference in sf.type_reference_directives()?.iter() {
                    if self
                        .l
                        .program
                        .resolved_type_reference_from_directive(file, reference)?
                        .and_then(|r| self.l.program.source_file(r.resolved_file_name.as_bytes()))
                        .map(tsr_compiler::ProgramFile::source)
                        == target
                    {
                        entries.push(ReferenceEntry {
                            node: None,
                            context: None,
                            source,
                            range: Some(reference.loc),
                        });
                    }
                }
            }
            for (import, specifier) in self.file_imports(source)? {
                if self.c.get_symbol_at_location(specifier)? != Some(symbol) {
                    continue;
                }
                if exclude_import_type
                    && view
                        .node(import)?
                        .data_source()
                        .as_import_type_node()
                        .is_some_and(|d| d.qualifier().is_none())
                {
                    continue;
                }
                entries.push(self.entry(specifier)?);
            }
            // These side-effect imports have no local binding to search in
            // the alias tracker. The native module reference selects JSX or
            // the first statement, then falls back to the file.
            for name in self.l.program.implicit_imports(file)? {
                if self.c.implicit_import_symbol(source, name.as_bytes())? == Some(symbol) {
                    let syntax = Syntax::new(view, source)?;
                    let mut range_node = None;
                    if name.as_bytes() != b"tslib" {
                        let mut stack = vec![source];
                        while let Some(node) = stack.pop() {
                            if matches!(
                                view.node(node)?.kind().known(),
                                Some(K::JsxElement | K::JsxSelfClosingElement | K::JsxFragment)
                            ) {
                                range_node = Some(node);
                                break;
                            }
                            stack.extend(syntax.children(node)?.into_iter().rev());
                        }
                    }
                    let node = range_node
                        .or(list(view, view.node(source)?.statement_list())?
                            .first()
                            .copied())
                        .unwrap_or(source);
                    entries.push(self.entry(node)?);
                }
            }
        }
        for decl in h::declarations(self.c, symbol)? {
            if self.c.node(decl)?.kind() == K::ModuleDeclaration && self.includes(decl)? {
                if let Some(name) = self.c.node(decl)?.name() {
                    entries.push(self.entry(name)?);
                }
            }
        }
        if let Some(exported) = self.table_entry(self.c.symbol(symbol)?.exports(), b"export=")? {
            for decl in h::declarations(self.c, exported)? {
                if !self.includes(decl)? {
                    continue;
                }
                let view = self.l.view(decl)?;
                let n = view.node(decl)?;
                let node = if n.kind() == K::BinaryExpression {
                    if let Some(left) = n
                        .data_source()
                        .as_binary_expression()
                        .unwrap()
                        .left()
                        .filter(|id| {
                            view.node(*id)
                                .is_ok_and(|n| n.kind() == K::PropertyAccessExpression)
                        })
                    {
                        view.node(left)?.expression()
                    } else {
                        None
                    }
                } else if n.kind() == K::ExportAssignment {
                    Syntax::new(view, self.source_of(decl)?)?
                        .nav()
                        .find_child_of_kind(decl, K::ExportKeyword)?
                } else {
                    n.name().or(Some(decl))
                };
                if let Some(node) = node {
                    entries.push(self.entry(node)?);
                }
            }
        }
        Ok(if entries.is_empty() {
            Vec::new()
        } else {
            vec![ReferenceGroup {
                kind: DefinitionKind::Symbol,
                symbol: Some(symbol),
                node: None,
                entries,
            }]
        })
    }
}
