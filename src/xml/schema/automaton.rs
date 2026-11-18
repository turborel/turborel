//! Content models compiled to deterministic automata [Schema1 §3.8.4,
//! §3.9.4, Appendix H], checked for Unique Particle Attribution and
//! Element Declarations Consistent [Schema1 §3.8.6].
//!
//! A content model is transcribed as Appendix H describes: each particle
//! of an element declaration or a wildcard is a position (Glushkov's
//! construction, which needs no epsilon transitions), numeric occurrence
//! ranges are unfolded into copies of their particle, then the automaton
//! is made deterministic by the subset construction, its transitions
//! labelled by particle, so the copies of one particle are one label. A
//! state from which two different particles that overlap may be taken
//! breaks Unique Particle Attribution. An all group is no automaton: its
//! particles are each taken at most once, in any order [Schema1 §3.8.4].
//!
//! \[DESIGN\] RULES.md D18: a content model of more than `MAX_POSITIONS`
//! positions once unfolded, or whose automaton has more than `MAX_STATES`
//! states, is SQLCODE -900. `{n,m}` unfolds into `n` copies and `m-n`
//! nested optional ones, `{n,unbounded}` into `n` copies the last of which
//! repeats, so a range costs its bound in positions but no more follow
//! edges than a sequence of as many particles.

use std::collections::{BTreeMap, HashMap};

use super::component::{Compositor, ElementId, Expanded, Particle, Schema, Term, Wildcard};
use super::{MAX_DEPTH, beyond, invalid};
use crate::error::Result;

/// \[DESIGN\] How many positions a content model has at most once its
/// occurrence ranges are unfolded (RULES.md D18): a bound on the memory
/// and time of its automaton, past the content models of the schemas
/// applications write; `maxOccurs` of 1,000 on one element is within it.
pub const MAX_POSITIONS: usize = 4096;
/// \[DESIGN\] How many states a content model's automaton has at most
/// (RULES.md D18), as the subset construction may make many from few
/// positions.
pub const MAX_STATES: usize = 16384;

/// A state of an automaton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateId(pub usize);

/// A particle of an element declaration or a wildcard: a label of the
/// automaton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaf {
    /// An element declaration particle.
    Element(ElementId),
    /// A wildcard particle.
    Wildcard(Wildcard),
}

/// A state: whether the content may end in it, and the particle each of
/// its transitions takes, in the order of the particles, with the state
/// it goes to.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// Whether the content may end here.
    accepting: bool,
    /// The particle each transition takes and the state it goes to.
    edges: Vec<(usize, StateId)>,
}

/// The automaton of a content model.
#[derive(Debug, Clone)]
pub enum Automaton {
    /// A deterministic automaton: its particles, its states, the first
    /// the initial one, and the particles an element name may match.
    Dfa {
        /// The particles, by label.
        leaves: Vec<Leaf>,
        /// The states.
        states: Vec<State>,
        /// For each element name, the labels of the element declaration
        /// particles it may match: the declaration's name, and those of
        /// the members of its substitution group [Schema1 §3.9.4 clause
        /// 2.3.3].
        names: BTreeMap<Expanded, Vec<usize>>,
    },
    /// An all group: its element declarations, each with its {min
    /// occurs}, any of which may come once, in any order [Schema1 §3.8.4
    /// clause 3]; and whether the group itself may be absent.
    All {
        /// The particles.
        leaves: Vec<(ElementId, u64)>,
        /// Whether the whole group is optional ({min occurs} 0).
        optional: bool,
    },
}

/// A fragment of the position automaton: its first and last positions,
/// and whether it matches the empty sequence.
#[derive(Debug, Clone, Default)]
struct Frag {
    first: Vec<usize>,
    last: Vec<usize>,
    nullable: bool,
}

