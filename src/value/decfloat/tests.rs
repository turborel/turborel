//! Unit tests: DECFLOAT values, their rounding, arithmetic, conversions,
//! keys and literals (RULES.md R-TEST-1, R-TEST-2, R-TEST-8).

use super::*;

/// The value `s` writes, a `<signed numeric literal>` without an exponent,
/// at `precision` digits.
fn at(s: &str, precision: u32) -> Option<DecFloat> {
    let (negative, body) = s.strip_prefix('-').map_or((false, s), |rest| (true, rest));
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let e = 0_i64.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    DecFloat::of_digits(negative, &format!("{whole}{fraction}"), e, precision)
}

/// The value `s` writes, at 34 digits.
fn d(s: &str) -> DecFloat {
    at(s, MAX_DECFLOAT_PRECISION).unwrap_or_else(|| panic!("{s} is in range"))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.5.2, §6.30 GR7] a value
/// is rounded to its precision half to even (IA192): below half down,
/// above up, a tie to the even digit unless a digit below is not 0, a
/// carry shortening it.
#[test]
fn values_round_half_to_even() {
    let r = |s: &str| at(s, 3).unwrap().to_string();
    assert_eq!(r("1.234"), "1.23", "below half");
    assert_eq!(r("1.236"), "1.24", "above half");
    assert_eq!(r("1.235"), "1.24", "a tie, to even");
    assert_eq!(r("1.245"), "1.24", "a tie, to even");
    assert_eq!(r("1.2450001"), "1.25", "not a tie: a digit below is not 0");
    assert_eq!(r("-1.235"), "-1.24", "of a negative value, as of its magnitude");
    assert_eq!(r("999.5"), "1000", "a carry");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.5.2, §6.1 SR36] a value
/// has one form, trailing zeros in its exponent, so 1.50 is 1.5; an
/// adjusted exponent past 6144 or below -6143 is out of range (IL054),
/// zero never; digits past 38 are rounded too.
#[test]
fn values_have_one_form_and_a_range() {
    assert_eq!(d("1.50"), d("1.5"));
    assert_eq!((d("1.50").coefficient(), d("1.50").exponent()), (15, -1_i32));
    assert_eq!(d("-0.00"), DecFloat::ZERO);
    assert_eq!(DecFloat::new(1, MAX_EXPONENT, 34).map(|v| v.to_string()), Some("1E6144".to_owned()));
    assert!(DecFloat::new(10, MAX_EXPONENT, 34).is_none(), "past the greatest exponent");
    assert!(DecFloat::new(1, MIN_EXPONENT, 34).is_some());
    assert!(DecFloat::new(1, MIN_EXPONENT - 1, 34).is_none(), "below the least");
    assert_eq!(DecFloat::new(0, MAX_EXPONENT * 2, 34), Some(DecFloat::ZERO));
    assert_eq!(at(&format!("1{}", "0".repeat(60)), 34).map(|v| v.to_string()), Some("1E60".to_owned()), "past 38 digits");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.30 GR4, GR7] a sum and
