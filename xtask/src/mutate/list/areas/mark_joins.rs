//! The mutations of mark joins (RULES.md R-TEST-9, D640 to D643): the null
//! list a left row of no null value reads, the right rows a left row of a
//! null value reads, ALL as NOT of the negated SOME, the unknown of a null
//! mark value, the rows alike held once only of no condition beyond the
//! keys, the marks of a batch, and the null list of the rows spilled.

use super::super::{Mutation, Package};

/// The test that compares every form made a mark join with its subquery.
const MATCHED: &str = "decorrelation::mark::mark_joins_match_the_subqueries";
/// The test of the marks of nulls and empty sets.
const NULLS: &str = "decorrelation::mark::marks_of_nulls_and_empty_sets";
/// The file of the held right rows.
const SIDE: &str = "src/exec/join/mark/side.rs";

/// Mechanisms whose removal must fail the mark join requirements.
pub const MARK_JOINS: [Mutation; 7] = [
    Mutation {
        name: "mark-null-list-unread",
        package: Package::Core,
        breaks: "a left row of no null value reads its bucket alone, so NOT IN of a set with a null is true",
        file: SIDE,
        original: "Ok(or(found, self.any(nulls, l, marker)?))",
        replacement: "Ok(found)",
        integration: &[NULLS, MATCHED],
        unit: &[],
    },
    Mutation {
        name: "mark-null-value-false",
        package: Package::Core,
        breaks: "a left row of a null value is marked false, so IN of a null value and a set not empty is false",
        file: SIDE,
        original: "return self.any(self.by_keys.as_ref().and_then(|k| k.get(&b)), l, marker);",
        replacement: "return Ok(Truth::False);",
        integration: &[NULLS, MATCHED],
        unit: &[],
    },
    Mutation {
        name: "mark-all-not-negated",
        package: Package::Core,
        breaks: "ALL is marked by its comparison, not by the comparison negated",
        file: "src/optimizer/unnesting/marked.rs",
        original: "let op = if *all { negated(*op) } else { *op };",
        replacement: "let op = *op;",
        integration: &[NULLS, MATCHED],
        unit: &[],
    },
    Mutation {
        name: "mark-null-value-true",
        package: Package::Core,
        breaks: "a pair of a null value makes an equality of the marks true, not unknown",
        file: "src/exec/join/mark.rs",
        original: "Ok(if unknown { Truth::Unknown } else { Truth::True })",
        replacement: "Ok(Truth::True)",
        integration: &[NULLS, MATCHED],
        unit: &[],
    },
    Mutation {
        name: "mark-alike-held-once-of-a-condition",
        package: Package::Core,
        breaks: "of rows alike in keys only the first is held though a condition beyond the keys decides",
        file: "src/exec/join/mark.rs",
        original: "if self.cond.is_some() || (self.compared.is_some() && self.marks.is_empty()) {",
        replacement: "if self.compared.is_some() && self.marks.is_empty() {",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "mark-batch-marks-null",
        package: Package::Core,
        breaks: "a mark join by batches gives each row an unknown mark",
        file: "src/exec/batch/joined/mark.rs",
        original: "*mark = side.mark(&l, &self.marker)?.value();",
        replacement: "*mark = Value::Null;",
        integration: &[MATCHED],
        unit: &[],
    },
    Mutation {
        name: "mark-spilled-null-list-unread",
        package: Package::Core,
        breaks: "spilled, a left row of no null value reads its bucket alone",
        file: SIDE,
        original: "Ok(or(any(&self.whole, &b, (l, marker))?, of_nulls))",
        replacement: "any(&self.whole, &b, (l, marker))",
        integration: &[NULLS, MATCHED],
        unit: &[],
    },
];
