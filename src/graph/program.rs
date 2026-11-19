//! \[DESIGN\] A path pattern as the binder hands it to path search
//! [GS §6, §7, §14 steps 3-5]: element tests with label expressions and
//! prefilters, concatenation, path pattern union and multiset
//! alternation, quantifiers, parenthesized path patterns with their
//! restrictors and prefilters, and at its head a selector and a
//! restrictor, with a postfilter applied after the selector [GS §6.8
//! R6.4]. `nfa.rs` compiles it to an automaton.
//!
//! A prefilter is a hook of the binder's: it is given the element tested
//! and the bindings of the variables it declared it reads, singletons and
//! groups [P1 §4.4] and, for the cross-iteration extension, PREV [GS §11.1].

use std::fmt;
use std::sync::Arc;

use super::binding::PathBinding;
use super::view::{ElementId, LabelId, Orientation};
use crate::error::{Code, Result, SqlError};

mod bound;

pub use bound::{Bound, Read, WHOLE_PATH};
pub(super) use bound::{Resolved, absent, is_absent, marked};

/// The number of a variable of a path pattern, its index in `variables`.
pub type VarId = u32;

/// \[GS §6.6\] A variable's kind of element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    /// It binds vertices.
    Vertex,
    /// It binds edges.
    Edge,
}

/// A named element variable of a pattern. Anonymous element patterns
/// have none [P1 §6.2]: their elements are bound to no variable, which is
/// what reduction makes of their fresh names [P1 §6.5].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    /// Its name.
    pub name: String,
    /// What it binds.
    pub kind: VarKind,
}

/// \[GS §6.2\] A label expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabelExpr {
    /// A label.
    Label(LabelId),
    /// `%`: any label at all.
    Wildcard,
    /// `!`.
    Not(Box<LabelExpr>),
    /// `&`.
    And(Box<LabelExpr>, Box<LabelExpr>),
    /// `|`.
    Or(Box<LabelExpr>, Box<LabelExpr>),
}

impl LabelExpr {
    /// Whether an element of `labels` satisfies it [GS §6.2].
    pub fn holds(&self, labels: &[LabelId]) -> bool {
        match self {
            LabelExpr::Label(label) => labels.contains(label),
            LabelExpr::Wildcard => !labels.is_empty(),
            LabelExpr::Not(e) => !e.holds(labels),
            LabelExpr::And(a, b) => a.holds(labels) && b.holds(labels),
            LabelExpr::Or(a, b) => a.holds(labels) || b.holds(labels),
        }
    }
}

impl fmt::Display for LabelExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelExpr::Label(label) => write!(f, "L{label}"),
            LabelExpr::Wildcard => f.write_str("%"),
            LabelExpr::Not(e) => write!(f, "!{e}"),
            LabelExpr::And(a, b) => write!(f, "({a}&{b})"),
            LabelExpr::Or(a, b) => write!(f, "({a}|{b})"),
        }
    }
}

/// \[DESIGN\] A prefilter's test, bound by the binder: true keeps the
/// element or the iteration. It may be called from several threads.
pub type Predicate = Arc<dyn Fn(&Bound<'_>) -> Result<bool> + Send + Sync>;

/// \[DESIGN\] A postfilter's test, of a whole path binding.
pub type Postfilter = Arc<dyn Fn(&PathBinding) -> Result<bool> + Send + Sync>;

/// \[DESIGN\] The cost of an edge, a COST expression [GS §7.2].
pub type CostFn = Arc<dyn Fn(ElementId) -> Result<f64> + Send + Sync>;

/// \[GS §6.8\] A prefilter: what it reads and its test.
#[derive(Clone)]
pub struct Filter {
    /// The variables it reads, beside the element it tests.
    pub reads: Vec<Read>,
    /// Its test.
    pub test: Predicate,
}

impl fmt::Debug for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Filter {:?}", self.reads)
    }
}

/// \[GS §6.1, §6.3\] An element pattern's variable, label expression and
/// prefilter.
#[derive(Clone, Debug, Default)]
pub struct ElementTest {
    /// Its variable, none if anonymous.
    pub var: Option<VarId>,
    /// Its label expression, none for any element.
    pub labels: Option<LabelExpr>,
    /// Its WHERE.
    pub filter: Option<Filter>,
}

/// \[GS §6.5\] A quantified pattern.
#[derive(Clone, Debug)]
pub struct Quantified {
    /// What is repeated.
    pub body: Pattern,
    /// The least iterations.
    pub min: u32,
    /// The most, none if unbounded.
    pub max: Option<u32>,
}

/// \[GS §6.4\] A parenthesized path pattern.
#[derive(Clone, Debug)]
pub struct Group {
    /// Its restrictor [GS §7.1].
    pub restrictor: Restrictor,
    /// Its path pattern.
    pub body: Pattern,
    /// Its WHERE, a prefilter of each of its matches.
    pub filter: Option<Filter>,
}

/// \[GS §6\] A path pattern expression.
#[derive(Clone, Debug)]
pub enum Pattern {
    /// A node pattern: a test of the vertex the path is at.
    Vertex(ElementTest),
    /// An edge pattern: an edge followed in one of the ways allowed.
    Edge(ElementTest, Orientation),
    /// Its patterns one after another.
    Concat(Vec<Pattern>),
    /// `|`: path pattern union, of set semantics [GS §6.7].
    Union(Vec<Pattern>),
    /// `|+|`: multiset alternation, keeping duplicates [GS §6.7].
    Alternation(Vec<Pattern>),
    /// A quantified pattern [GS §6.5].
    Quantified(Box<Quantified>),
    /// A parenthesized path pattern.
    Group(Box<Group>),
}

