//! The `diagnosticWriter` group: the 30 `internal/diagnosticwriter` operations
//! that `witness/s08-p5-errors-rust` does not already witness.
//!
//! Two of the eighteen cases run a production Rust entry point. The other
//! sixteen record a gap, and the gaps have three distinct shapes, which the
//! `production_home` of each record states rather than flattening into "absent":
//!
//! * ABSENT. `TryClearScreen`, the two watch-mode status renderers and the whole
//!   Go-only wrapper layer (`WrapASTDiagnostic`, `WrapASTDiagnostics`,
//!   `FromASTDiagnostics`, `ToDiagnostics`, `CompareASTDiagnostics`) have no
//!   counterpart anywhere in `crates/`. Rust passes `&[&Diagnostic]` straight
//!   into `DiagnosticWriter::format` (crates/tsr_compiler/src/diagnostic_writer/
//!   mod.rs:200), so there is nothing for a wrapper to be.
//!
//! * PRESENT BUT UNEXPORTED. `getCategoryFormat`, `diagnosticPrefix`,
//!   `writeWithStyleAndReset` and `prettyPathForFileError` each have a
//!   byte-for-byte counterpart in `tsr_compiler`, and every one of them is a
//!   private `fn` in a module that exports neither it nor a wrapper around it.
//!   Nothing outside the crate can reach them, so a driver cannot observe them,
//!   and saying so is not the same as saying the algorithm is missing.
//!
//! * PRESENT BUT ONLY BEHIND A PROGRAM. Everything that needs a
//!   `diagnostic_writer::File` -- the two FileLike wrappers, the tabular
//!   display, and every `ASTDiagnostic` method whose answer depends on
//!   `resolve` -- is reachable only through `DiagnosticWriter`, whose
//!   constructor takes a `&Program` (mod.rs:75). `File` has no public
//!   constructor: the only one is inside `DiagnosticWriter::file` (mod.rs:131),
//!   which resolves the diagnostic's `NodeId` against that program's arenas. So
//!   a diagnostic that did not come from a program cannot be rendered at all,
//!   and these operations cannot be addressed on their own.
//!
//! Preparation records the gap; it never emulates a missing algorithm to make a
//! comparison run, and it never reads a frozen observation.

use crate::api::{self, Outcome};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tsr_ast::Diagnostic;
use tsr_core::TextRange;
use tsr_jsstring::JsString;

const SUBJECT: &str = "diagnosticWriter";
const GO: &str = "tsc/internal/diagnosticwriter/diagnosticwriter.go:";

/// The two operations this group can answer by RUNNING production Rust.
///
/// Both are byte-flattening entry points, and both map onto the same Rust
/// function: `tsr_compiler::diagnostic_writer::flattened` (mod.rs:337), which
/// returns the assembled bytes instead of taking a writer. That collapses
/// Go's writer form and its string form into one, the same asymmetry the
/// existing witness already records for `WriteFormatDiagnostics` versus
/// `FormatDiagnosticsWithColorAndContext`. The nested-chain walk Go factors out
/// as `flattenDiagnosticMessageChain` is the iterative loop at mod.rs:339-357.
const FLATTEN: &str = "FlattenDiagnosticMessage";
const FLATTEN_AST: &str = "WriteFlattenedASTDiagnosticMessage";

