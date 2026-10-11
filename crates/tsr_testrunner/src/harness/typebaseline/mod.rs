//! The pinned tsbaseline type/symbol walker, over production checker APIs.
//! This harness records queries as executed, rather than replaying the oracle's
//! query list. Types are walked before symbols on the same checker operation.
mod decorate;
#[cfg(test)]
mod tests;
mod walker_api {
    pub use crate::portable_compiler::protocol::observation_rows;
    pub use tsr_checker::{
        symbol_format_flags, type_flags as tf, type_format_flags as ff, Operation, TypeRef,
    };
    pub use tsr_compiler::Program;
    pub use tsr_nodebuilder::{flags as builder_flags, internal_flags as builder_internal_flags};
}
#[path = "../../../../../tools/phase7/portable/walker/mod.rs"]
pub mod walker;
pub use walker::{NoTiming, Timing, Trace};

use serde_json::{json, Value};
use tsr_checker::{Operation, TypeRef};
use tsr_compiler::Program;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Row {
    line: usize,
    source_text: Vec<u8>,
    display: Vec<u8>,
}

pub struct InputFile<'a> {
    pub name: &'a [u8],
    pub content: &'a [u8],
}

/// Decorates observations returned by a tested artifact. A missing display
/// stops the symbol domain, exactly as the native walker does; no query is
/// performed here to fill it in.
pub fn observed_file(
    input: &InputFile<'_>,
    observations: Vec<walker::rows::Row>,
    symbols: bool,
) -> Result<Option<Vec<u8>>> {
    let rows = observations
        .into_iter()
        .map(decorate::observation_row)
        .collect::<Result<Vec<_>>>()?;
    if symbols && rows.iter().any(|row| row.display.is_empty()) {
        return Ok(None);
    }
    decorate::file(input.name, input.content, &rows).map(Some)
}

/// The shared baseline envelope after all executed file observations.
pub fn observed_baseline(output: &[u8], header: &[u8]) -> Option<Vec<u8>> {
    if output.is_empty() {
        return None;
    }
    let mut full = b"//// [".to_vec();
    full.extend_from_slice(header);
    full.extend_from_slice(b"] ////\r\n\r\n");
    full.extend_from_slice(output);
    Some(full)
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}

struct Walker<'a, 'checkers, 'operation> {
    program: &'a Program,
    /// The checker of each file (one, or every checker of the pool).
    op: &'a mut tsr_compiler::FileCheckers<'checkers, 'operation>,
    had_errors: bool,
    trace: &'a mut Trace,
    timing: &'a mut dyn Timing,
    /// Each walked type with the index of the checker that owns it.
    type_strings: Vec<(Value, TypeRef, usize)>,
}
impl Walker<'_, '_, '_> {
    fn baseline(&mut self, files: &[InputFile<'_>], header: &[u8], symbols: bool) -> Result<Value> {
        let text = self.baseline_text(files, header, symbols)?;
        self.timing.pause();
        let result = match text {
            None => json!({"state":"no_content"}),
            Some(full) => json!({"state":"content","text_hex":hex(&full)}),
        };
        self.timing.resume();
        Ok(result)
    }

    /// One kind of the walk: the `.types` text, or with `symbols` the
    /// `.symbols` text; `None` is `<no content>`.
    // port: tsc/internal/testutil/tsbaseline/type_symbol_baseline.go:generateBaseline
    // port: tsc/internal/testutil/tsbaseline/type_symbol_baseline.go:iterateBaseline
    fn baseline_text(
        &mut self,
        files: &[InputFile<'_>],
        header: &[u8],
        symbols: bool,
    ) -> Result<Option<Vec<u8>>> {
        let mut output = Vec::new();
        for input in files {
            let path = tsr_tspath::to_path(
                input.name,
                self.program.current_directory(),
                self.program.host().use_case_sensitive_file_names(),
            );
            let source = self
                .program
                .file(path.as_bytes())
                .ok_or("baseline source absent from program")?
                .source();
            // The pin's walker asks for this file's checker.
            self.op.select(source)?;
            let owner_key = self.op.current();
            let rows = walker::Collector::new(
                self.program,
                self.op.current_mut(),
                self.had_errors,
                self.trace,
                self.timing,
                &mut self.type_strings,
                owner_key,
            )
            .collect(source, symbols)?;
            let rows: Vec<_> = rows
                .into_iter()
                .map(decorate::observation_row)
                .collect::<Result<_>>()?;
            if symbols && rows.iter().any(|row| row.display.is_empty()) {
                break;
            }
            self.timing.pause();
            let decorated = decorate::file(input.name, input.content, &rows);
            output.extend_from_slice(&decorated?);
            self.timing.resume();
        }
        if output.is_empty() {
            return Ok(None);
        }
        self.timing.pause();
        let full = observed_baseline(&output, header);
        self.timing.resume();
        Ok(full)
    }
}

/// The pin's `typeWriterWalker` over the checkers a caller holds: each
/// [`TypeWriterWalker::text`] is one `checkBaselines` walk, run in the
/// order the caller runs the `type` and `symbol` sub-tests, on the same
/// checkers, so a failed walk leaves the other kind to run.
pub struct TypeWriterWalker<'a, 'checkers, 'operation> {
    walker: Walker<'a, 'checkers, 'operation>,
}

