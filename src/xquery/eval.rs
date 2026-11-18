//! The dynamic evaluation of a plan [XQuery §2.2.3.2]: each operation
//! evaluated in a dynamic context [XQuery §2.1.2] of its focus (`Focus`),
//! the values of the variables of its frame (`Frame`) and of the globals,
//! and what the query shares (`Evaluator`). Sequences, atomization and the
//! effective boolean value are in `eval/sequence.rs`; paths, axes and
//! predicates in `eval/path.rs`; the operators in `eval/arith.rs` and
//! `eval/compare.rs`, which call the library's (`xquery::library::op`); the
//! expressions that bind variables in `eval/flwor.rs`; sequence types,
//! casts and function calls in `eval/types.rs` and `eval/call.rs`;
//! constructors in `eval/construct.rs`; and validate in `eval/validate.rs`.
//!
//! \[DESIGN\] RULES.md R-CF-6, D44, D86: the evaluation recurses as the
//! plan nests and as functions call functions, one level each operation
//! evaluated within another and each call, to `MAX_DEPTH` in all, past
//! which it is SQLCODE -900. Each item of a loop, a path step or a
//! predicate looks whether the statement was cancelled. A sequence holds
//! at most `xml::MAX_NODES` items, a string `xml::MAX_TEXT` octets, and
//! the trees one evaluation constructs `xml::MAX_STORED` octets together.

mod arith;
mod call;
mod compare;
mod construct;
mod flwor;
mod path;
mod sequence;
mod types;
mod validate;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::error::{ErrorCode, beyond, error};
use super::functions::QueryContext;
use super::plan::{Op, Plan};
use crate::error::Result;
use crate::xml::{Item, Moment, Sequence};

pub(super) use sequence::ebv;

/// \[DESIGN\] How deep the evaluation of a query nests at most: operations
/// evaluated within others, and function calls within calls (RULES.md
/// R-CF-6). \[XQuery\] sets no limit, so a deeper evaluation is SQLCODE
/// -900 (D18). The value is no measurement: it is past what the queries
/// an application writes nest, a recursive function of a body a few
/// levels deep called some hundred deep, and the deepest evaluation runs
/// within half of a thread's default stack, as a test checks.
pub const MAX_DEPTH: usize = 256;

/// A sequence as the evaluation holds it.
type Seq = Vec<Item>;

/// What an evaluation of a query is given [ISO/IEC 9075-14:2006 draft
/// §6.18 GR1]: its context item, if one is passed; the values of the
/// variables passed, in the order of `Environment::variables`; its current
/// dateTime and implicit timezone [XQuery Appendix C.2]; and what says
/// whether the statement evaluating it was cancelled (RULES.md D44).
pub struct Input<'a> {
    /// The context item, if one is passed.
    pub context_item: Option<Item>,
    /// The values of the variables passed.
    pub variables: Vec<Sequence>,
    /// The current dateTime, with its time zone.
    pub now: Moment,
    /// The implicit timezone, in minutes east of UTC.
    pub timezone: i16,
    /// What fails if the statement was cancelled or ran past its time.
    pub interrupted: &'a dyn Fn() -> Result<()>,
}

/// The focus of an expression [XQuery §2.1.2]: its context item, if one is
/// defined, and the context position and size.
#[derive(Debug, Clone, Default)]
struct Focus {
    /// The context item.
    item: Option<Item>,
    /// The context position, from 1.
    position: usize,
    /// The context size.
    size: usize,
}

impl Focus {
    /// The focus of `item`, at `position` of `size`.
    fn of(item: Item, position: usize, size: usize) -> Focus {
        Focus { item: Some(item), position, size }
    }
}

/// The values of the variables a body binds, by slot.
#[derive(Debug)]
struct Frame {
    /// Each slot's value.
    slots: Vec<Rc<Seq>>,
}

impl Frame {
    /// A frame of `slots` slots, each the empty sequence.
    fn new(slots: usize) -> Frame {
        Frame { slots: (0..slots).map(|_| Rc::new(Vec::new())).collect() }
    }

    /// The value of `slot`.
    fn get(&self, slot: usize) -> Rc<Seq> {
        self.slots.get(slot).cloned().unwrap_or_default()
    }

