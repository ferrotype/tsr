//! Numeric text conversions and JavaScript arithmetic used by the scanner and checker.
//!
//! Inputs retain Go string bytes. Float and bigint libraries are called only
//! after the pinned Go grammar has accepted their input; see `SLICE.md`.

mod arithmetic;
mod bigint;
mod pseudobigint;
mod string;

pub use bigint::parse_pseudo_big_int;
pub use pseudobigint::PseudoBigInt;
pub use string::from_string;

/// An IEEE-754 JavaScript Number, including negative zero and non-finite values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Number(f64);

impl Number {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }
    pub const fn value(self) -> f64 {
        self.0
    }
}

impl std::fmt::Display for Number {
    /// port: tsc/internal/jsnum/string.go:Number.String
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            value if value.is_nan() => formatter.write_str("NaN"),
            f64::INFINITY => formatter.write_str("Infinity"),
            f64::NEG_INFINITY => formatter.write_str("-Infinity"),
            value => {
                // Go's safe-integer fast path also renders negative zero as 0.
                const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
                if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value)
                    && (value as i64) as f64 == value
                {
                    write!(formatter, "{}", value as i64)
                } else {
                    formatter.write_str(ryu_js::Buffer::new().format_finite(value))
                }
            }
        }
    }
}
