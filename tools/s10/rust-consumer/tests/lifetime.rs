// Compile the public-API tests as a genuinely separate Cargo consumer, with
// only its explicitly declared dependencies and no workspace test visibility.
#[path = "../../../../crates/tsr_embed/tests/lifetime.rs"]
mod lifetime;
