//! `testrunner/compiler_runner.go`: one compiler test variant from its units
//! to its nine sub-tests, each compared with the committed reference through
//! `crate::baseline`.
//!
//! Sub-test names are the pin's `t.Run` names: `error`, `content mapper`,
//! `output`, `sourcemap`, `sourcemap record`, `type`, `symbol`, `module
//! resolution`, `union ordering`, `source file parent pointers`. A sub-test
//! that panics is a failing sub-test naming the panic (`RecoverAndFail`).
//!
//! The `tsbaseline.Do*Baseline` functions are split here: the text each one
//! composes comes from the writers in `crate::harness` (bytes in, bytes or
//! a [`Failure`] out), and the baseline name, the `baseline.Run` call and
//! the checks the pin makes around it are the small `do_*` functions below.
//! Where a writer starts a compilation of its own (`DoJSEmitBaseline`'s
//! declaration re-compilation and `noCheck` repeat), this module runs it
//! through `crate::compile` and hands the writer the outputs.
use crate::baseline::{self, Options, Roots, NO_CONTENT};
use crate::compile::{self, Compilation, CompileError, TestFile};
use crate::enumerate::Variant;
use crate::harness::baselines::{
    self as writers, DeclarationCompilationContext, DeclarationCompilationResult, Failure,
    JsEmitInput, JsonErrorBaseline, RepeatOutputs, SourcemapInput, SourcemapRecordInput,
    TestFile as FileView,
};
use crate::harness::{errors, subtests, typebaseline};
use crate::harness_options::{HarnessOptions, TestConfiguration};
use crate::result::{Outcome, Report};
use crate::test_case_parser::{TestCaseContent, TestUnit};
use crate::{Mode, Stop, TestData, SRC_FOLDER};
use serde_json::Value;
use std::borrow::Cow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use tsr_ast::{Diagnostic, NodeId};
use tsr_checker::Operation;
use tsr_compiler::{CompilerCheckerPool, Program};
use tsr_core::CompilerOptions;
use tsr_tsoptions::ParsedCommandLine;

const ERROR: &str = "error";
const CONTENT_MAPPER: &str = "content mapper";
const OUTPUT: &str = "output";
const SOURCEMAP: &str = "sourcemap";
const SOURCEMAP_RECORD: &str = "sourcemap record";
const TYPE: &str = "type";
const SYMBOL: &str = "symbol";
const MODULE_RESOLUTION: &str = "module resolution";
const UNION_ORDERING: &str = "union ordering";
const PARENT_POINTERS: &str = "source file parent pointers";

/// `requireStr`.
const REQUIRE: &[u8] = b"require(";

/// What `newCompilerTest` settles before compiling: the units split into
/// the files passed on the command line and the other files on the file
/// system, the tsconfig unit, the current directory, and the options that
/// `SkipUnsupportedCompilerOptions` reads. `list` stops here.
pub struct Prepared {
    pub variant: Variant,
    pub content: TestCaseContent,
    pub current_directory: Vec<u8>,
    pub ts_config_files: Vec<TestFile>,
    pub to_be_compiled: Vec<TestFile>,
    pub other_files: Vec<TestFile>,
    pub has_non_dts_files: bool,
    pub options: CompilerOptions,
    pub harness_options: HarnessOptions,
}

/// The pin's `compilerTest` after `newCompilerTest`
/// (`compiler_runner.go:compilerTest`).
pub struct CompilerTest {
    pub prepared: Prepared,
    pub result: Compilation,
}

/// `makeUnitsFromTest`, the input split of `newCompilerTest` (a tsconfig
/// decides the roots; otherwise the last unit alone is compiled when
/// `noImplicitReferences` is set or it uses `require(` or a
/// `reference path`, else every unit), and `resolve_options`. Returns
/// `Stop::Skip` when `SkipUnsupportedCompilerOptions` would skip the test
/// and `Stop::Fatal` for its fatal cases and the parser's panics.
///
/// Like the pin, a relative `baseUrl` of the configuration is made absolute
/// against the current directory in the variant's own configuration, which
/// `compile` then applies.
// port: tsc/internal/testrunner/compiler_runner.go:newCompilerTest
pub fn prepare(mut variant: Variant, content: &[u8]) -> Result<Prepared, Stop> {
    let filename = path_bytes(&variant.file);
    let test_content = crate::test_case_parser::make_units_from_test(content, &filename)?;

    let current_directory = tsr_tspath::absolute(
        variant
            .configuration
            .as_ref()
            .and_then(|named| named.config.get("currentdirectory"))
            .map_or("", String::as_str)
            .as_bytes(),
        SRC_FOLDER,
    );

    let units = &test_content.units;
    let has_non_dts_files = units
        .iter()
        .any(|unit| !tsr_tspath::file_extension_is(&unit.name, tsr_tspath::EXTENSION_DTS));
    let mut ts_config_files = Vec::new();
    let mut to_be_compiled = Vec::new();
    let mut other_files = Vec::new();
    if let Some(ts_config) = &test_content.ts_config {
        let unit = test_content.ts_config_unit.as_ref().ok_or_else(|| {
            Stop::fatal("the test case has a parsed tsconfig but no tsconfig unit")
        })?;
        ts_config_files.push(create_harness_test_file(unit, &current_directory));
        for unit in units {
            let name = tsr_tspath::absolute(&unit.name, &current_directory);
            if ts_config
                .root_file_names
                .iter()
                .any(|root| root.as_bytes() == name.as_slice())
            {
                to_be_compiled.push(create_harness_test_file(unit, &current_directory));
            } else {
                other_files.push(create_harness_test_file(unit, &current_directory));
            }
        }
    } else {
        if let Some(base_url) = variant
            .configuration
            .as_mut()
            .and_then(|named| named.config.get_mut("baseurl"))
        {
            if !tsr_tspath::is_rooted_disk_path(base_url.as_bytes()) {
                *base_url = String::from_utf8_lossy(&tsr_tspath::absolute(
                    base_url.as_bytes(),
                    &current_directory,
                ))
                .into_owned();
            }
        }

        // `units[len(units)-1]` panics on a test without units.
        let Some((last_unit, rest)) = units.split_last() else {
            return Err(Stop::fatal(format!(
                "Panic on compiler test {}:\nruntime error: index out of range [-1]",
                variant.file.display()
            )));
        };
        // We need to assemble the list of input files for the compiler and other related files on the 'filesystem' (ie in a multi-file test)
        // If the last file in a test uses require or a triple slash reference we'll assume all other files will be brought in via references,
        // otherwise, assume all files are just meant to be in the same compilation session without explicit references to one another.
        let no_implicit_references = variant
            .configuration
            .as_ref()
            .and_then(|named| named.config.get("noimplicitreferences"))
            .is_some_and(|value| !value.is_empty());
        if no_implicit_references
            || contains(&last_unit.content, REQUIRE)
            || has_reference_path(&last_unit.content)
        {
            to_be_compiled.push(create_harness_test_file(last_unit, &current_directory));
            for unit in rest {
                other_files.push(create_harness_test_file(unit, &current_directory));
            }
        } else {
            to_be_compiled = units
                .iter()
                .map(|unit| create_harness_test_file(unit, &current_directory))
                .collect();
        }
    }

    let (options, harness_options) = compile::resolve_options(
        variant.configuration.as_ref().map(|named| &named.config),
        test_content.ts_config.as_ref(),
        &current_directory,
    )?;
    crate::configurations::skip_unsupported_compiler_options(&options)?;

    Ok(Prepared {
        variant,
        content: test_content,
        current_directory,
        ts_config_files,
        to_be_compiled,
        other_files,
        has_non_dts_files,
        options,
        harness_options,
    })
}

