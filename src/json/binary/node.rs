//! \[DESIGN\] A node of a stored document (`json/binary.rs`) read in place:
//! its kind, its scalar, its elements and members, a member of a key by a
//! binary search of the key directory, all without allocating; and the
//! item a node is, made only when asked for.

use std::cmp::Ordering;

use super::write::moment_tag;
use super::{
    APPROXIMATE, ARRAY, CONTAINER_HEAD, DATETIME, DATETIME_BODY, DECFLOAT, DECIMAL, FALSE, INTEGER, LONG, NULL, OBJECT, STRING, TRUE, WIDE,
    WIDE_WORD, WORD, corrupt, within,
};
use crate::error::Result;
use crate::json::{Item, Member, Scalar};
use crate::value::datetime::Moment;
use crate::value::{Datetime, DecFloat, Decimal, MAX_DECFLOAT_PRECISION, Value};

/// A node of a document: the octets from its tag to the document's end.
#[derive(Debug, Clone, Copy)]
pub struct Node<'a> {
    bytes: &'a [u8],
}

/// What kind of SQL/JSON item a node is [§4.48.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// The SQL/JSON null.
    Null,
    /// A truth value.
    Boolean,
    /// A number, exact, decimal floating-point or approximate.
    Number,
    /// A character string.
    String,
    /// A datetime.
    Datetime,
    /// An array.
    Array,
    /// An object.
    Object,
}

/// `n` octets of `bytes` from `at`.
fn slice(bytes: &[u8], at: usize, n: usize) -> Result<&[u8]> {
    at.checked_add(n).and_then(|end| bytes.get(at..end)).ok_or_else(|| corrupt("a node runs past its document"))
}

/// The `N` octets of `bytes` from `at`.
fn array_at<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N]> {
    <[u8; N]>::try_from(slice(bytes, at, N)?).map_err(|_| corrupt("a field is short"))
}

/// The little-endian u32 at `at` of `bytes`, as a usize.
fn word_at(bytes: &[u8], at: usize) -> Result<usize> {
    usize::try_from(u32::from_le_bytes(array_at(bytes, at)?)).map_err(|_| corrupt("a word is past this machine's sizes"))
}

impl<'a> Node<'a> {
    /// The node whose tag is the first of `bytes`.
    pub(super) fn at(bytes: &'a [u8]) -> Node<'a> {
        Node { bytes }
    }

    /// Its tag.
    fn tag(&self) -> Result<u8> {
        self.bytes.first().copied().ok_or_else(|| corrupt("a node is empty"))
    }

    /// What kind of item it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its tag is no node's.
    pub fn kind(&self) -> Result<NodeKind> {
        match self.tag()? {
            NULL => Ok(NodeKind::Null),
            FALSE | TRUE => Ok(NodeKind::Boolean),
            INTEGER | DECIMAL | WIDE | DECFLOAT | APPROXIMATE => Ok(NodeKind::Number),
            STRING => Ok(NodeKind::String),
            DATETIME => Ok(NodeKind::Datetime),
            ARRAY => Ok(NodeKind::Array),
            OBJECT => Ok(NodeKind::Object),
            _ => Err(corrupt("a node's tag is no kind's")),
        }
    }

    /// The octets of its body, after its tag.
    fn body(&self) -> &'a [u8] {
        self.bytes.get(1..).unwrap_or_default()
    }

