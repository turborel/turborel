//! \[DESIGN\] A path pattern compiled to an automaton [GS §14 step 5]: a
//! Thompson construction whose transitions test the vertex the path is
//! at or follow an edge, so a search walks the product of the graph and
//! its states [GS §15]. Union forks; multiset alternation forks too,
//! marking the branch taken, since its duplicates are kept [GS §6.7]. A
//! quantifier with a bound past one keeps a counter, never unrolled, so
//! a large bound costs no more states than a small one; `*`, `+`, `?`
//! are written out without one. The forks' epsilon closure is computed
//! once, at compile time.
//!
//! Scopes are the path, each parenthesized pattern, each quantifier and
//! each of its iterations. A scope holds the registers of the bindings a
//! prefilter in it reads, or a repeated variable joins on [GS §6.4]; a
//! restricted scope notes where it began, so its restrictor is checked
//! as the path grows [GS §7.1].

mod registers;

use std::fmt;

use super::program::{ElementTest, Filter, LabelExpr, PathPattern, Pattern, Predicate, Restrictor, Selector, VarId, zero_k};
use super::view::Orientation;
use crate::error::{Code, Result, SqlError};
use registers::Registers;

/// \[DESIGN\] How deep patterns may nest, so compiling, which recurses
/// once per level, cannot exhaust the stack (RULES.md R-CF-6).
pub(super) const MAX_DEPTH: usize = 256;

/// \[DESIGN\] The most instructions an automaton has: a pattern's size
/// times the copies `+` makes of its body, nested.
pub(super) const MAX_INSTRUCTIONS: usize = 1 << 20;

/// One instruction of an automaton.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Instr {
    /// Tests the vertex the path is at with test `test`, binding it.
    Vertex { test: usize, next: usize },
    /// Follows an edge passing test `test`, in a way `orientation` allows.
    Edge { test: usize, orientation: Orientation, next: usize },
    /// Goes on at each of its targets.
    Split(Vec<usize>),
    /// Marks branch `tag` of a multiset alternation taken.
    Tag { tag: u32, next: usize },
    /// Scope `scope` begins: its registers cleared, its start noted.
    Enter { scope: usize, next: usize },
    /// Scope `scope` ends: its prefilter tested.
    Leave { scope: usize, next: usize },
    /// A counted quantifier begins, its counter 0.
    Init { counter: usize, next: usize },
    /// Between iterations: another if fewer than `max`, out if `min` done.
    Loop { counter: usize, min: u32, max: Option<u32>, body: usize, exit: usize },
    /// An iteration ends: its counter counted, up to `cap`.
    Count { counter: usize, cap: u32, next: usize },
    /// The pattern is matched.
    Accept,
}

/// An element pattern's test, with the registers its binding goes to.
#[derive(Clone, Debug)]
pub(super) struct Test {
    pub(super) var: Option<VarId>,
    pub(super) labels: Option<LabelExpr>,
    pub(super) filter: Option<Compiled>,
    /// The registers its element is bound to, and whether each joins.
    pub(super) binds: Vec<(usize, bool)>,
    /// The restricted scopes' slots it is within, an edge's.
    pub(super) slots: Vec<usize>,
}

/// A prefilter with its reads resolved to registers.
#[derive(Clone)]
pub(super) struct Compiled {
    pub(super) test: Predicate,
    pub(super) reads: Vec<(VarId, Option<usize>, Option<usize>)>,
}

impl fmt::Debug for Compiled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Compiled {:?}", self.reads)
    }
}

/// What a scope is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScopeKind {
    /// The whole path.
    Path,
    /// A parenthesized pattern.
    Group,
    /// A quantified pattern, bounded or not.
    Quantifier { bounded: bool },
    /// One iteration of a quantified pattern.
    Iteration,
}

/// A scope, as compiled.
#[derive(Clone, Debug)]
pub(super) struct Scope {
    pub(super) restrictor: Restrictor,
    /// The thread's slot of its start, if it is restricted.
    pub(super) slot: Option<usize>,
    pub(super) filter: Option<Compiled>,
    /// The registers cleared as it begins.
    pub(super) clears: Vec<usize>,
    /// The PREV registers set as it begins, each from its source.
    pub(super) previous: Vec<(usize, usize)>,
}

