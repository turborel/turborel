//! Abstract syntax of queries: `<cursor specification>` [X3.135-1986 §8.3],
//! `<query expression>` [ISO/IEC 9075:1992 §7.10] and `<query
//! specification>` [X3.135-1986 §5.25] and their parts.

use super::{ColumnSpecification, CycleClause, SearchClause, SearchCondition, Subquery, TableName, ValueExpression, WindowDefinition};
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

/// [X3.135-1986 §8.3] `<sort specification>` ::= {`<unsigned integer>` | `<column specification>`} \[ASC | DESC\];
/// [ISO/IEC 9075-2:2023 §10.10] its `<sort key>` a `<value expression>`.
#[derive(Debug, Clone)]
pub struct SortSpecification {
    /// What it sorts on.
    pub key: SortColumn,
    /// The `<collation name>` of its `<collate clause>`, if written.
    pub collate: Option<TableName>,
    /// DESC was written.
    pub descending: bool,
    /// [ISO/IEC 9075-2:2023 §10.10] Its `<null ordering>`, if written:
    /// whether NULLS FIRST.
    pub nulls_first: Option<bool>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §7.7] `<grouping column reference>` ::= `<column
/// reference>` \[`<collate clause>`\]
#[derive(Debug, Clone)]
pub struct GroupingColumn {
    /// The `<column reference>`.
    pub column: ColumnSpecification,
    /// The `<collation name>` of its `<collate clause>`, if written.
    pub collate: Option<TableName>,
}

/// [ISO/IEC 9075-2:2023 §7.13] `<group by clause>` ::= GROUP BY \[`<set
/// quantifier>`\] `<grouping element list>`
#[derive(Debug, Clone)]
pub struct GroupBy {
    /// DISTINCT was written; ALL, or neither, is not \[SR6\].
    pub distinct: bool,
    /// The `<grouping element list>`.
    pub elements: Vec<GroupingElement>,
}

/// [ISO/IEC 9075-2:2023 §7.13] A `<grouping element>`, or a `<grouping
/// set>`, of the same forms.
#[derive(Debug, Clone)]
pub enum GroupingElement {
    /// An `<ordinary grouping set>`: a `<grouping column reference>`, or a
    /// `<grouping column reference list>` in parentheses.
    Ordinary(Vec<GroupingColumn>),
    /// A `<rollup list>`: ROLLUP of `<ordinary grouping set>`s.
    Rollup(Vec<Vec<GroupingColumn>>),
    /// A `<cube list>`: CUBE of `<ordinary grouping set>`s.
    Cube(Vec<Vec<GroupingColumn>>),
    /// A `<grouping sets specification>`: GROUPING SETS of `<grouping set>`s.
    Sets(Vec<GroupingElement>),
    /// The `<empty grouping set>`, ().
    Empty,
}

/// What a `<sort specification>` sorts on.
#[derive(Debug, Clone)]
pub enum SortColumn {
    /// An `<unsigned integer>`: the column's position, from 1.
    Position(u32),
    /// A `<column specification>`.
    Column(ColumnSpecification),
    /// [ISO/IEC 9075-2:2023 §10.10] Any other `<sort key>`, a `<value
    /// expression>`.
    Value(ValueExpression),
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
    /// [ISO/IEC 9075-2:2023 §7.17] A `<with clause>` and the `<query
    /// expression body>` its query names are in scope in.
    With(Box<WithQuery>),
}

