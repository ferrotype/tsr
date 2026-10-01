//! Source-to-source transforms operate on retained AST owners. Checker behavior
//! enters through the printer's resolver contract, never a checker dependency.
pub mod declarations;
pub mod modifier_visitor;
pub mod transformer;

pub use modifier_visitor::extract_modifiers;
pub use transformer::{
    chain, EmitModuleFormatOfFile, Error, Failure, SharedEmitResolver, SharedReferenceResolver,
    TransformOptions, Transformer, TransformerFactory,
};

#[cfg(test)]
mod transformer_tests;
