//! The transforming mapper (`compiler-test-mapper`): `#{name}` interpolations
//! become the named compiler option's JSON, and `// @box-*` lines become
//! diagnostic directives.
use crate::{initialize_result, unexpected_method, MapperHandler, ProjectLifecycleHandler};
use std::collections::HashMap;
use std::sync::Mutex;
use tsr_ast::span_map::{SpanMap, FEATURE_ALL, KIND_ATOM, KIND_VERBATIM};
use tsr_ast::SpanSegment;
use tsr_contentmapper::{
    CloseProjectParams, Diagnostic, DiagnosticDirectives, MappedDiagnosticDirective, MappedOutput,
    OpenProjectParams, TransformParams, TransformResultMessage, UnusedExpectDirectiveDiagnostic,
    DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT, DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE, METHOD_INITIALIZE,
    METHOD_TRANSFORM,
};
use tsr_core::collections::OrderedMap;
use tsr_core::TextRange;
use tsr_ipc::{Context, HandlerError, HandlerResult};
use tsr_json::RawValue;

const PREAMBLE: &str = "const __VERSION = \"1.0.0\";\n";

/// The compiler options the mapper's manifest declares.
pub const DECLARED_OPTIONS: &[&str] = &["target", "jsx"];

const DIAGNOSTIC_SOURCE: &str = "box";
const UNCLOSED_INTERPOLATION_CODE: i32 = 1000;

type Options = OrderedMap<String, RawValue>;

/// The transforming mapper, keeping each open project's compiler options.
#[derive(Default)]
pub struct Handler {
    compiler_options: Mutex<HashMap<String, Option<Options>>>,
}

impl ProjectLifecycleHandler for Handler {
    /// port: tsc/internal/testutil/contentmappertest/transforming.go:Handler.OpenProject
    fn open_project(&self, params: &OpenProjectParams) -> Result<(), HandlerError> {
        let mut options: Option<Options> = None;
        tsr_json::unmarshal(
            &params.compiler_options.0,
            &mut options,
            tsr_json::Options::default(),
        )?;
        self.compiler_options
            .lock()
            .expect("mapper projects")
            .insert(params.project_handle.clone(), options);
        Ok(())
    }

    /// port: tsc/internal/testutil/contentmappertest/transforming.go:Handler.CloseProject
    fn close_project(&self, params: &CloseProjectParams) {
        self.compiler_options
            .lock()
            .expect("mapper projects")
            .remove(&params.project_handle);
    }
}

impl MapperHandler for Handler {
    /// port: tsc/internal/testutil/contentmappertest/transforming.go:Handler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result(
                DIAGNOSTIC_SOURCE,
            )))),
            METHOD_TRANSFORM => {
                let mut params_value = TransformParams::default();
                tsr_json::unmarshal(params, &mut params_value, tsr_json::Options::default())?;
                let options = self
                    .compiler_options
                    .lock()
                    .expect("mapper projects")
                    .get(&params_value.project_handle)
                    .cloned()
                    .flatten();
                let Some(options) = options else {
                    return Err(format!(
                        "contentmappertest: project {} is not open",
                        tsr_jsstring::go_quote(params_value.project_handle.as_bytes())
                    )
                    .into());
                };
                let (text, mappings, diagnostics, diagnostic_directives) =
                    transform(&params_value.content, &options)?;
                Ok(Some(tsr_ipc::Response::json(TransformResultMessage {
                    output: MappedOutput {
                        text,
                        extension: mapped_extension(&params_value.content),
                        mappings: Some(mappings),
                        diagnostic_directives,
                    },
                    diagnostics,
                    supplemental: Vec::new(),
                })))
            }
            _ => Err(unexpected_method(method)),
        }
    }

    fn lifecycle(&self) -> Option<&dyn ProjectLifecycleHandler> {
        Some(self)
    }
}

/// The extension a first line `// @box-extension: .x` selects, `.ts` otherwise.
/// port: tsc/internal/testutil/contentmappertest/transforming.go:mappedExtension
fn mapped_extension(content: &str) -> String {
    const PREFIX: &str = "// @box-extension:";
    if let Some((first_line, _)) = content.split_once('\n') {
        if let Some(extension) = first_line.strip_prefix(PREFIX) {
            return extension.trim().to_owned();
        }
    }
    ".ts".to_owned()
}

fn position(value: usize) -> Result<i32, HandlerError> {
    Ok(i32::try_from(value)?)
}

