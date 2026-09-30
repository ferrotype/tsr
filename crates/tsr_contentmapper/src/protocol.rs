//! The content-mapper protocol's messages (`hostimpl.go`), with both codecs:
//! the host encodes params and decodes results, and in-process mappers do the
//! reverse. The codecs follow the pinned json v2 rules: case-sensitive names,
//! unknown names skipped, `omitempty` leaving out `""`, `null`, `[]` and `{}`.
use tsr_json::{Decode, Decoder, Encode, Encoder, Error, Kind, RawValue};
use tsr_jsonrpc::{custom_error, field, null, object, omitted, raw_field, Field};

/// Content mapper protocol method names.
pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_OPEN_PROJECT: &str = "openProject";
pub const METHOD_CLOSE_PROJECT: &str = "closeProject";
pub const METHOD_TRANSFORM: &str = "transform";

/// The coordinate space of a mapper's mappings and diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PositionEncoding(pub String);

impl PositionEncoding {
    pub const UTF8: &'static str = "utf-8";
    pub const UTF16: &'static str = "utf-16";
    pub fn utf8() -> Self {
        Self(Self::UTF8.into())
    }
    pub fn utf16() -> Self {
        Self(Self::UTF16.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Encode for PositionEncoding {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.0.encode(out)
    }
}

impl Decode for PositionEncoding {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        input.value(&mut self.0)
    }
}

/// `omitempty` over a string.
#[allow(clippy::ptr_arg)] // the field encodes the `String`
fn nonempty_string<'a>(name: &'a [u8], value: &'a String) -> Field<'a> {
    omitted(name, (!value.is_empty()).then_some(value as &dyn Encode))
}

/// `omitempty` over a slice.
#[allow(clippy::ptr_arg)] // the field encodes the `Vec`
fn nonempty_slice<'a, T: Encode>(name: &'a [u8], value: &'a Vec<T>) -> Field<'a> {
    omitted(name, (!value.is_empty()).then_some(value as &dyn Encode))
}

/// `omitempty` over a raw value: absent, or empty once encoded.
fn nonempty_raw<'a>(name: &'a [u8], value: Option<&'a RawValue>) -> Field<'a> {
    let empty = |raw: &RawValue| {
        let compact: Vec<u8> = raw
            .0
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        matches!(&compact[..], b"" | b"null" | b"\"\"" | b"{}" | b"[]")
    };
    omitted(
        name,
        value
            .filter(|raw| !empty(raw))
            .map(|raw| raw as &dyn Encode),
    )
}

/// The params of the initialize request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InitializeParams {
    /// The BCP 47 locale for mapper-authored messages, when configured.
    pub locale: String,
    pub position_encodings: Vec<PositionEncoding>,
}

impl Encode for InitializeParams {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                nonempty_string(b"locale", &self.locale),
                field(b"positionEncodings", &self.position_encodings),
            ],
        )
    }
}

impl Decode for InitializeParams {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"locale" => input.value(&mut self.locale),
            b"positionEncodings" => input.value(&mut self.position_encodings),
            _ => input.skip_value(),
        })
    }
}

/// A mapper's answer to initialize.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InitializeResult {
    pub position_encoding: PositionEncoding,
    /// The prefix of every mapper-authored diagnostic code.
    pub diagnostic_source: String,
}

impl Encode for InitializeResult {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"positionEncoding", &self.position_encoding),
                field(b"diagnosticSource", &self.diagnostic_source),
            ],
        )
    }
}

impl Decode for InitializeResult {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"positionEncoding" => input.value(&mut self.position_encoding),
            b"diagnosticSource" => input.value(&mut self.diagnostic_source),
            _ => input.skip_value(),
        })
    }
}

/// The params of openProject.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenProjectParams {
    pub config_file_name: String,
    pub project_handle: String,
    /// The mapper entry's options from the project's configuration.
    pub options: Option<RawValue>,
    /// The project's effective compiler options.
    pub compiler_options: RawValue,
}

impl Encode for OpenProjectParams {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"configFileName", &self.config_file_name),
                field(b"projectHandle", &self.project_handle),
                nonempty_raw(b"options", self.options.as_ref()),
                field(b"compilerOptions", &self.compiler_options),
            ],
        )
    }
}

impl Decode for OpenProjectParams {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"configFileName" => input.value(&mut self.config_file_name),
            b"projectHandle" => input.value(&mut self.project_handle),
            b"options" => raw_field(input, &mut self.options),
            b"compilerOptions" => input.value(&mut self.compiler_options),
            _ => input.skip_value(),
        })
    }
}

