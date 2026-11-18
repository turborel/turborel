//! \[EXTENSION\] A vector's text forms (design document §5.1): a dense
//! vector as a JSON array, `'[0.12, -0.5, 3.0E-2]'`, BINARY as one byte
//! (0 to 255) for each 8 dimensions; a sparse one as `'[dims, [indexes],
//! [values]]'` or the JSON object `'{"dims": d, "indices": [...],
//! "values": [...]}'`, indexes counted from 0. Numbers are written
//! shortest first: the fewest digits that read back as the same element
//! (R5.6).

use super::build::Vector;
use super::params::Shape;
use super::{BITS_PER_BYTE, Element, Layout, VectorRef, bad_literal, float, half_bits, mismatch, out_of_range};
use crate::error::Result;

/// The JSON object's member names (design document §5.1).
const DIMS_KEY: &str = "dims";
/// The member holding the indexes.
const INDICES_KEY: &str = "indices";
/// The member holding the values.
const VALUES_KEY: &str = "values";
/// The smallest magnitude written without an exponent.
const PLAIN_LOW: f64 = 1e-5;
/// The magnitude from which a number is written with an exponent.
const PLAIN_HIGH: f64 = 1e16;
/// The most significant digits a binary16 needs to read back.
const HALF_DIGITS: usize = 5;
/// The bit of the first dimension in a BINARY byte.
const HIGH_BIT: u8 = 0x80;

/// A text form, read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Parsed {
    /// The values of a JSON array.
    Dense(Vec<f64>),
    /// A sparse vector's dimension count and its indexed values.
    Sparse(u32, Vec<(u32, f64)>),
}

/// A reader of a text form.
struct Scan<'a> {
    rest: &'a str,
}

impl<'a> Scan<'a> {
    fn skip(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn peek(&mut self) -> Option<char> {
        self.skip();
        self.rest.chars().next()
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip();
        match self.rest.strip_prefix(c) {
            Some(after) => {
                self.rest = after;
                true
            }
            None => false,
        }
    }

    fn expect(&mut self, c: char) -> Result<()> {
        if self.eat(c) { Ok(()) } else { Err(bad_literal(&format!("expected '{c}'"))) }
    }

    fn number(&mut self) -> Result<f64> {
        self.skip();
        let end = self.rest.find(|c: char| !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))).unwrap_or(self.rest.len());
        let (word, after) = self.rest.split_at_checked(end).unwrap_or((self.rest, ""));
        let x: f64 = word.parse().map_err(|_| bad_literal(&format!("'{word}' is not a number")))?;
        if !x.is_finite() {
            return Err(out_of_range("NaN and infinity are not vector elements"));
        }
        self.rest = after;
        Ok(x)
    }

    fn whole(&mut self) -> Result<u32> {
        let x = self.number()?;
        float::integral(x).and_then(|i| u32::try_from(i).ok()).ok_or_else(|| bad_literal(&format!("{x} is not a dimension")))
    }

    fn key(&mut self) -> Result<&'a str> {
        self.expect('"')?;
        let end = self.rest.find('"').ok_or_else(|| bad_literal("an unclosed member name"))?;
        let (key, after) = self.rest.split_at_checked(end).ok_or_else(|| bad_literal("an unclosed member name"))?;
        self.rest = after.get(1..).unwrap_or_default();
        self.expect(':')?;
        Ok(key)
    }

    /// A JSON array of numbers, its `[` read.
    fn list(&mut self, mut each: impl FnMut(&mut Scan<'a>) -> Result<()>) -> Result<()> {
        if self.eat(']') {
            return Ok(());
        }
        loop {
            each(self)?;
            if self.eat(']') {
                return Ok(());
            }
            self.expect(',')?;
        }
    }

    fn numbers(&mut self) -> Result<Vec<f64>> {
        self.expect('[')?;
        let mut out = Vec::new();
        self.list(|s| s.number().map(|x| out.push(x)))?;
        Ok(out)
    }

    fn wholes(&mut self) -> Result<Vec<u32>> {
        self.expect('[')?;
        let mut out = Vec::new();
        self.list(|s| s.whole().map(|x| out.push(x)))?;
        Ok(out)
    }
}

/// Reads a text form.
///
/// # Errors
///
/// SQLCODE -301, SQLSTATE 22018, if `text` is not one (VEC-0005); 22003
/// for NaN or infinity (VEC-0007).
pub(crate) fn parse(text: &str) -> Result<Parsed> {
    let mut s = Scan { rest: text };
    let parsed = if s.eat('{') { object(&mut s)? } else { array(&mut s)? };
    if s.peek().is_some() {
        return Err(bad_literal("text after the vector"));
    }
    Ok(parsed)
}

fn array(s: &mut Scan<'_>) -> Result<Parsed> {
    s.expect('[')?;
    if s.eat(']') {
        return Ok(Parsed::Dense(Vec::new()));
    }
    let first = s.number()?;
    let mut values = vec![first];
    if s.eat(']') {
        return Ok(Parsed::Dense(values));
    }
    s.expect(',')?;
    if s.peek() == Some('[') {
        let dims = float::integral(first).and_then(|d| u32::try_from(d).ok()).ok_or_else(|| bad_literal("a sparse dimension count"))?;
        let indices = s.wholes()?;
        s.expect(',')?;
        let found = s.numbers()?;
        s.expect(']')?;
        return pairs(dims, indices, found);
    }
    s.list(|s| s.number().map(|x| values.push(x)))?;
    Ok(Parsed::Dense(values))
}