/// The position automaton being built.
#[derive(Debug, Default)]
struct Glushkov {
    /// Each position's label.
    labels: Vec<usize>,
    /// Each position's followers.
    follow: Vec<Vec<usize>>,
    /// The labels' particles, by the place of the particle.
    leaves: Vec<Leaf>,
    /// Each particle's label, by the model group it is in and its place
    /// there (none for a content type's own particle).
    keys: HashMap<(Option<usize>, usize), usize>,
}

impl Glushkov {
    /// A new position of label `label`.
    fn position(&mut self, label: usize) -> Result<usize> {
        if self.labels.len() >= MAX_POSITIONS {
            return Err(beyond("a content model has too many particles once its occurrences are unfolded"));
        }
        self.labels.push(label);
        self.follow.push(Vec::new());
        Ok(self.labels.len().saturating_sub(1))
    }

    /// Each position of `to` follows each of `from`.
    fn link(&mut self, from: &[usize], to: &[usize]) {
        for &p in from {
            if let Some(f) = self.follow.get_mut(p) {
                f.extend_from_slice(to);
            }
        }
    }

    /// The label of the particle at `key`, `term`.
    fn label(&mut self, key: (Option<usize>, usize), leaf: Leaf) -> usize {
        if let Some(&l) = self.keys.get(&key) {
            return l;
        }
        let l = self.leaves.len();
        self.leaves.push(leaf);
        self.keys.insert(key, l);
        l
    }

    /// The fragment of `a` then `b`.
    fn sequence(&mut self, a: Frag, b: Frag) -> Frag {
        self.link(&a.last, &b.first);
        let mut first = a.first;
        if a.nullable {
            first.extend(b.first);
        }
        let mut last = b.last;
        if b.nullable {
            last.extend(a.last);
        }
        Frag { first, last, nullable: a.nullable && b.nullable }
    }

    /// The fragment of particle `p`, at `key`, its range unfolded.
    fn particle(&mut self, schema: &Schema, p: &Particle, key: (Option<usize>, usize), depth: usize) -> Result<Frag> {
        let depth = depth.saturating_add(1);
        if depth > MAX_DEPTH {
            return Err(beyond("a content model nests too deeply"));
        }
        let required = if p.max.is_none() { p.min.max(1) } else { p.min };
        let mut frag = Frag { nullable: true, ..Frag::default() };
        let mut count = 0u64;
        while count < required {
            let copy = self.term(schema, &p.term, key, depth)?;
            let is_last = count.saturating_add(1) == required;
            let copy = if is_last && p.max.is_none() { self.repeat(copy) } else { copy };
            frag = self.sequence(frag, copy);
            count = count.saturating_add(1);
        }
        let optional = p.max.map_or(0, |max| max.saturating_sub(required));
        let unbounded_optional = p.max.is_none() && p.min == 0;
        let tail = self.optional_copies(schema, p, key, (optional, depth))?;
        frag = self.sequence(frag, tail);
        if unbounded_optional {
            frag.nullable = true;
        }
        Ok(frag)
    }

    /// `frag` repeated any number of times, once at least.
    fn repeat(&mut self, frag: Frag) -> Frag {
        let first = frag.first.clone();
        self.link(&frag.last, &first);
        frag
    }

    /// `n` nested optional copies of `p`'s term: `(t (t ...)?)?`.
    fn optional_copies(&mut self, schema: &Schema, p: &Particle, key: (Option<usize>, usize), (n, depth): (u64, usize)) -> Result<Frag> {
        if usize::try_from(n).map_or(true, |n| n > MAX_POSITIONS) {
            return Err(beyond("a content model has too many particles once its occurrences are unfolded"));
        }
        let mut rest = Frag { nullable: true, ..Frag::default() };
        for _ in 0..n {
            let copy = self.term(schema, &p.term, key, depth)?;
            let mut nested = self.sequence(copy, rest);
            nested.nullable = true;
            rest = nested;
        }
        Ok(rest)
    }

