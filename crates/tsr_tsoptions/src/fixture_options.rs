//! The pinned compiler-test option boundary. These defaults and accepted
//! harness settings belong to test fixtures, not ordinary compiler defaults.
use crate::{
    find_declaration, ConfigValue as V, EnumValue, OptionDeclaration, OptionKind, OptionSyntax,
    BUILD_OPTIONS, COMPILER_OPTIONS,
};
use ts_ast::Diagnostic;
use ts_core::{CompilerOptions, NewLineKind, Tristate};
use ts_diagnostics as d;
use ts_jsstring::{helpers::to_lower_go, wtf8::decode_utf8, JsString};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixtureOptionError {
    InvalidValue(JsString),
    UnsupportedOption(JsString),
}
impl std::fmt::Display for FixtureOptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for FixtureOptionError {}
fn text(value: &[u8]) -> JsString {
    JsString::from_bytes(value)
}
fn trim(mut value: &[u8], predicate: impl Fn(i32) -> bool) -> &[u8] {
    while !value.is_empty() {
        let (rune, width) = decode_utf8(value);
        if !predicate(rune) {
            break;
        }
        value = &value[width..];
    }
    let mut end = 0;
    let mut index = 0;
    while index < value.len() {
        let (rune, width) = decode_utf8(&value[index..]);
        index += width;
        if !predicate(rune) {
            end = index;
        }
    }
    &value[..end]
}
fn go_space(rune: i32) -> bool {
    matches!(rune,0x09..=0x0d|0x20|0x85|0xa0|0x1680|0x2000..=0x200a|0x2028|0x2029|0x202f|0x205f|0x3000)
}
fn enum_value(option: &OptionDeclaration, value: &[u8]) -> Option<V> {
    let lower = to_lower_go(value);
    option.enum_values.iter().find_map(|(key, value)| {
        (key.as_bytes() == lower).then(|| match value {
            EnumValue::String(value) => V::String(text(value.as_bytes())),
            EnumValue::Number(value) => V::Enum(*value),
        })
    })
}
fn enum_error(option: &OptionDeclaration) -> Diagnostic {
    Diagnostic::compiler(
        d::Argument_for_0_option_must_be_Colon_1,
        vec![
            text(format!("--{}", option.name).as_bytes()),
            text(option.enum_names().as_bytes()),
        ],
    )
}
/// port: tsc/internal/tsoptions/commandlineparser.go:ParseListTypeOption
pub fn parse_list_type_option(
    option: &'static OptionDeclaration,
    value: &[u8],
) -> (V, Vec<Diagnostic>) {
    let value = trim(value, go_space);
    let mut errors = Vec::new();
    if value.is_empty() || value.starts_with(b"-") {
        return (V::Array(Some(Vec::new())), errors);
    }
    if option.kind == OptionKind::ListOrElement && !value.contains(&b',') {
        if option.extra_validation == "spec" {
            if let Some(message) = crate::spec_diagnostic(value, false) {
                return (
                    V::Array(Some(Vec::new())),
                    vec![Diagnostic::compiler(message, vec![])],
                );
            }
        }
        return (V::Array(Some(vec![V::String(text(value))])), errors);
    }
    let element = option.element.expect("list option element declaration");
    let mut values = None;
    for item in value.split(|b| *b == b',') {
        let converted = match element.kind {
            OptionKind::String => {
                if element.extra_validation == "spec" {
                    if let Some(message) = crate::spec_diagnostic(item, false) {
                        errors.push(Diagnostic::compiler(message, vec![]));
                        continue;
                    }
                }
                (!item.is_empty()).then(|| V::String(text(item)))
            }
            OptionKind::Boolean | OptionKind::Object | OptionKind::Number => {
                panic!("List of {} is not yet supported.", element.kind.as_str())
            }
            _ => {
                let item = trim(item, ts_scanner::is_white_space_like);
                if item.is_empty() {
                    continue;
                }
                match enum_value(element, item) {
                    Some(value @ V::String(_)) => Some(value),
                    Some(_) => None,
                    None => {
                        errors.push(enum_error(element));
                        None
                    }
                }
            }
        };
        if let Some(value) = converted {
            values.get_or_insert_with(Vec::new).push(value);
        }
    }
    (V::Array(values), errors)
}
/// source: tsc/internal/testutil/harnessutil/harnessutil.go:getOptionValue
fn fixture_value(
    option: &'static OptionDeclaration,
    value: &[u8],
    cwd: &[u8],
) -> Result<V, FixtureOptionError> {
    let invalid = || FixtureOptionError::InvalidValue(text(option.name.as_bytes()));
    match option.kind {
        OptionKind::String => Ok(V::String(if option.is_file_path {
            JsString::from_bytes(ts_tspath::absolute(value, cwd))
        } else {
            text(value)
        })),
        OptionKind::Boolean => match to_lower_go(value).as_slice() {
            b"true" => Ok(V::Boolean(true)),
            b"false" => Ok(V::Boolean(false)),
            _ => Err(invalid()),
        },
        OptionKind::Number => std::str::from_utf8(value)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .map(V::Integer)
            .ok_or_else(invalid),
        OptionKind::Enum => enum_value(option, value).ok_or_else(invalid),
        OptionKind::List | OptionKind::ListOrElement => {
            let (mut values, errors) = parse_list_type_option(option, value);
            if option.element.is_some_and(|element| element.is_file_path) {
                if let V::Array(Some(values)) = &mut values {
                    for value in values {
                        let name = value.as_string().expect("filepath list string");
                        *value = V::String(JsString::from_bytes(ts_tspath::absolute(
                            name.as_bytes(),
                            cwd,
                        )));
                    }
                }
                return Ok(values);
            }
            if errors.is_empty() {
                Ok(values)
            } else {
                Err(invalid())
            }
        }
        OptionKind::Object => Err(invalid()),
    }
}
const HARNESS: &[(&[u8], bool)] = &[
    (b"usecasesensitivefilenames", true),
    (b"baselinefile", false),
    (b"includebuiltfile", false),
    (b"filename", false),
    (b"libfiles", false),
    (b"noimplicitreferences", true),
    (b"currentdirectory", false),
    (b"symlink", false),
    (b"link", false),
    (b"notypesandsymbols", true),
    (b"fullemitpaths", true),
    (b"reportdiagnostics", true),
    (b"capturesuggestions", true),
];
/// Execute the source fixture defaults and sorted S07 access bridge settings.
/// Removed settings keep the source CLI diagnostics rather than disappearing.
pub fn apply_fixture_settings(
    options: &mut CompilerOptions,
    settings: &[(JsString, JsString)],
    cwd: &[u8],
    has_config: bool,
) -> Result<(bool, Vec<Diagnostic>), FixtureOptionError> {
    if options.new_line == NewLineKind::NONE {
        options.new_line = NewLineKind::CRLF;
    }
    if options.skip_default_lib_check == Tristate::UNKNOWN {
        options.skip_default_lib_check = Tristate::TRUE;
    }
    options.no_error_truncation = Tristate::TRUE;
    let mut settings: Vec<_> = settings.iter().collect();
    settings.sort_by(|a, b| a.0.cmp(&b.0));
    let mut case_sensitive = true;
    let mut errors = Vec::new();
    for (key, value) in settings {
        let name = key.as_bytes();
        if name == b"typescriptversion" {
            continue;
        }
        let option = COMPILER_OPTIONS
            .iter()
            .find(|option| to_lower_go(option.name.as_bytes()) == name);
        if let Some(option) = option {
            if option.kind == OptionKind::Enum && enum_value(option, value.as_bytes()).is_none() {
                let value = V::String(text(trim(
                    value.as_bytes(),
                    ts_scanner::is_white_space_like,
                )));
                errors.extend(
                    crate::convert_json_option(option, &value, cwd, OptionSyntax::default()).1,
                );
                continue;
            }
            let adjusted = if name == b"baseurl"
                && !has_config
                && ts_tspath::encoded_root_length(value.as_bytes()) <= 0
            {
                Some(ts_tspath::absolute(value.as_bytes(), cwd))
            } else {
                None
            };
            let value =
                fixture_value(option, adjusted.as_deref().unwrap_or(value.as_bytes()), cwd)?;
            crate::parse_compiler_options(option.name.as_bytes(), &value, options);
            continue;
        }
        if matches!(
            name,
            b"allownontsextensions"
                | b"noerrortruncation"
                | b"suppressoutputpathcheck"
                | b"nocheck"
        ) {
            let value = match to_lower_go(value.as_bytes()).as_slice() {
                b"true" => Tristate::TRUE,
                b"false" => Tristate::FALSE,
                _ => return Err(FixtureOptionError::InvalidValue(key.clone())),
            };
            match name {
                b"allownontsextensions" => options.allow_non_ts_extensions = value,
                b"noerrortruncation" => options.no_error_truncation = value,
                b"suppressoutputpathcheck" => options.suppress_output_path_check = value,
                _ => options.no_check = value,
            }
            continue;
        }
        if let Some((_, boolean)) = HARNESS.iter().find(|(key, _)| *key == name) {
            if *boolean {
                let boolean = match to_lower_go(value.as_bytes()).as_slice() {
                    b"true" => true,
                    b"false" => false,
                    _ => return Err(FixtureOptionError::InvalidValue(key.clone())),
                };
                if name == b"usecasesensitivefilenames" {
                    case_sensitive = boolean;
                }
            }
            continue;
        }
        // This is the source bridge's ParseCommandLine(["--"+name,value])
        // error path. A second switch/response argument requires the full CLI
        // operation and is not silently treated as a positional fixture value.
        if value.as_bytes().starts_with(b"-") || value.as_bytes().starts_with(b"@") {
            return Err(FixtureOptionError::UnsupportedOption(key.clone()));
        }
        if let Some(option) = find_declaration(BUILD_OPTIONS, name, false) {
            errors.push(Diagnostic::compiler(
                if option.name == "build" {
                    d::Option_build_must_be_the_first_command_line_argument
                } else {
                    d::Compiler_option_0_may_only_be_used_with_build
                },
                vec![key.clone()],
            ));
        } else {
            let suggestion = ts_scanner::get_spelling_suggestion_for_strings(
                name,
                COMPILER_OPTIONS.iter().map(|option| option.name.as_bytes()),
            );
            let mut argument = b"--".to_vec();
            argument.extend_from_slice(name);
            errors.push(if let Some(suggestion) = suggestion {
                Diagnostic::compiler(
                    d::Unknown_compiler_option_0_Did_you_mean_1,
                    vec![JsString::from_bytes(argument), text(suggestion)],
                )
            } else {
                Diagnostic::compiler(
                    d::Unknown_compiler_option_0,
                    vec![JsString::from_bytes(argument)],
                )
            });
        }
    }
    for value in [
        &mut options.out_dir,
        &mut options.project,
        &mut options.root_dir,
        &mut options.ts_build_info_file,
        &mut options.base_url,
        &mut options.declaration_dir,
    ] {
        if !value.is_empty() {
            *value = JsString::from_bytes(ts_tspath::absolute(value.as_bytes(), cwd));
        }
    }
    for values in [&mut options.root_dirs, &mut options.type_roots]
        .into_iter()
        .flatten()
    {
        for value in values {
            *value = JsString::from_bytes(ts_tspath::absolute(value.as_bytes(), cwd));
        }
    }
    Ok((case_sensitive, errors))
}
