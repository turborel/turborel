//! \[DESIGN\] The states of a search of walks: a vertex and a set of
//! threads, each numbered once, with the notes of their letters.
//!
//! Of an automaton whose threads hold no registers or counters, the
//! plain one of most patterns, a set's states are found in dense arrays
//! by vertex, its table's then its row's number, rather than by hashing,
//! up to `DENSE_ROWS` rows of a table and `DENSE_TABLES` tables; and the
//! set last made is compared before the sets are hashed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::error::Result;
use crate::graph::binding::{Letter, Note, Notes, PathBinding};
use crate::graph::nfa::Nfa;
use crate::graph::program::CostFn;
use crate::graph::run::{Runner, Successor, Thread};
use crate::graph::view::{ElementId, GraphView};

/// \[DESIGN\] The bytes a state is counted as: its entries in the maps,
/// its parent and its letter.
pub(super) const STATE_BYTES: usize = 96;

/// \[DESIGN\] The bytes each thread of a state's set is counted as.
pub(super) const THREAD_BYTES: usize = 48;

/// \[DESIGN\] The rows of a table whose states a dense array holds: 16
/// MiB of each set and table at most.
const DENSE_ROWS: u64 = 1 << 22;

/// \[DESIGN\] The tables whose states dense arrays hold.
const DENSE_TABLES: u32 = 1 << 10;

/// A dense array's place of no state.
const ABSENT: u32 = u32::MAX;

/// The states of each set, table and row of a plain automaton's search.
#[derive(Default)]
struct Dense {
    by_set: Vec<Vec<Vec<u32>>>,
}

impl Dense {
    /// The place of the state of `set` at `vertex`, if a dense array
    /// holds it.
    fn slot(&mut self, set: u32, vertex: ElementId) -> Option<&mut u32> {
        if vertex.row >= DENSE_ROWS || vertex.table >= DENSE_TABLES {
            return None;
        }
        let (s, t, r) = (index(set), index(vertex.table), usize::try_from(vertex.row).ok()?);
        if self.by_set.len() <= s {
            self.by_set.resize_with(s.saturating_add(1), Vec::new);
        }
        let tables = self.by_set.get_mut(s)?;
        if tables.len() <= t {
            tables.resize_with(t.saturating_add(1), Vec::new);
        }
        let rows = tables.get_mut(t)?;
        if rows.len() <= r {
            rows.resize(r.saturating_add(1), ABSENT);
        }
        rows.get_mut(r)
    }
}

/// What a step of a search costs.
#[derive(Clone, Copy)]
pub(crate) enum Costs<'s> {
    /// 1 an edge: a search by length.
    Length,
    /// The COST expression's [GS §7.2].
    Hook(&'s CostFn),
    /// The graph's cost of each edge.
    View,
}

/// What every search of a pattern is given.
#[derive(Clone)]
pub(crate) struct Context<'s> {
    pub(crate) nfa: &'s Nfa,
    pub(crate) view: &'s dyn GraphView,
    /// The cost of an edge.
    pub(crate) cost: Costs<'s>,
    /// The vertices a path may end at, if they are given.
    pub(crate) ends: Option<Arc<HashSet<ElementId>>>,
}

impl Context<'_> {
    /// Whether a path may end at `vertex`.
    pub(super) fn ends_at(&self, vertex: ElementId) -> bool {
        self.ends.as_ref().is_none_or(|ends| ends.contains(&vertex))
    }

    /// Whether every end given is in `found`.
    pub(super) fn all_found(&self, found: &HashSet<ElementId>) -> bool {
        self.ends.as_ref().is_some_and(|ends| ends.iter().all(|e| found.contains(e)))
    }

    /// Whether the search is by cost.
    pub(super) fn by_cost(&self) -> bool {
        match self.cost {
            Costs::Length => false,
            Costs::Hook(_) | Costs::View => true,
        }
    }
}

/// The states a search has made.
#[derive(Default)]
pub(super) struct States {
    /// Of a plain automaton, its states by set and vertex.
    dense: Option<Dense>,
    /// The set last found, and its number.
    last: Option<(Vec<Thread>, u32)>,
    sets: HashMap<Vec<Thread>, u32>,
    threads: Vec<Vec<Thread>>,
    nodes: HashMap<(ElementId, u32), u32>,
    /// Each state's vertex and set.
    of: Vec<(ElementId, u32)>,
    pub(super) notes: Notes,
}

