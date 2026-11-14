//! Unit tests of WIDTH_BUCKET's General Rules (RULES.md R-TEST-1, R-TEST-8):
//! every bucket against a brute force of GR17, of exact, approximate and
//! decimal floating-point operands, rising and falling; the product found
//! without forming it; the failures.

use super::*;
use crate::value::DOUBLE_PRECISION;

/// DOUBLE PRECISION, an operand's type.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };

/// INTEGER, a count's type.
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// A call bound with a count of `count`.
fn bound(count: DataType) -> WidthBucket {
    let f = WidthBucket::default();
    f.return_type(&[DOUBLE, DOUBLE, DOUBLE, count]).unwrap();
    f
}

/// The exact number `units` × 10^-`scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// The text of a call's result, or of its failure.
fn shown(r: Result<Value>) -> String {
    r.map_or_else(|e| format!("{:?}", e.state), |v| v.to_string())
}

/// GR17 by brute force, of tenths: the operand `o`, the bounds `b1` and
/// `b2`, `wbc` buckets; each bucket's lower edge found by stepping.
fn brute(o: i128, (b1, b2): (i128, i128), wbc: i128) -> i128 {
    let rising = b1 < b2;
    let (before, past) = if rising { (o < b1, o >= b2) } else { (o > b1, o <= b2) };
    if before {
        return 0;
    }
    if past {
        return wbc.checked_add(1).unwrap();
    }
    // The greatest k with k × width ≤ wbc × distance, by counting up.
    let distance = o.checked_sub(b1).unwrap().abs();
    let width = b2.checked_sub(b1).unwrap().abs();
    let mut k = 0_i128;
    while k.checked_add(1).unwrap().checked_mul(width).unwrap() <= wbc.checked_mul(distance).unwrap() {
        k = k.checked_add(1).unwrap();
    }
    k.checked_add(1).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR17d, GR17e] each operand from
