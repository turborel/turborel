//! \[DESIGN\] The labelled planar topology graph of up to two operands.
//! Their points, line strings and rings are noded together; each vertex
//! becomes a node and each noded segment an edge, shared by every part
//! running along it. Edges carry, per operand, how many line strings and
//! rings run along them and the area depth change across them. Each
//! edge is two half-edges; each node orders its outgoing half-edges
//! counterclockwise, which links half-edges into face cycles. Cycles get
//! each operand's area depth, the winding number of its rings, so every
//! node, edge and face can be located in each operand.

mod depth;

use std::cmp::Ordering;
use std::collections::HashMap;

use super::Point;
use super::noding::node;
use super::predicates::orient2d;
use super::segment::lexicographic;
use crate::error::Result;

/// The operands a graph holds, an operation's two or a single one.
pub(crate) const INPUTS: usize = 2;

/// What a path contributes to its operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// An isolated point.
    Point,
    /// A line string, whose ends count toward the mod 2 boundary rule.
    Line,
    /// A ring raising the area depth on its left by this much.
    Ring(i32),
}

/// A part of an operand, as a polyline.
#[derive(Clone, Debug)]
pub(crate) struct Path {
    pub(crate) input: usize,
    pub(crate) role: Role,
    pub(crate) vertices: Vec<Point>,
}

/// A vertex of the noded arrangement.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) at: Point,
    /// Line-string ends here, per operand.
    pub(crate) ends: [u32; INPUTS],
    /// Whether an operand has an isolated point here.
    pub(crate) point: [bool; INPUTS],
}

/// A segment of the noded arrangement, from its lower to its higher node.
#[derive(Clone, Debug)]
pub(crate) struct Edge {
    pub(crate) ends: [usize; 2],
    /// Line strings running along it, per operand.
    pub(crate) lines: [u32; INPUTS],
    /// Area depth on its left minus its right, per operand.
    pub(crate) delta: [i32; INPUTS],
}

/// A face cycle, half-edges each followed by the next around the face on
/// their left. A component's outer cycle bounds the face around it.
#[derive(Clone, Debug)]
pub(crate) struct Cycle {
    pub(crate) outer: bool,
    pub(crate) depth: [i32; INPUTS],
    pub(crate) halves: Vec<usize>,
}

/// The arrangement with its stars, cycles and depths.
#[derive(Debug)]
pub(crate) struct Graph {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    /// Each node's outgoing half-edges, counterclockwise from just past
    /// the direction of negative x.
    pub(crate) star: Vec<Vec<usize>>,
    slot: Vec<usize>,
    /// Each half-edge's cycle.
    pub(crate) face: Vec<usize>,
    pub(crate) cycles: Vec<Cycle>,
    /// Each node's connected component.
    pub(crate) component: Vec<usize>,
    /// Per component, the depth of the face around it.
    pub(crate) outside: Vec<[i32; INPUTS]>,
}

/// The half-edge in the other direction.
pub(crate) fn twin(half: usize) -> usize {
    half ^ 1
}

/// The edge a half-edge belongs to.
pub(crate) fn edge_of(half: usize) -> usize {
    half >> 1_u32
}

fn key([x, y]: Point) -> [u64; 2] {
    // Adding zero turns a negative zero positive, so equal positions
    // share one node.
    [(x + 0.0_f64).to_bits(), (y + 0.0_f64).to_bits()]
}

struct Builder {
    nodes: Vec<Node>,
    index: HashMap<[u64; 2], usize>,
    edges: Vec<Edge>,
    edge_index: HashMap<(usize, usize), usize>,
}

impl Builder {
    fn node(&mut self, at: Point) -> usize {
        let next = self.nodes.len();
        let id = *self.index.entry(key(at)).or_insert(next);
        if id == next {
            self.nodes.push(Node { at, ends: [0; INPUTS], point: [false; INPUTS] });
        }
        id
    }

