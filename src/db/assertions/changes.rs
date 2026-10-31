//! \[EXTENSION\] An assertion checked from the rows a statement changed
//! [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] (RULES.md D51), where its form
//! makes that the same as checking its whole condition; else whole.
//!
//! An immediate assertion is checked at the end of each statement that
//! changes a table it reads [§4.10.1], so it was satisfied as the statement
//! began: the statement before it was checked, or the assertion was
//! checked whole as it was made or made immediate, and a statement that
//! breaks it is undone. Two forms of a conjunct of its condition are
//! checked from the change alone, each over one table T, its rows filtered
//! by conditions of the row alone (`row_cond`): columns, constants,
//! arithmetic, comparisons, IS NULL, LIKE and CASE, no function, CAST,
//! subquery or outer reference. Such a condition of a row gives the same
//! truth, or the same error, every time it is evaluated on it, and raises
//! no warning; it depends on nothing but the row, so not on the time, the
//! session, nor a function a plug-in may replace.
//!
//! - `NOT EXISTS (SELECT ... FROM T WHERE p)` is false exactly when some
//!   row of T has `p` true. As the statement began none had, and each row
//!   gave `p` without an error: it was evaluated as the row was written, or
//!   as the assertion was checked whole. A row the statement did not change
//!   gives what it gave; one it deleted is gone. So the conjunct is false
//!   after it exactly when a row it inserted or updated has `p` true, and
//!   those rows alone are evaluated. Rows are independent of one another,
//!   so what other transactions write cannot combine with them: each
//!   transaction's rows are checked as it writes them, as a table's CHECK
//!   is.
//! - `(SELECT COUNT(*) FROM T WHERE p) op k`, or `k op (...)`, of a
//!   constant `k`: the count after is the count before plus the rows of
//!   the change that have `p` true now less those that had it before,
//!   each row as `p` gives it alone. A count no greater satisfies `<` and
//!   `<=` if the count before did, one no smaller `>` and `>=`, one equal
//!   any of them; a change that moves the count the other way is checked
//!   whole. The count before is the one this transaction sees, so this
//!   holds only for one that reads at one snapshot, its own: one reading at
//!   a snapshot of each statement checks whole. Whatever others commit
//!   meanwhile, a transaction whose change cannot move the count the wrong
//!   way leaves any state that satisfies it satisfying it once it commits;
//!   one whose change can reads T whole, its reads checked at commit (RULES.md
//!   D29).
//!
//! The conjuncts of an AND are checked each: the condition is false when
//! one of them is. A deferred assertion, or any other form, is checked
//! whole.

use std::sync::Arc;

use super::super::State;
use super::super::integrity::Effects;
use crate::catalog::{ConstraintKind, QualifiedName};
use crate::error::Result;
use crate::exec::{Heard, eval_cond};
use crate::functions::Registry;
use crate::logical::{Cond, Expr, LogicalPlan, Subquery, Truth, split_and};
use crate::storage::Row;
use crate::txn::Txn;
use crate::value::CompOp;

/// A conjunct of an assertion's condition that a statement's change
/// checks, of the rows of `table` that all of `conds` keep.
struct Form {
    table: QualifiedName,
    conds: Vec<Cond>,
    kind: Kind,
}

/// What the conjunct says of those rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// There is none.
    NoRow,
    /// Their count compares so with a constant: a change of `Safe`'s
    /// direction cannot make it false.
    Count(Safe),
}

/// The changes of a count that cannot make its comparison false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Safe {
    /// None that adds to it: of `<` and `<=`.
    Fewer,
    /// None that takes from it: of `>` and `>=`.
    More,
    /// None that changes it: of `=` and `<>`.
    Same,
}

/// What a change shows of an assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// It is not false.
    Holds,
    /// It is false.
    Broken,
    /// Only checking it whole tells.
    Unknown,
}

