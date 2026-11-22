//! The mutations of the allocations taken out of the TPC-C transaction
//! path (RULES.md R-TEST-9): an array cast where it is, a record kept whole
//! written in line, its fixed columns put in place, a log record's values
//! counted before it is written, a kept program's statements found by
//! their addresses, and an update's candidate row made without copying the
//! columns it sets.

use super::super::{Mutation, Package};

/// The record written in line.
const IN_LINE_FILE: &str = "src/engine/record/inline.rs";
/// The unit test of records written in line.
const KEPT_WHOLE: &str = "engine::record::tests::records_kept_whole_are_written_in_line";

/// Mechanisms of the TPC-C path's allocations whose removal must fail their tests.
pub const TPCC_ALLOCS: [Mutation; 7] = [
    Mutation {
        name: "allocs-array-cast-skipped",
        package: Package::Core,
        breaks: "an array no other value shares keeps its elements uncast",
        file: "src/value/types/array.rs",
        original: "*e = cast(std::mem::replace(e, Value::Null), element)?;",
        replacement: "*e = std::mem::replace(e, Value::Null);",
        integration: &[],
        unit: &["value::types::tests::arrays_cast_each_element_shared_or_not"],
    },
    Mutation {
        name: "allocs-in-line-past-threshold",
        package: Package::Core,
        breaks: "a value a byte past the threshold is kept in line",
        file: IN_LINE_FILE,
        original: "if end.saturating_sub(before) > spill.threshold {",
        replacement: "if end.saturating_sub(before) > spill.threshold.saturating_add(1) {",
        integration: &[],
        unit: &[KEPT_WHOLE],
    },
    Mutation {
        name: "allocs-in-line-past-most",
        package: Package::Core,
        breaks: "a record a byte longer than the most a row may take is kept whole",
        file: IN_LINE_FILE,
        original: "if record.len() > spill.most {",
        replacement: "if record.len() > spill.most.saturating_add(1) {",
        integration: &[],
        unit: &[KEPT_WHOLE],
    },
    Mutation {
        name: "allocs-fixed-column-misplaced",
        package: Package::Core,
        breaks: "a fixed column written in line lands at its offset in the fixed part counted from the record's start",
        file: IN_LINE_FILE,
        original: "put(record.get_mut(base..).unwrap_or_default(), self.fixed_at().saturating_add(at), bytes);",
        replacement: "put(record.get_mut(base..).unwrap_or_default(), at, bytes);",
        integration: &[],
        unit: &["engine::record::tests::rows_read_back_as_written"],
    },
    Mutation {
        name: "allocs-value-length-short",
        package: Package::Core,
        breaks: "a character string's bytes in a log record are counted without its length",
        file: "src/value/codec.rs",
        original: "Value::Char(s) => TAG_LEN.saturating_add(LENGTH_LEN).saturating_add(s.len()),",
        replacement: "Value::Char(s) => TAG_LEN.saturating_add(s.len()),",
        integration: &[],
        unit: &["value::codec::tests::value_lengths_are_the_bytes_written"],
    },
    Mutation {
        name: "allocs-kept-statement-missed",
        package: Package::Core,
        breaks: "a kept program's statement is not found among its own, so it keeps no plan",
        file: "src/db/routines/kept.rs",
        original: "binary_search_by_key(&address, |&(a, _)| a)",
        replacement: "binary_search_by_key(&address.wrapping_add(1), |&(a, _)| a)",
        integration: &["tpcc_plans::tpcc_procedures_plan_nothing_in_steady_state"],
        unit: &[],
    },
    Mutation {
        name: "allocs-unset-columns-dropped",
        package: Package::Core,
        breaks: "an update's candidate row has the columns it does not set null, not as they were",
        file: "src/db/assignment.rs",
        original: "if a.set.iter().any(|(at, _)| *at == i) { Value::Null } else { v.clone() }",
        replacement: "if a.set.iter().any(|(at, _)| *at != i) { Value::Null } else { v.clone() }",
        integration: &["kept_routines::a_kept_write_learns_its_plan_and_changes_each_calls_rows"],
        unit: &[],
    },
];
