//! [ISO/IEC 9075-11:2006 draft §6.45] SQL_SIZING's rows: a row of each
//! sizing item, the implementation-defined limits ISO/IEC 9075-2:2023
//! Annex B lists (IL001 to IL208), as ISO/IEC 9075-11 names no sizing item
//! itself and the items ISO/IEC 9075-3 adds are not defined here. An
//! item's SIZING_ID is its number in Annex B, its SIZING_NAME Annex B's
//! words, a word broken at a line's end joined; SUPPORTED_VALUE is the
//! limit the code keeps, read from the constant that keeps it, 0 if there
//! is none, and null of an item no feature here needs [Description 3].

use super::text;
use crate::storage::Row;
use crate::system::{Seen, number};
use crate::value::Decimal;
use crate::value::Value;

/// A sizing item: its number, its name, the value supported, and a
/// comment naming it in Annex B and saying what the value is of.
struct Item {
    id: usize,
    name: &'static str,
    value: fn() -> Value,
    comment: &'static str,
}

/// `n` as SUPPORTED_VALUE.
fn exact(n: impl Into<i128>) -> Value {
    Value::Exact(Decimal::new(n.into(), 0))
}

/// [§6.45 Description 3] The value of an item with no limit.
fn unlimited() -> Value {
    exact(0_u32)
}

