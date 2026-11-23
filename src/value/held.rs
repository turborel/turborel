//! \[DESIGN\] Whether a value is already one of a type (RULES.md D910):
//! a value CAST to the type [ISO/IEC 9075-2:2023 §6.13] and store
//! assignment to it [§9.2] both give back as it is, raising nothing. A
//! site that holds such values, an SQL variable or parameter, is read
//! with no cast made and no copy of a collection's elements.
//!
//! It answers for the null value, exact numbers, character strings, truth
//! values, and arrays of them. Of any other value it says no, and the
//! caller casts or assigns as before.

use super::types::MAX_TYPE_NESTING;
use super::{DataType, Value};

/// Whether CAST of `v` to `t`, and store assignment of `v` to `t`, give
/// `v` itself with neither an exception nor a warning.
pub(crate) fn held(v: &Value, t: &DataType) -> bool {
    held_within(v, t, 0)
}

/// `held`, `depth` levels inside an array. An array nested past
/// `MAX_TYPE_NESTING` is not answered for (RULES.md R-CF-6).
fn held_within(v: &Value, t: &DataType, depth: usize) -> bool {
    if let (Value::Array(elements), DataType::Array(a)) = (v, t) {
        // [§6.13 GR5c i, §9.2 GR3b xxii] within the maximum cardinality,
        // each element one of the element type.
        return depth < MAX_TYPE_NESTING
            && usize::try_from(a.max).is_ok_and(|max| elements.len() <= max)
            && elements.iter().all(|e| held_within(e, &a.element, depth.saturating_add(1)));
    }
    if v.is_null() {
        return answered(t);
    }
    scalar(v, t)
}

/// Whether `t` is a type `held` answers for, whose null value is null
/// cast or assigned to it.
fn answered(t: &DataType) -> bool {
    matches!(t, DataType::Exact { .. } | DataType::Character { .. } | DataType::Varying { .. } | DataType::Boolean | DataType::Array(_))
}

/// `held`, of a value that is neither null nor an array.
fn scalar(v: &Value, t: &DataType) -> bool {
    // [§6.13 GR8a, §9.2 GR3k] of the type's scale, its digits within the
    // precision.
    if let (Value::Exact(d), DataType::Exact { precision, scale, .. }) = (v, t) {
        return d.scale == *scale && d.fits(*precision, *scale);
    }
    // [§6.13 GR11, §9.2 GR3b] of the type's length exactly, so neither
    // padded nor cut.
    if let (Value::Char(s), DataType::Character { length, units }) = (v, t) {
        return usize::try_from(*length).is_ok_and(|l| units.count(s) == l);
    }
    // [§6.13 GR12, §9.2 GR3e] no longer than the type's length.
    if let (Value::Char(s), DataType::Varying { length, units }) = (v, t) {
        return usize::try_from(*length).is_ok_and(|l| units.count(s) <= l);
    }
    matches!((v, t), (Value::Bool(_), DataType::Boolean))
}

#[cfg(test)]
mod tests {
    use super::held;
    use crate::value::{CharUnits, DataType, Decimal, Spelled, Value, assign, cast_warned};

