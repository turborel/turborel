//! \[DESIGN\] Routine descriptors [ISO/IEC 9075-2:2023 §4.35.5] in the
//! catalog's bytes, as `catalog/codec.rs` writes the rest of its
//! descriptions.
//!
//! ```text
//! routine:    name, specific name, kind u8 (0 function, 1 procedure),
//!             parameters (count u32, each name (str), type, mode u8 (0
//!             IN, 1 OUT, 2 INOUT)), result type (0 none, or 1 and a
//!             type), deterministic u8, access u8 (0 CONTAINS SQL, 1 READS
//!             SQL DATA, 2 MODIFIES SQL DATA), null-call u8, text (str),
//!             body (str), schema (str), reads (count u32, names),
//!             horizon, invokes (count u32, names), sequences (count u32,
//!             names), execute (as a domain's usage), created u64,
//!             module (0 none, or 1 and its name (str))
//! module:     name, default schema (str), path (count u32, each a schema
//!             (str)), path as written (0 none, or 1 and str), temporary
//!             tables (count u32, names), text (str), execute (as a
//!             domain's usage), created u64
//! ```

use super::super::{DataAccess, ModuleDesc, ParameterMode, RoutineKind, SqlParameter, SqlRoutineDesc};
use super::{flag, get_all, horizon, put_all, put_flag, put_horizon, put_name, put_usage, qualified, usage};
use crate::bytes::{Put, Reader};
use crate::error::Result;
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
    out.put_u64(routine.created);
    put_flag(out, routine.module.is_some());
    if let Some(module) = &routine.module {
        out.put_str(module);
    }
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
    let (execute, created) = (usage(r)?, r.u64()?);
    let module = if flag(r)? { Some(r.str()?) } else { None };
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
        created,
        module,
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
    Ok(ModuleDesc { name, schema, path, path_text, temporaries, text, execute, created })
}

/// An SQL parameter: its name, declared type and mode.
fn put_parameter(out: &mut Vec<u8>, p: &SqlParameter) {
    out.put_str(&p.name);
    put_type(out, &p.data_type);
    out.put_u8(match p.mode {
        ParameterMode::In => IN,
        ParameterMode::Out => OUT,
        ParameterMode::InOut => INOUT,
    });
}

/// The SQL parameter `put_parameter` wrote.
fn parameter(r: &mut Reader<'_>) -> Result<SqlParameter> {
    let (name, data_type) = (r.str()?, data_type(r)?);
    let mode = match r.u8()? {
        IN => ParameterMode::In,
        OUT => ParameterMode::Out,
        INOUT => ParameterMode::InOut,
        _ => return Err(r.corrupt("not a parameter mode")),
    };
    Ok(SqlParameter { name, data_type, mode })
}

/// A schema's name of a module's path.
///
/// # Errors
///
/// SQLCODE -910 if the bytes are not one.
fn schema_name(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}
