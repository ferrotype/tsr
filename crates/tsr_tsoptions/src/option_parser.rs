//! Per-kind option parsing policies shared by config conversion and callers.
use crate::{ConfigValue, TypeAcquisition};
use tsr_ast::Diagnostic;
use tsr_core::{collections::OrderedMap, BuildOptions, CompilerOptions, WatchOptions};
use tsr_diagnostics::Message;
use tsr_jsstring::JsString;

/// Values passed here have already undergone option declaration validation.
pub trait OptionParser {
    fn parse_option(&mut self, key: &[u8], value: &ConfigValue) -> Vec<Diagnostic>;
    fn unknown_option(&self) -> &'static Message;
    fn unknown_did_you_mean(&self) -> &'static Message;
}
impl OptionParser for CompilerOptions {
    /// port: tsc/internal/tsoptions/parsinghelpers.go:compilerOptionsParser.ParseOption
    fn parse_option(&mut self, key: &[u8], value: &ConfigValue) -> Vec<Diagnostic> {
        crate::parse_compiler_options(key, value, self);
        Vec::new()
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:compilerOptionsParser.UnknownOptionDiagnostic
    fn unknown_option(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"compilerOptions")
            .expect("parser policy")
            .0
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:compilerOptionsParser.UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"compilerOptions")
            .expect("parser policy")
            .1
    }
}
impl OptionParser for WatchOptions {
    /// port: tsc/internal/tsoptions/parsinghelpers.go:watchOptionsParser.ParseOption
    fn parse_option(&mut self, key: &[u8], value: &ConfigValue) -> Vec<Diagnostic> {
        crate::parse_watch_options(key, value, self);
        Vec::new()
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:watchOptionsParser.UnknownOptionDiagnostic
    fn unknown_option(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"watchOptions")
            .expect("parser policy")
            .0
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:watchOptionsParser.UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"watchOptions")
            .expect("parser policy")
            .1
    }
}
impl OptionParser for BuildOptions {
    /// port: tsc/internal/tsoptions/parsinghelpers.go:buildOptionsParser.ParseOption
    fn parse_option(&mut self, key: &[u8], value: &ConfigValue) -> Vec<Diagnostic> {
        crate::parse_build_options(key, value, self);
        Vec::new()
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:buildOptionsParser.UnknownOptionDiagnostic
    fn unknown_option(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"buildOptions")
            .expect("parser policy")
            .0
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:buildOptionsParser.UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"buildOptions")
            .expect("parser policy")
            .1
    }
}
impl OptionParser for TypeAcquisition {
    /// port: tsc/internal/tsoptions/parsinghelpers.go:typeAcquisitionParser.ParseOption
    fn parse_option(&mut self, key: &[u8], value: &ConfigValue) -> Vec<Diagnostic> {
        TypeAcquisition::parse_option(self, key, value);
        Vec::new()
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:typeAcquisitionParser.UnknownOptionDiagnostic
    fn unknown_option(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"typeAcquisition")
            .expect("parser policy")
            .0
    }
    /// port: tsc/internal/tsoptions/parsinghelpers.go:typeAcquisitionParser.UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean(&self) -> &'static Message {
        crate::extra_key_diagnostics(b"typeAcquisition")
            .expect("parser policy")
            .1
    }
}
/// Assign a validated, ordered option map. Like the pin, ignore parser diagnostics;
/// conversion performs validation before this assignment step.
/// port: tsc/internal/tsoptions/tsconfigparsing.go:convertMapToOptions
pub fn convert_map_to_options(
    values: &OrderedMap<JsString, ConfigValue>,
    result: &mut impl OptionParser,
) {
    for (key, value) in values {
        result.parse_option(key.as_bytes(), value);
    }
}
