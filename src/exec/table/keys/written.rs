//! \[DESIGN\] (RULES.md D1226) The keys of a table's groups written to a
//! spill and read back as they were, lane by lane: whether the lane is
//! grouped unpadded, its nulls, and its vector of its type, each value's
//! bits as it holds them, a null's place too, so a lane read back is the
//! lane written. A lane of values writes each with `codec::put_value` and
//! its key's bytes; of a large object, an XML value, a value of a type
//! beyond the standard, or one that holds others, it writes nothing, as
//! what keeps such a value apart, or the types it is read by, are not the
//! spill's, and the table's groups stay where they are.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::codec::{put_value, value};
use crate::value::datetime::Moment;
use crate::value::{Datetime, Value};

use super::{Data, Keys, Lane, text_at};

/// The tags of a lane's types, as written.
const UNSET: u8 = 0;
const INTS: u8 = 1;
const FLOATS: u8 = 2;
const DATETIMES: u8 = 3;
const BOOLS: u8 = 4;
const TEXTS: u8 = 5;
const VALUES: u8 = 6;

/// Whether `value` is written by its bytes alone, read back the same.
pub(in crate::exec::table) fn written_alone(value: &Value) -> bool {
    match value {
        Value::Null
        | Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Reference(_)
        | Value::Json(_) => true,
        Value::Lob(_)
        | Value::Xml(_)
        | Value::Extension(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_) => false,
    }
}

/// Appends `d`, every field of it.
fn put_datetime(out: &mut Vec<u8>, d: &Datetime) {
    out.put_u8(match d.moment {
        Moment::Date => 0,
        Moment::Time => 1,
        Moment::Timestamp => 2,
    });
    out.put_u64(d.utc.cast_unsigned());
    out.put_u32(u32::from(d.zone.cast_unsigned()));
    out.put_u8(u8::from(d.zoned));
    out.put_u8(d.precision);
}

/// Reads a datetime `put_datetime` wrote.
fn datetime(reader: &mut Reader<'_>) -> Result<Datetime> {
    let moment = match reader.u8()? {
        0 => Moment::Date,
        1 => Moment::Time,
        2 => Moment::Timestamp,
        tag => return Err(reader.corrupt(&format!("no datetime's fields have tag {tag}"))),
    };
    let utc = reader.u64()?.cast_signed();
    let zone = u16::try_from(reader.u32()?).map_err(|_| reader.corrupt("a time zone past 16 bits"))?.cast_signed();
    let (zoned, precision) = (reader.u8()? != 0, reader.u8()?);
    Ok(Datetime { moment, utc, zone, zoned, precision })
}

/// The value at `g` of `items`, or what a missing one reads as.
fn at<T: Copy>(items: &[T], g: usize, missing: T) -> T {
    items.get(g).copied().unwrap_or(missing)
}

impl Lane {
    /// Appends the groups at `places`: whether it could write them.
    fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) -> bool {
        let groups = || places.iter().map(|&i| crate::storage::mask::index(i));
        out.put_u8(u8::from(self.unpadded));
        groups().for_each(|g| out.put_u8(u8::from(self.is_null(g))));
        match &self.data {
            Data::Unset => out.put_u8(UNSET),
            Data::Ints { units, scale } => {
                out.put_u8(INTS);
                out.put_u32(*scale);
                groups().for_each(|g| out.put_u64(at(units, g, 0).cast_unsigned()));
            }
            Data::Floats(numbers) => {
                out.put_u8(FLOATS);
                groups().for_each(|g| out.put_u64(at(numbers, g, 0.0_f64).to_bits()));
            }
            Data::Datetimes(datetimes) => {
                out.put_u8(DATETIMES);
                groups().for_each(|g| put_datetime(out, &at(datetimes, g, super::NO_DATETIME)));
            }
            Data::Bools(bools) => {
                out.put_u8(BOOLS);
                groups().for_each(|g| out.put_u8(u8::from(at(bools, g, false))));
            }
            Data::Texts { text, ends } => {
                out.put_u8(TEXTS);
                groups().for_each(|g| out.put_str(text_at(text, ends, g).unwrap_or("")));
            }
            Data::Values { values, keys, ends } => {
                out.put_u8(VALUES);
                for g in groups() {
                    let Some(v) = values.get(g).filter(|v| written_alone(v)) else { return false };
                    put_value(out, v);
                    out.put_bytes(text_at_bytes(keys, ends, g));
                }
            }
        }
        true
    }

    /// Reads a lane of `len` groups `put_groups` wrote.
    fn read(reader: &mut Reader<'_>, len: usize) -> Result<Lane> {
        let unpadded = reader.u8()? != 0;
        let nulls = (0..len).map(|_| reader.u8().map(|b| b != 0)).collect::<Result<Vec<_>>>()?;
        let data = match reader.u8()? {
            UNSET => Data::Unset,
            INTS => {
                let scale = reader.u32()?;
                Data::Ints { units: (0..len).map(|_| reader.u64().map(u64::cast_signed)).collect::<Result<_>>()?, scale }
            }
            FLOATS => Data::Floats((0..len).map(|_| reader.u64().map(f64::from_bits)).collect::<Result<_>>()?),
            DATETIMES => Data::Datetimes((0..len).map(|_| datetime(reader)).collect::<Result<_>>()?),
            BOOLS => Data::Bools((0..len).map(|_| reader.u8().map(|b| b != 0)).collect::<Result<_>>()?),
            TEXTS => {
                let (mut text, mut ends) = (String::new(), Vec::with_capacity(len));
                for _ in 0..len {
                    text.push_str(&reader.str()?);
                    ends.push(text.len());
                }
                Data::Texts { text, ends }
            }
            VALUES => {
                let (mut values, mut keys, mut ends) = (Vec::with_capacity(len), Vec::new(), Vec::with_capacity(len));
                for _ in 0..len {
                    values.push(value(reader)?);
                    keys.extend_from_slice(reader.bytes()?);
                    ends.push(keys.len());
                }
                Data::Values { values, keys, ends }
            }
            tag => return Err(reader.corrupt(&format!("no lane of keys has tag {tag}"))),
        };
        let nulled = nulls.contains(&true);
        let mut lane = Lane { unpadded, data, nulls, nulled, heap: 0, room: 0 };
        lane.heap = lane.held();
        Ok(lane)
    }
}

/// The bytes from the end before `g` to the `g`th end of `ends` in `keys`.
fn text_at_bytes<'k>(keys: &'k [u8], ends: &[usize], g: usize) -> &'k [u8] {
    let from = g.checked_sub(1).map_or(Some(0), |before| ends.get(before).copied());
    from.zip(ends.get(g)).and_then(|(from, &to)| keys.get(from..to)).unwrap_or(&[])
}

impl Keys {
    /// Appends the keys of the groups at `places`, lane by lane: whether
    /// every lane could write them.
    pub(in crate::exec) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) -> bool {
        out.put_u32(u32::try_from(self.lanes.len()).unwrap_or(u32::MAX));
        self.lanes.iter().all(|lane| lane.put_groups(places, out))
    }

    /// Reads the keys of `len` groups `put_groups` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -901 if the bytes are not keys so written.
    pub(in crate::exec) fn read(reader: &mut Reader<'_>, len: usize) -> Result<Keys> {
        let width = crate::storage::mask::index(reader.u32()?);
        Ok(Keys { lanes: (0..width).map(|_| Lane::read(reader, len)).collect::<Result<_>>()? })
    }
}

// Unit tests: every lane read back as written (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