/// A mapper's answer to openProject. Only mappers declaring dynamic
/// configuration may return a config identity or watched files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenProjectResult {
    pub config_identity: String,
    pub watched_files: Vec<String>,
    pub option_diagnostics: Vec<OptionDiagnosticResult>,
}

impl Encode for OpenProjectResult {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"configIdentity", &self.config_identity),
                nonempty_slice(b"watchedFiles", &self.watched_files),
                nonempty_slice(b"optionDiagnostics", &self.option_diagnostics),
            ],
        )
    }
}

impl Decode for OpenProjectResult {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"configIdentity" => input.value(&mut self.config_identity),
            b"watchedFiles" => input.value(&mut self.watched_files),
            b"optionDiagnostics" => input.value(&mut self.option_diagnostics),
            _ => input.skip_value(),
        })
    }
}

/// A report on the mapper entry's options, located by its path within them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptionDiagnosticResult {
    pub path: Vec<RawValue>,
    pub message_text: String,
    pub code: i32,
}

impl Encode for OptionDiagnosticResult {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"path", &self.path),
                field(b"messageText", &self.message_text),
                field(b"code", &self.code),
            ],
        )
    }
}

impl Decode for OptionDiagnosticResult {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"path" => input.value(&mut self.path),
            b"messageText" => input.value(&mut self.message_text),
            b"code" => input.value(&mut self.code),
            _ => input.skip_value(),
        })
    }
}

/// The params of closeProject.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloseProjectParams {
    pub project_handle: String,
}

impl Encode for CloseProjectParams {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(out, &[field(b"projectHandle", &self.project_handle)])
    }
}

impl Decode for CloseProjectParams {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"projectHandle" => input.value(&mut self.project_handle),
            _ => input.skip_value(),
        })
    }
}

/// The params of transform.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransformParams {
    pub file_name: String,
    pub content: String,
    pub project_handle: String,
}

impl Encode for TransformParams {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"fileName", &self.file_name),
                field(b"content", &self.content),
                field(b"projectHandle", &self.project_handle),
            ],
        )
    }
}

impl Decode for TransformParams {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"fileName" => input.value(&mut self.file_name),
            b"content" => input.value(&mut self.content),
            b"projectHandle" => input.value(&mut self.project_handle),
            _ => input.skip_value(),
        })
    }
}

/// Virtual source text and its mapping to an original input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MappedOutput {
    pub text: String,
    /// Determines the virtual source's syntax.
    pub extension: String,
    /// The span map's tuple JSON; absent or empty means fully synthesized.
    pub mappings: Option<RawValue>,
    pub diagnostic_directives: Option<DiagnosticDirectives>,
}

impl MappedOutput {
    fn fields(&self) -> [Field<'_>; 4] {
        [
            field(b"text", &self.text),
            field(b"extension", &self.extension),
            nonempty_raw(b"mappings", self.mappings.as_ref()),
            omitted(
                b"diagnosticDirectives",
                self.diagnostic_directives
                    .as_ref()
                    .map(|value| value as &dyn Encode),
            ),
        ]
    }

    fn decode_field(&mut self, name: &[u8], input: &mut Decoder<'_>) -> Option<Result<(), Error>> {
        Some(match name {
            b"text" => input.value(&mut self.text),
            b"extension" => input.value(&mut self.extension),
            b"mappings" => raw_field(input, &mut self.mappings),
            b"diagnosticDirectives" => input.value(&mut self.diagnostic_directives),
            _ => return None,
        })
    }
}

impl Encode for MappedOutput {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(out, &self.fields())
    }
}

impl Decode for MappedOutput {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| {
            self.decode_field(name, input)
                .unwrap_or_else(|| input.skip_value())
        })
    }
}

/// The numeric policy of a mapped diagnostic directive.
pub type DiagnosticDirectivePolicy = u8;
pub const DIAGNOSTIC_DIRECTIVE_POLICY_IGNORE: DiagnosticDirectivePolicy = 0;
pub const DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT: DiagnosticDirectivePolicy = 1;

/// The diagnostic an expect directive reports when nothing is suppressed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnusedExpectDirectiveDiagnostic {
    pub code: i32,
    pub message_text: String,
}

