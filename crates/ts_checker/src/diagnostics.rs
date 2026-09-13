//! Checker-local diagnostic admission and duplicate declaration details. The
//! operation already owns exclusive checker access; no second lock is needed.

use crate::{program::ProgramContext, types::Map, CheckerState, Error};
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{AstView, Diagnostic, JsString};
use ts_core::TextRange;
use ts_diagnostics::{self as messages, Message};

#[derive(Default)]
struct DiagnosticBucket {
    indices: Vec<usize>,
    sorted: bool,
}

#[derive(Default)]
pub(crate) struct DiagnosticStore {
    entries: Vec<Diagnostic>,
    locations: Map<(JsString, TextRange, i32), Vec<usize>>,
    files: Map<JsString, DiagnosticBucket>,
    globals: DiagnosticBucket,
}

impl DiagnosticStore {
    // port: tsc/internal/ast/diagnostic.go:DiagnosticsCollection.Add
    fn add<'a>(
        &mut self,
        diagnostic: Diagnostic,
        path: JsString,
        file_name: &impl Fn(NodeId) -> Result<&'a [u8], Error>,
    ) -> Result<usize, Error> {
        validate_files(&diagnostic, file_name)?;
        let key = (path.clone(), diagnostic.loc, diagnostic.code);
        if let Some(indices) = self.locations.get(&key) {
            for &index in indices {
                if ts_ast::equal_diagnostics(&self.entries[index], &diagnostic, file_name)? {
                    return Ok(index);
                }
            }
        }
        let index = self.entries.len();
        let bucket = if diagnostic.file.is_some() {
            self.files.entry(path).or_default()
        } else {
            &mut self.globals
        };
        bucket.indices.push(index);
        bucket.sorted = false;
        self.locations.entry(key).or_default().push(index);
        self.entries.push(diagnostic);
        Ok(index)
    }

    fn for_file<'a, 'file>(
        &'a mut self,
        path: Option<&[u8]>,
        file_name: &impl Fn(NodeId) -> Result<&'file [u8], Error>,
    ) -> Vec<&'a Diagnostic> {
        let bucket = match path {
            Some(path) => match self.files.get_mut(path) {
                Some(bucket) => bucket,
                None => return Vec::new(),
            },
            None => &mut self.globals,
        };
        if !bucket.sorted {
            bucket.indices.sort_by(|&left, &right| {
                ts_ast::compare_diagnostics(&self.entries[left], &self.entries[right], file_name)
                    .expect("diagnostic source owners were validated at admission")
            });
            bucket.sorted = true;
        }
        bucket
            .indices
            .iter()
            .map(|&index| &self.entries[index])
            .collect()
    }
}

fn validate_files<'a>(
    diagnostic: &Diagnostic,
    file_name: &impl Fn(NodeId) -> Result<&'a [u8], Error>,
) -> Result<(), Error> {
    if let Some(file) = diagnostic.file {
        file_name(file)?;
    }
    for related in diagnostic
        .related_information
        .iter()
        .chain(&diagnostic.message_chain)
    {
        validate_files(related, file_name)?;
    }
    Ok(())
}

fn file_name<'a>(
    factory: AstView<'a>,
    program: Option<&'a ProgramContext>,
    file: NodeId,
) -> Result<&'a [u8], Error> {
    let view = match factory.node(file) {
        Ok(_) => factory,
        Err(ts_arena::Error::WrongOwner) => program
            .ok_or(Error::Unsupported("diagnostic without a checker program"))?
            .ast(file)?,
        Err(error) => return Err(error.into()),
    };
    Ok(view.source_file(file)?.file_name())
}

