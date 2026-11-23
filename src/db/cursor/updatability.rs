//! A cursor's updatability [ISO/IEC 9075:1992 §13.1 SR5, SR6, SR12 to
//! SR14]: as declared, the columns it may set, and its rows opened for
//! positioned UPDATE and DELETE [§13.9 SR4, SR8]; and where FETCH moves
//! it [§13.3 SR3, SR4].

use std::sync::Arc;

use super::super::compiled::Updating;
use super::super::position::{Move, Position};
use super::super::roles::Valued;
use super::super::write::Written;
use super::super::{CursorRows, OpenCursor, State, Updatable, api, rule};
use super::ordered::{ordered_columns, row_condition};
use crate::ast::{CursorSpecification, FetchOrientation, Updatability, ValueSpecification};
use crate::binder;
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::physical::Plan;
use crate::txn::Txn;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075:1992 §13.1 SR5, SR6] How a cursor's updatability is
/// given: FOR READ ONLY, written or implicit; FOR UPDATE written; or FOR
/// UPDATE implicit, the cursor then updatable if its table is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Wanted {
    ReadOnly,
    Written,
    Implicit,
}

/// [ISO/IEC 9075:1992 §13.1 SR5, SR12] How the cursor of `spec`, declared
/// INSENSITIVE and SCROLL as `flags` say, gives its updatability.
///
/// # Errors
///
/// SQLCODE -103 if it is INSENSITIVE and FOR UPDATE \[SR12\].
pub(super) fn wanted(spec: &CursorSpecification, (insensitive, scroll): (bool, bool)) -> Result<Wanted> {
    match &spec.updatability {
        Some(Updatability::ReadOnly) => Ok(Wanted::ReadOnly),
        Some(Updatability::Update(_)) => {
            if insensitive {
                return Err(rule("[ISO/IEC 9075:1992 §13.1 SR12] an INSENSITIVE cursor is not FOR UPDATE".to_owned()));
            }
            Ok(Wanted::Written)
        }
        // [SR5a] INSENSITIVE, SCROLL or ORDER BY: READ ONLY; so too OFFSET
        // or FETCH FIRST, of a query not updatable [ISO/IEC 9075-2:2023
        // §7.17 SR7].
        None => Ok(if insensitive || scroll || spec.query.ordered().is_some() { Wanted::ReadOnly } else { Wanted::Implicit }),
    }
}

/// `wanted`, of a cursor `nameless` or not: of one no statement can name,
/// read only, unless its text says FOR UPDATE. No positioned statement is
/// positioned on it, and of a cursor of no `<cursor name>`, which can
/// write no `<cursor sensitivity>`, ASENSITIVE is implicit [ISO/IEC
/// 9075-4:2006 draft §14.13 SR6c; ISO/IEC 9075-2:2023 §14.2 SR1]: whether
/// changes made while it is open are visible through it is
/// implementation-dependent [§4.40.2], so it may be read as any read-only
/// cursor is (RULES.md D613).
pub(super) fn nameless_read_only(wanted: Wanted, nameless: bool) -> Wanted {
    match wanted {
        Wanted::Implicit => {
            if nameless {
                Wanted::ReadOnly
            } else {
                Wanted::Implicit
            }
        }
        Wanted::ReadOnly | Wanted::Written => wanted,
    }
}

/// [ISO/IEC 9075:1992 §13.1 SR6] Whether the cursor of `spec`, declared as
/// `flags` say, is updatable, as far as its text tells: FOR UPDATE, or an
/// updatable table.
///
/// # Errors
///
/// As `wanted`'s.
pub(in crate::db) fn declared_updatable(spec: &CursorSpecification, flags: (bool, bool)) -> Result<bool> {
    Ok(match wanted(spec, flags)? {
        Wanted::ReadOnly => false,
        Wanted::Written => true,
        Wanted::Implicit => binder::updatable(spec).is_some(),
    })
}

/// [ISO/IEC 9075:1992 §13.1 SR13, SR14] The columns of `table` (`desc`) a
/// positioned UPDATE through the cursor may set: those its FOR UPDATE
/// names, or, naming none, every one.
///
/// # Errors
///
/// SQLCODE -103 if it names a column `table` has not \[SR14\].
fn settable(desc: &TableDesc, updatability: Option<&Updatability>) -> Result<Vec<usize>> {
    let Some(Updatability::Update(Some(names))) = updatability else { return Ok((0..desc.columns.len()).collect()) };
    names
        .iter()
        .map(|(name, at)| {
            let refused = || rule(format!("[ISO/IEC 9075:1992 §13.1 SR14] {} has no column {name} to update", desc.name)).or_at(*at);
            desc.columns.iter().position(|c| c.name == *name).ok_or_else(refused)
        })
        .collect()
}

