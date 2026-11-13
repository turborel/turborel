//! [ISO/IEC 9075-2:2023 §7.18 SR8] The expanded `<with list element>` a
//! `<with list element>` with a `<search or cycle clause>` is equivalent
//! to:
//!
//! ```text
//! WQN(EWCL) AS
//!   ( SELECT ETLOSL FROM (TLO) TLOCRN(WCL)
//!     OP
//!     SELECT ETROSL
//!     FROM (SELECT ETROSL1 TROTE) TROCRN(EWCL)
//!     WHERE NCCON )
//! ```
//!
//! Each column of the expansion's own select lists is qualified by the
//! correlation name of the one table it reads, so no name of the
//! statement's is mistaken for it. The level of BREADTH FIRST is read as
//! the first field of the row the sequence column holds, as SR5c ii's
//! LEVEL names it: turborel names the fields of `ROW(...)` by their
//! places (`binder/composite.rs`). A `*` of TRO, which would take the
//! columns the expansion adds to the element's, is each table's columns,
//! the element's those of its `<with column list>`.

use super::super::composite::anonymous;
use super::Parts;
use crate::ast::{
    CaseExpression, Composite, Correspondence, DerivedTable, JoinedTable, Literal, Predicate, QueryExpression, QuerySpecification,
    SearchCondition, SearchOrder, SelectItem, SelectList, SelectSublist, SetOperation, SetOperator, Subquery, TableExpression, TableName,
    TableReference, TypeName, UnnestTable, ValueExpression, ValueSpecification, WithElement,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::CONCATENATION;
use crate::value::{ArithOp, CompOp, DataType, Decimal, INTEGER_PRECISION, Spelled};

/// [§7.18 SR8] TLOCRN, the correlation name of the first operand.
const ANCHOR: &str = "TLOCRN";
/// [§7.18 SR8] TROCRN, the correlation name of the second operand.
const STEP: &str = "TROCRN";
/// [§7.18 SR6d] P, the correlation name of the rows of the path column.
const PATH_ROWS: &str = "P";

/// [ISO/IEC 9075-2:2023 §7.18 SR8] The expanded element of `p`, and the
/// places of its columns that are arrays of the greatest maximum
/// cardinality (RULES.md D70).
///
/// # Errors
///
/// SQLCODE -900 if TRO's select list is `*` of a table that a NATURAL join
/// or a join USING columns reads.
pub(super) fn expanded(p: &Parts<'_>) -> Result<(WithElement, Vec<usize>)> {
    let e = p.element;
    let mut columns = p.columns.to_vec();
    columns.extend(added(p));
    let at = e.search.as_ref().map_or_else(|| e.cycle.as_ref().map_or(e.at, |c| c.at), |s| s.at);
    let x = Expansion { p, at };
    let union = SetOperation {
        op: SetOperator::Union,
        all: p.union.all,
        corresponding: Correspondence::Position,
        left: spec(x.anchor_list(), x.derived(p.union.left.clone(), ANCHOR, p.columns.to_vec()), None),
        right: spec(x.step_list(), x.derived(x.inner()?, STEP, columns.clone()), x.not_cycled()),
        at: p.union.at,
    };
    let element = WithElement {
        name: e.name.clone(),
        columns: Some(columns),
        query: QueryExpression::SetOp(Box::new(union)),
        search: None,
        cycle: None,
        at: e.at,
    };
    Ok((element, widened(p)))
}

/// The columns the expansion adds to the `<with column list>` of `p`: the
/// sequence column, then the cycle mark column and the path column \[SR7\].
fn added(p: &Parts<'_>) -> Vec<String> {
    let e = p.element;
    let search = e.search.iter().map(|s| s.sequence.clone());
    let cycle = e.cycle.iter().flat_map(|c| [c.mark.clone(), c.path.clone()]);
    search.chain(cycle).collect()
}

/// [RULES.md D70] The places among the expansion's columns of the sequence
/// column of DEPTH FIRST and of the path column, arrays that grow.
fn widened(p: &Parts<'_>) -> Vec<usize> {
    let e = p.element;
    let after = p.columns.len();
    let depth = e.search.as_ref().filter(|s| s.order == SearchOrder::Depth).map(|_| after);
    let path = e.cycle.as_ref().map(|_| after.saturating_add(usize::from(e.search.is_some())).saturating_add(1));
    depth.into_iter().chain(path).collect()
}

/// The `<query specification>` SELECT `list` FROM `from` \[WHERE `condition`\].
fn spec(list: Vec<SelectSublist>, from: TableReference, condition: Option<SearchCondition>) -> QueryExpression {
    QueryExpression::Spec(Box::new(specification(list, from, condition)))
}

/// `spec`'s `<query specification>` itself.
fn specification(list: Vec<SelectSublist>, from: TableReference, condition: Option<SearchCondition>) -> QuerySpecification {
    let table_expression = TableExpression { from: vec![from], where_clause: condition, group_by: None, having: None };
    QuerySpecification { distinct: false, select_list: SelectList::Values(list), table_expression }
}

/// The expansion of a `<with list element>`, written at `at`.
struct Expansion<'p, 'e> {
    /// What it expands.
    p: &'p Parts<'e>,
    /// Where its clause is written.
    at: usize,
}

impl Expansion<'_, '_> {
    /// The column `column` of the table `table` names.
    fn column(&self, table: &str, column: &str) -> ValueExpression {
        let qualifier = TableName { auth: None, ident: table.to_owned(), at: self.at };
        ValueExpression::Column(crate::ast::ColumnSpecification {
            qualifier: Some(qualifier),
            column: column.to_owned(),
            catalog: None,
            at: self.at,
        })
    }

    /// The columns `columns` of the table `table` names.
    fn columns(&self, table: &str, columns: &[String]) -> Vec<ValueExpression> {
        columns.iter().map(|c| self.column(table, c)).collect()
    }

    /// The `<derived column>` `value`, named `name`.
    fn item(&self, value: ValueExpression, name: &str) -> SelectSublist {
        SelectSublist::Derived(SelectItem { value, alias: Some(name.to_owned()), at: self.at })
    }

    /// The `<derived table>` (`query`) `correlation`(`columns`).
    fn derived(&self, query: QueryExpression, correlation: &str, columns: Vec<String>) -> TableReference {
        let query = Box::new(Subquery { query, at: self.at });
        TableReference::Derived(DerivedTable { query, correlation: correlation.to_owned(), columns: Some(columns) })
    }

    /// The composite expression `c`.
    fn composite(&self, c: Composite) -> ValueExpression {
        ValueExpression::Composite(Box::new(c), self.at)
    }

    /// ARRAY [ROW(`columns`)] of the table `table`.
    fn path_of(&self, table: &str, columns: &[String]) -> ValueExpression {
        let row = self.composite(Composite::Row(self.columns(table, columns)));
        self.composite(Composite::Array(vec![row]))
    }

    /// `left` || `right`.
    fn concatenated(&self, left: ValueExpression, right: ValueExpression) -> ValueExpression {
        ValueExpression::Function { name: CONCATENATION, args: vec![left, right], at: self.at }
    }

    /// ETLOSL: WCL, SCIN, CCIN, of TLOCRN.
    fn anchor_list(&self) -> Vec<SelectSublist> {
        let p = self.p;
        let mut list: Vec<SelectSublist> = p.columns.iter().map(|c| self.item(self.column(ANCHOR, c), c)).collect();
        if let Some(s) = &p.element.search {
            let start = match s.order {
                SearchOrder::Depth => self.path_of(ANCHOR, &s.columns),
                SearchOrder::Breadth => {
                    let mut fields = vec![integer_zero(self.at)];
                    fields.extend(self.columns(ANCHOR, &s.columns));
                    self.composite(Composite::Row(fields))
                }
            };
            list.push(self.item(start, &s.sequence));
        }
        if let (Some(c), Some((_, unmarked))) = (&p.element.cycle, &p.marks) {
            list.push(self.item(unmarked.clone(), &c.mark));
            list.push(self.item(self.path_of(ANCHOR, &c.columns), &c.path));
        }
        list
    }

    /// ETROSL: WCL, SCEX2, CCEX2, of TROCRN.
    fn step_list(&self) -> Vec<SelectSublist> {
        let p = self.p;
        let mut list: Vec<SelectSublist> = p.columns.iter().map(|c| self.item(self.column(STEP, c), c)).collect();
        if let Some(s) = &p.element.search {
            let sequence = self.column(STEP, &s.sequence);
            let next = match s.order {
                SearchOrder::Depth => self.concatenated(sequence, self.path_of(STEP, &s.columns)),
                SearchOrder::Breadth => {
                    let level = self.composite(Composite::Field(Box::new(sequence), anonymous(0)));
                    let one = ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(1, 0))));
                    let mut fields = vec![ValueExpression::Dyadic(Box::new(level), ArithOp::Add, Box::new(one))];
                    fields.extend(self.columns(STEP, &s.columns));
                    self.composite(Composite::Row(fields))
                }
            };
            list.push(self.item(next, &s.sequence));
        }
        if let (Some(c), Some((marked, unmarked))) = (&p.element.cycle, &p.marks) {
            let path = self.column(STEP, &c.path);
            let seen = SearchCondition::Predicate(Box::new(self.in_path(&c.columns, path.clone())), self.at);
            let mark = CaseExpression::Searched { whens: vec![(seen, Some(marked.clone()))], otherwise: Some(unmarked.clone()) };
            list.push(self.item(ValueExpression::Case(Box::new(mark)), &c.mark));
            list.push(self.item(self.concatenated(path, self.path_of(STEP, &c.columns)), &c.path));
        }
        list
    }

    /// [SR6d] ROW(CCL) IN (SELECT P.* FROM UNNEST(`path`) P), the cycle
    /// columns `columns` of TROCRN.
    fn in_path(&self, columns: &[String], path: ValueExpression) -> Predicate {
        let rows =
            UnnestTable { collections: vec![path], ordinality: false, correlation: PATH_ROWS.to_owned(), columns: None, at: self.at };
        let all = SelectSublist::All(TableName { auth: None, ident: PATH_ROWS.to_owned(), at: self.at });
        let query = QueryExpression::Spec(Box::new(specification(vec![all], TableReference::Unnest(rows), None)));
        let subquery = Box::new(Subquery { query, at: self.at });
        Predicate::InSubquery { value: self.columns(STEP, columns), subquery, not: false }
    }

    /// NCCON: TROCRN.CMC <> CMV of a `<cycle clause>`, or none.
    fn not_cycled(&self) -> Option<SearchCondition> {
        let (c, (marked, _)) = (self.p.element.cycle.as_ref()?, self.p.marks.as_ref()?);
        let compared = Predicate::Comparison(vec![self.column(STEP, &c.mark)], CompOp::Ne, vec![marked.clone()]);
        Some(SearchCondition::Predicate(Box::new(compared), self.at))
    }

    /// SELECT ETROSL1 TROTE: TRO with SCEX1 and CCEX1, of WQNCRN, after its
    /// select list, and the columns the expansion adds after the `<derived
    /// column list>` of WQNTR, if it has one.
    fn inner(&self) -> Result<QueryExpression> {
        let p = self.p;
        let crn = p.correlation();
        let mut list = self.step_items()?;
        list.extend(added(p).iter().map(|c| self.item(self.column(crn, c), c)));
        let mut table_expression = p.step.table_expression.clone();
        extend_reference(&mut table_expression.from, &p.element.name, &added(p));
        let spec = QuerySpecification { distinct: p.step.distinct, select_list: SelectList::Values(list), table_expression };
        Ok(QueryExpression::Spec(Box::new(spec)))
    }

    /// TROSL, each `<qualifier>`.* of WQNCRN, and a `*`, written as the
    /// columns it took before the expansion.
    fn step_items(&self) -> Result<Vec<SelectSublist>> {
        let p = self.p;
        let crn = p.correlation();
        let own: Vec<String> = p.reference.columns.clone().unwrap_or_else(|| p.columns.to_vec());
        let qualifiers = match &p.step.select_list {
            SelectList::Values(items) => items.clone(),
            SelectList::Star => every_table(&p.step.table_expression.from, self.at)?,
        };
        let mut list = Vec::new();
        for item in qualifiers {
            if let SelectSublist::All(q) = &item
                && q.auth.is_none()
                && q.ident == crn
            {
                list.extend(own.iter().map(|c| self.item(self.column(crn, c), c)));
            } else {
                list.push(item);
            }
        }
        Ok(list)
    }
}

