//! \[DESIGN\] (RULES.md D1300) A partition's groups adopted where they
//! lie. A partition of a grouping run in parallel merges its morsels'
//! groups into a table of its own (`made.rs`), each new group's key, first
//! row's number and states copied out of the morsel's groups handed over.
//! Past the groups a cache holds, that copy and the slots it is found by
//! are most of what a group costs to merge. A wide partition copies
//! nothing of a new group. It keeps the Mades it was handed, as they are
//! and shared as they were, and of each group its hash and where the
//! group lies, the Made and the place of its first row. The key and the
//! first row's number are read there.
//!
//! A group that takes a second row cannot merge it where the group lies,
//! as a Made is never written once its morsel's thread has made it, which
//! is what lets any thread read it. The first time a row meets a group
//! held already the group is taken up into a table of the partition's own,
//! key, first row and states, and that row and every later one merge
//! there (`wide/own.rs`). A group of one row has no state but the Made's.
//!
//! \[DESIGN\] (RULES.md D1301) The groups are listed by sub-part, a
//! sub-part named by bits of a group's hash, and a wave's rows are looked
//! up a sub-part at a time in slots that fit a cache (`wide/part.rs`).
//! The rows of a wave are first put in the order of their sub-parts, each
//! with its hash and where it lies, in one pass that reads the hashes in
//! the order the wave holds them. Each sub-part has a table of its own
//! for its groups of several rows.
//!
//! A wave's Mades come in the morsels' order and a Made's rows of a
//! partition in the morsel's, and the rows of a sub-part keep that order,
//! so the first row of a group to be looked up is its first row, and a
//! sub-part's groups are listed in the order of their first rows. A group
//! taken up is given the states of its first row and then its later
//! rows' in the order they come, the order a table merges them in, so
//! each set function's state is the one a table holds, of a function
//! whose state depends on that order too.

use std::sync::Arc;

use super::super::{GroupTable, Seed, narrow};
use super::Made;
use crate::error::Result;
use crate::exec::broken;
use crate::logical::AggCall;
use crate::storage::mask::index;
use part::{Handed, Part, Pass, bits_for, sub_of};

mod own;
mod part;
mod read;
pub(in crate::exec) use read::Lying;

/// The segment of a group that lies in the table of its sub-part's own.
const OWN: u32 = u32::MAX;

/// A Made a partition adopted: the Made, let go once none of the
/// partition's groups lies in it any more, and the bytes of it the
/// partition's rows hold.
struct Segment {
    made: Option<Arc<Made>>,
    bytes: usize,
}

/// The Made of segment `segment`, if any group still lies in it.
fn made_of(segments: &[Segment], segment: u32) -> Option<&Made> {
    segments.get(index(segment)).and_then(|held| held.made.as_deref())
}

/// A partition's run of a wave's Made: the Made and the places of the
/// partition's rows in it, in their order.
pub(in crate::exec) type Run<'a> = (&'a Arc<Made>, &'a [u32]);

/// A partition's groups, adopted where they lie.
pub(in crate::exec) struct Wide {
    /// The Mades its groups of one row lie in, in the order handed.
    segments: Vec<Segment>,
    /// How many groups lie in each segment.
    lying: Vec<usize>,
    /// Its groups by sub-part, as many sub-parts as `bits` bits name.
    parts: Vec<Part<GroupTable>>,
    bits: u32,
    /// Whether its sub-parts place their groups by keyed hashes, once a
    /// probe grew long (RULES.md D357).
    keyed: bool,
    aggs: Vec<AggCall>,
    seed: Seed,
    width: usize,
    scratch: Vec<u8>,
}

/// A wide partition between waves, as it goes to another thread: each
/// sub-part's table of its own frozen. It holds no slots.
pub(in crate::exec) struct Adopted {
    segments: Vec<Segment>,
    lying: Vec<usize>,
    parts: Vec<Part<Made>>,
    bits: u32,
    keyed: bool,
}

impl Adopted {
    /// The bytes it holds, its share of the Mades it adopted among them.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let segments = self.segments.iter().map(|segment| segment.bytes).sum::<usize>();
        let parts = self.parts.iter().map(|part| part.listed_bytes().saturating_add(part.own.bytes())).sum::<usize>();
        segments.saturating_add(parts).saturating_add(self.lying.capacity().saturating_mul(size_of::<usize>()))
    }
}

