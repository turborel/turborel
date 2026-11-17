//! Validation of a literal against a simple type [Schema2 §4.1.4
//! Datatype Valid] [Schema1 §3.1.4, §3.14.4]: its white space normalized,
//! matched against each step's patterns, mapped to a value of the
//! type's lexical space, and that value checked against each other facet
//! [Schema2 §4.3]; and the cast of an atomic value as a type derived by
//! restriction, as XQuery casts one [XQueryFO §17.1.1, §17.4, §17.5].
//!
//! A literal or value that is not valid is an XQuery error, err:FORG0001,
//! as a cast reports it [XQueryFO §17.1.1]; SQLSTATE 10000.

use std::cmp::Ordering;

use super::{MAX_TYPE_NESTING, SimpleType, Variety};
use crate::error::{Code, Result, SqlError};
use crate::xml::{Atomic, canonical, cast, compare, digits, equal, from_lexical, length, normalize, xquery_error};

/// The error of `literal`, or a value written so, not valid for `ty`, as
/// the facet `why` says.
fn not_valid(ty: &SimpleType, literal: &str, why: &str) -> SqlError {
    xquery_error(&format!("'{literal}' is not valid for {}: {why}", ty.name), "[Schema2 §4.1.4] [XQueryFO §17.1.1] err:FORG0001")
}

/// Whether two values of a type's value space are one: the same atomic
/// values in order [Schema2 §4.2.1].
fn same(a: &[Atomic], b: &[Atomic]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| equal(x, y))
}

impl SimpleType {
    /// [Schema2 §4.1.4 Datatype Valid] The value `literal` denotes in it:
    /// one atomic value of an atomic type, or of the atomic or list member
    /// of a union that first validates it [Schema2 §2.5.1.3]; the items of
    /// a list, its normalized literal split at its spaces.
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, err:FORG0001, if it is not valid; SQLCODE -900 if
    /// the type nests past `MAX_TYPE_NESTING`.
    pub fn validate(&self, literal: &str) -> Result<Vec<Atomic>> {
        self.validate_at(literal, 0)
    }

    /// `validate`, `depth` lists and unions within the type validated.
    fn validate_at(&self, literal: &str, depth: usize) -> Result<Vec<Atomic>> {
        if depth > MAX_TYPE_NESTING {
            return Err(SqlError::new(Code::Unsupported, format!("[DESIGN] lists and unions nest more than {MAX_TYPE_NESTING} deep")));
        }
        let (normalized, values) = match &self.variety {
            Variety::Union(members) => self.member_value(literal, members, depth)?,
            Variety::Atomic(t) => {
                let normalized = normalize(literal, self.white_space());
                self.patterns(&normalized, literal)?;
                let value =
                    from_lexical(&normalized, *t)?.ok_or_else(|| not_valid(self, literal, &format!("it is no literal of {}", t.name())))?;
                (normalized, vec![value])
            }
            Variety::List(item) => {
                let normalized = normalize(literal, self.white_space());
                self.patterns(&normalized, literal)?;
                let mut values = Vec::new();
                for token in normalized.split(' ').filter(|t| !t.is_empty()) {
                    values.extend(item.validate_at(token, depth.saturating_add(1))?);
                }
                (normalized, values)
            }
        };
        self.facet_valid(&values, &normalized)?;
        Ok(values)
    }

    /// The literal `literal` normalized by, and its value in, the first of
    /// `members` that validates it [Schema2 §2.5.1.3], matched against the
    /// union's own patterns.
    fn member_value(&self, literal: &str, members: &[std::sync::Arc<SimpleType>], depth: usize) -> Result<(String, Vec<Atomic>)> {
        for member in members {
            if let Ok(values) = member.validate_at(literal, depth.saturating_add(1)) {
                let normalized = normalize(literal, member.white_space());
                self.patterns(&normalized, literal)?;
                return Ok((normalized, values));
            }
        }
        Err(not_valid(self, literal, "no member type validates it"))
    }

    /// [Schema2 §4.3.4.4 pattern valid] Whether `text` matches a pattern
    /// of each derivation step that gives any, those of a step ORed
    /// [Schema2 §4.3.4.3]; `literal` names it in the error.
    fn patterns(&self, text: &str, literal: &str) -> Result<()> {
        for step in &self.facets.patterns {
            let mut matched = false;
            for p in step {
                matched = p.regex.is_match(text)?;
                if matched {
                    break;
                }
            }
            if !matched {
                let all: Vec<&str> = step.iter().map(|p| &*p.text).collect();
                return Err(not_valid(self, literal, &format!("it matches no pattern of {} [Schema2 §4.3.4.4]", all.join(" | "))));
            }
        }
        Ok(())
    }

