//! \[DESIGN\] Trigger descriptors [ISO/IEC 9075-2:2023 §4.46.1] in the
//! catalog's bytes, as `catalog/codec.rs` writes the rest of its
//! descriptions.
//!
//! ```text
//! trigger:    name, table name, time u8 (0 BEFORE, 1 AFTER, 2 INSTEAD OF),
//!             level u8 (0 ROW, 1 STATEMENT), event u8 (0 INSERT, 1 DELETE,
//!             2 UPDATE, then its columns, count u32, each str), the old
//!             row, new row, old table and new table names (each 0, or 1
//!             and str), text (str), schema (str), reads (count u32,
//!             names), horizon, columns (count u32, each a name and str),
//!             sequences (count u32, names), created u64, its timestamp
//!             of creation as a routine's instant
//! ```

use super::super::{ActionTime, TransitionNames, TriggerDesc, TriggerEvent, TriggerLevel};
use super::{flag, get_all, horizon, put_all, put_flag, put_horizon, put_name, qualified};
use crate::bytes::{Put, Reader};
use crate::error::Result;

#[cfg(test)]
mod tests;

// The tags of the trigger action times.

/// BEFORE's tag.
const BEFORE: u8 = 0;
/// AFTER's tag.
const AFTER: u8 = 1;
/// INSTEAD OF's tag.
const INSTEAD_OF: u8 = 2;

// The tags of the trigger levels.

/// FOR EACH ROW's tag.
const ROW: u8 = 0;
/// FOR EACH STATEMENT's tag.
const STATEMENT: u8 = 1;

// The tags of the trigger events.

/// INSERT's tag.
const INSERT: u8 = 0;
/// DELETE's tag.
const DELETE: u8 = 1;
/// UPDATE's tag, its trigger column list after it.
const UPDATE: u8 = 2;

/// [ISO/IEC 9075-2:2023 §4.46.1] `trigger` in bytes: every part of its
/// descriptor.
pub(crate) fn put_trigger(out: &mut Vec<u8>, trigger: &TriggerDesc) {
    put_name(out, &trigger.name);
    put_name(out, &trigger.table);
    out.put_u8(match trigger.time {
        ActionTime::Before => BEFORE,
        ActionTime::After => AFTER,
        ActionTime::InsteadOf => INSTEAD_OF,
    });
    out.put_u8(match trigger.level {
        TriggerLevel::Row => ROW,
        TriggerLevel::Statement => STATEMENT,
    });
    match &trigger.event {
        TriggerEvent::Insert => out.put_u8(INSERT),
        TriggerEvent::Delete => out.put_u8(DELETE),
        TriggerEvent::Update(columns) => {
            out.put_u8(UPDATE);
            put_all(out, columns.iter(), |out, c| out.put_str(c));
        }
    }
    let names = &trigger.names;
    for name in [&names.old_row, &names.new_row, &names.old_table, &names.new_table] {
        put_optional(out, name.as_deref());
    }
    out.put_str(&trigger.text);
    out.put_str(&trigger.schema);
    put_all(out, trigger.reads.iter(), put_name);
    put_horizon(out, &trigger.horizon);
    put_all(out, trigger.columns.iter(), |out, (table, column)| {
        put_name(out, table);
        out.put_str(column);
    });
    put_all(out, trigger.sequences.iter(), put_name);
    out.put_u64(trigger.created);
    super::routines::put_instant(out, trigger.created_at);
}

/// The trigger descriptor `put_trigger` wrote.
///
/// # Errors
///
/// SQLCODE -916 if it is not one: cut short, or a tag none of these.
pub(crate) fn trigger(r: &mut Reader<'_>) -> Result<TriggerDesc> {
    let (name, table) = (qualified(r)?, qualified(r)?);
    let time = match r.u8()? {
        BEFORE => ActionTime::Before,
        AFTER => ActionTime::After,
        INSTEAD_OF => ActionTime::InsteadOf,
        _ => return Err(r.corrupt("not a trigger action time")),
    };
    let level = match r.u8()? {
        ROW => TriggerLevel::Row,
        STATEMENT => TriggerLevel::Statement,
        _ => return Err(r.corrupt("not a trigger level")),
    };
    let event = match r.u8()? {
        INSERT => TriggerEvent::Insert,
        DELETE => TriggerEvent::Delete,
        UPDATE => TriggerEvent::Update(get_all(r, text)?),
        _ => return Err(r.corrupt("not a trigger event")),
    };
    let (old_row, new_row, old_table, new_table) = (optional(r)?, optional(r)?, optional(r)?, optional(r)?);
    let names = TransitionNames { old_row, new_row, old_table, new_table };
    let (text, schema, reads, horizon) = (r.str()?, r.str()?, get_all(r, qualified)?, horizon(r)?);
    let (columns, sequences) = (get_all(r, |r| Ok((qualified(r)?, r.str()?)))?, get_all(r, qualified)?);
    let (created, created_at) = (r.u64()?, super::routines::instant(r)?);
    Ok(TriggerDesc { name, table, time, level, event, names, text, schema, reads, horizon, columns, sequences, created, created_at })
}

/// A name that may be absent: 0, or 1 and the name.
pub(super) fn put_optional(out: &mut Vec<u8>, name: Option<&str>) {
    put_flag(out, name.is_some());
    if let Some(name) = name {
        out.put_str(name);
    }
}

/// A string, as `put_str` writes it.
fn text(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}

/// The name `put_optional` wrote, if any.
pub(super) fn optional(r: &mut Reader<'_>) -> Result<Option<String>> {
    Ok(if flag(r)? { Some(r.str()?) } else { None })
}
