//! The contract of a run of records appended together (RULES.md D882),
//! checked of every log with the rest of its contract (`logs::contract`).

use turborel::log::Log;

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
    assert_eq!(all(log, at[0]), at.iter().copied().zip(run).collect::<Vec<_>>(), "read back as appended");
}
