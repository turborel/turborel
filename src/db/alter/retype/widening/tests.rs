use super::*;
use crate::value::Spelled;
use crate::value::datetime::Field;

/// An exact numeric type of precision `precision` and scale `scale`.
fn exact(precision: u32, scale: u32) -> DataType {
    DataType::Exact { precision, scale, spelled: Spelled::Decimal }
}

/// An interval type of fields `start` to `end`, of leading precision
/// `leading` and fractional seconds precision `fraction`.
fn interval(start: Field, end: Field, (leading, fraction): (u8, u8)) -> DataType {
    DataType::Interval(Qualifier { start, end, leading, fraction })
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR15a, SR15b] A character or
/// binary string type gives way to one of its kind at least as long, of
/// the same units; not to a shorter one, nor to another kind.
#[test]
fn strings_widen_to_longer_strings_of_their_kind() {
    assert_eq!(narrowed(&DataType::character(5), &DataType::varying(5)), None);
    assert_eq!(narrowed(&DataType::varying(5), &DataType::varying(9)), None);
    assert_eq!(narrowed(&DataType::varying(9), &DataType::varying(5)), Some("SR15a"));
    let octets = DataType::Varying { length: 9, units: CharUnits::Octets };
    assert_eq!(narrowed(&DataType::varying(5), &octets), Some("SR15a"));
    assert_eq!(narrowed(&DataType::varying(5), &exact(9, 0)), Some("SR15a"));
    assert_eq!(narrowed(&DataType::Binary { length: 4 }, &DataType::VarBinary { length: 4 }), None);
    assert_eq!(narrowed(&DataType::VarBinary { length: 4 }, &DataType::Binary { length: 3 }), Some("SR15b"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR15c] An exact numeric type gives
/// way to one of no less precision, scale or digits before the point;
/// MC/DC of the three: each alone false refuses it.
#[test]
fn exact_numbers_widen_by_precision_scale_and_digits() {
    assert_eq!(narrowed(&exact(5, 2), &exact(7, 3)), None);
    assert_eq!(narrowed(&exact(5, 2), &exact(4, 2)), Some("SR15c"));
    assert_eq!(narrowed(&exact(5, 2), &exact(9, 1)), Some("SR15c"));
    assert_eq!(narrowed(&exact(5, 2), &exact(5, 3)), Some("SR15c"));
    let double = DataType::Approximate { precision: 53, spelled: Spelled::Float };
    assert_eq!(narrowed(&exact(5, 2), &double), Some("SR15c"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR15d to SR15g, SR15j, SR15k] An
/// approximate type widens by precision; a time or timestamp type keeps
/// its zone and widens its fractional precision; DATE, BOOLEAN and any
/// other type stay as they are.
#[test]
fn other_kinds_widen_within_their_kind() {
    let real = DataType::Approximate { precision: 24, spelled: Spelled::Real };
    let double = DataType::Approximate { precision: 53, spelled: Spelled::Float };
    assert_eq!(narrowed(&real, &double), None);
    assert_eq!(narrowed(&double, &real), Some("SR15d"));
    assert_eq!(narrowed(&DataType::Date, &DataType::Date), None);
    assert_eq!(narrowed(&DataType::Date, &DataType::Timestamp { precision: 6, zoned: false }), Some("SR15e"));
    let time = |precision, zoned| DataType::Time { precision, zoned };
    assert_eq!(narrowed(&time(0, false), &time(3, false)), None);
    assert_eq!(narrowed(&time(3, false), &time(0, false)), Some("SR15f"));
    assert_eq!(narrowed(&time(0, false), &time(3, true)), Some("SR15f"));
    let stamp = |precision, zoned| DataType::Timestamp { precision, zoned };
    assert_eq!(narrowed(&stamp(3, true), &stamp(6, true)), None);
    assert_eq!(narrowed(&stamp(3, true), &stamp(6, false)), Some("SR15g"));
    assert_eq!(narrowed(&DataType::Boolean, &DataType::Boolean), None);
    assert_eq!(narrowed(&DataType::Boolean, &DataType::character(5)), Some("SR15j"));
    assert_eq!(narrowed(&DataType::Json, &DataType::Json), None);
    assert_eq!(narrowed(&DataType::DecFloat { precision: 16 }, &DataType::DecFloat { precision: 34 }), Some("SR15k"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR15h, SR15i] An interval type
/// keeps its fields and widens its leading and fractional precisions.
#[test]
fn intervals_keep_their_fields() {
    let days = |leading, fraction| interval(Field::Day, Field::Second, (leading, fraction));
    assert_eq!(narrowed(&days(2, 3), &days(4, 6)), None);
    assert_eq!(narrowed(&days(4, 3), &days(2, 3)), Some("SR15h, SR15i"));
    assert_eq!(narrowed(&days(2, 6), &days(2, 3)), Some("SR15h, SR15i"));
    assert_eq!(narrowed(&days(2, 3), &interval(Field::Hour, Field::Second, (4, 6))), Some("SR15h, SR15i"));
    assert_eq!(narrowed(&interval(Field::Year, Field::Month, (2, 0)), &DataType::Date), Some("SR15h, SR15i"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.19 SR13, SR14] The predefined types
/// are those of no constructed or user-defined type.
#[test]
fn predefined_types_are_told() {
    assert!(predefined(&DataType::character(1)));
    assert!(predefined(&DataType::Json));
    assert!(!predefined(&DataType::array(DataType::Boolean, 3)));
}
