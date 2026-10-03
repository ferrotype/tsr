//! `harnessutil.go`, the options half: the harness's own `@option`s, the four
//! extra compiler options the harness accepts, and `SetOptionsFromTestConfig`.
use crate::Stop;
use std::collections::BTreeMap;
use tsr_core::CompilerOptions;
use tsr_jsstring::{equal_fold, helpers::to_lower_go, JsString};
use tsr_tsoptions::{
    ConfigValue, DefaultValueDescription, EnumValue, OptionDeclaration, OptionKind,
    COMPILER_OPTIONS,
};

/// A compiler setting to its string value after splitting by commas,
/// handling inclusions and exclusions and deduplicating; by lowercased name.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:TestConfiguration
pub type TestConfiguration = BTreeMap<String, String>;

// source: tsc/internal/testutil/harnessutil/harnessutil.go:NamedTestConfiguration
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamedTestConfiguration {
    /// `getFileBasedTestConfigurationDescription`: `key=value,key=value`
    /// over the varying options, sorted by key; empty when nothing varies.
    pub name: String,
    pub config: TestConfiguration,
}

// source: tsc/internal/testutil/harnessutil/harnessutil.go:HarnessOptions
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

/// A `tsoptions.CommandLineOption` literal that sets only a name, a kind and
/// possibly elements: every other field is Go's zero value.
const fn declaration(
    name: &'static str,
    kind: OptionKind,
    element: Option<&'static OptionDeclaration>,
) -> OptionDeclaration {
    OptionDeclaration {
        name,
        short_name: "",
        kind,
        is_file_path: false,
        is_tsconfig_only: false,
        is_command_line_only: false,
        enum_values: &[],
        deprecated_keys: &[],
        element,
        extra_validation: "",
        min_value: 0,
        allow_config_dir_template: false,
        preserve_falsy: false,
        category: None,
        description: None,
        default_value_description: DefaultValueDescription::Nil,
        show_in_simplified_help_view: false,
    }
}

/// The four booleans `compilerOptions` appends to `tsoptions.OptionsDeclarations`.
/// `noErrorTruncation` and `noCheck` are also declared there, and the first
/// declaration wins the lookup.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:compilerOptions
static HARNESS_COMPILER_OPTIONS: [OptionDeclaration; 4] = [
    declaration("allowNonTsExtensions", OptionKind::Boolean, None),
    declaration("noErrorTruncation", OptionKind::Boolean, None),
    declaration("suppressOutputPathCheck", OptionKind::Boolean, None),
    declaration("noCheck", OptionKind::Boolean, None),
];

/// `CommandLineOption.Elements()` of `libFiles` (`commandLineOptionElements`).
static LIB_FILES_ELEMENT: OptionDeclaration = declaration("libFiles", OptionKind::String, None);

// source: tsc/internal/testutil/harnessutil/harnessutil.go:harnessCommandLineOptions
static HARNESS_COMMAND_LINE_OPTIONS: [OptionDeclaration; 13] = [
    declaration("useCaseSensitiveFileNames", OptionKind::Boolean, None),
    declaration("baselineFile", OptionKind::String, None),
    declaration("includeBuiltFile", OptionKind::String, None),
    declaration("fileName", OptionKind::String, None),
    declaration("libFiles", OptionKind::List, Some(&LIB_FILES_ELEMENT)),
    declaration("noImplicitReferences", OptionKind::Boolean, None),
    declaration("currentDirectory", OptionKind::String, None),
    declaration("symlink", OptionKind::String, None),
    declaration("link", OptionKind::String, None),
    declaration("noTypesAndSymbols", OptionKind::Boolean, None),
    // Emitted js baseline will print full paths for every output file
    declaration("fullEmitPaths", OptionKind::Boolean, None),
    // used to enable error collection in `transpile` baselines
    declaration("reportDiagnostics", OptionKind::Boolean, None),
    // Adds suggestion diagnostics to error baselines
    declaration("captureSuggestions", OptionKind::Boolean, None),
];

