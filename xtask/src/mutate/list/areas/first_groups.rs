//! The mutations of the groups a FETCH FIRST with no ORDER BY asks of a
//! grouping (RULES.md R-TEST-9, D1215): how many it is asked for, the
//! groups a grouping alone stops at, and the groups a part fixes and
//! tells its input.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of first groups.
pub const FIRST_GROUPS: [Mutation; 4] = [
    Mutation {
        name: "first-groups-asked-one",
        package: Package::Core,
        breaks: "a grouping is asked for one group, whatever OFFSET and FETCH FIRST count",
        file: "src/exec/limit/bounded.rs",
        original: "input, est, streamed, first: Some(most.rows) }",
        replacement: "input, est, streamed, first: Some(1) }",
        integration: &["first_groups::the_first_groups_given_are_whole"],
        unit: &[],
    },
    Mutation {
        name: "first-groups-stop-early",
        package: Package::Core,
        breaks: "a grouping alone stops one group short of those asked for",
        file: "src/exec/batch/groups.rs",
        original: "if !self.spilled && self.held.len() >= self.most {",
        replacement: "if !self.spilled && self.held.len() >= self.most.saturating_sub(1) {",
        integration: &["first_groups::the_first_groups_given_are_whole"],
        unit: &[],
    },
    Mutation {
        name: "first-groups-nulls-unmatched",
        package: Package::Core,
        breaks: "a part's filter of a kept group's null key matches no null, so the null group loses its rows",
        file: "src/exec/parallel/grouped/first.rs",
        original: "let nulls = (0..keys.rows).any(|r| keys.cell(at, r).is_null());",
        replacement: "let nulls = false;",
        integration: &["first_groups::a_part_gives_the_groups_of_its_first_rows"],
        unit: &[],
    },
    Mutation {
        name: "first-groups-warning-lost",
        package: Package::Core,
        breaks: "a grouping whose set function takes a column that may hold a null drops rows, and their warning",
        file: "src/exec/batch/first.rs",
        original: "!read.taken.iter().any(|t| if let Taken::Column(at) = t { nullable(at) } else { false })",
        replacement: "read.taken.iter().all(|t| matches!(t, Taken::Count | Taken::Column(_)))",
        integration: &["first_groups::a_grouping_that_could_raise_reads_every_row"],
        unit: &[],
    },
];
