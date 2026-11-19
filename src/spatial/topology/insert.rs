//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.12, §11.3.13]
//! ST_AddEdgeNewFaces and ST_AddEdgeModFace: an edge added between two
//! nodes, splitting the face it runs through if it closes a ring.
//!
//! The new edge runs through one face, found from the edges at its start
//! node. Once its next edges and those at its nodes are set from the
//! stars, the rings on its two sides are traced. One ring on both sides
//! means it split nothing. Two rings mean it closed a ring of edges: a
//! ring that turns counterclockwise bounds a new face, one that turns
//! clockwise keeps the outside, which is the face that was split. What
//! else lay in that face, holes and isolated nodes, goes to whichever new
//! face holds it.

use super::edges::simple_line;
use super::predicates::{Side, counterclockwise, side};
use super::{Edge, Face, Point, Topology, UNIVERSAL_FACE, damaged, fault, line_of, point_of};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// What becomes of the face a new edge splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Faces {
    /// [§11.3.12] Two new faces replace it.
    New,
    /// [§11.3.13] It is kept on one side and a new face made on the other.
    Modify,
}

/// One side of a new edge after it is added: its ring and the face it
/// gets.
struct Part {
    /// The ring of signed edges.
    ring: Vec<i32>,
    /// Its points, closed.
    points: Vec<Point>,
    /// Whether it turns counterclockwise, bounding the area on its left.
    bounds: bool,
}

impl Topology {
    /// [§11.3.12, §11.3.13] An edge of `curve` added from node `start` to
    /// node `end`, the face it splits replaced as `faces` says; its
    /// identifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF29 (curve not simple); 2FF46 (non-existent node); 2FF57,
    /// 2FF33; 2FF34 (geometry crosses a node); 2FF35 (geometry crosses an
    /// edge); 2FF27 (coincident edge); 2FF10.
    pub fn add_edge(&mut self, start: i32, end: i32, curve: &Geom, faces: Faces) -> Result<i32> {
        let rule = match faces {
            Faces::New => "§11.3.12 Description 2h",
            Faces::Modify => "§11.3.13 Description 2h",
        };
        self.same_srid(curve, "§11.3.12 Description 3")?;
        let line = simple_line(curve, "§11.3.12 Description 2g")?;
        self.node(start, &format!("{rule} i"))?;
        self.node(end, &format!("{rule} ii"))?;
        let (p1, p2) = self.ends_at(&line, (start, end), &format!("{rule} iii"))?;
        self.no_isolated_node_on(&line, &format!("{rule} v"))?;
        self.coincident(&line, (start, end), &format!("{rule} vii"))?;
        self.meets_no_edge(&line, (None, &[p1, p2]), (&format!("{rule} vi"), SqlState::SPATIAL_GEOMETRY_CROSSES_AN_EDGE))?;
        self.insert_edge((start, end), curve, faces)
    }

    /// [§11.3.12 Description 2h vii, viii] That no edge joins the two nodes
    /// along the same curve, either way. Reading: an edge along the same
    /// points meets the new curve along its length, which the check of
    /// crossings refuses too, so this one, first in the text's order of
    /// the two that apply, is made before it.
    fn coincident(&self, line: &[Point], (start, end): (i32, i32), rule: &str) -> Result<()> {
        for (id, e) in &self.edges {
            let joins = (e.start == start && e.end == end) || (e.start == end && e.end == start);
            if !joins {
                continue;
            }
            let mut other = line_of(&e.geometry)?;
            if e.start != start {
                other.reverse();
            }
            let same = other.len() == line.len() && other.iter().zip(line).all(|(a, b)| super::predicates::same(*a, *b));
            if same {
                return Err(fault(rule, SqlState::SPATIAL_COINCIDENT_EDGE, &format!("edge {id} joins the nodes along the same curve")));
            }
        }
        Ok(())
    }

    /// The edge of `curve` from `start` to `end`, checked, inserted with
    /// its faces set and the face it splits replaced as `faces` says.
    pub(super) fn insert_edge(&mut self, (start, end): (i32, i32), curve: &Geom, faces: Faces) -> Result<i32> {
        let line = line_of(curve)?;
        let toward = *line.get(1).ok_or_else(|| damaged("a curve of one point"))?;
        let face = match self.face_toward(start, toward)? {
            Some(f) => f,
            None => self.nodes.get(&start).and_then(|n| n.containing_face).unwrap_or(UNIVERSAL_FACE),
        };
        let id = self.new_edge_id()?;
        self.edges
            .insert(id, Edge { start, end, next_left: id, next_right: id, left_face: face, right_face: face, geometry: curve.clone() });
        self.connect(&[start, end]);
        self.adopt_srid(curve);
        self.relink(&[start, end])?;
        let back = id.checked_neg().ok_or_else(|| damaged("an edge identifier has no negation"))?;
        let left = self.ring(id)?;
        if left.contains(&back) {
            return Ok(id);
        }
        let right = self.ring(back)?;
        self.split(face, [self.part(left)?, self.part(right)?], faces)?;
        Ok(id)
    }

