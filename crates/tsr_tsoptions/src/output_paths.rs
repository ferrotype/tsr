//! Pure output-name helpers shared by parsed configs and the compiler. No emit.
//!
//! The port of `tsc/internal/outputpaths`. Go's `*ast.SourceFile` is
//! `tsr_ast::SourceFileState`; the functions read only its file name, its
//! script kind (`IsJsonSourceFile`) and its content mapper.
use tsr_ast::SourceFileState;
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tspath as path;

/// Go's `OutputPathsHost`.
///
/// `common_source_directory` takes `&mut self` because both native hosts
/// compute it once on first use (`sync.Once`) and may record membership
/// diagnostics while doing so; the functions below ask for it exactly where
/// the Go functions call it, so a host that is never asked never computes it.
/// It returns a `JsString` so the host borrow ends before the other accessors
/// are read; a cached directory is returned by reference-count clone.
pub trait OutputPathsHost {
    fn common_source_directory(&mut self) -> JsString;
    fn content_mapper_extensions(&self) -> Vec<JsString>;
    fn get_current_directory(&self) -> &[u8];
    fn use_case_sensitive_file_names(&self) -> bool;
}

/// Go's `OutputPaths`: an empty path means the output is not written.
#[allow(
    clippy::struct_field_names,
    reason = "the fields keep the names of Go's OutputPaths accessors"
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputPaths {
    js_file_path: Vec<u8>,
    source_map_file_path: Vec<u8>,
    declaration_file_path: Vec<u8>,
    declaration_map_path: Vec<u8>,
}

impl OutputPaths {
    // port: tsc/internal/outputpaths/outputpaths.go:OutputPaths.DeclarationFilePath
    pub fn declaration_file_path(&self) -> &[u8] {
        &self.declaration_file_path
    }
    // port: tsc/internal/outputpaths/outputpaths.go:OutputPaths.JsFilePath
    pub fn js_file_path(&self) -> &[u8] {
        &self.js_file_path
    }
    // port: tsc/internal/outputpaths/outputpaths.go:OutputPaths.SourceMapFilePath
    pub fn source_map_file_path(&self) -> &[u8] {
        &self.source_map_file_path
    }
    // port: tsc/internal/outputpaths/outputpaths.go:OutputPaths.DeclarationMapPath
    pub fn declaration_map_path(&self) -> &[u8] {
        &self.declaration_map_path
    }
}

/// Go's `ForceEmitPaths`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForceEmitPaths {
    pub dts: bool,
    pub js: bool,
    pub declaration_map: bool,
}

// port: tsc/internal/outputpaths/outputpaths.go:GetOutputPathsFor
pub fn get_output_paths_for<H: OutputPathsHost + ?Sized>(
    source_file: &SourceFileState,
    options: &CompilerOptions,
    host: &mut H,
    force: ForceEmitPaths,
) -> OutputPaths {
    let own_output_file_path = get_own_emit_output_file_path(
        source_file.file_name(),
        options,
        host,
        get_output_extension(source_file.file_name(), options.jsx),
    );
    let is_json_file = tsr_ast::utilities::is_json_source_file(source_file);
    // If json file emits to the same location skip writing it, if emitDeclarationOnly skip writing it
    let is_json_emitted_to_same_location = is_json_file
        && path::compare_paths(
            source_file.file_name(),
            &own_output_file_path,
            host.get_current_directory(),
            host.use_case_sensitive_file_names(),
        )
        .is_eq();
    let mut paths = OutputPaths::default();
    if source_file.content_mapper().is_empty()
        && (force.js || !options.emit_declaration_only.is_true())
        && !is_json_emitted_to_same_location
    {
        paths.js_file_path = own_output_file_path;
        if !tsr_ast::utilities::is_json_source_file(source_file) {
            paths.source_map_file_path = get_source_map_file_path(&paths.js_file_path, options);
        }
    }
    if force.dts || options.emit_declarations() && !is_json_file {
        paths.declaration_file_path =
            get_declaration_emit_output_file_path(source_file.file_name(), options, host);
        if options.declaration_maps_enabled()
            || force.declaration_map && options.declaration_map.is_true()
        {
            paths.declaration_map_path = [paths.declaration_file_path.as_slice(), b".map"].concat();
        }
    }
    paths
}

// port: tsc/internal/outputpaths/outputpaths.go:ForEachEmittedFile
pub fn for_each_emitted_file<H: OutputPathsHost + ?Sized>(
    host: &mut H,
    options: &CompilerOptions,
    mut action: impl FnMut(&OutputPaths, &SourceFileState) -> bool,
    source_files: &[&SourceFileState],
    force_dts_emit: bool,
) -> bool {
    for &source_file in source_files {
        let paths = get_output_paths_for(
            source_file,
            options,
            host,
            ForceEmitPaths {
                dts: force_dts_emit,
                ..ForceEmitPaths::default()
            },
        );
        if action(&paths, source_file) {
            return true;
        }
    }
    false
}

