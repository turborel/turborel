//! Expressions [XQuery §3] [XQuery Appendix A.1 \[31\] to \[93\]]: one
//! `Expr` for every production from `Expr` down to `PrimaryExpr`, a
//! production that only passes its one operand on making no node of its
//! own. An operator chain of one precedence is one node, its operands in
//! the order written [XQuery Appendix A.4].

use super::direct::Constructor;
use super::types::{NodeTest, SequenceType, SingleType};
use super::{EmptyOrder, QName};

/// An expression [XQuery Appendix A.1 \[31\] to \[93\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// `Expr` \[31\] of two or more `ExprSingle`s, the comma operator
    /// [XQuery §3.3.1].
    Sequence(Vec<Expr>),
    /// `FLWORExpr` \[33\] [XQuery §3.8].
    Flwor(Box<Flwor>),
    /// `QuantifiedExpr` \[42\] [XQuery §3.11].
    Quantified(Box<Quantified>),
    /// `TypeswitchExpr` \[43\] [XQuery §3.12.2].
    Typeswitch(Box<Typeswitch>),
    /// `IfExpr` \[45\] [XQuery §3.10].
    If(Box<If>),
    /// `OrExpr` \[46\] of two or more operands [XQuery §3.6].
    Or(Vec<Expr>),
    /// `AndExpr` \[47\] of two or more operands.
    And(Vec<Expr>),
    /// `ComparisonExpr` \[48\] [XQuery §3.5].
    Comparison(Box<Comparison>),
    /// `RangeExpr` \[49\], its two operands [XQuery §3.3.1].
    Range(Box<[Expr; 2]>),
    /// `AdditiveExpr` \[50\]: its first operand, then each operator and
    /// the operand after it [XQuery §3.4].
    Additive(Box<Expr>, Vec<(AddOp, Expr)>),
    /// `MultiplicativeExpr` \[51\].
    Multiplicative(Box<Expr>, Vec<(MulOp, Expr)>),
    /// `UnionExpr` \[52\] of two or more operands, `union` and `|` alike
    /// [XQuery §3.3.3].
    Union(Vec<Expr>),
    /// `IntersectExceptExpr` \[53\].
    IntersectExcept(Box<Expr>, Vec<(SetOp, Expr)>),
    /// `InstanceofExpr` \[54\] [XQuery §3.12.1].
    InstanceOf(Box<Expr>, Box<SequenceType>),
    /// `TreatExpr` \[55\] [XQuery §3.12.6].
    Treat(Box<Expr>, Box<SequenceType>),
    /// `CastableExpr` \[56\] [XQuery §3.12.4].
    Castable(Box<Expr>, Box<SingleType>),
    /// `CastExpr` \[57\] [XQuery §3.12.3].
    Cast(Box<Expr>, Box<SingleType>),
    /// `UnaryExpr` \[58\] of one or more signs, in the order written, and
    /// its operand.
    Unary(Vec<Sign>, Box<Expr>),
    /// `ValidateExpr` \[63\] [XQuery §3.13].
    Validate(Option<ValidationMode>, Box<Expr>),
    /// `ExtensionExpr` \[65\] [XQuery §3.14]: its pragmas and the
    /// expression in its braces, if any.
    Extension(Vec<Pragma>, Option<Box<Expr>>),
    /// `PathExpr` \[68\] that is more than one `FilterExpr` [XQuery §3.2].
    Path(Box<Path>),
    /// `FilterExpr` \[81\] of one or more predicates: its primary
    /// expression and its predicates [XQuery §3.3.2].
    Filter(Box<Expr>, Vec<Expr>),
    /// `Literal` \[85\] [XQuery §3.1.1].
    Literal(Literal),
    /// `VarRef` \[87\] [XQuery §3.1.2].
    Variable(Box<QName>),
    /// `ParenthesizedExpr` \[89\] [XQuery §3.1.3].
    Parenthesized(Option<Box<Expr>>),
    /// `ContextItemExpr` \[90\] [XQuery §3.1.4].
    ContextItem,
    /// `FunctionCall` \[93\] [XQuery §3.1.5].
    FunctionCall(Box<QName>, Vec<Expr>),
    /// `OrderedExpr` \[91\] [XQuery §3.9].
    Ordered(Box<Expr>),
    /// `UnorderedExpr` \[92\].
    Unordered(Box<Expr>),
    /// `Constructor` \[94\] [XQuery §3.7].
    Constructor(Box<Constructor>),
}

