//! Phase 2 C5.5: the recorded fourslash services replay
//! (`scripts/phase2_services.py replay`).
#[path = "../../../tools/phase2/services/replay/mod.rs"]
mod replay;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    replay::main()
}
