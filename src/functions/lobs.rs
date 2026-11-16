//! [ISO/IEC 9075-2:2023 §8.2, §9.11] The equality operations of large
//! objects (RULES.md D79). A large object is read to be compared, which
//! may fail, and is in no bucket, so the binder does not compare one by a
//! bare comparison, which a hash join, an index probe or a scan's filter
//! would take as a key. It compares the proxies of the two operands
//! (`value::lob_proxies`): TRUE for the first, and whether the two are
//! equal for the second, each as null as its operand, in arrays and rows
//! alike. "=" and "<>" of the proxies are those of the operands, and so is
//! IS DISTINCT FROM [§8.15 GR1], the operations a large object takes
//! [§9.14 SR3]. Each proxy is a call that may raise, so no key is made of
//! it.

pub(super) mod ends;
pub mod like;
pub(super) mod ops;
pub(super) mod pad;
pub(super) mod search;
#[cfg(test)]
mod tests;

use super::ScalarFunction;
use crate::error::{Code, Result, fail};
use crate::value::collation::CollationRef;
use crate::value::lob::Lob;
use crate::value::types::composite::lob_ordered;
use crate::value::types::{ArrayType, Field};
use crate::value::{DataType, Value, lob_proxies};

/// Which operand's proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The first's: TRUE where a large object is, not read.
    First,
    /// The second's: whether the large objects at each place are equal.
    Second,
}

/// \[DESIGN\] The proxy of one operand of an equality operation of large
/// objects, of its two operands, told equal under `collation`, else the
/// default.
#[derive(Debug, Clone)]
pub struct LobProxy {
    side: Side,
    collation: Option<CollationRef>,
}

/// The name a proxy shows as, in EXPLAIN.
const PROXY_NAME: &str = "LARGE OBJECT EQUALITY";

impl LobProxy {
    /// The proxy of `side`, told equal under `collation`.
    pub fn new(side: Side, collation: Option<CollationRef>) -> LobProxy {
        LobProxy { side, collation }
    }

    /// Whether `a` and `b`, of one kind, are equal under its collation.
    fn equal(&self, a: &Lob, b: &Lob) -> Result<bool> {
        match &self.collation {
            Some(c) => crate::value::collation::lob_equal(a, b, c),
            None => Ok(a.compare_to(b)?.is_eq()),
        }
    }
}

/// The type of the proxies of operands of types `t` and `u`: of arrays,
/// an array of the proxies' type of their elements; of rows of one degree,
/// a row of their fields'; BOOLEAN where either is LOB-ordered; else `t`.
/// Each recursion is into an element or field type, bounded by how deep
/// types nest.
pub fn proxy_type(t: &DataType, u: &DataType) -> DataType {
    if !lob_ordered(t) && !lob_ordered(u) {
        return t.clone();
    }
    if let (DataType::Array(a), DataType::Array(b)) = (t, u) {
        return DataType::Array(std::sync::Arc::new(ArrayType { element: proxy_type(&a.element, &b.element), max: a.max }));
    }
    if let (DataType::Row(r), DataType::Row(s)) = (t, u)
        && r.fields.len() == s.fields.len()
    {
        let fields = r.fields.iter().zip(&s.fields).map(|(f, g)| Field { data_type: proxy_type(&f.data_type, &g.data_type), ..f.clone() });
        return DataType::row(fields.collect());
    }
    DataType::Boolean
}

impl ScalarFunction for LobProxy {
    fn name(&self) -> &str {
        PROXY_NAME
    }

    /// The operands' types, each place of a large object BOOLEAN.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] a large object's proxy takes two operands") };
        Ok(proxy_type(a, b))
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [a, b] = args else { return fail(Code::Api, "[DESIGN] a large object's proxy takes two operands") };
        let (first, second) = match self.side {
            Side::First => lob_proxies(a, b, None)?,
            Side::Second => lob_proxies(a, b, Some(&|x: &Lob, y: &Lob| self.equal(x, y)))?,
        };
        Ok(match self.side {
            Side::First => first,
            Side::Second => second,
        })
    }
}
