//! The fields messages are made of: values, data types, columns, rows,
//! parameters and failures, each written and read as the module's format
//! says. Split from lib.rs (RULES.md R-SIZE-4).

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::params::ParameterType;
use turborel::storage::Row;
use turborel::value::{DataType, Decimal, Value};

use crate::{Column, Failure};

pub(crate) const NULL: u8 = 0;
pub(crate) const CHARACTER: u8 = 1;
pub(crate) const EXACT: u8 = 2;
const APPROXIMATE: u8 = 3;

pub(crate) fn put_count(b: &mut Vec<u8>, n: usize) {
    b.put_u32(u32::try_from(n).unwrap_or(u32::MAX));
}

/// `count` items read by `item`, after their count.
pub(crate) fn counted<T>(r: &mut Reader<'_>, item: fn(&mut Reader<'_>) -> Result<T>) -> Result<Vec<T>> {
    let count = r.u32()?;
    // Not reserved for `count`: what is held grows only with items read,
    // so a false count cannot make the reader hold more than it was sent.
    let mut items = Vec::new();
    for _ in 0..count {
        items.push(item(r)?);
    }
    Ok(items)
}

pub(crate) fn put_flag(b: &mut Vec<u8>, set: bool) {
    b.put_u8(u8::from(set));
}

pub(crate) fn flag(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(r.corrupt("a flag is neither 0 nor 1")),
    }
}

pub(crate) fn put_strs(b: &mut Vec<u8>, strs: &[String]) {
    put_count(b, strs.len());
    for s in strs {
        b.put_str(s);
    }
}

pub(crate) fn strs(r: &mut Reader<'_>) -> Result<Vec<String>> {
    counted(r, string)
}

fn string(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}

fn put_type(b: &mut Vec<u8>, t: &DataType) {
    match t {
        DataType::Character { length } => {
            b.put_u8(CHARACTER);
            b.put_u32(*length);
        }
        DataType::Exact { precision, scale } => {
            b.put_u8(EXACT);
            b.put_u32(*precision);
            b.put_u32(*scale);
        }
        DataType::Approximate { precision } => {
            b.put_u8(APPROXIMATE);
            b.put_u32(*precision);
        }
    }
}

fn data_type(r: &mut Reader<'_>) -> Result<DataType> {
    match r.u8()? {
        CHARACTER => Ok(DataType::Character { length: r.u32()? }),
        EXACT => Ok(DataType::Exact { precision: r.u32()?, scale: r.u32()? }),
        APPROXIMATE => Ok(DataType::Approximate { precision: r.u32()? }),
        _ => Err(r.corrupt("no data type has this tag")),
    }
}

pub(crate) fn put_column(b: &mut Vec<u8>, c: &Column) {
    put_flag(b, c.name.is_some());
    if let Some(name) = &c.name {
        b.put_str(name);
    }
    put_type(b, &c.data_type);
    put_flag(b, c.not_null);
}

pub(crate) fn column(r: &mut Reader<'_>) -> Result<Column> {
    let name = if flag(r)? { Some(r.str()?) } else { None };
    Ok(Column { name, data_type: data_type(r)?, not_null: flag(r)? })
}

/// [ISO/IEC 9075:1992 §17.9 GR4] A dynamic parameter's type: CHARACTER
/// VARYING, of the longest length, flagged.
pub(crate) fn put_parameter(b: &mut Vec<u8>, p: &ParameterType) {
    put_flag(b, *p == ParameterType::Varying);
    put_type(b, &p.data_type());
}

pub(crate) fn parameter(r: &mut Reader<'_>) -> Result<ParameterType> {
    let varying = flag(r)?;
    let t = data_type(r)?;
    Ok(if varying { ParameterType::Varying } else { ParameterType::Of(t) })
}

fn put_value(b: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => b.put_u8(NULL),
        Value::Char(s) => {
            b.put_u8(CHARACTER);
            b.put_str(s);
        }
        Value::Exact(d) => {
            b.put_u8(EXACT);
            b.put_i128(d.units);
            b.put_u32(d.scale);
        }
        Value::Approx(f) => {
            b.put_u8(APPROXIMATE);
            b.put_u64(f.to_bits());
        }
    }
}

fn value(r: &mut Reader<'_>) -> Result<Value> {
    match r.u8()? {
        NULL => Ok(Value::Null),
        CHARACTER => Ok(Value::Char(r.str()?)),
        EXACT => Ok(Value::Exact(Decimal::new(r.i128()?, r.u32()?))),
        APPROXIMATE => Ok(Value::Approx(f64::from_bits(r.u64()?))),
        _ => Err(r.corrupt("no value has this tag")),
    }
}

/// The bytes `row` takes in a Rows reply, or as an Execute's values.
pub fn row_size(row: &Row) -> usize {
    let values = row.iter().map(|v| match v {
        Value::Null => 1_usize,
        Value::Char(s) => s.len().saturating_add(5),
        Value::Exact(_) => 21,
        Value::Approx(_) => 9,
    });
    values.fold(4, usize::saturating_add)
}

/// A row, or an Execute's values: their count, then each.
pub(crate) fn put_row(b: &mut Vec<u8>, row: &[Value]) {
    put_count(b, row.len());
    for v in row {
        put_value(b, v);
    }
}

pub(crate) fn row(r: &mut Reader<'_>) -> Result<Row> {
    counted(r, value)
}

pub(crate) fn put_failure(b: &mut Vec<u8>, f: &Failure) {
    b.put_u32(f.sqlcode.cast_unsigned());
    b.put_str(&f.sqlstate);
    b.put_str(&f.message);
    put_flag(b, f.rule.is_some());
    if let Some(rule) = &f.rule {
        b.put_str(rule);
    }
    put_flag(b, f.position.is_some());
    if let Some(position) = f.position {
        b.put_u64(position);
    }
}

pub(crate) fn failure(r: &mut Reader<'_>) -> Result<Failure> {
    let sqlcode = r.u32()?.cast_signed();
    let sqlstate = r.str()?;
    let message = r.str()?;
    let rule = if flag(r)? { Some(r.str()?) } else { None };
    let position = if flag(r)? { Some(r.u64()?) } else { None };
    Ok(Failure { sqlcode, sqlstate, message, rule, position })
}
