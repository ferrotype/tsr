//! The harness's `createProgram` (`harnessutil.go`): when the options set
//! `incremental`, the compiled program is wrapped in the incremental program
//! over the build info the harness's test reader reads from the program's
//! file system, so its diagnostics come through the incremental state and
//! its emit ends with the build info.
use std::sync::Arc;
use tsr_compiler::{CheckedProgram, ProgramLike};
use tsr_incremental::{
    create_host, new_build_info_reader, new_program, read_build_info_program, BuildInfo,
    BuildInfoReader, CompilerHost, ProgramCompilerHost,
};
use tsr_tsoptions::ParsedCommandLine;

// source: tsc/internal/testutil/harnessutil/harnessutil.go:testBuildInfoReader
struct TestBuildInfoReader {
    inner: Box<dyn BuildInfoReader>,
}

impl BuildInfoReader for TestBuildInfoReader {
    // source: tsc/internal/testutil/harnessutil/harnessutil.go:testBuildInfoReader.ReadBuildInfo
    fn read_build_info(&self, config: &ParsedCommandLine) -> Option<BuildInfo> {
        let mut r = self.inner.read_build_info(config)?;
        r.version = tsr_jsstring::JsString::from_bytes(tsr_core::version().as_bytes());
        Some(r)
    }
}

// source: tsc/internal/testutil/harnessutil/harnessutil.go:getTestBuildInfoReader
fn get_test_build_info_reader(host: Arc<dyn CompilerHost>) -> TestBuildInfoReader {
    TestBuildInfoReader {
        inner: new_build_info_reader(host),
    }
}

/// The program `createProgram` returns: the compiler's, or the incremental
/// program over it.
pub enum HarnessProgram {
    Program(Arc<CheckedProgram>),
    Incremental(Box<tsr_incremental::Program>),
}

impl HarnessProgram {
    /// The program as the harness's `compiler.ProgramLike`.
    pub fn program_like(&self) -> &dyn ProgramLike {
        match self {
            Self::Program(program) => program.as_ref(),
            Self::Incremental(program) => program.as_ref(),
        }
    }

    /// The loaded program underneath.
    pub fn program(&self) -> &Arc<tsr_compiler::Program> {
        self.program_like().checked_program().program()
    }
}

// source: tsc/internal/testutil/harnessutil/harnessutil.go:createProgram
pub fn create_program(program: Arc<CheckedProgram>) -> Result<HarnessProgram, tsr_compiler::Error> {
    if program.program().config().options.incremental.is_true() {
        return incremental_program(program);
    }
    Ok(HarnessProgram::Program(program))
}

/// `createProgram`'s incremental program: the old program read from the
/// build info with the test reader, and the incremental program over both.
pub fn incremental_program(
    program: Arc<CheckedProgram>,
) -> Result<HarnessProgram, tsr_compiler::Error> {
    let config = program.program().config();
    let host: Arc<dyn CompilerHost> = Arc::new(ProgramCompilerHost::new(program.program().clone()));
    let old_program = read_build_info_program(
        config,
        &get_test_build_info_reader(host.clone()),
        host.as_ref(),
    );
    let incremental_program = new_program(
        program,
        old_program.as_ref(),
        create_host(host),
        None,
        false,
    )?;
    Ok(HarnessProgram::Incremental(Box::new(incremental_program)))
}
