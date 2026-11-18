//! The operations of a plan [XQuery §3]: one for each kind of expression,
//! its names resolved, its abbreviations expanded [XQuery §3.2.4], the
//! implicit operations of the rest made by the evaluation of each.

use std::sync::Arc;

use super::super::ast::{Comparator, SetOp};
use super::super::collation::Collator;
use super::super::functions::XQueryFunction;
use super::super::types::{NodeKind, SeqType};
use crate::xml::{Atomic, AtomicType, Binding, Kind, QName};

/// An operation [XQuery §3].
#[derive(Debug)]
pub enum Op {
    /// `()`, the empty sequence [XQuery §3.1.3].
    Empty,
    /// A literal's value [XQuery §3.1.1].
    Atomic(Atomic),
    /// The comma operator [XQuery §3.3.1].
    Sequence(Vec<Op>),
    /// A variable bound in the frame, by its slot [XQuery §3.1.2].
    Local(usize),
    /// A variable of the prolog or passed in, by its place.
    Global(usize),
    /// `.` [XQuery §3.1.4].
    ContextItem,
    /// A path [XQuery §3.2].
    Path(Box<PathOp>),
    /// A filter expression: its primary expression and predicates [XQuery
    /// §3.3.2].
    Filter(Box<Op>, Vec<Op>),
    /// `to` [XQuery §3.3.1].
    Range(Box<[Op; 2]>),
    /// A chain of additive or multiplicative operators [XQuery §3.4].
    Arithmetic(Box<Op>, Vec<(Arith, Op)>),
    /// A unary operator: whether it negates, and its operand [XQuery §3.4].
    Unary(bool, Box<Op>),
    /// `union` [XQuery §3.3.3].
    Union(Vec<Op>),
    /// `intersect` and `except`.
    IntersectExcept(Box<Op>, Vec<(SetOp, Op)>),
    /// A comparison [XQuery §3.5].
    Compare(Box<CompareOp>),
    /// `and` [XQuery §3.6].
    And(Vec<Op>),
    /// `or`.
    Or(Vec<Op>),
    /// `if`: its test, then and else [XQuery §3.10].
    If(Box<[Op; 3]>),
    /// A FLWOR expression [XQuery §3.8].
    Flwor(Box<FlworOp>),
    /// `some` or `every` [XQuery §3.11].
    Quantified(Box<QuantifiedOp>),
    /// `typeswitch` [XQuery §3.12.2].
    Typeswitch(Box<TypeswitchOp>),
    /// `instance of` [XQuery §3.12.1].
    InstanceOf(Box<Op>, SeqType),
    /// `treat as` [XQuery §3.12.6].
    Treat(Box<Op>, SeqType),
    /// `castable as` [XQuery §3.12.4].
    Castable(Box<Op>, CastTarget),
    /// `cast as` [XQuery §3.12.3].
    Cast(Box<Op>, CastTarget),
    /// A function call [XQuery §3.1.5].
    Call(Box<CallOp>),
    /// `validate`, strict [XQuery §3.13].
    Validate(Box<Op>),
    /// A direct element constructor [XQuery §3.7.1].
    Element(Box<DirectElement>),
    /// A computed element constructor [XQuery §3.7.3.1].
    ComputedElement(Box<ComputedElement>),
    /// A computed attribute constructor: its name and content [XQuery
    /// §3.7.3.2].
    Attribute(Box<NameOp>, Option<Box<Op>>),
    /// `document` [XQuery §3.7.3.3].
    Document(Box<Op>),
    /// `text` [XQuery §3.7.3.4].
    Text(Box<Op>),
    /// A computed comment constructor [XQuery §3.7.3.6].
    Comment(Box<Op>),
    /// A direct comment constructor [XQuery §3.7.2].
    DirectComment(Box<str>),
    /// A processing instruction constructor, direct or computed [XQuery
    /// §3.7.2, §3.7.3.5].
    Pi(Box<PiOp>),
}

/// An arithmetic operator [XQuery §3.4].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    /// `+`.
    Add,
    /// `-`.
    Subtract,
    /// `*`.
    Multiply,
    /// `div`.
    Divide,
    /// `idiv`.
    IntegerDivide,
    /// `mod`.
    Modulo,
}

/// A path [XQuery §3.2]: where it starts, then its steps.
#[derive(Debug)]
pub struct PathOp {
    /// Where it starts.
    pub(in crate::xquery) start: PathStart,
    /// Its steps, `//` expanded to `descendant-or-self::node()`.
    pub(in crate::xquery) steps: Vec<StepOp>,
}

/// Where a path starts [XQuery §3.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStart {
    /// The context item.
    Context,
    /// `/`: the root of the context node's tree, a document node.
    Root,
}

