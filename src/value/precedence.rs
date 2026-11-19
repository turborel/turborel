//! [ISO/IEC 9075-2:2023 §9.7] Type precedence lists: which types a value
//! of a type may be given to a parameter of, by the type designators it
//! precedes, and in what order. A user-defined cast is appropriate to a
//! value only if its parameter's designator is in the value's list [§4.13,
//! §6.13 SR12]; of the routines of a name, an argument's list picks the
//! one whose parameter's designator comes first in it [§9.6].
//!
//! \[DESIGN\] Each exact numeric type is of the designator NUMERIC and each
//! approximate one of FLOAT (RULES.md D74): every exact type is one type
//! of a precision and a scale here, whatever its name, and every
//! approximate one a type of a binary precision, so SMALLINT, INTEGER,
//! BIGINT, NUMERIC and DECIMAL hold the same values alike and take each
//! other's [§9.7 SR9b vi, vii]. Without Feature F054, DATE's list is DATE
//! alone [§9.7 CR1]. No type has JSON, which turborel does not have, in its
//! list.

use super::DataType;
use super::types::family::family_of;

/// The places in a type precedence list [ISO/IEC 9075-2:2023 §9.7]: the
/// first, of the type's own designator, and those after it; no list here
/// has more than three (DECFLOAT's, third of an exact type's).
const FIRST: u8 = 0;
/// The second place.
const SECOND: u8 = 1;
/// The third place.
const THIRD: u8 = 2;

/// [ISO/IEC 9075-2:2023 §9.7] Where the type designator of `p`, a
/// parameter's type, is in the type precedence list of `t`, from 0, the
/// first; none if it is not in it. Two designators of one place have the
/// same precedence [SR9c iv 2 B]. Each recursion is into an element type,
/// so it is bounded by how deep types nest (`types::MAX_TYPE_NESTING`).
pub fn position(p: &DataType, t: &DataType) -> Option<u8> {
    match p {
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. } => of_string(p, t),
        DataType::Exact { .. } | DataType::Approximate { .. } | DataType::DecFloat { .. } => of_number(p, t),
        // [SR10, SR11] an interval type of its class.
        DataType::Interval(q) => matches!(t, DataType::Interval(r) if r.year_month() == q.year_month()).then_some(FIRST),
        // [SR12 to SR15] each of these of itself alone.
        DataType::Boolean => matches!(t, DataType::Boolean).then_some(FIRST),
        DataType::Date => matches!(t, DataType::Date).then_some(FIRST),
        DataType::Time { .. } => matches!(t, DataType::Time { .. }).then_some(FIRST),
        DataType::Timestamp { .. } => matches!(t, DataType::Timestamp { .. }).then_some(FIRST),
        // [SR17] the element type's list, of collections of one kind,
        // ARRAY or MULTISET, each designator in its element's place.
        DataType::Array(a) => {
            if let DataType::Array(b) = t {
                position(&a.element, &b.element)
            } else {
                None
            }
        }
        DataType::Multiset(a) => {
            if let DataType::Multiset(b) = t {
                position(&a.element, &b.element)
            } else {
                None
            }
        }
        // [SR4] a structured type's name, among its own and its proper
        // supertypes', the nearest first; [SR18] a reference type's as its
        // referenced type's is.
        DataType::Structured(s) => t.structured_type().and_then(|own| among(&s.name, own)),
        DataType::Reference(r) => {
            if let DataType::Reference(own) = t {
                among(&r.referenced.name, &own.referenced)
            } else {
                None
            }
        }
        // [SR4] a user-defined type's own name, as no distinct type has a
        // proper supertype; [SR19] a row type is no parameter's type.
        DataType::Distinct(_) | DataType::Row(_) | DataType::Json => (p == t).then_some(FIRST),
        DataType::Extension(_) => extension(p, t),
        // [ISO/IEC 9075-14:2006 draft §10.4 SR1] XML, of every XML type.
        DataType::Xml(_) => matches!(t, DataType::Xml(_)).then_some(FIRST),
    }
}

/// \[EXTENSION\] [SR4] A type of a family built in, among its own type
/// and its supertypes', the nearest first, whatever their parameters, as
/// a structured type's (RULES.md D120); another extension
/// type itself alone.
fn extension(p: &DataType, t: &DataType) -> Option<u8> {
    if let (Some((f, want)), Some((g, mut at))) = (family_of(p), family_of(t))
        && std::ptr::addr_eq(f, g)
    {
        for place in FIRST..=u8::MAX {
            if at.name() == want.name() {
                return Some(place);
            }
            at = *f.supertype(at)?;
        }
        return None;
    }
    (p == t).then_some(FIRST)
}

/// [ISO/IEC 9075-2:2023 §9.7 SR4] Where `name` is in the type precedence
/// list of the structured type `own`, from 0.
fn among(name: &crate::value::types::UdtName, own: &crate::value::types::StructuredType) -> Option<u8> {
    own.precedence().position(|n| n == name).and_then(|i| u8::try_from(i).ok())
}

