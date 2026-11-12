// Unit tests: values issued as §9.35 says, and reservations (RULES.md
// R-TEST-1, R-TEST-2).

use std::cell::RefCell;

use super::*;
use crate::db::sequences::DEFAULT_TYPE;

/// A generator of `increment` within `minimum` to `maximum`, at `origin`,
/// cycling if `cycle`.
fn seq((minimum, maximum): (i128, i128), increment: i128, origin: i128, cycle: bool) -> SequenceDesc {
    let name = QualifiedName { auth: "A".into(), ident: "S".into() };
    SequenceDesc { name, data_type: DEFAULT_TYPE, start: origin, minimum, maximum, increment, cycle, epoch: 1, origin, usage: Vec::new() }
}

/// The next `n` values of `s`, drawn with no log.
fn draw(counters: &Counters, s: &SequenceDesc, n: usize) -> Vec<i128> {
    (0..n).map(|_| counters.next(s, None).unwrap()).collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.35 GR3] a generator issues its origin,
/// then each value an increment on, ascending or descending, within its
/// bounds; one of CYCLE then its minimum, ascending, or its maximum,
/// descending \[GR3b\]; one of NO CYCLE raises SQLSTATE 2200H, each time
/// \[GR3a\].
#[test]
fn values_issued_9_35() {
    assert_eq!(draw(&Counters::default(), &seq((1, 10), 4, 1, true), 4), [1, 5, 9, 1]);
    assert_eq!(draw(&Counters::default(), &seq((-10, -1), -4, -2, true), 4), [-2, -6, -10, -1]);
    let counters = Counters::default();
    let once = seq((1, 3), 1, 2, false);
    assert_eq!(draw(&counters, &once, 2), [2, 3]);
    for _ in 0..2_u8 {
        let e = counters.next(&once, None).unwrap_err();
        assert_eq!((e.code, e.sqlstate()), (Code::Data, SqlState::SEQUENCE_GENERATOR_LIMIT_EXCEEDED));
    }
    assert!(exhausted(4, &once) && !exhausted(3, &once));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.35 GR3] a base before the cycle, as an
/// ALTER raising the minimum of an ascending generator leaves it, issues
/// the first of it plus a multiple of the increment within the bounds; a
/// cycle no such value is in issues none, of NO CYCLE.
#[test]
fn a_base_before_the_cycle_9_35() {
    let ascending = seq((10, 20), 3, 1, false);
    assert_eq!(first_in_cycle(1, &ascending), Some(10), "1 + 3 * 3");
    assert_eq!(first_in_cycle(2, &ascending), Some(11));
    assert_eq!(first_in_cycle(21, &ascending), None);
    let descending = seq((-20, -10), -3, -1, false);
    assert_eq!(first_in_cycle(-1, &descending), Some(-10));
    assert_eq!(first_in_cycle(-21, &descending), None);
    let skipped = seq((5, 6), 10, 1, false);
    assert_eq!(first_in_cycle(1, &skipped), None, "1 and 11 are both outside 5 to 6");
    assert_eq!(first_in_cycle(i128::MAX, &seq((1, i128::MAX), 1, 1, false)), Some(i128::MAX));
}

/// Verifies: [DESIGN] RULES.md D62, the reservations of a counter: the first
/// draw logs where to go on, `FIRST_RESERVED` values on, before issuing a
/// value; the next draws of the block log nothing; the next block is twice
/// as long, up to `MOST_RESERVED`.
#[test]
fn reservations_double_to_a_limit() {
    let counters = Counters::default();
    let s = seq((1, i128::MAX), 1, 1, false);
    let logged = RefCell::new(Vec::new());
    let log = |resume: i128| -> Result<()> {
        logged.borrow_mut().push(resume);
        Ok(())
    };
    let first = counters.next(&s, Some(&log)).unwrap();
    assert_eq!((first, logged.borrow().clone()), (1, vec![33]));
    for _ in 1..32_u8 {
        counters.next(&s, Some(&log)).unwrap();
    }
    assert_eq!(logged.borrow().len(), 1, "the block drawn from");
    assert_eq!(counters.next(&s, Some(&log)).unwrap(), 33);
    assert_eq!(logged.borrow().last(), Some(&(33 + 64)));
    let mut drawn = 33;
    while logged.borrow().len() < 8 {
        drawn = counters.next(&s, Some(&log)).unwrap();
    }
    let blocks: Vec<i128> = logged.borrow().windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(blocks, [64, 128, 256, 512, 1024, 1024, 1024]);
    assert_eq!(logged.borrow().last().copied(), Some(drawn + 1024));
}