    /// The side of a new edge that `ring` walks.
    fn part(&self, ring: Vec<i32>) -> Result<Part> {
        let points = self.ring_points(&ring)?;
        let bounds = counterclockwise(&points);
        Ok(Part { ring, points, bounds })
    }

    /// Face `face` split by a new edge into the two sides `parts`, the
    /// left one first.
    fn split(&mut self, face: i32, parts: [Part; 2], faces: Faces) -> Result<()> {
        let [left, right] = parts;
        // The side that keeps the outside: the one that turns clockwise,
        // else the right one.
        let (inner, outer) = if !left.bounds && right.bounds { (right, left) } else { (left, right) };
        let (inner_face, outer_face) = self.split_ids(face, faces)?;
        self.assign(&inner.ring, inner_face)?;
        self.assign(&outer.ring, outer_face)?;
        let rings: Vec<(&Vec<Point>, i32)> =
            [(&inner, inner_face), (&outer, outer_face)].into_iter().filter(|(p, _)| p.bounds).map(|(p, f)| (&p.points, f)).collect();
        let fallback = if outer.bounds { inner_face } else { outer_face };
        self.rehome(face, (&inner.ring, &outer.ring), (&rings, fallback))?;
        if faces == Faces::New && face != UNIVERSAL_FACE {
            self.faces.remove(&face);
        }
        for f in [inner_face, outer_face] {
            self.faces.entry(f).or_insert(Face { mbr: None });
        }
        self.refresh_mbr(&[inner_face, outer_face])
    }

    /// The faces of the inner and the outer side of a split of `face`: a new
    /// face inside, and outside the face itself, unless two new faces
    /// replace it [§11.3.12 Description 2h ix 3C]. The universal face is
    /// never replaced. When both sides bound area, the left one is new.
    fn split_ids(&self, face: i32, faces: Faces) -> Result<(i32, i32)> {
        let first = self.new_face_id()?;
        let second = first.checked_add(1).ok_or_else(|| damaged("no face identifier is left"))?;
        let keeps = faces == Faces::Modify || face == UNIVERSAL_FACE;
        Ok((first, if keeps { face } else { second }))
    }

    /// The faces on the left of each signed edge of `ring` set to `face`.
    pub(super) fn assign(&mut self, ring: &[i32], face: i32) -> Result<()> {
        for signed in ring {
            let edge = self.edges.get_mut(&signed.saturating_abs()).ok_or_else(|| damaged("a ring passes an edge that is not there"))?;
            if *signed > 0_i32 {
                edge.left_face = face;
            } else {
                edge.right_face = face;
            }
        }
        Ok(())
    }

    /// What lay in face `face` and is on neither side ring of the split,
    /// edges and isolated nodes, given to the bounded ring of `rings` that
    /// holds it, or `fallback`.
    fn rehome(&mut self, face: i32, (inner, outer): (&[i32], &[i32]), (rings, fallback): (&[(&Vec<Point>, i32)], i32)) -> Result<()> {
        let holder = |p: Point| rings.iter().find(|(r, _)| side(p, r) == Side::Inside).map_or(fallback, |(_, f)| *f);
        let on_rings = |id: i32| inner.iter().chain(outer).any(|s| s.saturating_abs() == id);
        let mut moves = Vec::new();
        for (id, e) in &self.edges {
            if on_rings(*id) || (e.left_face != face && e.right_face != face) {
                continue;
            }
            moves.push((*id, holder(self.at(e.start)?)));
        }
        for (id, to) in moves {
            if let Some(e) = self.edges.get_mut(&id) {
                e.left_face = if e.left_face == face { to } else { e.left_face };
                e.right_face = if e.right_face == face { to } else { e.right_face };
            }
        }
        let mut nodes = Vec::new();
        for (id, n) in &self.nodes {
            if n.containing_face == Some(face) {
                nodes.push((*id, holder(point_of(&n.geometry)?)));
            }
        }
        for (id, to) in nodes {
            if let Some(n) = self.nodes.get_mut(&id) {
                n.containing_face = Some(to);
            }
        }
        Ok(())
    }

    /// [§11.3.12 Description 2h ix 3E, F] The minimum bounding rectangle of
    /// each face of `faces` but the universal one set from its geometry.
    pub(super) fn refresh_mbr(&mut self, faces: &[i32]) -> Result<()> {
        let mut shapes = self.shapes()?;
        for f in faces {
            if *f == UNIVERSAL_FACE {
                continue;
            }
            let mbr = shapes.remove(f).and_then(|s| super::rings::mbr(&s, self.srid_or_unknown()));
            if let Some(face) = self.faces.get_mut(f) {
                face.mbr = mbr;
            }
        }
        Ok(())
    }
}