/// below bound 1 to past bound 2, by tenths, of bounds rising and falling
/// and counts 1 to 7, gives the bucket a brute force of GR17 gives, exact,
/// approximate and decimal floating-point alike: 0 before bound 1, WBC+1
/// at or past bound 2, and floor(WBC × distance / width) + 1 inside.
///
/// Decision (GR17d i, ii; GR17e i, ii): before bound 1, at or past bound 2,
/// or neither, each reached in each direction, the edges themselves
/// included.
#[test]
fn buckets_are_general_rule_17_6_31() {
    let f = bound(INTEGER);
    let decfloat = |v: i128| Value::DecFloat(DecFloat::new(v, -1, MAX_DECFLOAT_PRECISION).unwrap());
    let approx = |v: i128| Value::Approx(DecFloat::new(v, -1, MAX_DECFLOAT_PRECISION).unwrap().to_f64());
    for (b1, b2) in [(0, 50), (50, 0), (-15, 25), (25, -15), (3, 4)] {
        for wbc in 1..=7 {
            for o in -30..=60 {
                let want = brute(o, (b1, b2), wbc).to_string();
                let count = exact(wbc, 0);
                let of = |at: &dyn Fn(i128) -> Value| shown(f.call(&[at(o), at(b1), at(b2), count.clone()]));
                assert_eq!(of(&|v| exact(v, 1)), want, "exact {o} in {b1}..{b2} of {wbc}");
                assert_eq!(of(&approx), want, "approximate {o} in {b1}..{b2} of {wbc}");
                assert_eq!(of(&decfloat), want, "decimal floating point {o} in {b1}..{b2} of {wbc}");
            }
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR17d iii] of exact operands the
/// bucket is found exactly though WBC × distance is past 128 bits: the
/// greatest count, the widest range, operands a unit and a least unit below
/// bound 2 in the last bucket, one a unit past bound 1 in the first.
#[test]
fn exact_buckets_need_no_wide_product_6_31() {
    let f = bound(INTEGER);
    let most = 999_999_999_999_999_999_i128;
    let (low, high) = (exact(-most, 18), exact(most, 0));
    let count = exact(most, 0);
    assert_eq!(shown(f.call(&[exact(most - 1, 0), low.clone(), high.clone(), count.clone()])), most.to_string());
    let below_top = exact(most * 1_000_000_000_000_000_000 - 1, 18);
    assert_eq!(shown(f.call(&[below_top, low.clone(), high.clone(), count.clone()])), most.to_string());
    assert_eq!(shown(f.call(&[exact(1 - most, 18), low, high, count])), "1");
    // A factor of every bit against the long multiplication of u128.
    for (factor, part, whole) in [(7_u128, 3_u128, 10_u128), (u128::from(u64::MAX), 5, 9), (12_345, 0, 1), (1, 41, 42)] {
        let long = factor.checked_mul(part).unwrap().checked_div(whole).unwrap();
        assert_eq!(product_over(factor, part, whole), Some(long), "{factor} × {part} / {whole}");
    }
    assert_eq!(product_over(3, 2, 0), None, "no whole of 0");
    assert_eq!(product_over(3, 5, 4), None, "a part less than the whole");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 GR17a, GR17b, GR17c, GR17d ii]
/// a null operand gives null whatever the others; a count of 0 or below,
/// or equal bounds, raise 2201G; WBC+1 past the count's type, INTEGER of
/// 10 digits, raises 22003,
/// though WBC itself fits.
///
/// Decision (GR17b, GR17c): the count above 0 or not, the bounds equal or
/// not, each changing the outcome alone.
#[test]
fn failures_and_nulls_6_31() {
    let f = bound(INTEGER);
    let one = exact(1, 0);
    for null in 0..4 {
        let mut args = vec![one.clone(), exact(0, 0), exact(2, 0), one.clone()];
        args[null] = Value::Null;
        assert_eq!(shown(f.call(&args)), "NULL", "null operand {null}");
    }
    let state = |args: &[Value]| f.call(args).unwrap_err().state.unwrap();
    assert_eq!(state(&[one.clone(), exact(0, 0), exact(2, 0), exact(0, 0)]), SqlState::INVALID_ARGUMENT_FOR_WIDTH_BUCKET);
    assert_eq!(state(&[one.clone(), exact(0, 0), exact(2, 0), exact(-3, 0)]), SqlState::INVALID_ARGUMENT_FOR_WIDTH_BUCKET);
    assert_eq!(state(&[one.clone(), exact(2, 0), exact(20, 1), one.clone()]), SqlState::INVALID_ARGUMENT_FOR_WIDTH_BUCKET);
    assert_eq!(shown(f.call(&[one.clone(), exact(0, 0), exact(2, 0), one.clone()])), "1");
    let top = exact(9_999_999_999, 0);
    assert_eq!(state(&[exact(5, 0), exact(0, 0), exact(2, 0), top.clone()]), SqlState::NUMERIC_VALUE_OUT_OF_RANGE);
    assert_eq!(state(&[exact(-5, 0), exact(2, 0), exact(0, 0), top.clone()]), SqlState::NUMERIC_VALUE_OUT_OF_RANGE);
    assert_eq!(shown(f.call(&[exact(-5, 0), exact(0, 0), exact(2, 0), top.clone()])), "0");
    assert_eq!(shown(bound(COUNTED).call(&[exact(5, 0), exact(0, 0), exact(2, 0), top])), "10000000000");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.31 SR21] the count is exact of scale
/// 0, each operand a number, SQLCODE -103 if not; the result is of the
/// count's type; a `?` count NUMERIC(18, 0), a `?` operand or bound of the
/// first other operand or bound not one, none if each is.
#[test]
fn types_are_syntax_rule_21_6_31() {
    let f = WidthBucket::default();
    let money = DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Numeric };
    let code = |args: &[DataType]| WidthBucket::default().return_type(args).unwrap_err().code;
    assert_eq!(code(&[INTEGER, INTEGER, INTEGER, money.clone()]), Code::SyntaxRule);
    assert_eq!(code(&[INTEGER, INTEGER, INTEGER, DOUBLE]), Code::SyntaxRule);
    assert_eq!(code(&[DataType::Boolean, INTEGER, INTEGER, INTEGER]), Code::SyntaxRule);
    assert_eq!(f.return_type(&[money.clone(), DOUBLE, INTEGER, INTEGER]).unwrap(), INTEGER);
    assert_eq!(f.parameter_type(COUNT, &[None, None, None, None]), Some(ParameterType::Of(COUNTED)));
    let known = [None, Some(money.clone()), Some(INTEGER), None];
    assert_eq!(f.parameter_type(0, &known), Some(ParameterType::Of(money)));
    assert_eq!(f.parameter_type(1, &known), Some(ParameterType::Of(INTEGER)));
    assert_eq!(f.parameter_type(0, &[None, None, None, Some(INTEGER)]), None);
}
