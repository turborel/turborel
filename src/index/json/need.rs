//! \[DESIGN\] What a document must post to be found by a call (RULES.md
//! D34): a formula of terms, and of AND and OR, true of every document the
//! call is true of, and perhaps of more, which the WHERE check rejects.
//!
//! * JSON_EXISTS: the node its path's member chain reaches, and what each
//!   filter on the way asks. A filter's `@.k == lit`, or `@ == $v`, asks
//!   the value at its chain; another comparison, `exists`, `like_regex`
//!   or `starts with`, the nodes its operands reach; `&&` and `||` their
//!   operands' AND and OR; `!` and `is unknown` nothing. A wildcard member
//!   or an item method ends the chain: what follows asks nothing more.
//! * JSON_VALUE(doc, chain) = v: the value at the chain, of a returned
//!   type whose cast keeps its value (`CHARACTER VARYING` of turborel's
//!   greatest length, BOOLEAN); of another, the node.
//! * JSON_CONTAINS(doc, candidate): each scalar of the candidate at its
//!   member chain, and each empty array or object's node.
//!
//! A chain deeper than JSON.MAX_DEPTH asks for the node at that depth,
//! and a value under JSON.PATHS_ONLY the node it is of.

use super::terms::{Canon, Chain, Shape, Term, probe_canon, text};
use crate::functions::{JsonOperator, JsonProbe};
use crate::json::{Accessor, Comparison, Expr, Item, Keys, Predicate, parse};
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value};

/// What a document must post.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Need {
    /// Nothing: every document.
    Every,
    /// This term.
    Term(Term),
    /// Each of these.
    And(Vec<Need>),
    /// One of these.
    Or(Vec<Need>),
}

/// `a` and `b`.
fn and(a: Need, b: Need) -> Need {
    match (a, b) {
        (Need::Every, other) | (other, Need::Every) => other,
        (Need::And(mut xs), Need::And(ys)) => {
            xs.extend(ys);
            Need::And(xs)
        }
        (Need::And(mut xs), other) | (other, Need::And(mut xs)) => {
            xs.push(other);
            Need::And(xs)
        }
        (a, b) => Need::And(vec![a, b]),
    }
}

/// `a` or `b`.
fn or(a: Need, b: Need) -> Need {
    match (a, b) {
        (Need::Every, _) | (_, Need::Every) => Need::Every,
        (Need::Or(mut xs), Need::Or(ys)) => {
            xs.extend(ys);
            Need::Or(xs)
        }
        (Need::Or(mut xs), other) | (other, Need::Or(mut xs)) => {
            xs.push(other);
            Need::Or(xs)
        }
        (a, b) => Need::Or(vec![a, b]),
    }
}

/// What a probe reads: the index's shape, and each named variable's item,
/// none of one it cannot read.
struct Cx<'a> {
    shape: Shape,
    variables: Vec<(&'a str, Option<Item>)>,
}

impl Cx<'_> {
    /// The node at `chain`, cut to the depth the index posts.
    fn node(&self, chain: &[String]) -> Need {
        let kept = chain.get(..chain.len().min(self.shape.max_depth)).unwrap_or_default();
        Need::Term(Chain::of(kept.iter().map(String::as_str)).node())
    }

    /// One of `canons` at `chain`, if the index posts values there; else
    /// the node.
    fn value(&self, chain: &[String], canons: &[Canon]) -> Need {
        if self.shape.paths_only || chain.len() > self.shape.max_depth || canons.is_empty() {
            return self.node(chain);
        }
        let at = Chain::of(chain.iter().map(String::as_str));
        canons.iter().map(|canon| Need::Term(at.value(canon))).reduce(or).unwrap_or(Need::Every)
    }

    /// The item of a literal or a named variable, if known.
    fn scalar(&self, expr: &Expr) -> Option<Item> {
        match expr {
            Expr::Literal(item) => Some(item.clone()),
            Expr::Variable(name) => self.variables.iter().find(|(n, _)| n == name).and_then(|(_, item)| item.clone()),
            Expr::Context | Expr::Current | Expr::Last | Expr::Access(..) | Expr::Signed(..) | Expr::Arithmetic(..) => None,
        }
    }
}

/// Where a path expression reaches: the member chain from the root,
/// whether it is that node exactly or something below or made of it, and
/// what its filters ask.
#[derive(Debug, Clone)]
struct Walk {
    chain: Vec<String>,
    exact: bool,
    need: Need,
}

impl Walk {
    /// Its node, and what its filters ask.
    fn need(self, cx: &Cx<'_>) -> Need {
        and(cx.node(&self.chain), self.need)
    }
}

/// Where `expr` reaches, `current` the item of the innermost filter, if
/// it is a chain of accessors from the context item or the current item.
/// A parsed path nests no deeper than its bound (`json/path.rs`), so the
/// walk recurses that deep at most.
fn walk(expr: &Expr, cx: &Cx<'_>, current: Option<&Walk>) -> Option<Walk> {
    match expr {
        Expr::Context => Some(Walk { chain: Vec::new(), exact: true, need: Need::Every }),
        Expr::Current => current.map(|c| Walk { need: Need::Every, ..c.clone() }),
        Expr::Access(base, accessor) => {
            let mut w = walk(base, cx, current)?;
            match accessor {
                Accessor::Member(key) => {
                    if w.exact {
                        w.chain.push(key.clone());
                    }
                }
                Accessor::AnyMember | Accessor::Method(_) => w.exact = false,
                Accessor::Subscripts(_) | Accessor::AnyElement => {}
                Accessor::Filter(predicate) => {
                    let asked = asks(predicate, cx, &w);
                    w.need = and(std::mem::replace(&mut w.need, Need::Every), asked);
                }
            }
            Some(w)
        }
        Expr::Literal(_) | Expr::Variable(_) | Expr::Last | Expr::Signed(..) | Expr::Arithmetic(..) => None,
    }
}

