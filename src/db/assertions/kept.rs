//! \[EXTENSION\] The totals an assertion keeps [ISO/IEC 9075:1992 §11.34
//! GR2] (RULES.md D51): of each form of SUM or AVG of exact numbers, and of
//! each form of one table's rows whose set function's argument may be
//! null, the values counted, the nulls among the rows passing, and the
//! sum, each as a transaction sees them, so a change is judged with them
//! and not by reading the table whole.
//!
//! - **Where.** A table of the row store in DEFINITION_SCHEMA of each
//!   assertion keeping any, named for it (`kept_name`), which no statement
//!   names [§21.1]. Each statement that changes a table the assertion reads
//!   appends a row of each form whose totals it changes: the form, its
//!   change of the count, of the nulls and of the sum, and which way it
//!   moves the form's value (`Move`). Writers never meet on a row.
//! - **Made.** As the assertion is made, of the tables as its transaction
//!   sees them, which it holds exclusive and marks changed, so a writer
//!   whose snapshot predates it changes them only once it is published.
//! - **Read.** A form's totals are the sum of its rows the transaction
//!   sees, read unrecorded, and a read recorded, as a constraint's are
//!   (RULES.md D29), of the rows that move its value as the transaction's
//!   own change does: those two, moving it the same way, may break it
//!   together, so the second to commit fails; one moving it the other way
//!   cannot. SUM's sum is in range either way [§6.5 GR2b iv], so it is of
//!   either way; AVG's mean has no range of its own [GR2b ii], so only
//!   toward the bound. Of a value equal or unequal to a constant, every
//!   change counts.
//! - **Judged.** The totals after are judged by the set function's own
//!   accumulator, as its state merged (RULES.md D53), so as the whole
//!   check judges them: out of range, 22003; of no value, null.
//! - **Folded.** A job of the thread manager folds a table's rows into one
//!   of each form, its writes keeping every total (`fold.rs`).

use std::sync::Arc;

use super::changes::passes;
use super::delta::Changes;
use super::forms::{Form, Kind, Source, Totalled};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::engine::rows::ROW_STORE;
use crate::error::{Code, Result, SqlError};
use crate::exec::{Heard, eval, eval_cond};
use crate::index::Probe;
use crate::logical::{Cond, Expr, Truth};
use crate::storage::{Condition, Conjunct, Conjunction, Row};
use crate::system::DEFINITION_SCHEMA;
use crate::txn::{ReadMode, Txn};
use crate::value::codec::put_value;
use crate::value::{CharUnits, CompOp, DataType, Decimal, INTEGER_PRECISION, MAX_EXACT_PRECISION, Spelled, Value};

/// The columns of a table of kept totals, by place: the form, the change
/// of the values counted, of the nulls, the sum's units and scale, and
/// which way the change moves the form's value.
const FORM: usize = 0;
const COUNTED: usize = 1;
const NULLED: usize = 2;
const UNITS: usize = 3;
const SCALE: usize = 4;
const MOVE: usize = 5;
pub(super) const WIDTH: usize = 6;

/// The table of the totals assertion `assertion` keeps: in
/// DEFINITION_SCHEMA, its schema's length first, so no two assertions'
/// names meet.
pub(in crate::db) fn kept_name(assertion: &QualifiedName) -> QualifiedName {
    let ident = format!("ASSERTION TOTALS {} {}{}", assertion.auth.chars().count(), assertion.auth, assertion.ident);
    QualifiedName { auth: DEFINITION_SCHEMA.to_owned(), ident }
}

/// The description of a table of kept totals, `name`, of `generation`.
pub(super) fn kept_desc(name: QualifiedName, generation: u32) -> TableDesc {
    let integer = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };
    let count = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Decimal };
    let column = |named: &str, data_type| ColumnDesc {
        name: named.to_owned(),
        data_type,
        not_null: true,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let columns = vec![
        column("FORM", integer),
        column("COUNTED", count),
        column("NULLED", count),
        column("UNITS", DataType::Varying { length: 40, units: CharUnits::Characters }),
        column("SCALE", integer),
        column("MOVE", integer),
    ];
    TableDesc { name, columns, constraints: Vec::new(), engine: ROW_STORE.into(), options: Vec::new(), generation, temporary: None }
}

