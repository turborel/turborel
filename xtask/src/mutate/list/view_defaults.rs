//! The mutations of the defaults of a view's columns (RULES.md R-TEST-9),
//! each a column's counterpart column's default [ISO/IEC 9075-2:2023 §11.5
//! GR3d]: a projection, a grouping column, a join's second operand or a
//! scan's row id given the wrong counterpart, an updatable view's columns
//! none, a domain's default lost, and each place a row of a view an
//! INSTEAD OF trigger takes is given its defaults left null.

use super::{Mutation, Package};

/// The file of a MERGE's clauses INSTEAD OF triggers take.
const MERGE_ROUTED: &str = "src/db/merge/routed.rs";
/// The file of the INSTEAD OF trigger that takes a change of a view.
const ROUTED: &str = "src/db/triggers/routed.rs";
/// The file of the counterparts of a plan's columns.
const COUNTERPARTS: &str = "src/logical/counterparts.rs";
/// The file of the defaults of a view's columns.
const DEFAULTED: &str = "src/binder/counterparts.rs";
/// The file of an INSERT, UPDATE or DELETE of a view its trigger takes.
const REROUTED: &str = "src/db/triggers/rerouted.rs";
/// The test of an INSERT into a view its trigger takes.
const INSERTED: &str = "triggers_defaults::an_insert_a_trigger_takes_gives_each_column_not_named_its_counterparts_default";
/// The test of SET DEFAULT of a view its trigger takes.
const SET_DEFAULT: &str = "triggers_defaults::set_default_gives_a_column_its_counterparts_default";
/// The test of a MERGE into a view its triggers take.
const MERGE_DEFAULTS: &str = "triggers_defaults::a_merge_into_a_view_its_triggers_take_gives_its_defaults";
/// The test of a view of a join, and of a union.
const JOINED: &str = "triggers_defaults::a_view_of_a_join_takes_each_columns_default_and_one_of_a_union_none";
/// The test of an INSERT through a view over a trigger's.
const OVER: &str = "triggers_defaults::an_insert_through_a_view_over_a_trigger_view_gives_its_other_columns_their_defaults";