    /// The fragment of one occurrence of `term`, of the particle at `key`.
    fn term(&mut self, schema: &Schema, term: &Term, key: (Option<usize>, usize), depth: usize) -> Result<Frag> {
        let leaf = match term {
            Term::Element(e) => Leaf::Element(*e),
            Term::Wildcard(w) => Leaf::Wildcard(w.clone()),
            Term::Group(g) => {
                let group = schema.group(*g).ok_or_else(|| invalid("[Schema1 §3.8.1]", "a model group is missing"))?;
                return self.group(schema, g.0, (group.compositor, &group.particles), depth);
            }
        };
        let label = self.label(key, leaf);
        let p = self.position(label)?;
        Ok(Frag { first: vec![p], last: vec![p], nullable: false })
    }

    /// The fragment of model group `g` of `compositor` and `particles`.
    fn group(&mut self, schema: &Schema, g: usize, (compositor, particles): (Compositor, &[Particle]), depth: usize) -> Result<Frag> {
        match compositor {
            Compositor::Sequence => {
                let mut frag = Frag { nullable: true, ..Frag::default() };
                for (i, p) in particles.iter().enumerate() {
                    let next = self.particle(schema, p, (Some(g), i), depth)?;
                    frag = self.sequence(frag, next);
                }
                Ok(frag)
            }
            Compositor::Choice => {
                let mut frag = Frag::default();
                for (i, p) in particles.iter().enumerate() {
                    let next = self.particle(schema, p, (Some(g), i), depth)?;
                    frag.first.extend(next.first);
                    frag.last.extend(next.last);
                    frag.nullable = frag.nullable || next.nullable;
                }
                Ok(frag)
            }
            Compositor::All => Err(invalid("[Schema1 §3.8.6 cos-all-limited]", "an all group is the whole of a content model")),
        }
    }
}

/// The automaton of content model `p` of `schema`, checked for Unique
/// Particle Attribution [Schema1 §3.8.6 cos-nonambig].
///
/// # Errors
///
/// SQLSTATE 22000 if two particles that overlap compete [Schema1 §3.8.6
/// cos-nonambig, Appendix H], or an all group is not the whole of it
/// [Schema1 §3.8.6 cos-all-limited]; SQLCODE -900 past `MAX_POSITIONS`
/// or `MAX_STATES`.
pub fn compile(schema: &Schema, p: &Particle) -> Result<Automaton> {
    if let Some(all) = all_group(schema, p)? {
        return Ok(all);
    }
    let mut g = Glushkov::default();
    let root = g.particle(schema, p, (None, 0), 0)?;
    let names = names(schema, &g.leaves);
    let mut last = vec![false; g.labels.len()];
    for &q in &root.last {
        if let Some(l) = last.get_mut(q) {
            *l = true;
        }
    }
    let mut first = root.first;
    first.sort_unstable();
    first.dedup();
    let start = State { accepting: root.nullable, edges: Vec::new() };
    let mut dfa = Subsets { last, sets: HashMap::new(), states: vec![start], pending: vec![(StateId(0), first)] };
    while let Some((state, candidates)) = dfa.pending.pop() {
        let by_label = group_by_label(&g.labels, &candidates);
        unique(schema, &g.leaves, &by_label)?;
        for (label, positions) in by_label {
            let to = target(&mut dfa, positions, &g.follow)?;
            if let Some(s) = dfa.states.get_mut(state.0) {
                s.edges.push((label, to));
            }
        }
    }
    let mut states = dfa.states;
    for s in &mut states {
        s.edges.sort_unstable();
    }
    Ok(Automaton::Dfa { leaves: g.leaves, states, names })
}

/// The subset construction under way.
struct Subsets {
    /// Whether each position is a last one.
    last: Vec<bool>,
    /// Each state made, by the positions it is at.
    sets: HashMap<Vec<usize>, StateId>,
    /// The states made.
    states: Vec<State>,
    /// The states whose transitions are yet to make, each with the
    /// positions that may come next from it.
    pending: Vec<(StateId, Vec<usize>)>,
}

