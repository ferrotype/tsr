//! JSON-mode AST conversion from tsoptions/tsconfigparsing.go. Source parser
//! recovery and raw property positions remain visible to option diagnostics.
use crate::{
    diagnostic_for_node, find_declaration, property_name, ConfigValue, OptionDeclaration,
    OptionKind, TsConfigSourceFile, COMPILER_OPTIONS, ROOT_OPTIONS, TYPE_ACQUISITION_OPTIONS,
};
use std::sync::Arc;
use ts_ast::{Diagnostic, NodeDataRead, NodeId, SyntaxKind as K};
use ts_core::TextRange;
use ts_diagnostics::{self as d, Message};
use ts_jsstring::{JsString, SourceText};

#[derive(Debug)]
pub struct ConfigText {
    pub source: Arc<TsConfigSourceFile>,
    pub value: ConfigValue,
    pub diagnostics: Vec<Diagnostic>,
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:ParseConfigFileTextToJson
pub fn parse_config_file_text_to_json(
    file_name: JsString,
    path: JsString,
    text: SourceText,
) -> ConfigText {
    ts_parser::on_parser_worker(|| {
        let source = Arc::new(TsConfigSourceFile::parse(file_name, path, text));
        let (value, mut diagnostics) = convert_config_file_to_object(&source, None);
        if let Some(first) = source
            .file
            .view()
            .source_file(source.root)
            .expect("config source")
            .diagnostics
            .first()
        {
            diagnostics = vec![first.clone()];
        }
        ConfigText {
            source,
            value,
            diagnostics,
        }
    })
}

pub type PropertyNotifier<'a> = dyn FnMut(
        &JsString,
        &ConfigValue,
        NodeId,
        Option<&'static OptionDeclaration>,
        Option<&'static OptionDeclaration>,
    ) -> Vec<Diagnostic>
    + 'a;
static ROOT_DECLARATION: OptionDeclaration = OptionDeclaration {
    name: "undefined",
    short_name: "",
    kind: OptionKind::Object,
    is_file_path: false,
    is_tsconfig_only: false,
    is_command_line_only: false,
    enum_values: &[],
    deprecated_keys: &[],
    element: None,
    extra_validation: "",
    min_value: 0,
    allow_config_dir_template: false,
    preserve_falsy: false,
};
fn child_option(
    parent: Option<&OptionDeclaration>,
    name: &[u8],
) -> Option<&'static OptionDeclaration> {
    let choices = match parent?.name {
        "undefined" => ROOT_OPTIONS,
        "compilerOptions" => COMPILER_OPTIONS,
        "typeAcquisition" => TYPE_ACQUISITION_OPTIONS,
        _ => return None,
    };
    find_declaration(choices, name, false).filter(|option| option.name.as_bytes() == name)
}
fn raw_diagnostic(
    config: &TsConfigSourceFile,
    node: NodeId,
    message: &'static Message,
    args: Vec<JsString>,
) -> Diagnostic {
    let node = config.file.view().node(node).expect("JSON diagnostic node");
    Diagnostic::new(
        Some(config.root),
        TextRange::new(i64::from(node.pos()), i64::from(node.end())),
        message,
        args,
    )
}

/// port: tsc/internal/tsoptions/tsconfigparsing.go:convertConfigFileToObject
/// A notifier requests source option validation as each property is visited.
pub fn convert_config_file_to_object(
    config: &TsConfigSourceFile,
    mut notifier: Option<&mut PropertyNotifier<'_>>,
) -> (ConfigValue, Vec<Diagnostic>) {
    let view = config.file.view();
    let root = view.node(config.root).expect("JSON source root");
    let NodeDataRead::SourceFile(data) = root.data() else {
        panic!("config root must be a source file")
    };
    let expression = data.statements().and_then(|list| {
        let list = view.list(list).expect("JSON statements");
        let nodes = view.node_slice(list.nodes()).expect("JSON statement slice");
        nodes
            .first()
            .flatten()
            .and_then(|node| view.node(node).expect("JSON statement").expression())
    });
    let Some(mut expression) = expression else {
        return (ConfigValue::EmptyStruct, vec![]);
    };
    let root_options = notifier.as_ref().map(|_| &ROOT_DECLARATION);
    if view.node(expression).expect("JSON root expression").kind() != K::ObjectLiteralExpression {
        let source = view.source_file(config.root).expect("JSON source");
        let name = if ts_tspath::base_name(source.file_name()) == b"jsconfig.json" {
            b"jsconfig.json".as_slice()
        } else {
            b"tsconfig.json".as_slice()
        };
        let errors = vec![diagnostic_for_node(
            config,
            expression,
            d::The_root_value_of_a_0_file_must_be_an_object,
            vec![JsString::from_bytes(name)],
        )];
        let root = view.node(expression).expect("JSON root expression");
        if let NodeDataRead::ArrayLiteralExpression(data) = root.data() {
            if let Some(list) = data.elements() {
                let list = view.list(list).expect("recovered JSON elements");
                let nodes = view.node_slice(list.nodes()).expect("recovered JSON slice");
                let object = nodes.iter().flatten().find(|node| {
                    view.node(*node).expect("JSON element").kind() == K::ObjectLiteralExpression
                });
                if let Some(object) = object {
                    expression = object;
                    // Source recovery returns conversion diagnostics directly,
                    // discarding the already-created root-value diagnostic.
                    return convert_value(config, expression, root_options, &mut notifier);
                }
            }
        }
        return (ConfigValue::Object(Vec::new()), errors);
    }
    convert_value(config, expression, root_options, &mut notifier)
}

