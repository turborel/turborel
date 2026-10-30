//! Abstract syntax of queries: `<cursor specification>` [X3.135-1986 §8.3],
//! `<query expression>` [ISO/IEC 9075:1992 §7.10] and `<query
//! specification>` [X3.135-1986 §5.25] and their parts. Split from ast.rs
//! (RULES.md R-SIZE-2).

use super::{ColumnSpecification, SearchCondition, Subquery, TableName, ValueExpression};
// ---------------------------------------------------------------------------
// [X3.135-1986 §8.3] <cursor specification> and [X3.135-1986 §5.25] <query specification>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] `<cursor specification>`
/// ::= `<query expression>` \[`<order by clause>`\] \[`<updatability
/// clause>`\]
#[derive(Debug, Clone)]
pub struct CursorSpecification {
    /// The `<query expression>`.
    pub query: QueryExpression,
    /// `<order by clause>` ::= ORDER BY `<sort specification>` \[{, `<sort specification>`}...\]
    pub order_by: Vec<SortSpecification>,
    /// The `<updatability clause>`, if written.
    pub updatability: Option<Updatability>,
}

/// [ISO/IEC 9075:1992 §13.1] `<updatability clause>` ::= FOR {READ ONLY |
/// UPDATE \[OF `<column name list>`\]}
#[derive(Debug, Clone)]
pub enum Updatability {
    /// FOR READ ONLY.
    ReadOnly,
    /// FOR UPDATE, with the columns of its `<column name list>`, each with
    /// where it begins, if one is written.
    Update(Option<Vec<(String, usize)>>),
}

/// [X3.135-1986 §8.3] `<sort specification>` ::= {`<unsigned integer>` | `<column specification>`} \[ASC | DESC\]
#[derive(Debug, Clone)]
pub struct SortSpecification {
    /// The column sorted on.
    pub key: SortColumn,
    /// DESC was written.
    pub descending: bool,
}

/// How a `<sort specification>` names its column.
#[derive(Debug, Clone)]
pub enum SortColumn {
    /// An `<unsigned integer>`: the column's position, from 1.
    Position(u32),
    /// A `<column specification>`.
    Column(ColumnSpecification),
}

/// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §7.10] `<query expression>`: a
/// `<simple table>`, or a set operation on two. Parentheses only group:
/// the operations are nested as they were written. TABLE `<table name>` is
/// (SELECT * FROM `<table name>`) \[SR2\], and a `<joined table>` SELECT *
/// FROM it \[SR13b\], so each is held as that `<query specification>`.
#[derive(Debug, Clone)]
pub enum QueryExpression {
    /// A `<query specification>`.
    Spec(Box<QuerySpecification>),
    /// [ISO/IEC 9075:1992 §7.2] `<table value constructor>` ::= VALUES
    /// `<row value constructor>` \[{, `<row value constructor>`}...\]
    Values(Vec<ValuesRow>),
    /// UNION, EXCEPT or INTERSECT.
    SetOp(Box<SetOperation>),
}

impl QueryExpression {
    /// Its `<query specification>`, if it is just one.
    pub fn spec(&self) -> Option<&QuerySpecification> {
        if let QueryExpression::Spec(s) = self { Some(s) } else { None }
    }

    /// Adds to `out` the `<table name>`s the `<from clause>`s of it and of
    /// its subqueries name, as written.
    pub fn table_names<'a>(&'a self, out: &mut Vec<&'a TableName>) {
        let mut subqueries = Vec::new();
        let spec = match self {
            QueryExpression::Spec(spec) => spec,
            QueryExpression::SetOp(s) => {
                s.left.table_names(out);
                s.right.table_names(out);
                return;
            }
            QueryExpression::Values(rows) => {
                for element in rows.iter().flat_map(|r| &r.elements) {
                    if let RowElement::Value(v, _) = element {
                        v.for_each_subquery(&mut |s| subqueries.push(s));
                    }
                }
                for s in subqueries {
                    s.query.table_names(out);
                }
                return;
            }
        };
        let te = &spec.table_expression;
        let mut refs: Vec<&TableReference> = te.from.iter().collect();
        while let Some(r) = refs.pop() {
            match r {
                TableReference::Table(t) => out.push(&t.table),
                TableReference::Derived(d) => subqueries.push(&*d.query),
                TableReference::Joined(j) => {
                    refs.extend([&j.left, &j.right]);
                    if let Some(JoinSpecification::On(c)) = &j.on {
                        c.for_each_subquery(&mut |s| subqueries.push(s));
                    }
                }
            }
        }
        for c in te.where_clause.iter().chain(&te.having) {
            c.for_each_subquery(&mut |s| subqueries.push(s));
        }
        if let SelectList::Values(items) = &spec.select_list {
            for item in items {
                if let SelectSublist::Derived(d) = item {
                    d.value.for_each_subquery(&mut |s| subqueries.push(s));
                }
            }
        }
        for s in subqueries {
            s.query.table_names(out);
        }
    }
}