    /// The characters of the string it is, read in place; none if it is
    /// no string.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn as_str(&self) -> Result<Option<&'a str>> {
        if self.tag()? != STRING {
            return Ok(None);
        }
        string_at(self.body(), 0).map(|(s, _)| Some(s))
    }

    /// The number of its elements or members; none if it is neither an
    /// array nor an object.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn len(&self) -> Result<Option<usize>> {
        match self.tag()? {
            ARRAY | OBJECT => word_at(self.body(), 0).map(Some),
            _ => Ok(None),
        }
    }

    /// Where its first element or member begins in its body, after its
    /// offsets.
    fn first(&self) -> Result<usize> {
        let count = word_at(self.body(), 0)?;
        count.checked_mul(WORD).and_then(|n| n.checked_add(CONTAINER_HEAD)).ok_or_else(|| corrupt("a count is past its document"))
    }

    /// The offset of the `i`th element or directory entry, counted from
    /// its first element or member, and where that first one begins in its
    /// body.
    fn offset(&self, i: usize) -> Result<(usize, usize)> {
        let at = word_at(self.body(), CONTAINER_HEAD.saturating_add(i.saturating_mul(WORD)))?;
        Ok((at, self.first()?))
    }

    /// Its `i`th element, from 0, if it is an array with one.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn element(&self, i: usize) -> Result<Option<Node<'a>>> {
        if self.tag()? != ARRAY || i >= word_at(self.body(), 0)? {
            return Ok(None);
        }
        let (at, first) = self.offset(i)?;
        let from = first.checked_add(at).ok_or_else(|| corrupt("an offset is past its document"))?;
        let bytes = self.body().get(from..).filter(|b| !b.is_empty()).ok_or_else(|| corrupt("an offset is past its document"))?;
        Ok(Some(Node::at(bytes)))
    }

    /// The member its key directory has `i`th, from 0, of an object: its
    /// key and its value.
    fn entry(&self, i: usize) -> Result<(&'a str, Node<'a>)> {
        let (at, first) = self.offset(i)?;
        let from = first.checked_add(at).ok_or_else(|| corrupt("an offset is past its document"))?;
        member_at(self.body(), from)
    }

    /// The value of its first member of the key `key`, as written, if it
    /// is an object with one (UA075): found by a binary search of its key
    /// directory.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn member(&self, key: &str) -> Result<Option<Node<'a>>> {
        if self.tag()? != OBJECT {
            return Ok(None);
        }
        let (mut low, mut high) = (0_usize, word_at(self.body(), 0)?);
        // The first entry whose key is not less than `key`: of one key, the
        // directory keeps the order written.
        while low < high {
            let middle = low.saturating_add(high.saturating_sub(low) >> 1);
            let (k, _) = self.entry(middle)?;
            match k.as_bytes().cmp(key.as_bytes()) {
                Ordering::Less => low = middle.saturating_add(1),
                Ordering::Equal | Ordering::Greater => high = middle,
            }
        }
        if low >= word_at(self.body(), 0)? {
            return Ok(None);
        }
        let (k, value) = self.entry(low)?;
        Ok((k == key).then_some(value))
    }

    /// Its elements, in order, if it is an array.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn elements(&self) -> Result<Option<Vec<Node<'a>>>> {
        let Some(count) = self.len()?.filter(|_| self.tag().is_ok_and(|t| t == ARRAY)) else { return Ok(None) };
        (0..count).map(|i| self.element(i)?.ok_or_else(|| corrupt("an element is missing"))).collect::<Result<Vec<_>>>().map(Some)
    }

    /// Its members, each its key and value, in the order written, if it
    /// is an object.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn members(&self) -> Result<Option<Vec<(&'a str, Node<'a>)>>> {
        if self.tag()? != OBJECT {
            return Ok(None);
        }
        let count = word_at(self.body(), 0)?;
        let first = self.first()?;
        let mut at = first;
        // A count read from damaged octets may be far beyond what the body
        // holds, so room is made for no more members than it has octets.
        let mut members = Vec::with_capacity(count.min(self.body().len()));
        for _ in 0..count {
            let (key, value) = member_at(self.body(), at)?;
            at = at.saturating_add(WORD).saturating_add(key.len()).saturating_add(value.length()?);
            members.push((key, value));
        }
        Ok(Some(members))
    }

    /// The octets it takes, its tag included.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn length(&self) -> Result<usize> {
        let body = match self.tag()? {
            NULL | FALSE | TRUE => 0,
            INTEGER | APPROXIMATE => LONG,
            DECIMAL => LONG.saturating_add(1),
            WIDE | DECFLOAT => WIDE_WORD.saturating_add(WORD),
            STRING => WORD.saturating_add(word_at(self.body(), 0)?),
            DATETIME => DATETIME_BODY,
            ARRAY | OBJECT => {
                let count = word_at(self.body(), 0)?;
                let size = word_at(self.body(), WORD)?;
                CONTAINER_HEAD.saturating_add(count.saturating_mul(WORD)).saturating_add(size)
            }
            _ => return Err(corrupt("a node's tag is no kind's")),
        };
        Ok(body.saturating_add(1))
    }

    /// The SQL value of the scalar it is: a character string, a number, a
    /// truth value or a datetime; none if it is the SQL/JSON null, an array
    /// or an object.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged.
    pub fn scalar(&self) -> Result<Option<Value>> {
        let body = self.body();
        let value = match self.tag()? {
            NULL | ARRAY | OBJECT => return Ok(None),
            FALSE => Value::Bool(false),
            TRUE => Value::Bool(true),
            INTEGER => Value::Exact(Decimal::new(i128::from(i64::from_le_bytes(array_at(body, 0)?)), 0)),
            DECIMAL => {
                let units = i128::from(i64::from_le_bytes(array_at(body, 0)?));
                let [scale] = array_at::<1>(body, LONG)?;
                Value::Exact(Decimal::new(units, u32::from(scale)))
            }
            WIDE => Value::Exact(Decimal::new(i128::from_le_bytes(array_at(body, 0)?), u32::from_le_bytes(array_at(body, WIDE_WORD)?))),
            DECFLOAT => {
                let coefficient = i128::from_le_bytes(array_at(body, 0)?);
                let exponent = i64::from(i32::from_le_bytes(array_at(body, WIDE_WORD)?));
                let d = DecFloat::new(coefficient, exponent, MAX_DECFLOAT_PRECISION).ok_or_else(|| corrupt("a number is out of range"))?;
                Value::DecFloat(d)
            }
            APPROXIMATE => Value::Approx(f64::from_bits(u64::from_le_bytes(array_at(body, 0)?))),
            STRING => Value::Char(string_at(body, 0)?.0.to_owned()),
            DATETIME => Value::Datetime(datetime_at(body)?),
            _ => return Err(corrupt("a node's tag is no kind's")),
        };
        Ok(Some(value))
    }

    /// The SQL/JSON item it is, made whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if its octets are damaged; -900 if arrays and objects
    /// nest deeper than `MAX_NESTING`.
    pub fn item(&self) -> Result<Item> {
        self.item_within(0)
    }

    /// `item`, `depth` arrays and objects in.
    fn item_within(&self, depth: usize) -> Result<Item> {
        within(depth)?;
        if let Some(elements) = self.elements()? {
            let items = elements.iter().map(|e| e.item_within(depth.saturating_add(1))).collect::<Result<Vec<_>>>()?;
            return Ok(Item::Array(items));
        }
        if let Some(members) = self.members()? {
            let mut items = Vec::with_capacity(members.len());
            for (key, value) in members {
                items.push(Member { key: key.to_owned(), value: value.item_within(depth.saturating_add(1))? });
            }
            return Ok(Item::Object(items));
        }
        match self.scalar()? {
            Some(value) => Scalar::new(value).map(Item::Scalar).ok_or_else(|| corrupt("a scalar is of no scalar's type")),
            None => Ok(Item::Null),
        }
    }

    /// That it is whole within its document, `depth` arrays and objects in:
    /// its length, checked.
    pub(super) fn check(&self, depth: usize) -> Result<usize> {
        within(depth)?;
        let length = self.length()?;
        slice(self.bytes, 0, length)?;
        match self.tag()? {
            STRING => {
                string_at(self.body(), 0)?;
            }
            DATETIME => {
                datetime_at(self.body())?;
            }
            DECFLOAT => {
                self.scalar()?;
            }
            ARRAY => self.check_elements(depth)?,
            OBJECT => self.check_members(depth)?,
            _ => {}
        }
        Ok(length)
    }

    /// That each element of an array is whole and where its offset says,
    /// one after another, filling its size.
    fn check_elements(&self, depth: usize) -> Result<()> {
        let count = word_at(self.body(), 0)?;
        let mut expected = 0_usize;
        for i in 0..count {
            let (at, _) = self.offset(i)?;
            if at != expected {
                return Err(corrupt("an element's offset is not where it is"));
            }
            let element = self.element(i)?.ok_or_else(|| corrupt("an element is missing"))?;
            expected = expected.saturating_add(element.check(depth.saturating_add(1))?);
        }
        if expected != word_at(self.body(), WORD)? {
            return Err(corrupt("an array's size is not its elements'"));
        }
        Ok(())
    }

    /// That each member of an object is whole, filling its size, and that
    /// its directory lists each once, sorted.
    fn check_members(&self, depth: usize) -> Result<()> {
        let count = word_at(self.body(), 0)?;
        let first = self.first()?;
        let mut starts = Vec::with_capacity(count.min(self.body().len()));
        let mut at = 0_usize;
        for _ in 0..count {
            let (key, value) = member_at(self.body(), first.saturating_add(at))?;
            starts.push(at);
            at = at.saturating_add(WORD).saturating_add(key.len()).saturating_add(value.check(depth.saturating_add(1))?);
        }
        if at != word_at(self.body(), WORD)? {
            return Err(corrupt("an object's size is not its members'"));
        }
        let mut previous: Option<(&str, usize)> = None;
        for i in 0..count {
            let (offset, _) = self.offset(i)?;
            let (key, _) = self.entry(i)?;
            if !starts.contains(&offset) || previous.is_some_and(|(k, o)| (k.as_bytes(), o) >= (key.as_bytes(), offset)) {
                return Err(corrupt("an object's key directory is not sorted"));
            }
            previous = Some((key, offset));
        }
        Ok(())
    }
}