// port: tsc/internal/testrunner/compiler_runner.go:createHarnessTestFile
fn create_harness_test_file(unit: &TestUnit, current_directory: &[u8]) -> TestFile {
    TestFile {
        unit_name: tsr_tspath::absolute(&unit.name, current_directory),
        content: unit.content.clone(),
    }
}

/// `strings.Contains`.
fn contains(text: &[u8], needle: &[u8]) -> bool {
    text.windows(needle.len()).any(|window| window == needle)
}

/// `referencesRegex`: `reference\spath`, where RE2's `\s` is `[\t\n\f\r ]`.
fn has_reference_path(text: &[u8]) -> bool {
    const REFERENCE: &[u8] = b"reference";
    const PATH: &[u8] = b"path";
    text.windows(REFERENCE.len() + 1 + PATH.len())
        .any(|window| {
            window.starts_with(REFERENCE)
                && matches!(
                    window[REFERENCE.len()],
                    b'\t' | b'\n' | b'\x0c' | b'\r' | b' '
                )
                && window.ends_with(PATH)
        })
}

/// `CompileFiles` over a prepared variant, then the content-mapped units'
/// text is replaced by what the compiler parsed (`sf.Text()`). The pin
/// replaces it in the very test files `Repeat` compiles again, so the
/// compilation's kept inputs take the same text.
// port: tsc/internal/testrunner/compiler_runner.go:newCompilerTest
pub fn compile(
    mut prepared: Prepared,
    testdata: &TestData,
    mode: Mode,
) -> Result<CompilerTest, Stop> {
    let mut result = compile::compile_files(
        prepared.to_be_compiled.clone(),
        prepared.other_files.clone(),
        prepared
            .variant
            .configuration
            .as_ref()
            .map(|named| &named.config),
        prepared.content.ts_config.clone(),
        &prepared.current_directory,
        prepared.content.symlinks.clone(),
        testdata,
        mode,
    )
    .map_err(compile_stop)?;

    // Content-mapped files are transformed during program construction; the transformed text is what the
    // compiler actually parses and reports positions against. Baseline that text (rather than the original
    // foreign source) so the type, symbol, and error baselines line up with the compiler's positions.
    let program = result.program.program().clone();
    for file in prepared
        .to_be_compiled
        .iter_mut()
        .chain(prepared.other_files.iter_mut())
    {
        let Some(text) = content_mapped_text(&program, &file.unit_name) else {
            continue;
        };
        for input in result
            .inputs
            .input_files
            .iter_mut()
            .chain(result.inputs.other_files.iter_mut())
        {
            if input.unit_name == file.unit_name {
                input.content.clone_from(&text);
            }
        }
        file.content = text;
    }

    Ok(CompilerTest { prepared, result })
}

/// The pin's `t.Fatalf` and `t.Skipf` keep their kind; a refusal or error
/// of the compiler or of this crate's plumbing fails the test.
fn compile_stop(error: CompileError) -> Stop {
    match error {
        CompileError::Stop(stop) => stop,
        error => Stop::Fatal(error.to_string()),
    }
}

/// `sf.Text()` of the program's file for `unit_name` when a content mapper
/// produced it (`sf.ContentMapper() != ""`).
fn content_mapped_text(program: &Program, unit_name: &[u8]) -> Option<Vec<u8>> {
    let source = program
        .source_file(unit_name)?
        .bound()
        .view()
        .source_file()
        .ok()?;
    (!source.content_mapper().is_empty()).then(|| source.text().as_bytes().to_vec())
}

