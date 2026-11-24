//! \[DESIGN\] Routine descriptors [ISO/IEC 9075-2:2023 §4.35.5] in the
//! catalog's bytes, as `catalog/codec.rs` writes the rest of its
//! descriptions.
//!
//! ```text
//! routine:    name, specific name, kind u8 (0 function, 1 procedure),
//!             parameters (count u32, each a parameter), result type (0 none, or 1 and a
//!             type), deterministic u8, access u8 (0 CONTAINS SQL, 1 READS
//!             SQL DATA, 2 MODIFIES SQL DATA), null-call u8, text (str),
//!             body (str), schema (str), reads (count u32, names),
//!             horizon, invokes (count u32, names), sequences (count u32,
//!             names), execute (as a domain's usage), debug (as a
//!             domain's usage, RULES.md D1000), created u64,
//!             stamps, module (0 none, or 1 and its name (str)), PTF (0 none, or
//!             1 and a ptf), method, distance (0 none, or 1 and a distance)
//! distance:   symmetric u8, non-negative u8, triangle u8, formats (count
//!             u32, each a code u8), expression (str)
//! parameter:  name (str), type, mode u8 (0 IN, 1 OUT, 2 INOUT), generic
//!             u8 (0 none, 1 a generic table parameter, 2 a descriptor
//!             parameter), and of a generic table parameter pass through
//!             u8 and semantics u8 (0 row, 1 set KEEP ON EMPTY, 2 set
//!             PRUNE ON EMPTY)
//! ptf:        columns (0 none, or 1 and count u32, each name (str) and
//!             type), only pass through u8, private parameters (count u32,
//!             parameters), describe, start (each 0 none, or 1 and a
//!             name), fulfill (a name), finish (0 none, or 1 and a name)
//! module:     name, default schema (str), path (count u32, each a schema
//!             (str)), path as written (0 none, or 1 and str), temporary
//!             tables (count u32, names), text (str), execute (as a
//!             domain's usage), created u64, stamps
//! stamps:     the creation timestamp, then the last-altered timestamp,
//!             each an instant
//! instant:    microseconds from 0001-01-01 00:00 UTC i128, time zone
//!             displacement in minutes i128
//! ```

use super::super::{
    DataAccess, DistanceDesc, GenericParameter, GenericTable, ModuleDesc, ParameterMode, PtfDesc, QualifiedName, RoutineKind, SqlParameter,
    SqlRoutineDesc, Stamps, TableSemantics,
};
use super::{flag, get_all, horizon, put_all, put_flag, put_horizon, put_name, put_usage, qualified, usage};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::datetime::Now;
use crate::value::types::tags::{data_type, put_type};

#[cfg(test)]
mod tests;

// The tags of the kinds of routine.

/// A function's tag.
const FUNCTION: u8 = 0;
/// A procedure's tag.
const PROCEDURE: u8 = 1;

// The tags of the parameter modes.

/// IN's tag.
const IN: u8 = 0;
/// OUT's tag.
const OUT: u8 = 1;
/// INOUT's tag.
const INOUT: u8 = 2;

// The tags of the SQL-data access indications.

// The tags of what a parameter of no <data type> is.

/// A parameter of a `<data type>`'s tag.
const DATA: u8 = 0;
/// A generic table parameter's tag.
const TABLE: u8 = 1;
/// A descriptor parameter's tag.
const DESCRIPTOR: u8 = 2;

// The tags of a generic table parameter's semantics.

/// WITH ROW SEMANTICS's tag.
const ROW: u8 = 0;
/// WITH SET SEMANTICS KEEP ON EMPTY's tag.
const KEEP: u8 = 1;
/// WITH SET SEMANTICS PRUNE ON EMPTY's tag.
const PRUNE: u8 = 2;

/// CONTAINS SQL's tag.
const CONTAINS: u8 = 0;
/// READS SQL DATA's tag.
const READS: u8 = 1;
/// MODIFIES SQL DATA's tag.
const MODIFIES: u8 = 2;

/// [ISO/IEC 9075-2:2023 §4.35.5] `routine` in bytes: every part of its
/// descriptor.
pub(crate) fn put_routine(out: &mut Vec<u8>, routine: &SqlRoutineDesc) {
    put_name(out, &routine.name);
    put_name(out, &routine.specific);
    out.put_u8(match routine.kind {
        RoutineKind::Function => FUNCTION,
        RoutineKind::Procedure => PROCEDURE,
    });
    put_all(out, routine.parameters.iter(), put_parameter);
    put_flag(out, routine.returns.is_some());
    if let Some(returns) = &routine.returns {
        put_type(out, returns);
    }
    put_flag(out, routine.deterministic);
    out.put_u8(match routine.access {
        DataAccess::Contains => CONTAINS,
        DataAccess::Reads => READS,
        DataAccess::Modifies => MODIFIES,
    });
    put_flag(out, routine.null_call);
    out.put_str(&routine.text);
    out.put_str(&routine.body);
    out.put_str(&routine.schema);
    put_all(out, routine.reads.iter(), put_name);
    put_horizon(out, &routine.horizon);
    put_all(out, routine.sequences.iter(), put_name);
    put_usage(out, &routine.execute);
    put_usage(out, &routine.debug);
    out.put_u64(routine.created);
    put_stamps(out, &routine.stamps);
    put_flag(out, routine.module.is_some());
    if let Some(module) = &routine.module {
        out.put_str(module);
    }
    put_flag(out, routine.ptf.is_some());
    if let Some(ptf) = &routine.ptf {
        put_ptf(out, ptf);
    }
    super::structured::put_method_of(out, routine.method.as_ref());
    put_flag(out, routine.distance.is_some());
    if let Some(d) = &routine.distance {
        put_flag(out, d.symmetric);
        put_flag(out, d.non_negative);
        put_flag(out, d.triangle);
        out.put_u32(u32::try_from(d.formats.len()).unwrap_or(u32::MAX));
        d.formats.iter().for_each(|f| out.put_u8(*f));
        out.put_str(&d.expression);
    }
}

