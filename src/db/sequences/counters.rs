//! \[DESIGN\] RULES.md D62: the counters of sequence generators. A counter
//! holds a generator's current base value, which no transaction controls
//! [ISO/IEC 9075-2:2023 §4.29.1], so it lives apart from the definitions,
//! shared by every snapshot. It is keyed by the generator's name and the
//! epoch its descriptor names. A definition gives a new epoch, and so does
//! each ALTER that restarts the generator, so a statement draws from the
//! counter of the descriptor it sees, and a counter is never taken for
//! another's.
//!
//! A value is issued only once a log record holds where to go on after a
//! crash. Each record reserves a block of values beyond the current base,
//! logged and synced before any of them is issued, with no lock held across
//! the log, a plug-in (RULES.md R-ERR-4). Those not issued when the system
//! fails are skipped, never issued twice. A block doubles with each
//! reservation, up to `MOST_RESERVED`, so a long run of draws syncs the log
//! rarely. A checkpoint settles every counter, recording its exact base and
//! dropping what it reserved, so the next draw reserves anew and is logged
//! after the checkpoint.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};

use crate::catalog::{QualifiedName, SequenceDesc};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::sync::lock;

/// The values the first block a counter reserves holds, and the first after
/// a checkpoint settles it.
const FIRST_RESERVED: u64 = 32;
/// The values a block holds at most: so many may be skipped by a crash.
const MOST_RESERVED: u64 = 1024;

/// A counter's key: its generator's name and epoch.
type Key = (QualifiedName, u64);

/// \[DESIGN\] The counters of a database's sequence generators.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    held: Mutex<Held>,
    /// Signalled as a reservation ends.
    reserved: Condvar,
}

/// What the counters hold, under their lock.
#[derive(Debug, Default)]
struct Held {
    /// The last epoch given.
    epochs: u64,
    by: HashMap<Key, Counter>,
}

/// One generator's counter in one epoch.
#[derive(Debug)]
struct Counter {
    /// The current base value: the next value issued, if it is in the
    /// cycle. Past the cycle's end, the generator is exhausted.
    base: i128,
    /// The values that may be issued before another block is reserved.
    left: u64,
    /// The values the next block holds.
    block: u64,
    /// A block is being logged.
    reserving: bool,
    /// Moved on by a settling or a replayed record, so a block reserved
    /// before it is not taken.
    turn: u64,
}

impl Counter {
    fn at(base: i128) -> Counter {
        Counter { base, left: 0, block: FIRST_RESERVED, reserving: false, turn: 0 }
    }
}

/// A counter as a checkpoint records it: its key and its base.
pub(crate) type Settled = (QualifiedName, u64, i128);

impl Counters {
    /// A new epoch, after every one given.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a thread panicked holding the counters.
    pub(crate) fn epoch(&self) -> Result<u64> {
        let mut held = lock(&self.held)?;
        held.epochs = held.epochs.saturating_add(1);
        Ok(held.epochs)
    }

    /// Notes `epoch` given, so none given later is it.
    ///
    /// # Errors
    ///
    /// As `epoch`.
    pub(crate) fn advance_past(&self, epoch: u64) -> Result<()> {
        let mut held = lock(&self.held)?;
        held.epochs = held.epochs.max(epoch);
        Ok(())
    }

    /// The current base value of `seq`'s counter: its origin if it has
    /// drawn none.
    ///
    /// # Errors
    ///
    /// As `epoch`.
    pub(crate) fn base(&self, seq: &SequenceDesc) -> Result<i128> {
        let held = lock(&self.held)?;
        Ok(held.by.get(&(seq.name.clone(), seq.epoch)).map_or(seq.origin, |c| c.base))
    }

    /// [ISO/IEC 9075-2:2023 §9.35] The next value of `seq`, from its
    /// counter, made at its origin if it has none. If none is reserved,
    /// `log` is given where a crash would go on from, before a value is
    /// issued; without one, every value is issued at once.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200H if its cycle is exhausted and it is of NO CYCLE
    /// \[GR3a\]; `log`'s.
    pub(crate) fn next(&self, seq: &SequenceDesc, log: Option<&dyn Fn(i128) -> Result<()>>) -> Result<i128> {
        let key = (seq.name.clone(), seq.epoch);
        let mut held = lock(&self.held)?;
        loop {
            let counter = held.by.entry(key.clone()).or_insert_with(|| Counter::at(seq.origin));
            let (value, after) = issue(counter.base, seq)?;
            let Some(log) = log.filter(|_| counter.left == 0) else {
                counter.base = after;
                counter.left = counter.left.saturating_sub(1);
                return Ok(value);
            };
            if counter.reserving {
                held = self.reserved.wait(held).map_err(|_| poisoned())?;
                continue;
            }
            let (resume, turn, block) = (reach(counter.base, seq, counter.block), counter.turn, counter.block);
            counter.reserving = true;
            drop(held);
            let logged = log(resume);
            held = lock(&self.held)?;
            self.reserved.notify_all();
            let Some(reserved) = held.by.get_mut(&key) else { continue };
            reserved.reserving = false;
            logged?;
            if reserved.turn == turn {
                reserved.left = block;
                reserved.block = block.saturating_mul(2).min(MOST_RESERVED);
            }
        }
    }

