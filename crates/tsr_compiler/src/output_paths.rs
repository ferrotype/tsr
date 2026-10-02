//! The output-name calculation needed by option diagnostics. No files are emitted.
use crate::{Error, Program, ProgramFile};
use tsr_core::ScriptKind;
use tsr_jsstring::JsString;
pub(crate) use tsr_tsoptions::output_paths::{build_info_file, computed_common};
use tsr_tsoptions::output_paths::{
    get_common_source_directory, get_output_declaration_file_name_worker,
    get_output_js_file_name_worker, get_output_paths_for, get_source_file_path_in_new_dir_worker,
    ForceEmitPaths, OutputPathsHost,
};
use tsr_tspath as path;

/// The program as `OutputPathsHost` (Go's `emitHost` over a `Program`), with
/// the common source directory the caller already computed.
struct ProgramOutputPathsHost<'a> {
    program: &'a Program,
    common_source_directory: JsString,
}
impl<'a> ProgramOutputPathsHost<'a> {
    fn new(program: &'a Program, common_source_directory: &[u8]) -> Self {
        Self {
            program,
            common_source_directory: JsString::from_bytes(common_source_directory),
        }
    }
}
impl OutputPathsHost for ProgramOutputPathsHost<'_> {
    fn common_source_directory(&mut self) -> JsString {
        self.common_source_directory.clone()
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.program.content_mapper_extensions()
    }
    fn get_current_directory(&self) -> &[u8] {
        self.program.current_directory()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.program.use_case_sensitive_file_names()
    }
}

fn separator(mut directory: Vec<u8>) -> Vec<u8> {
    if !directory.is_empty() && !matches!(directory.last(), Some(b'/' | b'\\')) {
        directory.push(b'/');
    }
    directory
}
/// port: tsc/internal/outputpaths/commonsourcedirectory.go:GetComputedCommonSourceDirectory
pub(crate) fn computed_common_directory(files: &[JsString], program: &Program) -> Vec<u8> {
    separator(computed_common(
        files,
        program.current_directory(),
        program.use_case_sensitive_file_names(),
    ))
}
/// Path-only `GetCommonSourceDirectory`; option verification supplies the
/// source membership diagnostics, in the source function's call order.
pub(crate) fn common_directory(program: &Program, files: &[JsString]) -> Vec<u8> {
    get_common_source_directory(
        program.options(),
        || files.to_vec(),
        program.current_directory(),
        program.use_case_sensitive_file_names(),
        None,
    )
}
/// `Program.CommonSourceDirectory` for the files the program may emit; its
/// callers compute it once.
pub(crate) fn common_source_directory(program: &Program) -> Result<Vec<u8>, tsr_arena::Error> {
    let mut files = Vec::new();
    for file in program.files() {
        if may_emit_with_force_dts(file, program, false)? {
            let source = file.bound().view().source_file()?;
            files.push(source.parse_options().file_name.clone());
        }
    }
    Ok(common_directory(program, &files))
}

pub(crate) fn may_emit(file: &ProgramFile, program: &Program) -> Result<bool, Error> {
    Ok(may_emit_with_force(file, program, false, false)?)
}

pub(crate) fn may_emit_with_force_dts(
    file: &ProgramFile,
    program: &Program,
    force_dts_emit: bool,
) -> Result<bool, tsr_arena::Error> {
    may_emit_with_force(file, program, force_dts_emit, false)
}

