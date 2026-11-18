//! \[EXTENSION\] RULES.md D86: a lookup's own answer of one value: whether
//! a node its path selects has a value its comparisons hold of [XQuery
//! §3.2, §3.5.2]. What a caller of a lookup checks each row it fetches by,
//! as for a function lookup (RULES.md D34), and what the tests of an
//! index compare its candidates with.
//!
//! The answer is read in one pass over the value's stored form
//! (`xml::Events`), no tree built. Each element open keeps which of the
//! path's steps match ending at it and which matched at it or above, as
//! bits, each child's made of its parent's and of its name; so a value's
//! nodes cost the path's steps each, and what is held is a stack of open
//! elements, bounded by `xml::MAX_NESTING`, and the text of the elements
//! selected while one is open: an element's string value is the text of
//! the stream from its start to its end [XQueryDM §6.2.3], so the elements
//! open share one buffer, each its own part of it.
//!
//! The nodes are taken in document order, an element before its
//! attributes: the first whose comparisons hold, or raise, answers. An
//! element's comparisons are known as it ends, after the nodes below it,
//! so an answer of a node is kept until no element selected before it is
//! open.

use std::borrow::Cow;

use super::lookup::{Axis, Comparison, Literal, MAX_STEPS, Step, Target, XmlLookup};
use crate::error::{Code, Result, SqlError};
use crate::value::CompOp;
use crate::value::xml::XmlValue;
use crate::xml::{Atomic, AtomicType, Event, Events, MAX_NESTING, MAX_STORED, QName, cast};

/// Bits of steps: bit k for step k, from 1; bit 0 for none, at the item.
type Steps = u128;

/// The bits of the steps of `steps` whose test `test` gives true.
fn step_bits<T>(steps: &[T], test: impl Fn(&T) -> bool) -> Steps {
    steps.iter().zip(1_u32..).filter(|(s, _)| test(s)).fold(0, |bits, (_, k)| bits | bit(k))
}

/// Bit `k`.
fn bit(k: u32) -> Steps {
    1_u128.checked_shl(k).unwrap_or(0)
}

/// What the path's steps match, compiled: each step's axis, and which
/// steps a name passes, by the item's names.
pub(super) struct Compiled<'l> {
    steps: &'l [Step],
    /// The steps of the child axis, and of the descendant.
    child: Steps,
    descendant: Steps,
    /// The bit of the last step.
    last: Steps,
}

impl<'l> Compiled<'l> {
    /// `steps` compiled.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of no step; -900 of more than `MAX_STEPS`.
    pub(super) fn of(steps: &'l [Step]) -> Result<Compiled<'l>> {
        if steps.is_empty() {
            return Err(SqlError::new(Code::Api, "[DESIGN] an XML lookup is given a path of no step (RULES.md D86)"));
        }
        if steps.len() > MAX_STEPS {
            return Err(SqlError::new(
                Code::Unsupported,
                format!("[DESIGN] an XML lookup takes at most {MAX_STEPS} steps (RULES.md D18, D86)"),
            ));
        }
        let last = bit(u32::try_from(steps.len()).unwrap_or(0));
        Ok(Compiled {
            steps,
            child: step_bits(steps, |s| s.axis == Axis::Child),
            descendant: step_bits(steps, |s| s.axis == Axis::Descendant),
            last,
        })
    }

    /// The steps each name of `names` passes.
    pub(super) fn names(&self, names: &[QName]) -> Vec<Steps> {
        names.iter().map(|name| step_bits(self.steps, |s| s.name.as_ref().is_none_or(|n| n.same(name)))).collect()
    }

    /// The steps that match ending at a node of a name passing `passed`,
    /// below a node at which `ended` end and below or at which `reached`
    /// were: a child step's after its step before ended at the node above,
    /// a descendant step's after it ended at or above it.
    pub(super) fn child(&self, passed: Steps, (ended, reached): (Steps, Steps)) -> Steps {
        let after = |s: Steps| s.checked_shl(1).unwrap_or(0);
        passed & ((after(ended) & self.child) | (after(reached) & self.descendant))
    }

    /// Whether `ended` holds the last step.
    pub(super) fn selects(&self, ended: Steps) -> bool {
        ended & self.last != 0
    }
}

/// An element open: the steps ending at it and matched at or above it,
/// and, if it is selected, its place in document order and where its
/// text begins.
struct Open {
    ended: Steps,
    reached: Steps,
    selected: Option<(u64, usize)>,
}

/// The answer of a value, being read.
struct Reading<'l> {
    lookup: &'l XmlLookup,
    path: Compiled<'l>,
    /// The steps each name of the item read passes.
    names: Vec<Steps>,
    open: Vec<Open>,
    /// The places of the elements selected and open, outermost first.
    pending: Vec<u64>,
    /// The text since the outermost element selected and open began.
    text: String,
    /// The place of the node met last.
    at: u64,
    /// The first answer, by place: true, or the error raised.
    first: Option<(u64, Option<SqlError>)>,
}

