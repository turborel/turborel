//! The mutations of the batch's shape and its kernels (RULES.md R-TEST-9,
//! D354, D355): an exact constant folded to a column's scale, a filter's
//! selections chained, a null word read once a word, a mask's bits past
//! its length cleared, a morsel's groups numbered by their first rows, and
//! a grouping of many groups kept serial.

use super::super::{Mutation, Package};

/// The filters of datetimes, truth values and ORs, by batch and by rows.
const FILTERS: &str = "batch_kinds::datetimes_truth_values_and_ors_filter_as_rows_do";

/// Mechanisms whose removal must fail the batch's requirements.
pub const BATCHES: [Mutation; 6] = [
    Mutation {
        name: "batches-fold-less-by-floor",
        package: Package::Core,
        breaks: "`<` of a constant not divided by the scale folds by its ceiling",
        file: "src/exec/batch/judge/ints.rs",
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
];
