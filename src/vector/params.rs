//! \[EXTENSION\] A VECTOR type's parameters (design document §3.1):
//! `VECTOR`, `VECTOR(dims)` and `VECTOR(dims, format [, layout])`, kept
//! in the two unsigned integers a type beyond the standard has (RULES.md
//! D53).
//!
//! The first is the dimension count, 0 for `*`. The second, if written,
//! is the format's code (`Element::code`), 0 for `*`, plus `LAYOUT_STEP`
//! for SPARSE. With no parameters every dimension count and format is
//! taken (R3.1), and either layout. A plain VECTOR parameter of a routine
//! must take a sparse vector of more dimensions than a dense one may
//! have, so DENSE is fixed only where written. With the count alone the
//! format is FLOAT32 and the layout DENSE (R3.2). The parser reads `*` and the words through
//! `param_word`, then folds a third parameter into the second with
//! `pack_params`.

use super::build::dims_fault;
use super::kind::VECTOR_KIND;
use super::{Element, Layout, NAME, bad_type};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::DataType;
use crate::value::types::{ExtensionType, TypeParams};

/// The value of `*`, a dimension count or format left open.
pub const ANY: u32 = 0;
/// The position of the format among the written parameters.
pub const FORMAT_PARAM: usize = 1;
/// The position of the layout among the written parameters.
pub const LAYOUT_PARAM: usize = 2;
/// What SPARSE adds to the second stored parameter.
pub const LAYOUT_STEP: u32 = 256;
/// The `*` of `dim_spec` and `format_spec`.
const STAR: &str = "*";
/// The parameters' encoding in the catalog.
const VERSION: u8 = 1;
/// The most written parameters: dimension count, format and layout.
const MOST_WRITTEN: usize = 3;

/// What a VECTOR type fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// Its dimension count, if fixed.
    pub dims: Option<u32>,
    /// Its element format, if fixed.
    pub element: Option<Element>,
    /// Its layout, if fixed. Plain VECTOR takes either.
    pub layout: Option<Layout>,
}

/// The open shape, plain VECTOR's (R3.1).
pub(crate) const OPEN: Shape = Shape { dims: None, element: None, layout: None };

/// The value a word or `*` written as parameter `at` (from 0) stands for,
/// if one: `*` for the dimension count or format, a format's name for
/// the format, DENSE or SPARSE for the layout, in any case (R3.3).
pub fn param_word(at: usize, word: &str) -> Option<u32> {
    let star = word.trim() == STAR;
    match at {
        0 => star.then_some(ANY),
        FORMAT_PARAM => {
            if star {
                return Some(ANY);
            }
            Element::named(word).map(|e| u32::from(e.code()))
        }
        LAYOUT_PARAM => Layout::named(word).map(|l| match l {
            Layout::Dense => 0,
            Layout::Sparse => LAYOUT_STEP,
        }),
        _ => None,
    }
}

/// The stored parameters of `written`, each as a number or as
/// `param_word` gave it: a third, the layout, folded into the second.
pub fn pack_params(written: &[u32]) -> Option<Vec<u32>> {
    match written {
        [dims, format, layout] => Some(vec![*dims, format.checked_add(*layout)?]),
        _ => (written.len() < MOST_WRITTEN).then(|| written.to_vec()),
    }
}

/// The shape stored parameters `params` give.
pub(crate) fn shape(params: &[u32]) -> Result<Shape> {
    let dims = |d: u32| (d != ANY).then_some(d);
    let shaped = match params {
        [] => OPEN,
        [d] => Shape { dims: dims(*d), element: Some(Element::Float32), layout: Some(Layout::Dense) },
        [d, second] => {
            let code = second % LAYOUT_STEP;
            let layout = match second / LAYOUT_STEP {
                0 => Layout::Dense,
                1 => Layout::Sparse,
                _ => return Err(bad_type("the layout is DENSE or SPARSE")),
            };
            let element = if code == ANY {
                None
            } else {
                let code = u8::try_from(code).ok().and_then(Element::of_code);
                Some(code.ok_or_else(|| bad_type("the format is FLOAT32, FLOAT16, FLOAT64, INT8, BINARY or *"))?)
            };
            Shape { dims: dims(*d), element, layout: Some(layout) }
        }
        _ => return Err(bad_type("it takes a dimension count, a format and a layout")),
    };
    check(shaped)?;
    Ok(shaped)
}

