//! The fields messages are made of: values, data types, columns, rows,
//! parameters and failures, each written and read as the module's format
//! says. Split from lib.rs (RULES.md R-SIZE-4).

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::params::ParameterType;
use turborel::storage::Row;
use turborel::value::types::{ExtensionType, TypeParams};
use turborel::value::{DataType, SqlType, Value, types};

use crate::{Column, Failure};

pub(crate) const NULL: u8 = 0;
pub(crate) const CHARACTER: u8 = 1;
pub(crate) const EXACT: u8 = 2;
const APPROXIMATE: u8 = 3;
/// A type or value beyond the standard, a plug-in's (RULES.md D53).
const EXTENSION: u8 = 4;
/// CHARACTER VARYING, as a type: its values are strings, as CHARACTER's.
const VARYING: u8 = 5;

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

/// A type: its kind's tag, then its parameters, as its kind writes them;
/// of a kind beyond the standard, its name and its parameters as written,
/// which a client reads without the kind.
fn put_type(b: &mut Vec<u8>, t: &DataType) {
    match t {
        DataType::Character { .. } => b.put_u8(CHARACTER),
        DataType::Varying { .. } => b.put_u8(VARYING),
        DataType::Exact { .. } => b.put_u8(EXACT),
        DataType::Approximate { .. } => b.put_u8(APPROXIMATE),
        DataType::Extension(e) => {
            b.put_u8(EXTENSION);
            b.put_str(e.kind.name());
            put_count(b, e.params.values().len());
            for p in e.params.values() {
                b.put_u32(*p);
            }
            return;
        }
    }
    t.kind().put_params(t, b);
}

/// A parameter of a type beyond the standard.
fn param(r: &mut Reader<'_>) -> Result<u32> {
    r.u32()
}

/// The kind of type or value of `tag`, one of the standard's.
fn kind_of(tag: u8) -> Option<&'static dyn SqlType> {
    match tag {
        CHARACTER => Some(&types::CHARACTER),
        EXACT => Some(&types::EXACT),
        APPROXIMATE => Some(&types::APPROXIMATE),
        VARYING => Some(&types::VARYING),
        _ => None,
    }
}

fn data_type(r: &mut Reader<'_>) -> Result<DataType> {
    let tag = r.u8()?;
    if tag == EXTENSION {
        let name = r.str()?;
        let params = counted(r, param)?;
        let params = TypeParams::of(&params).ok_or_else(|| r.corrupt("a type has too many parameters"))?;
        // A kind not registered where it is read, as a client's: its values
        // are shown as their text (`types::Unregistered`).
        let kind = r.types().and_then(|t| t.kind(&name)).unwrap_or(&types::UNREGISTERED_KIND);
        return Ok(DataType::Extension(ExtensionType { kind, params }));
    }
    kind_of(tag).ok_or_else(|| r.corrupt("no data type has this tag"))?.params(r)
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

/// A value: its kind's tag, then its bytes, as its kind writes them; of a
/// kind beyond the standard, its kind's name, the value as text and its
/// kind's bytes, so a client without the kind still shows it.
fn put_value(b: &mut Vec<u8>, v: &Value) {
    let tag = match v {
        Value::Null => NULL,
        Value::Char(_) => CHARACTER,
        Value::Exact(_) => EXACT,
        Value::Approx(_) => APPROXIMATE,
        Value::Extension(e) => {
            b.put_u8(EXTENSION);
            // One received unregistered goes back as it came.
            let (name, text, bytes) = types::parts(v).filter(|_| e.kind.name() == types::UNREGISTERED.name()).unwrap_or_else(|| {
                let mut bytes = Vec::new();
                e.kind.put_value(v, &mut bytes);
                (e.kind.name().to_owned(), e.kind.text(v), bytes)
            });
            b.put_str(&name);
            b.put_str(&text);
            b.put_bytes(&bytes);
            return;
        }
    };
    b.put_u8(tag);
    if let Some(kind) = v.kind() {
        kind.put_value(v, b);
    }
}

fn value(r: &mut Reader<'_>) -> Result<Value> {
    match r.u8()? {
        NULL => Ok(Value::Null),
        EXTENSION => {
            let (name, text, bytes) = (r.str()?, r.str()?, r.bytes()?);
            match r.types().and_then(|t| t.get(&name)) {
                Some(kind) => kind.value(&mut Reader::received(bytes, "a value")),
                None => Ok(types::unregistered(&name, &text, bytes)),
            }
        }
        tag => kind_of(tag).ok_or_else(|| r.corrupt("no value has this tag"))?.value(r),
    }
}

/// The bytes `row` takes in a Rows reply, or as an Execute's values.
pub fn row_size(row: &Row) -> usize {
    let values = row.iter().map(|v| match v {
        Value::Null => 1_usize,
        Value::Char(s) => s.len().saturating_add(5),
        Value::Exact(_) => 21,
        Value::Approx(_) => 9,
        Value::Extension(_) => {
            let mut b = Vec::new();
            put_value(&mut b, v);
            b.len()
        }
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
