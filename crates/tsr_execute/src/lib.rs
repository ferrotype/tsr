//! Command-line dispatch from the pinned `execute` package.
pub use tsr_tsc as tsc;
pub use tsr_tsc::{fswatch, unsupported, watchmanager, Unsupported};
mod command;
mod compile;
pub use command::{command_line, find_config_file};
/// Historical X0 refusal, retained for consumers inspecting old observations.
pub const COMMAND_LINE_OPERATION: &str = "execute.CommandLine is Phase 4 X1";
