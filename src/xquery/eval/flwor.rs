//! The expressions that bind variables, evaluated [XQuery §3.8, §3.11,
//! §3.12.2]: the tuples of a FLWOR expression's clauses, its where
//! clause, its order by clause, stable, and its return clause; the
//! tuples of a quantified expression; a typeswitch's effective case.
//!
//! \[DESIGN\] RULES.md R-CF-6: the clauses, and a quantified expression's
//! variables, are bound one level deeper each, within `MAX_DEPTH`.

use std::cmp::Ordering;
use std::rc::Rc;

use super::super::error::{ErrorCode, error};
use super::super::plan::{Clause, FlworOp, Op, OrderKey, QuantifiedOp, TypeswitchOp};
use super::super::types::SeqType;
use super::sequence::{atomize, bounded, extend};
use super::{Evaluator, Focus, Frame, Seq, ebv};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Primitive};

/// [Schema2 §3.2.5.1] The literal of not-a-number.
const NAN: &str = "NaN";

/// An order key's value [XQuery §3.8.3]: none, NaN, or a value.
#[derive(Debug, Clone)]
enum Key {
    /// The empty sequence.
    Empty,
    /// A NaN.
    NaN,
    /// Another value.
    Value(Atomic),
}

/// A tuple's result and its order keys.
type Ordered = (Vec<Key>, Seq);

