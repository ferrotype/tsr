//! Single-file JavaScript and declaration emit: the pin's `transpile`
//! package. [`transpile_module`] and [`transpile_declaration`] load the
//! source text as the only root of a program over an in-memory file system
//! (the pin's `transpileFS`), with module resolution skipped, and emit it
//! through [`CheckedProgram::emit`] with a write callback that keeps the one
//! output and its source map.
//!
//! Where the pin takes a context, these take a [`CheckerRequest`]: its token
//! cancels the emit, and a canceled transpilation returns `Ok(None)` (the
//! pin's nil output).
mod fs;

use fs::TranspileFs;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use tsr_arena::Counters;
use tsr_ast::Diagnostic;
use tsr_checker::CheckerRequest;
use tsr_compiler::{
    CheckedProgram, EmitOnly, EmitOptions, Error, FileCache, Program, ProgramOptions, WriteFileData,
};
use tsr_core::debug::{self, Argument};
use tsr_core::{CompilerOptions, JsxEmit, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;

/// Options configures single-file transpilation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options<'a> {
    /// CompilerOptions are the base compiler options to use for the
    /// transpilation. If `None`, a default set of compiler options is used.
    /// Regardless of what is provided, a number of options are
    /// unconditionally overridden; see [`transpile_module`] and
    /// [`transpile_declaration`]. The transpilation works on a copy.
    pub compiler_options: Option<&'a CompilerOptions>,

    /// FileName is the name given to the synthesized input file. It only
    /// needs to be provided if the source text relies on characteristics
    /// implied by the file's extension or path, e.g. its extension controls
    /// whether the file is parsed as a script or module, whether JSX syntax
    /// is allowed, etc. Defaults (when empty) to "module.ts", or
    /// "module.tsx" if `CompilerOptions.jsx` is set.
    pub file_name: &'a [u8],

    /// ReportDiagnostics indicates whether syntactic and compiler option
    /// diagnostics should be included in the result. Regardless of this
    /// setting, diagnostics produced while emitting (including declaration
    /// emit errors such as those produced by isolated declarations) are
    /// always included.
    pub report_diagnostics: bool,
}

/// Output contains the emitted text and any requested diagnostics.
pub struct Output {
    pub output_text: JsString,
    pub diagnostics: Vec<Diagnostic>,
    /// Empty when no source map was emitted.
    pub source_map_text: JsString,
    /// The transpilation's program, which owns the files the diagnostics
    /// name: a diagnostic's file is an id that keeps no storage alive, where
    /// the pin's diagnostic retains its source file.
    pub program: Arc<Program>,
}

/// inputDirectory is the synthetic current directory used to root the
/// single input file created for transpilation.
const INPUT_DIRECTORY: &[u8] = b"/";

/// libDirectory is the synthetic directory that the barebones default
/// library file is placed in for declaration transpilation. See
/// [`BAREBONES_LIB_CONTENT`].
const LIB_DIRECTORY: &[u8] = b"/lib";