/// Every gap this group reports, with the pinned authority, the signature the
/// port would need, and where the counterpart is or is not. Keyed by the bare
/// Go symbol; the request carries the full id and it is matched against this
/// table rather than trusted, so a request cannot make up an operation.
const GAPS: &[(&str, &str, &str, &str)] = &[
    (
        "ASTDiagnostic.resolve",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:96-110 -- four arms: no file, a \
         mapper's own diagnostic (Source non-empty) which keeps its range and renders against \
         the original text, a compiler diagnostic on a span-mapped file whose range is mapped \
         back, and one whose range falls in a synthesized gap and keeps the virtual range",
        "fn resolve(&self, d: &Diagnostic) -> ResolvedLocation { loc, use_original, synthesized \
         }, the shared step every position and file accessor goes through",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT as a step. There is no \
         resolve: DiagnosticWriter::file (mod.rs:107) handles only the `original` flag and \
         REFUSES a span-mapped file outright with Error::Unsupported(\"ASTDiagnostic.resolve \
         content-map span translation\") (mod.rs:117-118), and the renderers read d.loc.pos() \
         raw (mod.rs:236, pretty.rs:30). Nothing could observe it anyway: the whole path needs \
         DiagnosticWriter, whose constructor takes a &Program (mod.rs:75)",
    ),
    (
        "ASTDiagnostic.Pos",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:78 -- d.resolve().loc.Pos(), which is \
         the RESOLVED position, not the stored one",
        "fn pos(&self, d: &Diagnostic) -> i64 returning the resolved position",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT. Rust reads the stored \
         d.loc.pos() directly (mod.rs:236, pretty.rs:30) with no resolve step between, so there \
         is no entry point that takes a diagnostic and answers its reported position",
    ),
    (
        "ASTDiagnostic.End",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:79 -- d.resolve().loc.End()",
        "fn end(&self, d: &Diagnostic) -> i64 returning the resolved end",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT. Rust reads d.loc.end() \
         directly (pretty.rs:48) with no resolve step",
    ),
    (
        "ASTDiagnostic.Len",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:80 -- d.resolve().loc.Len(); the \
         pinned snippet renderer takes a LENGTH where Rust takes an end",
        "fn len(&self, d: &Diagnostic) -> i64 returning the resolved length",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT. Rust's snippet takes start \
         and end (pretty.rs:75-83) and never computes a resolved length",
    ),
    (
        "ASTDiagnostic.Source",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:74-76 -- the wrapper's own Source, \
         which is what marks a diagnostic as coming from a content mapper",
        "fn source(d: &Diagnostic) -> &[u8]",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT as an operation. There is no \
         wrapper type; callers read the public field d.source (used at mod.rs:268), so nothing \
         is addressable as this accessor",
    ),
    (
        "ASTDiagnostic.MessageChain",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:143-157 -- re-wraps the chain AND, \
         when resolve says the range is synthesized, appends a note built from \
         diagnostics.This_location_is_in_virtual_code_produced_by_the_content_mapper_0_... \
         naming the file's content mapper",
        "fn message_chain(&self, d: &Diagnostic) -> Vec<Cow<Diagnostic>>, appending the \
         synthesized-code note when the resolved location is synthesized",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT. flattened walks \
         d.message_chain directly (mod.rs:339-346) and never appends a note; the message \
         100030 exists in crates/tsr_diagnostics/src/generated.rs:4917 but nothing constructs \
         it. The arm also needs the span map Rust refuses at mod.rs:117-118",
    ),
    (
        "ASTDiagnostic.RelatedInformation",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:47-54 -- allocates a new \
         []Diagnostic of *ASTDiagnostic so each entry's own File and resolve run later",
        "fn related_information(&self, d: &Diagnostic) -> Vec<Cow<Diagnostic>>",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT. Rust iterates \
         d.related_information directly (pretty.rs:54) with no re-wrapping step, because there \
         is no wrapper type to re-wrap into",
    ),
    (
        "newOriginalTextFile",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:319-327 -- builds a FileLike over the \
         file's OriginalText with a line map computed from that text and a caller-supplied name",
        "fn original_text_file(source: &SourceFileRead, file_name: JsString) -> File",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- PRESENT BUT NOT ADDRESSABLE. The \
         equivalent File is built at mod.rs:131-138, taking text from source.original_text() \
         when the diagnostic is external; but File has no public constructor and that code is \
         inside DiagnosticWriter::file, which needs a &Program (mod.rs:75). A diagnostic that \
         did not come from a program cannot produce one",
    ),
    (
        "originalTextFile.FileName",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:327 -- returns the name the wrapper \
         was constructed with, which is the CANONICAL name, not the virtual one",
        "File::name(&self) -> &[u8] on a File built over the original text",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:41 -- PRESENT BUT NOT REACHABLE. \
         File::name is public, but File is only constructible inside DiagnosticWriter::file \
         (mod.rs:131), which needs a &Program (mod.rs:75)",
    ),
    (
        "originalTextFile.Text",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:328 -- returns the ORIGINAL, \
         untransformed text, not the virtual text the file was parsed from",
        "File::text(&self) -> &[u8] on a File built over the original text",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:44 -- PRESENT BUT NOT REACHABLE, for \
         the same reason as originalTextFile.FileName",
    ),
    (
        "originalTextFile.ECMALineMap",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:329 -- returns the line map computed \
         from the ORIGINAL text at construction (:322), not the source file's own",
        "the line starts a File over the original text carries, today the private field \
         File::lines (mod.rs:38), which no public accessor exposes",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:38 -- PRESENT BUT NEITHER EXPORTED NOR \
         REACHABLE: `lines` is private, only File::line_and_character (mod.rs:51) reads it, and \
         File itself needs a &Program to exist",
    ),
    (
        "renamedFile.FileName",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:337 -- the canonical name over the \
         virtual file, the arm taken when the range is synthesized so useOriginal is false",
        "File::name(&self) -> &[u8] on a File built over the VIRTUAL text with the canonical name",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:41 -- PRESENT BUT NOT REACHABLE. \
         mod.rs:121-129 does take the canonical name while keeping the virtual text, so the \
         pairing exists; File still has no public constructor and needs a &Program (mod.rs:75)",
    ),
    (
        "renamedFile.Text",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:338 -- delegates to the VIRTUAL \
         file's text, the opposite pairing to originalTextFile",
        "File::text(&self) -> &[u8] on a File built over the virtual text",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:44 -- PRESENT BUT NOT REACHABLE, for \
         the same reason as renamedFile.FileName",
    ),
    (
        "renamedFile.ECMALineMap",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:339 -- delegates to the virtual \
         file's own line map rather than computing one",
        "the line starts a File over the virtual text carries, today the private field \
         File::lines (mod.rs:38)",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:38 -- PRESENT BUT NEITHER EXPORTED NOR \
         REACHABLE, for the same reason as originalTextFile.ECMALineMap",
    ),
    (
        "diagnosticPrefix",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:375-380 -- returns Source when it is \
         non-empty and \"TS\" otherwise, keyed ONLY on Source",
        "pub fn diagnostic_prefix(d: &Diagnostic) -> &[u8]",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:267 -- PRESENT BUT UNEXPORTED. `fn \
         prefix` is the byte-for-byte counterpart and is private to the module; neither it nor \
         a wrapper is re-exported from crates/tsr_compiler/src/lib.rs, so no driver can call it",
    ),
    (
        "getCategoryFormat",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:382-394 -- error 91m, warning 93m, \
         suggestion 90m, message 94m, and a panic on anything else",
        "pub fn category_format(category: i32) -> Result<&'static [u8]>",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:258 -- PRESENT BUT UNEXPORTED. `fn \
         color` is the byte-for-byte counterpart, private to the module, and returns \
         Error::Unsupported where the pin panics. Its sibling `pub fn category` (mod.rs:249) IS \
         exported, so the asymmetry is in this one function and not in the module's style",
    ),
    (
        "writeWithStyleAndReset",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:398-403 -- writes the style, the \
         text and the reset UNCONDITIONALLY, so an empty style still emits a bare reset",
        "pub fn write_with_style_and_reset(out: &mut Vec<u8>, text: &[u8], style: &[u8])",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs:274 -- PRESENT BUT UNEXPORTED, and the \
         signature differs: `fn styled` is private and takes a `pretty` flag that suppresses \
         both escapes, a decision the pin makes in its callers by passing a different writer \
         (diagnosticwriter.go:396)",
    ),
    (
        "prettyPathForFileError",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:544-559 -- the file name, converted \
         to a relative path only when the name AND the current directory are both absolute, \
         followed by the grey line number of the FIRST error in the slice",
        "fn pretty_path_for_file_error(file: &File, errors: &[&Diagnostic], options: \
         &FormattingOptions) -> Vec<u8>",
        "crates/tsr_compiler/src/diagnostic_writer/pretty.rs:149 -- PRESENT BUT UNEXPORTED AND \
         BEHIND A PROGRAM. `fn pretty_path` is the counterpart, private, a method on \
         DiagnosticWriter, and it takes a &File that only DiagnosticWriter::file can build \
         (mod.rs:131), which needs a &Program (mod.rs:75)",
    ),
    (
        "writeTabularErrorsDisplay",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:514-542 -- the \"Errors  Files\" \
         header and one right-aligned count per file, padded to max(len(\"Errors\"), digits)",
        "fn write_tabular_errors_display(summary: &ErrorSummary, options: &FormattingOptions) -> \
         Vec<u8>, taking an already-grouped summary",
        "crates/tsr_compiler/src/diagnostic_writer/pretty.rs:224-242 -- PRESENT BUT INLINE. The \
         same rendering exists, but as a block inside error_summary rather than a function, so \
         it cannot be driven without also running the grouping and sorting that precede it; and \
         error_summary is a method on DiagnosticWriter, which needs a &Program (mod.rs:75)",
    ),
    (
        "FormatDiagnosticsStatusWithColorAndTime",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:581-586 -- \"[\", the caller's time \
         styled grey, \"] \", then the flattened message",
        "pub fn format_status_with_color_and_time(out: &mut Vec<u8>, time: &[u8], d: \
         &Diagnostic, options: &FormattingOptions) -> Result<()>",
        "crates/tsr_compiler -- ABSENT. Nothing in crates/ renders a watch-mode status line; a \
         search for the bracketed-time form finds no counterpart",
    ),
    (
        "FormatDiagnosticsStatusAndTime",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:588-591 -- the time, the literal \
         \" - \" produced by a two-operand fmt.Fprint, then the flattened message",
        "pub fn format_status_and_time(out: &mut Vec<u8>, time: &[u8], d: &Diagnostic, options: \
         &FormattingOptions) -> Result<()>",
        "crates/tsr_compiler -- ABSENT, for the same reason as \
         FormatDiagnosticsStatusWithColorAndTime",
    ),
    (
        "TryClearScreen",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:601-611 -- clears only when the \
         diagnostic's code is one of ScreenStartingCodes (:596-600) and none of \
         PreserveWatchOutput, ExtendedDiagnostics and Diagnostics IsTrue",
        "pub fn try_clear_screen(out: &mut Vec<u8>, d: &Diagnostic, options: &CompilerOptions) \
         -> bool",
        "crates/tsr_compiler -- ABSENT. No crate writes the \\x1b[2J\\x1b[3J\\x1b[H sequence and \
         none reads PreserveWatchOutput for this purpose; there is no watch mode to host it",
    ),
    (
        "WrapASTDiagnostic",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:158-160 -- embeds the *ast.Diagnostic \
         in an *ASTDiagnostic so the interface methods can override File, Pos, End, Len, Source, \
         MessageChain and RelatedInformation",
        "a wrapper type over &Diagnostic that overrides the display accessors",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT BY DESIGN. Rust has no \
         wrapper: DiagnosticWriter::format takes &[&Diagnostic] straight in (mod.rs:200), so \
         there is nothing to wrap and no operation to port. This is a Go-only layer",
    ),
    (
        "WrapASTDiagnostics",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:162-168 -- the slice form, allocating \
         one wrapper per element and preserving order and identity",
        "the slice form of the wrapper above",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT BY DESIGN, for the same \
         reason as WrapASTDiagnostic. witness/s08-p5-errors-rust already excluded it on exactly \
         this ground",
    ),
    (
        "FromASTDiagnostics",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:170-176 -- the same allocation as \
         WrapASTDiagnostics but returning []Diagnostic, the interface slice, rather than \
         []*ASTDiagnostic",
        "a conversion from concrete diagnostics to the display interface",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT BY DESIGN. There is no \
         display interface in the Rust port; &Diagnostic is the only form",
    ),
    (
        "ToDiagnostics",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:178-184 -- a generic widening of \
         []T (T Diagnostic) to []Diagnostic that creates no new wrapper",
        "a generic widening to the display interface",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT BY DESIGN. Nothing to widen \
         to; witness/s08-p5-errors-rust already excluded it on this ground",
    ),
    (
        "CompareASTDiagnostics",
        "tsc/internal/diagnosticwriter/diagnosticwriter.go:186-188 -- unwraps both operands and \
         delegates to ast.CompareDiagnostics (ast/diagnostic.go:482)",
        "a comparator over the display wrapper",
        "crates/tsr_compiler/src/diagnostic_writer/mod.rs -- ABSENT. DiagnosticWriter::sorted \
         (mod.rs:142) sorts with tsr_ast::compare_diagnostics \
         (crates/tsr_ast/src/diagnostic_order.rs:136), which is a port of the DIFFERENT \
         operation ast/diagnostic.go:CompareDiagnostics and takes a file-name resolver instead \
         of reading a wrapped file. The unwrapping shim has no counterpart because there is \
         nothing wrapped. witness/s08-p5-errors-rust already excluded it on this ground",
    ),
];

fn gap(requested: &str) -> Outcome {
    let bare = requested.strip_prefix(GO).unwrap_or(requested);
    match GAPS.iter().find(|(name, ..)| *name == bare) {
        Some((name, authority, signature, home)) => {
            Outcome::missing(format!("{GO}{name}"), authority, signature, home)
        }
        None => Outcome::Failed(format!(
            "no reviewed diagnosticWriter record for operation {requested:?}"
        )),
    }
}

// --------------------------------------------------------------- observed ---

fn field<'a>(spec: &'a Value, name: &str) -> Option<&'a Value> {
    spec.get(name)
}