/// A step of a path [XQuery §3.2.1].
#[derive(Debug)]
pub enum StepOp {
    /// An axis step: its axis, node test and predicates.
    Axis(Axis, StepTest, Vec<Op>),
    /// A filter expression, evaluated with each node as its focus.
    Filter(Op, Vec<Op>),
}

/// An axis [XQuery §3.2.1.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// `child`.
    Child,
    /// `descendant`.
    Descendant,
    /// `attribute`.
    Attribute,
    /// `self`.
    SelfNode,
    /// `descendant-or-self`.
    DescendantOrSelf,
    /// `following-sibling`.
    FollowingSibling,
    /// `following`.
    Following,
    /// `parent`.
    Parent,
    /// `ancestor`.
    Ancestor,
    /// `preceding-sibling`.
    PrecedingSibling,
    /// `preceding`.
    Preceding,
    /// `ancestor-or-self`.
    AncestorOrSelf,
}

impl Axis {
    /// Whether it is a reverse axis [XQuery §3.2.1.1].
    #[must_use]
    pub fn reverse(self) -> bool {
        match self {
            Axis::Parent | Axis::Ancestor | Axis::PrecedingSibling | Axis::Preceding | Axis::AncestorOrSelf => true,
            Axis::Child
            | Axis::Descendant
            | Axis::Attribute
            | Axis::SelfNode
            | Axis::DescendantOrSelf
            | Axis::FollowingSibling
            | Axis::Following => false,
        }
    }

    /// Its principal node kind [XQuery §3.2.1.1].
    #[must_use]
    pub fn principal(self) -> Kind {
        if self == Axis::Attribute { Kind::Attribute } else { Kind::Element }
    }
}

/// A node test, resolved [XQuery §3.2.1.2].
#[derive(Debug)]
pub enum StepTest {
    /// A name test: the namespace, and the local name, none for any.
    Name(Namespace, Option<Box<str>>),
    /// A kind test.
    Kind(NodeKind),
}

/// The namespace a name test names [XQuery §3.2.1.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Namespace {
    /// Any, of `*` and `*:NCName`.
    Any,
    /// None.
    Absent,
    /// This one.
    Uri(Box<str>),
}

impl Namespace {
    /// The namespace `uri` is, none if it is none.
    #[must_use]
    pub fn of(uri: Option<Box<str>>) -> Namespace {
        uri.map_or(Namespace::Absent, Namespace::Uri)
    }

    /// Whether a name in the namespace `uri`, none if in none, is in it.
    #[must_use]
    pub fn admits(&self, uri: Option<&str>) -> bool {
        match self {
            Namespace::Any => true,
            Namespace::Absent => uri.is_none(),
            Namespace::Uri(u) => uri == Some(&**u),
        }
    }
}

/// A comparison [XQuery §3.5].
#[derive(Debug)]
pub struct CompareOp {
    /// Its left operand.
    pub(in crate::xquery) left: Op,
    /// Its operator.
    pub(in crate::xquery) op: Comparator,
    /// Its right operand.
    pub(in crate::xquery) right: Op,
    /// The default collation strings are compared by.
    pub(in crate::xquery) collation: Collator,
}

/// A FLWOR expression [XQuery §3.8].
#[derive(Debug)]
pub struct FlworOp {
    /// Its `for` and `let` clauses, a variable each.
    pub(in crate::xquery) clauses: Vec<Clause>,
    /// Its `where` expression.
    pub(in crate::xquery) condition: Option<Op>,
    /// Its order specs, none if it has no `order by`.
    pub(in crate::xquery) order: Vec<OrderKey>,
    /// Its `return` expression.
    pub(in crate::xquery) result: Op,
}

/// A variable a FLWOR expression binds [XQuery §3.8.1].
#[derive(Debug)]
pub enum Clause {
    /// `for`: its slot, its positional variable's, its declared type and
    /// its binding sequence.
    For(usize, Option<usize>, Option<SeqType>, Op),
    /// `let`: its slot, declared type and value.
    Let(usize, Option<SeqType>, Op),
}

/// An order spec [XQuery §3.8.3].
#[derive(Debug)]
pub struct OrderKey {
    /// Its expression.
    pub(in crate::xquery) key: Op,
    /// Whether it is `descending`.
    pub(in crate::xquery) descending: bool,
    /// Whether the empty sequence orders greatest.
    pub(in crate::xquery) empty_greatest: bool,
    /// The collation strings are ordered by, and whether one was written.
    pub(in crate::xquery) collation: (Collator, bool),
}

