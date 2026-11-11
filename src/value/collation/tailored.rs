//! \[EXTENSION\] The collations `UNICODE_<LOCALE>`: UNICODE tailored for a
//! language by CLDR 38's rules for it \[UTS #35 Part 5\], so `UNICODE_SV`
//! sorts `å`, `ä` and `ö` after `z`, and `UNICODE_ES` puts `ñ` after `n`.
//!
//! The rules are in `cldr38.rules`, which `cargo xtask cldr` makes, and
//! tailor UNICODE itself, not CLDR's root collation, which orders currency
//! symbols, length marks and a few Cyrillic letters otherwise
//! (CONFORMANCE.md). A tailoring is made the first time a key is asked of
//! it (`build.rs`), and maps each string a rule names to its elements; any
//! other string has UNICODE's.
//!
//! The elements are 32 bits a level, so a key holds three bytes for each
//! of UNICODE's weights and five for a tailored one: the upper 16 bits, then
//! a byte that is zero if the lower are, or one and the lower 16. That
//! orders as the weights do, and no weight begins as the zero bytes
//! between levels do.

mod build;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use super::rules::{LEVELS, parse};
use super::table::{Element, Table, VERSION};
use super::{Collation, Pad, Unicode};
use crate::error::Result;
use build::{WIDE_SHIFT, build};

/// The tailorings' rules, generated, each after a line `@ NAME LOCALE TYPE`.
const RULES: &str = include_str!("cldr38.rules");
/// The digest of the rules as generated (`cargo xtask cldr`).
#[cfg(test)]
const RULES_DIGEST: u64 = 0xec7f_788e_6f95_939d;
/// What begins each tailoring's line.
const HEADER: &str = "\n@ ";
/// The CLDR release the rules are of.
const CLDR: &str = "CLDR 38";
/// \[UTS #10 §7.3\] The third-level weights of upper case.
const UPPER: [u16; 6] = [0x0008, 0x0009, 0x000A, 0x000B, 0x000C, 0x001D];
/// Under `[caseFirst upper]`, the flag that puts a third-level weight not of
/// upper case after all that are.
const NOT_UPPER: u32 = 1 << 31;
/// The fourth-level weight of what is not variable \[UTS #10 §4.4\].
const MAXIMUM: u32 = u32::MAX;
/// The lower 16 bits of a weight.
const LOW: u32 = 0xFFFF;
/// What follows a weight's upper 16 bits: none, or the lower.
const NARROW: u8 = 0;
const WIDE: u8 = 1;
/// What separates two levels of a key.
const SEPARATOR: [u8; 2] = [0, 0];
/// The levels of a key.
const KEY_LEVELS: usize = 4;

/// A collation element of 32-bit weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wide {
    weights: [u32; LEVELS],
    variable: bool,
    /// Whether it is of upper case, for `[caseFirst upper]`.
    upper: bool,
}

/// A tailoring, made.
pub(super) struct Built {
    map: HashMap<Vec<char>, Vec<Wide>>,
    longest: usize,
    upper_first: bool,
    backwards: bool,
}

impl Built {
    /// The characters a string the rules name begins with.
    fn starts(&self) -> HashSet<char> {
        self.map.keys().filter_map(|k| k.first().copied()).collect()
    }
}

/// A made tailoring and the characters its strings begin with.
struct Made {
    built: Built,
    starts: HashSet<char>,
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
        let (name, locale, kind) = (words.next()?, words.next()?, words.next()?);
        let version = format!("{VERSION} {CLDR} {locale} {kind}");
        Some(Tailored { name, version, rules, unicode: Unicode::new(Arc::clone(table)), made: OnceLock::new() })
    };
    RULES.split(HEADER).skip(1).filter_map(tailoring).collect()
}

impl Tailored {
    /// The tailoring its rules make.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they cannot be read or made.
    pub(super) fn build(&self) -> Result<Built> {
        build(&self.unicode, &parse(self.rules)?)
    }

    /// The tailoring, made once. Rules that cannot be made, which the tests
    /// of this module rule out, tailor nothing.
    fn made(&self) -> &Made {
        self.made.get_or_init(|| {
            let built = self.build().unwrap_or_else(|_| Built { map: HashMap::new(), longest: 0, upper_first: false, backwards: false });
            Made { starts: built.starts(), built }
        })
    }

    /// The elements of `chars`.
    fn elements(&self, mut chars: Vec<char>) -> Vec<Wide> {
        let Made { built, starts } = self.made();
        let mut out = Vec::with_capacity(chars.len());
        let mut base = Vec::new();
        let mut at = 0;
        while let Some(c) = chars.get(at) {
            if starts.contains(c)
                && let Some((found, next)) = matched(&built.map, built.longest, &self.unicode, &mut chars, at)
            {
                out.extend(found);
                at = next;
                continue;
            }
            base.clear();
            at = self.unicode.step(&mut chars, at, &mut base);
            out.extend(base.iter().copied().map(widened));
        }
        out
    }
}

/// Whether third-level weight `t` is of upper case.
fn is_upper(t: u16) -> bool {
    UPPER.contains(&t)
}

/// UNICODE's element `e`, widened.
fn widened(e: Element) -> Wide {
    let [p, s, t] = e.weights;
    let wide = |w: u16| u32::from(w) << WIDE_SHIFT;
    Wide { weights: [wide(p), wide(s), wide(t)], variable: e.variable, upper: is_upper(t) }
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

/// \[UTS #10 §4.4\] `elements`' four levels under Shifted, a third-level
/// weight not of upper case flagged if `upper_first`.
fn levels(elements: &[Wide], upper_first: bool) -> [Vec<u32>; KEY_LEVELS] {
    let mut levels: [Vec<u32>; KEY_LEVELS] = Default::default();
    let mut after_variable = false;
    for e in elements {
        let [l1, l2, mut l3] = e.weights;
        if upper_first && l3 != 0 && !e.upper {
            l3 |= NOT_UPPER;
        }
        let ignored = l1 == 0 && after_variable;
        let fourth = match (e.variable, l1 == 0, l2 == 0 && l3 == 0) {
            (true, _, _) => l1,
            (false, _, true) => 0,
            (false, true, false) if ignored => 0,
            (false, _, false) => MAXIMUM,
        };
        if !e.variable && !ignored {
            let [first, second, third, _] = &mut levels;
            for (level, weight) in [(first, l1), (second, l2), (third, l3)] {
                if weight != 0 {
                    level.push(weight);
                }
            }
        }
        if fourth != 0
            && let Some(level) = levels.get_mut(LEVELS)
        {
            level.push(fourth);
        }
        if e.variable {
            after_variable = true;
        } else if l1 != 0 {
            after_variable = false;
        }
    }
    levels
}

/// Weight `w`, added to `out` as the key holds it.
fn put(w: u32, out: &mut Vec<u8>) {
    let high = u16::try_from(w >> WIDE_SHIFT).unwrap_or(u16::MAX);
    let low = u16::try_from(w & LOW).unwrap_or(0);
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

    /// \[UTS #10 S3\] Each level's weights, the zero bytes between levels;
    /// the second's from the end under `[backwards 2]` \[UTS #10 §3.6\].
    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>) {
        let text = match pad {
            Pad::Space => text.trim_end_matches(' '),
            Pad::None => text,
        };
        let built = &self.made().built;
        let mut levels = levels(&self.elements(self.unicode.normalized(text)), built.upper_first);
        if built.backwards
            && let Some(second) = levels.get_mut(1)
        {
            second.reverse();
        }
        for (n, level) in levels.iter().enumerate() {
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
}

#[cfg(test)]
mod tests;