    /// Settles every counter for a checkpoint: what each reserved dropped,
    /// so its next value is logged anew. Its base, and the last epoch given.
    ///
    /// # Errors
    ///
    /// As `epoch`.
    pub(crate) fn settle(&self) -> Result<(u64, Vec<Settled>)> {
        let mut held = lock(&self.held)?;
        let mut settled = Vec::with_capacity(held.by.len());
        for ((name, epoch), counter) in &mut held.by {
            (counter.left, counter.block, counter.turn) = (0, FIRST_RESERVED, counter.turn.saturating_add(1));
            settled.push((name.clone(), *epoch, counter.base));
        }
        settled.sort();
        Ok((held.epochs, settled))
    }

    /// A counter set as a log record or an image holds it: at `base`, with
    /// nothing reserved.
    ///
    /// # Errors
    ///
    /// As `epoch`.
    pub(crate) fn replay(&self, name: &QualifiedName, epoch: u64, base: i128) -> Result<()> {
        let mut held = lock(&self.held)?;
        held.epochs = held.epochs.max(epoch);
        let counter = held.by.entry((name.clone(), epoch)).or_insert_with(|| Counter::at(base));
        (counter.base, counter.left, counter.turn) = (base, 0, counter.turn.saturating_add(1));
        Ok(())
    }

    /// Drops each counter but those of a key `kept` keeps.
    ///
    /// # Errors
    ///
    /// As `epoch`.
    pub(crate) fn prune(&self, kept: &dyn Fn(&QualifiedName, u64) -> bool) -> Result<()> {
        let mut held = lock(&self.held)?;
        held.by.retain(|(name, epoch), _| kept(name, *epoch));
        Ok(())
    }
}

/// The error of a lock whose holder panicked.
fn poisoned() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thread panicked while it held the counters of sequence generators (RULES.md D27)")
}

/// [ISO/IEC 9075-2:2023 §9.35 GR3] The value `seq` issues at current base
/// value `base`, and the base after it: the first of `base` plus a
/// multiple of the increment within its bounds; else, of CYCLE, the start
/// of a new cycle, its minimum ascending or its maximum descending.
///
/// # Errors
///
/// SQLSTATE 2200H, of NO CYCLE, if there is none \[GR3a\].
fn issue(base: i128, seq: &SequenceDesc) -> Result<(i128, i128)> {
    let value = match first_in_cycle(base, seq) {
        Some(value) => value,
        None if seq.cycle => {
            if seq.ascending() {
                seq.minimum
            } else {
                seq.maximum
            }
        }
        None => {
            return fail_with(
                Code::Data,
                SqlState::SEQUENCE_GENERATOR_LIMIT_EXCEEDED,
                format!(
                    "[ISO/IEC 9075-2:2023 §9.35 GR3a] sequence generator {} has issued every value of its cycle, and is of NO CYCLE",
                    seq.name
                ),
            );
        }
    };
    Ok((value, value.saturating_add(seq.increment)))
}

/// Whether `seq` has no value left in its cycle at current base value
/// `base`: the next is a new cycle's, or none.
pub(super) fn exhausted(base: i128, seq: &SequenceDesc) -> bool {
    first_in_cycle(base, seq).is_none()
}

/// The first of `base` plus a non-negative multiple of `seq`'s increment
/// within its bounds, if any is [ISO/IEC 9075-2:2023 §9.35 GR3].
fn first_in_cycle(base: i128, seq: &SequenceDesc) -> Option<i128> {
    let step = seq.increment.checked_abs()?;
    // How far before the cycle `base` is, the way the generator goes.
    let short = if seq.ascending() { seq.minimum.checked_sub(base)? } else { base.checked_sub(seq.maximum)? };
    let steps = if short > 0 { short.checked_add(step)?.checked_sub(1)?.checked_div(step)? } else { 0 };
    let value = base.checked_add(seq.increment.checked_mul(steps)?)?;
    (seq.minimum..=seq.maximum).contains(&value).then_some(value)
}

/// Where a crash would go on from once `block` values are issued from
/// `base`: the base after them, or after the last one issued before the
/// cycle is exhausted.
fn reach(base: i128, seq: &SequenceDesc, block: u64) -> i128 {
    let mut at = base;
    for _ in 0..block {
        match issue(at, seq) {
            Ok((_, after)) => at = after,
            Err(_) => break,
        }
    }
    at
}

// Unit tests: values issued as §9.35 says, and reservations (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
