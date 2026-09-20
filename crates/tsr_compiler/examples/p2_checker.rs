//! Named P2 protocol adapter over production Program and CheckerOwner APIs.
mod p2;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    p2::main()
}