impl<'a, 'checkers, 'operation> TypeWriterWalker<'a, 'checkers, 'operation> {
    // port: tsc/internal/testutil/tsbaseline/type_symbol_baseline.go:newTypeWriterWalker
    pub fn new(
        program: &'a Program,
        op: &'a mut tsr_compiler::FileCheckers<'checkers, 'operation>,
        had_errors: bool,
        trace: &'a mut Trace,
        timing: &'a mut dyn Timing,
    ) -> Self {
        Self {
            walker: Walker {
                program,
                op,
                had_errors,
                trace,
                timing,
                type_strings: Vec::new(),
            },
        }
    }

    /// The `.types` text of `files`, or with `symbols` their `.symbols`
    /// text, under `header`; `None` is `<no content>`.
    pub fn text(
        &mut self,
        files: &[InputFile<'_>],
        header: &[u8],
        symbols: bool,
    ) -> Result<Option<Vec<u8>>> {
        self.walker.baseline_text(files, header, symbols)
    }
}

#[allow(dead_code)] // Untimed entry point used by the P5 display harness.
pub fn generate(
    program: &Program,
    op: &mut Operation<'_>,
    files: &[InputFile<'_>],
    header: &[u8],
    had_errors: bool,
    trace: &mut Trace,
) -> Value {
    generate_with_timing(program, op, files, header, had_errors, trace, &mut NoTiming)
}

pub fn generate_with_timing(
    program: &Program,
    op: &mut Operation<'_>,
    files: &[InputFile<'_>],
    header: &[u8],
    had_errors: bool,
    trace: &mut Trace,
    timing: &mut dyn Timing,
) -> Value {
    let mut checkers = tsr_compiler::FileCheckers::single(op);
    generate_for_checkers(
        program,
        &mut checkers,
        files,
        header,
        had_errors,
        trace,
        timing,
    )
}

/// The walk with each file's own checker (the pin's `GetTypeCheckerForFile`).
pub fn generate_for_checkers(
    program: &Program,
    op: &mut tsr_compiler::FileCheckers<'_, '_>,
    files: &[InputFile<'_>],
    header: &[u8],
    had_errors: bool,
    trace: &mut Trace,
    timing: &mut dyn Timing,
) -> Value {
    let mut walker = Walker {
        program,
        op,
        had_errors,
        trace,
        timing,
        type_strings: Vec::new(),
    };
    let result = (|| {
        let types = walker.baseline(files, header, false)?;
        let symbols = walker.baseline(files, header, true)?;
        Ok::<_, Box<dyn std::error::Error>>((types, symbols))
    })();
    walker.timing.pause();
    let result = match result {
        Ok((types, symbols)) => {
            let mut result = json!({"state":"executed","types":types,"symbols":symbols,"queries":walker.trace.queries});
            if walker.trace.collect_type_strings {
                let display = (|| -> Result<Vec<Value>> {
                    let mut rows = Vec::new();
                    for (mut row, typ, checker) in walker.type_strings {
                        if walker.trace.record_queries {
                            walker.trace.active = row.clone();
                        } else {
                            walker.trace.count("TypeToString");
                        }
                        walker.timing.resume();
                        let text = {
                            walker.op.checker(checker).type_to_string(typ,
                                tsr_checker::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                                    | tsr_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE)
                        };
                        walker.timing.pause();
                        row["text_hex"] = json!(hex(text?.as_bytes()));
                        rows.push(row);
                    }
                    Ok(rows)
                })();
                result["public_type_strings"] = match display {
                    Ok(rows) => json!({"state":"executed","queries":rows}),
                    Err(error) => {
                        json!({"state":"failed","reason":error.to_string(),"active_query":walker.trace.active})
                    }
                };
            }
            if !walker.trace.record_queries {
                result["counts"] = json!(walker.trace.counts);
            }
            result
        }
        Err(error) => {
            json!({"state":"failed","class":"walker_error","reason":error.to_string(),"active_query":walker.trace.active,"queries":walker.trace.queries})
        }
    };
    walker.timing.resume();
    result
}