    /// [Schema2 §4.1.4 Datatype Valid 2] Whether `values`, the value of
    /// `literal`, is facet-valid for each facet but the patterns.
    fn facet_valid(&self, values: &[Atomic], literal: &str) -> Result<()> {
        let f = &self.facets;
        let units = match (&self.variety, values) {
            (Variety::List(_), _) => Some(values.len()),
            (Variety::Atomic(_) | Variety::Union(_), [one]) => length(one),
            (Variety::Atomic(_) | Variety::Union(_), _) => None,
        };
        let held = |slot: &Option<super::Held<u64>>| slot.as_ref().and_then(|h| usize::try_from(h.value).ok());
        let lengths = [
            (held(&f.length), [Ordering::Equal].as_slice(), "length [Schema2 §4.3.1.3]"),
            (held(&f.min_length), [Ordering::Equal, Ordering::Greater].as_slice(), "minLength [Schema2 §4.3.2.3]"),
            (held(&f.max_length), [Ordering::Less, Ordering::Equal].as_slice(), "maxLength [Schema2 §4.3.3.3]"),
        ];
        for (facet, valid, rule) in lengths {
            if let (Some(n), Some(limit)) = (units, facet)
                && !valid.contains(&n.cmp(&limit))
            {
                return Err(not_valid(self, literal, &format!("its length {n} breaks its {rule} {limit}")));
            }
        }
        if f.enumeration.as_ref().is_some_and(|all| !all.iter().any(|e| same(e, values))) {
            return Err(not_valid(self, literal, "it is not among its enumeration [Schema2 §4.3.5.4]"));
        }
        if let [one] = values {
            self.bounds_valid(one, literal)?;
            self.digits_valid(one, literal)?;
        }
        Ok(())
    }

    /// [Schema2 §4.3.7.3 to §4.3.10.3] Whether `value` is within each
    /// bound, an indeterminate order never so [Schema2 §3.2.6.3].
    fn bounds_valid(&self, value: &Atomic, literal: &str) -> Result<()> {
        let f = &self.facets;
        let bounds = [
            (&f.max_inclusive, [Ordering::Less, Ordering::Equal].as_slice(), "maxInclusive [Schema2 §4.3.7.3]"),
            (&f.max_exclusive, [Ordering::Less].as_slice(), "maxExclusive [Schema2 §4.3.8.3]"),
            (&f.min_exclusive, [Ordering::Greater].as_slice(), "minExclusive [Schema2 §4.3.9.3]"),
            (&f.min_inclusive, [Ordering::Greater, Ordering::Equal].as_slice(), "minInclusive [Schema2 §4.3.10.3]"),
        ];
        for (bound, valid, rule) in bounds {
            let Some(held) = bound else { continue };
            if !compare(value, &held.value).is_some_and(|o| valid.contains(&o)) {
                return Err(not_valid(self, literal, &format!("it is not within its {rule} {}", held.value.lexical)));
            }
        }
        Ok(())
    }

    /// [Schema2 §4.3.11.3, §4.3.12.3] Whether `value`, of a decimal, has
    /// no more digits, and no more after its period, than the facets say.
    fn digits_valid(&self, value: &Atomic, literal: &str) -> Result<()> {
        let Some((total, fraction)) = digits(value) else { return Ok(()) };
        let limit = |slot: &Option<super::Held<u64>>| slot.as_ref().and_then(|h| usize::try_from(h.value).ok());
        let checks = [
            (total, limit(&self.facets.total_digits), "totalDigits [Schema2 §4.3.11.3]"),
            (fraction, limit(&self.facets.fraction_digits), "fractionDigits [Schema2 §4.3.12.3]"),
        ];
        for (n, most, rule) in checks {
            if most.is_some_and(|m| n > m) {
                return Err(not_valid(self, literal, &format!("its {n} digits break its {rule}")));
            }
        }
        Ok(())
    }

    /// `value` cast as this type, an atomic one [XQueryFO §17.1.1, §17.2]:
    /// a string or untyped value validated as a literal, its patterns
    /// matched before it is cast; another value cast as the built-in type
    /// this is derived from, then checked against its facets, its patterns
    /// matched against the canonical representation of the value as of its
    /// own primitive type [XQueryFO §17.4], or as of this type's if they
    /// differ [XQueryFO §17.5].
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, XQuery error: of a list or union, which no value is
    /// cast as, err:XPST0051 [XQuery §2.5.4.2]; as `cast` refuses the cast;
    /// of a value the facets refuse, err:FORG0001.
    pub fn cast(&self, value: &Atomic) -> Result<Atomic> {
        let Some(target) = self.atomic() else {
            return Err(xquery_error(&format!("{} is no atomic type", self.name), "[XQuery §2.5.4.2] err:XPST0051"));
        };
        if value.kind.primitive().textual() {
            let mut values = self.validate(&value.lexical)?;
            return values.pop().ok_or_else(|| not_valid(self, &value.lexical, "it has no value"));
        }
        let typed = cast(value, target)?;
        let within = value.kind.primitive().schema() == target.primitive().schema();
        let source = if within { canonical(value) } else { canonical(&typed) };
        self.patterns(&source, &value.lexical)?;
        self.facet_valid(std::slice::from_ref(&typed), &value.lexical)?;
        Ok(typed)
    }
}