/// Applies every `name: value` of `config`: `typescriptversion` is ignored;
/// a compiler option (`get_command_line_option`) is converted with
/// `get_option_value` and set with `tsr_tsoptions::parse_compiler_options`;
/// a harness option (`get_harness_option`) is set on `harness`; anything
/// else is `Stop::Fatal("Unknown compiler option '<name>'.")` unless
/// `allow_unknown_options`.
///
/// The pin ranges over a Go map, so which of several failing settings is
/// reported is random there; here the settings apply in name order.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:SetOptionsFromTestConfig
pub fn set_options_from_test_config(
    config: &TestConfiguration,
    compiler_options: &mut CompilerOptions,
    harness: &mut HarnessOptions,
    current_directory: &[u8],
    allow_unknown_options: bool,
) -> Result<(), Stop> {
    for (name, value) in config {
        if name == "typescriptversion" {
            continue;
        }

        if let Some(command_line_option) = get_command_line_option(name) {
            let parsed_value = get_option_value(command_line_option, value, current_directory)?;
            // `tsoptions.ParseCompilerOptions` reports no errors at the pin,
            // so the harness's "Error parsing value" fatal cannot happen.
            tsr_tsoptions::parse_compiler_options(
                command_line_option.name.as_bytes(),
                &parsed_value,
                compiler_options,
            );
            continue;
        }
        if let Some(harness_option) = get_harness_option(name) {
            let parsed_value = get_option_value(harness_option, value, current_directory)?;
            parse_harness_option(harness_option.name, &parsed_value, harness)?;
            continue;
        }
        if !allow_unknown_options {
            return Err(Stop::fatal(format!("Unknown compiler option '{name}'.")));
        }
    }
    Ok(())
}

/// The pin's `OptionsDeclarations` plus the four booleans the harness adds
/// (`allowNonTsExtensions`, `noErrorTruncation`, `suppressOutputPathCheck`,
/// `noCheck`), matched case-insensitively by name (`strings.EqualFold`); the
/// first match wins.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getCommandLineOption
pub fn get_command_line_option(name: &str) -> Option<&'static OptionDeclaration> {
    COMPILER_OPTIONS
        .iter()
        .chain(HARNESS_COMPILER_OPTIONS.iter())
        .find(|option| equal_fold(option.name.as_bytes(), name.as_bytes()))
}

/// `harnessCommandLineOptions`, matched case-insensitively by name.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getHarnessOption
pub fn get_harness_option(name: &str) -> Option<&'static OptionDeclaration> {
    HARNESS_COMMAND_LINE_OPTIONS
        .iter()
        .find(|option| equal_fold(option.name.as_bytes(), name.as_bytes()))
}

/// Sets one harness option from its converted value. The pin's type
/// assertions cannot fail for values `get_option_value` produced; a
/// mismatch is fatal here.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:parseHarnessOption
pub fn parse_harness_option(
    key: &str,
    value: &ConfigValue,
    harness: &mut HarnessOptions,
) -> Result<(), Stop> {
    match key {
        "useCaseSensitiveFileNames" => harness.use_case_sensitive_file_names = boolean(key, value)?,
        "baselineFile" => harness.baseline_file = string(key, value)?,
        "includeBuiltFile" => harness.include_built_file = string(key, value)?,
        "fileName" => harness.file_name = string(key, value)?,
        "libFiles" => {
            let ConfigValue::Array(values) = value else {
                return Err(type_mismatch(key, value, "[]any"));
            };
            harness.lib_files = values
                .iter()
                .flatten()
                .map(|value| string(key, value))
                .collect::<Result<_, _>>()?;
        }
        "noImplicitReferences" => harness.no_implicit_references = boolean(key, value)?,
        "currentDirectory" => harness.current_directory = string(key, value)?,
        "symlink" => harness.symlink = string(key, value)?,
        "link" => harness.link = string(key, value)?,
        "noTypesAndSymbols" => harness.no_types_and_symbols = boolean(key, value)?,
        "fullEmitPaths" => harness.full_emit_paths = boolean(key, value)?,
        "reportDiagnostics" => harness.report_diagnostics = boolean(key, value)?,
        "captureSuggestions" => harness.capture_suggestions = boolean(key, value)?,
        "typescriptVersion" => harness.typescript_version = string(key, value)?,
        _ => return Err(Stop::fatal(format!("Unknown harness option '{key}'."))),
    }
    Ok(())
}

fn boolean(key: &str, value: &ConfigValue) -> Result<bool, Stop> {
    match value {
        ConfigValue::Boolean(value) => Ok(*value),
        _ => Err(type_mismatch(key, value, "bool")),
    }
}

