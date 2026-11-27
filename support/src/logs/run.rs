//! The contract of a run of records appended together (RULES.md D882),
//! and of the bytes a log tells between its positions (D1231), checked of
//! every log with the rest of its contract (`logs::contract`).

use turborel::log::{Log, Lsn};

use super::{all, record};

/// The contract of `Log::append_run` (RULES.md D882): the records laid
/// end to end are appended in order, each at a position of its own past
/// the one before, and read back as `append` of each would be; an empty
/// run appends none.
pub(super) fn run_contract(log: &dyn Log) {
    let run: Vec<Vec<u8>> = (100..112).map(record).chain([record(3)]).collect();
    let ends: Vec<usize> = run.iter().scan(0, |end, r| Some(std::mem::replace(end, *end + r.len()) + r.len())).collect();
    let before = log.end();
    assert!(log.append_run(&[], &[]).unwrap().is_empty() && log.end() == before, "an empty run");
    let at = log.append_run(&run.concat(), &ends).unwrap();
    assert!(at.len() == run.len() && at[0] >= before && at.windows(2).all(|w| w[0] < w[1]), "a position each, in order");
    let lens: Vec<usize> = run.iter().map(Vec::len).collect();
    assert_eq!(all(log, at[0]), at.iter().copied().zip(run).collect::<Vec<_>>(), "read back as appended");
    bytes_contract(log, &at, &lens);
}

/// \[DESIGN\] RULES.md D1231: the bytes `log` tells between `positions`,
/// of records of `lens` bytes appended in order and nothing else since:
/// none backwards or from a position to itself; growing with the later
/// position; and from the first to the end, the records' bytes, give or
/// take what a log adds to each and the segments it counts as their size,
/// never their number.
fn bytes_contract(log: &dyn Log, positions: &[Lsn], lens: &[usize]) {
    let (first, end) = (positions[0], log.end());
    assert_eq!((log.bytes_between(end, first), log.bytes_between(first, first)), (0, 0), "none backwards");
    let counted: Vec<u64> = positions.iter().map(|&at| log.bytes_between(first, at)).collect();
    assert!(counted.windows(2).all(|w| w[0] <= w[1]), "growing with the later position: {counted:?}");
    let appended = u64::try_from(lens.iter().sum::<usize>()).unwrap();
    let whole = log.bytes_between(first, end);
    assert!(whole * 2 >= appended && whole <= appended * 2 + 64 * 1024, "{whole} bytes told of {appended} appended");
}
