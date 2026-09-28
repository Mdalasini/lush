//! Shared checked Int/Float arithmetic for compile-time constant evaluation.
//!
//! Reused by later constant-folding passes (step 3+).

use std::fmt;

pub const MIN_INT: i64 = i64::MIN;
pub const MAX_INT: i64 = i64::MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum NumericError {
    IntOverflow,
    DivByZero,
    MinIntDivNegOne,
    MinIntNeg,
    FloatNonFinite,
    IntOutOfRange,
    FloatOutOfRange,
}

impl fmt::Display for NumericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumericError::IntOverflow => write!(f, "integer overflow"),
            NumericError::DivByZero => write!(f, "division by zero"),
            NumericError::MinIntDivNegOne => {
                write!(f, "`MIN_INT / -1` (or `% -1`) is undefined for Int")
            }
            NumericError::MinIntNeg => write!(f, "negating `MIN_INT` overflows"),
            NumericError::FloatNonFinite => {
                write!(f, "Float operation would produce NaN or infinity")
            }
            NumericError::IntOutOfRange => write!(f, "integer literal out of range for Int"),
            NumericError::FloatOutOfRange => {
                write!(f, "float literal outside the finite binary64 range")
            }
        }
    }
}

pub fn parse_int_digits(digits: &str, base: u32) -> Result<u128, NumericError> {
    u128::from_str_radix(digits, base).map_err(|_| NumericError::IntOutOfRange)
}

/// Parse a non-negated Int literal into `i64`. Magnitude `2^63` is rejected here;
/// callers accept it only under direct prefix negation (`MIN_INT`).
pub fn int_literal_value(digits: &str, base: u32) -> Result<i64, NumericError> {
    let v = parse_int_digits(digits, base)?;
    if v > MAX_INT as u128 {
        return Err(NumericError::IntOutOfRange);
    }
    Ok(v as i64)
}

/// Value of a directly negated integer literal token (magnitude may be `2^63`).
pub fn negated_int_literal_value(digits: &str, base: u32) -> Result<i64, NumericError> {
    let v = parse_int_digits(digits, base)?;
    if v == (MAX_INT as u128) + 1 {
        return Ok(MIN_INT);
    }
    if v <= MAX_INT as u128 {
        return Ok(-(v as i64));
    }
    Err(NumericError::IntOutOfRange)
}

pub fn parse_float_literal(raw: &str) -> Result<f64, NumericError> {
    let cleaned: String = raw.chars().filter(|c| *c != '_').collect();
    let v: f64 = cleaned.parse().map_err(|_| NumericError::FloatOutOfRange)?;
    if !v.is_finite() {
        return Err(NumericError::FloatOutOfRange);
    }
    Ok(v)
}

pub fn int_add(a: i64, b: i64) -> Result<i64, NumericError> {
    a.checked_add(b).ok_or(NumericError::IntOverflow)
}
pub fn int_sub(a: i64, b: i64) -> Result<i64, NumericError> {
    a.checked_sub(b).ok_or(NumericError::IntOverflow)
}
pub fn int_mul(a: i64, b: i64) -> Result<i64, NumericError> {
    a.checked_mul(b).ok_or(NumericError::IntOverflow)
}
pub fn int_div(a: i64, b: i64) -> Result<i64, NumericError> {
    if b == 0 {
        return Err(NumericError::DivByZero);
    }
    if a == MIN_INT && b == -1 {
        return Err(NumericError::MinIntDivNegOne);
    }
    Ok(a / b) // truncates toward zero in Rust
}
pub fn int_rem(a: i64, b: i64) -> Result<i64, NumericError> {
    if b == 0 {
        return Err(NumericError::DivByZero);
    }
    if a == MIN_INT && b == -1 {
        return Err(NumericError::MinIntDivNegOne);
    }
    Ok(a % b) // dividend sign
}
pub fn int_neg(a: i64) -> Result<i64, NumericError> {
    a.checked_neg().ok_or(NumericError::MinIntNeg)
}

pub fn float_add(a: f64, b: f64) -> Result<f64, NumericError> {
    finite(a + b)
}
pub fn float_sub(a: f64, b: f64) -> Result<f64, NumericError> {
    finite(a - b)
}
pub fn float_mul(a: f64, b: f64) -> Result<f64, NumericError> {
    finite(a * b)
}
pub fn float_div(a: f64, b: f64) -> Result<f64, NumericError> {
    finite(a / b)
}
pub fn float_neg(a: f64) -> Result<f64, NumericError> {
    finite(-a)
}

fn finite(v: f64) -> Result<f64, NumericError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(NumericError::FloatNonFinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_literal_max_ok() {
        assert_eq!(
            int_literal_value("9223372036854775807", 10).unwrap(),
            MAX_INT
        );
    }

    #[test]
    fn int_literal_past_max_err() {
        assert!(int_literal_value("9223372036854775808", 10).is_err());
    }

    #[test]
    fn negated_min_int() {
        assert_eq!(
            negated_int_literal_value("9223372036854775808", 10).unwrap(),
            MIN_INT
        );
    }

    #[test]
    fn hex_no_wrap() {
        assert!(int_literal_value("FFFFFFFFFFFFFFFF", 16).is_err());
    }

    #[test]
    fn div_truncates_toward_zero() {
        assert_eq!(int_div(-7, 3).unwrap(), -2);
        assert_eq!(int_rem(-7, 3).unwrap(), -1);
    }

    #[test]
    fn min_int_div_neg_one() {
        assert!(matches!(
            int_div(MIN_INT, -1),
            Err(NumericError::MinIntDivNegOne)
        ));
        assert!(matches!(
            int_rem(MIN_INT, -1),
            Err(NumericError::MinIntDivNegOne)
        ));
    }

    #[test]
    fn float_nonfinite() {
        assert!(float_div(1.0, 0.0).is_err());
        assert!(parse_float_literal("1e400").is_err());
    }

    #[test]
    fn float_underflow_ok() {
        assert!(parse_float_literal("1e-400").is_ok());
    }
}
