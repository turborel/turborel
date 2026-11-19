//! \[EXTENSION\] Abstract syntax of the graph pattern matching language
//! that SQL/PGQ and GQL share, GPML [P1 §4, §5]: a `<graph pattern>`, its
//! path patterns, their element patterns, label expressions, quantifiers,
//! unions, restrictors, selectors and match modes.
//!
//! One tree serves every front end, as written and, once
//! `parser::graph::normalize` has run, in normal form [P1 §6.2]: every
//! element named, edges in full form, a node pattern on each side of every
//! edge, and `*` and `+` read as their bounds. The GRAPH_TABLE around it is
//! in `graph/table.rs`, the expressions it adds in `graph/expression.rs`,
//! and the edge orientations' table in `graph/orientation.rs`.

mod compat;
mod expression;
mod hints;
mod orientation;
mod table;

pub use compat::{GraphMeasure, MatchClause, PathTable};
pub use expression::{GRAPH_FUNCTIONS, GraphFunction, GraphFunctionName, GraphPredicate, PathEnd, SameKind, VariableKind};
pub use hints::{GraphHints, PathAlgorithm};
pub use orientation::{Directions, ORIENTATIONS, Orientation, OrientationForm};
pub use table::{GraphColumn, GraphLimits, GraphReference, GraphRows, GraphTable};

use super::{SearchCondition, ValueExpression};

/// A variable of a graph pattern, an element's or a path's: as written,
/// or one normalization gave an element written with none [P1 §6.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphVariable {
    /// Its name: an `<identifier>` as the lexer gives it, or for one
    /// normalization gave, a name no other variable of the pattern has.
    pub name: String,
    /// Where it is written, or where its element begins if normalization
    /// named it.
    pub at: usize,
    /// Whether normalization gave it: the element was written with none.
    pub anonymous: bool,
}

/// [P1 §4.3] `<graph pattern>` ::= \[`<match mode>`\] `<path pattern
/// list>` \[KEEP `<path pattern prefix>`\] \[WHERE `<search
/// condition>`\] \[`<segment filter>`...\]: the path patterns, joined on the singleton variables
/// they share; KEEP's prefix applies to each [P1 §7.2]; the WHERE is the
/// postfilter, applied after any selector [P1 §5.2].
#[derive(Debug, Clone)]
pub struct GraphPattern {
    /// The match mode, if written.
    pub mode: Option<MatchMode>,
    /// The path patterns, in order, one at least.
    pub paths: Vec<PathPattern>,
    /// The prefix KEEP gives every path pattern, if written.
    pub keep: Option<PathPrefix>,
    /// The postfilter, if written.
    pub where_clause: Option<SearchCondition>,
    /// \[EXTENSION\] The postfilters of consecutive edges, after it.
    pub segments: Vec<SegmentFilter>,
    /// Where the pattern begins.
    pub at: usize,
}

/// \[EXTENSION\] FOR EACH SEGMENT ( `<variable>` , `<variable>` ) IN
/// `<path variable>` WHERE `<search condition>`: a postfilter true of each
/// two consecutive edges of the path, bound to the two variables.
#[derive(Debug, Clone)]
pub struct SegmentFilter {
    /// The earlier edge's variable.
    pub first: GraphVariable,
    /// The later edge's variable.
    pub second: GraphVariable,
    /// The path.
    pub path: GraphVariable,
    /// The condition.
    pub condition: SearchCondition,
}

/// [P1 §7.1] `<match mode>`: whether an element may be bound twice
/// across the path patterns of a graph pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// REPEATABLE ELEMENTS, the default: homomorphism.
    RepeatableElements,
    /// DIFFERENT EDGES: no edge bound twice.
    DifferentEdges,
    /// \[EXTENSION\] DIFFERENT VERTICES: no vertex bound twice.
    DifferentVertices,
}

/// [P1 §4.2] `<path pattern>` ::= \[`<path variable>` =\] \[`<path
/// pattern prefix>`\] `<path pattern expression>`, the two first in either
/// order.
#[derive(Debug, Clone)]
pub struct PathPattern {
    /// The path variable, if written.
    pub variable: Option<GraphVariable>,
    /// The selector and restrictor, if written.
    pub prefix: Option<PathPrefix>,
    /// The pattern.
    pub expression: PathExpression,
    /// Where it begins.
    pub at: usize,
}

