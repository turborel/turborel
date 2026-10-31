//! A spill provider for unit tests: areas whose runs are held in memory,
//! read back, or, if broken, not; tables it has none of.

use std::sync::Arc;

use super::{Area, Records, Spill, SpillArea, SpillRun, SpillTable, TempFiles, WorkAreas};
use crate::error::{Code, Result, SqlError};
use crate::memory::MemoryBudget;

/// The provider.
pub(crate) struct Held {
    pub(crate) broken: bool,
}

struct HeldArea {
    broken: bool,
}

struct HeldRun {
    records: Vec<Vec<u8>>,
    broken: bool,
}

impl Spill for Held {
    fn name(&self) -> &'static str {
        "held"
    }
    fn area(&self, _memory: &Arc<MemoryBudget>, _temp: &Arc<TempFiles>, _area: Area) -> Result<Box<dyn SpillArea>> {
        Ok(Box::new(HeldArea { broken: self.broken }))
    }
}

impl SpillArea for HeldArea {
    fn table(&self) -> Result<Box<dyn SpillTable>> {
        Err(SqlError::new(Code::Api, "[DESIGN] no tables"))
    }
    fn run(&self) -> Result<Box<dyn SpillRun>> {
        Ok(Box::new(HeldRun { records: Vec::new(), broken: self.broken }))
    }
}

impl SpillRun for HeldRun {
    fn push(&mut self, record: &[u8]) -> Result<()> {
        self.records.push(record.to_vec());
        Ok(())
    }
    fn records(&mut self) -> Result<Records> {
        if self.broken {
            return Err(SqlError::new(Code::Io, "[DESIGN] told to fail"));
        }
        Ok(Box::new(self.records.clone().into_iter().map(Ok)))
    }
}

/// Work areas of a budget of `budget` bytes, of `spill`, or of the
/// built-in provider if none.
pub(crate) fn areas(spill: Option<Arc<dyn Spill>>, budget: usize) -> Arc<WorkAreas> {
    let areas = Arc::new(WorkAreas::new(MemoryBudget::new(budget)));
    if let Some(spill) = spill {
        areas.set_spill(spill);
    }
    areas
}
