//! `harnessutil.go`, the options half: the harness's own `@option`s, the four
//! extra compiler options the harness accepts, and `SetOptionsFromTestConfig`.
use crate::Stop;
use std::collections::BTreeMap;
use tsr_core::CompilerOptions;
use tsr_tsoptions::{ConfigValue, OptionDeclaration};

/// A compiler setting to its string value after splitting by commas,
/// handling inclusions and exclusions and deduplicating; by lowercased name.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:TestConfiguration
pub type TestConfiguration = BTreeMap<String, String>;

// port: tsc/internal/testutil/harnessutil/harnessutil.go:NamedTestConfiguration
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamedTestConfiguration {
    /// `getFileBasedTestConfigurationDescription`: `key=value,key=value`
    /// over the varying options, sorted by key; empty when nothing varies.
    pub name: String,
    pub config: TestConfiguration,
}

// port: tsc/internal/testutil/harnessutil/harnessutil.go:HarnessOptions
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HarnessOptions {
    pub use_case_sensitive_file_names: bool,
    pub baseline_file: Vec<u8>,
    pub include_built_file: Vec<u8>,
    pub file_name: Vec<u8>,
    pub lib_files: Vec<Vec<u8>>,
    pub no_implicit_references: bool,
    pub current_directory: Vec<u8>,
    pub symlink: Vec<u8>,
    pub link: Vec<u8>,
    pub no_types_and_symbols: bool,
    pub full_emit_paths: bool,
    pub report_diagnostics: bool,
    pub capture_suggestions: bool,
    pub typescript_version: Vec<u8>,
}

/// Applies every `name: value` of `config`: `typescriptversion` is ignored;
/// a compiler option (`get_command_line_option`) is converted with
/// `get_option_value` and set with `tsr_tsoptions::parse_compiler_options`;
/// a harness option (`get_harness_option`) is set on `harness`; anything
/// else is `Stop::Fatal("Unknown compiler option '<name>'.")` unless
/// `allow_unknown_options`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:SetOptionsFromTestConfig
pub fn set_options_from_test_config(
    config: &TestConfiguration,
    compiler_options: &mut CompilerOptions,
    harness: &mut HarnessOptions,
    current_directory: &[u8],
    allow_unknown_options: bool,
) -> Result<(), Stop> {
    let _ = (
        config,
        compiler_options,
        harness,
        current_directory,
        allow_unknown_options,
    );
    todo!("port SetOptionsFromTestConfig")
}

/// The pin's `OptionsDeclarations` plus the four booleans the harness adds
/// (`allowNonTsExtensions`, `noErrorTruncation`, `suppressOutputPathCheck`,
/// `noCheck`), matched case-insensitively by name.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getCommandLineOption
pub fn get_command_line_option(name: &str) -> Option<&'static OptionDeclaration> {
    let _ = name;
    todo!("port getCommandLineOption over compilerOptions")
}

/// `harnessCommandLineOptions`, matched case-insensitively by name.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getHarnessOption
pub fn get_harness_option(name: &str) -> Option<&'static OptionDeclaration> {
    let _ = name;
    todo!("port getHarnessOption")
}

/// Sets one harness option from its converted value.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:parseHarnessOption
pub fn parse_harness_option(
    key: &str,
    value: &ConfigValue,
    harness: &mut HarnessOptions,
) -> Result<(), Stop> {
    let _ = (key, value, harness);
    todo!("port parseHarnessOption")
}

/// Converts a directive's text to the option's value: file-path strings
/// are made absolute against `cwd`; numbers, booleans and enum keys are
/// checked; lists go through `parse_list_type_option` (file-path elements
/// made absolute); object options are fatal.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getOptionValue
pub fn get_option_value(
    option: &OptionDeclaration,
    value: &str,
    cwd: &[u8],
) -> Result<ConfigValue, Stop> {
    let _ = (option, value, cwd);
    todo!("port getOptionValue")
}

/// The normalized value a directive string denotes for `option`: the enum
/// value, the boolean, or the string itself; `None` when the option or the
/// enum key is unknown.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:tryGetValueOfOptionString
pub fn try_get_value_of_option_string(option: &str, value: &str) -> Option<ConfigValue> {
    let _ = (option, value);
    todo!("port tryGetValueOfOptionString")
}

/// Every value `*` expands to: the enum keys, or `true`/`false`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getAllValuesForOption
pub fn get_all_values_for_option(option: &str) -> Vec<String> {
    let _ = option;
    todo!("port getAllValuesForOption")
}

/// `tsconfig.json` / `jsconfig.json` (lowercased), or empty.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:GetConfigNameFromFileName
pub fn get_config_name_from_file_name(file_name: &[u8]) -> &'static str {
    let _ = file_name;
    todo!("port GetConfigNameFromFileName")
}