/// A compiled path pattern.
#[derive(Debug)]
pub struct Nfa {
    pub(super) instrs: Vec<Instr>,
    pub(super) tests: Vec<Test>,
    pub(super) scopes: Vec<Scope>,
    pub(super) start: usize,
    pub(super) registers: usize,
    pub(super) counters: usize,
    pub(super) slots: usize,
    /// For each instruction, the instructions its forks reach first.
    pub(super) closure: Vec<Vec<usize>>,
    /// The selector and restrictor at the pattern's head.
    pub(super) selector: Selector,
    pub(super) restrictor: Restrictor,
}

/// A pattern with its scopes, tests and tags numbered.
#[derive(Clone, Debug)]
pub(super) enum Node {
    Element { test: usize, edge: Option<Orientation> },
    Concat(Vec<Node>),
    Union(Vec<Node>),
    Alternation(Vec<(u32, Node)>),
    Quantified { scope: usize, body_scope: usize, min: u32, max: Option<u32>, counter: Option<usize>, body: Box<Node> },
    Group { scope: usize, body: Box<Node> },
}

/// What numbering a pattern gathers.
#[derive(Default)]
pub(super) struct Numbering {
    pub(super) scopes: Vec<(Option<usize>, ScopeKind, Restrictor, Option<Filter>)>,
    /// Each element test, with its scope and whether it is an edge's.
    pub(super) tests: Vec<(usize, ElementTest, bool)>,
    tags: u32,
    pub(super) counters: usize,
}

impl Numbering {
    /// A new scope of `kind` within `parent`.
    fn scope(&mut self, parent: Option<usize>, kind: ScopeKind, restrictor: Restrictor, filter: Option<Filter>) -> usize {
        self.scopes.push((parent, kind, restrictor, filter));
        self.scopes.len().saturating_sub(1)
    }

    /// `pattern` numbered, within scope `scope`, `depth` levels deep.
    fn node(&mut self, pattern: &Pattern, scope: usize, depth: usize) -> Result<Node> {
        if depth > MAX_DEPTH {
            return Err(SqlError::new(Code::Unsupported, format!("[DESIGN] a path pattern nests more than {MAX_DEPTH} levels")));
        }
        let deeper = depth.saturating_add(1);
        Ok(match pattern {
            Pattern::Vertex(test) => self.element(test, scope, None),
            Pattern::Edge(test, orientation) => self.element(test, scope, Some(*orientation)),
            Pattern::Concat(parts) => Node::Concat(parts.iter().map(|p| self.node(p, scope, deeper)).collect::<Result<_>>()?),
            Pattern::Union(parts) => Node::Union(parts.iter().map(|p| self.node(p, scope, deeper)).collect::<Result<_>>()?),
            Pattern::Alternation(parts) => Node::Alternation(
                parts
                    .iter()
                    .map(|p| {
                        self.tags = self.tags.saturating_add(1);
                        Ok((self.tags, self.node(p, scope, deeper)?))
                    })
                    .collect::<Result<_>>()?,
            ),
            Pattern::Quantified(q) => self.quantified(q.as_ref(), scope, deeper)?,
            Pattern::Group(g) => {
                let inner = self.scope(Some(scope), ScopeKind::Group, g.restrictor, g.filter.clone());
                Node::Group { scope: inner, body: Box::new(self.node(&g.body, inner, deeper)?) }
            }
        })
    }

    /// An element pattern numbered, an edge's if it has an orientation.
    fn element(&mut self, test: &ElementTest, scope: usize, edge: Option<Orientation>) -> Node {
        self.tests.push((scope, test.clone(), edge.is_some()));
        Node::Element { test: self.tests.len().saturating_sub(1), edge }
    }

    /// A quantified pattern numbered: its scope, its iterations' and,
    /// unless written out, its counter.
    fn quantified(&mut self, q: &super::program::Quantified, scope: usize, depth: usize) -> Result<Node> {
        let outer = self.scope(Some(scope), ScopeKind::Quantifier { bounded: q.max.is_some() }, Restrictor::Walk, None);
        let body_scope = self.scope(Some(outer), ScopeKind::Iteration, Restrictor::Walk, None);
        let body = Box::new(self.node(&q.body, body_scope, depth)?);
        let counter = (!unrolled(q.min, q.max)).then(|| {
            self.counters = self.counters.saturating_add(1);
            self.counters.saturating_sub(1)
        });
        Ok(Node::Quantified { scope: outer, body_scope, min: q.min, max: q.max, counter, body })
    }
}