/// Verifies: [DESIGN] RULES.md D62: a reservation's log failing issues no
/// value, its error given; the next draw reserves again, and issues the
/// value the failed one would have.
#[test]
fn a_failed_reservation_issues_nothing() {
    let counters = Counters::default();
    let s = seq((1, 100), 1, 1, false);
    let failing = |_: i128| -> Result<()> { Err(SqlError::new(Code::Api, "the log failed")) };
    assert_eq!(counters.next(&s, Some(&failing)).map_err(|e| e.code), Err(Code::Api));
    let ok = |_: i128| -> Result<()> { Ok(()) };
    assert_eq!(counters.next(&s, Some(&ok)).unwrap(), 1);
}

/// Verifies: [DESIGN] RULES.md D62, a reservation logged near a cycle's end:
/// where to go on is after the last value issued, so the generator is
/// exhausted once recovered from it; of CYCLE, past the new cycle's start.
#[test]
fn a_reservation_reaches_no_further_than_its_values() {
    let once = seq((1, 5), 2, 1, false);
    assert_eq!(reach(1, &once, 32), 7, "1, 3, 5 issued");
    assert!(exhausted(7, &once));
    let cycling = seq((1, 5), 2, 1, true);
    assert_eq!(reach(1, &cycling, 4), 3, "1, 3, 5, then 1 again");
}

/// Verifies: [DESIGN] RULES.md D62: settling every counter for a checkpoint
/// gives each base, by key, and the last epoch; it drops what each
/// reserved, so the next draw logs anew, a first block again.
#[test]
fn a_settled_counter_reserves_anew() {
    let counters = Counters::default();
    let epoch = counters.epoch().unwrap();
    let s = SequenceDesc { epoch, ..seq((1, 1000), 1, 1, false) };
    let logged = RefCell::new(Vec::new());
    let log = |resume: i128| -> Result<()> {
        logged.borrow_mut().push(resume);
        Ok(())
    };
    for _ in 0..40_u8 {
        counters.next(&s, Some(&log)).unwrap();
    }
    assert_eq!(logged.borrow().clone(), [33, 33 + 64]);
    assert_eq!(counters.settle().unwrap(), (epoch, vec![(s.name.clone(), epoch, 41)]));
    assert_eq!(counters.next(&s, Some(&log)).unwrap(), 41);
    assert_eq!(logged.borrow().last(), Some(&(41 + 32)));
}

/// Verifies: [DESIGN] RULES.md D62, the turn of a counter: a block whose
/// record is being logged as the counter is settled, or set by a record
/// replayed, is not taken, as the record may be before the checkpoint's
/// start; the draw reserves again, logging a second record.
#[test]
fn a_block_reserved_across_a_settling_is_not_taken() {
    let counters = Counters::default();
    let s = seq((1, 1000), 1, 1, false);
    let logged = RefCell::new(Vec::new());
    let log = |resume: i128| -> Result<()> {
        if logged.borrow().is_empty() {
            counters.settle().unwrap();
        }
        logged.borrow_mut().push(resume);
        Ok(())
    };
    assert_eq!(counters.next(&s, Some(&log)).unwrap(), 1);
    assert_eq!(logged.borrow().clone(), [33, 33], "the first block given up");
    let replaying = |resume: i128| -> Result<()> {
        if logged.borrow().len() == 2 {
            counters.replay(&s.name, s.epoch, 500).unwrap();
        }
        logged.borrow_mut().push(resume);
        Ok(())
    };
    for _ in 1..32_u8 {
        counters.next(&s, Some(&replaying)).unwrap();
    }
    assert_eq!(counters.next(&s, Some(&replaying)).unwrap(), 500, "the replayed base, its block reserved again");
    assert_eq!(logged.borrow().clone(), [33, 33, 33 + 64, 500 + 64]);
}

/// Verifies: [DESIGN] RULES.md D62: a counter replayed is at the base its
/// record holds, made if it was not, and moves the epochs past its own;
/// epochs given after are later; pruning keeps only the counters kept.
#[test]
fn counters_replayed_and_pruned() {
    let counters = Counters::default();
    let s = SequenceDesc { epoch: 9, ..seq((1, 1000), 1, 1, false) };
    counters.replay(&s.name, 9, 70).unwrap();
    assert_eq!(counters.base(&s).unwrap(), 70);
    assert_eq!(counters.epoch().unwrap(), 10);
    counters.advance_past(20).unwrap();
    assert_eq!(counters.epoch().unwrap(), 21);
    let other = SequenceDesc { epoch: 21, ..s.clone() };
    assert_eq!(counters.base(&other).unwrap(), 1, "none drawn: its origin");
    counters.next(&other, None).unwrap();
    counters.prune(&|_, epoch| epoch == 21).unwrap();
    assert_eq!(counters.base(&s).unwrap(), 1, "pruned: its origin again");
    assert_eq!(counters.base(&other).unwrap(), 2);
}
