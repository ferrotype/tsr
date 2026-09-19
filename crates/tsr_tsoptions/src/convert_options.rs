//! Config option validation and conversion from tsconfigparsing.go:341–503.
use crate::{
    diagnostic_for_node, option_declaration, parse_compiler_options, ConfigValue, EnumValue,
    OptionDeclaration, OptionKind, TsConfigSourceFile, COMPILER_OPTIONS,
};
use std::borrow::Cow;
use ts_ast::{Diagnostic, NodeDataRead, NodeId};
use ts_core::{CompilerOptions, Tristate};
use ts_diagnostics::{self as d, Message};
use ts_jsstring::JsString;

#[derive(Clone, Copy, Default)]
pub struct OptionSyntax<'a> {
    pub config: Option<&'a TsConfigSourceFile>,
    pub property: Option<NodeId>,
    pub value: Option<NodeId>,
}
fn diagnostic(
    syntax: OptionSyntax<'_>,
    node: Option<NodeId>,
    message: &'static Message,
    args: Vec<JsString>,
) -> Diagnostic {
    if let (Some(config), Some(node)) = (syntax.config, node) {
        diagnostic_for_node(config, node, message, args)
    } else {
        Diagnostic::compiler(message, args)
    }
}
fn text(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}

/// port: tsc/internal/tsoptions/tsconfigparsing.go:isCompilerOptionsValue
pub fn is_option_value(option: &OptionDeclaration, value: &ConfigValue) -> bool {
    if value.is_null() {
        return !option.disallow_null();
    }
    match (option.kind, value) {
        (OptionKind::List | OptionKind::ListOrElement, ConfigValue::Array(_))
        | (OptionKind::String | OptionKind::Enum, ConfigValue::String(_))
        | (OptionKind::Boolean, ConfigValue::Boolean(_))
        | (OptionKind::Number, ConfigValue::Number(_))
        | (OptionKind::Object, ConfigValue::Object(_)) => true,
        (OptionKind::ListOrElement, value) => {
            is_option_value(option.element.expect("list element"), value)
        }
        _ => false,
    }
}

/// Retain source order and borrow unchanged scalar/object values. The caller
/// retains config syntax while using diagnostics carrying its non-owning IDs.
pub fn convert_json_option<'a>(
    option: &OptionDeclaration,
    value: &'a ConfigValue,
    base_path: &[u8],
    syntax: OptionSyntax<'_>,
) -> (Cow<'a, ConfigValue>, Vec<Diagnostic>) {
    if option.is_command_line_only {
        let name = syntax
            .config
            .zip(syntax.property)
            .and_then(|(config, property)| {
                config
                    .file
                    .view()
                    .node(property)
                    .expect("config property")
                    .name()
            });
        return (
            Cow::Owned(ConfigValue::Null),
            vec![diagnostic(
                syntax,
                name,
                d::Option_0_can_only_be_specified_on_command_line,
                vec![text(option.name)],
            )],
        );
    }
    if !is_option_value(option, value) {
        return (
            Cow::Owned(ConfigValue::Null),
            vec![diagnostic(
                syntax,
                syntax.value,
                d::Compiler_option_0_requires_a_value_of_type_1,
                vec![text(option.name), text(&option.value_type_name())],
            )],
        );
    }
    match option.kind {
        OptionKind::List => return convert_list(option, value, base_path, syntax),
        OptionKind::ListOrElement => {
            if matches!(value, ConfigValue::Array(_)) {
                return convert_list(option, value, base_path, syntax);
            }
            // `extends` disallows null before this branch. Source listOrElement
            // option declarations have a concrete element for scalar values.
            return convert_json_option(
                option.element.expect("list element"),
                value,
                base_path,
                syntax,
            );
        }
        OptionKind::Enum => {
            let Some(value) = value.as_string().filter(|v| !v.is_empty()) else {
                return (Cow::Owned(ConfigValue::Null), vec![]);
            };
            let lower = ts_jsstring::helpers::to_lower_go(value.as_bytes());
            if let Some((_, value)) = option
                .enum_values
                .iter()
                .find(|(key, _)| key.as_bytes() == lower)
            {
                return (
                    Cow::Owned(match value {
                        EnumValue::String(value) => ConfigValue::String(text(value)),
                        EnumValue::Number(value) => ConfigValue::Enum(*value),
                    }),
                    vec![],
                );
            }
            return (
                Cow::Owned(ConfigValue::Null),
                vec![diagnostic(
                    syntax,
                    syntax.value,
                    d::Argument_for_0_option_must_be_Colon_1,
                    vec![
                        text(&format!("--{}", option.name)),
                        text(&option.enum_names()),
                    ],
                )],
            );
        }
        _ => {}
    }
    if value.is_null() {
        return (Cow::Borrowed(value), vec![]);
    }
    if option.extra_validation == "spec" {
        if let Some(message) = spec_diagnostic(
            value.as_string().expect("validated string spec").as_bytes(),
            false,
        ) {
            // The source validator omits the spec argument at this call site.
            return (
                Cow::Owned(ConfigValue::Null),
                vec![diagnostic(syntax, syntax.value, message, vec![])],
            );
        }
    }
    // Locale declarations are command-line-only and return above.
    assert_ne!(
        option.extra_validation, "locale",
        "locale config declarations are command-line-only"
    );
    if option.is_file_path {
        let input = value.as_string().expect("file path is a validated string");
        let slashes = ts_tspath::normalize_slashes(input.as_bytes());
        let normalized = if starts_with_config_dir(&slashes) {
            slashes.into_owned()
        } else {
            ts_tspath::absolute(&slashes, base_path)
        };
        return (
            Cow::Owned(ConfigValue::String(if normalized.is_empty() {
                text(".")
            } else {
                JsString::from_bytes(normalized)
            })),
            vec![],
        );
    }
    (Cow::Borrowed(value), vec![])
}
fn starts_with_config_dir(value: &[u8]) -> bool {
    crate::starts_with_config_dir(value)
}
fn convert_list(
    option: &OptionDeclaration,
    value: &ConfigValue,
    base_path: &[u8],
    syntax: OptionSyntax<'_>,
) -> (Cow<'static, ConfigValue>, Vec<Diagnostic>) {
    let ConfigValue::Array(values) = value else {
        return (Cow::Owned(ConfigValue::Array(None)), vec![]);
    };
    let Some(values) = values else {
        return (Cow::Owned(ConfigValue::Array(None)), vec![]);
    };
    let mut result = Vec::with_capacity(values.len());
    let mut errors = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let mut element_syntax = syntax;
        if let (Some(config), Some(node)) = (syntax.config, syntax.value) {
            let view = config.file.view();
            let node = view.node(node).expect("config list expression");
            let NodeDataRead::ArrayLiteralExpression(data) = node.data() else {
                panic!("array option syntax requires an array expression")
            };
            let list = view
                .list(data.elements().expect("nonempty array list"))
                .expect("config array list");
            element_syntax.value = view
                .node_slice(list.nodes())
                .expect("config array slice")
                .at(index);
        }
        let (converted, mut diagnostics) = convert_json_option(
            option.element.expect("list element declaration"),
            value,
            base_path,
            element_syntax,
        );
        errors.append(&mut diagnostics);
        if option.preserve_falsy
            || !matches!(
                converted.as_ref(),
                ConfigValue::Null | ConfigValue::Boolean(false) | ConfigValue::Integer(0)
            ) && !converted
                .as_ref()
                .as_string()
                .is_some_and(JsString::is_empty)
        {
            result.push(converted.into_owned());
        }
    }
    (Cow::Owned(ConfigValue::Array(Some(result))), errors)
}

