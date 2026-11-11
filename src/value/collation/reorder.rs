//! \[EXTENSION\] Script reordering \[UTS #35 Part 5 §3.13\]. The option
//! `[reorder Grek]` puts Greek before the other scripts, as Greek
//! dictionaries do.
//!
//! The groups are CLDR's, laid over UNICODE's primaries in
//! `cldr38.groups`, which `cargo xtask cldr` makes. Each is a list of
//! ranges of primaries, and together they run from the first primary to
//! the last of the unified ideographs without a gap. A reordering moves
//! whole groups. The special groups not named stay first, then come those
//! named in the order named, the others where `others` is named or else
//! after them. A
//! primary is moved by the place its range now starts at, so the order
//! within a group stays. Primaries past the ideographs' stay where they
//! are.

use crate::error::Result;

use super::rules::damaged;

/// The groups, generated, a line for each range.
const GROUPS: &str = include_str!("cldr38.groups");
/// The groups before the letters.
const SPECIAL: [&str; 5] = ["space", "punct", "symbol", "currency", "digit"];
/// The codes standing for every group not named.
const OTHERS: [&str; 2] = ["others", "Zzzz"];
const HEX: u32 = 16;
/// The fields of a line: first, last, and the codes.
const FIELDS: usize = 3;

/// A group: its codes, and its ranges of primaries in order.
struct Group {
    codes: &'static str,
    ranges: Vec<(u16, u16)>,
}

impl Group {
    fn special(&self) -> bool {
        SPECIAL.contains(&self.codes)
    }

    fn named(&self, code: &str) -> bool {
        self.codes.split_whitespace().any(|c| c == code)
    }
}

/// The groups, in the order of their first range.
fn groups() -> Result<Vec<Group>> {
    let mut out: Vec<Group> = Vec::new();
    for line in GROUPS.lines().filter(|l| !l.starts_with('#')) {
        let mut fields = line.splitn(FIELDS, ' ');
        let weight =
            |field: Option<&str>| field.and_then(|f| u16::from_str_radix(f, HEX).ok()).ok_or_else(|| damaged("a reordering group"));
        let (first, last) = (weight(fields.next())?, weight(fields.next())?);
        let codes = fields.next().ok_or_else(|| damaged("a reordering group of no codes"))?;
        match out.iter_mut().find(|g| g.codes == codes) {
            Some(group) => group.ranges.push((first, last)),
            None => out.push(Group { codes, ranges: vec![(first, last)] }),
        }
    }
    Ok(out)
}

/// A range of primaries and where it now starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Moved {
    first: u16,
    last: u16,
    to: u16,
}

/// A reordering of the groups.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Reorder {
    /// Each range, in the order of `first`.
    moved: Vec<Moved>,
}

impl Reorder {
    /// The reordering `codes` name, none if it moves nothing.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a code names no group.
    pub(super) fn new(codes: &[String]) -> Result<Option<Reorder>> {
        let groups = groups()?;
        // Each group named, in order; `None` where the others go.
        let mut named: Vec<Option<usize>> = Vec::new();
        for code in codes {
            let item = if OTHERS.contains(&code.as_str()) {
                None
            } else {
                Some(groups.iter().position(|g| g.named(code)).ok_or_else(|| damaged(&format!("[reorder {code}]")))?)
            };
            if !named.contains(&item) {
                named.push(item);
            }
        }
        if !named.contains(&None) {
            named.push(None);
        }
        let order = order(&groups, &named);
        let mut to = groups.iter().flat_map(|g| g.ranges.iter().map(|r| r.0)).min().unwrap_or(0);
        let mut moved = Vec::new();
        for (first, last) in order {
            moved.push(Moved { first, last, to });
            to = last
                .checked_sub(first)
                .and_then(|l| to.checked_add(l))
                .and_then(|t| t.checked_add(1))
                .ok_or_else(|| damaged("the reordering groups overlap"))?;
        }
        moved.sort_by_key(|m| m.first);
        Ok(moved.iter().any(|m| m.to != m.first).then_some(Reorder { moved }))
    }