// port: tsc/internal/outputpaths/outputpaths.go:GetOutputJSFileName
pub fn get_output_js_file_name<H: OutputPathsHost + ?Sized>(
    input_file_name: &[u8],
    options: &CompilerOptions,
    host: &mut H,
) -> Vec<u8> {
    if options.emit_declaration_only.is_true() || is_content_mapped_file_name(input_file_name, host)
    {
        return Vec::new();
    }
    let output_file_name = get_output_js_file_name_worker(input_file_name, options, host);
    if !path::file_extension_is(&output_file_name, path::EXTENSION_JSON)
        || path::compare_paths(
            input_file_name,
            &output_file_name,
            host.get_current_directory(),
            host.use_case_sensitive_file_names(),
        )
        .is_ne()
    {
        return output_file_name;
    }
    Vec::new()
}

// port: tsc/internal/outputpaths/outputpaths.go:isContentMappedFileName
fn is_content_mapped_file_name<H: OutputPathsHost + ?Sized>(file_name: &[u8], host: &H) -> bool {
    !path::longest_extension_from_path(
        file_name,
        &host.content_mapper_extensions(),
        !host.use_case_sensitive_file_names(),
    )
    .is_empty()
}

// port: tsc/internal/outputpaths/outputpaths.go:GetOutputJSFileNameWorker
pub fn get_output_js_file_name_worker<H: OutputPathsHost + ?Sized>(
    input_file_name: &[u8],
    options: &CompilerOptions,
    host: &mut H,
) -> Vec<u8> {
    path::change_extension(
        &get_output_path_without_changing_extension(
            input_file_name,
            options.out_dir.as_bytes(),
            host,
        ),
        get_output_extension(input_file_name, options.jsx),
    )
}

/// port: tsc/internal/outputpaths/outputpaths.go:GetOutputDeclarationFileNameWorker
pub fn get_output_declaration_file_name_worker<H: OutputPathsHost + ?Sized>(
    input_file_name: &[u8],
    options: &CompilerOptions,
    host: &mut H,
) -> Vec<u8> {
    let dir = if options.declaration_dir.is_empty() {
        &options.out_dir
    } else {
        &options.declaration_dir
    };
    let output = get_output_path_without_changing_extension(input_file_name, dir.as_bytes(), host);
    declaration_extension(&output, &host.content_mapper_extensions())
}

// port: tsc/internal/outputpaths/outputpaths.go:GetOutputExtension
pub fn get_output_extension(file_name: &[u8], jsx: tsr_core::JsxEmit) -> &'static [u8] {
    if path::file_extension_is(file_name, path::EXTENSION_JSON) {
        path::EXTENSION_JSON
    } else if jsx == tsr_core::JsxEmit::PRESERVE
        && path::file_extension_is_one_of(file_name, &[path::EXTENSION_JSX, path::EXTENSION_TSX])
    {
        path::EXTENSION_JSX
    } else if path::file_extension_is_one_of(file_name, &[path::EXTENSION_MTS, path::EXTENSION_MJS])
    {
        path::EXTENSION_MJS
    } else if path::file_extension_is_one_of(file_name, &[path::EXTENSION_CTS, path::EXTENSION_CJS])
    {
        path::EXTENSION_CJS
    } else {
        path::EXTENSION_JS
    }
}

