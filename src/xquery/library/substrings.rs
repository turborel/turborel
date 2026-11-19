//! The functions based on substring matching [XQueryFO §7.5]:
//! `fn:contains`, `fn:starts-with`, `fn:ends-with`, `fn:substring-before`
//! and `fn:substring-after`, each by the collation units of a collation
//! (`collation.rs`): of the codepoint collation and of one that compares a
//! character at a time, a character each; of `UNICODE` and its tailorings,
//! a character and what may not be cut from it, so a contraction or a
//! character's marks.
//!
//! \[DESIGN\] RULES.md D106: a minimal match [XQueryFO §7.5.1] of the
//! units sought is a run of the text's units whose weights at each level,
//! joined, are those of the units sought, beginning and ending at a unit
//! that is not ignorable, so no unit at either end could be left out. A
//! run's weights are its units', joined, as a unit is weighed alone, so a
//! run is matched unit by unit, each level's weights so far a beginning of
//! the sought's. The characters a match covers are its units', so a
//! substring before or after one never splits a unit.

use super::super::collation::Units;
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, boolean, collator, string_or_empty, string_value};
use crate::error::Result;
use crate::xml::Sequence;

/// The functions of [XQueryFO §7.5].
pub(super) const FUNCTIONS: [Builtin; 5] = [
    Builtin::function("contains", 2, 3, contains),
    Builtin::function("starts-with", 2, 3, starts_with),
    Builtin::function("ends-with", 2, 3, ends_with),
    Builtin::function("substring-before", 2, 3, substring_before),
    Builtin::function("substring-after", 2, 3, substring_after),
];

/// The two strings of `args`, the empty sequence each the zero-length
/// string, their collation units by the collation of `args`'s third or
/// the default, and the first's characters.
struct Operands {
    text: Vec<char>,
    units: Units,
    sought: Units,
}

impl Operands {
    /// Those of `args` in `focus`.
    fn of(focus: &Context<'_>, args: &[Sequence]) -> Result<Operands> {
        let collator = collator(focus, args, 2)?;
        let text = string_or_empty(arg(args, 0))?;
        let sought = string_or_empty(arg(args, 1))?;
        Ok(Operands { units: collator.units(&text)?, sought: collator.units(&sought)?, text: text.chars().collect() })
    }

    /// Whether the string sought is the zero-length string, or holds only
    /// ignorable units, which is taken as it [XQueryFO §7.5.1].
    fn nothing_sought(&self) -> bool {
        (0..self.sought.len()).all(|u| self.sought.ignorable(u))
    }

    /// The text's units that are not ignorable, in order.
    fn weighed(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        (0..self.units.len()).filter(|&u| !self.units.ignorable(u))
    }

    /// The last unit of the minimal match of the units sought beginning at
    /// the text's unit `first`, if one does: the units from it taken while
    /// each level's weights so far begin the sought's, the match ending
    /// at the first that makes them all the sought's. None of an
    /// ignorable unit, which no minimal match begins with.
    fn match_from(&self, first: usize) -> Option<usize> {
        if self.units.ignorable(first) {
            return None;
        }
        let mut held = vec![0_usize; self.sought.levels()];
        for unit in first..self.units.len() {
            for (level, at) in held.iter_mut().enumerate() {
                let weights = self.units.weights(level, unit);
                let end = at.checked_add(weights.len())?;
                if self.sought.joined(level).get(*at..end)? != weights {
                    return None;
                }
                *at = end;
            }
            if held.iter().enumerate().all(|(level, &at)| at == self.sought.joined(level).len()) {
                return Some(unit);
            }
        }
        None
    }

    /// The first minimal match, the one beginning first: its first and
    /// last units.
    fn first(&self) -> Option<(usize, usize)> {
        self.weighed().find_map(|f| self.match_from(f).map(|last| (f, last)))
    }

    /// The text's characters from `from` to `to`.
    fn characters(&self, from: usize, to: usize) -> String {
        self.text.get(from..to).unwrap_or_default().iter().collect()
    }
}

/// [XQueryFO §7.5.1] `fn:contains($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:boolean`: whether the first
/// holds a minimal match of the units of the second; true of a second of
/// no units but ignorable ones.
///
/// # Errors
///
/// err:FOCH0002 of a collation not supported; err:FOCH0004 of one with no
/// collation units.
fn contains(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    Ok(boolean(o.nothing_sought() || o.first().is_some()))
}

/// [XQueryFO §7.5.2] `fn:starts-with($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:boolean`: whether a minimal
/// match begins at the first unit that is not ignorable.
///
/// # Errors
///
/// As `contains`.
fn starts_with(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    Ok(boolean(o.nothing_sought() || o.weighed().next().and_then(|f| o.match_from(f)).is_some()))
}

/// [XQueryFO §7.5.3] `fn:ends-with($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:boolean`: whether a minimal
/// match ends at the last unit that is not ignorable.
///
/// # Errors
///
/// As `contains`.
fn ends_with(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    let last = o.weighed().next_back();
    Ok(boolean(o.nothing_sought() || o.weighed().any(|f| o.match_from(f) == last)))
}

/// [XQueryFO §7.5.4] `fn:substring-before($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:string`: what precedes the
/// first minimal match of the second in the first; the zero-length string
/// if there is none, or the second has no units but ignorable ones.
///
/// # Errors
///
/// As `contains`.
fn substring_before(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    let before = o.first().filter(|_| !o.nothing_sought()).map(|(f, _)| o.units.characters(f).0);
    string_value(before.map_or_else(String::new, |at| o.characters(0, at)))
}

/// [XQueryFO §7.5.5] `fn:substring-after($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:string`: what follows the
/// first minimal match of the second in the first; the zero-length string
/// if there is none; the first if the second has no units but ignorable
/// ones, the zero-length string if the first has none but them too.
///
/// # Errors
///
/// As `contains`.
fn substring_after(focus: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    let after = if o.nothing_sought() { o.weighed().next().map(|_| 0) } else { o.first().map(|(_, last)| o.units.characters(last).1) };
    string_value(after.map_or_else(String::new, |at| o.characters(at, o.text.len())))
}

#[cfg(test)]
mod tests;
