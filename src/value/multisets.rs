//! [ISO/IEC 9075-2:2023 §4.12.3] What multisets are made of and compared
//! by: their elements in classes, each of elements not distinct from one
//! another, or, of elements holding no null, equal to one another; and,
//! where a null makes an element's equality unknown, a matching of what
//! the classes leave (RULES.md D80).
//!
//! Equality [§8.2 GR1b iii] and SUBMULTISET [§8.17 GR3] ask whether the
//! elements of one multiset pair with those of another, each pair equal,
//! or each equal or unknown. Elements holding no null pair by their
//! classes: two equal elements are interchangeable. What is left pairs
//! by a maximum matching, the null elements matching any element. The
//! grouping operations [§9.13], MULTISET UNION DISTINCT, EXCEPT, INTERSECT,
//! SET, IS A SET and INTERSECTION, count each class of elements not
//! distinct from one another [§8.15].

use std::cmp::Ordering;
use std::collections::HashMap;
use std::convert::Infallible;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::distinct::{distinct, key_bucket};
use super::types::composite::rows_hold;
use super::{Bucket, CompOp, NormKey, Value, bucket, compare};
use crate::error::{Result, SqlError};

/// What makes elements alike, so of one class: the bucket a value's class
/// is found under, none being a bucket no value holds, and whether two
/// values are alike.
trait Alike {
    /// What finding either fails with.
    type Error;
    /// The bucket `v`'s class is found under.
    fn bucket(&self, v: &Value) -> std::result::Result<Option<Bucket>, Self::Error>;
    /// Whether `a` and `b` are alike.
    fn same(&self, a: &Value, b: &Value) -> std::result::Result<bool, Self::Error>;
}

/// The classes of elements not distinct from one another [§8.15]: a null
/// like a null.
struct NotDistinct;

impl Alike for NotDistinct {
    type Error = SqlError;
    fn bucket(&self, v: &Value) -> Result<Option<Bucket>> {
        key_bucket(v, true)
    }
    fn same(&self, a: &Value, b: &Value) -> Result<bool> {
        Ok(!distinct(a, b)?)
    }
}

/// The classes of elements equal to one another [§8.2], of elements
/// holding no null: neither ever fails.
struct EqualOnes;

impl Alike for EqualOnes {
    type Error = Infallible;
    fn bucket(&self, v: &Value) -> std::result::Result<Option<Bucket>, Infallible> {
        Ok(bucket(v))
    }
    fn same(&self, a: &Value, b: &Value) -> std::result::Result<bool, Infallible> {
        Ok(compare(a, b) == Some(Ordering::Equal))
    }
}

/// The value of `r`, which never fails.
fn sure<T>(r: std::result::Result<T, Infallible>) -> T {
    match r {
        Ok(v) => v,
        Err(never) => match never {},
    }
}

/// A class of alike elements: the first found, and how many of each of
/// two operands' elements are in it.
#[derive(Debug, Clone, Copy)]
struct Class<'v> {
    first: &'v Value,
    left: usize,
    right: usize,
}

/// The classes of the elements of `left` and `right`, in the order each
/// is first found, and the class of each element of `left`, in order: a
/// value's class found under the bucket `alike` gives it, of the values
/// it finds alike; the classes of one bucket searched one by one.
fn classes<'v, A: Alike>(
    (left, right): (&[&'v Value], &[&'v Value]),
    alike: &A,
) -> std::result::Result<(Vec<Class<'v>>, Vec<usize>), A::Error> {
    let mut found: HashMap<Option<Bucket>, Vec<usize>> = HashMap::new();
    let mut all: Vec<Class<'v>> = Vec::new();
    let mut of_left = Vec::with_capacity(left.len());
    let sides = left.iter().map(|v| (*v, true)).chain(right.iter().map(|v| (*v, false)));
    for (v, is_left) in sides {
        let ids = found.entry(alike.bucket(v)?).or_default();
        let mut at = None;
        for &id in ids.iter() {
            if let Some(class) = all.get(id)
                && alike.same(class.first, v)?
            {
                at = Some(id);
                break;
            }
        }
        let id = at.unwrap_or(all.len());
        if at.is_none() {
            ids.push(id);
            all.push(Class { first: v, left: 0, right: 0 });
        }
        if let Some(class) = all.get_mut(id) {
            if is_left {
                class.left = class.left.saturating_add(1);
            } else {
                class.right = class.right.saturating_add(1);
            }
        }
        if is_left {
            of_left.push(id);
        }
    }
    Ok((all, of_left))
}