/// a difference are the values of 34 digits nearest them: exact where 34
/// hold them; of operands far apart the larger, rounded as the smaller's
/// sign says.
#[test]
fn sums_round_to_34_digits() {
    assert_eq!(d("0.1").plus(d("0.2")), Some(d("0.3")));
    assert_eq!(d("1.5").minus(d("1.50")), Some(DecFloat::ZERO));
    assert_eq!(d("-2").minus(d("3")).map(|v| v.to_string()), Some("-5".to_owned()));
    assert_eq!(d("2").minus(d("3")).map(|v| v.to_string()), Some("-1".to_owned()));
    let big = DecFloat::new(1, 40, 34).unwrap();
    let tiny = DecFloat::new(3, -30, 34).unwrap();
    assert_eq!(big.plus(tiny), Some(big), "far apart, the larger kept");
    assert_eq!(big.minus(tiny), Some(big), "rounded back up");
    let nines = d(&"9".repeat(34));
    assert_eq!(nines.plus(d("0.5")).map(|v| v.to_string()), Some("1E34".to_owned()), "a tie of an odd digit, up");
    assert_eq!(nines.plus(d("0.4")), Some(nines));
    let near = DecFloat::new(1, 10, 34).unwrap();
    assert_eq!(
        near.minus(DecFloat::new(1, -24, 34).unwrap()).map(|v| v.coefficient()),
        Some(9_999_999_999_999_999_999_999_999_999_999_999_i128)
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.30 GR4, GR7] a product
/// past 128 bits rounds; a quotient's digits are found past the rounding
/// digit, and what remains breaks a tie; a result out of range is none,
/// as is a quotient by 0.
#[test]
fn products_and_quotients_round_to_34_digits() {
    let nines = d(&"9".repeat(34));
    assert_eq!(d("1.1").times(d("1.1")), Some(d("1.21")));
    assert_eq!(nines.times(nines).map(|v| v.to_string()), Some("9.999999999999999999999999999999998E67".to_owned()));
    assert_eq!(d("1").over(d("3")).map(|v| v.to_string()), Some(format!("0.{}", "3".repeat(34))));
    assert_eq!(d("2").over(d("3")).map(|v| v.to_string()), Some(format!("0.{}7", "6".repeat(33))));
    assert_eq!(d("1").over(d("8")), Some(d("0.125")));
    assert_eq!(d("-6").over(d("4")), Some(d("-1.5")));
    assert_eq!(d("1").over(DecFloat::ZERO), None);
    let greatest = DecFloat::new(1, MAX_EXPONENT, 34).unwrap();
    assert_eq!(greatest.times(d("10")), None, "out of range");
    assert_eq!(greatest.plus(greatest).map(|v| v.to_string()), Some("2E6144".to_owned()));
    assert_eq!(DecFloat::new(1, MIN_EXPONENT, 34).unwrap().over(d("10")), None, "below the range");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR8a, GR9a, GR10a]
/// an exact number becomes the same value; an approximate one its exact
/// binary value rounded to the precision; one becomes an exact number at
/// a scale, its other digits truncated toward zero (IA002), and an `f64`
/// correctly rounded; one too large is none, or infinite.
#[test]
fn values_convert_to_and_from_other_numbers() {
    assert_eq!(DecFloat::of_decimal(&Decimal::new(-12_345, 2)), Some(d("-123.45")));
    assert_eq!(DecFloat::of_f64(0.1, 34).map(|v| v.to_string()), Some("0.1000000000000000055511151231257827".to_owned()));
    assert_eq!(DecFloat::of_f64(0.1, 16), Some(d("0.1")));
    assert_eq!(DecFloat::of_f64(-2.5, 34), Some(d("-2.5")));
    assert_eq!(DecFloat::of_f64(0.0, 34), Some(DecFloat::ZERO));
    assert!(DecFloat::of_f64(f64::NAN, 34).is_none() && DecFloat::of_f64(f64::INFINITY, 34).is_none());
    let v = d("-123.456");
    assert_eq!(v.to_decimal(2).map(|x| (x.units, x.scale)), Some((-12_345, 2)), "truncated toward zero");
    assert_eq!(v.to_decimal(5).map(|x| (x.units, x.scale)), Some((-12_345_600, 5)));
    assert_eq!(DecFloat::new(5, -60, 34).unwrap().to_decimal(2).map(|x| x.units), Some(0));
    assert!(DecFloat::new(1, 60, 34).unwrap().to_decimal(0).is_none());
    assert!((d("0.1").to_f64() - 0.1_f64).abs() < f64::EPSILON);
    assert!(DecFloat::new(1, 400, 34).unwrap().to_f64().is_infinite());
}

/// Every value of up to 3 of the digits 0, 1, 5 and 9, each of the
/// exponents -40, -1, 0, 3 and 6000, of either sign, and 34 nines.
fn some_values() -> Vec<DecFloat> {
    let mut coefficients = vec![0_i128];
    let mut last = vec![0_i128];
    for _ in 0_u8..3 {
        last = last.iter().flat_map(|c| [0_i128, 1, 5, 9].map(|digit| c.saturating_mul(10).saturating_add(digit))).collect();
        coefficients.extend(&last);
    }
    let mut out = Vec::new();
    for c in coefficients {
        for e in [-40, -1, 0, 3, 6000] {
            out.extend([c, c.saturating_neg()].iter().filter_map(|&c| DecFloat::new(c, e, 34)));
        }
    }
    out.push(d(&"9".repeat(34)));
    out
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.2 GR3] values compare
/// by value, against their exact `f64`s where those are finite; their
/// keys, compared as bytes, order as they do; no key begins another; each
/// reads back the same. A malformed key reads as none.
#[test]
fn keys_order_as_values_compare() {
    let all = some_values();
    let keys: Vec<Vec<u8>> = all
        .iter()
        .map(|v| {
            let mut out = Vec::new();
            v.put_key(&mut out);
            out
        })
        .collect();
    for (a, ka) in all.iter().zip(&keys) {
        let mut rest = ka.as_slice();
        assert_eq!(DecFloat::take_key(&mut rest), Some(*a), "{a} reads back");
        assert!(rest.is_empty(), "{a}'s key is read whole");
        for (b, kb) in all.iter().zip(&keys) {
            assert_eq!(ka.cmp(kb), a.cmp(b), "the keys of {a} and {b}");
            assert!(ka == kb || !kb.starts_with(ka));
            if a.exponent() < 300_i32 && b.exponent() < 300_i32 && a.cmp(b) != Ordering::Equal {
                assert_eq!(a.to_f64().partial_cmp(&b.to_f64()), Some(a.cmp(b)), "{a} and {b}");
            }
        }
    }
    for bad in [&[][..], &[3][..], &[2, 0][..]] {
        assert!(DecFloat::take_key(&mut &bad[..]).is_none(), "{bad:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR11c, GR12c] a value
/// is written as an exact numeric literal if its leading digit's exponent
/// is from -6 to 33, else with an exponent, one digit before the point
/// (IV110); its numeral writes every digit.
#[test]
fn values_are_written_as_literals() {
    assert_eq!(d("0").to_string(), "0");
    assert_eq!(d("-123.45").to_string(), "-123.45");
    assert_eq!(d("100").to_string(), "100");
    assert_eq!(d("0.000001").to_string(), "0.000001");
    assert_eq!(d("0.0000001").to_string(), "1E-7");
    assert_eq!(d("-0.00000012").to_string(), "-1.2E-7");
    assert_eq!(DecFloat::new(1, 33, 34).unwrap().to_string(), format!("1{}", "0".repeat(33)));
    assert_eq!(DecFloat::new(12, 33, 34).unwrap().to_string(), "1.2E34");
    assert_eq!(DecFloat::new(-15, -10, 34).unwrap().numeral(), "-0.0000000015");
    assert_eq!(DecFloat::new(15, 3, 34).unwrap().numeral(), "15000");
}