impl State {
    /// [ISO/IEC 9075:1992 §4.10.1, §11.34 GR2] That assertion `name`,
    /// immediate, so satisfied as the statement that did `effects` began,
    /// is not false after it, as transaction `tx` sees what it reads: from
    /// the rows the statement changed, where its form allows, else whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the assertion, if it is false.
    pub(in crate::db) fn check_assertion_after(self: &Arc<Self>, tx: &Txn, name: &QualifiedName, effects: &mut Effects<'_>) -> Result<()> {
        let Some(a) = self.catalog.assertion(name) else { return Ok(()) };
        let ConstraintKind::Check { reads, .. } = &a.kind else { return Ok(()) };
        for t in reads {
            self.read_lock(tx, t)?;
        }
        let plan = self.assertion_plan(a)?;
        let verdict = match forms_of(&plan, &self.functions) {
            Some(forms) => self.verdict(tx, &forms, effects)?,
            None => Verdict::Unknown,
        };
        match verdict {
            Verdict::Holds => Ok(()),
            Verdict::Broken => Err(super::broken(name)),
            Verdict::Unknown => self.assertion_holds(tx, name, plan),
        }
    }

    /// What the change `effects` shows of an assertion whose condition is
    /// the conjunction of `forms`.
    fn verdict(&self, tx: &Txn, forms: &[Form], effects: &mut Effects<'_>) -> Result<Verdict> {
        let mut verdict = Verdict::Holds;
        for form in forms {
            match self.form_verdict(tx, form, effects)? {
                Verdict::Holds => {}
                Verdict::Broken => return Ok(Verdict::Broken),
                Verdict::Unknown => verdict = Verdict::Unknown,
            }
        }
        Ok(verdict)
    }

    /// What the change `effects` shows of `form`.
    fn form_verdict(&self, tx: &Txn, form: &Form, effects: &mut Effects<'_>) -> Result<Verdict> {
        let safe = match form.kind {
            Kind::NoRow => None,
            Kind::Count(safe) => Some(safe),
        };
        // The count before is the one of this transaction's snapshot.
        if safe.is_some() && !tx.one_snapshot() {
            return Ok(Verdict::Unknown);
        }
        let storage = self.storage_of(&form.table)?;
        let mut change: i64 = 0;
        let mut broken = false;
        effects.visit_changed(&form.table, &mut |id, before| {
            if broken {
                return Ok(());
            }
            let now = storage.get(tx.id, id)?;
            let kept = now.as_ref().map(|row| keeps(&form.conds, row, tx)).transpose()?.unwrap_or(false);
            if safe.is_none() {
                broken = kept;
                return Ok(());
            }
            let had = before.map(|row| keeps(&form.conds, row, tx)).transpose()?.unwrap_or(false);
            change = change.saturating_add(i64::from(kept)).saturating_sub(i64::from(had));
            Ok(())
        })?;
        if broken {
            return Ok(Verdict::Broken);
        }
        let harmless = match safe {
            None => true,
            Some(Safe::Fewer) => change <= 0,
            Some(Safe::More) => change >= 0,
            Some(Safe::Same) => change == 0,
        };
        Ok(if harmless { Verdict::Holds } else { Verdict::Unknown })
    }
}

