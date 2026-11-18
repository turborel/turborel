//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.2, §8.6.3, §17.1.2 to
//! §17.1.6] The routines of TINs and their elements: the ST_TINElement
//! constructors, observers and mutators, the ST_TIN constructors of
//! triangles and of elements, which the constrained Delaunay
//! triangulation of `crate::spatial::curves::tin` builds, and
//! ST_TINElements, its observer and its mutator (RULES.md
//! D-SPATIAL-CURVES).

use std::sync::Arc;

use super::model::{Body as ModelBody, Geom, Kind, Surface as ModelSurface};
use super::observers::{DOUBLE, INTEGER, signature};
use super::representations::spatial;
use super::tin_element::{self, Parts, TAG_LENGTH, TYPE_LENGTH};
use super::{MAX_GEOMETRY_ARRAY_ELEMENTS, data_type, data_type_of, kind};
use crate::error::{Result, SqlState};
use crate::functions::ScalarFunction;
use crate::spatial::curves::tin::{ElementKind, triangulate};
use crate::spatial::formats::model::{from_geom, to_geom};
use crate::spatial::formats::tree::{self, Body, Element, Geometry, Surface, Tin};
use crate::value::{CharUnits, DataType, Value};

/// A routine of TINs and their elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TinRoutine {
    /// [§17.1.2] ST_TINElement(type, [id,] tag, geometry).
    Element,
    /// [§17.1.3] ST_ElementType() and ST_ElementType(type).
    ElementType,
    /// [§17.1.4] ST_ElementID() and ST_ElementID(id).
    ElementId,
    /// [§17.1.5] ST_ElementTag() and ST_ElementTag(tag).
    ElementTag,
    /// [§17.1.6] ST_ElementGeometry() and ST_ElementGeometry(geometry).
    ElementGeometry,
    /// [§8.6.2] ST_TIN(triangles, elements, maxsidelength [, srid]) and
    /// ST_TIN(elements, maxsidelength [, srid]).
    Tin,
    /// [§8.6.3] ST_TINElements() and ST_TINElements(elements,
    /// triangulate).
    Elements,
    /// [§17.1.7] ST_IsEmpty() of an element, a method alone: the
    /// geometry types' ST_IsEmpty has its name as a function
    /// (`binder/structured.rs`).
    IsEmpty,
}

/// The routines.
pub const TIN_ROUTINES: [TinRoutine; 7] = [
    TinRoutine::Element,
    TinRoutine::ElementType,
    TinRoutine::ElementId,
    TinRoutine::ElementTag,
    TinRoutine::ElementGeometry,
    TinRoutine::Tin,
    TinRoutine::Elements,
];

/// The arguments of ST_TINElement without an identifier, and of ST_TIN
/// of elements and an SRID or of triangles.
const WITHOUT_ID: usize = 3;
/// The arguments of ST_TINElement with an identifier, and of ST_TIN of
/// triangles and an SRID.
const WITH_ID: usize = 4;
/// The arguments of a mutator of an element, and of ST_TIN of elements.
const MUTATOR: usize = 2;
/// The position of ST_TIN's maximum side length, its SRID after it, in
/// the form of triangles.
const MAX_SIDE_AT: usize = 2;

/// ST_TIN's parameter `position`: (triangles, elements, max) or
/// (elements, max), then the SRID. The arrays are taken as they are given.
fn tin_parameter(position: usize, known: &[Option<DataType>]) -> Option<DataType> {
    let triangles = matches!(known.get(1), Some(Some(DataType::Array(_))));
    let at = if triangles { position } else { position.saturating_add(1) };
    if at == MAX_SIDE_AT {
        return Some(DOUBLE);
    }
    (at == MAX_SIDE_AT.saturating_add(1)).then_some(INTEGER)
}

/// The routines as functions.
pub(crate) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    TIN_ROUTINES.iter().map(|r| -> Arc<dyn ScalarFunction> { Arc::new(*r) }).collect()
}

