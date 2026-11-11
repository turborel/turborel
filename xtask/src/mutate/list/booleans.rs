//! The mutations of BOOLEAN (RULES.md R-TEST-9): truth values read as
//! values and conditions, the set functions of truth values, an index on
//! a BOOLEAN probed, and a dynamic parameter counted once however often
//! the parser reads it.

use super::{Mutation, Package};

/// The mutations of BOOLEAN, in the order they run.
pub const BOOLEANS: [Mutation; 6] = [
    Mutation {
        name: "boolean-predicand-untyped",
        package: Package::Core,
        breaks: "a value written as a condition is taken whatever its type, so WHERE K of a number is no error",
        file: "src/binder.rs",
        original: "        if t != DataType::Boolean {",
        replacement: "        if false {",
        integration: &["booleans::a_predicand_is_boolean_6_46"],
        unit: &[],
    },
    Mutation {
        name: "boolean-test-taken-as-null",
        package: Package::Core,
        breaks: "IS of a predicand is read as a null predicate, so B IS TRUE is a syntax error",
        file: "src/parser/expression.rs",
        original: "        self.is_kw(kw::IS) && (null_at(1) || (matches!(self.peek_at(1), Tok::KeyWord(kw::NOT)) && null_at(2)))",
        replacement: "        self.is_kw(kw::IS)",
        integration: &["booleans::boolean_literals_are_kept_5_3"],
        unit: &[],
    },
    Mutation {
        name: "every-as-any",
        package: Package::Core,
        breaks: "EVERY is True once one value is True, as ANY is",
        file: "src/functions/truths.rs",
        original: "        self.so_far = Some(if self.every { was && b } else { was || b });",
        replacement: "        self.so_far = Some(was || b);",
        integration: &["booleans::every_any_and_some_10_9"],
        unit: &["functions::truths::tests::every_and_any_10_9"],
    },
    Mutation {
        name: "boolean-probe-unplaced",
        package: Package::Core,
        breaks: "a truth value is not placed among a BOOLEAN column's values, so an index on one is never probed",
        file: "src/physical/probe.rs",
        original: "        | (DataType::Boolean, Value::Bool(_))\n",
        replacement: "",
        integration: &["booleans::a_boolean_index_is_probed_d34"],
        unit: &[],
    },
    Mutation {
        name: "boolean-argument-unfit",
        package: Package::Core,
        breaks: "a host program's truth value does not fit a BOOLEAN parameter, so it cannot be given",
        file: "src/params.rs",
        original: "            | (Value::Bool(_), DataType::Boolean)\n",
        replacement: "",
        integration: &["booleans::booleans_are_described_and_given_20_1"],
        unit: &[],
    },
    Mutation {
        name: "dynamic-parameter-recounted",
        package: Package::Core,
        breaks: "a dynamic parameter read again as the parser backtracks is counted again, so (? + 1) = 2 asks for two",
        file: "src/parser/literal.rs",
        original: "            let index = self.toks.iter().take(self.pos).filter(|t| matches!(t.tok, Tok::Delimiter(delim::QUESTION))).count();",
        replacement: "            let index = read;",
        integration: &["dynamic_statements::a_parenthesized_parameter_is_counted_once_17_6"],
        unit: &[],
    },
];