/// The mutations of the defaults of a view's columns.
pub const VIEW_DEFAULTS: [Mutation; 14] = [
    Mutation {
        name: "view-defaults-counterpart-of-the-next-column",
        package: Package::Core,
        breaks: "a projection's column reference is taken as the counterpart of the column after the one it references",
        file: COUNTERPARTS,
        original: "if let Expr::Column(k) = e { inner.get(*k).cloned().flatten() } else { None }",
        replacement: "if let Expr::Column(k) = e { inner.get(k.saturating_add(1)).cloned().flatten() } else { None }",
        integration: &[INSERTED],
        unit: &["logical::counterparts::tests::a_projection_keeps_only_its_column_references"],
    },
    Mutation {
        name: "view-defaults-grouping-column-no-counterpart",
        package: Package::Core,
        breaks: "a grouping column has no counterpart",
        file: COUNTERPARTS,
        original: "groups.iter().map(|&g| inner.get(g).cloned().flatten()).collect();",
        replacement: "groups.iter().map(|_| None).collect();",
        integration: &[],
        unit: &["logical::counterparts::tests::a_grouping_column_is_its_column_and_a_set_function_none"],
    },
    Mutation {
        name: "view-defaults-join-second-operand-no-counterpart",
        package: Package::Core,
        breaks: "a column of a join's second operand has no counterpart",
        file: COUNTERPARTS,
        original: "out.extend(right.counterparts());",
        replacement: "out.extend(vec![None; right.width()]);",
        integration: &[JOINED],
        unit: &["logical::counterparts::tests::a_product_is_both_operands_in_order"],
    },
    Mutation {
        name: "view-defaults-row-id-a-counterpart",
        package: Package::Core,
        breaks: "the row id a MERGE's target reads is taken as a column of its table",
        file: COUNTERPARTS,
        original: "let columns = names.len().saturating_sub(usize::from(*located));",
        replacement: "let columns = names.len().saturating_sub(usize::from(*located && false));",
        integration: &[],
        unit: &["logical::counterparts::tests::a_scan_is_its_own_table_but_for_its_row_id"],
    },
    Mutation {
        name: "view-defaults-updatable-view-no-defaults",
        package: Package::Core,
        breaks: "an updatable view's columns take no default of its base table's",
        file: DEFAULTED,
        original: "Source::Base(vb) => vb.map.iter().map(|&k| vb.base.columns.get(k).cloned()).collect(),",
        replacement: "Source::Base(_) => Vec::new(),",
        integration: &[OVER],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-domain-default-lost",
        package: Package::Core,
        breaks: "a view's column whose counterpart takes its domain's default takes none",
        file: DEFAULTED,
        original: "cc.default.clone().or_else(|| cc.domain.as_ref().and_then(|d| d.default.clone()))",
        replacement: "cc.default.clone().or_else(|| cc.domain.as_ref().and_then(|d| d.default.clone()).and(None))",
        integration: &[INSERTED],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-merge-set-default-null",
        package: Package::Core,
        breaks: "SET DEFAULT of a MERGE into a view its triggers take gives the null value",
        file: "src/binder/views.rs",
        original: "desc.columns = super::counterparts::defaulted(db, std::mem::take(&mut desc.columns), &source);",
        replacement: "desc.columns = std::mem::take(&mut desc.columns);",
        integration: &[MERGE_DEFAULTS],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-insert-unnamed-null",
        package: Package::Core,
        breaks: "a column an INSERT into a view its trigger takes names not is null",
        file: REROUTED,
        original: "let mut new = defaults.clone();",
        replacement: "let mut new = vec![Value::Null; defaults.len()];",
        integration: &[INSERTED],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-default-values-null",
        package: Package::Core,
        breaks: "INSERT DEFAULT VALUES into a view its trigger takes gives a row of nulls",
        file: REROUTED,
        original: "out.push((0, Row::new(), defaults))?;",
        replacement: "out.push((0, Row::new(), vec![Value::Null; defaults.len()]))?;",
        integration: &[INSERTED],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-insert-default-null",
        package: Package::Core,
        breaks: "DEFAULT among an INSERT's values into a view its trigger takes is null",
        file: REROUTED,
        original: "default: defaults.get(k).cloned().unwrap_or(Value::Null),",
        replacement: "default: Value::Null,",
        integration: &[INSERTED],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-update-default-null",
        package: Package::Core,
        breaks: "SET DEFAULT of an UPDATE of a view its trigger takes gives the null value",
        file: REROUTED,
        original: "UpdateSource::Default => defaults.get(k).cloned().unwrap_or(Value::Null),",
        replacement: "UpdateSource::Default => Value::Null,",
        integration: &[SET_DEFAULT],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-nested-insert-other-columns-null",
        package: Package::Core,
        breaks: "a column of a trigger's view that the view over it inserted into lacks is null",
        file: ROUTED,
        original: "(0..self.width).map(|j| defaults.get(j).cloned().unwrap_or(Value::Null)).collect();",
        replacement: "(0..self.width).map(|j| defaults.get(j.saturating_add(self.width)).cloned().unwrap_or(Value::Null)).collect();",
        integration: &[OVER],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-merge-insert-unnamed-null",
        package: Package::Core,
        breaks: "a column a MERGE's insert a trigger takes names not is null",
        file: MERGE_ROUTED,
        original: "None => Ok(default.clone()),",
        replacement: "None => Ok(default.clone()).and(Ok(Value::Null)),",
        integration: &[MERGE_DEFAULTS, OVER],
        unit: &[],
    },
    Mutation {
        name: "view-defaults-merge-insert-default-null",
        package: Package::Core,
        breaks: "DEFAULT among a MERGE's insert values a trigger takes is null",
        file: MERGE_ROUTED,
        original: "let default = j.and_then(|j| defaults.get(j)).cloned().unwrap_or(Value::Null);",
        replacement: "let default = j.and(None::<Value>).unwrap_or(Value::Null);",
        integration: &[MERGE_DEFAULTS],
        unit: &[],
    },
];
