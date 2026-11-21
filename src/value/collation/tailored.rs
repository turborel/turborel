//! \[EXTENSION\] The collations `UNICODE_<LOCALE>`: UNICODE tailored for a
//! language by CLDR 38's rules for it \[UTS #35 Part 5\], so `UNICODE_SV`
//! sorts `å`, `ä` and `ö` after `z`, `UNICODE_ES` puts `ñ` after `n`, and
//! `UNICODE_EL` puts Greek before Latin.
//!
//! The rules are in `cldr38.rules`, which `cargo xtask cldr` makes, a
//! collation for each type of each locale but those for search, and an
//! import names a section of the same file. They tailor UNICODE itself,
//! not CLDR's root collation, which orders currency symbols, length marks,
//! a few Cyrillic letters and the digits of scripts otherwise
//! (CONFORMANCE.md). A tailoring is made the first time a key is asked of
//! it (`build.rs`), and maps each string a rule names to its elements; any
//! other string has UNICODE's, its primaries moved as `[reorder]` says
//! (`reorder.rs`).
//!
//! The elements are 64 bits a level, so a key holds three bytes for each
//! of UNICODE's weights and seven for a tailored one: the upper 32 bits'
//! lower 16, then a byte that is zero if the lower 32 are, or one and the
//! lower 32. That orders as the weights do, and no weight begins as the
//! zero bytes between levels do.

mod build;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use super::reorder::Reorder;
use super::rules::{LEVELS, QUATERNARY, Rules, damaged, parse};
use super::table::{Element, Table, VERSION};
use super::units::{Unit, split, weighed};
use super::{Collation, Pad, Unicode};
use crate::error::Result;
use build::{WIDE_SHIFT, build};

/// The tailorings' rules, generated, each after a line `@ NAME LOCALE TYPE`.
const RULES: &str = include_str!("cldr38.rules");
/// The digest of the rules as generated (`cargo xtask cldr`).
#[cfg(test)]
const RULES_DIGEST: u64 = 0xaef4_8267_106b_2eed;
/// What begins each tailoring's line, and the name of rules only imported.
const HEADER: &str = "\n@ ";
const NO_NAME: &str = "-";
/// What a line importing a section's rules begins with, and how deep
/// imports may go.
const IMPORT: &str = "[import ";
const MOST_IMPORTS: usize = 4;
/// The CLDR release the rules are of, and the form of the keys made.
const CLDR: &str = "CLDR 38";
const KEYS: &str = "keys 2";
/// \[UTS #10 §7.3\] The third-level weights of upper case.
const UPPER: [u16; 6] = [0x0008, 0x0009, 0x000A, 0x000B, 0x000C, 0x001D];
/// Under `[caseFirst upper]`, the flag that puts a third-level weight not of
/// upper case after all that are: the top bit of what the key holds of the
/// upper 32 bits.
const NOT_UPPER: u64 = 1 << 47;
/// The fourth-level weight of what is not variable \[UTS #10 §4.4\]:
/// above every variable weight, tailored ones among them, which are below
/// the digits, and held in three bytes.
const MAXIMUM: u64 = 0xFFFF << WIDE_SHIFT;
/// The bits of a weight the key holds first, and the lower 32.
const HIGH: u64 = 0xFFFF;
const LOW: u64 = 0xFFFF_FFFF;
/// What follows a weight's upper bits: none, or the lower.
const NARROW: u8 = 0;
const WIDE: u8 = 1;
/// What separates two levels of a key.
const SEPARATOR: [u8; 2] = [0, 0];

/// Each string a tailoring names and its elements.
pub(super) type Strings = HashMap<Vec<char>, Vec<Wide>>;
/// Each string weighed otherwise after a prefix: the prefixes, and the
/// elements after each.
pub(super) type Prefixes<V> = HashMap<Vec<char>, Vec<(Vec<char>, Vec<V>)>>;

/// A collation element of 64-bit weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wide {
    weights: [u64; LEVELS],
    variable: bool,
    /// Whether it is of upper case, for `[caseFirst upper]`.
    upper: bool,
}

