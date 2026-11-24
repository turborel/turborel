//! The mutations of the defects a review of comments found (RULES.md
//! R-TEST-9, D1040 to D1046): an index join is read direct only by a
//! unique index; a filter stays above a projection that is not
//! deterministic; COPY DESCRIPTOR with TYPE copies a scope; a slow
//! statement keeps its text while its spills are only compressed; an IVF
//! directory frees its pages when a write fails; a range join is a plan
//! of no order; and a learned plan is not weighed as a unique probe.

use super::super::{Mutation, Package};

/// Each fix of the review, taken back, which its test must then fail.
pub const REVIEW_FIXES: [Mutation; 5] = [
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
    Mutation {
        name: "copy-type-scope-dropped",
        package: Package::Core,
        breaks: "COPY DESCRIPTOR with TYPE leaves a reference type's SCOPE components behind",
        file: "src/db/dynamic/ptf.rs",
        original: "    DescriptorItem::ScopeCatalog,\n    DescriptorItem::ScopeSchema,\n    DescriptorItem::ScopeName,\n];",
        replacement: "    DescriptorItem::Type,\n    DescriptorItem::Type,\n    DescriptorItem::Type,\n];",
        integration: &["ptf_statements::a_reference_types_scope_is_copied_with_its_type_20_6"],
        unit: &[],
    },
    Mutation {
        name: "slow-text-withheld-for-a-compression",
        package: Package::Core,
        breaks: "a slow statement loses its text while the spill areas are compressed and nothing is encrypted",
        file: "src/db/slow.rs",
        original: "    let text = (!st.spill_encodings().confidential()).then(text);",
        replacement: "    let text = st.spill_encodings().stages().is_empty().then(text);",
        integration: &["diagnostics::a_slow_statement_is_told_with_its_text_while_spills_are_only_compressed"],
        unit: &[],
    },
    Mutation {
        name: "ivf-directory-pages-leaked",
        package: Package::Core,
        breaks: "an IVF directory whose write of a page fails leaves the pages it made in use",
        file: "src/engine/ivf/lists.rs",
        original: "    let written = made(pool, file, chunks.len().max(1), &mut pages).and_then(|()| filled(pool, file, &pages, &chunks));",
        replacement: "    let written = made(pool, file, chunks.len().max(1), &mut pages);\n    filled(pool, file, &pages, &chunks)?;",
        integration: &[],
        unit: &["engine::ivf::tests::ivf_directory_pages_are_freed_when_their_write_fails"],
    },
];
