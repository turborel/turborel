//! [ISO/IEC 9075-2:2023 §4.8] A value of the JSON type, an SQL/JSON value
//! that is not the null value: an SQL/JSON item [§4.48.3], held as the
//! octets of its stored form (`json/binary.rs`, RULES.md D89).
//!
//! The octets are shared, so a value is cloned without copying them, and
//! read in place: `root` is the node an accessor walks.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use crate::error::Result;
use crate::json::binary::{self, Node};
use crate::json::{Item, Keys, parse, serialize};

/// A value of the JSON type: an SQL/JSON item's stored form, whole.
#[derive(Clone, PartialEq, Eq)]
pub struct Json(Arc<[u8]>);

impl Json {
    /// The value of the SQL/JSON item `item`.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it nests deeper than `json::MAX_NESTING`, or its
    /// stored form would be longer than `binary::MAX_DOCUMENT_OCTETS`.
    pub fn of_item(item: &Item) -> Result<Json> {
        binary::encode(item).map(|bytes| Json(bytes.into()))
    }

    /// [§9.42] The value of the JSON text `text`, its objects' keys as
    /// `keys` says.
    ///
    /// # Errors
    ///
    /// As `json::parse`, and `of_item`.
    pub fn parse(text: &str, keys: Keys) -> Result<Json> {
        Json::of_item(&parse(text, keys)?)
    }

    /// The value of the stored form `bytes`, checked whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `bytes` is no stored form (`binary::validate`).
    pub fn of_bytes(bytes: &[u8]) -> Result<Json> {
        binary::validate(bytes)?;
        Ok(Json(bytes.into()))
    }

    /// Its stored form.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// The node of its item, read in place.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn root(&self) -> Result<Node<'_>> {
        binary::root(&self.0)
    }

    /// The SQL/JSON item it is, made whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn item(&self) -> Result<Item> {
        binary::decode(&self.0)
    }

    /// [§9.43] Its JSON text.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22032 if it is or holds a datetime, which no JSON text
    /// is; else as `item`.
    pub fn text(&self) -> Result<String> {
        serialize(&self.item()?)
    }

    /// [§8.2 GR10] Its comparison key (`binary::document_key`).
    ///
    /// # Errors
    ///
    /// As `item`.
    pub fn key(&self) -> Result<Vec<u8>> {
        binary::document_key(&self.0)
    }

    /// [§8.2 GR10] How it compares with `other`, none if either's octets
    /// are damaged.
    pub fn compare(&self, other: &Json) -> Option<Ordering> {
        binary::compare_documents(&self.0, &other.0).ok()
    }
}

/// Its JSON text, or, of an item no JSON text is, a note saying so.
impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.text() {
            Ok(text) => f.write_str(&text),
            Err(e) => write!(f, "<{}>", e.msg),
        }
    }
}

impl fmt::Debug for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Json({self})")
    }
}
