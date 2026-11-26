//! \[DESIGN\] Rows given by column straight to a group (RULES.md D1148).
//! An execution of a copy's INSERT given its rows by column as text
//! (`Session::execute_prepared_lanes`) parses each column straight into
//! its type (`storage/lanes/parse.rs`), and the storage writes the group
//! of them from each column's records, no row of values made, where
//! nothing the rows need is given a row alone:
//!
//! * the INSERT is into a base table, through no view WITH CHECK OPTION,
//!   of no identity, generated or self-referencing column;
//! * its rows are checked as they go in (`rows_checked`): no trigger, no
//!   deferred constraint, no assertion reads the table;
//! * the table qualifies for extents (D1147) and its load writes groups;
//! * every column is of a type a record holds plainly, no large object,
//!   no user-defined or extension type;
//! * the rows hold its NOT NULL constraints and CHECKs, each checked by
//!   the columns it reads alone.
//!
//! Each column the INSERT gives no value takes its default, the same of
//! each row, as the statement found it. Else, or if a cell is not a
//! value of its column's type, the rows go in as rows, each cell cast as a
//! value given is, which tells what fails and where.

use super::Target;
use crate::db::{Session, State, StatementContext};
use crate::error::{Code, Result, SqlError};
use crate::params::{Gift, Given, Load, conformed_row};
use crate::storage::Row;
use crate::storage::lanes::{Lane, Lanes, Records, parse, set_null};
use crate::sync::lock;
use crate::value::{DataType, Value, assign};

#[cfg(test)]
thread_local! {
    /// The executions this thread put in by column, for its tests.
    static BY_COLUMN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Session {
    /// \[DESIGN\] RULES.md D1148: the rows `given` holds by column, if it
    /// holds them so, inserted into `target` as one group straight from
    /// their columns, if they may go so: how many; none if they go in as
    /// rows, `given` left as it was.
    ///
    /// # Errors
    ///
    /// As a column's parse, `hold_back`, `State::insert_records`, the
    /// load's end and the statement's.
    pub(super) fn insert_lanes(
        &self,
        cx: StatementContext<'_>,
        target: &Target,
        given: &Given,
        load: Option<Load>,
    ) -> Result<Option<usize>> {
        let StatementContext { st, tx } = cx;
        let mut held = lock(given)?;
        let Some(Gift::Lanes(lanes, types)) = held.as_ref() else { return Ok(None) };
        let effects = self.effects(st);
        if !plain(st, target)? || lanes.rows == 0 {
            return Ok(None);
        }
        let Some(mut checked) = st.rows_checked(target.name(), &effects) else { return Ok(None) };
        let mut together = st.together(target.name())?;
        if !together.qualifies() {
            return Ok(None);
        }
        let columns = records_of(target, (lanes, types))?;
        if !checked.hold_by_column((st, tx), &columns)? {
            return Ok(None);
        }
        self.settle_cursors(st, target.name());
        super::bulk::load::hold_back(st, tx, &mut together)?;
        let Some(ids) = st.insert_records(tx, &together, &columns)? else { return Ok(None) };
        held.take();
        drop(held);
        #[cfg(test)]
        BY_COLUMN.with(|n| n.set(n.get().saturating_add(1)));
        super::bulk::load::end(st, tx, load)?;
        st.end_statement(tx, effects)?;
        Ok(Some(ids.len()))
    }
}

/// Whether the rows of an INSERT into `target` may go in by column: into
/// a base table, through no view WITH CHECK OPTION, of no column a row is
/// given a value of, each of a type a record holds plainly, rows of which
/// go in together.
fn plain(st: &State, target: &Target) -> Result<bool> {
    let generated = target.generating.columns().next().is_some() || target.generating.drawn_identity().is_some();
    if generated || !target.checks().is_empty() || !st.inserts_together(target.name())? {
        return Ok(false);
    }
    Ok(target.desc.columns.iter().all(|c| plain_type(&c.data_type)))
}

/// Whether a value of `t` is held plainly in a record: a number, a
/// character string, a datetime or a truth value.
fn plain_type(t: &DataType) -> bool {
    matches!(
        t,
        DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Boolean
    )
}

/// The records of each column of the table of `target`, in order: of an
/// object column, its lane parsed into its type, its parameter's of
/// `types` first, as a value given is cast; of any other, its default, the
/// same of each row.
///
/// # Errors
///
/// As `parsed`.
fn records_of(target: &Target, (lanes, types): (&Lanes, &[DataType])) -> Result<Vec<Records>> {
    let mut columns = Vec::with_capacity(target.desc.columns.len());
    for (at, column) in target.desc.columns.iter().enumerate() {
        let given = target.object().iter().position(|&c| c == at).and_then(|i| Some((lanes.columns.get(i)?, types.get(i)?)));
        let records = match given {
            Some((lane, param)) if *param == column.data_type => parse(param, lane, lanes.rows)?,
            Some((lane, param)) => parsed((param, &column.data_type), lane, lanes.rows)?,
            None => repeated(&column.data_type, target.defaults.get(at).unwrap_or(&Value::Null), lanes.rows)?,
        };
        columns.push(records);
    }
    Ok(columns)
}

/// The records of the `rows` cells of `lane`, each cast to `param`, as a
/// value given is, then assigned to `column`, as INSERT assigns: of a
/// parameter of a type other than its column's, which `parse` does not
/// take.
///
/// # Errors
///
/// As `Lane::cell`, `conformed_row`, `assign` and `encode_record_into`.
fn parsed((param, column): (&DataType, &DataType), lane: &Lane, rows: usize) -> Result<Records> {
    lane.check(rows)?;
    let mut records = Records { nulls: lane.nulls.clone(), ..Records::default() };
    for n in 0..rows {
        if let Some(text) = lane.cell(n)? {
            let given: Row = vec![Value::Char(text.to_owned().into())];
            let value = conformed_row(given, std::slice::from_ref(param))?.pop().unwrap_or(Value::Null);
            let value = assign(value, column, "ISO/IEC 9075:1992 §13.8 GR4b iii")?;
            if value.is_null() {
                set_null(&mut records.nulls, n);
            } else {
                records.bytes.extend_from_slice(&column.encode_record(&value)?);
            }
        }
        records.ends.push(u32::try_from(records.bytes.len()).map_err(|_| too_large())?);
    }
    Ok(records)
}

/// `rows` records of `value`, of type `t`.
///
/// # Errors
///
/// As `DataType::encode_record`.
fn repeated(t: &DataType, value: &Value, rows: usize) -> Result<Records> {
    let mut records = Records { nulls: vec![0; rows.div_ceil(8)], ..Records::default() };
    if value.is_null() {
        for n in 0..rows {
            set_null(&mut records.nulls, n);
        }
        records.ends = vec![0; rows];
        return Ok(records);
    }
    let one = t.encode_record(value)?;
    records.bytes.reserve(one.len().saturating_mul(rows));
    for _ in 0..rows {
        records.bytes.extend_from_slice(&one);
        records.ends.push(u32::try_from(records.bytes.len()).map_err(|_| too_large())?);
    }
    Ok(records)
}

fn too_large() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a load's column is too large for one group")
}

#[cfg(test)]
mod tests;