/// [ISO/IEC 9075-2:2023 Annex B] The items, in its order.
const ITEMS: [Item; 40] = [
    Item {
        id: 1,
        name: "The maximum length of a fixed-length character string type.",
        value: || exact(crate::value::MAX_CHARACTER_LENGTH),
        comment: "IL001, in characters",
    },
    Item {
        id: 2,
        name: "The maximum length of a large object character string type.",
        value: || exact(crate::value::lob::MAX_LOB_LENGTH),
        comment: "IL002, in characters",
    },
    Item {
        id: 3,
        name: "The maximum length of a fixed-length binary string type.",
        value: || exact(crate::value::MAX_BINARY_LENGTH),
        comment: "IL003, in octets",
    },
    Item {
        id: 4,
        name: "The maximum length of a large object binary string type.",
        value: || exact(crate::value::lob::MAX_LOB_LENGTH),
        comment: "IL004, in octets",
    },
    Item {
        id: 6,
        name: "The maximum length of a variable-length character string type.",
        value: || exact(crate::value::MAX_CHARACTER_LENGTH),
        comment: "IL006, in characters",
    },
    Item {
        id: 7,
        name: "The maximum length of a variable-length binary string type.",
        value: || exact(crate::value::MAX_BINARY_LENGTH),
        comment: "IL007, in octets",
    },
    Item {
        id: 8,
        name: "The maximum cardinality of an array type.",
        value: || exact(crate::value::types::MAX_ARRAY_CARDINALITY),
        comment: "IL008",
    },
    Item {
        id: 9,
        name: "The maximum value for the precision of the NUMERIC data type.",
        value: || exact(crate::value::MAX_EXACT_PRECISION),
        comment: "IL009, in decimal digits",
    },
    Item {
        id: 38,
        name: "The precision of the SQL-session context statement timestamp is the implementation-defined maximum timestamp precision.",
        value: || exact(crate::value::datetime::MAX_TIME_PRECISION),
        comment: "IL038",
    },
    Item {
        id: 39,
        name: "The maximum number of SQL/JSON members in an SQL/JSON object or SQL/JSON array.",
        value: unlimited,
        comment: "IL039: none of an item parsed; an object or array constructed holds IL051's and IL052's",
    },
    Item { id: 40, name: "The maximum length of the key in an SQL/JSON member.", value: unlimited, comment: "IL040: none" },
    Item {
        id: 41,
        name: "The maximum length of an SQL/JSON element or the maximum length of the key of an SQL/JSON member when it is a string type.",
        value: unlimited,
        comment: "IL041: none",
    },
    Item {
        id: 42,
        name: "The maximum lengths for character string types and binary string types.",
        value: || exact(crate::value::lob::MAX_LOB_LENGTH),
        comment: "IL042: of a large object length; a length is at most IL006's and IL007's",
    },
    Item {
        id: 43,
        name: "The maximum values of precision and of scale for the exact numeric types DECIMAL and NUMERIC.",
        value: || exact(crate::value::MAX_EXACT_PRECISION),
        comment: "IL043: of both, in decimal digits",
    },
    Item {
        id: 44,
        name: "The maximum values of precision for the approximate numeric type FLOAT.",
        value: || exact(crate::value::DOUBLE_PRECISION),
        comment: "IL044, in binary digits",
    },
    Item {
        id: 45,
        name: "The maximum value of time and timestamp fractional seconds precision for a datetime type.",
        value: || exact(crate::value::datetime::MAX_TIME_PRECISION),
        comment: "IL045",
    },
    Item {
        id: 46,
        name: "The maximum length of character transliteration, pad function, transcoding, normalize function without normalize function result length specified, or specific type method.",
        value: || exact(crate::value::MAX_CHARACTER_LENGTH),
        comment: "IL046: of a pad function, of a large object IL002's; no transliteration, transcoding or normalize function is defined",
    },
    Item {
        id: 47,
        name: "The maximum length of the declared type of regex transliteration when no XQuery replacement string is specified.",
        value: || exact(crate::value::MAX_CHARACTER_LENGTH),
        comment: "IL047: of a character string subject, of a large object IL002's",
    },
    Item {
        id: 48,
        name: "The maximum length of the result of the classifier function.",
        value: || number(crate::lexer::MAX_IDENTIFIER_LENGTH),
        comment: "IL048",
    },
    Item {
        id: 49,
        name: "The maximum length of a JSON name.",
        value: || number(crate::value::lob::CHUNK_OCTETS),
        comment: "IL049: of a large object, read whole, in octets; of a character string type, its length",
    },
    Item {
        id: 50,
        name: "The maximum length of a string value expression contained in a JSON input expression.",
        value: || number(crate::value::lob::CHUNK_OCTETS),
        comment: "IL050: of a large object, read whole, in octets; of a character string type, its length",
    },
    Item { id: 51, name: "The maximum number of members in a JSON object.", value: || number(crate::json::MAX_MEMBERS), comment: "IL051" },
    Item { id: 52, name: "The maximum number of elements in a JSON array.", value: || number(crate::json::MAX_ELEMENTS), comment: "IL052" },
    Item {
        id: 53,
        name: "The maximum precision of a data type defined as FLOAT.",
        value: || exact(crate::value::DOUBLE_PRECISION),
        comment: "IL053, in binary digits",
    },
    Item {
        id: 54,
        name: "The maximum and minimum values of the exponent in the FLOAT, REAL, DOUBLE PRECISION, and decimal floating-point types.",
        value: || exact(crate::value::decfloat::MAX_EXPONENT),
        comment: "IL054: the greatest adjusted exponent of DECFLOAT, its least -6143; an approximate number's are an IEEE 754 binary64's",
    },
    Item {
        id: 55,
        name: "The maximum value of timestamp precision.",
        value: || exact(crate::value::datetime::MAX_TIME_PRECISION),
        comment: "IL055",
    },
    Item {
        id: 56,
        name: "The maximum number of SQL item descriptors of the initial result row type descriptor area.",
        value: || exact(crate::db::dynamic::ptf::MAX_RESULT_ITEMS),
        comment: "IL056",
    },
    Item {
        id: 57,
        name: "The maximum number of SQL item descriptors of the intermediate result row type descriptor area.",
        value: || exact(crate::db::dynamic::ptf::MAX_RESULT_ITEMS),
        comment: "IL057",
    },
    Item {
        id: 58,
        name: "The maximum length of a PTF extended name for a PTF cursor or PTF descriptor area.",
        value: || exact(crate::catalog::PTF_NAME_LENGTH),
        comment: "IL058",
    },
    Item {
        id: 59,
        name: "The maximum value of interval leading field precision.",
        value: || exact(crate::value::interval::MAX_LEADING_PRECISION),
        comment: "IL059",
    },
    Item {
        id: 60,
        name: "The maximum value of interval fractional seconds precision.",
        value: || exact(crate::value::datetime::MAX_TIME_PRECISION),
        comment: "IL060",
    },
    Item {
        id: 61,
        name: "The minimum permitted negative time zone displacement.",
        value: || exact(crate::value::datetime::MIN_DISPLACEMENT.unsigned_abs()),
        comment: "IL061: in minutes, before UTC",
    },
    Item {
        id: 62,
        name: "The maximum permitted positive time zone displacement.",
        value: || exact(crate::value::datetime::MAX_DISPLACEMENT),
        comment: "IL062: in minutes, after UTC",
    },
    Item {
        id: 63,
        name: "The maximum number of digits immediately contained in an unsigned integer.",
        value: unlimited,
        comment: "IL063: none; its value is at most 4294967295",
    },
    Item {
        id: 64,
        name: "the maximum length of the returned variable-length character string when a JSON simplified accessor op specifies number or size.",
        value: || exact(crate::value::MAX_CHARACTER_LENGTH),
        comment: "IL064: of number(); size() returns BIGINT",
    },
    Item {
        id: 66,
        name: "The maximum number and maximum numeric precision of type Scale_s declarations in an Interfaces.SQL.Numeric library unit package when the caller language of the externally-invoked procedure is Ada.",
        value: || Value::Null,
        comment: "IL066: no externally-invoked procedure",
    },
    Item {
        id: 205,
        name: "The maximum length of each variable-length character item in the diagnostics area.",
        value: || exact(crate::value::MAX_CHARACTER_LENGTH),
        comment: "IL205",
    },
    Item {
        id: 206,
        name: "The maximum number of SQL descriptor areas.",
        value: || number(crate::db::dynamic::MAX_DESCRIPTORS),
        comment: "IL206: of a session at one time",
    },
    Item {
        id: 207,
        name: "The maximum number of item descriptor areas for a single SQL descriptor area.",
        value: || exact(crate::db::dynamic::MAX_OCCURRENCES),
        comment: "IL207",
    },
    Item {
        id: 208,
        name: "The maximum number of savepoints per SQL-transaction.",
        value: || number(crate::db::savepoints::MAX_SAVEPOINTS),
        comment: "IL208",
    },
];

/// [§6.45] A row of each item.
pub(super) fn items(_: &Seen<'_>) -> Vec<Row> {
    ITEMS.iter().map(|i| vec![number(i.id), text(i.name), (i.value)(), text(i.comment)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 9075-11:2006 draft §6.45 Description 1, 2]
    /// each item has a row, numbered as Annex B numbers it, and no two
    /// share a number or a name (SQL_SIZING_PRIMARY_KEY,
    /// SQL_SIZING_SIZING_NAME_UNIQUE).
    #[test]
    fn each_item_is_numbered_and_named_once() {
        let mut ids: Vec<usize> = ITEMS.iter().map(|i| i.id).collect();
        let mut names: Vec<&str> = ITEMS.iter().map(|i| i.name).collect();
        assert!(ids.is_sorted());
        ids.dedup();
        names.sort_unstable();
        names.dedup();
        assert_eq!((ids.len(), names.len()), (ITEMS.len(), ITEMS.len()));
        for i in &ITEMS {
            assert!(i.comment.starts_with(&format!("IL{:03}", i.id)), "{}", i.comment);
        }
    }
}
