//! Command-line diagnostic and status reporting. Reporters retain presentation
//! settings, while each call borrows the owner of its diagnostic source files.
use crate::{write_all, CommandLineTesting, SharedWriter, System};
use tsr_ast::Diagnostic;
use tsr_compiler::diagnostic_writer::{
    try_clear_screen, DiagnosticSources, DiagnosticWriter, FormattingOptions,
};
use tsr_compiler::Error;
use tsr_core::CompilerOptions;
use tsr_locale::Locale;
use tsr_vfs::iofs::Time;

/// port: tsc/internal/execute/tsc/diagnostics.go:getFormatOptsOfSys
pub fn get_format_opts_of_sys(sys: &dyn System, locale: Locale) -> FormattingOptions {
    FormattingOptions {
        new_line: b"\n".to_vec(),
        current_directory: sys.get_current_directory().to_vec(),
        case_sensitive: sys.fs().use_case_sensitive_file_names(),
        locale,
    }
}

/// `FORCE_COLOR` takes precedence even when its value disables color. Values
/// are exact bytes: an explicitly empty value enables it; absence falls through.
/// port: tsc/internal/execute/tsc/diagnostics.go:defaultIsPretty
pub fn default_is_pretty(sys: &dyn System) -> bool {
    if let Some(force_color) = sys.get_environment_variable("FORCE_COLOR") {
        return matches!(force_color.as_bytes(), b"" | b"1" | b"2" | b"3" | b"true");
    }
    if sys
        .get_environment_variable("NO_COLOR")
        .is_some_and(|value| !value.as_bytes().is_empty())
    {
        return false;
    }
    if sys
        .get_environment_variable("TERM")
        .is_some_and(|value| value.as_bytes() == b"dumb")
    {
        return false;
    }
    sys.write_output_is_tty()
}

/// port: tsc/internal/execute/tsc/diagnostics.go:shouldBePretty
pub fn should_be_pretty(sys: &dyn System, options: Option<&CompilerOptions>) -> bool {
    match options {
        Some(options) if !options.pretty.is_unknown() => options.pretty.is_true(),
        _ => default_is_pretty(sys),
    }
}

/// port: tsc/internal/execute/tsc/diagnostics.go:QuietDiagnosticReporter
pub fn quiet_diagnostic_reporter(_: &Diagnostic) {}

/// port: tsc/internal/execute/tsc/diagnostics.go:QuietDiagnosticsReporter
pub fn quiet_diagnostics_reporter(_: &[Diagnostic]) {}

pub struct DiagnosticReporter {
    writer: SharedWriter,
    formatting: Option<FormattingOptions>,
    pretty: bool,
}

impl DiagnosticReporter {
    /// The source provider must retain the diagnostic's actual AST owner.
    /// A foreign or retired source is an error, never a guessed file path.
    pub fn report(
        &self,
        sources: &dyn DiagnosticSources,
        diagnostic: &Diagnostic,
    ) -> Result<(), Error> {
        let Some(formatting) = &self.formatting else {
            quiet_diagnostic_reporter(diagnostic);
            return Ok(());
        };
        let mut formatter = DiagnosticWriter::from_sources(sources, formatting.clone());
        let mut bytes = formatter.format(&[diagnostic], self.pretty)?;
        if self.pretty {
            bytes.extend_from_slice(&formatting.new_line);
        }
        write_all(self.writer.as_ref(), &bytes);
        Ok(())
    }
}

/// port: tsc/internal/execute/tsc/diagnostics.go:CreateDiagnosticReporter
pub fn create_diagnostic_reporter(
    sys: &dyn System,
    writer: SharedWriter,
    locale: Locale,
    options: &CompilerOptions,
) -> DiagnosticReporter {
    if options.quiet.is_true() {
        return DiagnosticReporter {
            writer,
            formatting: None,
            pretty: false,
        };
    }
    DiagnosticReporter {
        writer,
        formatting: Some(get_format_opts_of_sys(sys, locale)),
        pretty: should_be_pretty(sys, Some(options)),
    }
}

pub struct DiagnosticsReporter<'a> {
    sys: &'a dyn System,
    formatting: Option<FormattingOptions>,
}

impl DiagnosticsReporter<'_> {
    pub fn report(
        &self,
        sources: &dyn DiagnosticSources,
        diagnostics: &[Diagnostic],
    ) -> Result<(), Error> {
        let Some(formatting) = &self.formatting else {
            quiet_diagnostics_reporter(diagnostics);
            return Ok(());
        };
        let mut formatter = DiagnosticWriter::from_sources(sources, formatting.clone());
        let diagnostics: Vec<_> = diagnostics.iter().collect();
        let bytes = formatter.error_summary(&diagnostics)?;
        write_all(self.sys.writer().as_ref(), &bytes);
        Ok(())
    }
}

