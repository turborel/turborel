//! \[DESIGN\] (RULES.md D1290) A column of states of MAX or MIN of
//! character strings: each group's best string so far, all of them in one
//! text, with where each group's lies. A string of a batch is compared
//! with its group's best where both lie, by the order the function's
//! accumulator tells (`Accumulator::text_order`), and copied to the end
//! of the text only when it wins. The string it takes the place of stays
//! in the text, dead, until the dead octets pass what the column holds
//! beside them, the live octets and the places; the live strings are then
//! copied to a text of their own. So the column holds at most twice what
//! it needs, and what copying costs is paid by the octets that died.
//!
//! A group is written, merged and finished as its accumulator writes,
//! merges and finishes it, and a column fed anything but character
//! strings turns into accumulators, each given its group's best.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken, eliminated};
use crate::functions::{Accumulator, AggregateRef, TextOrder};
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::{Batch, Column, Texts};
use crate::value::codec::{put_value, value};
use crate::value::{DataType, Value};

use super::super::super::Pending;
use super::Typed;

/// Each group's best string: the order they are compared in; the text
/// holding them, and strings bettered since; where each group's lies in
/// it, and whether it has one; how many octets of the text no group's
/// string holds; and the set function and its result's type, of which a
/// group's accumulator is made.
pub(in crate::exec) struct Best {
    order: TextOrder,
    text: String,
    spans: Vec<(usize, usize)>,
    held: Vec<bool>,
    dead: usize,
    func: AggregateRef,
    result: DataType,
}

