//! The item method `keyvalue()` evaluated [ISO/IEC 9075-2:2023 §9.46
//! GR11g ii 6 C VIII]: of each object, an object of each of its members,
//! of the members `key`, `value` and `id`.
//!
//! Implementation-defined and -dependent choices: (IA072) an object of two
//! members of one key raises no exception, and (UA058) none of them is
//! deleted, as the wildcard member accessor deletes none; (US040) the
//! members are taken in the order they are kept. The `id` of the members of
//! an object Ij, "an implementation-defined exact numeric value of scale 0
//! (zero) that uniquely identifies Ij" \[VIII 1 c i\], is \[DESIGN\] its
//! place in the base, counted from 0: unique among the objects of one
//! `keyvalue()`, not among those of two.
//!
//! As printed, \[VIII 1 c vi\] defines OBJh, 1 ≤ h ≤ q, of Ki and BVi; the
//! key and the bound value of the h-th member are meant.

use super::super::{count, exception};
use crate::error::{Result, SqlState};
use crate::json::{Item, Member};

// [§9.46 GR11g ii 6 C VIII 1 c vi] The keys of the members of each object
// `keyvalue()` gives.
/// Of the key.
const KEY: &str = "key";
/// Of the bound value.
const VALUE: &str = "value";
/// Of the object's identifier.
const ID: &str = "id";

/// [§9.46 GR11g ii 6 C VIII] `keyvalue()` of each item of `base`: the
/// objects of the members of each, in turn.
///
/// # Errors
///
/// SQLSTATE 2203C if an item is no object \[VIII 1 a\], in either mode.
pub(super) fn keyvalue(base: &[Item]) -> Result<Vec<Item>> {
    let mut pairs = Vec::new();
    for (place, each) in base.iter().enumerate() {
        let Some(members) = each.as_object() else {
            return Err(exception(
                SqlState::SQL_JSON_OBJECT_NOT_FOUND,
                "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VIII 1 a] an SQL/JSON item of keyvalue() is not an object",
            ));
        };
        let id = count(place);
        pairs.extend(members.iter().map(|member| {
            Item::Object(vec![
                Member { key: KEY.to_owned(), value: Item::Scalar(member.key.clone().into()) },
                Member { key: VALUE.to_owned(), value: member.value.clone() },
                Member { key: ID.to_owned(), value: id.clone() },
            ])
        }));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests;