/// Whether every condition of `conds` is true of `row`.
fn keeps(conds: &[Cond], row: &Row, tx: &Txn) -> Result<bool> {
    for c in conds {
        if eval_cond(c, row, &Heard(tx.stop.warnings()))? != Truth::True {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The forms of the conjuncts of the condition of `plan`, an assertion's
/// bound query, `SELECT 1 FROM (VALUES (1)) WHERE NOT (condition)`, if
/// each has one; `functions` the database's.
fn forms_of(plan: &LogicalPlan, functions: &Registry) -> Option<Vec<Form>> {
    let LogicalPlan::Project { input, .. } = plan else { return None };
    let LogicalPlan::Filter { cond: Cond::Not(condition), input: values } = &**input else { return None };
    if !matches!(**values, LogicalPlan::Values { .. }) {
        return None;
    }
    let mut conjuncts = Vec::new();
    split_and((**condition).clone(), &mut conjuncts);
    conjuncts.iter().map(|c| form_of(c, functions)).collect()
}

/// The form of the conjunct `c`, if it has one.
fn form_of(c: &Cond, functions: &Registry) -> Option<Form> {
    match c {
        Cond::Not(inner) => {
            let Cond::Exists(s) = &**inner else { return None };
            exists_form(s)
        }
        Cond::Compare(Expr::Subquery(s, 0), op, Expr::Const(_)) => count_form(s, *op, functions),
        Cond::Compare(Expr::Const(_), op, Expr::Subquery(s, 0)) => count_form(s, op.flip(), functions),
        Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Is(..)
        | Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::In(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(_)
        | Cond::Match { .. } => None,
    }
}

/// `NOT EXISTS (SELECT ... FROM T WHERE p)`: of `s`, the subquery.
fn exists_form(s: &Subquery) -> Option<Form> {
    let rows = if let LogicalPlan::Project { exprs, input } = &s.plan {
        // The select list of EXISTS, which raises nothing.
        exprs.iter().all(|e| matches!(e, Expr::Column(_) | Expr::Const(_))).then_some(&**input)?
    } else {
        &s.plan
    };
    let (table, conds) = filtered_scan(rows).filter(|_| s.outer.is_empty())?;
    Some(Form { table, conds, kind: Kind::NoRow })
}

/// `(SELECT COUNT(*) FROM T WHERE p) op k`: of `s`, the subquery, and
/// `op`, the comparison with the count first.
fn count_form(s: &Subquery, op: CompOp, functions: &Registry) -> Option<Form> {
    let LogicalPlan::Project { exprs, input } = &s.plan else { return None };
    let LogicalPlan::Aggregate { groups, aggs, input: rows } = &**input else { return None };
    let ([Expr::Column(0)], [agg]) = (exprs.as_slice(), aggs.as_slice()) else { return None };
    // COUNT(*) alone: COUNT of a column eliminates its nulls, with a warning.
    let counted = groups.is_empty() && agg.arg.is_none() && !agg.distinct && functions.is_standard_count(&agg.func);
    let (table, conds) = filtered_scan(rows).filter(|_| counted && s.outer.is_empty())?;
    let safe = match op {
        CompOp::Lt | CompOp::Le => Safe::Fewer,
        CompOp::Gt | CompOp::Ge => Safe::More,
        CompOp::Eq | CompOp::Ne => Safe::Same,
    };
    Some(Form { table, conds, kind: Kind::Count(safe) })
}

/// The table `plan` reads and the conditions it keeps its rows by, if it is
/// filters of conditions of the row alone over a scan.
fn filtered_scan(plan: &LogicalPlan) -> Option<(QualifiedName, Vec<Cond>)> {
    let mut conds = Vec::new();
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Filter { cond, input } => {
                if !row_cond(cond) {
                    return None;
                }
                conds.push(cond.clone());
                node = input;
            }
            LogicalPlan::Scan { table, .. } => return Some((table.clone(), conds)),
            LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Project { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. } => return None,
        }
    }
}

/// Whether `c` is a condition of the row alone, as the module says.
fn row_cond(c: &Cond) -> bool {
    match c {
        Cond::Const(_) => true,
        Cond::Or(a, b) | Cond::And(a, b) => row_cond(a) && row_cond(b),
        Cond::Not(a) | Cond::Is(a, ..) => row_cond(a),
        Cond::Compare(a, _, b) => row_expr(a) && row_expr(b),
        Cond::IsNull(e, _) | Cond::In(e, _) => row_expr(e),
        Cond::Like(e, pattern, escape) => row_expr(e) && row_expr(pattern) && escape.as_ref().is_none_or(row_expr),
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(_) | Cond::Match { .. } => false,
    }
}

/// Whether `e` is an expression of the row alone, as the module says.
fn row_expr(e: &Expr) -> bool {
    match e {
        Expr::Column(_) | Expr::Const(_) => true,
        Expr::Neg(x) => row_expr(x),
        Expr::Arith(a, _, b) => row_expr(a) && row_expr(b),
        Expr::Case(whens, otherwise) => whens.iter().all(|(c, result)| row_cond(c) && row_expr(result)) && row_expr(otherwise),
        Expr::Call(..) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Cast(..) | Expr::Domain(..) | Expr::Subquery(..) => false,
    }
}

// Unit tests: the forms found of an assertion's bound query, and the
// shapes of none (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::functions::AggregateRef;
    use crate::logical::{AggCall, InSet};
    use crate::value::{ArithOp, DataType, Decimal, Spelled, Value};

    const COUNTED: DataType = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric };

    fn table() -> QualifiedName {
        QualifiedName { auth: "A".into(), ident: "T".into() }
    }

    fn scan() -> LogicalPlan {
        LogicalPlan::Scan { table: table(), names: vec!["K".into(), "N".into()], needed: vec![0, 1] }
    }

    fn number(n: i128) -> Expr {
        Expr::Const(Value::Exact(Decimal::new(n, 0)))
    }

    /// The rows of T where `cond`.
    fn filtered(cond: Cond) -> LogicalPlan {
        LogicalPlan::Filter { cond, input: Box::new(scan()) }
    }

    fn subquery(plan: LogicalPlan) -> Arc<Subquery> {
        Arc::new(Subquery { plan, outer: Vec::new() })
    }

    /// `NOT EXISTS (SELECT exprs FROM rows)`.
    fn none_of(exprs: Vec<Expr>, rows: LogicalPlan) -> Cond {
        Cond::Not(Box::new(Cond::Exists(subquery(LogicalPlan::Project { exprs, input: Box::new(rows) }))))
    }

    /// `(SELECT f(arg) FROM rows) op k`.
    fn counted(f: AggregateRef, arg: Option<Expr>, rows: LogicalPlan, op: CompOp) -> Cond {
        let agg = AggCall { func: f, arg, distinct: false, result: COUNTED, label: "COUNT(*)".into() };
        let aggregate = LogicalPlan::Aggregate { groups: Vec::new(), aggs: vec![agg], input: Box::new(rows) };
        let s = subquery(LogicalPlan::Project { exprs: vec![Expr::Column(0)], input: Box::new(aggregate) });
        Cond::Compare(Expr::Subquery(s, 0), op, number(10))
    }

    /// The bound query of an assertion of `condition`.
    fn asserting(condition: Cond) -> LogicalPlan {
        let values = LogicalPlan::Values { rows: vec![vec![number(1)]], width: 1 };
        let filter = LogicalPlan::Filter { cond: Cond::Not(Box::new(condition)), input: Box::new(values) };
        LogicalPlan::Project { exprs: vec![number(1)], input: Box::new(filter) }
    }

    fn kinds(condition: Cond, functions: &Registry) -> Option<Vec<Kind>> {
        forms_of(&asserting(condition), functions).map(|forms| forms.into_iter().map(|f| f.kind).collect())
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN]
    /// RULES.md D51: NOT EXISTS of filters of a row's conditions over a
    /// table, and COUNT(*) of them compared with a constant, either side,
    /// are found, each conjunct of an AND.
    #[test]
    fn forms_are_found_of_each_conjunct() {
        let functions = Registry::with_set_functions();
        let count = functions.aggregate("COUNT").unwrap();
        let row = Cond::And(
            Box::new(Cond::Compare(Expr::Neg(Box::new(Expr::Column(1))), CompOp::Lt, number(0))),
            Box::new(Cond::Is(Box::new(Cond::IsNull(Expr::Column(0), true)), Truth::True, false)),
        );
        let listed = Cond::In(Expr::Column(0), Arc::new(InSet::of(vec![Value::Exact(Decimal::new(1, 0))])));
        let row = Cond::And(Box::new(row), Box::new(listed));
        let case = Expr::Case(vec![(Cond::Const(Truth::True), Expr::Column(0))], Box::new(number(0)));
        let like = Cond::Like(Expr::Column(1), number(1), Some(number(2)));
        let Cond::Compare(s, op, k) = counted(count.clone(), None, scan(), CompOp::Lt) else { panic!("a comparison") };
        let flipped = Cond::Compare(k, op, s);
        let condition = [
            none_of(vec![number(1)], filtered(row)),
            counted(
                count.clone(),
                None,
                filtered(Cond::Or(Box::new(like), Box::new(Cond::Compare(case, CompOp::Eq, number(3))))),
                CompOp::Le,
            ),
            counted(count.clone(), None, scan(), CompOp::Ne),
            flipped,
        ]
        .into_iter()
        .reduce(|a, b| Cond::And(Box::new(a), Box::new(b)))
        .unwrap();
        assert_eq!(
            kinds(condition, &functions),
            Some(vec![Kind::NoRow, Kind::Count(Safe::Fewer), Kind::Count(Safe::Same), Kind::Count(Safe::More)])
        );
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2]; [DESIGN]
    /// RULES.md D51: no form is found of a function, a CAST, a subquery in
    /// a filter, an outer reference, a select list that can raise, a join,
    /// COUNT of a column or DISTINCT, a COUNT a plug-in replaced, an OR of
    /// conjuncts, nor of a query not an assertion's.
    #[test]
    fn other_shapes_have_no_form() {
        let functions = Registry::with_set_functions();
        let count = functions.aggregate("COUNT").unwrap();
        let mut replaced = Registry::with_set_functions();
        replaced.add_aggregate(Arc::clone(&functions.aggregate("SUM").unwrap().0)).unwrap();
        let sum = functions.aggregate("SUM").unwrap();
        let length = functions.scalar("CHAR_LENGTH").unwrap();
        let cast = Expr::Cast(Box::new(Expr::Column(0)), COUNTED);
        let call = Expr::Call(length, vec![Expr::Column(1)]);
        let inner = Cond::Exists(subquery(scan()));
        let raising = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Div, Box::new(number(0)));
        let joined = LogicalPlan::Product { left: Box::new(scan()), right: Box::new(scan()) };
        let cases = [
            none_of(vec![number(1)], filtered(Cond::Compare(call, CompOp::Eq, number(1)))),
            none_of(vec![number(1)], filtered(Cond::IsNull(cast, false))),
            none_of(vec![number(1)], filtered(inner)),
            none_of(vec![raising], scan()),
            none_of(vec![number(1)], joined),
            Cond::Not(Box::new(Cond::Exists(Arc::new(Subquery { plan: scan(), outer: vec![Expr::Column(0)] })))),
            Cond::Not(Box::new(Cond::Const(Truth::True))),
            counted(count.clone(), Some(Expr::Column(1)), scan(), CompOp::Le),
            counted(sum, None, scan(), CompOp::Le),
            Cond::Or(
                Box::new(counted(count.clone(), None, scan(), CompOp::Le)),
                Box::new(counted(count.clone(), None, scan(), CompOp::Ge)),
            ),
        ];
        for (n, condition) in cases.into_iter().enumerate() {
            assert!(kinds(condition, &functions).is_none(), "case {n}");
        }
        let kept = replaced.aggregate("COUNT").unwrap();
        assert!(kinds(counted(kept, None, scan(), CompOp::Le), &replaced).is_some(), "its COUNT, another set function replaced");
        let own = Arc::clone(&count.0);
        let mut other = Registry::with_set_functions();
        other.add_aggregate(own).unwrap();
        assert!(kinds(counted(count, None, scan(), CompOp::Le), &other).is_none(), "another registry's COUNT");
        assert!(forms_of(&scan(), &functions).is_none());
        let unfiltered = LogicalPlan::Project { exprs: vec![number(1)], input: Box::new(scan()) };
        assert!(forms_of(&unfiltered, &functions).is_none());
        let over_rows = LogicalPlan::Project {
            exprs: vec![number(1)],
            input: Box::new(LogicalPlan::Filter { cond: Cond::Not(Box::new(Cond::Const(Truth::True))), input: Box::new(scan()) }),
        };
        assert!(forms_of(&over_rows, &functions).is_none());
    }
}
