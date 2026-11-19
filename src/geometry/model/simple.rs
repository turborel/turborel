//! \[EXTENSION\] [OGC 06-103r4 §8.2.8] The well-known binary
//! representation of Simple feature access where it lays a value out
//! otherwise than ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68 does: a
//! triangle as a polygon of one ring (WKBTriangle), and a TIN as its
//! triangles, each a polygon, with no elements or maximum side length
//! after them (WKBTIN). Every other type is laid out alike in both
//! (RULES.md D136).
//!
//! A reader given bytes reads them as §5.1.68's first, then as these
//! (`model/decode.rs`); `write_simple_wkb` writes these, for AsBinary of
//! Simple feature access (`model/encode.rs`).

use super::decode::{Reader, wrong};
use super::{Body, Coords, Kind, Surface, Tin};
use crate::error::Result;

impl Reader<'_> {
    /// A triangle's body: of §5.1.68, its three points; of Simple feature
    /// access, its rings, none or one.
    pub(super) fn triangle_of(&mut self, little: bool) -> Result<Coords> {
        if !self.simple {
            return self.triangle(little);
        }
        let mut rings = self.rings(little)?;
        let ring = rings.pop().unwrap_or_default();
        if rings.is_empty() { Ok(ring) } else { Err(wrong("a triangle has one ring")) }
    }

    /// A TIN's body: of §5.1.68, its triangles, elements and maximum side
    /// length; of Simple feature access, its triangles alone, each a
    /// polygon or a triangle.
    pub(super) fn tin_of(&mut self, little: bool, depth: usize) -> Result<Tin> {
        if !self.simple {
            return self.tin(little, depth);
        }
        let triangles = self.parts(little, depth, triangle_ring)?;
        Ok(Tin { triangles, elements: Vec::new(), max_side_length: None })
    }
}

/// The ring of a TIN's part of Simple feature access: a triangle's, or a
/// polygon's of one ring; none of any other part. The model checks that
/// it is a closed ring of four points.
fn triangle_ring(_: Kind, body: Body) -> Option<Coords> {
    let Body::Surface(surface) = body else { return None };
    if let Surface::Triangle(ring) = surface {
        return Some(ring);
    }
    let Surface::Polygon(mut rings) = surface else { return None };
    let ring = rings.pop().unwrap_or_default();
    rings.is_empty().then_some(ring)
}

#[cfg(test)]
mod tests {
    use super::super::{Dims, Geom, read_wkb, write_simple_wkb, write_wkb};
    use super::*;

    fn triangle() -> Coords {
        vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    }

    /// Verifies: [EXTENSION] [OGC 06-103r4 §8.2.8] A triangle and a TIN are
    /// written as Simple feature access lays them out, read back by the
    /// reader of ISO/IEC 13249-3's, and differ from that text's; a value
    /// of neither is written alike in both.
    #[test]
    fn triangles_and_tins_round_trip_in_both_layouts() {
        let tin = Tin { triangles: vec![triangle(), triangle()], elements: Vec::new(), max_side_length: None };
        for body in [Body::Surface(Surface::Triangle(triangle())), Body::Surface(Surface::Tin(tin))] {
            let g = Geom::new(4326, Dims::Xy, body).unwrap();
            let simple = write_simple_wkb(&g);
            assert_ne!(simple, write_wkb(&g));
            assert_eq!(read_wkb(&simple, 4326).unwrap(), g);
            assert_eq!(read_wkb(&write_wkb(&g), 4326).unwrap(), g);
        }
        let empty = Geom::new(0, Dims::Xyz, Body::Surface(Surface::Triangle(Vec::new()))).unwrap();
        assert_eq!(read_wkb(&write_simple_wkb(&empty), 0).unwrap(), empty);
        let polygon = Geom::new(0, Dims::Xy, Body::Surface(Surface::Polygon(vec![triangle()]))).unwrap();
        assert_eq!(write_simple_wkb(&polygon), write_wkb(&polygon));
    }

    /// Verifies: [EXTENSION] [OGC 06-103r4 §8.2.8] A triangle of two rings,
    /// or a TIN of a point, is no representation in either layout.
    #[test]
    fn malformed_simple_parts_are_refused() {
        let ring = |w: &mut Vec<u8>| {
            w.extend_from_slice(&4_u32.to_le_bytes());
            for n in triangle() {
                w.extend_from_slice(&n.to_le_bytes());
            }
        };
        let mut two = vec![1_u8];
        two.extend_from_slice(&17_u32.to_le_bytes());
        two.extend_from_slice(&2_u32.to_le_bytes());
        ring(&mut two);
        ring(&mut two);
        assert_eq!(read_wkb(&two, 0).unwrap_err().sqlstate().as_str(), "2FF23");
        let mut of_point = vec![1_u8];
        of_point.extend_from_slice(&16_u32.to_le_bytes());
        of_point.extend_from_slice(&1_u32.to_le_bytes());
        of_point.push(1);
        of_point.extend_from_slice(&1_u32.to_le_bytes());
        of_point.extend_from_slice(&0.0_f64.to_le_bytes());
        of_point.extend_from_slice(&0.0_f64.to_le_bytes());
        assert!(read_wkb(&of_point, 0).is_err());
    }
}
