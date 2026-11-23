//! [OGC 06-104r4 §7.2.20] The text types of Simple feature access, SQL
//! option, in its "SQL with Geometry Types" environment, the class tS
//! [Annex A.2.6.2, A.4.1 e] (RULES.md D136).
//!
//! * ANNOTATION_TEXT_ELEMENT is a text string, its location, its leader
//!   line and its text attributes, each read by a method of its name and
//!   changed by the method of its name of one argument. Its constructor
//!   method, of four arguments, is ANNOTATION_TEXT_ELEMENT, the type's
//!   name [ISO/IEC 9075-2:2023 §4.9.3.3], and the function
//!   AnnotationTextElement the clause names.
//! * ANNOTATION_TEXT_ELEMENT_ARRAY is a varying array of elements:
//!   ElementN(n) is its element n, the null value past its end, and
//!   ElementN(element, n) the array with element n replaced, or added
//!   after the last.
//! * ANNOTATION_TEXT is its elements and their envelope: the envelope of
//!   their locations and leader lines, each of one SRID, or the null
//!   value if they have none. CONCAT appends another text's elements.
//!
//! Each type has the constructor function every structured type has,
//! its attributes the null value, and the array none [§4.9.3.3]. Each
//! routine gives the null value of a null argument, as the clause
//! declares them RETURNS NULL ON NULL INPUT. MaxArraySize, the length of
//! a text string and of an array, is `MAX_ARRAY_SIZE`. The text has no
//! well-known text or binary [Annex A.2.6 Note]; a value is shown as the
//! constructors that make it.

use super::Support::{self, AnnotationText as T, TextElement as E, TextElementArray as A};
use super::ops::{Op, Ty, arg, elements, integer, text};
use super::types::{constructed, geometry_literal, quoted};
use super::values::{bytes_of, geometry_of, not_of, put_geometry, put_some, read_all, some};
use super::wrap;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::geometry::model::{Body, Collection, Dims, Geom, Kind};
use crate::value::Value;

/// MaxArraySize: the characters of a text string or of text attributes,
/// and the elements of an array.
pub const MAX_ARRAY_SIZE: u32 = 4096;

const ELEMENT: Ty = Ty::Of(E);
const ARRAY: Ty = Ty::Of(A);
const TEXT: Ty = Ty::Of(T);
const STRING: Ty = Ty::Varchar(MAX_ARRAY_SIZE);
const GEOMETRY: Ty = Ty::Geom(Kind::Geometry);
const ELEMENTS: Ty = Ty::OfArray(E);
const ELEMENT_PARAMS: &[Ty] = &[STRING, GEOMETRY, GEOMETRY, STRING];

/// A data exception `state` of the clause.
fn fault(state: SqlState, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[OGC 06-104r4 §7.2.20] {detail}")).with_state(state)
}

/// An ANNOTATION_TEXT_ELEMENT value: none of each attribute that is the
/// null value.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct Element {
    pub value: Option<String>,
    pub location: Option<Vec<u8>>,
    pub leader: Option<Vec<u8>>,
    pub attributes: Option<String>,
}

impl Element {
    fn put(&self, out: &mut Vec<u8>) {
        put_some(out, self.value.as_deref(), Put::put_str);
        put_some(out, self.location.as_deref(), put_geometry);
        put_some(out, self.leader.as_deref(), put_geometry);
        put_some(out, self.attributes.as_deref(), Put::put_str);
    }

    fn read(r: &mut Reader<'_>) -> Result<Element> {
        let (value, location) = (some(r, Reader::str)?, some(r, geometry_of)?);
        Ok(Element { value, location, leader: some(r, geometry_of)?, attributes: some(r, Reader::str)? })
    }

    pub(super) fn value_of(&self) -> Value {
        let mut out = Vec::new();
        self.put(&mut out);
        wrap(E, out)
    }

    fn of(v: &Value) -> Result<Element> {
        read_all(bytes_of(v, E)?, E, Element::read)
    }