fn varying(n: usize) -> DataType {
    DataType::Varying { length: u32::try_from(n).unwrap_or(u32::MAX), units: CharUnits::Characters }
}

fn elements_type() -> DataType {
    DataType::array(tin_element::data_type(), MAX_GEOMETRY_ARRAY_ELEMENTS)
}

fn text(v: Option<&Value>, name: &str) -> Result<String> {
    let Some(Value::Char(s)) = v else { return Err(signature(name)) };
    Ok(s.clone())
}

fn integer(v: Option<&Value>, name: &str) -> Result<i32> {
    let Some(Value::Exact(d)) = v else { return Err(signature(name)) };
    i32::try_from(d.units).ok().filter(|_| d.scale == 0).ok_or_else(|| signature(name))
}

fn geometry(v: Option<&Value>, name: &str) -> Result<Geom> {
    let v = v.ok_or_else(|| signature(name))?;
    kind::payload(v).ok_or_else(|| signature(name))?;
    Geom::from_value(v)
}

fn items(v: Option<&Value>, name: &str) -> Result<Vec<Value>> {
    let Some(Value::Array(all)) = v else { return Err(signature(name)) };
    Ok(all.to_vec())
}

impl TinRoutine {
    /// The arities it takes.
    fn arities(self) -> &'static [usize] {
        match self {
            Self::Element => &[0, WITHOUT_ID, WITH_ID],
            Self::IsEmpty => &[1],
            Self::ElementType | Self::ElementId | Self::ElementTag | Self::ElementGeometry => &[1, MUTATOR],
            Self::Tin => &[MUTATOR, WITHOUT_ID, WITH_ID],
            Self::Elements => &[1, WITHOUT_ID],
        }
    }

    /// The type of the parameter at `position` of a call of `n` arguments.
    fn parameter(self, position: usize, n: usize, known: &[Option<DataType>]) -> Option<DataType> {
        if self == Self::Element {
            // ST_TINElement(type, [id,] tag, geometry).
            let at = if n == WITH_ID { position } else { position.saturating_add(usize::from(position > 0)) };
            return [varying(TYPE_LENGTH), INTEGER, varying(TAG_LENGTH), data_type()].get(at).cloned();
        }
        if self == Self::Tin {
            return tin_parameter(position, known);
        }
        if self == Self::Elements && position > 0 {
            // The elements are taken as they are given, the flag is INTEGER.
            return (position == MUTATOR).then_some(INTEGER);
        }
        let first = if self == Self::Elements { data_type_of(Kind::Tin) } else { tin_element::data_type() };
        let second = match self {
            Self::ElementType => varying(TYPE_LENGTH),
            Self::ElementId => INTEGER,
            Self::ElementTag => varying(TAG_LENGTH),
            Self::ElementGeometry | Self::Element | Self::Tin | Self::Elements | Self::IsEmpty => data_type(),
        };
        [first, second].get(position).cloned()
    }

    fn element(self, args: &[Value]) -> Result<Value> {
        if args.is_empty() {
            return Ok(tin_element::empty());
        }
        let name = self.name_of();
        let element_type = text(args.first(), name)?;
        let (id, rest) = if args.len() == WITH_ID { (integer(args.get(1), name)?, args.get(MUTATOR..)) } else { (0_i32, args.get(1..)) };
        let [tag, g] = rest.unwrap_or_default() else { return Err(signature(name)) };
        tin_element::element(&element_type, id, &text(Some(tag), name)?, &geometry(Some(g), name)?)
    }

    /// An observer or mutator of an element.
    fn part(self, args: &[Value]) -> Result<Value> {
        let name = self.name_of();
        let mut parts = Parts::of_value(args.first().ok_or_else(|| signature(name))?)?;
        let Some(new) = args.get(1) else {
            // [§17.1.3 Description 2a] An observer of the empty set gives
            // the null value, as of a part not given.
            return Ok(match self {
                Self::ElementType => parts.element_type.map_or(Value::Null, Value::Char),
                Self::ElementId => parts.id.map_or(Value::Null, |id| Value::Exact(crate::value::Decimal::new(i128::from(id), 0))),
                Self::ElementTag => parts.tag.map_or(Value::Null, Value::Char),
                Self::IsEmpty => Value::Exact(crate::value::Decimal::new(i128::from(parts.is_empty()), 0)),
                Self::ElementGeometry | Self::Element | Self::Tin | Self::Elements => parts.geometry.map_or(Value::Null, |g| g.to_value()),
            });
        };
        match self {
            Self::ElementType => parts.element_type = Some(text(Some(new), name)?),
            Self::ElementId => parts.id = Some(integer(Some(new), name)?),
            Self::ElementTag => parts.tag = Some(text(Some(new), name)?),
            Self::ElementGeometry | Self::Element | Self::Tin | Self::Elements | Self::IsEmpty => {
                parts.geometry = Some(geometry(Some(new), name)?);
            }
        }
        parts.to_value()
    }

    fn name_of(self) -> &'static str {
        match self {
            Self::Element => "ST_TINELEMENT",
            Self::ElementType => "ST_ELEMENTTYPE",
            Self::ElementId => "ST_ELEMENTID",
            Self::ElementTag => "ST_ELEMENTTAG",
            Self::ElementGeometry => "ST_ELEMENTGEOMETRY",
            Self::Tin => "ST_TIN",
            Self::Elements => "ST_TINELEMENTS",
            Self::IsEmpty => "ST_ISEMPTY",
        }
    }
}