impl Evaluator<'_> {
    /// A FLWOR expression [XQuery §3.8]: the return clause of each tuple
    /// the where clause keeps, in the order of its order by clause, else
    /// of the tuples.
    pub(super) fn flwor(&self, f: &FlworOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut results: Vec<Ordered> = Vec::new();
        let mut held = 0_usize;
        self.tuples((f, 0), frame, focus, &mut |this, inner| {
            this.check()?;
            if let Some(condition) = &f.condition
                && !ebv(&this.eval(condition, inner, focus)?)?
            {
                return Ok(());
            }
            let keys = f.order.iter().map(|k| this.key(k, inner, focus)).collect::<Result<Vec<_>>>()?;
            let result = this.eval(&f.result, inner, focus)?;
            held = held.saturating_add(result.len());
            bounded(held)?;
            results.push((keys, result));
            Ok(())
        })?;
        if !f.order.is_empty() {
            self.comparable(&f.order, &results)?;
            results.sort_by(|a, b| self.tuple_order(&f.order, &a.0, &b.0));
        }
        let mut out = Vec::new();
        for (_, result) in results {
            extend(&mut out, result)?;
        }
        Ok(out)
    }

    /// Each tuple of the clauses of `f` from the `at`th, given to `each`
    /// with its variables bound in `frame` [XQuery §3.8.1].
    fn tuples(
        &self,
        (f, at): (&FlworOp, usize),
        frame: &mut Frame,
        focus: &Focus,
        each: &mut dyn FnMut(&Self, &mut Frame) -> Result<()>,
    ) -> Result<()> {
        let Some(clause) = f.clauses.get(at) else { return each(self, frame) };
        let _deeper = self.deeper()?;
        let next = at.saturating_add(1);
        match clause {
            Clause::Let(slot, declared, value) => {
                let value = self.shared(value, frame, focus)?;
                matching(declared.as_ref(), &value)?;
                frame.set(*slot, value);
                self.tuples((f, next), frame, focus, each)
            }
            Clause::For(slot, position, declared, input) => {
                let items = self.shared(input, frame, focus)?;
                for (n, item) in items.iter().enumerate() {
                    let bound = Rc::new(vec![item.clone()]);
                    matching(declared.as_ref(), &bound)?;
                    frame.set(*slot, bound);
                    if let Some(p) = position {
                        frame.set(*p, Rc::new(vec![integer(n.saturating_add(1))]));
                    }
                    self.tuples((f, next), frame, focus, each)?;
                }
                Ok(())
            }
        }
    }

    /// The value of an order spec of a tuple [XQuery §3.8.3]: atomized,
    /// one value or none \[err:XPTY0004\], an untyped one a string.
    fn key(&self, spec: &OrderKey, frame: &mut Frame, focus: &Focus) -> Result<Key> {
        let mut atomized = atomize(&self.eval(&spec.key, frame, focus)?)?;
        if atomized.len() > 1 {
            return Err(error(ErrorCode::XPTY0004, "an order spec's value is more than one atomic value [XQuery §3.8.3]"));
        }
        Ok(match atomized.pop() {
            None => Key::Empty,
            Some(a) => {
                let nan = matches!(a.kind.primitive(), Primitive::Float | Primitive::Double) && *a.lexical == *NAN;
                let a = if a.kind == AtomicType::UntypedAtomic { Atomic { kind: AtomicType::String, ..a } } else { a };
                if nan { Key::NaN } else { Key::Value(a) }
            }
        })
    }

    /// The error unless the values of each order spec are of types that
    /// compare [XQuery §3.8.3] \[err:XPTY0004\].
    fn comparable(&self, specs: &[OrderKey], results: &[Ordered]) -> Result<()> {
        for (n, spec) in specs.iter().enumerate() {
            let values: Vec<&Atomic> =
                results.iter().filter_map(|(keys, _)| if let Some(Key::Value(a)) = keys.get(n) { Some(a) } else { None }).collect();
            if let Some(first) = values.first() {
                for v in &values {
                    self.order(first, v, &spec.collation.0, false)?;
                }
            }
        }
        Ok(())
    }

    /// How the tuples of keys `a` and `b` order [XQuery §3.8.3]: by the
    /// first spec whose values are unequal, the empty sequence and NaN
    /// least or greatest as the spec says, descending reversing it.
    fn tuple_order(&self, specs: &[OrderKey], a: &[Key], b: &[Key]) -> Ordering {
        for (spec, (x, y)) in specs.iter().zip(a.iter().zip(b)) {
            let rank = |k: &Key| match (k, spec.empty_greatest) {
                (Key::Empty, false) | (Key::Value(_), true) => 0_u8,
                (Key::NaN, _) => 1,
                (Key::Value(_), false) | (Key::Empty, true) => 2,
            };
            let order = match (x, y) {
                (Key::Value(v), Key::Value(w)) => self.order(v, w, &spec.collation.0, false).ok().flatten().unwrap_or(Ordering::Equal),
                (Key::Empty | Key::NaN | Key::Value(_), Key::Empty | Key::NaN | Key::Value(_)) => rank(x).cmp(&rank(y)),
            };
            let order = if spec.descending { order.reverse() } else { order };
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    }

    /// A quantified expression [XQuery §3.11]: `some` true if the test is
    /// true of a tuple, `every` false if it is false of one.
    pub(super) fn quantified(&self, q: &QuantifiedOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let found = self.satisfied(q, 0, frame, focus)?;
        Ok(super::sequence::boolean(found != q.every))
    }

    /// Whether a tuple of the variables of `q` from the `at`th decides it:
    /// makes its test true of `some`, false of `every`.
    fn satisfied(&self, q: &QuantifiedOp, at: usize, frame: &mut Frame, focus: &Focus) -> Result<bool> {
        let Some((slot, declared, input)) = q.bindings.get(at) else {
            return Ok(ebv(&self.eval(&q.test, frame, focus)?)? != q.every);
        };
        let _deeper = self.deeper()?;
        let items = self.shared(input, frame, focus)?;
        for item in items.iter() {
            self.check()?;
            let bound = Rc::new(vec![item.clone()]);
            matching(declared.as_ref(), &bound)?;
            frame.set(*slot, bound);
            if self.satisfied(q, at.saturating_add(1), frame, focus)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A typeswitch [XQuery §3.12.2]: the result of the first case whose
    /// type the operand's value matches, else the default's, its variable
    /// bound to the value.
    pub(super) fn typeswitch(&self, t: &TypeswitchOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let value = Rc::new(self.eval(&t.operand, frame, focus)?);
        let (slot, result): (&Option<usize>, &Op) =
            t.cases.iter().find(|(_, matches, _)| matches.matches(&value)).map_or((&t.default.0, &t.default.1), |(s, _, r)| (s, r));
        if let Some(slot) = slot {
            frame.set(*slot, value);
        }
        self.eval(result, frame, focus)
    }
}

/// The error unless `value` matches `declared`, if a variable declares a
/// type [XQuery §3.8.1, §3.11] \[err:XPTY0004\].
fn matching(declared: Option<&SeqType>, value: &[Item]) -> Result<()> {
    if declared.is_none_or(|t| t.matches(value)) {
        return Ok(());
    }
    Err(error(ErrorCode::XPTY0004, "the value bound to a variable does not match its declared type [XQuery §3.8.1]"))
}

/// The `xs:integer` `n`.
fn integer(n: usize) -> Item {
    Item::Atomic(Atomic::new(AtomicType::Integer, n.to_string()))
}