/// [ISO/IEC 9075:1992 §13.3 SR3, SR4, GR2] Where FETCH `orientation` moves
/// a cursor, SCROLL if `scroll`, its J's value as `valued` gives it.
///
/// # Errors
///
/// SQLCODE -103 if it is not NEXT and the cursor is not SCROLL \[SR3\], or
/// its J is not exact numeric with scale 0 \[SR4\]; -301 if J is null.
pub(in crate::db) fn movement(orientation: &FetchOrientation, scroll: bool, valued: &Valued<'_>) -> Result<Move> {
    let to = match orientation {
        FetchOrientation::Next => Move::Next,
        FetchOrientation::Prior => Move::Prior,
        FetchOrientation::First => Move::First,
        FetchOrientation::Last => Move::Last,
        FetchOrientation::Absolute(j) => Move::Absolute(how_far(j, valued)?),
        FetchOrientation::Relative(j) => Move::Relative(how_far(j, valued)?),
    };
    if !scroll && !matches!(to, Move::Next) {
        return Err(rule("[ISO/IEC 9075:1992 §13.3 SR3] the cursor is not SCROLL: it fetches NEXT alone".to_owned()));
    }
    Ok(to)
}

/// [ISO/IEC 9075:1992 §13.3 SR4, GR2a] The value J of ABSOLUTE or RELATIVE.
fn how_far(j: &ValueSpecification, valued: &Valued<'_>) -> Result<i128> {
    let not_exact = || rule("[ISO/IEC 9075:1992 §13.3 SR4] ABSOLUTE and RELATIVE take an exact numeric with scale 0".to_owned());
    let (value, data_type) = valued(j)?;
    if !matches!(data_type, DataType::Exact { scale: 0, .. }) {
        return Err(not_exact());
    }
    match value {
        Value::Exact(d) => Ok(d.units),
        Value::Null => fail_with(
            Code::Data,
            SqlState::NULL_VALUE_NO_INDICATOR,
            "[DESIGN] ABSOLUTE and RELATIVE take a number, not the null value".to_owned(),
        ),
        Value::Char(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => Err(not_exact()),
    }
}

/// An updatable cursor of `spec`, compiled as `updating` says, its
/// `<from clause>` naming `named`: its rows, of its table and of each
/// proper subtable it reads, may be updated or deleted through it, and its
/// subqueries' tables are read.
pub(super) fn open_updatable(
    st: &Arc<State>,
    tx: &Txn,
    spec: &CursorSpecification,
    (named, updating): (QualifiedName, Updating),
) -> Result<OpenCursor> {
    let Updating { compiled: c, table, subtables } = updating;
    st.update_lock(tx, &table)?;
    for (sub, _) in &subtables {
        st.update_lock(tx, sub)?;
    }
    for t in c.optimized.subquery_tables() {
        st.read_lock(tx, &t)?;
    }
    let projection: Vec<usize> = c
        .columns
        .iter()
        .map(|col| col.origin.ok_or_else(|| api(format!("[DESIGN] a cursor's column is not a column of {table}"))))
        .collect::<Result<_>>()?;
    let settable = settable(&st.base_table(&table)?, spec.updatability.as_ref())?;
    let ordered = ordered_columns(&c.optimized)?;
    let condition = row_condition(&c.optimized);
    let (subtables, mut plans): (Vec<QualifiedName>, Vec<Plan>) = subtables.into_iter().unzip();
    plans.insert(0, c.physical);
    let (ids, of) = super::super::scroll::ordered_ids(plans, st, tx.id)?;
    let snapshot = Arc::clone(st);
    let at = Position::Before(0);
    let only = binder::updatable_only(spec);
    let rows = Updatable {
        snapshot,
        named,
        table,
        ids,
        subtables,
        of,
        only,
        projection,
        condition,
        at,
        settable,
        ordered,
        changed: Vec::new(),
        on: None,
    };
    Ok(OpenCursor { columns: c.columns.into(), rows: CursorRows::Updatable(Box::new(rows)) })
}

/// [ISO/IEC 9075:1992 §13.9 SR4, SR8] The columns a positioned UPDATE `w`
/// through cursor `name`, updatable as `u`, sets are among those its FOR
/// UPDATE names, and none of them is read by its ORDER BY. A positioned
/// DELETE sets none.
pub(super) fn check_object_columns(u: &Updatable, w: &Written, name: &str) -> Result<()> {
    for (column, _) in w.set() {
        let named = || w.bound.desc.columns.get(*column).map_or_else(|| column.to_string(), |c| c.name.clone());
        if !u.settable.contains(column) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.9 SR8] cursor {name} is not FOR UPDATE of {}", named())));
        }
        if u.ordered.contains(column) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §13.9 SR4] cursor {name} is ordered by {}, which it does not update", named())));
        }
    }
    Ok(())
}
