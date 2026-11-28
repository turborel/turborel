//! The mutations of when a partition of a grouping goes wide (RULES.md
//! R-TEST-9, D1302): of entries mostly new, counted over the wave so
//! far, or on course to hold many groups.

use super::super::{Mutation, Package};

/// A partition wide only of entries mostly new, or on course for many groups.
const WIDE_NEW: &str =
    "exec::parallel::grouped::waves::tests::a_partition_goes_wide_only_of_entries_mostly_new_or_on_course_for_many_groups";

/// Mechanisms whose removal must fail the requirements of when a
/// partition goes wide.
pub const WIDE_PARTS: [Mutation; 3] = [
    Mutation {
        name: "wide-whatever-share-of-a-waves-rows-met-a-group",
        package: Package::Core,
        breaks: "a partition whose entries mostly meet a group it holds goes wide once its table passes the count, and pays for each (RULES.md D1302)",
        file: "src/exec/parallel/grouped/waves/wide.rs",
        original: "let mostly_new = met.saturating_mul(THOUSAND) <= self.met.saturating_mul(counted.so_far);",
        replacement: "let mostly_new = met.saturating_mul(THOUSAND) <= THOUSAND.saturating_mul(counted.so_far);",
        integration: &[],
        unit: &[WIDE_NEW],
    },
    Mutation {
        name: "wide-by-the-last-made-of-a-wave-alone",
        package: Package::Core,
        breaks: "the entries a wave has handed a partition are counted of its last Made alone, fewer than the groups the wave made, so none seems to have met a held group (RULES.md D1302)",
        file: "src/exec/parallel/grouped/waves/wide.rs",
        original: "counted.so_far = counted.so_far.saturating_add(made.groups_of(p));",
        replacement: "counted.so_far = made.groups_of(p);",
        integration: &[],
        unit: &[WIDE_NEW],
    },
    Mutation {
        name: "wide-never-by-the-groups-a-wave-is-on-course-for",
        package: Package::Core,
        breaks: "a partition on course to hold many groups, many of whose entries meet a held group, stays a table however far past a cache it grows (RULES.md D1302)",
        file: "src/exec/parallel/grouped/waves/wide.rs",
        original: "mostly_new || counted.began.saturating_add(to_come) >= self.course",
        replacement: "mostly_new",
        integration: &[],
        unit: &[WIDE_NEW],
    },
];
