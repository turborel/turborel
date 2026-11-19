//! [ISO/IEC 9075-2:2023 §14.9 SR7a, §14.14 SR7a] A searched DELETE or
//! UPDATE FOR PORTION OF a period of its table, FROM FROMVAL TO TOVAL. Its
//! rows are those whose period overlaps the portion \[SR7a iv, SR7a v\],
//! an UPDATE sets the period to the part within it \[§14.14 SR7a vi\],
//! and each row's parts outside it are kept as rows of their own
//! [§15.8 GR9, §15.14 GR12], whose bounds two expressions over the old
//! row give (`db/write/portion.rs`). Each is written as SQL text would
//! write it and bound as the statement's own.

use super::expression::dynamic_of;
use super::statements::WriteKind;
use super::{Binder, Scope, rule};
use crate::ast::{CaseExpression, ColumnSpecification, Portion, Predicate, SearchCondition, SetClause, TableName, TypeName};
use crate::ast::{UpdateSource, ValueExpression};
use crate::catalog::TableDesc;
use crate::error::Result;
use crate::logical::{Expr, expr_columns};
use crate::value::{CompOp, DataType};

/// [ISO/IEC 9075-2:2023 §15.8 GR9, §15.14 GR12] What a statement FOR
/// PORTION OF a period keeps of each row it changes, over the old row.
#[derive(Debug, Clone)]
pub struct Pieces {
    /// The positions of the period's start and end columns.
    pub columns: [usize; 2],
    /// The end of the row's part before the portion and the start of its
    /// part after, each null if it has none.
    pub bounds: [Expr; 2],
}