/// [ISO/IEC 9075:1992 §7.1] A `<row value constructor>` of a `<table value
/// constructor>`: its elements, and where it begins.
#[derive(Debug, Clone)]
pub struct ValuesRow {
    /// The `<row value constructor element>`s.
    pub elements: Vec<RowElement>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §7.1] `<row value constructor element>` ::= `<value
/// expression>` | NULL | DEFAULT, and where it begins.
#[derive(Debug, Clone)]
pub enum RowElement {
    /// A `<value expression>`.
    Value(ValueExpression, usize),
    /// `<null specification>` ::= NULL.
    Null(usize),
    /// `<default specification>` ::= DEFAULT.
    Default(usize),
}

/// [ISO/IEC 9075:1992 §7.10] A set operation: `left` UNION, EXCEPT or
/// INTERSECT \[ALL\] \[`<corresponding spec>`\] `right`.
#[derive(Debug, Clone)]
pub struct SetOperation {
    /// The set operator.
    pub op: SetOperator,
    /// ALL was written.
    pub all: bool,
    /// How the operands' columns correspond.
    pub corresponding: Correspondence,
    /// The first operand.
    pub left: QueryExpression,
    /// The second operand.
    pub right: QueryExpression,
    /// Where the operator is written.
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §7.10] A set operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperator {
    /// UNION.
    Union,
    /// EXCEPT.
    Except,
    /// INTERSECT.
    Intersect,
}

/// [ISO/IEC 9075:1992 §7.10] How the columns of a set operation's operands
/// correspond.
#[derive(Debug, Clone)]
pub enum Correspondence {
    /// By position: no `<corresponding spec>` \[SR9b\].
    Position,
    /// CORRESPONDING: the columns both name \[SR9a iii 1\].
    Names,
    /// CORRESPONDING BY (`<corresponding column list>`) \[SR9a iii 2\].
    By(Vec<String>),
}

/// [X3.135-1986 §5.25; ISO/IEC 9075:1992 §7.9] `<query specification>` ::=
///     SELECT \[ALL | DISTINCT\] `<select list>` `<table expression>`
#[derive(Debug, Clone)]
pub struct QuerySpecification {
    /// DISTINCT was written.
    pub distinct: bool,
    /// The `<select list>`.
    pub select_list: SelectList,
    /// The `<table expression>`.
    pub table_expression: TableExpression,
}

/// [X3.135-1986 §5.25; ISO/IEC 9075:1992 §7.9] `<select list>` ::= * |
/// `<select sublist>` \[{, `<select sublist>`}...\]
#[derive(Debug, Clone)]
pub enum SelectList {
    /// `<select sublist>`s, in order.
    Values(Vec<SelectSublist>),
    /// *
    Star,
}

impl SelectList {
    /// Whether every item is a column: * or `<qualifier>`.*, or a `<column
    /// specification>` [X3.135-1986 §5.25 SR11b; ISO/IEC 9075:1992 §7.9
    /// SR12b].
    pub fn columns_only(&self) -> bool {
        match self {
            SelectList::Star => true,
            SelectList::Values(vs) => vs.iter().all(|v| match v {
                SelectSublist::Derived(item) => matches!(item.value, ValueExpression::Column(_)),
                SelectSublist::All(_) => true,
            }),
        }
    }
}

/// [ISO/IEC 9075:1992 §7.9] `<select sublist>` ::= `<derived column>` |
/// `<qualifier>`.*
#[derive(Debug, Clone)]
pub enum SelectSublist {
    /// A `<derived column>`.
    Derived(SelectItem),
    /// `<qualifier>`.*: the columns of the table it designates \[SR4\].
    All(TableName),
}