fn convert_value(
    config: &TsConfigSourceFile,
    node: NodeId,
    option: Option<&'static OptionDeclaration>,
    notifier: &mut Option<&mut PropertyNotifier<'_>>,
) -> (ConfigValue, Vec<Diagnostic>) {
    let view = config.file.view();
    let read = view.node(node).expect("JSON value node");
    match read.kind().known() {
        Some(K::TrueKeyword) => return (ConfigValue::Boolean(true), vec![]),
        Some(K::FalseKeyword) => return (ConfigValue::Boolean(false), vec![]),
        Some(K::NullKeyword) => return (ConfigValue::Null, vec![]),
        Some(K::StringLiteral) => {
            return (
                ConfigValue::String(view.node_text(node).expect("JSON string").into_js_string()),
                vec![],
            );
        }
        Some(K::NumericLiteral) => {
            return (
                ConfigValue::Number(
                    ts_jsnum::from_string(view.node_text(node).expect("JSON number").as_bytes())
                        .value(),
                ),
                vec![],
            );
        }
        Some(K::PrefixUnaryExpression) => {
            let NodeDataRead::PrefixUnaryExpression(data) = read.data() else {
                unreachable!("unary payload")
            };
            if data.operator() == K::MinusToken {
                let operand = data.operand().expect("unary operand");
                if view.node(operand).expect("unary JSON operand").kind() == K::NumericLiteral {
                    return (
                        ConfigValue::Number(
                            -ts_jsnum::from_string(
                                view.node_text(operand).expect("JSON number").as_bytes(),
                            )
                            .value(),
                        ),
                        vec![],
                    );
                }
            }
        }
        Some(K::ObjectLiteralExpression) => return convert_object(config, node, option, notifier),
        Some(K::ArrayLiteralExpression) => {
            let NodeDataRead::ArrayLiteralExpression(data) = read.data() else {
                unreachable!("array payload")
            };
            let mut values = None;
            let mut errors = Vec::new();
            if let Some(list) = data.elements() {
                let list = view.list(list).expect("JSON array list");
                let nodes = view.node_slice(list.nodes()).expect("JSON array slice");
                if nodes.is_empty() {
                    values = Some(Vec::new());
                }
                for element in nodes.iter().flatten() {
                    let (value, mut diagnostics) =
                        convert_value(config, element, option, &mut None);
                    errors.append(&mut diagnostics);
                    if !value.is_null() {
                        values.get_or_insert_with(Vec::new).push(value);
                    }
                }
            } else {
                values = Some(Vec::new());
            }
            return (ConfigValue::Array(values), errors);
        }
        _ => {}
    }
    let diagnostic = if let Some(option) = option {
        raw_diagnostic(
            config,
            node,
            d::Compiler_option_0_requires_a_value_of_type_1,
            vec![
                JsString::from_bytes(option.name.as_bytes()),
                JsString::from_bytes(option.value_type_name().into_bytes()),
            ],
        )
    } else {
        raw_diagnostic(config,node,d::Property_value_can_only_be_string_literal_numeric_literal_true_false_null_object_literal_or_array_literal,vec![])
    };
    (ConfigValue::Null, vec![diagnostic])
}
fn convert_object(
    config: &TsConfigSourceFile,
    node: NodeId,
    parent: Option<&'static OptionDeclaration>,
    notifier: &mut Option<&mut PropertyNotifier<'_>>,
) -> (ConfigValue, Vec<Diagnostic>) {
    let view = config.file.view();
    let read = view.node(node).expect("JSON object");
    let NodeDataRead::ObjectLiteralExpression(data) = read.data() else {
        unreachable!("object payload")
    };
    let mut result = ConfigValue::Object(Vec::new());
    let mut errors = Vec::new();
    if let Some(list) = data.properties() {
        let list = view.list(list).expect("JSON properties");
        let nodes = view.node_slice(list.nodes()).expect("JSON property slice");
        for property in nodes.iter().flatten() {
            let element = view.node(property).expect("JSON property");
            let NodeDataRead::PropertyAssignment(data) = element.data() else {
                errors.push(raw_diagnostic(
                    config,
                    property,
                    d::Property_assignment_expected,
                    vec![],
                ));
                continue;
            };
            if let Some(question) = element
                .question_token(view)
                .expect("property question token")
            {
                errors.push(raw_diagnostic(
                    config,
                    question,
                    d::The_0_modifier_can_only_be_used_in_TypeScript_files,
                    vec![JsString::from_bytes(b"?".as_slice())],
                ));
            }
            let name = data
                .name()
                .and_then(|name| property_name(config, name))
                .unwrap_or_default();
            let option = (!name.is_empty())
                .then(|| child_option(parent, name.as_bytes()))
                .flatten();
            let (value, mut diagnostics) = convert_value(
                config,
                data.initializer().expect("property initializer"),
                option,
                notifier,
            );
            errors.append(&mut diagnostics);
            if !name.is_empty() {
                result.set(name.clone(), value);
                if let Some(callback) = notifier.as_mut() {
                    errors.extend(callback(
                        &name,
                        result.get(name.as_bytes()).expect("assigned JSON property"),
                        property,
                        parent,
                        option,
                    ));
                }
            }
        }
    }
    (result, errors)
}
