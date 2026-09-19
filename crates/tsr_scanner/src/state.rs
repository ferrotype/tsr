use std::collections::HashMap;
use std::sync::Arc;

use ts_ast::{token_flags as flags, CommentDirective, SyntaxKind, TokenFlags};
use ts_core::{LanguageVariant, ScriptTarget, TextRange};
use ts_diagnostics::Message;
use ts_jsstring::{JsString, SourceText};

/// A scanner diagnostic argument retains the Go value's type and raw bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticArgument {
    String(Vec<u8>),
    Integer(i64),
}

impl From<Vec<u8>> for DiagnosticArgument {
    fn from(value: Vec<u8>) -> Self {
        Self::String(value)
    }
}
impl From<&[u8]> for DiagnosticArgument {
    fn from(value: &[u8]) -> Self {
        Self::String(value.to_vec())
    }
}
impl From<&str> for DiagnosticArgument {
    fn from(value: &str) -> Self {
        Self::String(value.as_bytes().to_vec())
    }
}
impl From<String> for DiagnosticArgument {
    fn from(value: String) -> Self {
        Self::String(value.into_bytes())
    }
}
impl From<i64> for DiagnosticArgument {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

/// Delivered synchronously in upstream emission order, outside checkpoint state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannerDiagnostic {
    pub message: &'static Message,
    pub start: i64,
    pub length: i64,
    pub args: Vec<DiagnosticArgument>,
}

pub type ErrorCallback<'src> = Box<dyn FnMut(ScannerDiagnostic) + Send + 'src>;

enum ErrorSink<'src> {
    Ignore,
    Callback(ErrorCallback<'src>),
    Buffered(Vec<ScannerDiagnostic>),
}

/// Retains cooked storage across scanner advancement. Source views still borrow
/// their original text; retaining the source owner is the caller's responsibility.
#[derive(Clone, Debug)]
pub enum TokenValue<'src> {
    Borrowed(&'src [u8]),
    Cooked(Arc<[u8]>),
    Static(&'static [u8]),
}

impl TokenValue<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) | Self::Static(bytes) => bytes,
            Self::Cooked(bytes) => bytes,
        }
    }

    /// Transfer cooked storage or retain a slice of its actual source allocation.
    /// Returns None for a nonempty borrowed value from another source. The byte
    /// range is reclassified by JsString without repair. Source/cooked bytes are
    /// shared; scanner-supplied static constants receive their own small storage.
    pub fn into_js_string(self, source: &SourceText) -> Option<JsString> {
        match self {
            Self::Cooked(bytes) => Some(JsString::from_bytes(bytes)),
            Self::Static(bytes) => Some(JsString::from_bytes(bytes)),
            Self::Borrowed(bytes) => {
                if bytes.is_empty() {
                    return source.slice(0..0);
                }
                // Addresses select a checked range in the retained owner; no
                // pointer is reconstructed or dereferenced from an integer.
                let start = bytes
                    .as_ptr()
                    .addr()
                    .checked_sub(source.as_bytes().as_ptr().addr())?;
                let end = start.checked_add(bytes.len())?;
                source.slice(start..end)
            }
        }
    }
}
impl From<Vec<u8>> for TokenValue<'_> {
    fn from(value: Vec<u8>) -> Self {
        if value.is_empty() {
            Self::Borrowed(b"")
        } else {
            Self::Cooked(value.into())
        }
    }
}
impl Default for TokenValue<'_> {
    fn default() -> Self {
        Self::Borrowed(b"")
    }
}
impl PartialEq for TokenValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}
impl Eq for TokenValue<'_> {}
impl std::hash::Hash for TokenValue<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(self.as_bytes(), state);
    }
}
impl std::borrow::Borrow<[u8]> for TokenValue<'_> {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScannerState<'src> {
    pub(crate) pos: i64,
    pub(crate) full_start_pos: i64,
    pub(crate) token_start: i64,
    pub(crate) token: SyntaxKind,
    pub(crate) token_value: TokenValue<'src>,
    pub(crate) token_flags: TokenFlags,
    pub(crate) comment_directives: Option<Arc<Vec<CommentDirective>>>,
    pub(crate) skip_jsdoc_leading_asterisks: i64,
}

/// A checkpoint must be committed or rewound, newest first, on its origin scanner.
/// It retains value storage across text replacement but does not retain the source
/// owner: borrowed source bytes must still outlive the scanner and checkpoint.
/// Dropping a checkpoint does not commit it. If unwinding abandons a checkpoint,
/// discard that scanner; Reset deliberately preserves outstanding checkpoints.
#[must_use = "commit or rewind this checkpoint before its enclosing checkpoint"]
pub struct Checkpoint<'src> {
    owner: Arc<CheckpointOwner>,
    id: u64,
    state: ScannerState<'src>,
}

