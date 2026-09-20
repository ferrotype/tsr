//! The formatter: a port of `tsc/internal/format`.
//!
//! It was brought in step by step (docs/S09-3-formatter-plan.md), and each step
//! is held to the pinned implementation by `scripts/s09_format.py compare` over
//! the frozen parser inventory: the formatting scanner, the rules and their
//! selection, the smart indenter, and the span worker behind the entry points.
#![forbid(unsafe_code)]

mod api;
mod context;
mod indent;
mod lsutil;
mod recursion;
mod rule;
mod rulecontext;
mod rules;
mod rulesmap;
mod scanner;
mod settings;
mod span;
mod util;

#[doc(hidden)]
pub mod probe;

pub use api::{
    format_document, format_node_given_indentation, format_on_closing_curly, format_on_enter,
    format_on_opening_curly, format_on_semicolon, format_selection, format_span, FormatContext,
};
pub use context::{FormatRequestKind, FormattingContext};
pub use indent::{
    find_first_non_whitespace_column, get_containing_list, get_indentation,
    get_indentation_for_node, node_will_indent_child, should_indent_child_node,
};
pub use scanner::TextRangeWithKind;
pub use settings::{EditorSettings, FormatCodeSettings, IndentStyle, SemicolonPreference};
pub use tsr_astnav::Error;
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
    pub view: tsr_ast::AstView<'a>,
    pub source: tsr_arena::NodeId,
    pub jsdoc: &'p mut dyn tsr_ast::JsDocProvider,
}

impl<'a> FormatFile<'a, '_> {
    pub(crate) fn node(&self, id: tsr_arena::NodeId) -> Result<tsr_ast::NodeRead<'a>, Error> {
        Ok(self.view.node(id)?)
    }

    pub(crate) fn navigator(&mut self) -> tsr_astnav::Navigator<'a, '_> {
        tsr_astnav::Navigator::new(self.view, self.source, self.jsdoc)
    }

    /// `scanner.GetECMALineOfPosition`, over the file's cached line map.
    pub(crate) fn line_of(&self, position: i64) -> Result<i64, Error> {
        let file = self.view.source_file(self.source)?;
        Ok(tsr_jsstring::scanner_positions::compute_line_of_position(
            file.ecma_line_map(),
            position as isize,
        ) as i64)
    }

    /// One entry of the ECMA line map. Upstream indexes the slice directly, so
    /// a line outside it is a runtime panic with this message.
    pub(crate) fn line_start(&self, line: i64) -> Result<i64, Error> {
        let file = self.view.source_file(self.source)?;
        let map = file.ecma_line_map();
        match usize::try_from(line).ok().and_then(|index| map.get(index)) {
            Some(&start) => Ok(i64::from(start)),
            None if line < 0 => Err(Error::Assertion(format!(
                "runtime error: index out of range [{line}]"
            ))),
            None => Err(Error::Assertion(format!(
                "runtime error: index out of range [{line}] with length {}",
                map.len()
            ))),
        }
    }

    /// `scanner.GetECMALineAndByteOffsetOfPosition`.
    pub(crate) fn line_and_offset(&self, position: i64) -> Result<(i64, i64), Error> {
        let line = self.line_of(position)?;
        Ok((line, position - self.line_start(line)?))
    }

    /// `scanner.GetTokenPosOfNode(node, file, false)`.
    pub(crate) fn token_pos(&mut self, node: tsr_arena::NodeId) -> Result<i64, Error> {
        Ok(tsr_scanner::get_token_pos_of_node(
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