fn object(s: &mut Scan<'_>) -> Result<Parsed> {
    let (mut dims, mut indices, mut values) = (None, None, None);
    loop {
        let key = s.key()?;
        if key == DIMS_KEY {
            dims = Some(s.whole()?);
        } else if key == INDICES_KEY {
            indices = Some(s.wholes()?);
        } else if key == VALUES_KEY {
            values = Some(s.numbers()?);
        } else {
            return Err(bad_literal(&format!("an unknown member \"{key}\"")));
        }
        if s.eat('}') {
            break;
        }
        s.expect(',')?;
    }
    match (dims, indices, values) {
        (Some(d), Some(i), Some(v)) => pairs(d, i, v),
        _ => Err(bad_literal("a sparse object needs dims, indices and values")),
    }
}

fn pairs(dims: u32, indices: Vec<u32>, values: Vec<f64>) -> Result<Parsed> {
    if indices.len() != values.len() {
        return Err(bad_literal("a sparse vector needs as many values as indexes"));
    }
    Ok(Parsed::Sparse(dims, indices.into_iter().zip(values).collect()))
}

/// The vector a text form gives a type of shape `shape`: its format the
/// shape's, else FLOAT32 (R5.1); its layout the shape's, if fixed; its dimension
/// count the shape's, if fixed (R5.2).
///
/// # Errors
///
/// As `parse`; SQLCODE -301 for a dimension count not the shape's, an
/// element out of its format's range, or a BINARY byte not 0 to 255.
pub(crate) fn vector(text: &str, shape: Shape) -> Result<Vector> {
    let element = shape.element.unwrap_or(Element::Float32);
    let made = match parse(text)? {
        Parsed::Dense(values) => {
            let values = if matches!(element, Element::Binary) { bits(&values)? } else { values };
            Vector::dense(element, values)?
        }
        Parsed::Sparse(dims, pairs) => Vector::sparse(element, dims, pairs)?,
    };
    if let Some(dims) = shape.dims
        && dims != made.dims
    {
        return Err(mismatch(&format!("dimension mismatch: expected {dims}, got {}", made.dims)));
    }
    match shape.layout {
        Some(layout) => made.laid(layout),
        None => Ok(made),
    }
}

/// BINARY's bytes as its bits, the first in each byte's high bit.
fn bits(bytes: &[f64]) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(bytes.len().saturating_mul(8));
    for x in bytes {
        let byte = float::integral(*x)
            .and_then(|b| u8::try_from(b).ok())
            .ok_or_else(|| out_of_range(&format!("{x} is not a byte of a BINARY vector")))?;
        out.extend((0..BITS_PER_BYTE).map(|i| f64::from(u8::from(byte & (HIGH_BIT >> i) != 0))));
    }
    Ok(out)
}

/// A number as a vector of `element` writes it.
pub(crate) fn number(x: f64, element: Element) -> String {
    match element {
        Element::Float64 => plain(x),
        Element::Float32 => {
            let s = super::single(x);
            let magnitude = f64::from(s.abs());
            if s == 0.0 || (PLAIN_LOW..PLAIN_HIGH).contains(&magnitude) { format!("{s}") } else { format!("{s:e}") }
        }
        Element::Float16 => {
            let bits = half_bits(x);
            let shortest =
                (0..HALF_DIGITS).filter_map(|p| format!("{x:.p$e}").parse::<f64>().ok()).find(|y| half_bits(*y) == bits).unwrap_or(x);
            plain(shortest)
        }
        Element::Int8 | Element::Binary => float::integral(x).map_or_else(|| plain(x), |i| i.to_string()),
    }
}

fn plain(x: f64) -> String {
    if x == 0.0 || (PLAIN_LOW..PLAIN_HIGH).contains(&x.abs()) { format!("{x}") } else { format!("{x:e}") }
}

fn joined(items: impl Iterator<Item = String>) -> String {
    format!("[{}]", items.collect::<Vec<_>>().join(","))
}

/// `v` in its text form: a JSON array, BINARY as bytes, sparse as
/// `[dims,[indexes],[values]]`.
pub(crate) fn write(v: VectorRef<'_>) -> String {
    let values = (0..v.stored()).map(|i| number(v.value(i).unwrap_or_default(), v.element));
    match (v.layout(), v.element) {
        (Layout::Dense, Element::Binary) => joined(v.bytes.iter().map(u8::to_string)),
        (Layout::Dense, _) => joined(values),
        (Layout::Sparse, _) => {
            let indices = joined((0..v.stored()).map(|i| v.index(i).unwrap_or_default().to_string()));
            format!("[{},{indices},{}]", v.dims, joined(values))
        }
    }
}

/// `v` in `layout`'s text form, a dense one's values written sparse or
/// a sparse one's dense (FROM_VECTOR's FORMAT).
pub(crate) fn write_as(v: VectorRef<'_>, layout: Layout) -> String {
    match (v.layout(), layout) {
        (Layout::Dense, Layout::Dense) | (Layout::Sparse, Layout::Sparse) => write(v),
        (Layout::Sparse, Layout::Dense) => joined((0..v.dims).map(|d| number(v.at(d).unwrap_or_default(), v.element))),
        (Layout::Dense, Layout::Sparse) => {
            let kept: Vec<(u32, f64)> = (0..v.dims).filter_map(|d| Some((d, v.at(d)?))).filter(|(_, x)| *x != 0.0_f64).collect();
            let indices = joined(kept.iter().map(|(d, _)| d.to_string()));
            format!("[{},{indices},{}]", v.dims, joined(kept.iter().map(|(_, x)| number(*x, v.element))))
        }
    }
}