/// Summary output depends on pretty mode, independently of `quiet`.
/// port: tsc/internal/execute/tsc/diagnostics.go:CreateReportErrorSummary
pub fn create_report_error_summary<'a>(
    sys: &'a dyn System,
    locale: Locale,
    options: &CompilerOptions,
) -> DiagnosticsReporter<'a> {
    DiagnosticsReporter {
        sys,
        formatting: should_be_pretty(sys, Some(options))
            .then(|| get_format_opts_of_sys(sys, locale)),
    }
}

enum StatusDestination<'a> {
    Builder(SharedWriter),
    Watch(&'a CompilerOptions),
}

pub struct StatusReporter<'a> {
    sys: &'a dyn System,
    destination: StatusDestination<'a>,
    testing: Option<&'a dyn CommandLineTesting>,
    formatting: Option<FormattingOptions>,
    pretty: bool,
}

struct StatusEnd<'a> {
    testing: Option<&'a dyn CommandLineTesting>,
    builder_writer: Option<&'a SharedWriter>,
}

impl Drop for StatusEnd<'_> {
    fn drop(&mut self) {
        if let Some(testing) = self.testing {
            match self.builder_writer {
                Some(writer) => testing.on_build_status_report_end(writer),
                None => testing.on_watch_status_report_end(),
            }
        }
    }
}

impl StatusReporter<'_> {
    pub fn report(
        &self,
        sources: &dyn DiagnosticSources,
        diagnostic: &Diagnostic,
    ) -> Result<(), Error> {
        let Some(formatting) = &self.formatting else {
            quiet_diagnostic_reporter(diagnostic);
            return Ok(());
        };
        let watch_writer;
        let (writer, builder_writer) = match &self.destination {
            StatusDestination::Builder(writer) => (writer, Some(writer)),
            StatusDestination::Watch(_) => {
                watch_writer = self.sys.writer();
                (&watch_writer, None)
            }
        };
        if let Some(testing) = self.testing {
            match builder_writer {
                Some(writer) => testing.on_build_status_report_start(writer),
                None => testing.on_watch_status_report_start(),
            }
        }
        // The pin defers the end hook immediately after the start hook.
        let _end = StatusEnd {
            testing: self.testing,
            builder_writer,
        };
        if let StatusDestination::Watch(options) = &self.destination {
            let mut clear = Vec::new();
            try_clear_screen(&mut clear, diagnostic, options);
            write_all(writer.as_ref(), &clear);
        }
        let time = self.sys.format_watch_time(self.sys.now());
        let formatter = DiagnosticWriter::from_sources(sources, formatting.clone());
        let mut bytes = formatter.status(diagnostic, &time, self.pretty)?;
        bytes.extend_from_slice(&formatting.new_line);
        bytes.extend_from_slice(&formatting.new_line);
        write_all(writer.as_ref(), &bytes);
        Ok(())
    }
}

/// port: tsc/internal/execute/tsc/diagnostics.go:CreateBuilderStatusReporter
pub fn create_builder_status_reporter<'a>(
    sys: &'a dyn System,
    writer: SharedWriter,
    locale: Locale,
    options: &CompilerOptions,
    testing: Option<&'a dyn CommandLineTesting>,
) -> StatusReporter<'a> {
    let quiet = options.quiet.is_true();
    StatusReporter {
        sys,
        destination: StatusDestination::Builder(writer),
        testing,
        formatting: (!quiet).then(|| get_format_opts_of_sys(sys, locale)),
        pretty: !quiet && should_be_pretty(sys, Some(options)),
    }
}

/// Watch status deliberately does not consult `quiet`.
/// port: tsc/internal/execute/tsc/diagnostics.go:CreateWatchStatusReporter
pub fn create_watch_status_reporter<'a>(
    sys: &'a dyn System,
    locale: Locale,
    options: &'a CompilerOptions,
    testing: Option<&'a dyn CommandLineTesting>,
) -> StatusReporter<'a> {
    StatusReporter {
        sys,
        destination: StatusDestination::Watch(options),
        testing,
        formatting: Some(get_format_opts_of_sys(sys, locale)),
        pretty: should_be_pretty(sys, Some(options)),
    }
}

