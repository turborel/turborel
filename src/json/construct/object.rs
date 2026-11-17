//! JSON_OBJECT [ISO/IEC 9075-2:2023 §6.34], `<JSON object constructor>`:
//! a JSON object of name and value pairs; and the members of an object
//! being made, which JSON_OBJECTAGG gathers too [§10.11 GR4].

use std::collections::HashSet;

use super::{Input, MAX_MEMBERS, OnNull, input_item, key, within};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::serialize::string_type;
use crate::json::{IMPLICIT_RETURNING, Item, Keys, Member, serialize_as};
use crate::value::{DataType, Value};

/// [§6.34] A `<JSON name and value>`, `[ KEY ] name VALUE value` or
/// `name : value` (Feature T814), its values evaluated.
#[derive(Debug, Clone, Copy)]
pub struct NameAndValue<'a> {
    /// The value of the `<JSON name>`, a character string.
    pub name: &'a Value,
    /// The `<JSON input expression>`.
    pub value: Input<'a>,
}

/// [§6.34, §10.11] What follows the `<JSON name and value>`s of a
/// JSON_OBJECT or a JSON_OBJECTAGG: each clause explicit or implicit, as
/// its Syntax Rules make one (`ObjectOptions::IMPLICIT`).
#[derive(Debug, Clone, Copy)]
pub struct ObjectOptions<'a> {
    /// The `<JSON constructor null clause>`; without one, NULL ON NULL,
    /// `OnNull::Null` [§6.34 SR6a, §10.11 SR4a].
    pub on_null: OnNull,
    /// The `<JSON key uniqueness constraint>` WUK; without one, WITHOUT
    /// UNIQUE KEYS, `Keys::Any` [§6.34 SR6b, §10.11 SR4b]. WITH UNIQUE
    /// KEYS is Feature T830.
    pub keys: Keys,
    /// The `<data type>` of the `<JSON output clause>`, FORMAT JSON, a
    /// character string type, which is the declared type [§6.34 SR6d,
    /// §10.11 SR4d]; without one, `IMPLICIT_RETURNING` (ID098, ID134)
    /// [§6.34 SR2b, §10.11 SR1b].
    pub returning: &'a DataType,
}

impl ObjectOptions<'static> {
    /// Every clause implicit: NULL ON NULL, WITHOUT UNIQUE KEYS, RETURNING
    /// `IMPLICIT_RETURNING` FORMAT JSON.
    pub const IMPLICIT: ObjectOptions<'static> = ObjectOptions { on_null: OnNull::Null, keys: Keys::Any, returning: &IMPLICIT_RETURNING };
}

/// The members of a JSON object being made, one `<JSON name and value>`
/// at a time, in order [§6.34 GR2d ii, §10.11 GR4b ii]. `count` counts
/// those kept, of which at most `MAX_MEMBERS` are held, so what it holds
/// is bounded; `seen` holds, under WITH UNIQUE KEYS, every key given,
/// those left out ABSENT ON NULL too, and `duplicate` the first key given
/// twice.
#[derive(Debug, Clone)]
pub(super) struct Members {
    on_null: OnNull,
    keys: Keys,
    kept: Vec<Member>,
    count: usize,
    seen: HashSet<String>,
    duplicate: Option<String>,
}

impl Members {
    /// No members yet, of the null clause `on_null` and the key uniqueness
    /// constraint `keys`.
    pub(super) fn new(on_null: OnNull, keys: Keys) -> Members {
        Members { on_null, keys, kept: Vec::new(), count: 0, seen: HashSet::new(), duplicate: None }
    }

    /// [§6.34 GR2d ii 1, §10.11 GR4b ii 2] Mi, of the value `name` of the
    /// `<JSON name>` and the `<JSON input expression>` `value`, its bound
    /// value parsed, if FORMAT JSON, under WUK; left out if it is the
    /// SQL/JSON null and the null clause ABSENT ON NULL [GR2d ii 3 B].
    ///
    /// # Errors
    ///
    /// As `key` and `input_item`.
    pub(super) fn add(&mut self, name: &Value, value: &Input<'_>) -> Result<()> {
        let key = key(name)?;
        let bound = input_item(value, self.keys)?;
        if self.keys == Keys::Unique {
            self.note(&key);
        }
        if self.on_null == OnNull::Absent && matches!(bound, Item::Null) {
            return Ok(());
        }
        self.count = self.count.saturating_add(1);
        if self.count <= MAX_MEMBERS {
            self.kept.push(Member { key, value: bound });
        }
        Ok(())
    }

    /// `key` noted as given, and as the first given twice if it is.
    fn note(&mut self, key: &str) {
        let fresh = self.seen.insert(key.to_owned());
        if !fresh && self.duplicate.is_none() {
            self.duplicate = Some(key.to_owned());
        }
    }

    /// [§6.34 GR2d ii 2, 3, §10.11 GR4b ii 3, 4] The members kept, if no
    /// two keys given are one under WITH UNIQUE KEYS and they are not too
    /// many.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22030 if two keys are one under WITH UNIQUE KEYS \[GR2d ii
    /// 2\]; 2203E if more than `MAX_MEMBERS` are kept \[GR2d ii 3\].
    pub(super) fn checked(&self) -> Result<&[Member]> {
        if let Some(key) = &self.duplicate {
            return Err(SqlError::new(
                Code::Data,
                format!("[ISO/IEC 9075-2:2023 §6.34 GR2d ii 2] two members of a JSON object WITH UNIQUE KEYS have the key \"{key}\""),
            )
            .with_state(SqlState::DUPLICATE_JSON_OBJECT_KEY_VALUE));
        }
        within(
            self.count,
            MAX_MEMBERS,
            SqlState::TOO_MANY_JSON_OBJECT_MEMBERS,
            "[ISO/IEC 9075-2:2023 §6.34 GR2d ii 3] the members of a JSON object are too many",
        )?;
        Ok(&self.kept)
    }
}

/// [§6.34 GR2, GR5a] JSON_OBJECT of the `<JSON name and value>`s
/// `members`, in order, and the clauses `options`: the JSON text of the
/// object whose members are theirs \[GR2d ii\], with no members of none
/// \[GR2d i\], those whose bound value is the SQL/JSON null left out if
/// `options.on_null` is ABSENT ON NULL, serialized to `options.returning`
/// [§9.43].
///
/// # Errors
///
/// SQLCODE -103 if `options.returning` is no string type [§9.43 SR2a];
/// -900 if it is a binary string type (RULES.md D18); in the order the
/// rules apply, of each member in turn: SQLSTATE 22004 if a name is the
/// null value \[GR2d ii 1 D\], SQLCODE -103 if it is no character string;
/// as `input_item` of its value (22032, 22037 of a FORMAT JSON one, 22037
/// of an object in it of two members of one key under WITH UNIQUE KEYS);
/// then 22030 if two names are one under WITH UNIQUE KEYS, the members
/// left out too \[GR2d ii 2\]; 2203E if the members are more than
/// `MAX_MEMBERS` \[GR2d ii 3\]; as `serialize_as`, 22001 if the text is
/// longer than `options.returning` [§9.43 GR2a iii].
pub fn json_object(members: &[NameAndValue<'_>], options: &ObjectOptions<'_>) -> Result<Value> {
    string_type(options.returning)?;
    let mut made = Members::new(options.on_null, options.keys);
    for member in members {
        made.add(member.name, &member.value)?;
    }
    made.checked()?;
    serialize_as(&Item::Object(made.kept), options.returning)
}

#[cfg(test)]
mod tests;
