//! The mutations of the join estimates from keys and bounds and of the
//! join method chosen by regret (RULES.md R-TEST-9, D384-D386): the pairs'
//! correction dropped, the bound's clamp dropped, a table changed since
//! its pass bounded as it was, the margin ignored, and a guessed column
//! of a class left unbounded by its declared key, or bounded by a counted
//! one or by a key that may be null (D1157).

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail their tests.
pub const JOIN_ESTIMATES: [Mutation; 7] = [
    Mutation {
        name: "join-estimates-correction-dropped",
        package: Package::Core,
        breaks: "a join on several columns of a key is estimated as if each column were independent",
        file: "src/physical/cardinality/joins.rs",
        original: "(classes * pairs).min(1.0_f64)",
        replacement: "classes.min(1.0_f64)",
        integration: &[],
        unit: &[
            "physical::cardinality::keyed_tests::a_join_on_a_composite_key_keeps_each_line_once",
            "physical::cardinality::keyed_tests::corrected_pairs_give_one_answer_for_every_order",
        ],
    },
    Mutation {
        name: "join-estimates-clamp-dropped",
        package: Package::Core,
        breaks: "an estimate above its bound is kept",
        file: "src/physical/cardinality.rs",
        original: "Card { rows: self.rows.min(upper), upper }",
        replacement: "Card { rows: self.rows, upper }",
        integration: &[],
        unit: &["physical::cardinality::keyed_tests::an_estimate_is_clamped_to_its_bound"],
    },
    Mutation {
        name: "join-estimates-stale-most",
        package: Package::Core,
        breaks: "the most rows of one value of a table changed since its pass are taken as the pass found them",
        file: "src/stats/since.rs",
        original: "most: f.most + self.since.changes",
        replacement: "most: f.most",
        integration: &[],
        unit: &["physical::cardinality::keyed_tests::a_table_changed_since_its_pass_keeps_its_bound"],
    },
    Mutation {
        name: "join-estimates-margin-ignored",
        package: Package::Core,
        breaks: "an index join is chosen by its cost at the estimate alone, whatever PLANNER.ESTIMATE_MARGIN",
        file: "src/physical/robust.rs",
        original: "(rows * margin.max(1.0_f64)).min(upper).max(rows)",
        replacement: "rows.min(upper).max(rows)",
        integration: &["robust_joins::the_margin_hands_a_risky_index_join_to_the_hash_join"],
        unit: &["physical::robust::tests::a_margin_of_one_trusts_the_estimate"],
    },
    Mutation {
        name: "class-bound-by-counted",
        package: Package::Core,
        breaks: "a guessed column of a class is bounded by a column statistics counted too, so the plan follows which tables are counted (D1157)",
        file: "src/physical/cardinality/composite.rs",
        original: "        self.table_keys(found.table).domain(found.column).then(|| self.table_rows(found.table)).flatten()",
        replacement: "        self.statistics(found.table).and_then(|st| st.distinct(found.column)).or_else(|| self.table_keys(found.table).domain(found.column).then(|| self.table_rows(found.table)).flatten())",
        integration: &["joins::a_join_order_is_the_same_whichever_tables_are_counted"],
        unit: &[],
    },
    Mutation {
        name: "class-bound-by-nullable-key",
        package: Package::Core,
        breaks: "a UNIQUE of a column that may be null bounds a guessed column of its class (D1157)",
        file: "src/physical/cardinality/keys.rs",
        original: ".filter(|&c| not_null(&[c]))",
        replacement: ".filter(|_| true)",
        integration: &[],
        unit: &["physical::cardinality::keyed_tests::a_guessed_column_is_bounded_by_a_declared_key_of_its_class"],
    },
    Mutation {
        name: "class-bound-dropped",
        package: Package::Core,
        breaks: "a guessed column of a class is counted of its rows' values whatever key the class equates (D1157)",
        file: "src/physical/cardinality/composite.rs",
        original: "let members: Vec<Vec<Member>> = members.iter().map(|class| self.bounded(leaves, class)).collect();",
        replacement: "let members: Vec<Vec<Member>> = members.to_vec();",
        integration: &[],
        unit: &["physical::cardinality::keyed_tests::a_guessed_column_is_bounded_by_a_declared_key_of_its_class"],
    },
];
