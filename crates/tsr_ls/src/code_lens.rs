use crate::{
    references::{is_declaration, ReferenceOptions, SearchState},
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{
    modifier_flags as mf,
    span_map::{FEATURE_CODE_LENS, FEATURE_IMPLEMENTATION, FEATURE_REFERENCES},
    utilities as ast, AstView, NodeId, SyntaxKind as K,
};
use tsr_checker::Operation;
use tsr_compiler::diagnostic_writer::DiagnosticSources;
use tsr_lsproto as lsp;

/// Optional booleans preserve the native settings refresh distinction between
/// an unset preference and an explicitly disabled one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodeLensOptions {
    pub references: Option<bool>,
    pub implementations: Option<bool>,
    pub all_functions: Option<bool>,
    pub interface_methods: Option<bool>,
    pub all_class_methods: Option<bool>,
}
impl CodeLensOptions {
    pub fn enabled(self) -> bool {
        self.references == Some(true) || self.implementations == Some(true)
    }
}
fn valid_reference(v: AstView<'_>, node: NodeId, o: CodeLensOptions) -> Result<bool> {
    let n = v.node(node)?;
    Ok(match n.kind().known() {
        Some(K::FunctionDeclaration) if o.all_functions == Some(true) => true,
        Some(K::FunctionDeclaration | K::VariableDeclaration) => {
            ast::get_combined_modifier_flags(v, node)? & mf::EXPORT != 0
        }
        Some(
            K::ClassDeclaration
            | K::InterfaceDeclaration
            | K::TypeAliasDeclaration
            | K::EnumDeclaration
            | K::EnumMember,
        ) => true,
        Some(
            K::MethodDeclaration
            | K::MethodSignature
            | K::Constructor
            | K::GetAccessor
            | K::SetAccessor
            | K::PropertyDeclaration
            | K::PropertySignature,
        ) => n.parent().is_some_and(|id| {
            v.node(id).is_ok_and(|n| {
                matches!(
                    n.kind().known(),
                    Some(K::ClassDeclaration | K::InterfaceDeclaration | K::TypeLiteral)
                )
            })
        }),
        _ => false,
    })
}
fn valid_implementation(v: AstView<'_>, node: NodeId, o: CodeLensOptions) -> Result<bool> {
    let n = v.node(node)?;
    Ok(match n.kind().known() {
        Some(K::InterfaceDeclaration) => true,
        Some(K::MethodSignature) => {
            o.interface_methods == Some(true)
                && n.parent().is_some_and(|id| {
                    v.node(id)
                        .is_ok_and(|n| n.kind() == K::InterfaceDeclaration)
                })
        }
        Some(K::MethodDeclaration)
            if o.all_class_methods == Some(true)
                && n.parent().is_some_and(|id| {
                    v.node(id).is_ok_and(|n| n.kind() == K::ClassDeclaration)
                }) =>
        {
            !ast::has_syntactic_modifier(v, node, mf::PRIVATE)?
                && n.name()
                    .is_some_and(|id| v.node(id).is_ok_and(|n| n.kind() != K::PrivateIdentifier))
        }
        Some(
            K::MethodDeclaration
            | K::ClassDeclaration
            | K::Constructor
            | K::GetAccessor
            | K::SetAccessor
            | K::PropertyDeclaration,
        ) => ast::has_syntactic_modifier(v, node, mf::ABSTRACT)?,
        _ => false,
    })
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/codelens.go:LanguageService.ProvideCodeLenses
    pub fn code_lenses(
        &mut self,
        params: &lsp::CodeLensParams,
        options: CodeLensOptions,
    ) -> Result<lsp::CodeLensesOrNull> {
        if !options.enabled() {
            return Ok(lsp::CodeLensesOrNull::default());
        }
        let source = self.file(&params.text_document.uri)?;
        let sources = std::iter::once((source, None))
            .chain(
                self.program
                    .supplemental_sources(source)?
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| (n, Some(i as i32))),
            )
            .collect::<Vec<_>>();
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for (source, index) in sources {
            let bound = self
                .program
                .file_of_node(source)
                .ok_or(tsr_arena::Error::WrongOwner)?
                .bound()
                .view();
            let view = bound.ast();
            let syntax = Syntax::new(view, source)?;
            let mut last = None;
            enum Task {
                Node(NodeId),
                Restore(Option<tsr_ast::SymbolId>),
            }
            let mut stack = vec![Task::Node(source)];
            while let Some(task) = stack.pop() {
                self.check_canceled()?;
                let node = match task {
                    Task::Restore(saved) => {
                        last = saved;
                        continue;
                    }
                    Task::Node(node) => node,
                };
                let n = view.node(node)?;
                let symbol = bound.node_binding(node)?.and_then(|b| b.symbol);
                if last != symbol {
                    last = symbol;
                    for (enabled, implementation) in [
                        (options.references == Some(true), false),
                        (options.implementations == Some(true), true),
                    ] {
                        if !enabled
                            || !(if implementation {
                                valid_implementation(view, node, options)?
                            } else {
                                valid_reference(view, node, options)?
                            })
                        {
                            continue;
                        }
                        let position = tsr_scanner::skip_trivia(
                            syntax.file.text().as_bytes(),
                            i64::from(view.node(n.name().unwrap_or(node))?.pos()),
                        );
                        let (range, f) = self.range(
                            source,
                            tsr_core::TextRange::new(position, i64::from(n.end())),
                            FEATURE_CODE_LENS,
                        )?;
                        if f.is_none()
                            || !seen.insert((
                                implementation,
                                range.start.line,
                                range.start.character,
                                range.end.line,
                                range.end.character,
                            ))
                        {
                            continue;
                        }
                        let kind = if implementation {
                            lsp::CodeLensKind::IMPLEMENTATIONS
                        } else {
                            lsp::CodeLensKind::REFERENCES
                        };
                        result.push(Some(Box::new(lsp::CodeLens {
                            range,
                            command: None,
                            data: Some(Box::new(lsp::CodeLensData {
                                kind: lsp::CodeLensKind(kind.into()),
                                uri: params.text_document.uri.clone(),
                                position: position as i32,
                                supplemental_file_index: index.map(Box::new),
                            })),
                        })));
                    }
                }
                stack.push(Task::Restore(last));
                stack.extend(syntax.children(node)?.into_iter().rev().map(Task::Node));
            }
        }
        Ok(lsp::CodeLensesOrNull {
            code_lenses: Some(Box::new(result)),
        })
    }
    // port: tsc/internal/ls/codelens.go:LanguageService.ResolveCodeLens
    pub fn resolve_code_lens(
        &mut self,
        c: &mut Operation<'_>,
        lens: lsp::CodeLens,
        command: Option<&str>,
        locale: &tsr_locale::Locale,
    ) -> Result<lsp::CodeLens> {
        let locations = self.code_lens_locations(c, &lens)?;
        Self::code_lens_result(lens, &locations, command, locale)
    }

    pub fn code_lens_locations(
        &mut self,
        c: &mut Operation<'_>,
        lens: &lsp::CodeLens,
    ) -> Result<Vec<lsp::Location>> {
        let data = lens.data.as_deref().ok_or(tsr_arena::Error::InvalidGraph)?;
        let mut source = self.file(&data.uri)?;
        if let Some(&index) = data.supplemental_file_index.as_deref() {
            source = self
                .program
                .supplemental_sources(source)?
                .get(index as usize)
                .copied()
                .ok_or(tsr_arena::Error::InvalidSlot)?;
        }
        let node = Syntax::new(self.view(source)?, source)?
            .nav()
            .get_touching_property_name(i64::from(data.position))?;
        let mut locations = Vec::new();
        let mut seen = HashSet::new();
        let implementation = data.kind.0 == lsp::CodeLensKind::IMPLEMENTATIONS;
        if implementation {
            for entry in self.implementation_entries(c, node, i64::from(data.position))? {
                if let Some(n) = entry.node {
                    let n = c.node(n)?;
                    if n.pos() <= data.position && data.position <= n.end() {
                        continue;
                    }
                }
                if let Some(l) = self.entry_location(&entry, FEATURE_IMPLEMENTATION)? {
                    locations.push(l);
                }
            }
        } else if data.kind.0 == lsp::CodeLensKind::REFERENCES {
            let files = self.program.files().iter().map(|f| f.source()).collect();
            let groups = SearchState::new(
                self,
                c,
                files,
                ReferenceOptions {
                    adjust: true,
                    ..Default::default()
                },
            )
            .for_node(node, i64::from(data.position))?;
            for group in groups {
                self.record_cross_project_group(c, &group)?;
                for entry in group.entries {
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
                    if let Some(l) = self.entry_location(&entry, FEATURE_REFERENCES)? {
                        let r = &l.range;
                        if seen.insert((
                            l.uri.0.clone(),
                            r.start.line,
                            r.start.character,
                            r.end.line,
                            r.end.character,
                        )) {
                            locations.push(l);
                        }
                    }
                }
            }
        }
        Ok(locations)
    }

    /// Finalize a lens after all projects' locations have been combined.
    pub fn code_lens_result(
        mut lens: lsp::CodeLens,
        locations: &[lsp::Location],
        command: Option<&str>,
        locale: &tsr_locale::Locale,
    ) -> Result<lsp::CodeLens> {
        let data = lens.data.as_deref().ok_or(tsr_arena::Error::InvalidGraph)?;
        let implementation = data.kind.0 == lsp::CodeLensKind::IMPLEMENTATIONS;
        use tsr_diagnostics as d;
        let message = match (implementation, locations.len() == 1) {
            (true, true) => d::X_1_implementation,
            (true, false) => d::X_0_implementations,
            (false, true) => d::X_1_reference,
            (false, false) => d::X_0_references,
        };
        let title = String::from_utf8_lossy(&message.localize(
            locale,
            &[tsr_diagnostics::Argument::Int(locations.len() as i64)],
        ))
        .into_owned();
        let mut cmd = lsp::Command {
            title,
            ..Default::default()
        };
        if let Some(command) = command.filter(|_| !locations.is_empty()) {
            cmd.command = command.into();
            cmd.arguments = Some(Box::new(vec![
                lsp::Any::String(data.uri.0.clone()),
                position_any(&lens.range.start),
                lsp::Any::Array(
                    locations
                        .iter()
                        .map(|l| {
                            lsp::Any::Object(
                                [
                                    ("uri".into(), lsp::Any::String(l.uri.0.clone())),
                                    (
                                        "range".into(),
                                        lsp::Any::Object(
                                            [
                                                ("start".into(), position_any(&l.range.start)),
                                                ("end".into(), position_any(&l.range.end)),
                                            ]
                                            .into(),
                                        ),
                                    ),
                                ]
                                .into(),
                            )
                        })
                        .collect(),
                ),
            ]));
        }
        lens.command = Some(Box::new(cmd));
        Ok(lens)
    }
}
fn position_any(p: &lsp::Position) -> lsp::Any {
    lsp::Any::Object(
        [
            ("line".into(), lsp::Any::Number(f64::from(p.line))),
            ("character".into(), lsp::Any::Number(f64::from(p.character))),
        ]
        .into(),
    )
}
