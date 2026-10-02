use crate::{tsc, unsupported};
use std::sync::Arc;
use tsc::{CommandLineResult, CommandLineTesting, ExitStatus, System};
use tsr_ast::Diagnostic;
use tsr_compiler::{CompilerConfigHost, Error};
use tsr_core::collections::OrderedMap;
use tsr_diagnostics as d;
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigValue, ParsedCommandLine};

fn finished(status: ExitStatus) -> CommandLineResult {
    CommandLineResult {
        status,
        watcher: None,
    }
}

/// port: tsc/internal/execute/tsc.go:CommandLine
pub fn command_line(
    ctx: &Context,
    sys: Arc<dyn System>,
    args: &[JsString],
    testing: Option<Arc<dyn CommandLineTesting>>,
) -> CommandLineResult {
    let host =
        CompilerConfigHost::new_live(sys.fs(), JsString::from_bytes(sys.get_current_directory()));
    let result = if args.first().is_some_and(|arg| {
        matches!(
            tsr_jsstring::helpers::to_lower_go(arg.as_bytes()).as_slice(),
            b"-b" | b"--b" | b"-build" | b"--build"
        )
    }) {
        let command = tsr_tsoptions::parse_build_command_line(args, &host);
        build_compilation(ctx, sys, command, testing)
    } else {
        let command = tsr_tsoptions::parse_command_line(args, &host);
        compilation(ctx, sys, command, testing)
    };
    match result {
        Ok(result) => result,
        Err(Error::Unsupported(reason)) => unsupported(reason),
        Err(Error::Host(tsr_vfs::Error::Unsupported(reason))) => unsupported(reason),
        Err(error) => panic!("command-line compilation failed: {error}"),
    }
}

fn profiling_warning(sys: &dyn System, directory: &JsString) {
    if !directory.is_empty() {
        // Phase 4 decision 6: accepted flag, explicit limitation until Phase 7.
        tsc::write_all(
            sys.writer().as_ref(),
            b"Warning: profiling is not available in this build.\n",
        );
    }
}

/// port: tsc/internal/execute/tsc.go:tscBuildCompilation
fn build_compilation(
    ctx: &Context,
    sys: Arc<dyn System>,
    command: tsr_tsoptions::ParsedBuildCommandLine,
    testing: Option<Arc<dyn CommandLineTesting>>,
) -> Result<CommandLineResult, Error> {
    let locale = command.locale().clone();
    let report = tsc::create_diagnostic_reporter(
        sys.as_ref(),
        sys.writer(),
        locale.clone(),
        &command.compiler_options,
    );
    let sources = ParsedCommandLine::new(command.compiler_options.clone(), Vec::new());
    if !command.errors.is_empty() {
        for diagnostic in &command.errors {
            report.report(&sources, diagnostic)?;
        }
        return Ok(finished(ExitStatus::DiagnosticsPresent_OutputsSkipped));
    }
    profiling_warning(sys.as_ref(), &command.compiler_options.pprof_dir);
    if command.compiler_options.help.is_true() {
        tsc::help::print_version(sys.as_ref(), &locale);
        tsc::help::print_build_help(sys.as_ref(), &locale, tsr_tsoptions::BUILD_OPTIONS);
        return Ok(finished(ExitStatus::Success));
    }
    tsr_build::start(
        ctx,
        tsr_build::Options {
            sys,
            command,
            testing,
        },
    )
}

