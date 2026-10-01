//! The emit-host operations the declaration transformers read (the pin's
//! `DeclarationEmitHost` interface). `GetEffectiveDeclarationFlags` and
//! `GetEmitResolver` are the resolver's; the module-specifier host is unread
//! by these transformers.
use tsr_ast::{FileReference, NodeId};
pub use tsr_tsoptions::output_paths::OutputPaths;

/// Go's `DeclarationEmitHost`. Files are the program's parsed source files,
/// named by their root node.
pub trait DeclarationEmitHost {
    fn get_current_directory(&self) -> &[u8];
    fn use_case_sensitive_file_names(&self) -> bool;
    /// The program file a triple-slash `path` reference of `origin` names.
    fn get_source_file_from_reference(
        &self,
        origin: NodeId,
        reference: &FileReference,
    ) -> Option<NodeId>;
    /// The program's file named `file_name`: how a mapped file's supplemental
    /// files, which it names, are found.
    fn get_source_file(&self, file_name: &[u8]) -> Option<NodeId>;
    fn get_output_paths_for(&self, file: NodeId, force_dts_paths: bool) -> OutputPaths;
    fn source_file_may_be_emitted(&self, file: NodeId, force_dts_emit: bool) -> bool;
}
