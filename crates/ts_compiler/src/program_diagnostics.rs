//! The include processor computes diagnostics at first observation. Verification
//! writes are already retained by Program; a cache hit only borrows this snapshot.
use crate::{Error, Program};
use std::{collections::BTreeMap, sync::OnceLock};
use ts_arena::Error as AstError;
use ts_ast::{Diagnostic, NodeId, SourceFileRead};
use ts_jsstring::JsString;
type LocationIndex<'a> = BTreeMap<(&'a [u8], i64, i64, i32), Vec<usize>>;

#[derive(Default)]
pub(crate) struct ProgramDiagnostics {
    value: OnceLock<Result<Snapshot, AstError>>,
}
struct Snapshot {
    program: Vec<Diagnostic>,
    globals: Vec<Diagnostic>,
    files: BTreeMap<JsString, Vec<Diagnostic>>,
}
impl ProgramDiagnostics {
    #[cfg(test)]
    pub(crate) fn initialized(&self) -> bool {
        self.value.get().is_some()
    }
    /// port: tsc/internal/compiler/includeprocessor.go:includeProcessor.getDiagnostics
    fn get(&self, program: &Program) -> Result<&Snapshot, AstError> {
        self.value
            .get_or_init(|| Snapshot::compute(program))
            .as_ref()
            .map_err(|error| *error)
    }
}

impl Snapshot {
    fn compute(program: &Program) -> Result<Self, AstError> {
        let file_name = |id| source_names(program, id).map(|(name, _)| name);
        let mut result = Self {
            program: Vec::new(),
            globals: Vec::new(),
            files: BTreeMap::new(),
        };
        let mut locations = LocationIndex::new();
        // The loader-only observation API remains unchanged. Its records are
        // retained here alongside the verifier's later source processing writes.
        for diagnostic in program.include_diagnostics() {
            result.add(diagnostic.clone(), program, &file_name, &mut locations)?;
        }
        for request in &program.option_verification().include_diagnostics {
            result.add(
                program.explain_file_include_with_reason(
                    request.file.as_bytes(),
                    None,
                    request.message,
                    request.args.clone(),
                )?,
                program,
                &file_name,
                &mut locations,
            )?;
        }
        sort(&mut result.globals, &file_name)?;
        for diagnostics in result.files.values_mut() {
            sort(diagnostics, &file_name)?;
        }
        result
            .program
            .clone_from(&program.option_verification().diagnostics);
        result.program.extend(result.globals.iter().cloned());
        validate_owners(&result.program, &file_name)?;
        ts_core::sort_like_go(&mut result.program, &mut |a, b| {
            ts_ast::compare_diagnostics(a, b, &file_name).expect("validated diagnostic owners")
        });
        result.program = compact_and_merge_related_infos(result.program, &file_name);
        Ok(result)
    }

    /// port: tsc/internal/ast/diagnostic.go:DiagnosticsCollection.Add
    fn add<'a>(
        &mut self,
        diagnostic: Diagnostic,
        program: &'a Program,
        file_name: &impl Fn(NodeId) -> Result<&'a [u8], AstError>,
        locations: &mut LocationIndex<'a>,
    ) -> Result<(), AstError> {
        let mut path = b"".as_slice();
        let bucket = if let Some(file) = diagnostic.file {
            path = source_names(program, file)?.1;
            if !self.files.contains_key(path) {
                self.files.insert(JsString::from_bytes(path), Vec::new());
            }
            self.files
                .get_mut(path)
                .expect("file diagnostic bucket inserted")
        } else {
            &mut self.globals
        };
        // CompareDiagnostics can equate diagnostics with different chain codes.
        // Check the complete location collision group, including non-adjacent
        // equal records, before the source's stable sort.
        let collisions = locations
            .entry((
                path,
                diagnostic.loc.pos(),
                diagnostic.loc.end(),
                diagnostic.code,
            ))
            .or_default();
        for &index in collisions.iter() {
            if ts_ast::equal_diagnostics(&bucket[index], &diagnostic, file_name)? {
                return Ok(());
            }
        }
        collisions.push(bucket.len());
        bucket.push(diagnostic);
        Ok(())
    }
}
fn source_state(program: &Program, source: NodeId) -> Result<SourceFileRead<'_>, AstError> {
    if let Some(config) = program
        .config()
        .config_file
        .iter()
        .chain(&program.config().config_dependencies)
        .find(|config| config.root == source)
    {
        config.file.view().source_file(source)
    } else {
        let index = program
            .owners
            .node_file_index(source)
            .ok_or(AstError::WrongOwner)?;
        program.files()[index]
            .bound()
            .view()
            .ast()
            .source_file(source)
    }
}
fn source_names(program: &Program, source: NodeId) -> Result<(&[u8], &[u8]), AstError> {
    let state = source_state(program, source)?;
    Ok((state.file_name(), state.parse_options().path.as_bytes()))
}

