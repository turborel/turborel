//! Derivation by restriction [Schema2 §4.1.2.1]: the facets a step may
//! specify [Schema2 §4.1.3, §4.1.5], each a value of its base's value
//! space where it is one, merged with its base's facets, and the rules a
//! restriction keeps: a fixed facet keeps its value, each facet narrows
//! its base's, and the facets agree with one another [Schema2 §4.3].
//!
//! \[DESIGN\] RULES.md D86: a bound a step specifies replaces its base's
//! bound on that side, inclusive or exclusive, which the rules above make
//! no wider; so "both minInclusive and minExclusive specified for the same
//! datatype" [Schema2 §4.3.9.4] is read, as the rule for the upper bounds
//! [Schema2 §4.3.8.4] says, of one derivation step.

use std::cmp::Ordering;
use std::sync::Arc;

use super::facet::{Facet, FacetKind, FacetSpec, Facets, Held, Pattern, applicable};
use super::{SimpleType, invalid};
use crate::error::Result;
use crate::regex::Regex;
use crate::xml::{WhiteSpace, compare, equal};

/// A bound, the rule it keeps, and each bound of its base it must not
/// pass with the orders that pass it.
type Narrower = (FacetKind, &'static str, [(FacetKind, &'static [Ordering]); 4]);

/// [Schema2 §4.3.7.4, §4.3.8.4, §4.3.9.4, §4.3.10.4 valid restriction]
/// For a bound a step specifies, each bound of its base it must not pass,
/// and the orders of its value to that bound that pass it.
const NARROWER: [Narrower; 4] = [
    (
        FacetKind::MaxInclusive,
        "§4.3.7.4 maxInclusive valid restriction",
        [
            (FacetKind::MaxInclusive, &[Ordering::Greater]),
            (FacetKind::MaxExclusive, &[Ordering::Greater, Ordering::Equal]),
            (FacetKind::MinInclusive, &[Ordering::Less]),
            (FacetKind::MinExclusive, &[Ordering::Less, Ordering::Equal]),
        ],
    ),
    (
        FacetKind::MaxExclusive,
        "§4.3.8.4 maxExclusive valid restriction",
        [
            (FacetKind::MaxExclusive, &[Ordering::Greater]),
            (FacetKind::MaxInclusive, &[Ordering::Greater]),
            (FacetKind::MinInclusive, &[Ordering::Less, Ordering::Equal]),
            (FacetKind::MinExclusive, &[Ordering::Less, Ordering::Equal]),
        ],
    ),
    (
        FacetKind::MinExclusive,
        "§4.3.9.4 minExclusive valid restriction",
        [
            (FacetKind::MinExclusive, &[Ordering::Less]),
            (FacetKind::MaxInclusive, &[Ordering::Greater]),
            (FacetKind::MinInclusive, &[Ordering::Less]),
            (FacetKind::MaxExclusive, &[Ordering::Greater, Ordering::Equal]),
        ],
    ),
    (
        FacetKind::MinInclusive,
        "§4.3.10.4 minInclusive valid restriction",
        [
            (FacetKind::MinInclusive, &[Ordering::Less]),
            (FacetKind::MaxInclusive, &[Ordering::Greater]),
            (FacetKind::MinExclusive, &[Ordering::Less, Ordering::Equal]),
            (FacetKind::MaxExclusive, &[Ordering::Greater, Ordering::Equal]),
        ],
    ),
];

/// [Schema2 §4.3.7.4, §4.3.8.4, §4.3.9.4, §4.3.10.4] The lower and upper
/// bounds of one type that must be in order, and the orders that are not.
const ORDERED: [(FacetKind, FacetKind, &[Ordering], &str); 4] = [
    (FacetKind::MinInclusive, FacetKind::MaxInclusive, &[Ordering::Greater], "§4.3.7.4 minInclusive <= maxInclusive"),
    (FacetKind::MinExclusive, FacetKind::MaxExclusive, &[Ordering::Greater], "§4.3.8.4 minExclusive <= maxExclusive"),
    (FacetKind::MinExclusive, FacetKind::MaxInclusive, &[Ordering::Greater, Ordering::Equal], "§4.3.9.4 minExclusive < maxInclusive"),
    (FacetKind::MinInclusive, FacetKind::MaxExclusive, &[Ordering::Greater, Ordering::Equal], "§4.3.10.4 minInclusive < maxExclusive"),
];