// port: tsc/internal/outputpaths/outputpaths.go:GetDeclarationEmitOutputFilePath
pub fn get_declaration_emit_output_file_path<H: OutputPathsHost + ?Sized>(
    file: &[u8],
    options: &CompilerOptions,
    host: &mut H,
) -> Vec<u8> {
    let output_dir = if !options.declaration_dir.is_empty() {
        Some(&options.declaration_dir)
    } else if !options.out_dir.is_empty() {
        Some(&options.out_dir)
    } else {
        None
    };
    let path = if let Some(output_dir) = output_dir {
        let common_source_directory = host.common_source_directory();
        get_source_file_path_in_new_dir_worker(
            file,
            output_dir.as_bytes(),
            host.get_current_directory(),
            common_source_directory.as_bytes(),
            host.use_case_sensitive_file_names(),
        )
    } else {
        file.to_vec()
    };
    declaration_extension(&path, &host.content_mapper_extensions())
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

// port: tsc/internal/outputpaths/outputpaths.go:GetSourceFilePathInNewDir
pub fn get_source_file_path_in_new_dir(
    file_name: &[u8],
    new_dir_path: &[u8],
    current_directory: &[u8],
    common_source_directory: &[u8],
    use_case_sensitive_file_names: bool,
) -> Vec<u8> {
    get_source_file_path_in_new_dir_worker(
        file_name,
        new_dir_path,
        current_directory,
        common_source_directory,
        use_case_sensitive_file_names,
    )
}

// port: tsc/internal/outputpaths/outputpaths.go:getOutputPathWithoutChangingExtension
fn get_output_path_without_changing_extension<H: OutputPathsHost + ?Sized>(
    input_file_name: &[u8],
    output_directory: &[u8],
    host: &mut H,
) -> Vec<u8> {
    if !output_directory.is_empty() {
        let common_source_directory = host.common_source_directory();
        return path::resolve(
            output_directory,
            &[&path::relative_from_directory(
                common_source_directory.as_bytes(),
                input_file_name,
                host.get_current_directory(),
                host.use_case_sensitive_file_names(),
            )],
        );
    }
    input_file_name.to_vec()
}

/// A case-insensitive prefix is trimmed by its rune count, as
/// `TrimFilePathPrefix` does, not by its byte length.
// port: tsc/internal/outputpaths/outputpaths.go:GetSourceFilePathInNewDirWorker
pub fn get_source_file_path_in_new_dir_worker(
    file_name: &[u8],
    new_dir_path: &[u8],
    current_directory: &[u8],
    common_source_directory: &[u8],
    use_case_sensitive_file_names: bool,
) -> Vec<u8> {
    let source_file_path = path::absolute(file_name, current_directory);
    let source_file_path = path::trim_file_path_prefix(
        &source_file_path,
        common_source_directory,
        use_case_sensitive_file_names,
    )
    .unwrap_or(&source_file_path);
    path::combine(new_dir_path, &[source_file_path])
}

// port: tsc/internal/outputpaths/outputpaths.go:getOwnEmitOutputFilePath
fn get_own_emit_output_file_path<H: OutputPathsHost + ?Sized>(
    file_name: &[u8],
    options: &CompilerOptions,
    host: &mut H,
    extension: &[u8],
) -> Vec<u8> {
    let emit_output_file_path = if options.out_dir.is_empty() {
        file_name.to_vec()
    } else {
        // Go reads the current directory before CommonSourceDirectory; that
        // read has no effect, so the order of the two host calls is not observable.
        let common_source_directory = host.common_source_directory();
        get_source_file_path_in_new_dir(
            file_name,
            options.out_dir.as_bytes(),
            host.get_current_directory(),
            common_source_directory.as_bytes(),
            host.use_case_sensitive_file_names(),
        )
    };
    [
        path::remove_file_extension(&emit_output_file_path),
        extension,
    ]
    .concat()
}

// port: tsc/internal/outputpaths/outputpaths.go:GetSourceMapFilePath
pub fn get_source_map_file_path(js_file_path: &[u8], options: &CompilerOptions) -> Vec<u8> {
    if options.source_map.is_true() && !options.inline_source_map.is_true() {
        return [js_file_path, b".map"].concat();
    }
    Vec::new()
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

/// Go's `checkSourceFilesBelongToPath func([]string, string) bool` parameter.
pub type CheckSourceFilesBelongToPath<'a> = &'a mut dyn FnMut(&[JsString], &[u8]) -> bool;

/// `files` is called at most once, and only on a branch that reads it; the
/// result of `check_source_files_belong_to_path` is ignored, as in Go.
// port: tsc/internal/outputpaths/commonsourcedirectory.go:GetCommonSourceDirectory
pub fn get_common_source_directory(
    options: &CompilerOptions,
    files: impl FnOnce() -> Vec<JsString>,
    current_directory: &[u8],
    use_case_sensitive_file_names: bool,
    check_source_files_belong_to_path: Option<CheckSourceFilesBelongToPath<'_>>,
) -> Vec<u8> {
    let mut common_source_directory = if !options.root_dir.is_empty() {
        // If a rootDir is specified use it as the commonSourceDirectory
        if let Some(check) = check_source_files_belong_to_path {
            check(&files(), options.root_dir.as_bytes());
        }
        options.root_dir.as_bytes().to_vec()
    } else if !options.config_file_path.is_empty() {
        // If the rootDir is not specified, then the common source directory is the directory of the config file.
        let common_source_directory = path::directory(options.config_file_path.as_bytes());
        if let Some(check) = check_source_files_belong_to_path {
            check(&files(), &common_source_directory);
        }
        common_source_directory
    } else {
        computed_common(&files(), current_directory, use_case_sensitive_file_names)
    };
    if !common_source_directory.is_empty() {
        // Make sure directory path ends with directory separator so this string can directly
        // used to replace with "" to get the relative path of the source file and the relative path doesn't
        // start with / making it rooted path
        // (EnsureTrailingDirectorySeparator, appending in place.)
        if !path::has_trailing_directory_separator(&common_source_directory) {
            common_source_directory.push(b'/');
        }
    }
    common_source_directory
}
