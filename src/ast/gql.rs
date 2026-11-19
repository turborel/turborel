//! \[EXTENSION\] Abstract syntax of GQL (ISO/IEC 39075:2024), the second
//! host language of the property graph implementation standard (§9): its
//! session and transaction statements, and the procedures its queries and
//! data-modifying statements make, linear blocks chained by NEXT and
//! combined by UNION, EXCEPT, INTERSECT and OTHERWISE.
//!
//! A clause's MATCH pattern is GPML's, `GraphPattern`, as GRAPH_TABLE's
//! is. Each value expression and condition keeps, beside its tree, the
//! tokens of the GQL text it was read from (`GqlSpan`): the lowering to
//! SQL (`binder/gql.rs`) writes it again from them, its variables
//! replaced by the columns of the binding table. The graph type
//! definitions of Mode C (§4.3) are in `gql/types.rs`.

mod types;

pub use types::{EdgeTypeDefinition, GqlGraphDefinition, GraphTypeDefinition, GraphTyping, NodeTypeDefinition, PropertyTypeDefinition};

use super::{GraphPattern, PathPattern, SearchCondition, TableName, ValueExpression};

/// The tokens of a GQL text an expression was read from: from `start`
/// up to `end`, token numbers of the text as the lexer gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GqlSpan {
    /// Its first token.
    pub start: usize,
    /// The token after its last.
    pub end: usize,
}

/// A value expression of GQL, and its tokens.
#[derive(Debug, Clone)]
pub struct GqlExpression {
    /// Its tree.
    pub value: ValueExpression,
    /// Its tokens.
    pub span: GqlSpan,
}

/// A search condition of GQL, and its tokens.
#[derive(Debug, Clone)]
pub struct GqlCondition {
    /// Its tree.
    pub condition: SearchCondition,
    /// Its tokens.
    pub span: GqlSpan,
}

/// \[EXTENSION\] A GQL request: a session or transaction statement, or a
/// procedure of queries and data-modifying statements.
#[derive(Debug, Clone)]
pub enum GqlRequest {
    /// A `<session activity>`.
    Session(GqlSession),
    /// A `<start transaction command>`, COMMIT or ROLLBACK.
    Transaction(GqlTransaction),
    /// A procedure's statements.
    Procedure(GqlProcedure),
}

/// \[EXTENSION\] GQL's session statements.
#[derive(Debug, Clone)]
pub enum GqlSession {
    /// SESSION SET SCHEMA `<schema name>`.
    SetSchema(String),
    /// SESSION SET \[PROPERTY\] GRAPH `<graph name>`.
    SetGraph(TableName),
    /// SESSION SET TIME ZONE `<character string literal>`.
    SetTimeZone(String),
    /// SESSION RESET \[ALL | SCHEMA | \[PROPERTY\] GRAPH | TIME ZONE\]:
    /// what it resets, all if none.
    Reset(Option<GqlReset>),
    /// SESSION CLOSE.
    Close,
}

/// What SESSION RESET resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GqlReset {
    /// The schema.
    Schema,
    /// The graph.
    Graph,
    /// The time zone.
    TimeZone,
}

/// \[EXTENSION\] GQL's transaction statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GqlTransaction {
    /// START TRANSACTION \[READ ONLY | READ WRITE\]: whether READ ONLY,
    /// if written.
    Start(Option<bool>),
    /// COMMIT \[WORK\].
    Commit,
    /// ROLLBACK \[WORK\].
    Rollback,
}

/// \[EXTENSION\] A procedure: composite statements chained by NEXT, each
/// reading the table the one before returned [P2 §5.1].
#[derive(Debug, Clone)]
pub struct GqlProcedure {
    /// The statements, in order, one at least.
    pub statements: Vec<GqlComposite>,
}

