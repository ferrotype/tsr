//! The reprint witness (docs/PHASE3-plan.md decision 7): the Rust printer over
//! every source file of the program that is not a default library, in program
//! order, once with comments and once with `RemoveComments`, as the native
//! oracle's `phase3Reprint` runs the pinned `printer.EmitSourceFile`.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::panic::{catch_unwind, AssertUnwindSafe};
use tsr_compiler::Program;
use tsr_core::NewLineKind;
use tsr_printer::{EmitContext, Printer, PrinterOptions};

/// Lowercase hex of `raw`.
pub fn hex(raw: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(raw.len() * 2);
    for byte in raw {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

pub fn sha256(raw: &[u8]) -> String {
    hex(&Sha256::digest(raw))
}

/// One `EmitSourceFile` with a fresh emit context. A printer error is the
/// port's refusal (the pin panics on the same input); a Rust panic is a
/// production failure with its location, as the Phase 2 sub-tests record it.
fn print(
    view: tsr_ast::AstView<'_>,
    source: tsr_ast::NodeId,
    remove_comments: bool,
    texts: bool,
    last_panic: &dyn Fn() -> Option<String>,
) -> Value {
    let printed = catch_unwind(AssertUnwindSafe(|| {
        let context = EmitContext::new();
        let mut printer = Printer::new(
            PrinterOptions {
                remove_comments,
                new_line: NewLineKind::LF,
                ..PrinterOptions::default()
            },
            &context,
        );
        printer.emit_source_file(view, source)
    }));
    match printed {
        Ok(Ok(text)) => {
            let mut value = json!({"state":"printed","sha256":sha256(&text),"bytes":text.len()});
            if texts {
                value["text_hex"] = json!(hex(&text));
            }
            value
        }
        Ok(Err(error)) => json!({"state":"refused","reason":error.to_string()}),
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"state":"failed","class":"panic","reason":reason,"location":last_panic()})
        }
    }
}

/// `{"state":"executed","files":[...]}` over the program's non-library files.
pub fn observe(program: &Program, texts: bool, last_panic: &dyn Fn() -> Option<String>) -> Value {
    let mut files = Vec::new();
    for file in program.files() {
        let bound = file.bound().view();
        let source = match bound.source_file() {
            Ok(source) => source,
            Err(error) => {
                return json!({"state":"failed","class":"compiler_error","reason":error.to_string()})
            }
        };
        let options = source.parse_options();
        if program.is_lib(options.path.as_bytes()) {
            continue;
        }
        let view = bound.ast();
        files.push(json!({
            "name_hex": hex(options.file_name.as_bytes()),
            "source_sha256": sha256(source.text().as_bytes()),
            "comments": print(view, file.source(), false, texts, last_panic),
            "no_comments": print(view, file.source(), true, texts, last_panic),
        }));
    }
    json!({"state":"executed","files":files})
}
