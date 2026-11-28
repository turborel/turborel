//! The mutations of the column store's skipping (RULES.md R-TEST-9,
//! D510-D512): the shadow bits a write and a group published set, the
//! zones of the tree's rows a scan reads the delta by, and imprints, their
//! masks and the order conjuncts are judged in, dictionaries in order,
//! column sketches (D525), and a comparison a run's parts prove (D1293).

use super::super::{Mutation, Package};

/// The shadow bits' and the delta zones' unit tests.
const BITS: &str = "engine::rows::frozen::shadowed::shadow_bits_mark_writes_and_keep_scans_the_same";
const PUBLISHED: &str = "engine::rows::frozen::tests::published::a_group_read_before_its_bits_are_marked_misses_no_row";
const DELTA: &str = "engine::rows::frozen::shadowed::delta_zones_skip_the_tree_rows_no_condition_matches";
/// Imprints' unit tests.
const KEPT: &str = "engine::columns::imprints::tests::imprints_never_leave_out_a_matching_row";
const MASKS: &str = "engine::columns::imprints::tests::masks_are_of_the_bins_a_condition_may_match";
/// Dictionaries in order.
const SORTED: &str = "engine::columns::judged::sorted::tests::dictionaries_in_order_judge_by_codes";
const ORDERED: &str = "engine::columns::imprints::tests::conjuncts_are_ordered_narrowest_first";
/// Column sketches' unit tests.
const CODES: &str = "engine::columns::sketch::tests::codes_order_as_their_values";
const VERDICTS: &str = "engine::columns::judged::sketched::tests::codes_say_what_their_values_say";
const SCANS: &str = "engine::columns::judged::sketched::tests::scans_by_sketches_keep_what_values_keep";
const SPARSE: &str = "engine::columns::judged::sketched::tests::few_rows_asked_for_are_compared_without_the_sketch";

/// A comparison a run's parts prove.
const PROVED: &str = "src/engine/columns/judged/proved.rs";
const BOUNDS: &str = "engine::columns::judged::proved::tests::a_comparison_is_proved_at_the_bounds_its_parts_reach_and_not_past_them";
const NULLS: &str = "engine::columns::judged::proved::tests::a_part_that_holds_a_null_proves_nothing";
const EACH_ROW: &str = "engine::columns::judged::proved::tests::what_is_proved_of_a_run_is_true_of_each_of_its_rows";
const HELD: &str = "engine::columns::judged::proved::tests::rows_no_chunk_holds_prove_nothing";

/// Mechanisms whose removal must fail the skipping's requirements.
pub const SKIPPING: [Mutation; 25] = [
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
        name: "shadow-bits-settled",
        package: Package::Core,
        breaks: "a scan trusts a group's shadow bits before they are marked, so a row the mark has yet to find is missed",
        file: "src/engine/rows/frozen/shadow_bits.rs",
        original: "        if !self.settled() {\n            return vec![(self.first, self.last)];\n        }\n",
        replacement: "",
        integration: &[],
        unit: &[PUBLISHED],
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
        file: "src/engine/columns/fields.rs",
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
    Mutation {
        name: "sketch-point-code",
        package: Package::Core,
        breaks: "a value equal to a point is given the code below it, among the values between the edges",
        file: "src/engine/columns/sketch.rs",
        original: "below.saturating_add(u8::from(point && equal))",
        replacement: "below",
        integration: &[],
        unit: &[CODES],
    },
    Mutation {
        name: "sketch-bin-edge",
        package: Package::Core,
        breaks: "the values up to an edge are taken all below it, though the edge is among them",
        file: "src/engine/columns/sketch.rs",
        original: "Ordering::Equal => (equal && !inclusive).then_some(Ordering::Less),",
        replacement: "Ordering::Equal => equal.then_some(Ordering::Less),",
        integration: &[],
        unit: &[CODES, VERDICTS],
    },
    Mutation {
        name: "sketch-boundary",
        package: Package::Core,
        breaks: "a code a constant falls within is said to keep or leave all its rows, none compared",
        file: "src/engine/columns/judged/sketched.rs",
        original: "        if !mixed.get() {",
        replacement: "        if mixed.get() || !mixed.get() {",
        integration: &[],
        unit: &[VERDICTS, SCANS],
    },
    Mutation {
        name: "sketch-verdict",
        package: Package::Core,
        breaks: "a code whose values the conjunct is true of is said to leave them, and one false of them to keep them",
        file: "src/engine/columns/judged/sketched.rs",
        original: "out.set(code, if keeps { Verdict::Keep } else { Verdict::Drop });",
        replacement: "out.set(code, if keeps { Verdict::Drop } else { Verdict::Keep });",
        integration: &[],
        unit: &[VERDICTS, SCANS],
    },
    Mutation {
        name: "sketch-null",
        package: Package::Core,
        breaks: "a null's code leaves its row, whatever the conjunct is of a null",
        file: "src/engine/columns/judged/sketched.rs",
        original: "if pred.of_null { Verdict::Keep } else { Verdict::Drop }",
        replacement: "Verdict::Drop",
        integration: &[],
        unit: &[VERDICTS, SCANS],
    },
    Mutation {
        name: "sketch-kept",
        package: Package::Core,
        breaks: "a row its code keeps is left out",
        file: "src/engine/columns/judged/coded.rs",
        original: "                Verdict::Keep => {\n                    out.push(r);\n                    continue;\n                }",
        replacement: "                Verdict::Keep => continue,",
        integration: &[],
        unit: &[SCANS],
    },
    Mutation {
        name: "sketch-sparse",
        package: Package::Core,
        breaks: "a run of few rows asked for reads the sketch's codes and judges each code of the group",
        file: "src/engine/columns/judged/coded.rs",
        original: "if symbols && dense {",
        replacement: "if symbols {",
        integration: &[],
        unit: &[SPARSE],
    },
    Mutation {
        name: "proved-with-a-null",
        package: Package::Core,
        breaks: "a part that holds a null proves a comparison, so the null's row is given unjudged by the scan",
        file: PROVED,
        original: ".filter(|_| part.nulls == 0)",
        replacement: ".filter(|_| part.nulls < u32::MAX)",
        integration: &[],
        unit: &[NULLS],
    },
    Mutation {
        name: "proved-strict-at-its-bound",
        package: Package::Core,
        breaks: "a part whose least value is the constant proves the column greater than it",
        file: PROVED,
        original: "        CompOp::Gt => low() == Some(Ordering::Greater),",
        replacement: "        CompOp::Gt => low().is_some_and(Ordering::is_ge),",
        integration: &[],
        unit: &[BOUNDS, EACH_ROW],
    },
    Mutation {
        name: "proved-by-its-first-part",
        package: Package::Core,
        breaks: "a run is proved by the first part it lies in alone, whatever the parts after it hold",
        file: PROVED,
        original: "zone.parts.get(low..=high)",
        replacement: "zone.parts.get(low..=low)",
        integration: &[],
        unit: &[BOUNDS],
    },
    Mutation {
        name: "proved-past-its-chunks",
        package: Package::Core,
        breaks: "a run is proved by the chunk its first row is in alone, whatever the chunks after it hold",
        file: PROVED,
        original: "    at >= to\n}",
        replacement: "    at > from\n}",
        integration: &[],
        unit: &[HELD],
    },
];
