//! `tsoptions/showconfig.go`: `ConvertToTSConfig` and its helpers.
//!
//! Ports of `tsc/internal/tsoptions/showconfig.go`, witnessed by the `tsoptions` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::affects::{OptionField, COMPILER_OPTION_FIELDS};
use crate::options_value::compiler_options_value;
use crate::{
    option_declaration, ConfigValue, EnumValue, OptionDeclaration, OptionKind, ParsedCommandLine,
};
use tsr_core::collections::OrderedMap;
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;

/// Go's `defaultIncludeSpec`.
const DEFAULT_INCLUDE_SPEC: &[u8] = b"**/*";
/// The category keys `serializeCompilerOptions` skips.
const COMMAND_LINE_OPTIONS: &str = "Command_line_Options_6171";
const OUTPUT_FORMATTING: &str = "Output_Formatting_6256";

/// Go's `TSConfig`, the `--showConfig` document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TsConfig {
    pub compiler_options: OrderedMap<JsString, ConfigValue>,
    /// `[path, circular]` per reference.
    pub references: Option<Vec<(JsString, bool)>>,
    pub files: Option<Vec<JsString>>,
    pub include: Option<Vec<JsString>>,
    pub exclude: Option<Vec<JsString>>,
    pub compile_on_save: Option<bool>,
}

/// Go's `impliedOption.compute` result: `(is_bool, value)`, the `any` its
/// table compares and serializes (a bool, or an enum's integer).
pub type ImpliedValue = (bool, i64);

/// Go's `computeFn`: a typed option getter viewed as the table's value.
/// port: tsc/internal/tsoptions/showconfig.go:computeFn
pub fn compute_fn(value: impl Into<ImpliedInput>) -> (bool, i64) {
    match value.into() {
        ImpliedInput::Bool(value) => (true, i64::from(value)),
        ImpliedInput::Int(value) => (false, i64::from(value)),
    }
}

/// The two value kinds the implied getters return.
pub enum ImpliedInput {
    Bool(bool),
    Int(i32),
}

impl From<bool> for ImpliedInput {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for ImpliedInput {
    fn from(value: i32) -> Self {
        Self::Int(value)
    }
}

/// Go's `impliedOption`.
struct ImpliedOption {
    name: &'static str,
    dependencies: &'static [&'static str],
    compute: fn(&CompilerOptions) -> ImpliedValue,
}

/// Go's `impliedOptions`, in order.
const IMPLIED_OPTIONS: &[ImpliedOption] = &[
    ImpliedOption {
        name: "Module",
        dependencies: &["Target"],
        compute: |o| compute_fn(o.emit_module_kind().0),
    },
    ImpliedOption {
        name: "ModuleResolution",
        dependencies: &["Module", "Target"],
        compute: |o| compute_fn(o.module_resolution_kind().0),
    },
    ImpliedOption {
        name: "ModuleDetection",
        dependencies: &["Module", "Target"],
        compute: |o| compute_fn(o.emit_module_detection_kind().0),
    },
    ImpliedOption {
        name: "IsolatedModules",
        dependencies: &["VerbatimModuleSyntax"],
        compute: |o| compute_fn(o.isolated_modules()),
    },
    ImpliedOption {
        name: "PreserveConstEnums",
        dependencies: &["IsolatedModules", "VerbatimModuleSyntax"],
        compute: |o| compute_fn(o.should_preserve_const_enums()),
    },
    ImpliedOption {
        name: "Declaration",
        dependencies: &["Composite"],
        compute: |o| compute_fn(o.emit_declarations()),
    },
    ImpliedOption {
        name: "DeclarationMap",
        dependencies: &["Declaration", "Composite"],
        compute: |o| compute_fn(o.declaration_maps_enabled()),
    },
    ImpliedOption {
        name: "Incremental",
        dependencies: &["Composite"],
        compute: |o| compute_fn(o.is_incremental()),
    },
    ImpliedOption {
        name: "UseDefineForClassFields",
        dependencies: &["Target", "Module"],
        compute: |o| compute_fn(o.use_define_for_class_fields()),
    },
    ImpliedOption {
        name: "ResolvePackageJsonExports",
        dependencies: &["ModuleResolution", "Module", "Target"],
        compute: |o| compute_fn(o.resolve_package_json_exports()),
    },
    ImpliedOption {
        name: "ResolvePackageJsonImports",
        dependencies: &[
            "ModuleResolution",
            "ResolvePackageJsonExports",
            "Module",
            "Target",
        ],
        compute: |o| compute_fn(o.resolve_package_json_imports()),
    },
    ImpliedOption {
        name: "ResolveJsonModule",
        dependencies: &["ModuleResolution", "Module", "Target"],
        compute: |o| compute_fn(o.resolve_json_module()),
    },
    ImpliedOption {
        name: "AllowJs",
        dependencies: &["CheckJs"],
        compute: |o| compute_fn(o.allow_js()),
    },
    ImpliedOption {
        name: "AllowImportingTsExtensions",
        dependencies: &["RewriteRelativeImportExtensions"],
        compute: |o| compute_fn(o.allow_importing_ts_extensions()),
    },
];