/// `FLWORExpr` [XQuery Appendix A.1 \[33\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flwor {
    /// Its `for` and `let` clauses, one or more.
    pub clauses: Vec<FlworClause>,
    /// `WhereClause` \[37\]'s expression, if it has one.
    pub condition: Option<Expr>,
    /// `OrderByClause` \[38\], if it has one.
    pub order_by: Option<OrderBy>,
    /// The expression after `return`.
    pub result: Expr,
}

/// A `ForClause` \[34\] or `LetClause` \[36\] [XQuery §3.8.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlworClause {
    /// `for`, binding each variable in turn.
    For(Vec<ForBinding>),
    /// `let`.
    Let(Vec<LetBinding>),
}

/// One variable a `ForClause` binds [XQuery Appendix A.1 \[34\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForBinding {
    /// The variable.
    pub var: QName,
    /// Its `TypeDeclaration`, if it has one.
    pub declared: Option<SequenceType>,
    /// `PositionalVar` \[35\]'s variable, if it has one.
    pub position: Option<QName>,
    /// The expression after `in`.
    pub input: Expr,
}

/// One variable a `LetClause` binds [XQuery Appendix A.1 \[36\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetBinding {
    /// The variable.
    pub var: QName,
    /// Its `TypeDeclaration`, if it has one.
    pub declared: Option<SequenceType>,
    /// The expression after `:=`.
    pub value: Expr,
}

/// `OrderByClause` [XQuery Appendix A.1 \[38\]] [XQuery §3.8.3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBy {
    /// Whether it is `stable order by`.
    pub stable: bool,
    /// `OrderSpecList` \[39\], one or more.
    pub specs: Vec<OrderSpec>,
}

/// `OrderSpec` [XQuery Appendix A.1 \[40\]], with its `OrderModifier`
/// \[41\], each part as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderSpec {
    /// The expression ordered by.
    pub key: Expr,
    /// `ascending` or `descending`, if written.
    pub direction: Option<Direction>,
    /// `empty greatest` or `empty least`, if written.
    pub empty: Option<EmptyOrder>,
    /// The collation's `URILiteral`, if written.
    pub collation: Option<String>,
}

/// An order modifier's direction [XQuery Appendix A.1 \[41\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `ascending`.
    Ascending,
    /// `descending`.
    Descending,
}

/// `QuantifiedExpr` [XQuery Appendix A.1 \[42\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quantified {
    /// `some` or `every`.
    pub quantifier: Quantifier,
    /// Its variables, one or more.
    pub bindings: Vec<QuantifiedBinding>,
    /// The expression after `satisfies`.
    pub test: Expr,
}

/// A quantified expression's quantifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantifier {
    /// `some`.
    Some,
    /// `every`.
    Every,
}

/// One variable a quantified expression binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantifiedBinding {
    /// The variable.
    pub var: QName,
    /// Its `TypeDeclaration`, if it has one.
    pub declared: Option<SequenceType>,
    /// The expression after `in`.
    pub input: Expr,
}

/// `TypeswitchExpr` [XQuery Appendix A.1 \[43\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typeswitch {
    /// The operand in parentheses.
    pub operand: Expr,
    /// `CaseClause` \[44\]s, one or more.
    pub cases: Vec<CaseClause>,
    /// The default clause's variable, if it has one.
    pub default_var: Option<QName>,
    /// The default clause's expression.
    pub default: Expr,
}

/// `CaseClause` [XQuery Appendix A.1 \[44\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseClause {
    /// Its variable, if it has one.
    pub var: Option<QName>,
    /// The sequence type it matches.
    pub matches: SequenceType,
    /// The expression after `return`.
    pub result: Expr,
}

/// `IfExpr` [XQuery Appendix A.1 \[45\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct If {
    /// The test in parentheses.
    pub test: Expr,
    /// The expression after `then`.
    pub then: Expr,
    /// The expression after `else`.
    pub otherwise: Expr,
}

/// `ComparisonExpr` [XQuery Appendix A.1 \[48\]] with its comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// The left operand.
    pub left: Expr,
    /// The comparison.
    pub op: Comparator,
    /// The right operand.
    pub right: Expr,
}

/// A comparison operator [XQuery §3.5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparator {
    /// `ValueComp` \[61\].
    Value(ValueComp),
    /// `GeneralComp` \[60\].
    General(GeneralComp),
    /// `NodeComp` \[62\].
    Node(NodeComp),
}

/// `GeneralComp` [XQuery Appendix A.1 \[60\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneralComp {
    /// `=`.
    Eq,
    /// `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
}

/// `ValueComp` [XQuery Appendix A.1 \[61\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueComp {
    /// `eq`.
    Eq,
    /// `ne`.
    Ne,
    /// `lt`.
    Lt,
    /// `le`.
    Le,
    /// `gt`.
    Gt,
    /// `ge`.
    Ge,
}

