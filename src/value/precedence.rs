//! [ISO/IEC 9075-2:2023 §9.7] Type precedence lists: which types a value
//! of a type may be given to a parameter of, by the type designators it
//! precedes. A user-defined cast is appropriate to a value only if its
//! parameter's designator is in the value's list [§4.13, §6.13 SR12].
//!
//! \[DESIGN\] Only a distinct type's routines have parameters (RULES.md
//! D69), each of its source type or of itself. An exact numeric source is
//! of the designator NUMERIC, an approximate one of FLOAT, as the cast
//! functions are made of them [§11.51 SR7a] (IV223). Without Feature F054,
//! DATE's list is DATE alone [§9.7 CR1].

use super::DataType;

/// [ISO/IEC 9075-2:2023 §9.7] Whether the type designator of `p`, a
/// parameter's type, is in the type precedence list of `t`, as IV223
/// takes a number's designator. Each recursion is into an element type,
/// so it is bounded by how deep types nest (`types::MAX_TYPE_NESTING`).
pub fn in_precedence_list(p: &DataType, t: &DataType) -> bool {
    match p {
        DataType::Character { .. } | DataType::Varying { .. } | DataType::Binary { .. } | DataType::VarBinary { .. } => of_string(p, t),
        // [SR9b iv, vi, vii] NUMERIC, of the greatest precision of an exact
        // type, follows each exact type, and none approximate or decimal
        // floating-point.
        DataType::Exact { .. } => matches!(t, DataType::Exact { .. }),
        // [SR9b iv, vi] FLOAT, of the greatest binary precision (IL044),
        // follows each exact and approximate type.
        DataType::Approximate { .. } => matches!(t, DataType::Exact { .. } | DataType::Approximate { .. }),
        // [SR9b ii] DECFLOAT follows each numeric type.
        DataType::DecFloat { .. } => t.is_numeric(),
        // [SR10, SR11] an interval type of its class.
        DataType::Interval(q) => matches!(t, DataType::Interval(r) if r.year_month() == q.year_month()),
        // [SR12 to SR15] each of these of itself alone.
        DataType::Boolean => matches!(t, DataType::Boolean),
        DataType::Date => matches!(t, DataType::Date),
        DataType::Time { .. } => matches!(t, DataType::Time { .. }),
        DataType::Timestamp { .. } => matches!(t, DataType::Timestamp { .. }),
        // [SR17] the element type's list, of arrays.
        DataType::Array(a) => matches!(t, DataType::Array(b) if in_precedence_list(&a.element, &b.element)),
        // [SR4] a user-defined type's own name, as no distinct type has a
        // proper supertype; [SR19] a row type is no parameter's type.
        DataType::Distinct(_) | DataType::Row(_) | DataType::Extension(_) => p == t,
    }
}

/// [ISO/IEC 9075-2:2023 §9.7 SR5 to SR8] Whether the designator of `p`,
/// a string type, is in the list of `t`: CHARACTER first of a fixed-length
/// string's alone, VARCHAR of either; and so of binary strings.
fn of_string(p: &DataType, t: &DataType) -> bool {
    match p {
        DataType::Character { .. } => matches!(t, DataType::Character { .. }),
        DataType::Varying { .. } => matches!(t, DataType::Character { .. } | DataType::Varying { .. }),
        DataType::Binary { .. } => matches!(t, DataType::Binary { .. }),
        DataType::VarBinary { .. } => matches!(t, DataType::Binary { .. } | DataType::VarBinary { .. }),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Interval(_)
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => false,
    }
}

// Unit tests: each designator's place in the lists §9.7 gives (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Spelled;
    use crate::value::datetime::Field;
    use crate::value::interval::Qualifier;

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.7 SR5 to SR17] a parameter of a
    /// varying string takes a fixed one, but not the reverse; NUMERIC takes
    /// each exact type and no approximate one; FLOAT takes both; DECFLOAT
    /// takes every number; an interval takes its class alone; DATE takes no
    /// TIMESTAMP (F054); an array takes an array whose element its element
    /// takes; a distinct type itself alone.
    #[test]
    fn each_designator_is_in_the_lists_section_9_7_gives() {
        let chars = DataType::character(3);
        let varying = DataType::varying(3);
        assert!(in_precedence_list(&varying, &chars) && !in_precedence_list(&chars, &varying));
        let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
        let real = DataType::Approximate { precision: 24, spelled: Spelled::Real };
        let numeric = DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric };
        assert!(in_precedence_list(&numeric, &integer) && !in_precedence_list(&numeric, &real));
        assert!(in_precedence_list(&real, &integer) && in_precedence_list(&real, &real));
        let decfloat = DataType::DecFloat { precision: 16 };
        assert!(in_precedence_list(&decfloat, &real) && !in_precedence_list(&real, &decfloat));
        let year = DataType::Interval(Qualifier::new(Field::Year, Field::Year, 2, 0));
        let day = DataType::Interval(Qualifier::new(Field::Day, Field::Day, 2, 0));
        assert!(in_precedence_list(&year, &year) && !in_precedence_list(&year, &day));
        let timestamp = DataType::Timestamp { precision: 0, zoned: false };
        assert!(!in_precedence_list(&timestamp, &DataType::Date) && !in_precedence_list(&DataType::Date, &timestamp));
        assert!(in_precedence_list(&DataType::array(numeric.clone(), 3), &DataType::array(integer.clone(), 9)));
        assert!(!in_precedence_list(&DataType::array(numeric, 3), &integer));
        let money = DataType::distinct("S", "MONEY", integer.clone());
        assert!(in_precedence_list(&money, &money) && !in_precedence_list(&money, &integer) && !in_precedence_list(&integer, &money));
    }
}