/// A composite query statement: linear statements combined by set
/// operators, left to right.
#[derive(Debug, Clone)]
pub struct GqlComposite {
    /// The first.
    pub first: GqlLinear,
    /// Each later one and the operator before it.
    pub rest: Vec<(GqlSetOperator, GqlLinear)>,
}

/// A set operator between two linear statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GqlSetOperator {
    /// UNION, ALL if `true`.
    Union(bool),
    /// EXCEPT, ALL if `true`.
    Except(bool),
    /// INTERSECT, ALL if `true`.
    Intersect(bool),
    /// OTHERWISE: the right's rows if the left has none.
    Otherwise,
}

/// A linear statement: the graph it uses, its clauses in order, and how
/// it ends.
#[derive(Debug, Clone)]
pub struct GqlLinear {
    /// USE `<graph name>`, if written.
    pub graph: Option<TableName>,
    /// Its clauses, in order.
    pub clauses: Vec<GqlClause>,
    /// RETURN, FINISH, or nothing (after data-modifying statements).
    pub result: GqlResult,
    /// Where it begins.
    pub at: usize,
}

/// A clause of a linear statement, each a function from binding tables
/// to binding tables [P2 §5.1].
#[derive(Debug, Clone)]
pub enum GqlClause {
    /// \[OPTIONAL\] MATCH `<graph pattern>`.
    Match {
        /// OPTIONAL was written.
        optional: bool,
        /// The pattern, as written.
        pattern: GraphPattern,
        /// The pattern's tokens.
        span: GqlSpan,
    },
    /// FILTER `<search condition>`, or WHERE as a statement.
    Filter(GqlCondition),
    /// LET `<variable>` = `<value expression>` \[, ...\].
    Let(Vec<(String, GqlExpression)>),
    /// FOR `<variable>` IN `<list value>` \[WITH ORDINALITY | WITH OFFSET
    /// `<variable>`\].
    For {
        /// The variable each element is bound to.
        variable: String,
        /// The list.
        list: GqlExpression,
        /// The variable of the element's place, and whether it counts
        /// from 0 (OFFSET) rather than 1 (ORDINALITY).
        place: Option<(String, bool)>,
    },
    /// ORDER BY, OFFSET or SKIP, and LIMIT, as a statement.
    Page(GqlPage),
    /// CALL, of an inline procedure or a named one.
    Call(GqlCall),
    /// INSERT `<path pattern>` \[, ...\].
    Insert(Vec<PathPattern>),
    /// SET `<set item>` \[, ...\].
    Set(Vec<GqlSetItem>),
    /// REMOVE `<remove item>` \[, ...\].
    Remove(Vec<GqlRemoveItem>),
    /// \[DETACH | NODETACH\] DELETE `<value expression>` \[, ...\].
    Delete {
        /// DETACH was written.
        detach: bool,
        /// What is deleted: element variables.
        items: Vec<(String, usize)>,
    },
}

/// ORDER BY, OFFSET and LIMIT.
#[derive(Debug, Clone, Default)]
pub struct GqlPage {
    /// The sort keys, in order.
    pub order: Vec<GqlSort>,
    /// OFFSET or SKIP, if written.
    pub offset: Option<GqlExpression>,
    /// LIMIT, if written.
    pub limit: Option<GqlExpression>,
}

impl GqlPage {
    /// Whether nothing is written.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.offset.is_none() && self.limit.is_none()
    }
}

/// A sort key.
#[derive(Debug, Clone)]
pub struct GqlSort {
    /// Its value.
    pub value: GqlExpression,
    /// DESC (or DESCENDING) was written.
    pub descending: bool,
    /// NULLS FIRST (`Some(true)`) or NULLS LAST, if written.
    pub nulls_first: Option<bool>,
}

/// How a linear statement ends.
#[derive(Debug, Clone)]
pub enum GqlResult {
    /// RETURN.
    Return(Box<GqlReturn>),
    /// FINISH: no rows.
    Finish,
    /// Nothing written, after a data-modifying statement.
    None,
}

