//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68] The parts of a
//! well-known binary representation with a layout of their own: affine
//! placements, the curves given by parameters, NURBS curves and TINs, as
//! `model/encode.rs` writes them.

use super::decode::{COUNT_BYTES, HEADER_BYTES, NUMBER_BYTES, Reader, wrong};
use super::encode::EMPTY_ORDINATE;
use super::{Body, Clothoid, Elliptical, Knot, Measures, Nurbs, NurbsPoint, Placement, Spiral, SpiralType, Surface, Tin, TinElement};
use super::{WKB_AFFINEPLACEMENT, WKB_Z};
use crate::error::Result;

/// A curve given by parameters, read: its placement, its numbers and its
/// measures.
type Parametric<const N: usize> = (Placement, [f64; N], Option<Measures>);

/// The measures read of a curve given by parameters.
#[derive(Clone, Copy)]
enum Measured {
    /// None, of a value without m.
    NoM,
    /// The empty curve's.
    Empty,
    /// These.
    Of(Measures),
}

impl Measured {
    /// The measures a curve that is not empty has.
    fn given(self) -> Option<Measures> {
        if let Measured::Of(m) = self { Some(m) } else { None }
    }
}

/// The bytes of a NURBS control point's order and weight flag.
const POINT_FLAG_BYTES: usize = 2;
/// The bytes of a knot: its order, value and multiplicity.
const KNOT_BYTES: usize = 10;

