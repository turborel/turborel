//! The mutations of direct range reads (RULES.md R-TEST-9, D930): a kept
//! plan is read direct only while the setting asks for it; a row is kept
//! only if the version read has its entry's key; the range read is told
//! to the table; FETCH FIRST stops at its count; a set function passes
//! over a null; a column given twice is copied, not moved; a cursor of a
//! long range is left to the executor (D931); and a join by a whole
//! unique key pairs no left row with a null key, and keeps only the pairs
//! the rest of its condition is true of (D932).

use super::super::{Mutation, Package};

/// Mechanisms of direct range reads whose removal must fail their tests.
pub const RANGE_READS: [Mutation; 9] = [
    Mutation {
        name: "range-read-setting-ignored",
        package: Package::Core,
        breaks: "point operations turned off still run a kept plan as its direct range read",
        file: "src/physical/kept/filled.rs",
        original: "        if self.fills || !crate::exec::point_operations(st) {",
        replacement: "        if self.fills {",
        integration: &[],
        unit: &["exec::range::tests::point_operations_off_run_a_kept_range_through_the_executor"],
    },
    Mutation {
        name: "range-read-key-unchecked",
        package: Package::Core,
        breaks: "an entry of a key its row left gives the row, so a range counts it where it no longer is",
        file: "src/exec/range/scan.rs",
        original: "        if key.as_slice() != entry.key && !Key(decode_key(entry.key, entry.types)?).is_of(listed, &scan.key) {",
        replacement: "        if false {",
        integration: &["range_reads::a_range_read_direct_passes_over_an_entry_of_a_key_its_row_left"],
        unit: &[],
    },
    Mutation {
        name: "range-read-unrecorded",
        package: Package::Core,
        breaks: "a range read direct is not told to its table, so a serializable transaction that read it commits past a row added to it",
        file: "src/exec/range/scan.rs",
        original: "        storage.read_by_condition(tx, &told)?;",
        replacement: "        let _ = &told;",
        integration: &["range_reads::a_range_read_direct_is_recorded_where_reads_are_checked"],
        unit: &[],
    },
    Mutation {
        name: "range-read-limit-reads-on",
        package: Package::Core,
        breaks: "FETCH FIRST read direct gives rows past its count",
        file: "src/exec/range.rs",
        original: "        Ok(each(row)? && (left != Some(0) || drain))",
        replacement: "        each(row)",
        integration: &[],
        unit: &["exec::range::tests::a_range_of_an_index_is_read_as_its_plan_reads"],
    },
    Mutation {
        name: "range-read-fold-takes-nulls",
        package: Package::Core,
        breaks: "a set function folded over a range is fed the null values of its argument",
        file: "src/exec/range/fold.rs",
        original: "                warn(at.subqueries, SqlState::NULL_VALUE_ELIMINATED);\n                continue;",
        replacement: "                warn(at.subqueries, SqlState::NULL_VALUE_ELIMINATED);",
        integration: &["range_reads::ranges_are_read_direct_as_the_executor_reads_them"],
        unit: &["exec::range::tests::a_range_of_an_index_is_read_as_its_plan_reads"],
    },
    Mutation {
        name: "range-read-column-moved-twice",
        package: Package::Core,
        breaks: "a column a direct read gives twice is moved out of the row each time, the first a null",
        file: "src/exec/range.rs",
        original: "            Ok(if again { value.clone() } else { std::mem::replace(value, Value::Null) })",
        replacement: "            Ok(std::mem::replace(value, Value::Null))",
        integration: &[],
        unit: &["exec::range::tests::a_range_of_an_index_is_read_as_its_plan_reads"],
    },
    Mutation {
        name: "range-read-cursor-long-range-cut",
        package: Package::Core,
        breaks: "a cursor of a range longer than it reads ahead gives the rows read ahead alone",
        file: "src/db/stream/ranged.rs",
        original: "    if most.is_some_and(|most| rows.len() >= most) {",
        replacement: "    if false {",
        integration: &["range_reads::cursors_of_ranges_read_direct_give_what_the_executor_gives"],
        unit: &[],
    },
    Mutation {
        name: "range-read-join-null-key-probed",
        package: Package::Core,
        breaks: "a left row with a null for the key probes with it, and fails or pairs",
        file: "src/exec/join/indexed/direct.rs",
        original: "            let Some(value) = left.get(k).filter(|v| !v.is_null()) else { return Ok(false) };",
        replacement: "            let Some(value) = left.get(k) else { return Ok(false) };",
        integration: &[],
        unit: &["exec::range::tests::a_null_key_of_a_direct_join_reads_nothing"],
    },
    Mutation {
        name: "range-read-join-condition-dropped",
        package: Package::Core,
        breaks: "a join read direct keeps every pair its probe finds, whatever the rest of its condition",
        file: "src/exec/join/indexed/direct.rs",
        original: "            Some(cond) => Ok(eval_cond(cond, pair, at.subqueries)? == Truth::True),",
        replacement: "            Some(_) => Ok(true),",
        integration: &["range_reads::ranges_are_read_direct_as_the_executor_reads_them"],
        unit: &["exec::range::tests::a_join_by_a_whole_unique_key_is_read_as_its_plan_reads"],
    },
];