fn string(key: &str, value: &ConfigValue) -> Result<Vec<u8>, Stop> {
    match value {
        ConfigValue::String(value) => Ok(value.as_bytes().to_vec()),
        _ => Err(type_mismatch(key, value, "string")),
    }
}

fn type_mismatch(key: &str, value: &ConfigValue, expected: &str) -> Stop {
    Stop::fatal(format!(
        "interface conversion: harness option '{key}' is {value:?}, not {expected}"
    ))
}

/// Converts a directive's text to the option's value: file-path strings
/// are made absolute against `cwd`; numbers, booleans and enum keys are
/// checked; lists go through `parse_list_type_option` (file-path elements
/// made absolute, without the error check, as at the pin); object options
/// are fatal. `parse_list_type_option` needs a `'static` declaration, and
/// every declaration here is one.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getOptionValue
pub fn get_option_value(
    option: &'static OptionDeclaration,
    value: &str,
    cwd: &[u8],
) -> Result<ConfigValue, Stop> {
    match option.kind {
        OptionKind::String => Ok(ConfigValue::String(if option.is_file_path {
            JsString::from_bytes(tsr_tspath::absolute(value.as_bytes(), cwd))
        } else {
            JsString::from_bytes(value.as_bytes())
        })),
        // strconv.Atoi: an optional sign and decimal digits within int64.
        OptionKind::Number => value.parse::<i64>().map(ConfigValue::Integer).map_err(|_| {
            Stop::fatal(format!(
                "Value for option '{}' must be a number, got: {value}",
                option.name
            ))
        }),
        OptionKind::Boolean => match to_lower_go(value.as_bytes()).as_slice() {
            b"true" => Ok(ConfigValue::Boolean(true)),
            b"false" => Ok(ConfigValue::Boolean(false)),
            _ => Err(Stop::fatal(format!(
                "Value for option '{}' must be a boolean, got: {value}",
                option.name
            ))),
        },
        OptionKind::Enum => enum_value(option, value).ok_or_else(|| {
            let keys: Vec<&str> = option.enum_values.iter().map(|(key, _)| *key).collect();
            Stop::fatal(format!(
                "Value for option '{}' must be one of {}, got: {value}",
                option.name,
                keys.join(",")
            ))
        }),
        OptionKind::List | OptionKind::ListOrElement => {
            let (mut list, errors) =
                tsr_tsoptions::parse_list_type_option(option, value.as_bytes());
            if option.element.is_some_and(|element| element.is_file_path) {
                if let ConfigValue::Array(Some(items)) = &mut list {
                    for item in items {
                        let ConfigValue::String(name) = item else {
                            return Err(Stop::fatal(format!(
                                "interface conversion: '{}' element {item:?} is not a string",
                                option.name
                            )));
                        };
                        *item = ConfigValue::String(JsString::from_bytes(tsr_tspath::absolute(
                            name.as_bytes(),
                            cwd,
                        )));
                    }
                }
                return Ok(list);
            }
            if !errors.is_empty() {
                return Err(Stop::fatal(format!(
                    "Unknown value '{value}' for compiler option '{}'",
                    option.name
                )));
            }
            Ok(list)
        }
        OptionKind::Object => Err(Stop::fatal(format!(
            "Object type options like '{}' are not supported",
            option.name
        ))),
    }
}

/// `option.EnumMap().Get(strings.ToLower(value))`, as the typed value the
/// map holds.
fn enum_value(option: &OptionDeclaration, value: &str) -> Option<ConfigValue> {
    let lower = to_lower_go(value.as_bytes());
    option
        .enum_values
        .iter()
        .find(|(key, _)| key.as_bytes() == lower.as_slice())
        .map(|(_, value)| match value {
            EnumValue::Number(value) => ConfigValue::Enum(*value),
            EnumValue::String(value) => ConfigValue::String(JsString::from_bytes(value.as_bytes())),
        })
}

/// `tryGetValueOfOptionString`, fatal for an unknown option or value:
/// `Unknown value '<value>' for option '<option>'`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getValueOfOptionString
pub fn get_value_of_option_string(option: &str, value: &str) -> Result<ConfigValue, Stop> {
    try_get_value_of_option_string(option, value)
        .ok_or_else(|| Stop::fatal(format!("Unknown value '{value}' for option '{option}'")))
}