/// Whether `form` keeps totals: SUM or AVG; or, of a table's rows, COUNT,
/// or a set function's argument that may be null, whose nulls tell
/// whether the warning of a null eliminated is raised [§6.5 GR1b].
pub(super) fn keeps(form: &Form) -> bool {
    let of_table = matches!(form.source, Source::Table(_));
    matches!(form.kind, Kind::Total(_)) || (of_table && (form.nulls.is_some() || matches!(form.kind, Kind::Count(..))))
}

/// A form's totals, or a change of them: the values counted, the nulls,
/// and the sum of the values, none of no value.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Totals {
    pub(super) counted: i128,
    pub(super) nulled: i128,
    pub(super) sum: Option<Decimal>,
}

impl Totals {
    /// These and `other` together.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past what 128 bits hold, as SUM's own sum is.
    pub(super) fn plus(self, other: Totals) -> Result<Totals> {
        let sum = match (self.sum, other.sum) {
            (Some(a), Some(b)) => Some(a.wide_add(b).ok_or_else(beyond)?),
            (one, None) | (None, one) => one,
        };
        let counted = self.counted.checked_add(other.counted).ok_or_else(beyond)?;
        Ok(Totals { counted, nulled: self.nulled.checked_add(other.nulled).ok_or_else(beyond)?, sum })
    }

    /// These less `other`.
    ///
    /// # Errors
    ///
    /// As `plus`.
    pub(super) fn minus(self, other: Totals) -> Result<Totals> {
        let negated = |n: i128| n.checked_neg().ok_or_else(beyond);
        let sum = other.sum.map(|d| negated(d.units).map(|units| Decimal::new(units, d.scale))).transpose()?;
        self.plus(Totals { counted: negated(other.counted)?, nulled: negated(other.nulled)?, sum })
    }

    /// Whether they change nothing.
    pub(super) fn is_zero(&self) -> bool {
        self.counted == 0 && self.nulled == 0 && self.sum.is_none_or(|d| d.units == 0)
    }
}

/// SQLCODE -900: totals 128 bits cannot hold, a limit of this
/// implementation, as SUM's own (`functions/sum.rs`).
fn beyond() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a total kept needs more than the 128 bits this implementation sums in")
}

/// Which way a change moves a form's value: SUM's sum, or of AVG the sum
/// less the bound times the count, whose sign the mean's side of the bound
/// follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Move {
    Up,
    Down,
    Still,
}

impl Move {
    /// As kept: 1, -1 or 0.
    fn sign(self) -> i128 {
        match self {
            Move::Up => 1,
            Move::Down => -1,
            Move::Still => 0,
        }
    }

    /// Of a number's sign.
    fn of(sign: std::cmp::Ordering) -> Move {
        match sign {
            std::cmp::Ordering::Greater => Move::Up,
            std::cmp::Ordering::Less => Move::Down,
            std::cmp::Ordering::Equal => Move::Still,
        }
    }
}

/// Which way `delta` moves the value of `t`'s form.
pub(super) fn moves(t: &Totalled, delta: &Totals) -> Move {
    let sum = delta.sum.unwrap_or(Decimal::new(0, 0));
    if !t.avg {
        return Move::of(sum.units.cmp(&0));
    }
    let Expr::Const(Value::Exact(k)) = &t.k else { return Move::Still };
    // sum - k * count, at the larger scale; past 128 bits, as f64s, whose
    // sign is then sure, the magnitudes far from zero.
    let scale = sum.scale.max(k.scale);
    let lift = |d: &Decimal| 10_i128.checked_pow(scale.saturating_sub(d.scale)).and_then(|p| d.units.checked_mul(p));
    let exact = lift(&sum).zip(lift(k).and_then(|kk| kk.checked_mul(delta.counted))).and_then(|(s, kc)| s.checked_sub(kc));
    match exact {
        Some(difference) => Move::of(difference.cmp(&0)),
        None => Move::of((sum.to_f64() - k.to_f64() * Decimal::new(delta.counted, 0).to_f64()).total_cmp(&0.0_f64)),
    }
}