/// A quantified expression [XQuery §3.11].
#[derive(Debug)]
pub struct QuantifiedOp {
    /// Whether it is `every`.
    pub(in crate::xquery) every: bool,
    /// Its variables: each slot, declared type and binding sequence.
    pub(in crate::xquery) bindings: Vec<(usize, Option<SeqType>, Op)>,
    /// Its test.
    pub(in crate::xquery) test: Op,
}

/// A typeswitch expression [XQuery §3.12.2].
#[derive(Debug)]
pub struct TypeswitchOp {
    /// Its operand.
    pub(in crate::xquery) operand: Op,
    /// Its case clauses: each variable's slot, type and result.
    pub(in crate::xquery) cases: Vec<(Option<usize>, SeqType, Op)>,
    /// Its default clause's variable's slot and result.
    pub(in crate::xquery) default: (Option<usize>, Op),
}

/// The target type of a cast [XQuery §3.12.3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastTarget {
    /// The atomic type.
    pub(in crate::xquery) to: AtomicType,
    /// Whether `?` allows the empty sequence.
    pub(in crate::xquery) optional: bool,
}

/// A function call [XQuery §3.1.5].
#[derive(Debug)]
pub struct CallOp {
    /// The function.
    pub(in crate::xquery) callee: Callee,
    /// Its arguments.
    pub(in crate::xquery) arguments: Vec<Op>,
}

/// The function a call calls.
pub enum Callee {
    /// A function of the prolog, by its place.
    Declared(usize),
    /// A function of the library.
    Library(Arc<dyn XQueryFunction>),
}

impl std::fmt::Debug for Callee {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Callee::Declared(at) => write!(f, "Declared({at})"),
            Callee::Library(function) => write!(f, "Library({:?})", function.name()),
        }
    }
}

/// A direct element constructor [XQuery §3.7.1].
#[derive(Debug)]
pub struct DirectElement {
    /// Its name, expanded.
    pub(in crate::xquery) name: QName,
    /// The bindings its namespace declaration attributes make [XQuery
    /// §3.7.1.2].
    pub(in crate::xquery) declared: Vec<Binding>,
    /// Those of the direct element constructors it is within, the inner
    /// overriding, which an element made standing alone declares too
    /// [XQuery §3.7.4].
    pub(in crate::xquery) inherited: Vec<Binding>,
    /// Its attributes: each name and value's parts [XQuery §3.7.1.1].
    pub(in crate::xquery) attributes: Vec<(QName, Vec<Part>)>,
    /// Its content [XQuery §3.7.1.3].
    pub(in crate::xquery) content: Vec<Content>,
}

/// A part of a direct attribute's value [XQuery §3.7.1.1].
#[derive(Debug)]
pub enum Part {
    /// Characters, normalized.
    Text(Box<str>),
    /// An enclosed expression.
    Enclosed(Op),
}

/// A part of a direct element's content [XQuery §3.7.1.3].
#[derive(Debug)]
pub enum Content {
    /// Characters, boundary whitespace removed by the policy.
    Text(Box<str>),
    /// A nested direct element constructor.
    Element(DirectElement),
    /// A nested direct comment constructor.
    Comment(Box<str>),
    /// A nested direct processing instruction constructor: its target and
    /// content.
    Pi(Box<str>, Box<str>),
    /// An enclosed expression.
    Enclosed(Op),
}

/// A computed element constructor [XQuery §3.7.3.1].
#[derive(Debug)]
pub struct ComputedElement {
    /// Its name.
    pub(in crate::xquery) name: NameOp,
    /// Its content expression.
    pub(in crate::xquery) content: Option<Op>,
    /// The bindings of the direct element constructors it is within.
    pub(in crate::xquery) inherited: Vec<Binding>,
}

/// The name of a computed element or attribute [XQuery §3.7.3.1,
/// §3.7.3.2].
#[derive(Debug)]
pub enum NameOp {
    /// A QName, expanded.
    Fixed(QName),
    /// A name expression, and the statically known namespaces its string
    /// is resolved by.
    Computed(Op, Arc<StaticNames>),
}

/// The statically known namespaces and default element/type namespace of
/// a name expression, which resolve the QName its string is [XQuery
/// §3.7.3.1].
#[derive(Debug)]
pub struct StaticNames {
    /// The prefixes bound, each to its URI.
    pub(in crate::xquery) prefixes: Vec<(Box<str>, Box<str>)>,
    /// The default element/type namespace, if any.
    pub(in crate::xquery) default_element: Option<Box<str>>,
}

/// A processing instruction constructor [XQuery §3.7.2, §3.7.3.5].
#[derive(Debug)]
pub struct PiOp {
    /// Its target, or the name expression of one.
    pub(in crate::xquery) target: Result<Box<str>, Op>,
    /// Its content expression, or the content of a direct one.
    pub(in crate::xquery) content: Result<Box<str>, Option<Op>>,
}
