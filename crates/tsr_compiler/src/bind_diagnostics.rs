//! Binder-only program diagnostics. Files are bound before publication, so this
//! observation reads their retained results without invoking the checker.
use crate::{Error, Program};
use tsr_ast::{Diagnostic, NodeId};

impl Program {
    /// Binder diagnostics for one retained source-file ID, or all program files.
    /// Foreign IDs and IDs that do not identify a source file are rejected.
    ///
    /// Native `BindSourceFiles` runs on demand. This loader instead publishes
    /// only `CompletedFile`s, bound by `FileCache::acquire`, so repeated queries
    /// only read the existing result. Returned diagnostics contain non-owning
    /// file IDs; keep the program alive when resolving or rendering them.
    /// port: tsc/internal/compiler/program.go:Program.GetBindDiagnostics
    pub fn bind_diagnostics(&self, file: Option<NodeId>) -> Result<Vec<Diagnostic>, Error> {
        if let Some(file) = file {
            let index = self
                .owners
                .node_file_index(file)
                .ok_or(tsr_arena::Error::WrongOwner)?;
            let source = self.files()[index].bound().view().ast().source_file(file)?;
            return self.filter_and_sort_diagnostics(source.bind_diagnostics());
        }
        let mut diagnostics = Vec::new();
        for file in self.files() {
            let source = file.bound().view().source_file()?;
            diagnostics.extend_from_slice(source.bind_diagnostics());
        }
        self.filter_and_sort_diagnostics(&diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use crate::{FileCache, Program, ProgramOptions};
    use std::sync::Arc;
    use tsr_core::{CompilerOptions, Tristate};
    use tsr_jsstring::JsString;

    fn program() -> Program {
        let mut files = tsr_vfs::MemoryBuilder::new(b"/", true);
        files.insert_loaded(b"/a.ts", b"export {}; let a = 1; let a = 2;".as_slice());
        files.insert_loaded(b"/z.ts", b"export {}; let z = 1; let z = 2;".as_slice());
        files.insert_loaded(
            b"/clean.ts",
            b"export {}; let invalid = ; let x = missing;".as_slice(),
        );
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        no_emit: Tristate::TRUE,
                        ..Default::default()
                    },
                    [b"/z.ts".as_slice(), b"/a.ts", b"/clean.ts"]
                        .map(JsString::from_bytes)
                        .to_vec(),
                ),
                host: Arc::new(files.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/missing".as_slice()),
                skip_module_resolution: true,
            },
            &mut FileCache::new(),
            &tsr_arena::Counters::new(),
        )
        .unwrap()
    }

    #[test]
    fn binding_diagnostics_select_and_sort_without_parser_or_checker_records() {
        let program = program();
        let a = program.file(b"/a.ts").unwrap().source();
        let z = program.file(b"/z.ts").unwrap().source();
        let clean = program.file(b"/clean.ts").unwrap().source();
        let all = program.bind_diagnostics(None).unwrap();
        assert_eq!(
            all.iter().map(|d| d.file).collect::<Vec<_>>(),
            [Some(a), Some(a), Some(z), Some(z)]
        );
        assert!(all.iter().all(|d| d.code == 2451));
        let selected = program.bind_diagnostics(Some(z)).unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|d| d.file == Some(z)));
        assert!(program.bind_diagnostics(Some(clean)).unwrap().is_empty());
        assert_eq!(program.bind_diagnostics(None).unwrap().len(), all.len());
    }

    #[test]
    fn foreign_and_non_source_ids_are_rejected() {
        let first = program();
        let second = program();
        assert!(matches!(
            first.bind_diagnostics(Some(second.file(b"/a.ts").unwrap().source())),
            Err(crate::Error::Ast(tsr_arena::Error::WrongOwner))
        ));
        let file = first.file(b"/a.ts").unwrap();
        let view = file.bound().view().ast();
        let statements = view.node(file.source()).unwrap().statements(view).unwrap();
        let statement = view
            .node_slice(statements)
            .unwrap()
            .iter()
            .flatten()
            .next()
            .unwrap();
        assert!(first.bind_diagnostics(Some(statement)).is_err());
    }
}