impl Wide {
    /// The groups of `table`, a partition's that grew past what a cache
    /// holds, as a wide partition's: the table frozen is its first
    /// segment and every group lies where the table held it, all in one
    /// sub-part until a wave is merged.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn of(table: GroupTable) -> Result<Wide> {
        let (aggs, seed, width) = (table.aggs.clone(), table.seed.clone(), table.keys.width());
        let mut made = table.frozen()?;
        let keyed = made.keyed.take();
        let (hashes, bytes, by_keys) = (made.hashes.clone(), made.bytes(), keyed.is_some());
        let mut part = Part::new(GroupTable::of_own(width, &aggs, &seed));
        part.lies = (0..hashes.len()).map(|g| (0, narrow(g))).collect();
        (part.hashes, part.keyed) = (hashes, keyed.unwrap_or_default());
        Ok(Wide {
            lying: vec![part.hashes.len()],
            segments: vec![Segment { made: Some(Arc::new(made)), bytes }],
            parts: vec![part],
            bits: 0,
            keyed: by_keys,
            aggs,
            seed,
            width,
            scratch: Vec::new(),
        })
    }

    /// How many groups.
    pub(in crate::exec) fn len(&self) -> usize {
        self.parts.iter().map(|part| part.hashes.len()).sum()
    }

    /// Whether it places its groups by keyed hashes (RULES.md D357).
    #[cfg(test)]
    pub(in crate::exec) fn is_keyed(&self) -> bool {
        self.keyed
    }

    /// How many sub-parts its groups are listed in.
    #[cfg(test)]
    pub(in crate::exec) fn sub_parts(&self) -> usize {
        self.parts.len()
    }

    /// Lets go of each Made that none of its groups lies in any more, and
    /// of its bytes.
    fn let_go(&mut self) {
        for (segment, _) in self.segments.iter_mut().zip(&self.lying).filter(|(_, lying)| **lying == 0) {
            (segment.made, segment.bytes) = (None, 0);
        }
    }

    /// \[DESIGN\] (RULES.md D1301) Lists its groups in the sub-parts that
    /// `bits` bits name, if those are more than it has: each sub-part
    /// split among those the further bits name, in its order.
    fn parted(&mut self, bits: u32) -> Result<()> {
        let more = bits.saturating_sub(self.bits);
        if more == 0 {
            return Ok(());
        }
        let parts = std::mem::take(&mut self.parts);
        for part in parts {
            self.parts.extend(part.split((bits, more), &self.seed)?);
        }
        self.bits = bits;
        Ok(())
    }

    /// \[DESIGN\] (RULES.md D1301) The rows of `wave` in the order of
    /// their sub-parts, `handed` in all, each with its hash and where it
    /// lies, the Mades' segments numbered from `base` on, and where each
    /// sub-part's rows begin, with the last's end: counted, then placed,
    /// so the rows of a sub-part keep the wave's order.
    fn scattered(&self, wave: &[Run<'_>], (base, handed): (usize, usize)) -> (Vec<Handed>, Vec<usize>) {
        let hash_at = |made: &Made, place: u32| made.hashes.get(index(place)).copied().unwrap_or(0);
        let mut starts = vec![0_usize; self.parts.len().saturating_add(1)];
        for &(made, places) in wave {
            for &place in places {
                if let Some(count) = starts.get_mut(sub_of(hash_at(made, place), self.bits).saturating_add(1)) {
                    *count = count.saturating_add(1);
                }
            }
        }
        let mut sum = 0_usize;
        for start in &mut starts {
            sum = sum.saturating_add(*start);
            *start = sum;
        }
        let (mut rows, mut next): (Vec<Handed>, Vec<usize>) = (vec![(0, 0, 0); handed], starts.clone());
        for (segment, &(made, places)) in (base..).zip(wave) {
            for &place in places {
                let hash = hash_at(made, place);
                let Some(at) = next.get_mut(sub_of(hash, self.bits)) else { continue };
                if let Some(row) = rows.get_mut(*at) {
                    *row = (hash, narrow(segment), place);
                }
                *at = at.saturating_add(1);
            }
        }
        (rows, starts)
    }

    /// \[DESIGN\] (RULES.md D1300, D1301) Merges the runs of `wave`, a
    /// partition's of each of a wave's Mades, in their order. The Mades
    /// are adopted, its groups listed in sub-parts enough that one's
    /// groups and rows are `sub` or so, and the rows looked up a sub-part
    /// at a time (`Part::looked_up`). A Made none of whose groups lies in
    /// it after that is let go. Gives how many of the rows were new
    /// groups.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(in crate::exec) fn absorb_wave(&mut self, wave: &[Run<'_>], sub: usize) -> Result<usize> {
        let (before, base) = (self.len(), self.segments.len());
        let handed: usize = wave.iter().map(|(_, places)| places.len()).sum();
        for &(made, places) in wave {
            let bytes = made.bytes().saturating_mul(places.len()).checked_div(made.len()).unwrap_or(0);
            self.segments.push(Segment { made: Some(Arc::clone(made)), bytes });
            self.lying.push(0);
        }
        self.parted(bits_for(before.saturating_add(handed), sub).max(self.bits))?;
        let (rows, starts) = self.scattered(wave, (base, handed));
        let Wide { parts, segments, lying, keyed, seed, scratch, .. } = self;
        let mut pass = Pass { segments: segments.as_slice(), lying: lying.as_mut_slice(), keyed, seed, scratch };
        for (part, bounds) in parts.iter_mut().zip(starts.windows(2)) {
            let (Some(&from), Some(&to)) = (bounds.first(), bounds.last()) else { continue };
            part.looked_up(rows.get(from..to).unwrap_or(&[]), &mut pass)?;
        }
        self.let_go();
        Ok(self.len().saturating_sub(before))
    }

    /// As it goes to another thread between waves: each sub-part's table
    /// of its own frozen.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn frozen(self) -> Result<Adopted> {
        let parts = self.parts.into_iter().map(|part| part.with(GroupTable::frozen)).collect::<Result<Vec<_>>>()?;
        Ok(Adopted { segments: self.segments, lying: self.lying, parts, bits: self.bits, keyed: self.keyed })
    }

    /// The wide partition `adopted` was, taken up again: each sub-part's
    /// table of its own thawed, of the set functions `aggs`, its keys
    /// hashed by `seed`. No group is placed until a wave is merged.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(in crate::exec) fn thawed(adopted: Adopted, aggs: &[AggCall], seed: &Seed) -> Result<Wide> {
        let width = adopted.parts.first().map_or(0, |part| part.own.keys.width());
        let thaw = |own: Made| GroupTable::thawed_unplaced(own, aggs, seed);
        Ok(Wide {
            parts: adopted.parts.into_iter().map(|part| part.with(thaw)).collect::<Result<Vec<_>>>()?,
            segments: adopted.segments,
            lying: adopted.lying,
            bits: adopted.bits,
            keyed: adopted.keyed,
            aggs: aggs.to_vec(),
            seed: seed.clone(),
            width,
            scratch: Vec::new(),
        })
    }

    /// Its groups gathered into one table, in the order of their first
    /// rows, for what reads a table: each group of one row taken up from
    /// where it lies, a Made's a set function at a time, and each group
    /// of several rows from its sub-part's own table. The sub-parts'
    /// lists are each in that order and are put in one by the first
    /// rows' numbers, read where the groups lie.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; SQLCODE -910 if an accumulator writes no
    /// state.
    pub(in crate::exec) fn into_table(self) -> Result<GroupTable> {
        let len = self.len();
        let mut table = GroupTable::unplaced(self.width, &self.aggs, &self.seed, len);
        table.apart = false;
        let (segments, mut order) = (self.segments, Vec::with_capacity(len));
        let mut owns = Vec::with_capacity(self.parts.len());
        for (k, part) in (0_u32..).zip(self.parts) {
            let own = part.own.frozen()?;
            for &(segment, place) in &part.lies {
                let (from, of) = if segment == OWN { (Some(&own), k) } else { (made_of(&segments, segment), 0) };
                let first = from.and_then(|made| made.firsts.get(index(place)).copied());
                order.push((first.ok_or_else(|| broken("a group lies where a wide partition holds none"))?, segment, of, place));
            }
            owns.push(own);
        }
        order.sort_unstable();
        taken_in_order(&mut table, &order, (&segments, &owns))?;
        table.placed_all();
        Ok(table)
    }
}

/// A group to gather: its first row's number, its segment, the sub-part
/// whose own table holds it if its segment is `OWN`, and its place.
type Gathered = (u64, u32, u32, u32);

/// Appends to `table` the groups of `order`, in that order, each run of
/// groups that lie in one Made, or in one sub-part's table of `owns`,
/// taken up at once.
fn taken_in_order(table: &mut GroupTable, order: &[Gathered], (segments, owns): (&[Segment], &[Made])) -> Result<()> {
    let (mut run, mut of): (Vec<u32>, Option<(u32, u32)>) = (Vec::new(), None);
    let taken = |into: &mut GroupTable, from: Option<(u32, u32)>, places: &[u32]| {
        let Some((segment, part)) = from else { return Ok(()) };
        let made = if segment == OWN { owns.get(index(part)) } else { made_of(segments, segment) };
        into.take_up(made.ok_or_else(|| broken("a segment a wide partition let go holds a group"))?, places)
    };
    for &(_, segment, part, place) in order {
        if of != Some((segment, part)) {
            taken(table, of, &run)?;
            run.clear();
            of = Some((segment, part));
        }
        run.push(place);
    }
    taken(table, of, &run)
}

// Unit tests: a wide partition holds the groups a table holds (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