fn text_field(spec: &Value, name: &str) -> JsString {
    JsString::from_bytes(
        field(spec, name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .as_bytes()
            .to_vec(),
    )
}

fn int_field(spec: &Value, name: &str) -> i64 {
    field(spec, name)
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("odd-length hex payload {text:?}"));
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).map_err(|_| format!("bad hex {text:?}"))?;
            u8::from_str_radix(digits, 16).map_err(|_| format!("bad hex {text:?}"))
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The request's arguments, as UTF-8 under `args` or as hex under `args_hex`.
/// Exactly one of the two may be present, as on the native side: an argument
/// that can carry invalid bytes must not be allowed to arrive two ways.
fn arguments(spec: &Value) -> Result<Vec<JsString>, String> {
    let plain = field(spec, "args").and_then(Value::as_array);
    let encoded = field(spec, "args_hex").and_then(Value::as_array);
    match (plain, encoded) {
        (Some(_), Some(_)) => Err("a diagnostic spec carries both args and args_hex".into()),
        (Some(items), None) => Ok(items
            .iter()
            .map(|item| JsString::from_bytes(item.as_str().unwrap_or_default().as_bytes().to_vec()))
            .collect()),
        (None, Some(items)) => items
            .iter()
            .map(|item| {
                Ok(JsString::from_bytes(decode_hex(
                    item.as_str().unwrap_or_default(),
                )?))
            })
            .collect(),
        (None, None) => Ok(Vec::new()),
    }
}