/// What reading a form's totals records, to be checked at commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Register {
    /// Nothing: the change cannot move the value toward breaking it.
    Nothing,
    /// Every row of the form.
    Form,
    /// The rows of the form that move its value so.
    Moving(Move),
}

impl Register {
    /// What a transaction whose change moves a value compared by `op`, the
    /// value first, `mv` records: of a value in range either way, as SUM's
    /// [§6.5 GR2b iv], if `ranged`; else, as AVG's and COUNT's, toward the
    /// bound alone.
    pub(super) fn of(op: CompOp, ranged: bool, mv: Move) -> Register {
        match (mv, op, ranged) {
            (Move::Still, _, _) | (Move::Up, CompOp::Gt | CompOp::Ge, false) | (Move::Down, CompOp::Lt | CompOp::Le, false) => {
                Register::Nothing
            }
            (_, CompOp::Eq | CompOp::Ne, _) => Register::Form,
            (_, _, true) | (Move::Up, CompOp::Lt | CompOp::Le, false) | (Move::Down, CompOp::Gt | CompOp::Ge, false) => {
                Register::Moving(mv)
            }
        }
    }

    /// The condition read of form `at`'s rows, if any.
    fn condition(self, at: usize) -> Option<Conjunction> {
        let compared = |column: usize, op: CompOp, n: i128| {
            Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value: number(n) } })
        };
        let of_form = compared(FORM, CompOp::Eq, i128::try_from(at).unwrap_or(i128::MAX));
        match self {
            Register::Nothing | Register::Moving(Move::Still) => None,
            Register::Form => Some(Conjunction(vec![of_form])),
            Register::Moving(Move::Up) => Some(Conjunction(vec![of_form, compared(MOVE, CompOp::Gt, 0)])),
            Register::Moving(Move::Down) => Some(Conjunction(vec![of_form, compared(MOVE, CompOp::Lt, 0)])),
        }
    }
}

/// `n` as an exact number.
fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// What `row`, passing or not, adds to `form`'s totals.
///
/// # Errors
///
/// A step's or the argument's evaluation's.
pub(super) fn row_totals(form: &Form, row: &Row, tx: &Txn) -> Result<Totals> {
    let heard = Heard(tx.stop.warnings());
    let mut out = Totals::default();
    if let Some((at, x)) = &form.nulls
        && passes(form.steps.get(..*at).unwrap_or_default(), row, tx)?
        && eval(x, row, &heard)?.is_null()
    {
        out.nulled = 1;
    }
    if matches!(form.kind, Kind::Count(..)) && passes(&form.steps, row, tx)? {
        // COUNT's steps keep its argument's nulls out (`forms.rs`).
        out.counted = 1;
    }
    if let Kind::Total(t) = &form.kind
        && passes(&form.steps, row, tx)?
    {
        match eval(&t.x, row, &heard)? {
            Value::Exact(d) => out = Totals { counted: 1, sum: Some(d), ..out },
            Value::Null => {}
            other @ (Value::Char(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_)) => {
                return Err(SqlError::new(Code::Api, format!("[DESIGN] {other:?} is no exact number of a SUM or AVG kept")));
            }
        }
    }
    Ok(out)
}

/// The judgment of `t` by its totals `after`: a value of no row is null,
/// so unknown; else as the set function finishes it.
///
/// # Errors
///
/// Its finish's, SQLSTATE 22003 out of range [ISO/IEC 9075:1992 §6.5 GR2b
/// iv], as the whole check raises it.
pub(super) fn judged(t: &Totalled, after: &Totals, tx: &Txn) -> Result<Truth> {
    if after.counted == 0 {
        return Ok(Truth::Unknown);
    }
    let mut state = Vec::new();
    if t.avg {
        state.extend_from_slice(&after.counted.to_le_bytes());
    }
    put_value(&mut state, &Value::Exact(after.sum.unwrap_or(Decimal::new(0, 0))));
    let mut accumulator = t.agg.func.0.accumulator(t.agg.result);
    accumulator.merge(&state)?;
    compared(accumulator.finish()?, t.op, &t.k, tx)
}

/// The truth of `value op k` [ISO/IEC 9075:1992 §8.2].
///
/// # Errors
///
/// The comparison's.
fn compared(value: Value, op: CompOp, k: &Expr, tx: &Txn) -> Result<Truth> {
    eval_cond(&Cond::Compare(Expr::Const(value), op, k.clone()), &Vec::new(), &Heard(tx.stop.warnings()))
}

