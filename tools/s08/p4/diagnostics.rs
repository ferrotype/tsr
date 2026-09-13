//! Lossless diagnostic records for failure attribution, without baseline rendering.
use serde_json::{json, Value};
use ts_compiler::Program;
pub fn hex(raw: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(raw.len() * 2);
    for byte in raw {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 15)]));
    }
    result
}
fn payload(program: &Program, d: &ts_ast::Diagnostic) -> Value {
    let file = d.file.map(|id| {
        if let Some(config) = program
            .config()
            .config_file
            .as_ref()
            .filter(|c| c.root == id)
        {
            return hex(config
                .file
                .view()
                .source_file(config.root)
                .expect("config source")
                .parse_options()
                .file_name
                .as_bytes());
        }
        hex(program
            .files()
            .iter()
            .find(|f| f.source() == id)
            .expect("diagnostic source retained")
            .bound()
            .view()
            .source_file()
            .expect("published source")
            .parse_options()
            .file_name
            .as_bytes())
    });
    json!({"file_hex":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,
        "key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),"source_hex":hex(d.source.as_bytes()),
        "args_hex":d.message_args.iter().map(|a|hex(a.as_bytes())).collect::<Vec<_>>(),
        "chain":d.message_chain.iter().map(|d|payload(program,d)).collect::<Vec<_>>(),
        "related":d.related_information.iter().map(|d|payload(program,d)).collect::<Vec<_>>(),
        "unnecessary":d.reports_unnecessary,"deprecated":d.reports_deprecated,"skipped_on_no_emit":d.skipped_on_no_emit})
}
pub fn phase(program: &Program, values: &[ts_ast::Diagnostic]) -> Value {
    json!({"state":"executed","diagnostics":values.iter().map(|d|payload(program,d)).collect::<Vec<_>>()})
}
