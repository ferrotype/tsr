use super::*;
use crate::{SharedWriter, Writer};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tsr_contentmapper::{SpawnError, Spawner};
use tsr_core::{CompilerOptions, Tristate};
use tsr_ipc::Stream;
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigValue, ParseConfigHost};
use tsr_vfs::{iofs::Time, FileSystem, MemoryBuilder, MemorySnapshot};

#[derive(Default)]
struct Buffer(Mutex<Vec<u8>>);
impl Writer for Buffer {
    fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
}
struct Sys {
    output: Arc<Buffer>,
    fs: Arc<MemorySnapshot>,
    width: i64,
    tty: bool,
    env: BTreeMap<String, JsString>,
}
impl Default for Sys {
    fn default() -> Self {
        Self {
            output: Arc::default(),
            fs: Arc::new(MemoryBuilder::new(b"/home/src/workspaces/project", true).finish()),
            width: 0,
            tty: true,
            env: BTreeMap::new(),
        }
    }
}
impl Sys {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.output.0.lock().unwrap())
    }
}
impl Spawner for Sys {
    fn spawn(
        &self,
        _: &[JsString],
        _: &[u8],
        _: Box<dyn std::io::Write + Send>,
    ) -> Result<Stream, SpawnError> {
        panic!("help and init do not spawn")
    }
}
impl System for Sys {
    fn writer(&self) -> SharedWriter {
        self.output.clone()
    }
    fn error_writer(&self) -> SharedWriter {
        self.output.clone()
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
    fn default_library_path(&self) -> &[u8] {
        b"/lib"
    }
    fn get_current_directory(&self) -> &[u8] {
        b"/home/src/workspaces/project"
    }
    fn write_output_is_tty(&self) -> bool {
        self.tty
    }
    fn get_width_of_terminal(&self) -> i64 {
        self.width
    }
    fn get_environment_variable(&self, name: &str) -> Option<JsString> {
        self.env.get(name).cloned()
    }
    fn now(&self) -> Time {
        panic!("help and init do not read the clock")
    }
    fn since_start(&self) -> Duration {
        panic!("help and init do not read the clock")
    }
}
impl ParseConfigHost for Sys {
    fn fs(&self) -> &dyn FileSystem {
        &*self.fs
    }
    fn current_directory(&self) -> &[u8] {
        self.get_current_directory()
    }
    fn resolve_config(&self, _: &[u8], _: &[u8]) -> Result<Option<JsString>, tsr_vfs::Error> {
        panic!("command-line parse does not resolve config")
    }
    fn resolve_content_mapper(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        panic!("command-line parse does not resolve content mappers")
    }
}

fn baseline(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../upstream/tsc/testdata/baselines/reference/tsc/commandLine")
        .join(format!("{name}.js"));
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn help_matches_pinned_easy_all_and_wide_terminal_outputs() {
    for (name, all, width, version) in [
        ("help", false, 0, false),
        ("help-all", true, 0, false),
        (
            "show-help-with-ExitStatus.DiagnosticsPresent_OutputsSkipped",
            false,
            120,
            true,
        ),
        (
            "show-help-with-ExitStatus.DiagnosticsPresent_OutputsSkipped-when-host-cannot-provide-terminal-width",
            false,
            0,
            true,
        ),
    ] {
        let sys = Sys {
            width,
            ..Default::default()
        };
        let options = CompilerOptions {
            all: if all {
                Tristate::TRUE
            } else {
                Tristate::UNKNOWN
            },
            ..Default::default()
        };
        if version {
            print_version(&sys, &Locale::default());
        }
        print_help(
            &sys,
            &Locale::default(),
            &ParsedCommandLine::new(options, vec![]),
        );
        let actual = String::from_utf8(sys.take())
            .unwrap()
            .replace(tsr_core::version(), "FakeTSVersion");
        let expected = baseline(name);
        // The runner appends one transcript separator after the console bytes.
        let expected = expected
            .split_once("Output::\n")
            .unwrap()
            .1
            .strip_suffix('\n')
            .unwrap();
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn init_matches_pinned_defaults_overrides_lists_aliases_and_extra_order() {
    let sys = Sys::default();
    let cases: &[(&str, &[&str])] = &[
        (
            "files-options",
            &["--init", "file0.st", "file1.ts", "file2.ts"],
        ),
        (
            "boolean-value-compiler-options",
            &["--init", "--noUnusedLocals"],
        ),
        (
            "enum-value-compiler-options",
            &["--init", "--target", "es5", "--jsx", "react"],
        ),
        (
            "list-compiler-options",
            &["--init", "--types", "jquery,mocha"],
        ),
        (
            "list-compiler-options-with-enum-value",
            &["--init", "--lib", "es5,es2015.core"],
        ),
        (
            "advanced-options",
            &[
                "--init",
                "--declaration",
                "--declarationDir",
                "lib",
                "--skipLibCheck",
                "--noErrorTruncation",
            ],
        ),
        ("--help", &["--init", "--help"]),
    ];
    for (name, args) in cases {
        let args: Vec<_> = args
            .iter()
            .map(|arg| JsString::from_bytes(arg.as_bytes()))
            .collect();
        let parsed = tsr_tsoptions::parse_command_line(&args, &sys);
        let actual =
            crate::init::generate_tsconfig(parsed.raw.as_object().unwrap(), &Locale::default());
        let fixture = baseline(&format!("Initialized-TSConfig-with-{name}"));
        let expected = fixture
            .split_once(" *new* \n")
            .unwrap()
            .1
            .strip_suffix("\n\n")
            .unwrap();
        assert_eq!(String::from_utf8(actual).unwrap(), expected, "{name}");
    }
}

#[test]
fn colors_follow_windows_terminal_rich_color_and_disabled_output_rules() {
    let mut sys = Sys::default();
    sys.env
        .insert("OS".into(), JsString::from_bytes(b"WINDOWS_NT".as_slice()));
    assert_eq!(create_colors(&sys).blue(b"x"), b"\x1b[97mx\x1b[39m");
    for (name, value) in [("WT_SESSION", "session"), ("TERM_PROGRAM", "vscode")] {
        sys.env
            .insert(name.into(), JsString::from_bytes(value.as_bytes()));
        assert_eq!(create_colors(&sys).blue(b"x"), b"\x1b[94mx\x1b[39m");
        sys.env.remove(name);
    }
    for (name, value) in [("COLORTERM", "truecolor"), ("TERM", "xterm-256color")] {
        sys.env
            .insert(name.into(), JsString::from_bytes(value.as_bytes()));
        assert_eq!(
            create_colors(&sys).blue_background(b"x"),
            b"\x1b[48;5;68mx\x1b[39;49m"
        );
        sys.env.remove(name);
    }
    assert_eq!(
        create_colors(&sys).blue_background(b"x"),
        b"\x1b[44mx\x1b[39;49m"
    );
    sys.tty = false;
    assert_eq!(create_colors(&sys).bold(b"\xff"), b"\xff");
    assert_eq!(create_colors(&sys).blue_background(b"x"), b"x");
}

#[test]
fn localized_help_wraps_by_bytes_and_header_padding_counts_runes() {
    let sys = Sys {
        tty: false,
        width: 8,
        ..Default::default()
    };
    assert_eq!(
        get_header(&sys, "é".as_bytes()),
        "é       \n     TS \n".as_bytes()
    );
    assert_eq!(
        get_pretty_output(&create_colors(&sys), b"x", "éa".as_bytes(), 1, 2, 3, false),
        b"x \xc3\n  \xa9\n  a\n"
    );
    let locale = Locale::parse("de-DE").0;
    let help = print_easy_help(
        &Sys::default(),
        &locale,
        &get_options_for_help(&ParsedCommandLine::new(CompilerOptions::default(), vec![])),
    );
    assert!(help.windows(b"--help".len()).any(|part| part == b"--help"));
    assert!(help
        .windows(d::Print_this_message.localize(&locale, &[]).len())
        .any(|part| part == d::Print_this_message.localize(&locale, &[])));
}

#[test]
fn init_keeps_existing_file_and_reports_its_normalized_path() {
    let mut fs = MemoryBuilder::new(b"/home/src/workspaces/project", true);
    fs.insert_loaded(b"tsconfig.json", b"existing".as_slice());
    let sys = Sys {
        fs: Arc::new(fs.finish()),
        ..Default::default()
    };
    let reported = Mutex::new(Vec::new());
    crate::init::write_config_file(
        &sys,
        &Locale::default(),
        &|diagnostic| reported.lock().unwrap().push(diagnostic.clone()),
        &tsr_core::collections::OrderedMap::default(),
    );
    assert!(sys.take().is_empty());
    let diagnostics = reported.into_inner().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        d::A_tsconfig_json_file_is_already_defined_at_Colon_0.code
    );
    assert_eq!(
        diagnostics[0].message_args,
        vec![JsString::from_bytes(
            b"/home/src/workspaces/project/tsconfig.json".as_slice()
        )]
    );
}

#[test]
#[should_panic(expected = "No matching value of 99")]
fn init_rejects_untyped_numeric_enum_values() {
    let mut options = tsr_core::collections::OrderedMap::default();
    options.insert(
        JsString::from_bytes(b"target".as_slice()),
        ConfigValue::Integer(99),
    );
    crate::init::generate_tsconfig(&options, &Locale::default());
}
