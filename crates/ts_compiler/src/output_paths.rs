//! The output-name calculation needed by option diagnostics. No files are emitted.
use crate::{Error, Program, ProgramFile};
use ts_core::{CompilerOptions, JsxEmit, ScriptKind};
use ts_jsstring::JsString;
use ts_tspath as path;

fn separator(mut directory: Vec<u8>) -> Vec<u8> {
    if !directory.is_empty() && !matches!(directory.last(), Some(b'/' | b'\\')) {
        directory.push(b'/');
    }
    directory
}
/// port: tsc/internal/outputpaths/commonsourcedirectory.go:computeCommonSourceDirectoryOfFilenames
pub(crate) fn computed_common(files: &[JsString], cwd: &[u8], case_sensitive: bool) -> Vec<u8> {
    let mut common: Option<Vec<Vec<u8>>> = None;
    for file in files {
        let mut components = path::normalized_components(file.as_bytes(), cwd);
        components.pop();
        if let Some(common) = &mut common {
            let length = common
                .iter()
                .zip(&components)
                .take_while(|(a, b)| {
                    path::canonical(a, case_sensitive) == path::canonical(b, case_sensitive)
                })
                .count();
            if length == 0 {
                return Vec::new();
            }
            common.truncate(length);
        } else {
            common = Some(components);
        }
    }
    let common = common.unwrap_or_default();
    if common.is_empty() {
        cwd.to_vec()
    } else {
        path::path_from_components(&common)
    }
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
) -> Result<bool, ts_arena::Error> {
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
/// Declaration names for the S07 mapper-free operation closure.
/// port: tsc/internal/outputpaths/outputpaths.go:ChangeToDeclarationExtension
fn declaration_extension(file: &[u8]) -> Vec<u8> {
    let mut base = path::remove_file_extension(file);
    if base == file {
        let filename = path::base_name(file);
        if let Some(index) = filename.iter().rposition(|&byte| byte == b'.') {
            base = &file[..file.len() - filename.len() + index];
        }
    }
    let extension = if extension_is(file, b".mts") || extension_is(file, b".mjs") {
        b".d.mts".to_vec()
    } else if extension_is(file, b".cts") || extension_is(file, b".cjs") {
        b".d.cts".to_vec()
    } else if [b".ts".as_slice(), b".tsx", b".js", b".jsx"]
        .iter()
        .any(|extension| extension_is(file, extension))
    {
        b".d.ts".to_vec()
    } else {
        let filename = path::base_name(file);
        filename.iter().rposition(|&byte| byte == b'.').map_or_else(
            || b".d.ts".to_vec(),
            |index| [b".d".as_slice(), &filename[index..], b".ts"].concat(),
        )
    };
    [base, &extension].concat()
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
/// port: tsc/internal/outputpaths/outputpaths.go:GetBuildInfoFileName
pub(crate) fn build_info_file(
    options: &CompilerOptions,
    cwd: &[u8],
    case_sensitive: bool,
) -> Vec<u8> {
    if !options.is_incremental() && !options.build.is_true() {
        return Vec::new();
    }
    if !options.ts_build_info_file.is_empty() {
        return options.ts_build_info_file.as_bytes().to_vec();
    }
    if options.config_file_path.is_empty() {
        return Vec::new();
    }
    let config = path::remove_file_extension(options.config_file_path.as_bytes());
    let base = if options.out_dir.is_empty() {
        config.to_vec()
    } else if !options.root_dir.is_empty() {
        path::resolve(
            options.out_dir.as_bytes(),
            &[&path::relative_from_directory(
                options.root_dir.as_bytes(),
                config,
                cwd,
                case_sensitive,
            )],
        )
    } else {
        path::combine(options.out_dir.as_bytes(), &[path::base_name(config)])
    };
    [base.as_slice(), b".tsbuildinfo"].concat()
}

/// Unconditional workers used by import-map inversion, regardless of emit flags.
// port: tsc/internal/outputpaths/outputpaths.go:GetOutputJSFileNameWorker
// port: tsc/internal/outputpaths/outputpaths.go:GetOutputDeclarationFileNameWorker
pub(crate) fn module_specifier_output_name(
    file: &[u8],
    program: &Program,
    common: &[u8],
    declaration: bool,
) -> Vec<u8> {
    let options = program.options();
    let directory = if declaration && !options.declaration_dir.is_empty() {
        &options.declaration_dir
    } else {
        &options.out_dir
    };
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
    if declaration {
        declaration_extension(&output)
    } else {
        [
            path::remove_file_extension(&output),
            output_extension(file, options.jsx),
        ]
        .concat()
    }
}
