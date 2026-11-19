//! The {namespace constraint} of a wildcard [Schema1 §3.10]: whether it
//! allows a namespace name (Wildcard allows Namespace Name [Schema1
//! §3.10.4]), and its intensional subset, union and intersection [Schema1
//! §3.10.6].

use super::component::NamespaceConstraint;

/// A namespace name, or absent.
type Namespace = Option<Box<str>>;

/// [Schema1 §3.10.2] The values of a namespace attribute: any namespace.
const ANY: &str = "##any";
/// Any namespace but the target namespace.
const OTHER: &str = "##other";
/// The target namespace.
const TARGET: &str = "##targetNamespace";
/// No namespace.
const LOCAL: &str = "##local";

impl NamespaceConstraint {
    /// [Schema1 §3.10.4 cvc-wildcard-namespace] Whether namespace name
    /// `ns`, none if absent, is valid with respect to it: any; not and a
    /// namespace other than `ns`, `ns` not absent; or a set holding `ns`.
    #[must_use]
    pub fn allows(&self, ns: Option<&str>) -> bool {
        match self {
            NamespaceConstraint::Any => true,
            NamespaceConstraint::Not(test) => ns.is_some() && test.as_deref() != ns,
            NamespaceConstraint::Set(set) => set.iter().any(|m| m.as_deref() == ns),
        }
    }

    /// [Schema1 §3.10.6 cos-ns-subset] Whether it is an intensional subset
    /// of `sup`.
    #[must_use]
    pub fn subset_of(&self, sup: &NamespaceConstraint) -> bool {
        match (self, sup) {
            (_, NamespaceConstraint::Any) => true,
            (NamespaceConstraint::Not(a), NamespaceConstraint::Not(b)) => a == b,
            (NamespaceConstraint::Set(sub), NamespaceConstraint::Set(set)) => sub.iter().all(|m| set.contains(m)),
            (NamespaceConstraint::Set(sub), NamespaceConstraint::Not(test)) => !sub.contains(test),
            (NamespaceConstraint::Any | NamespaceConstraint::Not(_), NamespaceConstraint::Set(_) | NamespaceConstraint::Not(_)) => false,
        }
    }

    /// [Schema1 §3.10.6 cos-aw-union] The intensional union of it and
    /// `other`; none if not expressible, of negations of two different
    /// namespace names \[4\].
    #[must_use]
    pub fn union(&self, other: &NamespaceConstraint) -> Option<NamespaceConstraint> {
        if self == other {
            return Some(self.clone());
        }
        Some(match (self, other) {
            (NamespaceConstraint::Any, _) | (_, NamespaceConstraint::Any) => NamespaceConstraint::Any,
            (NamespaceConstraint::Set(a), NamespaceConstraint::Set(b)) => NamespaceConstraint::Set(merged(a, b)),
            (NamespaceConstraint::Not(_), NamespaceConstraint::Not(_)) => return None,
            (NamespaceConstraint::Not(test), NamespaceConstraint::Set(set))
            | (NamespaceConstraint::Set(set), NamespaceConstraint::Not(test)) => {
                if set.contains(test) {
                    NamespaceConstraint::Any
                } else {
                    NamespaceConstraint::Not(test.clone())
                }
            }
        })
    }

    /// [Schema1 §3.10.6 cos-aw-intersect] The intensional intersection of
    /// it and `other`; none if not expressible, of negations of two
    /// different namespace names \[5\].
    #[must_use]
    pub fn intersection(&self, other: &NamespaceConstraint) -> Option<NamespaceConstraint> {
        if self == other {
            return Some(self.clone());
        }
        Some(match (self, other) {
            (NamespaceConstraint::Any, o) | (o, NamespaceConstraint::Any) => o.clone(),
            (NamespaceConstraint::Not(test), NamespaceConstraint::Set(set))
            | (NamespaceConstraint::Set(set), NamespaceConstraint::Not(test)) => {
                NamespaceConstraint::Set(set.iter().filter(|m| *m != test).cloned().collect())
            }
            (NamespaceConstraint::Set(a), NamespaceConstraint::Set(b)) => {
                NamespaceConstraint::Set(a.iter().filter(|m| b.contains(m)).cloned().collect())
            }
            (NamespaceConstraint::Not(_), NamespaceConstraint::Not(_)) => return None,
        })
    }

    /// Whether some namespace name, or absent, is allowed by both it and
    /// `other`: their intersection is not the empty set [Schema1 Appendix
    /// H]. Two negations of different names both allow every name but
    /// those two.
    #[must_use]
    pub fn overlaps(&self, other: &NamespaceConstraint) -> bool {
        match self.intersection(other) {
            Some(NamespaceConstraint::Set(set)) => !set.is_empty(),
            Some(NamespaceConstraint::Any | NamespaceConstraint::Not(_)) | None => true,
        }
    }
}

/// The members of `a` and `b`, in order, each once.
fn merged(a: &[Namespace], b: &[Namespace]) -> Vec<Namespace> {
    let mut out: Vec<Namespace> = a.iter().chain(b).cloned().collect();
    out.sort();
    out.dedup();
    out
}

/// [Schema1 §3.10.2] The {namespace constraint} of a namespace attribute
/// `value`, in a schema document of target namespace `tns`: any if
/// absent or `##any`; not and the target namespace of `##other`; else
/// the set of the URIs it lists, `##targetNamespace` the target namespace
/// and `##local` absent. None if it lists `##any` or `##other` among
/// others, which the Schema for Schemas does not allow [Schema1
/// Appendix A].
#[must_use]
pub fn namespace_constraint(value: Option<&str>, tns: Option<&str>) -> Option<NamespaceConstraint> {
    let Some(value) = value else { return Some(NamespaceConstraint::Any) };
    let words: Vec<&str> = value.split_ascii_whitespace().collect();
    match words.as_slice() {
        [ANY] => return Some(NamespaceConstraint::Any),
        [OTHER] => return Some(NamespaceConstraint::Not(tns.map(Into::into))),
        _ => {}
    }
    let mut set = Vec::new();
    for word in words {
        let member = match word {
            ANY | OTHER => return None,
            TARGET => tns.map(Into::into),
            LOCAL => None,
            uri => Some(uri.into()),
        };
        set.push(member);
    }
    set.sort();
    set.dedup();
    Some(NamespaceConstraint::Set(set))
}
