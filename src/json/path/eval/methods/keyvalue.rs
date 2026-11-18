//! The item method `keyvalue()` evaluated [ISO/IEC 9075-2:2023 §9.46
//! GR11g ii 6 C VIII]: of each object, an object of each of its members,
//! of the members `key`, `value` and `id`.
//!
//! Implementation-defined and -dependent choices: (IA072) an object of two
//! members of one key raises no exception, and (UA058) none of them is
//! deleted, as the wildcard member accessor deletes none; (US040) the
//! members are taken in the order they are kept. The `id` of the members of
//! an object Ij, "an implementation-defined exact numeric value of scale 0
//! (zero) that uniquely identifies Ij" \[VIII 1 c i\], is \[DESIGN\] the
//! evaluator's identity of it (`eval.rs`, `Identity`): its place in the
//! context item, or a variable, counted from 0, each item before those
//! within it, so an object of theirs has one `id` however the accessors
//! reach it, and two objects have two. An object the path made, the
//! objects `keyvalue()` gives and the values copied into them among them,
//! has an `id` past every place, a new one each time.
//!
//! As printed, \[VIII 1 c vi\] defines OBJh, 1 ≤ h ≤ q, of Ki and BVi; the
//! key and the bound value of the h-th member are meant.

use std::borrow::Cow;

use super::super::exception;
use crate::error::{Result, SqlState};
use crate::json::{Item, Member};
use crate::value::Decimal;

// [§9.46 GR11g ii 6 C VIII 1 c vi] The keys of the members of each object
// `keyvalue()` gives.
/// Of the key.
const KEY: &str = "key";
/// Of the bound value.
const VALUE: &str = "value";
/// Of the object's identifier.
const ID: &str = "id";

/// [§9.46 GR11g ii 6 C VIII] `keyvalue()` of each item of `base`: the
/// objects of the members of each, in turn, of `identity` the `id` of
/// each object.
///
/// # Errors
///
/// SQLSTATE 2203C if an item is no object \[VIII 1 a\], in either mode.
pub(super) fn keyvalue(base: &[Cow<'_, Item>], identity: impl Fn(&Cow<'_, Item>) -> u64) -> Result<Vec<Item>> {
    let mut pairs = Vec::new();
    for each in base {
        let Some(members) = each.as_object() else {
            return Err(exception(
                SqlState::SQL_JSON_OBJECT_NOT_FOUND,
                "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VIII 1 a] an SQL/JSON item of keyvalue() is not an object",
            ));
        };
        let id = Item::Scalar(Decimal::new(i128::from(identity(each)), 0).into());
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
