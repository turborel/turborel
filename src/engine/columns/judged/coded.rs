//! \[DESIGN\] (RULES.md D360) A conjunct of a column of texts coded by a
//! table of symbols (`fsst.rs`) judged in place, no text decoded, as
//! Boncz, Neumann and Leis, "FSST" (PVLDB 2020), §5, compare a constant
//! with compressed strings. FSST encodes the constant with the same table
//! and compares the code strings, which is exact only for strings compared
//! octet by octet; SQL compares strings padded with spaces to one length
//! [X3.135-1986 §5.11 GR5], so `'a'` equals `'a '`, whose codes need not
//! begin with `'a'`'s. Each text's symbols are compared instead with the
//! constant's bytes as they are read, the first that differs deciding
//! (`Symbols::compare_padded`), which orders as `compare_strings` does and
//! reads, of most texts, a code or two. A constant compared unpadded, as
//! a collation of NO PAD that orders strings as their octets do compares
//! (RULES.md D1183), is compared the same way with no pad
//! (`Symbols::compare_unpadded`), by no dictionary's order and no sketch,
//! which are of the padded order.
//!
//! A chunk of a dictionary in order is judged by its codes instead, of a
//! column of any character string type (`sorted.rs`). Of a group with a
//! sketch of the column, a row of a chunk coded by symbols is kept or
//! left by its sketch's code first, and only a row of a boundary code
//! compared (`sketched.rs`, RULES.md D525), unless few of a run's rows
//! are asked for.
//!
//! \[DESIGN\] (RULES.md D1251) A LIKE or NOT LIKE of a constant pattern
//! of no `_` is judged in place too (`liked`). FSST gives no way to
//! match a pattern on codes, as a piece of it is coded in as many ways as
//! there are octets before it. The pattern's machine over octets
//! (`value/like/machine.rs`) is stepped instead, a code at a time, by a
//! table of the state after each state and code made once for the
//! pattern and the group's table of symbols (`fsst/stepped.rs`). LIKE
//! compares no string padded [ISO/IEC 9075-2:2023 §8.5 SR3d], and a
//! pattern a scan is told is one whose characters match as themselves,
//! so the octets a VARCHAR's codes give are what the pattern is matched
//! with. A chunk of a VARCHAR column not coded by symbols keeps its
//! texts as they were written, each alone, once for the chunk or as a
//! dictionary's entries, and the pattern's pieces are searched for in
//! those octets where they lie (D1074), so a LIKE is judged of every
//! chunk of the column, whatever its encoding. The rows of a chunk whose
//! codes are malformed where they are read are all kept, and decoding
//! them raises what is malformed.

use std::cell::Cell;

use super::super::encoded::Body;
use super::super::fsst::Stepped;
use super::super::group::{Group, Seal, Zone};
use super::super::scan::Reading;
use super::super::sketch::{Verdict, Verdicts};
use super::super::{Chunk, padding};
use super::{Against, KEPT_SHARE, Pred, sketched, sorted, text_keeps};
use crate::error::Result;
use crate::storage::Liked;
use crate::storage::mask::index;
use crate::value::DataType;
use std::cmp::Ordering;

