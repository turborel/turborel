//! \[DESIGN\] What a data type does (RULES.md D39, D53): one contract,
//! `SqlType`, for SQL's standard types, built in, and for the types beyond
//! them, plug-ins.
//!
//! A `DataType` names its kind (`DataType::kind`), and every type-level
//! operation goes through it: how the type is written, what it compares
//! with, store assignment and CAST to it, its value's bytes in the log and
//! on the wire, in a record and in an index key, its literal, and its
//! descriptor in the Information Schema. Values compare and hash by their
//! own kind (`value::compare`, `value::bucket`): a built-in kind's value is
//! a variant of `Value` matched directly, so the hot path makes no virtual
//! call.

use std::cmp::Ordering;
use std::fmt;

use super::{Bucket, DataType, Value};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};

mod approximate;
mod character;
mod exact;

pub use approximate::{APPROXIMATE, Approximate};
pub use character::{CHARACTER, Character};
pub use exact::{EXACT, Exact};

/// Bytes of a number in a record or a key: a 64-bit word.
pub(crate) const NUMBER_BYTES: usize = 8;
/// The sign bit of a 64-bit word.
const SIGN: u64 = 1 << 63;

/// A key that is not one of its columns' types.
pub(crate) fn malformed() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] an index key is malformed")
}

/// The first `n` bytes of `rest`, which then holds those after.
///
/// # Errors
///
/// SQLCODE -916 if `rest` is shorter.
pub(crate) fn take<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    let (taken, after) = rest.split_at_checked(n).ok_or_else(malformed)?;
    *rest = after;
    Ok(taken)
}

/// A data type's behaviour, given the type itself (`t`) where its
/// parameters matter: a length, a precision and scale. Each method speaks
/// for values of its own kind; one given a value of another kind fails, or
/// answers none, as it says.
///
/// Contract (RULES.md R-SAFE-3; checked for every built-in kind by
/// `test/integration/type_contract.rs`), for a type `t` of the kind and
/// values `a`, `b` of `t` (as `assign` makes them):
/// * `t` is comparable with itself; `assign` of `a` to `t` gives `a` again.
/// * `value` reads back what `put_value` wrote, and `params` the type whose
///   parameters `put_params` wrote.
/// * `decode_record` reads back what `encode_record` wrote, which is
///   `record_width` bytes long, if that is given.
/// * `decode_key` reads back a value that compares equal to the one
///   `encode_key` wrote (-0.0 reads back as 0.0), and consumes exactly its
///   key; the keys of `a` and `b`, compared as bytes, order as `compare`
///   orders `a` and `b`.
/// * If `a` and `b` compare equal, their buckets are equal.
/// * The anchor of `a` is `a` itself, exactly.
pub trait SqlType: Send + Sync + fmt::Debug {
    /// The name it is written by, in a `<data type>`.
    fn name(&self) -> &str;

    /// `t` as written in a `<data type>`.
    fn written(&self, t: &DataType) -> String;

    /// Whether its values are comparable with those of `other`
    /// [X3.135-1986 §4.2]: in a comparison, a union, a CASE.
    fn comparable(&self, other: &DataType) -> bool;

    /// How `a`, of its kind, compares with `b` [X3.135-1986 §5.11]: none if
    /// either is null or they are not comparable.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering>;

    /// The hash bucket of `v`, shared by every value it compares equal
    /// with: none for the null value, which equals nothing.
    fn bucket(&self, v: &Value) -> Option<Bucket>;

    /// [ISO/IEC 9075:1992 §9.2] Store assignment of `v` to a column,
    /// parameter or target of `t`, as the rule `rule` (a citation) applies
    /// it.
    ///
    /// # Errors
    ///
    /// A data exception if `v` does not fit `t`, or cannot be assigned to
    /// it.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value>;

    /// [ISO/IEC 9075:1992 §6.10] `v` cast to `t`.
    ///
    /// # Errors
    ///
    /// A data exception if `v` has no value of `t`.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value>;

    /// How `v`, of its kind, is written as a `<literal>`.
    fn literal(&self, v: &Value) -> String;

    /// `v` as a number on a line, for interpolating within a histogram's
    /// bucket, if its kind has one.
    fn position(&self, v: &Value) -> Option<f64>;

    /// [ISO/IEC 9075:1992 §21.3.5] How the Information Schema describes `t`.
    fn descriptor(&self, t: &DataType) -> Descriptor;

    /// Appends `v`'s bytes, of its kind, to `out`: for the log, the
    /// catalog and the wire, which read them back with `value`.
    fn put_value(&self, v: &Value, out: &mut Vec<u8>);

