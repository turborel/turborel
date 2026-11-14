//! ANY_VALUE, the set function of a value of its group [ISO/IEC
//! 9075-2:2023 §10.9]; and the value a group has of a column functionally
//! dependent on its grouping columns [§6.7 GR1e], kept as ANY_VALUE keeps
//! its value, a null too.

use super::{Accumulator, AggregateFunction};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::value::codec::{put_value, value};
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §10.9] ANY_VALUE's name, no key word of SQL-92.
pub const ANY_VALUE: &str = "ANY_VALUE";

/// ANY_VALUE. [ISO/IEC 9075-2:2023 §10.9 SR7f] The result type is the
/// argument's. [GR7d v] Of the values not null, an implementation-dependent
/// one (UV062): the first one it is given.
pub(super) struct AnyValue;

impl AggregateFunction for AnyValue {
    fn name(&self) -> &str {
        ANY_VALUE
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        // [ISO/IEC 9075-2:2023 §10.9 F] only COUNT has the form (*); the
        // extension call syntax always passes an argument too.
        arg.ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §10.9 F] {ANY_VALUE} takes an argument")))
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(First { first: None, written: !matches!(result, DataType::Extension(_)) })
    }
}

/// The first value given, if one was; and whether its state is written
/// (`Accumulator::write_state`): not of a plug-in's type, whose values are
/// read back only through the types registered.
struct First {
    first: Option<Value>,
    written: bool,
}

impl Accumulator for First {
    fn update(&mut self, v: &Value) -> Result<()> {
        if self.first.is_none() {
            self.first = Some(v.clone());
        }
        Ok(())
    }
    /// The first value not null of those at `rows`, or of all, if it has
    /// none yet; it reads no further.
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        if self.first.is_some() {
            return Ok(());
        }
        let found = match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(r)).find(|v| !v.is_null()),
            None => values.iter().find(|v| !v.is_null()),
        };
        self.first = found.cloned();
        Ok(())
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        if self.written {
            put_value(out, self.first.as_ref().unwrap_or(&Value::Null));
        }
        self.written
    }
    /// The value of a state of values given after its own, kept only if it
    /// has none, as when given each value.
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let mut reader = Reader::new(state, "a state of ANY_VALUE");
        let later = value(&mut reader)?;
        reader.finish()?;
        if later.is_null() { Ok(()) } else { self.update(&later) }
    }
    /// [ISO/IEC 9075-2:2023 §10.9 GR7d ii] Null of no values.
    fn finish(&self) -> Result<Value> {
        Ok(self.first.clone().unwrap_or(Value::Null))
    }
}

/// [ISO/IEC 9075-2:2023 §6.7 GR1e, §7.13 GR3a] The value a group has of a
/// group-invariant column reference: its column's in the group's first
/// row. Every row of the group has a value not distinct from it, so which
/// row gives it matters only to a character string, a datetime with time
/// zone or a value of a user-defined type, whose value is then
/// implementation-dependent (UV070, UV086). Its argument is a row of the
/// column alone, never null, so a null value is kept and no warning is
/// raised of it [ISO/IEC 9075:1992 §6.5 GR1b], as none is eliminated. No
/// call names it: the binder adds it to a grouped query that references
/// such a column (`binder/invariant.rs`), and its result type is the
/// column's.
pub(crate) struct GroupValue;

impl AggregateFunction for GroupValue {
    fn name(&self) -> &str {
        ANY_VALUE
    }
    /// The type of the one field of its argument's row type.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        let field = match arg {
            Some(DataType::Row(row)) => row.fields.first().map(|f| f.data_type.clone()),
            Some(_) | None => None,
        };
        field.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the value of a group-invariant column is of a row of it"))
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(Kept(First { first: None, written: !matches!(result, DataType::Extension(_)) }))
    }
}

/// The first row of one value given, kept as `First` keeps it: its value
/// is the result.
struct Kept(First);

impl Accumulator for Kept {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.0.update(v)
    }
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        self.0.update_many(values, rows)
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        self.0.write_state(out)
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        self.0.merge(state)
    }
    /// The value of the row kept; null of no row, a group of no rows.
    fn finish(&self) -> Result<Value> {
        let Some(Value::Row(fields)) = &self.0.first else { return Ok(Value::Null) };
        Ok(fields.first().cloned().unwrap_or(Value::Null))
    }
}

// Unit tests: the value kept, given values one at a time, by vectors and
// by states merged (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR7d ii, v] ANY_VALUE
    /// is null of no values and else the first given, one at a time, of a
    /// vector, nulls passed over, or of rows chosen; a state merged gives
    /// its value only to an accumulator of none.
    #[test]
    fn the_first_value_is_kept() {
        let fresh = || AnyValue.accumulator(DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer });
        let mut a = fresh();
        assert!(a.finish().unwrap().is_null());
        a.update(&int(3)).unwrap();
        a.update(&int(4)).unwrap();
        assert_eq!(a.finish().unwrap().norm(), int(3).norm());
        let mut b = fresh();
        b.update_many(&[Value::Null, int(5), int(6)], None).unwrap();
        assert_eq!(b.finish().unwrap().norm(), int(5).norm());
        let mut c = fresh();
        c.update_many(&[int(7), Value::Null, int(8)], Some(&[1, 2])).unwrap();
        c.update_many(&[int(9)], None).unwrap();
        assert_eq!(c.finish().unwrap().norm(), int(8).norm());
        let mut state = Vec::new();
        assert!(c.write_state(&mut state));
        let mut empty = fresh();
        empty.merge(&state).unwrap();
        assert_eq!(empty.finish().unwrap().norm(), int(8).norm());
        a.merge(&state).unwrap();
        assert_eq!(a.finish().unwrap().norm(), int(3).norm());
    }

    /// The row of one value `v`, as the group value is given it.
    fn row(v: Value) -> Value {
        Value::Row(vec![v].into())
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.7 GR1e] the value of
    /// a group-invariant column is its first row's, a null too, null of no
    /// rows, by one row at a time, by vectors and by states merged, a state
    /// kept only by an accumulator of none; its type is the field's.
    #[test]
    fn the_group_value_is_the_first_rows() {
        let t = DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer };
        let field = crate::value::types::Field { name: "C".to_owned(), data_type: t.clone(), unnamed: false };
        assert_eq!(GroupValue.result_type(Some(DataType::row(vec![field]))).unwrap(), t);
        assert_eq!(GroupValue.result_type(Some(t.clone())).unwrap_err().code, Code::Api);
        let fresh = || GroupValue.accumulator(t.clone());
        let mut a = fresh();
        assert!(a.finish().unwrap().is_null());
        a.update(&row(Value::Null)).unwrap();
        a.update(&row(int(4))).unwrap();
        assert!(a.finish().unwrap().is_null());
        let mut b = fresh();
        b.update_many(&[row(int(5)), row(int(6))], Some(&[1])).unwrap();
        assert_eq!(b.finish().unwrap().norm(), int(6).norm());
        let mut state = Vec::new();
        assert!(b.write_state(&mut state));
        let mut empty = fresh();
        empty.merge(&state).unwrap();
        assert_eq!(empty.finish().unwrap().norm(), int(6).norm());
        a.merge(&state).unwrap();
        assert!(a.finish().unwrap().is_null());
    }
}
