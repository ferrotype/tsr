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

use tsr_jsstring::JsString;

/// Go's `GetOutputExtension`.
fn output_extension(file_name: &[u8], jsx: tsr_core::JsxEmit) -> &'static [u8] {
    if tsr_tspath::file_extension_is(file_name, b".json") {
        b".json"
    } else if jsx == tsr_core::JsxEmit::PRESERVE
        && tsr_tspath::file_extension_is_one_of(file_name, &[b".jsx", b".tsx"])
    {
        b".jsx"
    } else if tsr_tspath::file_extension_is_one_of(file_name, &[b".mts", b".mjs"]) {
        b".mjs"
    } else if tsr_tspath::file_extension_is_one_of(file_name, &[b".cts", b".cjs"]) {
        b".cjs"
    } else {
        b".js"
    }
}

/// Go's `GetSourceMapFilePath`.
fn source_map_file_path(js_file_path: &[u8], options: &tsr_core::CompilerOptions) -> Vec<u8> {
    if options.source_map.is_true() && !options.inline_source_map.is_true() {
        return [js_file_path, b".map"].concat();
    }
    Vec::new()
}

impl ParsedCommandLine {
    /// Go's `getOutputPathWithoutChangingExtension`.
    fn output_path_without_changing_extension(
        &mut self,
        input: &[u8],
        output_directory: &[u8],
    ) -> Vec<u8> {
        if output_directory.is_empty() {
            return input.to_vec();
        }
        let common = self.common_source_directory().to_vec();
        tsr_tspath::resolve(
            output_directory,
            &[&tsr_tspath::relative_from_directory(
                &common,
                input,
                self.current_directory(),
                self.use_case_sensitive_file_names(),
            )],
        )
    }

    /// Go's `GetOutputJSFileName` with the parsed command line as its host.
    fn output_js_file_name(&mut self, input: &[u8]) -> Vec<u8> {
        let mapper_extensions = self.content_mapper_extensions();
        let content_mapped = !tsr_tspath::longest_extension_from_path(
            input,
            &mapper_extensions,
            !self.use_case_sensitive_file_names(),
        )
        .is_empty();
        if self.options.emit_declaration_only.is_true() || content_mapped {
            return Vec::new();
        }
        let out_dir = self.options.out_dir.as_bytes().to_vec();
        let output = tsr_tspath::change_extension(
            &self.output_path_without_changing_extension(input, &out_dir),
            output_extension(input, self.options.jsx),
        );
        if !tsr_tspath::file_extension_is(&output, b".json")
            || tsr_tspath::compare_paths(
                input,
                &output,
                self.current_directory(),
                self.use_case_sensitive_file_names(),
            ) != std::cmp::Ordering::Equal
        {
            return output;
        }
        Vec::new()
    }

    /// Go's iterator `GetOutputFileNames`, collected: each input's JS output
    /// and source map, then its declaration file and declaration map.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetOutputFileNames
    pub fn output_file_names(&mut self) -> Vec<JsString> {
        let mut out = Vec::new();
        for file_name in self.root_file_names.clone() {
            let file_name = file_name.as_bytes();
            if tsr_core::path::is_declaration_file_name(file_name) {
                continue;
            }
            let js_file_name = self.output_js_file_name(file_name);
            let is_json = tsr_tspath::file_extension_is(file_name, b".json");
            if !js_file_name.is_empty() {
                out.push(JsString::from_bytes(js_file_name.as_slice()));
                if !is_json {
                    let source_map = source_map_file_path(&js_file_name, &self.options);
                    if !source_map.is_empty() {
                        out.push(JsString::from_bytes(source_map.as_slice()));
                    }
                }
            }
            if is_json {
                continue;
            }
            if self.options.emit_declarations() {
                let common = self.common_source_directory().to_vec();
                let mapper_extensions = self.content_mapper_extensions();
                let dts_file_name = crate::output_paths::output_declaration_file_name(
                    file_name,
                    &self.options,
                    &common,
                    self.current_directory(),
                    self.use_case_sensitive_file_names(),
                    &mapper_extensions,
                );
                if !dts_file_name.is_empty() {
                    out.push(JsString::from_bytes(dts_file_name.as_slice()));
                    if self.content_mapper_for_file_name(file_name).is_none()
                        && self.options.declaration_maps_enabled()
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

    /// Go's `WithFileNames`: a copy with other file names (nil for none);
    /// the caches and specs are shared as Go shares their pointers.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.WithFileNames
    pub fn with_file_names(&self, file_names: Option<Vec<JsString>>) -> Self {
        let mut copy = self.clone();
        copy.root_file_names = file_names.unwrap_or_default();
        copy
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
