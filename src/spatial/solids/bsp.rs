//! \[DESIGN\] Binary space partitions of solids, for point membership
//! (Thibault and Naylor, "Set Operations on Polyhedra Using Binary Space
//! Partitioning Trees", 1987). A solid's boundary faces, convex and facing
//! out, are split by the planes of each other: a piece falling off a
//! node's front is outside the solid, off its back inside. A piece lying
//! in a node's plane is classified in both subtrees; where they differ it
//! is on the boundary, and the subtree saying outside tells which way the
//! boundary faces. Trees are built and walked with explicit stacks, so no
//! solid's size bounds a recursion.

use std::rc::Rc;

use super::kernel::Plane;
use super::pieces::{Piece, Poly, Side};

/// Where a piece lies against a solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// In its interior.
    In,
    /// In its exterior.
    Out,
    /// On its boundary, facing as the boundary does (polygons), or just
    /// on it (points and segments).
    OnSame,
    /// On its boundary, facing against it.
    OnOpposite,
}

/// A node: its plane, and its subtrees, None a leaf.
#[derive(Debug)]
struct Node {
    plane: Rc<Plane>,
    front: Option<usize>,
    back: Option<usize>,
}

/// The partition of one solid.
#[derive(Debug, Default)]
pub struct Tree {
    nodes: Vec<Node>,
}

/// A slot a subtree is to fill: the root, or a node's front or back.
#[derive(Clone, Copy)]
enum Slot {
    Root,
    Front(usize),
    Back(usize),
}

impl Tree {
    /// The partition of a solid of boundary `faces`, which close and face
    /// out; None for no faces.
    pub fn of(faces: Vec<Poly>) -> Option<Tree> {
        let mut tree = Tree::default();
        let mut work = vec![(Slot::Root, faces)];
        while let Some((slot, mut group)) = work.pop() {
            let Some(first) = group.pop() else { continue };
            let plane = first.support.clone();
            let (mut front, mut back) = (Vec::new(), Vec::new());
            for face in group {
                match face.split(&plane) {
                    Side::Front(f) => front.push(f),
                    Side::Back(b) => back.push(b),
                    Side::Both(f, b) => {
                        front.push(f);
                        back.push(b);
                    }
                    Side::On(..) => {}
                }
            }
            let at = tree.nodes.len();
            tree.nodes.push(Node { plane, front: None, back: None });
            tree.link(slot, at);
            if !front.is_empty() {
                work.push((Slot::Front(at), front));
            }
            if !back.is_empty() {
                work.push((Slot::Back(at), back));
            }
        }
        (!tree.nodes.is_empty()).then_some(tree)
    }

    fn link(&mut self, slot: Slot, at: usize) {
        match slot {
            Slot::Root => {}
            Slot::Front(parent) => {
                if let Some(node) = self.nodes.get_mut(parent) {
                    node.front = Some(at);
                }
            }
            Slot::Back(parent) => {
                if let Some(node) = self.nodes.get_mut(parent) {
                    node.back = Some(at);
                }
            }
        }
    }

    /// The parts of `piece` with where each lies against the solid.
    pub fn classify<T: Piece>(&self, piece: T) -> Vec<(T, Label)> {
        let mut out = Vec::new();
        let mut work = vec![Task { piece, at: Some(0), leaf: Label::Out, frames: Vec::new() }];
        while let Some(task) = work.pop() {
            self.step(task, &mut work, &mut out);
        }
        out
    }

    /// One step of classifying: down one node, or out of a leaf.
    fn step<T: Piece>(&self, task: Task<T>, work: &mut Vec<Task<T>>, out: &mut Vec<(T, Label)>) {
        let Some(node) = task.at.and_then(|at| self.nodes.get(at)) else {
            return Self::settle(task, work, out);
        };
        let down = |piece: T, front: bool, frames: Vec<Frame>| {
            let (at, leaf) = if front { (node.front, Label::Out) } else { (node.back, Label::In) };
            Task { piece, at, leaf, frames }
        };
        match task.piece.split(&node.plane) {
            Side::Front(f) => work.push(down(f, true, task.frames)),
            Side::Back(b) => work.push(down(b, false, task.frames)),
            Side::Both(f, b) => {
                work.push(down(f, true, task.frames.clone()));
                work.push(down(b, false, task.frames));
            }
            Side::On(piece, same) => {
                let mut frames = task.frames;
                frames.push(Frame { back: node.back, same, front_label: None });
                work.push(down(piece, true, frames));
            }
        }
    }

    /// A piece out of a leaf of label `task.leaf`: done, or on to the back
    /// subtree of the node it lay in, or combined with that node's front.
    fn settle<T: Piece>(mut task: Task<T>, work: &mut Vec<Task<T>>, out: &mut Vec<(T, Label)>) {
        let mut label = task.leaf;
        while let Some(frame) = task.frames.pop() {
            match frame.front_label {
                None => {
                    let leaf = Label::In;
                    task.frames.push(Frame { front_label: Some(label), ..frame });
                    work.push(Task { piece: task.piece, at: frame.back, leaf, frames: task.frames });
                    return;
                }
                Some(front) => label = combine(front, label, frame.same),
            }
        }
        out.push((task.piece, label));
    }
}

/// A node a piece lay in: classified in the front subtree, then the back.
#[derive(Clone, Copy)]
struct Frame {
    back: Option<usize>,
    same: bool,
    front_label: Option<Label>,
}

/// A piece on its way down: the node it is at, None at a leaf of label
/// `leaf`, and the nodes it lay in.
struct Task<T> {
    piece: T,
    at: Option<usize>,
    leaf: Label,
    frames: Vec<Frame>,
}

/// The label of a piece lying in a node's plane, `front` in its front
/// subtree and `back` in its back one, facing the plane's way if `same`.
fn combine(front: Label, back: Label, same: bool) -> Label {
    let (Some(outer), Some(inner)) = (inside(front), inside(back)) else {
        return if inside(front).is_none() { front } else { back };
    };
    let facing = |forward: bool| if forward { Label::OnSame } else { Label::OnOpposite };
    match (outer, inner) {
        (false, true) => facing(same),
        (true, false) => facing(!same),
        (true, true) => Label::In,
        (false, false) => Label::Out,
    }
}

/// Whether `label` is inside (true) or outside (false); None on the
/// boundary.
fn inside(label: Label) -> Option<bool> {
    match label {
        Label::In => Some(true),
        Label::Out => Some(false),
        Label::OnSame | Label::OnOpposite => None,
    }
}