/// Go's case-insensitive `CommandLineCompilerOptionsMap.Get`.
fn compiler_option(name: &str) -> Option<&'static OptionDeclaration> {
    option_declaration(name.to_ascii_lowercase().as_bytes(), false)
        .or_else(|| option_declaration(name.as_bytes(), false))
        .or_else(|| {
            let mut lower_first = name.to_owned();
            if let Some(first) = lower_first.get_mut(..1) {
                first.make_ascii_lowercase();
            }
            option_declaration(lower_first.as_bytes(), false)
        })
}

fn enum_map(declaration: &OptionDeclaration) -> Option<&'static [(&'static str, EnumValue)]> {
    (declaration.kind == OptionKind::Enum).then_some(declaration.enum_values)
}

/// A value an enum map is searched for: Go compares `any` values.
#[derive(Clone, Copy)]
enum Lookup<'a> {
    Number(i32),
    Text(&'a [u8]),
}

fn entry_is(entry: &EnumValue, value: Lookup<'_>) -> bool {
    match (entry, value) {
        (EnumValue::Number(entry), Lookup::Number(value)) => *entry == value,
        (EnumValue::String(entry), Lookup::Text(value)) => entry.as_bytes() == value,
        _ => false,
    }
}

/// Go's `getNameOfCompilerOptionValue`: the first key whose value is `value`.
/// port: tsc/internal/tsoptions/showconfig.go:getNameOfCompilerOptionValue
fn get_name_of_compiler_option_value(
    value: Lookup<'_>,
    enum_map: &[(&'static str, EnumValue)],
) -> String {
    for (key, entry) in enum_map {
        if entry_is(entry, value) {
            return (*key).to_owned();
        }
    }
    String::new()
}

/// Go's `serializeEnumValue`: the key of an integer value, else the direct
/// comparison's.
/// port: tsc/internal/tsoptions/showconfig.go:serializeEnumValue
fn serialize_enum_value(value: Lookup<'_>, enum_map: &[(&'static str, EnumValue)]) -> String {
    if let Lookup::Number(number) = value {
        for (key, entry) in enum_map {
            if let EnumValue::Number(entry) = entry {
                if *entry == number {
                    return (*key).to_owned();
                }
            }
        }
    }
    get_name_of_compiler_option_value(value, enum_map)
}

/// port: tsc/internal/tsoptions/showconfig.go:serializeImpliedOptionValue
fn serialize_implied_option_value(
    declaration: &OptionDeclaration,
    value: ImpliedValue,
) -> Option<ConfigValue> {
    if let Some(enum_map) = enum_map(declaration) {
        let number = i32::try_from(value.1).ok()?;
        let key = serialize_enum_value(Lookup::Number(number), enum_map);
        if key.is_empty() {
            return None;
        }
        return Some(ConfigValue::String(JsString::from_bytes(key.as_bytes())));
    }
    Some(if value.0 {
        ConfigValue::Boolean(value.1 != 0)
    } else {
        ConfigValue::Integer(value.1)
    })
}