/// \[EXTENSION\] A distance function's declarations, as `put_routine`
/// writes them.
fn distance(r: &mut Reader<'_>) -> Result<DistanceDesc> {
    let (symmetric, non_negative, triangle) = (flag(r)?, flag(r)?, flag(r)?);
    let count = r.u32()?;
    let formats = (0..count).map(|_| r.u8()).collect::<Result<Vec<u8>>>()?;
    Ok(DistanceDesc { symmetric, non_negative, triangle, formats, expression: r.str()? })
}

/// [ISO/IEC 9075-2:2023 §4.35.4] What the descriptor of a polymorphic
/// table function has more, in bytes.
fn put_ptf(out: &mut Vec<u8>, ptf: &PtfDesc) {
    put_flag(out, ptf.columns.is_some());
    if let Some(columns) = &ptf.columns {
        put_all(out, columns.iter(), |out, (name, t)| {
            out.put_str(name);
            put_type(out, t);
        });
    }
    put_flag(out, ptf.only_pass_through);
    put_all(out, ptf.private.iter(), put_parameter);
    put_some_name(out, ptf.describe.as_ref());
    put_some_name(out, ptf.start.as_ref());
    put_name(out, &ptf.fulfill);
    put_some_name(out, ptf.finish.as_ref());
}

/// The `PtfDesc` `put_ptf` wrote.
fn ptf(r: &mut Reader<'_>) -> Result<PtfDesc> {
    let columns = if flag(r)? { Some(get_all(r, |r| Ok((r.str()?, data_type(r)?)))?) } else { None };
    let only_pass_through = flag(r)?;
    let private = get_all(r, parameter)?;
    let (describe, start) = (some_name(r)?, some_name(r)?);
    let (fulfill, finish) = (qualified(r)?, some_name(r)?);
    Ok(PtfDesc { columns, only_pass_through, private, describe, start, fulfill, finish })
}

/// A name that may be none.
fn put_some_name(out: &mut Vec<u8>, name: Option<&QualifiedName>) {
    put_flag(out, name.is_some());
    if let Some(name) = name {
        put_name(out, name);
    }
}

/// The name `put_some_name` wrote, if one.
fn some_name(r: &mut Reader<'_>) -> Result<Option<QualifiedName>> {
    if flag(r)? { qualified(r).map(Some) } else { Ok(None) }
}

/// The routine descriptor `put_routine` wrote.
///
/// # Errors
///
/// SQLCODE -916 if it is not one: cut short, or a tag none of these.
pub(crate) fn routine(r: &mut Reader<'_>) -> Result<SqlRoutineDesc> {
    let (name, specific) = (qualified(r)?, qualified(r)?);
    let kind = match r.u8()? {
        FUNCTION => RoutineKind::Function,
        PROCEDURE => RoutineKind::Procedure,
        _ => return Err(r.corrupt("not a kind of routine")),
    };
    let parameters = get_all(r, parameter)?;
    let returns = if flag(r)? { Some(data_type(r)?) } else { None };
    let deterministic = flag(r)?;
    let access = match r.u8()? {
        CONTAINS => DataAccess::Contains,
        READS => DataAccess::Reads,
        MODIFIES => DataAccess::Modifies,
        _ => return Err(r.corrupt("not an SQL-data access indication")),
    };
    let null_call = flag(r)?;
    let (text, body, schema) = (r.str()?, r.str()?, r.str()?);
    let (reads, horizon, sequences) = (get_all(r, qualified)?, horizon(r)?, get_all(r, qualified)?);
    let (execute, debug) = (usage(r)?, usage(r)?);
    let (created, stamps) = (r.u64()?, stamps(r)?);
    let module = if flag(r)? { Some(r.str()?) } else { None };
    let ptf = if flag(r)? { Some(Box::new(ptf(r)?)) } else { None };
    let method = super::structured::method_of(r)?;
    let distance = if flag(r)? { Some(Box::new(distance(r)?)) } else { None };
    Ok(SqlRoutineDesc {
        name,
        specific,
        kind,
        parameters,
        returns,
        deterministic,
        access,
        null_call,
        text,
        body,
        schema,
        reads,
        horizon,
        sequences,
        execute,
        debug,
        created,
        stamps,
        module,
        method,
        ptf,
        distance,
    })
}

