//! The mutations of the one estimator (RULES.md R-TEST-9, D370-D372): the
//! floor of one row lost, Cardenas' formula taken as the lesser of the
//! rows and the values, its power rounded to 1, the whole table taken as a
//! sample, an index's sketch ignored, an outer join's preserved rows lost,
//! lineage stopped at a grouping, a table joined with itself taken as one
//! relation, the optimizer's local fixpoint lost, and of D1203 a set
//! function compared kept a third, a sum's rows taken alike in number and
//! an extreme taken of one value. Those of D1279 break what a table's
//! sample shows of an expression grouped by. They are the lattice of its
//! values ignored, a value seen once counted as one, a null given no
//! place, and the count taken where nothing is merged, past the columns'
//! bound, of a row that raises, of a column cut, of a plan of few rows,
//! at each asking, and by a function's name alone.

use super::super::{Mutation, Package};

/// The estimator's shared definitions.
const CARDINALITY_FILE: &str = "src/physical/cardinality.rs";
/// The groups.
const GROUPS_FILE: &str = "src/physical/cardinality/groups.rs";
/// The lineage of a column.
const LINEAGE_FILE: &str = "src/physical/cardinality/lineage.rs";

/// What a table's sample shows of an expression.
const SHOWN_FILE: &str = "src/physical/cardinality/shown.rs";

/// The test of an expression counted as the sample shows it.
const SHOWN: &str = "physical::cardinality::shown::tests::an_expression_grouped_by_is_counted_as_the_sample_shows_it";
/// The test of each case in which the sample shows nothing.
const SHOWS_NOTHING: &str = "physical::cardinality::shown::tests::where_the_sample_shows_nothing_the_columns_bound_stands";
/// The test of an answer found once, of each function its own.
const SHOWN_ONCE: &str = "physical::cardinality::shown::tests::an_answer_is_found_once_and_is_its_own_function_s";

/// The test of set functions compared with constants.
const HAVING: &str = "physical::cardinality::having::tests::a_set_function_compared_keeps_what_its_column_tells";

