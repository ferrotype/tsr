//! Result-owned, first-use caches. Options replacement preserves initialized
//! caches as at the pin; changing config specs explicitly invalidates globs.
use crate::{config_mappers::ContentMapper, ParsedCommandLine, ProjectReference, TypeAcquisition};
use std::{collections::BTreeMap, sync::OnceLock};
use tsr_core::{CompilerOptions, WatchOptions};
use tsr_jsstring::JsString;
use tsr_tspath as path;

#[derive(Clone, Debug, Default)]
pub struct ParsedOptions {
    pub compiler_options: CompilerOptions,
    pub watch_options: Option<WatchOptions>,
    pub type_acquisition: Option<TypeAcquisition>,
    pub file_names: Vec<JsString>,
    pub project_references: Option<Vec<ProjectReference>>,
    pub content_mappers: Option<Vec<ContentMapper>>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedCaches {
    common_directory: Option<JsString>,
    output_maps: Option<OutputMaps>,
    names: OnceLock<BTreeMap<JsString, JsString>>,
    references: OnceLock<Vec<JsString>>,
    pub(crate) globs: OnceLock<Option<Vec<tsr_glob::Glob>>>,
    locale: OnceLock<tsr_locale::Locale>,
}
/// port: tsc/internal/core/projectreference.go:ResolveProjectReferencePath
pub fn resolve_project_reference_path(reference: &ProjectReference) -> JsString {
    resolve_config_file_name_of_project_reference(reference.path.as_bytes())
}
/// port: tsc/internal/core/projectreference.go:ResolveConfigFileNameOfProjectReference
pub fn resolve_config_file_name_of_project_reference(name: &[u8]) -> JsString {
    JsString::from_bytes(if path::file_extension_is(name, b".json") {
        name.to_vec()
    } else {
        path::combine(name, &[b"tsconfig.json"])
    })
}
impl ParsedCommandLine {
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.FileNamesByPath
    pub fn file_names_by_path(&self) -> &BTreeMap<JsString, JsString> {
        self.caches.names.get_or_init(|| {
            self.root_file_names
                .iter()
                .map(|name| {
                    (
                        path::to_path(
                            name.as_bytes(),
                            self.current_directory(),
                            self.use_case_sensitive_file_names(),
                        ),
                        name.clone(),
                    )
                })
                .collect()
        })
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ResolvedProjectReferencePaths
    pub fn resolved_project_reference_paths(&self) -> &[JsString] {
        self.caches.references.get_or_init(|| {
            self.project_references
                .iter()
                .flatten()
                .map(resolve_project_reference_path)
                .collect()
        })
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ExtendedSourceFiles
    pub fn extended_source_files(&self) -> &[JsString] {
        self.config_file
            .as_ref()
            .map_or(&[], |file| &file.extended_source_files)
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ContentMapperExtensions
    pub fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.content_mappers
            .iter()
            .flatten()
            .flat_map(|mapper| mapper.extensions.iter().cloned())
            .collect()
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetContentMapperForFileName
    pub fn content_mapper_for_file_name(&self, file_name: &[u8]) -> Option<&ContentMapper> {
        let ignore_case = !self.use_case_sensitive_file_names();
        let extension = path::longest_extension_from_path(
            file_name,
            &self.content_mapper_extensions(),
            ignore_case,
        );
        self.content_mappers.iter().flatten().find(|mapper| {
            mapper.extensions.iter().any(|candidate| {
                candidate.as_bytes() == extension
                    || ignore_case && tsr_jsstring::equal_fold(candidate.as_bytes(), extension)
            })
        })
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.fileGlobPatterns
    pub fn file_glob_patterns(&self) -> (Vec<u8>, Vec<u8>) {
        let mut pattern = b"*.{js,jsx,mjs,cjs,ts,tsx,mts,cts,json".to_vec();
        for extension in self.content_mapper_extensions() {
            pattern.push(b',');
            pattern.extend_from_slice(
                extension
                    .as_bytes()
                    .strip_prefix(b".")
                    .unwrap_or(extension.as_bytes()),
            );
        }
        pattern.push(b'}');
        (pattern.clone(), [b"**/".as_slice(), &pattern].concat())
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.WildcardDirectoryGlobs
    pub fn wildcard_directory_globs(&self) -> Option<&[tsr_glob::Glob]> {
        self.caches
            .globs
            .get_or_init(|| {
                self.wildcard_directories().map(|directories| {
                    let (plain, recursive) = self.file_glob_patterns();
                    directories
                        .iter()
                        .filter_map(|(directory, recurse)| {
                            let directory = path::normalize(directory.as_bytes());
                            let pattern = [
                                directory.as_ref(),
                                b"/",
                                if *recurse { &recursive } else { &plain },
                            ]
                            .concat();
                            tsr_glob::Glob::parse(&pattern).ok()
                        })
                        .collect()
                })
            })
            .as_deref()
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SetTypeAcquisition
    pub fn set_type_acquisition(&mut self, value: Option<TypeAcquisition>) {
        self.type_acquisition = value;
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SetCompilerOptions
    pub fn set_compiler_options(&mut self, value: CompilerOptions) {
        self.options = value;
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SetParsedOptions
    pub fn set_parsed_options(&mut self, value: ParsedOptions) {
        self.options = value.compiler_options;
        self.watch_options = value.watch_options;
        self.type_acquisition = value.type_acquisition;
        self.root_file_names = value.file_names;
        self.project_references = value.project_references;
        self.content_mappers = value.content_mappers;
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.Locale
    pub fn locale(&self) -> &tsr_locale::Locale {
        self.caches.locale.get_or_init(|| {
            tsr_locale::Locale::parse(&String::from_utf8_lossy(self.options.locale.as_bytes())).0
        })
    }
}

/// Both indexes share these owned names; the borrowed view supplies the parse
/// result identity without storing a self-reference or introducing an Arc cycle.
#[derive(Clone, Debug)]
pub struct SourceOutputNames {
    pub source: JsString,
    pub output_dts: JsString,
}
pub struct SourceOutputAndProjectReference<'a> {
    pub names: &'a SourceOutputNames,
    pub resolved: &'a ParsedCommandLine,
}
#[derive(Clone, Debug, Default)]
struct OutputMaps {
    sources: BTreeMap<JsString, std::sync::Arc<SourceOutputNames>>,
    outputs: BTreeMap<JsString, std::sync::Arc<SourceOutputNames>>,
}
impl ParsedCommandLine {
    /// The membership check runs once the directory function returns, with the
    /// file list and root it was given: `GetCommonSourceDirectory` ignores the
    /// check's result and only appends a separator after it, so the deferral is
    /// not observable.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.CommonSourceDirectory
    pub fn common_source_directory(&mut self) -> &[u8] {
        if self.caches.common_directory.is_none() {
            let files = || {
                self.root_file_names
                    .iter()
                    .filter(|file| {
                        !(self.options.no_emit_for_js_files.is_true()
                            && path::has_js_file_extension(file.as_bytes())
                            || path::is_declaration_file_name(file.as_bytes()))
                    })
                    .cloned()
                    .collect()
            };
            let mut check: Option<(Vec<JsString>, Vec<u8>)> = None;
            let directory = crate::output_paths::get_common_source_directory(
                &self.options,
                files,
                self.current_directory(),
                self.use_case_sensitive_file_names(),
                Some(&mut |files: &[JsString], root: &[u8]| {
                    check = Some((files.to_vec(), root.to_vec()));
                    true
                }),
            );
            if let Some((files, root)) = check {
                self.check_source_files_belong_to_path(&files, &root);
            }
            self.caches.common_directory = Some(JsString::from_bytes(directory));
        }
        self.caches
            .common_directory
            .as_ref()
            .expect("initialized common source directory")
            .as_bytes()
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.checkSourceFilesBelongToPath
    fn check_source_files_belong_to_path(&mut self, files: &[JsString], root: &[u8]) -> bool {
        let mut all = true;
        for file in files {
            if !path::contains_path(
                root,
                file.as_bytes(),
                self.current_directory(),
                self.use_case_sensitive_file_names(),
            ) {
                self.errors.push(tsr_ast::Diagnostic::compiler(tsr_diagnostics::File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files, vec![
                    path::to_path(file.as_bytes(), self.current_directory(), self.use_case_sensitive_file_names()), JsString::from_bytes(root),
                ]));
                all = false;
            }
        }
        all
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetBuildInfoFileName
    pub fn build_info_file_name(&self) -> JsString {
        JsString::from_bytes(crate::output_paths::build_info_file(
            &self.options,
            self.current_directory(),
            self.use_case_sensitive_file_names(),
        ))
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.ParseInputOutputNames
    pub fn parse_input_output_names(&mut self) {
        if self.caches.output_maps.is_some() {
            return;
        }
        let mut maps = OutputMaps::default();
        // The parsed command line is the output-path host; CommonSourceDirectory
        // stays lazy: only an output directory makes the worker ask for it.
        let options = self.options.clone();
        for index in 0..self.root_file_names.len() {
            let source = self.root_file_names[index].clone();
            let output = if path::is_declaration_file_name(source.as_bytes())
                || path::file_extension_is(source.as_bytes(), path::EXTENSION_JSON)
            {
                Vec::new()
            } else {
                crate::output_paths::get_output_declaration_file_name_worker(
                    source.as_bytes(),
                    &options,
                    self,
                )
            };
            let names = std::sync::Arc::new(SourceOutputNames {
                source: source.clone(),
                output_dts: JsString::from_bytes(output),
            });
            if !names.output_dts.is_empty() {
                maps.outputs.insert(
                    path::to_path(
                        names.output_dts.as_bytes(),
                        self.current_directory(),
                        self.use_case_sensitive_file_names(),
                    ),
                    names.clone(),
                );
            }
            maps.sources.insert(
                path::to_path(
                    source.as_bytes(),
                    self.current_directory(),
                    self.use_case_sensitive_file_names(),
                ),
                names,
            );
        }
        self.caches.output_maps = Some(maps);
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.SourceToProjectReference
    pub fn source_to_project_reference(
        &self,
    ) -> impl Iterator<Item = (&JsString, SourceOutputAndProjectReference<'_>)> {
        self.caches
            .output_maps
            .iter()
            .flat_map(|maps| &maps.sources)
            .map(|(key, names)| {
                (
                    key,
                    SourceOutputAndProjectReference {
                        names,
                        resolved: self,
                    },
                )
            })
    }
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.OutputDtsToProjectReference
    pub fn output_dts_to_project_reference(
        &self,
    ) -> impl Iterator<Item = (&JsString, SourceOutputAndProjectReference<'_>)> {
        self.caches
            .output_maps
            .iter()
            .flat_map(|maps| &maps.outputs)
            .map(|(key, names)| {
                (
                    key,
                    SourceOutputAndProjectReference {
                        names,
                        resolved: self,
                    },
                )
            })
    }
}

impl ParsedCaches {
    /// WithFileNames and ReloadFileNames copy only includeGlobs from these
    /// caches. In particular, names and output maps must describe the new files.
    pub(crate) fn for_new_file_names(&self) -> Self {
        Self {
            globs: self.globs.clone(),
            ..Self::default()
        }
    }
}

/// Go's `var _ outputpaths.OutputPathsHost = (*ParsedCommandLine)(nil)`.
impl crate::output_paths::OutputPathsHost for ParsedCommandLine {
    fn common_source_directory(&mut self) -> JsString {
        ParsedCommandLine::common_source_directory(self);
        self.caches
            .common_directory
            .clone()
            .expect("initialized common source directory")
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        ParsedCommandLine::content_mapper_extensions(self)
    }
    fn get_current_directory(&self) -> &[u8] {
        self.current_directory()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        ParsedCommandLine::use_case_sensitive_file_names(self)
    }
}
