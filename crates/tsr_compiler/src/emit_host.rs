//! `compiler/emitHost.go`: the emit host one file's emitter reads, over the
//! program and the emit resolver of the checker that checks the file.
//!
//! The pin's host also serves the declaration transformer and the printer's
//! module-specifier queries. Three operations are not offered: their program
//! counterpart has no Rust port yet (`GetSourceFileFromReference`, T7's), or
//! the program does not retain what they read (`GetRedirectTargets` reads the
//! loader's `redirectTargetsMap` in load order, `ResolveModuleName` the
//! program's module resolver).
use crate::{output_paths, CheckedProgram, Error, Program, ProgramCheckerHost, ProgramFile};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::NodeId;
use tsr_checker::{CheckerHost, CheckerRequest, ProjectReferenceSource};
use tsr_core::{CompilerOptions, ModuleKind, ResolutionMode};
use tsr_jsstring::JsString;
use tsr_module::symlinks::KnownSymlinks;
use tsr_module::ResolvedModule;
use tsr_transformers::SharedEmitResolver;
use tsr_tsoptions::output_paths::{
    get_output_paths_for, ForceEmitPaths, OutputPaths, OutputPathsHost,
};

/// `emitHost`: the program and the file's checker's emit resolver.
pub struct EmitHost<'a> {
    program: &'a Program,
    checker_host: &'a ProgramCheckerHost,
    emit_resolver: SharedEmitResolver<'a>,
    /// `Program.CommonSourceDirectory`, which the program computes once.
    common_source_directory: JsString,
}

