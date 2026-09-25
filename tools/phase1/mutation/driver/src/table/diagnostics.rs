//! Group `diagnostics`: AST diagnostics.
//! Go: `tools/phase1/tables/go/diagnostics_columns.go`; spec:
//! `data/phase1/tables/diagnostics.json`.
use super::helpers::{is_kind, node_map};
use super::{decode, text, Column};
use crate::protocol::{hex, unhex};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_arena::Error;
use tsr_ast::diagnostic_api::{DiagnosticsCollection, RepopulateDiagnosticInfo};
use tsr_ast::{AstView, Diagnostic, JsString, NodeId, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_locale::Locale;

pub const COLUMNS: &[&str] = &[
    "ast.NewDiagnosticFromText",
    "ast.NewDiagnosticFromSerialized",
    "ast.NewExternalDiagnostic",
    "ast.NewDiagnosticChain",
    "ast.Diagnostic.RepopulateInfo",
    "ast.DiagnosticsCollection.GetDiagnostics",
];

/// Go's `diagnosticCase`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    category: i32,
    #[serde(default)]
    text_hex: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    args_hex: Vec<String>,
    #[serde(default)]
    source_hex: String,
    #[serde(default)]
    unnecessary: bool,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    chain: bool,
    #[serde(default)]
    related: bool,
    #[serde(default)]
    pos: i64,
    #[serde(default)]
    end: i64,
}

/// Go's `diagnosticCases`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    cases: Vec<Case>,
    #[serde(default)]
    locales: Vec<String>,
}

fn bytes(text: &str) -> Result<Vec<u8>, String> {
    unhex(&json!(text))
}

fn js(text: &str) -> Result<JsString, String> {
    Ok(JsString::from_bytes(bytes(text)?.as_slice()))
}

fn ad_hoc(pos: i64, end: i64, code: i32, text: &[u8]) -> Arc<Diagnostic> {
    Arc::new(Diagnostic::from_text(
        None,
        TextRange::new(pos, end),
        code,
        3,
        text,
        Vec::new(),
        Vec::new(),
        false,
        false,
    ))
}

impl Case {
    /// Go's `parts`.
    fn parts(&self) -> (Vec<Arc<Diagnostic>>, Vec<Arc<Diagnostic>>) {
        let chain = if self.chain {
            vec![ad_hoc(1, 2, 7001, b"chained")]
        } else {
            Vec::new()
        };
        let related = if self.related {
            vec![ad_hoc(3, 4, 7002, b"related")]
        } else {
            Vec::new()
        };
        (chain, related)
    }

    fn args(&self) -> Result<Vec<JsString>, String> {
        self.args_hex.iter().map(|arg| js(arg)).collect()
    }

    fn range(&self) -> TextRange {
        TextRange::new(self.pos, self.end)
    }
}

/// Go's `diagnosticValue`.
fn diagnostic_value(d: &Diagnostic, locales: &[Locale]) -> Result<Value, String> {
    let localized = locales
        .iter()
        .map(|locale| Ok(json!(hex(&d.localize(None, locale).map_err(text)?))))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(json!([
        d.code,
        d.category,
        hex(d.message_key.as_bytes()),
        d.message_args
            .iter()
            .map(|arg| hex(arg.as_bytes()))
            .collect::<Vec<_>>(),
        hex(d.message_text.as_bytes()),
        d.reports_unnecessary,
        d.reports_deprecated,
        d.skipped_on_no_emit,
        d.message_chain.len(),
        d.related_information.len(),
        d.loc.pos(),
        d.loc.end(),
        hex(&d.string(None).map_err(text)?),
        localized,
    ]))
}