/// [ISO/IEC 9075-4:2006 draft §9.21 GR3] `module` in bytes: every part of
/// its descriptor.
pub(crate) fn put_module(out: &mut Vec<u8>, module: &ModuleDesc) {
    put_name(out, &module.name);
    out.put_str(&module.schema);
    put_all(out, module.path.iter(), |out, s| out.put_str(s));
    put_flag(out, module.path_text.is_some());
    if let Some(text) = &module.path_text {
        out.put_str(text);
    }
    put_all(out, module.temporaries.iter(), put_name);
    out.put_str(&module.text);
    put_usage(out, &module.execute);
    out.put_u64(module.created);
    put_stamps(out, &module.stamps);
}

/// The module descriptor `put_module` wrote.
///
/// # Errors
///
/// SQLCODE -910 if the bytes are not a module descriptor.
pub(crate) fn module(r: &mut Reader<'_>) -> Result<ModuleDesc> {
    let (name, schema) = (qualified(r)?, r.str()?);
    let path = get_all(r, schema_name)?;
    let path_text = if flag(r)? { Some(r.str()?) } else { None };
    let (temporaries, text, execute, created) = (get_all(r, qualified)?, r.str()?, usage(r)?, r.u64()?);
    Ok(ModuleDesc { name, schema, path, path_text, temporaries, text, execute, created, stamps: stamps(r)? })
}

/// [ISO/IEC 9075-2:2023 §4.35.5] A descriptor's creation and last-altered
/// timestamps.
pub(super) fn put_stamps(out: &mut Vec<u8>, stamps: &Stamps) {
    put_instant(out, stamps.created);
    put_instant(out, stamps.last_altered);
}

/// The timestamps `put_stamps` wrote.
pub(super) fn stamps(r: &mut Reader<'_>) -> Result<Stamps> {
    Ok(Stamps { created: instant(r)?, last_altered: instant(r)? })
}

/// A timestamp: its instant and its time zone displacement.
pub(super) fn put_instant(out: &mut Vec<u8>, now: Now) {
    out.put_i128(i128::from(now.utc));
    out.put_i128(i128::from(now.zone));
}

/// The timestamp `put_instant` wrote.
pub(super) fn instant(r: &mut Reader<'_>) -> Result<Now> {
    let utc = i64::try_from(r.i128()?).map_err(|_| r.corrupt("an instant is out of range"))?;
    let zone = i16::try_from(r.i128()?).map_err(|_| r.corrupt("a time zone displacement is out of range"))?;
    Ok(Now { utc, zone })
}

/// An SQL parameter: its name, declared type, mode, kind and default.
pub(super) fn put_parameter(out: &mut Vec<u8>, p: &SqlParameter) {
    out.put_str(&p.name);
    put_type(out, &p.data_type);
    out.put_u8(match p.mode {
        ParameterMode::In => IN,
        ParameterMode::Out => OUT,
        ParameterMode::InOut => INOUT,
    });
    match p.generic {
        None => out.put_u8(DATA),
        Some(GenericParameter::Descriptor) => out.put_u8(DESCRIPTOR),
        Some(GenericParameter::Table(t)) => {
            out.put_u8(TABLE);
            put_flag(out, t.pass_through);
            out.put_u8(match t.semantics {
                TableSemantics::Row => ROW,
                TableSemantics::Set { prune: false } => KEEP,
                TableSemantics::Set { prune: true } => PRUNE,
            });
        }
    }
    super::triggers::put_optional(out, p.default.as_deref());
}

/// The SQL parameter `put_parameter` wrote.
pub(super) fn parameter(r: &mut Reader<'_>) -> Result<SqlParameter> {
    let (name, data_type) = (r.str()?, data_type(r)?);
    let mode = match r.u8()? {
        IN => ParameterMode::In,
        OUT => ParameterMode::Out,
        INOUT => ParameterMode::InOut,
        _ => return Err(r.corrupt("not a parameter mode")),
    };
    let generic = match r.u8()? {
        DATA => None,
        DESCRIPTOR => Some(GenericParameter::Descriptor),
        TABLE => {
            let pass_through = flag(r)?;
            let semantics = match r.u8()? {
                ROW => TableSemantics::Row,
                KEEP => TableSemantics::Set { prune: false },
                PRUNE => TableSemantics::Set { prune: true },
                _ => return Err(r.corrupt("not a generic table semantics")),
            };
            Some(GenericParameter::Table(GenericTable { pass_through, semantics }))
        }
        _ => return Err(r.corrupt("not what a parameter is")),
    };
    let default = super::triggers::optional(r)?;
    Ok(SqlParameter { name, data_type, mode, generic, default })
}

/// A schema's name of a module's path.
///
/// # Errors
///
/// SQLCODE -910 if the bytes are not one.
fn schema_name(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}
