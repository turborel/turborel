// Unit tests: the defaults, bounds and representable values §9.36 and
// §9.37 give (RULES.md R-TEST-1, R-TEST-2).

use super::*;

/// The `<signed numeric literal>` of `units` with `scale` digits after the
/// point.
fn number(units: i128, scale: u32) -> SignedNumber {
    SignedNumber { value: Some(Decimal::new(units, scale)), at: 0 }
}

/// SMALLINT, of 5 decimal digits here.
fn smallint() -> DataType {
    DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.36 SR10] a value is taken when it is
/// whole and its type's precision holds it, its digits after a point all
/// 0; refused (SQLCODE -103) if it has a fraction, more digits, or is
/// approximate. MC/DC: whole and held; not whole; not held.
#[test]
fn representable_values_9_36() {
    assert_eq!(integer(number(99_999, 0), smallint()).ok(), Some(99_999));
    assert_eq!(integer(number(-99_999, 0), smallint()).ok(), Some(-99_999));
    assert_eq!(integer(number(1_200, 2), smallint()).ok(), Some(12), "12.00");
    let refused = |n: SignedNumber| integer(n, smallint()).err().map(|e| e.code);
    assert_eq!(refused(number(125, 1)), Some(Code::SyntaxRule), "12.5");
    assert_eq!(refused(number(100_000, 0)), Some(Code::SyntaxRule));
    assert_eq!(refused(number(i128::MIN, 0)), Some(Code::SyntaxRule), "no absolute value");
    assert_eq!(refused(SignedNumber { value: None, at: 0 }), Some(Code::SyntaxRule), "approximate");
    assert_eq!(greatest_of(DEFAULT_TYPE), 999_999_999_999_999_999);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.36 SR5, SR7, SR8, SR11, SR13] the
/// bounds of no options: 1 to the type's greatest ascending (ID113,
/// ID112), its least to -1 descending, NO CYCLE; an increment of 0, or a
/// maximum not greater than the minimum, refused. MC/DC: maximum above
/// the minimum, equal, below.
#[test]
fn bounds_by_default_9_36() {
    let none = SequenceOptions::default();
    let up = bounds(&none, 1, smallint(), None).unwrap();
    assert_eq!((up.minimum, up.maximum, up.cycle), (1, 99_999, false));
    let down = bounds(&none, -1, smallint(), None).unwrap();
    assert_eq!((down.minimum, down.maximum), (-99_999, -1));
    assert_eq!(bounds(&none, 0, smallint(), None).err().map(|e| e.code), Some(Code::SyntaxRule));
    let given = |max: i128, min: i128| SequenceOptions {
        maximum: Some(Some(number(max, 0))),
        minimum: Some(Some(number(min, 0))),
        ..SequenceOptions::default()
    };
    assert!(bounds(&given(6, 5), 1, smallint(), None).is_ok());
    assert_eq!(bounds(&given(5, 5), 1, smallint(), None).err().map(|e| e.code), Some(Code::SyntaxRule));
    assert_eq!(bounds(&given(4, 5), 1, smallint(), None).err().map(|e| e.code), Some(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.37 SR6 to SR8] an alteration keeps
/// each bound and the cycle option it does not give; NO MAXVALUE gives the
/// default again, as of the increment it now has.
#[test]
fn bounds_kept_by_an_alteration_9_37() {
    let before = SequenceDesc {
        name: QualifiedName { auth: "A".into(), ident: "S".into() },
        data_type: smallint(),
        start: 3,
        minimum: 3,
        maximum: 9,
        increment: 1,
        cycle: true,
        epoch: 1,
        origin: 3,
        usage: Vec::new(),
    };
    let kept = bounds(&SequenceOptions::default(), 2, smallint(), Some(&before)).unwrap();
    assert_eq!((kept.minimum, kept.maximum, kept.increment, kept.cycle), (3, 9, 2, true));
    let reset = SequenceOptions { maximum: Some(None), cycle: Some(false), ..SequenceOptions::default() };
    let reset = bounds(&reset, 1, smallint(), Some(&before)).unwrap();
    assert_eq!((reset.minimum, reset.maximum, reset.cycle), (3, 99_999, false));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.36 SR12] a start is within the bounds,
/// both included. MC/DC: below, at each end, above.
#[test]
fn a_start_within_the_bounds_9_36() {
    let values = Values { increment: 1, maximum: 9, minimum: 3, cycle: false };
    let within = |v: i128| within(v, &values, "§9.36 SR12", "start").is_ok();
    assert_eq!([within(2), within(3), within(9), within(10)], [false, true, true, false]);
}

/// Verifies: [DESIGN] RULES.md D62: a statement's epochs are given anew,
/// each after the last; a record's are those it logged, in order, and one
/// too few is refused (SQLCODE -910); either way the counters move past
/// each, and the epochs taken are given to log.
#[test]
fn epochs_given_or_logged() {
    let counters = Counters::default();
    let mut fresh = Epochs::fresh();
    assert!(fresh.are_fresh());
    assert_eq!((fresh.next(&counters).unwrap(), fresh.next(&counters).unwrap()), (1, 2));
    assert_eq!(fresh.given(), [1, 2]);
    let mut logged = Epochs::logged(&[7]);
    assert!(!logged.are_fresh());
    assert_eq!(logged.next(&counters).unwrap(), 7);
    assert_eq!(logged.next(&counters).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(logged.given(), [7]);
    assert_eq!(counters.epoch().unwrap(), 8, "past the epoch logged");
}