/// CAST (0 AS INTEGER), the first level of BREADTH FIRST (RULES.md D70).
fn integer_zero(at: usize) -> ValueExpression {
    let zero = ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(0, 0))));
    let integer = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };
    ValueExpression::Cast { operand: Some(Box::new(zero)), target: TypeName::Data(integer), format: None, at }
}

/// [ISO/IEC 9075:1992 §7.9 SR3] What `*` of a FROM clause `from` is: each
/// table's columns in turn, `<qualifier>`.* of each.
///
/// # Errors
///
/// SQLCODE -900 if a NATURAL join or a join USING columns is among them,
/// which gives its common columns once.
fn every_table(from: &[TableReference], at: usize) -> Result<Vec<SelectSublist>> {
    let mut list = Vec::new();
    let mut todo: Vec<&TableReference> = from.iter().rev().collect();
    while let Some(r) = todo.pop() {
        let correlation = match r {
            TableReference::Table(t) => t.correlation.clone().map_or_else(|| t.table.clone(), |c| TableName { auth: None, ident: c, at }),
            TableReference::Derived(d) => TableName { auth: None, ident: d.correlation.clone(), at },
            TableReference::Unnest(u) => TableName { auth: None, ident: u.correlation.clone(), at },
            TableReference::Joined(j) => {
                if coalesces(j) {
                    let msg = "a <search or cycle clause> of a recursive query whose second operand selects * of a NATURAL join or a join USING columns is not supported";
                    return Err(SqlError::new(Code::Unsupported, msg));
                }
                todo.extend([&j.right, &j.left]);
                continue;
            }
        };
        list.push(SelectSublist::All(correlation));
    }
    Ok(list)
}

/// Whether the join `j` gives common columns once: NATURAL, or USING.
fn coalesces(j: &JoinedTable) -> bool {
    j.natural || matches!(j.on, Some(crate::ast::JoinSpecification::Using(_)))
}

/// The `<derived column list>` of the `<table reference>` of `from` that is
/// the query name `name`, if it has one, with `added` after it: the
/// columns the expansion adds to its element's.
fn extend_reference(from: &mut [TableReference], name: &str, added: &[String]) {
    let mut todo: Vec<&mut TableReference> = from.iter_mut().collect();
    while let Some(r) = todo.pop() {
        match r {
            TableReference::Table(t) => {
                if t.table.auth.is_none()
                    && t.table.ident == name
                    && let Some(columns) = &mut t.columns
                {
                    columns.extend(added.iter().cloned());
                }
            }
            TableReference::Joined(j) => {
                let JoinedTable { left, right, .. } = &mut **j;
                todo.extend([left, right]);
            }
            TableReference::Derived(_) | TableReference::Unnest(_) => {}
        }
    }
}