/// The tree's element of `parts`.
///
/// [§8.6.2 Description 4] An element of a TIN has a type and a geometry.
fn tree_element(parts: &Parts) -> Result<(Element, Geometry)> {
    let kind = parts
        .element_type
        .as_deref()
        .and_then(ElementKind::from_name)
        .ok_or_else(|| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", "an element of no known TIN element type"))?;
    let g = parts.geometry.as_ref().ok_or_else(|| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", "an element of no geometry"))?;
    let value = from_geom(g)?;
    let tag = parts.tag.clone().unwrap_or_default();
    Ok((Element { kind, id: parts.id.unwrap_or(0), tag, body: value.body.clone() }, value))
}

/// [§8.6.2] ST_TIN of `triangles`, if given, else of the triangulation
/// of `elements`, with `max_side`, in reference system `srid`.
fn tin(triangles: Option<Vec<Value>>, elements: &[Value], max_side: f64, srid: i32) -> Result<Value> {
    let parts = elements.iter().filter(|v| !v.is_null()).map(Parts::of_value).collect::<Result<Vec<_>>>()?;
    let trees = parts.iter().map(tree_element).collect::<Result<Vec<_>>>()?;
    let given = triangles
        .map(|all| all.iter().filter(|v| !v.is_null()).map(|v| from_geom(&geometry(Some(v), "ST_TIN")?)).collect::<Result<Vec<_>>>());
    let given = given.transpose()?;
    let dims = given.as_ref().and_then(|g| g.first()).or_else(|| trees.first().map(|(_, g)| g)).map_or(super::model::Dims::Xy, |g| g.dims);
    let max = (max_side > 0.0_f64).then_some(max_side);
    let rings: Vec<tree::Ring> = if let Some(given) = given {
        given
            .into_iter()
            .map(|t| if let Body::Surface(Surface::Triangle(ring)) = t.body { Ok(ring) } else { Err(signature("ST_TIN")) })
            .collect::<Result<_>>()?
    } else {
        let constraints: Vec<_> = trees.iter().map(|(e, _)| tree::constraint(e, dims)).collect();
        triangulate(&constraints, max)?.into_iter().map(|[a, b, c]| vec![a, b, c, a]).collect()
    };
    let kept = trees.into_iter().map(|(e, _)| e).collect();
    let body = Body::Surface(Surface::Tin(Tin { triangles: rings, elements: kept, max_side: max }));
    Ok(to_geom(&Geometry { dims, body }, srid)?.to_value())
}

/// [§8.6.3 Description 4] ST_TINElements(elements, triangulate): `g`
/// with `elements`, its triangles those of their triangulation within
/// its maximum side length if `triangulate` is 1 and there are any, and
/// its own else.
fn with_elements(g: &Geom, elements: &[Value], again: bool) -> Result<Value> {
    let parts = elements.iter().filter(|v| !v.is_null()).map(Parts::of_value).collect::<Result<Vec<_>>>()?;
    if parts.iter().any(|p| p.geometry.as_ref().is_some_and(|e| e.srid != g.srid)) {
        return Err(spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "§8.6.3 Description 4b", "mixed spatial reference systems"));
    }
    let trees = parts.iter().map(tree_element).collect::<Result<Vec<_>>>()?;
    let mut tree = from_geom(g)?;
    let dims = tree.dims;
    let Body::Surface(Surface::Tin(t)) = &mut tree.body else { return Err(signature("ST_TINELEMENTS")) };
    t.elements = trees.into_iter().map(|(e, _)| e).collect();
    if again && !t.elements.is_empty() {
        let constraints: Vec<_> = t.elements.iter().map(|e| tree::constraint(e, dims)).collect();
        t.triangles = triangulate(&constraints, t.max_side)?.into_iter().map(|[a, b, c]| vec![a, b, c, a]).collect();
    }
    Ok(to_geom(&tree, g.srid)?.to_value())
}

impl ScalarFunction for TinRoutine {
    fn name(&self) -> &str {
        self.name_of()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if !self.arities().contains(&args.len()) {
            return Err(signature(self.name_of()));
        }
        Ok(match (self, args.len()) {
            (Self::Element, _) | (Self::ElementType | Self::ElementId | Self::ElementTag | Self::ElementGeometry, MUTATOR) => {
                tin_element::data_type()
            }
            (Self::ElementType, _) => varying(TYPE_LENGTH),
            (Self::ElementId | Self::IsEmpty, _) => INTEGER,
            (Self::ElementTag, _) => varying(TAG_LENGTH),
            (Self::ElementGeometry, _) => data_type(),
            (Self::Tin, _) | (Self::Elements, WITHOUT_ID) => data_type_of(Kind::Tin),
            (Self::Elements, _) => elements_type(),
        })
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        self.parameter(position, known.len(), known)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        match self {
            Self::Element => self.element(args),
            Self::ElementType | Self::ElementId | Self::ElementTag | Self::ElementGeometry | Self::IsEmpty => self.part(args),
            Self::Elements => {
                let name = self.name_of();
                let g = geometry(args.first(), name)?;
                if args.len() == WITHOUT_ID {
                    return with_elements(&g, &items(args.get(1), name)?, integer(args.get(MUTATOR), name)? == 1);
                }
                let ModelBody::Surface(ModelSurface::Tin(t)) = &g.body else { return Err(signature(self.name_of())) };
                let all = t.elements.iter().map(|e| Parts::of_model(e, &g).to_value()).collect::<Result<Vec<_>>>()?;
                Ok(Value::Array(all.into()))
            }
            Self::Tin => {
                let name = self.name_of();
                // The triangles' form has its elements second.
                let triangles = matches!(args.get(1), Some(Value::Array(_)));
                let (given, rest) =
                    if triangles { (Some(items(args.first(), name)?), args.get(1..).unwrap_or_default()) } else { (None, args) };
                let max = super::observers::number(rest.get(1), name)?;
                let srid = rest.get(2).map(|v| integer(Some(v), name)).transpose()?.unwrap_or(0);
                tin(given, &items(rest.first(), name)?, max, srid)
            }
        }
    }

    fn deterministic(&self) -> bool {
        true
    }
}
