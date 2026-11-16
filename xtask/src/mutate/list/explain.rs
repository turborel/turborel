//! The mutations of EXPLAIN and of a statement's first word (RULES.md
//! R-TEST-9): EXPLAIN not read as the statement, its query's errors
//! placed as though EXPLAIN were not written, and a first word that
//! begins no statement refused as a table's name.

use super::{Mutation, Package};

/// The test of EXPLAIN in direct SQL.
const EXPLAINED: &str = "direct::explain_gives_the_plan_as_rows";

/// The mutations of EXPLAIN.
pub const EXPLAIN: [Mutation; 3] = [
    Mutation {
        name: "explain-not-read",
        package: Package::Core,
        breaks: "EXPLAIN before a query is read as a table's name, and refused",
        file: "src/db/direct.rs",
        original: "            && w == EXPLAIN\n",
        replacement: "            && w == \"NO SUCH WORD\"\n",
        integration: &[EXPLAINED],
        unit: &[],
    },
    Mutation {
        name: "explain-errors-misplaced",
        package: Package::Core,
        breaks: "an error of EXPLAIN's query is placed as though EXPLAIN were not written",
        file: "src/db/direct.rs",
        original: "let rest = format!(\"{}{}\", \" \".repeat(at), sql.chars().skip(at).collect::<String>());",
        replacement: "let rest = sql.chars().skip(at).collect::<String>();",
        integration: &[EXPLAINED],
        unit: &[],
    },
    Mutation {
        name: "explain-first-word-untold",
        package: Package::Core,
        breaks: "a first word that begins no statement is refused as a table's name not followed by a join",
        file: "src/parser/query.rs",
        original: "let (at, first) = (self.here(), self.pos == 0);",
        replacement: "let (at, first) = (self.here(), false);",
        integration: &[EXPLAINED],
        unit: &[],
    },
];
