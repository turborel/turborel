//! \[DESIGN\] Expressions evaluated a vector at a time (RULES.md D362,
//! D363). An expression of a batch's columns is compiled once for its
//! operator into a tree of nodes (`compile.rs`), then evaluated over each
//! batch, each node once over the rows given, after Boncz, Zukowski and
//! Nes (CIDR 2005), §4, and Kersten et al. (VLDB 2018), §4: the cost of
//! interpreting the tree is paid once a vector, not once a value.
//!
//! Each node takes the shape of its operands' vectors: exact numbers as
//! integers of a scale (`ints.rs`), approximate numbers as `f64`s
//! (`floats.rs`), comparisons into truth values a bit each (`compare.rs`),
//! three-valued logic a word of 64 rows at a time (`logic.rs`), CASE by
//! the rows each WHEN keeps (`case.rs`), CAST (`cast.rs`), and functions
//! by their vector bodies (`calls.rs`, `ScalarFunction::eval_batch`). Of
//! any other shape a node applies the rule a row's evaluation applies
//! (`value::arith`, `compare`, `cast_warned`, ...) to each value of its
//! operands' vectors, so its values are a row's. A subtree no node takes,
//! or one nested past `MAX_DEPTH`, is evaluated a row at a time
//! (`rows.rs`).
//!
//! A batch whose evaluation would raise an error, or a warning, gives
//! none: the caller evaluates that batch again a row at a time, in order,
//! so the error raised is the first row's and the rows before it come
//! first, as row by row (RULES.md D15). Integer arithmetic sets a flag of
//! the batch on overflow or on a result past `MAX_EXACT_PRECISION`
//! digits; any flag gives none. Nothing a vector path computes is seen
//! but through its result, so evaluating a batch twice changes nothing.

mod arith;
mod calls;
mod case;
mod cast;
mod compare;
mod compile;
mod floats;
mod ints;
mod logic;
mod rows;
mod vector;

use std::sync::Arc;

use crate::exec::Subqueries;
use crate::functions::ScalarRef;
use crate::kernels::Kernels;
use crate::logical::{Cond, Expr, InSet, Truth};
use crate::storage::mask::index;
use crate::storage::{Batch, Column};
use crate::value::collation::CollationRef;
use crate::value::{ArithOp, CompOp, DataType, LikePattern, Value};
use vector::Vector;
pub(in crate::exec::batch) use vector::{constant_column, gather};

/// \[DESIGN\] The deepest node evaluated over vectors (RULES.md R-CF-6):
/// a subtree below it is evaluated a row at a time, so no evaluation by
/// vectors adds a frame for each level of a deep one.
pub(in crate::exec::batch) const MAX_DEPTH: usize = 32;

/// \[DESIGN\] An expression compiled for vectors: each node of the shape
/// its operator takes, its columns the inputs of its `Program`.
#[derive(Debug, Clone)]
enum Node {
    /// The vector of the program's input at this place.
    Input(usize),
    /// A constant, every row's.
    Const(Value),
    /// [X3.135-1986 §5.9 GR4] monadic minus.
    Neg(Box<Node>),
    /// [X3.135-1986 §5.9 GR5] a dyadic arithmetic operator.
    Arith(ArithOp, Box<Node>, Box<Node>),
    /// [X3.135-1986 §5.11] a comparison, giving truth values.
    Compare(CompOp, Box<Node>, Box<Node>),
    /// [X3.135-1986 §5.18 GR2] AND, OR, NOT.
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    /// [ISO/IEC 9075:1992 §8.12] IS \[NOT\] a truth value.
    Is(Box<Node>, Truth, bool),
    /// [X3.135-1986 §5.15] IS NULL, or, if the flag, IS NOT NULL.
    IsNull(Box<Node>, bool),
    /// [ISO/IEC 9075-2:2023 §8.15] IS DISTINCT FROM.
    Distinct(Box<Node>, Box<Node>),
    /// [X3.135-1986 §5.14] LIKE a constant pattern, split once; none if a
    /// null pattern or escape makes it unknown of every row.
    Like(Box<Node>, Option<LikePattern>),
    /// [X3.135-1986 §5.14] LIKE a pattern not a constant, ESCAPE a
    /// character if given, each row's matched as a row's evaluation
    /// matches it.
    LikeOf(Box<Node>, Box<Node>, Option<Box<Node>>),
    /// [X3.135-1986 §5.13] IN a list, by its set.
    In(Box<Node>, Arc<InSet>),
    /// [ISO/IEC 9075-2:2023 §4.6.1] a BOOLEAN value's truth.
    Truth(Box<Node>),
    /// [ISO/IEC 9075:1992 §6.9] CASE: each WHEN's condition and result,
    /// and the ELSE.
    Case(Vec<(Node, Node)>, Box<Node>),
    /// [ISO/IEC 9075:1992 §6.10] CAST.
    Cast(Box<Node>, DataType),
    /// A scalar function's call.
    Call(ScalarRef, Vec<Node>),
    /// \[EXTENSION\] a character string's sort key under a collation.
    SortKey(Box<Node>, CollationRef),
    /// A value expression evaluated a row at a time, its columns the
    /// program's inputs.
    Rows(Expr),
    /// A condition evaluated a row at a time, as `Rows`.
    RowsCond(Cond),
}

