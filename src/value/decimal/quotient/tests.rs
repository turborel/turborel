use super::*;

fn q(a: i128, sa: u32, b: i128, sb: u32) -> Result<String> {
    Decimal::new(a, sa).checked_div(Decimal::new(b, sb)).map(|d| d.to_string())
}

/// 10^38 - 1, the greatest units of 38 digits.
const MOST: i128 = 99_999_999_999_999_999_999_999_999_999_999_999_999;

/// Verifies: [X3.135-1986 §5.9 SR4d, GR6b] RULES.md D697: a quotient is
/// at scale max(6, s1 + d2 + 1, s2), truncated toward zero, its trailing
/// zeros dropped down to the operands' larger scale; its leading digit is
/// never lost, however small.
#[test]
fn quotients_keep_their_leading_digits() {
    assert_eq!(q(1, 0, 3, 0).unwrap(), "0.333333");
    assert_eq!(q(2, 0, 3, 0).unwrap(), "0.666666", "truncated, not rounded");
    assert_eq!(q(6, 0, 2, 0).unwrap(), "3");
    assert_eq!(q(1, 0, 4, 0).unwrap(), "0.25");
    assert_eq!(q(-7, 0, 2, 0).unwrap(), "-3.5");
    assert_eq!(q(7, 0, -2, 0).unwrap(), "-3.5");
    assert_eq!(q(1050, 2, 2, 0).unwrap(), "5.25");
    assert_eq!(q(600, 2, 2, 0).unwrap(), "3.00", "not below the operands' scale");
    assert_eq!(q(0, 0, 7, 3).unwrap(), "0.000", "zero at the operands' scale");
    assert_eq!(q(10, 0, 3, 0).unwrap(), "3.333333");
    assert_eq!(q(1, 9, 3, 0).unwrap(), "0.00000000033", "scale s1 + d2 + 1");
    assert_eq!(q(1, 0, 3_000_000_000, 0).unwrap(), "0.00000000033", "scale s1 + d2 + 1");
    assert_eq!(q(1, 0, 1, 7).unwrap(), "10000000.0000000", "scale s2");
}

/// Verifies: [X3.135-1986 §5.9 GR6b] RULES.md D697 at the bounds of 38
/// digits: a quotient of many integer digits has as many fewer fractional
/// ones; one whose integer part is past 38 digits is refused with SQLCODE
/// -301.
#[test]
fn quotients_at_38_digits() {
    assert_eq!(q(MOST, 0, 1, 2).unwrap_err().code, Code::Data, "an integer part of 40 digits");
    assert_eq!(q(MOST, 0, 1, 0).unwrap(), MOST.to_string());
    assert_eq!(q(MOST, 0, -1, 0).unwrap(), (-MOST).to_string());
    assert_eq!(q(MOST, 0, MOST, 0).unwrap(), "1");
    assert_eq!(q(1, 0, MOST, 0).unwrap(), format!("0.{}1", "0".repeat(37)), "a divisor past a tenth of the range");
    assert_eq!(q(MOST, 38, 3, 0).unwrap(), format!("0.{}", "3".repeat(38)));
    let ten_to = |n: u32| 10_i128.pow(n);
    assert_eq!(q(ten_to(30), 0, 3, 0).unwrap(), format!("{}.333333", "3".repeat(30)), "31 digits and 6");
    assert_eq!(q(ten_to(36), 0, 3, 0).unwrap(), format!("{}.33", "3".repeat(36)), "36 digits and 2");
}

/// Verifies: [DESIGN] RULES.md D697: `quotient_bound` gives I = p1 - s1 +
/// s2 integer digits and scale max(6, s1 + p2 + 1), at most 38 - I.
#[test]
fn quotient_bounds_keep_integer_room() {
    assert_eq!(quotient_bound((10, 0), (10, 0)), (21, 11));
    assert_eq!(quotient_bound((3, 0), (1, 0)), (9, 6));
    assert_eq!(quotient_bound((15, 2), (4, 2)), (22, 7));
    assert_eq!(quotient_bound((38, 0), (10, 0)), (38, 0));
    assert_eq!(quotient_bound((30, 0), (10, 0)), (38, 8));
    assert_eq!(quotient_bound((38, 2), (1, 8)), (38, 0), "44 integer digits");
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