/// Which way `delta` moves the value of `form`: of SUM, AVG or COUNT; of
/// nulls alone, none.
fn moved(form: &Form, delta: &Totals) -> Move {
    match &form.kind {
        Kind::Total(t) => moves(t, delta),
        Kind::Count(..) => Move::of(delta.counted.cmp(&0)),
        Kind::NoRow | Kind::Bound | Kind::Reach(_) | Kind::Distinct(..) | Kind::Apart { .. } => Move::Still,
    }
}

/// A row of kept totals: of form `at`, the change `delta`, moving `mv`.
pub(super) fn kept_row(at: usize, delta: &Totals, mv: Move) -> Row {
    let sum = delta.sum.unwrap_or(Decimal::new(0, 0));
    vec![
        number(i128::try_from(at).unwrap_or(i128::MAX)),
        number(delta.counted),
        number(delta.nulled),
        Value::Char(sum.units.to_string()),
        number(i128::from(sum.scale)),
        number(mv.sign()),
    ]
}

/// Of a row of kept totals, its form and its totals.
///
/// # Errors
///
/// SQLCODE -916 if it is not one this module writes.
pub(super) fn parse_row(row: &Row) -> Result<(usize, Totals)> {
    let corrupt = || SqlError::new(Code::Corrupt, "[DESIGN] a row of an assertion's kept totals is not one they write");
    let int = |at: usize| match row.get(at) {
        Some(Value::Exact(d)) => (d.scale == 0).then_some(d.units).ok_or_else(corrupt),
        _ => Err(corrupt()),
    };
    let Some(Value::Char(units)) = row.get(UNITS) else { return Err(corrupt()) };
    let units: i128 = units.trim_end().parse().map_err(|_| corrupt())?;
    let scale = u32::try_from(int(SCALE)?).map_err(|_| corrupt())?;
    let form = usize::try_from(int(FORM)?).map_err(|_| corrupt())?;
    Ok((form, Totals { counted: int(COUNTED)?, nulled: int(NULLED)?, sum: Some(Decimal::new(units, scale)) }))
}

/// What the totals kept of a form show of a change: of SUM or AVG, its
/// truth after; whether a null is eliminated after it; and the change, to
/// be appended.
pub(super) struct KeptSeen {
    pub(super) truth: Option<Truth>,
    pub(super) null: bool,
    pub(super) change: Totals,
}

impl super::super::State {
    /// What the change `changes` shows of `form`, form `at` of an
    /// assertion whose totals are kept in `table`: read, recording what
    /// the change's move needs (`Register`), and judged with the change's.
    ///
    /// # Errors
    ///
    /// Reading the rows', or the totals'; a step's; SQLSTATE 22003 of a sum
    /// out of range, as the whole check raises it.
    pub(super) fn kept_seen(
        &self,
        tx: &Txn,
        (table, at, form): (&QualifiedName, usize, &Form),
        changes: &mut dyn Changes,
    ) -> Result<KeptSeen> {
        let Source::Table(rows) = &form.source else { return Err(SqlError::new(Code::Api, "[DESIGN] totals kept of no table")) };
        let delta = self.change_totals(tx, (form, rows), changes)?;
        let register = match &form.kind {
            Kind::Total(t) => Register::of(t.op, !t.avg, moves(t, &delta)),
            Kind::Count(op, _) => Register::of(*op, false, moved(form, &delta)),
            Kind::NoRow | Kind::Bound | Kind::Reach(_) | Kind::Distinct(..) | Kind::Apart { .. } => Register::Nothing,
        };
        let after = self.read_totals(tx, (table, at), register)?.plus(delta)?;
        let truth = match &form.kind {
            Kind::Total(t) => Some(judged(t, &after, tx)?),
            // [ISO/IEC 9075:1992 §6.5 GR1a, GR2a] COUNT is a cardinality, of no row 0.
            Kind::Count(op, k) => Some(compared(number(after.counted), *op, k, tx)?),
            Kind::NoRow | Kind::Bound | Kind::Reach(_) | Kind::Distinct(..) | Kind::Apart { .. } => None,
        };
        Ok(KeptSeen { truth, null: after.nulled > 0, change: delta })
    }