/// The rows of `given`, places from `from`, or of all of the run `from` to
/// `to` of `group`, that `pred` keeps, of a column of type `t`, if every
/// chunk the run reads of it is coded by symbols or a dictionary in order
/// and the conjunct is judged of text (`text_keeps`): none if not, the
/// column then decoded.
///
/// # Errors
///
/// As decoding a chunk.
pub(super) fn kept<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (pred, t): (&Pred, &DataType),
    (from, to): (u32, u32),
    given: Option<&[u32]>,
) -> Result<Option<Vec<u32>>> {
    if padding(t).is_none() || text_keeps(&pred.conjunct, &|_| Some(Ordering::Equal)).is_none() {
        return Ok(None);
    }
    // Symbols of VARCHAR alone, never padded, as the table codes no other.
    let symbols = padding(t) == Some(None);
    let Some(zones) = group.zones(pred.column)? else { return Ok(None) };
    let all = super::rows(usize::try_from(to.saturating_sub(from)).unwrap_or(0), given);
    let dense = all.len().saturating_mul(SPARSE) >= usize::try_from(to.saturating_sub(from)).unwrap_or(usize::MAX);
    let sketch = if symbols && dense { sketch_of(group, (pages, &mut *prepared), pred, (from, to))? } else { None };
    let sketch = sketch.as_ref().map(|(codes, verdicts)| (codes.as_slice(), verdicts));
    let mut out = Vec::with_capacity(all.len());
    for (zone, these) in parted(zones, &all, (from, to)) {
        let first = zone.run.first;
        let judged = prepared.with(pages, (pred.column, zone, t), |chunk, _| {
            Ok(sorted::judged(chunk, pred, (first, from), these, &mut out)
                .or_else(|| symbols.then(|| judged(chunk, (pred, sketch), (first, from), these, &mut out)).flatten()))
        })?;
        if judged.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

/// Each chunk of `zones` the run `from` to `to` reads, with the rows of
/// `all`, places from `from`, ascending, that lie in it, but a chunk
/// none of them lies in.
fn parted<'z, 'r>(zones: &'z [Zone], all: &'r [u32], (from, to): (u32, u32)) -> Vec<(&'z Zone, &'r [u32])> {
    let mut rest = all;
    let mut out = Vec::new();
    for zone in zones.iter().filter(|z| z.run.first < to && z.run.first.saturating_add(z.run.rows) > from) {
        let end = zone.run.first.saturating_add(zone.run.rows).saturating_sub(from);
        let (these, after) = rest.split_at(rest.partition_point(|&r| r < end));
        rest = after;
        if !these.is_empty() {
            out.push((zone, these));
        }
    }
    out
}

/// \[DESIGN\] (RULES.md D1251) The rows of `given`, places from `from`,
/// or of all of the run `from` to `to` of `group`, that the LIKE `like`
/// of `pred` keeps, of a VARCHAR column of type `t`, each text matched
/// where it lies, none decoded, a chunk at a time. The rows of a chunk
/// it cannot judge so are all kept, for the filter to judge, and so are
/// the rows of the chunks left once the run is seen to be one given
/// whole (`whole_by`). None, of a column of another type, as a
/// CHARACTER string is padded as it is read, which no chunk holds.
///
/// # Errors
///
/// As reading a chunk.
pub(super) fn liked<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    (pred, like, t): (&Pred, &Liked, &DataType),
    (from, to): (u32, u32),
    given: Option<&[u32]>,
) -> Result<Option<Vec<u32>>> {
    if padding(t) != Some(None) {
        return Ok(None);
    }
    let Some(zones) = group.zones(pred.column)? else { return Ok(None) };
    let rows = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    let all = super::rows(rows, given);
    // The steps made are kept by the chunks opened, taken while a chunk
    // is read of them and put back whatever reading gives.
    let mut stepped = std::mem::take(&mut prepared.stepped);
    let (mut out, mut asked, mut read) = (Vec::new(), 0_usize, Ok(()));
    let mut chunks = parted(zones, &all, (from, to)).into_iter();
    for (zone, these) in chunks.by_ref() {
        let (first, before) = (zone.run.first, out.len());
        let judged = prepared.with(pages, (pred.column, zone, t), |chunk, _| {
            Ok(liked_of(chunk, (pred.at, like, &mut stepped), (first, from), these, &mut out))
        });
        match judged {
            Ok(judged) => or_all(judged, these, (before, &mut out)),
            Err(e) => {
                read = Err(e);
                break;
            }
        }
        asked = asked.saturating_add(these.len());
        if whole_by((out.len(), asked), (all.len(), rows)) {
            break;
        }
    }
    // The chunks left when the run was seen to be given whole.
    chunks.for_each(|(_, these)| out.extend_from_slice(these));
    prepared.stepped = stepped;
    read.map(|()| Some(out))
}

/// \[DESIGN\] (RULES.md D1251) Whether a run of `rows` rows, of which
/// `all` are asked, is seen to be one given whole, so the rows not yet
/// judged are kept unjudged: `kept` of the first `asked` of them were
/// kept. A run is given whole, and the filter judges every row of it
/// again, once one row in `KEPT_SHARE` of it is kept, so judging more of
/// such a run is work twice done. The rate kept so far, carried to all
/// the rows asked, tells it, once at least one in `KEPT_SHARE` of the
/// rows asked are judged: of fewer, even every one kept leaves the run
/// short of that share.
fn whole_by((kept, asked): (usize, usize), (all, rows): (usize, usize)) -> bool {
    let sampled = asked.saturating_mul(KEPT_SHARE) >= all;
    sampled && kept.saturating_mul(all).saturating_mul(KEPT_SHARE) >= asked.saturating_mul(rows)
}

/// What judging a chunk gave of `these`, pushed to `out` past its first
/// `before` rows: as it is, if the chunk was judged; else every row of
/// `these` in the place of those pushed before the chunk gave way, as
/// the filter judges what the scan does not.
fn or_all(judged: Option<()>, these: &[u32], (before, out): (usize, &mut Vec<u32>)) {
    if judged.is_none() {
        out.truncate(before);
        out.extend_from_slice(these);
    }
}

/// Pushes to `out` the rows of `these`, places from `from`, that `like`,
/// the LIKE at place `at` of the scan's condition, keeps of `chunk`,
/// whose first row is `first`: none of a null, of which it is unknown
/// [X3.135-1986 §5.14 GR4]. Texts coded by symbols are matched by the
/// pattern's machine stepped over their codes, and texts kept as they
/// were written, each alone, once for the chunk or as a dictionary's
/// entries, by the pattern's pieces searched for in their octets. None
/// if the chunk holds no texts, or a text's codes or place are
/// malformed, so it is decoded and raises what is malformed as it would
/// have.
fn liked_of(
    chunk: &Chunk<'_>,
    (at, like, stepped): (usize, &Liked, &mut Stepped),
    (first, from): (u32, u32),
    these: &[u32],
    out: &mut Vec<u32>,
) -> Option<()> {
    let by_steps = match &chunk.body {
        Body::Fsst(texts) => Some((texts, stepped.of(at, texts.symbols(), like.machine())?)),
        Body::Fixed { .. } | Body::Varying { .. } | Body::Constant { .. } | Body::Dictionary { .. } => None,
        // Of numbers.
        Body::Frame { .. } | Body::Runs(_) | Body::Alp(_) => return None,
    };
    for &r in these {
        let n = index(from.saturating_add(r).saturating_sub(first));
        if chunk.is_null(n) {
            continue;
        }
        let kept = match by_steps {
            Some((texts, steps)) => texts.liked(n, steps)? != like.not(),
            None => like.keeps(chunk.body.whole_field(n)?),
        };
        if kept {
            out.push(r);
        }
    }
    Some(())
}

/// \[DESIGN\] A run's rows are judged by their sketch's codes only if at
/// least one of each `SPARSE` of them is asked for. Of fewer, reading the
/// codes and judging each code of the group costs more than comparing the
/// texts of the rows asked for (RULES.md D525).
const SPARSE: usize = 8;

/// The codes of the sketch of `pred`'s column of the rows `from` to `to`
/// of `group`, and what each says of it, made once a group; none if the
/// group has no sketch of the column or the conjunct is not judged by one.
///
/// # Errors
///
/// As `Pages::with`.
fn sketch_of<S: Seal>(
    group: &Group,
    (pages, prepared): Reading<'_, '_, S>,
    pred: &Pred,
    (from, to): (u32, u32),
) -> Result<Option<(Vec<u8>, Verdicts)>> {
    let Some(edges) = group.sketches.get(pred.column).filter(|e| !e.is_empty()) else { return Ok(None) };
    let Some(verdicts) = prepared.sketches.verdicts(pred.at, || sketched::verdicts(edges, pred)) else { return Ok(None) };
    Ok(prepared.sketches.codes(group, pages, pred.column, (from, to))?.map(|codes| (codes, verdicts)))
}

/// Pushes to `out` the rows of `these`, places from `from`, that `pred`
/// keeps of `chunk`, whose first row is `first`, by the codes of their
/// sketch first, if given, each code as `sketch` says: none if it is not
/// coded by symbols, or a text's codes are malformed, so it is decoded
/// and raises what is malformed as it would have.
fn judged(
    chunk: &Chunk<'_>,
    (pred, sketch): (&Pred, Option<(&[u8], &Verdicts)>),
    (first, from): (u32, u32),
    these: &[u32],
    out: &mut Vec<u32>,
) -> Option<()> {
    let Body::Fsst(coded) = &chunk.body else { return None };
    let failed = Cell::new(false);
    for &r in these {
        if let Some((codes, verdicts)) = sketch {
            match codes.get(index(r)).map_or(Verdict::Check, |&code| verdicts.of(code)) {
                Verdict::Keep => {
                    out.push(r);
                    continue;
                }
                Verdict::Drop => continue,
                Verdict::Check => {}
            }
        }
        let n = index(from.saturating_add(r).saturating_sub(first));
        if chunk.is_null(n) {
            if pred.of_null {
                out.push(r);
            }
            continue;
        }
        let order = |c: Against<'_>| {
            let o = match c {
                Against::Padded(text) => coded.compare_padded(n, text.as_bytes()),
                Against::Unpadded(octets) => coded.compare_unpadded(n, octets),
            };
            failed.set(failed.get() || o.is_none());
            o
        };
        let keeps = text_keeps(&pred.conjunct, &order)?;
        if failed.get() {
            return None;
        }
        if keeps {
            out.push(r);
        }
    }
    Some(())
}

// Unit tests: a LIKE judged of a group's texts in place, and where it
// gives way (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
