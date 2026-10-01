//! Shared programs and observations of the Phase 3 contract suites
//! (`t1_contracts.rs` to `t8_contracts.rs`, docs/PHASE3-plan.md section 5):
//! in-memory programs loaded in either test-program mode, an emit through
//! `CheckedProgram::emit` with an in-memory write callback, and the parts of
//! an emit result two emits are compared on.
use std::sync::{Arc, Mutex};
use tsr_arena::Counters;
use tsr_checker::CheckerRequest;
use tsr_compiler::{
    CheckedProgram, EmitOptions, EmitResult, FileCache, Program, ProgramOptions, WriteFileData,
};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;

/// The global interfaces a `noLib` program needs for its checker to report
/// no global-type errors.
pub const LIB: &str = "interface Array<T> { length: number; [n: number]: T }\n\
interface Boolean {}\ninterface CallableFunction {}\ninterface Function {}\n\
interface IArguments {}\ninterface NewableFunction {}\ninterface Number {}\n\
interface Object {}\ninterface RegExp {}\ninterface String {}\n\
interface Promise<T> { then(): void }\ninterface PromiseConstructor {}\n\
declare var Promise: PromiseConstructor;\n\
interface TemplateStringsArray extends Array<string> {}\n";

/// The pin's two test-program modes (`TS_TEST_PROGRAM_SINGLE_THREADED`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Single,
    Concurrent,
}

impl Mode {
    pub const BOTH: [Mode; 2] = [Mode::Single, Mode::Concurrent];

    fn single_threaded(self) -> Tristate {
        match self {
            Mode::Single => Tristate::TRUE,
            Mode::Concurrent => Tristate::FALSE,
        }
    }
}

/// `files` (absolute names) as the program's root files, under `/`, with
/// `options` and no default library.
pub fn load(
    files: &[(String, String)],
    options: &CompilerOptions,
    mode: Mode,
    counters: &Counters,
) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let program = Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..options.clone()
                },
                files
                    .iter()
                    .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
                    .collect(),
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
            skip_module_resolution: false,
            single_threaded: mode.single_threaded(),
        },
        &mut FileCache::new(),
        counters,
    )
    .expect("the program loads");
    Arc::new(program)
}

/// [`load`] with the compiler's checker pool, in its own counter domain.
pub fn checked(
    files: &[(String, String)],
    options: &CompilerOptions,
    mode: Mode,
) -> (CheckedProgram, Counters) {
    let counters = Counters::new();
    let program = load(files, options, mode, &counters);
    (CheckedProgram::new(program, &counters, None), counters)
}

/// `(name, text)` pairs from string literals.
pub fn files(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|&(name, text)| (name.to_owned(), text.to_owned()))
        .collect()
}

/// What two emits are compared on: `EmitSkipped`, `EmittedFiles` in order,
/// the emit diagnostics in order (by file name, never by node identity),
/// the source maps in order and every written file, sorted by name because
/// the concurrent mode writes in completion order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub emit_skipped: bool,
    pub emitted_files: Vec<String>,
    pub diagnostics: Vec<String>,
    pub source_maps: Vec<(String, Vec<String>, tsr_sourcemap::RawSourceMap)>,
    pub written: Vec<(String, Vec<u8>)>,
}

impl Observed {
    /// The text written to `name`.
    pub fn text(&self, name: &str) -> &[u8] {
        &self
            .written
            .iter()
            .find(|(written, _)| written == name)
            .unwrap_or_else(|| panic!("nothing was written to {name}"))
            .1
    }
}

fn lossy(text: &JsString) -> String {
    String::from_utf8_lossy(text.as_bytes()).into_owned()
}