/// Whether `v` is or holds the null value, in a row's fields.
fn holds_null(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Row(fields) => fields.iter().any(holds_null),
        Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => false,
    }
}

/// The elements of `of` holding no null, and those holding one.
fn known_and_partial(of: &[Value]) -> (Vec<&Value>, Vec<&Value>) {
    of.iter().partition(|v| !holds_null(v))
}

/// The elements of each operand its classes leave unpaired: of each
/// class, those past the other's count; then each operand's `partial`.
fn unpaired<'v>(all: &[Class<'v>], partial: (Vec<&'v Value>, Vec<&'v Value>)) -> (Vec<&'v Value>, Vec<&'v Value>) {
    let (mut left, mut right) = partial;
    for c in all {
        left.extend(std::iter::repeat_n(c.first, c.left.saturating_sub(c.right)));
        right.extend(std::iter::repeat_n(c.first, c.right.saturating_sub(c.left)));
    }
    (left, right)
}

/// [ISO/IEC 9075-2:2023 §8.2 GR1b iii] Whether multisets of the elements
/// `xs` and `ys` are equal: false of other cardinalities; true if their
/// elements pair, each pair equal \[2\]; unknown, none, if they pair each
/// pair equal or unknown \[3\]; else false \[4\]. Its elements are of
/// no LOB-ordered type [§9.13 SR3], so none is read to be compared.
pub fn equal(xs: &[Value], ys: &[Value]) -> Option<bool> {
    if xs.len() != ys.len() {
        return Some(false);
    }
    let ((kx, px), (ky, py)) = (known_and_partial(xs), known_and_partial(ys));
    let (all, _) = sure(classes((&kx, &ky), &EqualOnes));
    if px.is_empty() && py.is_empty() {
        return Some(all.iter().all(|c| c.left == c.right));
    }
    let (left, right) = unpaired(&all, (px, py));
    let (matched, (lx, _), (_, wy)) = matching(&left, &right);
    // Each side leaves as many unpaired as the other, as the cardinalities
    // are equal, so a pairing that covers the left covers the right too.
    if matched.saturating_add(wy) >= lx { None } else { Some(false) }
}

/// [ISO/IEC 9075-2:2023 §8.17 GR3c] Whether a multiset of the elements
/// `xs` is a submultiset of one of `ys`, of no fewer: true if each of its
/// elements pairs with one of the other, each pair equal \[ii\]; unknown,
/// none, if each pair is equal or unknown \[iii\]; else false \[iv\].
/// Its elements are of no LOB-ordered type [§9.13 SR3].
pub fn submultiset(xs: &[Value], ys: &[Value]) -> Option<bool> {
    if xs.len() > ys.len() {
        return Some(false);
    }
    let ((kx, px), (ky, py)) = (known_and_partial(xs), known_and_partial(ys));
    let (all, _) = sure(classes((&kx, &ky), &EqualOnes));
    if px.is_empty() && all.iter().all(|c| c.left <= c.right) {
        return Some(true);
    }
    let (left, right) = unpaired(&all, (px, py));
    let (matched, (lx, _), (_, wy)) = matching(&left, &right);
    if matched.saturating_add(wy) >= lx { None } else { Some(false) }
}