impl std::fmt::Debug for Checkpoint<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Checkpoint")
            .field("id", &self.id)
            .field("position", &self.state.pos)
            .field("token", &self.state.token)
            .field("value_bytes", &self.state.token_value.as_bytes().len())
            .field(
                "directives",
                &self.state.comment_directives.as_deref().map_or(0, Vec::len),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct CheckpointOwner;

pub struct Scanner<'src> {
    pub(crate) text: &'src [u8],
    pub(crate) end: i64,
    pub(crate) state: ScannerState<'src>,
    pub(crate) language_variant: LanguageVariant,
    pub(crate) script_target: ScriptTarget,
    pub(crate) skip_trivia: bool,
    error_sink: ErrorSink<'src>,
    pub(crate) number_cache: HashMap<TokenValue<'src>, TokenValue<'src>>,
    pub(crate) hex_number_cache: HashMap<TokenValue<'src>, TokenValue<'src>>,
    pub(crate) hex_digit_cache: HashMap<TokenValue<'src>, TokenValue<'src>>,
    checkpoint_owner: Option<Arc<CheckpointOwner>>,
    next_checkpoint_id: u64,
    checkpoints: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdentifierVariant {
    Standard,
    Jsx,
    RegExpGroupName,
}

pub(crate) mod escape_flags {
    pub(crate) const STRING: i32 = 1 << 0;
    pub(crate) const REPORT_ERRORS: i32 = 1 << 1;
    pub(crate) const REGULAR_EXPRESSION: i32 = 1 << 2;
    pub(crate) const ANNEX_B: i32 = 1 << 3;
    pub(crate) const ANY_UNICODE_MODE: i32 = 1 << 4;
    pub(crate) const ATOM_ESCAPE: i32 = 1 << 5;
    pub(crate) const REPORT_INVALID_ESCAPE_ERRORS: i32 = REGULAR_EXPRESSION | REPORT_ERRORS;
    pub(crate) const ALLOW_EXTENDED_UNICODE_ESCAPE: i32 = STRING | ANY_UNICODE_MODE;
}

impl Default for ScannerState<'_> {
    fn default() -> Self {
        Self {
            pos: 0,
            full_start_pos: 0,
            token_start: 0,
            token: SyntaxKind::Unknown,
            token_value: TokenValue::default(),
            token_flags: flags::NONE,
            comment_directives: None,
            skip_jsdoc_leading_asterisks: 0,
        }
    }
}

/// port: tsc/internal/scanner/scanner.go:defaultScanner
fn default_scanner<'src>() -> Scanner<'src> {
    Scanner {
        text: b"",
        end: 0,
        state: ScannerState::default(),
        language_variant: LanguageVariant::STANDARD,
        script_target: ScriptTarget::NONE,
        skip_trivia: true,
        error_sink: ErrorSink::Ignore,
        number_cache: HashMap::new(),
        hex_number_cache: HashMap::new(),
        hex_digit_cache: HashMap::new(),
        checkpoint_owner: None,
        next_checkpoint_id: 0,
        checkpoints: Vec::new(),
    }
}

impl Default for Scanner<'_> {
    fn default() -> Self {
        default_scanner()
    }
}

/// port: tsc/internal/scanner/scanner.go:cleared
fn cleared<K, V>(mut map: HashMap<K, V>) -> HashMap<K, V> {
    map.clear();
    map
}

