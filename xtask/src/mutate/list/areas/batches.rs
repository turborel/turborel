//! The mutations of the batch's shape and its kernels (RULES.md R-TEST-9,
//! D354, D355): an exact constant folded to a column's scale, a filter's
//! selections chained, a null word read once a word, a mask's bits past
//! its length cleared, a chunk's bitmap joined to a mask at its offset, a
//! morsel's groups numbered by their first rows, a grouping of many
//! groups kept serial, and an IN list of exact constants folded to a
//! vector's scale (D1295).

use super::super::{Mutation, Package};

/// The filters of datetimes, truth values and ORs, by batch and by rows.
const FILTERS: &str = "batch_kinds::datetimes_truth_values_and_ors_filter_as_rows_do";

/// An IN list folded to a scale, and its tests: the list alone, a scan's
/// and a filter's.
const LISTED: &str = "src/storage/batch/listed.rs";
const LIST_FINDS: &str = "storage::batch::listed::tests::a_list_finds_the_units_equal_to_a_constant";
const LIST_FOLDS: &str = "storage::batch::listed::tests::constants_fold_to_the_scale_or_equal_nothing";
const LIST_SCANNED: &str = "engine::columns::judged::tests::lists_of_exact_constants_keep_what_their_values_keep";
const LIST_FILTERED: &str = "exec::batch::judge::tests::lists_of_exact_constants_over_integers_keep_what_values_keep";

/// Mechanisms whose removal must fail the batch's requirements.
pub const BATCHES: [Mutation; 14] = [
    Mutation {
        name: "batches-fold-less-by-floor",
        package: Package::Core,
        breaks: "`<` of a constant not divided by the scale folds by its ceiling",
        file: "src/storage/batch/bound.rs",
        original: "        (CompOp::Lt | CompOp::Ge, _) => bounded(op, ceil),",
        replacement: "        (CompOp::Lt | CompOp::Ge, _) => bounded(op, floor),",
        integration: &[],
        unit: &["exec::batch::judge::ints::tests::constants_fold_to_bounds_of_the_scale"],
    },
    Mutation {
        name: "batches-selection-unchained",
        package: Package::Core,
        breaks: "each conjunct judges the rows the one before kept",
        file: "src/exec/batch/judge.rs",
        original: "            let over = kept.as_deref().or(rows);",
        replacement: "            let over = rows;",
        integration: &[FILTERS],
        unit: &[],
    },
    Mutation {
        name: "batches-null-word-once",
        package: Package::Core,
        breaks: "a selection reads the null word of each 64 rows",
        file: "src/kernels/portable/select.rs",
        original: "        if n & (WORD_BITS - 1) == 0 {",
        replacement: "        if n == 0 {",
        integration: &["kernels::portable_kernels_keep_the_contract"],
        unit: &[],
    },
    Mutation {
        name: "batches-mask-untrimmed",
        package: Package::Core,
        breaks: "a mask's bits past its length stay cleared",
        file: "src/storage/batch/mask.rs",
        original: "        if tail > 0",
        replacement: "        if tail > WORD_BITS",
        integration: &[],
        unit: &["storage::batch::mask::tests::masks_read_from_bytes_at_any_offset"],
    },
    Mutation {
        name: "batches-morsel-rows-unnumbered",
        package: Package::Core,
        breaks: "a morsel's groups by batch are numbered by their first rows, in order",
        file: "src/exec/batch/groups/morsel.rs",
        original: "        number = number.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));",
        replacement: "        number = number.saturating_add(u64::from(count == usize::MAX));",
        integration: &["batch_kinds::morsels_read_by_batch_give_the_rows_alone_gives"],
        unit: &[],
    },
    Mutation {
        name: "batches-exchange-never-outweighs",
        package: Package::Core,
        breaks: "a grouping of many groups stays serial",
        file: "src/physical/parallel/exchange.rs",
        original: "    parallel >= r\n",
        replacement: "    parallel > f64::MAX\n",
        integration: &["batch_kinds::groupings_of_many_groups_stay_serial"],
        unit: &[],
    },
    Mutation {
        name: "batches-mask-append-unshifted",
        package: Package::Core,
        breaks: "a mask appended past a part of a word shifts its bits past the bits before",
        file: "src/storage/batch/mask.rs",
        original: "                    *last |= word << shift;",
        replacement: "                    *last |= word;",
        integration: &[],
        unit: &["storage::batch::mask::tests::masks_append_at_any_length"],
    },
    Mutation {
        name: "batches-chunk-nulls-from-start",
        package: Package::Core,
        breaks: "a chunk's nulls of a range are read from the range's first position",
        file: "src/engine/columns.rs",
        original: "        nulls.append(&Mask::from_lsb_bytes(self.nulls, from, count));",
        replacement: "        nulls.append(&Mask::from_lsb_bytes(self.nulls, 0, count));",
        integration: &[],
        unit: &["engine::columns::tests::chunks_give_their_truth_values"],
    },
    Mutation {
        name: "in-list-not-in-with-a-null",
        package: Package::Core,
        breaks: "NOT IN a list with a null constant is true of a row equal to none of the others",
        file: LISTED,
        original: "        if self.not { !self.null && !found } else { found }",
        replacement: "        if self.not { !found } else { found }",
        integration: &[],
        unit: &[LIST_FINDS],
    },
    Mutation {
        name: "in-list-a-span-past-a-word",
        package: Package::Core,
        breaks: "constants that span a word's bits and one more are kept as one word, the greatest lost",
        file: LISTED,
        original: ".filter(|&s| s < WORD)?;",
        replacement: ".filter(|&s| s <= WORD)?;",
        integration: &[],
        unit: &[LIST_FINDS],
    },
    Mutation {
        name: "in-list-out-of-order",
        package: Package::Core,
        breaks: "constants are halved as written, out of order",
        file: LISTED,
        original: "        units.sort_unstable();\n",
        replacement: "",
        integration: &[],
        unit: &[LIST_FINDS],
    },
    Mutation {
        name: "in-list-unheld-constant-refuses",
        package: Package::Core,
        breaks: "a constant no integer of the scale equals leaves the whole list unfolded",
        file: LISTED,
        original: "                Bound::All | Bound::None => {}",
        replacement: "                Bound::All | Bound::None => return None,",
        integration: &[],
        unit: &[LIST_FOLDS],
    },
    Mutation {
        name: "in-list-scan-not-in-as-in",
        package: Package::Core,
        breaks: "a scan judges NOT IN a list of exact constants as IN it",
        file: "src/engine/columns/judged.rs",
        original: "            Conjunct::NotIn { values, .. } => Listed::of(scale, values, true),",
        replacement: "            Conjunct::NotIn { values, .. } => Listed::of(scale, values, false),",
        integration: &[],
        unit: &[LIST_SCANNED],
    },
    Mutation {
        name: "in-list-filter-not-in-as-in",
        package: Package::Core,
        breaks: "a filter judges NOT IN a list of exact constants over integers as IN it",
        file: "src/exec/batch/judge/ints.rs",
        original: "Listed::of(ints.scale, set.values(), *not)",
        replacement: "Listed::of(ints.scale, set.values(), false)",
        integration: &[],
        unit: &[LIST_FILTERED],
    },
];