type Transformed = (
    String,
    RawValue,
    Vec<Diagnostic>,
    Option<DiagnosticDirectives>,
);

/// The virtual text, its span map, the unclosed-interpolation errors and the
/// directives of one input.
/// port: tsc/internal/testutil/contentmappertest/transforming.go:transform
fn transform(content: &str, options: &Options) -> Result<Transformed, HandlerError> {
    let mut virtual_text = String::from(PREAMBLE);
    let mut segments: Vec<SpanSegment> = Vec::new();
    let mut diagnostics = Vec::new();
    let write_verbatim = |virtual_text: &mut String,
                          segments: &mut Vec<SpanSegment>,
                          from: usize,
                          to: usize|
     -> Result<(), HandlerError> {
        if to <= from {
            return Ok(());
        }
        let virtual_start = position(virtual_text.len())?;
        virtual_text.push_str(&content[from..to]);
        segments.push(SpanSegment {
            virtual_start,
            virtual_end: position(virtual_text.len())?,
            original_start: position(from)?,
            original_end: position(to)?,
            kind: KIND_VERBATIM,
            features: FEATURE_ALL,
        });
        Ok(())
    };
    let write_atom = |virtual_text: &mut String,
                      segments: &mut Vec<SpanSegment>,
                      value: &str,
                      from: usize,
                      to: usize|
     -> Result<(), HandlerError> {
        let virtual_start = position(virtual_text.len())?;
        virtual_text.push_str(value);
        segments.push(SpanSegment {
            virtual_start,
            virtual_end: position(virtual_text.len())?,
            original_start: position(from)?,
            original_end: position(to)?,
            kind: KIND_ATOM,
            features: FEATURE_ALL,
        });
        Ok(())
    };
    let mut pos = 0;
    while pos < content.len() {
        let Some(relative) = content[pos..].find("#{") else {
            write_verbatim(&mut virtual_text, &mut segments, pos, content.len())?;
            break;
        };
        let token_start = pos + relative;
        let line_end = content[token_start..]
            .find('\n')
            .map_or(content.len(), |offset| token_start + offset);
        let Some(close) = content[token_start..line_end].find('}') else {
            write_verbatim(&mut virtual_text, &mut segments, pos, token_start)?;
            write_atom(
                &mut virtual_text,
                &mut segments,
                "undefined",
                token_start,
                line_end,
            )?;
            diagnostics.push(Diagnostic {
                message_text: "Unclosed interpolation.".into(),
                start: i64::try_from(token_start)?,
                length: i64::try_from(line_end - token_start)?,
                code: UNCLOSED_INTERPOLATION_CODE,
            });
            pos = line_end;
            continue;
        };
        let token_end = token_start + close + 1;
        let name = &content[token_start + 2..token_end - 1];
        write_verbatim(&mut virtual_text, &mut segments, pos, token_start)?;
        write_atom(
            &mut virtual_text,
            &mut segments,
            &render_option(options, name),
            token_start,
            token_end,
        )?;
        pos = token_end;
    }
    let span_map = SpanMap::new(&segments);
    let mappings = RawValue(span_map.marshal()?);
    let directives = diagnostic_directives(content, &span_map)?;
    Ok((virtual_text, mappings, diagnostics, directives))
}

fn wrap(
    directives: Vec<MappedDiagnosticDirective>,
    unused: Vec<UnusedExpectDirectiveDiagnostic>,
) -> DiagnosticDirectives {
    DiagnosticDirectives {
        unused_expect_directive_diagnostics: unused,
        directives,
    }
}

fn directive(policy: u8) -> MappedDiagnosticDirective {
    MappedDiagnosticDirective {
        policy,
        ..MappedDiagnosticDirective::default()
    }
}