/// `NodeComp` [XQuery Appendix A.1 \[62\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeComp {
    /// `is`.
    Is,
    /// `<<`.
    Precedes,
    /// `>>`.
    Follows,
}

/// An additive operator [XQuery Appendix A.1 \[50\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOp {
    /// `+`.
    Plus,
    /// `-`.
    Minus,
}

/// A multiplicative operator [XQuery Appendix A.1 \[51\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulOp {
    /// `*`.
    Times,
    /// `div`.
    Div,
    /// `idiv`.
    Idiv,
    /// `mod`.
    Mod,
}

/// An operator of `IntersectExceptExpr` [XQuery Appendix A.1 \[53\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOp {
    /// `intersect`.
    Intersect,
    /// `except`.
    Except,
}

/// A sign of `UnaryExpr` [XQuery Appendix A.1 \[58\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// `-`.
    Minus,
    /// `+`.
    Plus,
}

/// `ValidationMode` [XQuery Appendix A.1 \[64\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationMode {
    /// `lax`.
    Lax,
    /// `strict`.
    Strict,
}

/// `Pragma` [XQuery Appendix A.1 \[66\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pragma {
    /// Its QName.
    pub name: QName,
    /// `PragmaContents` \[67\]: every character after the QName to the
    /// `#)`, as written.
    pub contents: String,
}

/// `Literal` [XQuery Appendix A.1 \[85\]]: a numeric literal as written,
/// its value cast from it as it is evaluated [XQuery §3.1.1], or a string
/// literal's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    /// `IntegerLiteral` \[141\].
    Integer(String),
    /// `DecimalLiteral` \[142\].
    Decimal(String),
    /// `DoubleLiteral` \[143\].
    Double(String),
    /// `StringLiteral` \[144\]: its characters, its escapes and references
    /// replaced.
    String(String),
}

/// `PathExpr` [XQuery Appendix A.1 \[68\]] and its `RelativePathExpr`
/// \[69\]: where it starts, then its steps, each after its separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// What the path starts with.
    pub root: Root,
    /// Its first step: none only for a `/` alone.
    pub first: Option<StepExpr>,
    /// Each later step, after its separator.
    pub rest: Vec<(Separator, StepExpr)>,
}

/// What a path starts with [XQuery §3.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    /// A step: the path is relative.
    Relative,
    /// `/`, the root of the context node's tree.
    Slash,
    /// `//`, the root and its descendants.
    DoubleSlash,
}

/// A separator of steps [XQuery Appendix A.1 \[69\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    /// `/`.
    Slash,
    /// `//`, `/descendant-or-self::node()/` [XQuery §3.2.4].
    DoubleSlash,
}

/// `StepExpr` [XQuery Appendix A.1 \[70\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepExpr {
    /// `AxisStep` \[71\].
    Axis(Box<AxisStep>),
    /// `FilterExpr` \[81\]: a primary expression, and its predicates.
    Filter(Expr, Vec<Expr>),
}

/// `AxisStep` [XQuery Appendix A.1 \[71\]] [XQuery §3.2.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisStep {
    /// The step.
    pub step: Step,
    /// `PredicateList` \[82\].
    pub predicates: Vec<Expr>,
}

/// A step, as written [XQuery Appendix A.1 \[72\] to \[77\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `ForwardAxis NodeTest`.
    Forward(ForwardAxis, NodeTest),
    /// `ReverseAxis NodeTest`.
    Reverse(ReverseAxis, NodeTest),
    /// `AbbrevForwardStep` \[74\] with no `@`.
    Abbreviated(NodeTest),
    /// `AbbrevForwardStep` with `@`, `attribute::` [XQuery §3.2.4].
    Attribute(NodeTest),
    /// `AbbrevReverseStep` \[77\], `..`, `parent::node()`.
    Parent,
}

/// `ForwardAxis` [XQuery Appendix A.1 \[73\]] [XQuery §3.2.1.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardAxis {
    /// `child::`.
    Child,
    /// `descendant::`.
    Descendant,
    /// `attribute::`.
    Attribute,
    /// `self::`.
    SelfAxis,
    /// `descendant-or-self::`.
    DescendantOrSelf,
    /// `following-sibling::`.
    FollowingSibling,
    /// `following::`.
    Following,
}

/// `ReverseAxis` [XQuery Appendix A.1 \[76\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverseAxis {
    /// `parent::`.
    Parent,
    /// `ancestor::`.
    Ancestor,
    /// `preceding-sibling::`.
    PrecedingSibling,
    /// `preceding::`.
    Preceding,
    /// `ancestor-or-self::`.
    AncestorOrSelf,
}
