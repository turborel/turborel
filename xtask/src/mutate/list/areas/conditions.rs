//! The mutations of the states of conditions (RULES.md R-TEST-9, D970,
//! D971): a numeric literal no type holds refused as a limit, not a
//! Syntax Rule; its text cast refused as no literal, or its long fraction
//! as out of range; a path expression's literal refused as a limit; and a
//! statement prepared given a class of its own.

use super::super::{Mutation, Package};

/// The file of literals.
const LITERALS: &str = "src/parser/literal.rs";
/// The test of a literal past the maximum precision, run.
const EXACT: &str = "language::an_exact_literal_beyond_the_maximum_precision_breaks_a_syntax_rule_5_3";
/// The test of a literal no type holds, prepared.
const PREPARED: &str = "syntax_states::a_literal_no_type_holds_breaks_a_syntax_rule_prepared_5_3";
/// The test of the text of a number no type holds.
const TEXT: &str = "syntax_states::text_of_a_number_no_type_holds_is_out_of_range_6_13";
/// The unit test of the text of a number no exact type holds.
const TEXT_UNIT: &str = "value::cast::tests::literals_no_exact_number_holds";

/// Mechanisms whose removal must fail the states the standard gives.
pub const CONDITIONS: [Mutation; 6] = [
    Mutation {
        name: "exact-literal-syntax-rule",
        package: Package::Core,
        breaks: "an exact literal of more digits than the maximum precision is refused as a limit, -900, not by SR29",
        file: LITERALS,
        original: "let d = parse_exact(&t).map_err(|m| SqlError::at(Code::SyntaxRule, at, m))?;",
        replacement: "let d = parse_exact(&t).map_err(|m| SqlError::at(Code::Unsupported, at, m))?;",
        integration: &[EXACT, PREPARED],
        unit: &[],
    },
    Mutation {
        name: "approximate-literal-syntax-rule",
        package: Package::Core,
        breaks: "an approximate literal past DOUBLE PRECISION's range is refused as a limit, -900, not by SR30",
        file: LITERALS,
        original: "            Code::SyntaxRule,\n            at,\n            format!(\"[ISO/IEC 9075-2:2023 §5.3 SR30]",
        replacement: "            Code::Unsupported,\n            at,\n            format!(\"[ISO/IEC 9075-2:2023 §5.3 SR30]",
        integration: &["language::an_approximate_literal_out_of_range_breaks_a_syntax_rule_5_3", PREPARED],
        unit: &[],
    },
    Mutation {
        name: "cast-text-out-of-range",
        package: Package::Core,
        breaks: "text of a number past every number's range is refused as no literal, 22018, not 22003",
        file: "src/value/cast.rs",
        original: "                SqlState::NUMERIC_VALUE_OUT_OF_RANGE,\n                format!(\"[ISO/IEC 9075-2:2023 §6.13 GR8a ii",
        replacement: "                SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,\n                format!(\"[ISO/IEC 9075-2:2023 §6.13 GR8a ii",
        integration: &[TEXT],
        unit: &[TEXT_UNIT],
    },
    Mutation {
        name: "cast-text-fraction-truncated",
        package: Package::Core,
        breaks: "a fraction past what its whole digits leave of the maximum precision is kept, so its text overflows and is refused",
        file: "src/value/cast.rs",
        original: "    let kept = digits_in(lit.fraction).take(room);",
        replacement: "    let kept = digits_in(lit.fraction).take(usize::MAX);",
        integration: &[TEXT],
        unit: &[TEXT_UNIT],
    },
    Mutation {
        name: "json-path-literal-syntax-rule",
        package: Package::Core,
        breaks: "a path expression's numeric literal no number holds is refused as a limit, -900, not as the literal it is",
        file: "src/json/path/lex.rs",
        original: "    SqlError::new(Code::SyntaxRule, format!(\"[ISO/IEC 9075-2:2023 §9.45 GR1a",
        replacement: "    SqlError::new(Code::Unsupported, format!(\"[ISO/IEC 9075-2:2023 §9.45 GR1a",
        integration: &[],
        unit: &[
            "json::path::lex::tests::a_decimal_literal_with_an_exponent_is_approximate",
            "json::path::lex::tests::hex_integer_literals_are_exact_numbers",
        ],
    },
    Mutation {
        name: "prepared-syntax-class",
        package: Package::Core,
        breaks: "a statement prepared that breaks a rule is given a class of its own, as SQL-92's 37, not SQL:2023's 42000",
        file: "src/db/prepared.rs",
        original: "        let made = self.prepared(sql);",
        replacement: "        let made = self.prepared(sql).map_err(|e| if e.sqlstate().is_syntax_or_access() { e.with_state(SqlState::DYNAMIC_SQL_ERROR) } else { e });",
        integration: &[
            "syntax_states::a_rule_broken_is_42000_run_directly_or_prepared_20_7",
            "prepared::prepared_statements_are_refused_saying_why",
        ],
        unit: &[],
    },
];