impl Reader<'_> {
    /// [§5.1.68 `<affineplacement binary representation>`]; none of the
    /// empty form, a location of NaN and no direction.
    fn placement(&mut self) -> Result<Option<Placement>> {
        let little = self.order()?;
        let wanted = if self.dims().has_z() { WKB_AFFINEPLACEMENT.saturating_add(WKB_Z) } else { WKB_AFFINEPLACEMENT };
        if self.uint(little)? != wanted {
            return Err(wrong("an affine placement's code is not 102, or 1102 of a value with z"));
        }
        let planar = self.dims().planar();
        let location = self.maybe_empty(little, planar)?;
        let count = self.count(little, planar.saturating_mul(NUMBER_BYTES))?;
        let directions = self.numbers(little, count.saturating_mul(planar))?;
        match (location, count) {
            (Some(location), _) => Ok(Some(Placement { location, directions })),
            (None, 0) => Ok(None),
            (None, 1..) => Err(wrong("an empty affine placement has no reference direction")),
        }
    }

    /// A curve given by a placement and `N` numbers, then its measures of
    /// a value with m: none if it is empty or left out.
    fn parametric<const N: usize>(&mut self, little: bool) -> Result<Option<Parametric<N>>> {
        if self.omitted() {
            return Ok(None);
        }
        let placement = self.placement()?;
        let numbers = self.maybe_empty(little, N)?;
        let measures = self.measures(little)?;
        match (placement, numbers, measures) {
            (Some(p), Some(n), Measured::NoM | Measured::Of(_)) => {
                Ok(Some((p, n.try_into().map_err(|_| wrong("the bytes end too soon"))?, measures.given())))
            }
            (None, None, Measured::NoM | Measured::Empty) => Ok(None),
            (Some(_) | None, Some(_) | None, Measured::NoM | Measured::Empty | Measured::Of(_)) => {
                Err(wrong("a curve given by parameters is partly empty"))
            }
        }
    }

    /// The measures of a value with m.
    fn measures(&mut self, little: bool) -> Result<Measured> {
        if !self.dims().has_m() {
            return Ok(Measured::NoM);
        }
        Ok(self.maybe_empty(little, 2)?.map_or(Measured::Empty, |m| {
            Measured::Of(Measures { start: m.first().copied().unwrap_or_default(), end: m.get(1).copied().unwrap_or_default() })
        }))
    }

    pub(super) fn elliptical(&mut self, little: bool) -> Result<Option<Elliptical>> {
        Ok(self.parametric::<4>(little)?.map(|(placement, [u_axis, v_axis, start_angle, end_angle], measures)| Elliptical {
            placement,
            u_axis,
            v_axis,
            start_angle,
            end_angle,
            measures,
        }))
    }

    pub(super) fn clothoid(&mut self, little: bool) -> Result<Option<Clothoid>> {
        Ok(self.parametric::<3>(little)?.map(|(placement, [scale_factor, start_distance, end_distance], measures)| Clothoid {
            placement,
            scale_factor,
            start_distance,
            end_distance,
            measures,
        }))
    }

    pub(super) fn spiral(&mut self, little: bool) -> Result<Option<Spiral>> {
        if self.omitted() {
            return Ok(None);
        }
        let placement = self.placement()?;
        let numbers = self.maybe_empty(little, 3)?;
        let name = self.letters()?;
        let measures = self.measures(little)?;
        let (Some(placement), Some(numbers), Measured::NoM | Measured::Of(_)) = (placement, numbers, measures) else {
            return if name.is_empty() { Ok(None) } else { Err(wrong("an empty spiral names no spiral type")) };
        };
        let measures = measures.given();
        let spiral_type =
            SpiralType::named(&name).ok_or_else(|| wrong("a spiral type is none of clothoid, bloss, biquadratic, sine and cosine"))?;
        if self.canonical && spiral_type.name() != name {
            return Err(wrong("a stored spiral type is not in lower case"));
        }
        let [length, start_curvature, end_curvature] = numbers.try_into().map_err(|_| wrong("the bytes end too soon"))?;
        Ok(Some(Spiral { placement, length, start_curvature, end_curvature, spiral_type, measures }))
    }

    /// `<byte> <letters>`: a length, then that many letters.
    fn letters(&mut self) -> Result<String> {
        let length = usize::from(self.byte()?);
        let (taken, rest) = self.rest.split_at_checked(length).ok_or_else(|| wrong("the bytes end too soon"))?;
        self.rest = rest;
        let text =
            std::str::from_utf8(taken).ok().filter(|t| super::parts::letters(t)).ok_or_else(|| wrong("letters are not of <letters>"))?;
        Ok(text.to_owned())
    }

    /// [§5.1.68 `<nurbscurve binary representation>`].
    pub(super) fn nurbs(&mut self, little: bool) -> Result<Option<Nurbs>> {
        if self.omitted() {
            return Ok(None);
        }
        let degree = self.byte()?;
        let planar = self.dims().planar();
        let count = self.count(little, planar.saturating_mul(NUMBER_BYTES).saturating_add(POINT_FLAG_BYTES))?;
        self.charge(count)?;
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            points.push(self.nurbs_point(planar)?);
        }
        let knot_count = self.count(little, KNOT_BYTES)?;
        self.charge(knot_count)?;
        let mut knots = Vec::with_capacity(knot_count);
        for _ in 0..knot_count {
            let order = self.order()?;
            let value = self.number(order)?;
            knots.push(Knot { value, multiplicity: self.byte()? });
        }
        let measures = self.measures(little)?;
        if points.is_empty() && degree == 0 && knots.is_empty() {
            return Ok(None);
        }
        match measures {
            Measured::NoM | Measured::Of(_) => Ok(Some(Nurbs { degree, points, knots, measures: measures.given() })),
            Measured::Empty => Err(wrong("a NURBS curve that is not empty has measures of a value with m")),
        }
    }

    fn nurbs_point(&mut self, planar: usize) -> Result<NurbsPoint> {
        let little = self.order()?;
        let point = self.numbers(little, planar)?;
        let weight = match self.byte()? {
            0 => None,
            1 => Some(self.number(little)?),
            2..=u8::MAX => return Err(wrong("a <bit> is a byte, 0 or 1")),
        };
        Ok(NurbsPoint { point, weight })
    }

    /// [§5.1.68 `<tin binary representation>`].
    pub(super) fn tin(&mut self, little: bool, depth: usize) -> Result<Tin> {
        let empty = Tin { triangles: Vec::new(), elements: Vec::new(), max_side_length: None };
        if self.omitted() {
            return Ok(empty);
        }
        let triangles = self.parts(little, depth, |_, body| match body {
            Body::Surface(Surface::Triangle(ring)) => Some(ring),
            Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
        })?;
        let count = self.count(little, HEADER_BYTES.saturating_add(COUNT_BYTES))?;
        let mut elements = Vec::with_capacity(count);
        for _ in 0..count {
            elements.push(self.element(depth)?);
        }
        let max = self.number(little)?;
        let null = if self.canonical { max.to_bits() == EMPTY_ORDINATE } else { max.is_nan() };
        let max_side_length = if null { None } else { Some(max) };
        Ok(Tin { triangles, elements, max_side_length })
    }

    fn element(&mut self, depth: usize) -> Result<TinElement> {
        self.charge(1)?;
        let little = self.order()?;
        let element_type = self.letters()?;
        let id = self.int(little)?;
        let tag = self.letters()?;
        let (_, geometry) = self.geometry(depth)?;
        Ok(TinElement { element_type, id, tag, geometry: Box::new(geometry) })
    }
}
