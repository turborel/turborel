//! \[DESIGN\] Columns in the catalog's bytes, as `catalog/codec.rs` lays
//! them out: each column's name, type, nullability, default, domain,
//! collation, and how its values are generated.

use super::super::{ColumnDefault, ColumnDesc, ColumnDomain, Generated, Generation, Identity, QualifiedName};
use super::{data_type, flag, horizon, put_flag, put_horizon, put_name, put_sequence, put_type, qualified, sequence};
use crate::ast::UserFunction;
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::codec::{put_value, value};
use crate::value::datetime::Moment;

/// `column` in bytes, as the module of `catalog/codec.rs` lays it out.
pub(super) fn put_column(out: &mut Vec<u8>, column: &ColumnDesc) {
    out.put_str(&column.name);
    put_type(out, &column.data_type.clone());
    put_flag(out, column.not_null);
    put_default(out, column.default.as_ref());
    // Its domain, 1 then its name and default, or 0 for none.
    match &column.domain {
        None => out.put_u8(0),
        Some(ColumnDomain { name, default }) => {
            out.put_u8(1);
            put_name(out, name);
            put_default(out, default.as_ref());
        }
    }
    put_collation_name(out, column.collation.as_ref());
    put_generated(out, column.generated.as_ref());
}

/// [ISO/IEC 9075-2:2023 §11.4 GR5f, GR5j] How a column's values are
/// generated: 0 for a base column that is no identity column; 1, ALWAYS u8
/// and its sequence generator, for an identity column; 2, the text and its
/// names' schema, for a generated column; 3 and its sequence generator, for
/// a system-generated self-referencing column; 4, the count and names of
/// its derivational attributes, for a derived one.
fn put_generated(out: &mut Vec<u8>, generated: Option<&Generated>) {
    match generated {
        None => out.put_u8(0),
        Some(Generated::Identity(Identity { always, sequence })) => {
            out.put_u8(1);
            put_flag(out, *always);
            put_sequence(out, sequence);
        }
        Some(Generated::Expression(Generation { text, names, horizon })) => {
            out.put_u8(2);
            out.put_str(text);
            out.put_str(names);
            put_horizon(out, horizon);
        }
        Some(Generated::SystemReference(sequence)) => {
            out.put_u8(3);
            put_sequence(out, sequence);
        }
        Some(Generated::DerivedReference(names)) => {
            out.put_u8(4);
            out.put_u32(u32::try_from(names.len()).unwrap_or(u32::MAX));
            for n in names {
                out.put_str(n);
            }
        }
    }
}

/// What `put_generated` wrote.
fn generated(r: &mut Reader<'_>) -> Result<Option<Generated>> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(Generated::Identity(Identity { always: flag(r)?, sequence: sequence(r)? })),
        2 => Some(Generated::Expression(Generation { text: r.str()?, names: r.str()?, horizon: horizon(r)? })),
        3 => Some(Generated::SystemReference(sequence(r)?)),
        4 => {
            let n = r.u32()?;
            Some(Generated::DerivedReference((0..n).map(|_| r.str()).collect::<Result<_>>()?))
        }
        _ => return Err(r.corrupt("not how a column's values are generated")),
    })
}

/// A declared type collation: 0 for none, or 1 then its name.
pub(super) fn put_collation_name(out: &mut Vec<u8>, name: Option<&QualifiedName>) {
    put_flag(out, name.is_some());
    if let Some(name) = name {
        put_name(out, name);
    }
}

/// The name `put_collation_name` wrote.
pub(super) fn collation_name(r: &mut Reader<'_>) -> Result<Option<QualifiedName>> {
    if flag(r)? { qualified(r).map(Some) } else { Ok(None) }
}

/// A default: 0 for none, 1 and a value, 2 for a user's, or 3 for a
/// datetime value function's: 0 CURRENT_DATE, 1 CURRENT_TIME or 2
/// CURRENT_TIMESTAMP, and its precision.
pub(crate) fn put_default(out: &mut Vec<u8>, default: Option<&ColumnDefault>) {
    match default {
        None => out.put_u8(0),
        Some(ColumnDefault::Value(v)) => {
            out.put_u8(1);
            put_value(out, v);
        }
        Some(ColumnDefault::User(f)) => {
            out.put_u8(2);
            out.put_u8(match f {
                UserFunction::Current => 0,
                UserFunction::Session => 1,
                UserFunction::System => 2,
                UserFunction::Role => 3,
            });
        }
        Some(ColumnDefault::Datetime(moment, precision)) => {
            out.put_u8(3);
            out.put_u8(match moment {
                Moment::Date => 0,
                Moment::Time => 1,
                Moment::Timestamp => 2,
            });
            out.put_u32(*precision);
        }
    }
}

/// The default `put_default` wrote.
pub(crate) fn default(r: &mut Reader<'_>) -> Result<Option<ColumnDefault>> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(ColumnDefault::Value(value(r)?)),
        2 => Some(ColumnDefault::User(match r.u8()? {
            0 => UserFunction::Current,
            1 => UserFunction::Session,
            2 => UserFunction::System,
            3 => UserFunction::Role,
            _ => return Err(r.corrupt("not a user value function")),
        })),
        3 => {
            let moment = match r.u8()? {
                0 => Moment::Date,
                1 => Moment::Time,
                2 => Moment::Timestamp,
                _ => return Err(r.corrupt("not a datetime value function")),
            };
            Some(ColumnDefault::Datetime(moment, r.u32()?))
        }
        _ => return Err(r.corrupt("not a column default")),
    })
}

/// The column `put_column` wrote.
pub(super) fn column(r: &mut Reader<'_>) -> Result<ColumnDesc> {
    let (name, data_type, not_null) = (r.str()?, data_type(r)?, flag(r)?);
    let own = default(r)?;
    let domain = if flag(r)? { Some(ColumnDomain { name: qualified(r)?, default: default(r)? }) } else { None };
    let collation = collation_name(r)?;
    Ok(ColumnDesc { name, data_type, not_null, default: own, domain, collation, generated: generated(r)? })
}