/// The directives of an input: a scripted invalid shape, or one per
/// `// @box-ignore` and `// @box-expect-error:` line whose next line maps to
/// one virtual span.
/// port: tsc/internal/testutil/contentmappertest/transforming.go:diagnosticDirectives
fn diagnostic_directives(
    content: &str,
    mappings: &SpanMap,
) -> Result<Option<DiagnosticDirectives>, HandlerError> {
    const INVALID_PREFIX: &str = "// @box-invalid-directive:";
    if let Some(rest) = content.strip_prefix(INVALID_PREFIX) {
        let first_line = rest.split('\n').next().unwrap_or_default();
        let unused = UnusedExpectDirectiveDiagnostic::default;
        let scripted = match first_line.trim() {
            "invalid-range" => Some(wrap(
                vec![MappedDiagnosticDirective {
                    virtual_start: -1,
                    ..directive(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE)
                }],
                Vec::new(),
            )),
            "original-range-out-of-bounds" => Some(wrap(
                vec![MappedDiagnosticDirective {
                    original_start: i64::try_from(content.len())? + 1,
                    ..directive(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE)
                }],
                Vec::new(),
            )),
            "virtual-range-out-of-bounds" => Some(wrap(
                vec![MappedDiagnosticDirective {
                    virtual_start: 1 << 20,
                    virtual_end: 1 << 20,
                    ..directive(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE)
                }],
                Vec::new(),
            )),
            "invalid-policy" => Some(wrap(vec![directive(2)], Vec::new())),
            "ignore-with-unused-diagnostic" => Some(wrap(
                vec![directive(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE)],
                vec![unused()],
            )),
            "expect-without-unused-diagnostic" => Some(wrap(
                vec![directive(DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT)],
                Vec::new(),
            )),
            "invalid-unused-diagnostic-index" => Some(wrap(
                vec![MappedDiagnosticDirective {
                    unused_expect_directive_index: Some(1),
                    ..directive(DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT)
                }],
                vec![unused()],
            )),
            "overlap" => Some(wrap(
                vec![
                    MappedDiagnosticDirective {
                        virtual_end: 2,
                        ..directive(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE)
                    },
                    MappedDiagnosticDirective {
                        virtual_start: 1,
                        virtual_end: 3,
                        ..directive(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE)
                    },
                ],
                Vec::new(),
            )),
            _ => None,
        };
        if let Some(scripted) = scripted {
            return Ok(Some(scripted));
        }
    }
    const IGNORE_PREFIX: &str = "// @box-ignore";
    const EXPECT_PREFIX: &str = "// @box-expect-error";
    let mut result: Vec<MappedDiagnosticDirective> = Vec::new();
    let mut unused_diagnostics: Vec<UnusedExpectDirectiveDiagnostic> = Vec::new();
    let mut line_start = 0;
    while line_start < content.len() {
        let line_end = content[line_start..]
            .find('\n')
            .map_or(content.len(), |offset| line_start + offset);
        let trimmed = content[line_start..line_end].trim();
        let mut policy = None;
        let mut unused_index = None;
        if trimmed == IGNORE_PREFIX {
            policy = Some(DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE);
        } else if let Some(message) = trimmed.strip_prefix(&format!("{EXPECT_PREFIX}:")) {
            policy = Some(DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT);
            unused_index = Some(i64::try_from(unused_diagnostics.len())?);
            unused_diagnostics.push(UnusedExpectDirectiveDiagnostic {
                code: 2578,
                message_text: message.trim().to_owned(),
            });
        }
        if let Some(policy) = policy.filter(|_| line_end < content.len()) {
            let affected_start = line_end + 1;
            let affected_length = content[affected_start..]
                .find('\n')
                .unwrap_or(content.len() - affected_start);
            let virtual_spans = mappings.original_to_virtual_spans(
                TextRange::new(
                    i64::try_from(affected_start)?,
                    i64::try_from(affected_start + affected_length)?,
                ),
                FEATURE_ALL,
            );
            if let [span] = virtual_spans.as_slice() {
                result.push(MappedDiagnosticDirective {
                    original_start: i64::try_from(line_start)?,
                    original_length: i64::try_from(line_end - line_start)?,
                    virtual_start: span.span.pos(),
                    virtual_end: span.span.end(),
                    policy,
                    unused_expect_directive_index: unused_index,
                });
            }
        }
        if line_end == content.len() {
            break;
        }
        line_start = line_end + 1;
    }
    if unused_diagnostics.len() == 1 {
        for directive in &mut result {
            directive.unused_expect_directive_index = None;
        }
    }
    if result.is_empty() && unused_diagnostics.is_empty() {
        return Ok(None);
    }
    Ok(Some(wrap(result, unused_diagnostics)))
}

/// The option's JSON, or `undefined` when the project does not set it.
/// port: tsc/internal/testutil/contentmappertest/transforming.go:renderOption
fn render_option(options: &Options, name: &str) -> String {
    options
        .get(name)
        .filter(|value| !value.0.is_empty())
        .map_or_else(
            || "undefined".to_owned(),
            |value| String::from_utf8_lossy(&value.0).into_owned(),
        )
}
