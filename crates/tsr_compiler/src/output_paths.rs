//! The output-name calculation needed by option diagnostics. No files are emitted.
use crate::{Error, Program, ProgramFile};
use tsr_core::{JsxEmit, ScriptKind};
pub(crate) use tsr_tsoptions::output_paths::{build_info_file, computed_common};
fn declaration_extension(file: &[u8]) -> Vec<u8> {
    tsr_tsoptions::output_paths::declaration_extension(file, &[])
}
use tsr_jsstring::JsString;
use tsr_tspath as path;

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
        program.host().use_case_sensitive_file_names(),
    ))
}
/// Path-only half of GetCommonSourceDirectory; option verification supplies the
/// source membership diagnostics, in the source function's call order.
pub(crate) fn common_directory(program: &Program, files: &[JsString]) -> Vec<u8> {
    let options = program.options();
    separator(if !options.root_dir.is_empty() {
        options.root_dir.as_bytes().to_vec()
    } else if !options.config_file_path.is_empty() {
        path::directory(options.config_file_path.as_bytes())
    } else {
        computed_common(
            files,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        )
    })
}
/// port: tsc/internal/outputpaths/outputpaths.go:GetSourceFilePathInNewDirWorker
pub(crate) fn source_in_new_directory(
    file: &[u8],
    new_dir: &[u8],
    program: &Program,
    common: &[u8],
) -> Vec<u8> {
    let absolute = path::absolute(file, program.current_directory());
    let rest = path::trim_file_path_prefix(
        &absolute,
        common,
        program.host().use_case_sensitive_file_names(),
    )
    .unwrap_or(&absolute);
    path::combine(new_dir, &[rest])
}
/// port: tsc/internal/compiler/emitter.go:sourceFileMayBeEmitted
pub(crate) fn may_emit(file: &ProgramFile, program: &Program) -> Result<bool, Error> {
    Ok(may_emit_with_force_dts(file, program, false)?)
}

pub(crate) fn may_emit_with_force_dts(
    file: &ProgramFile,
    program: &Program,
    force_dts_emit: bool,
) -> Result<bool, tsr_arena::Error> {
    let source = file.bound().view().source_file()?;
    let options = program.options();
    if options.no_emit_for_js_files.is_true() && source.is_js() || source.is_declaration_file {
        return Ok(false);
    }
    if !source.content_mapper().is_empty() && !force_dts_emit && !options.emit_declarations() {
        return Ok(false);
    }
    if program.is_external_library(source.parse_options().path.as_bytes()) {
        return Ok(false);
    }
    if force_dts_emit {
        return Ok(true);
    }
    // Project-reference and mapper execution are excluded by the S07 operation
    // boundary before a Program is constructed; no reference redirect is hidden.
    if source.script_kind != ScriptKind::JSON {
        return Ok(true);
    }
    if options.out_dir.is_empty() {
        return Ok(false);
    }
    if !options.root_dir.is_empty() || !options.config_file_path.is_empty() {
        let common = path::absolute(&common_directory(program, &[]), program.current_directory());
        let output = source_in_new_directory(
            source.parse_options().file_name.as_bytes(),
            options.out_dir.as_bytes(),
            program,
            &common,
        );
        if path::compare_paths(
            source.parse_options().file_name.as_bytes(),
            &output,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        )
        .is_eq()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn extension_is(file: &[u8], extension: &[u8]) -> bool {
    file.len() > extension.len() && file.ends_with(extension)
}
/// port: tsc/internal/outputpaths/outputpaths.go:GetOutputExtension
fn output_extension(file: &[u8], jsx: JsxEmit) -> &'static [u8] {
    if extension_is(file, b".json") {
        b".json"
    } else if jsx == JsxEmit::PRESERVE
        && (extension_is(file, b".jsx") || extension_is(file, b".tsx"))
    {
        b".jsx"
    } else if extension_is(file, b".mts") || extension_is(file, b".mjs") {
        b".mjs"
    } else if extension_is(file, b".cts") || extension_is(file, b".cjs") {
        b".cjs"
    } else {
        b".js"
    }
}
/// port: tsc/internal/outputpaths/outputpaths.go:GetOutputPathsFor
pub(crate) fn output_names(
    file: &ProgramFile,
    program: &Program,
    common: &[u8],
) -> Result<[Vec<u8>; 4], Error> {
    let source = file.bound().view().source_file()?;
    let options = program.options();
    let name = source.parse_options().file_name.as_bytes();
    let own = if options.out_dir.is_empty() {
        name.to_vec()
    } else {
        source_in_new_directory(name, options.out_dir.as_bytes(), program, common)
    };
    let own = [
        path::remove_file_extension(&own),
        output_extension(name, options.jsx),
    ]
    .concat();
    let json = source.script_kind == ScriptKind::JSON;
    let json_same = json
        && path::compare_paths(
            name,
            &own,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        )
        .is_eq();
    let mut result = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    if source.content_mapper().is_empty() && !options.emit_declaration_only.is_true() && !json_same
    {
        result[0] = own;
        if !json && options.source_map.is_true() && !options.inline_source_map.is_true() {
            result[1] = [&result[0], b".map".as_slice()].concat();
        }
    }
    if options.emit_declarations() && !json {
        let directory = if options.declaration_dir.is_empty() {
            &options.out_dir
        } else {
            &options.declaration_dir
        };
        let output = if directory.is_empty() {
            name.to_vec()
        } else {
            source_in_new_directory(name, directory.as_bytes(), program, common)
        };
        result[2] = declaration_extension(&output);
        if options.declaration_maps_enabled() {
            result[3] = [&result[2], b".map".as_slice()].concat();
        }
    }
    Ok(result)
}
/// Unconditional workers used by import-map inversion, regardless of emit flags.
// port: tsc/internal/outputpaths/outputpaths.go:GetOutputJSFileNameWorker
pub(crate) fn module_specifier_output_name(
    file: &[u8],
    program: &Program,
    common: &[u8],
    declaration: bool,
) -> Vec<u8> {
    if declaration {
        return tsr_tsoptions::output_paths::output_declaration_file_name(
            file,
            program.options(),
            common,
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
            &[],
        );
    }
    let options = program.options();
    let directory = &options.out_dir;
    let output = if directory.is_empty() {
        file.to_vec()
    } else {
        path::resolve(
            directory.as_bytes(),
            &[&path::relative_from_directory(
                common,
                file,
                program.current_directory(),
                program.host().use_case_sensitive_file_names(),
            )],
        )
    };
    [
        path::remove_file_extension(&output),
        output_extension(file, options.jsx),
    ]
    .concat()
}