/// What a tailoring weighs a string it does not name by: UNICODE, no
/// contraction beginning with a character `[suppressContractions]` names
/// made, its primaries moved as `[reorder]` says.
pub(super) struct Base {
    unicode: Unicode,
    suppressed: HashSet<char>,
    reorder: Option<Reorder>,
}

impl Base {
    /// The base `rules` name.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if their reordering names no group.
    fn new(unicode: Unicode, rules: &Rules) -> Result<Base> {
        Ok(Base { unicode, suppressed: rules.suppressed.iter().copied().collect(), reorder: Reorder::new(&rules.reorder)? })
    }

    /// \[UTS #10 S2\] The elements of the longest match at `at`, added to
    /// `out`, and where the next starts.
    fn step(&self, chars: &mut Vec<char>, at: usize, out: &mut Vec<Element>) -> usize {
        match chars.get(at) {
            Some(c) if self.suppressed.contains(c) => {
                self.unicode.alone(*c, out);
                at.saturating_add(1)
            }
            _ => self.unicode.step(chars, at, out),
        }
    }

    /// Where UNICODE's primary `p` now is.
    fn primary(&self, p: u16) -> u16 {
        self.reorder.as_ref().map_or(p, |r| r.primary(p))
    }

    /// UNICODE's element `e`, its primary moved: not that of the second of
    /// an implicit weight's two elements, which is ignorable at the second
    /// level and is read only after the first.
    fn moved(&self, e: Element) -> Element {
        let [p, s, t] = e.weights;
        let p = if s == 0 { p } else { self.primary(p) };
        Element { weights: [p, s, t], variable: e.variable }
    }

    /// UNICODE's element `e`, its primary moved, widened.
    fn widened(&self, e: Element) -> Wide {
        let [p, s, t] = self.moved(e).weights;
        let wide = |w: u16| u64::from(w) << WIDE_SHIFT;
        Wide { weights: [wide(p), wide(s), wide(t)], variable: e.variable, upper: is_upper(t) }
    }
}

/// A tailoring, made.
pub(super) struct Built {
    map: Strings,
    prefixed: Prefixes<Wide>,
    longest: usize,
    upper_first: bool,
    backwards: bool,
    /// The levels a key holds.
    strength: usize,
    base: Base,
}

impl Built {
    /// The characters a string the rules name begins with.
    fn starts(&self) -> HashSet<char> {
        self.map.keys().chain(self.prefixed.keys()).filter_map(|k| k.first().copied()).collect()
    }

    /// The characters a match made before them may take in or weigh by: of
    /// the strings the rules name, each after the first, and each of a
    /// string weighed after a prefix and of its prefixes.
    fn continuing(&self) -> HashSet<char> {
        let after_first = self.map.keys().flat_map(|k| k.iter().skip(1));
        let prefixed = self.prefixed.iter().flat_map(|(k, list)| k.iter().chain(list.iter().flat_map(|(p, _)| p.iter())));
        after_first.chain(prefixed).copied().collect()
    }
}

/// A made tailoring, the characters its strings begin with, and those
/// `Built::continuing` gives.
struct Made {
    built: Built,
    starts: HashSet<char>,
    continuing: HashSet<char>,
}

/// A collation `UNICODE_<LOCALE>`.
pub(super) struct Tailored {
    name: &'static str,
    version: String,
    rules: &'static str,
    unicode: Unicode,
    made: OnceLock<Made>,
}

/// The tailorings `RULES` holds, of `table`.
pub(super) fn all(table: &Arc<Table>) -> Vec<Tailored> {
    let tailoring = |part: &'static str| {
        let (head, rules) = part.split_once('\n')?;
        let mut words = head.split_whitespace();
        let (name, locale, kind) = (words.next().filter(|n| *n != NO_NAME)?, words.next()?, words.next()?);
        let version = format!("{VERSION} {CLDR} {locale} {kind} {KEYS}");
        Some(Tailored { name, version, rules, unicode: Unicode::new(Arc::clone(table)), made: OnceLock::new() })
    };
    RULES.split(HEADER).skip(1).filter_map(tailoring).collect()
}