impl SimpleType {
    /// The type derived from `base` by restriction with the facets
    /// `specified` [Schema2 §4.1.2.1]: of `base`'s variety, its facets
    /// `base`'s with these merged in.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if a facet does not apply to `base`, or is
    /// specified twice, or its value is not of `base`'s value space or
    /// not a regular expression, or it widens or changes a facet of
    /// `base` it may not, or the facets disagree [Schema2 §4.1.3, §4.1.5,
    /// §4.3].
    pub fn restrict(base: &Arc<SimpleType>, specified: &[FacetSpec]) -> Result<SimpleType> {
        step(base, specified)?;
        let mut facets = base.facets.clone();
        let (mut patterns, mut enumeration) = (Vec::new(), Vec::new());
        for s in specified {
            match &s.facet {
                Facet::Pattern(p) => patterns.push(pattern(p)?),
                Facet::Enumeration(literal) => enumeration.push(base.validate(literal).map_err(|_| {
                    invalid("§4.3.5.5 enumeration valid restriction", &format!("'{literal}' is no value of {}", base.name))
                })?),
                Facet::MaxInclusive(literal)
                | Facet::MaxExclusive(literal)
                | Facet::MinExclusive(literal)
                | Facet::MinInclusive(literal) => {
                    bound(&mut facets, base, (s.facet.kind(), literal), s.fixed)?;
                }
                Facet::Length(_)
                | Facet::MinLength(_)
                | Facet::MaxLength(_)
                | Facet::WhiteSpace(_)
                | Facet::TotalDigits(_)
                | Facet::FractionDigits(_) => count(&mut facets, s)?,
            }
        }
        if !patterns.is_empty() {
            facets.patterns.push(patterns);
        }
        if !enumeration.is_empty() {
            facets.enumeration = Some(enumeration);
        }
        agree(&facets)?;
        let name = format!("a restriction of {}", base.name).into();
        Ok(SimpleType { name, variety: base.variety.clone(), facets, depth: base.depth })
    }
}

/// [Schema2 §4.1.3, §4.1.5, §4.3.8.4, §4.3.9.4] The facets of one step:
/// each applicable to `base`; none but pattern and enumeration twice; not
/// both bounds of one side.
fn step(base: &SimpleType, specified: &[FacetSpec]) -> Result<()> {
    let mut seen: Vec<FacetKind> = Vec::with_capacity(specified.len());
    for s in specified {
        let kind = s.facet.kind();
        if !applicable(&base.variety, kind) {
            return Err(invalid("§4.1.5 applicable facets", &format!("{kind} does not apply to {}", base.name)));
        }
        if seen.contains(&kind) && !matches!(kind, FacetKind::Pattern | FacetKind::Enumeration) {
            return Err(invalid("§4.1.3 Single Facet Value", &format!("{kind} is specified twice")));
        }
        seen.push(kind);
    }
    if seen.contains(&FacetKind::MaxInclusive) && seen.contains(&FacetKind::MaxExclusive) {
        return Err(invalid("§4.3.8.4 maxInclusive and maxExclusive", "both are specified"));
    }
    if seen.contains(&FacetKind::MinInclusive) && seen.contains(&FacetKind::MinExclusive) {
        return Err(invalid("§4.3.9.4 minInclusive and minExclusive", "both are specified"));
    }
    Ok(())
}

/// [Schema2 §4.3.4.2] A pattern's value, compiled: a regular expression
/// of [Schema2 Appendix F].
fn pattern(text: &str) -> Result<Pattern> {
    let regex = Regex::schema(text).map_err(|e| invalid("§4.3.4.2", &format!("'{text}' is no regular expression: {e}")))?;
    Ok(Pattern { text: text.into(), regex })
}

