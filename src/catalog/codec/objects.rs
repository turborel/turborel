//! \[DESIGN\] Collations, sequence generators and user-defined types in
//! the catalog's bytes, as `catalog/codec.rs` writes the rest of its
//! descriptions.
//!
//! ```text
//! collation:  name, base (str), pad u8 (0 PAD SPACE, 1 NO PAD), usage
//! sequence:   name, type, start i128, minimum i128, maximum i128,
//!             increment i128, cycle u8, epoch u64, origin i128, usage
//! user type:  name, source type, the routine to it, the routine to its
//!             source, usage
//! routine:    name (str), specific name (str), EXECUTE grants as usage
//! XML Schema: name, target namespace (str), schema location (str),
//!             document (str), the XML Schemas it uses (count u32, each
//!             a name), usage
//! usage:      as a domain's
//! ```

use super::super::{CollationDesc, RoutineDesc, SequenceDesc, UserTypeDesc, XmlSchemaDesc};
use super::{data_type, flag, put_flag, put_name, put_type, put_usage, qualified, usage};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::collation::Pad;

/// [ISO/IEC 9075-2:2023 §11.43] `collation` in bytes: its name, base, pad
/// and USAGE grants.
pub(crate) fn put_collation(out: &mut Vec<u8>, collation: &CollationDesc) {
    put_name(out, &collation.name);
    out.put_str(&collation.base);
    put_flag(out, collation.pad == Pad::None);
    put_usage(out, &collation.usage);
}

/// The collation `put_collation` wrote.
pub(crate) fn collation(r: &mut Reader<'_>) -> Result<CollationDesc> {
    let (name, base) = (qualified(r)?, r.str()?);
    let pad = if flag(r)? { Pad::None } else { Pad::Space };
    Ok(CollationDesc { name, base, pad, usage: usage(r)? })
}

/// [ISO/IEC 9075-14:2006 draft §4.2.6] `xml_schema` in bytes: its name,
/// target namespace, schema location and document, the registered XML
/// Schemas it uses, and its USAGE grants; not the schema read from it,
/// which is read again (RULES.md D86).
pub(crate) fn put_xml_schema(out: &mut Vec<u8>, xml_schema: &XmlSchemaDesc) {
    put_name(out, &xml_schema.name);
    out.put_str(&xml_schema.target_namespace);
    out.put_str(&xml_schema.location);
    out.put_str(&xml_schema.document);
    out.put_u32(u32::try_from(xml_schema.uses.len()).unwrap_or(u32::MAX));
    xml_schema.uses.iter().for_each(|u| put_name(out, u));
    put_usage(out, &xml_schema.usage);
}

/// The registered XML Schema `put_xml_schema` wrote, its schema not yet
/// read.
pub(crate) fn xml_schema(r: &mut Reader<'_>) -> Result<XmlSchemaDesc> {
    let (name, target_namespace, location, document) = (qualified(r)?, r.str()?, r.str()?, r.str()?);
    let uses = (0..r.u32()?).map(|_| qualified(r)).collect::<Result<_>>()?;
    Ok(XmlSchemaDesc { name, target_namespace, location, document, uses, usage: usage(r)?, schema: None })
}

/// [ISO/IEC 9075-2:2023 §4.29.1] `sequence` in bytes: its name, type,
/// values and cycle option, its counter's epoch and origin, and its USAGE
/// grants.
pub(crate) fn put_sequence(out: &mut Vec<u8>, sequence: &SequenceDesc) {
    put_name(out, &sequence.name);
    put_type(out, &sequence.data_type.clone());
    for v in [sequence.start, sequence.minimum, sequence.maximum, sequence.increment] {
        out.put_i128(v);
    }
    put_flag(out, sequence.cycle);
    out.put_u64(sequence.epoch);
    out.put_i128(sequence.origin);
    put_usage(out, &sequence.usage);
}

/// The sequence generator `put_sequence` wrote.
pub(crate) fn sequence(r: &mut Reader<'_>) -> Result<SequenceDesc> {
    let (name, data_type) = (qualified(r)?, data_type(r)?);
    let (start, minimum, maximum, increment) = (r.i128()?, r.i128()?, r.i128()?, r.i128()?);
    let (cycle, epoch, origin) = (flag(r)?, r.u64()?, r.i128()?);
    Ok(SequenceDesc { name, data_type, start, minimum, maximum, increment, cycle, epoch, origin, usage: usage(r)? })
}

/// [ISO/IEC 9075-2:2023 §4.9.2] `user_type` in bytes: its name, its source
/// type, its two routines, its USAGE grants, and its method
/// specifications.
pub(crate) fn put_user_type(out: &mut Vec<u8>, user_type: &UserTypeDesc) {
    put_name(out, &user_type.name);
    put_type(out, &user_type.source);
    for routine in [&user_type.to_distinct, &user_type.to_source] {
        out.put_str(&routine.name);
        out.put_str(&routine.specific);
        put_usage(out, &routine.execute);
    }
    put_usage(out, &user_type.usage);
    super::put_all(out, user_type.methods.iter(), super::structured::put_method);
}

/// The user-defined type `put_user_type` wrote.
pub(crate) fn user_type(r: &mut Reader<'_>) -> Result<UserTypeDesc> {
    let (name, source) = (qualified(r)?, data_type(r)?);
    let routine = |from: &mut Reader<'_>| -> Result<RoutineDesc> {
        Ok(RoutineDesc { name: from.str()?, specific: from.str()?, execute: usage(from)? })
    };
    let (to_distinct, to_source) = (routine(r)?, routine(r)?);
    let usage = usage(r)?;
    Ok(UserTypeDesc { name, source, to_distinct, to_source, usage, methods: super::get_all(r, super::structured::method)? })
}