    /// Where primary `p` now is.
    pub(super) fn primary(&self, p: u16) -> u16 {
        let at = self.moved.partition_point(|m| m.last < p);
        match self.moved.get(at) {
            Some(m) if m.first <= p => p.wrapping_sub(m.first).wrapping_add(m.to),
            _ => p,
        }
    }
}

/// The ranges of `groups` in their new order: those of the special groups
/// not named in the order they had, then each group `named` names, `None`
/// the groups not named.
fn order(groups: &[Group], named: &[Option<usize>]) -> Vec<(u16, u16)> {
    let unnamed = |special: bool| {
        let not_named = move |g: &usize| !named.contains(&Some(*g));
        (0..groups.len()).filter(move |g| groups.get(*g).is_some_and(|g| g.special() == special)).filter(not_named)
    };
    let ranges = |g: usize| groups.get(g).map(|g| g.ranges.clone()).unwrap_or_default();
    let mut out: Vec<(u16, u16)> = unnamed(true).flat_map(ranges).collect();
    out.sort_unstable();
    for item in named {
        match item {
            Some(g) => out.extend(ranges(*g)),
            None => out.extend(unnamed(false).flat_map(ranges)),
        }
    }
    out
}

/// The first primary of the group primary `p` is in, if it is in one.
///
/// # Errors
///
/// SQLCODE -916 if the groups cannot be read.
pub(super) fn group_first(p: u16) -> Result<Option<u16>> {
    let groups = groups()?;
    let group = groups.iter().find(|g| g.ranges.iter().any(|(first, last)| (*first..=*last).contains(&p)));
    Ok(group.and_then(|g| g.ranges.iter().map(|r| r.0).min()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn first(code: &str) -> u16 {
        groups().unwrap().iter().find(|g| g.named(code)).unwrap().ranges[0].0
    }

    /// Verifies: [EXTENSION] RULES.md D60: the groups' ranges run from the
    /// first to the last without a gap or overlap, and every script group
    /// is one range.
    #[test]
    fn the_groups_cover_the_primaries_once() {
        let groups = groups().unwrap();
        let mut ranges: Vec<(u16, u16)> = groups.iter().flat_map(|g| g.ranges.iter().copied()).collect();
        ranges.sort_unstable();
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].1.checked_add(1), Some(pair[1].0), "{pair:?}");
        }
        assert!(groups.iter().filter(|g| !g.special()).all(|g| g.ranges.len() == 1));
        assert_eq!(ranges.last().map(|r| r.1), Some(0xFBBF));
    }

    /// Verifies: [EXTENSION] RULES.md D60: a group named goes first after
    /// the special groups, its order within kept; the others follow, or
    /// go where `others` is named; a special group named moves too; a
    /// reordering that moves nothing is none, and an unknown code is
    /// refused.
    #[test]
    fn named_groups_go_first() {
        let greek = Reorder::new(&codes("Grek")).unwrap().unwrap();
        let (latin, alpha) = (first("Latn"), first("Grek"));
        assert!(greek.primary(alpha) < greek.primary(latin));
        assert!(greek.primary(alpha) < greek.primary(alpha.wrapping_add(1)));
        assert!(greek.primary(first("digit")) < greek.primary(alpha));
        assert_eq!(greek.primary(0xFBC0), 0xFBC0);
        let after = Reorder::new(&codes("others digit")).unwrap().unwrap();
        assert!(after.primary(first("Hani")) < after.primary(first("digit")));
        assert!(after.primary(first("currency")) < after.primary(latin));
        let han = Reorder::new(&codes("Hani Bopo")).unwrap().unwrap();
        assert!(han.primary(0xFB40) < han.primary(first("Bopo")) && han.primary(first("Bopo")) < han.primary(latin));
        assert_eq!(Reorder::new(&codes("others")).unwrap(), None);
        assert_eq!(Reorder::new(&codes("Latn")).unwrap(), None);
        assert!(Reorder::new(&codes("Xxxx")).is_err());
        assert_eq!(group_first(alpha.wrapping_add(3)).unwrap(), Some(alpha));
        assert_eq!(group_first(0xFFFD).unwrap(), None);
    }
}
