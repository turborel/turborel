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
use super::options::Shared;
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

/// The metric an index measures by, and the names of the one it orders
/// by and of this one (`Place::metric`).
pub(crate) struct Measured {
    /// The metric measured by.
    pub(crate) metric: Arc<dyn VectorMetric>,
    /// The metric ordered by's name, and the measured one's.
    pub(crate) names: (String, String),
}

/// The plug-ins an index's options name, and its shared options.
pub(crate) struct Named<'a> {
    /// Its metric's name.
    pub(crate) metric: &'a str,
    /// Its quantizer's name, or `NONE`.
    pub(crate) quantizer: &'a str,
    /// Its shared options.
    pub(crate) shared: Shared,
}

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

    /// The metric an index of `column` ordered by the metric named `name`
    /// measures by, if it may order by it: one measuring the column's
    /// format; of a column declared `normalized`, the metric it orders as
    /// there (`MetricProperties::normalized_as`, `options.rs`); and of a
    /// graph index (`graph`), SYMMETRIC and NON_NEGATIVE (the vector design
    /// document R8.2).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a metric the database does not know; -900 for
    /// one the index may not take.
    pub(crate) fn metric(&self, (name, graph, normalized): (&str, bool, bool), column: Column) -> Result<Measured> {
        let declared = metric_named(&self.metrics, name)
            .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[EXTENSION] there is no vector metric {name}")))?;
        if !declared.formats().contains(&column.element) {
            return Err(refused(
                E::METHOD,
                &format!("orders by {}, which does not measure {} vectors", declared.name(), column.element.name()),
            ));
        }
        let unit = declared.properties().normalized_as.filter(|_| normalized).and_then(|m| metric_named(&self.metrics, m));
        let metric = unit.unwrap_or_else(|| Arc::clone(&declared));
        let p = metric.properties();
        if graph && !(p.symmetric && p.non_negative) {
            return Err(refused(E::METHOD, &format!("orders by a SYMMETRIC, NON_NEGATIVE metric (R8.2), and {} is not", metric.name())));
        }
        Ok(Measured { names: (declared.name().to_owned(), metric.name().to_owned()), metric })
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

    /// HNSW's or IVF's metric and quantizer of an index of `column` as
    /// `o` names them, with codes of its code bytes; of a graph index
    /// (`graph`), the metric as `metric` requires.
    ///
    /// # Errors
    ///
    /// As `metric` and `quantizer`.
    pub(crate) fn codec_plugins(&self, o: &Named<'_>, column: Column, graph: bool) -> Result<(CodecPlugins, (String, String))> {
        let m = self.metric((o.metric, graph, o.shared.normalized), column)?;
        let measure = hnsw::Metric::new(m.metric.name(), super::distance(&m.metric, column)).with_norms(super::normed(&m.metric));
        let quantizer = self.quantizer(o.quantizer)?.map(|q| super::codec_quantizer(q, o.shared.code_bytes));
        Ok(((measure, quantizer), m.names))
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