/// [ISO/IEC 9075-2:2023 §7.17] `<with clause>` ::= WITH \[RECURSIVE\]
/// `<with list>`, and the query it is written before.
#[derive(Debug, Clone)]
pub struct WithQuery {
    /// RECURSIVE was written: the `<with list>` is potentially recursive
    /// \[SR3e\].
    pub recursive: bool,
    /// The `<with list element>`s, in order.
    pub elements: Vec<WithElement>,
    /// The `<query expression body>`.
    pub body: QueryExpression,
    /// Where WITH is written.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.17] `<with list element>` ::= `<query name>`
/// \[(`<with column list>`)\] AS `<table subquery>` \[`<search or cycle
/// clause>`\]
#[derive(Debug, Clone)]
pub struct WithElement {
    /// The `<query name>`.
    pub name: String,
    /// The `<with column list>`, if written.
    pub columns: Option<Vec<String>>,
    /// The `<query expression>` of its `<table subquery>`.
    pub query: QueryExpression,
    /// [§7.18] The `<search clause>` of its `<search or cycle clause>`, if
    /// written.
    pub search: Option<SearchClause>,
    /// [§7.18] The `<cycle clause>` of its `<search or cycle clause>`, if
    /// written.
    pub cycle: Option<CycleClause>,
    /// Where its `<query name>` is written.
    pub at: usize,
}

impl QueryExpression {
    /// Its `<query specification>`, if it is just one.
    pub fn spec(&self) -> Option<&QuerySpecification> {
        if let QueryExpression::Spec(s) = self { Some(s) } else { None }
    }

    /// Its `<query expression body>`: itself, or that after its `<with
    /// clause>`s [ISO/IEC 9075-2:2023 §7.17].
    pub fn body(&self) -> &QueryExpression {
        let mut q = self;
        while let QueryExpression::With(w) = q {
            q = &w.body;
        }
        q
    }

    /// Adds to `out` the `<table name>`s the `<from clause>`s of it and of
    /// its subqueries name, as written: not a `<query name>` in scope
    /// there, which names no table [ISO/IEC 9075-2:2023 §7.17 SR3i].
    pub fn table_names<'a>(&'a self, out: &mut Vec<&'a TableName>) {
        self.for_each_named(&mut |t, query| {
            if !query {
                out.push(&t.table);
            }
        });
    }

    /// Calls `each` with every `<table reference>` that names a table or a
    /// query, in the `<from clause>`s of it and of its subqueries, and
    /// whether a `<with clause>` within it has a query of that name in
    /// scope there [ISO/IEC 9075-2:2023 §7.17 SR3i]: one named with no
    /// schema.
    pub fn for_each_named<'a>(&'a self, each: &mut dyn FnMut(&'a NamedTable, bool)) {
        self.named_in(&mut Vec::new(), each);
    }

    /// `for_each_named`, the query names of `scope` in scope.
    pub fn named_in<'a>(&'a self, scope: &mut Vec<&'a str>, each: &mut dyn FnMut(&'a NamedTable, bool)) {
        let mut subqueries = Vec::new();
        let spec = match self {
            QueryExpression::Spec(spec) => spec,
            QueryExpression::SetOp(s) => {
                s.left.named_in(scope, each);
                s.right.named_in(scope, each);
                return;
            }
            QueryExpression::With(w) => return w.named_in(scope, each),
            QueryExpression::Values(rows) => {
                for element in rows.iter().flat_map(|r| &r.elements) {
                    if let RowElement::Value(v, _) = element {
                        v.for_each_subquery(&mut |s| subqueries.push(s));
                    }
                }
                for s in subqueries {
                    s.query.named_in(scope, each);
                }
                return;
            }
        };
        let te = &spec.table_expression;
        named_in_from(&te.from, scope, each, &mut subqueries);
        for c in te.where_clause.iter().chain(&te.having) {
            c.for_each_subquery(&mut |s| subqueries.push(s));
        }
        if let SelectList::Values(items) = &spec.select_list {
            for item in items {
                match item {
                    SelectSublist::Derived(SelectItem { value, .. }) | SelectSublist::Fields { value, .. } => {
                        value.for_each_subquery(&mut |s| subqueries.push(s));
                    }
                    SelectSublist::All(_) => {}
                }
            }
        }
        for s in subqueries {
            s.query.named_in(scope, each);
        }
    }
}

