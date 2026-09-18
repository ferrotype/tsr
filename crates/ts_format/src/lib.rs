//! The formatter: a port of `tsc/internal/format`.
//!
//! It is being brought in step by step (docs/S09-3-formatter-plan.md). Each
//! step is held to the pinned implementation by `scripts/s09_format.py compare`
//! over the frozen parser inventory. This crate currently holds the
//! foundations: settings, the formatting scanner, the formatting context, the
//! rule types and the AST helpers the rest is built on.
#![forbid(unsafe_code)]
// The port lands bottom-up, so until the span worker (step F6) arrives most of
// these foundations have no caller inside the crate. Remove with F6.
#![allow(dead_code)]

mod context;
mod lsutil;
mod rule;
mod rulecontext;
mod rules;
mod rulesmap;
mod scanner;
mod settings;
mod util;

#[doc(hidden)]
pub mod probe;

pub use context::{FormatRequestKind, FormattingContext};
pub use scanner::TextRangeWithKind;
pub use settings::{EditorSettings, FormatCodeSettings, IndentStyle, SemicolonPreference};
pub use ts_astnav::Error;
pub use util::get_line_start_position_for_position;

/// `debug.Assert(condition)`: upstream panics with this message.
pub(crate) fn debug_assert(condition: bool) -> Result<(), Error> {
    if condition {
        Ok(())
    } else {
        Err(Error::Assertion("Debug failure. False expression.".into()))
    }
}

/// What every formatter entry point works over: the file's view, its
/// `SourceFile` node and the JSDoc the navigation needs.
pub struct FormatFile<'a, 'p> {
    pub view: ts_ast::AstView<'a>,
    pub source: ts_arena::NodeId,
    pub jsdoc: &'p mut dyn ts_ast::JsDocProvider,
}

impl<'a> FormatFile<'a, '_> {
    pub(crate) fn node(&self, id: ts_arena::NodeId) -> Result<ts_ast::NodeRead<'a>, Error> {
        Ok(self.view.node(id)?)
    }

    pub(crate) fn navigator(&mut self) -> ts_astnav::Navigator<'a, '_> {
        ts_astnav::Navigator::new(self.view, self.source, self.jsdoc)
    }

    /// `scanner.GetECMALineOfPosition`, over the file's cached line map.
    pub(crate) fn line_of(&self, position: i64) -> Result<i64, Error> {
        let file = self.view.source_file(self.source)?;
        Ok(ts_jsstring::scanner_positions::compute_line_of_position(
            file.ecma_line_map(),
            position as isize,
        ) as i64)
    }

    /// `scanner.GetTokenPosOfNode(node, file, false)`.
    pub(crate) fn token_pos(&mut self, node: ts_arena::NodeId) -> Result<i64, Error> {
        Ok(ts_scanner::get_token_pos_of_node(
            self.view,
            self.source,
            node,
            false,
            self.jsdoc,
        )?)
    }
}

#[cfg(test)]
mod tests;
