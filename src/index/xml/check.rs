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

use super::lookup::{Axis, Comparison, Literal, MAX_STEPS, Step, XmlFilter, XmlLookup, XmlQuery};
use crate::error::{Code, Result, SqlError};
use crate::value::CompOp;
use crate::value::xml::XmlValue;
use reading::Reading;

use crate::xml::{Atomic, AtomicType, Event, Events, MAX_STORED, QName, cast};

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
        read(self, None, &octets_of(value)?)
    }

    /// `matches` of the value whose stored form is `octets`.
    ///
    /// # Errors
    ///
    /// As `matches`; SQLCODE -916 if the octets are no stored form.
    pub fn matches_stored(&self, octets: &[u8]) -> Result<bool> {
        read(self, None, octets)
    }
}

impl XmlQuery {
    /// Whether `value` has a node the lookup's path selects of which every
    /// comparison holds and whose anchor the filter holds of, as
    /// `XmlLookup::matches` answers: the filter holds of an anchor if a
    /// node it selects holds, else raises if one raised.
    ///
    /// # Errors
    ///
    /// As `XmlLookup::matches`; SQLCODE -910 of a filter not of an
    /// ancestor the lookup's last steps, child steps, reach.
    pub fn matches(&self, value: &XmlValue) -> Result<bool> {
        read(&self.lookup, self.filter.as_ref(), &octets_of(value)?)
    }

    /// `matches` of the value whose stored form is `octets`.
    ///
    /// # Errors
    ///
    /// As `matches`; SQLCODE -916 if the octets are no stored form.
    pub fn matches_stored(&self, octets: &[u8]) -> Result<bool> {
        read(&self.lookup, self.filter.as_ref(), octets)
    }
}

/// The octets of `value`'s stored form, read whole if it is a large object.
fn octets_of(value: &XmlValue) -> Result<Vec<u8>> {
    let lob = value.lob()?;
    if let Some(octets) = lob.inline() {
        return Ok(octets.to_vec());
    }
    let most = u64::try_from(MAX_STORED).unwrap_or(u64::MAX);
    lob.bounded(most)?.ok_or_else(|| {
        SqlError::new(Code::Corrupt, format!("[DESIGN] an XML value stored is longer than {MAX_STORED} octets (RULES.md D86)"))
    })
}

/// The answer of `lookup`, filtered by `filter` if given, of the value
/// whose stored form is `octets`.
fn read(lookup: &XmlLookup, filter: Option<&XmlFilter>, octets: &[u8]) -> Result<bool> {
    let mut reading = Reading::of(lookup, filter)?;
    let mut events = Events::new(octets)?;
    while let Some(event) = events.next_event()? {
        if event == Event::Node {
            reading.named(events.names());
        }
        reading.event(event)?;
        if let Some(answer) = reading.answered() {
            return answer;
        }
    }
    Ok(false)
}

impl XmlLookup {
    /// Whether every comparison holds of a node's value `text`, in order.
    ///
    /// # Errors
    ///
    /// err:FORG0001 if a numeric one meets a value that is no number.
    pub(super) fn compared(&self, text: &str) -> Result<bool> {
        compared_by(&self.comparisons, text)
    }
}

/// Whether every comparison of `comparisons` holds of a node's value
/// `text`, in order.
///
/// # Errors
///
/// err:FORG0001 if a numeric one meets a value that is no number.
pub(super) fn compared_by(comparisons: &[Comparison], text: &str) -> Result<bool> {
    for comparison in comparisons {
        if !holds(comparison, text)? {
            return Ok(false);
        }
    }
    Ok(true)
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
            let cast = cast(&Atomic::new(AtomicType::UntypedAtomic, text), AtomicType::Double)?;
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

mod reading;

// Unit tests: the answer of each value of many made, and of many lookups,
// that of the value decoded into a tree and walked (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
pub(super) mod tests;
