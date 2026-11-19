//! Structured types, typed tables and views, SQL-invoked methods, and
//! user-defined casts, orderings and transforms, in bytes (RULES.md D81).
//!
//! ```text
//! structured type: name, supertype (flag, name), supertypes (count, each
//!                  schema and name str), instantiable, final (flags),
//!                  attributes (count, each name, type, default (flag, str),
//!                  collation (flag, name), defining type, observer and
//!                  mutator as routines), methods (count, each a method
//!                  specification), reference (u8 0 system, 1 user and a
//!                  type, 2 derived and its attributes), casts (flag, two
//!                  str), constructor (flag, routine), USAGE and UNDER grants
//! routine:         name, specific name (str), EXECUTE grants, stamps (as
//!                  a routine descriptor's)
//! method spec:     name, specific, kind (u8), overrides (flag, name),
//!                  parameters, result type, SELF AS RESULT, deterministic
//!                  (flags), access (u8), null call (flag), created (u64),
//!                  stamps
//! typed:           u8 0, or 1 and its type, self-referencing column (str),
//!                  generation (u8), supertable (flag, name)
//! method of:       u8 0, or 1 and its type and kind (u8)
//! cast:            source, target (types), function (name), assignment
//! ordering:        type, form (u8), category (u8) and function (name, or
//!                  flag and name of STATE)
//! transforms:      type, groups (count, each name, to-sql and from-sql
//!                  (flag, name))
//! ```

use super::super::{
    AttributeDesc, CastDesc, DataAccess, MethodSpecDesc, OrderingCategory, OrderingDesc, QualifiedName, RefRepresentation, RoutineDesc,
    StructuredDesc, TransformGroupDesc, TypedDesc,
};
use super::routines::{parameter, put_parameter};
use super::{flag, get_all, put_all, put_flag, put_name, put_usage, qualified, usage};
use crate::ast::{MethodKind, OrderingForm, RefGeneration};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::types::UdtName;
use crate::value::types::tags::{data_type, put_type};

#[cfg(test)]
mod tests;

/// The tag of nothing that may be there, or of its first kind.
const NONE: u8 = 0;
/// The tag of something that is there, or of a second kind.
const SOME: u8 = 1;
/// The tag of a third kind.
const THIRD: u8 = 2;

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

/// A routine a type's definition makes: its name, specific name and
/// EXECUTE grants.
pub(super) fn put_routine_desc(out: &mut Vec<u8>, r: &RoutineDesc) {
    out.put_str(&r.name);
    out.put_str(&r.specific);
    put_usage(out, &r.execute);
    super::routines::put_stamps(out, &r.stamps);
}

/// The routine `put_routine_desc` wrote.
pub(super) fn routine_desc(r: &mut Reader<'_>) -> Result<RoutineDesc> {
    Ok(RoutineDesc { name: r.str()?, specific: r.str()?, execute: usage(r)?, stamps: super::routines::stamps(r)? })
}

/// A method's kind.
fn put_kind(out: &mut Vec<u8>, kind: MethodKind) {
    out.put_u8(match kind {
        MethodKind::Instance => NONE,
        MethodKind::Static => SOME,
        MethodKind::Constructor => THIRD,
    });
}

/// The kind `put_kind` wrote.
fn kind(r: &mut Reader<'_>) -> Result<MethodKind> {
    match r.u8()? {
        NONE => Ok(MethodKind::Instance),
        SOME => Ok(MethodKind::Static),
        THIRD => Ok(MethodKind::Constructor),
        _ => Err(r.corrupt("not a kind of method")),
    }
}

/// An SQL-data access indication, as a routine's is written.
fn put_access(out: &mut Vec<u8>, access: DataAccess) {
    out.put_u8(match access {
        DataAccess::Contains => NONE,
        DataAccess::Reads => SOME,
        DataAccess::Modifies => THIRD,
    });
}

