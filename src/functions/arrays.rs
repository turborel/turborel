//! The functions of arrays [ISO/IEC 9075-2:2023 §6.25, §6.31, §6.47,
//! §6.48, §6.49, §10.9, §15.5], their names no key words of SQL-92. Each
//! is bound by the binder as its syntax is read, not registered, and
//! called as any function is (RULES.md D68).

use std::cmp::Ordering;
use std::sync::Arc;

use super::strings::whole;
use super::{Accumulator, AggregateFunction, ScalarFunction};
use crate::error::{Code, Result, SqlError, SqlState, fail, fail_with};
use crate::value::collation::CollationRef;
use crate::value::types::MAX_ARRAY_CARDINALITY;
use crate::value::types::composite::array_ordered;
use crate::value::{DataType, Decimal, Value, sort_cmp};

/// [ISO/IEC 9075-2:2023 §6.31] `<cardinality expression>`'s name.
pub const CARDINALITY: &str = "CARDINALITY";
/// [ISO/IEC 9075-2:2023 §6.31] `<max cardinality expression>`'s name.
pub const ARRAY_MAX_CARDINALITY: &str = "ARRAY_MAX_CARDINALITY";
/// [ISO/IEC 9075-2:2023 §6.48] `<trim array function>`'s name.
pub const TRIM_ARRAY: &str = "TRIM_ARRAY";
/// [ISO/IEC 9075-2:2023 §10.9] `<array aggregate function>`'s name.
pub const ARRAY_AGG: &str = "ARRAY_AGG";

/// The names the calls of the functions here show.
const ARRAY: &str = "ARRAY";
const ELEMENT: &str = "ARRAY ELEMENT";
const ASSIGN_ELEMENT: &str = "ARRAY ELEMENT ASSIGNMENT";
const ARRAY_CONCATENATION: &str = "ARRAY CONCATENATION";

/// IL008 as a count.
fn most() -> usize {
    usize::try_from(MAX_ARRAY_CARDINALITY).unwrap_or(usize::MAX)
}

/// The elements of `v`, an array; none if it is null.
fn elements<'v>(v: Option<&'v Value>, of: &str) -> Result<Option<&'v Arc<[Value]>>> {
    match v {
        Some(Value::Array(a)) => Ok(Some(a)),
        None | Some(Value::Null) => Ok(None),
        Some(_) => fail(Code::Api, format!("[DESIGN] {of} was given a value not an array")),
    }
}

/// [ISO/IEC 9075-2:2023 §6.25 GR2b, §6.48 GR2c, GR2g] data exception,
/// array element error.
fn element_error<T>(rule: &str, what: &str) -> Result<T> {
    fail_with(Code::Data, SqlState::ARRAY_ELEMENT_ERROR, format!("[ISO/IEC 9075-2:2023 {rule}] {what}"))
}

/// [ISO/IEC 9075-2:2023 §6.49 GR2] `<array value constructor by
/// enumeration>`: its elements, each already cast to the element type.
pub struct ArrayOf(pub DataType);

impl ScalarFunction for ArrayOf {
    fn name(&self) -> &str {
        ARRAY
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(Value::Array(args.into()))
    }
}

/// [ISO/IEC 9075-2:2023 §6.25] `<array element reference>`: null if the
/// array or the position is \[GR1\]; the element at a position from 1 to
/// the cardinality \[GR2a\]; else array element error \[GR2b\].
pub struct ElementOf(pub DataType);