/// The character strings of `column`, if it is a vector of them.
fn texts_of(column: &Column) -> Option<&Texts> {
    match column {
        Column::Texts(texts) => Some(texts),
        Column::Late(late) => late.made().and_then(texts_of),
        Column::Values(_) | Column::Ints(_) | Column::Floats(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Nulls(_) => None,
    }
}

impl Typed {
    /// A column of each group's best string, of no group yet, if `agg`
    /// is MAX or MIN of character strings in an order of their text
    /// alone.
    pub(in crate::exec::table) fn best(agg: &AggCall) -> Option<Typed> {
        let order = agg.func.0.accumulator(agg.result.clone()).text_order()?;
        Some(Typed::Best(Best::new(order, agg)))
    }
}

impl Best {
    /// A column of no group, of the order `order`, of the set function
    /// and result of `agg`.
    fn new(order: TextOrder, agg: &AggCall) -> Best {
        let (text, spans, held) = (String::new(), Vec::new(), Vec::new());
        Best { order, text, spans, held, dead: 0, func: agg.func.clone(), result: agg.result.clone() }
    }

    /// A column of no group, of its order and set function.
    fn emptied(&self) -> Best {
        let (text, spans, held) = (String::new(), Vec::new(), Vec::new());
        Best { order: self.order, text, spans, held, dead: 0, func: self.func.clone(), result: self.result.clone() }
    }

    /// How many groups.
    pub(super) fn len(&self) -> usize {
        self.spans.len()
    }

    /// Adds `more` groups', of no string yet.
    pub(super) fn grow_by(&mut self, more: usize) {
        let len = self.spans.len().saturating_add(more);
        self.spans.resize(len, (0, 0));
        self.held.resize(len, false);
    }

    /// Room for `more` groups more.
    pub(super) fn reserve(&mut self, more: usize) {
        self.spans.reserve(more);
        self.held.reserve(more);
    }

    /// Group `g`'s best string, if it has one.
    fn get(&self, g: usize) -> Option<&str> {
        let (from, to) = self.spans.get(g).copied().filter(|_| self.held.get(g) == Some(&true))?;
        self.text.get(from..to)
    }

    /// Gives group `g` the string `candidate`, if it has none or
    /// `candidate` betters the one it has: copied to the text's end, the
    /// string it takes the place of left dead.
    fn take(&mut self, g: usize, candidate: &str) {
        if self.get(g).is_some_and(|best| !self.order.better(candidate, best)) {
            return;
        }
        let (Some(span), Some(held)) = (self.spans.get_mut(g), self.held.get_mut(g)) else { return };
        self.dead = self.dead.saturating_add(span.1.saturating_sub(span.0));
        let from = self.text.len();
        self.text.push_str(candidate);
        *span = (from, self.text.len());
        *held = true;
        if self.crowded() {
            self.compact();
        }
    }

    /// Whether the dead octets are more than what the column holds beside
    /// them, its live octets and its groups' places. Copying the live
    /// strings then reads no more than the octets that died since the
    /// last copy.
    fn crowded(&self) -> bool {
        let places = self.spans.len().saturating_mul(size_of::<(usize, usize)>());
        self.dead > self.text.len().saturating_sub(self.dead).saturating_add(places)
    }

    /// Copies the live strings to a text of their own, in the groups'
    /// order, so none is dead.
    fn compact(&mut self) {
        let old = std::mem::take(&mut self.text);
        self.text.reserve(old.len().saturating_sub(self.dead));
        for (span, _) in self.spans.iter_mut().zip(&self.held).filter(|(_, held)| **held) {
            let start = self.text.len();
            self.text.push_str(old.get(span.0..span.1).unwrap_or_default());
            *span = (start, self.text.len());
        }
        self.dead = 0;
    }

    /// Feeds each row of `pending` to its group, of `batch`, as `take`
    /// says, of `lane` if evaluated by vectors, each string compared where
    /// it lies, a null eliminated heard by `runner`: whether it could,
    /// false, fed nothing, of anything but character strings and nulls.
    pub(super) fn feed(&mut self, pending: &Pending, (batch, take, lane): (&Batch, &Taken, Option<&Fed<'_>>), runner: &Runner) -> bool {
        let (column, places) = match (take, lane) {
            (Taken::Column(c), _) => (batch.columns.get(*c), &pending.read),
            (Taken::Expr(_), Some(Fed::Made(column))) => (Some(column), &pending.given),
            (Taken::Expr(_), Some(Fed::Const(v))) => return self.feed_values(|_| *v, (&pending.given, &pending.groups), runner),
            (Taken::Expr(_), None) | (Taken::Count | Taken::Kept(_), _) => return false,
        };
        if let Some(texts) = column.and_then(texts_of) {
            let mut any = false;
            for (&at, &g) in places.iter().zip(&pending.groups) {
                match texts.get(index(at)) {
                    Some(candidate) => self.take(index(g), candidate),
                    None => any = true,
                }
            }
            eliminated(any, runner);
            return true;
        }
        match column {
            Some(Column::Values(values)) => {
                self.feed_values(|at| values.get(at).unwrap_or(&Value::Null), (places, &pending.groups), runner)
            }
            Some(Column::Nulls(_)) => {
                eliminated(!pending.groups.is_empty(), runner);
                true
            }
            Some(Column::Texts(_) | Column::Ints(_) | Column::Floats(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Late(_))
            | None => false,
        }
    }

    /// Feeds each group of `groups` the value `value` gives at the place
    /// of `places` beside it, a null eliminated heard by `runner`: whether
    /// it could, false, fed nothing, if any is neither a character string
    /// nor a null.
    fn feed_values<'v>(&mut self, value: impl Fn(usize) -> &'v Value, (places, groups): (&[u32], &[u32]), runner: &Runner) -> bool {
        let fits = |v: &Value| v.is_null() || matches!(v, Value::Char(_));
        if !places.iter().all(|&at| fits(value(index(at)))) {
            return false;
        }
        let mut any = false;
        for (&at, &g) in places.iter().zip(groups) {
            if let Value::Char(candidate) = value(index(at)) {
                self.take(index(g), candidate);
            } else {
                any = true;
            }
        }
        eliminated(any, runner);
        true
    }

    /// Merges group `i`'s of `other` into group `g`'s, if both are of one
    /// order: whether they were.
    pub(super) fn merge_from(&mut self, g: usize, other: &Best, i: usize) -> bool {
        if other.order == self.order
            && let Some(candidate) = other.get(i)
        {
            self.take(g, candidate);
        }
        other.order == self.order
    }

    /// \[DESIGN\] (RULES.md D1110) Merges each group at `places` of
    /// `other` into the group of `groups` beside it, if both are of one
    /// order: whether they were.
    pub(super) fn merge_all(&mut self, groups: &[u32], other: &Best, places: &[u32]) -> bool {
        if other.order != self.order {
            return false;
        }
        for (&g, &i) in groups.iter().zip(places) {
            if let Some(candidate) = other.get(index(i)) {
                self.take(index(g), candidate);
            }
        }
        true
    }

    /// Merges `state`, as the accumulator writes it, into group `g`'s, if
    /// its value is a character string or a null: whether it did.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is no state written.
    pub(super) fn merge_written(&mut self, g: usize, state: &[u8]) -> Result<bool> {
        let mut reader = Reader::new(state, "a state of MAX or MIN");
        let written = value(&mut reader)?;
        reader.finish()?;
        if let Value::Char(candidate) = &written {
            self.take(g, candidate);
        }
        Ok(written.is_null() || matches!(written, Value::Char(_)))
    }

    /// Group `g`'s accumulator, as it would be: the function's, given the
    /// group's best.
    pub(super) fn boxed_of(&self, g: usize) -> Box<dyn Accumulator> {
        let mut acc = self.func.0.accumulator(self.result.clone());
        if let Some(best) = self.get(g) {
            // MAX and MIN take a character string without fail.
            let _ = acc.update(&Value::Char(best.into()));
        }
        acc
    }

    /// Group `g`'s result, its best string, or a null of none.
    pub(super) fn finish(&self, g: usize) -> Value {
        self.get(g).map_or(Value::Null, |best| Value::Char(best.into()))
    }

    /// Group `g`'s state, written to `out` as its accumulator writes it.
    pub(super) fn write(&self, g: usize, out: &mut Vec<u8>) {
        put_value(out, &self.finish(g));
    }

    /// Keeps the first `len` groups', the strings of the rest left dead.
    pub(super) fn truncate(&mut self, len: usize) {
        let cut: usize = (len..self.spans.len()).filter_map(|g| self.get(g)).map(str::len).fold(0, usize::saturating_add);
        self.dead = self.dead.saturating_add(cut);
        self.spans.truncate(len);
        self.spans.shrink_to_fit();
        self.held.truncate(len);
        self.held.shrink_to_fit();
        if self.crowded() {
            self.compact();
        }
    }

    /// The bytes it holds.
    pub(super) fn bytes(&self) -> usize {
        let places = self.spans.capacity().saturating_mul(size_of::<(usize, usize)>());
        self.text.capacity().saturating_add(places).saturating_add(self.held.capacity())
    }

    /// \[DESIGN\] (RULES.md D1110) The states of the groups at `order`, in
    /// that order, their strings alone in the text.
    pub(super) fn gathered(&self, order: &[u32]) -> Best {
        let mut made = self.emptied();
        made.grow_by(order.len());
        for (g, &i) in order.iter().enumerate() {
            if let Some(best) = self.get(index(i)) {
                made.take(g, best);
            }
        }
        made
    }

    /// \[DESIGN\] (RULES.md D1226) Writes the states of the groups at
    /// `places` to `out`, in that order: of each, whether it has a
    /// string, and the string.
    pub(super) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) {
        for &i in places {
            let best = self.get(index(i));
            out.put_u8(u8::from(best.is_some()));
            out.put_str(best.unwrap_or_default());
        }
    }

    /// The states of `len` groups read back as `put_groups` wrote them,
    /// into this column of no group.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not there, or no text.
    pub(super) fn read(mut self, reader: &mut Reader<'_>, len: usize) -> Result<Best> {
        self.grow_by(len);
        for g in 0..len {
            let held = reader.u8()? != 0;
            let best = reader.str()?;
            if held {
                self.take(g, &best);
            }
        }
        Ok(self)
    }
}

// Unit tests: the column against an accumulator a group (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
