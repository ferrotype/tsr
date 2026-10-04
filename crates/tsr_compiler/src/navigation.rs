//! Read-only navigation into resolutions retained by the published program.
use crate::{metadata, Error, Program, ProgramFile};
use tsr_ast::{FileReference, NodeId};
use tsr_core::ResolutionMode;
use tsr_module::{ResolvedModule, ResolvedTypeReferenceDirective};

/// Module format for a navigation target which need not belong to the program.
pub fn navigation_module_format(name: &[u8], package_type: &[u8]) -> tsr_core::ModuleKind {
    metadata::implied_node_format_for_file(name, package_type)
}
impl Program {
    pub fn usage_resolution_mode(
        &self,
        file: &ProgramFile,
        specifier: NodeId,
    ) -> Result<ResolutionMode, Error> {
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        let path = source.parse_options().path.as_bytes();
        metadata::usage_mode(
            view,
            source.file_name(),
            self.metadata(path).ok_or(tsr_arena::Error::InvalidGraph)?,
            specifier,
            self.options_for_file(path, source.file_name()),
        )
    }

    /// Implicit side-effect imports have no local bindings. Navigation uses
    /// the same eligibility and names as the loader, in the pin's JSX/helpers
    /// order, without publishing synthetic nodes into the parsed source.
    pub fn implicit_imports(
        &self,
        file: &ProgramFile,
    ) -> Result<Vec<tsr_jsstring::JsString>, Error> {
        use tsr_core::ScriptKind;
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        let options =
            self.options_for_file(source.parse_options().path.as_bytes(), source.file_name());
        let mut result = Vec::new();
        if matches!(
            source.script_kind,
            ScriptKind::JS | ScriptKind::JSX | ScriptKind::TSX
        ) {
            let runtime = metadata::jsx_runtime_import(
                metadata::jsx_implicit_import_base(view, file.source(), options)?.as_bytes(),
                options,
            );
            if !runtime.is_empty() {
                result.push(runtime);
            }
        }
        if options.import_helpers.is_true()
            && (matches!(source.script_kind, ScriptKind::JS | ScriptKind::JSX)
                || !source.is_declaration_file
                    && (options.isolated_modules() || source.external_module_indicator.is_some()))
        {
            result.push(tsr_jsstring::JsString::from_bytes(b"tslib".as_slice()));
        }
        Ok(result)
    }
    pub fn source_file_from_reference(
        &self,
        origin: &ProgramFile,
        reference: &FileReference,
    ) -> Option<&ProgramFile> {
        crate::declaration_host::get_source_file_from_reference(self, origin, reference)
    }

    // port: tsc/internal/compiler/program.go:Program.GetLibFileFromReference
    pub fn lib_file_from_reference(&self, reference: &FileReference) -> Option<&ProgramFile> {
        let path = tsr_tsoptions::lib_file_name(reference.file_name.as_bytes())?;
        self.file(path.as_bytes())
    }

    // port: tsc/internal/compiler/program.go:Program.GetResolvedTypeReferenceDirectiveFromTypeReferenceDirective
    pub fn resolved_type_reference_from_directive<'a>(
        &'a self,
        file: &'a ProgramFile,
        reference: &FileReference,
    ) -> Result<Option<&'a ResolvedTypeReferenceDirective>, Error> {
        let source = file.bound().view().source_file()?;
        let mode = if reference.resolution_mode != 0 {
            tsr_core::ModuleKind(reference.resolution_mode as i32)
        } else {
            self.default_resolution_mode(file)?
        };
        Ok(self
            .resolved_type_reference_directives(source.parse_options().path.as_bytes())
            .find(|r| r.name == reference.file_name && r.mode == mode)
            .map(|r| &r.result))
    }

    pub fn default_resolution_mode(&self, file: &ProgramFile) -> Result<ResolutionMode, Error> {
        let source = file.bound().view().source_file()?;
        let path = source.parse_options().path.as_bytes();
        Ok(metadata::default_resolution_mode_for_file(
            source.file_name(),
            self.metadata(path).ok_or(tsr_arena::Error::InvalidGraph)?,
            self.options_for_file(path, source.file_name()),
        ))
    }

    // port: tsc/internal/compiler/program.go:Program.GetResolvedModuleFromModuleSpecifier
    pub fn resolved_module_from_specifier(
        &self,
        file: &ProgramFile,
        specifier: NodeId,
    ) -> Result<Option<&ResolvedModule>, Error> {
        let view = file.bound().view().ast();
        let source = view.source_file(file.source())?;
        let path = source.parse_options().path.as_bytes();
        let mode = metadata::usage_mode(
            view,
            source.file_name(),
            self.metadata(path).ok_or(tsr_arena::Error::InvalidGraph)?,
            specifier,
            self.options_for_file(path, source.file_name()),
        )?;
        let name = view.node_text(specifier)?;
        let resolutions = self.resolutions();
        Ok(resolutions
            .binary_search_by(|r| {
                (r.file.as_bytes(), r.name.as_bytes(), r.mode).cmp(&(path, name.as_bytes(), mode))
            })
            .ok()
            .map(|i| &resolutions[i].result))
    }
}
