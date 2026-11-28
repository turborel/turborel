//! The mutations of the grouping columns computed after the grouping
//! (RULES.md R-TEST-9, D1305): a column computed after the grouping
//! though its expression may raise, is not deterministic, reads a column
//! whose values a group may tell apart, a column of another node or one
//! of the operand an outer join extends with nulls, with a group made
//! before any row or with no column kept, the projection below losing a
//! column a set function reads, and a select list that may raise merged;
//! and of the select list computed after FETCH FIRST (D1306): an
//! expression moved that may raise or that the sort reads, the sort's
//! keys left where they were, and the columns the expressions read not
//! handed on.

use super::super::{Mutation, Package};

/// The file of the grouping columns computed after the grouping.
const COMPUTED_FILE: &str = "src/optimizer/dependent_keys/computed.rs";

/// The test of grouping columns that stay grouped by.
const NOT_COMPUTED: &str = "rewrites::computed_keys::a_grouping_column_that_may_not_be_computed_of_the_others_is_left";

/// The test of grouping columns computed after the grouping.
const COMPUTED: &str = "rewrites::computed_keys::a_grouping_column_computed_of_the_others_is_computed_after_the_grouping";

/// The file of the select list computed after FETCH FIRST.
const LATE_LIST_FILE: &str = "src/optimizer/late_fetch/projected.rs";

/// The test of select lists that stay below their FETCH FIRST.
const NOT_LATE: &str = "rewrites::computed_late::a_select_list_that_may_not_be_computed_late_is_left";

/// The test of expressions computed after FETCH FIRST.
const LATE: &str = "rewrites::computed_late::expressions_of_the_select_list_are_computed_of_the_rows_kept";

/// The mutations of the two rules.
pub const COMPUTED_KEYS: [Mutation; 13] = [
    Mutation {
        name: "computed-keys-may-raise",
        package: Package::Core,
        breaks: "a grouping column whose expression may raise is computed of the groups, not of every row (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "expr_pure(e) && !can_raise_typed(e, &|c| plan_column_type(over, c, catalog))",
        replacement: "expr_pure(e)",
        integration: &[NOT_COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-not-deterministic",
        package: Package::Core,
        breaks: "a grouping column whose expression is not deterministic is computed once of a group (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "expr_pure(e) && !can_raise_typed(",
        replacement: "!can_raise_typed(",
        integration: &[],
        unit: &["optimizer::dependent_keys::computed::tests::a_call_not_deterministic_stays_a_grouping_column"],
    },
    Mutation {
        name: "computed-keys-told-apart",
        package: Package::Core,
        breaks: "a grouping column is computed of a column whose values a group may tell apart (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "groups.get(j).is_some_and(|&g| known.one_value(g))",
        replacement: "groups.get(j).is_some()",
        integration: &[NOT_COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-another-node",
        package: Package::Core,
        breaks: "a grouping column is computed of the column of its number of another node (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "ptr::eq(self.of, of) && if let",
        replacement: "if let",
        integration: &[NOT_COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-nulls-extended",
        package: Package::Core,
        breaks: "a grouping column of the operand an outer join extends with nulls is computed of nulls (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "OuterJoin::Left => first,",
        replacement: "OuterJoin::Left => true,",
        integration: &[NOT_COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-seeded",
        package: Package::Core,
        breaks: "a grouping with a group made before any row has a column computed of the others (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "groups.len() < TWO_COLUMNS || !seeds.is_empty()",
        replacement: "groups.len() < TWO_COLUMNS",
        integration: &[],
        unit: &["optimizer::dependent_keys::computed::tests::a_grouping_of_a_group_made_before_any_row_is_left"],
    },
    Mutation {
        name: "computed-keys-none-kept",
        package: Package::Core,
        breaks: "a grouping by constants alone groups by none, one group even of no rows (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "if later.iter().all(Option::is_some)",
        replacement: "if later.is_empty()",
        integration: &["rewrites::computed_keys::constants_and_expressions_found_through_joins_are_computed_after_the_grouping"],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-read-column-dropped",
        package: Package::Core,
        breaks: "the projection below the grouping loses a column a set function still reads (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "filter(|c| read.contains(c) || !out.contains(c))",
        replacement: "filter(|c| !out.contains(c))",
        integration: &[COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "computed-keys-unquiet-moved",
        package: Package::Core,
        breaks: "a select list that may raise is merged or passed by a condition, evaluated of fewer groups (RULES.md D1305)",
        file: COMPUTED_FILE,
        original: "all(|e| of_groups(e) && quiet(e, grouped, catalog))",
        replacement: "all(|e| of_groups(e))",
        integration: &[NOT_COMPUTED],
        unit: &[],
    },
    Mutation {
        name: "late-list-unquiet",
        package: Package::Core,
        breaks: "an expression that may raise or is not deterministic is computed of the rows FETCH FIRST keeps alone (RULES.md D1306)",
        file: LATE_LIST_FILE,
        original: "!matches!(e, Expr::Column(_)) && quiet(e, rows, catalog)",
        replacement: "!matches!(e, Expr::Column(_))",
        integration: &[NOT_LATE],
        unit: &["optimizer::late_fetch::projected::tests::a_call_not_deterministic_stays_below_the_fetch_first"],
    },
    Mutation {
        name: "late-list-sort-key-moved",
        package: Package::Core,
        breaks: "an expression the sort reads is computed after the sort (RULES.md D1306)",
        file: LATE_LIST_FILE,
        original: ".all(|k| matches!(exprs.get(k.column), Some(Expr::Column(_))))",
        replacement: ".all(|k| exprs.get(k.column).is_some())",
        integration: &[NOT_LATE],
        unit: &[],
    },
    Mutation {
        name: "late-list-keys-where-they-were",
        package: Package::Core,
        breaks: "the sort below reads its keys where the select list had them before (RULES.md D1306)",
        file: LATE_LIST_FILE,
        original: "SortKey { column: moved(k.column), ..*k }",
        replacement: "SortKey { column: k.column, ..*k }",
        integration: &[LATE],
        unit: &[],
    },
    Mutation {
        name: "late-list-columns-not-handed-on",
        package: Package::Core,
        breaks: "the select list below hands on no column the expressions above read (RULES.md D1306)",
        file: LATE_LIST_FILE,
        original: "if place_of(&below, c).is_none() {",
        replacement: "if place_of(&below, c).is_none() && c > usize::MAX {",
        integration: &[LATE],
        unit: &[],
    },
];
