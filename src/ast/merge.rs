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
    /// ONLY ( `<table name>` ) was written: of a typed table, its rows
    /// that have no subrow in a proper subtable [GR6a i 1 A I].
    pub only: bool,
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
    /// \[EXTENSION\] Of INTO PROPERTY, which it is written as, what it
    /// writes.
    pub property: Option<Box<IntoProperty>>,
}

/// \[EXTENSION\] `GRAPH_ALGO.<name> ( ... ) INTO PROPERTY <column>`
/// `[ OF <table> [ ( <key column> ) ] ]` (RULES.md D-GRAPH-ALGO), a graph
/// algorithm's result written as a property of the vertices [GS §18]: the
/// MERGE that sets the column of each row whose key is a result row's
/// vertex to its value, the column added first if the table has none of
/// its name. Of a property graph named in place of the edges, the rows
/// are its vertex tables', each of the vertices its identities name, of
/// every vertex table unless OF names one (`db/graphs/property.rs`).
#[derive(Debug, Clone)]
pub struct IntoProperty {
    /// The invocation, as written.
    pub invocation: String,
    /// The table, as written, if OF names one.
    pub table: Option<String>,
    /// The name the edges are written as, if they are a name: of a
    /// property graph, whose vertex tables the property is written to.
    pub graph: Option<TableName>,
    /// The column written.
    pub column: String,
    /// The table's key column, if written, else its primary key's.
    pub key: Option<String>,
    /// The name of the result's value column.
    pub value: String,
    /// The type of the column added, as written; none of a vertex's key's.
    pub value_type: Option<&'static str>,
}

impl IntoProperty {
    /// The table written, or the graph's name if OF names none.
    pub fn target(&self) -> String {
        let graph = self.graph.as_ref().map(|g| g.auth.as_ref().map_or_else(|| g.ident.clone(), |a| format!("{a}.{}", g.ident)));
        self.table.clone().or(graph).unwrap_or_default()
    }

    /// The text of the MERGE that writes the property, the table's key
    /// column `key`.
    pub fn merge(&self, key: &str) -> String {
        let id = crate::catalog::identifier;
        format!(
            "MERGE INTO {} AS GRAPH_ALGO_TARGET USING {} AS GRAPH_ALGO_RESULT ON GRAPH_ALGO_TARGET.{} = GRAPH_ALGO_RESULT.VERTEX \
             WHEN MATCHED THEN UPDATE SET {} = GRAPH_ALGO_RESULT.{}",
            self.target(),
            self.invocation,
            id(key),
            id(&self.column),
            id(&self.value)
        )
    }
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