/// The normalized value a directive string denotes for `option`: the enum
/// value, the boolean, or the string itself; `None` when the option or the
/// enum key is unknown.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:tryGetValueOfOptionString
pub fn try_get_value_of_option_string(option: &str, value: &str) -> Option<ConfigValue> {
    let option_decl = get_command_line_option(option)?;
    match option_decl.kind {
        OptionKind::Enum => enum_value(option_decl, value),
        OptionKind::Boolean => match to_lower_go(value.as_bytes()).as_slice() {
            b"true" => Some(ConfigValue::Boolean(true)),
            b"false" => Some(ConfigValue::Boolean(false)),
            _ => None,
        },
        _ => Some(ConfigValue::String(JsString::from_bytes(value.as_bytes()))),
    }
}

/// Every value `*` expands to: the enum keys in the enum map's order, or
/// `true`/`false`.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:getAllValuesForOption
pub fn get_all_values_for_option(option: &str) -> Vec<String> {
    let Some(option_decl) = get_command_line_option(option) else {
        return Vec::new();
    };
    match option_decl.kind {
        OptionKind::Enum => option_decl
            .enum_values
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect(),
        OptionKind::Boolean => vec!["true".to_string(), "false".to_string()],
        _ => Vec::new(),
    }
}

/// `tsconfig.json` / `jsconfig.json` (the lowercased base name), or empty.
// port: tsc/internal/testutil/harnessutil/harnessutil.go:GetConfigNameFromFileName
pub fn get_config_name_from_file_name(file_name: &[u8]) -> &'static str {
    let basename_lower = to_lower_go(tsr_tspath::base_name(file_name));
    match basename_lower.as_slice() {
        b"tsconfig.json" => "tsconfig.json",
        b"jsconfig.json" => "jsconfig.json",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        get_all_values_for_option, get_command_line_option, get_config_name_from_file_name,
        get_harness_option, get_option_value, set_options_from_test_config,
        try_get_value_of_option_string, HarnessOptions, TestConfiguration,
    };
    use crate::Stop;
    use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
    use tsr_jsstring::JsString;
    use tsr_tsoptions::{ConfigValue, OptionKind};

    fn config(settings: &[(&str, &str)]) -> TestConfiguration {
        settings
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn lookups_are_case_insensitive_and_include_the_harness_booleans() {
        assert_eq!(
            get_command_line_option("TARGET").map(|o| o.name),
            Some("target")
        );
        for name in ["allownontsextensions", "suppressoutputpathcheck"] {
            let option = get_command_line_option(name).expect("harness compiler option");
            assert_eq!(option.kind, OptionKind::Boolean);
        }
        assert_eq!(
            get_harness_option("nOImplicitReferences").map(|o| o.name),
            Some("noImplicitReferences")
        );
        assert!(get_command_line_option("filename").is_none());
        assert_eq!(
            get_harness_option("filename").map(|o| o.name),
            Some("fileName")
        );
    }

    #[test]
    fn settings_set_compiler_and_harness_options() {
        let mut options = CompilerOptions::default();
        let mut harness = HarnessOptions::default();
        set_options_from_test_config(
            &config(&[
                ("target", "ES2015"),
                ("module", "commonjs"),
                ("strict", "TRUE"),
                ("allownontsextensions", "true"),
                ("nocheck", "true"),
                ("outdir", "out"),
                ("lib", "es2015, dom"),
                ("rootdirs", "a,b"),
                ("maxnodemodulejsdepth", "2"),
                ("libfiles", "react.d.ts,lib.d.ts"),
                ("currentdirectory", "/home"),
                ("notypesandsymbols", "true"),
                ("typescriptversion", "5.0"),
            ]),
            &mut options,
            &mut harness,
            b"/.src",
            false,
        )
        .expect("valid settings");
        assert_eq!(options.target, ScriptTarget::ES2015);
        assert_eq!(options.module, ModuleKind::COMMON_JS);
        assert_eq!(options.strict, Tristate::TRUE);
        assert_eq!(options.allow_non_ts_extensions, Tristate::TRUE);
        assert_eq!(options.no_check, Tristate::TRUE);
        // outDir is a file path: absolute against the current directory.
        assert_eq!(options.out_dir.as_bytes(), b"/.src/out");
        let lib: Vec<&[u8]> = options
            .lib
            .as_deref()
            .expect("lib")
            .iter()
            .map(JsString::as_bytes)
            .collect();
        assert_eq!(lib, vec![b"lib.es2015.d.ts".as_slice(), b"lib.dom.d.ts"]);
        let root_dirs: Vec<&[u8]> = options
            .root_dirs
            .as_deref()
            .expect("rootDirs")
            .iter()
            .map(JsString::as_bytes)
            .collect();
        assert_eq!(root_dirs, vec![b"/.src/a".as_slice(), b"/.src/b"]);
        assert_eq!(options.max_node_module_js_depth, Some(2));
        assert_eq!(
            harness.lib_files,
            vec![b"react.d.ts".to_vec(), b"lib.d.ts".to_vec()]
        );
        assert_eq!(harness.current_directory, b"/home");
        assert!(harness.no_types_and_symbols);
    }

    #[test]
    fn invalid_settings_are_fatal_unless_unknown_options_are_allowed() {
        let run = |settings: &[(&str, &str)], allow_unknown: bool| {
            set_options_from_test_config(
                &config(settings),
                &mut CompilerOptions::default(),
                &mut HarnessOptions::default(),
                b"/.src",
                allow_unknown,
            )
        };
        assert_eq!(
            run(&[("nosuchoption", "1")], false),
            Err(Stop::fatal("Unknown compiler option 'nosuchoption'."))
        );
        assert_eq!(run(&[("nosuchoption", "1")], true), Ok(()));
        assert_eq!(
            run(&[("strict", "yes")], true),
            Err(Stop::fatal(
                "Value for option 'strict' must be a boolean, got: yes"
            ))
        );
        assert_eq!(
            run(&[("module", "es3")], true),
            Err(Stop::fatal(
                "Value for option 'module' must be one of commonjs,amd,system,umd,es6,es2015,es2020,es2022,esnext,node16,node18,node20,nodenext,preserve, got: es3"
            ))
        );
        assert_eq!(
            run(&[("maxnodemodulejsdepth", "two")], true),
            Err(Stop::fatal(
                "Value for option 'maxNodeModuleJsDepth' must be a number, got: two"
            ))
        );
        assert_eq!(
            run(&[("lib", "es2015,nosuchlib")], true),
            Err(Stop::fatal(
                "Unknown value 'es2015,nosuchlib' for compiler option 'lib'"
            ))
        );
        assert_eq!(
            run(&[("paths", "{}")], true),
            Err(Stop::fatal(
                "Object type options like 'paths' are not supported"
            ))
        );
    }

    #[test]
    fn option_strings_normalize_to_their_values() {
        assert_eq!(
            try_get_value_of_option_string("target", "ES6"),
            try_get_value_of_option_string("target", "es2015")
        );
        assert_eq!(
            try_get_value_of_option_string("strict", "False"),
            Some(ConfigValue::Boolean(false))
        );
        assert_eq!(try_get_value_of_option_string("target", "es3"), None);
        assert_eq!(try_get_value_of_option_string("nosuchoption", "x"), None);
        assert!(matches!(
            try_get_value_of_option_string("outdir", "Out"),
            Some(ConfigValue::String(value)) if value.as_bytes() == b"Out"
        ));
        assert_eq!(get_all_values_for_option("strict"), vec!["true", "false"]);
        assert_eq!(
            get_all_values_for_option("moduledetection"),
            vec!["auto", "legacy", "force"]
        );
        assert!(get_all_values_for_option("outdir").is_empty());
        let option = get_command_line_option("declarationdir").expect("option");
        assert!(matches!(
            get_option_value(option, "../d", b"/.src/x").expect("string"),
            ConfigValue::String(value) if value.as_bytes() == b"/.src/d"
        ));
    }

    #[test]
    fn config_names_are_case_insensitive_base_names() {
        assert_eq!(
            get_config_name_from_file_name(b"/a/TSConfig.JSON"),
            "tsconfig.json"
        );
        assert_eq!(
            get_config_name_from_file_name(b"jsconfig.json"),
            "jsconfig.json"
        );
        assert_eq!(get_config_name_from_file_name(b"/a/tsconfig.base.json"), "");
    }
}
