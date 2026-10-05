//! Definition positions used by the project coordinator to expand a search.
//! These values contain protocol positions only; no checker-local symbols escape.
use crate::{
    converters::Script,
    reference_helpers,
    references::{DefinitionKind, ReferenceGroup, ReferenceOptions, SearchState},
    source_map::MapHost,
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::{HashSet, VecDeque};
use tsr_ast::{
    modifier_flags,
    span_map::{FEATURE_IMPLEMENTATION, FEATURE_REFERENCES, FEATURE_RENAME},
    utilities, NodeId, SyntaxKind as K,
};
use tsr_checker::Operation;
use tsr_lsproto as lsp;
use tsr_printer::emit_resolver::DeclarationEmitResolver;
use tsr_sourcemap::{DocumentPosition, Host};

#[derive(Clone, Debug)]
pub struct CrossProjectPosition {
    pub uri: lsp::DocumentUri,
    pub position: lsp::Position,
}

#[derive(Clone, Debug)]
pub struct CrossProjectDefinition {
    pub position: CrossProjectPosition,
    pub source: Option<CrossProjectPosition>,
    pub generated: Option<CrossProjectPosition>,
}

#[derive(Clone, Copy, Debug)]
pub struct CrossProjectSearchOptions {
    pub rename: bool,
    pub implementations: bool,
    pub aliases: bool,
}
impl Default for CrossProjectSearchOptions {
    fn default() -> Self {
        Self {
            rename: false,
            implementations: false,
            aliases: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CrossProjectTargets {
    pub default_definition: Option<CrossProjectDefinition>,
    pub original_positions: Vec<CrossProjectPosition>,
}

impl LanguageService<'_> {
    /// Finds the first visible definition and all original definition positions
    /// in reference-group order. The coordinator uses the default definition
    /// only for the default project and follows originals from every project.
    // port: tsc/internal/ls/findallreferences.go:LanguageService.provideSymbolsAndEntries
    // port: tsc/internal/ls/findallreferences.go:LanguageService.provideSymbolsAndEntriesAtPosition
    pub fn cross_project_targets(
        &mut self,
        checker: &mut Operation<'_>,
        uri: &lsp::DocumentUri,
        position: &lsp::Position,
        rename: bool,
        implementations: bool,
    ) -> Result<CrossProjectTargets> {
        self.cross_project_targets_with_options(
            checker,
            uri,
            position,
            CrossProjectSearchOptions {
                rename,
                implementations,
                ..Default::default()
            },
        )
    }

    pub fn cross_project_targets_with_options(
        &mut self,
        checker: &mut Operation<'_>,
        uri: &lsp::DocumentUri,
        position: &lsp::Position,
        options: CrossProjectSearchOptions,
    ) -> Result<CrossProjectTargets> {
        let CrossProjectSearchOptions {
            rename,
            implementations,
            aliases,
        } = options;
        self.check_canceled()?;
        let source = self.file(uri)?;
        let feature = if implementations {
            FEATURE_IMPLEMENTATION
        } else if rename {
            FEATURE_RENAME
        } else {
            FEATURE_REFERENCES
        };
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            position,
            feature,
        )?;
        let files = self
            .program
            .files()
            .iter()
            .map(|file| file.source())
            .collect::<Vec<_>>();
        let options = ReferenceOptions {
            implementations,
            rename,
            adjust: true,
            aliases,
        };
        let mut targets = CrossProjectTargets::default();
        for mapped in positions {
            if !mapped.mapped.fidelity.is_single_segment() {
                continue;
            }
            let mut syntax = Syntax::new(self.view(mapped.script)?, mapped.script)?;
            let position = i64::from(mapped.mapped.position);
            let mut node = syntax.nav().get_touching_property_name(position)?;
            if rename {
                node = crate::meaning::adjusted_location(syntax.view, node, true)?;
            }
            if rename && !crate::rename::eligible(syntax.view, node)?
                || implementations && node == mapped.script
            {
                continue;
            }
            let mut queue = VecDeque::from([(node, position)]);
            let mut seen_nodes = HashSet::new();
            let mut seen_definitions = HashSet::new();
            while let Some((node, position)) = queue.pop_front() {
                self.check_canceled()?;
                let groups = SearchState::new(self, checker, files.clone(), options)
                    .for_node(node, position)?;
                for group in groups {
                    if implementations {
                        let mut new_references = false;
                        for entry in &group.entries {
                            if let Some(node) = entry.node {
                                if seen_nodes.insert(node) {
                                    new_references = true;
                                    queue.push_back((node, i64::from(checker.node(node)?.pos())));
                                }
                            }
                        }
                        if !new_references && !seen_definitions.insert(group.symbol) {
                            continue;
                        }
                    }
                    if targets.default_definition.is_none() {
                        targets.default_definition = self.non_local_definition(checker, &group)?;
                    }
                    self.original_definition_locations(
                        checker,
                        &group,
                        &mut targets.original_positions,
                    )?;
                }
                if !implementations {
                    break;
                }
            }
        }
        Ok(targets)
    }

    fn declaration_position(&mut self, declaration: NodeId) -> Result<(NodeId, i64)> {
        let file = self
            .program
            .file_of_node(declaration)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        let mut syntax = Syntax::new(file.bound().view().ast(), file.source())?;
        let name = syntax.view.node(declaration)?.name().unwrap_or(declaration);
        Ok((file.source(), syntax.reference_range(name, None)?.pos()))
    }

    fn position_in_source(
        &mut self,
        source: NodeId,
        position: i64,
    ) -> Result<Option<CrossProjectPosition>> {
        let file = self.source(source)?;
        let original = file.original_file_name()?;
        let script = Script {
            file_name: file.file_name(),
            text: file.text().as_bytes(),
            original_file_name: original.as_bytes(),
            original_text: file.original_text(),
            span_map: file.span_map(),
        };
        let (position, fidelity) = self.converters.to_lsp_position(&script, position as i32);
        Ok((!fidelity.is_none()).then(|| CrossProjectPosition {
            uri: lsp::DocumentUri::from_file_name(file.file_name()),
            position,
        }))
    }

    fn mapped_project_position(
        &mut self,
        mapped: &DocumentPosition,
    ) -> Option<CrossProjectPosition> {
        let text = MapHost(self.program).read_file(mapped.file_name.as_bytes())?;
        let (position, fidelity) = self.converters.to_lsp_position(
            &Script::plain(mapped.file_name.as_bytes(), text.as_bytes()),
            mapped.pos as i32,
        );
        (!fidelity.is_none()).then(|| CrossProjectPosition {
            uri: lsp::DocumentUri::from_file_name(mapped.file_name.as_bytes()),
            position,
        })
    }

    // port: tsc/internal/ls/findallreferences.go:LanguageService.getNonLocalDefinition
    fn non_local_definition(
        &mut self,
        checker: &mut Operation<'_>,
        group: &ReferenceGroup,
    ) -> Result<Option<CrossProjectDefinition>> {
        let Some(symbol) = usable_definition_symbol(group) else {
            return Ok(None);
        };
        for declaration in reference_helpers::declarations(checker, symbol)? {
            if !self.definition_visible(checker, declaration)? {
                continue;
            }
            let (source, start) = self.declaration_position(declaration)?;
            let Some(position) = self.position_in_source(source, start)? else {
                continue;
            };
            let name = self.source(source)?.parse_options().file_name.clone();
            let source = self
                .source_position(name.as_bytes(), start)
                .and_then(|mapped| self.mapped_project_position(&mapped));
            let generated = self
                .generated_position(name.as_bytes(), start)
                .and_then(|mapped| self.mapped_project_position(&mapped));
            return Ok(Some(CrossProjectDefinition {
                position,
                source,
                generated,
            }));
        }
        Ok(None)
    }

    // port: tsc/internal/ls/findallreferences.go:isDefinitionVisible
    fn definition_visible(
        &self,
        checker: &mut Operation<'_>,
        mut declaration: NodeId,
    ) -> Result<bool> {
        loop {
            if checker.is_declaration_visible(declaration)? {
                return Ok(true);
            }
            let view = self.view(declaration)?;
            let node = view.node(declaration)?;
            let Some(parent) = node.parent() else {
                return Ok(false);
            };
            if tsr_ast::utilities_middle::has_initializer(&view.node(parent)?)
                && view.node(parent)?.initializer() == Some(declaration)
            {
                declaration = parent;
                continue;
            }
            match node.kind().known() {
                Some(
                    K::PropertyDeclaration | K::GetAccessor | K::SetAccessor | K::MethodDeclaration,
                ) => {
                    if utilities::has_syntactic_modifier(
                        view,
                        declaration,
                        modifier_flags::PRIVATE,
                    )? || node.name().is_some_and(|name| {
                        view.node(name)
                            .is_ok_and(|name| name.kind() == K::PrivateIdentifier)
                    }) {
                        return Ok(false);
                    }
                }
                Some(
                    K::Constructor
                    | K::PropertyAssignment
                    | K::ShorthandPropertyAssignment
                    | K::ObjectLiteralExpression
                    | K::ClassExpression
                    | K::ArrowFunction
                    | K::FunctionExpression,
                ) => {}
                _ => return Ok(false),
            }
            declaration = parent;
        }
    }

    // port: tsc/internal/ls/findallreferences.go:LanguageService.forEachOriginalDefinitionLocation
    fn original_definition_locations(
        &mut self,
        checker: &Operation<'_>,
        group: &ReferenceGroup,
        positions: &mut Vec<CrossProjectPosition>,
    ) -> Result<()> {
        let Some(symbol) = usable_definition_symbol(group) else {
            return Ok(());
        };
        for declaration in reference_helpers::declarations(checker, symbol)? {
            let (source, start) = self.declaration_position(declaration)?;
            let file = self.source(source)?;
            let name = file.parse_options().file_name.clone();
            let position = if tsr_tspath::is_declaration_file_name(name.as_bytes()) {
                self.source_position(name.as_bytes(), start)
                    .and_then(|mapped| self.mapped_project_position(&mapped))
            } else if self
                .program
                .is_source_from_project_reference(file.parse_options().path.as_bytes())
            {
                self.position_in_source(source, start)?
            } else {
                None
            };
            if let Some(position) = position {
                positions.push(position);
            }
        }
        Ok(())
    }
}

// port: tsc/internal/ls/findallreferences.go:SymbolAndEntries.canUseDefinitionSymbol
fn usable_definition_symbol(group: &ReferenceGroup) -> Option<tsr_checker::SymbolRef> {
    matches!(group.kind, DefinitionKind::Symbol | DefinitionKind::This)
        .then_some(group.symbol)
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tsr_core::CancellationToken;

    fn targets(text: &[u8], marker: &[u8]) -> CrossProjectTargets {
        targets_with_implementations(text, marker, false)
    }

    fn targets_with_implementations(
        text: &[u8],
        marker: &[u8],
        implementations: bool,
    ) -> CrossProjectTargets {
        targets_with_options(
            text,
            marker,
            CrossProjectSearchOptions {
                implementations,
                ..Default::default()
            },
        )
    }

    fn targets_with_options(
        text: &[u8],
        marker: &[u8],
        options: CrossProjectSearchOptions,
    ) -> CrossProjectTargets {
        let program = Arc::new(crate::tests::program(b"/index.ts", text));
        let source = program.source_file(b"/index.ts").unwrap().source();
        let pool =
            tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let mut service = LanguageService::new(
            &program,
            tsr_jsstring::PositionEncoding::Utf16,
            CancellationToken::new(),
        );
        let position = text
            .windows(marker.len())
            .rposition(|part| part == marker)
            .unwrap() as u32;
        service
            .cross_project_targets_with_options(
                &mut checker,
                &lsp::DocumentUri("file:///index.ts".into()),
                &lsp::Position {
                    line: 0,
                    character: position,
                },
                options,
            )
            .unwrap()
    }

    #[test]
    fn imported_interface_implementations_are_found_from_the_owner_location() {
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        fs.insert_loaded(b"/a/a.ts", "/*😀*/ export function shared() { return 1; }\nexport interface Service { run(): void; }\n".as_bytes());
        fs.insert_loaded(b"/b/b.ts", b"import {shared, Service} from \"../a/a\";\nexport function callerB() { shared(); }\nexport class B implements Service { run() {} }\n".as_slice());
        fs.insert_loaded(
            b"/a/tsconfig.json",
            br#"{"compilerOptions":{"noLib":true,"composite":true},"files":["a.ts"]}"#.as_slice(),
        );
        let host = Arc::new(fs.finish());
        for root in [b"/a/a.ts".as_slice(), b"/b/b.ts".as_slice()] {
            let mut config = tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    composite: tsr_core::Tristate::TRUE,
                    ..Default::default()
                },
                vec![tsr_jsstring::JsString::from_bytes(root)],
            );
            if root == b"/b/b.ts" {
                config.project_references = Some(vec![tsr_tsoptions::ProjectReference {
                    path: tsr_jsstring::JsString::from_bytes(b"/a".as_slice()),
                    original_path: tsr_jsstring::JsString::from_bytes(b"../a".as_slice()),
                    circular: false,
                }]);
            }
            let program = Arc::new(
                tsr_compiler::Program::load_live_for_project(
                    tsr_compiler::ProgramOptions {
                        config,
                        host: host.clone(),
                        current_directory: tsr_jsstring::JsString::from_bytes(b"/".as_slice()),
                        default_library_path: tsr_jsstring::JsString::from_bytes(b"/".as_slice()),
                        skip_module_resolution: false,
                        single_threaded: tsr_core::Tristate::TRUE,
                    },
                    &mut tsr_compiler::FileCache::new(),
                    &tsr_arena::Counters::new(),
                )
                .unwrap(),
            );
            let source = program.source_file(b"/a/a.ts").unwrap().source();
            let pool = tsr_compiler::CompilerCheckerPool::new(
                program.clone(),
                &tsr_arena::Counters::new(),
            );
            let mut checker = pool.checker_for_file_exclusive(source).unwrap();
            let mut service = LanguageService::new(
                &program,
                tsr_jsstring::PositionEncoding::Utf16,
                CancellationToken::new(),
            );
            let uri = lsp::DocumentUri("file:///a/a.ts".into());
            let position = lsp::Position {
                line: 1,
                character: 17,
            };
            let targets = service
                .cross_project_targets(&mut checker, &uri, &position, false, true)
                .unwrap();
            assert!(targets.default_definition.is_some());
            let result = service
                .implementations(
                    &mut checker,
                    &lsp::ImplementationParams {
                        text_document: lsp::TextDocumentIdentifier { uri },
                        position,
                        ..Default::default()
                    },
                    true,
                )
                .unwrap();
            assert_eq!(
                result.definition_links.unwrap().len(),
                usize::from(root == b"/b/b.ts")
            );
        }
    }

    #[test]
    fn interface_without_local_implementations_retains_cross_project_definition() {
        let result = targets_with_implementations(b"export interface Service {}", b"Service", true);
        let definition = result.default_definition.expect("interface definition");
        assert_eq!(definition.position.uri.0, "file:///index.ts");
        assert_eq!(definition.position.position.character, 17);
        assert!(result.original_positions.is_empty());
    }

    #[test]
    fn exported_arrow_definition_expands_search_but_local_parameter_does_not() {
        let visible = targets(b"export const visible = () => 1; visible();", b"visible");
        let definition = visible.default_definition.unwrap();
        assert_eq!(definition.position.uri.0, "file:///index.ts");
        assert_eq!(definition.position.position.character, 13);
        assert!(definition.source.is_none());
        assert!(definition.generated.is_none());
        assert!(visible.original_positions.is_empty());
        let local = targets(
            b"function f(local: number) { return local; } export {};",
            b"local",
        );
        assert!(local.default_definition.is_none());
    }

    #[test]
    fn private_member_is_not_a_cross_project_definition() {
        let private = targets(
            b"export class C { private hidden = 1; method() { return this.hidden; } }",
            b"hidden",
        );
        assert!(private.default_definition.is_none());
    }

    #[test]
    fn declaration_maps_supply_original_and_default_source_positions() {
        let program = Arc::new(crate::tests::source_map_program(br#"{"version":3,"file":"lib.d.ts","sources":["source.ts"],"names":[],"mappings":"wBACgB,GAAG"}"#));
        let source = program.source_file(b"/index.ts").unwrap().source();
        let pool =
            tsr_compiler::CompilerCheckerPool::new(program.clone(), &tsr_arena::Counters::new());
        let mut checker = pool.checker_for_file_exclusive(source).unwrap();
        let mut service = LanguageService::new(
            &program,
            tsr_jsstring::PositionEncoding::Utf16,
            CancellationToken::new(),
        );
        let targets = service
            .cross_project_targets(
                &mut checker,
                &lsp::DocumentUri("file:///index.ts".into()),
                &lsp::Position {
                    line: 0,
                    character: 28,
                },
                false,
                false,
            )
            .unwrap();
        let default = targets.default_definition.unwrap();
        assert_eq!(default.position.uri.0, "file:///lib.d.ts");
        assert_eq!(default.position.position.character, 24);
        let source = default.source.unwrap();
        assert_eq!(source.uri.0, "file:///source.ts");
        assert_eq!((source.position.line, source.position.character), (1, 16));
        assert!(targets
            .original_positions
            .iter()
            .any(|position| position.uri.0 == "file:///source.ts"
                && position.position.line == 1
                && position.position.character == 16));
    }
}
