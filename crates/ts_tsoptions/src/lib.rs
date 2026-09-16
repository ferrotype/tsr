//! Loader configuration and the pinned option/library interpretation slice.
use ts_core::{CompilerOptions, ScriptTarget};
use ts_jsstring::JsString;
pub const LIB_MAP: &[(&str, &str)] = &[
    ("es5", "lib.es5.d.ts"),
    ("es6", "lib.es2015.d.ts"),
    ("es2015", "lib.es2015.d.ts"),
    ("es7", "lib.es2016.d.ts"),
    ("es2016", "lib.es2016.d.ts"),
    ("es2017", "lib.es2017.d.ts"),
    ("es2018", "lib.es2018.d.ts"),
    ("es2019", "lib.es2019.d.ts"),
    ("es2020", "lib.es2020.d.ts"),
    ("es2021", "lib.es2021.d.ts"),
    ("es2022", "lib.es2022.d.ts"),
    ("es2023", "lib.es2023.d.ts"),
    ("es2024", "lib.es2024.d.ts"),
    ("es2025", "lib.es2025.d.ts"),
    ("esnext", "lib.esnext.d.ts"),
    ("dom", "lib.dom.d.ts"),
    ("dom.iterable", "lib.dom.iterable.d.ts"),
    ("dom.asynciterable", "lib.dom.asynciterable.d.ts"),
    ("webworker", "lib.webworker.d.ts"),
    (
        "webworker.importscripts",
        "lib.webworker.importscripts.d.ts",
    ),
    ("webworker.iterable", "lib.webworker.iterable.d.ts"),
    (
        "webworker.asynciterable",
        "lib.webworker.asynciterable.d.ts",
    ),
    ("scripthost", "lib.scripthost.d.ts"),
    ("es2015.core", "lib.es2015.core.d.ts"),
    ("es2015.collection", "lib.es2015.collection.d.ts"),
    ("es2015.generator", "lib.es2015.generator.d.ts"),
    ("es2015.iterable", "lib.es2015.iterable.d.ts"),
    ("es2015.promise", "lib.es2015.promise.d.ts"),
    ("es2015.proxy", "lib.es2015.proxy.d.ts"),
    ("es2015.reflect", "lib.es2015.reflect.d.ts"),
    ("es2015.symbol", "lib.es2015.symbol.d.ts"),
    (
        "es2015.symbol.wellknown",
        "lib.es2015.symbol.wellknown.d.ts",
    ),
    ("es2016.array.include", "lib.es2016.array.include.d.ts"),
    ("es2016.intl", "lib.es2016.intl.d.ts"),
    ("es2017.arraybuffer", "lib.es2017.arraybuffer.d.ts"),
    ("es2017.date", "lib.es2017.date.d.ts"),
    ("es2017.object", "lib.es2017.object.d.ts"),
    ("es2017.sharedmemory", "lib.es2017.sharedmemory.d.ts"),
    ("es2017.string", "lib.es2017.string.d.ts"),
    ("es2017.intl", "lib.es2017.intl.d.ts"),
    ("es2017.typedarrays", "lib.es2017.typedarrays.d.ts"),
    ("es2018.asyncgenerator", "lib.es2018.asyncgenerator.d.ts"),
    ("es2018.asynciterable", "lib.es2018.asynciterable.d.ts"),
    ("es2018.intl", "lib.es2018.intl.d.ts"),
    ("es2018.promise", "lib.es2018.promise.d.ts"),
    ("es2018.regexp", "lib.es2018.regexp.d.ts"),
    ("es2019.array", "lib.es2019.array.d.ts"),
    ("es2019.object", "lib.es2019.object.d.ts"),
    ("es2019.string", "lib.es2019.string.d.ts"),
    ("es2019.symbol", "lib.es2019.symbol.d.ts"),
    ("es2019.intl", "lib.es2019.intl.d.ts"),
    ("es2020.bigint", "lib.es2020.bigint.d.ts"),
    ("es2020.date", "lib.es2020.date.d.ts"),
    ("es2020.promise", "lib.es2020.promise.d.ts"),
    ("es2020.sharedmemory", "lib.es2020.sharedmemory.d.ts"),
    ("es2020.string", "lib.es2020.string.d.ts"),
    (
        "es2020.symbol.wellknown",
        "lib.es2020.symbol.wellknown.d.ts",
    ),
    ("es2020.intl", "lib.es2020.intl.d.ts"),
    ("es2020.number", "lib.es2020.number.d.ts"),
    ("es2021.promise", "lib.es2021.promise.d.ts"),
    ("es2021.string", "lib.es2021.string.d.ts"),
    ("es2021.weakref", "lib.es2021.weakref.d.ts"),
    ("es2021.intl", "lib.es2021.intl.d.ts"),
    ("es2022.array", "lib.es2022.array.d.ts"),
    ("es2022.error", "lib.es2022.error.d.ts"),
    ("es2022.intl", "lib.es2022.intl.d.ts"),
    ("es2022.object", "lib.es2022.object.d.ts"),
    ("es2022.string", "lib.es2022.string.d.ts"),
    ("es2022.regexp", "lib.es2022.regexp.d.ts"),
    ("es2023.array", "lib.es2023.array.d.ts"),
    ("es2023.collection", "lib.es2023.collection.d.ts"),
    ("es2023.intl", "lib.es2023.intl.d.ts"),
    ("es2024.arraybuffer", "lib.es2024.arraybuffer.d.ts"),
    ("es2024.collection", "lib.es2024.collection.d.ts"),
    ("es2024.object", "lib.es2024.object.d.ts"),
    ("es2024.promise", "lib.es2024.promise.d.ts"),
    ("es2024.regexp", "lib.es2024.regexp.d.ts"),
    ("es2024.sharedmemory", "lib.es2024.sharedmemory.d.ts"),
    ("es2024.string", "lib.es2024.string.d.ts"),
    ("es2025.collection", "lib.es2025.collection.d.ts"),
    ("es2025.float16", "lib.es2025.float16.d.ts"),
    ("es2025.intl", "lib.es2025.intl.d.ts"),
    ("es2025.iterator", "lib.es2025.iterator.d.ts"),
    ("es2025.promise", "lib.es2025.promise.d.ts"),
    ("es2025.regexp", "lib.es2025.regexp.d.ts"),
    ("esnext.asynciterable", "lib.es2018.asynciterable.d.ts"),
    ("esnext.symbol", "lib.es2019.symbol.d.ts"),
    ("esnext.bigint", "lib.es2020.bigint.d.ts"),
    ("esnext.weakref", "lib.es2021.weakref.d.ts"),
    ("esnext.object", "lib.es2024.object.d.ts"),
    ("esnext.regexp", "lib.es2024.regexp.d.ts"),
    ("esnext.string", "lib.es2024.string.d.ts"),
    ("esnext.float16", "lib.es2025.float16.d.ts"),
    ("esnext.iterator", "lib.es2025.iterator.d.ts"),
    ("esnext.promise", "lib.es2025.promise.d.ts"),
    ("esnext.array", "lib.esnext.array.d.ts"),
    ("esnext.collection", "lib.esnext.collection.d.ts"),
    ("esnext.date", "lib.esnext.date.d.ts"),
    ("esnext.decorators", "lib.esnext.decorators.d.ts"),
    ("esnext.disposable", "lib.esnext.disposable.d.ts"),
    ("esnext.error", "lib.esnext.error.d.ts"),
    ("esnext.intl", "lib.esnext.intl.d.ts"),
    ("esnext.sharedmemory", "lib.esnext.sharedmemory.d.ts"),
    ("esnext.temporal", "lib.esnext.temporal.d.ts"),
    ("esnext.typedarrays", "lib.esnext.typedarrays.d.ts"),
    ("decorators", "lib.decorators.d.ts"),
    ("decorators.legacy", "lib.decorators.legacy.d.ts"),
];
/// port: tsc/internal/tsoptions/enummaps.go:GetLibFileName
pub fn lib_file_name(name: &[u8]) -> Option<&'static str> {
    let name = ts_tspath::file_name_lower_case(name);
    LIB_MAP.iter().find_map(|(key, file)| {
        (key.as_bytes() == name.as_ref() || file.as_bytes() == name.as_ref()).then_some(*file)
    })
}
/// port: tsc/internal/tsoptions/enummaps.go:GetDefaultLibFileName
pub fn default_lib_file_name(options: &CompilerOptions) -> &'static str {
    match options.emit_script_target() {
        ScriptTarget::ESNEXT => "lib.esnext.full.d.ts",
        ScriptTarget::ES2025 => "lib.es2025.full.d.ts",
        ScriptTarget::ES2024 => "lib.es2024.full.d.ts",
        ScriptTarget::ES2023 => "lib.es2023.full.d.ts",
        ScriptTarget::ES2022 => "lib.es2022.full.d.ts",
        ScriptTarget::ES2021 => "lib.es2021.full.d.ts",
        ScriptTarget::ES2020 => "lib.es2020.full.d.ts",
        ScriptTarget::ES2019 => "lib.es2019.full.d.ts",
        ScriptTarget::ES2018 => "lib.es2018.full.d.ts",
        ScriptTarget::ES2017 => "lib.es2017.full.d.ts",
        ScriptTarget::ES2016 => "lib.es2016.full.d.ts",
        ScriptTarget::ES2015 => "lib.es6.d.ts",
        _ => "lib.d.ts",
    }
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:GetSupportedExtensions
pub fn supported_extensions(options: &CompilerOptions, extra: &[JsString]) -> Vec<Vec<JsString>> {
    let builtins: &[&[&str]] = if options.allow_js() {
        &[
            &[".ts", ".tsx", ".d.ts", ".js", ".jsx"],
            &[".cts", ".d.cts", ".cjs"],
            &[".mts", ".d.mts", ".mjs"],
        ]
    } else {
        &[
            &[".ts", ".tsx", ".d.ts"],
            &[".cts", ".d.cts"],
            &[".mts", ".d.mts"],
        ]
    };
    let mut result: Vec<Vec<JsString>> = builtins
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|s| JsString::from_bytes(s.as_bytes()))
                .collect()
        })
        .collect();
    for extension in extra {
        if !builtins
            .iter()
            .flat_map(|group| group.iter())
            .any(|e| e.as_bytes() == extension.as_bytes())
        {
            result.push(vec![extension.clone()]);
        }
    }
    result
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:GetSupportedExtensionsWithJsonIfResolveJsonModule
pub fn supported_extensions_with_json(
    options: &CompilerOptions,
    extra: &[JsString],
) -> Vec<Vec<JsString>> {
    let mut result = supported_extensions(options, extra);
    if options.resolve_json_module() {
        result.push(vec![JsString::from_bytes(b".json".as_slice())]);
    }
    result
}
#[derive(Clone, Debug)]
pub struct ParsedCommandLine {
    pub options: CompilerOptions,
    pub root_file_names: Vec<JsString>,
    pub config_file: Option<std::sync::Arc<TsConfigSourceFile>>,
    pub errors: Vec<ts_ast::Diagnostic>,
    pub raw: ConfigValue,
    pub compile_on_save: Option<bool>,
    pub config_specs: Option<ConfigFileSpecs>,
    pub config_base_path: JsString,
    pub config_case_sensitive: bool,
    pub config_dependencies: Vec<std::sync::Arc<TsConfigSourceFile>>,
    pub type_acquisition: Option<TypeAcquisition>,
    pub project_references: Option<Vec<ProjectReference>>,
    pub literal_file_names_len: usize,
    pub content_mappers: Option<Vec<config_mappers::ContentMapper>>,
}
impl ParsedCommandLine {
    /// Syntax diagnostics precede option/conversion diagnostics, as in the
    /// native API; neither collection alone is the complete config phase.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.GetConfigFileParsingDiagnostics
    pub fn config_file_parsing_diagnostics(&self) -> Vec<ts_ast::Diagnostic> {
        let mut diagnostics = self.config_file.as_ref().map_or_else(Vec::new, |config| {
            config
                .file
                .view()
                .source_file(config.root)
                .expect("config source")
                .diagnostics
                .clone()
        });
        diagnostics.extend_from_slice(&self.errors);
        diagnostics
    }