impl CheckerState {
    pub(crate) fn add_related_diagnostic(
        &mut self,
        index: usize,
        diagnostic: Diagnostic,
    ) -> Result<(), Error> {
        validate_files(&diagnostic, &|file| {
            file_name(self.factory.view(), self.program.as_ref(), file)
        })?;
        self.diagnostics
            .entries
            .get_mut(index)
            .ok_or(Error::MissingLink("related diagnostic index"))?
            .related_information
            .push(std::sync::Arc::new(diagnostic));
        self.diagnostics.globals.sorted = false;
        for bucket in self.diagnostics.files.values_mut() {
            bucket.sorted = false;
        }
        Ok(())
    }
    // port: tsc/internal/checker/utilities.go:NewDiagnosticForNode
    pub(crate) fn diagnostic_for_node(
        &self,
        node: Option<NodeId>,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Result<Diagnostic, Error> {
        let (file, range) = if let Some(node) = node {
            let view = self.ast(node)?;
            let file = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
                .ok_or(Error::MissingLink("diagnostic node source file"))?;
            (
                Some(file),
                ts_scanner::get_error_range_for_node(view, file, node)?,
            )
        } else {
            (None, TextRange::default())
        };
        Ok(Diagnostic::new(file, range, message, args))
    }

    // port: tsc/internal/checker/checker.go:Checker.addDiagnostic
    /// The returned index names the checker-owned record to which related
    /// information can be attached. At the native serialization limit the
    /// diagnostic is discarded and no storage index is returned.
    pub(crate) fn add_diagnostic(
        &mut self,
        diagnostic: Diagnostic,
    ) -> Result<Option<usize>, Error> {
        self.add_to_diagnostic_store(diagnostic, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.addSuggestionDiagnostic
    pub(crate) fn add_suggestion_diagnostic(
        &mut self,
        diagnostic: Diagnostic,
    ) -> Result<Option<usize>, Error> {
        self.add_to_diagnostic_store(diagnostic, true)
    }

    fn add_to_diagnostic_store(
        &mut self,
        diagnostic: Diagnostic,
        suggestion: bool,
    ) -> Result<Option<usize>, Error> {
        if self.serialization_level >= 2 {
            return Ok(None);
        }
        let path = diagnostic.file.map_or(Ok(JsString::default()), |file| {
            Ok::<_, Error>(
                self.ast(file)?
                    .source_file(file)?
                    .parse_options()
                    .path
                    .clone(),
            )
        })?;
        let factory = self.factory.view();
        let program = self.program.as_ref();
        let store = if suggestion {
            &mut self.suggestions
        } else {
            &mut self.diagnostics
        };
        store
            .add(diagnostic, path, &|file| file_name(factory, program, file))
            .map(Some)
    }

    // port: tsc/internal/checker/checker.go:Checker.error
    pub(crate) fn error_at(
        &mut self,
        node: Option<NodeId>,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Result<Option<usize>, Error> {
        self.add_diagnostic(self.diagnostic_for_node(node, message, args)?)
    }

    /// Borrow records in native stable diagnostic order. Only a dirty bucket is
    /// sorted; repeated calls allocate the result pointer list, not diagnostics.
    pub(crate) fn diagnostics_for_file(
        &mut self,
        file: Option<NodeId>,
    ) -> Result<Vec<&Diagnostic>, Error> {
        self.read_diagnostic_store(file, false)
    }

    pub(crate) fn suggestions_for_file(
        &mut self,
        file: Option<NodeId>,
    ) -> Result<Vec<&Diagnostic>, Error> {
        self.read_diagnostic_store(file, true)
    }

    fn read_diagnostic_store(
        &mut self,
        file: Option<NodeId>,
        suggestion: bool,
    ) -> Result<Vec<&Diagnostic>, Error> {
        let path = file
            .map(|file| {
                Ok::<_, Error>(
                    self.ast(file)?
                        .source_file(file)?
                        .parse_options()
                        .path
                        .clone(),
                )
            })
            .transpose()?;
        let factory = self.factory.view();
        let program = self.program.as_ref();
        let store = if suggestion {
            &mut self.suggestions
        } else {
            &mut self.diagnostics
        };
        Ok(
            store.for_file(path.as_ref().map(JsString::as_bytes), &|file| {
                file_name(factory, program, file)
            }),
        )
    }

    pub(crate) fn adjusted_node_for_error(
        &self,
        node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        node.map(|node| {
            Ok(ts_ast::get_name_of_declaration(self.ast(node)?, Some(node))?.or(Some(node)))
        })
        .transpose()
        .map(Option::flatten)
    }

    pub(crate) fn first_symbol_declaration(
        &self,
        symbol: SymbolId,
    ) -> Result<Option<NodeId>, Error> {
        Ok(self.symbol_declarations(symbol)?.first().flatten())
    }

    // port: tsc/internal/checker/checker.go:Checker.addDuplicateDeclarationError
    fn duplicate_declaration_error(
        &mut self,
        node: Option<NodeId>,
        message: &'static Message,
        symbol_name: &JsString,
        related_nodes: &[Option<NodeId>],
    ) -> Result<(), Error> {
        let error_node = self.adjusted_node_for_error(node)?.or(node);
        let Some(index) = self.error_at(error_node, message, vec![symbol_name.clone()])? else {
            return Ok(());
        };
        for &related in related_nodes {
            let adjusted = self.adjusted_node_for_error(related)?;
            if adjusted == error_node {
                continue;
            }
            let leading = self.diagnostic_for_node(
                adjusted,
                messages::X_0_was_also_declared_here,
                vec![symbol_name.clone()],
            )?;
            let following = self.diagnostic_for_node(adjusted, messages::X_and_here, Vec::new())?;
            let info = &self.diagnostics.entries[index].related_information;
            if info.len() >= 5 {
                continue;
            }
            let name = |file| file_name(self.factory.view(), self.program.as_ref(), file);
            let mut duplicate = false;
            for existing in info {
                if ts_ast::compare_diagnostics(existing, &leading, &name)?.is_eq()
                    || ts_ast::compare_diagnostics(existing, &following, &name)?.is_eq()
                {
                    duplicate = true;
                    break;
                }
            }
            if !duplicate {
                let next = if info.is_empty() { leading } else { following };
                self.diagnostics.entries[index]
                    .related_information
                    .push(Arc::new(next));
            }
        }
        Ok(())
    }

    pub(crate) fn duplicate_symbol_errors(
        &mut self,
        target: SymbolId,
        message: &'static Message,
        symbol_name: &JsString,
        source: SymbolId,
    ) -> Result<(), Error> {
        let declarations = self.symbol_declarations(target)?.to_vec();
        let related = self.symbol_declarations(source)?.to_vec();
        for node in declarations {
            self.duplicate_declaration_error(node, message, symbol_name, &related)?;
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl DiagnosticStore {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.vec_capacity("diagnostics", &self.entries, self.entries.capacity());
        for entry in &self.entries {
            census.diagnostic(entry);
        }
        census.map("diagnostics", &self.locations);
        for ((path, _, _), entries) in &self.locations {
            census.text("diagnostics", path);
            census.vec_capacity("diagnostics", entries, entries.capacity());
        }
        census.map("diagnostics", &self.files);
        for (path, bucket) in &self.files {
            census.text("diagnostics", path);
            census.vec_capacity("diagnostics", &bucket.indices, bucket.indices.capacity());
        }
        census.vec_capacity(
            "diagnostics",
            &self.globals.indices,
            self.globals.indices.capacity(),
        );
    }
}