impl Encode for UnusedExpectDirectiveDiagnostic {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"code", &self.code),
                field(b"messageText", &self.message_text),
            ],
        )
    }
}

impl Decode for UnusedExpectDirectiveDiagnostic {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"code" => input.value(&mut self.code),
            b"messageText" => input.value(&mut self.message_text),
            _ => input.skip_value(),
        })
    }
}

/// Framework directives over virtual ranges, sharing unused-expect diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticDirectives {
    pub unused_expect_directive_diagnostics: Vec<UnusedExpectDirectiveDiagnostic>,
    pub directives: Vec<MappedDiagnosticDirective>,
}

impl Encode for DiagnosticDirectives {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(
                    b"unusedExpectDirectiveDiagnostics",
                    &self.unused_expect_directive_diagnostics,
                ),
                field(b"directives", &self.directives),
            ],
        )
    }
}

impl Decode for DiagnosticDirectives {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"unusedExpectDirectiveDiagnostics" => {
                input.value(&mut self.unused_expect_directive_diagnostics)
            }
            b"directives" => input.value(&mut self.directives),
            _ => input.skip_value(),
        })
    }
}

/// `[originalStart, originalLength, virtualStart, virtualEnd, policy,
/// unusedExpectDirectiveIndex?]`. Without the index, an expect directive
/// selects the only unused-expect diagnostic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MappedDiagnosticDirective {
    pub original_start: i64,
    pub original_length: i64,
    pub virtual_start: i64,
    pub virtual_end: i64,
    pub policy: DiagnosticDirectivePolicy,
    pub unused_expect_directive_index: Option<i64>,
}

impl Encode for MappedDiagnosticDirective {
    /// port: tsc/internal/contentmapper/hostimpl.go:MappedDiagnosticDirective.MarshalJSONTo
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        let mut tuple = vec![
            self.original_start,
            self.original_length,
            self.virtual_start,
            self.virtual_end,
            i64::from(self.policy),
        ];
        tuple.extend(self.unused_expect_directive_index);
        out.array(tuple.iter())
    }
}

impl Decode for MappedDiagnosticDirective {
    fn type_name() -> &'static str {
        "contentmapper.MappedDiagnosticDirective"
    }
    /// port: tsc/internal/contentmapper/hostimpl.go:MappedDiagnosticDirective.UnmarshalJSONFrom
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        let (offset, pointer) = input.next_location()?;
        let kind = input.peek_kind();
        let mut tuple: Vec<RawValue> = Vec::new();
        input.value(&mut tuple)?;
        let error = |message: String| {
            custom_error(
                <Self as Decode>::type_name(),
                offset,
                pointer.clone(),
                kind,
                message,
            )
        };
        if tuple.len() != 5 && tuple.len() != 6 {
            return Err(error(format!(
                "diagnostic directive tuple must contain 5 or 6 elements, got {}",
                tuple.len()
            )));
        }
        *self = Self::default();
        let element = |index: usize, value: &mut dyn DecodeInto| {
            value.decode_from(&tuple[index].0).map_err(|cause| {
                error(format!(
                    "invalid diagnostic directive tuple element {index}: {cause}"
                ))
            })
        };
        element(0, &mut self.original_start)?;
        element(1, &mut self.original_length)?;
        element(2, &mut self.virtual_start)?;
        element(3, &mut self.virtual_end)?;
        let mut policy = 0u64;
        element(4, &mut policy)?;
        self.policy = u8::try_from(policy).map_err(|_| {
            error(format!(
                "invalid diagnostic directive tuple element 4: cannot unmarshal JSON number {policy} into Go uint8: value out of range"
            ))
        })?;
        if tuple.len() == 6 {
            let mut index = 0i64;
            element(5, &mut index)?;
            self.unused_expect_directive_index = Some(index);
        }
        Ok(())
    }
}

/// `json.Unmarshal` of one tuple element into its field.
trait DecodeInto {
    fn decode_from(&mut self, bytes: &[u8]) -> Result<(), Error>;
}

impl<T: Decode> DecodeInto for T {
    fn decode_from(&mut self, bytes: &[u8]) -> Result<(), Error> {
        tsr_json::unmarshal(bytes, self, tsr_json::Options::default())
    }
}

/// A mapper-authored error in original-source coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostic {
    pub message_text: String,
    pub start: i64,
    pub length: i64,
    pub code: i32,
}

impl Encode for Diagnostic {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"messageText", &self.message_text),
                field(b"start", &self.start),
                field(b"length", &self.length),
                field(b"code", &self.code),
            ],
        )
    }
}

