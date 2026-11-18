//! The names of element and attribute types as sets of QNames: whether
//! every name one matches another matches (`name_within`), and the names
//! both match (`names_both`).
//!
//! \[EXTENSION\] RULES.md D106: [XQueryFS §2.4.2] writes a name as a QName
//! or `*`, and its schema mapping does not handle a wildcard's namespace
//! constraint [XQueryFS Appendix C.11]. A name here is also narrowed
//! (`Narrowed`) to the namespaces a wildcard's {namespace constraint}
//! allows [Schema1 §3.10.4 Wildcard allows Namespace Name], and past the
//! names an attribute wildcard does not match, those of the attribute
//! uses beside it and of `xsi`'s four attributes [Schema1 §3.4.4
//! Element Locally Valid (Complex Type) 3]: each value of such a type
//! is of one of those names, so a type is as precise as the constraint.

use std::fmt::Write;

use super::super::plan::Namespace;
use super::types::Name;

/// The namespaces a name is in, of a wildcard narrowed past `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Namespaces {
    /// These, two or more, in order, each once.
    Among(Vec<Namespace>),
    /// Every one but these, in order, each once; none of them `Any`.
    NotAmong(Vec<Namespace>),
}

/// What narrows a name of namespace `Any` past its namespace and local
/// name: the namespaces it is in, and the names it does not match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Narrowed {
    /// The namespaces it is in, if not every one.
    pub namespaces: Option<Namespaces>,
    /// The QNames it does not match, in order, each once.
    pub except: Vec<(Namespace, Box<str>)>,
}

/// A namespace's place in order: none first, then by URI.
fn key(ns: &Namespace) -> Option<&str> {
    match ns {
        Namespace::Any | Namespace::Absent => None,
        Namespace::Uri(uri) => Some(uri),
    }
}

/// `list` in order, each once.
fn ordered(mut list: Vec<Namespace>) -> Vec<Namespace> {
    list.sort_by(|a, b| key(a).cmp(&key(b)));
    list.dedup();
    list
}

/// The namespaces `name` is in.
fn namespaces(name: &Name) -> Namespaces {
    match (&name.namespace, name.narrowed.as_ref().and_then(|n| n.namespaces.as_ref())) {
        (Namespace::Any, Some(narrowed)) => narrowed.clone(),
        (Namespace::Any, None) => Namespaces::NotAmong(Vec::new()),
        (one, _) => Namespaces::Among(vec![one.clone()]),
    }
}

/// Whether `ns` is among `set`.
fn has(set: &Namespaces, ns: &Namespace) -> bool {
    match set {
        Namespaces::Among(list) => list.contains(ns),
        Namespaces::NotAmong(list) => !list.contains(ns),
    }
}

/// Whether every namespace of `a` is of `b`.
fn subset(a: &Namespaces, b: &Namespaces) -> bool {
    match (a, b) {
        (Namespaces::Among(x), _) => x.iter().all(|ns| has(b, ns)),
        (Namespaces::NotAmong(_), Namespaces::Among(_)) => false,
        (Namespaces::NotAmong(x), Namespaces::NotAmong(y)) => y.iter().all(|ns| x.contains(ns)),
    }
}

/// The namespaces of both `a` and `b`.
fn intersection(a: &Namespaces, b: &Namespaces) -> Namespaces {
    match (a, b) {
        (Namespaces::Among(x), other) | (other, Namespaces::Among(x)) => {
            Namespaces::Among(x.iter().filter(|ns| has(other, ns)).cloned().collect())
        }
        (Namespaces::NotAmong(x), Namespaces::NotAmong(y)) => Namespaces::NotAmong(ordered(x.iter().chain(y).cloned().collect())),
    }
}

/// Whether `name` matches the QName `(ns, local)`.
fn matches(name: &Name, (ns, local): (&Namespace, &str)) -> bool {
    has(&namespaces(name), ns)
        && name.local.as_deref().is_none_or(|l| l == local)
        && !name.narrowed.as_ref().is_some_and(|n| n.except.iter().any(|(x, l)| x == ns && &**l == local))
}

/// The QNames `name` does not match.
fn except(name: &Name) -> &[(Namespace, Box<str>)] {
    name.narrowed.as_ref().map_or(&[], |n| &n.except)
}

/// The name of the namespaces `set` and local name `local`, but the
/// QNames `except`; none if it matches none.
#[must_use]
pub fn narrowed(set: Namespaces, local: Option<Box<str>>, except: Vec<(Namespace, Box<str>)>) -> Option<Name> {
    let (namespace, namespaces) = match set {
        Namespaces::Among(list) => match <[Namespace; 1]>::try_from(ordered(list)) {
            Ok([one]) => (one, None),
            Err(list) => {
                if list.is_empty() {
                    return None;
                }
                (Namespace::Any, Some(Namespaces::Among(list)))
            }
        },
        Namespaces::NotAmong(list) => (Namespace::Any, (!list.is_empty()).then(|| Namespaces::NotAmong(ordered(list)))),
    };
    let base = Name { namespace, local, narrowed: namespaces.map(|n| Box::new(Narrowed { namespaces: Some(n), except: Vec::new() })) };
    let mut except: Vec<(Namespace, Box<str>)> = except.into_iter().filter(|(ns, l)| matches(&base, (ns, l))).collect();
    except.sort_by(|(a, x), (b, y)| (key(a), x).cmp(&(key(b), y)));
    except.dedup();
    // Of one QName, it matches none if it is excepted.
    if base.local.is_some() && base.namespace != Namespace::Any && !except.is_empty() {
        return None;
    }
    if except.is_empty() {
        return Some(base);
    }
    let among = base.narrowed.and_then(|n| n.namespaces);
    Some(Name { narrowed: Some(Box::new(Narrowed { namespaces: among, except })), ..base })
}

/// Whether every name `a` matches `b` matches.
#[must_use]
pub fn name_within(a: &Name, b: &Name) -> bool {
    let local = b.local.is_none() || (a.local.is_some() && a.local == b.local);
    local && subset(&namespaces(a), &namespaces(b)) && except(b).iter().all(|(ns, l)| !matches(a, (ns, l)))
}

/// The names both `a` and `b` match, the most specific name of them, if
/// any.
#[must_use]
pub fn names_both(a: &Name, b: &Name) -> Option<Name> {
    let local = match (&a.local, &b.local) {
        (None, other) | (other, None) => other.clone(),
        (Some(x), Some(y)) => {
            if x != y {
                return None;
            }
            Some(x.clone())
        }
    };
    let both = intersection(&namespaces(a), &namespaces(b));
    narrowed(both, local, except(a).iter().chain(except(b)).cloned().collect())
}

/// The namespaces `set` written, `{a|b}` of some and `{^a|b}` of all but
/// some, none written for none.
#[must_use]
pub fn namespaces_written(set: &Namespaces) -> String {
    let list = |list: &[Namespace]| list.iter().map(|ns| key(ns).unwrap_or("")).collect::<Vec<_>>().join("|");
    match set {
        Namespaces::Among(among) => format!("{{{}}}", list(among)),
        Namespaces::NotAmong(not) => format!("{{^{}}}", list(not)),
    }
}

/// The QNames `name` does not match written, ` except` before each.
#[must_use]
pub fn except_written(name: &Name) -> String {
    except(name).iter().fold(String::new(), |mut out, (ns, local)| {
        let _ = write!(out, " except {{{}}}{local}", key(ns).unwrap_or(""));
        out
    })
}

#[cfg(test)]
mod tests;
