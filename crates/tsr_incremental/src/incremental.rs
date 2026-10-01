//! Reading a build info and the old program it describes (`incremental.go`).
use crate::build_info::{content_mapper_identities, BuildInfo};
use crate::host::CompilerHost;
use crate::program::Program;
use std::sync::Arc;
use tsr_tsoptions::ParsedCommandLine;

/// `BuildInfoReader`: where a build reads the previous build's build info.
pub trait BuildInfoReader {
    fn read_build_info(&self, config: &ParsedCommandLine) -> Option<BuildInfo>;
}

struct BuildInfoReaderImpl {
    host: Arc<dyn CompilerHost>,
}

impl BuildInfoReader for BuildInfoReaderImpl {
    // port: tsc/internal/execute/incremental/incremental.go:buildInfoReader.ReadBuildInfo
    fn read_build_info(&self, config: &ParsedCommandLine) -> Option<BuildInfo> {
        let build_info_file_name = config.build_info_file_name();
        if build_info_file_name.is_empty() {
            return None;
        }

        // Read build info file
        let data = self
            .host
            .fs()
            .read_file(build_info_file_name.as_bytes())
            .ok()
            .flatten()?;
        let mut build_info = BuildInfo::default();
        let error = tsr_json::unmarshal(
            data.text.as_bytes(),
            &mut build_info,
            tsr_json::Options::default(),
        );
        if error.is_err() {
            return None;
        }
        Some(build_info)
    }
}

// port: tsc/internal/execute/incremental/incremental.go:NewBuildInfoReader
pub fn new_build_info_reader(host: Arc<dyn CompilerHost>) -> Box<dyn BuildInfoReader> {
    Box::new(BuildInfoReaderImpl { host })
}

/// The old program a build info describes, or `None` when there is no build
/// info, it is of another version, it is not an incremental program's, or the
/// content mappers that produced its files have changed.
// port: tsc/internal/execute/incremental/incremental.go:ReadBuildInfoProgram
pub fn read_build_info_program(
    config: &ParsedCommandLine,
    reader: &dyn BuildInfoReader,
    host: &dyn CompilerHost,
) -> Option<Program> {
    // Read buildInfo file
    let build_info = reader.read_build_info(config)?;
    if !build_info.is_valid_version() || !BuildInfo::is_incremental(Some(&build_info)) {
        return None;
    }
    // If any configured content mapper's identity has changed, files it produced may be stale, so the
    // old program cannot be reused.
    let content_mapper_identities =
        content_mapper_identities(host.content_mapper_project().map(AsRef::as_ref)).ok()?;
    if !build_info.content_mapper_identities_match(content_mapper_identities.as_deref()) {
        return None;
    }

    // Convert to information that can be used to create incremental program
    Some(Program::from_snapshot(
        crate::build_info_to_snapshot::build_info_to_snapshot(&build_info, config, host),
    ))
}
