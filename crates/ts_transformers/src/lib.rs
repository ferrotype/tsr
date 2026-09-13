//! Source-to-source transforms operate on retained AST owners. Checker behavior
//! enters through the printer's resolver contract, never a checker dependency.
pub mod declarations;
