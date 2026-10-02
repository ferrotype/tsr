//! Command-line dispatch from the pinned `execute` package.
pub use tsr_tsc as tsc;
pub use tsr_tsc::{fswatch, watchmanager};
mod command;
mod compile;
#[cfg(test)]
mod refusal_tests;
pub use command::{command_line, find_config_file};
/// Historical X0 refusal, retained for consumers inspecting old observations.
pub const COMMAND_LINE_OPERATION: &str = "execute.CommandLine is Phase 4 X1";

/// Names an explicitly unsupported operation without reclassifying other errors.
/// Callers can report a refusal separately from a compiler failure or a panic.
pub fn unsupported_operation(error: &tsr_compiler::Error) -> Option<&'static str> {
    match error {
        tsr_compiler::Error::Unsupported(operation)
        | tsr_compiler::Error::Host(tsr_vfs::Error::Unsupported(operation))
        | tsr_compiler::Error::Checker(tsr_checker::Error::Unsupported(operation)) => {
            Some(operation)
        }
        _ => None,
    }
}