    /// The constructor of its four attributes, each a string or a
    /// geometry value.
    fn constructed(args: &[Value]) -> Result<Value> {
        let location = Some(geometry(args, 1)?);
        Ok(Element { value: Some(string(args, 0)?), location, leader: Some(geometry(args, 2)?), attributes: Some(string(args, 3)?) }
            .value_of())
    }

    /// The value of it with `change` made of `args`.
    fn changed(args: &[Value], change: fn(&mut Element, &[Value]) -> Result<()>) -> Result<Value> {
        let mut e = Element::of(arg(args, 0)?)?;
        change(&mut e, args)?;
        Ok(e.value_of())
    }

    /// The literal of a value of it.
    fn shown(&self) -> String {
        let parts = [
            ("VALUE", self.value.as_deref().map(quoted)),
            ("LOCATION", self.location.as_deref().map(geometry_literal)),
            ("LEADERLINE", self.leader.as_deref().map(geometry_literal)),
            ("TEXTATTRIBUTES", self.attributes.as_deref().map(quoted)),
        ];
        constructed(E.name(), &parts)
    }
}

/// Argument `at`, a text string of at most `MAX_ARRAY_SIZE` characters.
fn string(args: &[Value], at: usize) -> Result<String> {
    let s = text(args, at)?;
    if s.chars().count() > usize::try_from(MAX_ARRAY_SIZE).unwrap_or(usize::MAX) {
        return Err(fault(SqlState::STRING_DATA_RIGHT_TRUNCATION, "a text string is longer than MaxArraySize"));
    }
    Ok(s)
}

/// Argument `at`, a geometry value's payload.
fn geometry(args: &[Value], at: usize) -> Result<Vec<u8>> {
    Ok(super::ops::geometry(args, at)?.encode())
}

/// Elements, as an array or a text holds them.
fn put_elements(out: &mut Vec<u8>, all: &[Element]) {
    out.put_u32(u32::try_from(all.len()).unwrap_or(u32::MAX));
    for e in all {
        e.put(out);
    }
}

fn read_elements(r: &mut Reader<'_>) -> Result<Vec<Element>> {
    let n = r.u32()?;
    (0..n).map(|_| Element::read(r)).collect()
}

/// An ANNOTATION_TEXT_ELEMENT_ARRAY value's elements.
pub(super) fn array_value(all: &[Element]) -> Value {
    let mut out = Vec::new();
    put_elements(&mut out, all);
    wrap(A, out)
}

fn array_of(v: &Value) -> Result<Vec<Element>> {
    read_all(bytes_of(v, A)?, A, read_elements)
}

/// The literal of an array of `all`.
fn array_shown(all: &[Element]) -> String {
    if all.is_empty() {
        return format!("NEW {}()", A.name());
    }
    let each: Vec<String> = all.iter().map(Element::shown).collect();
    format!("{}(ARRAY[{}])", A.name(), each.join(", "))
}

/// The elements of the SQL array argument `at`, none of them null and at
/// most `MAX_ARRAY_SIZE`.
fn listed(args: &[Value], at: usize) -> Result<Vec<Element>> {
    let items = elements(args, at)?;
    if items.len() > usize::try_from(MAX_ARRAY_SIZE).unwrap_or(usize::MAX) {
        return Err(fault(SqlState::ARRAY_ELEMENT_ERROR, "an array holds more than MaxArraySize elements"));
    }
    if items.iter().any(Value::is_null) {
        return Err(fault(SqlState::SPATIAL_ELEMENT_IS_A_NULL_VALUE, "an element of the array is the null value"));
    }
    items.iter().map(Element::of).collect()
}

/// ElementN(n): element n, counted from 1, or the null value past the
/// array's ends.
fn element_n(args: &[Value]) -> Result<Value> {
    let all = array_of(arg(args, 0)?)?;
    let at = usize::try_from(integer(args, 1)?).ok().and_then(|n| n.checked_sub(1));
    Ok(at.and_then(|i| all.get(i)).map_or(Value::Null, Element::value_of))
}