/// Go's `diagnosticValuesColumn`.
fn values_column(
    input: &Value,
    build: fn(&Case) -> Result<Diagnostic, String>,
) -> Result<Column, String> {
    let cases: Cases = decode(input)?;
    let locales = cases
        .locales
        .iter()
        .map(|name| match Locale::parse(name) {
            (locale, true) => Ok(locale),
            _ => Err(format!("bad locale {name:?}")),
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Box::new(move || {
        cases
            .cases
            .iter()
            .map(|case| diagnostic_value(&build(case)?, &locales))
            .collect::<Result<Vec<_>, String>>()
            .map(Value::Array)
    }))
}

fn cannot_find_name() -> &'static tsr_diagnostics::Message {
    tsr_diagnostics::by_key("Cannot_find_name_0_2304").expect("pinned message")
}

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.NewDiagnosticFromText" => values_column(input, |case| {
            let (chain, related) = case.parts();
            Ok(Diagnostic::from_text(
                None,
                case.range(),
                case.code,
                case.category,
                &bytes(&case.text_hex)?,
                chain,
                related,
                case.unnecessary,
                case.deprecated,
            ))
        }),
        "ast.NewDiagnosticFromSerialized" => values_column(input, |case| {
            let (chain, related) = case.parts();
            Ok(Diagnostic::from_serialized(
                None,
                case.range(),
                case.code,
                case.category,
                JsString::from_bytes(case.key.as_bytes()),
                case.args()?,
                chain,
                related,
                case.unnecessary,
                case.deprecated,
                case.skipped,
            ))
        }),
        "ast.NewExternalDiagnostic" => values_column(input, |case| {
            Ok(Diagnostic::external(
                None,
                case.range(),
                js(&case.source_hex)?,
                case.category,
                case.code,
                js(&case.text_hex)?,
            ))
        }),
        "ast.NewDiagnosticChain" => values_column(input, |case| {
            let chain = if case.chain {
                let (_, related) = case.parts();
                Some(Arc::new(Diagnostic::from_text(
                    None,
                    case.range(),
                    case.code,
                    case.category,
                    &bytes(&case.text_hex)?,
                    Vec::new(),
                    related,
                    false,
                    false,
                )))
            } else {
                None
            };
            Ok(Diagnostic::chain(chain, cannot_find_name(), case.args()?))
        }),
        "ast.Diagnostic.RepopulateInfo" => repopulate_info(input),
        "ast.DiagnosticsCollection.GetDiagnostics" => {
            node_map("source", input, source_file_kind, |_, view, node| {
                get_diagnostics(view, node)
            })
        }
        _ => return None,
    })
}

/// Go's `ast.Diagnostic.RepopulateInfo` column.
fn repopulate_info(input: &Value) -> Result<Column, String> {
    let cases: Cases = decode(input)?;
    Ok(Box::new(move || {
        let mut out = Vec::new();
        for case in &cases.cases {
            let mut d = Diagnostic::from_text(
                None,
                case.range(),
                case.code,
                case.category,
                &bytes(&case.text_hex)?,
                Vec::new(),
                Vec::new(),
                false,
                false,
            );
            let before = d.repopulate_info().is_none();
            d.set_repopulate_info(Some(Arc::new(RepopulateDiagnosticInfo {
                kind: case.code % 3,
                module_reference: js(&case.source_hex)?,
                mode: case.category,
                package_name: js(&case.text_hex)?,
            })));
            let after = d.repopulate_info().map_or(Value::Null, |info| {
                json!([
                    info.kind,
                    hex(info.module_reference.as_bytes()),
                    info.mode,
                    hex(info.package_name.as_bytes())
                ])
            });
            out.push(json!([before, after]));
        }
        Ok(Value::Array(out))
    }))
}

/// Go's `ast.DiagnosticsCollection.GetDiagnostics` column.
fn get_diagnostics(view: AstView<'_>, file: NodeId) -> Result<Value, String> {
    let message = cannot_find_name();
    let statements: Vec<NodeId> = view
        .node_slice(
            view.node(file)
                .map_err(text)?
                .statements(view)
                .map_err(text)?,
        )
        .map_err(text)?
        .iter()
        .flatten()
        .take(20)
        .collect();
    let path = view.source_file(file).map_err(text)?.path().to_vec();
    let mut collection = DiagnosticsCollection::default();
    for (index, statement) in statements.iter().enumerate().rev() {
        let range = view.node(*statement).map_err(text)?.range();
        let argument = JsString::from_bytes((index % 3).to_string().as_bytes());
        collection.add(
            Arc::new(Diagnostic::new(Some(file), range, message, vec![argument])),
            Some(&path),
        );
    }
    collection.add(
        Arc::new(Diagnostic::compiler(
            message,
            vec![JsString::from_bytes(b"g1".as_slice())],
        )),
        None,
    );
    collection.add(
        Arc::new(Diagnostic::compiler(
            message,
            vec![JsString::from_bytes(b"g0".as_slice())],
        )),
        None,
    );
    let file_name = |id: NodeId| -> Result<&[u8], Error> { Ok(view.source_file(id)?.file_name()) };
    let sorted = collection.get_diagnostics(&file_name).map_err(text)?;
    Ok(Value::Array(
        sorted
            .iter()
            .map(|d| {
                json!([
                    d.file.is_some(),
                    d.loc.pos(),
                    d.loc.end(),
                    d.code,
                    d.message_args
                        .iter()
                        .map(|arg| hex(arg.as_bytes()))
                        .collect::<Vec<_>>()
                ])
            })
            .collect(),
    ))
}

fn source_file_kind(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::SourceFile])
}
