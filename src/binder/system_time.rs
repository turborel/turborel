//! [ISO/IEC 9075-2:2023 §7.6 SR9c, SR9d, GR2c ii 1] A system-versioned
//! table read FOR SYSTEM_TIME AS OF a point in time, BETWEEN two or FROM
//! one TO another: of its current rows and its historical rows, kept in
//! its history (RULES.md D82), those whose system-time period the
//! specification says.
//!
//! Read with none, or AS OF CURRENT_TIMESTAMP as none implies [SR9c], it
//! is read as its current rows, as any table is. Of its rows then, its
//! current rows hold the transaction timestamp or an earlier start and
//! the highest value as their end, so that reading is theirs alone.

use std::collections::BTreeSet;

use super::from::Bounds;
use super::set_functions::column_references;
use super::views::Source;
use super::{Binder, Scope, rule};
use crate::ast::{NamedTable, SYSTEM_TIME, SystemTimeKind, SystemTimeRead, TypeName, ValueExpression};
use crate::catalog::{QualifiedName, TableDesc};
use crate::db::write::system_time::history_name;
use crate::error::Result;
use crate::logical::{Cond, Expr, LogicalPlan, SetOp};
use crate::value::{CompOp, DataType};

/// The rule a set function in a point in time breaks.
const NO_SET_FUNCTION: &str = "[ISO/IEC 9075-2:2023 §7.6 SR9d] a <point in time> contains no set function outside a subquery";

/// [ISO/IEC 9075-2:2023 §7.6 SR9c, SR9d, GR2c ii 1] Where the rows of the
/// table `name`, of `desc` and read from `source`, come from, read as
/// `read` says, if written: of a system-versioned table read FOR
/// SYSTEM_TIME other than AS OF CURRENT_TIMESTAMP, its rows and its
/// historical rows whose period it says, of its columns but none of its
/// constraints, which its historical rows do not keep; else as they are.
///
/// # Errors
///
/// SQLCODE -103 if the table is not system-versioned \[SR9d i\], or a
/// point in time is no datetime, or references a column or an SQL
/// parameter \[SR9d iii\], or holds a set function; or, but AS OF
/// CURRENT_TIMESTAMP, in a constraint [§9.16 SR1s, §11.9 CR2].
pub(super) fn system_time_source(
    b: &Bounds<'_>,
    (name, desc, source): (&QualifiedName, TableDesc, Source),
    read: Option<&SystemTimeRead>,
) -> Result<(TableDesc, Source)> {
    let Some(read) = read else { return Ok((desc, source)) };
    let history = history_name(name);
    if !matches!(source, Source::Table) || b.cx.db.catalog.table(&history).is_none() {
        return Err(not_versioned(&name.ident, read.at));
    }
    if read.current() {
        return Ok((desc, source));
    }
    // [§9.16 SR1s] Any other is a potential source of non-determinism.
    if b.cx.purpose.constrains() {
        return Err(super::expression::nondeterministic_source("s", &format!("{name} read FOR SYSTEM_TIME")).or_at(read.at));
    }
    let Some([start, end]) = desc.period_columns(SYSTEM_TIME) else {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6 GR2c ii 1 A] {name} has no system-time period")));
    };
    let start_type = desc.columns.get(start).map_or(DataType::Date, |c| c.data_type.clone());
    let binder = Binder::new(b.cx, b.outer, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let cond = period_condition(&binder, read, ([start, end], &start_type))?;
    let names: Vec<String> = desc.columns.iter().map(|c| c.name.clone()).collect();
    let needed: Vec<usize> = (0..names.len()).collect();
    let scan = |table: QualifiedName| LogicalPlan::Scan { table, names: names.clone(), needed: needed.clone(), located: false };
    let rows = LogicalPlan::SetOp {
        op: SetOp::Union,
        all: true,
        compared: None,
        left: Box::new(scan(name.clone())),
        right: Box::new(histories(b, name, history).into_iter().map(scan).reduce(union_all).unwrap_or_else(|| scan(history_name(name)))),
    };
    let plan = LogicalPlan::Filter { cond, input: Box::new(rows) };
    Ok((TableDesc { constraints: Vec::new(), ..desc }, Source::Derived(plan)))
}

/// [ISO/IEC 9075-2:2023 §7.6 GR2c ii 1 B, C, D] The condition a row of
/// system-time period start and end columns `columns`, the start of type
/// `start_type`, meets if `read` reads it, its points in time bound by
/// `binder`.
///
/// # Errors
///
/// As a point in time is bound (`point_in_time`).
fn period_condition(binder: &Binder<'_>, read: &SystemTimeRead, ([start, end], start_type): ([usize; 2], &DataType)) -> Result<Cond> {
    let point = |v: &ValueExpression| point_in_time(binder, v, (start_type, read.at));
    let (s, e) = (Expr::Column(start), Expr::Column(end));
    let compared = |l: &Expr, op: CompOp, r: &Expr| Cond::Compare(l.clone(), op, r.clone());
    let and = |l: Cond, r: Cond| Cond::And(Box::new(l), Box::new(r));
    Ok(match &read.kind {
        // [GR2c ii 1 D I] SSTARTCOL <= POT1 AND SENDCOL > POT1.
        SystemTimeKind::AsOf(v) => {
            let pot1 = point(v)?;
            and(compared(&s, CompOp::Le, &pot1), compared(&e, CompOp::Gt, &pot1))
        }
        // [GR2c ii 1 C, D II] POT1 <= POT2 AND SENDCOL > POT1 AND
        // SSTARTCOL <= POT2, the two exchanged if SYMMETRIC and out of order.
        SystemTimeKind::Between(symmetric, v1, v2) => {
            let (pot1, pot2) = ordered(*symmetric, (point(v1)?, point(v2)?));
            and(and(compared(&pot1, CompOp::Le, &pot2), compared(&e, CompOp::Gt, &pot1)), compared(&s, CompOp::Le, &pot2))
        }
        // [GR2c ii 1 D III] POT1 < POT2 AND SENDCOL > POT1 AND SSTARTCOL < POT2.
        SystemTimeKind::FromTo(v1, v2) => {
            let (pot1, pot2) = (point(v1)?, point(v2)?);
            and(and(compared(&pot1, CompOp::Lt, &pot2), compared(&e, CompOp::Gt, &pot1)), compared(&s, CompOp::Lt, &pot2))
        }
    })
}

