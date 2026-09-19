//! `jsnum.PseudoBigInt`: a bigint literal's sign and base-10 magnitude, kept as
//! bytes because the scanner hands the checker Go string bytes.

/// The absolute value has no leading zeros; zero is the empty string, and zero
/// is never negative.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PseudoBigInt {
    pub negative: bool,
    pub base10_value: Vec<u8>,
}

impl PseudoBigInt {
    // port: tsc/internal/jsnum/pseudobigint.go:NewPseudoBigInt
    pub fn new(value: &[u8], negative: bool) -> Self {
        let start = value
            .iter()
            .position(|byte| *byte != b'0')
            .unwrap_or(value.len());
        let base10_value = value[start..].to_vec();
        Self {
            negative: negative && !base10_value.is_empty(),
            base10_value,
        }
    }

    /// The literal's text: `-` for negatives, `0` for zero.
    // port: tsc/internal/jsnum/pseudobigint.go:PseudoBigInt.String
    pub fn to_text(&self) -> Vec<u8> {
        if self.base10_value.is_empty() {
            return b"0".to_vec();
        }
        let mut text = Vec::with_capacity(self.base10_value.len() + 1);
        if self.negative {
            text.push(b'-');
        }
        text.extend_from_slice(&self.base10_value);
        text
    }
}

#[cfg(test)]
mod tests {
    use super::PseudoBigInt;

    #[test]
    fn zero_is_never_negative_and_leading_zeros_are_dropped() {
        assert_eq!(PseudoBigInt::new(b"000", true), PseudoBigInt::default());
        assert_eq!(PseudoBigInt::default().to_text(), b"0");
        let value = PseudoBigInt::new(b"0042", true);
        assert_eq!(value.base10_value, b"42");
        assert!(value.negative);
        assert_eq!(value.to_text(), b"-42");
        assert_eq!(PseudoBigInt::new(b"7", false).to_text(), b"7");
    }
}