/// `runSingleConfigTest`: compile, then the nine `verify*` sub-tests in the
/// pin's order, each reported under the variant's id. A compile failure
/// fails every sub-test the pin would have run with the same reason.
// port: tsc/internal/testrunner/compiler_runner.go:CompilerBaselineRunner.runSingleConfigTest
pub fn run_single_config_test(
    prepared: Prepared,
    testdata: &TestData,
    roots: &Roots,
    mode: Mode,
    report: &mut Report,
) {
    let id = prepared.variant.id();
    let planned = planned_subtests(&prepared);
    let panic_context = format!("Panic on compiler test {}", prepared.variant.file.display());
    let stopped = match catch_unwind(AssertUnwindSafe(|| compile(prepared, testdata, mode))) {
        Ok(Ok(test)) => {
            test.verify_diagnostics(roots, report);
            test.verify_content_mapper(roots, report);
            test.verify_javascript_output(roots, testdata, report);
            test.verify_source_map_output(roots, testdata, report);
            test.verify_source_map_record(roots, testdata, report);
            test.verify_types_and_symbols(roots, testdata, report);
            test.verify_module_resolution(roots, report);
            test.verify_union_ordering(report);
            test.verify_parent_pointers(report);
            return;
        }
        Ok(Err(stop)) => Outcome::from(stop),
        Err(payload) => Outcome::fail(format!(
            "{panic_context}:\n{}",
            panic_message(payload.as_ref())
        )),
    };
    for (subtest, skip) in planned {
        let outcome = skip.map_or_else(|| stopped.clone(), Outcome::skip);
        report.subtest(&id, subtest, outcome);
    }
}

/// The sub-tests `runSingleConfigTest` runs for the variant, in order, with
/// the reason of the one the pin always skips (`skippedEmitTests`): what a
/// compile failure reports failing.
fn planned_subtests(prepared: &Prepared) -> Vec<(&'static str, Option<&'static str>)> {
    let mut planned = vec![(ERROR, None), (CONTENT_MAPPER, None)];
    if prepared.has_non_dts_files {
        planned.push((OUTPUT, skipped_emit_reason(&prepared.variant.basename)));
    }
    planned.extend([(SOURCEMAP, None), (SOURCEMAP_RECORD, None)]);
    if !prepared.harness_options.no_types_and_symbols {
        planned.extend([(TYPE, None), (SYMBOL, None)]);
    }
    if prepared.options.trace_resolution.is_true() {
        planned.push((MODULE_RESOLUTION, None));
    }
    planned.extend([(UNION_ORDERING, None), (PARENT_POINTERS, None)]);
    planned
}

fn skipped_emit_reason(basename: &str) -> Option<&'static str> {
    SKIPPED_EMIT_TESTS
        .iter()
        .find(|(name, _)| *name == basename)
        .map(|&(_, reason)| reason)
}

/// Tests whose `output` sub-test the pin skips, with its reason
/// (`compiler_runner.go:skippedEmitTests`).
pub const SKIPPED_EMIT_TESTS: &[(&str, &str)] = &[
    (
        "filesEmittingIntoSameOutput.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "jsFileCompilationWithJsEmitPathSameAsInput.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "grammarErrors.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "jsFileCompilationEmitBlockedCorrectly.ts",
        "Output order nondeterministic due to collision on filename during parallel emit.",
    ),
    (
        "jsDeclarationsReexportAliasesEsModuleInterop.ts",
        "cls.d.ts is missing statements when run concurrently.",
    ),
    (
        "jsFileCompilationWithoutJsExtensions.ts",
        "No files are emitted.",
    ),
    (
        "typeOnlyMerge2.ts",
        "Nondeterministic contents when run concurrently.",
    ),
    (
        "typeOnlyMerge3.ts",
        "Nondeterministic contents when run concurrently.",
    ),
];

impl CompilerTest {
    fn id(&self) -> String {
        self.prepared.variant.id()
    }

    fn configured_name(&self) -> &str {
        &self.prepared.variant.configured_name
    }

    fn filename(&self) -> String {
        self.prepared.variant.file.display().to_string()
    }