/// [ISO/IEC 9075-2:2023 §7.6 SR9d i] That `t`, which names a query of a
/// WITH clause or a transition table, is read with no FOR SYSTEM_TIME.
///
/// # Errors
///
/// SQLCODE -103 if it is read so.
pub(super) fn unversioned(t: &NamedTable) -> Result<()> {
    t.system_time.as_ref().map_or(Ok(()), |read| Err(not_versioned(&t.table.ident, read.at)))
}

/// [ISO/IEC 9075-2:2023 §7.6 SR9d i] The refusal of `name`, no
/// system-versioned table, read FOR SYSTEM_TIME at `at`.
fn not_versioned(name: &str, at: usize) -> crate::error::SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §7.6 SR9d i] {name} is not a system-versioned table, read FOR SYSTEM_TIME")).or_at(at)
}

/// [ISO/IEC 9075-2:2023 §7.6 GR2c ii 1 C I] POT1 and POT2 of `pot`, of
/// BETWEEN, exchanged if `symmetric` and the first is later.
fn ordered(symmetric: bool, (pot1, pot2): (Expr, Expr)) -> (Expr, Expr) {
    if !symmetric {
        return (pot1, pot2);
    }
    let later = Cond::Compare(pot1.clone(), CompOp::Gt, pot2.clone());
    let first = Expr::Case(vec![(later.clone(), pot2.clone())].into(), Box::new(pot1.clone()));
    let second = Expr::Case(vec![(later, pot1)].into(), Box::new(pot2));
    (first, second)
}

/// [ISO/IEC 9075-2:2023 §7.6 SR9d iii, GR2c ii 1 B, C] The point in time
/// `v`, written at `at`, bound by `binder` and CAST to `start_type`, DT,
/// the type of the system-time period start column: a `?` of DT.
///
/// # Errors
///
/// SQLCODE -103 if it is no datetime, references a column or an SQL
/// parameter, or holds a set function.
fn point_in_time(binder: &Binder<'_>, v: &ValueExpression, (start_type, at): (&DataType, usize)) -> Result<Expr> {
    let mut referenced = Vec::new();
    column_references(v, &mut referenced);
    if let Some(r) = referenced.first() {
        let ValueExpression::Column(cs) = r else {
            return Err(rule("[ISO/IEC 9075-2:2023 §7.6 SR9d iii] a <point in time> contains no <column reference>").or_at(at));
        };
        let what = if binder.sql_parameter(cs)?.is_some() { "<SQL parameter reference>" } else { "<column reference>" };
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6 SR9d iii] a <point in time> contains no {what}, {}", cs.column)).or_at(cs.at));
    }
    let scope = Scope::Rows(NO_SET_FUNCTION);
    if super::expression::dynamic_of(v).is_none() {
        let (_, t) = binder.value_expression(v, scope)?;
        if !matches!(t, DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. }) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6] a <point in time> is a <datetime value expression>, not {t}")).or_at(at));
        }
    }
    let cast = ValueExpression::Cast {
        operand: Some(Box::new(v.clone())),
        target: TypeName::Data(start_type.clone()),
        format: None,
        passing: None,
        at,
    };
    let (e, _) = binder.value_expression(&cast, scope)?;
    let mut columns = Vec::new();
    crate::logical::expr_columns(&e, &mut columns);
    if !columns.is_empty() || outer(&e) {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.6 SR9d iii] a <point in time> contains no <column reference>").or_at(at));
    }
    Ok(e)
}

/// Whether `e` references a column of an outer query.
fn outer(e: &Expr) -> bool {
    let mut found = matches!(e, Expr::Outer(..));
    e.for_each_child(&mut |child| found |= outer(child));
    found
}

/// \[EXTENSION\] The histories of `name`: its own, `history`, and of a
/// partitioned table each of its partitions', where their historical
/// rows are kept (RULES.md D159).
fn histories(b: &Bounds<'_>, name: &QualifiedName, history: QualifiedName) -> Vec<QualifiedName> {
    let mut all = vec![history];
    for d in b.cx.db.catalog.partitioning().descendants(name) {
        let of = history_name(&d);
        if b.cx.db.catalog.table(&of).is_some() {
            all.push(of);
        }
    }
    all
}

/// The rows of `left`, then of `right`.
fn union_all(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
    LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left: Box::new(left), right: Box::new(right) }
}
