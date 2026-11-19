//! \[EXTENSION\] A plug-in's partitioning strategy (RULES.md D169), a
//! `PartitionStrategy` each of whose questions is one CALL. Bounds and
//! restrictions cross as protocol 1 writes them:
//!
//! ```text
//! bound       := u8 0 DEFAULT | 1 RANGE data data | 2 LIST rows | 3 WITH count u32 (str value)...
//! data        := count u32, each u8 0 value | 1 MINVALUE | 2 MAXVALUE
//! restriction := u8 0 none | 1 equal values | 2 range end end | 3 IS NULL
//! end         := u8 0 | u8 1 value inclusive u8
//! ```

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::plugin::errors::violation;
use crate::plugin::payload::{
    PARTITION_ADMITS, PARTITION_BOUND, PARTITION_CHECK_KEY, PARTITION_CONTAINS, PARTITION_OVERLAPS, PARTITION_ROUTE, put_strings,
};
use crate::plugin::values::{put_value, put_values, value};
use crate::plugin::wire::{put_rows, rows};
use crate::value::{DataType, Value};

use super::pending::{PartitionBound, PartitionStrategy, RangeDatum, Restriction};
use super::{Link, answer};

/// A plug-in's strategy.
#[derive(Debug)]
pub struct PluginStrategy {
    link: Link,
}

impl PluginStrategy {
    /// The strategy `link` names.
    pub fn new(link: Link) -> PluginStrategy {
        PluginStrategy { link }
    }

    /// Operation `op`, of the payload `put` writes.
    fn ask(&self, op: u8, put: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> Result<Vec<u8>> {
        let mut payload = Vec::new();
        put(&mut payload)?;
        self.link.plain(op, payload)
    }
}

/// The names of `types`, as a program reads them.
fn type_names(types: &[DataType]) -> Vec<String> {
    types.iter().map(ToString::to_string).collect()
}

/// `b`, written.
///
/// # Errors
///
/// SQLSTATE 38T03 of a value no protocol 1 carries.
pub fn put_bound(out: &mut Vec<u8>, b: &PartitionBound) -> Result<()> {
    match b {
        PartitionBound::Default => out.put_u8(0),
        PartitionBound::Range { from, to } => {
            out.put_u8(1);
            put_data(out, from)?;
            put_data(out, to)?;
        }
        PartitionBound::List(keys) => {
            out.put_u8(2);
            put_rows(out, keys)?;
        }
        PartitionBound::With(named) => {
            out.put_u8(3);
            out.put_u32(u32::try_from(named.len()).unwrap_or(u32::MAX));
            for (name, v) in named {
                out.put_str(name);
                put_value(out, v)?;
            }
        }
    }
    Ok(())
}

/// The ends of a range, written.
fn put_data(out: &mut Vec<u8>, data: &[RangeDatum]) -> Result<()> {
    out.put_u32(u32::try_from(data.len()).unwrap_or(u32::MAX));
    for d in data {
        match d {
            RangeDatum::Value(v) => {
                out.put_u8(0);
                put_value(out, v)?;
            }
            RangeDatum::MinValue => out.put_u8(1),
            RangeDatum::MaxValue => out.put_u8(2),
        }
    }
    Ok(())
}

/// A bound, read.
///
/// # Errors
///
/// SQLSTATE 38T03 of a form no bound's.
pub fn bound(r: &mut Reader<'_>) -> Result<PartitionBound> {
    Ok(match r.u8()? {
        0 => PartitionBound::Default,
        1 => PartitionBound::Range { from: data(r)?, to: data(r)? },
        2 => PartitionBound::List(rows(r)?),
        3 => PartitionBound::With((0..r.u32()?).map(|_| Ok((r.str()?, value(r)?))).collect::<Result<_>>()?),
        other => return Err(violation(&format!("no bound is of form {other}"))),
    })
}

/// The ends of a range, read.
fn data(r: &mut Reader<'_>) -> Result<Vec<RangeDatum>> {
    (0..r.u32()?)
        .map(|_| match r.u8()? {
            0 => Ok(RangeDatum::Value(value(r)?)),
            1 => Ok(RangeDatum::MinValue),
            2 => Ok(RangeDatum::MaxValue),
            other => Err(violation(&format!("no end of a range is of form {other}"))),
        })
        .collect()
}

/// `restriction`, written.
fn put_restriction(out: &mut Vec<u8>, restriction: Option<&Restriction>) -> Result<()> {
    match restriction {
        None => out.put_u8(0),
        Some(Restriction::Equal(values)) => {
            out.put_u8(1);
            put_values(out, values)?;
        }
        Some(Restriction::Range { low, high }) => {
            out.put_u8(2);
            for end in [low, high] {
                put_end(out, end.as_ref())?;
            }
        }
        Some(Restriction::Null) => out.put_u8(3),
    }
    Ok(())
}

/// An end of a restriction's range, written.
fn put_end(out: &mut Vec<u8>, end: Option<&(Value, bool)>) -> Result<()> {
    let Some((v, inclusive)) = end else {
        out.put_u8(0);
        return Ok(());
    };
    out.put_u8(1);
    put_value(out, v)?;
    out.put_u8(u8::from(*inclusive));
    Ok(())
}

/// A truth answered.
fn truth(got: &[u8]) -> Result<bool> {
    answer(got, super::byte).map(|b| b != 0)
}

impl PartitionStrategy for PluginStrategy {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn check_key(&self, types: &[DataType]) -> Result<()> {
        let got = self.ask(PARTITION_CHECK_KEY, |p| {
            put_strings(p, &type_names(types));
            Ok(())
        })?;
        answer(&got, |_| Ok(()))
    }
    fn bound(&self, types: &[DataType], spec: &PartitionBound) -> Result<PartitionBound> {
        let got = self.ask(PARTITION_BOUND, |p| {
            put_strings(p, &type_names(types));
            put_bound(p, spec)
        })?;
        answer(&got, bound)
    }
    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool> {
        truth(&self.ask(PARTITION_OVERLAPS, |p| put_bound(p, a).and_then(|()| put_bound(p, b)))?)
    }
    fn contains(&self, b: &PartitionBound, key: &[Value]) -> Result<bool> {
        truth(&self.ask(PARTITION_CONTAINS, |p| put_bound(p, b).and_then(|()| put_values(p, key)))?)
    }
    fn route(&self, bounds: &[&PartitionBound], key: &[Value]) -> Result<Option<usize>> {
        let got = self.ask(PARTITION_ROUTE, |p| {
            p.put_u32(u32::try_from(bounds.len()).unwrap_or(u32::MAX));
            bounds.iter().try_for_each(|b| put_bound(p, b))?;
            put_values(p, key)
        })?;
        let at = answer(&got, |r| if r.u8()? == 0 { Ok(None) } else { r.u32().map(Some) })?;
        let at = at.map(|a| usize::try_from(a).unwrap_or(usize::MAX));
        if let Some(a) = at.filter(|a| *a >= bounds.len()) {
            return Err(violation(&format!("it routed to sibling {a} of {}", bounds.len())));
        }
        Ok(at)
    }
    fn admits(&self, b: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool> {
        truth(&self.ask(PARTITION_ADMITS, |p| {
            put_bound(p, b)?;
            p.put_u32(u32::try_from(restrictions.len()).unwrap_or(u32::MAX));
            restrictions.iter().try_for_each(|r| put_restriction(p, r.as_ref()))
        })?)
    }
}