    /// `slot` bound to `value`.
    fn set(&mut self, slot: usize, value: Rc<Seq>) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = value;
        }
    }
}

/// An evaluation of a plan.
struct Evaluator<'q> {
    /// The plan.
    plan: &'q Plan,
    /// What the calls of functions share of the dynamic context.
    context: QueryContext<'q>,
    /// The values of the variables passed.
    passed: Vec<Rc<Seq>>,
    /// The values of the globals evaluated so far.
    globals: RefCell<Vec<Option<Rc<Seq>>>>,
    /// How deep the evaluation is now.
    depth: Cell<usize>,
    /// How many octets the trees it constructed hold.
    constructed: Cell<usize>,
    /// The focus of the query body and the prolog's initializing
    /// expressions.
    initial: Focus,
}

/// A level of the evaluation's depth, given back as it is dropped.
struct Deeper<'a>(&'a Cell<usize>);

impl Drop for Deeper<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// The value of `plan` given `input` [XQuery §2.2.3.2]: its body
/// evaluated with the context item passed as its focus, at position 1 of
/// 1 [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii 2 B II].
pub(super) fn evaluate(plan: &Plan, input: Input<'_>) -> Result<Sequence> {
    let context = QueryContext {
        now: input.now,
        timezone: input.timezone,
        collation: &plan.settings.collation,
        collations: &plan.collations,
        base_uri: plan.settings.base_uri.as_deref(),
        interrupted: input.interrupted,
    };
    let evaluator = Evaluator {
        plan,
        context,
        passed: input.variables.into_iter().map(|s| Rc::new(s.into_items())).collect(),
        globals: RefCell::new(vec![None; plan.globals.len()]),
        depth: Cell::new(0),
        constructed: Cell::new(0),
        initial: input.context_item.map_or_else(Focus::default, |item| Focus::of(item, 1, 1)),
    };
    let mut frame = Frame::new(plan.body.slots);
    evaluator.eval(&plan.body.op, &mut frame, &evaluator.initial).map(Sequence::new)
}

