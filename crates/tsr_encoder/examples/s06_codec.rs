//! S06 component adapter; parser requests use ts_parser's E1 example.
#[path = "support/component.rs"]
mod component;
#[path = "support/protocol.rs"]
mod protocol;
fn main() {
    if let Err(error) = protocol::run(component::validate, component::execute) {
        eprintln!("S06 component protocol: {error}");
        std::process::exit(2);
    }
}