/// The string whose length is at `at` of `body`, and the octets it takes.
fn string_at(body: &[u8], at: usize) -> Result<(&str, usize)> {
    let length = word_at(body, at)?;
    let octets = slice(body, at.saturating_add(WORD), length)?;
    let s = std::str::from_utf8(octets).map_err(|_| corrupt("a string is not UTF-8"))?;
    Ok((s, WORD.saturating_add(length)))
}

/// The member at `at` of an object's body: its key and its value.
fn member_at(body: &[u8], at: usize) -> Result<(&str, Node<'_>)> {
    let (key, taken) = string_at(body, at)?;
    let value = body.get(at.saturating_add(taken)..).filter(|b| !b.is_empty()).ok_or_else(|| corrupt("a member has no value"))?;
    Ok((key, Node::at(value)))
}

/// Where a datetime's instant begins in its body, after its moment.
const UTC_AT: usize = 1;
/// Where its zone begins, after its instant.
const ZONE_AT: usize = UTC_AT + LONG;
/// Where its zoned flag and precision begin, after its zone.
const FLAGS_AT: usize = ZONE_AT + 2;

/// The datetime of a datetime node's body.
fn datetime_at(body: &[u8]) -> Result<Datetime> {
    let [tag] = array_at::<1>(body, 0)?;
    let moment = [Moment::Date, Moment::Time, Moment::Timestamp]
        .into_iter()
        .find(|m| moment_tag(*m) == tag)
        .ok_or_else(|| corrupt("a datetime's moment is no moment"))?;
    let utc = i64::from_le_bytes(array_at(body, UTC_AT)?);
    let zone = i16::from_le_bytes(array_at(body, ZONE_AT)?);
    let [zoned, precision] = array_at::<2>(body, FLAGS_AT)?;
    let zoned = match zoned {
        0 => false,
        1 => true,
        _ => return Err(corrupt("a datetime's zoned flag is neither 0 nor 1")),
    };
    Ok(Datetime { moment, utc, zone, zoned, precision })
}