/// The error of a fixed facet `kind` given another value [Schema2
/// §4.3.1.1 and the like].
fn fixed(kind: FacetKind) -> crate::error::SqlError {
    invalid("§4.3", &format!("{kind} is fixed by the base type, and given another value"))
}

/// A bound of `kind` whose value is `literal`, of `base`'s value space,
/// merged into `facets`, which hold `base`'s: no other than a fixed one of
/// `base`, nor beyond a bound of `base` [Schema2 §4.3.7.4, §4.3.8.4,
/// §4.3.9.4, §4.3.10.4]; the other bound of its side removed.
fn bound(facets: &mut Facets, base: &SimpleType, (kind, literal): (FacetKind, &str), fixed_now: bool) -> Result<()> {
    let why = || format!("'{literal}' is no value of {}", base.name);
    // [Schema2 §4.3.7.2] a value of the base's value space, its own bounds
    // aside, which the rules below compare it with: so a bound may restate
    // its base's exclusive one, as they allow.
    let mut unbounded = base.clone();
    for slot in [
        &mut unbounded.facets.max_inclusive,
        &mut unbounded.facets.max_exclusive,
        &mut unbounded.facets.min_exclusive,
        &mut unbounded.facets.min_inclusive,
    ] {
        *slot = None;
    }
    let values = unbounded.validate(literal).map_err(|_| invalid("§4.3.7.2", &why()))?;
    let [value] = values.as_slice() else { return Err(invalid("§4.3.7.2", &why())) };
    if base.facets.bound(kind).is_some_and(|held| held.fixed && !equal(&held.value, value)) {
        return Err(fixed(kind));
    }
    let none: &[(FacetKind, &[Ordering])] = &[];
    let (rule, rules) = NARROWER.iter().find(|(k, ..)| *k == kind).map_or(("§4.3", none), |(_, rule, rules)| (*rule, rules.as_slice()));
    for (parent, wrong) in rules {
        let order = base.facets.bound(*parent).and_then(|held| compare(value, &held.value));
        if order.is_some_and(|o| wrong.contains(&o)) {
            return Err(invalid(rule, &format!("{kind} '{literal}' is beyond the base type's {parent}")));
        }
    }
    let held = Some(Held { value: value.clone(), fixed: fixed_now });
    let (this, other) = match kind {
        FacetKind::MaxInclusive => (&mut facets.max_inclusive, &mut facets.max_exclusive),
        FacetKind::MaxExclusive => (&mut facets.max_exclusive, &mut facets.max_inclusive),
        FacetKind::MinExclusive => (&mut facets.min_exclusive, &mut facets.min_inclusive),
        FacetKind::MinInclusive
        | FacetKind::Length
        | FacetKind::MinLength
        | FacetKind::MaxLength
        | FacetKind::Pattern
        | FacetKind::Enumeration
        | FacetKind::WhiteSpace
        | FacetKind::TotalDigits
        | FacetKind::FractionDigits => (&mut facets.min_inclusive, &mut facets.min_exclusive),
    };
    *this = held;
    *other = None;
    Ok(())
}

