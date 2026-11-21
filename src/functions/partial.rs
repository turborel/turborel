//! \[DESIGN\] A set function computed by two groupings (RULES.md D341):
//! its part of each group of the first, as bytes (`Partial`), and the
//! parts of a group of the second taken in (`Merged`). The optimizer's
//! rule for DISTINCT set functions (`optimizer/distinct_aggregates.rs`)
//! groups by their arguments first, and a set function without DISTINCT
//! of the same query is computed so.
//!
//! A part is the function's state (`Accumulator::write_state`), which
//! the second grouping merges, where the function writes one. Where it
//! writes none, as a sum of approximate numbers whose total depends on
//! the order it is added in, its part is its result, made of every row of
//! its group in the first grouping, which the rule sees to by giving it
//! the rows of one group alone.

mod scaled;

pub(crate) use scaled::{Paired, SCALED, Scaled};

use super::{Accumulator, AggregateFunction, AggregateRef, Columnar};
use crate::bytes::Reader;
use crate::error::{Code, Result, fail};
use crate::storage::{Floats, Ints, Texts};
use crate::value::codec::{put_value, value};
use crate::value::types::composite;
use crate::value::{Binary, DataType, MAX_BINARY_LENGTH, Value};

/// The name a part shows as, before the function's own.
pub(crate) const PARTIAL: &str = "PARTIAL";

/// The name of the function taking parts in.
pub(crate) const MERGED: &str = "MERGED";

/// The first byte of a part that is a state.
const STATE: u8 = 0;

/// The first byte of a part that is a result.
const FINISHED: u8 = 1;

/// The type of a part: a binary string of any length.
pub(crate) const PART_TYPE: DataType = DataType::VarBinary { length: MAX_BINARY_LENGTH };

/// How the set function `func` of result type `result` is computed in two
/// groupings: by its state, or by its result of one group alone; none if
/// its result cannot be held as a part, a value of a plug-in's type or one
/// a table may keep apart, which bytes would name by its id alone.
pub(crate) fn split_of(func: &AggregateRef, result: &DataType) -> Option<Split> {
    if func.0.accumulator(result.clone()).write_state(&mut Vec::new()) {
        return Some(Split::State);
    }
    let held = !matches!(result, DataType::Extension(_)) && !composite::may_keep_apart(result);
    held.then_some(Split::Whole)
}

/// How a set function is computed in two groupings (`split_of`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Split {
    /// Its states merged, of any groups.
    State,
    /// Its result, of one group alone.
    Whole,
}

/// The part of the set function `func`, of result type `result`, of a
/// group: of type `PART_TYPE`, never null.
pub(crate) struct Partial {
    /// The set function.
    pub(crate) func: AggregateRef,
    /// Its result type.
    pub(crate) result: DataType,
}

impl AggregateFunction for Partial {
    fn name(&self) -> &str {
        PARTIAL
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(PART_TYPE)
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(PartAcc(self.func.0.accumulator(self.result.clone())))
    }
    /// \[DESIGN\] (RULES.md D661) The function's kind, if it is a
    /// built-in a grouping keeps in columns, and its result type.
    fn parted(&self) -> Option<(Columnar, DataType)> {
        self.func.0.columnar().map(|c| (c, self.result.clone()))
    }
}

/// \[DESIGN\] (RULES.md D661) `acc`, a built-in's accumulator a grouping
/// made of a column of states, as the accumulator of its part.
pub(crate) fn parted(acc: Box<dyn Accumulator>) -> Box<dyn Accumulator> {
    Box::new(PartAcc(acc))
}

/// The accumulator of a part: the function's own, given what it is.
struct PartAcc(Box<dyn Accumulator>);

impl Accumulator for PartAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.0.update(v)
    }
    fn update_many(&mut self, values: &[Value], rows: Option<&[u32]>) -> Result<()> {
        self.0.update_many(values, rows)
    }
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[u32]>) -> Result<()> {
        self.0.update_ints(ints, rows)
    }
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[u32]>) -> Result<()> {
        self.0.update_floats(floats, rows)
    }
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[u32]>) -> Result<()> {
        self.0.update_texts(texts, rows)
    }
    fn update_rows(&mut self, rows: usize) -> Result<()> {
        self.0.update_rows(rows)
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        self.0.write_state(out)
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        self.0.merge(state)
    }
    /// Its state, or, if it writes none, its result, which judges it as
    /// the function judges its result once [ISO/IEC 9075:1992 §6.5 GR2b].
    fn finish(&self) -> Result<Value> {
        let mut out = vec![STATE];
        if !self.0.write_state(&mut out) {
            out = vec![FINISHED];
            put_value(&mut out, &self.0.finish()?);
        }
        Ok(Value::Binary(Binary::new(out)))
    }
}

/// The set function `func`, of result type `result`, of the parts
/// `Partial` gives of it: its states merged, in the order they come, or
/// the result one gives.
pub(crate) struct Merged {
    /// The set function.
    pub(crate) func: AggregateRef,
    /// Its result type.
    pub(crate) result: DataType,
}

impl AggregateFunction for Merged {
    fn name(&self) -> &str {
        MERGED
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(MergedAcc { acc: self.func.0.accumulator(self.result.clone()), result: None })
    }
}

/// The function's accumulator, given the states of parts, and the result
/// a part gave, if one did.
struct MergedAcc {
    acc: Box<dyn Accumulator>,
    result: Option<Value>,
}

impl Accumulator for MergedAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        match v {
            Value::Binary(b) => self.merge(b.bytes()),
            Value::Null
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Char(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => fail(Code::Corrupt, "[DESIGN] a part of a set function that is not one"),
        }
    }
    /// A result held, after its tag; else its state, after its own.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        if let Some(v) = &self.result {
            out.push(FINISHED);
            put_value(out, v);
            return true;
        }
        let mut state = vec![STATE];
        let written = self.acc.write_state(&mut state);
        if written {
            out.extend_from_slice(&state);
        }
        written
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        match state.split_first() {
            Some((&STATE, rest)) => self.acc.merge(rest),
            Some((&FINISHED, rest)) => {
                let mut reader = Reader::new(rest, "a result of a set function");
                self.result = Some(value(&mut reader)?);
                reader.finish()
            }
            Some(_) | None => fail(Code::Corrupt, "[DESIGN] a part of a set function of no kind it has"),
        }
    }
    fn finish(&self) -> Result<Value> {
        self.result.clone().map_or_else(|| self.acc.finish(), Ok)
    }
}

// Unit tests: parts made and taken in (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