/// \[DESIGN\] An expression of columns of a batch, compiled once for its
/// operator: the expression, its columns the inputs, which are places of
/// a batch; and its tree of nodes.
#[derive(Debug, Clone)]
pub(in crate::exec) struct Program {
    /// The expression, its columns places of `inputs`: what a row at a
    /// time evaluates.
    pub(in crate::exec) expr: Expr,
    /// The places of the batch its columns read.
    pub(in crate::exec) inputs: Vec<usize>,
    node: Node,
}

/// What a node is evaluated of: its program's input vectors, all of
/// `count` rows, and the statement, for calls and warnings.
struct Cx<'a> {
    inputs: Vec<&'a Column>,
    count: usize,
    statement: &'a dyn Subqueries,
}

impl Program {
    /// `e`, its columns places of `inputs`, compiled for vectors.
    pub(in crate::exec) fn of(e: Expr, inputs: Vec<usize>) -> Program {
        let node = compile::compiled(&e, MAX_DEPTH);
        Program { expr: e, inputs, node }
    }

    /// Whether its root is evaluated over vectors, not a row at a time.
    pub(in crate::exec::batch) fn vectored(&self) -> bool {
        !matches!(self.node, Node::Rows(_) | Node::RowsCond(_))
    }

    /// Whether it is a constant, and which.
    pub(in crate::exec::batch) fn constant(&self) -> Option<&Value> {
        if let Node::Const(v) = &self.node { Some(v) } else { None }
    }

    /// Its values of the rows `rows` of `batch`, or of all, a vector of
    /// as many: none if a row's evaluation would raise an error or a
    /// warning, or the batch lacks an input, which the caller then
    /// evaluates a row at a time.
    pub(in crate::exec) fn column(&self, batch: &Batch, rows: Option<&[u32]>, statement: &dyn Subqueries) -> Option<Column> {
        let count = rows.map_or(batch.rows, <[u32]>::len);
        let columns = self.inputs.iter().map(|&at| batch.columns.get(at)).collect::<Option<Vec<&Column>>>()?;
        let made: Vec<Column>;
        let inputs = match rows {
            None => columns,
            Some(rows) => {
                made = columns.iter().map(|c| gather(c, rows)).collect();
                made.iter().collect()
            }
        };
        let cx = Cx { inputs, count, statement };
        evaluated(&self.node, &cx).map(|v| v.into_column(count))
    }

    /// Of a program of a condition, the rows of `rows`, rows of `batch`,
    /// or of all, it is true of, ascending: none as `column` gives none.
    pub(in crate::exec::batch) fn kept(
        &self,
        batch: &Batch,
        rows: Option<&[u32]>,
        (statement, kernels): (&dyn Subqueries, &dyn Kernels),
    ) -> Option<Vec<u32>> {
        let truths = logic::truths(Vector::Owned(self.column(batch, rows, statement)?), rows.map_or(batch.rows, <[u32]>::len));
        let mut out = Vec::new();
        kernels.select_mask(truths.bits.words(), (truths.nulls.as_ref(), None), &mut out);
        if let Some(rows) = rows {
            for place in &mut out {
                *place = rows.get(index(*place)).copied().unwrap_or(u32::MAX);
            }
        }
        Some(out)
    }
}

