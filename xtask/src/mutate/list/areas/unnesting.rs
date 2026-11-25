//! The mutations of unnesting and inlining (RULES.md R-TEST-9, D310 to
//! D319). A scalar subquery of two rows that no longer fails, the COUNT
//! bug in both cases, bindings merged by their trailing spaces, ALL
//! counted as SOME, a conditional subquery that can raise unnested, OFFSET
//! kept for its own row, and functions inlined that are not deterministic
//! or that leave a parameter unread, and a join beside a
//! subquery left a product. Of D1060, a SUM taken as never raising, the
//! rows a sum is of left uncounted, a grouping's SUM typed as its
//! argument, a scalar subquery of any number of rows taken as raising
//! nothing, and a CASE bounded by its first result. Of D1064, a conjunct
//! that can raise moved above a semi join. Of D1065, an uncorrelated NOT
//! IN left nested.

use super::super::{Mutation, Package};

/// The test of the correlated forms over random tables.
const MATCHED: &str = "decorrelation::unnested_queries_match_nested_ones";
/// The test of nulls, empty sets and cardinality.
const NULLS: &str = "decorrelation::nulls_empty_sets_and_cardinality";
/// The test of a join beside a correlated subquery.
const JOINED: &str = "decorrelation::tpch::a_join_beside_a_correlated_subquery_stays_a_join";
/// The test of inlined functions.
const INLINED: &str = "decorrelation::functions::functions_are_inlined_and_unnested";

/// The test of a SUM that can overflow kept to the outer rows' keys.
const OVERFLOW: &str = "decorrelation::totals::a_sum_that_can_overflow_is_computed_for_the_outer_keys_alone";
/// The test of the sums the declared types keep within their digits.
const FITS: &str = "logical::raises::totals::tests::sums_fit_while_their_digits_and_the_rows_do";
/// The test of the types of a grouping's columns.
const GROUPED: &str = "logical::raises::totals::tests::a_grouping_s_columns_are_typed_by_their_set_functions";
/// The test of a subquery that holds a scalar subquery.
const HELD: &str = "decorrelation::totals::a_subquery_holding_a_sum_s_subquery_is_a_semi_join";

/// The mutations of unnesting and inlining.
pub const UNNESTING: [Mutation; 19] = [
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
    Mutation {
        name: "unnest-sum-never-raises",
        package: Package::Core,
        breaks: "a subquery of a SUM that can overflow is pulled up, and raises of a key no outer row has (D1060, D15)",
        file: "src/optimizer/unnesting/pure.rs",
        original: "Some(Standard::Sum | Standard::Avg) => !total_fits(a, judge.types),",
        replacement: "Some(Standard::Sum | Standard::Avg) => false,",
        integration: &[OVERFLOW],
        unit: &[],
    },
    Mutation {
        name: "sum-fits-rows-uncounted",
        package: Package::Core,
        breaks: "a sum is bounded by its values' digits alone, not those of the rows it is of too (D1060)",
        file: "src/logical/raises/totals.rs",
        original: "is_some_and(|(p, _)| p.saturating_add(ROW_DIGITS) <= MAX_EXACT_PRECISION)",
        replacement: "is_some_and(|(p, _)| p <= MAX_EXACT_PRECISION)",
        integration: &[OVERFLOW],
        unit: &[FITS],
    },
    Mutation {
        name: "grouping-sum-typed-as-argument",
        package: Package::Core,
        breaks: "a grouping's SUM column is typed as its argument, so arithmetic on it is taken to fit (D1060)",
        file: "src/logical/raises/totals.rs",
        original: "exact((p.saturating_add(ROW_DIGITS), s)).or_else(|| Some(a.result.clone()))",
        replacement: "exact((p, s)).or_else(|| Some(a.result.clone()))",
        integration: &[],
        unit: &[GROUPED],
    },
    Mutation {
        name: "unnest-scalar-subquery-of-any-rows",
        package: Package::Core,
        breaks: "a scalar subquery that may give two rows is taken as raising nothing (D1060)",
        file: "src/optimizer/unnesting/pure.rs",
        original: "let subquery = |s: &Subquery, one: bool| (one && !s.plan.one_row_at_most()) || judged(&s.plan, ctx, deeper);",
        replacement: "let subquery = |s: &Subquery, one: bool| (one && s.outer.len() > usize::MAX) || judged(&s.plan, ctx, deeper);",
        integration: &[HELD],
        unit: &[],
    },
    Mutation {
        name: "case-bounded-by-first-result",
        package: Package::Core,
        breaks: "a CASE is bounded by its first result alone, not the widest of them (D1060)",
        file: "src/logical/raises.rs",
        original: "|(w, held): (u32, u32)| (w.max(p.saturating_sub(s)), held.max(s))",
        replacement: "|(w, held): (u32, u32)| (w, held.max(s))",
        integration: &[],
        unit: &["logical::raises::tests::a_case_is_bounded_by_its_widest_results"],
    },
    Mutation {
        name: "semi-join-raising-conjunct-above",
        package: Package::Core,
        breaks: "a conjunct of a correlated subquery that can raise is judged of every row, below the semi join (D1064, D15)",
        file: "src/optimizer/unnesting/semi.rs",
        original: "&& !pure::cond_raises_over(c, input, ctx);",
        replacement: "&& (c.has_subquery() || !pure::cond_raises_over(c, input, ctx));",
        integration: &["decorrelation::totals::a_subquery_s_conjunct_is_judged_after_a_semi_join"],
        unit: &[],
    },
    Mutation {
        name: "uncorrelated-not-in-left-nested",
        package: Package::Core,
        breaks: "an uncorrelated NOT IN that is a conjunct is offered to the semi join rule, and made an antijoin (D1065)",
        file: "src/optimizer/unnesting/found.rs",
        original: "let taken = (not && quantifier != Quantifier::One) || quantifier == Quantifier::All;",
        replacement: "let taken = !not && quantifier == Quantifier::All;",
        integration: &["decorrelation::semi::semi_joins_match_the_subqueries"],
        unit: &[],
    },
];