impl ScalarFunction for ElementOf {
    fn name(&self) -> &str {
        ELEMENT
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (Some(array), Some(i)) = (elements(args.first(), ELEMENT)?, whole(args.get(1))?) else { return Ok(Value::Null) };
        let at = usize::try_from(i).ok().and_then(|i| i.checked_sub(1));
        match at.and_then(|at| array.get(at)) {
            Some(e) => Ok(e.clone()),
            None => element_error("§6.25 GR2b", &format!("no element {i} is in an array of {} elements", array.len())),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §6.31 GR1b, GR7] `<cardinality expression>`: the
/// number of elements, null of a null array.
pub struct CardinalityOf(pub DataType);

impl ScalarFunction for CardinalityOf {
    fn name(&self) -> &str {
        CARDINALITY
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        Ok(match elements(args.first(), CARDINALITY)? {
            Some(a) => Value::Exact(Decimal::new(i128::try_from(a.len()).unwrap_or(i128::MAX), 0)),
            None => Value::Null,
        })
    }
}

/// [ISO/IEC 9075-2:2023 §6.48 GR2] `<trim array function>`: the array
/// without its last NV elements, null if either is null, an array element
/// error if NV is negative or more than the cardinality.
pub struct TrimArrayOf(pub DataType);

impl ScalarFunction for TrimArrayOf {
    fn name(&self) -> &str {
        TRIM_ARRAY
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        // [GR2b, GR2c] the number first, as its rules come first.
        let Some(n) = whole(args.get(1))? else { return Ok(Value::Null) };
        if n < 0 {
            return element_error("§6.48 GR2c", &format!("{n} elements are trimmed"));
        }
        let Some(array) = elements(args.first(), TRIM_ARRAY)? else { return Ok(Value::Null) };
        let kept = usize::try_from(n).ok().and_then(|n| array.len().checked_sub(n));
        match kept.and_then(|k| array.get(..k)) {
            Some(kept) => Ok(Value::Array(kept.into())),
            None => element_error("§6.48 GR2g", &format!("{n} elements are trimmed from an array of {}", array.len())),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §6.47 GR2] `<array concatenation>`: null if
/// either is; else array data, right truncation if the elements of both
/// are more than IL008; else the elements of the first, then the second's.
pub struct ArrayConcatenation(pub DataType);

impl ScalarFunction for ArrayConcatenation {
    fn name(&self) -> &str {
        ARRAY_CONCATENATION
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (Some(a), Some(b)) = (elements(args.first(), ARRAY_CONCATENATION)?, elements(args.get(1), ARRAY_CONCATENATION)?) else {
            return Ok(Value::Null);
        };
        let total = a.len().saturating_add(b.len());
        if total > most() {
            return crate::value::types::array::truncated("§6.47 GR2b", total, MAX_ARRAY_CARDINALITY);
        }
        Ok(Value::Array(a.iter().chain(b.iter()).cloned().collect()))
    }
}

/// [ISO/IEC 9075-2:2023 §15.5 GR3b i] The array an `<update target>` of
/// an element leaves: the target array, the position and the value, the
/// value already assigned to the element type; the array with the element
/// at the position replaced, or extended with nulls up to it past the
/// cardinality, within the maximum cardinality `.1`.
pub struct AssignElement(pub DataType, pub u32);

impl ScalarFunction for AssignElement {
    fn name(&self) -> &str {
        ASSIGN_ELEMENT
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.0.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(array) = elements(args.first(), ASSIGN_ELEMENT)? else {
            return fail_with(
                Code::Data,
                SqlState::NULL_VALUE_IN_ARRAY_TARGET,
                "[ISO/IEC 9075-2:2023 §15.5 GR3b i 1] an element of a null array is assigned",
            );
        };
        let max = usize::try_from(self.1).unwrap_or(usize::MAX);
        let i = whole(args.get(1))?;
        let Some(at) = i.and_then(|i| usize::try_from(i).ok()).and_then(|i| i.checked_sub(1)).filter(|&at| at < max) else {
            let shown = i.map_or_else(|| "NULL".to_owned(), |i| i.to_string());
            return element_error("§15.5 GR3b i 4", &format!("element {shown} of an array of maximum cardinality {max} is assigned"));
        };
        let mut assigned = array.to_vec();
        if at >= assigned.len() {
            assigned.resize(at.saturating_add(1), Value::Null);
        }
        if let Some(slot) = assigned.get_mut(at) {
            *slot = args.get(2).cloned().unwrap_or(Value::Null);
        }
        Ok(Value::Array(assigned.into()))
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 GR12] ARRAY_AGG: its argument a row of the
/// value and its sort keys, so no null is removed before it \[NOTE 528\];
/// each key ascending or, where `descending` says so, descending, as ORDER
/// BY orders [§10.10], by its collation if it has one. Of `<array value
/// constructor by query>` [§6.49 GR3], no rows make an empty array.
pub struct ArrayAggregate {
    /// Per sort key, whether DESC was written.
    pub descending: Vec<bool>,
    /// Per sort key, the collation it orders by, if not the default.
    pub collations: Vec<Option<CollationRef>>,
    /// Whether it is an `<array value constructor by query>`'s.
    pub by_query: bool,
}

impl AggregateFunction for ArrayAggregate {
    fn name(&self) -> &str {
        ARRAY_AGG
    }
    /// [ISO/IEC 9075-2:2023 §10.9 SR13] An array of the value's type, of
    /// IL008's maximum cardinality.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        match arg {
            Some(DataType::Row(r)) => {
                // [§9.14 SR2f, SR3] no key is array-ordered.
                if let Some(key) = r.fields.iter().skip(1).find(|f| array_ordered(&f.data_type)) {
                    return fail(
                        Code::SyntaxRule,
                        format!(
                            "[ISO/IEC 9075-2:2023 §9.14 SR3] ARRAY_AGG orders by values of {}, and arrays are compared for equality alone",
                            key.data_type
                        ),
                    );
                }
                let value = r.fields.first().map(|f| f.data_type.clone());
                value.map(|t| DataType::array(t, MAX_ARRAY_CARDINALITY)).ok_or_else(no_argument)
            }
            Some(_) | None => Err(no_argument()),
        }
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        let (descending, collations) = (self.descending.clone(), self.collations.clone());
        Box::new(Gathered { rows: Vec::new(), descending, collations, by_query: self.by_query })
    }
    /// Its keys' directions and collations, by their places in its row.
    fn label_suffix(&self) -> String {
        let keys = self.descending.iter().zip(&self.collations).enumerate().map(|(i, (&down, collation))| {
            let collated = collation.as_ref().map_or_else(String::new, |c| format!(" COLLATE {}", c.collation.name()));
            format!("{}{collated}{}", i.saturating_add(2), if down { " DESC" } else { "" })
        });
        let keys: Vec<String> = keys.collect();
        if keys.is_empty() { String::new() } else { format!(" ORDER BY {}", keys.join(", ")) }
    }
}

fn no_argument() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] ARRAY_AGG was bound without its row of a value and sort keys")
}

/// The rows of value and keys so far, each key as its collation orders it.
struct Gathered {
    rows: Vec<Arc<[Value]>>,
    descending: Vec<bool>,
    collations: Vec<Option<CollationRef>>,
    by_query: bool,
}

impl Gathered {
    /// [ISO/IEC 9075-2:2023 §10.10] How two rows order by their keys.
    fn order(a: &[Value], b: &[Value], descending: &[bool]) -> Ordering {
        let keys = a.iter().zip(b).skip(1).zip(descending);
        keys.map(|((x, y), &down)| if down { sort_cmp(y, x) } else { sort_cmp(x, y) }).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
    }
}

impl Accumulator for Gathered {
    /// [ISO/IEC 9075-2:2023 §10.9 GR12e] array data, right truncation
    /// past IL008, as soon as it is passed, so it holds no more.
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(row) = v else { return Err(no_argument()) };
        if self.rows.len() >= most() {
            let rule = if self.by_query { "§6.49 GR3b" } else { "§10.9 GR12e" };
            return crate::value::types::array::truncated(rule, self.rows.len().saturating_add(1), MAX_ARRAY_CARDINALITY);
        }
        // Each key by its collation's sort key, so `sort_cmp` orders it so.
        let keyed = row.iter().enumerate().map(|(i, part)| {
            let collation = i.checked_sub(1).and_then(|k| self.collations.get(k)).and_then(Option::as_ref);
            collation.map_or_else(|| part.clone(), |c| c.sort_key(part))
        });
        self.rows.push(if self.collations.iter().any(Option::is_some) { keyed.collect() } else { Arc::clone(row) });
        Ok(())
    }
    /// [ISO/IEC 9075-2:2023 §10.9 GR12g] null of no rows; else the values,
    /// ordered by their keys \[GR12c\], stably.
    fn finish(&self) -> Result<Value> {
        if self.rows.is_empty() && !self.by_query {
            return Ok(Value::Null);
        }
        let mut rows: Vec<&Arc<[Value]>> = self.rows.iter().collect();
        rows.sort_by(|a, b| Gathered::order(a, b, &self.descending));
        Ok(Value::Array(rows.into_iter().map(|r| r.first().cloned().unwrap_or(Value::Null)).collect()))
    }
}
