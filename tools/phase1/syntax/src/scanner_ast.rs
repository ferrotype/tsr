//! Direct calls for AST-facing scanner utilities. The request supplies source
//! bytes and node ranges; production code computes every observed answer.

use serde_json::{json, Value};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use tsr_arena::Counters;
use tsr_ast::{
    node_flags, AstBuilder, EagerJsDocProvider, FactoryMethods, IdentifierData, NodeId,
    SourceFileParseOptions, SyntaxKind as K,
};
use tsr_core::TextRange;
use tsr_jsstring::{scanner_positions, JsString, SourceText};

use crate::api::{subject, Outcome};

fn bytes(value: &Value) -> Result<Vec<u8>, String> {
    let text = value.as_str().ok_or("scanner hex value must be text")?;
    if !text.len().is_multiple_of(2) {
        return Err("scanner hex value must have even length".into());
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hi = char::from(pair[0])
                .to_digit(16)
                .ok_or("invalid scanner hex")?;
            let lo = char::from(pair[1])
                .to_digit(16)
                .ok_or("invalid scanner hex")?;
            Ok((hi * 16 + lo) as u8)
        })
        .collect()
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    value[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("scanner request has no {key} array"))
}
fn number(value: &Value) -> Result<i64, String> {
    value
        .as_i64()
        .ok_or_else(|| "scanner position must be an integer".into())
}
fn make_node(f: &mut AstBuilder, file: NodeId, spec: &Value) -> Result<NodeId, String> {
    let parts = array(spec, "text_hex")?
        .iter()
        .map(|part| bytes(part).map(JsString::from_bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let text = f.text_slice(parts).map_err(|e| format!("{e:?}"))?;
    let node = match spec["kind"].as_str() {
        Some("JSDocText") => f.new_js_doc_text(text),
        Some("JSDocLink") => f.new_js_doc_link(None, text),
        Some("JSDocLinkCode") => f.new_js_doc_link_code(None, text),
        Some("JSDocLinkPlain") => f.new_js_doc_link_plain(None, text),
        Some("Identifier") => f.new_identifier(JsString::from_bytes(b"constructed".as_slice())),
        Some("JsxText") => f.new_jsx_text(JsString::from_bytes(b"constructed".as_slice()), false),
        Some("JSDocComment") => f.new_js_doc(None, None),
        Some("SemicolonToken") => f.new_token(K::SemicolonToken.into()),
        kind => return Err(format!("unknown scanner node kind {kind:?}")),
    };
    let mut node_mut = f.node_mut(node).map_err(|e| format!("{e:?}"))?;
    node_mut.set_range(TextRange::new(number(&spec["pos"])?, number(&spec["end"])?));
    node_mut.set_parent(Some(file));
    if spec["in_jsdoc"].as_bool() == Some(true) {
        node_mut.set_flags(node_flags::JS_DOC);
    }
    drop(node_mut);
    if let Some(doc) = spec.get("doc").filter(|v| !v.is_null()) {
        let range = doc
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or("doc needs two offsets")?;
        let root = f.new_js_doc(None, None);
        let mut doc_node = f.node_mut(root).map_err(|e| format!("{e:?}"))?;
        doc_node.set_range(TextRange::new(number(&range[0])?, number(&range[1])?));
        doc_node.set_parent(Some(node));
        drop(doc_node);
        let flags = f.view().node(node).map_err(|e| format!("{e:?}"))?.flags();
        f.node_mut(node)
            .map_err(|e| format!("{e:?}"))?
            .set_flags(flags | node_flags::HAS_JS_DOC);
        f.seed_source_jsdoc(file, node, vec![root])
            .map_err(|e| format!("{e:?}"))?;
    }
    Ok(node)
}
fn nil_element_comment_observation(
    call: impl FnOnce() -> Result<JsString, tsr_arena::Error>,
) -> Result<Value, String> {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(_) => Err("nil-element comment did not produce the native nil-pointer panic".into()),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied());
            if message == Some("runtime error: invalid memory address or nil pointer dereference") {
                Ok(json!(["panic", message.unwrap()]))
            } else {
                resume_unwind(payload)
            }
        }
    }
}
fn run(request: &Value) -> Result<Value, String> {
    let call = request["call"]
        .as_str()
        .ok_or("scanner request has no call")?;
    let expected = match call {
        "default_state" => "tsc/internal/scanner/scanner.go:defaultScanner",
        "comment" | "comment_nil_element" => {
            "tsc/internal/scanner/utilities.go:GetTextOfJSDocComment"
        }
        "lines" => "tsc/internal/scanner/scanner.go:GetECMALineStarts",
        "token_position" => "tsc/internal/scanner/scanner.go:GetTokenPosOfNode",
        "scan_token" => "tsc/internal/scanner/scanner.go:ScanTokenAtPosition",
        "keyword" => "tsc/internal/scanner/utilities.go:IdentifierToKeywordKind",
        _ => return Err(format!("unknown scanner AST call {call:?}")),
    };
    if request["operation"].as_str() != Some(expected) {
        return Err(format!("wrong scanner operation for {call}"));
    }
    if call == "comment_nil_element" && request["case"] != "syntax/scanner-ast/comment-nil-element"
    {
        return Err("nil-element panic observation requires its named request".into());
    }
    let text = bytes(&request["source_hex"])?;
    let mut f = AstBuilder::new(SourceText::default(), &Counters::new());
    let file = f.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/scanner.ts".as_slice()),
            ..Default::default()
        },
        SourceText::from_loaded_bytes(text.as_slice()),
        None,
        None,
    );
    let mut out = Vec::new();
    match call {
        "default_state" => {
            let mut scanner = tsr_scanner::Scanner::new();
            let snapshot = |s: &tsr_scanner::Scanner<'_>| {
                json!([
                    s.token() as u16,
                    s.token_full_start(),
                    s.token_start(),
                    s.token_end(),
                    s.token_flags(),
                    crate::hex(s.token_text())
                ])
            };
            out.push(snapshot(&scanner));
            scanner.set_text(&text);
            scanner.scan();
            out.push(snapshot(&scanner));
            scanner.set_skip_trivia(false);
            scanner.reset();
            out.push(snapshot(&scanner));
            scanner.set_text(&text);
            scanner.scan();
            out.push(snapshot(&scanner));
        }
        "lines" => {
            let source = f.view().source_file(file).map_err(|e| format!("{e:?}"))?;
            let first = source.ecma_line_map();
            let second = source.ecma_line_map();
            out.push(json!(first));
            out.push(json!(std::ptr::eq(first, second)));
            for pos in array(request, "positions")? {
                let pos = isize::try_from(number(pos)?).map_err(|e| e.to_string())?;
                let (line, offset) =
                    scanner_positions::get_ecma_line_and_byte_offset_of_position(&text, pos);
                out.push(json!([
                    scanner_positions::get_ecma_line_of_position(&text, pos),
                    line,
                    offset
                ]));
            }
            for at in array(request, "coordinates")? {
                let at = at
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .ok_or("coordinate needs two numbers")?;
                let line = isize::try_from(number(&at[0])?).map_err(|e| e.to_string())?;
                let offset = isize::try_from(number(&at[1])?).map_err(|e| e.to_string())?;
                out.push(json!([
                    scanner_positions::get_ecma_position_of_line_and_byte_offset(
                        &text, line, offset
                    ),
                    scanner_positions::get_ecma_position_of_line_and_utf16_character(
                        &text, line, offset
                    )
                ]));
            }
        }
        "scan_token" => {
            for pos in array(request, "positions")? {
                out.push(json!(
                    tsr_scanner::scan_token_at_position(f.view(), file, number(pos)?)
                        .map_err(|e| format!("{e:?}"))? as u16
                ));
            }
        }
        "keyword" => {
            for name in array(request, "names_hex")? {
                out.push(json!(
                    tsr_scanner::identifier_to_keyword_kind(&IdentifierData {
                        text: JsString::from_bytes(bytes(name)?)
                    }) as u16
                ));
            }
        }
        "comment" | "comment_nil_element" | "token_position" => {
            let mut provider = EagerJsDocProvider::default();
            let list = if request["nodes"].is_null() {
                None
            } else {
                let mut nodes = Vec::new();
                for spec in array(request, "nodes")? {
                    if spec.is_null() && call != "token_position" {
                        nodes.push(None);
                        continue;
                    }
                    let node = make_node(&mut f, file, spec)?;
                    if call == "token_position" {
                        out.push(json!(tsr_scanner::get_token_pos_of_node(
                            f.view(),
                            file,
                            node,
                            spec["include_jsdoc"].as_bool().unwrap_or(false),
                            &mut provider
                        )
                        .map_err(|e| format!("{e:?}"))?));
                    }
                    nodes.push(Some(node));
                }
                let nodes = f.node_slice(nodes).map_err(|e| format!("{e:?}"))?;
                Some(
                    f.new_list(TextRange::new(-1, -1), nodes)
                        .map_err(|e| format!("{e:?}"))?,
                )
            };
            if call == "comment" {
                out.push(json!(crate::hex(
                    tsr_scanner::get_text_of_jsdoc_comment(f.view(), list)
                        .map_err(|e| format!("{e:?}"))?
                        .as_bytes()
                )));
            } else if call == "comment_nil_element" {
                out.push(nil_element_comment_observation(|| {
                    tsr_scanner::get_text_of_jsdoc_comment(f.view(), list)
                })?);
            }
        }
        _ => unreachable!("call checked"),
    }
    Ok(json!({"ordered":out}))
}
pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == "scannerAst").then(|| match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment_request(call: &str, nodes: &Value) -> Value {
        json!({"case":"syntax/scanner-ast/comment-nil-element",
            "operation":"tsc/internal/scanner/utilities.go:GetTextOfJSDocComment",
            "call":call, "source_hex":"", "nodes":nodes})
    }

    #[test]
    fn nil_element_observation_requires_the_actual_native_panic() {
        let valid = json!([{"kind":"JSDocText", "pos":0, "end":0,
            "text_hex":["626f64790b0c"]}]);
        assert_eq!(
            run(&comment_request("comment", &valid)).unwrap(),
            json!({"ordered":["626f6479"]})
        );
        assert!(run(&comment_request("comment_nil_element", &valid)).is_err());
        assert_eq!(
            run(&comment_request("comment_nil_element", &json!([null]))).unwrap(),
            json!({"ordered":[["panic","runtime error: invalid memory address or nil pointer dereference"]]})
        );
        assert!(catch_unwind(|| run(&comment_request("comment", &json!([null])))).is_err());
        assert!(
            catch_unwind(|| nil_element_comment_observation(|| panic!("unexpected panic")))
                .is_err()
        );
        let mut wrong = comment_request("comment_nil_element", &json!([null]));
        wrong["case"] = json!("another-request");
        assert!(run(&wrong).is_err());
    }
}