/// port: tsc/internal/tsoptions/showconfig.go:anyDependencyProvided
fn any_dependency_provided(
    dependencies: &[&str],
    provided: &std::collections::HashSet<Vec<u8>>,
) -> bool {
    for dependency in dependencies {
        if let Some(declaration) = compiler_option(dependency) {
            if provided.contains(declaration.name.as_bytes()) {
                return true;
            }
        }
    }
    false
}

/// port: tsc/internal/tsoptions/showconfig.go:addImpliedOptions
fn add_implied_options(
    option_map: &mut OrderedMap<JsString, ConfigValue>,
    options: &CompilerOptions,
) {
    let provided: std::collections::HashSet<Vec<u8>> = option_map
        .keys()
        .map(|key| key.as_bytes().to_vec())
        .collect();
    let default_options = CompilerOptions::default();
    for entry in IMPLIED_OPTIONS {
        let Some(declaration) = compiler_option(entry.name) else {
            continue;
        };
        if provided.contains(declaration.name.as_bytes()) {
            continue;
        }
        if !any_dependency_provided(entry.dependencies, &provided) {
            continue;
        }
        let implied = (entry.compute)(options);
        if implied == (entry.compute)(&default_options) {
            continue;
        }
        let Some(serialized) = serialize_implied_option_value(declaration, implied) else {
            continue;
        };
        option_map.insert(
            JsString::from_bytes(declaration.name.as_bytes()),
            serialized,
        );
    }
}

fn relative_to_config(
    path: &[u8],
    config_file_path: &[u8],
    cwd: &[u8],
    case_sensitive: bool,
) -> JsString {
    let config_dir = tsr_tspath::directory(config_file_path);
    let absolute = tsr_tspath::absolute(path, &config_dir);
    JsString::from_bytes(
        tsr_tspath::relative_from_file(config_file_path, &absolute, cwd, case_sensitive).as_slice(),
    )
}

/// Go's `serializeCompilerOptions`. The port marker is on the category
/// test, a site the mutation splicer can negate (a map has no replacement
/// value).
fn serialize_compiler_options(
    options: &CompilerOptions,
    config_file_path: &[u8],
    cwd: &[u8],
    case_sensitive: bool,
) -> OrderedMap<JsString, ConfigValue> {
    let mut result = OrderedMap::default();
    let values = compiler_options_value(options);
    for field in COMPILER_OPTION_FIELDS {
        let OptionField {
            declaration: name,
            category,
            ..
        } = *field;
        if name.is_empty() {
            continue;
        }
        let Some(declaration) = option_declaration(name.as_bytes(), false) else {
            continue;
        };
        // port: tsc/internal/tsoptions/showconfig.go:serializeCompilerOptions
        if category == COMMAND_LINE_OPTIONS || category == OUTPUT_FORMATTING {
            continue;
        }
        let Some(value) = values.get(name.as_bytes()) else {
            continue;
        };
        let key = JsString::from_bytes(name.as_bytes());
        if let Some(enum_map) = enum_map(declaration) {
            let lookup = match value {
                ConfigValue::Integer(number) => i32::try_from(*number).ok().map(Lookup::Number),
                ConfigValue::String(text) => Some(Lookup::Text(text.as_bytes())),
                _ => None,
            };
            if let Some(lookup) = lookup {
                let serialized = serialize_enum_value(lookup, enum_map);
                if !serialized.is_empty() {
                    result.insert(
                        key,
                        ConfigValue::String(JsString::from_bytes(serialized.as_bytes())),
                    );
                }
            }
            continue;
        }
        match declaration.kind {
            OptionKind::List => {
                let element = declaration.element;
                let strings: Option<Vec<JsString>> = match value {
                    ConfigValue::Array(Some(items)) => items
                        .iter()
                        .map(|item| match item {
                            ConfigValue::String(text) => Some(text.clone()),
                            _ => None,
                        })
                        .collect(),
                    ConfigValue::Array(None) => Some(Vec::new()),
                    _ => None,
                };
                if let (Some(element), Some(strings)) = (element, &strings) {
                    if element.is_file_path {
                        let relative = strings
                            .iter()
                            .map(|s| {
                                ConfigValue::String(relative_to_config(
                                    s.as_bytes(),
                                    config_file_path,
                                    cwd,
                                    case_sensitive,
                                ))
                            })
                            .collect();
                        result.insert(key, ConfigValue::Array(Some(relative)));
                        continue;
                    }
                    if element.kind == OptionKind::Enum {
                        let serialized = strings
                            .iter()
                            .map(|s| {
                                let found = get_name_of_compiler_option_value(
                                    Lookup::Text(s.as_bytes()),
                                    element.enum_values,
                                );
                                ConfigValue::String(if found.is_empty() {
                                    s.clone()
                                } else {
                                    JsString::from_bytes(found.as_bytes())
                                })
                            })
                            .collect();
                        result.insert(key, ConfigValue::Array(Some(serialized)));
                        continue;
                    }
                }
                result.insert(key, value.clone());
            }
            OptionKind::String => {
                if declaration.is_file_path {
                    if let ConfigValue::String(text) = value {
                        if !text.is_empty() {
                            result.insert(
                                key,
                                ConfigValue::String(relative_to_config(
                                    text.as_bytes(),
                                    config_file_path,
                                    cwd,
                                    case_sensitive,
                                )),
                            );
                            continue;
                        }
                    }
                }
                result.insert(key, value.clone());
            }
            _ => {
                result.insert(key, value.clone());
            }
        }
    }
    result
}

