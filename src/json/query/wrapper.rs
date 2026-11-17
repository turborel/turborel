//! The behaviours of JSON_QUERY [ISO/IEC 9075-2:2023 §6.35], a `<JSON
//! query wrapper behavior>` and its ON EMPTY and ON ERROR behaviours; and
//! an SQL/JSON sequence converted to an SQL/JSON item by them [§9.44],
//! as of a JSON type, which JSON_QUERY returns if its declared type is one
//! [§6.35 GR5a]. \[DESIGN\] turborel has no JSON type yet, so no SQL
//! statement converts so.

use super::{caught, exception};
use crate::error::{Result, SqlState};
use crate::json::Item;

/// [§6.35] A `<JSON query wrapper behavior>`: whether an SQL/JSON
/// sequence is wrapped in an SQL/JSON array. \[SR6\] Without one, WITHOUT
/// ARRAY is implicit, `Wrapper::Without`; \[SR7\] WITH without CONDITIONAL
/// or UNCONDITIONAL is WITH UNCONDITIONAL ARRAY, `Wrapper::Unconditional`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrapper {
    /// `WITHOUT [ ARRAY ]`: never; the default \[SR6\].
    Without,
    /// `WITH CONDITIONAL [ ARRAY ]`: unless the sequence is one item
    /// [§9.44 GR4a iv], or, serialized, one array or object [§9.49 GR4a
    /// iv].
    Conditional,
    /// `WITH [ UNCONDITIONAL ] [ ARRAY ]`: always, if it has an item.
    Unconditional,
}

/// [§6.35] A `<JSON query empty behavior>` or `<JSON query error
/// behavior>`: what JSON_QUERY gives of an empty SQL/JSON sequence, or in
/// place of an exception condition. \[SR4, SR5\] Without one, NULL is
/// implicit, `QueryBehavior::Null`, ON EMPTY and ON ERROR alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryBehavior {
    /// `ERROR`: the exception condition, of no SQL/JSON item if empty.
    Error,
    /// `NULL`: the null value; the default \[SR4, SR5\].
    Null,
    /// `EMPTY ARRAY`: an SQL/JSON array of no elements.
    EmptyArray,
    /// `EMPTY OBJECT`: an SQL/JSON object of no members.
    EmptyObject,
}

impl Wrapper {
    /// [§9.44 GR4a, §9.49 GR4a] WRAPIT: whether `seq` is wrapped; never
    /// if it is empty, so that ON EMPTY supersedes the wrapper [NOTE 479].
    /// `single` says whether WITH CONDITIONAL ARRAY leaves it as it is.
    pub(super) fn wraps(self, seq: &[Item], single: fn(&[Item]) -> bool) -> bool {
        if seq.is_empty() {
            return false;
        }
        match self {
            Wrapper::Without => false,
            Wrapper::Conditional => !single(seq),
            Wrapper::Unconditional => true,
        }
    }
}

/// [§9.44 GR4b, §9.49 GR4b] SEQ2: `seq`, or, if `wrapit`, an SQL/JSON
/// sequence of one item, the array of `seq`'s items in order.
pub(super) fn wrapped(seq: Vec<Item>, wrapit: bool) -> Vec<Item> {
    if wrapit { vec![Item::Array(seq)] } else { seq }
}

/// [§9.44 GR4c, §9.49 GR4c] V: the one item of SEQ2 `seq`; of none, the
/// item `on_empty` gives, none of `QueryBehavior::Null` [NOTE 480].
/// `rule` is the General Rule applied, its subclause and number, for the
/// errors.
///
/// # Errors
///
/// SQLSTATE 22035 if `seq` is empty and `on_empty` is ERROR \[GR4c i 1\];
/// 22034 if it has more than one item \[GR4c iii\].
pub(super) fn only(seq: Vec<Item>, on_empty: QueryBehavior, rule: &str) -> Result<Option<Item>> {
    let mut items = seq.into_iter();
    let Some(item) = items.next() else {
        return match on_empty {
            QueryBehavior::Error => Err(exception(
                SqlState::NO_SQL_JSON_ITEM,
                &format!("[ISO/IEC 9075-2:2023 {rule}c i 1] the SQL/JSON path expression gives no SQL/JSON item, and ERROR ON EMPTY"),
            )),
            QueryBehavior::Null | QueryBehavior::EmptyArray | QueryBehavior::EmptyObject => Ok(on_empty.item()),
        };
    };
    if items.next().is_some() {
        return Err(exception(
            SqlState::MORE_THAN_ONE_SQL_JSON_ITEM,
            &format!("[ISO/IEC 9075-2:2023 {rule}c iii] the SQL/JSON path expression gives more than one SQL/JSON item, and no wrapper"),
        ));
    }
    Ok(Some(item))
}

impl QueryBehavior {
    /// The item an empty sequence or an exception condition is replaced
    /// by: none of NULL, and of ERROR, which replaces neither.
    pub(super) fn item(self) -> Option<Item> {
        match self {
            QueryBehavior::Error | QueryBehavior::Null => None,
            QueryBehavior::EmptyArray => Some(Item::Array(Vec::new())),
            QueryBehavior::EmptyObject => Some(Item::Object(Vec::new())),
        }
    }
}

/// [§9.44 GR4a iv 1] Whether `seq` is a single SQL/JSON item.
fn single_item(seq: &[Item]) -> bool {
    seq.len() == 1
}

/// [§9.44] The SQL/JSON item the SQL/JSON sequence of `status`, STATUS IN
/// and SQL/JSON SEQUENCE, converts to, wrapped as `wrapper` says, of an
/// empty one as `on_empty` says \[GR4\]; none, the null value, if empty
/// and `on_empty` is NULL \[GR5a i\]. An exception condition, of `status`
/// or the rules, is replaced as `on_error` says \[GR5b\]; an error not one
/// is raised (`query::is_exception`).
///
/// # Errors
///
/// The exception condition of `status`, or SQLSTATE 22035 or 22034 of
/// `only`, if `on_error` is ERROR \[GR5b i\]; an error not an exception
/// condition.
pub fn sequence_to_item(
    status: Result<Vec<Item>>,
    wrapper: Wrapper,
    on_empty: QueryBehavior,
    on_error: QueryBehavior,
) -> Result<Option<Item>> {
    let temp = status.and_then(|seq| {
        let wrapit = wrapper.wraps(&seq, single_item);
        only(wrapped(seq, wrapit), on_empty, "§9.44 GR4")
    });
    caught(temp, |error| match on_error {
        QueryBehavior::Error => Err(error),
        QueryBehavior::Null | QueryBehavior::EmptyArray | QueryBehavior::EmptyObject => Ok(on_error.item()),
    })
}

#[cfg(test)]
mod tests;