    /// Reads a value `put_value` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not a value of its kind.
    fn value(&self, r: &mut Reader<'_>) -> Result<Value>;

    /// Appends `t`'s parameters to `out`, which `params` reads back.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>);

    /// Reads a type of its kind whose parameters `put_params` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not its parameters.
    fn params(&self, r: &mut Reader<'_>) -> Result<DataType>;

    /// The width of a value of `t` in a record's fixed part, if it has one;
    /// none for a value of variable length.
    fn record_width(&self, t: &DataType) -> Option<usize>;

    /// The bytes of `v`, not null, as a column of `t` in a record: of the
    /// width `record_width` says, if it says one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `v` is not a value of `t`.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>>;

    /// The value of a column of `t` whose record bytes are `field`; none if
    /// they are not one.
    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value>;

    /// Appends the key of `v`, not null, as a column of `t` to `out`:
    /// bytes that, compared as unsigned bytes from the first, order keys as
    /// their values compare, and that no other value's key begins with.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `v` is not a value of `t`.
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()>;

    /// The value of a column of `t` whose key begins `rest`, which then
    /// holds the bytes after it.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `rest` begins with no key of `t`.
    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value>;

    /// The greatest value of `t` not above `v`, and whether it is `v`
    /// itself: the anchor among a column's keys of a value not of the
    /// column's type (`engine::key::anchor`). None if `v` is of a kind not
    /// comparable with `t`.
    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)>;
}

/// [ISO/IEC 9075:1992 §21.3.5] A data type as the Information Schema
/// describes it: DATA_TYPE and the columns that apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    /// DATA_TYPE: its name.
    pub name: String,
    /// CHARACTER_MAXIMUM_LENGTH, of a character string.
    pub length: Option<u32>,
    /// CHARACTER_OCTET_LENGTH, of a character string.
    pub octets: Option<usize>,
    /// Whether it has a collation, SQL_TEXT's: a character string.
    pub collated: bool,
    /// NUMERIC_PRECISION, of a number.
    pub precision: Option<u32>,
    /// NUMERIC_PRECISION_RADIX: 10 for decimal precision, 2 for binary.
    pub radix: Option<u32>,
    /// NUMERIC_SCALE, of an exact number.
    pub scale: Option<u32>,
    /// DATETIME_PRECISION, of a datetime or interval.
    pub datetime_precision: Option<u32>,
}

impl Descriptor {
    /// A descriptor of `name`, every other column null.
    pub fn of(name: &str) -> Descriptor {
        Descriptor {
            name: name.to_owned(),
            length: None,
            octets: None,
            collated: false,
            precision: None,
            radix: None,
            scale: None,
            datetime_precision: None,
        }
    }
}

impl DataType {
    /// Its kind: what it does.
    pub fn kind(&self) -> &'static dyn SqlType {
        match self {
            DataType::Character { .. } => &CHARACTER,
            DataType::Exact { .. } => &EXACT,
            DataType::Approximate { .. } => &APPROXIMATE,
        }
    }
}

/// The hot paths of a row's columns, a call to a built-in kind made
/// directly, so it is inlined: a record's bytes and a key's, each way.
impl DataType {
    /// Its kind's `encode_record`.
    ///
    /// # Errors
    ///
    /// As `SqlType::encode_record` says.
    pub fn encode_record(&self, v: &Value) -> Result<Vec<u8>> {
        match self {
            DataType::Character { .. } => CHARACTER.encode_record(v, self),
            DataType::Exact { .. } => EXACT.encode_record(v, self),
            DataType::Approximate { .. } => APPROXIMATE.encode_record(v, self),
        }
    }

    /// Its kind's `decode_record`.
    pub fn decode_record(&self, field: &[u8]) -> Option<Value> {
        match self {
            DataType::Character { .. } => CHARACTER.decode_record(field, self),
            DataType::Exact { .. } => EXACT.decode_record(field, self),
            DataType::Approximate { .. } => APPROXIMATE.decode_record(field, self),
        }
    }

    /// Its kind's `encode_key`.
    ///
    /// # Errors
    ///
    /// As `SqlType::encode_key` says.
    pub fn encode_key(&self, v: &Value, out: &mut Vec<u8>) -> Result<()> {
        match self {
            DataType::Character { .. } => CHARACTER.encode_key(v, self, out),
            DataType::Exact { .. } => EXACT.encode_key(v, self, out),
            DataType::Approximate { .. } => APPROXIMATE.encode_key(v, self, out),
        }
    }

    /// Its kind's `decode_key`.
    ///
    /// # Errors
    ///
    /// As `SqlType::decode_key` says.
    pub fn decode_key(&self, rest: &mut &[u8]) -> Result<Value> {
        match self {
            DataType::Character { .. } => CHARACTER.decode_key(rest, self),
            DataType::Exact { .. } => EXACT.decode_key(rest, self),
            DataType::Approximate { .. } => APPROXIMATE.decode_key(rest, self),
        }
    }
}

impl Value {
    /// The kind of type it is a value of; none for the null value, which
    /// is of every type.
    pub fn kind(&self) -> Option<&'static dyn SqlType> {
        match self {
            Value::Null => None,
            Value::Char(_) => Some(&CHARACTER),
            Value::Exact(_) => Some(&EXACT),
            Value::Approx(_) => Some(&APPROXIMATE),
        }
    }
}
