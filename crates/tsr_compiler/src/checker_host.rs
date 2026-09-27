//! Retained program data for the checker; no loading or resolution is repeated.
//! Project-reference lookups read the program's published reference mapper.

use crate::{metadata, output_paths, Program, ProgramFile};
use std::sync::{Arc, OnceLock};
use tsr_ast::{CompletedFile, NodeId, SourceFileMetaData};
use tsr_checker::{CheckerHost, Error, ProjectReferenceSource};
use tsr_core::{CompilerOptions, ModuleKind, ResolutionMode};
use tsr_module::ResolvedModule;
use tsr_tsoptions::ParsedCommandLine;
use tsr_tspath as path;

pub struct ProgramCheckerHost {
    program: Arc<Program>,
    pub(crate) known_symlinks: OnceLock<Result<tsr_module::symlinks::KnownSymlinks, Error>>,
    common_source_directory: OnceLock<Result<Vec<u8>, tsr_arena::Error>>,
}

impl ProgramCheckerHost {
    pub fn new(program: Arc<Program>) -> Self {
        Self {
            program,
            known_symlinks: OnceLock::new(),
            common_source_directory: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    fn file(&self, file_name: &[u8]) -> Option<&ProgramFile> {
        self.program.source_file(file_name)
    }

    fn required_file(&self, file_name: &[u8]) -> Result<&ProgramFile, Error> {
        self.file(file_name)
            .ok_or_else(|| tsr_arena::Error::WrongOwner.into())
    }

    /// The options a retained file was loaded with (getCompilerOptionsForFile).
    fn file_options(&self, file: &ProgramFile) -> Result<&CompilerOptions, Error> {
        let source = file.bound().view().source_file()?;
        Ok(self.program.options_for_file(
            source.parse_options().path.as_bytes(),
            source.parse_options().file_name.as_bytes(),
        ))
    }

    fn file_metadata(&self, file: &ProgramFile) -> Result<&SourceFileMetaData, Error> {
        let source = file.bound().view().source_file()?;
        self.program
            .metadata(source.parse_options().path.as_bytes())
            .ok_or_else(|| tsr_arena::Error::InvalidGraph.into())
    }

    fn emitted_file_names(&self) -> Result<Vec<tsr_jsstring::JsString>, tsr_arena::Error> {
        let mut names = Vec::new();
        for file in self.program.files() {
            if output_paths::may_emit_with_force_dts(file, &self.program, false)? {
                let source = file.bound().view().source_file()?;
                names.push(source.parse_options().file_name.clone());
            }
        }
        Ok(names)
    }
}

impl CheckerHost for ProgramCheckerHost {
    fn options(&self) -> &CompilerOptions {
        self.program.options()
    }

    fn source_file_count(&self) -> usize {
        self.program.files().len()
    }

    fn source_file(&self, index: usize) -> &CompletedFile {
        self.program.files()[index].bound()
    }

    // port: tsc/internal/compiler/program.go:Program.FileExists
    fn file_exists(&self, file_name: &[u8]) -> Result<bool, tsr_vfs::Error> {
        self.program.host().file_exists(file_name)
    }

    fn get_source_file(&self, file_name: &[u8]) -> Option<&CompletedFile> {
        self.file(file_name).map(ProgramFile::bound)
    }

    // port: tsc/internal/compiler/program.go:Program.GetSourceFileForResolvedModule
    fn get_source_file_for_resolved_module(&self, file_name: &[u8]) -> Option<&CompletedFile> {
        // Package redirects are already entries in Program's by-path index. A
        // referenced project's source is in the program as its output.
        self.get_source_file(file_name).or_else(|| {
            let output = self.program.parse_file_redirect(file_name)?;
            self.get_source_file(output.as_bytes())
        })
    }

    // port: tsc/internal/compiler/program.go:Program.GetEmitModuleFormatOfFile
    fn get_emit_module_format_of_file(&self, file_name: &[u8]) -> Result<ModuleKind, Error> {
        let file = self.required_file(file_name)?;
        let source = file.bound().view().source_file()?;
        Ok(metadata::emit_format(
            source.parse_options().file_name.as_bytes(),
            self.file_options(file)?,
            self.file_metadata(file)?,
        ))
    }

    // port: tsc/internal/compiler/program.go:Program.GetEmitSyntaxForUsageLocation
    fn get_emit_syntax_for_usage_location(
        &self,
        file_name: &[u8],
        usage_location: NodeId,
    ) -> Result<ResolutionMode, Error> {
        let file = self.required_file(file_name)?;
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        Ok(metadata::emit_syntax(
            view,
            source.parse_options().file_name.as_bytes(),
            self.file_metadata(file)?,
            usage_location,
            self.file_options(file)?,
        )?)
    }

    fn get_implied_node_format_for_emit(&self, file_name: &[u8]) -> Result<ModuleKind, Error> {
        let file = self.required_file(file_name)?;
        let source = file.bound().view().source_file()?;
        let parse = source.parse_options();
        Ok(self
            .program
            .implied_node_format_for_emit(parse.path.as_bytes(), parse.file_name.as_bytes()))
    }

    // port: tsc/internal/compiler/program.go:Program.GetModeForUsageLocation
    fn get_mode_for_usage_location(
        &self,
        file_name: &[u8],
        usage: NodeId,
    ) -> Result<ResolutionMode, Error> {
        let file = self.required_file(file_name)?;
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        metadata::usage_mode(
            view,
            source.parse_options().file_name.as_bytes(),
            self.file_metadata(file)?,
            usage,
            self.file_options(file)?,
        )
        .map_err(|error| match error {
            crate::Error::Ast(error) => error.into(),
            crate::Error::Unsupported(operation) => Error::Unsupported(operation),
            // Mode selection is a pure metadata/AST operation; any future
            // dependency must extend this adapter instead of hiding failure.
            _ => Error::Unsupported("GetModeForUsageLocation: compiler metadata failure"),
        })
    }

    // port: tsc/internal/compiler/fileloader.go:fileLoader.createSyntheticImport
    // port: tsc/internal/compiler/fileloader.go:getModeForUsageLocation
    fn get_import_helpers_resolution_mode(
        &self,
        file_name: &[u8],
    ) -> Result<ResolutionMode, Error> {
        let file = self.required_file(file_name)?;
        let source = file.bound().view().source_file()?;
        Ok(metadata::normal_mode(
            source.parse_options().file_name.as_bytes(),
            self.file_metadata(file)?,
            self.file_options(file)?,
        ))
    }

    fn get_default_resolution_mode_for_file(
        &self,
        file_name: &[u8],
    ) -> Result<ResolutionMode, Error> {
        let file = self.required_file(file_name)?;
        let source = file.bound().view().source_file()?;
        Ok(metadata::default_resolution_mode_for_file(
            source.parse_options().file_name.as_bytes(),
            self.file_metadata(file)?,
            self.file_options(file)?,
        ))
    }

    // port: tsc/internal/compiler/program.go:Program.GetResolvedModule
    fn get_resolved_module(
        &self,
        file_name: &[u8],
        module_reference: &[u8],
        mode: ResolutionMode,
    ) -> Result<Option<&ResolvedModule>, Error> {
        let file = self.required_file(file_name)?;
        let source = file.bound().view().source_file()?;
        let file_path = source.parse_options().path.as_bytes();
        let resolutions = self.program.resolutions();
        // Loader publication sorts and deduplicates this exact tuple.
        Ok(resolutions
            .binary_search_by(|entry| {
                (entry.file.as_bytes(), entry.name.as_bytes(), entry.mode).cmp(&(
                    file_path,
                    module_reference,
                    mode,
                ))
            })
            .ok()
            .map(|index| &resolutions[index].result))
    }

    // port: tsc/internal/compiler/program.go:Program.GetSourceFileMetaData
    fn get_source_file_meta_data(&self, file_name: &[u8]) -> Result<&SourceFileMetaData, Error> {
        self.file_metadata(self.required_file(file_name)?)
    }

    // port: tsc/internal/ast/ast.go:Node.JSDoc
    fn jsdoc(
        &self,
        view: tsr_ast::AstView<'_>,
        source: NodeId,
        parent: NodeId,
    ) -> Result<tsr_ast::JSDocRoots, Error> {
        use tsr_ast::JsDocProvider;
        tsr_parser::ParserJsDocProvider::default()
            .jsdoc(view, source, parent)
            .map_err(Error::from)
    }

    // port: tsc/internal/compiler/program.go:Program.GetPackagesMap
    fn package_bundles_types(&self, package_name: &[u8]) -> Result<Option<bool>, Error> {
        let mut found = None;
        for resolution in self.program.resolutions() {
            let module = &resolution.result;
            if module.package_id.name.as_bytes() == package_name {
                let bundles = found.unwrap_or(false) || module.extension.as_bytes() == b".d.ts";
                found = Some(bundles);
            }
        }
        Ok(found)
    }

    // port: tsc/internal/compiler/program.go:Program.SourceFileMayBeEmitted
    fn source_file_may_be_emitted(
        &self,
        file: &CompletedFile,
        force_dts_emit: bool,
    ) -> Result<bool, Error> {
        let index = self
            .program
            .owners
            .node_file_index(file.source())
            .ok_or(tsr_arena::Error::WrongOwner)?;
        let retained = &self.program.files()[index];
        if retained.source() != file.source() {
            return Err(tsr_arena::Error::WrongOwner.into());
        }
        Ok(output_paths::may_emit_with_force_dts(
            retained,
            &self.program,
            force_dts_emit,
        )?)
    }

    fn is_source_file_default_library(&self, path: &[u8]) -> bool {
        self.program.is_lib(path)
    }

    fn get_redirect_for_resolution(
        &self,
        file_name: &[u8],
    ) -> Result<Option<&ParsedCommandLine>, Error> {
        let source = self
            .required_file(file_name)?
            .bound()
            .view()
            .source_file()?;
        Ok(self.program.redirect_for_resolution(
            source.parse_options().path.as_bytes(),
            source.parse_options().file_name.as_bytes(),
        ))
    }

    // port: tsc/internal/compiler/program.go:Program.GetProjectReferenceFromOutputDts
    fn get_project_reference_from_output_dts(
        &self,
        path: &[u8],
    ) -> Result<Option<&ParsedCommandLine>, Error> {
        Ok(self
            .program
            .project_reference_from_output_dts(self.program.to_path(path).as_bytes()))
    }

    // port: tsc/internal/compiler/program.go:Program.GetProjectReferenceFromSource
    fn get_project_reference_from_source(
        &self,
        path: &[u8],
    ) -> Result<Option<ProjectReferenceSource<'_>>, Error> {
        Ok(self
            .program
            .project_reference_from_source(self.program.to_path(path).as_bytes())
            .map(|(output_dts, resolved)| ProjectReferenceSource {
                output_dts,
                resolved,
            }))
    }

    fn get_module_specifier_paths(
        &self,
        importer: &[u8],
        target: &[u8],
    ) -> Result<Vec<tsr_checker::ModuleSpecifierPath>, Error> {
        self.module_specifier_paths(importer, target)
    }
    // port: tsc/internal/compiler/program.go:Program.GetPackageJsonInfo
    fn get_package_json_info(
        &self,
        file: &[u8],
    ) -> Result<Option<Arc<tsr_module::PackageJson>>, Error> {
        let directory = path::directory(file);
        let result = self
            .program
            .package_resolver
            .lock()
            .expect("retained package resolver poisoned")
            .package_scope_untraced(&directory)?;
        Ok(result.filter(|package| package.directory.as_bytes() == directory))
    }
    // port: tsc/internal/compiler/program.go:Program.GetNearestAncestorDirectoryWithPackageJson
    fn get_nearest_ancestor_directory_with_package_json(
        &self,
        dir: &[u8],
    ) -> Result<Option<tsr_jsstring::JsString>, Error> {
        Ok(self
            .program
            .package_resolver
            .lock()
            .expect("retained package resolver poisoned")
            .package_scope_untraced(dir)?
            .map(|package| package.directory.clone()))
    }
    fn get_global_typings_cache_location(&self) -> Result<tsr_jsstring::JsString, Error> {
        // ProgramOptions has no global typings-cache input and Resolver::new
        // constructs no global cache. Package-local @types remains supported.
        Ok(tsr_jsstring::JsString::default())
    }
    fn get_output_js_file_name(&self, file: &[u8]) -> Result<tsr_jsstring::JsString, Error> {
        Ok(tsr_jsstring::JsString::from_bytes(
            output_paths::module_specifier_output_name(
                file,
                &self.program,
                self.common_source_directory()?,
                false,
            ),
        ))
    }
    fn get_output_declaration_file_name(
        &self,
        file: &[u8],
    ) -> Result<tsr_jsstring::JsString, Error> {
        Ok(tsr_jsstring::JsString::from_bytes(
            output_paths::module_specifier_output_name(
                file,
                &self.program,
                self.common_source_directory()?,
                true,
            ),
        ))
    }

    // port: tsc/internal/compiler/program.go:Program.CommonSourceDirectory
    fn common_source_directory(&self) -> Result<&[u8], Error> {
        match self.common_source_directory.get_or_init(|| {
            let files = self.emitted_file_names()?;
            // Program::load already ran the corresponding option verifier,
            // including checkSourceFilesBelongToPath's membership diagnostics.
            Ok(output_paths::common_directory(&self.program, &files))
        }) {
            Ok(directory) => Ok(directory),
            Err(error) => Err((*error).into()),
        }
    }

    fn get_current_directory(&self) -> &[u8] {
        self.program.current_directory()
    }

    fn use_case_sensitive_file_names(&self) -> bool {
        self.program.use_case_sensitive_file_names()
    }
}

#[cfg(test)]
#[path = "checker_host_tests.rs"]
mod tests;