/// Declaration emit works without a `lib`, but some local inferences you'd
/// expect to work won't without at least a minimal `lib` available, since
/// the checker will type inferred declarations as `any` without these
/// defined. Late bound symbol names, in particular, are impossible to define
/// without `Symbol` at least partially defined.
const BAREBONES_LIB_CONTENT: &str = "interface Boolean {}
interface Function {}
interface CallableFunction {}
interface NewableFunction {}
interface IArguments {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
interface Array<T> { length: number; [n: number]: T; }
interface SymbolConstructor {
    (desc?: string | number): symbol;
    for(name: string): symbol;
    readonly toStringTag: symbol;
}
declare var Symbol: SymbolConstructor;
interface Symbol {
    readonly [Symbol.toStringTag]: string;
}";

/// TranspileModule transpiles a single file of source text to JavaScript
/// using the specified options. If no compiler options are provided, a
/// default set of compiler options is used. It returns `Ok(None)` if the
/// request is canceled before emission completes.
///
/// Extra compiler options that are unconditionally used by this function are:
///   - IsolatedModules = true (unless VerbatimModuleSyntax is set, which
///     makes this option redundant)
///   - NoCheck = true
///   - NoResolve = true
///   - NoLib = true
///   - Declaration = false
///   - DeclarationMap = false
///   - IsolatedDeclarations = false
// port: tsc/internal/transpile/transpile.go:TranspileModule
pub fn transpile_module(
    request: &CheckerRequest,
    input: &[u8],
    options: &Options<'_>,
) -> Result<Option<Output>, Error> {
    transpile_worker(request, input, options, false /*declaration*/)
}

/// TranspileDeclaration creates a declaration (.d.ts) file from a single
/// file of source text using the specified options. If no compiler options
/// are provided, a default set of compiler options is used.
///
/// Note that, because only the single input file is available, the
/// resulting declaration file may differ from the one a full program
/// type-check and emit would produce.
///
/// Extra compiler options that are unconditionally used by this function are:
///   - IsolatedModules = true (unless VerbatimModuleSyntax is set, which
///     makes this option redundant)
///   - NoCheck = true
///   - NoResolve = true
///   - NoLib = false
///   - Declaration = true
///   - EmitDeclarationOnly = true
///   - IsolatedDeclarations = true
// port: tsc/internal/transpile/transpile.go:TranspileDeclaration
pub fn transpile_declaration(
    request: &CheckerRequest,
    input: &[u8],
    options: &Options<'_>,
) -> Result<Option<Output>, Error> {
    transpile_worker(request, input, options, true /*declaration*/)
}

/// What the write callback kept: the one output and its source map.
#[derive(Default)]
struct Written {
    output_text: Option<JsString>,
    source_map_text: Option<JsString>,
}

// port: tsc/internal/transpile/transpile.go:transpileWorker
fn transpile_worker(
    request: &CheckerRequest,
    input: &[u8],
    options: &Options<'_>,
    declaration: bool,
) -> Result<Option<Output>, Error> {
    let mut opts = match options.compiler_options {
        Some(compiler_options) => compiler_options.clone(),
        None => CompilerOptions::default(),
    };

    // Clear options that do not apply to single-file transpilation.
    opts.incremental = Tristate::UNKNOWN;
    opts.declaration = Tristate::UNKNOWN;
    opts.emit_declaration_only = Tristate::UNKNOWN;
    opts.no_emit = Tristate::UNKNOWN;
    opts.lib = None;
    opts.out_file = JsString::default();
    opts.composite = Tristate::UNKNOWN;
    opts.ts_build_info_file = JsString::default();
    opts.paths = None;
    opts.root_dirs = None;
    opts.types = None;
    opts.allow_importing_ts_extensions = Tristate::UNKNOWN;
    opts.no_emit_on_error = Tristate::UNKNOWN;
    opts.declaration_dir = JsString::default();

    // Do not set `isolatedModules` if `verbatimModuleSyntax` was supplied,
    // since it would be redundant.
    if !opts.verbatim_module_syntax.is_true() {
        opts.isolated_modules = Tristate::TRUE;
    }
    opts.no_check = Tristate::TRUE;
    opts.no_resolve = Tristate::TRUE;

    // transpileModule/transpileDeclaration do not write anything to disk, so
    // there's no need to verify there are no conflicts between input and
    // output paths.
    opts.suppress_output_path_check = Tristate::TRUE;

    // FileName can be a non-ts file.
    opts.allow_non_ts_extensions = Tristate::TRUE;

    if declaration {
        opts.declaration = Tristate::TRUE;
        opts.emit_declaration_only = Tristate::TRUE;
        opts.isolated_declarations = Tristate::TRUE;
    } else {
        opts.declaration = Tristate::FALSE;
        opts.declaration_map = Tristate::FALSE;
        opts.isolated_declarations = Tristate::FALSE;
    }

    // When transpiling declarations, we need a lib. GetDefaultLibFileName
    // will cause the barebones lib below to be used instead of a real lib.
    if declaration {
        opts.no_lib = Tristate::FALSE;
    } else {
        opts.no_lib = Tristate::TRUE;
    }

    // If jsx is specified, then treat the file as .tsx.
    let mut file_name = options.file_name;
    if file_name.is_empty() {
        if opts.jsx == JsxEmit::NONE {
            file_name = b"module.ts";
        } else {
            file_name = b"module.tsx";
        }
    }
    let input_file_name = tsr_tspath::absolute(file_name, INPUT_DIRECTORY);

    let mut files = BTreeMap::from([(input_file_name.clone(), input.to_vec())]);

    // Declaration emit needs a default lib to resolve global types (e.g.
    // `Array`, `Symbol`); plain transpilation sets NoLib so none is read.
    // The default lib name depends on the configured target.
    if declaration {
        let lib_file_name = tsr_tsoptions::default_lib_file_name(&opts);
        files.insert(
            tsr_tspath::combine(LIB_DIRECTORY, &[lib_file_name.as_bytes()]),
            BAREBONES_LIB_CONTENT.as_bytes().to_vec(),
        );
    }

    // The pin's `NewCompilerHost(inputDirectory, transpileFS, libDirectory,
    // nil, nil, nil)` reduces to the loader's host, current directory and
    // default library path.
    let counters = Counters::new();
    let program = Arc::new(Program::load(
        ProgramOptions {
            config: ParsedCommandLine::new(
                opts,
                vec![JsString::from_bytes(input_file_name.clone())],
            ),
            host: Arc::new(TranspileFs::new(files)),
            current_directory: JsString::from_bytes(INPUT_DIRECTORY),
            default_library_path: JsString::from_bytes(LIB_DIRECTORY),
            skip_module_resolution: true,
            single_threaded: Tristate::UNKNOWN,
        },
        &mut FileCache::new(),
        &counters,
    )?);
    let checked = CheckedProgram::new(program.clone(), &counters, None);

    let mut all_diagnostics = Vec::new();
    if options.report_diagnostics {
        let source_file = program.source_file(&input_file_name);
        all_diagnostics.extend(program.syntactic_diagnostics(source_file)?);
        all_diagnostics.extend(program.config_file_parsing_diagnostics());
        all_diagnostics.extend_from_slice(program.program_diagnostics()?);
    }

    let mut emit_only = EmitOnly::All;
    if declaration {
        emit_only = EmitOnly::Dts;
    }

    let written = Mutex::new(Written::default());
    let write_file = |file_name: &[u8], text: &[u8], _data: &mut WriteFileData| {
        let mut written = written.lock().unwrap_or_else(PoisonError::into_inner);
        if file_name.ends_with(b".map") {
            debug::assert(
                written.source_map_text.is_none(),
                &[Argument::String(&format!(
                    "Unexpected multiple source map outputs, file: {}",
                    String::from_utf8_lossy(file_name)
                ))],
            );
            written.source_map_text = Some(JsString::from_bytes(text));
        } else {
            debug::assert(
                written.output_text.is_none(),
                &[Argument::String(&format!(
                    "Unexpected multiple outputs, file: {}",
                    String::from_utf8_lossy(file_name)
                ))],
            );
            written.output_text = Some(JsString::from_bytes(text));
        }
        Ok(())
    };
    let result = checked.emit(
        request,
        &EmitOptions {
            emit_only,
            force_emit: declaration,
            write_file: Some(&write_file),
            ..EmitOptions::default()
        },
    )?;
    let Some(result) = result else {
        return Ok(None);
    };

    // Diagnostics produced during emit (e.g. isolated declaration errors)
    // are always included, regardless of ReportDiagnostics.
    all_diagnostics.extend(result.diagnostics);

    let written = written.into_inner().unwrap_or_else(PoisonError::into_inner);
    debug::assert(
        written.output_text.is_some(),
        &[Argument::String("Output generation failed")],
    );

    Ok(Some(Output {
        output_text: written.output_text.unwrap_or_default(),
        diagnostics: all_diagnostics,
        source_map_text: written.source_map_text.unwrap_or_default(),
        program,
    }))
}

#[cfg(test)]
mod tests;
