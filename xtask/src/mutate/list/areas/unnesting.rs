//! The mutations of unnesting and inlining (RULES.md R-TEST-9, D310 to
//! D319). A scalar subquery of two rows that no longer fails, the COUNT
//! bug in both cases, bindings merged by their trailing spaces, ALL
//! counted as SOME, a conditional subquery that can raise unnested, OFFSET
//! kept for its own row, and functions inlined that are not deterministic
//! or that leave a parameter unread, and a join beside a
//! subquery left a product.

use super::super::{Mutation, Package};

/// The test of the correlated forms over random tables.
const MATCHED: &str = "decorrelation::unnested_queries_match_nested_ones";
/// The test of nulls, empty sets and cardinality.
const NULLS: &str = "decorrelation::nulls_empty_sets_and_cardinality";
/// The test of a join beside a correlated subquery.
const JOINED: &str = "decorrelation::tpch::a_join_beside_a_correlated_subquery_stays_a_join";
/// The test of inlined functions.
const INLINED: &str = "decorrelation::functions::functions_are_inlined_and_unnested";

/// The mutations of unnesting and inlining.
pub const UNNESTING: [Mutation; 12] = [
    Mutation {
        name: "unnest-two-rows-pass",
        package: Package::Core,
        breaks: "a scalar subquery of two rows for a binding gives its first instead of failing",
        file: "src/optimizer/unnesting/calls.rs",
        original: "if compare(count, &one) == Some(Ordering::Greater) {",
        replacement: "if compare(count, &one) == Some(Ordering::Less) {",
        integration: &[NULLS],
        unit: &[],
    },
    Mutation {
        name: "unnest-simple-count-bug",
        package: Package::Core,
        breaks: "a grouping of no grouping columns pulled up gives null, not its value over no row, where a binding has none",
        file: "src/optimizer/unnesting/simple.rs",
        original: "let defaults = if groups.is_empty() {",
        replacement: "let defaults = if groups.is_empty() && groups.len() > 1 {",
        integration: &[NULLS],
        unit: &[],
    },
    Mutation {
        name: "unnest-domain-count-bug",
        package: Package::Core,
        breaks: "a grouping of no grouping columns under D counts the row of nulls of a binding with no rows",
        file: "src/optimizer/unnesting/domain.rs",
        original: "                AggCall { filter, ..a }\n",
        replacement: "                AggCall { filter: a.filter.clone(), ..a }\n",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "unnest-trailing-merged",
        package: Package::Core,
        breaks: "D merges bindings that differ in trailing spaces alone",
        file: "src/optimizer/unnesting/domain.rs",
        original: "    keys.extend(sub.outer.iter().map(|o| Expr::Call(trailing.clone(), vec![o.clone()])));\n",
        replacement: "    keys.extend(sub.outer.iter().map(|_| Expr::Const(Value::Null)));\n",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "unnest-all-as-some",
        package: Package::Core,
        breaks: "a quantified ALL counts the rows its comparison is true of, as SOME does",
        file: "src/optimizer/unnesting/joined.rs",
        original: "Cond::Is(Box::new(c.clone()), Truth::False, false)",
        replacement: "Cond::Is(Box::new(c.clone()), Truth::True, false)",
        integration: &[NULLS],
        unit: &[],
    },
    Mutation {
        name: "unnest-conditional-raising",
        package: Package::Core,
        breaks: "a subquery a CASE may leave unevaluated is unnested though it can raise",
        file: "src/optimizer/unnesting.rs",
        original: "if !pure || stops || (found.conditional && (outer_raises || plan_raises())) {",
        replacement: "if !pure || stops {",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "unnest-offset-kept",
        package: Package::Core,
        breaks: "OFFSET under D keeps the last row it skips",
        file: "src/optimizer/unnesting/domain/ordered.rs",
        original: "CompOp::Gt, constant(offset)",
        replacement: "CompOp::Ge, constant(offset)",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "unnest-no-semi-join",
        package: Package::Core,
        breaks: "an uncorrelated IN of WHERE is left a subquery, not made a semi join",
        file: "src/optimizer/unnesting/found.rs",
        original: "            semi_joins(cond, &mut out);\n",
        replacement: "",
        integration: &["decorrelation::tpch::tpch_subqueries_are_unnested"],
        unit: &[],
    },
    Mutation {
        name: "inline-not-deterministic",
        package: Package::Core,
        breaks: "a function that is not deterministic is inlined",
        file: "src/optimizer/inlining.rs",
        original: "!i.modifies && i.deterministic && !i.null_call;",
        replacement: "!i.modifies && !i.null_call;",
        integration: &[INLINED],
        unit: &[],
    },
    Mutation {
        name: "inline-parameter-unread",
        package: Package::Core,
        breaks: "a function whose body leaves a parameter unread is inlined, its argument's error lost",
        file: "src/optimizer/inlining.rs",
        original: "if (0..i.params.len()).any(|p| !read.contains(&p)) {",
        replacement: "if (0..i.params.len()).any(|p| !read.contains(&p) && read.is_empty()) {",
        integration: &[INLINED],
        unit: &[],
    },
    Mutation {
        name: "unnest-unique-counted",
        package: Package::Core,
        breaks: "a subquery of one row for each binding is read through SINGLE, which can raise, so the WHERE's join stays above it, a product",
        file: "src/optimizer/unnesting/joined.rs",
        original: "let one = if self.unique {",
        replacement: "let one = if self.unique && self.base == 0 {",
        integration: &[JOINED],
        unit: &[],
    },
    Mutation {
        name: "unnest-domain-of-product",
        package: Package::Core,
        breaks: "D of a product whose one side holds the bindings is made of the whole product",
        file: "src/optimizer/unnesting/domain.rs",
        original: "let (side, other, keys) = if read.iter().all(|&c| c < lw) {",
        replacement: "let (side, other, keys) = if read.is_empty() && read.iter().all(|&c| c < lw) {",
        integration: &[JOINED],
        unit: &[],
    },
];
