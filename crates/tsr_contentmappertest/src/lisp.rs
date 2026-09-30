//! A mapper for one Lisp expression, whose operator is an alias.
use crate::{initialize_result, unexpected_method, MapperHandler};
use tsr_ast::span_map::{SpanMap, FEATURE_ALL, KIND_ALIAS, KIND_VERBATIM};
use tsr_ast::SpanSegment;
use tsr_contentmapper::{
    MappedOutput, TransformParams, TransformResultMessage, METHOD_INITIALIZE, METHOD_TRANSFORM,
};
use tsr_ipc::{Context, HandlerResult};
use tsr_json::RawValue;

pub struct LispHandler;

fn segment(virtual_range: (i32, i32), original_range: (i32, i32), kind: i32) -> SpanSegment {
    SpanSegment {
        virtual_start: virtual_range.0,
        virtual_end: virtual_range.1,
        original_start: original_range.0,
        original_end: original_range.1,
        kind,
        features: FEATURE_ALL,
    }
}

impl MapperHandler for LispHandler {
    /// port: tsc/internal/testutil/contentmappertest/lisp.go:lispHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(Box::new(initialize_result("lisp")))),
            METHOD_TRANSFORM => {
                let mut params_value = TransformParams::default();
                tsr_json::unmarshal(params, &mut params_value, tsr_json::Options::default())?;
                let content = &params_value.content;
                if content.strip_suffix('\n').unwrap_or(content) != r#"(+ 1 2 "oops")"# {
                    return Err(format!(
                        "contentmappertest: unsupported Lisp expression {}",
                        tsr_jsstring::go_quote(content.as_bytes())
                    )
                    .into());
                }
                let mappings = SpanMap::new(&[
                    segment((0, 3), (1, 2), KIND_ALIAS),
                    segment((4, 5), (3, 4), KIND_VERBATIM),
                    segment((7, 8), (5, 6), KIND_VERBATIM),
                    segment((10, 16), (7, 13), KIND_VERBATIM),
                ])
                .marshal()?;
                Ok(Some(Box::new(TransformResultMessage {
                    output: MappedOutput {
                        text: r#"add(1, 2, "oops");"#.into(),
                        extension: ".ts".into(),
                        mappings: Some(RawValue(mappings)),
                        diagnostic_directives: None,
                    },
                    diagnostics: Vec::new(),
                    supplemental: Vec::new(),
                })))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}