/// [P1 §5.1] A `<path pattern prefix>`: a selector, a restrictor, or a
/// selector then a restrictor. Inside parentheses only a restrictor may
/// be written (GRF-0033).
#[derive(Debug, Clone)]
pub struct PathPrefix {
    /// The selector, if written.
    pub selector: Option<Selector>,
    /// The restrictor, if written.
    pub restrictor: Option<Restrictor>,
    /// Where it begins.
    pub at: usize,
}

/// [P1 §5.1, Fig. 7] A restrictor: a path predicate under which a
/// pattern has finitely many matches, but WALK's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restrictor {
    /// WALK: no restriction.
    Walk,
    /// TRAIL: no edge repeated.
    Trail,
    /// ACYCLIC: no node repeated.
    Acyclic,
    /// SIMPLE: no node repeated, but the first may be the last.
    Simple,
}

impl Restrictor {
    /// Whether it bounds the matches of an unbounded quantifier in its
    /// scope [P1 §5]: all but WALK do.
    #[must_use]
    pub fn restricts(self) -> bool {
        match self {
            Restrictor::Walk => false,
            Restrictor::Trail | Restrictor::Acyclic | Restrictor::Simple => true,
        }
    }
}

/// [P1 §5.1, Fig. 8] A selector: picks finitely many matches of each
/// partition by endpoints.
#[derive(Debug, Clone)]
pub enum Selector {
    /// ANY: one path.
    Any,
    /// ANY k: k paths.
    AnyK(u32),
    /// ANY SHORTEST: one shortest path.
    AnyShortest,
    /// ALL SHORTEST: every shortest path.
    AllShortest,
    /// SHORTEST k: the k shortest paths.
    ShortestK(u32),
    /// SHORTEST k GROUP or GROUPS: every path of the k shortest lengths.
    ShortestGroups(u32),
    /// \[EXTENSION\] ANY CHEAPEST, ALL CHEAPEST or CHEAPEST k, by the
    /// cost of each edge.
    Cheapest {
        /// Which of the cheapest.
        which: Cheapest,
        /// COST's expression of an edge, if written.
        cost: Option<Box<ValueExpression>>,
    },
}

/// \[EXTENSION\] Which paths a CHEAPEST selector keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cheapest {
    /// ANY CHEAPEST: one path of the least cost.
    Any,
    /// ALL CHEAPEST: every path of the least cost.
    All,
    /// CHEAPEST k: the k cheapest paths.
    K(u32),
}

/// [P1 §4.5] `<path pattern expression>`: a concatenation, or a union or
/// multiset alternation of them, which are not mixed unparenthesized.
#[derive(Debug, Clone)]
pub enum PathExpression {
    /// A `<path term>`: one concatenation.
    Term(PathTerm),
    /// `|`, path pattern union: its terms' matches, set semantics.
    Union(Vec<PathTerm>),
    /// `|+|`, multiset alternation: its terms' matches, duplicates kept.
    Alternation(Vec<PathTerm>),
}

impl PathExpression {
    /// Its terms: one of a concatenation, each operand of a union or
    /// alternation.
    #[must_use]
    pub fn terms(&self) -> &[PathTerm] {
        match self {
            PathExpression::Term(t) => std::slice::from_ref(t),
            PathExpression::Union(terms) | PathExpression::Alternation(terms) => terms,
        }
    }
}

/// [P1 §4.2] `<path term>`: path factors concatenated, one at least.
#[derive(Debug, Clone)]
pub struct PathTerm {
    /// The factors, in order.
    pub factors: Vec<PathFactor>,
}

/// [P1 §4.4] `<path factor>` ::= `<path primary>` \[`<quantifier>`\].
#[derive(Debug, Clone)]
pub struct PathFactor {
    /// The primary.
    pub primary: PathPrimary,
    /// Its quantifier, if written; normalization moves an edge's onto
    /// parentheses around it and its end nodes.
    pub quantifier: Option<Quantifier>,
}

/// [P1 §4.1, §4.4] `<path primary>`: an element pattern, or a
/// parenthesized path pattern.
#[derive(Debug, Clone)]
pub enum PathPrimary {
    /// A node pattern.
    Node(ElementPattern),
    /// An edge pattern.
    Edge(EdgePattern),
    /// A parenthesized path pattern.
    Parenthesized(Box<ParenthesizedPath>),
}