/// ElementN(element, n): the array with element n `element`, or with
/// `element` after its last of n one past it.
fn set_element_n(args: &[Value]) -> Result<Value> {
    let mut all = array_of(arg(args, 0)?)?;
    let element = Element::of(arg(args, 1)?)?;
    let len = all.len();
    let room = usize::try_from(MAX_ARRAY_SIZE).unwrap_or(usize::MAX);
    let at = usize::try_from(integer(args, 2)?).ok().and_then(|n| n.checked_sub(1)).filter(|i| *i <= len && *i < room);
    let at = at.ok_or_else(|| fault(SqlState::ARRAY_ELEMENT_ERROR, "the position is not in the array or one past it"))?;
    if let Some(slot) = all.get_mut(at) {
        *slot = element;
    } else {
        all.push(element);
    }
    Ok(array_value(&all))
}

/// An ANNOTATION_TEXT value: its envelope's payload and its elements,
/// none of each that is the null value.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct Text {
    pub envelope: Option<Vec<u8>>,
    pub elements: Option<Vec<Element>>,
}

impl Text {
    pub(super) fn value_of(&self) -> Value {
        let mut out = Vec::new();
        put_some(&mut out, self.envelope.as_deref(), put_geometry);
        put_some(&mut out, self.elements.as_deref(), put_elements);
        wrap(T, out)
    }

    fn of(v: &Value) -> Result<Text> {
        read_all(bytes_of(v, T)?, T, |r| Ok(Text { envelope: some(r, geometry_of)?, elements: some(r, read_elements)? }))
    }

    /// The text of `elements`, with their envelope.
    fn of_elements(elements: Vec<Element>) -> Result<Value> {
        Ok(Text { envelope: envelope(&elements)?, elements: Some(elements) }.value_of())
    }

    /// The literal of a value of it.
    fn shown(&self) -> String {
        self.elements.as_deref().map_or_else(|| format!("NEW {}()", T.name()), |all| format!("{}({})", T.name(), array_shown(all)))
    }
}

/// The envelope of the locations and leader lines of `elements`, as
/// ST_Envelope gives it of them all [ISO/IEC 13249-3:2016 (DIS, SC32
/// N2593) §5.1.19]; none if they have none.
///
/// # Errors
///
/// SQLSTATE 2FF10 if two are of other SRIDs.
fn envelope(elements: &[Element]) -> Result<Option<Vec<u8>>> {
    let parts = elements.iter().flat_map(|e| [e.location.as_deref(), e.leader.as_deref()]).flatten();
    let mut srid = None;
    let mut corners = Vec::new();
    for bytes in parts {
        let g = Geom::decode(bytes)?;
        if srid.is_some_and(|s| s != g.srid) {
            return Err(fault(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "the elements' geometries are of other SRIDs"));
        }
        srid = Some(g.srid);
        if let Some(b) = g.bounds()? {
            corners.extend([b.lower().to_vec(), b.upper().to_vec()]);
        }
    }
    let Some(srid) = srid.filter(|_| !corners.is_empty()) else { return Ok(None) };
    let points = Geom::new(srid, Dims::Xy, Body::Collection(Collection::MultiPoint(corners)))?;
    let value = crate::geometry::mutators::envelope(&points)?;
    Ok(crate::geometry::kind::payload(&value).map(<[u8]>::to_vec))
}

/// The text's elements, none of a text whose elements are the null value.
fn text_elements(v: &Value) -> Result<Vec<Element>> {
    Ok(Text::of(v)?.elements.unwrap_or_default())
}

/// A text's envelope, the null value if it has none.
fn text_envelope(args: &[Value]) -> Result<Value> {
    Ok(Text::of(arg(args, 0)?)?.envelope.map_or(Value::Null, crate::geometry::kind::wrap))
}

/// A text string attribute, the null value if it is.
fn string_value(s: Option<String>) -> Value {
    s.map_or(Value::Null, |s| Value::Char(s.into()))
}

