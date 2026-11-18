//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.14, §11.3.15]
//! ST_RemEdgeNewFace and ST_RemEdgeModFace: an edge removed and, if it
//! separated two faces, the two healed into one. The universal face
//! absorbs the other; else a new face replaces both, or the left face is
//! kept [§11.3.15 Description 2e iv, implementation-dependent].

use super::{Face, Topology, UNIVERSAL_FACE, damaged};
use crate::error::Result;

/// The rule of ST_RemEdgeNewFace's checks.
const NEW: &str = "§11.3.14 Description 2e";
/// The rule of ST_RemEdgeModFace's checks.
const MODIFY: &str = "§11.3.15 Description 2e";

impl Topology {
    /// [§11.3.14, §11.3.15] Edge `edge` removed, the faces it separated
    /// healed: into a new face if `new_face`, whose identifier is returned.
    /// Reading: the text returns a value only of a new face; else the
    /// function returns the null value. A node no edge ends at any more is
    /// kept [Description 2e iii], isolated in the healed face.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF43 (non-existent edge).
    pub fn remove_edge(&mut self, edge: i32, new_face: bool) -> Result<Option<i32>> {
        let rule = if new_face { NEW } else { MODIFY };
        let e = self.edge(edge, &format!("{rule} i"))?.clone();
        self.edges.remove(&edge);
        self.relink(&[e.start, e.end])?;
        let (f1, f2) = (e.left_face, e.right_face);
        let (kept, gone, made) = if f1 == f2 {
            (f1, Vec::new(), None)
        } else if f1 == UNIVERSAL_FACE {
            (f1, vec![f2], None)
        } else if f2 == UNIVERSAL_FACE {
            (f2, vec![f1], None)
        } else if new_face {
            let id = self.new_face_id()?;
            (id, vec![f1, f2], Some(id))
        } else {
            (f1, vec![f2], None)
        };
        if made.is_some() {
            self.faces.insert(kept, Face { mbr: None });
        }
        for f in &gone {
            self.faces.remove(f);
        }
        self.replace_faces(&gone, kept);
        self.isolate(&[e.start, e.end], kept);
        self.refresh_mbr(&[kept])?;
        if !self.faces.contains_key(&kept) {
            return Err(damaged(&format!("face {kept} is not there")));
        }
        Ok(made)
    }

    /// Every reference to a face of `gone` made to `kept`.
    fn replace_faces(&mut self, gone: &[i32], kept: i32) {
        let to = |f: i32| if gone.contains(&f) { kept } else { f };
        for e in self.edges.values_mut() {
            e.left_face = to(e.left_face);
            e.right_face = to(e.right_face);
        }
        for n in self.nodes.values_mut() {
            n.containing_face = n.containing_face.map(to);
        }
    }
}