/// Runs `task` with the emit host of `file`: the checker that checks the
/// file serves as its emit resolver and is released when `task` returns,
/// as the pin's `done`.
// port: tsc/internal/compiler/emitHost.go:newEmitHost
pub(crate) fn new_emit_host<R>(
    checked: &CheckedProgram,
    request: &CheckerRequest,
    checker_host: &ProgramCheckerHost,
    file: &ProgramFile,
    task: &mut dyn FnMut(&EmitHost<'_>) -> R,
) -> Result<R, Error> {
    let common_source_directory = JsString::from_bytes(checker_host.common_source_directory()?);
    let mut result = None;
    checked.with_type_checker_for_file(request, file.source(), &mut |operation| {
        let host = EmitHost {
            program: checked.program(),
            checker_host,
            emit_resolver: Rc::new(RefCell::new(operation)),
            common_source_directory: common_source_directory.clone(),
        };
        result = Some(task(&host));
        Ok(())
    })?;
    Ok(result.expect("the emit task ran"))
}

impl<'a> EmitHost<'a> {
    pub fn program(&self) -> &'a Program {
        self.program
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetModeForUsageLocation
    pub fn get_mode_for_usage_location(
        &self,
        file_name: &[u8],
        module_specifier: NodeId,
    ) -> Result<ResolutionMode, Error> {
        Ok(self
            .checker_host
            .get_mode_for_usage_location(file_name, module_specifier)?)
    }

    /// `file` by its name; `module_specifier` is a node of that file.
    // port: tsc/internal/compiler/emitHost.go:emitHost.GetResolvedModuleFromModuleSpecifier
    pub fn get_resolved_module_from_module_specifier(
        &self,
        file_name: &[u8],
        module_specifier: NodeId,
    ) -> Result<Option<&'a ResolvedModule>, Error> {
        // `Program.GetResolvedModuleFromModuleSpecifier`.
        let file = self
            .program
            .source_file(file_name)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        let view = file.bound().view().ast();
        let node = view.node(module_specifier)?;
        assert!(
            tsr_ast::utilities::is_string_literal_like(&node),
            "moduleSpecifier must be a StringLiteralLike"
        );
        let mode = self
            .checker_host
            .get_mode_for_usage_location(file_name, module_specifier)?;
        let text = view.node_text(module_specifier)?;
        Ok(self
            .checker_host
            .get_resolved_module(file_name, text.as_bytes(), mode)?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetDefaultResolutionModeForFile
    pub fn get_default_resolution_mode_for_file(
        &self,
        file_name: &[u8],
    ) -> Result<ResolutionMode, Error> {
        Ok(self
            .checker_host
            .get_default_resolution_mode_for_file(file_name)?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetEmitModuleFormatOfFile
    pub fn get_emit_module_format_of_file(&self, file_name: &[u8]) -> Result<ModuleKind, Error> {
        Ok(self.program.emit_module_format_of_file_name(file_name)?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.FileExists
    pub fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(self.checker_host.file_exists(path)?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetGlobalTypingsCacheLocation
    pub fn get_global_typings_cache_location(&self) -> Result<JsString, Error> {
        Ok(self.checker_host.get_global_typings_cache_location()?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetNearestAncestorDirectoryWithPackageJson
    pub fn get_nearest_ancestor_directory_with_package_json(
        &self,
        dirname: &[u8],
    ) -> Result<Option<JsString>, Error> {
        Ok(self
            .checker_host
            .get_nearest_ancestor_directory_with_package_json(dirname)?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetPackageJsonInfo
    pub fn get_package_json_info(
        &self,
        pkg_json_path: &[u8],
    ) -> Result<Option<Arc<tsr_module::PackageJson>>, Error> {
        Ok(self.checker_host.get_package_json_info(pkg_json_path)?)
    }

    /// `file` by its path and name.
    // port: tsc/internal/compiler/emitHost.go:emitHost.GetSourceOfProjectReferenceIfOutputIncluded
    pub fn get_source_of_project_reference_if_output_included<'n>(
        &self,
        path: &[u8],
        file_name: &'n [u8],
    ) -> &'n [u8]
    where
        'a: 'n,
    {
        self.program
            .source_of_project_reference_if_output_included(path, file_name)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetProjectReferenceFromSource
    pub fn get_project_reference_from_source(
        &self,
        path: &[u8],
    ) -> Result<Option<ProjectReferenceSource<'a>>, Error> {
        Ok(self.checker_host.get_project_reference_from_source(path)?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetEffectiveDeclarationFlags
    pub fn get_effective_declaration_flags(&self, node: NodeId, flags: u32) -> Result<u32, Error> {
        self.get_emit_resolver()
            .borrow_mut()
            .get_effective_declaration_flags(node, flags)
            .map_err(|error| Error::Transform(error.into()))
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetOutputPathsFor
    pub fn get_output_paths_for(
        &self,
        file: &ProgramFile,
        force_dts_paths: bool,
    ) -> Result<OutputPaths, Error> {
        let source = file.bound().view().source_file()?;
        Ok(get_output_paths_for(
            &source,
            self.options(),
            &mut self.output_paths_host(),
            ForceEmitPaths {
                dts: force_dts_paths,
                ..ForceEmitPaths::default()
            },
        ))
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.SourceFileMayBeEmitted
    pub fn source_file_may_be_emitted(
        &self,
        file: &ProgramFile,
        force_dts_emit: bool,
    ) -> Result<bool, Error> {
        Ok(output_paths::may_emit_with_force(
            file,
            self.program,
            force_dts_emit,
            false,
        )?)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.Options
    pub fn options(&self) -> &'a CompilerOptions {
        self.program.options()
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.SourceFiles
    pub fn source_files(&self) -> &'a [Arc<ProgramFile>] {
        self.program.files()
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetCurrentDirectory
    pub fn get_current_directory(&self) -> &'a [u8] {
        self.program.current_directory()
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.CommonSourceDirectory
    pub fn common_source_directory(&self) -> &[u8] {
        self.common_source_directory.as_bytes()
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.ContentMapperExtensions
    pub fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.program.content_mapper_extensions()
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.UseCaseSensitiveFileNames
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.program.use_case_sensitive_file_names()
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.IsEmitBlocked
    pub fn is_emit_blocked(&self, file: &[u8]) -> bool {
        self.program.is_emit_blocked(file)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.WriteFile
    pub fn write_file(&self, file_name: &[u8], text: &[u8]) -> Result<(), tsr_vfs::Error> {
        self.program.host().write_file(file_name, text)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetEmitResolver
    pub fn get_emit_resolver(&self) -> SharedEmitResolver<'a> {
        self.emit_resolver.clone()
    }

    /// `file` by its path.
    // port: tsc/internal/compiler/emitHost.go:emitHost.IsSourceFileFromExternalLibrary
    pub fn is_source_file_from_external_library(&self, path: &[u8]) -> bool {
        self.program.is_external_library(path)
    }

    // port: tsc/internal/compiler/emitHost.go:emitHost.GetSymlinkCache
    pub fn get_symlink_cache(&self) -> Result<&'a KnownSymlinks, Error> {
        Ok(self.checker_host.known_symlinks()?)
    }

    /// The host as `outputpaths`' host, which the pin's `emitHost` is.
    pub(crate) fn output_paths_host(&self) -> EmitOutputPathsHost<'_, 'a> {
        EmitOutputPathsHost(self)
    }
}

/// [`EmitHost`] as an [`OutputPathsHost`].
pub(crate) struct EmitOutputPathsHost<'h, 'a>(&'h EmitHost<'a>);

impl OutputPathsHost for EmitOutputPathsHost<'_, '_> {
    fn common_source_directory(&mut self) -> JsString {
        self.0.common_source_directory.clone()
    }
    fn content_mapper_extensions(&self) -> Vec<JsString> {
        self.0.content_mapper_extensions()
    }
    fn get_current_directory(&self) -> &[u8] {
        self.0.get_current_directory()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.0.use_case_sensitive_file_names()
    }
}
