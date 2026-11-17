//! Keep the benchmark's tables in memory but spill its large result
//! cursors to the benchmark's own temporary directory. The global memory
//! budget and each area's buffer pool remain the normal ones.

use std::path::Path;
use std::sync::Arc;

use turborel::disk::io::OsFileSystem;
use turborel::error::Result;
use turborel::memory::MemoryBudget;
use turborel::spill::{Area, PagedSpill, Spill, SpillArea, TempFiles};

pub struct OnDisk {
    temp: Arc<TempFiles>,
}

impl OnDisk {
    pub fn new(dir: &Path) -> Self {
        Self { temp: Arc::new(TempFiles::new(Arc::new(OsFileSystem), dir)) }
    }
}

impl Spill for OnDisk {
    fn name(&self) -> &'static str {
        "geometry_benchmark_spill"
    }

    fn area(&self, memory: &Arc<MemoryBudget>, _temp: &Arc<TempFiles>, area: Area) -> Result<Box<dyn SpillArea>> {
        // This benchmark never changes encodings: its synthetic input and
        // scratch files are unencrypted throughout every compared run.
        PagedSpill.area(memory, &self.temp, area)
    }
}
