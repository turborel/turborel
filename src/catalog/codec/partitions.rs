//! \[EXTENSION\] Partitioning descriptors (RULES.md D153) in the catalog's
//! bytes, as `catalog/codec.rs` writes the rest of its descriptions.
//!
//! ```text
//! partitioning: next id u64, count u32, each a descriptor, by its
//!               table's name
//! descriptor:   table name, key (0 none, or 1 and a key), of (0 none,
//!               or 1, the parent's name, a bound, id u64)
//! key:          strategy (str), positions, expressions, templates
//!               (count u32, each a template)
//! expressions:  count u32, each place u32 and text (str)
//! template:     name (str), positions, method (str), unique u8,
//!               options, expressions, nulls u8
//! bound:        0 default | 1 range, from and to (count u32, each a
//!               datum) | 2 list (count u32, each count u32 of values)
//!               | 3 with, options
//! datum:        0 a value | 1 MINVALUE | 2 MAXVALUE
//! ```

use super::super::{IndexTemplate, PartitionBound, PartitionDesc, PartitionKey, PartitionOf, Partitioning, RangeDatum};
use super::{count, flag, get_all, options, position, positions, put_all, put_flag, put_name, put_options, put_positions, qualified};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::codec::{put_value, value};

/// The tags of a bound's kinds.
const DEFAULT: u8 = 0;
/// FROM ... TO ...
const RANGE: u8 = 1;
/// IN (...)
const LIST: u8 = 2;
/// WITH (...)
const WITH: u8 = 3;
/// The tags of a range datum's kinds: a value's is 0.
const VALUE: u8 = 0;
/// MINVALUE.
const MIN_VALUE: u8 = 1;
/// MAXVALUE.
const MAX_VALUE: u8 = 2;

/// `partitioning` in bytes: every descriptor.
pub(crate) fn put_partitioning(out: &mut Vec<u8>, partitioning: &Partitioning) {
    out.put_u64(partitioning.next_id());
    let all: Vec<&PartitionDesc> = partitioning.all().collect();
    put_all(out, all.into_iter(), put_desc);
}

/// The descriptors `put_partitioning` wrote.
///
/// # Errors
///
/// SQLCODE -916 if the bytes are not them.
pub(crate) fn partitioning(r: &mut Reader<'_>) -> Result<Partitioning> {
    let mut out = Partitioning::default();
    out.set_next_id(r.u64()?);
    for d in get_all(r, desc)? {
        out.put(d);
    }
    Ok(out)
}

/// One descriptor in bytes.
fn put_desc(out: &mut Vec<u8>, d: &PartitionDesc) {
    put_name(out, &d.table);
    put_flag(out, d.key.is_some());
    if let Some(key) = &d.key {
        out.put_str(&key.strategy);
        put_positions(out, key.columns.iter());
        put_expressions(out, &key.expressions);
        put_all(out, key.templates.iter(), put_template);
    }
    put_flag(out, d.of.is_some());
    if let Some(of) = &d.of {
        put_name(out, &of.parent);
        put_bound(out, &of.bound);
        out.put_u64(of.id);
    }
}

/// The descriptor `put_desc` wrote.
fn desc(r: &mut Reader<'_>) -> Result<PartitionDesc> {
    let table = qualified(r)?;
    let key = if flag(r)? {
        let (strategy, columns, expressions) = (r.str()?, positions(r)?, expressions(r)?);
        Some(PartitionKey { strategy, columns, expressions, templates: get_all(r, template)? })
    } else {
        None
    };
    let of = if flag(r)? { Some(PartitionOf { parent: qualified(r)?, bound: bound(r)?, id: r.u64()? }) } else { None };
    Ok(PartitionDesc { table, key, of })
}

/// Key parts that are expressions in bytes.
fn put_expressions(out: &mut Vec<u8>, expressions: &[(usize, String)]) {
    put_all(out, expressions.iter(), |out, (part, text)| {
        out.put_u32(count(*part));
        out.put_str(text);
    });
}

/// The expressions `put_expressions` wrote.
fn expressions(r: &mut Reader<'_>) -> Result<Vec<(usize, String)>> {
    get_all(r, |r| Ok((position(r)?, r.str()?)))
}

/// An index template in bytes.
fn put_template(out: &mut Vec<u8>, t: &IndexTemplate) {
    out.put_str(&t.name);
    put_positions(out, t.columns.iter());
    out.put_str(&t.method);
    put_flag(out, t.unique);
    put_options(out, &t.options);
    put_expressions(out, &t.expressions);
    put_flag(out, t.nulls);
}

/// The template `put_template` wrote.
fn template(r: &mut Reader<'_>) -> Result<IndexTemplate> {
    let (name, columns, method, unique, options) = (r.str()?, positions(r)?, r.str()?, flag(r)?, options(r)?);
    let expressions = expressions(r)?;
    Ok(IndexTemplate { name, columns, method, unique, options, expressions, nulls: flag(r)? })
}

/// A bound in bytes.
pub(crate) fn put_bound(out: &mut Vec<u8>, bound: &PartitionBound) {
    match bound {
        PartitionBound::Default => out.put_u8(DEFAULT),
        PartitionBound::Range { from, to } => {
            out.put_u8(RANGE);
            put_all(out, from.iter(), put_datum);
            put_all(out, to.iter(), put_datum);
        }
        PartitionBound::List(keys) => {
            out.put_u8(LIST);
            put_all(out, keys.iter(), |out, key| put_all(out, key.iter(), put_value));
        }
        PartitionBound::With(options) => {
            out.put_u8(WITH);
            put_options(out, options);
        }
    }
}

/// The bound `put_bound` wrote.
pub(crate) fn bound(r: &mut Reader<'_>) -> Result<PartitionBound> {
    Ok(match r.u8()? {
        DEFAULT => PartitionBound::Default,
        RANGE => PartitionBound::Range { from: get_all(r, datum)?, to: get_all(r, datum)? },
        LIST => PartitionBound::List(get_all(r, |r| get_all(r, value))?),
        WITH => PartitionBound::With(options(r)?),
        _ => return Err(r.corrupt("a partition bound of no kind")),
    })
}

/// A range datum in bytes.
fn put_datum(out: &mut Vec<u8>, d: &RangeDatum) {
    match d {
        RangeDatum::Value(v) => {
            out.put_u8(VALUE);
            put_value(out, v);
        }
        RangeDatum::MinValue => out.put_u8(MIN_VALUE),
        RangeDatum::MaxValue => out.put_u8(MAX_VALUE),
    }
}

/// The datum `put_datum` wrote.
fn datum(r: &mut Reader<'_>) -> Result<RangeDatum> {
    Ok(match r.u8()? {
        VALUE => RangeDatum::Value(value(r)?),
        MIN_VALUE => RangeDatum::MinValue,
        MAX_VALUE => RangeDatum::MaxValue,
        _ => return Err(r.corrupt("a range datum of no kind")),
    })
}
