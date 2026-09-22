//! Pure output-name helpers shared by parsed configs and the compiler. No emit.
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tspath as path;
/// port: tsc/internal/outputpaths/commonsourcedirectory.go:computeCommonSourceDirectoryOfFilenames
pub fn computed_common(files: &[JsString], cwd: &[u8], case_sensitive: bool) -> Vec<u8> {
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
/// port: tsc/internal/outputpaths/outputpaths.go:ChangeToDeclarationExtension
pub fn declaration_extension(file: &[u8], mapper_extensions: &[JsString]) -> Vec<u8> {
    let extension = path::longest_extension_from_path(file, mapper_extensions, false);
    if !extension.is_empty() {
        return [
            &file[..file.len() - extension.len()],
            b".d",
            extension,
            b".ts",
        ]
        .concat();
    }
    let mut base = path::remove_file_extension(file);
    if base == file {
        let filename = path::base_name(file);
        if let Some(index) = filename.iter().rposition(|&byte| byte == b'.') {
            base = &file[..file.len() - filename.len() + index];
        }
    }
    let extension = path::declaration_emit_extension_for_path(file);
    [base, &extension].concat()
}
/// port: tsc/internal/outputpaths/outputpaths.go:GetBuildInfoFileName
pub fn build_info_file(options: &CompilerOptions, cwd: &[u8], case_sensitive: bool) -> Vec<u8> {
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

/// port: tsc/internal/outputpaths/outputpaths.go:GetOutputDeclarationFileNameWorker
pub fn output_declaration_file_name(
    file: &[u8],
    options: &CompilerOptions,
    common: &[u8],
    cwd: &[u8],
    case_sensitive: bool,
    mapper_extensions: &[JsString],
) -> Vec<u8> {
    let directory = if options.declaration_dir.is_empty() {
        &options.out_dir
    } else {
        &options.declaration_dir
    };
    let output = if directory.is_empty() {
        file.to_vec()
    } else {
        path::resolve(
            directory.as_bytes(),
            &[&path::relative_from_directory(
                common,
                file,
                cwd,
                case_sensitive,
            )],
        )
    };
    declaration_extension(&output, mapper_extensions)
}