    fn edge(&mut self, from: usize, to: usize) -> (usize, bool) {
        let (low, high, forward) = if from < to { (from, to, true) } else { (to, from, false) };
        let next = self.edges.len();
        let id = *self.edge_index.entry((low, high)).or_insert(next);
        if id == next {
            self.edges.push(Edge { ends: [low, high], lines: [0; INPUTS], delta: [0_i32; INPUTS] });
        }
        (id, forward)
    }

    fn add(&mut self, path: &Path, vertices: &[Point]) {
        let input = path.input.min(INPUTS.saturating_sub(1));
        let ids: Vec<usize> = vertices.iter().map(|at| self.node(*at)).collect();
        let (Some(first), Some(last)) = (ids.first().copied(), ids.last().copied()) else { return };
        if ids.len() == 1 {
            if let Some(flag) = self.nodes.get_mut(first).and_then(|node| node.point.get_mut(input)) {
                *flag = true;
            }
            return;
        }
        if path.role == Role::Line {
            for end in [first, last] {
                if let Some(count) = self.nodes.get_mut(end).and_then(|node| node.ends.get_mut(input)) {
                    *count = count.saturating_add(1);
                }
            }
        }
        for pair in ids.windows(2) {
            let (Some(from), Some(to)) = (pair.first().copied(), pair.last().copied()) else { continue };
            let (id, forward) = self.edge(from, to);
            let Some(edge) = self.edges.get_mut(id) else { continue };
            match path.role {
                Role::Point | Role::Line => {
                    if let Some(count) = edge.lines.get_mut(input) {
                        *count = count.saturating_add(1);
                    }
                }
                Role::Ring(raise) => {
                    let signed = if forward { raise } else { raise.saturating_neg() };
                    if let Some(delta) = edge.delta.get_mut(input) {
                        *delta = delta.saturating_add(signed);
                    }
                }
            }
        }
    }
}

impl Graph {
    /// Nodes the paths together and builds the labelled graph.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if noding does not settle.
    pub(crate) fn build(paths: &[Path]) -> Result<Graph> {
        let noded = node(&paths.iter().map(|path| path.vertices.clone()).collect::<Vec<_>>())?;
        let mut builder = Builder { nodes: Vec::new(), index: HashMap::new(), edges: Vec::new(), edge_index: HashMap::new() };
        for (path, vertices) in paths.iter().zip(&noded) {
            builder.add(path, vertices);
        }
        let Builder { nodes, edges, .. } = builder;
        let mut graph = Graph {
            nodes,
            edges,
            star: Vec::new(),
            slot: Vec::new(),
            face: Vec::new(),
            cycles: Vec::new(),
            component: Vec::new(),
            outside: Vec::new(),
        };
        graph.order_stars();
        graph.trace_cycles();
        depth::assign(&mut graph);
        Ok(graph)
    }

    /// The node a half-edge leaves.
    pub(crate) fn origin(&self, half: usize) -> usize {
        let [low, high] = self.edges.get(edge_of(half)).map_or([0, 0], |edge| edge.ends);
        if half & 1 == 0 { low } else { high }
    }

    /// The node a half-edge reaches.
    pub(crate) fn head(&self, half: usize) -> usize {
        self.origin(twin(half))
    }

    /// A node's position.
    pub(crate) fn at(&self, node: usize) -> Point {
        self.nodes.get(node).map_or([0.0_f64; 2], |node| node.at)
    }

    /// An operand's area depth on the left of a half-edge.
    pub(crate) fn left_depth(&self, half: usize, input: usize) -> i32 {
        self.face.get(half).and_then(|cycle| self.cycles.get(*cycle)).map_or(0, |cycle| cycle.depth.get(input).copied().unwrap_or_default())
    }

    /// The area depth change across a half-edge, left minus right.
    pub(crate) fn delta(&self, half: usize, input: usize) -> i32 {
        let delta = self.edges.get(edge_of(half)).and_then(|edge| edge.delta.get(input).copied()).unwrap_or_default();
        if half & 1 == 0 { delta } else { delta.saturating_neg() }
    }

