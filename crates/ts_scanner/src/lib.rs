//! The pinned compiler scanner over borrowed, arbitrary source bytes.
//!
//! Token views borrow their source or cooked storage. Checkpoints are opaque,
//! consumed in LIFO order, and restore scanner state rather than configuration.

mod binder_helpers;
mod escape;
mod identifier;
mod literal;
mod number;
mod regexp;
mod rescan;
mod scan;
mod spelling;
mod state;
mod tables_generated;
mod trivia;
mod utilities;

pub use identifier::{
    get_identifier_token, get_viable_keyword_suggestions, is_identifier_part,
    is_identifier_part_ex, is_identifier_start, is_valid_identifier, string_to_token,
    token_to_string,
};
pub use spelling::{equal_fold, get_spelling_suggestion, get_spelling_suggestion_for_strings};
pub use state::TokenValue as RetainedTokenValue;
pub use state::{Checkpoint, DiagnosticArgument, ErrorCallback, Scanner, ScannerDiagnostic};
pub use trivia::{
    could_start_trivia, get_leading_comment_ranges, get_shebang, get_trailing_comment_ranges,
    skip_trivia, skip_trivia_ex, CommentRange, SkipTriviaOptions,
};
pub use utilities::{
    identifier_to_keyword_kind, is_identifier_text, is_intrinsic_jsx_name,
    normalize_jsdoc_type_source_text,
};

pub(crate) use state::{escape_flags, IdentifierVariant, TokenValue};

pub use utilities::{is_line_break, is_white_space_like, is_white_space_single_line};

pub use binder_helpers::*;
