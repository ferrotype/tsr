use num_bigint::{BigInt, BigUint, Sign};

/// Scanner bigint normalization. The default branch intentionally accepts
/// opaque bytes; only a recognized second-byte radix marker invokes parsing.
///
/// # Panics
/// A malformed radix input panics with its exact bytes encoded in hex. The
/// oracle compares the invalid-bigint reason and those bytes, retaining Go's
/// differently quoted raw panic text alongside the Rust text.
/// port: tsc/internal/jsnum/pseudobigint.go:ParsePseudoBigInt
pub fn parse_pseudo_big_int(bytes: &[u8]) -> Vec<u8> {
    let bytes = bytes.strip_suffix(b"n").unwrap_or(bytes);
    if !bytes
        .get(1)
        .is_some_and(|byte| matches!(byte, b'b' | b'B' | b'o' | b'O' | b'x' | b'X'))
    {
        let start = bytes
            .iter()
            .position(|byte| *byte != b'0')
            .unwrap_or(bytes.len());
        return if start == bytes.len() {
            b"0".to_vec()
        } else {
            bytes[start..].to_vec()
        };
    }
    parse_go_big_int(bytes)
        .unwrap_or_else(|| panic!("Failed to parse big int (hex): {}", Hex(bytes)))
        .to_string()
        .into_bytes()
}

/// big.Int.SetString(base=0) permits an underscore after a radix prefix or
/// between digits, but not consecutive/trailing separators. num-bigint's
/// more permissive underscore parser must never define this boundary.
pub(crate) fn parse_go_big_int(mut bytes: &[u8]) -> Option<BigInt> {
    let sign = if let Some(rest) = bytes.strip_prefix(b"-") {
        bytes = rest;
        Sign::Minus
    } else {
        bytes = bytes.strip_prefix(b"+").unwrap_or(bytes);
        Sign::Plus
    };
    let (radix, prefix_len) = if bytes.len() >= 2 {
        match &bytes[..2] {
            b"0b" | b"0B" => (2, 2),
            b"0o" | b"0O" => (8, 2),
            b"0x" | b"0X" => (16, 2),
            _ if bytes[0] == b'0' => (8, 1),
            _ => (10, 0),
        }
    } else {
        (10, 0)
    };
    let digits = &bytes[prefix_len..];
    let mut previous_digit = prefix_len != 0;
    let mut digit_count = 0;
    for &byte in digits {
        if byte == b'_' {
            if !previous_digit {
                return None;
            }
            previous_digit = false;
        } else {
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => return None,
            };
            if digit >= radix {
                return None;
            }
            previous_digit = true;
            digit_count += 1;
        }
    }
    if digit_count == 0 || !previous_digit {
        return None;
    }
    // Go permits the separator immediately after a prefix; num-bigint insists
    // its digit string begin with a digit, after which it ignores separators.
    let digits = digits.strip_prefix(b"_").unwrap_or(digits);
    BigUint::parse_bytes(digits, radix).map(|magnitude| BigInt::from_biguint(sign, magnitude))
}

struct Hex<'a>(&'a [u8]);
impl std::fmt::Display for Hex<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