/// The rules of the section of `locale` and `kind`.
fn section(locale: &str, kind: &str) -> Option<&'static str> {
    RULES.split(HEADER).skip(1).find_map(|part| {
        let (head, rules) = part.split_once('\n')?;
        let mut words = head.split_whitespace().skip(1);
        (words.next() == Some(locale) && words.next() == Some(kind)).then_some(rules)
    })
}

/// `rules`, each import replaced by the rules it names, `depth` imports
/// deep.
///
/// # Errors
///
/// SQLCODE -916 if an import names no section, or imports go too deep.
fn resolved(rules: &str, depth: usize) -> Result<String> {
    if depth > MOST_IMPORTS {
        return Err(damaged("imports too deep"));
    }
    let mut out = String::with_capacity(rules.len());
    for line in rules.lines() {
        if let Some(named) = line.strip_prefix(IMPORT).and_then(|l| l.strip_suffix(']')) {
            let (locale, kind) = named.split_once(' ').unwrap_or((named, ""));
            let imported = section(locale, kind).ok_or_else(|| damaged(&format!("an import of {named}")))?;
            out.push_str(&resolved(imported, depth.saturating_add(1))?);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

impl Tailored {
    /// The tailoring its rules make.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they cannot be read or made.
    pub(super) fn build(&self) -> Result<Built> {
        let rules = parse(&resolved(self.rules, 0)?)?;
        build(Base::new(self.unicode.clone(), &rules)?, &rules)
    }

    /// The tailoring, made once. Rules that cannot be made, which the tests
    /// of this module rule out, tailor nothing.
    fn made(&self) -> &Made {
        self.made.get_or_init(|| {
            let built = self.build().unwrap_or_else(|_| Built {
                map: HashMap::new(),
                prefixed: HashMap::new(),
                longest: 0,
                upper_first: false,
                backwards: false,
                strength: QUATERNARY,
                base: Base { unicode: self.unicode.clone(), suppressed: HashSet::new(), reorder: None },
            });
            Made { starts: built.starts(), continuing: built.continuing(), built }
        })
    }

    /// Whether a string may be cut before `c`: UNICODE's `Unicode::cut_before`
    /// allows it, no string the rules name begins with or holds the
    /// starter `c` begins with, nor weighs by it, and that starter's first
    /// element, its own whatever follows, has a first-level weight, which
    /// sets Shifted's state (`levels`) whatever it was before.
    fn cuts_before(&self, c: char) -> bool {
        let Made { built, starts, continuing } = self.made();
        let Some(first) = built.base.unicode.cut_before(c) else { return false };
        !starts.contains(&first) && !continuing.contains(&first) && self.elements(vec![first]).first().is_some_and(|e| e.weights[0] != 0)
    }

    /// The elements of `chars`.
    fn elements(&self, mut chars: Vec<char>) -> Vec<Wide> {
        let mut out = Vec::with_capacity(chars.len());
        let mut at = 0;
        while at < chars.len() {
            at = self.step(&mut chars, at, &mut out);
        }
        out
    }

    /// The elements of the string the rules name, or of UNICODE's longest
    /// match, at `at` of `chars`, added to `out`, and where the next
    /// starts.
    fn step(&self, chars: &mut Vec<char>, at: usize, out: &mut Vec<Wide>) -> usize {
        let Made { built, starts, .. } = self.made();
        if chars.get(at).is_some_and(|c| starts.contains(c)) {
            if let Some((found, next)) = prefixed(&built.prefixed, built.longest, chars, at) {
                out.extend_from_slice(found);
                return next;
            }
            if let Some((found, next)) = matched(&built.map, built.longest, &built.base.unicode, chars, at) {
                out.extend(found);
                return next;
            }
        }
        let mut base = Vec::new();
        let next = built.base.step(chars, at, &mut base);
        out.extend(base.iter().map(|e| built.base.widened(*e)));
        next
    }
}

/// Whether third-level weight `t` is of upper case.
fn is_upper(t: u16) -> bool {
    UPPER.contains(&t)
}

/// \[UTS #35 Part 5 §3.9\] What `prefixed` holds of the longest string at
/// `at` that follows one of its prefixes, the longest prefix first; and
/// where the next starts.
fn prefixed<'p>(prefixed: &'p Prefixes<Wide>, longest: usize, chars: &[char], at: usize) -> Option<(&'p [Wide], usize)> {
    if prefixed.is_empty() {
        return None;
    }
    let before = chars.get(..at)?;
    (1..=longest).rev().find_map(|length| {
        let end = at.checked_add(length)?;
        let list = prefixed.get(chars.get(at..end)?)?;
        let found = list.iter().filter(|(p, _)| before.ends_with(p)).max_by_key(|(p, _)| p.len())?;
        Some((found.1.as_slice(), end))
    })
}

/// \[UTS #10 S2.1\] What `map` holds of the longest string at `at`, and
/// what S2.1.1 adds to it of the unblocked non-starters after it, which are
/// taken out of `chars`; and where the next starts.
fn matched<V: Clone>(
    map: &HashMap<Vec<char>, Vec<V>>,
    longest: usize,
    unicode: &Unicode,
    chars: &mut Vec<char>,
    at: usize,
) -> Option<(Vec<V>, usize)> {
    let mut length = longest.min(chars.len().saturating_sub(at));
    let mut found = None;
    while length > 0 && found.is_none() {
        found = at.checked_add(length).and_then(|end| chars.get(at..end)).and_then(|s| map.get(s));
        if found.is_none() {
            length = length.saturating_sub(1);
        }
    }
    let after = at.saturating_add(length.max(1));
    let mut matched: Vec<char> = chars.get(at..after)?.to_vec();
    let (mut next, mut greatest) = (after, 0);
    while let Some(&c) = chars.get(next) {
        let class = unicode.class(c);
        if class == 0 {
            break;
        }
        if greatest < class && matched.len() < longest {
            matched.push(c);
            if let Some(more) = map.get(&matched) {
                found = Some(more);
                chars.remove(next);
                continue;
            }
            matched.pop();
        }
        greatest = greatest.max(class);
        next = next.saturating_add(1);
    }
    found.map(|v| (v.clone(), after))
}

/// \[UTS #10 §4.4\] What each of `elements` adds to the four levels under
/// Shifted, 0 at a level it adds nothing to, a third-level weight not of
/// upper case flagged if `upper_first`.
fn element_weights(elements: &[Wide], upper_first: bool) -> impl Iterator<Item = [u64; QUATERNARY]> + '_ {
    elements.iter().scan(false, move |after_variable, e| {
        let [l1, l2, mut l3] = e.weights;
        if upper_first && l3 != 0 && !e.upper {
            l3 |= NOT_UPPER;
        }
        let ignored = l1 == 0 && *after_variable;
        let fourth = match (e.variable, l1 == 0, l2 == 0 && l3 == 0) {
            (true, _, _) => l1,
            (false, _, true) => 0,
            (false, true, false) if ignored => 0,
            (false, _, false) => MAXIMUM,
        };
        let [first, second, third] = if e.variable || ignored { [0; 3] } else { [l1, l2, l3] };
        if e.variable {
            *after_variable = true;
        } else if l1 != 0 {
            *after_variable = false;
        }
        Some([first, second, third, fourth])
    })
}

/// \[UTS #10 §4.4\] `elements`' four levels under Shifted, a third-level
/// weight not of upper case flagged if `upper_first`.
fn levels(elements: &[Wide], upper_first: bool) -> [Vec<u64>; QUATERNARY] {
    let mut levels: [Vec<u64>; QUATERNARY] = Default::default();
    for weights in element_weights(elements, upper_first) {
        for (level, weight) in levels.iter_mut().zip(weights) {
            if weight != 0 {
                level.push(weight);
            }
        }
    }
    levels
}

/// Weight `w`, added to `out` as the key holds it.
fn put(w: u64, out: &mut Vec<u8>) {
    let high = u16::try_from((w >> WIDE_SHIFT) & HIGH).unwrap_or(u16::MAX);
    let low = u32::try_from(w & LOW).unwrap_or(0);
    out.extend_from_slice(&high.to_be_bytes());
    if low == 0 {
        out.push(NARROW);
    } else {
        out.push(WIDE);
        out.extend_from_slice(&low.to_be_bytes());
    }
}

impl Collation for Tailored {
    fn name(&self) -> &str {
        self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    /// \[UTS #10 S3\] Each level's weights up to `[strength n]`, the zero
    /// bytes between levels; the second's from the end under
    /// `[backwards 2]` \[UTS #10 §3.6\].
    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>) {
        let text = match pad {
            Pad::Space => text.trim_end_matches(' '),
            Pad::None => text,
        };
        let built = &self.made().built;
        let mut levels = levels(&self.elements(built.base.unicode.normalized(text)), built.upper_first);
        if built.backwards
            && let Some(second) = levels.get_mut(1)
        {
            second.reverse();
        }
        for (n, level) in levels.iter().enumerate().take(built.strength) {
            if n > 0 {
                out.extend_from_slice(&SEPARATOR);
            }
            for w in level {
                put(*w, out);
            }
        }
    }

    /// Characters may contract or expand, so none compares alone.
    fn fold(&self, _: char) -> Option<char> {
        None
    }

    /// A variable character weighs only if the fourth level is compared.
    fn weighs(&self, c: char) -> bool {
        let fourth = self.made().built.strength >= QUATERNARY;
        self.elements(vec![c]).iter().any(|e| e.weights[0] != 0 && (fourth || !e.variable))
    }

    /// Those up to `[strength n]`.
    fn levels(&self) -> usize {
        self.made().built.strength
    }

    /// \[UTS #10 S3\] The weights `sort_key` holds of `level` under PAD
    /// NONE, as it holds them, each weight's bytes saying how many follow
    /// and none beginning as the separator does, so two keys are equal
    /// exactly when each level's weights are. Under `[backwards 2]` the
    /// second level is given forward: the key holds the reverse of the
    /// whole string's, and two sequences' reverses are equal exactly when
    /// they are, so equality is the same, and forward weights of pieces
    /// join as those of the whole do.
    fn level_weights(&self, text: &str, level: usize, out: &mut Vec<u8>) {
        let built = &self.made().built;
        if level > built.strength {
            return;
        }
        let all = levels(&self.elements(built.base.unicode.normalized(text)), built.upper_first);
        for w in level.checked_sub(1).and_then(|n| all.get(n)).into_iter().flatten() {
            put(*w, out);
        }
    }

    /// The last character of `text` that `cuts_before` allows a cut before:
    /// the elements of the whole are those of the pieces, joined, and each
    /// level of them the levels of the pieces, joined, `[caseFirst upper]`
    /// flagging each element alone.
    fn safe_cut(&self, text: &str) -> Option<usize> {
        text.char_indices().rev().find(|&(_, c)| self.cuts_before(c)).map(|(at, _)| at)
    }

    /// Each character its steps begin at whose normalized form begins
    /// with a starter, and what follows up to the next (`units.rs`), each
    /// level's weights forward, as `level_weights` gives them, up to
    /// `[strength n]`.
    fn units(&self, text: &str) -> Option<Vec<Unit>> {
        let built = &self.made().built;
        let (chars, begins) = built.base.unicode.normalized_marked(text);
        let (elements, starts) = split(chars, begins, |chars, at, out| self.step(chars, at, out));
        let per: Vec<[u64; QUATERNARY]> = element_weights(&elements, built.upper_first).collect();
        Some(weighed(&starts, &per, built.strength, put))
    }
}

#[cfg(test)]
mod tests;
