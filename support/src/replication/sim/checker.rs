//! What the simulator checks of a run, as it runs: [VRR §8.1]'s safety,
//! "every committed operation survives into all subsequent views in the
//! same position in the serial order"; at most one primary serving in each
//! view, the one the view names; commits told only to a serving primary,
//! never going back; outcomes of records in doubt as the log decided them.

use std::collections::BTreeMap;

use turborel_replication::ReplicaId;

/// A violation, as the test reports it.
pub type Violation = String;

/// The record a replica appended at a position, and the outcome it was
/// told, if it was.
#[derive(Debug, Clone)]
struct Outcome {
    replica: ReplicaId,
    at: u64,
    record: Vec<u8>,
    kept: bool,
}

/// What a run has shown so far.
#[derive(Debug, Default)]
pub struct Checker {
    /// The committed log as far as any replica has shown it.
    committed: Vec<Vec<u8>>,
    /// Who served as primary in each view.
    primaries: BTreeMap<u64, ReplicaId>,
    /// The outcomes told.
    outcomes: Vec<Outcome>,
    /// Records acknowledged to a serving primary's database: committed.
    pub acknowledged: u64,
}

impl Checker {
    /// The committed log as far as shown.
    pub fn committed(&self) -> &[Vec<u8>] {
        &self.committed
    }

    /// `prefix` is shown committed by `replica`: it must agree with what
    /// was shown before, position by position.
    pub fn commit(&mut self, replica: ReplicaId, prefix: &[Vec<u8>]) -> Result<(), Violation> {
        for (index, record) in prefix.iter().enumerate() {
            let Some(known) = self.committed.get(index) else {
                self.committed.push(record.clone());
                continue;
            };
            if known != record {
                return Err(format!("replica {} shows {record:?} committed at {}, where {known:?} was committed", replica.0, index + 1));
            }
        }
        Ok(())
    }

    /// `replica` serves as primary of `view`, of a group of `members`.
    pub fn serves(&mut self, replica: ReplicaId, view: u64, members: &[ReplicaId]) -> Result<(), Violation> {
        let named = members[usize::try_from(view % u64::try_from(members.len()).unwrap()).unwrap()];
        if named != replica {
            return Err(format!("replica {} serves view {view}, whose primary is {}", replica.0, named.0));
        }
        if let Some(other) = self.primaries.insert(view, replica) {
            return Err(format!("replicas {} and {} both served as primary of view {view}", other.0, replica.0));
        }
        Ok(())
    }

    /// `replica` was told whether its record at `at` was kept.
    pub fn outcome(&mut self, replica: ReplicaId, at: u64, record: Vec<u8>, kept: bool) {
        self.outcomes.push(Outcome { replica, at, record, kept });
    }

    /// At the end of a run, every outcome as the committed log has it.
    pub fn check_outcomes(&self) -> Result<(), Violation> {
        for o in &self.outcomes {
            let Some(committed) = self.committed.get(usize::try_from(o.at - 1).unwrap()) else {
                return Err(format!("replica {} was told of its record at {}, past the committed log", o.replica.0, o.at));
            };
            if (committed == &o.record) != o.kept {
                return Err(format!("replica {} was told its record at {} was kept: {}, wrongly", o.replica.0, o.at, o.kept));
            }
        }
        Ok(())
    }

    /// How many outcomes were told.
    pub fn outcomes(&self) -> usize {
        self.outcomes.len()
    }

    /// The highest view a primary served.
    pub fn last_view(&self) -> u64 {
        self.primaries.keys().next_back().copied().unwrap_or(0)
    }
}