/// port: tsc/internal/execute/tsc.go:tscCompilation
fn compilation(
    ctx: &Context,
    sys: Arc<dyn System>,
    command: ParsedCommandLine,
    testing: Option<Arc<dyn CommandLineTesting>>,
) -> Result<CommandLineResult, Error> {
    let locale = command.locale().clone();
    let options = &command.options;
    let reporter =
        tsc::create_diagnostic_reporter(sys.as_ref(), sys.writer(), locale.clone(), options);
    if !command.errors.is_empty() {
        for diagnostic in &command.errors {
            reporter.report(&command, diagnostic)?;
        }
        return Ok(finished(ExitStatus::DiagnosticsPresent_OutputsSkipped));
    }
    profiling_warning(sys.as_ref(), &options.pprof_dir);
    if options.init.is_true() {
        let errors = std::cell::RefCell::new(None);
        tsc::init::write_config_file(
            sys.as_ref(),
            &locale,
            &|diagnostic| {
                if let Err(error) = reporter.report(&command, diagnostic) {
                    *errors.borrow_mut() = Some(error);
                }
            },
            command.raw.as_object().expect("command line raw options"),
        );
        if let Some(error) = errors.into_inner() {
            return Err(error);
        }
        return Ok(finished(ExitStatus::Success));
    }
    if options.version.is_true() {
        tsc::help::print_version(sys.as_ref(), &locale);
        return Ok(finished(ExitStatus::Success));
    }
    if options.help.is_true() || options.all.is_true() {
        tsc::help::print_help(sys.as_ref(), &locale, &command);
        return Ok(finished(ExitStatus::Success));
    }
    let report_error = |message, args| -> Result<CommandLineResult, Error> {
        reporter.report(&command, &Diagnostic::compiler(message, args))?;
        Ok(finished(ExitStatus::DiagnosticsPresent_OutputsSkipped))
    };
    if options.watch.is_true() && options.list_files_only.is_true() {
        return report_error(
            d::Options_0_and_1_cannot_be_combined,
            vec![
                JsString::from_bytes(b"watch".as_slice()),
                JsString::from_bytes(b"listFilesOnly".as_slice()),
            ],
        );
    }
    let fs = sys.fs();
    let mut config_name = Vec::new();
    if !options.project.is_empty() {
        if !command.root_file_names.is_empty() {
            return report_error(
                d::Option_project_cannot_be_mixed_with_source_files_on_a_command_line,
                vec![],
            );
        }
        let path = tsr_tspath::normalize(options.project.as_bytes());
        if fs.directory_exists(&path)? {
            config_name = tsr_tspath::combine(&path, &[b"tsconfig.json"]);
            if !fs.file_exists(&config_name)? {
                return report_error(
                    d::Cannot_find_a_tsconfig_json_file_at_the_current_directory_Colon_0,
                    vec![JsString::from_bytes(config_name.as_slice())],
                );
            }
        } else {
            config_name = path.into_owned();
            if !fs.file_exists(&config_name)? {
                return report_error(
                    d::The_specified_path_does_not_exist_Colon_0,
                    vec![JsString::from_bytes(config_name.as_slice())],
                );
            }
        }
    } else if !options.ignore_config.is_true() || command.root_file_names.is_empty() {
        config_name = find_config_file(
            &tsr_tspath::normalize(sys.get_current_directory()),
            |path| fs.file_exists(path).unwrap_or(false),
            b"tsconfig.json",
        );
        if !command.root_file_names.is_empty() {
            if !config_name.is_empty() {
                return report_error(d::X_tsconfig_json_is_present_but_will_not_be_loaded_if_files_are_specified_on_commandline_Use_ignoreConfig_to_skip_this_error, vec![]);
            }
        } else if config_name.is_empty() {
            if options.show_config.is_true() {
                return report_error(
                    d::Cannot_find_a_tsconfig_json_file_at_the_current_directory_Colon_0,
                    vec![JsString::from_bytes(
                        tsr_tspath::normalize(sys.get_current_directory()).as_ref(),
                    )],
                );
            }
            tsc::help::print_version(sys.as_ref(), &locale);
            tsc::help::print_help(sys.as_ref(), &locale, &command);
            return Ok(finished(ExitStatus::DiagnosticsPresent_OutputsSkipped));
        }
    }
    let show_config = options.show_config.is_true();
    let compiler_options_from_command_line = command.options.clone();
    let command_line_raw = command.raw.clone();
    let host = CompilerConfigHost::new_live(fs, JsString::from_bytes(sys.get_current_directory()));
    let cache = tsr_tsoptions::ExtendedConfigCache::new(&host);
    let mut times = tsc::CompileTimes::default();
    let config = if config_name.is_empty() {
        command
    } else {
        let start = sys.now();
        let mut raw = OrderedMap::default();
        raw.insert(
            JsString::from_bytes(b"compilerOptions".as_slice()),
            command.raw.clone(),
        );
        let result =
            cache.read_config_file(&config_name, &command.options, &ConfigValue::Object(raw))?;
        times.config_time = tsc::elapsed(sys.now(), start);
        if !result.read_errors.is_empty() {
            for diagnostic in &result.read_errors {
                reporter.report(&command, diagnostic)?;
            }
            return Ok(finished(ExitStatus::DiagnosticsPresent_OutputsGenerated));
        }
        result.command_line.expect("successful configuration read")
    };
    if show_config {
        show_configuration(sys.as_ref(), &config, &config_name);
        return Ok(finished(ExitStatus::Success));
    }
    if config.options.watch.is_true() {
        return tsc::watcher::start(
            ctx,
            tsc::watcher::Options {
                sys,
                config,
                compiler_options_from_command_line,
                command_line_raw,
                report_diagnostic: reporter,
                testing,
            },
        );
    }
    crate::compile::perform_compilation(ctx, sys, config, reporter, times, testing)
}

/// port: tsc/internal/execute/tsc.go:findConfigFile
pub fn find_config_file(
    search_path: &[u8],
    mut file_exists: impl FnMut(&[u8]) -> bool,
    config_name: &[u8],
) -> Vec<u8> {
    tsr_tspath::for_each_ancestor_directory(search_path, |ancestor| {
        let path = tsr_tspath::combine(ancestor, &[config_name]);
        let found = file_exists(&path);
        (path, found)
    })
    .unwrap_or_default()
}

/// port: tsc/internal/execute/tsc.go:showConfig
fn show_configuration(sys: &dyn System, config: &ParsedCommandLine, name: &[u8]) {
    let value = tsr_tsoptions::show_config::convert_to_ts_config(config, name);
    if let Ok(bytes) = tsr_json::marshal_indent(&value, "", "    ") {
        tsc::write_all(sys.writer().as_ref(), &bytes);
    }
}
