//! [ISO/IEC 9075-2:2023 §14.12] The `<merge statement>`: rows of a table
//! updated, deleted or inserted from one source, each as the first of its
//! WHEN clauses that applies to it says.

use super::{ColumnName, Overriding, SearchCondition, SetClause, TableName, TableReference, ValuesRow};

/// [ISO/IEC 9075-2:2023 §14.12] `<merge statement>` ::= MERGE INTO
/// `<target table>` \[\[AS\] `<merge correlation name>`\] USING `<table
/// reference>` ON `<search condition>` `<merge operation specification>`
#[derive(Debug, Clone)]
pub struct MergeStatement {
    /// The `<table name>` of the `<target table>` \[SR1\].
    pub target: TableName,
    /// The `<merge correlation name>`, if written, and where \[SR8a\].
    pub correlation: Option<(String, usize)>,
    /// The `<table reference>` the source rows come from \[SR11\].
    pub source: TableReference,
    /// The `<search condition>` after ON, which matches a target row to
    /// a source row \[SR19\].
    pub on: SearchCondition,
    /// The `<merge when clause>`s of its `<merge operation
    /// specification>`, in the order written, at least one \[GR6a\].
    pub clauses: Vec<MergeWhen>,
}

/// [ISO/IEC 9075-2:2023 §14.12] `<merge when clause>` ::= `<merge when
/// matched clause>` | `<merge when not matched clause>`
#[derive(Debug, Clone)]
pub enum MergeWhen {
    /// WHEN MATCHED \[AND `<search condition>`\] THEN `<merge update or
    /// delete specification>`.
    Matched {
        /// The `<search condition>` after AND, if written.
        condition: Option<SearchCondition>,
        /// What it does to each of its subject rows.
        action: MatchedAction,
        /// Where WHEN begins.
        at: usize,
    },
    /// WHEN NOT MATCHED \[AND `<search condition>`\] THEN `<merge insert
    /// specification>`.
    NotMatched {
        /// The `<search condition>` after AND, if written.
        condition: Option<SearchCondition>,
        /// The row it inserts for each source row it applies to.
        insert: MergeInsert,
        /// Where WHEN begins.
        at: usize,
    },
}

/// [ISO/IEC 9075-2:2023 §14.12] `<merge update or delete specification>`
/// ::= `<merge update specification>` | `<merge delete specification>`
#[derive(Debug, Clone)]
pub enum MatchedAction {
    /// UPDATE SET `<set clause list>`, its `<set clause>`s in order.
    Update(Vec<SetClause>),
    /// DELETE \[Feature F314\].
    Delete,
}

/// [ISO/IEC 9075-2:2023 §14.12] `<merge insert specification>` ::= INSERT
/// \[(`<insert column list>`)\] \[`<override clause>`\] VALUES `<merge
/// insert value list>`
#[derive(Debug, Clone)]
pub struct MergeInsert {
    /// The `<insert column list>`, if written; else every column of the
    /// target table, in order \[SR18a\].
    pub columns: Option<Vec<ColumnName>>,
    /// The `<override clause>`, if written \[SR18h\].
    pub overriding: Option<Overriding>,
    /// The `<merge insert value list>`: each `<merge insert value
    /// element>` a value expression, NULL, DEFAULT or another
    /// `<contextually typed value specification>`, as a row of a `<table
    /// value constructor>` is \[SR18b, SR18e\].
    pub values: ValuesRow,
    /// Where INSERT begins.
    pub at: usize,
}
