//! Mutations of the library of XQuery 1.0 Functions and Operators: its
//! decimals and roundings, the promotion of numbers, the casts and
//! equality of atomic values it rests on, the resolution of URIs and the
//! effective boolean value (RULES.md R-TEST-9, D86).

use super::{Mutation, Package};

/// Each mechanism of the library broken alone, and the tests that fail.
pub const XQUERY_FUNCTIONS: [Mutation; 8] = [
    Mutation {
        name: "xquery-decimal-division-scale",
        package: Package::Core,
        breaks: "a quotient of decimals keeps no digits after the period but its operands' scale",
        file: "src/xquery/library/decimal.rs",
        original: "self.divide(other, DIVISION_SCALE.max(self.scale).max(other.scale))",
        replacement: "self.divide(other, self.scale.max(other.scale))",
        integration: &[],
        unit: &["xquery::library::decimal::tests::quotients_and_remainders", "xquery::library::numeric::tests::operands_are_promoted"],
    },
    Mutation {
        name: "xquery-round-half-to-even",
        package: Package::Core,
        breaks: "a half is rounded to the odd digit, not the even one",
        file: "src/xquery/library/decimal.rs",
        original: "(half == Ordering::Equal && last.checked_rem(2) == Some(1))",
        replacement: "(half == Ordering::Equal && last.checked_rem(2) == Some(0))",
        integration: &[],
        unit: &[
            "xquery::library::decimal::tests::rounding_by_each_mode",
            "xquery::library::rounding::tests::examples_of_round_half_to_even",
        ],
    },
    Mutation {
        name: "xquery-integer-quotient-decimal",
        package: Package::Core,
        breaks: "the quotient of two integers is an integer, not a decimal",
        file: "src/xquery/library/numeric.rs",
        original: "if op == Arithmetic::Divide { Number::Decimal(result) } else { Number::Integer(result) }",
        replacement: "Number::Integer(result)",
        integration: &[],
        unit: &["xquery::library::numeric::tests::operands_are_promoted"],
    },
    Mutation {
        name: "xquery-round-half-up",
        package: Package::Core,
        breaks: "fn:round of a double takes a half down, not up",
        file: "src/xquery/library/rounding.rs",
        original: "if x - below >= HALF",
        replacement: "if x - below > HALF",
        integration: &[],
        unit: &["xquery::library::rounding::tests::special_values_and_types"],
    },
    Mutation {
        name: "xquery-double-to-float-truncated",
        package: Package::Core,
        breaks: "a double cast to a float is rounded to the nearest, not truncated",
        file: "src/xml/atomic/number.rs",
        original: "                return truncated_single(*x);",
        replacement: "                return single(&format!(\"{x:e}\")).unwrap_or_default();",
        integration: &[],
        unit: &["xml::atomic::tests::casts_among_numbers"],
    },
    Mutation {
        name: "xquery-qname-namespace-equality",
        package: Package::Core,
        breaks: "two QNames of one local name are equal whatever their namespaces",
        file: "src/xml/atomic/value.rs",
        original: "(Datum::Name(su, sl), Datum::Name(tu, tl)) => su == tu && sl == tl,",
        replacement: "(Datum::Name(_, sl), Datum::Name(_, tl)) => sl == tl,",
        integration: &[],
        unit: &["xml::atomic::value::tests::qualified_names_are_equal_by_namespace_and_local_name"],
    },
    Mutation {
        name: "xquery-resolve-dot-dot",
        package: Package::Core,
        breaks: "a .. segment of a relative reference removes no segment of the path",
        file: "src/xquery/library/uri.rs",
        original: "            output.truncate(output.rfind('/').unwrap_or(0));\n",
        replacement: "",
        integration: &[],
        unit: &["xquery::library::uri::tests::rfc_3986_examples"],
    },
    Mutation {
        name: "xquery-effective-boolean-nan",
        package: Package::Core,
        breaks: "the effective boolean value of NaN is true",
        file: "src/xquery/library/booleans.rs",
        original: "!matches!(x.classify(), FpCategory::Zero | FpCategory::Nan)",
        replacement: "!matches!(x.classify(), FpCategory::Zero)",
        integration: &[],
        unit: &["xquery::library::booleans::tests::effective_boolean_values"],
    },
];