    /// The program loader consumes an already interpreted configuration. Full
    /// tsconfig extends/include/project-reference interpretation is a separate API.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:NewParsedCommandLine
    pub fn new(options: CompilerOptions, root_file_names: Vec<JsString>) -> Self {
        Self {
            options,
            root_file_names,
            config_file: None,
            errors: Vec::new(),
            raw: ConfigValue::Null,
            compile_on_save: None,
            config_specs: None,
            config_base_path: JsString::default(),
            config_case_sensitive: true,
            config_dependencies: Vec::new(),
            type_acquisition: None,
            project_references: None,
            literal_file_names_len: 0,
            content_mappers: None,
        }
    }
}
pub mod raw;

mod config_value;
pub use config_value::ConfigValue;
mod config_syntax;
pub use config_syntax::{
    diagnostic_for_node, find_property, find_property_in_object, property_name, TsConfigSourceFile,
};

mod option_declarations;
pub use option_declarations::{
    find_declaration, option_declaration, EnumValue, OptionDeclaration, OptionKind, BUILD_OPTIONS,
    COMPILER_OPTIONS, ROOT_OPTIONS, TYPE_ACQUISITION_OPTIONS, WATCH_OPTIONS,
};
mod parse_options;
pub use parse_options::{
    parse_compiler_options, parse_number, parse_string, parse_string_array, parse_string_map,
    parse_tristate,
};
mod convert_options;
pub use convert_options::{
    compiler_options_from_json, convert_json_option, default_compiler_options, is_option_value,
    spec_diagnostic, OptionSyntax,
};

mod config_text;
pub use config_text::{
    convert_config_file_to_object, parse_config_file_text_to_json, ConfigText, PropertyNotifier,
};

mod config_specs;
pub mod glob;
pub use config_specs::ConfigFileSpecs;
mod config_files;
pub use config_files::file_names_from_specs;
mod config_substitution;
pub use config_substitution::{starts_with_config_dir, substitute_options, substitute_path};
mod merge_options;
pub use merge_options::merge_compiler_options;
mod config_host;
pub use config_host::ParseConfigHost;
mod config_parse;
pub use config_parse::{
    parse_json_source_file_config_file_content, ProjectReference, TypeAcquisition,
};

pub mod config_json;
pub use config_json::stringify_json;

mod options_value;
pub use options_value::compiler_options_value;
pub mod config_mappers;
mod fixture_options;
pub use fixture_options::{apply_fixture_settings, parse_list_type_option, FixtureOptionError};
mod config_read;
pub use config_read::{
    get_parsed_command_line_of_config_file, get_parsed_command_line_of_config_file_path,
    ReadConfigResult,
};