/// Mechanisms of the estimator and the rule driver whose removal must fail
/// their tests.
pub const ESTIMATES: [Mutation; 22] = [
    Mutation {
        name: "estimates-one-row-floor",
        package: Package::Core,
        breaks: "a filter keeping no row of an input of rows is estimated to give none",
        file: CARDINALITY_FILE,
        original: "rows.max(of.min(1.0_f64)).max(0.0_f64)",
        replacement: "rows.max(0.0_f64)",
        integration: &[],
        unit: &["physical::cardinality::tests::defaults_of_what_has_no_count"],
    },
    Mutation {
        name: "estimates-cardenas-as-least",
        package: Package::Core,
        breaks: "n rows of D values are taken to meet min(n, D) of them, not D(1 - (1 - 1/D)^n)",
        file: GROUPS_FILE,
        original: "(d * met).clamp(1.0_f64, n)",
        replacement: "d.clamp(1.0_f64, n)",
        integration: &[],
        unit: &["physical::cardinality::tests::cardenas_at_its_boundaries"],
    },
    Mutation {
        name: "estimates-cardenas-power-rounded",
        package: Package::Core,
        breaks: "rows drawn from far more values than a number tells from 1 are estimated to meet one of them",
        file: GROUPS_FILE,
        original: "let met = -((-1.0_f64 / d).ln_1p() * n).exp_m1();",
        replacement: "let met = 1.0_f64 - ((-1.0_f64 / d).ln_1p() * n).exp();",
        integration: &[],
        unit: &["physical::cardinality::tests::cardenas_of_values_past_a_number_s_digits"],
    },
    Mutation {
        name: "estimates-whole-table-sampled",
        package: Package::Core,
        breaks: "a grouping of every row of its table by a key is estimated as a sample's groups",
        file: GROUPS_FILE,
        original: "            return distinct.min(n).max(n.min(1.0_f64));",
        replacement: "            return cardenas(distinct, n);",
        integration: &[],
        unit: &["physical::cardinality::tests::groupings_are_estimated_by_their_groups"],
    },
    Mutation {
        name: "estimates-sketch-ignored",
        package: Package::Core,
        breaks: "a grouping by an index's key multiplies its columns' distinct values, correlated or not",
        file: GROUPS_FILE,
        original: "(self.sketched(table, &keys).unwrap_or(capped), rows)",
        replacement: "(capped, rows)",
        integration: &[],
        unit: &["physical::cardinality::tests::groupings_are_estimated_by_their_groups"],
    },
    Mutation {
        name: "estimates-outer-left-lost",
        package: Package::Core,
        breaks: "a LEFT join is estimated below its left operand's rows",
        file: "src/physical/cardinality/nodes.rs",
        original: "rows = rows.max(l.rows);",
        replacement: "rows = rows.max(0.0_f64);",
        integration: &[],
        unit: &["physical::cardinality::tests::outer_joins_keep_their_preserved_rows"],
    },
    Mutation {
        name: "estimates-grouping-lineage",
        package: Package::Core,
        breaks: "a grouping column above a grouping has no table, so a filter or join over it takes defaults",
        file: LINEAGE_FILE,
        original: "LogicalPlan::Aggregate { groups, input, .. } => Some((&**input, *groups.get(column)?)),",
        replacement: "LogicalPlan::Aggregate { .. } => None,",
        integration: &[],
        unit: &["physical::cardinality::tests::lineage_through_each_node"],
    },
    Mutation {
        name: "estimates-self-join-one-relation",
        package: Package::Core,
        breaks: "two scans of one table are taken as one relation, so an equality of their columns is a filter",
        file: LINEAGE_FILE,
        original: "if first.is_some_and(|known| !std::ptr::eq(known.scan, found.scan)) {",
        replacement: "if first.is_some_and(|known| known.table != found.table) {",
        integration: &[],
        unit: &["physical::cardinality::tests::lineage_through_each_node"],
    },
    Mutation {
        name: "estimates-local-fixpoint-lost",
        package: Package::Core,
        breaks: "a node a rule rewrote is left to the next pass, so the rules take as many passes as when restarted",
        file: "src/optimizer.rs",
        original: "    while *budget > 0 {",
        replacement: "    for _ in 0..usize::from(*budget > 0) {",
        integration: &[],
        unit: &["optimizer::drivers::rewriting_a_node_again_takes_fewer_passes"],
    },
    Mutation {
        name: "estimates-having-a-third",
        package: Package::Core,
        breaks: "a set function compared with a constant keeps a third of the groups, whatever its column tells (RULES.md D1203)",
        file: "src/physical/cardinality/nodes.rs",
        original: ".or_else(|| self.set_function_share(c, over))",
        replacement: "",
        integration: &[],
        unit: &[HAVING],
    },
    Mutation {
        name: "estimates-having-rows-alike-in-number",
        package: Package::Core,
        breaks: "a sum's variance leaves out the rows of a group varying in number (RULES.md D1203)",
        file: "src/physical/cardinality/having.rs",
        original: "m * x.variance() + (m - 1.0_f64).max(0.0_f64) * x.mean * x.mean",
        replacement: "m * x.variance()",
        integration: &[],
        unit: &[HAVING],
    },
    Mutation {
        name: "estimates-having-extreme-of-one-value",
        package: Package::Core,
        breaks: "a group's greatest value is within a bound as often as one value is (RULES.md D1203)",
        file: "src/physical/cardinality/having.rs",
        original: "share.clamp(0.0_f64, 1.0_f64).powf(m)",
        replacement: "share.clamp(0.0_f64, 1.0_f64)",
        integration: &[],
        unit: &[HAVING],
    },
    Mutation {
        name: "shown-lattice-ignored",
        package: Package::Core,
        breaks: "an expression's values are counted by those the sample holds alone, not by the lattice they lie on",
        file: SHOWN_FILE,
        original: "Some(lattice.places().map_or(counted, |places| counted.min(places)))",
        replacement: "Some(counted)",
        integration: &[],
        unit: &[SHOWN],
    },
    Mutation {
        name: "shown-seen-once-counted-as-one",
        package: Package::Core,
        breaks: "a value the sample holds once is counted as one value, not as the root of the rows a sampled row stands for",
        file: SHOWN_FILE,
        original: "each.sqrt() * once + (distinct - once)",
        replacement: "distinct",
        integration: &[],
        unit: &["physical::cardinality::shown::tests::values_seen_once_count_as_the_root_of_the_rows_a_row_stands_for"],
    },
    Mutation {
        name: "shown-null-given-no-place",
        package: Package::Core,
        breaks: "a null among an expression's values is given no place beside the lattice's",
        file: SHOWN_FILE,
        original: "steps.saturating_add(1).saturating_add(u128::from(self.nulls))",
        replacement: "steps.saturating_add(1)",
        integration: &[],
        unit: &["physical::cardinality::shown::tests::a_lattice_has_a_place_each_step_and_one_of_a_null"],
    },
    Mutation {
        name: "shown-where-nothing-is-merged",
        package: Package::Core,
        breaks: "the sample's count is taken of an expression it shows merging no values, in place of the columns' statistics",
        file: SHOWN_FILE,
        original: "if values.len() >= inputs.len() {",
        replacement: "if values.is_empty() {",
        integration: &[],
        unit: &[SHOWS_NOTHING],
    },
    Mutation {
        name: "shown-past-the-columns-bound",
        package: Package::Core,
        breaks: "what the sample shows is taken where it is more than the columns read allow",
        file: "src/physical/cardinality/nodes.rs",
        original: "self.shown_distinct(&computed, whole).map_or(read, |shown| read.min(shown))",
        replacement: "self.shown_distinct(&computed, whole).unwrap_or(read)",
        integration: &[],
        unit: &[SHOWS_NOTHING],
    },
    Mutation {
        name: "shown-raising-row-ends-it",
        package: Package::Core,
        breaks: "a sampled row whose evaluation raises leaves the sample showing nothing, where the others show the values",
        file: SHOWN_FILE,
        original: "let Ok(value) = eval(e, row, &NoSubqueries) else { continue };",
        replacement: "let Ok(value) = eval(e, row, &NoSubqueries) else { return None };",
        integration: &[],
        unit: &["physical::cardinality::shown::tests::a_sampled_row_that_raises_is_left_out"],
    },
    Mutation {
        name: "shown-of-a-column-cut",
        package: Package::Core,
        breaks: "an expression of a string cut to the characters a sampled row keeps is counted by the values of what was kept",
        file: SHOWN_FILE,
        original: "Value::Char(s) => s.len() >= ROW_CHARACTERS,",
        replacement: "Value::Char(s) => s.len() > ROW_CHARACTERS,",
        integration: &[],
        unit: &[SHOWS_NOTHING],
    },
    Mutation {
        name: "shown-asked-of-few-rows",
        package: Package::Core,
        breaks: "the sample is evaluated for a plan of no more rows than it holds",
        file: SHOWN_FILE,
        original: "if whole <= cost_of(rows.len()) {",
        replacement: "if whole < 0.0_f64 {",
        integration: &[],
        unit: &[SHOWS_NOTHING, SHOWN_ONCE],
    },
    Mutation {
        name: "shown-found-at-each-asking",
        package: Package::Core,
        breaks: "what the sample shows of an expression is found again each time a statement asks",
        file: "src/physical/cardinality/seen.rs",
        original: "let known = self.shown.borrow().get(&key).map(|&(_, shown)| shown);",
        replacement: "let known: Option<Option<f64>> = None;",
        integration: &[],
        unit: &[SHOWN_ONCE],
    },
    Mutation {
        name: "shown-key-of-the-name-alone",
        package: Package::Core,
        breaks: "two functions of one name that hold what differs share one answer of the sample",
        file: "src/physical/estimate/fingerprint.rs",
        original: "Arc::as_ptr(&f.0).cast::<()>().addr().hash(&mut h);",
        replacement: "f.0.name().hash(&mut h);",
        integration: &[],
        unit: &[SHOWN_ONCE],
    },
];
