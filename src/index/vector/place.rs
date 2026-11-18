//! \[EXTENSION\] Where a vector index method keeps its indexes: the
//! registered file of the database's buffer pool, or a budgeted file in
//! memory, as the R-tree's (RULES.md D72); the memory budget its engines
//! reserve of (D38); the metrics and quantizers its options name, the
//! database's; and the indexes it made, for its checkpoint to write
//! what each keeps in memory before the file is synced.

use std::sync::{Arc, Mutex, Weak};

use super::Column;
use super::column::refused;
use super::index::{Engine, Held};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::engine::hnsw;
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::{Host, MemoryBudget};
use crate::sync::lock_builtin;
use crate::vector::metric::{Metrics, VectorMetric, built_in_metrics, metric_named};
use crate::vector::quantizer::{Quantizer, Quantizers, built_in_quantizers, quantizer_named};

/// HNSW's and IVF's metric and quantizer.
pub(crate) type CodecPlugins = (hnsw::Metric, Option<Arc<dyn hnsw::Quantizer>>);

/// The name of a quantizer option's value for none.
pub(crate) const NO_QUANTIZER: &str = "NONE";

/// A vector index method's place.
pub(crate) struct Place<E: Engine> {
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    memory: Arc<MemoryPages>,
    metrics: Metrics,
    quantizers: Quantizers,
    made: Mutex<Vec<Weak<Held<E>>>>,
}

impl<E: Engine> Place<E> {
    /// In the registered file of `pool`.
    pub(crate) fn new(pool: Arc<BufferPool>, file: FileId) -> Place<E> {
        Self::with(Some((pool, file)), MemoryPages::new(0))
    }

    /// In a budgeted file in memory, made when the first index is.
    pub(crate) fn in_memory() -> Place<E> {
        Self::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// In the file in memory the database's built-in methods share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> Place<E> {
        Self::with(None, memory)
    }

    fn with(files: Option<(Arc<BufferPool>, FileId)>, memory: Arc<MemoryPages>) -> Place<E> {
        Place {
            files: Mutex::new(files),
            memory,
            metrics: built_in_metrics(),
            quantizers: built_in_quantizers(),
            made: Mutex::new(Vec::new()),
        }
    }

    /// Of the metrics `metrics` gives.
    pub(crate) fn with_metrics(self, metrics: Metrics) -> Place<E> {
        Place { metrics, ..self }
    }

    /// Of the quantizers `quantizers` gives.
    pub(crate) fn with_quantizers(self, quantizers: Quantizers) -> Place<E> {
        Place { quantizers, ..self }
    }

    /// Takes the database's memory budget.
    pub(crate) fn attach(&self, host: &Host) {
        self.memory.use_budget(Arc::clone(&host.memory));
    }

    /// The pool and file its indexes' pages are in.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the file in memory cannot be made.
    pub(crate) fn files(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut files = lock_builtin(&self.files);
        if let Some((pool, file)) = files.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let (pool, file) = self.memory.file("the pool in memory of the built-in engine and index methods")?;
        *files = Some((Arc::clone(&pool), file));
        Ok((pool, file))
    }

    /// The memory budget its engines reserve of.
    pub(crate) fn budget(&self) -> Arc<MemoryBudget> {
        self.memory.budget()
    }

    /// The metric named `name`, if an index of `column` may order by it:
    /// one measuring the column's format, and of a graph index (`graph`),
    /// SYMMETRIC and NON_NEGATIVE (the vector design document R8.2).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a metric the database does not know; -900 for
    /// one the index may not take.
    pub(crate) fn metric(&self, (name, graph): (&str, bool), column: Column) -> Result<Arc<dyn VectorMetric>> {
        let metric = metric_named(&self.metrics, name)
            .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[EXTENSION] there is no vector metric {name}")))?;
        if !metric.formats().contains(&column.element) {
            return Err(refused(
                E::METHOD,
                &format!("orders by {}, which does not measure {} vectors", metric.name(), column.element.name()),
            ));
        }
        let p = metric.properties();
        if graph && !(p.symmetric && p.non_negative) {
            return Err(refused(E::METHOD, &format!("orders by a SYMMETRIC, NON_NEGATIVE metric (R8.2), and {} is not", metric.name())));
        }
        Ok(metric)
    }

    /// The name of the metric named `name`, as the metric gives it: an
    /// alias's metric's, if the database knows one.
    pub(crate) fn metric_name(&self, name: &str) -> Option<String> {
        metric_named(&self.metrics, name).map(|m| m.name().to_owned())
    }

    /// The quantizer named `name`, or none for `NONE`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a quantizer the database does not know.
    pub(crate) fn quantizer(&self, name: &str) -> Result<Option<Arc<dyn Quantizer>>> {
        if name.eq_ignore_ascii_case(NO_QUANTIZER) {
            return Ok(None);
        }
        quantizer_named(&self.quantizers, name)
            .map(Some)
            .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[EXTENSION] there is no vector quantizer {name}")))
    }

    /// HNSW's or IVF's metric and quantizer of an index of `column`, the
    /// ones named, and the metric's name; of a graph index (`graph`), the
    /// metric as `metric` requires.
    ///
    /// # Errors
    ///
    /// As `metric` and `quantizer`.
    pub(crate) fn codec_plugins(&self, (metric, quantizer): (&str, &str), column: Column, graph: bool) -> Result<(CodecPlugins, String)> {
        let metric = self.metric((metric, graph), column)?;
        let measure = hnsw::Metric::new(metric.name(), super::distance(&metric, column));
        let quantizer = self.quantizer(quantizer)?.map(super::codec_quantizer);
        Ok(((measure, quantizer), metric.name().to_owned()))
    }

    /// Keeps `held`, made or opened, for checkpoints.
    pub(crate) fn keep(&self, held: &Arc<Held<E>>) {
        let mut made = lock_builtin(&self.made);
        made.retain(|w| w.strong_count() > 0);
        made.push(Arc::downgrade(held));
    }

    /// Writes what each index it made keeps in memory, then syncs the file.
    ///
    /// # Errors
    ///
    /// An engine's; the buffer pool's.
    pub(crate) fn checkpoint(&self) -> Result<()> {
        // Taken out of the lock first: flushing writes pages, and holds no
        // lock of the method's meanwhile (R-ERR-4).
        let held: Vec<Arc<Held<E>>> = lock_builtin(&self.made).iter().filter_map(Weak::upgrade).collect();
        for h in held.iter().filter(|h| !h.discarded()) {
            h.engine.flush()?;
        }
        drop(held);
        let files = lock_builtin(&self.files).clone();
        match files {
            Some((pool, file)) => pool.sync(file, Durability::Full),
            None => Ok(()),
        }
    }
}
