//! Source-to-source transforms operate on retained AST owners. Checker behavior
//! enters through the printer's resolver contract, never a checker dependency.
//!
//! The script transforms follow one idiom (see [`transformer`]): a constructor
//! `new_*_transformer(opts) -> Option<Transformer>` whose visit function is a
//! closure over the transformer's state; mutable state lives in `Cell`s and
//! `RefCell`s that are never borrowed across a visit; the visitor argument
//! gives the factory (`visitor.factory_mut()`) and child visiting
//! (`visitor.visit_node`, `visit_nodes`, `visit_each_child`); the emit context
//! is the options' clone; resolver queries go through the options' shared
//! handles; a failed storage read or resolver query is recorded in the
//! options' `Failure` and the node is returned unchanged.
pub mod declarations;
pub mod destructuring;
pub mod estransforms;
pub mod inliners;
pub mod jsxtransforms;
pub mod modifier_visitor;
pub mod moduletransforms;
pub mod transformer;
pub mod tstransforms;
pub mod utilities;

pub use modifier_visitor::extract_modifiers;
pub use transformer::{
    chain, EmitModuleFormatOfFile, Error, Failure, SharedEmitResolver, SharedReferenceResolver,
    TransformOptions, Transformer, TransformerFactory,
};

#[cfg(test)]
mod transformer_tests;
#[cfg(test)]
mod utilities_tests;
