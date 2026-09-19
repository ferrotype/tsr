//! The program surface the checker consumes (`checker.Program` and `checker.Host`
//! in `tsc/internal/checker/checker.go`; `Host` is
//! `modulespecifiers.ModuleSpecifierGenerationHost`).
//!
//! The trait lives here, below `tsr_compiler`, and the compiler implements it over
//! its loader (plan §3). Only members whose types already exist below the
//! compiler are in the trait. The rest are P2 obligations, listed so none of them
//! is quietly given a default:
//!
//! | Upstream member | Status |
//! | --- | --- |
//! | `Options`, `SourceFiles`, `FileExists`, `GetSourceFile`, `IsSourceFileDefaultLibrary`, `CommonSourceDirectory`, `GetCurrentDirectory`, `UseCaseSensitiveFileNames` | In the trait; the loader already has the data |
//! | `GetSourceFileForResolvedModule`, `GetResolvedModule`, `GetEmitModuleFormatOfFile`, `GetImpliedNodeFormatForEmit`, `GetEmitSyntaxForUsageLocation`, `GetModeForUsageLocation`, `GetDefaultResolutionModeForFile`, `SourceFileMayBeEmitted` | Implemented by the compiler's retained program adapter |
//! | `BindSourceFiles` | Satisfied by construction: program files are `CompletedFile`s |
//! | `GetSourceFileMetaData` | In the trait; metadata lives in `tsr_ast`, as upstream |
//! | `GetResolvedModules`, `GetPackagesMap` | P2: whole-map views; add when a caller in the closure needs them |
//! | `GetJSXRuntimeImportSpecifier` | JSX remains outside the frozen denominator |
//! | `GetImportHelpersImportSpecifier` | P4: checker consumes the synthetic import as its retained `tslib` reference and host-computed import resolution mode; no synthetic syntax escapes the compiler |
//! | `GetRedirectTargets`, `GetSourceOfProjectReferenceIfOutputIncluded` | P4: package-identity redirects feed the module paths; project references remain rejected by loading |
//! | `GetRedirectForResolution`, `GetProjectReferenceFromSource`, `GetProjectReferenceFromOutputDts` | P4: the compiler adapter proves the project-reference map is empty because loading rejects nonempty references; the lookup therefore returns `None` without supporting reference loading |
//! | `GetSymlinkCache`, `GetPackageJsonInfo`, `GetNearestAncestorDirectoryWithPackageJson`, `GetGlobalTypingsCacheLocation` | P4: module paths use retained resolutions plus native runtime-dependency discovery; direct package queries share the retained resolver cache; global typings cache has no configured input |
//! | `ContentMapperExtensions`, `GetResolvedModuleFromModuleSpecifier` | Content mapper execution remains outside the loaded-program closure; module references use retained resolutions |

use crate::Error;
use std::sync::Arc;
use tsr_arena::NodeId;
use tsr_ast::{CompletedFile, SourceFileMetaData};
use tsr_core::{CompilerOptions, ModuleKind, ResolutionMode};
use tsr_jsstring::JsString;
use tsr_module::ResolvedModule;
use tsr_tsoptions::ParsedCommandLine;

/// One spelling supplied by GetEachFileNameOfModule, before proximity sorting.
#[derive(Clone, Debug)]
pub struct ModuleSpecifierPath {
    pub file_name: JsString,
    pub is_in_node_modules: bool,
    pub is_redirect: bool,
}

/// File names and directories are bytes, as everywhere in this port.
pub trait CheckerHost: Send + Sync {
    fn options(&self) -> &CompilerOptions;
    fn source_file_count(&self) -> usize;
    /// Program order; the checker's file index map and node ordering follow it.
    fn source_file(&self, index: usize) -> &CompletedFile;
    fn file_exists(&self, file_name: &[u8]) -> Result<bool, tsr_vfs::Error>;
    fn get_source_file(&self, file_name: &[u8]) -> Option<&CompletedFile>;
    fn get_source_file_for_resolved_module(&self, file_name: &[u8]) -> Option<&CompletedFile>;
    fn get_emit_module_format_of_file(&self, file_name: &[u8]) -> Result<ModuleKind, Error>;
    fn get_emit_syntax_for_usage_location(
        &self,
        file_name: &[u8],
        usage_location: NodeId,
    ) -> Result<ResolutionMode, Error>;
    fn get_mode_for_usage_location(
        &self,
        file_name: &[u8],
        usage_location: NodeId,
    ) -> Result<ResolutionMode, Error>;
    /// Mode of the compiler's synthetic, attribute-free import of `tslib`.
    fn get_import_helpers_resolution_mode(&self, file_name: &[u8])
        -> Result<ResolutionMode, Error>;
    fn get_default_resolution_mode_for_file(
        &self,
        file_name: &[u8],
    ) -> Result<ResolutionMode, Error>;
    fn get_implied_node_format_for_emit(&self, file_name: &[u8]) -> Result<ModuleKind, Error>;
    fn get_resolved_module(
        &self,
        file_name: &[u8],
        module_reference: &[u8],
        mode: ResolutionMode,
    ) -> Result<Option<&ResolvedModule>, Error>;
    fn get_source_file_meta_data(&self, file_name: &[u8]) -> Result<&SourceFileMetaData, Error>;
    /// `GetPackagesMap()[packageName]`: `None` when no resolution named the
    /// package, otherwise whether any resolution of it was a `.d.ts`.
    fn package_bundles_types(&self, package_name: &[u8]) -> Result<Option<bool>, Error>;
    /// `Node.JSDoc(file)`: the JSDoc roots of `parent`, parsing them on first
    /// use when the file carries lazy JSDoc.
    fn jsdoc(
        &self,
        view: tsr_ast::AstView<'_>,
        source: NodeId,
        parent: NodeId,
    ) -> Result<tsr_ast::JSDocRoots, Error>;
    fn source_file_may_be_emitted(
        &self,
        file: &CompletedFile,
        force_dts_emit: bool,
    ) -> Result<bool, Error>;
    fn is_source_file_default_library(&self, path: &[u8]) -> bool;
    /// The retained compiler adapter proves these reference lookups are empty
    /// because loading rejects nonempty project-reference configurations.
    fn get_redirect_for_resolution(
        &self,
        file_name: &[u8],
    ) -> Result<Option<&ParsedCommandLine>, Error>;
    fn get_project_reference_from_output_dts(
        &self,
        path: &[u8],
    ) -> Result<Option<&ParsedCommandLine>, Error>;
    fn get_project_reference_from_source(
        &self,
        path: &[u8],
    ) -> Result<Option<&ParsedCommandLine>, Error>;
    fn get_module_specifier_paths(
        &self,
        importer: &[u8],
        target: &[u8],
    ) -> Result<Vec<ModuleSpecifierPath>, Error>;
    fn get_package_json_info(
        &self,
        file: &[u8],
    ) -> Result<Option<Arc<tsr_module::PackageJson>>, Error>;
    fn get_nearest_ancestor_directory_with_package_json(
        &self,
        dir: &[u8],
    ) -> Result<Option<JsString>, Error>;
    fn get_global_typings_cache_location(&self) -> Result<JsString, Error>;
    fn get_output_js_file_name(&self, file: &[u8]) -> Result<JsString, Error>;
    fn get_output_declaration_file_name(&self, file: &[u8]) -> Result<JsString, Error>;
    fn common_source_directory(&self) -> Result<&[u8], Error>;
    fn get_current_directory(&self) -> &[u8];
    fn use_case_sensitive_file_names(&self) -> bool;
}
