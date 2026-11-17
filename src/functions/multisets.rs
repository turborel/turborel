//! The functions of multisets [ISO/IEC 9075-2:2023 §6.26, §6.50 to §6.52,
//! §8.16 to §8.18, §10.9], their names no key words of SQL-92. Each is
//! bound by the binder as its syntax is read, not registered, and called
//! as any function is (RULES.md D68, D80). How their elements are grouped
//! and paired is `value/multisets.rs`'s.

use super::{Accumulator, AggregateFunction, ScalarFunction};
use crate::error::{Code, Result, SqlError, SqlState, fail, fail_with};
use crate::value::multisets::{BagOp, combined, is_set, submultiset};
use crate::value::types::composite::rows_hold;
use crate::value::types::multiset::{made, overflow};
use crate::value::{CompOp, DataType, Value, proxies};

/// [ISO/IEC 9075-2:2023 §6.26] `<multiset element reference>`'s name.
pub const ELEMENT: &str = "ELEMENT";
/// [ISO/IEC 9075-2:2023 §10.9] COLLECT's name.
pub const COLLECT: &str = "COLLECT";
/// [ISO/IEC 9075-2:2023 §10.9] FUSION's name.
pub const FUSION: &str = "FUSION";
/// [ISO/IEC 9075-2:2023 §10.9] INTERSECTION's name.
pub const INTERSECTION: &str = "INTERSECTION";
/// [ISO/IEC 9075-2:2023 §8.16] MEMBER, of the `<member predicate>`.
pub const MEMBER: &str = "MEMBER";
/// [ISO/IEC 9075-2:2023 §8.17] SUBMULTISET, of the `<submultiset
/// predicate>`.
pub const SUBMULTISET: &str = "SUBMULTISET";

/// The names the calls of the functions here show.
const MULTISET: &str = "MULTISET";
const SET: &str = "SET";
const SET_PREDICATE: &str = "IS A SET";

/// The elements of `v`, a multiset; none if it is null.
fn elements<'v>(v: Option<&'v Value>, of: &str) -> Result<Option<&'v [Value]>> {
    match v {
        Some(Value::Multiset(m)) => Ok(Some(m)),
        None | Some(Value::Null) => Ok(None),
        Some(_) => fail(Code::Api, format!("[DESIGN] {of} was given a value not a multiset")),
    }
}

/// [ISO/IEC 9075-2:2023 §6.52 GR2] `<multiset value constructor by
/// enumeration>`: its elements, each already cast to the element type.
pub struct MultisetOf(pub DataType);

impl ScalarFunction for MultisetOf {
    fn name(&self) -> &str {
        MULTISET
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        made(args.to_vec(), "§6.52 GR2")
    }
}

/// [ISO/IEC 9075-2:2023 §6.26 SR1] `<multiset element reference>`, as the
/// scalar subquery of UNNEST of it: null of a null or empty multiset, as
/// of no row [§7.11 GR1b]; its element of one; else cardinality violation
/// [§7.11 GR1a], 21000.
pub struct ElementOfMultiset(pub DataType);

