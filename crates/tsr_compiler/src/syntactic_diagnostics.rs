//! Program syntactic diagnostics: parser and JavaScript grammar records, plus the
//! option-dependent JavaScript checks the checker owns only for checked files.
use crate::{Error, Program, ProgramFile};
use std::ops::ControlFlow;
use ts_ast::{
    subtree_flags, AstView, ChildVisitor, Diagnostic, NodeId, NodeListId, NodeSlice,
    SyntaxKind as K,
};
use ts_core::{CompilerOptions, TextRange};

impl Program {
    /// One retained file, or every program file when `file` is absent.
    /// port: tsc/internal/compiler/program.go:Program.GetSyntacticDiagnostics
    // port: tsc/internal/compiler/program.go:Program.collectDiagnostics
    pub fn syntactic_diagnostics(
        &self,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let mut diagnostics = Vec::new();
        match file {
            Some(file) => {
                self.retained_source(file)?;
                self.file_syntactic_diagnostics(file, &mut diagnostics)?;
            }
            None => {
                for file in self.files() {
                    self.file_syntactic_diagnostics(file, &mut diagnostics)?;
                }
            }
        }
        self.filter_and_sort_diagnostics(&diagnostics)
    }

    fn file_syntactic_diagnostics(
        &self,
        file: &ProgramFile,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(), Error> {
        let source = file.bound().view().source_file()?;
        diagnostics.extend_from_slice(source.diagnostics());
        diagnostics.extend_from_slice(source.js_diagnostics());
        if ts_ast::utilities::is_source_file_js(&source)
            && !ts_ast::utilities::is_check_js_enabled_for_file(&source, self.options())
        {
            additional_js_syntactic_diagnostics(
                file.bound().view().ast(),
                file.source(),
                self.options(),
                diagnostics,
            )?;
        }
        Ok(())
    }
}

/// The parser reports option-free JavaScript grammar. Parameter decorators need
/// `experimentalDecorators`, and an unchecked file never reaches that checker error.
/// port: tsc/internal/compiler/program.go:getAdditionalJSSyntacticDiagnostics
fn additional_js_syntactic_diagnostics(
    view: AstView<'_>,
    file: NodeId,
    options: &CompilerOptions,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), Error> {
    if options.experimental_decorators.is_true() {
        return Ok(());
    }
    // Explicit preorder stack: the source visitor recurses once per nesting level.
    let mut pending = Vec::new();
    push_children(view, file, &mut pending)?;
    while let Some(node) = pending.pop() {
        if view.subtree_facts(node) & subtree_flags::DECORATORS == 0 {
            continue;
        }
        let read = view.node(node)?;
        if read.kind() == K::Parameter && ts_ast::utilities_middle::has_decorators(view, &read)? {
            for modifier in view
                .node_slice(read.modifier_nodes(view)?)?
                .iter()
                .flatten()
            {
                let decorator = view.node(modifier)?;
                if ts_ast::is_decorator(&decorator) {
                    diagnostics.push(Diagnostic::new(
                        Some(file),
                        TextRange::new(i64::from(decorator.pos()), i64::from(decorator.end())),
                        ts_diagnostics::Decorators_are_not_valid_here,
                        Vec::new(),
                    ));
                    break;
                }
            }
        }
        push_children(view, node, &mut pending)?;
    }
    Ok(())
}

/// Stacks immediate children so they pop in source order.
fn push_children(view: AstView<'_>, node: NodeId, pending: &mut Vec<NodeId>) -> Result<(), Error> {
    let start = pending.len();
    let mut children = Children {
        view,
        nodes: pending,
        error: None,
    };
    let _ = view.node(node)?.for_each_child(&mut children);
    if let Some(error) = children.error {
        return Err(error.into());
    }
    pending[start..].reverse();
    Ok(())
}

struct Children<'a, 'b> {
    view: AstView<'a>,
    nodes: &'b mut Vec<NodeId>,
    error: Option<ts_arena::Error>,
}

impl ChildVisitor for Children<'_, '_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{FileCache, Program, ProgramOptions};
    use std::sync::Arc;
    use ts_core::{CompilerOptions, ScriptTarget, Tristate};
    use ts_jsstring::JsString;

