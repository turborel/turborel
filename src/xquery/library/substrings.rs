//! The functions based on substring matching [XQueryFO §7.5]:
//! `fn:contains`, `fn:starts-with`, `fn:ends-with`, `fn:substring-before`
//! and `fn:substring-after`, each by the collation units of a collation
//! (`collation.rs`), one for each character, so the characters a match
//! covers are the units it matches.

use super::args::{arg, boolean, string, string_or_empty, string_value};
use super::collation::Collator;
use super::{Builtin, Focus};
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
    units: Vec<char>,
    sought: Vec<char>,
}

impl Operands {
    /// Those of `args` in `focus`.
    fn of(focus: &Focus, args: &[Sequence]) -> Result<Operands> {
        let given = args.get(2).map(string).transpose()?;
        let collator = Collator::of(focus, given.as_deref())?;
        let text = string_or_empty(arg(args, 0))?;
        let sought = string_or_empty(arg(args, 1))?;
        Ok(Operands { units: collator.units(&text)?, sought: collator.units(&sought)?, text: text.chars().collect() })
    }

    /// Where the units sought first occur in the text's, the first unit's
    /// place; none if they do not.
    fn first(&self) -> Option<usize> {
        if self.sought.is_empty() {
            return Some(0);
        }
        self.units.windows(self.sought.len()).position(|w| w == self.sought.as_slice())
    }

    /// The text's characters from `from` to `to`.
    fn characters(&self, from: usize, to: usize) -> String {
        self.text.get(from..to).unwrap_or_default().iter().collect()
    }
}

/// [XQueryFO §7.5.1] `fn:contains($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:boolean`: whether the first
/// holds the units of the second; true of a zero-length second.
///
/// # Errors
///
/// err:FOCH0002 of a collation not supported; err:FOCH0004 of one with no
/// collation units.
fn contains(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    Ok(boolean(Operands::of(focus, args)?.first().is_some()))
}

/// [XQueryFO §7.5.2] `fn:starts-with($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:boolean`.
///
/// # Errors
///
/// As `contains`.
fn starts_with(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    Ok(boolean(o.units.starts_with(&o.sought)))
}

/// [XQueryFO §7.5.3] `fn:ends-with($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:boolean`.
///
/// # Errors
///
/// As `contains`.
fn ends_with(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    Ok(boolean(o.units.ends_with(&o.sought)))
}

/// [XQueryFO §7.5.4] `fn:substring-before($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:string`: what precedes the
/// first occurrence of the second in the first; the zero-length string if
/// it does not occur, or is zero-length.
///
/// # Errors
///
/// As `contains`.
fn substring_before(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    string_value(o.first().map_or_else(String::new, |at| o.characters(0, at)))
}

/// [XQueryFO §7.5.5] `fn:substring-after($arg1 as xs:string?, $arg2 as
/// xs:string?, $collation as xs:string) as xs:string`: what follows the
/// first occurrence of the second in the first; the zero-length string if
/// it does not occur; the first if the second is zero-length.
///
/// # Errors
///
/// As `contains`.
fn substring_after(focus: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let o = Operands::of(focus, args)?;
    let after = o.first().map(|at| at.saturating_add(o.sought.len()));
    string_value(after.map_or_else(String::new, |at| o.characters(at, o.text.len())))
}

#[cfg(test)]
mod tests;
