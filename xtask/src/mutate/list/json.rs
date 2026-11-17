//! The mutations of the SQL/JSON query operators (RULES.md R-TEST-9,
//! D84): what ON ERROR replaces, lax mode's unwrapping, JSON_QUERY's
//! conditional wrapper, the Syntax Rules the binder checks of a path, the
//! PASSING identifier's value, the warning of a call, and the name that
//! tells two calls apart.

use super::{Mutation, Package};

/// The mutations of the SQL/JSON query operators, in the order they run.
pub const JSON: [Mutation; 8] = [
    Mutation {
        name: "json-on-error-catches-every-error",
        package: Package::Core,
        breaks: "ON ERROR replaces any error, not a data exception alone, so a limit of turborel's, JSON text nested 200 deep, is null",
        file: "src/json/query.rs",
        original: "    error.sqlstate().class() == SqlState::DATA_EXCEPTION.class()",
        replacement: "    true",
        integration: &["json_functions::json_value_behaviors_9_48"],
        unit: &[],
    },
    Mutation {
        name: "json-lax-does-not-unwrap",
        package: Package::Core,
        breaks: "lax mode does not unwrap an array an accessor is of, so lax $.a of [{\"a\":8}] gives no item",
        file: "src/json/path/eval.rs",
        original: "            Mode::Lax => unwrap(seq),",
        replacement: "            Mode::Lax => seq,",
        integration: &["json_functions::json_value_6_28"],
        unit: &[],
    },
    Mutation {
        name: "json-conditional-wrapper-leaves-a-scalar",
        package: Package::Core,
        breaks: "JSON_QUERY's WITH CONDITIONAL WRAPPER leaves any one item unwrapped, a lone scalar too, as §9.44 does and §9.49 GR4a iv does not",
        file: "src/json/query/json_query.rs",
        original: "    matches!(seq, [Item::Array(_) | Item::Object(_)])",
        replacement: "    seq.len() == 1",
        integration: &["json_functions::json_query_6_35"],
        unit: &[],
    },
    Mutation {
        name: "json-sr4-checked-only-when-run",
        package: Package::Core,
        breaks: "a named variable no PASSING argument has is not refused as the statement is prepared, only as it runs",
        file: "src/binder/json.rs",
        original: "        path.check_variables(&names).or_at(common.path_at)?;",
        replacement: "        let _ = &names;",
        integration: &["json_functions::passing_t823"],
        unit: &[],
    },
    Mutation {
        name: "json-delimited-identifier-folded",
        package: Package::Core,
        breaks: "a delimited PASSING identifier is folded to upper case, so AS \"x\" is $X and $x is passed nothing",
        file: "src/parser/json.rs",
        original: "Ok(JsonArgument { input, name, at })",
        replacement: "Ok(JsonArgument { input, name: name.to_uppercase(), at })",
        integration: &["json_functions::passing_t823"],
        unit: &[],
    },
    Mutation {
        name: "json-starts-with-of-format-json",
        package: Package::Core,
        breaks: "§9.46 SR8 lets the initial of starts with be an argument FORMAT JSON",
        file: "src/binder/json.rs",
        original: "if argument.input.format.is_some() || !t.is_character() {",
        replacement: "if !t.is_character() {",
        integration: &["json_functions::starts_with_9_46"],
        unit: &[],
    },
    Mutation {
        name: "json-call-warning-dropped",
        package: Package::Core,
        breaks: "a function call's warning is dropped, so JSON_VALUE cutting a string to its RETURNING type completes without 01004",
        file: "src/exec/eval.rs",
        original: "let (value, warning) = function.0.call_warned(&values, subqueries.temp_lobs())?;",
        replacement: "let (value, warning) = (function.0.call_warned(&values, subqueries.temp_lobs())?.0, None::<SqlState>);",
        integration: &["json_functions::warnings_9_48"],
        unit: &[],
    },
    Mutation {
        name: "json-call-named-without-clauses",
        package: Package::Core,
        breaks: "a call's name is its operator's alone, so two sums of JSON_VALUE of one column and other paths are taken for one",
        file: "src/functions/json.rs",
        original: "    format!(\"{shown} {clauses}\")",
        replacement: "    name.to_owned()",
        integration: &["json_functions::where_written_8_23"],
        unit: &[],
    },
];