impl XmlLookup {
    /// Whether `value` has a node the lookup's path selects of which every
    /// comparison holds; nodes are taken in document order, an element
    /// before its attributes, and the first that holds answers.
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, err:FORG0001, if a numeric comparison meets a node,
    /// before any that holds, whose value does not cast to xs:double
    /// [XQuery §3.5.2]; SQLCODE -910 of a path of no step, -900 of one of
    /// more than `MAX_STEPS`; the errors of reading the value.
    pub fn matches(&self, value: &XmlValue) -> Result<bool> {
        let lob = value.lob()?;
        let octets = if let Some(octets) = lob.inline() {
            Cow::Borrowed(octets)
        } else {
            let most = u64::try_from(MAX_STORED).unwrap_or(u64::MAX);
            let read = lob.bounded(most)?;
            Cow::Owned(read.ok_or_else(|| {
                SqlError::new(Code::Corrupt, format!("[DESIGN] an XML value stored is longer than {MAX_STORED} octets (RULES.md D86)"))
            })?)
        };
        self.matches_stored(&octets)
    }

    /// `matches` of the value whose stored form is `octets`.
    ///
    /// # Errors
    ///
    /// As `matches`; SQLCODE -916 if the octets are no stored form.
    pub fn matches_stored(&self, octets: &[u8]) -> Result<bool> {
        let mut reading = Reading {
            lookup: self,
            path: Compiled::of(&self.pattern.steps)?,
            names: Vec::new(),
            open: Vec::new(),
            pending: Vec::new(),
            text: String::new(),
            at: 0,
            first: None,
        };
        let mut events = Events::new(octets)?;
        while let Some(event) = events.next_event()? {
            if event == Event::Node {
                reading.names = reading.path.names(events.names());
            }
            reading.event(event)?;
            if let Some(answer) = reading.answered() {
                return answer;
            }
        }
        Ok(false)
    }
}

