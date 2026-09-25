//! The copied observer functions must stay byte-identical to the evidence
//! producers, so the driver's frames and digests are theirs.

const S06: &str = include_str!("../../../../../crates/tsr_parser/examples/s06.rs");
const BINDER: &str = include_str!("../../../../../crates/tsr_binder/examples/binder.rs");
const E1_COPY: &str = include_str!("e1.rs");
const BINDER_COPY: &str = include_str!("binder.rs");

/// The text of the top-level `fn name(` through its closing column-zero brace.
fn item<'a>(source: &'a str, name: &str) -> &'a str {
    let header = format!("\nfn {name}(");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("no top-level fn {name}"))
        + 1;
    let end = source[start..]
        .find("\n}\n")
        .unwrap_or_else(|| panic!("fn {name} has no closing brace"))
        + start
        + 3;
    &source[start..end]
}

#[test]
fn e1_observers_are_the_s06_example_functions() {
    for name in ["validate", "diagnostic", "table", "parse"] {
        assert_eq!(
            item(E1_COPY, name),
            item(S06, name),
            "src/e1.rs fn {name} drifted from crates/tsr_parser/examples/s06.rs"
        );
    }
}

#[test]
fn binder_observers_are_the_s07_example_functions() {
    for name in ["validate", "execute"] {
        assert_eq!(
            item(BINDER_COPY, name),
            item(BINDER, name),
            "src/binder.rs fn {name} drifted from crates/tsr_binder/examples/binder.rs"
        );
    }
}