fn nested(spec: &Value, name: &str) -> Result<Vec<Arc<Diagnostic>>, String> {
    let Some(items) = field(spec, name).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|item| Ok(Arc::new(build(item)?)))
        .collect()
}

/// Build one diagnostic from the request's spec, through the same two pinned
/// shapes the native probe uses: already-localized external text, or a message
/// key resolved out of the generated table. Nothing here formats a message; it
/// only assembles the input `flattened` is then asked about.
fn build(spec: &Value) -> Result<Diagnostic, String> {
    if !field(spec, "file")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .is_empty()
    {
        return Err("a diagnostic on a file cannot be built without a Program".into());
    }
    let loc = TextRange::new(int_field(spec, "pos"), int_field(spec, "end"));
    let code =
        i32::try_from(int_field(spec, "code")).map_err(|_| "code out of range".to_owned())?;
    let category = i32::try_from(int_field(spec, "category"))
        .map_err(|_| "category out of range".to_owned())?;
    let source = text_field(spec, "source");
    let message_text = text_field(spec, "message_text");
    let message_key = text_field(spec, "message_key");
    let chain = nested(spec, "chain")?;
    let related = nested(spec, "related")?;

    let mut diagnostic = if message_text.as_bytes().is_empty() {
        if message_key.as_bytes().is_empty() {
            return Err(
                "a diagnostic spec needs exactly one of message_text and message_key".into(),
            );
        }
        // The counterpart of ast.NewDiagnosticFromSerialized (ast/diagnostic.go:166):
        // `message` stays None so Localize resolves through the key, which is
        // what makes the message table -- not a pointer the harness chose --
        // the thing under comparison.
        Diagnostic {
            file: None,
            loc,
            code,
            category,
            source,
            message: None,
            message_text: JsString::default(),
            message_key,
            message_args: arguments(spec)?,
            message_chain: Vec::new(),
            related_information: Vec::new(),
            reports_unnecessary: false,
            reports_deprecated: false,
            skipped_on_no_emit: false,
        }
    } else {
        // The counterpart of ast.NewExternalDiagnostic (ast/diagnostic.go:247).
        Diagnostic::external(None, loc, source, category, code, message_text)
    };
    diagnostic.message_chain = chain;
    diagnostic.related_information = related;
    Ok(diagnostic)
}