/// RETURN \[DISTINCT | ALL\] `<return item list>` \[GROUP BY ...\] and
/// its page.
#[derive(Debug, Clone)]
pub struct GqlReturn {
    /// DISTINCT was written.
    pub distinct: bool,
    /// Its items, or none for `*`.
    pub items: Option<Vec<GqlReturnItem>>,
    /// GROUP BY's names, if written.
    pub group_by: Vec<(String, usize)>,
    /// Its ORDER BY, OFFSET and LIMIT.
    pub page: GqlPage,
}

/// `<value expression>` \[AS `<identifier>`\].
#[derive(Debug, Clone)]
pub struct GqlReturnItem {
    /// The value.
    pub value: GqlExpression,
    /// Its name, if written.
    pub name: Option<String>,
}

/// CALL and what it calls.
#[derive(Debug, Clone)]
pub enum GqlCall {
    /// \[OPTIONAL\] CALL \[( `<variable>` \[, ...\] )\] { `<procedure>` }.
    Inline {
        /// OPTIONAL was written.
        optional: bool,
        /// The variables its scope clause imports, if written.
        scope: Option<Vec<String>>,
        /// The procedure.
        body: Box<GqlProcedure>,
    },
    /// \[OPTIONAL\] CALL `<procedure name>` ( `<arguments>` ) \[YIELD
    /// `<column>` \[AS `<variable>`\] \[, ...\]\].
    Named {
        /// OPTIONAL was written.
        optional: bool,
        /// The routine's name.
        name: TableName,
        /// Its arguments.
        arguments: Vec<GqlExpression>,
        /// The columns yielded, each and its variable.
        yields: Vec<(String, String)>,
    },
}

/// A SET item.
#[derive(Debug, Clone)]
pub enum GqlSetItem {
    /// `<variable>` . `<property>` = `<value expression>`.
    Property {
        /// The element's variable.
        variable: String,
        /// The property.
        property: String,
        /// The value.
        value: GqlExpression,
        /// Where it begins.
        at: usize,
    },
    /// `<variable>` = { `<property>` : `<value>` \[, ...\] }: every
    /// property replaced.
    All {
        /// The element's variable.
        variable: String,
        /// The properties and their values.
        properties: Vec<(String, GqlExpression)>,
        /// Where it begins.
        at: usize,
    },
    /// `<variable>` : `<label>`, or IS `<label>`.
    Label {
        /// The element's variable.
        variable: String,
        /// The label.
        label: String,
        /// Where it begins.
        at: usize,
    },
}

/// A REMOVE item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GqlRemoveItem {
    /// `<variable>` . `<property>`, and where it begins.
    Property(String, String, usize),
    /// `<variable>` : `<label>`, and where it begins.
    Label(String, String, usize),
}

/// \[EXTENSION\] EXECUTE GQL \[ON GRAPH `<graph name>`\] `<character
/// string literal>`: a GQL request run on the graph (graph standard §9),
/// or on the session's GQL graph if none is named.
#[derive(Debug, Clone)]
pub struct ExecuteGql {
    /// The graph, if named.
    pub graph: Option<TableName>,
    /// The request's text.
    pub text: String,
    /// Where it begins.
    pub at: usize,
}

/// \[EXTENSION\] GQL_TABLE ( `<graph name>` , `<character string
/// literal>` ) \[\[AS\] `<correlation name>` \[( `<derived column list>`
/// )\]\]: the table a GQL query of the graph returns, a `<table primary>`
/// (graph standard §9).
#[derive(Debug, Clone)]
pub struct GqlTable {
    /// The graph.
    pub graph: TableName,
    /// The query's text.
    pub text: String,
    /// The correlation name, if written.
    pub correlation: Option<String>,
    /// The derived column list, if written.
    pub columns: Option<Vec<String>>,
    /// Where it begins.
    pub at: usize,
}