/// The indication `put_access` wrote.
fn access(r: &mut Reader<'_>) -> Result<DataAccess> {
    match r.u8()? {
        NONE => Ok(DataAccess::Contains),
        SOME => Ok(DataAccess::Reads),
        THIRD => Ok(DataAccess::Modifies),
        _ => Err(r.corrupt("not an SQL-data access indication")),
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.3] `t` in bytes.
pub(crate) fn put_structured_type(out: &mut Vec<u8>, t: &StructuredDesc) {
    put_name(out, &t.name);
    put_some_name(out, t.supertype.as_ref());
    put_all(out, t.supertypes.iter(), |out, s| {
        out.put_str(&s.schema);
        out.put_str(&s.name);
    });
    put_flag(out, t.instantiable);
    put_flag(out, t.is_final);
    put_all(out, t.attributes.iter(), put_attribute);
    put_all(out, t.methods.iter(), put_method);
    match &t.reference {
        RefRepresentation::System => out.put_u8(NONE),
        RefRepresentation::User(bt) => {
            out.put_u8(SOME);
            put_type(out, bt);
        }
        RefRepresentation::Derived(attributes) => {
            out.put_u8(THIRD);
            put_all(out, attributes.iter(), |out, a| out.put_str(a));
        }
    }
    put_flag(out, t.ref_casts.is_some());
    if let Some((to_ref, to_type)) = &t.ref_casts {
        out.put_str(to_ref);
        out.put_str(to_type);
    }
    put_flag(out, t.constructor.is_some());
    if let Some(c) = &t.constructor {
        put_routine_desc(out, c);
    }
    put_usage(out, &t.usage);
    put_usage(out, &t.under);
}

/// The structured type `put_structured_type` wrote.
pub(crate) fn structured_type(r: &mut Reader<'_>) -> Result<StructuredDesc> {
    let (name, supertype) = (qualified(r)?, some_name(r)?);
    let supertypes = get_all(r, |r| Ok(UdtName { schema: r.str()?, name: r.str()? }))?;
    let (instantiable, is_final) = (flag(r)?, flag(r)?);
    let attributes = get_all(r, attribute)?;
    let methods = get_all(r, method)?;
    let reference = match r.u8()? {
        NONE => RefRepresentation::System,
        SOME => RefRepresentation::User(data_type(r)?),
        THIRD => RefRepresentation::Derived(get_all(r, string)?),
        _ => return Err(r.corrupt("not a reference representation")),
    };
    let ref_casts = if flag(r)? { Some((r.str()?, r.str()?)) } else { None };
    let constructor = if flag(r)? { Some(routine_desc(r)?) } else { None };
    let (usage, under) = (usage(r)?, usage(r)?);
    Ok(StructuredDesc {
        name,
        supertype,
        supertypes,
        instantiable,
        is_final,
        attributes,
        methods,
        reference,
        ref_casts,
        constructor,
        usage,
        under,
    })
}

/// An attribute's descriptor.
fn put_attribute(out: &mut Vec<u8>, a: &AttributeDesc) {
    out.put_str(&a.name);
    put_type(out, &a.data_type);
    put_flag(out, a.default.is_some());
    if let Some(d) = &a.default {
        out.put_str(d);
    }
    put_some_name(out, a.collation.as_ref());
    put_name(out, &a.defined_by);
    put_routine_desc(out, &a.observer);
    put_routine_desc(out, &a.mutator);
}

/// The attribute `put_attribute` wrote.
fn attribute(r: &mut Reader<'_>) -> Result<AttributeDesc> {
    let (name, data_type) = (r.str()?, data_type(r)?);
    let default = if flag(r)? { Some(r.str()?) } else { None };
    let (collation, defined_by) = (some_name(r)?, qualified(r)?);
    let (observer, mutator) = (routine_desc(r)?, routine_desc(r)?);
    Ok(AttributeDesc { name, data_type, default, collation, defined_by, observer, mutator })
}

/// A method specification's descriptor.
pub(super) fn put_method(out: &mut Vec<u8>, m: &MethodSpecDesc) {
    out.put_str(&m.name);
    put_name(out, &m.specific);
    put_kind(out, m.kind);
    put_some_name(out, m.overrides.as_ref());
    put_all(out, m.parameters.iter(), put_parameter);
    put_type(out, &m.returns);
    put_flag(out, m.self_as_result);
    put_flag(out, m.deterministic);
    put_access(out, m.access);
    put_flag(out, m.null_call);
    out.put_u64(m.created);
    super::routines::put_stamps(out, &m.stamps);
}

/// The method specification `put_method` wrote.
pub(super) fn method(r: &mut Reader<'_>) -> Result<MethodSpecDesc> {
    let (name, specific, kind, overrides) = (r.str()?, qualified(r)?, kind(r)?, some_name(r)?);
    let (parameters, returns) = (get_all(r, parameter)?, data_type(r)?);
    let (self_as_result, deterministic, access, null_call) = (flag(r)?, flag(r)?, access(r)?, flag(r)?);
    let (created, stamps) = (r.u64()?, super::routines::stamps(r)?);
    Ok(MethodSpecDesc {
        name,
        specific,
        kind,
        overrides,
        parameters,
        returns,
        self_as_result,
        deterministic,
        access,
        null_call,
        created,
        stamps,
    })
}

/// What makes a table or view typed, if it is.
pub(super) fn put_typed(out: &mut Vec<u8>, typed: Option<&TypedDesc>) {
    let Some(t) = typed else {
        out.put_u8(NONE);
        return;
    };
    out.put_u8(SOME);
    put_name(out, &t.udt);
    out.put_str(&t.self_referencing);
    out.put_u8(match t.generation {
        RefGeneration::System => NONE,
        RefGeneration::User => SOME,
        RefGeneration::Derived => THIRD,
    });
    put_some_name(out, t.supertable.as_ref());
}

/// What `put_typed` wrote.
pub(super) fn typed(r: &mut Reader<'_>) -> Result<Option<TypedDesc>> {
    match r.u8()? {
        NONE => return Ok(None),
        SOME => {}
        _ => return Err(r.corrupt("not a typed table's flag")),
    }
    let (udt, self_referencing) = (qualified(r)?, r.str()?);
    let generation = match r.u8()? {
        NONE => RefGeneration::System,
        SOME => RefGeneration::User,
        THIRD => RefGeneration::Derived,
        _ => return Err(r.corrupt("not a reference generation")),
    };
    let supertable = some_name(r)?;
    Ok(Some(TypedDesc { udt, self_referencing, generation, supertable }))
}

/// The type a method is of, and its kind, if it is a method.
pub(super) fn put_method_of(out: &mut Vec<u8>, of: Option<&(QualifiedName, MethodKind)>) {
    let Some((udt, k)) = of else {
        out.put_u8(NONE);
        return;
    };
    out.put_u8(SOME);
    put_name(out, udt);
    put_kind(out, *k);
}

/// What `put_method_of` wrote.
pub(super) fn method_of(r: &mut Reader<'_>) -> Result<Option<(QualifiedName, MethodKind)>> {
    match r.u8()? {
        NONE => Ok(None),
        SOME => Ok(Some((qualified(r)?, kind(r)?))),
        _ => Err(r.corrupt("not a method's flag")),
    }
}

/// [ISO/IEC 9075-2:2023 §4.10] A user-defined cast.
pub(crate) fn put_cast(out: &mut Vec<u8>, c: &CastDesc) {
    put_type(out, &c.source);
    put_type(out, &c.target);
    put_name(out, &c.function);
    put_flag(out, c.assignment);
}

/// The cast `put_cast` wrote.
pub(crate) fn cast(r: &mut Reader<'_>) -> Result<CastDesc> {
    Ok(CastDesc { source: data_type(r)?, target: data_type(r)?, function: qualified(r)?, assignment: flag(r)? })
}

/// [ISO/IEC 9075-2:2023 §4.13] A user-defined ordering.
pub(crate) fn put_ordering(out: &mut Vec<u8>, o: &OrderingDesc) {
    put_name(out, &o.udt);
    put_flag(out, o.form == OrderingForm::Full);
    match &o.category {
        OrderingCategory::Relative(f) => {
            out.put_u8(NONE);
            put_name(out, f);
        }
        OrderingCategory::Map(f) => {
            out.put_u8(SOME);
            put_name(out, f);
        }
        OrderingCategory::State(f) => {
            out.put_u8(THIRD);
            put_some_name(out, f.as_ref());
        }
    }
}

/// The ordering `put_ordering` wrote.
pub(crate) fn ordering(r: &mut Reader<'_>) -> Result<OrderingDesc> {
    let udt = qualified(r)?;
    let form = if flag(r)? { OrderingForm::Full } else { OrderingForm::Equals };
    let category = match r.u8()? {
        NONE => OrderingCategory::Relative(qualified(r)?),
        SOME => OrderingCategory::Map(qualified(r)?),
        THIRD => OrderingCategory::State(some_name(r)?),
        _ => return Err(r.corrupt("not an ordering category")),
    };
    Ok(OrderingDesc { udt, form, category })
}

/// [ISO/IEC 9075-2:2023 §4.9.5] A type's transform groups.
pub(crate) fn put_transforms(out: &mut Vec<u8>, (udt, groups): (&QualifiedName, &[TransformGroupDesc])) {
    put_name(out, udt);
    put_all(out, groups.iter(), |out, g| {
        out.put_str(&g.name);
        put_some_name(out, g.to_sql.as_ref());
        put_some_name(out, g.from_sql.as_ref());
    });
}

/// The type and groups `put_transforms` wrote.
pub(crate) fn transforms(r: &mut Reader<'_>) -> Result<(QualifiedName, Vec<TransformGroupDesc>)> {
    let udt = qualified(r)?;
    let groups = get_all(r, |r| Ok(TransformGroupDesc { name: r.str()?, to_sql: some_name(r)?, from_sql: some_name(r)? }))?;
    Ok((udt, groups))
}

/// A string, read.
fn string(r: &mut Reader<'_>) -> Result<String> {
    r.str()
}
