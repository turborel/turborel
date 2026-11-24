//! The mutations of the defects a review of comments found (RULES.md
//! R-TEST-9, D1040 to D1046): an index join is read direct only by a
//! unique index; a filter stays above a projection that is not
//! deterministic; COPY DESCRIPTOR with TYPE copies a scope; a slow
//! statement keeps its text while its spills are only compressed; an IVF
//! directory frees its pages when a write fails; a range join is a plan
//! of no order; and a learned plan is not weighed as a unique probe.

use super::super::{Mutation, Package};

/// Each fix of the review, taken back, which its test must then fail.
pub const REVIEW_FIXES: [Mutation; 2] = [
    Mutation {
        name: "direct-join-by-a-key-not-unique",
        package: Package::Core,
        breaks: "an index join by an index that is not unique is read direct, each left row paired with its key's first row alone",
        file: "src/exec/join/indexed/direct.rs",
        original: "        if !ix.unique || keys.len() != ix.columns.len() || !keys.iter().zip(&ix.columns).all(same) {",
        replacement: "        if keys.len() != ix.columns.len() || !keys.iter().zip(&ix.columns).all(same) {",
        integration: &[],
        unit: &["exec::range::tests::a_join_by_a_whole_key_not_unique_pairs_every_row_of_the_key"],
    },
    Mutation {
        name: "filter-below-projection-not-deterministic",
        package: Package::Core,
        breaks: "a filter goes below a projection of a call that is not deterministic, and judges another value than the row has",
        file: "src/optimizer/views.rs",
        original: "        if cond.can_raise() || exprs.iter().any(Expr::can_raise) || !exprs.iter().all(expr_pure) {",
        replacement: "        if cond.can_raise() || exprs.iter().any(|e| e.can_raise() || (!expr_pure(e) && false)) {",
        integration: &["functions::a_filter_judges_the_value_a_call_not_deterministic_gave"],
        unit: &["optimizer::views::tests::a_filter_stays_above_a_projection_not_deterministic"],
    },
];