/// [ISO/IEC 9075-2:2023 §9.7] Whether the type designator of `p`, a
/// parameter's type, is in the type precedence list of `t`.
pub fn in_precedence_list(p: &DataType, t: &DataType) -> bool {
    position(p, t).is_some()
}

/// [ISO/IEC 9075-2:2023 §9.18 SR10a, SR10d iv] Whether a function whose
/// parameters are of the types `parameters` takes arguments of the types
/// `args`, none of a `<dynamic parameter specification>`: as many, the
/// type precedence list of each typed argument holding the designator of
/// its parameter's type (no parameter has a default, Feature T525 not
/// supported).
pub fn takes(parameters: &[DataType], args: &[Option<DataType>]) -> bool {
    parameters.len() == args.len() && parameters.iter().zip(args).all(|(p, a)| a.as_ref().is_none_or(|t| in_precedence_list(p, t)))
}

/// [ISO/IEC 9075-2:2023 §9.6] Of the routines whose parameters are of the
/// types `routines`, each taking `args` (`takes`), the subject routines,
/// by their places in `routines`: for each argument in turn but a
/// `<dynamic parameter specification>`, while more than one is left,
/// those whose parameter's designator comes first in the argument's type
/// precedence list \[SR6b\].
pub fn subject_routines(routines: &[&[DataType]], args: &[Option<DataType>]) -> Vec<usize> {
    let mut left: Vec<usize> = (0..routines.len()).collect();
    for (r, arg) in args.iter().enumerate() {
        let Some(t) = arg else { continue };
        if left.len() < 2 {
            break;
        }
        let place = |i: usize| routines.get(i).and_then(|ps| ps.get(r)).and_then(|p| position(p, t));
        let first = left.iter().filter_map(|&i| place(i)).min();
        left.retain(|&i| place(i) == first);
    }
    left
}

/// [ISO/IEC 9075-2:2023 §11.60 SR20b] Whether a function whose parameters
/// are of the types `parameters` is told apart from `others`, the other
/// functions of its name in its schema: of arguments of its parameters'
/// types, of those that take them [SR20b iii] it alone is the subject
/// routine [SR20b iv]. \[DESIGN\] The values "arbitrarily selected" of a
/// type compatible with each parameter's [SR20b ii] are of the
/// parameter's type itself, as of values of another type a function might
/// not take its own arguments.
pub fn told_apart(parameters: &[DataType], others: &[&[DataType]]) -> bool {
    let args: Vec<Option<DataType>> = parameters.iter().cloned().map(Some).collect();
    let mut set: Vec<&[DataType]> = vec![parameters];
    set.extend(others.iter().copied().filter(|o| takes(o, &args)));
    subject_routines(&set, &args) == [0]
}

/// [ISO/IEC 9075-2:2023 §9.7 SR5 to SR8] Where the designator of `p`, a
/// string type, is in the list of `t`, a string type of its family:
/// CHARACTER, then CHARACTER VARYING, then CHARACTER LARGE OBJECT, of a
/// fixed-length string; CHARACTER VARYING, then CHARACTER LARGE OBJECT, of
/// a varying one; CHARACTER LARGE OBJECT of a large object [§4.3.1]; and
/// so of binary strings.
fn of_string(p: &DataType, t: &DataType) -> Option<u8> {
    if p.is_character() != t.is_character() || p.is_binary() != t.is_binary() {
        return None;
    }
    // The place of each length a string type is of: fixed, varying, large.
    let rank = |d: &DataType| match d {
        DataType::Character { .. } | DataType::Binary { .. } => Some(0_u8),
        DataType::Varying { .. } | DataType::VarBinary { .. } => Some(1),
        DataType::Lob { .. } => Some(2),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Interval(_)
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => None,
    };
    let (own, of) = (rank(t)?, rank(p)?);
    [FIRST, SECOND, THIRD].get(usize::from(of.checked_sub(own)?)).copied()
}