impl<'src> Scanner<'src> {
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasUnicodeEscape
    pub fn has_unicode_escape(&self) -> bool {
        self.state.token_flags & flags::UNICODE_ESCAPE != 0
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasExtendedUnicodeEscape
    pub fn has_extended_unicode_escape(&self) -> bool {
        self.state.token_flags & flags::EXTENDED_UNICODE_ESCAPE != 0
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasPrecedingLineBreak
    pub fn has_preceding_line_break(&self) -> bool {
        self.state.token_flags & flags::PRECEDING_LINE_BREAK != 0
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasPrecedingJSDocComment
    pub fn has_preceding_jsdoc_comment(&self) -> bool {
        self.state.token_flags & flags::PRECEDING_JSDOC_COMMENT != 0
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasPrecedingJSDocLeadingAsterisks
    pub fn has_preceding_jsdoc_leading_asterisks(&self) -> bool {
        self.state.token_flags & flags::PRECEDING_JSDOC_LEADING_ASTERISKS != 0
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasPrecedingJSDocWithDeprecatedTag
    pub fn has_preceding_jsdoc_with_deprecated_tag(&self) -> bool {
        self.state.token_flags & flags::PRECEDING_JSDOC_WITH_DEPRECATED != 0
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.HasPrecedingJSDocWithSeeOrLink
    pub fn has_preceding_jsdoc_with_see_or_link(&self) -> bool {
        self.state.token_flags & flags::PRECEDING_JSDOC_WITH_SEE_OR_LINK != 0
    }
    /// port: tsc/internal/scanner/scanner.go:NewScanner
    pub fn new() -> Self {
        default_scanner()
    }

    /// Reset configuration and state while keeping cache capacity and outstanding checkpoints.
    /// port: tsc/internal/scanner/scanner.go:Scanner.Reset
    pub fn reset(&mut self) {
        *self = Self {
            number_cache: cleared(std::mem::take(&mut self.number_cache)),
            hex_number_cache: cleared(std::mem::take(&mut self.hex_number_cache)),
            hex_digit_cache: cleared(std::mem::take(&mut self.hex_digit_cache)),
            checkpoint_owner: self.checkpoint_owner.take(),
            next_checkpoint_id: self.next_checkpoint_id,
            checkpoints: std::mem::take(&mut self.checkpoints),
            ..default_scanner()
        };
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.Text
    pub fn text(&self) -> &'src [u8] {
        self.text
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.Token
    pub fn token(&self) -> SyntaxKind {
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenFlags
    pub fn token_flags(&self) -> TokenFlags {
        self.state.token_flags
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenFullStart
    pub fn token_full_start(&self) -> i64 {
        self.state.full_start_pos
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenStart
    pub fn token_start(&self) -> i64 {
        self.state.token_start
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenEnd
    pub fn token_end(&self) -> i64 {
        self.state.pos
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenText
    pub fn token_text(&self) -> &'src [u8] {
        self.slice(self.state.token_start, self.state.pos)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenValue
    pub fn token_value(&self) -> &[u8] {
        self.state.token_value.as_bytes()
    }
    /// Explicitly preserve the value before advancing or replacing scanner text.
    pub fn retain_token_value(&self) -> TokenValue<'src> {
        self.state.token_value.clone()
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.TokenRange
    pub fn token_range(&self) -> TextRange {
        TextRange::new(self.state.token_start, self.state.pos)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.CommentDirectives
    pub fn comment_directives(&self) -> &[CommentDirective] {
        self.state
            .comment_directives
            .as_deref()
            .map_or(&[], Vec::as_slice)
    }

    /// Retain state for a same-scanner LIFO rewind or commit; configuration is excluded.
    /// port: tsc/internal/scanner/scanner.go:Scanner.Mark
    pub fn mark(&mut self) -> Checkpoint<'src> {
        let owner = Arc::clone(
            self.checkpoint_owner
                .get_or_insert_with(|| Arc::new(CheckpointOwner)),
        );
        self.next_checkpoint_id = self
            .next_checkpoint_id
            .checked_add(1)
            .expect("checkpoint identity exhausted");
        let id = self.next_checkpoint_id;
        self.checkpoints.push(id);
        Checkpoint {
            owner,
            id,
            state: self.state.clone(),
        }
    }

    fn consume_checkpoint(&mut self, checkpoint: &Checkpoint<'_>) {
        assert!(
            self.checkpoint_owner
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &checkpoint.owner)),
            "checkpoint belongs to another scanner"
        );
        assert_eq!(
            self.checkpoints.last(),
            Some(&checkpoint.id),
            "checkpoints must be consumed in LIFO order"
        );
        self.checkpoints.pop();
    }

    /// Restore saved state, keeping the current source, end and configuration.
    /// port: tsc/internal/scanner/scanner.go:Scanner.Rewind
    pub fn rewind(&mut self, checkpoint: Checkpoint<'src>) {
        self.consume_checkpoint(&checkpoint);
        self.state = checkpoint.state;
    }
    pub fn commit(&mut self, checkpoint: Checkpoint<'src>) {
        self.consume_checkpoint(&checkpoint);
        drop(checkpoint);
    }

    /// Positions beyond EOF are accepted; subsequent bounds-sensitive reads may panic.
    /// port: tsc/internal/scanner/scanner.go:Scanner.ResetPos
    pub fn reset_pos(&mut self, pos: i64) {
        assert!(pos >= 0, "Cannot reset token state to negative position");
        self.state.pos = pos;
        self.state.full_start_pos = pos;
        self.state.token_start = pos;
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ResetTokenState
    pub fn reset_token_state(&mut self, pos: i64) {
        self.reset_pos(pos);
        self.state.token = SyntaxKind::Unknown;
        self.state.token_value = TokenValue::default();
        self.state.token_flags = flags::NONE;
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.SetSkipJSDocLeadingAsterisks
    pub fn set_skip_jsdoc_leading_asterisks(&mut self, skip: bool) {
        self.state.skip_jsdoc_leading_asterisks = self
            .state
            .skip_jsdoc_leading_asterisks
            .wrapping_add(if skip { 1 } else { -1 });
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.SetSkipTrivia
    pub fn set_skip_trivia(&mut self, skip: bool) {
        self.skip_trivia = skip;
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.SetText
    pub fn set_text(&mut self, text: &'src [u8]) {
        self.text = text;
        self.end = text.len() as i64;
        self.state = ScannerState::default();
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.SetOnError
    pub fn set_on_error(&mut self, callback: Option<ErrorCallback<'src>>) {
        self.error_sink = callback.map_or(ErrorSink::Ignore, ErrorSink::Callback);
    }
    /// Select buffered delivery for parser integration. Replacing the sink drops
    /// any pending diagnostics; the parser drains after every scanner operation.
    /// A valid-input buffer reserves no allocation. Reset restores ignored errors.
    pub fn buffer_diagnostics(&mut self) {
        self.error_sink = ErrorSink::Buffered(Vec::new());
    }

    /// Drain in emission order. Diagnostics are not restored by a checkpoint.
    pub fn drain_diagnostics(&mut self) -> impl Iterator<Item = ScannerDiagnostic> + '_ {
        let drained = match &mut self.error_sink {
            ErrorSink::Buffered(errors) => Some(errors.drain(..)),
            ErrorSink::Ignore | ErrorSink::Callback(_) => None,
        };
        drained.into_iter().flatten()
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.SetLanguageVariant
    pub fn set_language_variant(&mut self, variant: LanguageVariant) {
        self.language_variant = variant;
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.SetScriptTarget
    pub fn set_script_target(&mut self, target: ScriptTarget) {
        self.script_target = target;
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.languageVersion
    pub(crate) fn language_version(&self) -> ScriptTarget {
        if self.script_target == ScriptTarget::NONE {
            ScriptTarget::LATEST
        } else {
            self.script_target
        }
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.error
    pub(crate) fn error(&mut self, message: &'static Message) {
        self.error_at(message, self.state.pos, 0, vec![]);
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.errorAt
    pub(crate) fn error_at(
        &mut self,
        message: &'static Message,
        start: i64,
        length: i64,
        args: Vec<DiagnosticArgument>,
    ) {
        let diagnostic = ScannerDiagnostic {
            message,
            start,
            length,
            args,
        };
        match &mut self.error_sink {
            ErrorSink::Callback(callback) => callback(diagnostic),
            ErrorSink::Buffered(errors) => errors.push(diagnostic),
            ErrorSink::Ignore => {}
        }
    }

    pub(crate) fn slice(&self, start: i64, end: i64) -> &'src [u8] {
        &self.text[usize::try_from(start).expect("slice bounds out of range")
            ..usize::try_from(end).expect("slice bounds out of range")]
    }
    pub(crate) fn tail(&self, start: i64) -> &'src [u8] {
        &self.text[usize::try_from(start).expect("slice bounds out of range")..]
    }
    /// Return one byte or -1, not a decoded scalar.
    /// port: tsc/internal/scanner/scanner.go:Scanner.char
    pub(crate) fn char(&self) -> i32 {
        if self.state.pos < self.end {
            i32::from(self.text[usize::try_from(self.state.pos).expect("index out of range")])
        } else {
            -1
        }
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.charAt
    pub(crate) fn char_at(&self, offset: i64) -> i32 {
        let pos = self.state.pos.wrapping_add(offset);
        if pos < self.end {
            i32::from(self.text[usize::try_from(pos).expect("index out of range")])
        } else {
            -1
        }
    }
    /// Deliberately decode the full source tail, including beyond a temporary regexp end.
    /// port: tsc/internal/scanner/scanner.go:Scanner.charAndSize
    pub(crate) fn char_and_size(&self) -> (i32, usize) {
        if self.state.pos < self.end {
            let byte = self.text[usize::try_from(self.state.pos).expect("index out of range")];
            if byte < 0x80 {
                return (i32::from(byte), 1);
            }
        }
        ts_jsstring::wtf8::decode_utf8(self.tail(self.state.pos))
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.scanASCIIWhile
    pub(crate) fn scan_ascii_while(&mut self, pred: impl Fn(u8) -> bool) {
        self.state.pos += self
            .slice(self.state.pos, self.end)
            .iter()
            .take_while(|&&byte| byte < 0x80 && pred(byte))
            .count() as i64;
    }
}