/// A maximum matching of the elements `left` and `right` that are no
/// null, an element matching one whose equality with it is not false, a
/// row's as its fields' [§8.2 GR7]:
/// the pairs it makes, and of each side, its elements no null and its
/// nulls, each of which matches any element.
fn matching(left: &[&Value], right: &[&Value]) -> (usize, (usize, usize), (usize, usize)) {
    let (lv, ln): (Vec<&Value>, Vec<&Value>) = left.iter().copied().partition(|v| !v.is_null());
    let (rv, rn): (Vec<&Value>, Vec<&Value>) = right.iter().copied().partition(|v| !v.is_null());
    let mut edges = Vec::with_capacity(lv.len());
    for x in &lv {
        let mut to = Vec::new();
        for (j, y) in rv.iter().enumerate() {
            if rows_hold(std::slice::from_ref(*x), CompOp::Eq, std::slice::from_ref(*y)) != Some(false) {
                to.push(j);
            }
        }
        edges.push(to);
    }
    let mut owner: Vec<Option<usize>> = vec![None; rv.len()];
    let mut pairs = 0_usize;
    for start in 0..lv.len() {
        let mut seen = vec![false; rv.len()];
        if augment(start, &edges, &mut owner, &mut seen) {
            pairs = pairs.saturating_add(1);
        }
    }
    (pairs, (lv.len(), ln.len()), (rv.len(), rn.len()))
}

/// Kuhn's augmenting path from the left vertex `start` over `edges`, the
/// right vertices `owner` gives a left one matched, `seen` those tried:
/// whether it found one, the matching then grown by it. Its search is a
/// stack of left vertices and the edge each tries next, so it recurses
/// not at all (RULES.md R-CF-6), and each right vertex is tried once.
fn augment(start: usize, edges: &[Vec<usize>], owner: &mut [Option<usize>], seen: &mut [bool]) -> bool {
    let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
    let mut through: Vec<usize> = Vec::new();
    while let Some((u, next)) = stack.last().copied() {
        let Some(&v) = edges.get(u).and_then(|e| e.get(next)) else {
            stack.pop();
            if !stack.is_empty() {
                through.pop();
            }
            continue;
        };
        if let Some(top) = stack.last_mut() {
            top.1 = next.saturating_add(1);
        }
        if seen.get(v).copied().unwrap_or(true) {
            continue;
        }
        if let Some(s) = seen.get_mut(v) {
            *s = true;
        }
        through.push(v);
        if let Some(w) = owner.get(v).copied().flatten() {
            stack.push((w, 0));
            continue;
        }
        for (&(left, _), &right) in stack.iter().zip(&through) {
            if let Some(o) = owner.get_mut(right) {
                *o = Some(left);
            }
        }
        return true;
    }
    false
}

/// [ISO/IEC 9075-2:2023 §8.15 GR1c vi] Whether multisets of the elements
/// `xs` and `ys` are distinct: whether some value is not distinct from
/// more elements of one than of the other.
///
/// # Errors
///
/// As comparing two elements fails.
pub fn bags_distinct(xs: &[Value], ys: &[Value]) -> Result<bool> {
    if xs.len() != ys.len() {
        return Ok(true);
    }
    let (left, right): (Vec<&Value>, Vec<&Value>) = (xs.iter().collect(), ys.iter().collect());
    let (all, _) = classes((&left, &right), &NotDistinct)?;
    Ok(all.iter().any(|c| c.left != c.right))
}

/// [ISO/IEC 9075-2:2023 §6.50] How MULTISET UNION, EXCEPT and INTERSECT
/// make their result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BagOp {
    /// MULTISET UNION.
    Union,
    /// MULTISET EXCEPT.
    Except,
    /// MULTISET INTERSECT.
    Intersect,
}

