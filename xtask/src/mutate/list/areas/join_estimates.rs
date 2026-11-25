//! The mutations of the join estimates from keys and bounds and of the
//! join method chosen by regret (RULES.md R-TEST-9, D384-D386): the pairs'
//! correction dropped, the bound's clamp dropped, a table changed since
//! its pass bounded as it was, the margin ignored, and a guessed column
//! left past its class's known ones (D1152).

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail their tests.
pub const JOIN_ESTIMATES: [Mutation; 5] = [
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
        name: "join-estimates-guess-unbounded",
        package: Package::Core,
        breaks: "a column without statistics equated with a key of few values is counted of its table's rows",
        file: "src/physical/cardinality/composite.rs",
        original: "Member { distinct: m.distinct.min(least), ..*m }",
        replacement: "Member { distinct: m.distinct, ..*m }",
        integration: &[],
        unit: &["physical::cardinality::keyed_tests::a_guessed_column_is_bounded_by_its_class_key"],
    },
];