/// port: tsc/internal/tsoptions/showconfig.go:filterSameAsDefaultInclude
fn filter_same_as_default_include(specs: &[JsString]) -> Option<Vec<JsString>> {
    if specs.is_empty() {
        return None;
    }
    if specs.len() == 1 && specs[0].as_bytes() == DEFAULT_INCLUDE_SPEC {
        return None;
    }
    Some(specs.to_vec())
}

/// Go's `ConvertToTSConfig`. The port marker is on the files test, a site
/// the mutation splicer can negate (a document has no replacement value).
pub fn convert_to_ts_config(parsed: &ParsedCommandLine, config_file_name: &[u8]) -> TsConfig {
    let config_file_name: &[u8] = if config_file_name.is_empty() {
        b"tsconfig.json"
    } else {
        config_file_name
    };
    let cwd = parsed.current_directory().to_vec();
    let case_sensitive = parsed.use_case_sensitive_file_names();
    let config_path = tsr_tspath::absolute(config_file_name, &cwd);
    let mut files = Vec::new();
    for file in &parsed.root_file_names {
        let absolute = tsr_tspath::absolute(file.as_bytes(), &cwd);
        files.push(JsString::from_bytes(
            tsr_tspath::relative_from_file(&config_path, &absolute, &cwd, case_sensitive)
                .as_slice(),
        ));
    }
    let mut option_map =
        serialize_compiler_options(&parsed.options, &config_path, &cwd, case_sensitive);
    for name in [
        "showConfig",
        "configFile",
        "configFilePath",
        "help",
        "init",
        "listFilesOnly",
        "listEmittedFiles",
        "project",
        "build",
        "version",
    ] {
        option_map.remove(name.as_bytes());
    }
    add_implied_options(&mut option_map, &parsed.options);
    let mut config = TsConfig {
        compiler_options: option_map,
        ..TsConfig::default()
    };
    if let Some(references) = parsed
        .project_references
        .as_ref()
        .filter(|references| !references.is_empty())
    {
        config.references = Some(
            references
                .iter()
                .map(|reference| (reference.original_path.clone(), reference.circular))
                .collect(),
        );
    }
    // port: tsc/internal/tsoptions/showconfig.go:ConvertToTSConfig
    if !files.is_empty() {
        config.files = Some(files);
    }
    if parsed.config_file.is_some() {
        if let Some(specs) = parsed.config_specs.as_ref() {
            config.include = filter_same_as_default_include(&specs.validated_includes);
            // Go's validateSpecs appends to a nil slice, so no valid exclude
            // spec is a nil list.
            config.exclude =
                (!specs.validated_excludes.is_empty()).then(|| specs.validated_excludes.clone());
        }
    }
    if parsed.compile_on_save == Some(true) {
        config.compile_on_save = Some(true);
    }
    config
}