/// [ISO/IEC 9075-2:2023 §9.7 SR9] Where the designator of `p`, a number
/// type, is in the list of `t`: of an exact type, NUMERIC, then FLOAT,
/// then DECFLOAT [SR9b ii to v] (RULES.md D74); of an approximate type,
/// FLOAT, then DECFLOAT; of DECFLOAT, DECFLOAT alone [SR9b i].
fn of_number(p: &DataType, t: &DataType) -> Option<u8> {
    let exact = matches!(t, DataType::Exact { .. });
    let approximate = matches!(t, DataType::Approximate { .. });
    match p {
        DataType::Exact { .. } => exact.then_some(FIRST),
        DataType::Approximate { .. } => {
            if approximate {
                Some(FIRST)
            } else {
                exact.then_some(SECOND)
            }
        }
        DataType::DecFloat { .. } => {
            if matches!(t, DataType::DecFloat { .. }) {
                Some(FIRST)
            } else if approximate {
                Some(SECOND)
            } else {
                exact.then_some(THIRD)
            }
        }
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Interval(_)
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => None,
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

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.7 SR5, SR6, SR9; RULES.md D74] the
    /// places of the designators in a list: CHARACTER before CHARACTER
    /// VARYING of a fixed-length string, and BINARY before BINARY VARYING;
    /// of an exact number NUMERIC, then FLOAT, then DECFLOAT, each exact type
    /// in NUMERIC's place whatever its name; of an approximate one FLOAT,
    /// then DECFLOAT; an array's designators in its element's places.
    #[test]
    fn designators_take_the_places_of_their_list() {
        let (chars, varying) = (DataType::character(3), DataType::varying(3));
        assert_eq!((position(&chars, &chars), position(&varying, &chars), position(&varying, &varying)), (Some(0), Some(1), Some(0)));
        let (binary, varbinary) = (DataType::Binary { length: 2 }, DataType::VarBinary { length: 2 });
        assert_eq!((position(&binary, &binary), position(&varbinary, &binary)), (Some(0), Some(1)));
        let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
        let bigint = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Bigint };
        let double = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
        let decfloat = DataType::DecFloat { precision: 34 };
        let of_integer = [&bigint, &integer, &double, &decfloat].map(|p| position(p, &integer));
        assert_eq!(of_integer, [Some(0), Some(0), Some(1), Some(2)]);
        assert_eq!([&double, &decfloat, &integer].map(|p| position(p, &double)), [Some(0), Some(1), None]);
        assert_eq!([&decfloat, &double].map(|p| position(p, &decfloat)), [Some(0), None]);
        let (of_ints, of_chars) = (DataType::array(integer.clone(), 4), DataType::array(chars.clone(), 4));
        assert_eq!(position(&DataType::array(double, 2), &of_ints), Some(1));
        assert_eq!(position(&DataType::array(varying, 2), &of_chars), Some(1));
        assert_eq!(position(&of_chars, &chars), None);
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.18 SR10a, SR10d] a function
    /// takes arguments as many as its parameters, each of a type whose list
    /// holds its parameter's designator; a `?` takes any parameter.
    #[test]
    fn a_function_takes_arguments_its_parameters_hold() {
        let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
        let double = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
        let of_integer = [integer.clone()];
        assert!(takes(&of_integer, &[Some(integer.clone())]) && takes(&of_integer, &[None]));
        assert!(!takes(&of_integer, &[Some(double.clone())]) && !takes(&of_integer, &[]));
        assert!(!takes(&of_integer, &[Some(integer.clone()), Some(integer)]));
        assert!(takes(&[double], &[Some(DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric })]));
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.6 SR6b] the subject routines
    /// are kept argument by argument, left to right, those whose parameter
    /// comes first in the argument's list; a `?` decides nothing, so two
    /// told apart only by it are both kept.
    #[test]
    fn subject_routines_are_chosen_left_to_right_9_6() {
        let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
        let double = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
        let (first, second) = ([integer.clone(), double.clone()], [double.clone(), integer.clone()]);
        let both: [&[DataType]; 2] = [&first, &second];
        assert_eq!(subject_routines(&both, &[Some(integer.clone()), Some(integer.clone())]), [0]);
        assert_eq!(subject_routines(&both, &[None, Some(integer.clone())]), [1]);
        assert_eq!(subject_routines(&both, &[None, None]), [0, 1]);
        let (chars, varying) = ([DataType::character(1)], [DataType::varying(1)]);
        let strings: [&[DataType]; 2] = [&varying, &chars];
        assert_eq!(subject_routines(&strings, &[Some(DataType::character(4))]), [1]);
        assert_eq!(subject_routines(&[&[double]], &[Some(integer)]), [0]);
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR20b; RULES.md D74] a
    /// function is told apart from the others of its name if of arguments
    /// of its own parameters' types it alone is the subject routine: INTEGER
    /// from DOUBLE PRECISION, CHARACTER from CHARACTER VARYING, either
    /// order, and any of another arity; INTEGER not from BIGINT nor NUMERIC,
    /// of one place.
    #[test]
    fn functions_are_told_apart_by_their_own_types_11_60() {
        let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
        let bigint = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Bigint };
        let numeric = DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric };
        let double = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
        let (of_integer, of_double) = ([integer.clone()], [double.clone()]);
        assert!(told_apart(&of_integer, &[&of_double]) && told_apart(&of_double, &[&of_integer]));
        let (chars, varying) = ([DataType::character(1)], [DataType::varying(1)]);
        assert!(told_apart(&chars, &[&varying]) && told_apart(&varying, &[&chars]));
        assert!(told_apart(&[integer.clone(), integer.clone()], &[&of_integer]));
        assert!(!told_apart(&of_integer, &[&[bigint]]) && !told_apart(&of_integer, &[&[numeric]]));
        assert!(!told_apart(&of_integer, &[&of_double, std::slice::from_ref(&integer)]));
        assert!(told_apart(&of_integer, &[]));
    }
}
