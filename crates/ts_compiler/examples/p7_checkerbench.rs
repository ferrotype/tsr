//! S08 checkerbench child (`data/s08/checker-workload.json`): serial frozen
//! acceptance variants, one checker interval each, in one fresh process.
#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../tools/s08/p7/child.rs"]
mod child;
#[path = "../../../tools/s08/p5/corpus.rs"]
mod corpus;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    child::main()
}