    /// The half-edge after `half` around the face on its left. At its
    /// head, it is the outgoing half-edge just clockwise of the way back.
    pub(crate) fn next(&self, half: usize) -> usize {
        let back = twin(half);
        let star = self.star.get(self.head(half)).map_or(&[][..], Vec::as_slice);
        let slot = self.slot.get(back).copied().unwrap_or_default();
        let previous = slot.checked_sub(1).unwrap_or(star.len().saturating_sub(1));
        star.get(previous).copied().unwrap_or(back)
    }

    /// The position of a half-edge in its origin's star.
    pub(crate) fn slot(&self, half: usize) -> usize {
        self.slot.get(half).copied().unwrap_or_default()
    }

    fn order_stars(&mut self) {
        let mut star = vec![Vec::new(); self.nodes.len()];
        for (id, edge) in self.edges.iter().enumerate() {
            let [low, high] = edge.ends;
            let forward = id.saturating_mul(2);
            star.get_mut(low).into_iter().for_each(|list: &mut Vec<usize>| list.push(forward));
            star.get_mut(high).into_iter().for_each(|list| list.push(forward.saturating_add(1)));
        }
        let mut slot = vec![0_usize; self.edges.len().saturating_mul(2)];
        for (node, list) in star.iter_mut().enumerate() {
            let centre = self.at(node);
            list.sort_by(|left, right| angular(centre, self.at(self.head(*left)), self.at(self.head(*right))));
            for (position, half) in list.iter().enumerate() {
                if let Some(entry) = slot.get_mut(*half) {
                    *entry = position;
                }
            }
        }
        self.star = star;
        self.slot = slot;
    }

    fn trace_cycles(&mut self) {
        let halves = self.edges.len().saturating_mul(2);
        let mut face = vec![usize::MAX; halves];
        let mut cycles = Vec::new();
        for start in 0..halves {
            if face.get(start).is_none_or(|cycle| *cycle != usize::MAX) {
                continue;
            }
            let id = cycles.len();
            let mut members = Vec::new();
            let mut half = start;
            while face.get(half).is_some_and(|cycle| *cycle == usize::MAX) {
                if let Some(entry) = face.get_mut(half) {
                    *entry = id;
                }
                members.push(half);
                half = self.next(half);
            }
            let outer = self.is_outer(&members);
            cycles.push(Cycle { outer, depth: [0_i32; INPUTS], halves: members });
        }
        self.face = face;
        self.cycles = cycles;
    }

    /// A cycle is a component's outer cycle when, at its lexicographically
    /// least node, it takes the sector holding the direction of negative
    /// x, the one between the last and first half-edges of that star. No
    /// bounded face reaches that direction from its own least point.
    fn is_outer(&self, members: &[usize]) -> bool {
        let Some(least) =
            members.iter().map(|half| self.origin(*half)).min_by(|left, right| lexicographic(self.at(*left), self.at(*right)))
        else {
            return true;
        };
        members.iter().any(|half| self.head(*half) == least && self.slot(twin(*half)) == 0)
    }
}

/// The counterclockwise order of directions from `centre`, starting just
/// past negative x, so angles in (-π, π] ascending. Directions below the
/// centre come first; within a half, a counterclockwise turn orders them,
/// and of two opposite directions on the axis, positive x comes first.
pub(crate) fn angular(centre: Point, left: Point, right: Point) -> Ordering {
    let [cx, cy] = centre;
    let lower = |[_, y]: Point| y < cy;
    match (lower(left), lower(right)) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) | (false, false) => match orient2d(centre, left, right) {
            Ordering::Greater => Ordering::Less,
            Ordering::Less => Ordering::Greater,
            Ordering::Equal => {
                let ([lx, _], [rx, _]) = (left, right);
                (rx > cx).cmp(&(lx > cx))
            }
        },
    }
}