/// The vector of `node` in `cx`: none as `Program::column` gives none.
fn evaluated<'a>(node: &Node, cx: &Cx<'a>) -> Option<Vector<'a>> {
    let n = cx.count;
    match node {
        Node::Input(at) => cx.inputs.get(*at).map(|c| Vector::Column(c)),
        Node::Const(v) => Some(Vector::Const(v.clone())),
        Node::Neg(x) => arith::negated(&evaluated(x, cx)?, n),
        Node::Arith(op, a, b) => arith::arith(*op, (&evaluated(a, cx)?, &evaluated(b, cx)?), n),
        Node::Compare(op, a, b) => compare::compared(*op, &evaluated(a, cx)?, &evaluated(b, cx)?, n),
        Node::And(..) | Node::Or(..) | Node::Not(_) | Node::Is(..) | Node::IsNull(..) | Node::Truth(_) => logical(node, cx),
        Node::Distinct(a, b) => compare::distinct(&evaluated(a, cx)?, &evaluated(b, cx)?, n),
        Node::Like(x, pattern) => compare::liked(&evaluated(x, cx)?, pattern.as_ref(), n),
        Node::LikeOf(x, pattern, escape) => {
            let escape = escape.as_ref().map(|e| evaluated(e, cx)).map_or(Some(None), |e| e.map(Some))?;
            compare::liked_of((&evaluated(x, cx)?, &evaluated(pattern, cx)?, escape.as_ref()), (n, cx.statement))
        }
        Node::In(x, set) => compare::within(&evaluated(x, cx)?, set, n),
        Node::Case(whens, otherwise) => case::case((whens, otherwise), cx),
        Node::Cast(x, to) => cast::cast(&evaluated(x, cx)?, to, n),
        Node::Call(function, args) => {
            let args = args.iter().map(|a| evaluated(a, cx)).collect::<Option<Vec<_>>>()?;
            calls::called(function, &args, (n, cx.statement))
        }
        Node::SortKey(x, collation) => calls::sort_keys(&evaluated(x, cx)?, collation, n),
        Node::Rows(e) => rows::values_of(e, cx),
        Node::RowsCond(c) => rows::truths_of(c, cx),
    }
}

/// The truth values of a logical node in `cx`, as `evaluated` gives them.
fn logical<'a>(node: &Node, cx: &Cx<'a>) -> Option<Vector<'a>> {
    let n = cx.count;
    let truths = |x: &Node| Some(logic::truths(evaluated(x, cx)?, n));
    let made = match node {
        Node::And(a, b) => logic::and(&truths(a)?, &truths(b)?),
        Node::Or(a, b) => logic::or(&truths(a)?, &truths(b)?),
        Node::Not(x) => logic::not(&truths(x)?),
        Node::Is(x, value, not) => logic::is(&truths(x)?, *value, *not),
        Node::IsNull(x, not) => logic::is_null(&evaluated(x, cx)?, *not, n),
        Node::Truth(x) => truths(x)?,
        Node::Input(_)
        | Node::Const(_)
        | Node::Neg(_)
        | Node::Arith(..)
        | Node::Compare(..)
        | Node::Distinct(..)
        | Node::Like(..)
        | Node::LikeOf(..)
        | Node::In(..)
        | Node::Case(..)
        | Node::Cast(..)
        | Node::Call(..)
        | Node::SortKey(..)
        | Node::Rows(_)
        | Node::RowsCond(_) => return None,
    };
    Some(Vector::Owned(Column::Bools(made)))
}

// Unit tests: every node against a row's evaluation over generated
// batches, of a fixed seed (RULES.md R-TEST-1, R-TEST-5).
#[cfg(test)]
mod tests;
