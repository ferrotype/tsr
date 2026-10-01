//! `supplementalreferences.go`: the declaration file of a content mapper's
//! canonical output references the declaration files of its supplemental
//! outputs.
use super::host::DeclarationEmitHost;
use tsr_ast::{AstBuilder, Diagnostic, Factory, FileReference, JsString, NodeId};

/// SupplementalReferencesTransformer adds triple-slash path references from a content mapper's
/// canonical declaration output to the declaration files emitted for its supplemental files.
/// This ensures that consumers loading the canonical declaration also include the supplemental types.
pub struct SupplementalReferencesTransformer<'a> {
    host: &'a dyn DeclarationEmitHost,
    supplemental_files: Vec<NodeId>,
    declaration_file_path: JsString,
    force_declaration_paths: bool,
}

impl<'a> SupplementalReferencesTransformer<'a> {
    /// `source_file` is read from `output`, which retains it.
    // port: tsc/internal/transformers/declarations/supplementalreferences.go:NewSupplementalReferencesTransformer
    pub fn new(
        host: &'a dyn DeclarationEmitHost,
        output: &AstBuilder,
        source_file: NodeId,
        declaration_file_path: JsString,
        force_declaration_paths: bool,
    ) -> Result<Self, tsr_arena::Error> {
        let supplemental_files = output
            .read_source_file(source_file)?
            .supplemental_source_files()?
            .iter()
            .flatten()
            .copied()
            .collect();
        Ok(Self {
            host,
            supplemental_files,
            declaration_file_path,
            force_declaration_paths,
        })
    }

    /// Appends the references to the transformed file's `ReferencedFiles`.
    // port: tsc/internal/transformers/declarations/supplementalreferences.go:SupplementalReferencesTransformer.TransformSourceFile
    pub fn transform_source_file(
        &self,
        output: &mut AstBuilder,
        source_file: NodeId,
    ) -> Result<NodeId, tsr_arena::Error> {
        for &supplemental in &self.supplemental_files {
            if !self
                .host
                .source_file_may_be_emitted(supplemental, self.force_declaration_paths)
            {
                continue;
            }
            let paths = self
                .host
                .get_output_paths_for(supplemental, self.force_declaration_paths);
            let declaration_path = paths.declaration_file_path();
            if declaration_path.is_empty() {
                continue;
            }
            let mut referenced_files: Vec<FileReference> = output
                .read_source_file(source_file)?
                .referenced_files()?
                .to_vec();
            referenced_files.push(FileReference {
                loc: tsr_core::TextRange::new(-1, -1),
                file_name: JsString::from_bytes(tsr_tspath::relative_from_file(
                    self.declaration_file_path.as_bytes(),
                    declaration_path,
                    self.host.get_current_directory(),
                    self.host.use_case_sensitive_file_names(),
                )),
                resolution_mode: Default::default(),
                preserve: false,
            });
            let referenced_files = output.source_references(referenced_files)?;
            output.mut_source_file(source_file)?.referenced_files = referenced_files;
        }
        Ok(source_file)
    }

    // port: tsc/internal/transformers/declarations/supplementalreferences.go:SupplementalReferencesTransformer.GetDiagnostics
    pub fn get_diagnostics(&self) -> Vec<Diagnostic> {
        Vec::new()
    }
}
