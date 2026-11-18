//! \[DESIGN\] Path bindings [P1 §6.4, §6.5]: what a path search gives,
//! and the steps it is built from.
//!
//! A search's step is a letter: the edge followed, if any, the vertex
//! reached, the variables bound to each, and the branches of multiset
//! alternations taken on the way. Reduction strips iteration annotations
//! and the fresh names of anonymous elements [P1 §6.5], so a binding is
//! its elements with the variables bound to each; the branches stay, as
//! multiset alternation keeps the duplicates they tell apart [GS §6.7].
//! Two runs of the automaton giving one sequence of letters give one
//! binding, which is the deduplication of path pattern union.

use std::collections::HashMap;
use std::fmt;

use super::program::VarId;
use super::view::ElementId;

/// The variables bound at one vertex, or the branches taken before it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Note {
    /// The variables bound to the vertex, each as often as it was.
    pub(super) vars: Vec<VarId>,
    /// The multiset alternation branches taken, in order.
    pub(super) tags: Vec<u32>,
}

/// The number of an interned note.
pub(super) type NoteId = u32;

/// The notes of a search, each kept once.
#[derive(Debug, Default)]
pub(super) struct Notes {
    all: Vec<Note>,
    ids: HashMap<Note, NoteId>,
}

impl Notes {
    /// The number of `note`, interned.
    pub(super) fn id(&mut self, note: Note) -> NoteId {
        if let Some(&id) = self.ids.get(&note) {
            return id;
        }
        let id = NoteId::try_from(self.all.len()).unwrap_or(NoteId::MAX);
        self.all.push(note.clone());
        self.ids.insert(note, id);
        id
    }

    /// The note numbered `id`.
    pub(super) fn get(&self, id: NoteId) -> Option<&Note> {
        usize::try_from(id).ok().and_then(|i| self.all.get(i))
    }
}

/// One step of a path: the edge followed, none for the first vertex, and
/// the vertex reached, with what was bound to each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Letter {
    /// The edge followed and its variable.
    pub(super) edge: Option<(ElementId, Option<VarId>)>,
    /// The vertex reached.
    pub(super) vertex: ElementId,
    /// The variables bound to it and the branches taken.
    pub(super) note: NoteId,
}

/// \[P1 §6.5\] A reduced path binding: the path's elements, a vertex
/// first and last, edges between, and the variables bound to each.
#[derive(Clone, Debug, PartialEq)]
pub struct PathBinding {
    /// Its elements: vertex, edge, vertex, ...
    pub elements: Vec<ElementId>,
    /// The variables bound to each element, as often as each was.
    pub vars: Vec<Vec<VarId>>,
    /// The branches of multiset alternations taken, at each vertex.
    pub tags: Vec<Vec<u32>>,
    /// Its cost, of a search by cost [GS §7.2].
    pub cost: Option<f64>,
}

impl PathBinding {
    /// The binding spelled by `letters`, their notes in `notes`.
    pub(super) fn of(letters: &[Letter], notes: &Notes) -> PathBinding {
        let mut binding = PathBinding { elements: Vec::new(), vars: Vec::new(), tags: Vec::new(), cost: None };
        for letter in letters {
            if let Some((edge, var)) = letter.edge {
                binding.elements.push(edge);
                binding.vars.push(var.into_iter().collect());
            }
            let note = notes.get(letter.note).cloned().unwrap_or_default();
            binding.elements.push(letter.vertex);
            binding.vars.push(note.vars);
            binding.tags.push(note.tags);
        }
        binding
    }

    /// Its first vertex.
    pub fn start(&self) -> Option<ElementId> {
        self.elements.first().copied()
    }

    /// Its last vertex.
    pub fn end(&self) -> Option<ElementId> {
        self.elements.last().copied()
    }

    /// \[GS §6.9\] PATH_LENGTH: its number of edges.
    pub fn length(&self) -> usize {
        self.elements.len().saturating_sub(1).div_euclid(2)
    }

    /// \[GS §6.9\] VERTICES: its vertices, in order.
    pub fn vertices(&self) -> Vec<ElementId> {
        self.elements.iter().step_by(2).copied().collect()
    }

    /// \[GS §6.9\] EDGES: its edges, in order.
    pub fn edges(&self) -> Vec<ElementId> {
        self.elements.iter().skip(1).step_by(2).copied().collect()
    }

    /// The elements bound to `var`, in order: a group's list [P1 §4.4].
    pub fn group(&self, var: VarId) -> Vec<ElementId> {
        self.elements.iter().zip(&self.vars).flat_map(|(e, vars)| vars.iter().filter(|&&v| v == var).map(move |_| *e)).collect()
    }

    /// The element singleton `var` is bound to, if it is bound.
    pub fn singleton(&self, var: VarId) -> Option<ElementId> {
        self.group(var).first().copied()
    }

    /// \[GS §6.9\] IS_TRAIL: whether no edge repeats.
    pub fn is_trail(&self) -> bool {
        distinct(&self.edges())
    }

    /// \[GS §6.9\] IS_ACYCLIC: whether no vertex repeats.
    pub fn is_acyclic(&self) -> bool {
        distinct(&self.vertices())
    }

    /// \[GS §6.9\] IS_SIMPLE: whether no vertex repeats, but the first
    /// may be the last.
    pub fn is_simple(&self) -> bool {
        let vertices = self.vertices();
        let closed = vertices.len() > 1 && vertices.first() == vertices.last();
        distinct(if closed { vertices.get(1..).unwrap_or_default() } else { vertices.as_slice() })
    }
}

/// Whether `elements` are each different.
fn distinct(elements: &[ElementId]) -> bool {
    let mut sorted = elements.to_vec();
    sorted.sort_unstable();
    sorted.windows(2).all(|w| w.first() != w.get(1))
}

impl fmt::Display for PathBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("path(")?;
        for (i, element) in self.elements.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{element}")?;
        }
        f.write_str(")")
    }
}
