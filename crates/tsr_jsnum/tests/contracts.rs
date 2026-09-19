use std::fmt::Write;
use ts_jsnum::{from_string, parse_pseudo_big_int, Number};

#[test]
fn number_grammar_keeps_sign_radix_and_whitespace_distinct() {
    assert_eq!(from_string(b"-0").value().to_bits(), (-0.0_f64).to_bits());
    assert_eq!(
        from_string(b"-0e-9999").value().to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(from_string(b"1e9999").value(), f64::INFINITY);
    assert_eq!(
        from_string("\u{feff}\u{2007}42\u{2029}".as_bytes()).value(),
        42.0
    );
    for bytes in ["\u{85}42", "42\u{200b}", "-0x1", "+0b1", "0x", "1e+", "1_0"] {
        assert!(from_string(bytes.as_bytes()).value().is_nan(), "{bytes:?}");
    }
    assert!(from_string(b"1\xff").value().is_nan());
    assert_eq!(from_string(b"00088").value(), 88.0);
    assert_eq!(
        from_string(b"0x20000000000001").value(),
        9_007_199_254_740_992.0
    );
    assert_eq!(
        from_string(b"0x20000000000003").value(),
        9_007_199_254_740_996.0
    );
}

#[test]
fn formatting_uses_ecmascript_exponent_boundaries() {
    for (value, expected) in [
        (-0.0, "0"),
        (1e-6, "0.000001"),
        (1e-7, "1e-7"),
        (1e20, "100000000000000000000"),
        (1e21, "1e+21"),
        (f64::from_bits(1), "5e-324"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (f64::NAN, "NaN"),
    ] {
        assert_eq!(Number::new(value).to_string(), expected);
    }
}

#[test]
fn pseudo_bigint_preserves_opaque_decimal_branch_and_go_separators() {
    for (input, expected) in [
        (&b"hello"[..], &b"hello"[..]),
        (b"000\xffn", b"\xff"),
        (b"000n", b"0"),
        (b"0b111n", b"7"),
        (b"0x_1n", b"1"),
        (b"0o7_7n", b"63"),
        (b"0x20000000000001n", b"9007199254740993"),
    ] {
        assert_eq!(parse_pseudo_big_int(input), expected);
    }
    for input in [b"0x__1n".as_slice(), b"0x1_n", b"0b2n", b"xb1n", b"abc"] {
        let panic = std::panic::catch_unwind(|| parse_pseudo_big_int(input)).unwrap_err();
        let message = panic.downcast_ref::<String>().unwrap();
        let stripped = input.strip_suffix(b"n").unwrap_or(input);
        let mut hex = String::new();
        for byte in stripped {
            write!(hex, "{byte:02x}").unwrap();
        }
        assert_eq!(message, &format!("Failed to parse big int (hex): {hex}"));
    }
}
