use crate::{
    find_declaration,
    fixture_options::{enum_error, enum_value, trim},
    parse_list_type_option, ConfigValue as V, OptionDeclaration, OptionKind, ParseConfigHost,
    BUILD_OPTIONS, COMPILER_OPTIONS, WATCH_OPTIONS,
};
use std::{borrow::Cow, collections::HashSet};
use tsr_ast::Diagnostic;
use tsr_core::collections::OrderedMap;
use tsr_diagnostics::{self as d, Message};
use tsr_jsstring::{wtf8::decode_utf8, JsString};

#[derive(Clone, Copy)]
pub(super) enum Mode {
    Compiler,
    Build,
}
impl Mode {
    fn declarations(self) -> &'static [OptionDeclaration] {
        match self {
            Self::Compiler => COMPILER_OPTIONS,
            Self::Build => BUILD_OPTIONS,
        }
    }
    fn mismatch(self) -> &'static Message {
        match self {
            Self::Compiler => d::Compiler_option_0_expects_an_argument,
            Self::Build => d::Build_option_0_requires_a_value_of_type_1,
        }
    }
    fn unknown(self, name: &[u8], argument: &[u8]) -> Diagnostic {
        let (alternate, alternate_message, unknown, suggestion_message) = match self {
            Self::Compiler => (
                BUILD_OPTIONS,
                d::Compiler_option_0_may_only_be_used_with_build,
                d::Unknown_compiler_option_0,
                d::Unknown_compiler_option_0_Did_you_mean_1,
            ),
            Self::Build => (
                COMPILER_OPTIONS,
                d::Compiler_option_0_may_not_be_used_with_build,
                d::Unknown_build_option_0,
                d::Unknown_build_option_0_Did_you_mean_1,
            ),
        };
        if let Some(option) = find_declaration(alternate, name, false) {
            return diagnostic(
                if option.name == "build" {
                    d::Option_build_must_be_the_first_command_line_argument
                } else {
                    alternate_message
                },
                &[name],
            );
        }
        let suggestion = tsr_scanner::get_spelling_suggestion_for_strings(
            name,
            self.declarations()
                .iter()
                .map(|option| option.name.as_bytes()),
        );
        if let Some(suggestion) = suggestion {
            diagnostic(suggestion_message, &[argument, suggestion])
        } else {
            diagnostic(unknown, &[argument])
        }
    }
}

pub(super) struct Parsed {
    pub options: OrderedMap<JsString, V>,
    pub files: Vec<JsString>,
    pub errors: Vec<Diagnostic>,
}
fn diagnostic(message: &'static Message, args: &[&[u8]]) -> Diagnostic {
    Diagnostic::compiler(
        message,
        args.iter().map(|arg| JsString::from_bytes(*arg)).collect(),
    )
}

/// port: tsc/internal/tsoptions/commandlineparser.go:getInputOptionName
pub fn input_option_name(input: &[u8]) -> &[u8] {
    let input = input.strip_prefix(b"-").unwrap_or(input);
    input.strip_prefix(b"-").unwrap_or(input)
}

// An explicit response-file stack preserves native depth-first ordering and
// the active-path cycle guard without putting user-controlled nesting on the
// Rust call stack. Sibling references are deliberately read again.
struct Frame<'a> {
    args: Cow<'a, [JsString]>,
    next: usize,
    response: Option<JsString>,
}

