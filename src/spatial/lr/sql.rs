//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15, §21] Linear referencing
//! in SQL: the thirteen types as kinds of type built in (`sql/kind.rs`),
//! each routine of §15 as a function of its name (`sql/routines.rs`), a
//! method `v.m(args)` and a constructor `NEW T(args)` invoking the
//! function of its name with SELF first (RULES.md D87), and the
//! procedures and queries of the §21 catalog (`sql/catalog.rs`).
//!
//! The routines of a name are one function (`sql/dispatch.rs`), which
//! takes the routine whose parameters its arguments' declared types fit,
//! and, as it runs, the one whose parameters its values fit. A name a
//! geometry routine has too, ST_Point, is one function of both, the
//! geometry routine invoked of arguments no linear referencing routine
//! takes.

mod args;
mod catalog;
mod dispatch;
mod kind;
mod routine;
mod routines;

use std::sync::Arc;

pub use catalog::procedures;
pub use kind::{data_type_of, kinds, of_data_type};

use super::{Catalog, LrKind, LrValue, from_gml, from_text, from_text_or_gml, linear_units};
use crate::error::Result;
use crate::functions::ScalarFunction;
use crate::spatial::formats::gml::Srs;
use crate::system::SRS_SEED;
use crate::value::Value;
use args::{Args, text_of};

/// ST_MaxLRMNameLength [§15.1.1 DR3].
pub const LRM_NAME_LENGTH: u32 = 128;
/// ST_MaxUnitNameLength [§15.1.1 DR4], as ST_UNITS_OF_MEASURE's
/// UNIT_NAME has it [§20.4 DR1].
pub const UNIT_LENGTH: u32 = 64;
/// ST_MaxConstraintLength [§15.1.1 DR5].
pub const CONSTRAINT_LENGTH: u32 = 256;
/// The length of ST_LRMType and ST_LEType, CHARACTER VARYING(128)
/// [§15.1.1], and of a referent's type.
pub const TYPE_LENGTH: u32 = 128;
/// ST_MaxFeatureIDLength [§15.3.1 DR4].
pub const FEATURE_ID_LENGTH: u32 = 128;
/// The length of a referent's name and of a topology or network's.
pub const NAME_LENGTH: u32 = 128;
/// ST_MaxDescriptionLength, as §20.3's.
pub const DESCRIPTION_LENGTH: u32 = crate::system::SRS_DESCRIPTION_LENGTH;

/// The functions `existing` with the routines of §15, one function of
/// each name, which takes a function of `existing` of its name, a
/// geometry routine, as its own of other arguments.
pub fn with_functions(mut existing: Vec<Arc<dyn ScalarFunction>>) -> Vec<Arc<dyn ScalarFunction>> {
    let mut names: Vec<&'static str> = routines::all().map(|r| r.name).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let fallback = existing.iter().position(|f| f.name() == name).map(|at| existing.swap_remove(at));
        existing.push(Arc::new(dispatch::LrFunction::new(name, fallback)));
    }
    existing
}

/// The value of the stored form `bytes` of a linear referencing value.
pub fn stored_value(bytes: &[u8]) -> Option<Value> {
    LrValue::decode(bytes).ok().and_then(|v| kind::to_value(&v).ok())
}

/// [§20.3] The reference systems supplied, as a geometry of GML names one
/// in its srsName: by SRTEXT, by `AUTHORITY:CODE`.
struct SeededSrs;

impl Srs for SeededSrs {
    fn by_text(&self, text: &str) -> Option<i32> {
        SRS_SEED.iter().find(|s| s.definition == text).map(|s| s.id)
    }

    fn by_code(&self, authority: &str, code: &str) -> Option<i32> {
        SRS_SEED.iter().find(|s| s.organization.eq_ignore_ascii_case(authority) && s.id.to_string() == code).map(|s| s.id)
    }

    fn name(&self, srid: i32) -> Option<String> {
        SRS_SEED.iter().find(|s| s.id == srid).map(|s| format!("{}:{}", s.organization, s.id))
    }
}

/// The value of `text`, a well-known text or GML representation of a
/// value of `kind`, as a cast or a store assignment reads it: of the
/// units supplied and no catalog.
///
/// # Errors
///
/// As `from_text_or_gml`.
fn read_text(text: &str, kind: LrKind) -> Result<LrValue> {
    let units = linear_units();
    from_text_or_gml(text.trim_end_matches(' '), kind, &Catalog::of_units(&units), &SeededSrs)
}

/// The text of argument 0 of a null-call routine.
fn text_arg(a: &Args<'_>) -> Result<String> {
    let v = a.values.first().unwrap_or(&Value::Null);
    Ok(text_of(v)?.unwrap_or_default())
}

/// [§15.1.2 Description 2 and its peers] The constructor of one CHARACTER
/// LARGE OBJECT: the value of its well-known text or GML representation.
///
/// # Errors
///
/// As `from_text_or_gml`.
fn from_text_or_gml_arg(a: &Args<'_>, kind: LrKind) -> Result<Value> {
    kind::to_value(&from_text_or_gml(&text_arg(a)?, kind, &a.catalog, &SeededSrs)?)
}

/// [§15.1.11 and its peers] The function of the well-known text.
///
/// # Errors
///
/// As `from_text`.
fn from_text_arg(a: &Args<'_>, kind: LrKind) -> Result<Value> {
    kind::to_value(&from_text(&text_arg(a)?, kind, &a.catalog)?)
}

/// [§15.1.12 and its peers] The function of the GML representation.
///
/// # Errors
///
/// As `from_gml`.
fn from_gml_arg(a: &Args<'_>, kind: LrKind) -> Result<Value> {
    kind::to_value(&from_gml(&text_arg(a)?, kind, &a.catalog, &SeededSrs)?)
}