/// Go's `03:04:05 PM` layout for a UTC fake-system clock. The OS system must
/// override `System::format_watch_time` to format its local clock location.
pub fn format_watch_time(time: Time) -> Vec<u8> {
    let seconds = time.unix().0.rem_euclid(86_400);
    let hour = seconds / 3_600;
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    format!(
        "{hour12:02}:{:02}:{:02} {}",
        seconds % 3_600 / 60,
        seconds % 60,
        if hour < 12 { "AM" } else { "PM" },
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Trace, Writer};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tsr_contentmapper::{SpawnError, Spawner};
    use tsr_core::{collections::SyncMap, TextRange, Tristate};
    use tsr_ipc::Stream;
    use tsr_jsstring::{JsString, SourceText};
    use tsr_tsoptions::{ParsedCommandLine, TsConfigSourceFile};
    use tsr_vfs::{FileSystem, MemoryBuilder};

    #[derive(Default)]
    struct Buffer(Mutex<Vec<u8>>);
    impl Writer for Buffer {
        fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
    }
    impl Buffer {
        fn take(&self) -> Vec<u8> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    struct Sys {
        output: Arc<Buffer>,
        error: Arc<Buffer>,
        env: BTreeMap<String, JsString>,
        tty: bool,
        queries_forbidden: bool,
        time_calls: AtomicUsize,
        local_time: Option<Vec<u8>>,
    }
    impl Default for Sys {
        fn default() -> Self {
            Self {
                output: Arc::default(),
                error: Arc::default(),
                env: BTreeMap::new(),
                tty: false,
                queries_forbidden: false,
                time_calls: AtomicUsize::new(0),
                local_time: None,
            }
        }
    }
    impl Spawner for Sys {
        fn spawn(
            &self,
            _: &[JsString],
            _: &[u8],
            _: Box<dyn std::io::Write + Send>,
        ) -> Result<Stream, SpawnError> {
            panic!("reporting does not spawn")
        }
    }
    impl System for Sys {
        fn writer(&self) -> SharedWriter {
            self.output.clone()
        }
        fn error_writer(&self) -> SharedWriter {
            self.error.clone()
        }
        fn fs(&self) -> Arc<dyn FileSystem> {
            assert!(!self.queries_forbidden, "quiet reporter queried filesystem");
            Arc::new(MemoryBuilder::new(b"/work", true).finish())
        }
        fn default_library_path(&self) -> &[u8] {
            b"/lib"
        }
        fn get_current_directory(&self) -> &[u8] {
            assert!(!self.queries_forbidden, "quiet reporter queried cwd");
            b"/work"
        }
        fn write_output_is_tty(&self) -> bool {
            assert!(!self.queries_forbidden, "quiet reporter queried tty");
            self.tty
        }
        fn get_width_of_terminal(&self) -> i64 {
            80
        }
        fn get_environment_variable(&self, name: &str) -> Option<JsString> {
            assert!(
                !self.queries_forbidden,
                "quiet reporter queried environment"
            );
            self.env.get(name).cloned()
        }
        fn now(&self) -> Time {
            assert!(!self.queries_forbidden, "quiet reporter read clock");
            self.time_calls.fetch_add(1, Ordering::Relaxed);
            Time::from_unix(13 * 3600 + 2 * 60 + 3, 0)
        }
        fn format_watch_time(&self, time: Time) -> Vec<u8> {
            self.local_time
                .clone()
                .unwrap_or_else(|| format_watch_time(time))
        }
        fn since_start(&self) -> Duration {
            Duration::ZERO
        }
    }
    impl CommandLineTesting for Sys {
        fn on_emitted_files(
            &self,
            _: Option<&tsr_compiler::EmitResult>,
            _: Option<&SyncMap<JsString, Time>>,
        ) {
        }
        fn on_list_files_start(&self, _: &SharedWriter) {}
        fn on_list_files_end(&self, _: &SharedWriter) {}
        fn on_statistics_start(&self, _: &SharedWriter) {}
        fn on_statistics_end(&self, _: &SharedWriter) {}
        fn on_build_status_report_start(&self, writer: &SharedWriter) {
            write_all(writer.as_ref(), b"<build>\n");
        }
        fn on_build_status_report_end(&self, writer: &SharedWriter) {
            write_all(writer.as_ref(), b"</build>\n");
        }
        fn on_watch_status_report_start(&self) {
            write_all(self.output.as_ref(), b"<watch>\n");
        }
        fn on_watch_status_report_end(&self) {
            write_all(self.output.as_ref(), b"</watch>\n");
        }
        fn get_trace(&self, _: SharedWriter, _: Locale) -> Trace {
            Box::new(|_, _| {})
        }
        fn on_program(&self, _: &tsr_incremental::Program) {}
    }

    fn sources() -> ParsedCommandLine {
        ParsedCommandLine::new(CompilerOptions::default(), vec![])
    }
    fn diagnostic() -> Diagnostic {
        Diagnostic::compiler(
            tsr_diagnostics::Unknown_compiler_option_0,
            vec![JsString::from_bytes(b"unknown".as_slice())],
        )
    }

    #[test]
    fn pretty_environment_precedence_preserves_absent_empty_and_exact_values() {
        let mut sys = Sys::default();
        assert!(!default_is_pretty(&sys));
        sys.tty = true;
        assert!(default_is_pretty(&sys));
        sys.env.insert("NO_COLOR".into(), JsString::default());
        assert!(default_is_pretty(&sys));
        sys.env
            .insert("NO_COLOR".into(), JsString::from_bytes(b"1".as_slice()));
        assert!(!default_is_pretty(&sys));
        sys.env
            .insert("TERM".into(), JsString::from_bytes(b"dumb".as_slice()));
        for value in [b"".as_slice(), b"1", b"2", b"3", b"true"] {
            sys.env
                .insert("FORCE_COLOR".into(), JsString::from_bytes(value));
            assert!(default_is_pretty(&sys), "{value:?}");
        }
        for value in [b"0".as_slice(), b"false", b"4", b"TRUE", b" true", b"\xff"] {
            sys.env
                .insert("FORCE_COLOR".into(), JsString::from_bytes(value));
            assert!(!default_is_pretty(&sys), "{value:?}");
        }
        sys.env.remove("FORCE_COLOR");
        sys.env.remove("NO_COLOR");
        assert!(!default_is_pretty(&sys));
        sys.env
            .insert("TERM".into(), JsString::from_bytes(b"DUMB".as_slice()));
        assert!(default_is_pretty(&sys));
        for (pretty, expected) in [
            (Tristate::UNKNOWN, true),
            (Tristate::FALSE, false),
            (Tristate::TRUE, true),
            (Tristate(255), false),
        ] {
            let options = CompilerOptions {
                pretty,
                ..Default::default()
            };
            assert_eq!(should_be_pretty(&sys, Some(&options)), expected);
        }
        assert!(should_be_pretty(&sys, None));
        sys.env
            .insert("FORCE_COLOR".into(), JsString::from_bytes(b"0".as_slice()));
        assert!(should_be_pretty(
            &sys,
            Some(&CompilerOptions {
                pretty: Tristate::TRUE,
                ..Default::default()
            })
        ));
    }

    #[test]
    fn diagnostic_reporter_preserves_envelope_locale_and_trailing_newline() {
        let sys = Sys::default();
        let mut options = CompilerOptions::default();
        create_diagnostic_reporter(&sys, sys.error_writer(), Locale::default(), &options)
            .report(&sources(), &diagnostic())
            .unwrap();
        assert_eq!(
            sys.error.take(),
            b"error TS5023: Unknown compiler option 'unknown'.\n"
        );
        assert!(sys.output.take().is_empty());
        options.pretty = Tristate::TRUE;
        create_diagnostic_reporter(&sys, sys.error_writer(), Locale::parse("de-DE").0, &options)
            .report(&sources(), &diagnostic())
            .unwrap();
        assert_eq!(
            sys.error.take(),
            "\x1b[91merror\x1b[0m\x1b[90m TS5023: \x1b[0mUnbekannte Compileroption \"unknown\".\n"
                .as_bytes()
        );
    }

    #[test]
    fn reporter_resolves_only_the_supplied_source_owner_and_keeps_raw_bytes() {
        let sys = Sys::default();
        let options = CompilerOptions::default();
        let reporter = create_diagnostic_reporter(&sys, sys.writer(), Locale::default(), &options);
        let config = Arc::new(TsConfigSourceFile::parse(
            JsString::from_bytes(b"/work/config.json".as_slice()),
            JsString::from_bytes(b"/work/config.json".as_slice()),
            SourceText::from_bytes(b"{}\n{}".as_slice()),
        ));
        let diagnostic = Diagnostic::external(
            Some(config.root),
            TextRange::new(3, 4),
            JsString::default(),
            1,
            9999,
            JsString::from_bytes(b"raw\xff".as_slice()),
        );
        let mut owner = sources();
        owner.config_file = Some(config);
        reporter.report(&owner, &diagnostic).unwrap();
        assert_eq!(
            sys.output.take(),
            b"config.json(2,1): error TS9999: raw\xff\n"
        );
        assert!(reporter.report(&sources(), &diagnostic).is_err());
        assert!(sys.output.take().is_empty());
    }

    #[test]
    fn quiet_diagnostics_and_builder_status_do_not_query_or_format() {
        let sys = Sys {
            queries_forbidden: true,
            ..Default::default()
        };
        let options = CompilerOptions {
            quiet: Tristate::TRUE,
            ..Default::default()
        };
        let mut invalid = diagnostic();
        invalid.category = 999;
        create_diagnostic_reporter(&sys, sys.writer(), Locale::default(), &options)
            .report(&sources(), &invalid)
            .unwrap();
        create_builder_status_reporter(&sys, sys.writer(), Locale::default(), &options, Some(&sys))
            .report(&sources(), &invalid)
            .unwrap();
        assert!(sys.output.take().is_empty());
        assert_eq!(sys.time_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn summary_obeys_pretty_and_localization_even_when_quiet() {
        let sys = Sys::default();
        let mut options = CompilerOptions {
            quiet: Tristate::TRUE,
            ..Default::default()
        };
        create_report_error_summary(&sys, Locale::default(), &options)
            .report(&sources(), &[diagnostic()])
            .unwrap();
        assert!(sys.output.take().is_empty());
        options.pretty = Tristate::TRUE;
        create_report_error_summary(&sys, Locale::parse("de-DE").0, &options)
            .report(&sources(), &[diagnostic()])
            .unwrap();
        assert_eq!(sys.output.take(), "\n1 Fehler gefunden.\n\n".as_bytes());
    }

    #[test]
    fn watch_status_clear_screen_quiet_and_hooks_follow_pinned_order() {
        let sys = Sys::default();
        let start =
            Diagnostic::compiler(tsr_diagnostics::Starting_compilation_in_watch_mode, vec![]);
        for preserve in 0..4 {
            let options = CompilerOptions {
                quiet: Tristate::TRUE,
                preserve_watch_output: (preserve == 1).into(),
                diagnostics: (preserve == 2).into(),
                extended_diagnostics: (preserve == 3).into(),
                ..Default::default()
            };
            create_watch_status_reporter(&sys, Locale::default(), &options, Some(&sys))
                .report(&sources(), &start)
                .unwrap();
            let clear = if preserve == 0 {
                "\x1b[2J\x1b[3J\x1b[H"
            } else {
                ""
            };
            assert_eq!(sys.output.take(), format!("<watch>\n{clear}01:02:03 PM - Starting compilation in watch mode...\n\n</watch>\n").as_bytes());
        }
        assert_eq!(sys.time_calls.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn builder_status_uses_selected_writer_pretty_time_and_scoped_hooks() {
        let sys = Sys {
            local_time: Some(b"03:02:03 PM".to_vec()),
            ..Default::default()
        };
        let options = CompilerOptions {
            pretty: Tristate::TRUE,
            ..Default::default()
        };
        create_builder_status_reporter(
            &sys,
            sys.error_writer(),
            Locale::default(),
            &options,
            Some(&sys),
        )
        .report(&sources(), &diagnostic())
        .unwrap();
        assert_eq!(sys.error.take(), b"<build>\n[\x1b[90m03:02:03 PM\x1b[0m] Unknown compiler option 'unknown'.\n\n</build>\n");
        assert!(sys.output.take().is_empty());
        assert_eq!(sys.time_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn status_end_hook_runs_when_diagnostic_formatting_fails() {
        let sys = Sys::default();
        let options = CompilerOptions::default();
        let invalid = Diagnostic::external(
            None,
            TextRange::new(-1, -1),
            JsString::default(),
            3,
            tsr_diagnostics::Starting_compilation_in_watch_mode.code,
            JsString::default(),
        );
        assert!(
            create_watch_status_reporter(&sys, Locale::default(), &options, Some(&sys))
                .report(&sources(), &invalid)
                .is_err()
        );
        assert_eq!(
            sys.output.take(),
            b"<watch>\n\x1b[2J\x1b[3J\x1b[H</watch>\n"
        );
    }

    #[test]
    fn watch_time_matches_go_padded_twelve_hour_layout() {
        for (seconds, expected) in [
            (0, "12:00:00 AM"),
            (3600 + 2 * 60 + 3, "01:02:03 AM"),
            (12 * 3600, "12:00:00 PM"),
            (13 * 3600 + 2 * 60 + 3, "01:02:03 PM"),
            (-1, "11:59:59 PM"),
        ] {
            assert_eq!(
                format_watch_time(Time::from_unix(seconds, 0)),
                expected.as_bytes()
            );
        }
        assert_eq!(format_watch_time(Time::ZERO), b"12:00:00 AM");
    }
}