impl Decode for Diagnostic {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"messageText" => input.value(&mut self.message_text),
            b"start" => input.value(&mut self.start),
            b"length" => input.value(&mut self.length),
            b"code" => input.value(&mut self.code),
            _ => input.skip_value(),
        })
    }
}

/// The canonical output for one input file, with its mapper-authored errors
/// and supplemental outputs; the mapped output's fields are inlined.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransformResultMessage {
    pub output: MappedOutput,
    pub diagnostics: Vec<Diagnostic>,
    pub supplemental: Vec<MappedOutput>,
}

impl Encode for TransformResultMessage {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        let [text, extension, mappings, directives] = self.output.fields();
        object(
            out,
            &[
                text,
                extension,
                mappings,
                directives,
                nonempty_slice(b"diagnostics", &self.diagnostics),
                nonempty_slice(b"supplemental", &self.supplemental),
            ],
        )
    }
}

impl Decode for TransformResultMessage {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| {
            if let Some(result) = self.output.decode_field(name, input) {
                return result;
            }
            match name {
                b"diagnostics" => input.value(&mut self.diagnostics),
                b"supplemental" => input.value(&mut self.supplemental),
                _ => input.skip_value(),
            }
        })
    }
}

/// A raw JSON value's first significant byte, as the pin's `json.Value.Kind`.
pub fn raw_kind(value: &RawValue) -> Kind {
    value.kind()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marshal(value: &dyn Encode) -> String {
        String::from_utf8(tsr_json::marshal(value, tsr_json::Options::default()).unwrap()).unwrap()
    }

    #[test]
    fn empty_fields_follow_omitempty_and_results_inline_their_output() {
        assert_eq!(
            marshal(&InitializeParams {
                locale: String::new(),
                position_encodings: vec![PositionEncoding::utf8(), PositionEncoding::utf16()]
            }),
            r#"{"positionEncodings":["utf-8","utf-16"]}"#
        );
        assert_eq!(
            marshal(&OpenProjectParams {
                config_file_name: "/tsconfig.json".into(),
                project_handle: "m:0".into(),
                options: Some(RawValue(b"{}".to_vec())),
                compiler_options: RawValue(b"{}".to_vec()),
            }),
            r#"{"configFileName":"/tsconfig.json","projectHandle":"m:0","compilerOptions":{}}"#
        );
        assert_eq!(
            marshal(&OpenProjectResult::default()),
            r#"{"configIdentity":""}"#
        );
        let result = TransformResultMessage {
            output: MappedOutput {
                text: "x".into(),
                extension: ".ts".into(),
                mappings: Some(RawValue(b"[[0,1,0,1,0]]".to_vec())),
                diagnostic_directives: Some(DiagnosticDirectives {
                    unused_expect_directive_diagnostics: Vec::new(),
                    directives: vec![MappedDiagnosticDirective {
                        policy: 1,
                        unused_expect_directive_index: Some(0),
                        ..MappedDiagnosticDirective::default()
                    }],
                }),
            },
            diagnostics: Vec::new(),
            supplemental: Vec::new(),
        };
        let text = marshal(&result);
        assert_eq!(
            text,
            r#"{"text":"x","extension":".ts","mappings":[[0,1,0,1,0]],"diagnosticDirectives":{"unusedExpectDirectiveDiagnostics":[],"directives":[[0,0,0,0,1,0]]}}"#
        );
        let mut decoded = TransformResultMessage::default();
        tsr_json::unmarshal(text.as_bytes(), &mut decoded, tsr_json::Options::default()).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn directive_tuples_reject_other_arities_and_element_types() {
        let decode = |bytes: &[u8]| {
            let mut directive = MappedDiagnosticDirective::default();
            tsr_json::unmarshal(bytes, &mut directive, tsr_json::Options::default())
                .map(|()| directive)
        };
        assert_eq!(decode(b"[1,2,3,4,0]").unwrap().virtual_end, 4);
        let arity = decode(b"[1,2,3]").unwrap_err().to_string();
        assert!(
            arity.contains("diagnostic directive tuple must contain 5 or 6 elements, got 3"),
            "{arity}"
        );
        let element = decode(br#"[1,2,3,4,"x"]"#).unwrap_err().to_string();
        assert!(
            element.contains("invalid diagnostic directive tuple element 4"),
            "{element}"
        );
    }
}
