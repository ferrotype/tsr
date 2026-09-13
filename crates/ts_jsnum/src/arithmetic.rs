//! Number operations whose JavaScript semantics differ from Rust casts or operators.

use crate::Number;
use num_bigint::BigInt;
use num_traits::ToPrimitive;

impl Number {
    // port: tsc/internal/jsnum/jsnum.go:Number.toInt32
    pub fn to_int32(self) -> i32 {
        let value = self.value();
        let small = value as i32;
        if f64::from(small) == value {
            return small;
        }
        if !value.is_finite() {
            return 0;
        }
        // Reduce before conversion: Rust's saturating float cast alone is not ToInt32.
        (value.trunc() % 4_294_967_296.0) as i64 as i32
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.toUint32
    pub fn to_uint32(self) -> u32 {
        self.to_int32() as u32
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.toShiftCount
    fn shift_count(self) -> u32 {
        self.to_uint32() & 31
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.SignedRightShift
    #[must_use]
    pub fn signed_right_shift(self, other: Self) -> Self {
        Self::new(f64::from(self.to_int32() >> other.shift_count()))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.UnsignedRightShift
    #[must_use]
    pub fn unsigned_right_shift(self, other: Self) -> Self {
        Self::new(f64::from(self.to_uint32() >> other.shift_count()))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.LeftShift
    #[must_use]
    pub fn left_shift(self, other: Self) -> Self {
        Self::new(f64::from(self.to_int32().wrapping_shl(other.shift_count())))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.BitwiseNOT
    #[must_use]
    pub fn bitwise_not(self) -> Self {
        Self::new(f64::from(!self.to_int32()))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.BitwiseOR
    #[must_use]
    pub fn bitwise_or(self, other: Self) -> Self {
        Self::new(f64::from(self.to_int32() | other.to_int32()))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.BitwiseAND
    #[must_use]
    pub fn bitwise_and(self, other: Self) -> Self {
        Self::new(f64::from(self.to_int32() & other.to_int32()))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.BitwiseXOR
    #[must_use]
    pub fn bitwise_xor(self, other: Self) -> Self {
        Self::new(f64::from(self.to_int32() ^ other.to_int32()))
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.Remainder
    #[must_use]
    pub fn remainder(self, divisor: Self) -> Self {
        let value = self.value();
        let divisor = divisor.value();
        Self::new(
            if value.is_nan() || divisor.is_nan() || value.is_infinite() {
                f64::NAN
            } else if divisor.is_infinite() {
                value
            } else if divisor == 0.0 {
                f64::NAN
            } else if value == 0.0 {
                value
            } else {
                value % divisor
            },
        )
    }

    // port: tsc/internal/jsnum/jsnum.go:Number.Exponentiate
    #[must_use]
    pub fn exponentiate(self, exponent: Self) -> Self {
        let base = self.value();
        let exponent = exponent.value();
        if ((base == 1.0 || base == -1.0) && exponent.is_infinite())
            || base == 1.0 && exponent.is_nan()
        {
            return Self::new(f64::NAN);
        }
        if base >= i64::MIN as f64
            && base <= i64::MAX as f64
            && base == base.trunc()
            && exponent >= 0.0
            && exponent <= i64::MAX as f64
            && exponent == exponent.trunc()
            && exponent.is_finite()
        {
            let magnitude = exponent * base.abs().log2();
            if magnitude > 53.0 && magnitude <= f64::MAX.log2() {
                // Here |base| >= 2 and exponent <= 1024. Go accepts the exact
                // 2^63 endpoint because MaxInt64 is rounded to f64. Its native
                // conversion is MinInt64 on amd64 and MaxInt64 on arm64; Rust
                // saturates on both. Preserve the pinned native distinction.
                let integer = if cfg!(target_arch = "x86_64") && base == 9_223_372_036_854_775_808.0
                {
                    i64::MIN
                } else {
                    base as i64
                };
                let integer = BigInt::from(integer).pow(exponent as u32);
                // Go rounds SetInt to a 256-bit big.Float before Float64.
                // Preserve that intermediate rounding, including halfway ties.
                let integer = round_integer_to_256_bits(integer);
                return Self::new(integer.to_f64().expect("integer power converts to f64"));
            }
        }
        Self::new(base.powf(exponent))
    }
}

fn round_integer_to_256_bits(integer: BigInt) -> BigInt {
    let magnitude = integer.magnitude();
    let bits = magnitude.bits();
    if bits <= 256 {
        return integer;
    }
    let discarded = bits - 256;
    let mut leading = magnitude >> discarded;
    let midpoint = magnitude.bit(discarded - 1);
    let below_midpoint = magnitude
        .trailing_zeros()
        .is_some_and(|zeros| zeros < discarded - 1);
    if midpoint && (below_midpoint || leading.bit(0)) {
        leading += 1u32;
    }
    BigInt::from_biguint(integer.sign(), leading << discarded)
}

#[cfg(test)]
mod tests {
    use crate::Number;

    #[test]
    fn javascript_integer_conversion_wraps_instead_of_saturating() {
        for (input, expected) in [
            (4_294_967_295.0, -1),
            (4_294_967_296.0, 0),
            (-4_294_967_297.75, -1),
            (2_147_483_648.0, i32::MIN),
            (f64::INFINITY, 0),
            (f64::NAN, 0),
        ] {
            assert_eq!(Number::new(input).to_int32(), expected);
        }
        assert_eq!(
            Number::new(-1.0)
                .unsigned_right_shift(Number::new(0.0))
                .value(),
            4_294_967_295.0
        );
        assert_eq!(
            Number::new(1.0).left_shift(Number::new(63.0)).value(),
            -2_147_483_648.0
        );
    }

    #[test]
    fn remainder_and_power_preserve_javascript_exceptional_values() {
        assert!(Number::new(1.0)
            .exponentiate(Number::new(f64::NAN))
            .value()
            .is_nan());
        assert!(Number::new(-1.0)
            .exponentiate(Number::new(f64::INFINITY))
            .value()
            .is_nan());
        assert_eq!(
            Number::new(f64::NAN).exponentiate(Number::new(0.0)).value(),
            1.0
        );
        assert_eq!(
            Number::new(-0.0)
                .remainder(Number::new(5.0))
                .value()
                .to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            Number::new(3.0).exponentiate(Number::new(34.0)).value(),
            16_677_181_699_666_568.0
        );
        let endpoint = Number::new(9_223_372_036_854_775_808.0)
            .exponentiate(Number::new(3.0))
            .value();
        assert_eq!(
            endpoint,
            if cfg!(target_arch = "x86_64") {
                -2.0f64.powi(189)
            } else {
                2.0f64.powi(189)
            }
        );
    }
}