/// A statement FOR PORTION OF a period: its search condition and SET
/// clauses with the period's, and its pieces unbound.
pub(super) struct Portioned {
    /// The `<set clause>`s, with SR7a vi's of an UPDATE.
    pub(super) set: Vec<SetClause>,
    /// SC: the `<search condition>` and the portion's \[SR7a iv, v\].
    pub(super) cond: SearchCondition,
    /// FROMVAL and TOVAL, as written, and where each Syntax Rule of them
    /// is.
    points: [(ValueExpression, &'static str); 2],
    /// The end of the part before the portion and the start of the part
    /// after it, each null if the row has none.
    pieces: [ValueExpression; 2],
    /// The positions of the period's start and end columns.
    columns: [usize; 2],
}

/// A comparison `a op b` at `at`.
fn compared(a: &ValueExpression, op: CompOp, b: &ValueExpression, at: usize) -> SearchCondition {
    SearchCondition::Predicate(Box::new(Predicate::Comparison(vec![a.clone()], op, vec![b.clone()])), at)
}

/// `a AND b`.
fn and(a: SearchCondition, b: SearchCondition) -> SearchCondition {
    SearchCondition::And(Box::new(a), Box::new(b))
}

/// CASE WHEN `when` THEN `then` \[ELSE `otherwise`\] END.
fn case(when: SearchCondition, then: ValueExpression, otherwise: Option<ValueExpression>) -> ValueExpression {
    ValueExpression::Case(Box::new(CaseExpression::Searched { whens: vec![(when, Some(then))], otherwise }))
}

/// Whether `e` invokes, but in a subquery, an SQL-invoked function that is
/// possibly non-deterministic or possibly modifies SQL-data: a worklist,
/// as `Expr::invokes_modifying` walks one.
fn unfit(e: &Expr) -> bool {
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        if let Expr::Invoked(i) = next
            && (!i.deterministic || i.modifies)
        {
            return true;
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    false
}

impl Portioned {
    /// The statement of `kind` on `table`, described by `desc`, FOR
    /// `portion`, of SET clauses `set` and search condition `cond`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the table has no such period \[SR7a\], or an
    /// UPDATE's SET clause names its start or end column [§14.14 SR7a ii].
    pub(super) fn of(
        (desc, table): (&TableDesc, &TableName),
        (kind, portion): (WriteKind, &Portion),
        set: &[SetClause],
        cond: Option<&SearchCondition>,
    ) -> Result<Portioned> {
        let update = kind == WriteKind::UpdateSearched;
        let clause = if update { "§14.14" } else { "§14.9" };
        let at = portion.at;
        let named = |n: &String| desc.periods.iter().find(|p| p.name == *n);
        let Some(period) = named(&portion.period) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 {clause} SR7a] {} has no period {}", desc.name, portion.period)).or_at(at));
        };
        if let Some(s) = set.iter().find(|s| s.column == period.start || s.column == period.end) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §14.14 SR7a ii] {} is a column of period {} and is set by FOR PORTION OF",
                s.column, period.name
            );
            return Err(rule(msg).or_at(s.at));
        }
        // [SR7a i] BCD, the start column's declared type.
        let (Some(s), Some(e)) = (desc.column(&period.start), desc.column(&period.end)) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR2e] period {} names no columns of {}", period.name, desc.name)));
        };
        let bcd = desc.columns.get(s).map_or(DataType::Date, |c| c.data_type.clone());
        let cast = |v: &ValueExpression| ValueExpression::Cast {
            operand: Some(Box::new(v.clone())),
            target: TypeName::Data(bcd.clone()),
            format: None,
            passing: None,
            at,
        };
        // \[DESIGN\] A `?` point in time is of BCD.
        let typed = |v: &ValueExpression| if dynamic_of(v).is_some() { cast(v) } else { v.clone() };
        let (from, to) = (typed(&portion.from), typed(&portion.to));
        let column = |name: &str| {
            ValueExpression::Column(ColumnSpecification { qualifier: Some(table.clone()), column: name.to_owned(), catalog: None, at })
        };
        let (start, end) = (column(&period.start), column(&period.end));
        // [SR7a iv, v] TSC AND (FROMVAL < TOVAL) AND (BENDCOL > FROMVAL) AND (BSTARTCOL < TOVAL).
        let own =
            and(and(compared(&from, CompOp::Lt, &to, at), compared(&end, CompOp::Gt, &from, at)), compared(&start, CompOp::Lt, &to, at));
        let cond = cond.map_or_else(|| own.clone(), |c| and(c.clone(), own.clone()));
        let mut set = set.to_vec();
        if update {
            // [§14.14 SR7a vi] the period clipped to the portion.
            let clipped = |(name, bound, op, point): (&str, &ValueExpression, CompOp, &ValueExpression)| SetClause {
                column: name.to_owned(),
                attributes: Vec::new(),
                element: None,
                value: UpdateSource::Value(case(compared(bound, op, point, at), bound.clone(), Some(cast(point)))),
                at,
            };
            set.push(clipped((&period.start, &start, CompOp::Gt, &from)));
            set.push(clipped((&period.end, &end, CompOp::Lt, &to)));
        }
        // [§15.8 GR9c, GR9d; §15.14 GR12c, GR12d] the part before FROMVAL
        // and the part after TOVAL, if the row has either.
        let within = |point: &ValueExpression| and(compared(&start, CompOp::Lt, point, at), compared(point, CompOp::Lt, &end, at));
        let pieces = [case(within(&from), cast(&from), None), case(within(&to), cast(&to), None)];
        let items = if update { ["§14.14 SR7a iii", "§14.14 SR7a iv"] } else { ["§14.9 SR7a ii", "§14.9 SR7a iii"] };
        Ok(Portioned { set, cond, points: [(from, items[0]), (to, items[1])], pieces, columns: [s, e] })
    }

    /// Its pieces bound by `binder`, where `scope` says, after its points
    /// in time are judged: the end of the part before the portion and the
    /// start of the part after it, over the old row.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a point in time is no datetime, references a column
    /// of the table, or invokes a function possibly non-deterministic or
    /// modifying SQL-data \[SR7a ii, SR7a iii\].
    pub(super) fn bind(&self, binder: &Binder<'_>, scope: Scope<'_>) -> Result<Pieces> {
        for (point, item) in &self.points {
            let (e, t) = binder.value_expression(point, scope)?;
            if !matches!(t, DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. }) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 {item}] a <point in time> is a datetime, not {t}")));
            }
            let mut columns = Vec::new();
            expr_columns(&e, &mut columns);
            if !columns.is_empty() {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 {item}] a <point in time> shall not reference a column of the table")));
            }
            if unfit(&e) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 {item}] a <point in time> shall not invoke a function that is possibly non-deterministic or modifies SQL-data"
                )));
            }
        }
        let [before, after] = &self.pieces;
        let bounds = [binder.value_expression(before, scope)?.0, binder.value_expression(after, scope)?.0];
        Ok(Pieces { columns: self.columns, bounds })
    }
}
