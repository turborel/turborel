//! The mutations of the column store's skipping (RULES.md R-TEST-9,
//! D510-D512): the shadow bits a write and a group published set, the
//! zones of the tree's rows a scan reads the delta by, and imprints, their
//! masks and the order conjuncts are judged in, and dictionaries in order.

use super::super::{Mutation, Package};

/// The shadow bits' and the delta zones' unit tests.
const BITS: &str = "engine::rows::frozen::shadowed::shadow_bits_mark_writes_and_keep_scans_the_same";
const DELTA: &str = "engine::rows::frozen::shadowed::delta_zones_skip_the_tree_rows_no_condition_matches";
/// Imprints' unit tests.
const KEPT: &str = "engine::columns::imprints::tests::imprints_never_leave_out_a_matching_row";
const MASKS: &str = "engine::columns::imprints::tests::masks_are_of_the_bins_a_condition_may_match";
/// Dictionaries in order.
const SORTED: &str = "engine::columns::judged::sorted::tests::dictionaries_in_order_judge_by_codes";
const ORDERED: &str = "engine::columns::imprints::tests::conjuncts_are_ordered_narrowest_first";

/// Mechanisms whose removal must fail the skipping's requirements.
pub const SKIPPING: [Mutation; 13] = [
    Mutation {
        name: "shadow-bits-writer",
        package: Package::Core,
        breaks: "a change of a frozen row sets no shadow bit, so a scan reads its group's old value",
        file: "src/engine/rows/mvcc/versions.rs",
        original: "        self.mark_shadow(id);\n        if thawed.is_some() {",
        replacement: "        if thawed.is_some() {",
        integration: &[],
        unit: &[BITS],
    },
    Mutation {
        name: "shadow-bits-published",
        package: Package::Core,
        breaks: "a group published marks no entry the tree holds in its range, so a row left open as it froze is missed",
        file: "src/engine/rows/frozen/freeze.rs",
        original: "            self.mark_published(m, &entries)?;",
        replacement: "            let _ = m;",
        integration: &[],
        unit: &[BITS],
    },
    Mutation {
        name: "delta-zones",
        package: Package::Core,
        breaks: "a scan reads every row of the tree, whatever zones its condition may match",
        file: "src/engine/rows/frozen/batches.rs",
        original: "RowTable::ranged(&self.table, self.table.zones.ranges(self.condition.as_ref()))",
        replacement: "RowTable::ranged(&self.table, vec![(0, None)])",
        integration: &[],
        unit: &[DELTA],
    },
    Mutation {
        name: "imprints-mask",
        package: Package::Core,
        breaks: "a block is kept when its word shares no bit with the mask, and skipped when it does",
        file: "src/engine/columns/imprints.rs",
        original: "        if u64::from_le_bytes(*word) & mask != 0 {",
        replacement: "        if u64::from_le_bytes(*word) & mask == 0 {",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "imprints-null",
        package: Package::Core,
        breaks: "a conjunct true of a null is judged by imprints, whose nulls set no bit",
        file: "src/engine/columns/imprints.rs",
        original: "    if edges.is_empty() || conjunct.matches_key(&[&Value::Null]) {",
        replacement: "    if edges.is_empty() {",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "imprints-narrow",
        package: Package::Core,
        breaks: "a scan reads every block its zone maps keep, its imprints unused",
        file: "src/engine/columns/scan.rs",
        original: "            runs = both(&runs, &imprints::bridged(&alone));",
        replacement: "            let _ = &alone;",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "imprints-order",
        package: Package::Core,
        breaks: "conjuncts are judged in the order written, not narrowest first",
        file: "src/engine/columns/scan.rs",
        original: "    kept.sort_by_key(|&(_, rows)| rows);",
        replacement: "",
        integration: &[],
        unit: &[ORDERED],
    },
    Mutation {
        name: "skipping-two-columns",
        package: Package::Core,
        breaks: "a conjunct of two columns takes the other column as having no value, so no part may match",
        file: "src/engine/columns/scan.rs",
        original: "                *slot = imprints::whole(zones);",
        replacement: "                *slot = None;",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "imprints-useful",
        package: Package::Core,
        breaks: "a column scattered so its blocks fall in most bins gets imprints",
        file: "src/engine/columns/imprints.rs",
        original: "    set <= MOST_SET.saturating_mul(words.len())",
        replacement: "    set <= BINS.saturating_mul(words.len())",
        integration: &[],
        unit: &[KEPT],
    },
    Mutation {
        name: "imprints-gap",
        package: Package::Core,
        breaks: "a scan skips every gap imprints leave, its runs cut short",
        file: "src/engine/columns/imprints.rs",
        original: "from.saturating_sub(last.1) < GAP",
        replacement: "from.saturating_sub(last.1) < 1",
        integration: &[],
        unit: &[MASKS],
    },
    Mutation {
        name: "dictionary-in-order",
        package: Package::Core,
        breaks: "a dictionary of texts keeps its entries as first seen",
        file: "src/engine/columns.rs",
        original: "(sizer.table(), padding(t).is_some())",
        replacement: "(sizer.table(), false)",
        integration: &[],
        unit: &[SORTED],
    },
    Mutation {
        name: "dictionary-order-unsorted",
        package: Package::Core,
        breaks: "a dictionary says its entries are in order and keeps them in reverse",
        file: "src/engine/columns/encoded/dictionary.rs",
        original: "compare_strings(x, y).then_with",
        replacement: "compare_strings(y, x).then_with",
        integration: &[],
        unit: &[SORTED],
    },
    Mutation {
        name: "dictionary-spans",
        package: Package::Core,
        breaks: "the entries equal to a constant are taken to begin where those above it do",
        file: "src/engine/columns/judged/sorted.rs",
        original: "Some(order(e)? != Ordering::Less)",
        replacement: "Some(order(e)? == Ordering::Greater)",
        integration: &[],
        unit: &[SORTED],
    },
];
