//! `runner.go`: one scenario's transcript. The first command, then for every
//! edit the edit, the command (or a watch cycle) and, beside it in a work
//! group, a clean build of the same state whose outputs must equal the
//! incremental ones.
use crate::baseline::diff_text;
use crate::execute::tsc::{CommandLineResult, ExitStatus};
use crate::execute::{command_line, tsc};
use crate::sys::{new_test_sys, FileMap, TestSys};
use std::collections::BTreeMap;
use std::sync::Arc;
use tsr_core::workgroup::WorkGroup;
use tsr_ipc::Context;
use tsr_jsstring::JsString;

/// An edit's closure: what it does to the fake system.
pub type EditFn = Arc<dyn Fn(&TestSys) + Send + Sync>;

/// `tscEdit`.
#[derive(Clone, Default)]
pub struct TscEdit {
    pub caption: String,
    /// `nil` runs the scenario's own arguments.
    pub command_line_args: Option<Vec<JsString>>,
    pub edit: Option<EditFn>,
    pub expected_diff: String,
}

/// `noChange`.
pub fn no_change() -> TscEdit {
    TscEdit {
        caption: "no change".to_owned(),
        ..TscEdit::default()
    }
}

/// `tscInput`.
#[derive(Clone, Default)]
pub struct TscInput {
    pub sub_scenario: String,
    pub command_line_args: Vec<JsString>,
    pub files: FileMap,
    pub cwd: Vec<u8>,
    pub edits: Vec<TscEdit>,
    pub env: BTreeMap<String, String>,
    /// `nil` keeps the fake system's own (a terminal).
    pub output_is_tty: Option<bool>,
    pub ignore_case: bool,
    pub windows_style_root: Vec<u8>,
}

/// How far a transcript got: what the binary reports for a run that stopped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The commands (a watch scenario's edits run watch cycles) the
    /// transcript holds.
    pub commands: usize,
    /// The edits whose whole step (with its clean-build comparison) the
    /// transcript holds.
    pub edits_completed: usize,
    /// Where the run is: `header`, `initial`, `edit` or `done`.
    pub stage: &'static str,
}

/// The transcript as far as it got: the baseline text and the progress. The
/// runner writes both as it goes, so a refusal or a panic leaves them where
/// the run stopped.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub text: Vec<u8>,
    pub progress: Progress,
}

/// A completed run: where `baseline.Run` puts the transcript and what the pin
/// reports with `t.Errorf` (empty when the run has no unexpected difference).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    /// The reference path below `baselines/reference`.
    pub baseline_path: String,
    pub unexpected_diff: Vec<u8>,
}

/// Cancels the scenario's context when the run ends, as the pin's
/// `t.Context()` is canceled when the subtest ends.
struct CancelOnDrop(Context);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// `strings.Join(args, " ")`.
fn join(args: &[JsString]) -> Vec<u8> {
    args.iter()
        .map(JsString::as_bytes)
        .collect::<Vec<_>>()
        .join(&b' ')
}

impl TscInput {
    /// # Panics
    /// On an exit status outside the six, as the pin's runner does.
    // port: tsc/internal/execute/tsctests/runner.go:tscInput.executeCommand
    pub fn execute_command(
        &self,
        ctx: &Context,
        sys: &Arc<TestSys>,
        baseline_builder: &mut Vec<u8>,
        command_line_args: &[JsString],
    ) -> CommandLineResult {
        baseline_builder.extend_from_slice(b"tsgo ");
        baseline_builder.extend_from_slice(&join(command_line_args));
        baseline_builder.push(b'\n');
        let result = command_line(ctx, sys.clone(), command_line_args, Some(sys.clone()));
        baseline_builder.extend_from_slice(exit_status_line(result.status));
        result
    }

