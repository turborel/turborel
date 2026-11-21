//! The mutations of the result type of values brought together (RULES.md
//! R-TEST-9, D396): numbers of one type written alike shown by the name
//! of a type computed.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of a result type.
pub const SET_TYPES: [Mutation; 1] = [Mutation {
    name: "set-type-alike-computed",
    package: Package::Server,
    breaks: "numbers of one type written alike are brought together as NUMERIC, so a VALUES of an INTEGER is not INTEGER [§9.5 SR3c]",
    file: "src/binder/case.rs",
    original: "    if let Some(first) = types.first().filter(|first| types.iter().all(|t| written_alike(t, first))) {",
    replacement: "    if let Some(first) = types.first().filter(|first| types.iter().all(|t| written_alike(t, first)) && types.is_empty()) {",
    integration: &["declared::a_cast_is_written_as_the_type_it_casts_to"],
    unit: &[],
}];
