//! \[DESIGN\] A family of types built in, each a kind of type beyond the
//! standard (RULES.md D53), ordered by a type hierarchy as a structured
//! type's subtypes are [ISO/IEC 9075-2:2023 §4.9.5] (RULES.md
//! D-SPATIAL-FAMILY): SQL/MM Spatial's 27 geometry types [ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §4.2]. Its values are of one payload,
//! which says their most specific type; a value of a type is a value of
//! each of its supertypes.

use std::fmt;

use super::{Kind, SqlType};
use crate::value::{DataType, Value};

/// A family of kinds of type in a hierarchy.
pub trait Family: Send + Sync + fmt::Debug {
    /// Whether `sub` is `sup` or a subtype of it.
    fn is_subtype(&self, sub: &dyn SqlType, sup: &dyn SqlType) -> bool;

    /// The kind of type of the family named `name`, written as a type is.
    fn named(&self, name: &str) -> Option<Kind>;

    /// The most specific type of `v`, a value of the family.
    fn most_specific(&self, v: &Value) -> Option<Kind>;

    /// The direct supertype of `kind`, none of the family's root.
    fn supertype(&self, kind: &dyn SqlType) -> Option<Kind>;

    /// Whether `kind` is instantiable [ISO/IEC 9075-2:2023 §11.51 SR8].
    fn instantiable(&self, kind: &dyn SqlType) -> bool;

    /// [ISO/IEC 9075-2:2023 §11.65] The name of the function the family
    /// is ordered by: EQUALS ONLY BY RELATIVE, `f(x, y)` 0 of values
    /// equal.
    fn relative_ordering(&self) -> &'static str;

    /// The kinds of the family, the root first.
    fn kinds(&self) -> &'static [Kind];
}

/// The family of `t`, an extension type of one.
pub fn family_of(t: &DataType) -> Option<(&'static dyn Family, &'static dyn SqlType)> {
    let DataType::Extension(e) = t else { return None };
    let kind: &'static dyn SqlType = *e.kind;
    kind.family().map(|f| (f, kind))
}

/// [ISO/IEC 9075-2:2023 §9.3 SR3g] The least common supertype of
/// `types`, all of one family: of the parameters they share, none if they
/// differ. None if one is of no family or of another.
pub fn common_supertype(types: &[DataType]) -> Option<DataType> {
    let (family, first) = family_of(types.first()?)?;
    let kinds: Vec<&'static dyn SqlType> =
        types.iter().map(|t| family_of(t).filter(|(f, _)| std::ptr::addr_eq(*f, family)).map(|(_, k)| k)).collect::<Option<_>>()?;
    let mut at: &'static dyn SqlType = first;
    while !kinds.iter().all(|k| family.is_subtype(*k, at)) {
        at = *family.supertype(at)?;
    }
    let params_of = |t: &DataType| if let DataType::Extension(e) = t { Some(e.params) } else { None };
    let shared = params_of(types.first()?)?;
    let params = if types.iter().all(|t| params_of(t) == Some(shared)) { shared } else { super::TypeParams::default() };
    let kind = family.kinds().iter().copied().find(|k| k.name() == at.name())?;
    Some(DataType::Extension(super::ExtensionType { kind, params }))
}

/// Whether a value of type `source` may be assigned to a site of type
/// `target`, both of one family: `source` is `target` or a subtype of it
/// [ISO/IEC 9075-2:2023 §9.2 SR2]. None if either is of no family.
pub fn assignable(target: &DataType, source: &DataType) -> Option<bool> {
    let ((f, to), (g, from)) = (family_of(target)?, family_of(source)?);
    Some(std::ptr::addr_eq(f, g) && f.is_subtype(from, to))
}
