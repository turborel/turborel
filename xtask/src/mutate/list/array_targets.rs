//! The mutations of array element assignment (RULES.md R-TEST-9, Feature
//! S097): a target `A[i]` whose array is of a distinct type refused, whose
//! position is not checked to be an integer, or that assigns the whole
//! array; of a new row column, the element not assigned; of CALL, an
//! argument `A[i]` read as `A`, a position that is an SQL parameter not
//! taken; and a qualified position not read.

use super::{Mutation, Package};

/// The file of an element target's check and value.
const ELEMENT: &str = "src/db/assignment/element.rs";
/// The file of a CALL's arguments.
const CALL: &str = "src/db/routines/call.rs";
/// The test of an element of an SQL parameter assigned by SELECT INTO.
const SELECTED: &str = "array_targets::select_into_assigns_an_element_14_7";
/// The test of an element assigned by CALL.
const CALLED: &str = "array_targets::a_call_assigns_an_element_10_4";
/// The test of an element of the new row assigned.
const NEW_ROW: &str = "array_targets::a_trigger_assigns_an_element_of_the_new_row_6_4";

/// The mutations of array element assignment.
pub const ARRAY_TARGETS: [Mutation; 7] = [
    Mutation {
        name: "array-targets-distinct-refused",
        package: Package::Core,
        breaks: "an element of a distinct type whose source is an array is refused as no array's",
        file: ELEMENT,
        original: "    let DataType::Array(a) = declared.represented() else {\n        return Err(rule(",
        replacement: "    let DataType::Array(a) = &declared else {\n        return Err(rule(",
        integration: &[NEW_ROW],
        unit: &[],
    },
    Mutation {
        name: "array-targets-position-unchecked",
        package: Package::Core,
        breaks: "a decimal literal is taken as the position of an element target",
        file: ELEMENT,
        original: "ElementPosition::Literal(Literal::Exact(d)) => d.scale == 0,",
        replacement: "ElementPosition::Literal(Literal::Exact(d)) => d.scale < 2,",
        integration: &["array_targets::element_targets_are_checked_6_4"],
        unit: &[],
    },
    Mutation {
        name: "array-targets-whole-array",
        package: Package::Core,
        breaks: "a target A[i] takes an array of the one value, its other elements lost",
        file: ELEMENT,
        original: "    with_element(&array, i, (a.max, fetched), (NULL_ARRAY, NO_ELEMENT))",
        replacement: "    { let _ = (array, i); Ok(Value::Array(vec![fetched].into())) }",
        integration: &[SELECTED, CALLED],
        unit: &[],
    },
    Mutation {
        name: "array-targets-new-row-whole",
        package: Package::Core,
        breaks: "a new row column's element target assigns the value to the whole column",
        file: "src/db/assignment.rs",
        original: "            let value = element::assigned(params, spec, fetched.clone())?;\n            params.assign_transition(",
        replacement: "            let value = fetched.clone();\n            params.assign_transition(",
        integration: &[NEW_ROW],
        unit: &[],
    },
    Mutation {
        name: "array-targets-call-whole",
        package: Package::Core,
        breaks: "a CALL argument A[i] is read as the target A",
        file: CALL,
        original: "spec.element = Some(Box::new(position_of(cx, index)?));",
        replacement: "let _ = index;",
        integration: &[CALLED],
        unit: &[],
    },
    Mutation {
        name: "array-targets-call-parameter-position",
        package: Package::Core,
        breaks: "an SQL parameter is not taken as the position of a CALL argument's element",
        file: CALL,
        original: "cx.params.routine_scope()?.parameter(qualifier, &cs.column).map(|_| ElementPosition::Parameter(cs.column.clone()))",
        replacement: "{ let _ = qualifier; None }",
        integration: &[CALLED],
        unit: &[],
    },
    Mutation {
        name: "array-targets-qualified-position",
        package: Package::Core,
        breaks: "a position R.P is read as R alone",
        file: "src/parser/target.rs",
        original: "            if qualified {\n                self.advance();\n                self.advance();\n            }\n            return Ok(ElementPosition::Parameter(name));",
        replacement: "            let _ = qualified;\n            return Ok(ElementPosition::Parameter(name));",
        integration: &[],
        unit: &["parser::routines::tests::an_element_is_a_target_6_4"],
    },
];
