//! [ISO/IEC 9075-2:2023 §8.2, §9.11] Equality operations of large objects,
//! bound as comparisons of their proxies (`functions/lobs.rs`): a large
//! object is read to be compared, which may fail, and is in no bucket, so
//! it is compared by calls no hash join, index probe or scan takes as a
//! key (RULES.md D79).

use std::sync::Arc;

use crate::functions::ScalarRef;
use crate::functions::lobs::{LobProxy, Side};
use crate::logical::Expr;
use crate::value::DataType;
use crate::value::collation::CollationRef;
use crate::value::types::composite::lob_ordered;

/// `a` and `b`, operands of types `t` and `u` of an equality operation, as
/// their proxies, if either is LOB-ordered, told equal under `collation`
/// if they are character strings; none of others, compared as they are.
pub(super) fn proxied(a: &Expr, b: &Expr, (t, u): (&DataType, &DataType), collation: Option<&CollationRef>) -> Option<(Expr, Expr)> {
    if !lob_ordered(t) && !lob_ordered(u) {
        return None;
    }
    let collation = collation.filter(|_| t.is_character() && u.is_character()).cloned();
    let call = |side| Expr::Call(ScalarRef(Arc::new(LobProxy::new(side, collation.clone()))), vec![a.clone(), b.clone()]);
    Some((call(Side::First), call(Side::Second)))
}