/// Refuses a shape the design document does (R3.4, R3.8).
fn check(s: Shape) -> Result<()> {
    let element = s.element.unwrap_or(Element::Float32);
    if matches!((s.element, s.layout), (Some(Element::Binary), Some(Layout::Sparse))) {
        return Err(bad_type("BINARY SPARSE is not a layout; use DENSE BINARY"));
    }
    match s.dims.and_then(|d| dims_fault(d, element, s.layout.unwrap_or(Layout::Dense))) {
        Some(fault) => Err(bad_type(&fault)),
        None => Ok(()),
    }
}

/// The stored parameters of `s`.
fn stored(s: Shape) -> Vec<u32> {
    let dims = s.dims.unwrap_or(ANY);
    let code = s.element.map_or(ANY, |e| u32::from(e.code()));
    let layout = if matches!(s.layout, Some(Layout::Sparse)) { LAYOUT_STEP } else { 0 };
    match (s.dims, s.element, s.layout) {
        (None, None, None) => Vec::new(),
        (_, Some(Element::Float32), Some(Layout::Dense)) => vec![dims],
        _ => vec![dims, code.saturating_add(layout)],
    }
}

/// The VECTOR type of shape `s`.
///
/// # Errors
///
/// SQLCODE -103 if the design document refuses `s` (R3.4, R3.8).
pub fn typed(s: Shape) -> Result<DataType> {
    check(s)?;
    let params = TypeParams::of(&stored(s)).ok_or_else(|| bad_type("its parameters"))?;
    Ok(DataType::Extension(ExtensionType { kind: &VECTOR_KIND, params }))
}

/// What `t` fixes, if it is a VECTOR type.
pub fn shape_of(t: &DataType) -> Option<Shape> {
    let DataType::Extension(e) = t else { return None };
    if e.kind.name() != NAME {
        return None;
    }
    shape(e.params.values()).ok()
}

/// A dense column's dimension count and format, if `t` is a VECTOR type
/// fixing both: as a vector index needs (R3.6, R10.3).
pub fn fixed_shape(t: &DataType) -> Option<(u32, Element)> {
    let s = shape_of(t)?;
    match s.layout? {
        Layout::Dense => Some((s.dims?, s.element?)),
        Layout::Sparse => None,
    }
}

/// `t` as written.
pub(crate) fn written_of(t: &DataType) -> String {
    let Some(s) = shape_of(t) else { return NAME.to_owned() };
    let dims = s.dims.map_or_else(|| STAR.to_owned(), |d| d.to_string());
    match (stored(s).len(), s.layout) {
        (0, _) => NAME.to_owned(),
        (1, _) => format!("{NAME}({dims})"),
        (_, None | Some(Layout::Dense)) => format!("{NAME}({dims}, {})", s.element.map_or(STAR, Element::name)),
        (_, Some(Layout::Sparse)) => format!("{NAME}({dims}, {}, {})", s.element.map_or(STAR, Element::name), Layout::Sparse.name()),
    }
}

/// Appends `t`'s parameters to `out`.
pub(crate) fn put(t: &DataType, out: &mut Vec<u8>) {
    let params = shape_of(t).map(stored).unwrap_or_default();
    out.put_u8(VERSION);
    out.put_u8(u8::try_from(params.len()).unwrap_or_default());
    for p in params {
        out.put_u32(p);
    }
}

/// Reads parameters `put` wrote.
pub(crate) fn read(r: &mut Reader<'_>) -> Result<DataType> {
    if r.u8()? != VERSION {
        return Err(r.corrupt("unknown VECTOR descriptor version"));
    }
    let count = r.u8()?;
    let mut params = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        params.push(r.u32()?);
    }
    let s = shape(&params).map_err(|_| r.corrupt("invalid VECTOR parameters"))?;
    typed(s).map_err(|_| r.corrupt("invalid VECTOR parameters"))
}
