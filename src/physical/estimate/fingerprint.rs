//! \[DESIGN\] The key under which a table's sample remembers what was
//! asked of it (`RowSample::remembered`, RULES.md D382): a structural
//! fingerprint of the question and of the conditions it is asked of,
//! hashed by XXH3 (`hash/xxhash.rs`). Each condition and
//! expression writes its kind, then what it holds: a column its place, a
//! constant its value as `Value::norm` makes it, so values identical by
//! the standard are one, an operator its kind, a function, a collation, a
//! type or a domain its name. The conjuncts are fingerprinted one by one,
//! and their fingerprints sorted, so a conjunction is one question
//! whatever the order it is written in.
//!
//! A sample is asked only of conditions it can evaluate
//! (`estimate/sampled.rs`), of no subquery, outer reference, `?`,
//! sequence value or SQL routine; those write their kind alone.

use std::hash::{BuildHasher, Hash, Hasher};
use std::mem::discriminant;
use std::sync::Arc;

use crate::hash::xxhash::{Xxh3BuildHasher, Xxh3Hasher};
use crate::logical::{Cond, Expr};

/// The key of the question `question` asked of the conjuncts `conds`,
/// and of `more`, written after them.
pub(super) fn key_of(question: &str, conds: &[&Cond], more: &dyn Fn(&mut Xxh3Hasher)) -> u64 {
    let mut each: Vec<u64> = conds
        .iter()
        .map(|c| {
            let mut h = Xxh3BuildHasher::default().build_hasher();
            cond(c, &mut h);
            h.finish()
        })
        .collect();
    each.sort_unstable();
    let mut h = Xxh3BuildHasher::default().build_hasher();
    h.write(question.as_bytes());
    each.hash(&mut h);
    more(&mut h);
    h.finish()
}

/// \[DESIGN\] (RULES.md D1279) The key of the question `question` asked
/// of the expression `e` within one statement, and of `more`, written
/// after it. It writes `e` as a condition's expressions are written, and
/// each function it calls by the address of its handle too. Two functions of
/// one name may differ in what they hold, as EXTRACT of two fields does,
/// and only the handle tells them apart (`logical/sets.rs`). The asker
/// keeps `e` for as long as it keeps the key, so no address in it is
/// another function's.
pub(in crate::physical) fn key_of_expr(question: &str, e: &Expr, more: &dyn Fn(&mut Xxh3Hasher)) -> u64 {
    let mut h = Xxh3BuildHasher::default().build_hasher();
    h.write(question.as_bytes());
    expr(e, &mut h);
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        if let Expr::Call(f, _) = next {
            Arc::as_ptr(&f.0).cast::<()>().addr().hash(&mut h);
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    more(&mut h);
    h.finish()
}

/// Writes `c` to `h`.
fn cond(c: &Cond, h: &mut Xxh3Hasher) {
    discriminant(c).hash(h);
    match c {
        Cond::Const(t) => discriminant(t).hash(h),
        Cond::Or(a, b) | Cond::And(a, b) => {
            cond(a, h);
            cond(b, h);
        }
        Cond::Not(a) => cond(a, h),
        Cond::Compare(a, op, b) => {
            expr(a, h);
            discriminant(op).hash(h);
            expr(b, h);
        }
        Cond::IsNull(e, not) => {
            expr(e, h);
            not.hash(h);
        }
        Cond::Distinct(a, b) => {
            expr(a, h);
            expr(b, h);
        }
        Cond::Like(a, pattern, escape) => {
            expr(a, h);
            expr(pattern, h);
            if let Some(e) = escape {
                expr(e, h);
            }
        }
        Cond::Is(a, t, not) => {
            cond(a, h);
            discriminant(t).hash(h);
            not.hash(h);
        }
        Cond::Value(e) => expr(e, h),
        Cond::In(e, set) => {
            expr(e, h);
            for v in set.values() {
                v.norm().hash(h);
            }
        }
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(..) | Cond::Match { .. } => {}
    }
}

/// Writes `e` to `h`.
fn expr(e: &Expr, h: &mut Xxh3Hasher) {
    discriminant(e).hash(h);
    match e {
        Expr::Column(at) => at.hash(h),
        Expr::Const(v) => v.norm().hash(h),
        Expr::Neg(a) | Expr::Warned(a, _) => expr(a, h),
        Expr::Arith(a, op, b) => {
            expr(a, h);
            discriminant(op).hash(h);
            expr(b, h);
        }
        Expr::Call(f, args) => {
            h.write(f.0.name().as_bytes());
            for a in args {
                expr(a, h);
            }
        }
        Expr::Case(arms, otherwise) => {
            for (when, then) in arms.iter() {
                cond(when, h);
                expr(then, h);
            }
            expr(otherwise, h);
        }
        Expr::Cast(a, to) => {
            expr(a, h);
            h.write(to.to_string().as_bytes());
        }
        Expr::Domain(a, checks) => {
            expr(a, h);
            h.write(checks.domain.to_string().as_bytes());
        }
        Expr::SortKey(a, collation) => {
            expr(a, h);
            h.write(collation.collation.name().as_bytes());
            discriminant(&collation.pad).hash(h);
        }
        Expr::Truth(c) => cond(c, h),
        Expr::Outer(..) | Expr::Dynamic(_) | Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_) => {}
    }
}
