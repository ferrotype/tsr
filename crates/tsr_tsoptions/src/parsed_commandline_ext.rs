//! `ParsedCommandLine` methods of `tsoptions/parsedcommandline.go` beyond
//! `parsed_accessors.rs`.
//!
//! Witnessed by the `tsoptions` group of the Phase 1 operation tables
//! (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::ParsedCommandLine;

impl ParsedCommandLine {
    /// `directory_path` is already a `tspath.Path`; each wildcard directory is
    /// reduced with `to_path` and compared as a path, recursively or exactly.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.PossiblyMatchesDirectoryName
    pub fn possibly_matches_directory_name(&self, directory_path: &tsr_tspath::Path) -> bool {
        for (wildcard_dir, recursive) in self.wildcard_directories().into_iter().flatten() {
            let wildcard_dir_path = tsr_tspath::Path::from_bytes(
                tsr_tspath::to_path(
                    wildcard_dir.as_bytes(),
                    self.current_directory(),
                    self.use_case_sensitive_file_names(),
                )
                .as_bytes()
                .to_vec(),
            );
            if *recursive {
                if wildcard_dir_path.contains_path(directory_path) {
                    return true;
                }
            } else if wildcard_dir_path == *directory_path {
                return true;
            }
        }
        false
    }
}

use crate::output_paths::{
    get_output_declaration_file_name_worker, get_output_js_file_name, get_source_map_file_path,
};
use tsr_jsstring::JsString;

impl ParsedCommandLine {
    /// Go's iterator `GetOutputFileNames`, collected: each input's JS output
    /// and source map, then its declaration file and declaration map.
    ///
    /// The parsed command line is its own `OutputPathsHost`, so the options
    /// the output-path functions read are cloned once up front; the common
    /// source directory stays lazy, computed only when an output directory
    /// asks for it, as in Go.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetOutputFileNames
    pub fn output_file_names(&mut self) -> Vec<JsString> {
        let options = self.options.clone();
        let mut out = Vec::new();
        for file_name in self.root_file_names.clone() {
            let file_name = file_name.as_bytes();
            if tsr_core::path::is_declaration_file_name(file_name) {
                continue;
            }
            let js_file_name = get_output_js_file_name(file_name, &options, self);
            let is_json = tsr_tspath::file_extension_is(file_name, tsr_tspath::EXTENSION_JSON);
            if !js_file_name.is_empty() {
                out.push(JsString::from_bytes(js_file_name.as_slice()));
                if !is_json {
                    let source_map = get_source_map_file_path(&js_file_name, &options);
                    if !source_map.is_empty() {
                        out.push(JsString::from_bytes(source_map.as_slice()));
                    }
                }
            }
            if is_json {
                continue;
            }
            if options.emit_declarations() {
                let dts_file_name =
                    get_output_declaration_file_name_worker(file_name, &options, self);
                if !dts_file_name.is_empty() {
                    out.push(JsString::from_bytes(dts_file_name.as_slice()));
                    if self.content_mapper_for_file_name(file_name).is_none()
                        && options.declaration_maps_enabled()
                    {
                        out.push(JsString::from_bytes(
                            [dts_file_name.as_slice(), b".map"].concat().as_slice(),
                        ));
                    }
                }
            }
        }
        out
    }

    /// The first `literal_file_names_len` file names of a config-backed
    /// command line, else nil.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.LiteralFileNames
    pub fn literal_file_names(&self) -> Option<&[JsString]> {
        if self.config_file.is_some() {
            return Some(&self.root_file_names[..self.literal_file_names_len]);
        }
        None
    }

    /// Go's `WithFileNames`: a copy with other file names (nil for none).
    /// Keep wildcard directories and include globs; reset the other caches.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.WithFileNames
    #[must_use]
    pub fn with_file_names(&self, file_names: Option<Vec<JsString>>) -> Self {
        self.copy_with_file_names(file_names.unwrap_or_default(), self.literal_file_names_len)
    }

    // The two pinned constructors initialize the same fields, leaving the
    // file-dependent caches (and locale/reference caches) uninitialized.
    // Construct directly so warm output maps are not cloned just to drop them.
    fn copy_with_file_names(
        &self,
        file_names: Vec<JsString>,
        literal_file_names_len: usize,
    ) -> Self {
        Self {
            options: self.options.clone(),
            watch_options: self.watch_options.clone(),
            root_file_names: file_names,
            config_file: self.config_file.clone(),
            errors: self.errors.clone(),
            raw: self.raw.clone(),
            compile_on_save: self.compile_on_save,
            config_specs: self.config_specs.clone(),
            config_base_path: self.config_base_path.clone(),
            config_case_sensitive: self.config_case_sensitive,
            wildcard_directories_cache: self.wildcard_directories_cache.clone(),
            caches: self.caches.for_new_file_names(),
            config_dependencies: self.config_dependencies.clone(),
            type_acquisition: self.type_acquisition.clone(),
            project_references: self.project_references.clone(),
            literal_file_names_len,
            content_mappers: self.content_mappers.clone(),
        }
    }

    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.PossiblyMatchesFileName
    pub fn possibly_matches_file_name(&self, file_name: &[u8]) -> bool {
        let path = tsr_tspath::to_path(
            file_name,
            self.current_directory(),
            self.use_case_sensitive_file_names(),
        );
        if self.file_names_by_path().contains_key(path.as_bytes()) {
            return true;
        }
        let includes = self
            .config_specs
            .as_ref()
            .expect("runtime error: invalid memory address or nil pointer dereference")
            .validated_includes
            .clone();
        for include in &includes {
            let include = include.as_bytes();
            if !include.iter().any(|&byte| byte == b'*' || byte == b'?')
                && !crate::glob::is_implicit_glob(include)
            {
                let include_path = tsr_tspath::to_path(
                    include,
                    self.current_directory(),
                    self.use_case_sensitive_file_names(),
                );
                if include_path.as_bytes() == path.as_bytes() {
                    return true;
                }
            }
        }
        if self.content_mapper_for_file_name(file_name).is_some() {
            let directory = tsr_tspath::Path::from_bytes(path.as_bytes().to_vec()).directory_path();
            if self.possibly_matches_directory_name(&directory) {
                return true;
            }
        }
        if let Some(globs) = self.wildcard_directory_globs() {
            for glob in globs {
                if glob.matches(file_name) {
                    return true;
                }
            }
        }
        false
    }
}

impl ParsedCommandLine {
    /// Go's `ReloadFileNamesOfParsedCommandLine`: a copy whose file names are
    /// read again from the config's specs on `fs`, sharing the rest.
    pub fn reload_file_names_of_parsed_command_line(
        &self,
        fs: &dyn tsr_vfs::FileSystem,
    ) -> Result<Self, tsr_vfs::Error> {
        let (file_names, literal_file_names_len) = self.reloaded_file_names(fs)?;
        Ok(self.copy_with_file_names(file_names, literal_file_names_len))
    }

    /// The file names `ReloadFileNamesOfParsedCommandLine` reads: its work,
    /// apart from the copy. Go dereferences the config's specs, so a command
    /// line without a config panics.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ReloadFileNamesOfParsedCommandLine
    fn reloaded_file_names(
        &self,
        fs: &dyn tsr_vfs::FileSystem,
    ) -> Result<(Vec<JsString>, usize), tsr_vfs::Error> {
        let specs = self
            .config_specs
            .as_ref()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        crate::file_names_from_specs(
            specs,
            self.current_directory(),
            &self.options,
            fs,
            &self.content_mapper_extensions(),
        )
    }
}