impl Reading<'_> {
    /// Takes `event`.
    fn event(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Node | Event::Atomic | Event::Other | Event::Attribute { .. } => {}
            Event::Document => self.push(None)?,
            Event::Element { name, attributes } => {
                self.push(Some(name))?;
                for (attribute, value) in attributes {
                    self.attribute(attribute, value);
                }
            }
            Event::Text(text) => {
                if !self.pending.is_empty() {
                    self.text.push_str(text);
                }
            }
            Event::End => self.end(),
        }
        Ok(())
    }

    /// The steps the name numbered `name` passes.
    fn passes(&self, name: u32) -> Steps {
        usize::try_from(name).ok().and_then(|n| self.names.get(n)).copied().unwrap_or(0)
    }

    /// Opens a document node, or an element named `name`: its item's root,
    /// at which no step but none ends, if nothing is open.
    fn push(&mut self, name: Option<u32>) -> Result<()> {
        if self.open.len() > MAX_NESTING {
            return Err(SqlError::new(
                Code::Corrupt,
                format!("[DESIGN] a stored XML value nests deeper than {MAX_NESTING} (RULES.md D86)"),
            ));
        }
        let Some(parent) = self.open.last() else {
            self.open.push(Open { ended: bit(0), reached: bit(0), selected: None });
            return Ok(());
        };
        self.at = self.at.saturating_add(1);
        let ended = name.map_or(0, |n| self.path.child(self.passes(n), (parent.ended, parent.reached)));
        let reached = parent.reached | ended;
        let selected = (self.lookup.pattern.target == Target::Element && self.path.selects(ended)).then_some((self.at, self.text.len()));
        if let Some((at, _)) = selected {
            self.pending.push(at);
        }
        self.open.push(Open { ended, reached, selected });
        Ok(())
    }

    /// An attribute of the element open last, named `name`, of `value`.
    fn attribute(&mut self, name: u32, value: &str) {
        self.at = self.at.saturating_add(1);
        if self.lookup.pattern.target != Target::Attribute {
            return;
        }
        let Some(element) = self.open.last() else { return };
        if self.path.selects(self.path.child(self.passes(name), (element.ended, element.reached))) {
            let answer = self.lookup.compared(value);
            self.answer(self.at, answer);
        }
    }

    /// Closes the node open last, answering of it if it is selected.
    fn end(&mut self) {
        let Some(closed) = self.open.pop() else { return };
        let Some((at, start)) = closed.selected else { return };
        self.pending.pop();
        let answer = self.lookup.compared(self.text.get(start..).unwrap_or_default());
        self.answer(at, answer);
        if self.pending.is_empty() {
            self.text.clear();
        }
    }

    /// The answer `answer` of the node at place `at`: kept if it holds or
    /// raises, and is the first so far.
    fn answer(&mut self, at: u64, answer: Result<bool>) {
        let kept = match answer {
            Ok(false) => return,
            Ok(true) => None,
            Err(e) => Some(e),
        };
        if self.first.as_ref().is_none_or(|(first, _)| at < *first) {
            self.first = Some((at, kept));
        }
    }

    /// The answer, once no node before the first that answers may still
    /// answer: none is an element selected and open.
    fn answered(&mut self) -> Option<Result<bool>> {
        let (at, _) = self.first.as_ref()?;
        if self.pending.first().is_some_and(|open| open < at) {
            return None;
        }
        self.first.take().map(|(_, raised)| raised.map_or(Ok(true), Err))
    }
}

impl XmlLookup {
    /// Whether every comparison holds of a node's value `text`, in order.
    ///
    /// # Errors
    ///
    /// err:FORG0001 if a numeric one meets a value that is no number.
    pub(super) fn compared(&self, text: &str) -> Result<bool> {
        for comparison in &self.comparisons {
            if !holds(comparison, text)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Whether `comparison` holds of a node's `xdt:untypedAtomic` value
/// `text` [XQuery §3.5.2]: as a string, by code points, with a string or
/// `xdt:untypedAtomic` literal; as an xs:double with a number.
///
/// # Errors
///
/// err:FORG0001 if a number is compared and `text` does not cast to one.
fn holds(comparison: &Comparison, text: &str) -> Result<bool> {
    let order = match &comparison.literal {
        Literal::String(s) | Literal::Untyped(s) => Some(text.cmp(s.as_str())),
        Literal::Number(n) => {
            let cast = cast(&Atomic { kind: AtomicType::UntypedAtomic, lexical: text.into() }, AtomicType::Double)?;
            let number = match &*cast.lexical {
                "INF" => f64::INFINITY,
                "-INF" => f64::NEG_INFINITY,
                lexical => lexical.parse::<f64>().unwrap_or(f64::NAN),
            };
            number.partial_cmp(n)
        }
    };
    Ok(order.is_some_and(|o| match comparison.op {
        CompOp::Eq => o.is_eq(),
        CompOp::Ne => o.is_ne(),
        CompOp::Lt => o.is_lt(),
        CompOp::Le => o.is_le(),
        CompOp::Gt => o.is_gt(),
        CompOp::Ge => o.is_ge(),
    }))
}

// Unit tests: the answer of each value of many made, and of many lookups,
// that of the value decoded into a tree and walked (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;