impl States {
    /// The states of a search of `nfa`: in dense arrays if it is plain.
    pub(super) fn of(nfa: &Nfa) -> States {
        States { dense: nfa.plain().then(Dense::default), ..States::default() }
    }

    /// The number of the set `threads`.
    fn set(&mut self, threads: Vec<Thread>) -> u32 {
        if let Some((last, set)) = &self.last
            && same(last, &threads)
        {
            return *set;
        }
        let next = u32::try_from(self.threads.len()).unwrap_or(u32::MAX);
        let set = if let Some(&set) = self.sets.get(&threads) {
            set
        } else {
            self.threads.push(threads.clone());
            self.sets.insert(threads.clone(), next);
            next
        };
        self.last = Some((threads, set));
        set
    }

    /// The state of `vertex` and `threads`, and whether it is new.
    pub(super) fn node(&mut self, vertex: ElementId, threads: Vec<Thread>) -> (u32, bool) {
        let set = self.set(threads);
        let fresh = u32::try_from(self.of.len()).unwrap_or(u32::MAX);
        if let Some(slot) = self.dense.as_mut().and_then(|d| d.slot(set, vertex)) {
            if *slot != ABSENT {
                return (*slot, false);
            }
            *slot = fresh;
        } else if let Some(&node) = self.nodes.get(&(vertex, set)) {
            return (node, false);
        } else {
            self.nodes.insert((vertex, set), fresh);
        }
        self.of.push((vertex, set));
        (fresh, true)
    }

    /// The bytes state `node` is counted as.
    pub(super) fn bytes(&self, node: u32) -> usize {
        STATE_BYTES.saturating_add(self.threads(node).len().saturating_mul(THREAD_BYTES))
    }

    /// The vertex of state `node`.
    pub(super) fn vertex(&self, node: u32) -> ElementId {
        self.of.get(index(node)).map_or(ElementId::new(0, 0), |s| s.0)
    }

    /// The threads of state `node`.
    pub(super) fn threads(&self, node: u32) -> &[Thread] {
        self.of.get(index(node)).and_then(|s| self.threads.get(index(s.1))).map_or(&[], Vec::as_slice)
    }

    /// The letter of `successor`.
    pub(super) fn letter(&mut self, successor: &Successor) -> Letter {
        Letter {
            edge: Some((successor.hop.edge, successor.var)),
            vertex: successor.hop.vertex,
            note: self.notes.id(successor.note.clone()),
        }
    }

    /// The letter a path begun at `vertex` starts with.
    pub(super) fn first(&mut self, vertex: ElementId, note: Note) -> Letter {
        Letter { edge: None, vertex, note: self.notes.id(note) }
    }

    /// The successors of state `node`, `position` edges into its walks.
    pub(super) fn successors(&self, runner: &mut Runner<'_>, node: u32, position: usize) -> Result<Vec<Successor>> {
        let mut out = Vec::new();
        runner.successors(self.vertex(node), (position, None), self.threads(node), &mut out)?;
        Ok(out)
    }
}

/// Whether thread `t` holds no counter, register or mark, as a plain
/// automaton's threads hold none.
fn bare(t: &Thread) -> bool {
    t.counters.is_empty() && t.registers.is_empty() && t.marks.is_empty()
}

/// Whether the sets `a` and `b` are one: of bare threads, by their
/// instructions alone.
fn same(a: &[Thread], b: &[Thread]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(t, u)| if bare(t) && bare(u) { t.pc == u.pc } else { t == u })
}

/// `n` as an index.
pub(super) fn index(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// The binding of the walk to the state of `at`, read back through
/// `parents`, each state's parent and the letter reaching it.
pub(super) fn walk_to(parents: &[(Option<u32>, Letter)], at: u32, notes: &Notes) -> PathBinding {
    let mut letters = Vec::new();
    let mut next = Some(at);
    while let Some((parent, letter)) = next.and_then(|n| parents.get(index(n))) {
        letters.push(*letter);
        next = *parent;
    }
    letters.reverse();
    PathBinding::of(&letters, notes)
}