/// What a filter's `predicate` asks of a document, `current` its item.
fn asks(predicate: &Predicate, cx: &Cx<'_>, current: &Walk) -> Need {
    let reaches = |e: &Expr| walk(e, cx, Some(current)).map_or(Need::Every, |w| w.need(cx));
    match predicate {
        Predicate::And(a, b) => and(asks(a, cx, current), asks(b, cx, current)),
        Predicate::Or(a, b) => or(asks(a, cx, current), asks(b, cx, current)),
        Predicate::Not(_) | Predicate::IsUnknown(_) => Need::Every,
        Predicate::Exists(e) | Predicate::LikeRegex(e, _) | Predicate::StartsWith(e, _) => reaches(e),
        Predicate::Compare(left, op, right) => {
            let equal = |w: Option<Walk>, item: Option<Item>| {
                let w = w.filter(|w| w.exact)?;
                let canon = probe_canon(&item?)?;
                Some(and(cx.value(&w.chain, &[canon]), w.need))
            };
            let (l, r) = (walk(left, cx, Some(current)), walk(right, cx, Some(current)));
            if *op == Comparison::Equal {
                let found = equal(l.clone(), cx.scalar(right)).or_else(|| equal(r.clone(), cx.scalar(left)));
                if let Some(need) = found {
                    return need;
                }
            }
            and(l.map_or(Need::Every, |w| w.need(cx)), r.map_or(Need::Every, |w| w.need(cx)))
        }
    }
}

/// The item a PASSING argument's value is: of FORMAT JSON, its JSON text
/// parsed; none if it cannot be read, which then asks nothing.
fn passed(value: &Value, format_json: bool) -> Option<Item> {
    if !format_json {
        return Item::of_value(value).ok();
    }
    let Value::Char(json) = value else { return None };
    parse(json, Keys::Any).ok()
}

/// The fields of a probe's value: its row's, or the value alone.
fn fields(value: &Value) -> Vec<Value> {
    if let Value::Row(fields) = value {
        return fields.to_vec();
    }
    if value.is_null() { Vec::new() } else { vec![value.clone()] }
}

/// What the call `probe` of the probe's `value`, its PASSING arguments'
/// values then the value compared with or contained, asks of a document
/// an index of `shape` holds.
pub(super) fn need_of(probe: &JsonProbe<'_>, value: &Value, shape: Shape) -> Need {
    let fields = fields(value);
    let variables =
        probe.passing.iter().zip(&fields).map(|((name, format_json), field)| (name.as_str(), passed(field, *format_json))).collect();
    let cx = Cx { shape, variables };
    let compared = fields.get(probe.passing.len());
    match (probe.operator, probe.path) {
        (JsonOperator::Exists, Some(path)) => walk(&path.expr, &cx, None).map_or(Need::Every, |w| w.need(&cx)),
        (JsonOperator::Value, Some(path)) => {
            let Some(w) = walk(&path.expr, &cx, None) else { return Need::Every };
            let canons = compared.map(|v| canons_of(v, probe.returning)).unwrap_or_default();
            if w.exact && !canons.is_empty() { and(cx.value(&w.chain, &canons), w.need) } else { w.need(&cx) }
        }
        (JsonOperator::Contains, _) => compared.map_or(Need::Every, |candidate| contained(candidate, &cx)),
        (JsonOperator::Exists | JsonOperator::Value | JsonOperator::Query, _) => Need::Every,
    }
}

/// The canonical values a JSON_VALUE result equal to `value` posts, of
/// the returned type `returning`: none where a cast may change a value.
fn canons_of(value: &Value, returning: Option<&DataType>) -> Vec<Canon> {
    let widest = returning.is_some_and(|t| *t == DataType::varying(MAX_CHARACTER_LENGTH));
    if let (Value::Char(s), true) = (value, widest) {
        return vec![text(s)];
    }
    if let (Value::Bool(truth), Some(DataType::Boolean)) = (value, returning) {
        return vec![Canon::Bool(*truth)];
    }
    Vec::new()
}

/// What a candidate's JSON text asks of a document containing it: none of
/// one that is no JSON text, so every document is found, and the WHERE
/// check raises as a scan would (RULES.md D15).
fn contained(candidate: &Value, cx: &Cx<'_>) -> Need {
    let Value::Char(json) = candidate else { return Need::Every };
    let Ok(item) = parse(json, Keys::Any) else { return Need::Every };
    let mut chain = Vec::new();
    leaves(&item, &mut chain, cx)
}

/// Each leaf of `item` at `chain`, AND. Parsed items nest no deeper than
/// `json::MAX_NESTING`.
fn leaves(item: &Item, chain: &mut Vec<String>, cx: &Cx<'_>) -> Need {
    match item {
        Item::Null | Item::Scalar(_) => probe_canon(item).map_or_else(|| cx.node(chain), |canon| cx.value(chain, &[canon])),
        Item::Array(elements) => {
            let each = elements.iter().map(|e| leaves(e, chain, cx)).reduce(and);
            each.unwrap_or_else(|| cx.node(chain))
        }
        Item::Object(members) => {
            let mut need = cx.node(chain);
            for member in members {
                chain.push(member.key.clone());
                need = and(need, leaves(&member.value, chain, cx));
                chain.pop();
            }
            need
        }
    }
}

#[cfg(test)]
mod tests;