/// port: tsc/internal/compiler/emitter.go:sourceFileMayBeEmitted
pub(crate) fn may_emit_with_force(
    file: &ProgramFile,
    program: &Program,
    force_dts_emit: bool,
    force_js_emit: bool,
) -> Result<bool, tsr_arena::Error> {
    let source = file.bound().view().source_file()?;
    let options = program.options();
    // Js files are emitted only if option is enabled
    if !force_js_emit && options.no_emit_for_js_files.is_true() && source.is_js() {
        return Ok(false);
    }
    // Declaration files are not emitted
    if source.is_declaration_file {
        return Ok(false);
    }
    if !source.content_mapper().is_empty() && !force_dts_emit && !options.emit_declarations() {
        return Ok(false);
    }
    if program.is_external_library(source.parse_options().path.as_bytes()) {
        return Ok(false);
    }
    // forcing dts emit => file needs to be emitted
    if force_dts_emit || force_js_emit {
        return Ok(true);
    }
    // Source files from referenced projects are not emitted. Only a source
    // without a declaration output (a declaration or JSON file) is loaded.
    if program
        .project_reference_from_source(source.parse_options().path.as_bytes())
        .is_some()
    {
        return Ok(false);
    }
    if source.script_kind != ScriptKind::JSON {
        return Ok(true);
    }
    if options.out_dir.is_empty() {
        return Ok(false);
    }
    if !options.root_dir.is_empty() || !options.config_file_path.is_empty() {
        let common = path::absolute(&common_directory(program, &[]), program.current_directory());
        let output = get_source_file_path_in_new_dir_worker(
            source.parse_options().file_name.as_bytes(),
            options.out_dir.as_bytes(),
            program.current_directory(),
            &common,
            program.use_case_sensitive_file_names(),
        );
        if path::compare_paths(
            source.parse_options().file_name.as_bytes(),
            &output,
            program.current_directory(),
            program.use_case_sensitive_file_names(),
        )
        .is_eq()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
/// `GetOutputPathsFor` without forced paths, as the four output names in
/// `OutputPaths` order: JavaScript, source map, declaration, declaration map.
pub(crate) fn output_names(
    file: &ProgramFile,
    program: &Program,
    common: &[u8],
) -> Result<[Vec<u8>; 4], Error> {
    let source = file.bound().view().source_file()?;
    let paths = get_output_paths_for(
        &source,
        program.options(),
        &mut ProgramOutputPathsHost::new(program, common),
        ForceEmitPaths::default(),
    );
    Ok([
        paths.js_file_path().to_vec(),
        paths.source_map_file_path().to_vec(),
        paths.declaration_file_path().to_vec(),
        paths.declaration_map_path().to_vec(),
    ])
}
/// `GetOutputPathsFor(file, options, host, force)` over the program, with the
/// common source directory the caller computed.
pub(crate) fn output_paths_for(
    file: &ProgramFile,
    program: &Program,
    common: &[u8],
    force: ForceEmitPaths,
) -> Result<tsr_tsoptions::output_paths::OutputPaths, tsr_arena::Error> {
    let source = file.bound().view().source_file()?;
    Ok(get_output_paths_for(
        &source,
        program.options(),
        &mut ProgramOutputPathsHost::new(program, common),
        force,
    ))
}
/// Unconditional workers used by import-map inversion, regardless of emit flags.
pub(crate) fn module_specifier_output_name(
    file: &[u8],
    program: &Program,
    common: &[u8],
    declaration: bool,
) -> Vec<u8> {
    let mut host = ProgramOutputPathsHost::new(program, common);
    if declaration {
        return get_output_declaration_file_name_worker(file, program.options(), &mut host);
    }
    get_output_js_file_name_worker(file, program.options(), &mut host)
}

impl Program {
    /// The files whose JavaScript `Program.Emit` transforms, in program order:
    /// the emitted files with a JavaScript output path that `noEmit` and the
    /// blocked outputs leave in place, as `emitter.emitJSFile` selects them.
    /// `noEmitOnError` needs the program's diagnostics and stays the caller's.
    pub fn javascript_emit_files(&self) -> Result<Vec<&std::sync::Arc<ProgramFile>>, Error> {
        if self.options().no_emit.is_true() {
            return Ok(Vec::new());
        }
        let mut emitted = Vec::new();
        for file in self.files() {
            if may_emit(file, self)? {
                emitted.push(file);
            }
        }
        let names = emitted
            .iter()
            .map(|file| {
                file.bound()
                    .view()
                    .source_file()
                    .map(|source| source.parse_options().file_name.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let common = common_directory(self, &names);
        let blocked = &self.option_verification().blocked_output_paths;
        let mut result = Vec::new();
        for file in emitted {
            let [javascript, ..] = output_names(file, self, &common)?;
            if !javascript.is_empty() && !blocked.contains(&self.to_path(&javascript)) {
                result.push(file);
            }
        }
        Ok(result)
    }
}
