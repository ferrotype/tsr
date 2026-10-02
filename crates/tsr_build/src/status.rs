use crate::*;

/// The sixteen pinned `upToDateStatusType` cases, in source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    ConfigFileNotFound,
    BuildErrors,
    UpstreamErrors,
    UpToDate,
    UpToDateWithUpstreamTypes,
    UpToDateWithInputFileText,
    InputFileMissing,
    OutputMissing,
    InputFileNewer,
    OutOfDateBuildInfoWithPendingEmit,
    OutOfDateBuildInfoWithErrors,
    OutOfDateOptions,
    OutOfDateRoots,
    TsVersionOutOfDate,
    ForceBuild,
    Solution,
}

/// The status data preserves whether its times came from a disk inspection.
/// A just-built task has an output name but no input/output-time payload.
#[derive(Clone, Debug)]
pub struct Status {
    pub kind: StatusKind,
    pub input: JsString,
    pub output: JsString,
    pub input_time: Time,
    pub output_time: Time,
    pub build_info: JsString,
    pub has_times: bool,
    pub ref_has_upstream_errors: bool,
}
impl Status {
    pub(crate) fn plain(kind: StatusKind) -> Self {
        Self {
            kind,
            input: JsString::default(),
            output: JsString::default(),
            input_time: Time::ZERO,
            output_time: Time::ZERO,
            build_info: JsString::default(),
            has_times: false,
            ref_has_upstream_errors: false,
        }
    }
    pub(crate) fn file(kind: StatusKind, file: JsString) -> Self {
        Self {
            output: file,
            ..Self::plain(kind)
        }
    }
    pub(crate) fn pair(kind: StatusKind, input: JsString, output: JsString) -> Self {
        Self {
            input,
            output,
            ..Self::plain(kind)
        }
    }
    // port: tsc/internal/execute/build/uptodatestatus.go:upToDateStatus.isError
    pub fn is_error(&self) -> bool {
        matches!(
            self.kind,
            StatusKind::ConfigFileNotFound | StatusKind::BuildErrors | StatusKind::UpstreamErrors
        )
    }
    // port: tsc/internal/execute/build/uptodatestatus.go:upToDateStatus.isPseudoBuild
    pub fn is_pseudo_build(&self) -> bool {
        matches!(
            self.kind,
            StatusKind::UpToDateWithUpstreamTypes | StatusKind::UpToDateWithInputFileText
        )
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.reportUpToDateStatus
    pub(crate) fn report(
        &self,
        o: &Orchestrator,
        config: &[u8],
        writer: SharedWriter,
    ) -> Result<(), Error> {
        if !o.opts.command.build_options.verbose.is_true() {
            return Ok(());
        }
        use StatusKind::*;
        let mut args = vec![o.relative(config)];
        let message = match self.kind {
            ConfigFileNotFound => d::Project_0_is_out_of_date_because_config_file_does_not_exist,
            UpstreamErrors => {
                args.push(o.relative(self.input.as_bytes()));
                if self.ref_has_upstream_errors { d::Project_0_can_t_be_built_because_its_dependency_1_was_not_built } else { d::Project_0_can_t_be_built_because_its_dependency_1_has_errors }
            }
            BuildErrors => d::Project_0_is_out_of_date_because_it_has_errors,
            UpToDate => {
                if !self.has_times { return Ok(()); }
                args.extend([o.relative(self.input.as_bytes()), o.relative(self.output.as_bytes())]);
                d::Project_0_is_up_to_date_because_newest_input_1_is_older_than_output_2
            }
            UpToDateWithUpstreamTypes => d::Project_0_is_up_to_date_with_d_ts_files_from_its_dependencies,
            UpToDateWithInputFileText => d::Project_0_is_up_to_date_but_needs_to_update_timestamps_of_output_files_that_are_older_than_input_files,
            InputFileMissing => { args.push(o.relative(self.output.as_bytes())); d::Project_0_is_out_of_date_because_input_1_does_not_exist }
            OutputMissing => { args.push(o.relative(self.output.as_bytes())); d::Project_0_is_out_of_date_because_output_file_1_does_not_exist }
            InputFileNewer => { args.extend([o.relative(self.output.as_bytes()), o.relative(self.input.as_bytes())]); d::Project_0_is_out_of_date_because_output_1_is_older_than_input_2 }
            OutOfDateBuildInfoWithPendingEmit => { args.push(o.relative(self.output.as_bytes())); d::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_that_some_of_the_changes_were_not_emitted }
            OutOfDateBuildInfoWithErrors => { args.push(o.relative(self.output.as_bytes())); d::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_that_program_needs_to_report_errors }
            OutOfDateOptions => { args.push(o.relative(self.output.as_bytes())); d::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_there_is_change_in_compilerOptions }
            OutOfDateRoots => { args.extend([o.relative(self.output.as_bytes()), o.relative(self.input.as_bytes())]); d::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_that_file_2_was_root_file_of_compilation_but_not_any_more }
            TsVersionOutOfDate => { args.extend([o.relative(self.output.as_bytes()), JsString::from_bytes(tsr_core::version().as_bytes())]); d::Project_0_is_out_of_date_because_output_for_it_was_generated_with_version_1_that_differs_with_current_version_2 }
            ForceBuild => d::Project_0_is_being_forcibly_rebuilt,
            Solution => return Ok(()),
        };
        o.status_report(writer, message, args)
    }
}