/// The state of the automaton at `positions`, made if new, the positions
/// that follow them pending.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_STATES`.
fn target(dfa: &mut Subsets, mut positions: Vec<usize>, follow: &[Vec<usize>]) -> Result<StateId> {
    positions.sort_unstable();
    if let Some(&to) = dfa.sets.get(&positions) {
        return Ok(to);
    }
    if dfa.states.len() >= MAX_STATES {
        return Err(beyond("a content model's automaton has too many states"));
    }
    let to = StateId(dfa.states.len());
    let accepting = positions.iter().any(|&q| dfa.last.get(q).copied().unwrap_or(false));
    let mut next: Vec<usize> = positions.iter().flat_map(|&q| follow.get(q).cloned().unwrap_or_default()).collect();
    next.sort_unstable();
    next.dedup();
    dfa.states.push(State { accepting, edges: Vec::new() });
    dfa.sets.insert(positions, to);
    dfa.pending.push((to, next));
    Ok(to)
}

/// The positions `candidates` grouped by label, in label order.
fn group_by_label(labels: &[usize], candidates: &[usize]) -> BTreeMap<usize, Vec<usize>> {
    let mut out: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &q in candidates {
        if let Some(&l) = labels.get(q) {
            out.entry(l).or_default().push(q);
        }
    }
    out
}

/// [Schema1 §3.8.6 cos-nonambig, Appendix H] Whether the particles that
/// may be taken next, `by_label`'s labels, overlap no two of them: no
/// element name either matches, and no wildcard allows the namespace of
/// one or overlaps another.
///
/// # Errors
///
/// SQLSTATE 22000 if two overlap.
fn unique(schema: &Schema, leaves: &[Leaf], by_label: &BTreeMap<usize, Vec<usize>>) -> Result<()> {
    let ambiguous = || invalid("[Schema1 §3.8.6 cos-nonambig]", "a content model has two particles that compete for one element");
    let mut seen: BTreeMap<&Expanded, usize> = BTreeMap::new();
    let mut wildcards: Vec<&Wildcard> = Vec::new();
    for &label in by_label.keys() {
        match leaves.get(label) {
            Some(Leaf::Element(e)) => {
                for name in group_names(schema, *e) {
                    if seen.insert(name, label).is_some_and(|other| other != label) {
                        return Err(ambiguous());
                    }
                }
            }
            Some(Leaf::Wildcard(w)) => {
                if wildcards.iter().any(|o| o.constraint.overlaps(&w.constraint)) {
                    return Err(ambiguous());
                }
                wildcards.push(w);
            }
            None => {}
        }
    }
    if seen.keys().any(|n| wildcards.iter().any(|w| w.constraint.allows(n.uri.as_deref()))) {
        return Err(ambiguous());
    }
    Ok(())
}

/// The names element declaration `e` may match in a content model: its
/// own, and those of its substitution group's members [Schema1 §3.3.6,
/// §3.9.4 clause 2.3.3].
pub fn group_names(schema: &Schema, e: ElementId) -> Vec<&Expanded> {
    substitution_group(schema, e).into_iter().filter_map(|m| schema.element(m)).map(|d| &d.name).collect()
}

/// [Schema1 §3.3.6] The substitution group of top-level element
/// declaration `e`: it, and every top-level declaration whose chain of
/// substitution group affiliations reaches it within `MAX_CHAIN`; of a
/// local declaration, it alone.
pub fn substitution_group(schema: &Schema, e: ElementId) -> Vec<ElementId> {
    let mut out = vec![e];
    if schema.element(e).is_none_or(|d| d.scope != super::component::Scope::Global) {
        return out;
    }
    for &member in schema.element_names.values() {
        let mut at = member;
        for _ in 0..super::MAX_CHAIN {
            let Some(head) = schema.element(at).and_then(|d| d.substitution_group) else { break };
            if head == e {
                out.push(member);
                break;
            }
            at = head;
        }
    }
    out
}

/// For each element name, the labels of `leaves` it may match.
fn names(schema: &Schema, leaves: &[Leaf]) -> BTreeMap<Expanded, Vec<usize>> {
    let mut out: BTreeMap<Expanded, Vec<usize>> = BTreeMap::new();
    for (label, leaf) in leaves.iter().enumerate() {
        if let Leaf::Element(e) = leaf {
            for name in group_names(schema, *e) {
                out.entry(name.clone()).or_default().push(label);
            }
        }
    }
    out
}