/// `result` and the files written while producing it, with diagnostic
/// files named through `program`.
pub fn observe(
    program: &Program,
    result: &EmitResult,
    mut written: Vec<(String, Vec<u8>)>,
) -> Observed {
    let file_name = |id| {
        program
            .files()
            .iter()
            .find(|file| file.source() == id)
            .map(|file| {
                let source = file.bound().view().source_file().expect("a parsed file");
                String::from_utf8_lossy(source.file_name()).into_owned()
            })
            .unwrap_or_else(|| "<foreign>".to_owned())
    };
    let diagnostics = result
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let args: Vec<String> = diagnostic.message_args.iter().map(lossy).collect();
            format!(
                "{}:{}:{} TS{} {args:?}",
                diagnostic.file.map(file_name).unwrap_or_default(),
                diagnostic.loc.pos(),
                diagnostic.loc.end(),
                diagnostic.code
            )
        })
        .collect();
    written.sort();
    Observed {
        emit_skipped: result.emit_skipped,
        emitted_files: result.emitted_files.iter().map(lossy).collect(),
        diagnostics,
        source_maps: result
            .source_maps
            .iter()
            .map(|map| {
                (
                    lossy(&map.generated_file),
                    map.input_source_file_names.iter().map(lossy).collect(),
                    map.source_map.clone(),
                )
            })
            .collect(),
        written,
    }
}

/// A write callback that records every write.
#[derive(Default)]
pub struct Recorder(pub Mutex<Vec<(String, Vec<u8>)>>);

impl Recorder {
    pub fn write(&self, name: &[u8], text: &[u8]) {
        self.0
            .lock()
            .expect("recorder")
            .push((String::from_utf8_lossy(name).into_owned(), text.to_vec()));
    }

    pub fn take(&self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut *self.0.lock().expect("recorder"))
    }
}

/// `CheckedProgram::emit` of every file with an in-memory write callback.
pub fn emit(
    checked: &CheckedProgram,
    request: &CheckerRequest,
) -> Result<Option<Observed>, tsr_compiler::Error> {
    let recorder = Recorder::default();
    let write_file = |name: &[u8], text: &[u8], _: &mut WriteFileData| {
        recorder.write(name, text);
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        ..EmitOptions::default()
    };
    let result = checked.emit(request, &options)?;
    Ok(result.map(|result| observe(checked.program(), &result, recorder.take())))
}

/// [`emit`] with the default request, which must produce a result.
pub fn emit_all(checked: &CheckedProgram) -> Observed {
    emit(checked, &CheckerRequest::default())
        .expect("the emit succeeds")
        .expect("an emit result")
}

/// The message of a caught panic.
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    "<non-string panic>".to_owned()
}

/// The stack the deep-input contracts emit on in the single-threaded mode:
/// the growth guards, not the stack's size, must carry the depth, as in the
/// C1 and C3 recursion contracts.
pub const SMALL_STACK: usize = 256 * 1024;

/// Runs `work` on a thread with `stack` bytes of stack.
pub fn on_stack<R: Send>(stack: usize, work: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("t-contracts".into())
            .stack_size(stack)
            .spawn_scoped(scope, work)
            .expect("a thread starts")
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
    })
}

/// Runs `work` on a thread with the stack a parallel work group reserves
/// (ADR 0011), as the emit's concurrent mode runs each file.
pub fn on_reserved_stack<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    on_stack(tsr_core::workgroup::RESERVED_STACK, work)
}

/// `files` emitted in both test-program modes, loaded on a reserved stack
/// (parsing and binding are not what these contracts stress): the
/// single-threaded emit on a thread with [`SMALL_STACK`], the concurrent one
/// on the work group's reserved stacks. The two emits must agree; the result
/// is the single-threaded one.
pub fn emit_deep(files: &[(String, String)], options: &CompilerOptions) -> Observed {
    let (single, _) = on_reserved_stack(|| checked(files, options, Mode::Single));
    let observed = on_stack(SMALL_STACK, || emit_all(&single));
    let (concurrent, _) = on_reserved_stack(|| checked(files, options, Mode::Concurrent));
    assert_eq!(
        emit_all(&concurrent),
        observed,
        "the concurrent emit differs from the single-threaded one"
    );
    observed
}

/// `count` copies of `unit` joined by `separator`.
pub fn repeat(unit: &str, count: usize, separator: &str) -> String {
    vec![unit; count].join(separator)
}

/// `unit(0)` to `unit(count - 1)`, concatenated.
pub fn numbered(count: usize, unit: impl Fn(usize) -> String) -> String {
    let mut text = String::new();
    for index in 0..count {
        text.push_str(&unit(index));
    }
    text
}

/// How often `needle` occurs in `text`.
pub fn occurrences(text: &[u8], needle: &str) -> usize {
    String::from_utf8_lossy(text).matches(needle).count()
}