/// A geometry attribute, the null value if it is.
fn geometry_value(g: Option<Vec<u8>>) -> Value {
    g.map_or(Value::Null, crate::geometry::kind::wrap)
}

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(E, &[], |_, _| Ok(Element::default().value_of())),
    Op::constructor(E, ELEMENT_PARAMS, |a, _| Element::constructed(a)),
    Op::function("ANNOTATIONTEXTELEMENT", ELEMENT_PARAMS, ELEMENT, |a, _| Element::constructed(a)),
    Op::method("VALUE", &[ELEMENT], STRING, |a, _| Ok(string_value(Element::of(arg(a, 0)?)?.value))),
    Op::method("VALUE", &[ELEMENT, STRING], ELEMENT, |a, _| {
        Element::changed(a, |e, a| {
            e.value = Some(string(a, 1)?);
            Ok(())
        })
    }),
    Op::method("TEXTATTRIBUTES", &[ELEMENT], STRING, |a, _| Ok(string_value(Element::of(arg(a, 0)?)?.attributes))),
    Op::method("TEXTATTRIBUTES", &[ELEMENT, STRING], ELEMENT, |a, _| {
        Element::changed(a, |e, a| {
            e.attributes = Some(string(a, 1)?);
            Ok(())
        })
    }),
    Op::method("LOCATION", &[ELEMENT], GEOMETRY, |a, _| Ok(geometry_value(Element::of(arg(a, 0)?)?.location))),
    Op::method("LOCATION", &[ELEMENT, GEOMETRY], ELEMENT, |a, _| {
        Element::changed(a, |e, a| {
            e.location = Some(geometry(a, 1)?);
            Ok(())
        })
    }),
    Op::method("LEADERLINE", &[ELEMENT], GEOMETRY, |a, _| Ok(geometry_value(Element::of(arg(a, 0)?)?.leader))),
    Op::method("LEADERLINE", &[ELEMENT, GEOMETRY], ELEMENT, |a, _| {
        Element::changed(a, |e, a| {
            e.leader = Some(geometry(a, 1)?);
            Ok(())
        })
    }),
    Op::constructor(A, &[], |_, _| Ok(array_value(&[]))),
    Op::constructor(A, &[ELEMENTS], |a, _| Ok(array_value(&listed(a, 0)?))),
    Op::method("ELEMENTN", &[ARRAY, Ty::Integer], ELEMENT, |a, _| element_n(a)),
    Op::method("ELEMENTN", &[ARRAY, ELEMENT, Ty::Integer], ARRAY, |a, _| set_element_n(a)),
    Op::constructor(T, &[], |_, _| Ok(Text::default().value_of())),
    Op::constructor(T, &[ARRAY], |a, _| Text::of_elements(array_of(arg(a, 0)?)?)),
    Op::method("CONCAT", &[TEXT, TEXT], TEXT, |a, _| Text::of_elements([text_elements(arg(a, 0)?)?, text_elements(arg(a, 1)?)?].concat())),
    Op::method("ENVELOPE", &[TEXT], GEOMETRY, |a, _| text_envelope(a)),
    Op::method("ELEMENT_ARRAY", &[TEXT], ARRAY, |a, _| Ok(Text::of(arg(a, 0)?)?.elements.map_or(Value::Null, |all| array_value(&all)))),
];

/// The literal of `v`, a value of one of the text types `s`.
///
/// # Errors
///
/// SQLCODE -301 if `v` is no value of `s`.
pub(super) fn shown(s: Support, v: &Value) -> Result<String> {
    match s {
        Support::TextElement => Ok(Element::of(v)?.shown()),
        Support::TextElementArray => Ok(array_shown(&array_of(v)?)),
        Support::AnnotationText => Ok(Text::of(v)?.shown()),
        Support::SpatialRefSys
        | Support::Angle
        | Support::Direction
        | Support::Vector
        | Support::AffinePlacement
        | Support::NurbsPoint
        | Support::Knot => Err(not_of(s)),
    }
}

/// That `v` is a stored value of the text type `s`.
///
/// # Errors
///
/// As its bytes are no value of `s`.
pub(super) fn check(s: Support, v: &Value) -> Result<()> {
    shown(s, v).map(drop)
}