fn replay(request: &Value) -> Result<Value, String> {
    let mut built: BTreeMap<String, Diagnostic> = BTreeMap::new();
    let mut rows = Vec::new();
    for action in api::actions(request) {
        let op = api::action_op(action);
        let row = match op {
            "define_diagnostic" => {
                let target = api::action_str(action, "target").to_owned();
                let spec = action
                    .get("diagnostic")
                    .ok_or("define_diagnostic carries no diagnostic spec")?;
                let diagnostic = build(spec)?;
                let row = json!({
                    "op": op,
                    "code": diagnostic.code,
                    "category": diagnostic.category,
                    "has_file": diagnostic.file.is_some(),
                    "chain_len": diagnostic.message_chain.len(),
                    "related_len": diagnostic.related_information.len(),
                });
                built.insert(target, diagnostic);
                row
            }
            "flatten" | "write_flattened_ast" => {
                let target = api::action_str(action, "target");
                let diagnostic = built
                    .get(target)
                    .ok_or_else(|| format!("no diagnostic named {target:?}"))?;
                let new_line = api::action_str(action, "new_line").as_bytes();
                let bytes = tsr_compiler::diagnostic_writer::flattened(diagnostic, new_line)
                    .map_err(|error| format!("flattened failed: {error:?}"))?;
                json!({ "op": op, "text_hex": encode_hex(&bytes) })
            }
            other => return Err(format!("unsupported action: {other}")),
        };
        rows.push(row);
    }
    Ok(api::ordered(rows))
}

pub fn observe(request: &Value) -> Option<Outcome> {
    if api::subject(request) != SUBJECT {
        return None;
    }
    let requested = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bare = requested.strip_prefix(GO).unwrap_or(requested);
    if bare != FLATTEN && bare != FLATTEN_AST {
        return Some(gap(requested));
    }
    Some(match replay(request) {
        Ok(observation) => Outcome::Observed(observation),
        Err(error) => Outcome::Failed(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_operation_labels_still_report_the_pinned_identity() {
        let row = crate::api::response(
            &json!({"case":"identity-control", "operation":"ASTDiagnostic.Source"}),
            gap("ASTDiagnostic.Source"),
        );
        assert_eq!(
            row["missing_operation"]["operation"],
            format!("{GO}ASTDiagnostic.Source")
        );
    }
}