/// The tables of `from` [ISO/IEC 9075-2:2023 §7.5], each to `each` with
/// whether a query name of `scope` names it, and its subqueries to
/// `subqueries`.
fn named_in_from<'a>(
    from: &'a [TableReference],
    scope: &[&'a str],
    each: &mut dyn FnMut(&'a NamedTable, bool),
    subqueries: &mut Vec<&'a Subquery>,
) {
    let mut refs: Vec<&TableReference> = from.iter().rev().collect();
    while let Some(r) = refs.pop() {
        match r {
            TableReference::Table(t) => each(t, t.table.auth.is_none() && scope.contains(&t.table.ident.as_str())),
            TableReference::Derived(d) => subqueries.push(&*d.query),
            TableReference::Unnest(u) => u.collections.iter().for_each(|v| v.for_each_subquery(&mut |s| subqueries.push(s))),
            TableReference::Joined(j) => {
                refs.extend([&j.right, &j.left]);
                if let Some(JoinSpecification::On(c)) = &j.on {
                    c.for_each_subquery(&mut |s| subqueries.push(s));
                }
            }
        }
    }
}

impl WithQuery {
    /// `QueryExpression::named_in` of its elements and body, each with the
    /// query names in scope there besides `scope`'s [ISO/IEC 9075-2:2023
    /// §7.17 SR3i]: of a non-recursive list, an element's the earlier
    /// elements', of a potentially recursive one, every element's.
    fn named_in<'a>(&'a self, scope: &mut Vec<&'a str>, each: &mut dyn FnMut(&'a NamedTable, bool)) {
        let base = scope.len();
        if self.recursive {
            scope.extend(self.elements.iter().map(|e| e.name.as_str()));
        }
        for e in &self.elements {
            e.query.named_in(scope, each);
            if !self.recursive {
                scope.push(&e.name);
            }
        }
        self.body.named_in(scope, each);
        scope.truncate(base);
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
                SelectSublist::Fields { .. } => false,
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
    /// `<qualifier>`.*: the columns of the table it designates \[SR4\];
    /// or [ISO/IEC 9075-2:2023 §7.16] the fields of the column of row type
    /// it names, an `<all fields reference>`.
    All(TableName),
    /// [ISO/IEC 9075-2:2023 §7.16] `<all fields reference>` ::= `<value
    /// expression primary>` . * \[AS ( `<all fields column name list>` )\].
    Fields {
        /// The `<value expression primary>`, of a row type.
        value: ValueExpression,
        /// The `<all fields column name list>`, if written [Feature T053].
        names: Option<Vec<String>>,
        /// Where it begins in the statement.
        at: usize,
    },
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
    /// [X3.135-1986 §5.22; ISO/IEC 9075-2:2023 §7.13] `<group by clause>`,
    /// if written.
    pub group_by: Option<GroupBy>,
    /// [X3.135-1986 §5.23] `<having clause>` ::= HAVING `<search condition>`
    pub having: Option<SearchCondition>,
    /// [ISO/IEC 9075-2:2023 §7.15] The `<window definition>`s of its `<window
    /// clause>`, none if it has none.
    pub window: Vec<WindowDefinition>,
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
    /// [ISO/IEC 9075-2:2023 §7.6] `<collection derived table>`
    /// `<correlation or recognition>`.
    Unnest(UnnestTable),
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

/// [ISO/IEC 9075-2:2023 §7.6] `<collection derived table>` ::= UNNEST (
/// `<collection value expression>` \[, ...\] ) \[WITH ORDINALITY\] \[AS\]
/// `<correlation name>` \[(`<derived column list>`)\].
#[derive(Debug, Clone)]
pub struct UnnestTable {
    /// The `<collection value expression>`s, in order.
    pub collections: Vec<ValueExpression>,
    /// Whether WITH ORDINALITY was written.
    pub ordinality: bool,
    /// The `<correlation name>`.
    pub correlation: String,
    /// The `<derived column list>`, if written \[SR8m i\].
    pub columns: Option<Vec<String>>,
    /// Where it begins in the statement.
    pub at: usize,
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
    /// The `<partitioned join column reference list>` of each operand, the
    /// first's then the second's, if it is a `<partitioned join table>`
    /// [ISO/IEC 9075-2:2023 §7.10].
    pub partitioned: [Option<Partitioning>; 2],
}

/// [ISO/IEC 9075-2:2023 §7.10] PARTITION BY `<partitioned join column
/// reference list>`: the `<column reference>`s an operand of a join is
/// partitioned by, and where PARTITION is written.
#[derive(Debug, Clone)]
pub struct Partitioning {
    /// The `<partitioned join column reference>`s, in order.
    pub columns: Vec<ColumnSpecification>,
    /// Where PARTITION begins in the statement.
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