/// The automaton of `p` if its term is an all group [Schema1 §3.8.4
/// clause 3]: its particles, which must be element declarations each of
/// {max occurs} 0 or 1 [Schema1 §3.8.6 cos-all-limited.2], none
/// overlapping another.
///
/// # Errors
///
/// SQLSTATE 22000 if the particle of an all group occurs other than once
/// [Schema1 §3.8.6 cos-all-limited.1.2], or two of its particles overlap
/// \[cos-nonambig\].
fn all_group(schema: &Schema, p: &Particle) -> Result<Option<Automaton>> {
    let Term::Group(g) = &p.term else { return Ok(None) };
    let Some(group) = schema.group(*g).filter(|g| g.compositor == Compositor::All) else { return Ok(None) };
    if p.max != Some(1) || p.min > 1 {
        return Err(invalid("[Schema1 §3.8.6 cos-all-limited.1.2]", "the particle of an all group occurs at most once"));
    }
    let mut leaves = Vec::new();
    let mut seen: Vec<&Expanded> = Vec::new();
    for q in &group.particles {
        let Term::Element(e) = &q.term else {
            return Err(invalid("[Schema1 §3.8.6 cos-all-limited]", "an all group holds element declarations alone"));
        };
        if q.max.is_none_or(|m| m > 1) {
            return Err(invalid("[Schema1 §3.8.6 cos-all-limited.2]", "an element of an all group occurs at most once"));
        }
        for name in group_names(schema, *e) {
            if seen.contains(&name) {
                return Err(invalid("[Schema1 §3.8.6 cos-nonambig]", "two particles of an all group match one element"));
            }
            seen.push(name);
        }
        leaves.push((*e, q.min));
    }
    Ok(Some(Automaton::All { leaves, optional: p.min == 0 }))
}

impl Automaton {
    /// The initial state.
    #[must_use]
    pub fn start(&self) -> StateId {
        StateId(0)
    }

    /// Whether the content may end in `state`.
    #[must_use]
    pub fn accepts(&self, state: StateId) -> bool {
        match self {
            Automaton::Dfa { states, .. } => states.get(state.0).is_some_and(|s| s.accepting),
            Automaton::All { .. } => false,
        }
    }

    /// The transition from `state` an element of name `name` takes: the
    /// particle matched and the state it goes to [Schema1 §3.9.4]. An
    /// element declaration particle whose name, or a member of whose
    /// substitution group, is `name` is taken before a wildcard, of which
    /// at most one allows it [Schema1 §3.8.6 cos-nonambig].
    #[must_use]
    pub fn step(&self, state: StateId, name: &Expanded) -> Option<(&Leaf, StateId)> {
        let Automaton::Dfa { leaves, states, names } = self else { return None };
        let edges = &states.get(state.0)?.edges;
        let by_name = names.get(name).into_iter().flatten().find_map(|l| edges.binary_search_by_key(l, |e| e.0).ok());
        let found = by_name.and_then(|i| edges.get(i)).or_else(|| {
            edges.iter().find(|(l, _)| match leaves.get(*l) {
                Some(Leaf::Wildcard(w)) => w.constraint.allows(name.uri.as_deref()),
                Some(Leaf::Element(_)) | None => false,
            })
        })?;
        Some((leaves.get(found.0)?, found.1))
    }

    /// The particles that may come next from `state`, for messages.
    #[must_use]
    pub fn expected(&self, state: StateId) -> Vec<&Leaf> {
        match self {
            Automaton::Dfa { leaves, states, .. } => {
                states.get(state.0).map(|s| s.edges.iter().filter_map(|(l, _)| leaves.get(*l)).collect()).unwrap_or_default()
            }
            Automaton::All { .. } => Vec::new(),
        }
    }
}