    /// Runs the scenario into `transcript`. `scenario` is the folder below
    /// the family (`commandLine`, `sample`, ...).
    // port: tsc/internal/execute/tsctests/runner.go:tscInput.run
    pub fn run(&self, scenario: &str, transcript: &mut Transcript) -> RunResult {
        // ctx is cancelled when the subtest ends, tearing down any content mapper host created during the run.
        let ctx = CancelOnDrop(Context::background().with_cancel());
        let ctx = &ctx.0;
        // initial test tsc compile
        transcript.progress.stage = "header";
        let baseline_builder = &mut transcript.text;
        let sys = new_test_sys(self, false);
        write_header(&sys, baseline_builder);
        transcript.progress.stage = "initial";
        let result = self.execute_command(ctx, &sys, baseline_builder, &self.command_line_args);
        transcript.progress.commands += 1;
        sys.serialize_state(baseline_builder);
        if result.watcher.is_some() && sys.mock_watch_backend.has_watches() {
            baseline_builder.extend_from_slice(&sys.mock_watch_backend.watch_state());
        }
        let mut unexpected_diff = Vec::new();
        unexpected_diff
            .extend_from_slice(&sys.baseline_programs(baseline_builder, "Initial build"));

        for (index, edit) in self.edits.iter().enumerate() {
            transcript.progress.stage = "edit";
            sys.clear_output();
            let mut non_incremental_sys: Option<Arc<TestSys>> = None;
            let command_line_args = edit
                .command_line_args
                .as_deref()
                .unwrap_or(&self.command_line_args);
            {
                let wg = WorkGroup::new(false);
                let commands = &mut transcript.progress.commands;
                let baseline_builder = &mut transcript.text;
                let unexpected = &mut unexpected_diff;
                let sys = &sys;
                let result = &result;
                wg.queue(move || {
                    baseline_builder.extend_from_slice(
                        format!("\n\nEdit [{index}]:: {}\n", edit.caption).as_bytes(),
                    );
                    if let Some(edit) = &edit.edit {
                        edit(sys);
                    }
                    let changed_paths = sys.fs_differ.changed_paths();
                    sys.baseline_fs_with_diff(baseline_builder);

                    match &result.watcher {
                        None => {
                            self.execute_command(ctx, sys, baseline_builder, command_line_args);
                        }
                        Some(watcher) => {
                            sys.mock_watch_backend.send_changed_paths(&changed_paths);
                            watcher.do_cycle();
                        }
                    }
                    *commands += 1;
                    sys.serialize_state(baseline_builder);
                    if result.watcher.is_some() && sys.mock_watch_backend.has_watches() {
                        baseline_builder.extend_from_slice(&sys.mock_watch_backend.watch_state());
                    }
                    unexpected.extend_from_slice(&sys.baseline_programs(
                        baseline_builder,
                        &format!("Edit [{index}]:: {}\n", edit.caption),
                    ));
                });
                let non_incremental = &mut non_incremental_sys;
                wg.queue(move || {
                    // Compute build with all the edits
                    let shadow = new_test_sys(self, true);
                    for edit in &self.edits[..=index] {
                        if let Some(edit) = &edit.edit {
                            edit(&shadow);
                        }
                    }
                    command_line(ctx, shadow.clone(), command_line_args, Some(shadow.clone()));
                    *non_incremental = Some(shadow);
                });
                wg.run_and_wait();
            }

            let non_incremental_sys =
                non_incremental_sys.expect("the clean build ran beside the edit");
            let diff = get_diff_for_incremental(&sys, &non_incremental_sys);
            let baseline_builder = &mut transcript.text;
            if !diff.is_empty() {
                let explanation = if edit.expected_diff.is_empty() {
                    "!!! Unexpected diff, please review and either fix or write explanation as expectedDiff !!!"
                } else {
                    edit.expected_diff.as_str()
                };
                baseline_builder
                    .extend_from_slice(format!("\n\nDiff:: {explanation}\n").as_bytes());
                baseline_builder.extend_from_slice(&diff);
                if edit.expected_diff.is_empty() {
                    unexpected_diff.extend_from_slice(
                        format!(
                            "Edit [{index}]:: {}\n!!! Unexpected diff, please review and either fix or write explanation as expectedDiff !!!\n",
                            edit.caption
                        )
                        .as_bytes(),
                    );
                    unexpected_diff.extend_from_slice(&diff);
                    unexpected_diff.push(b'\n');
                }
            } else if !edit.expected_diff.is_empty() {
                baseline_builder.extend_from_slice(
                    format!(
                        "\n\nDiff:: {} !!! Diff not found but explanation present, please review and remove the explanation !!!\n",
                        edit.expected_diff
                    )
                    .as_bytes(),
                );
                unexpected_diff.extend_from_slice(
                    format!(
                        "Edit [{index}]:: {}\n!!! Diff not found but explanation present, please review and remove the explanation !!!\n",
                        edit.caption
                    )
                    .as_bytes(),
                );
            }
            transcript.progress.edits_completed += 1;
        }
        transcript.progress.stage = "done";
        RunResult {
            baseline_path: format!(
                "{}/{scenario}/{}.js",
                self.get_baseline_sub_folder(),
                self.sub_scenario.replace(' ', "-")
            ),
            unexpected_diff,
        }
    }

