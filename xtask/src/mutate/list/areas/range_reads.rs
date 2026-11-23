//! The mutations of direct range reads (RULES.md R-TEST-9, D930): a kept
//! plan is read direct only while the setting asks for it; a row is kept
//! only if the version read has its entry's key; the range read is told
//! to the table; FETCH FIRST stops at its count; a set function passes
//! over a null; and a column given twice is copied, not moved.

use super::super::{Mutation, Package};

/// Mechanisms of direct range reads whose removal must fail their tests.
pub const RANGE_READS: [Mutation; 6] = [
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
];
