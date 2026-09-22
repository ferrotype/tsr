//! Argument parsing only: no compiler execution, watch loop or build scheduling.
//! Raw values retain insertion order and relative paths. Converted options own
//! their values, following the approved compiler-options clone divergence.
mod worker;
use crate::{
    find_declaration, parse_build_options, parse_compiler_options, parse_watch_options,
    ConfigValue as V, ParseConfigHost, ParsedCommandLine, COMPILER_OPTIONS,
};
use tsr_ast::Diagnostic;
use tsr_core::{BuildOptions, CompilerOptions, WatchOptions};
use tsr_diagnostics as d;
use tsr_jsstring::JsString;

pub use worker::input_option_name;

#[derive(Clone, Debug)]
pub struct ParsedBuildCommandLine {
    pub build_options: BuildOptions,
    pub compiler_options: CompilerOptions,
    pub watch_options: WatchOptions,
    pub projects: Vec<JsString>,
    pub errors: Vec<Diagnostic>,
    pub raw: V,
    pub current_directory: JsString,
    pub case_sensitive: bool,
    locale: std::sync::OnceLock<tsr_locale::Locale>,
    resolved_project_paths: std::sync::OnceLock<Vec<JsString>>,
}

/// port: tsc/internal/tsoptions/commandlineparser.go:ParseCommandLine
pub fn parse_command_line(args: &[JsString], host: &dyn ParseConfigHost) -> ParsedCommandLine {
    let parsed = worker::parse(args, host, worker::Mode::Compiler(COMPILER_OPTIONS));
    let mut result = ParsedCommandLine::new(CompilerOptions::default(), parsed.files);
    let mut watch = WatchOptions::default();
    for (key, value) in &parsed.options {
        // Native conversion uses the compiler table only, even for watch
        // values. Watch exclusion lists therefore remain relative here.
        let converted = absolute_value(key.as_bytes(), value, host.current_directory());
        parse_compiler_options(key.as_bytes(), &converted, &mut result.options);
        parse_watch_options(key.as_bytes(), &converted, &mut watch);
    }
    result.watch_options = Some(watch);
    result.errors = parsed.errors;
    result.raw = V::Object(parsed.options);
    result.config_base_path = JsString::from_bytes(host.current_directory());
    result.config_case_sensitive = host.fs().use_case_sensitive_file_names();
    result
}

fn absolute_value<'a>(key: &[u8], value: &'a V, cwd: &[u8]) -> std::borrow::Cow<'a, V> {
    crate::convert_option_to_absolute_path(key, value, crate::compiler_option_name_map(), cwd)
        .map_or(std::borrow::Cow::Borrowed(value), std::borrow::Cow::Owned)
}

/// port: tsc/internal/tsoptions/commandlineparser.go:ParseBuildCommandLine
pub fn parse_build_command_line(
    args: &[JsString],
    host: &dyn ParseConfigHost,
) -> ParsedBuildCommandLine {
    let parsed = worker::parse(args, host, worker::Mode::Build);
    let mut result = ParsedBuildCommandLine {
        build_options: BuildOptions::default(),
        compiler_options: CompilerOptions::default(),
        watch_options: WatchOptions::default(),
        projects: parsed.files,
        errors: parsed.errors,
        raw: V::Object(parsed.options),
        current_directory: JsString::from_bytes(host.current_directory()),
        case_sensitive: host.fs().use_case_sensitive_file_names(),
        locale: std::sync::OnceLock::new(),
        resolved_project_paths: std::sync::OnceLock::new(),
    };
    for (key, value) in result.raw.as_object().expect("worker raw object") {
        // At the pin BuildOpts = commonOptionsWithBuild + OptionsForBuild;
        // every name shared with CompilerNameMap is the same declaration.
        // Watch-only names occur in neither table and are not compiler values.
        if key.as_bytes() == b"build"
            || find_declaration(COMPILER_OPTIONS, key.as_bytes(), false).is_some()
        {
            parse_compiler_options(key.as_bytes(), value, &mut result.compiler_options);
        }
        parse_build_options(key.as_bytes(), value, &mut result.build_options);
        parse_watch_options(key.as_bytes(), value, &mut result.watch_options);
    }
    if result.projects.is_empty() {
        result.projects.push(JsString::from_bytes(b".".as_slice()));
    }
    let build = &result.build_options;
    let watch = result.compiler_options.watch.is_true();
    for (invalid, first, second) in [
        (
            build.clean.is_true() && build.force.is_true(),
            "clean",
            "force",
        ),
        (
            build.clean.is_true() && build.verbose.is_true(),
            "clean",
            "verbose",
        ),
        (build.clean.is_true() && watch, "clean", "watch"),
        (watch && build.dry.is_true(), "watch", "dry"),
    ] {
        if invalid {
            result.errors.push(Diagnostic::compiler(
                d::Options_0_and_1_cannot_be_combined,
                vec![
                    JsString::from_bytes(first.as_bytes()),
                    JsString::from_bytes(second.as_bytes()),
                ],
            ));
        }
    }
    result
}

impl ParsedBuildCommandLine {
    /// Resolve lazily, retaining the first result just like the pinned once
    /// cache. Later edits to `projects` or `current_directory` do not reset it.
    /// port: tsc/internal/tsoptions/parsedbuildcommandline.go:ParsedBuildCommandLine.ResolvedProjectPaths
    pub fn resolved_project_paths(&self) -> &[JsString] {
        self.resolved_project_paths.get_or_init(|| {
            self.projects
                .iter()
                .map(|project| {
                    let path = tsr_tspath::resolve(
                        self.current_directory.as_bytes(),
                        &[project.as_bytes()],
                    );
                    crate::resolve_config_file_name_of_project_reference(&path)
                })
                .collect()
        })
    }

    /// As in Go, the first locale lookup freezes the value for this parse
    /// result. Updating options afterwards does not reset that result cache.
    /// port: tsc/internal/tsoptions/parsedbuildcommandline.go:ParsedBuildCommandLine.Locale
    pub fn locale(&self) -> &tsr_locale::Locale {
        self.locale.get_or_init(|| {
            // CLI validation admits only valid tags. Lossy decoding also
            // retains the usable prefix of manually supplied malformed tags.
            tsr_locale::Locale::parse(&String::from_utf8_lossy(
                self.compiler_options.locale.as_bytes(),
            ))
            .0
        })
    }
}

/// Test-only counterpart of the pinned export_test.go worker. Uses the same
/// parser body; declarations and expected results are supplied independently.
#[cfg(feature = "harness")]
pub fn parse_command_line_test_worker(
    args: &[JsString],
    host: &dyn ParseConfigHost,
    declarations: &'static [crate::OptionDeclaration],
) -> (V, Vec<JsString>, Vec<Diagnostic>) {
    let parsed = worker::parse(args, host, worker::Mode::Compiler(declarations));
    (V::Object(parsed.options), parsed.files, parsed.errors)
}