    // port: tsc/internal/execute/tsctests/runner.go:tscInput.getBaselineSubFolder
    pub fn get_baseline_sub_folder(&self) -> String {
        let mut command_name = "tsc";
        if self
            .command_line_args
            .iter()
            .any(|arg| matches!(arg.as_bytes(), b"-b" | b"--b" | b"-build" | b"--build"))
        {
            command_name = "tsbuild";
        }
        let mut w = "";
        if self
            .command_line_args
            .iter()
            .any(|arg| matches!(arg.as_bytes(), b"-w" | b"--w" | b"-watch" | b"--watch"))
        {
            w = "Watch";
        }
        format!("{command_name}{w}")
    }
}

/// The `ExitStatus::` line `executeCommand` writes for `status`.
///
/// # Panics
/// On a status outside the six (`UnknownExitStatus`).
pub fn exit_status_line(status: ExitStatus) -> &'static [u8] {
    match status {
        ExitStatus::Success => b"ExitStatus:: Success",
        ExitStatus::DiagnosticsPresent_OutputsSkipped => {
            b"ExitStatus:: DiagnosticsPresent_OutputsSkipped"
        }
        ExitStatus::DiagnosticsPresent_OutputsGenerated => {
            b"ExitStatus:: DiagnosticsPresent_OutputsGenerated"
        }
        ExitStatus::InvalidProject_OutputsSkipped => b"ExitStatus:: InvalidProject_OutputsSkipped",
        ExitStatus::ProjectReferenceCycle_OutputsSkipped => {
            b"ExitStatus:: ProjectReferenceCycle_OutputsSkipped"
        }
        ExitStatus::NotImplemented => b"ExitStatus:: NotImplemented",
        ExitStatus(other) => panic!("UnknownExitStatus {other}"),
    }
}

/// The differences between the incremental system and a clean build of the
/// same state: every file the clean build wrote (a build info only by its
/// existence), then the comparable console output.
///
/// # Panics
/// When a file the clean build wrote is gone from its own file system.
// port: tsc/internal/execute/tsctests/runner.go:getDiffForIncremental
pub fn get_diff_for_incremental(
    incremental_sys: &TestSys,
    non_incremental_sys: &TestSys,
) -> Vec<u8> {
    let mut diff_builder = Vec::new();

    // ToSlice, then sorted.
    let non_incremental_outputs = non_incremental_sys.fs.written_files.to_slice();
    for non_incremental_output in &non_incremental_outputs {
        let name = String::from_utf8_lossy(non_incremental_output);
        if tsr_tspath::file_extension_is(non_incremental_output, b".tsbuildinfo")
            || non_incremental_output.ends_with(b".readable.baseline.txt")
        {
            // Just check existence
            if !incremental_sys
                .fs_from_file_map()
                .file_exists(non_incremental_output)
            {
                diff_builder.extend_from_slice(&diff_text(
                    format!("nonIncremental {name}").as_bytes(),
                    format!("incremental {name}").as_bytes(),
                    b"Exists",
                    b"",
                ));
                diff_builder.push(b'\n');
            }
        } else {
            let Some(non_incremental_text) = non_incremental_sys
                .fs_from_file_map()
                .read_file(non_incremental_output)
            else {
                panic!("Written file not found {name}");
            };
            let incremental_text = incremental_sys
                .fs_from_file_map()
                .read_file(non_incremental_output);
            if incremental_text.as_deref() != Some(non_incremental_text.as_slice()) {
                diff_builder.extend_from_slice(&diff_text(
                    format!("nonIncremental {name}").as_bytes(),
                    format!("incremental {name}").as_bytes(),
                    &non_incremental_text,
                    incremental_text.as_deref().unwrap_or_default(),
                ));
                diff_builder.push(b'\n');
            }
        }
    }

    let incremental_output = incremental_sys.get_output(true);
    let non_incremental_output = non_incremental_sys.get_output(true);
    if incremental_output != non_incremental_output {
        diff_builder.extend_from_slice(&diff_text(
            b"nonIncremental.output.txt",
            b"incremental.output.txt",
            &non_incremental_output,
            &incremental_output,
        ));
    }
    diff_builder
}

/// The exact pre-command transcript, independently witnessed by all baselines.
pub(crate) fn write_header(sys: &TestSys, baseline_builder: &mut Vec<u8>) {
    baseline_builder.extend_from_slice(b"currentDirectory::");
    baseline_builder.extend_from_slice(tsc::System::get_current_directory(sys));
    baseline_builder.extend_from_slice(b"\nuseCaseSensitiveFileNames::");
    baseline_builder.extend_from_slice(if tsc::System::fs(sys).use_case_sensitive_file_names() {
        b"true"
    } else {
        b"false"
    });
    baseline_builder.extend_from_slice(b"\nInput::\n");
    sys.baseline_fs_with_diff(baseline_builder);
}