impl ScalarFunction for ElementOfMultiset {
    fn name(&self) -> &str {
        ELEMENT
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        match elements(args.first(), ELEMENT)? {
            None | Some([]) => Ok(Value::Null),
            Some([one]) => Ok(one.clone()),
            Some(more) => fail_with(
                Code::Cardinality,
                SqlState::CARDINALITY_VIOLATION,
                format!("[ISO/IEC 9075-2:2023 §6.26 SR1, §7.11 GR1a] ELEMENT of a multiset of {} elements", more.len()),
            ),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §6.51 SR2] `<multiset set function>`: null of a
/// null multiset; else one element of each class of elements not
/// distinct from one another, as SELECT DISTINCT of UNNEST of it keeps.
pub struct SetOf(pub DataType);

impl ScalarFunction for SetOf {
    fn name(&self) -> &str {
        SET
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(m) = elements(args.first(), SET)? else { return Ok(Value::Null) };
        Ok(Value::Multiset(combined(BagOp::Union, true, m, &[])?.into()))
    }
}

/// [ISO/IEC 9075-2:2023 §6.50 SR2d, SR3d, SR4d] MULTISET UNION, EXCEPT or
/// INTERSECT, DISTINCT if `.1`, of operands already cast to the result
/// type `.2`: null if either is; else as `multisets::combined` makes it,
/// within `MAX_MULTISET_CARDINALITY` [§10.9 GR3b].
pub struct MultisetCombination(pub BagOp, pub bool, pub DataType);

impl MultisetCombination {
    /// How a call of it is written.
    fn spelled(&self) -> &'static str {
        match (self.0, self.1) {
            (BagOp::Union, false) => "MULTISET UNION ALL",
            (BagOp::Union, true) => "MULTISET UNION DISTINCT",
            (BagOp::Except, false) => "MULTISET EXCEPT ALL",
            (BagOp::Except, true) => "MULTISET EXCEPT DISTINCT",
            (BagOp::Intersect, false) => "MULTISET INTERSECT ALL",
            (BagOp::Intersect, true) => "MULTISET INTERSECT DISTINCT",
        }
    }
}

impl ScalarFunction for MultisetCombination {
    fn name(&self) -> &str {
        self.spelled()
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.2.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (Some(a), Some(b)) = (elements(args.first(), self.spelled())?, elements(args.get(1), self.spelled())?) else {
            return Ok(Value::Null);
        };
        made(combined(self.0, self.1, a, b)?, "§6.50 SR3d, §10.9 GR3b")
    }
}

/// [ISO/IEC 9075-2:2023 §8.2 GR1] Whether `x` = `e`, unknown if either is
/// or holds a null as that rule says; of large objects, read as they must
/// be (`value::proxies`).
fn equals(x: &Value, e: &Value) -> Result<Option<bool>> {
    Ok(match proxies(x, e)? {
        Some((p, q)) => rows_hold(&[p], CompOp::Eq, &[q]),
        None => rows_hold(std::slice::from_ref(x), CompOp::Eq, std::slice::from_ref(e)),
    })
}

/// The truth value `t` as a BOOLEAN value, null if unknown.
fn truth(t: Option<bool>) -> Value {
    t.map_or(Value::Null, Value::Bool)
}

/// [ISO/IEC 9075-2:2023 §8.16 GR3] "XV MEMBER OF MV": false of an empty
/// multiset \[GR3a\]; unknown if either is null \[GR3b\]; true if XV = an
/// element \[GR3c i\]; else unknown if an element is null \[GR3c ii\];
/// else false. An element compared unknown, as a row holding a null is,
/// is not equal and leaves it unknown, as `=` of it is [§8.2 GR1].
pub struct MemberOf;

impl ScalarFunction for MemberOf {
    fn name(&self) -> &str {
        MEMBER
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let x = args.first().unwrap_or(&Value::Null);
        let mv = elements(args.get(1), MEMBER)?;
        if mv.is_some_and(<[Value]>::is_empty) {
            return Ok(Value::Bool(false));
        }
        let Some(mv) = mv.filter(|_| !x.is_null()) else { return Ok(Value::Null) };
        let mut unknown = false;
        for e in mv {
            match equals(x, e)? {
                Some(true) => return Ok(Value::Bool(true)),
                Some(false) => {}
                None => unknown = true,
            }
        }
        Ok(truth(if unknown { None } else { Some(false) }))
    }
}

/// [ISO/IEC 9075-2:2023 §8.17 GR3] "CV SUBMULTISET OF MV": true of an
/// empty CV \[GR3a\]; unknown if either is null \[GR3b\]; false if CV has
/// more elements \[GR3c i\]; else as `multisets::submultiset` pairs them
/// \[GR3c ii to iv\].
pub struct SubmultisetOf;

impl ScalarFunction for SubmultisetOf {
    fn name(&self) -> &str {
        SUBMULTISET
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (cv, mv) = (elements(args.first(), SUBMULTISET)?, elements(args.get(1), SUBMULTISET)?);
        if cv.is_some_and(<[Value]>::is_empty) {
            return Ok(Value::Bool(true));
        }
        let (Some(cv), Some(mv)) = (cv, mv) else { return Ok(Value::Null) };
        if cv.len() > mv.len() {
            return Ok(Value::Bool(false));
        }
        Ok(truth(submultiset(cv, mv)))
    }
}

/// [ISO/IEC 9075-2:2023 §8.18 SR4] "CVE IS A SET", CARDINALITY ( CVE ) =
/// CARDINALITY ( SET ( CVE ) ): unknown if CVE is null; else whether no
/// two of its elements are not distinct from each other.
pub struct IsASet;

impl ScalarFunction for IsASet {
    fn name(&self) -> &str {
        SET_PREDICATE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(m) = elements(args.first(), SET_PREDICATE)? else { return Ok(Value::Null) };
        Ok(Value::Bool(is_set(m)?))
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 SR7k, SR7l, SR7m, SR7n] COLLECT, FUSION or
/// INTERSECTION. COLLECT ( X ) is FUSION ( MULTISET \[ X \] ), so it is
/// given the row ( X ), as ARRAY_AGG is, and keeps a null X as an element.
/// Of a `<multiset value constructor by query>` [§6.52 GR3], COLLECT of
/// its rows, no rows making an empty multiset.
pub struct MultisetAggregate {
    /// Which of them it is.
    pub kind: Gathering,
    /// Whether it is a `<multiset value constructor by query>`'s.
    pub by_query: bool,
}

/// [ISO/IEC 9075-2:2023 §10.9] The `<computational operation>`s of
/// multisets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gathering {
    /// COLLECT: the multiset of the values.
    Collect,
    /// FUSION: the multiset of the elements of the multisets.
    Fusion,
    /// INTERSECTION: the multiset of the elements all the multisets have.
    Intersection,
}

impl Gathering {
    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Gathering::Collect => COLLECT,
            Gathering::Fusion => FUSION,
            Gathering::Intersection => INTERSECTION,
        }
    }
}