/// [ISO/IEC 9075:1992 §7.9] `<derived column>` ::= `<value expression>`
/// \[\[AS\] `<column name>`\], and where it begins: the position of an error
/// in it that has none finer.
#[derive(Debug, Clone)]
pub struct SelectItem {
    /// The `<value expression>`.
    pub value: ValueExpression,
    /// The `<column name>` of its `<as clause>`, if written \[SR9a\].
    pub alias: Option<String>,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [X3.135-1986 §5.19] `<table expression>` ::=
///     `<from clause>` \[`<where clause>`\] \[`<group by clause>`\] \[`<having clause>`\]
#[derive(Debug, Clone)]
pub struct TableExpression {
    /// [X3.135-1986 §5.20] `<from clause>` ::= FROM `<table reference>` \[{,`<table reference>`}...\]
    pub from: Vec<TableReference>,
    /// [X3.135-1986 §5.21] `<where clause>` ::= WHERE `<search condition>`
    pub where_clause: Option<SearchCondition>,
    /// [X3.135-1986 §5.22] `<group by clause>` ::= GROUP BY `<column specification>` \[{, `<column specification>`}...\]
    pub group_by: Vec<ColumnSpecification>,
    /// [X3.135-1986 §5.23] `<having clause>` ::= HAVING `<search condition>`
    pub having: Option<SearchCondition>,
}

/// [X3.135-1986 §5.20; ISO/IEC 9075:1992 §6.3] `<table reference>`.
#[derive(Debug, Clone)]
pub enum TableReference {
    /// `<table name>` \[\[AS\] `<correlation name>` \[(`<derived column
    /// list>`)\]\]
    Table(NamedTable),
    /// `<derived table>` \[AS\] `<correlation name>` \[(`<derived column
    /// list>`)\]
    Derived(DerivedTable),
    /// `<joined table>` [§7.5].
    Joined(Box<JoinedTable>),
}

impl TableReference {
    /// The table it names, if it names one.
    pub fn named(&self) -> Option<&NamedTable> {
        if let TableReference::Table(t) = self { Some(t) } else { None }
    }
}

/// A `<table reference>` naming a table or view.
#[derive(Debug, Clone)]
pub struct NamedTable {
    /// The `<table name>`.
    pub table: TableName,
    /// The `<correlation name>`, if written.
    pub correlation: Option<String>,
    /// The `<derived column list>`, if written [ISO/IEC 9075:1992 §6.3
    /// SR7].
    pub columns: Option<Vec<String>>,
}

/// [ISO/IEC 9075:1992 §6.3] A `<derived table>`, its `<table subquery>`
/// named.
#[derive(Debug, Clone)]
pub struct DerivedTable {
    /// The `<table subquery>`.
    pub query: Box<Subquery>,
    /// The `<correlation name>`.
    pub correlation: String,
    /// The `<derived column list>`, if written \[SR7\].
    pub columns: Option<Vec<String>>,
}

/// [ISO/IEC 9075:1992 §7.5] `<joined table>` ::= `<cross join>` |
/// `<qualified join>` | (`<joined table>`)
#[derive(Debug, Clone)]
pub struct JoinedTable {
    /// The first `<table reference>`.
    pub left: TableReference,
    /// The second `<table reference>`.
    pub right: TableReference,
    /// CROSS, or the `<join type>`: INNER if none is written \[SR3\].
    pub kind: JoinKind,
    /// NATURAL was written.
    pub natural: bool,
    /// The `<join specification>`, if written.
    pub on: Option<JoinSpecification>,
    /// Where its join is written in the statement: the first key word
    /// after `left`.
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §7.5] What a join is: CROSS, or a `<join type>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// CROSS JOIN.
    Cross,
    /// INNER.
    Inner,
    /// LEFT \[OUTER\].
    Left,
    /// RIGHT \[OUTER\].
    Right,
    /// FULL \[OUTER\].
    Full,
    /// UNION.
    Union,
}

/// [ISO/IEC 9075:1992 §7.5] `<join specification>` ::= `<join condition>`
/// | `<named columns join>`
#[derive(Debug, Clone)]
pub enum JoinSpecification {
    /// ON `<search condition>`.
    On(SearchCondition),
    /// USING (`<join column list>`).
    Using(Vec<String>),
}
