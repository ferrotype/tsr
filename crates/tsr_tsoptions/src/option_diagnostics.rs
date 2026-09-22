//! Shared option-parser diagnostic policies; command-line workers and config
//! conversion use the same messages rather than reconstructing them in drivers.
use crate::{OptionDeclaration, BUILD_OPTIONS, COMPILER_OPTIONS, WATCH_OPTIONS};
use tsr_diagnostics::{self as d, Message};

#[derive(Clone, Copy)]
pub struct AlternateModeDiagnostics {
    pub diagnostic: &'static Message,
    pub declarations: &'static [OptionDeclaration],
}
#[derive(Clone, Copy)]
pub struct ParseCommandLineWorkerDiagnostics<'a> {
    pub declarations: &'a [OptionDeclaration],
    pub alternate: Option<AlternateModeDiagnostics>,
    pub unknown: &'static Message,
    pub did_you_mean: &'static Message,
    pub mismatch: &'static Message,
}
/// port: tsc/internal/tsoptions/diagnostics.go:getParseCommandLineWorkerDiagnostics
pub fn parse_command_line_worker_diagnostics(
    declarations: &[OptionDeclaration],
) -> ParseCommandLineWorkerDiagnostics<'_> {
    ParseCommandLineWorkerDiagnostics {
        declarations,
        alternate: Some(AlternateModeDiagnostics {
            diagnostic: d::Compiler_option_0_may_only_be_used_with_build,
            declarations: BUILD_OPTIONS,
        }),
        unknown: d::Unknown_compiler_option_0,
        did_you_mean: d::Unknown_compiler_option_0_Did_you_mean_1,
        mismatch: d::Compiler_option_0_expects_an_argument,
    }
}
pub fn build_worker_diagnostics() -> ParseCommandLineWorkerDiagnostics<'static> {
    ParseCommandLineWorkerDiagnostics {
        declarations: BUILD_OPTIONS,
        alternate: Some(AlternateModeDiagnostics {
            diagnostic: d::Compiler_option_0_may_not_be_used_with_build,
            declarations: COMPILER_OPTIONS,
        }),
        unknown: d::Unknown_build_option_0,
        did_you_mean: d::Unknown_build_option_0_Did_you_mean_1,
        mismatch: d::Build_option_0_requires_a_value_of_type_1,
    }
}
pub fn watch_worker_diagnostics() -> ParseCommandLineWorkerDiagnostics<'static> {
    ParseCommandLineWorkerDiagnostics {
        declarations: WATCH_OPTIONS,
        alternate: None,
        unknown: d::Unknown_watch_option_0,
        did_you_mean: d::Unknown_watch_option_0_Did_you_mean_1,
        mismatch: d::Watch_option_0_requires_a_value_of_type_1,
    }
}
/// port: tsc/internal/tsoptions/errors.go:extraKeyDiagnostics
/// port: tsc/internal/tsoptions/errors.go:extraKeyDidYouMeanDiagnostics
pub fn extra_key_diagnostics(parent: &[u8]) -> Option<(&'static Message, &'static Message)> {
    Some(match parent {
        b"compilerOptions" => (
            d::Unknown_compiler_option_0,
            d::Unknown_compiler_option_0_Did_you_mean_1,
        ),
        b"watchOptions" => (
            d::Unknown_watch_option_0,
            d::Unknown_watch_option_0_Did_you_mean_1,
        ),
        b"typeAcquisition" => (
            d::Unknown_type_acquisition_option_0,
            d::Unknown_type_acquisition_option_0_Did_you_mean_1,
        ),
        b"buildOptions" => (
            d::Unknown_build_option_0,
            d::Unknown_build_option_0_Did_you_mean_1,
        ),
        _ => return None,
    })
}