/// \[GS §7.1\] A restrictor [P1 Fig. 7].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Restrictor {
    /// No restriction.
    Walk,
    /// No repeated edges.
    Trail,
    /// No repeated vertices.
    Acyclic,
    /// No repeated vertices, but the first may be the last.
    Simple,
}

impl fmt::Display for Restrictor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Restrictor::Walk => "WALK",
            Restrictor::Trail => "TRAIL",
            Restrictor::Acyclic => "ACYCLIC",
            Restrictor::Simple => "SIMPLE",
        })
    }
}

/// \[GS §7.2\] A selector [P1 Fig. 8], with the CHEAPEST extensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Selector {
    /// None: every match.
    All,
    /// `ANY k`; `ANY` is `ANY 1`.
    Any(u64),
    /// `ANY SHORTEST`.
    AnyShortest,
    /// `ALL SHORTEST`.
    AllShortest,
    /// `SHORTEST k`.
    Shortest(u64),
    /// `SHORTEST k GROUP`.
    ShortestGroups(u64),
    /// `ANY CHEAPEST`.
    AnyCheapest,
    /// `ALL CHEAPEST`.
    AllCheapest,
    /// `CHEAPEST k`.
    Cheapest(u64),
}

impl Selector {
    /// Its k, the matches or groups it keeps of each partition.
    pub fn k(self) -> u64 {
        match self {
            Selector::Any(k) | Selector::Shortest(k) | Selector::ShortestGroups(k) | Selector::Cheapest(k) => k,
            Selector::All | Selector::AnyShortest | Selector::AllShortest | Selector::AnyCheapest | Selector::AllCheapest => 1,
        }
    }

    /// Whether it orders a partition by cost rather than length.
    pub fn by_cost(self) -> bool {
        matches!(self, Selector::AnyCheapest | Selector::AllCheapest | Selector::Cheapest(_))
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Selector::All => f.write_str("ALL"),
            Selector::Any(k) => write!(f, "ANY {k}"),
            Selector::AnyShortest => f.write_str("ANY SHORTEST"),
            Selector::AllShortest => f.write_str("ALL SHORTEST"),
            Selector::Shortest(k) => write!(f, "SHORTEST {k}"),
            Selector::ShortestGroups(k) => write!(f, "SHORTEST {k} GROUP"),
            Selector::AnyCheapest => f.write_str("ANY CHEAPEST"),
            Selector::AllCheapest => f.write_str("ALL CHEAPEST"),
            Selector::Cheapest(k) => write!(f, "CHEAPEST {k}"),
        }
    }
}

/// \[GS §7.3\] A graph pattern's match mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MatchMode {
    /// `REPEATABLE ELEMENTS`: homomorphism.
    #[default]
    RepeatableElements,
    /// `DIFFERENT EDGES`: no edge bound twice.
    DifferentEdges,
    /// `DIFFERENT VERTICES`: no vertex bound twice, an extension.
    DifferentVertices,
}

/// \[GS §6, §7\] A path pattern with its selector, restrictor, variables,
/// postfilter and the cost of CHEAPEST.
#[derive(Clone, Debug)]
pub struct PathPattern {
    /// Its variables, numbered by `VarId`.
    pub variables: Vec<Variable>,
    /// Its selector.
    pub selector: Selector,
    /// The restrictor at its head.
    pub restrictor: Restrictor,
    /// Its path pattern expression.
    pub pattern: Pattern,
    /// The postfilter applied after the selector [GS §6.8 R6.4].
    pub postfilter: Option<PostfilterHook>,
    /// The cost of an edge for CHEAPEST, the graph's if none [GS §7.2].
    pub cost: Option<CostHook>,
}

/// A postfilter, printable.
#[derive(Clone)]
pub struct PostfilterHook(pub Postfilter);

impl fmt::Debug for PostfilterHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Postfilter")
    }
}

/// A COST expression, printable.
#[derive(Clone)]
pub struct CostHook(pub CostFn);

impl fmt::Debug for CostHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cost")
    }
}

impl PathPattern {
    /// A path pattern of `pattern` over `variables`, with no selector,
    /// restrictor, postfilter or cost.
    pub fn new(variables: Vec<Variable>, pattern: Pattern) -> PathPattern {
        PathPattern { variables, selector: Selector::All, restrictor: Restrictor::Walk, pattern, postfilter: None, cost: None }
    }

    /// It with `selector`.
    #[must_use]
    pub fn select(mut self, selector: Selector) -> PathPattern {
        self.selector = selector;
        self
    }

    /// It with `restrictor` at its head.
    #[must_use]
    pub fn restrict(mut self, restrictor: Restrictor) -> PathPattern {
        self.restrictor = restrictor;
        self
    }

    /// It with postfilter `postfilter`.
    #[must_use]
    pub fn postfilter(mut self, postfilter: Postfilter) -> PathPattern {
        self.postfilter = Some(PostfilterHook(postfilter));
        self
    }

    /// It with `cost` the cost of an edge.
    #[must_use]
    pub fn cost(mut self, cost: CostFn) -> PathPattern {
        self.cost = Some(CostHook(cost));
        self
    }
}

/// \[GS §22\] The error of a termination rule broken, `code` its GRF name.
pub(super) fn termination(rule: &str, code: &str, what: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[GS §7.4 {rule}] {code}: {what}"))
}

/// The error of a selector whose k is not positive.
pub(super) fn zero_k(selector: Selector) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[GS §7.2] the k of {selector} is not positive"))
}