/// [P1 §4.1] The filler of an element pattern: ( \[`<variable>`\] \[IS
/// `<label expression>` | : `<label expression>`\] \[WHERE `<search
/// condition>`\] \[{ `<property name>` : `<value expression>` \[, ...\]
/// }\] ), of a node in parentheses, of an edge in brackets.
#[derive(Debug, Clone)]
pub struct ElementPattern {
    /// The element variable, if written or given by normalization.
    pub variable: Option<GraphVariable>,
    /// The label expression, if written.
    pub label: Option<LabelExpression>,
    /// The element's prefilter, if written.
    pub where_clause: Option<SearchCondition>,
    /// \[EXTENSION\] The properties it has, each equal to its value, as
    /// GQL writes them.
    pub properties: Vec<PropertySpec>,
    /// Where it begins.
    pub at: usize,
}

/// \[EXTENSION\] One `<property name>` : `<value expression>` of an
/// element pattern's property specification.
#[derive(Debug, Clone)]
pub struct PropertySpec {
    /// The property's name.
    pub name: String,
    /// The value it equals.
    pub value: ValueExpression,
    /// Where the name is written.
    pub at: usize,
}

/// [P1 §4.1, Fig. 5] An edge pattern: its orientation, and its filler
/// written in brackets, or none in an abbreviation.
#[derive(Debug, Clone)]
pub struct EdgePattern {
    /// Which edges it matches, and in which direction.
    pub orientation: Orientation,
    /// Whether it was written abbreviated, as `->`, with no filler;
    /// normalization writes it in full.
    pub abbreviated: bool,
    /// Its variable, label expression and prefilter.
    pub filler: ElementPattern,
}

/// [P1 §4.4] A parenthesized path pattern: ( \[`<path variable>` =\]
/// \[`<restrictor>`\] `<path pattern expression>` \[WHERE `<search
/// condition>`\] ), or the same in brackets.
#[derive(Debug, Clone)]
pub struct ParenthesizedPath {
    /// The subpath variable, if written.
    pub variable: Option<GraphVariable>,
    /// The prefix, if written: a restrictor, a selector being refused
    /// (GRF-0033).
    pub prefix: Option<PathPrefix>,
    /// The pattern.
    pub expression: PathExpression,
    /// Its prefilter, if written.
    pub where_clause: Option<SearchCondition>,
    /// Whether it is written in brackets.
    pub bracketed: bool,
    /// Where it begins.
    pub at: usize,
}

/// [P1 §4.4, Fig. 6] A quantifier: the least and most repetitions, none
/// the most of an unbounded one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantifier {
    /// The least repetitions.
    pub lower: u32,
    /// The most repetitions, none if unbounded.
    pub upper: Option<u32>,
    /// The symbol it was written as, if not braces; normalization keeps
    /// only `?`, whose singletons are conditional [P1 §4.6].
    pub sugar: Option<QuantifierSugar>,
    /// Where it is written.
    pub at: usize,
}

impl Quantifier {
    /// Whether it has no upper bound.
    #[must_use]
    pub fn unbounded(&self) -> bool {
        self.upper.is_none()
    }

    /// Whether it is `?`, which exposes its singletons as conditional
    /// singletons rather than group variables [P1 §4.6].
    #[must_use]
    pub fn optional(&self) -> bool {
        self.sugar == Some(QuantifierSugar::Question)
    }
}

/// [P1 Fig. 6] The quantifiers written as one symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantifierSugar {
    /// `*`, { 0, }.
    Star,
    /// `+`, { 1, }.
    Plus,
    /// `?`, { 0, 1 } of conditional singletons.
    Question,
}

/// [P1 §4.1] A label expression: a label, `%`, or `!`, `&` and `|` of
/// them, `!` binding tightest, then `&`, then `|`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelExpression {
    /// A label, and where it is written.
    Label(String, usize),
    /// `%`: any label; written where.
    Wildcard(usize),
    /// `!`: not the label expression.
    Negation(Box<LabelExpression>),
    /// `&`: both.
    Conjunction(Box<LabelExpression>, Box<LabelExpression>),
    /// `|`: either.
    Disjunction(Box<LabelExpression>, Box<LabelExpression>),
}
