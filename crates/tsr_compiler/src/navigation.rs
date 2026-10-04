//! Read-only navigation into resolutions retained by the published program.
use crate::{metadata, Error, Program, ProgramFile};
use tsr_ast::{FileReference, NodeId};
use tsr_core::ResolutionMode;
use tsr_module::{ResolvedModule, ResolvedTypeReferenceDirective};

impl Program {
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