pub fn spec_diagnostic(spec: &[u8], disallow_trailing_recursion: bool) -> Option<&'static Message> {
    let stripped = spec.strip_suffix(b"/").unwrap_or(spec);
    if disallow_trailing_recursion && (stripped == b"**" || stripped.ends_with(b"/**")) {
        return Some(d::File_specification_cannot_end_in_a_recursive_directory_wildcard_Asterisk_Asterisk_Colon_0);
    }
    let wildcard = if spec.starts_with(b"**/") {
        Some(0)
    } else {
        spec.windows(4).position(|w| w == b"/**/")
    };
    let parent = if spec.ends_with(b"/..") {
        Some(spec.len())
    } else {
        spec.windows(4).rposition(|w| w == b"/../")
    };
    if wildcard
        .zip(parent)
        .is_some_and(|(wildcard, parent)| parent > wildcard)
    {
        Some(d::File_specification_cannot_contain_a_parent_directory_that_appears_after_a_recursive_directory_wildcard_Asterisk_Asterisk_Colon_0)
    } else {
        None
    }
}

pub fn default_compiler_options(config_file_name: &[u8]) -> CompilerOptions {
    let mut options = CompilerOptions::default();
    if ts_tspath::base_name(config_file_name) == b"jsconfig.json" {
        options.allow_js = Tristate::TRUE;
        options.max_node_module_js_depth = Some(2);
        options.skip_lib_check = Tristate::TRUE;
        options.no_emit = Tristate::TRUE;
    }
    options
}

/// port: tsc/internal/tsoptions/tsconfigparsing.go:convertCompilerOptionsFromJsonWorker
pub fn compiler_options_from_json(
    json: &ConfigValue,
    base_path: &[u8],
    config_file_name: &[u8],
) -> (CompilerOptions, Vec<Diagnostic>) {
    let mut options = default_compiler_options(config_file_name);
    let mut errors = Vec::new();
    if let Some(entries) = json.as_object() {
        for (key, value) in entries {
            let Some(option) = option_declaration(key.as_bytes(), false) else {
                errors.push(unknown_option(key.as_bytes()));
                continue;
            };
            if option.name.as_bytes() != key.as_bytes() {
                errors.push(Diagnostic::compiler(
                    d::Unknown_compiler_option_0_Did_you_mean_1,
                    vec![key.clone(), text(option.name)],
                ));
                continue;
            }
            let (value, mut diagnostics) =
                convert_json_option(option, value, base_path, OptionSyntax::default());
            errors.append(&mut diagnostics);
            parse_compiler_options(key.as_bytes(), &value, &mut options);
        }
    }
    if !config_file_name.is_empty() {
        options.config_file_path =
            JsString::from_bytes(ts_tspath::normalize_slashes(config_file_name).into_owned());
    }
    (options, errors)
}
pub fn unknown_option(key: &[u8]) -> Diagnostic {
    let suggestion = ts_scanner::get_spelling_suggestion_for_strings(
        key,
        COMPILER_OPTIONS.iter().map(|option| option.name.as_bytes()),
    );
    if let Some(suggestion) = suggestion {
        Diagnostic::compiler(
            d::Unknown_compiler_option_0_Did_you_mean_1,
            vec![JsString::from_bytes(key), JsString::from_bytes(suggestion)],
        )
    } else {
        Diagnostic::compiler(
            d::Unknown_compiler_option_0,
            vec![JsString::from_bytes(key)],
        )
    }
}