/// Whether a quantifier of `min` and `max` is written out, needing no
/// counter: `*`, `+`, `?` and `{1}`.
fn unrolled(min: u32, max: Option<u32>) -> bool {
    match max {
        None => min <= 1,
        Some(max) => min <= 1 && max <= 1,
    }
}

/// Builds the instructions of a numbered pattern.
struct Emitter {
    instrs: Vec<Instr>,
}

impl Emitter {
    /// Adds `instr`, giving where it is.
    fn push(&mut self, instr: Instr) -> Result<usize> {
        if self.instrs.len() >= MAX_INSTRUCTIONS {
            return Err(SqlError::new(
                Code::Unsupported,
                format!("[DESIGN] a path pattern compiles to more than {MAX_INSTRUCTIONS} instructions"),
            ));
        }
        self.instrs.push(instr);
        Ok(self.instrs.len().saturating_sub(1))
    }

    /// Emits `node`, going on at `next`; gives where it begins.
    fn emit(&mut self, node: &Node, next: usize) -> Result<usize> {
        match node {
            Node::Element { test, edge: None } => self.push(Instr::Vertex { test: *test, next }),
            Node::Element { test, edge: Some(orientation) } => self.push(Instr::Edge { test: *test, orientation: *orientation, next }),
            Node::Concat(parts) => parts.iter().rev().try_fold(next, |at, part| self.emit(part, at)),
            Node::Union(parts) => {
                let entries = parts.iter().map(|p| self.emit(p, next)).collect::<Result<Vec<_>>>()?;
                self.push(Instr::Split(entries))
            }
            Node::Alternation(parts) => {
                let mut entries = Vec::new();
                for (tag, part) in parts {
                    let entry = self.emit(part, next)?;
                    entries.push(self.push(Instr::Tag { tag: *tag, next: entry })?);
                }
                self.push(Instr::Split(entries))
            }
            Node::Quantified { scope, body_scope, min, max, counter, body } => {
                let looped = self.quantified((*body_scope, body, *min, *max, *counter), next)?;
                self.push(Instr::Enter { scope: *scope, next: looped })
            }
            Node::Group { scope, body } => {
                let leave = self.push(Instr::Leave { scope: *scope, next })?;
                let entry = self.emit(body, leave)?;
                self.push(Instr::Enter { scope: *scope, next: entry })
            }
        }
    }

    /// An iteration of `body`, in scope `scope`, going on at `next`.
    fn iteration(&mut self, scope: usize, body: &Node, next: usize) -> Result<usize> {
        let entry = self.emit(body, next)?;
        self.push(Instr::Enter { scope, next: entry })
    }

    /// The iterations of a quantified pattern, going on at `next`.
    fn quantified(
        &mut self,
        (scope, body, min, max, counter): (usize, &Node, u32, Option<u32>, Option<usize>),
        next: usize,
    ) -> Result<usize> {
        if let Some(counter) = counter {
            // The loop's place is taken first, so an iteration can end there.
            let looped = self.push(Instr::Accept)?;
            let cap = max.unwrap_or(min);
            let count = self.push(Instr::Count { counter, cap, next: looped })?;
            let entry = self.iteration(scope, body, count)?;
            if let Some(slot) = self.instrs.get_mut(looped) {
                *slot = Instr::Loop { counter, min, max, body: entry, exit: next };
            }
            return self.push(Instr::Init { counter, next: looped });
        }
        match (min, max) {
            (_, Some(0)) => Ok(next),
            (0, Some(_)) => {
                let entry = self.iteration(scope, body, next)?;
                self.push(Instr::Split(vec![entry, next]))
            }
            (_, Some(_)) => self.iteration(scope, body, next),
            (min, None) => {
                let star = self.push(Instr::Accept)?;
                let again = self.iteration(scope, body, star)?;
                if let Some(slot) = self.instrs.get_mut(star) {
                    *slot = Instr::Split(vec![again, next]);
                }
                if min == 0 { Ok(star) } else { self.iteration(scope, body, star) }
            }
        }
    }
}