impl AggregateFunction for MultisetAggregate {
    fn name(&self) -> &str {
        self.kind.name()
    }
    /// [ISO/IEC 9075-2:2023 §10.9 SR7k, SR7m, SR7n] DT MULTISET of COLLECT,
    /// of the row ( X ) it is given; DT of FUSION and INTERSECTION, a
    /// multiset type or a distinct type of one.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        match (self.kind, arg) {
            (Gathering::Collect, Some(DataType::Row(r))) => {
                r.fields.first().map(|f| DataType::multiset(f.data_type.clone())).ok_or_else(|| no_argument(self.kind))
            }
            (Gathering::Fusion | Gathering::Intersection, Some(t)) if t.multiset_type().is_some() => Ok(t),
            (Gathering::Fusion | Gathering::Intersection, Some(t)) => {
                fail(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §10.9 SR7m, SR7n] {} is of a multiset, not of {t}", self.kind.name()))
            }
            (Gathering::Collect, Some(_)) | (_, None) => Err(no_argument(self.kind)),
        }
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(Gathered { kind: self.kind, by_query: self.by_query, elements: None })
    }
}

fn no_argument(kind: Gathering) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {} was bound without its argument", kind.name()))
}

/// The elements gathered so far, none before the first value.
struct Gathered {
    kind: Gathering,
    by_query: bool,
    elements: Option<Vec<Value>>,
}

impl Gathered {
    /// [ISO/IEC 9075-2:2023 §10.9 GR3b] multiset value overflow past
    /// `MAX_MULTISET_CARDINALITY`, as soon as it is passed, so it holds no
    /// more.
    fn within(&self, total: usize) -> Result<()> {
        if total > crate::value::types::multiset::most() {
            let rule = if self.by_query { "§6.52 GR3b, §10.9 GR3b" } else { "§10.9 GR3b" };
            return overflow(rule, total);
        }
        Ok(())
    }
}

impl Accumulator for Gathered {
    fn update(&mut self, v: &Value) -> Result<()> {
        let held = self.elements.as_ref().map_or(0, Vec::len);
        match (self.kind, v) {
            (Gathering::Collect, Value::Row(row)) => {
                self.within(held.saturating_add(1))?;
                self.elements.get_or_insert_with(Vec::new).push(row.first().cloned().unwrap_or(Value::Null));
            }
            // [GR7d x] each element of each multiset.
            (Gathering::Fusion, Value::Multiset(m)) => {
                self.within(held.saturating_add(m.len()))?;
                self.elements.get_or_insert_with(Vec::new).extend(m.iter().cloned());
            }
            // [GR7d xi] as many of each value as the multiset of fewest.
            (Gathering::Intersection, Value::Multiset(m)) => {
                let kept = match self.elements.take() {
                    None => m.to_vec(),
                    Some(so_far) => combined(BagOp::Intersect, false, &so_far, m)?,
                };
                self.elements = Some(kept);
            }
            (Gathering::Collect | Gathering::Fusion | Gathering::Intersection, _) => return Err(no_argument(self.kind)),
        }
        Ok(())
    }
    /// [ISO/IEC 9075-2:2023 §10.9 GR7d ii] null of no values; of a
    /// `<multiset value constructor by query>`, the empty multiset
    /// [§6.52 GR3b].
    fn finish(&self) -> Result<Value> {
        Ok(match (&self.elements, self.by_query) {
            (Some(e), _) => Value::Multiset(e.as_slice().into()),
            (None, true) => Value::Multiset(Vec::new().into()),
            (None, false) => Value::Null,
        })
    }
}

// Unit tests: each multiset value overflow, at the limit and one past it,
// and the aggregates' null of no values (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
