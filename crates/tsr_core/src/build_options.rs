//! Build-mode options; execution and project scheduling are separate consumers.
use crate::Tristate;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    pub dry: Tristate,
    pub force: Tristate,
    pub verbose: Tristate,
    pub builders: Option<isize>,
    pub stop_build_on_errors: Tristate,
    pub clean: Tristate,
}