    /// `baseline.Options{Subfolder: suiteName}`.
    fn baseline_options(&self) -> Options<'static> {
        Options {
            subfolder: self.prepared.variant.suite,
            skip_diff_with_old: false,
        }
    }

    /// `c.result.Program`'s compiler program.
    fn program(&self) -> &Program {
        self.result.program.program()
    }

    /// The program's checker pool with an operation held on each of its
    /// checkers, in checker order (`GetTypeCheckerForFile` per file,
    /// `ForEachCheckerParallel`).
    fn pool_operations(&self) -> Result<(&CompilerCheckerPool, Vec<Operation<'_>>), String> {
        let pool = self
            .result
            .program
            .program_like()
            .checked_program()
            .compiler_checker_pool()
            .ok_or("the program has no compiler checker pool")?;
        let operations = pool
            .checkers()
            .map_err(|error| format!("creating the program's checkers: {error:?}"))?
            .iter()
            .map(tsr_checker::CheckerOwner::operation)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("acquiring the program's checkers: {error:?}"))?;
        Ok((pool, operations))
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyDiagnostics
    pub fn verify_diagnostics(&self, roots: &Roots, report: &mut Report) {
        let outcome = recover_and_fail(
            &format!(
                "Panic on creating error baseline for test {}",
                self.filename()
            ),
            || {
                let program = self.program();
                let mut files: Vec<&TestFile> = self
                    .prepared
                    .ts_config_files
                    .iter()
                    .chain(&self.prepared.to_be_compiled)
                    .chain(&self.prepared.other_files)
                    .collect();
                let mut diagnostics = Cow::Borrowed(self.result.diagnostics.as_slice());
                // Content-mapped files' diagnostics are baselined separately (see verifyContentMapper), where they can
                // be rendered against the correct text; the squiggle renderer here assumes a single coordinate space.
                let content_mapped = self.content_mapped_file_names();
                if !content_mapped.is_empty() {
                    files.retain(|file| {
                        !content_mapped.contains(&tsr_tspath::absolute(
                            &file.unit_name,
                            &self.prepared.current_directory,
                        ))
                    });
                    diagnostics = Cow::Owned(
                        self.result
                            .diagnostics
                            .iter()
                            .filter(|diagnostic| {
                                diagnostic
                                    .file
                                    .and_then(|file| file_name_of(program, file))
                                    .is_none_or(|name| !content_mapped.contains(&name))
                            })
                            .cloned()
                            .collect(),
                    );
                }
                let inputs: Vec<errors::InputFile<'_>> = files
                    .iter()
                    .map(|file| errors::InputFile {
                        name: &file.unit_name,
                        content: &file.content,
                    })
                    .collect();
                do_error_baseline(
                    roots,
                    self.configured_name(),
                    program,
                    &inputs,
                    &diagnostics,
                    self.result.options.pretty.is_true(),
                    self.baseline_options(),
                )
            },
        );
        report.subtest(&self.id(), ERROR, outcome);
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyContentMapper
    pub fn verify_content_mapper(&self, roots: &Roots, report: &mut Report) {
        let outcome = recover_and_fail(
            &format!(
                "Panic on creating content mapper baseline for test {}",
                self.filename()
            ),
            || {
                do_content_mapper_baseline(
                    roots,
                    self.configured_name(),
                    self.program(),
                    &self.result.diagnostics,
                    self.baseline_options(),
                )
            },
        );
        report.subtest(&self.id(), CONTENT_MAPPER, outcome);
    }

    /// The absolute names of the files a content mapper produced.
    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.contentMappedFileNames
    pub fn content_mapped_file_names(&self) -> Vec<Vec<u8>> {
        let program = self.program();
        program
            .files()
            .iter()
            .filter_map(|file| {
                let source = file.bound().view().source_file().ok()?;
                program
                    .content_mapper(&source)
                    .map(|_| source.file_name().to_vec())
            })
            .collect()
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyJavaScriptOutput
    pub fn verify_javascript_output(
        &self,
        roots: &Roots,
        testdata: &TestData,
        report: &mut Report,
    ) {
        if !self.prepared.has_non_dts_files {
            return;
        }
        let outcome = match skipped_emit_reason(&self.prepared.variant.basename) {
            Some(reason) => Outcome::skip(reason),
            None => recover_and_fail(
                &format!("Panic on creating js output for test {}", self.filename()),
                || self.js_emit_baseline(roots, testdata),
            ),
        };
        report.subtest(&self.id(), OUTPUT, outcome);
    }

    /// `DoJSEmitBaseline` with the two compilations it starts: the
    /// declaration re-compilation (`compileDeclarationFiles`), then the
    /// `noCheck` repeat, each only where the pinned writer reaches it.
    fn js_emit_baseline(&self, roots: &Roots, testdata: &TestData) -> Outcome {
        let header = baseline_header(testdata, &self.prepared.variant.file);
        let result = match self.result.result() {
            Ok(result) => result,
            Err(failure) => return failure_outcome(&failure),
        };
        let options = &self.result.options;
        let harness = &self.result.harness_options;
        let ts_config_files = views(&self.prepared.ts_config_files);
        let to_be_compiled = views(&self.prepared.to_be_compiled);
        let other_files = views(&self.prepared.other_files);

        let mut failed_checks = Vec::new();
        let mut declaration = None;
        let mut repeat = None;
        // The writer's first `t.Fatal`, and a panic of
        // `prepareDeclarationCompilationContext`, stop it before either
        // compilation; `js_emit_baseline` reports them below.
        let stops_first = !options.no_emit.is_true()
            && !options.emit_declaration_only.is_true()
            && result.js.is_empty()
            && result.diagnostics == 0;
        if !stops_first {
            if let Ok(context) = writers::prepare_declaration_compilation_context(
                &to_be_compiled,
                &other_files,
                &result,
                options,
                &harness.current_directory,
            ) {
                if let Some(context) = context {
                    match self.compile_declaration_files(&context, &ts_config_files, testdata) {
                        Ok((compiled, checks)) => {
                            declaration = Some(compiled);
                            failed_checks = checks;
                        }
                        Err(outcome) => return outcome,
                    }
                }
                if !options.no_check.is_true() && !options.no_emit.is_true() {
                    match self.result.repeat(&no_check_configuration(), testdata) {
                        Ok(without_checking) => repeat = Some(without_checking),
                        Err(error) => {
                            return Outcome::fail(format!(
                                "the noCheck repeat did not compile: {error}"
                            ))
                        }
                    }
                }
            }
        }
        let repeat_result = match repeat.as_ref().map(Compilation::result).transpose() {
            Ok(repeat_result) => repeat_result,
            Err(failure) => return failure_outcome(&failure),
        };
        let repeat_outputs = repeat_result
            .as_ref()
            .map(|without_checking| RepeatOutputs {
                js: without_checking.js.clone(),
                dts: without_checking.dts.clone(),
            });

        let composed = writers::js_emit_baseline(&JsEmitInput {
            configured_name: self.configured_name().as_bytes(),
            header: &header,
            options,
            full_emit_paths: harness.full_emit_paths,
            harness_current_directory: &harness.current_directory,
            to_be_compiled: &to_be_compiled,
            other_files: &other_files,
            result: &result,
            declaration: declaration.as_ref(),
            no_check_repeat: repeat_outputs.as_ref(),
            json_errors: &JsonErrors,
        });
        let outcome = match composed {
            Ok(composed) => baseline::run(
                roots,
                &composed.path,
                &composed.actual,
                self.baseline_options(),
            ),
            Err(failure) => failure_outcome(&failure),
        };
        with_failures(outcome, &failed_checks)
    }

    /// `compileDeclarationFiles`: `CompileFilesEx` over the context's
    /// declaration files with the first compilation's harness options,
    /// compiler options and symlinks, and a configuration of the program's
    /// config file and content mappers when it has a config file; then the
    /// `DtsFileErrors` error baseline when it has diagnostics. Also returns
    /// the `assert.Check` failures of that error baseline.
    // port: tsc/internal/testutil/tsbaseline/js_emit_baseline.go:compileDeclarationFiles
    fn compile_declaration_files<'a>(
        &self,
        context: &DeclarationCompilationContext<'a>,
        ts_config_files: &[FileView<'a>],
        testdata: &TestData,
    ) -> Result<(DeclarationCompilationResult<'a>, Vec<String>), Outcome> {
        let config = self.program().config();
        let tsconfig = config.config_file.as_ref().map(|config_file| {
            let mut tsconfig = ParsedCommandLine::new(CompilerOptions::default(), Vec::new());
            tsconfig.config_file = Some(config_file.clone());
            tsconfig.content_mappers.clone_from(&config.content_mappers);
            tsconfig
        });
        let compiled = compile::compile_files_ex(
            compile::Inputs {
                input_files: owned(&context.decl_input_files),
                other_files: owned(&context.decl_other_files),
                harness_options: self.result.harness_options.clone(),
                compiler_options: self.result.options.clone(),
                current_directory: context.current_directory.to_vec(),
                symlinks: self.result.symlinks.clone(),
                tsconfig,
                mode: self.result.inputs.mode,
            },
            testdata,
        )
        .map_err(|error| {
            Outcome::fail(format!("the declaration files did not compile: {error}"))
        })?;
        let mut result = DeclarationCompilationResult {
            decl_input_files: context.decl_input_files.clone(),
            decl_other_files: context.decl_other_files.clone(),
            diagnostics: compiled.diagnostics.len(),
            error_baseline: Vec::new(),
        };
        let mut failed_checks = Vec::new();
        if !compiled.diagnostics.is_empty() {
            let files = writers::dts_file_error_inputs(ts_config_files, &result);
            let inputs: Vec<errors::InputFile<'_>> = files
                .iter()
                .map(|file| errors::InputFile {
                    name: file.unit_name,
                    content: file.content,
                })
                .collect();
            let rendered = errors::error_baseline(
                compiled.program.program(),
                &inputs,
                &compiled.diagnostics,
                false, /*pretty*/
            )
            .map_err(|error| {
                Outcome::fail(format!("the DtsFileErrors error baseline failed: {error}"))
            })?;
            result.error_baseline = rendered.text;
            failed_checks = rendered.failed_checks;
        }
        Ok((result, failed_checks))
    }

    /// The pin computes the header for `DoSourcemapBaseline`, which does not
    /// read it.
    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifySourceMapOutput
    pub fn verify_source_map_output(
        &self,
        roots: &Roots,
        _testdata: &TestData,
        report: &mut Report,
    ) {
        let outcome = recover_and_fail(
            &format!(
                "Panic on creating source map output for test {}",
                self.filename()
            ),
            || {
                let result = match self.result.result() {
                    Ok(result) => result,
                    Err(failure) => return failure_outcome(&failure),
                };
                let composed = writers::sourcemap_baseline(&SourcemapInput {
                    configured_name: self.configured_name().as_bytes(),
                    options: &self.result.options,
                    full_emit_paths: self.result.harness_options.full_emit_paths,
                    result: &result,
                });
                match composed {
                    // The writer returns without `baseline.Run`.
                    Ok(None) => Outcome::Pass,
                    Ok(Some(composed)) => baseline::run(
                        roots,
                        &composed.path,
                        &composed.actual,
                        self.baseline_options(),
                    ),
                    Err(failure) => failure_outcome(&failure),
                }
            },
        );
        report.subtest(&self.id(), SOURCEMAP, outcome);
    }

    /// The pin computes the header for `DoSourcemapRecordBaseline`, which
    /// does not read it.
    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifySourceMapRecord
    pub fn verify_source_map_record(
        &self,
        roots: &Roots,
        _testdata: &TestData,
        report: &mut Report,
    ) {
        let outcome = recover_and_fail(
            &format!(
                "Panic on creating source map record for test {}",
                self.filename()
            ),
            || {
                let result = match self.result.result() {
                    Ok(result) => result,
                    Err(failure) => return failure_outcome(&failure),
                };
                let composed = writers::sourcemap_record_baseline(&SourcemapRecordInput {
                    configured_name: self.configured_name().as_bytes(),
                    options: &self.result.options,
                    result: &result,
                });
                match composed {
                    Ok(composed) => baseline::run(
                        roots,
                        &composed.path,
                        &composed.actual,
                        self.baseline_options(),
                    ),
                    Err(failure) => failure_outcome(&failure),
                }
            },
        );
        report.subtest(&self.id(), SOURCEMAP_RECORD, outcome);
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyTypesAndSymbols
    pub fn verify_types_and_symbols(
        &self,
        roots: &Roots,
        testdata: &TestData,
        report: &mut Report,
    ) {
        if self.result.harness_options.no_types_and_symbols {
            return;
        }
        let program = self.program();
        let all_files: Vec<typebaseline::InputFile<'_>> = self
            .prepared
            .to_be_compiled
            .iter()
            .chain(&self.prepared.other_files)
            .filter(|file| program.source_file(&file.unit_name).is_some())
            .map(|file| typebaseline::InputFile {
                name: &file.unit_name,
                content: &file.content,
            })
            .collect();
        let header = baseline_header(testdata, &self.prepared.variant.file);
        self.do_type_and_symbol_baseline(
            roots,
            &header,
            &all_files,
            !self.result.diagnostics.is_empty(),
            report,
        );
    }

    /// The `type` and `symbol` sub-tests, one walker over the program's
    /// checkers for both, as `DoTypeAndSymbolBaseline` creates it before
    /// either sub-test. Without the checkers, both fail.
    // port: tsc/internal/testutil/tsbaseline/type_symbol_baseline.go:DoTypeAndSymbolBaseline
    // port: tsc/internal/testutil/tsbaseline/type_symbol_baseline.go:checkBaselines
    fn do_type_and_symbol_baseline(
        &self,
        roots: &Roots,
        header: &[u8],
        all_files: &[typebaseline::InputFile<'_>],
        has_error_baseline: bool,
        report: &mut Report,
    ) {
        let id = self.id();
        let (pool, mut operations) = match self.pool_operations() {
            Ok(held) => held,
            Err(reason) => {
                report.subtest(&id, TYPE, Outcome::fail(reason.clone()));
                report.subtest(&id, SYMBOL, Outcome::fail(reason));
                return;
            }
        };
        let mut checkers =
            match tsr_compiler::FileCheckers::for_pool(pool, operations.iter_mut().collect()) {
                Ok(checkers) => checkers,
                Err(error) => {
                    let reason = format!("the program's checkers: {error:?}");
                    report.subtest(&id, TYPE, Outcome::fail(reason.clone()));
                    report.subtest(&id, SYMBOL, Outcome::fail(reason));
                    return;
                }
            };
        let mut trace = typebaseline::Trace {
            record_queries: false,
            ..Default::default()
        };
        let mut timing = typebaseline::NoTiming;
        let mut walker = typebaseline::TypeWriterWalker::new(
            self.program(),
            &mut checkers,
            has_error_baseline,
            &mut trace,
            &mut timing,
        );
        let header_text = String::from_utf8_lossy(header).into_owned();
        for (subtest, symbols, extension) in [(TYPE, false, ".types"), (SYMBOL, true, ".symbols")] {
            let outcome = recover_and_fail(
                &format!("Panic on creating {subtest} baseline for test {header_text}"),
                || match walker.text(all_files, header, symbols) {
                    Ok(text) => baseline::run(
                        roots,
                        &replace_ts_extension(self.configured_name(), extension),
                        text.as_deref().unwrap_or(NO_CONTENT),
                        self.baseline_options(),
                    ),
                    Err(error) => Outcome::fail(format!("the {subtest} walk failed: {error}")),
                },
            );
            report.subtest(&id, subtest, outcome);
        }
    }

    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyModuleResolution
    pub fn verify_module_resolution(&self, roots: &Roots, report: &mut Report) {
        if !self.result.options.trace_resolution.is_true() {
            return;
        }
        let outcome = recover_and_fail(
            &format!(
                "Panic on creating module resolution baseline for test {}",
                self.filename()
            ),
            || {
                do_module_resolution_baseline(
                    roots,
                    self.configured_name(),
                    &self.result.trace,
                    Options {
                        skip_diff_with_old: true,
                        ..self.baseline_options()
                    },
                )
            },
        );
        report.subtest(&self.id(), MODULE_RESOLUTION, outcome);
    }

    /// Every union each checker interned sorts back to its order from its
    /// reversal and ten seeded shuffles. The pin recovers no panic here; a
    /// panic fails the sub-test as the test's own `RecoverAndFail` would.
    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyUnionOrdering
    pub fn verify_union_ordering(&self, report: &mut Report) {
        let outcome = recover_and_fail(
            &format!("Panic on compiler test {}", self.filename()),
            || {
                let operations = match self.pool_operations() {
                    Ok((_, operations)) => operations,
                    Err(reason) => return Outcome::fail(reason),
                };
                let checkers: Vec<&Operation<'_>> = operations.iter().collect();
                let verdict = subtests::union_ordering_checkers(&checkers);
                match verdict["state"].as_str() {
                    Some("executed") if verdict["inconsistent"].as_u64() == Some(0) => {
                        Outcome::Pass
                    }
                    Some("executed") => {
                        Outcome::fail("compareTypes does not sort union types consistently")
                    }
                    _ => failed_verdict("union ordering", &verdict),
                }
            },
        );
        report.subtest(&self.id(), UNION_ORDERING, outcome);
    }

    /// Below every non-library source file, each node's parent is the node
    /// the child walk came from. The pin recovers no panic here; a panic
    /// fails the sub-test as the test's own `RecoverAndFail` would.
    // port: tsc/internal/testrunner/compiler_runner.go:compilerTest.verifyParentPointers
    pub fn verify_parent_pointers(&self, report: &mut Report) {
        let outcome = recover_and_fail(
            &format!("Panic on compiler test {}", self.filename()),
            || {
                let verdict = subtests::parent_pointers(self.program());
                match (verdict["state"].as_str(), verdict["failure"].as_str()) {
                    (Some("executed"), None) => Outcome::Pass,
                    (Some("executed"), Some(failure)) => {
                        Outcome::fail_with(failure, verdict["node"].to_string())
                    }
                    _ => failed_verdict("source file parent pointers", &verdict),
                }
            },
        );
        report.subtest(&self.id(), PARENT_POINTERS, outcome);
    }
}

/// A sub-test verdict of `crate::harness::subtests` that did not execute.
fn failed_verdict(what: &str, verdict: &Value) -> Outcome {
    Outcome::fail(format!(
        "the {what} check failed ({}): {}",
        verdict["class"].as_str().unwrap_or("unknown"),
        verdict["reason"].as_str().unwrap_or("no reason")
    ))
}

/// `DoErrorBaseline`: `<configured name>.errors.txt`, `<no content>`
/// without diagnostics. The error baseline's failed `assert.Check`s and a
/// diagnostic with code -1 (`t.Fatalf` after the comparison) fail the
/// sub-test besides the comparison.
// port: tsc/internal/testutil/tsbaseline/error_baseline.go:DoErrorBaseline
fn do_error_baseline(
    roots: &Roots,
    baseline_path: &str,
    program: &Program,
    input_files: &[errors::InputFile<'_>],
    diagnostics: &[Diagnostic],
    pretty: bool,
    options: Options<'_>,
) -> Outcome {
    let baseline_path = replace_ts_extension(baseline_path, ".errors.txt");
    let (error_baseline, mut failures) = if diagnostics.is_empty() {
        (NO_CONTENT.to_vec(), Vec::new())
    } else {
        match errors::error_baseline(program, input_files, diagnostics, pretty) {
            Ok(composed) => (composed.text, composed.failed_checks),
            Err(error) => return Outcome::fail(format!("the error baseline failed: {error}")),
        }
    };
    let outcome = baseline::run(roots, &baseline_path, &error_baseline, options);
    if diagnostics.iter().any(|diagnostic| diagnostic.code == -1) {
        failures.push("Found diagnostic with code -1, which is used to log critical assertion violations in the baseline. Inspect and fix those failures.".to_owned());
    }
    with_failures(outcome, &failures)
}

/// `DoContentMapperBaseline`: `<configured name>.contentmapper` when the
/// program has content-mapped files; otherwise no `baseline.Run`, and the
/// sub-test passes.
// port: tsc/internal/testutil/tsbaseline/contentmapper_baseline.go:DoContentMapperBaseline
fn do_content_mapper_baseline(
    roots: &Roots,
    baseline_path: &str,
    program: &Program,
    diagnostics: &[Diagnostic],
    options: Options<'_>,
) -> Outcome {
    match errors::content_mapper(program, diagnostics) {
        Ok(None) => Outcome::Pass,
        Ok(Some(content)) => baseline::run(
            roots,
            &replace_ts_extension(baseline_path, ".contentmapper"),
            &content,
            options,
        ),
        Err(error) => Outcome::fail(format!("the content mapper baseline failed: {error}")),
    }
}

/// `DoModuleResolutionBaseline`: `<configured name>.trace.json`, the
/// trace or `<no content>`.
// port: tsc/internal/testutil/tsbaseline/module_resolution_baseline.go:DoModuleResolutionBaseline
fn do_module_resolution_baseline(
    roots: &Roots,
    baseline_path: &str,
    trace: &[u8],
    options: Options<'_>,
) -> Outcome {
    let baseline_path = replace_ts_extension(baseline_path, ".trace.json");
    let trace = if trace.is_empty() { NO_CONTENT } else { trace };
    baseline::run(roots, &baseline_path, trace, options)
}

/// The JSON branch of `DoJSEmitBaseline` (an emitted `.json` file whose
/// re-parse has diagnostics while the program has none) renders an error
/// baseline over a file outside any program, which the ported error writer
/// cannot address: such a test fails with this reason instead.
struct JsonErrors;

impl JsonErrorBaseline for JsonErrors {
    fn render(
        &self,
        file: &FileView<'_>,
        _parsed: &tsr_ast::ParsedFile,
        _diagnostics: &[Diagnostic],
    ) -> Result<Vec<u8>, Failure> {
        Err(Failure::Input(format!(
            "the runner renders no error baseline for the JSON output {}",
            String::from_utf8_lossy(file.unit_name)
        )))
    }
}

/// `testConfig["noCheck"] = "true"`.
fn no_check_configuration() -> TestConfiguration {
    TestConfiguration::from([("noCheck".to_owned(), "true".to_owned())])
}

fn views(files: &[TestFile]) -> Vec<FileView<'_>> {
    files.iter().map(TestFile::view).collect()
}

fn owned(files: &[FileView<'_>]) -> Vec<TestFile> {
    files
        .iter()
        .map(|file| TestFile {
            unit_name: file.unit_name.to_vec(),
            content: file.content.to_vec(),
        })
        .collect()
}

/// The name of the program file whose root is `file`.
fn file_name_of(program: &Program, file: NodeId) -> Option<Vec<u8>> {
    let file = program
        .files()
        .iter()
        .find(|candidate| candidate.source() == file)?;
    let source = file.bound().view().source_file().ok()?;
    Some(source.file_name().to_vec())
}

/// A writer's `t.Fatal` or panic, or a harness defect, as the sub-test's
/// failure.
fn failure_outcome(failure: &Failure) -> Outcome {
    Outcome::fail(failure.to_string())
}

/// `outcome` of `baseline.Run` together with the failures a sub-test
/// records without stopping (`assert.Check`) or after comparing
/// (`t.Fatalf`): any of them fails the sub-test, and a failed comparison
/// keeps its diff.
fn with_failures(outcome: Outcome, failures: &[String]) -> Outcome {
    if failures.is_empty() {
        return outcome;
    }
    let failures = failures.join("; ");
    match outcome {
        Outcome::Fail { reason, detail } => Outcome::Fail {
            reason: format!("{reason}; {failures}"),
            detail,
        },
        Outcome::Pass | Outcome::Skip { .. } => Outcome::fail(failures),
    }
}

/// `defer testutil.RecoverAndFail(t, message)`: a panic in `run` fails the
/// sub-test with `message` and the panic's payload (the pin adds the
/// stack).
// port: tsc/internal/testutil/testutil.go:RecoverAndFail
fn recover_and_fail(message: &str, run: impl FnOnce() -> Outcome) -> Outcome {
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(outcome) => outcome,
        Err(payload) => Outcome::fail(format!("{message}:\n{}", panic_message(payload.as_ref()))),
    }
}

/// A panic payload's message.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|&text| text.to_owned()))
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

/// `tsExtension.ReplaceAllString(name, extension)`: a trailing `.ts` or
/// `.tsx` replaced by `extension`; other names unchanged.
fn replace_ts_extension(name: &str, extension: &str) -> Vec<u8> {
    match name
        .strip_suffix(".tsx")
        .or_else(|| name.strip_suffix(".ts"))
    {
        Some(stem) => format!("{stem}{extension}").into_bytes(),
        None => name.as_bytes().to_vec(),
    }
}

/// `tspath.GetPathComponentsRelativeTo(repo.TestDataPath(), filename)` joined
/// back: the header the emit, source-map and type baselines print
/// (`tests/cases/compiler/foo.ts`). Both paths are made absolute first, as
/// the pin's are.
pub fn baseline_header(testdata: &TestData, file: &Path) -> Vec<u8> {
    let components = tsr_tspath::path_components_relative_to(
        &path_bytes(&absolute(testdata.path())),
        &path_bytes(&absolute(file)),
        b"",
        false,
    );
    tsr_tspath::path_from_components(&components)
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A path's bytes with forward slashes (`tspath.NormalizeSlashes`).
fn path_bytes(path: &Path) -> Vec<u8> {
    tsr_tspath::normalize_slashes(path.as_os_str().as_encoded_bytes()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        baseline_header, has_reference_path, planned_subtests, replace_ts_extension, with_failures,
        Prepared, OUTPUT,
    };
    use crate::enumerate::Variant;
    use crate::result::Outcome;
    use crate::test_case_parser::TestCaseContent;
    use crate::TestData;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_header_is_the_test_path_under_testdata() {
        let testdata = TestData {
            root: PathBuf::from("/repo/upstream/tsc/testdata"),
        };
        assert_eq!(
            baseline_header(
                &testdata,
                Path::new("/repo/upstream/tsc/testdata/tests/cases/compiler/foo.ts")
            ),
            b"tests/cases/compiler/foo.ts"
        );
        assert_eq!(
            baseline_header(
                &testdata,
                Path::new("/repo/upstream/tsc/testdata/tests/cases/conformance/types/union/a.tsx")
            ),
            b"tests/cases/conformance/types/union/a.tsx"
        );
    }

    #[test]
    fn baseline_names_replace_a_trailing_ts_or_tsx_extension() {
        assert_eq!(
            replace_ts_extension("foo(target=es2015).ts", ".errors.txt"),
            b"foo(target=es2015).errors.txt"
        );
        assert_eq!(replace_ts_extension("a.tsx", ".types"), b"a.types");
        assert_eq!(
            replace_ts_extension("a.tsx.ts", ".symbols"),
            b"a.tsx.symbols"
        );
        assert_eq!(
            replace_ts_extension("a.d.ts", ".trace.json"),
            b"a.d.trace.json"
        );
        assert_eq!(replace_ts_extension("a.js", ".types"), b"a.js");
    }

    #[test]
    fn reference_path_needs_one_re2_space_between_the_words() {
        assert!(has_reference_path(b"/// <reference path=\"a.ts\" />"));
        assert!(has_reference_path(b"reference\tpath"));
        assert!(has_reference_path(b"reference\x0cpath"));
        assert!(!has_reference_path(b"referencepath"));
        assert!(!has_reference_path(b"reference  path"));
        // RE2's \s excludes the vertical tab.
        assert!(!has_reference_path(b"reference\x0bpath"));
    }

    #[test]
    fn checks_fail_a_passing_comparison_and_join_a_failing_one() {
        assert_eq!(with_failures(Outcome::Pass, &[]), Outcome::Pass);
        assert_eq!(
            with_failures(Outcome::Pass, &["total number of errors".to_owned()]),
            Outcome::fail("total number of errors")
        );
        assert_eq!(
            with_failures(
                Outcome::fail_with("the baseline file x has changed.", "diff"),
                &["a".to_owned(), "b".to_owned()]
            ),
            Outcome::fail_with("the baseline file x has changed.; a; b", "diff")
        );
    }

    fn prepared(basename: &str, has_non_dts_files: bool) -> Prepared {
        Prepared {
            variant: Variant {
                suite: "compiler",
                file: PathBuf::from(format!("/t/{basename}")),
                basename: basename.to_owned(),
                test_name: basename.to_owned(),
                configured_name: basename.to_owned(),
                configuration: None,
            },
            content: TestCaseContent {
                units: Vec::new(),
                ts_config: None,
                ts_config_unit: None,
                symlinks: std::collections::BTreeMap::new(),
            },
            current_directory: b"/.src".to_vec(),
            ts_config_files: Vec::new(),
            to_be_compiled: Vec::new(),
            other_files: Vec::new(),
            has_non_dts_files,
            options: tsr_core::CompilerOptions::default(),
            harness_options: crate::harness_options::HarnessOptions::default(),
        }
    }

    #[test]
    fn a_compile_failure_names_the_sub_tests_the_pin_runs() {
        let names = |prepared: &Prepared| -> Vec<&'static str> {
            planned_subtests(prepared)
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        };
        assert_eq!(
            names(&prepared("a.ts", true)),
            [
                "error",
                "content mapper",
                "output",
                "sourcemap",
                "sourcemap record",
                "type",
                "symbol",
                "union ordering",
                "source file parent pointers"
            ]
        );
        let mut declarations_only = prepared("a.ts", false);
        declarations_only.harness_options.no_types_and_symbols = true;
        declarations_only.options.trace_resolution = tsr_core::Tristate::TRUE;
        assert_eq!(
            names(&declarations_only),
            [
                "error",
                "content mapper",
                "sourcemap",
                "sourcemap record",
                "module resolution",
                "union ordering",
                "source file parent pointers"
            ]
        );
        let skipped = planned_subtests(&prepared("grammarErrors.ts", true));
        assert!(skipped
            .iter()
            .any(|&(name, skip)| name == OUTPUT && skip.is_some()));
    }
}