impl Nfa {
    /// Compiles `path`, checking its termination rules [GS §7.4].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for GRF-0031 or GRF-0032, or a selector's k not
    /// positive; -900 past `MAX_DEPTH` or `MAX_INSTRUCTIONS`.
    pub fn compile(path: &PathPattern) -> Result<Nfa> {
        if path.selector.k() == 0 {
            return Err(zero_k(path.selector));
        }
        let mut numbering = Numbering::default();
        let top = numbering.scope(None, ScopeKind::Path, path.restrictor, None);
        let node = numbering.node(&path.pattern, top, 0)?;
        let registers = Registers::allocate(&numbering, path.selector)?;
        let mut emitter = Emitter { instrs: Vec::new() };
        let accept = emitter.push(Instr::Accept)?;
        let leave = emitter.push(Instr::Leave { scope: top, next: accept })?;
        let entry = emitter.emit(&node, leave)?;
        let start = emitter.push(Instr::Enter { scope: top, next: entry })?;
        let closure = closures(&emitter.instrs);
        let (tests, scopes, slots, count) = registers.finish(&numbering.tests, &numbering.scopes);
        let (selector, restrictor, counters) = (path.selector, path.restrictor, numbering.counters);
        Ok(Nfa { instrs: emitter.instrs, tests, scopes, start, registers: count, counters, slots, closure, selector, restrictor })
    }

    /// Whether a restrictor applies anywhere in it, so a search must keep
    /// each path rather than each state.
    pub fn restricted(&self) -> bool {
        self.slots > 0
    }

    /// Whether its threads hold no registers or counters, so a state is
    /// an instruction alone and the automaton may be run backwards.
    pub fn plain(&self) -> bool {
        self.registers == 0 && self.counters == 0 && self.slots == 0
    }

    /// The instruction at `pc`.
    pub(super) fn instr(&self, pc: usize) -> Option<&Instr> {
        self.instrs.get(pc)
    }
}

/// For each instruction, the instructions not forks that its forks reach,
/// in order, each once.
fn closures(instrs: &[Instr]) -> Vec<Vec<usize>> {
    (0..instrs.len())
        .map(|pc| {
            let (mut out, mut seen, mut stack) = (Vec::new(), vec![false; instrs.len()], vec![pc]);
            while let Some(at) = stack.pop() {
                if seen.get(at).copied().unwrap_or(true) {
                    continue;
                }
                if let Some(s) = seen.get_mut(at) {
                    *s = true;
                }
                if let Some(Instr::Split(targets)) = instrs.get(at) {
                    stack.extend(targets.iter().rev());
                } else {
                    out.push(at);
                }
            }
            out
        })
        .collect()
}

impl fmt::Display for Nfa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} {} start {}", self.selector, self.restrictor, self.start)?;
        for (pc, instr) in self.instrs.iter().enumerate() {
            write!(f, "{pc}: ")?;
            match instr {
                Instr::Vertex { test, next } => writeln!(f, "vertex {} -> {next}", self.describe(*test)),
                Instr::Edge { test, orientation, next } => writeln!(f, "edge {orientation} {} -> {next}", self.describe(*test)),
                Instr::Split(targets) => writeln!(f, "split {targets:?}"),
                Instr::Tag { tag, next } => writeln!(f, "tag {tag} -> {next}"),
                Instr::Enter { scope, next } => writeln!(f, "enter {scope} -> {next}"),
                Instr::Leave { scope, next } => writeln!(f, "leave {scope} -> {next}"),
                Instr::Init { counter, next } => writeln!(f, "init c{counter} -> {next}"),
                Instr::Loop { counter, min, max, body, exit } => {
                    let max = max.map_or_else(|| "inf".to_owned(), |m| m.to_string());
                    writeln!(f, "loop c{counter} {{{min},{max}}} body {body} exit {exit}")
                }
                Instr::Count { counter, cap, next } => writeln!(f, "count c{counter} cap {cap} -> {next}"),
                Instr::Accept => writeln!(f, "accept"),
            }?;
        }
        Ok(())
    }
}

impl Nfa {
    /// A test as EXPLAIN shows it: its variable, labels and prefilter.
    fn describe(&self, test: usize) -> String {
        let Some(t) = self.tests.get(test) else { return String::new() };
        let var = t.var.map_or_else(|| "_".to_owned(), |v| format!("v{v}"));
        let labels = t.labels.as_ref().map_or_else(String::new, |l| format!(" :{l}"));
        let filter = if t.filter.is_some() { " where" } else { "" };
        format!("{var}{labels}{filter}")
    }
}
