//! The mutations of a compiled program's values read where the frame
//! holds them and of its point changes (RULES.md R-TEST-9, D910 to D914):
//! a value taken as one of a type it is not of, an array cast as held
//! whose elements are not, a probe's value lost as it is read, a probe
//! placed once and not looked under, a candidate row missing a SET value,
//! an element appended in place and lost, a full array appended to, a
//! row written over the wrong slots, and an operand read uncast.

use super::{Mutation, Package};

/// The file of the values held of a type.
const HELD: &str = "src/value/held.rs";
/// The file of the in-place append.
const APPEND: &str = "src/db/routines/program/compiled/append.rs";
/// The unit test of values held against CAST and store assignment.
const HELD_AS_CAST: &str = "value::held::tests::a_value_held_of_a_type_is_what_cast_and_assignment_give";
/// The unit test of arrays cast and assigned.
const ARRAYS: &str = "value::held::tests::an_array_held_is_cast_and_assigned_as_it_is";
/// The unit test of operands that are slots.
const OPERANDS: &str = "exec::eval::calls::an_operand_slot_not_of_its_type_is_cast";
/// The test of point changes probed and set by elements of arrays.
const ELEMENTS: &str = "compiled_points::point_changes_by_elements_of_arrays";
/// The test of arrays grown an element at a time.
const GROWN: &str = "compiled_points::an_array_grows_in_place_as_it_does_as_bound";

/// The mutations of a compiled program's values and point changes.
pub const COMPILED_POINTS: [Mutation; 10] = [
    Mutation {
        name: "points-held-scale",
        package: Package::Core,
        breaks: "an exact number of a smaller scale is taken as one of its type, read with no cast",
        file: HELD,
        original: "return d.scale == *scale && d.fits(*precision, *scale);",
        replacement: "return d.fits(*precision, *scale);",
        integration: &[],
        unit: &[HELD_AS_CAST],
    },
    Mutation {
        name: "points-array-cast-held",
        package: Package::Core,
        breaks: "an array is cast as it is whatever its elements are",
        file: "src/value/types/array.rs",
        original: "if !of_multiset && given.iter().all(|e| held(e, element)) {",
        replacement: "if !of_multiset {",
        integration: &[],
        unit: &[ARRAYS],
    },
    Mutation {
        name: "points-slot-held",
        package: Package::Core,
        breaks: "a slot's value is read uncast whatever its type",
        file: "src/exec/eval.rs",
        original: "    if held(value, &slot.data_type) {\n        return Ok(value.clone());",
        replacement: "    if !value.is_null() {\n        return Ok(value.clone());",
        integration: &[],
        unit: &[OPERANDS],
    },
    Mutation {
        name: "points-operand-held",
        package: Package::Core,
        breaks: "an operand that is a slot is borrowed uncast whatever its value",
        file: "src/exec/eval/called.rs",
        original: "subqueries.slot(slot.index).filter(|value| held(value, &slot.data_type)).map(Ok)",
        replacement: "subqueries.slot(slot.index).map(Ok)",
        integration: &[],
        unit: &[OPERANDS],
    },
    Mutation {
        name: "points-probe-read",
        package: Package::Core,
        breaks: "a probe's value read over the slots where they are is lost",
        file: "src/physical/kept.rs",
        original: "crate::exec::eval(expr, &Vec::new(), &framed).ok()",
        replacement: "crate::exec::eval(expr, &Vec::new(), &framed).ok().map(|_| Value::Null)",
        integration: &[ELEMENTS],
        unit: &[],
    },
    Mutation {
        name: "points-probe-placed-once",
        package: Package::Core,
        breaks: "a point change does not look under the probe its plan placed",
        file: "src/db/write/direct.rs",
        original: "let found = point.entry_reading(st, tx.id, probe, columns)?;",
        replacement: "let found = point.entry_reading(st, tx.id, probe, columns)?.filter(|_| false);",
        integration: &[ELEMENTS],
        unit: &[],
    },
    Mutation {
        name: "points-candidate-in-place",
        package: Package::Core,
        breaks: "the candidate row made of the object row misses its first SET value",
        file: "src/db/assignment.rs",
        original: "for ((i, _), value) in a.set.iter().zip(values) {",
        replacement: "for ((i, _), value) in a.set.iter().zip(values).skip(1) {",
        integration: &[ELEMENTS, "compiled_writes::a_point_change_checks_its_rows_constraints"],
        unit: &[],
    },
    Mutation {
        name: "points-append-lost",
        package: Package::Core,
        breaks: "an element appended in place is not in the array",
        file: "src/value/types/array.rs",
        original: "    all.push(value);\n    Value::Array(all.into())",
        replacement: "    drop(value);\n    Value::Array(all.into())",
        integration: &[GROWN],
        unit: &[],
    },
    Mutation {
        name: "points-append-full",
        package: Package::Core,
        breaks: "an array at its maximum cardinality is appended to in place",
        file: APPEND,
        original: "is_ok_and(|max| elements.len() < max)",
        replacement: "is_ok_and(|max| elements.len() <= max)",
        integration: &["compiled_points::a_full_array_is_assigned_as_bound"],
        unit: &[],
    },
    Mutation {
        name: "points-direct-row",
        package: Package::Core,
        breaks: "a row written over the frame's slots reads none of them",
        file: "src/db/routines/program/compiled/valued.rs",
        original: "|slot| Expr::Dynamic(slot.clone()),",
        replacement: "|_| Expr::Const(Value::Null),",
        integration: &[GROWN],
        unit: &[],
    },
];