/// A facet whose value is a count or the whiteSpace, `s`, merged into
/// `facets`, which hold the base's: no other than a fixed one, and no
/// wider than the base's [Schema2 §4.3.1.4, §4.3.2.4, §4.3.3.4,
/// §4.3.6.4, §4.3.11.4].
fn count(facets: &mut Facets, s: &FacetSpec) -> Result<()> {
    let held = |value| Some(Held { value, fixed: s.fixed });
    let kind = s.facet.kind();
    match s.facet {
        Facet::Length(n) => {
            narrower(facets.length.as_ref(), (kind, n), &[Ordering::Less, Ordering::Greater], "§4.3.1.4 length valid restriction")?;
            facets.length = held(n);
        }
        Facet::MinLength(n) => {
            narrower(facets.min_length.as_ref(), (kind, n), &[Ordering::Less], "§4.3.2.4 minLength valid restriction")?;
            facets.min_length = held(n);
        }
        Facet::MaxLength(n) => {
            narrower(facets.max_length.as_ref(), (kind, n), &[Ordering::Greater], "§4.3.3.4 maxLength valid restriction")?;
            facets.max_length = held(n);
        }
        Facet::TotalDigits(n) => {
            if n == 0 {
                return Err(invalid("§4.3.11", "totalDigits is a positive integer"));
            }
            narrower(facets.total_digits.as_ref(), (kind, n), &[Ordering::Greater], "§4.3.11.4 totalDigits valid restriction")?;
            facets.total_digits = held(n);
        }
        Facet::FractionDigits(n) => {
            narrower(facets.fraction_digits.as_ref(), (kind, n), &[], "§4.3.12.1")?;
            facets.fraction_digits = held(n);
        }
        Facet::WhiteSpace(w) => white_space(facets, w, s.fixed)?,
        Facet::Pattern(_)
        | Facet::Enumeration(_)
        | Facet::MaxInclusive(_)
        | Facet::MaxExclusive(_)
        | Facet::MinExclusive(_)
        | Facet::MinInclusive(_) => {}
    }
    Ok(())
}

/// Whether the count `n` of facet `kind` may replace `slot`, the base's:
/// not if the base's is fixed and other, nor if it is in an order of
/// `wider` to the base's, by `rule`.
fn narrower(slot: Option<&Held<u64>>, (kind, n): (FacetKind, u64), wider: &[Ordering], rule: &str) -> Result<()> {
    let Some(parent) = slot else { return Ok(()) };
    if parent.fixed && parent.value != n {
        return Err(fixed(kind));
    }
    if wider.contains(&n.cmp(&parent.value)) {
        return Err(invalid(rule, &format!("{kind} {n} is wider than the base type's {}", parent.value)));
    }
    Ok(())
}

/// [Schema2 §4.3.6.1, §4.3.6.4 whiteSpace valid restriction] The
/// whiteSpace `w` merged into `facets`: not other than a fixed one, nor
/// preserve or replace over collapse, nor preserve over replace.
fn white_space(facets: &mut Facets, w: WhiteSpace, fixed_now: bool) -> Result<()> {
    if let Some(parent) = &facets.white_space {
        if parent.fixed && parent.value != w {
            return Err(fixed(FacetKind::WhiteSpace));
        }
        if w < parent.value {
            return Err(invalid(
                "§4.3.6.4 whiteSpace valid restriction",
                &format!("{w:?} is less than the base type's {:?}", parent.value),
            ));
        }
    }
    facets.white_space = Some(Held { value: w, fixed: fixed_now });
    Ok(())
}

/// [Schema2 §4.3.1.4, §4.3.2.4, §4.3.7.4 to §4.3.10.4, §4.3.12.4] The
/// facets of one type agree: length not with minLength or maxLength;
/// minLength no more than maxLength; each lower bound below or at each
/// upper one; fractionDigits no more than totalDigits.
fn agree(facets: &Facets) -> Result<()> {
    if facets.length.is_some() && (facets.min_length.is_some() || facets.max_length.is_some()) {
        return Err(invalid("§4.3.1.4 length and minLength or maxLength", "both are facets of the type"));
    }
    let value = |slot: &Option<Held<u64>>| slot.as_ref().map(|h| h.value);
    if let (Some(least), Some(most)) = (value(&facets.min_length), value(&facets.max_length))
        && least > most
    {
        return Err(invalid("§4.3.2.4 minLength <= maxLength", &format!("{least} is more than {most}")));
    }
    for (lower, upper, wrong, rule) in ORDERED {
        let order = facets.bound(lower).zip(facets.bound(upper)).and_then(|(l, u)| compare(&l.value, &u.value));
        if order.is_some_and(|o| wrong.contains(&o)) {
            return Err(invalid(rule, &format!("{lower} is not below {upper}")));
        }
    }
    if let (Some(fraction), Some(total)) = (value(&facets.fraction_digits), value(&facets.total_digits))
        && fraction > total
    {
        return Err(invalid("§4.3.12.4 fractionDigits less than or equal to totalDigits", &format!("{fraction} is more than {total}")));
    }
    Ok(())
}