/// port: tsc/internal/tsoptions/commandlineparser.go:parseCommandLineWorker
pub(super) fn parse(args: &[JsString], host: &dyn ParseConfigHost, mode: Mode) -> Parsed {
    let mut parsed = Parsed {
        options: OrderedMap::default(),
        files: Vec::new(),
        errors: Vec::new(),
    };
    let mut active = HashSet::new();
    let mut frames = vec![Frame {
        args: Cow::Borrowed(args),
        next: 0,
        response: None,
    }];
    while let Some(frame) = frames.last_mut() {
        let Some(argument) = frame.args.get(frame.next) else {
            if let Some(path) = frames.pop().expect("active frame").response {
                active.remove(&path);
            }
            continue;
        };
        frame.next += 1;
        let bytes = argument.as_bytes();
        match bytes.first() {
            None => {}
            Some(b'@') => {
                let file_name = tsr_tspath::absolute(&bytes[1..], host.current_directory());
                let path = tsr_tspath::to_path(
                    &file_name,
                    host.current_directory(),
                    host.fs().use_case_sensitive_file_names(),
                );
                if active.insert(path.clone()) {
                    let args = parsed.response_file(&file_name, host);
                    frames.push(Frame {
                        args: Cow::Owned(args),
                        next: 0,
                        response: Some(path),
                    });
                }
            }
            Some(b'-') => {
                let name = input_option_name(bytes);
                if let Some(option) = find_declaration(mode.declarations(), name, true) {
                    frame.next =
                        parsed.option_value(&frame.args, frame.next, option, mode.mismatch());
                } else if let Some(option) = find_declaration(WATCH_OPTIONS, name, true) {
                    frame.next = parsed.option_value(
                        &frame.args,
                        frame.next,
                        option,
                        d::Watch_option_0_requires_a_value_of_type_1,
                    );
                } else {
                    parsed.errors.push(mode.unknown(name, bytes));
                }
            }
            _ => parsed.files.push(argument.clone()),
        }
    }
    parsed
}

impl Parsed {
    /// port: tsc/internal/tsoptions/commandlineparser.go:commandLineParser.parseResponseFile
    fn response_file(&mut self, name: &[u8], host: &dyn ParseConfigHost) -> Vec<JsString> {
        // The pinned ReadFile boundary exposes success only, discarding OS
        // error details. A failure contributes exactly Cannot_read_file_0.
        let Ok(Some(file)) = host.fs().read_file(name) else {
            self.errors.push(diagnostic(d::Cannot_read_file_0, &[name]));
            return Vec::new();
        };
        // Go converts to []rune, replacing each invalid UTF-8 byte separately.
        // from_utf8_lossy groups malformed sequences and is not equivalent.
        let mut text = Vec::new();
        let mut bytes = file.text.as_bytes();
        while !bytes.is_empty() {
            let (rune, width) = decode_utf8(bytes);
            text.push(char::from_u32(rune as u32).expect("UTF-8 decoder scalar"));
            bytes = &bytes[width..];
        }
        let mut args = Vec::new();
        let mut pos = 0;
        while pos < text.len() {
            while text.get(pos).is_some_and(|c| *c <= ' ') {
                pos += 1;
            }
            if pos == text.len() {
                break;
            }
            if text[pos] == '"' {
                pos += 1;
                let start = pos;
                while text.get(pos).is_some_and(|c| *c != '"') {
                    pos += 1;
                }
                if pos == text.len() {
                    self.errors.push(diagnostic(
                        d::Unterminated_quoted_string_in_response_file_0,
                        &[name],
                    ));
                } else {
                    let value: String = text[start..pos].iter().collect();
                    args.push(JsString::from_bytes(value.into_bytes()));
                    pos += 1;
                }
            } else {
                let start = pos;
                while text.get(pos).is_some_and(|c| *c > ' ') {
                    pos += 1;
                }
                let value: String = text[start..pos].iter().collect();
                args.push(JsString::from_bytes(value.into_bytes()));
            }
        }
        args
    }