    /// The totals of form `at` kept in `table`, as transaction `tx` sees
    /// them: every row read unrecorded, then a read recorded as `register`
    /// says (`Register::condition`), to be checked at commit.
    ///
    /// # Errors
    ///
    /// Reading the table's; its rows', if not ones written here.
    pub(super) fn read_totals(&self, tx: &Txn, (table, at): (&QualifiedName, usize), register: Register) -> Result<Totals> {
        self.read_lock(tx, table)?;
        let storage = Arc::clone(self.storage_of(table)?);
        let columns: Vec<usize> = (0..WIDTH).collect();
        storage.read_mode(tx.id, ReadMode { snapshot: tx.snapshot(), checked: false })?;
        let mut seen = 0_usize;
        let read = Arc::clone(&storage).scan(tx.id, &columns).try_fold(Totals::default(), |total, found| {
            seen = seen.saturating_add(1);
            let (form, totals) = parse_row(&found?.1)?;
            if form == at { total.plus(totals) } else { Ok(total) }
        });
        storage.read_mode(tx.id, tx.read_mode())?;
        // Many rows, to be folded (`fold.rs`).
        if seen > super::fold::FOLD_ROWS {
            self.tx.note_fold(table);
        }
        if let Some(condition) = register.condition(at) {
            // Begun, the scan has recorded its read.
            drop(Arc::clone(&storage).scan_where(tx.id, &[FORM], &condition));
        }
        read
    }

    /// Adds to `table` the change `delta` of `form`, form `at` of its
    /// assertion: to the row transaction `tx` wrote of it before, if a
    /// statement undone has not taken it, else as a row of its own, so a
    /// transaction keeps one row of each form, which way its changes
    /// together move the form's value written in it. A change of nothing
    /// writes none.
    ///
    /// # Errors
    ///
    /// The write's; a row of `tx`'s not one written here.
    pub(super) fn append_totals(&self, tx: &Txn, (table, at): (&QualifiedName, usize), (delta, form): (Totals, &Form)) -> Result<()> {
        if delta.is_zero() {
            return Ok(());
        }
        let storage = self.storage_of(table)?;
        if let Some(id) = tx.kept_row(table, at)?
            && let Some(row) = storage.get(tx.id, id)?
        {
            let merged = parse_row(&row)?.1.plus(delta)?;
            return self.update_row(tx, table, id, &kept_row(at, &merged, moved(form, &merged)));
        }
        let id = self.insert_row(tx, table, &kept_row(at, &delta, moved(form, &delta)))?;
        tx.note_kept_row(table, at, id)
    }

    /// The change `changes` makes of `form`'s totals, of the rows of
    /// `table`: each as it is now, less as it was.
    ///
    /// # Errors
    ///
    /// Reading the rows', or a step's.
    pub(super) fn change_totals(&self, tx: &Txn, (form, table): (&Form, &QualifiedName), changes: &mut dyn Changes) -> Result<Totals> {
        let storage = self.storage_of(table)?;
        let mut delta = Totals::default();
        changes.visit(table, &mut |id, before| {
            if let Some(now) = storage.get(tx.id, id)? {
                delta = delta.plus(row_totals(form, &now, tx)?)?;
            }
            if let Some(before) = before {
                delta = delta.minus(row_totals(form, before, tx)?)?;
            }
            Ok(())
        })?;
        Ok(delta)
    }

    /// `form`'s totals of every row of `table`, as `tx` sees them.
    ///
    /// # Errors
    ///
    /// Reading the rows', or a step's.
    pub(super) fn whole_totals(&self, tx: &Txn, (form, table): (&Form, &QualifiedName)) -> Result<Totals> {
        let width = self.catalog.table(table).map_or(0, |d| d.columns.len());
        let storage = Arc::clone(self.storage_of(table)?);
        storage
            .scan(tx.id, &(0..width).collect::<Vec<_>>())
            .try_fold(Totals::default(), |total, found| total.plus(row_totals(form, &found?.1, tx)?))
    }
}

// Unit tests: totals added and taken, the way a change moves a value, what
// is recorded of it, and a row written read back (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