    fn int() -> DataType {
        DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }
    }

    fn fixed(length: u32) -> DataType {
        DataType::Character { length, units: CharUnits::Characters }
    }

    fn varying(length: u32) -> DataType {
        DataType::Varying { length, units: CharUnits::Characters }
    }

    fn double() -> DataType {
        DataType::Approximate { precision: 53, spelled: Spelled::default() }
    }

    fn exact(units: i128, scale: u32) -> Value {
        Value::Exact(Decimal::new(units, scale))
    }

    /// `v` as it is written for a comparison, its scale and padding shown.
    fn shown(v: &Value) -> String {
        format!("{v:?}")
    }

    fn text(s: &str) -> Value {
        Value::Char(s.to_owned())
    }

    /// The values and types the cases below pair, each with each.
    fn cases() -> (Vec<Value>, Vec<DataType>) {
        let values = vec![
            Value::Null,
            exact(7, 0),
            exact(70, 1),
            exact(1_000_000_000, 0),
            exact(-999_999_999, 0),
            text("ab"),
            text("ab "),
            text("abcd"),
            text("é "),
            Value::Bool(true),
            Value::Approx(1.5),
            Value::Array(vec![exact(1, 0), Value::Null].into()),
            Value::Array(vec![exact(1, 0), exact(15, 1)].into()),
            Value::Array(vec![text("ab "), text("abc")].into()),
            Value::Array(vec![text("ab")].into()),
            Value::Array(Vec::new().into()),
            Value::Array(vec![Value::Array(vec![exact(1, 0)].into())].into()),
        ];
        let types = vec![
            int(),
            DataType::Exact { precision: 4, scale: 1, spelled: Spelled::Decimal },
            fixed(3),
            DataType::Character { length: 2, units: CharUnits::Octets },
            varying(3),
            DataType::Boolean,
            double(),
            DataType::array(int(), 2),
            DataType::array(int(), 1),
            DataType::array(fixed(3), 2),
            DataType::array(varying(2), 4),
            DataType::array(DataType::array(int(), 3), 3),
            DataType::array(double(), 3),
        ];
        (values, types)
    }

    /// Verifies: [DESIGN] RULES.md D910. A value said to be held of a
    /// type is the value CAST gives of it [ISO/IEC 9075-2:2023 §6.13],
    /// with no warning, and the value store assignment gives of it
    /// [§9.2], for every pairing of the values and types here. The
    /// pairings a read relies on are said to be held, and those a cast
    /// would change, cut, pad or refuse are not.
    #[test]
    fn a_value_held_of_a_type_is_what_cast_and_assignment_give() {
        let (values, types) = cases();
        let pairs = values.iter().flat_map(|v| types.iter().map(move |t| (v, t))).filter(|(v, t)| held(v, t));
        let mut held_pairs = 0_usize;
        for (v, t) in pairs {
            held_pairs += 1;
            let (cast, warning) = cast_warned(v.clone(), t).unwrap();
            assert_eq!((shown(&cast), warning), (shown(v), None), "{v} cast to {t}");
            assert_eq!(shown(&assign(v.clone(), t, "§9.2").unwrap()), shown(v), "{v} assigned to {t}");
        }
        assert!(held_pairs > 20, "{held_pairs}");
    }

    /// Verifies: [DESIGN] RULES.md D910. The pairings a read relies on
    /// are said to be held, and those a cast would change, cut, pad or
    /// refuse are not. MC/DC of an exact number, its scale against its
    /// digits; of an array, its cardinality against its elements.
    #[test]
    fn what_is_held_and_what_is_not() {
        assert!(held(&exact(7, 0), &int()));
        assert!(held(&text("ab "), &fixed(3)));
        assert!(held(&text("ab"), &varying(3)));
        assert!(held(&Value::Array(vec![exact(1, 0), Value::Null].into()), &DataType::array(int(), 2)));
        // A scale to change, digits past the precision, a string to pad or
        // to cut, a cardinality past the maximum, an element to cast.
        assert!(!held(&exact(70, 1), &int()));
        assert!(!held(&exact(1_000_000_000, 0), &int()));
        assert!(!held(&text("ab"), &fixed(3)));
        assert!(!held(&text("abcd"), &varying(3)));
        assert!(!held(&Value::Array(vec![exact(1, 0), Value::Null].into()), &DataType::array(int(), 1)));
        assert!(!held(&Value::Array(vec![exact(1, 0), exact(15, 1)].into()), &DataType::array(int(), 2)));
        assert!(!held(&Value::Approx(1.5), &double()));
        assert!(!held(&Value::Null, &double()));
    }

    /// Verifies: [DESIGN] RULES.md D910. CAST and store assignment of an
    /// array held of an array type give the array itself, no element
    /// copied; of one with an element to cast, a new array of the casts
    /// [ISO/IEC 9075-2:2023 §6.13 GR5b, §9.2 GR3b xxii]; and of one past
    /// the maximum cardinality, SQLSTATE 2202F as before.
    #[test]
    fn an_array_held_is_cast_and_assigned_as_it_is() {
        let t = DataType::array(fixed(3), 2);
        let (Value::Array(given), kept) = (Value::Array(vec![text("ab "), Value::Null].into()), text("ab ")) else { panic!() };
        for made in [cast_warned(Value::Array(given.clone()), &t).unwrap().0, assign(Value::Array(given.clone()), &t, "§9.2").unwrap()] {
            let Value::Array(made) = made else { panic!("not an array") };
            assert!(std::sync::Arc::ptr_eq(&made, &given));
            assert_eq!(made.first().map(shown), Some(shown(&kept)));
        }
        let short = Value::Array(vec![text("ab"), exact(7, 0)].into());
        assert_eq!(shown(&cast_warned(short, &t).unwrap().0), shown(&Value::Array(vec![text("ab "), text("7  ")].into())));
        let padded = assign(Value::Array(vec![text("ab")].into()), &t, "§9.2").unwrap();
        assert_eq!(shown(&padded), shown(&Value::Array(vec![text("ab ")].into())));
        let long = Value::Array(vec![text("ab "), text("ab "), text("ab ")].into());
        assert_eq!(cast_warned(long.clone(), &t).unwrap_err().sqlstate().as_str(), "2202F");
        assert_eq!(assign(long, &t, "§9.2").unwrap_err().sqlstate().as_str(), "2202F");
    }
}