    // Pinned compiler/parameterDecoratorInJsFile.ts; `@dec` spans 60..64.
    const DECORATED: &[u8] =
        b"function dec(target, key, index) {}\n\nclass Foo {\n    method(@dec x) {}\n}";

    fn program(name: &[u8], text: &[u8], options: CompilerOptions) -> Program {
        let mut files = ts_vfs::MemoryBuilder::new(b"/", true);
        files.insert_loaded(name, text);
        Program::load(
            ProgramOptions {
                config: ts_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        allow_js: Tristate::TRUE,
                        no_emit: Tristate::TRUE,
                        no_lib: Tristate::TRUE,
                        ..options
                    },
                    vec![JsString::from_bytes(name)],
                ),
                host: Arc::new(files.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/missing".as_slice()),
                skip_module_resolution: true,
            },
            &mut FileCache::new(),
            &ts_arena::Counters::new(),
        )
        .unwrap()
    }

    fn observed(program: &Program) -> Vec<(i32, i64, i64)> {
        program
            .syntactic_diagnostics(None)
            .unwrap()
            .iter()
            .map(|d| (d.code, d.loc.pos(), d.loc.end()))
            .collect()
    }

    fn check_js(value: Tristate) -> CompilerOptions {
        CompilerOptions {
            check_js: value,
            ..Default::default()
        }
    }

    #[test]
    fn unchecked_javascript_reports_parameter_decorators() {
        // parameterDecoratorInJsFile(checkjs=false).errors.txt: a.js(4,12) TS1206.
        let unchecked = program(b"/a.js", DECORATED, check_js(Tristate::FALSE));
        assert_eq!(observed(&unchecked), [(1206, 60, 64)]);
        let file = unchecked.file(b"/a.js").unwrap();
        assert_eq!(
            unchecked.syntactic_diagnostics(Some(file)).unwrap().len(),
            1
        );
        // checkjs=true leaves the grammar error to the checker.
        assert!(observed(&program(b"/a.js", DECORATED, check_js(Tristate::TRUE))).is_empty());
        // The file directive outranks the option in both directions.
        let nocheck = [b"// @ts-nocheck\n".as_slice(), DECORATED].concat();
        assert_eq!(
            observed(&program(b"/a.js", &nocheck, check_js(Tristate::TRUE))),
            [(1206, 75, 79)]
        );
        let checked = [b"// @ts-check\n".as_slice(), DECORATED].concat();
        assert!(observed(&program(b"/a.js", &checked, check_js(Tristate::FALSE))).is_empty());
        let experimental = CompilerOptions {
            experimental_decorators: Tristate::TRUE,
            ..check_js(Tristate::FALSE)
        };
        assert!(observed(&program(b"/a.js", DECORATED, experimental)).is_empty());
        assert!(observed(&program(b"/a.ts", DECORATED, check_js(Tristate::FALSE))).is_empty());
    }

    #[test]
    fn javascript_grammar_records_join_sorted_parser_diagnostics() {
        // jsFileCompilationTypeOfParameter.errors.txt: a.js(1,15) TS8010. The
        // parse error follows it in source order although parser records come first.
        let options = CompilerOptions {
            target: ScriptTarget::ES2015,
            ..Default::default()
        };
        let text = b"function F(a: number) { }\nlet x = ;";
        let observed = observed(&program(b"/a.js", text, options));
        assert_eq!(observed[0], (8010, 14, 20));
        assert_eq!(
            observed.iter().map(|d| d.0).collect::<Vec<_>>(),
            [8010, 1109]
        );
    }

    #[test]
    fn foreign_file_handles_are_rejected() {
        let first = program(b"/a.js", DECORATED, check_js(Tristate::FALSE));
        let second = program(b"/a.js", DECORATED, check_js(Tristate::FALSE));
        assert!(matches!(
            first.syntactic_diagnostics(Some(second.file(b"/a.js").unwrap())),
            Err(crate::Error::Ast(ts_arena::Error::WrongOwner))
        ));
    }
}