fn sort<'a>(
    diagnostics: &mut [Diagnostic],
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], AstError>,
) -> Result<(), AstError> {
    validate_owners(diagnostics, file_name)?;
    diagnostics.sort_by(|a, b| {
        ts_ast::compare_diagnostics(a, b, file_name).expect("validated diagnostic owners")
    });
    Ok(())
}
fn validate_owners<'a>(
    diagnostics: &[Diagnostic],
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], AstError>,
) -> Result<(), AstError> {
    fn validate<'a>(
        diagnostic: &Diagnostic,
        file_name: &impl Fn(NodeId) -> Result<&'a [u8], AstError>,
    ) -> Result<(), AstError> {
        if let Some(file) = diagnostic.file {
            file_name(file)?;
        }
        for related in &diagnostic.related_information {
            validate(related, file_name)?;
        }
        Ok(())
    }
    for diagnostic in diagnostics {
        validate(diagnostic, file_name)?;
    }
    Ok(())
}

/// port: tsc/internal/compiler/program.go:compactAndMergeRelatedInfos
fn compact_and_merge_related_infos<'a>(
    diagnostics: Vec<Diagnostic>,
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], AstError>,
) -> Vec<Diagnostic> {
    if diagnostics.len() < 2 {
        return diagnostics;
    }
    let mut result = Vec::with_capacity(diagnostics.len());
    let mut input = diagnostics.into_iter().peekable();
    while let Some(mut diagnostic) = input.next() {
        let mut merged = false;
        while input.peek().is_some_and(|next| {
            ts_ast::equal_diagnostics_no_related_info(&diagnostic, next, file_name)
                .expect("validated diagnostic owners")
        }) {
            merged = true;
            diagnostic
                .related_information
                .extend(input.next().expect("peeked diagnostic").related_information);
        }
        if merged && !diagnostic.related_information.is_empty() {
            ts_core::sort_like_go(&mut diagnostic.related_information, &mut |a, b| {
                ts_ast::compare_diagnostics(a, b, file_name).expect("validated diagnostic owners")
            });
            diagnostic.related_information.dedup_by(|a, b| {
                ts_ast::equal_diagnostics(a, b, file_name).expect("validated diagnostic owners")
            });
        }
        result.push(diagnostic);
    }
    result
}
impl Program {
    /// Sort and merge an already-produced diagnostic set without running queries.
    /// File IDs remain checked against this program, including related records.
    /// port: tsc/internal/compiler/program.go:SortAndDeduplicateDiagnostics
    pub fn sort_and_deduplicate_diagnostics(
        &self,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<Diagnostic>, Error> {
        let file_name = |id| source_names(self, id).map(|(name, _)| name);
        validate_owners(diagnostics, &file_name)?;
        let mut sorted = diagnostics.to_vec();
        ts_core::sort_like_go(&mut sorted, &mut |a, b| {
            ts_ast::compare_diagnostics(a, b, &file_name).expect("validated diagnostic owners")
        });
        Ok(compact_and_merge_related_infos(sorted, &file_name))
    }
    /// Unnecessary-code reports on content-mapped files survive only when their
    /// span maps back to original text; that span translation is not ported.
    /// port: tsc/internal/compiler/program.go:filterAndSortDiagnostics
    pub(crate) fn filter_and_sort_diagnostics(
        &self,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<Diagnostic>, Error> {
        for diagnostic in diagnostics {
            let Some(file) = diagnostic.file else {
                continue;
            };
            if diagnostic.reports_unnecessary
                && diagnostic.source.is_empty()
                && source_state(self, file)?.span_map().is_some()
            {
                return Err(Error::Unsupported(
                    "filterAndSortDiagnostics content-map span fidelity",
                ));
            }
        }
        self.sort_and_deduplicate_diagnostics(diagnostics)
    }
    /// Source Program.GetProgramDiagnostics: direct verifier diagnostics plus
    /// only the include processor's global diagnostics. Checker work is absent.
    /// port: tsc/internal/compiler/program.go:Program.GetProgramDiagnostics
    pub fn program_diagnostics(&self) -> Result<&[Diagnostic], Error> {
        Ok(&self.diagnostic_snapshot.get(self)?.program)
    }

    /// Raw include collection globals, before Program's direct diagnostics.
    pub fn global_include_diagnostics(&self) -> Result<&[Diagnostic], Error> {
        Ok(&self.diagnostic_snapshot.get(self)?.globals)
    }

    /// Raw include collection for a canonical source path. This is the source
    /// DiagnosticsCollection accessor; it does not apply checker skip/directive
    /// filtering from Program.GetIncludeProcessorDiagnostics.
    /// port: tsc/internal/ast/diagnostic.go:DiagnosticsCollection.GetDiagnosticsForFile
    pub fn include_diagnostics_for_file(&self, path: &[u8]) -> Result<&[Diagnostic], Error> {
        Ok(self
            .diagnostic_snapshot
            .get(self)?
            .files
            .get(path)
            .map_or(&[], Vec::as_slice))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use ts_core::TextRange;
    fn program() -> Program {
        let mut files = ts_vfs::MemoryBuilder::new(b"/", true);
        files.insert_loaded(b"/file.ts", b"let value = 1;".as_slice());
        Program::load(
            crate::ProgramOptions {
                config: ts_tsoptions::ParsedCommandLine::new(
                    ts_core::CompilerOptions {
                        no_lib: ts_core::Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/file.ts".as_slice())],
                ),
                host: Arc::new(files.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/missing".as_slice()),
                skip_module_resolution: true,
            },
            &mut crate::FileCache::new(),
            &ts_arena::Counters::new(),
        )
        .unwrap()
    }
    fn observe(diagnostic: &Diagnostic) -> Value {
        use std::fmt::Write;
        let arguments: Vec<_> = diagnostic
            .message_args
            .iter()
            .map(|arg| {
                let mut text = String::new();
                for byte in arg.as_bytes() {
                    write!(text, "{byte:02x}").unwrap();
                }
                text
            })
            .collect();
        json!({"File":"","Pos":diagnostic.loc.pos(),"End":diagnostic.loc.end(),"Code":diagnostic.code,"Args":arguments,"Chain":diagnostic.message_chain.iter().map(|d|observe(d)).collect::<Vec<_>>(),"Related":diagnostic.related_information.iter().map(|d|observe(d)).collect::<Vec<_>>()})
    }
    #[test]
    fn independently_observed_go_related_merge_and_sort_ties() {
        let requests: Vec<Value> = serde_json::from_str(include_str!(
            "../../../data/s07/include-merge-requests.json"
        ))
        .unwrap();
        let expected: Vec<Value> = serde_json::from_str(include_str!(
            "../../../data/s07/include-merge-observations.json"
        ))
        .unwrap();
        assert_eq!(requests.len(), expected.len());
        let program = program();
        for (request, expected) in requests.iter().zip(expected) {
            assert_eq!(request["id"], expected["id"]);
            let diagnostics: Vec<_> = request["diagnostics"].as_array().unwrap().iter().map(|recipe| {
                let mut diagnostic = Diagnostic::compiler(ts_diagnostics::File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files,vec![JsString::from_bytes(b"same".as_slice()),JsString::from_bytes(b"root".as_slice())]);
                let external = |code| Arc::new(Diagnostic::external(None,TextRange::new(-1,-1),JsString::default(),1,code,JsString::from_bytes(b"x".as_slice())));
                let chain = i32::try_from(recipe["Chain"].as_i64().unwrap()).unwrap();
                if chain != 0 {diagnostic.message_chain.push(external(9000+chain));}
                diagnostic.related_information = recipe["Related"].as_array().unwrap().iter().map(|value|external(8000+i32::try_from(value.as_i64().unwrap()).unwrap())).collect();
                diagnostic
            }).collect();
            let diagnostics = program
                .sort_and_deduplicate_diagnostics(&diagnostics)
                .unwrap();
            assert_eq!(
                json!({"id":request["id"],"diagnostics":diagnostics.iter().map(observe).collect::<Vec<_>>()}),
                expected,
                "{}",
                request["id"]
            );
        }
    }

    #[test]
    fn public_aggregation_rejects_foreign_related_file_before_sorting() {
        let first = program();
        let second = program();
        let diagnostic = Diagnostic::external(
            Some(second.files()[0].source()),
            TextRange::new(0, 1),
            JsString::default(),
            1,
            9999,
            JsString::from_bytes(b"message".as_slice()),
        );
        assert!(first
            .sort_and_deduplicate_diagnostics(std::slice::from_ref(&diagnostic))
            .is_err());
        let mut parent = Diagnostic::external(
            None,
            TextRange::new(-1, -1),
            JsString::default(),
            1,
            9999,
            JsString::default(),
        );
        parent.related_information.push(Arc::new(diagnostic));
        assert!(first.sort_and_deduplicate_diagnostics(&[parent]).is_err());
    }
}
