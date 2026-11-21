use super::*;

fn q(a: i128, sa: u32, b: i128, sb: u32) -> Result<String> {
    Decimal::new(a, sa).checked_div(Decimal::new(b, sb)).map(|d| d.to_string())
}

/// 10^38 - 1, the greatest units of 38 digits.
const MOST: i128 = 99_999_999_999_999_999_999_999_999_999_999_999_999;

/// Verifies: [X3.135-1986 §5.9 SR4d, GR6b] a quotient loses no leading
/// significant digit: it has the most fractional digits that fit 38
/// digits, truncated toward zero, trailing zeros dropped down to the
/// operands' larger scale; an integer part past 38 digits, or a nonzero
/// quotient below 10^-38, is refused with SQLCODE -301 rather than given
/// wrong.
#[test]
fn quotients_keep_their_leading_digits() {
    assert_eq!(q(1, 0, 3, 0).unwrap(), format!("0.{}", "3".repeat(38)));
    assert_eq!(q(2, 0, 3, 0).unwrap(), format!("0.{}", "6".repeat(38)), "truncated, not rounded");
    assert_eq!(q(6, 0, 2, 0).unwrap(), "3");
    assert_eq!(q(1, 0, 4, 0).unwrap(), "0.25");
    assert_eq!(q(-7, 0, 2, 0).unwrap(), "-3.5");
    assert_eq!(q(7, 0, -2, 0).unwrap(), "-3.5");
    assert_eq!(q(1050, 2, 2, 0).unwrap(), "5.25");
    assert_eq!(q(600, 2, 2, 0).unwrap(), "3.00", "not below the operands' scale");
    assert_eq!(q(0, 0, 7, 3).unwrap(), "0.000", "zero at the operands' scale");
    assert_eq!(q(10, 0, 3, 0).unwrap(), format!("3.{}", "3".repeat(37)), "38 digits in all");
}

/// Verifies: [X3.135-1986 §5.9 GR6b] at the bounds of 38 digits: a
/// quotient below 10^-38, or of an integer part past 38 digits, is
/// refused with SQLCODE -301; one at them is exact.
#[test]
fn quotients_at_38_digits() {
    assert_eq!(q(1, 38, 1_000, 0).unwrap_err().code, Code::Data, "below 10^-38");
    assert_eq!(q(1, 35, 1_000, 0).unwrap(), format!("0.{}1", "0".repeat(37)));
    assert_eq!(q(MOST, 0, 1, 2).unwrap_err().code, Code::Data, "an integer part of 40 digits");
    assert_eq!(q(MOST, 0, 1, 0).unwrap(), MOST.to_string());
    assert_eq!(q(MOST, 0, -1, 0).unwrap(), (-MOST).to_string());
    assert_eq!(q(MOST, 0, MOST, 0).unwrap(), "1");
    assert_eq!(q(1, 0, MOST, 0).unwrap(), format!("0.{}1", "0".repeat(37)), "a divisor past a tenth of the range");
    assert_eq!(q(MOST, 38, 3, 0).unwrap(), format!("0.{}", "3".repeat(38)));
}

/// Verifies: [X3.135-1986 §5.9 GR6b] the quotient is taken at the scale its
/// digits allow, or one less when its digits are one more than the
/// least the operands' digits give: 99 over 10 at scale 38 would have 39
/// digits, so it is taken at 37.
#[test]
fn quotient_scale_steps_back_once() {
    assert_eq!(q(99, 0, 1, 0).unwrap(), "99");
    assert_eq!(q(99, 0, 9, 0).unwrap(), "11");
    assert_eq!(q(98, 0, 99, 0).unwrap(), format!("0.{}", "98".repeat(19)));
    assert_eq!(q(MOST, 0, 9, 0).unwrap(), "11111111111111111111111111111111111111");
    assert_eq!(Decimal::new(99, 0).widest_over(Decimal::new(10, 0)).map(|d| d.scale), Some(37));
}

/// Verifies: [DESIGN] long division appends a remainder's digits in steps
/// that fit 128 bits, and one at a time past a tenth of the range.
#[test]
fn shifted_quotients_are_exact() {
    assert_eq!(shifted_over(1, 38, 3), Some(33_333_333_333_333_333_333_333_333_333_333_333_333));
    assert_eq!(shifted_over(1, -2, 3), Some(0));
    assert_eq!(shifted_over(500, -2, 3), Some(1));
    assert_eq!(shifted_over(1, -200, 3), Some(0), "a divisor past 128 bits");
    let big = u128::MAX.checked_div(7).unwrap();
    assert_eq!(shifted_over(big - 1, 2, big), Some(99));
    assert_eq!(ten_over(5, 7), Some((7, 1)));
    assert_eq!(shifted_over(u128::MAX, 1, 1), None);
}

/// Verifies: [DESIGN] trailing fractional zeros go in steps down to the
/// floor and no further.
#[test]
fn stripped_stops_at_its_floor() {
    let shape = |d: Decimal| (d.units, d.scale);
    assert_eq!(shape(Decimal::new(25 * 10_i128.pow(36), 38).stripped(0)), (25, 2));
    assert_eq!(shape(Decimal::new(25 * 10_i128.pow(36), 38).stripped(5)), (25_000, 5));
    assert_eq!(shape(Decimal::new(123, 3).stripped(0)), (123, 3));
    assert_eq!(shape(Decimal::new(1_230, 4).stripped(0)), (123, 3), "one zero, an odd count");
}

/// Verifies: [ISO/IEC 9075:1992 §6.5 GR2b ii] a quotient at a fixed scale,
/// as AVG's, is truncated there and judged against 38 digits.
#[test]
fn quotients_at_a_scale() {
    let at = |a: i128, b: i128, scale: u32| Decimal::new(a, 0).quotient_at(Decimal::new(b, 0), scale).map(|d| d.to_string());
    assert_eq!(at(5, 3, 2).unwrap(), "1.66");
    assert_eq!(at(-5, 3, 2).unwrap(), "-1.66");
    assert_eq!(at(1, 0, 2).unwrap_err().code, Code::Data);
    assert_eq!(at(MOST, 1, 2).unwrap_err().code, Code::Data, "past 38 digits at that scale");
    let hundredth = MOST.checked_div(100).unwrap();
    assert_eq!(at(hundredth, 1, 2).unwrap(), format!("{hundredth}.00"));
}