    /// port: tsc/internal/tsoptions/commandlineparser.go:commandLineParser.parseOptionValue
    fn option_value(
        &mut self,
        args: &[JsString],
        mut i: usize,
        option: &'static OptionDeclaration,
        mismatch: &'static Message,
    ) -> usize {
        let next = args.get(i).map_or(b"".as_slice(), JsString::as_bytes);
        let key = JsString::from_bytes(option.name.as_bytes());
        if option.is_tsconfig_only {
            if next == b"null" {
                self.options.insert(key, V::Null);
                i += 1;
            } else if option.kind == OptionKind::Boolean {
                if next == b"false" {
                    self.options.insert(key, V::Boolean(false));
                    i += 1;
                } else {
                    if next == b"true" {
                        i += 1;
                    }
                    self.errors.push(diagnostic(d::Option_0_can_only_be_specified_in_tsconfig_json_file_or_set_to_false_or_null_on_command_line, &[option.name.as_bytes()]));
                }
            } else {
                self.errors.push(diagnostic(d::Option_0_can_only_be_specified_in_tsconfig_json_file_or_set_to_null_on_command_line, &[option.name.as_bytes()]));
                if !next.is_empty() && !next.starts_with(b"-") {
                    i += 1;
                }
            }
            return i;
        }
        if i >= args.len() {
            if option.kind == OptionKind::Boolean {
                self.options.insert(key, V::Boolean(true));
            } else {
                self.errors.push(diagnostic(
                    mismatch,
                    &[option.name.as_bytes(), option.value_type_name().as_bytes()],
                ));
                if option.kind == OptionKind::List {
                    self.options.insert(key, V::Array(Some(Vec::new())));
                } else if option.kind == OptionKind::Enum {
                    self.errors.push(enum_error(option));
                }
            }
            return i;
        }
        if next == b"null" {
            self.options.insert(key, V::Null);
            return i + 1;
        }
        match option.kind {
            OptionKind::Number => {
                match std::str::from_utf8(next)
                    .ok()
                    .and_then(|s| s.parse::<isize>().ok())
                {
                    Some(number) if (number as i64) >= option.min_value => {
                        self.options.insert(key, V::Integer(number as i64));
                    }
                    Some(_) => self.errors.push(diagnostic(
                        d::Option_0_requires_value_to_be_greater_than_1,
                        &[
                            option.name.as_bytes(),
                            option.min_value.to_string().as_bytes(),
                        ],
                    )),
                    None => self
                        .errors
                        .push(diagnostic(mismatch, &[option.name.as_bytes(), b"number"])),
                }
                i + 1
            }
            OptionKind::Boolean => {
                self.options.insert(key, V::Boolean(next != b"false"));
                i + usize::from(matches!(next, b"false" | b"true"))
            }
            OptionKind::String => {
                let error = if option.extra_validation == "locale" {
                    if std::str::from_utf8(next)
                        .is_ok_and(|value| tsr_locale::Locale::parse(value).1)
                    {
                        None
                    } else {
                        Some(diagnostic(
                            d::Locale_must_be_an_IETF_BCP_47_language_tag_Examples_Colon_0_1,
                            &[b"en", b"ja-jp"],
                        ))
                    }
                } else if option.extra_validation == "spec" {
                    crate::spec_diagnostic(next, false).map(|message| diagnostic(message, &[]))
                } else {
                    None
                };
                if let Some(error) = error {
                    self.errors.push(error);
                } else {
                    self.options
                        .insert(key, V::String(JsString::from_bytes(next)));
                }
                i + 1
            }
            OptionKind::List => {
                let (value, errors) = parse_list_type_option(option, next);
                let consume = value.as_array().is_some_and(|a| !a.is_empty()) || !errors.is_empty();
                self.options.insert(key, value);
                self.errors.extend(errors);
                i + usize::from(consume)
            }
            OptionKind::ListOrElement => panic!("listOrElement not supported here"),
            OptionKind::Enum | OptionKind::Object => {
                let next = trim(next, tsr_scanner::is_white_space_like);
                let value = if next.is_empty() || option.enum_values.is_empty() {
                    V::Null
                } else if let Some(value) = enum_value(option, next) {
                    value
                } else {
                    self.errors.push(enum_error(option));
                    V::Null
                };
                self.options.insert(key, value);
                i + 1
            }
        }
    }
}