/// [ISO/IEC 9075-2:2023 §6.50 SR2d, SR3d, SR4d] The elements of `op` of
/// multisets of the elements `xs` and `ys`, ALL or DISTINCT as `distinct`
/// says, as the query of UNNEST of each its rules give: of ALL, as many
/// of each class of elements not distinct from one another as UNION ALL
/// [§7.17 GR3b], EXCEPT ALL or INTERSECT ALL keep; of DISTINCT, one of
/// each class those keep. The elements kept are the first operand's,
/// then the second's.
///
/// # Errors
///
/// As comparing two elements fails.
pub fn combined(op: BagOp, distinct: bool, xs: &[Value], ys: &[Value]) -> Result<Vec<Value>> {
    if op == BagOp::Union && !distinct {
        return Ok(xs.iter().chain(ys).cloned().collect());
    }
    let (left, right): (Vec<&Value>, Vec<&Value>) = (xs.iter().collect(), ys.iter().collect());
    let (all, of_left) = classes((&left, &right), &NotDistinct)?;
    if op == BagOp::Union {
        return Ok(all.iter().map(|c| c.first.clone()).collect());
    }
    let mut kept = vec![0_usize; all.len()];
    let mut out = Vec::new();
    for (v, &id) in left.iter().zip(&of_left) {
        let (Some(c), Some(k)) = (all.get(id), kept.get_mut(id)) else { continue };
        let quota = match (op, distinct) {
            (BagOp::Except, false) => c.left.saturating_sub(c.right),
            (BagOp::Except, true) => usize::from(c.right == 0),
            (BagOp::Intersect, false) => c.left.min(c.right),
            (BagOp::Intersect, true) => usize::from(c.right > 0),
            (BagOp::Union, _) => c.left,
        };
        if *k < quota {
            *k = k.saturating_add(1);
            out.push((*v).clone());
        }
    }
    Ok(out)
}

/// [ISO/IEC 9075-2:2023 §8.18 SR4] Whether a multiset of the elements
/// `xs` is a set: whether no two are not distinct from each other.
///
/// # Errors
///
/// As comparing two elements fails.
pub fn is_set(xs: &[Value]) -> Result<bool> {
    let left: Vec<&Value> = xs.iter().collect();
    let (all, _) = classes((&left, &[]), &NotDistinct)?;
    Ok(all.len() == xs.len())
}

/// The hash of `b`, the same in each call.
fn hashed(b: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

/// The bucket of a multiset of `parts`, shared by every multiset whose
/// elements pair equal with them, in any order: their buckets' hashes
/// summed. None if a part has none, as such a multiset equals nothing.
pub fn bag_bucket(parts: &[Value]) -> Option<Bucket> {
    let mut sum = 0_u64;
    for p in parts {
        sum = sum.wrapping_add(hashed(&bucket(p)?));
    }
    Some(Bucket::Bag(sum))
}

/// The bucket of a multiset of `parts` on which a null matches a null,
/// as `bag_bucket` but of each part's such bucket.
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub fn null_bag_bucket(parts: &[Value], bucket_of: impl Fn(&Value) -> Result<Option<Bucket>>) -> Result<Option<Bucket>> {
    let mut sum = 0_u64;
    for p in parts {
        let Some(b) = bucket_of(p)? else { return Ok(None) };
        sum = sum.wrapping_add(hashed(&b));
    }
    Ok(Some(Bucket::Bag(sum)))
}

/// [ISO/IEC 9075-2:2023 §4.12.3] The form `Value::norm` gives a multiset
/// of `parts`: its parts' forms, in any order, equal when the multisets
/// are identical.
pub fn norm_bag(parts: &[Value]) -> NormKey {
    NormKey::Bag(BagKey(parts.iter().map(Value::norm).collect()))
}

/// The forms of a multiset's elements, equal to another's when each form
/// is in both as many times, hashed alike in any order.
#[derive(Debug, Clone)]
pub struct BagKey(Vec<NormKey>);

impl BagKey {
    /// The forms, in the order of the elements.
    pub fn keys(&self) -> &[NormKey] {
        &self.0
    }

    /// How many times each form is in it.
    fn counts(&self) -> HashMap<&NormKey, usize> {
        let mut counts: HashMap<&NormKey, usize> = HashMap::new();
        for k in &self.0 {
            let n = counts.entry(k).or_default();
            *n = n.saturating_add(1);
        }
        counts
    }
}

impl PartialEq for BagKey {
    fn eq(&self, other: &BagKey) -> bool {
        self.0.len() == other.0.len() && self.counts() == other.counts()
    }
}

impl Eq for BagKey {}

impl Hash for BagKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.iter().fold(0_u64, |sum, k| sum.wrapping_add(hashed(k))).hash(state);
    }
}

// Unit tests: equality, SUBMULTISET, distinctness and the grouping
// operations, each case of their rules (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