impl Evaluator<'_> {
    /// One level deeper, if within `MAX_DEPTH`.
    fn deeper(&self) -> Result<Deeper<'_>> {
        let depth = self.depth.get();
        if depth >= MAX_DEPTH {
            return Err(beyond(&format!("it nests deeper than {MAX_DEPTH} levels of operations and function calls")));
        }
        self.depth.set(depth.saturating_add(1));
        Ok(Deeper(&self.depth))
    }

    /// The error if the statement was cancelled (RULES.md D44).
    fn check(&self) -> Result<()> {
        (self.context.interrupted)()
    }

    /// The value of `op` in `frame` and `focus` [XQuery §3].
    fn eval(&self, op: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let _deeper = self.deeper()?;
        // [DESIGN] Each kind is evaluated by a function of its own, so the
        // frame the nesting of operations recurs through holds none of
        // their locals (RULES.md R-CF-6).
        match op {
            Op::Empty => Ok(Vec::new()),
            Op::Atomic(a) => Ok(vec![Item::Atomic(a.clone())]),
            Op::Sequence(ops) => self.comma(ops, frame, focus),
            Op::Local(_) | Op::Global(_) => self.shared(op, frame, focus).map(|s| (*s).clone()),
            Op::ContextItem => focus.item.clone().map(|i| vec![i]).ok_or_else(no_context_item),
            Op::Path(p) => self.path(p, frame, focus),
            Op::Filter(base, predicates) => self.filter(base, predicates, frame, focus),
            Op::Range(operands) => self.range(operands, frame, focus),
            Op::Arithmetic(first, rest) => self.arithmetic(first, rest, frame, focus),
            Op::Unary(negate, operand) => self.unary(*negate, operand, frame, focus),
            Op::Union(operands) => self.union(operands, frame, focus),
            Op::IntersectExcept(first, rest) => self.intersect_except(first, rest, frame, focus),
            Op::Compare(c) => self.compare(c, frame, focus),
            Op::And(operands) => self.logical(operands, false, frame, focus),
            Op::Or(operands) => self.logical(operands, true, frame, focus),
            Op::If(parts) => self.conditional(parts, frame, focus),
            Op::Flwor(f) => self.flwor(f, frame, focus),
            Op::Quantified(q) => self.quantified(q, frame, focus),
            Op::Typeswitch(t) => self.typeswitch(t, frame, focus),
            Op::InstanceOf(operand, t) => self.instance_of(operand, t, frame, focus),
            Op::Treat(operand, t) => self.treat(operand, t, frame, focus),
            Op::Castable(operand, target) => self.castable(operand, *target, frame, focus),
            Op::Cast(operand, target) => self.cast(operand, *target, frame, focus),
            Op::Call(c) => self.call(c, frame, focus),
            Op::Validate(mode, operand) => self.validate(*mode, operand, frame, focus),
            Op::Element(e) => self.direct_element(e, frame, focus),
            Op::ComputedElement(e) => self.computed_element(e, frame, focus),
            Op::Attribute(name, content) => self.computed_attribute(name, content.as_deref(), frame, focus),
            Op::Document(content) => self.document(content, frame, focus),
            Op::Text(content) => self.text(content, frame, focus),
            Op::Comment(content) => self.comment(content, frame, focus),
            Op::DirectComment(text) => self.direct_comment(text),
            Op::Pi(pi) => self.pi(pi, frame, focus),
        }
    }

    /// The value of `op` as one sequence held once: a variable's own,
    /// unchanged, so a large one is not copied to be read.
    fn shared(&self, op: &Op, frame: &mut Frame, focus: &Focus) -> Result<Rc<Seq>> {
        match op {
            Op::Local(slot) => Ok(frame.get(*slot)),
            Op::Global(at) => self.global(*at),
            Op::Empty
            | Op::Atomic(_)
            | Op::Sequence(_)
            | Op::ContextItem
            | Op::Path(_)
            | Op::Filter(..)
            | Op::Range(_)
            | Op::Arithmetic(..)
            | Op::Unary(..)
            | Op::Union(_)
            | Op::IntersectExcept(..)
            | Op::Compare(_)
            | Op::And(_)
            | Op::Or(_)
            | Op::If(_)
            | Op::Flwor(_)
            | Op::Quantified(_)
            | Op::Typeswitch(_)
            | Op::InstanceOf(..)
            | Op::Treat(..)
            | Op::Castable(..)
            | Op::Cast(..)
            | Op::Call(_)
            | Op::Validate(..)
            | Op::Element(_)
            | Op::ComputedElement(_)
            | Op::Attribute(..)
            | Op::Document(_)
            | Op::Text(_)
            | Op::Comment(_)
            | Op::DirectComment(_)
            | Op::Pi(_) => self.eval(op, frame, focus).map(Rc::new),
        }
    }

    /// The comma operator [XQuery §3.3.1]: each operand's items, in
    /// order.
    fn comma(&self, ops: &[Op], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut out = Vec::new();
        for op in ops {
            let items = self.eval(op, frame, focus)?;
            sequence::extend(&mut out, items)?;
        }
        Ok(out)
    }

    /// `and` or `or` [XQuery §3.6]: the effective boolean value of each
    /// operand in turn, until one is `or`'s value, `true` of `or`.
    fn logical(&self, operands: &[Op], or: bool, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        for operand in operands {
            if ebv(&self.eval(operand, frame, focus)?)? == or {
                return Ok(sequence::boolean(or));
            }
        }
        Ok(sequence::boolean(!or))
    }

    /// `if` [XQuery §3.10]: the then or else expression by the effective
    /// boolean value of the test, the other not evaluated.
    fn conditional(&self, parts: &[Op; 3], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let [test, then, otherwise] = parts;
        if ebv(&self.eval(test, frame, focus)?)? { self.eval(then, frame, focus) } else { self.eval(otherwise, frame, focus) }
    }
}

/// The error of a context item expression with no context item [XQuery
/// §3.1.4].
fn no_context_item() -> crate::error::SqlError {
    error(ErrorCode::XPDY0002, "the context item is undefined [XQuery §3.1.4]")
}

#[cfg(test)]
mod tests;
