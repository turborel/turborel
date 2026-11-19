//! \[EXTENSION\] VECTOR built in, and its plug-ins (RULES.md D93): the
//! type and its functions, given every database as it is made; distance
//! metrics (`VectorMetric`) and quantizers (`Quantizer`), built in and
//! registered (`Database::register_metric`, `register_quantizer`).
//! VECTOR_DISTANCE names a metric (design document §6.1 R6.2), and a
//! vector index names a metric and a quantizer (§10.2), a distance
//! function of a schema as `DISTANCE schema.udf_name`.
//!
//! The registries are shared, not copied with each change of the
//! database's state, as full text's rankings are: the functions of VECTOR
//! and the vector index methods are made as the database is, and see a
//! metric registered later at their next call.

use std::sync::{Arc, PoisonError, RwLock};

use super::{Database, State};
use crate::catalog::{ColumnDesc, SqlRoutineDesc};
use crate::diagnostics::Extension;
use crate::disk::buffer::{BufferPool, FileId};
use crate::error::{Code, Result, SqlError};
use crate::index::IndexMethod;
use crate::lexer::authorization_identifier;
use crate::value::CompOp;
use crate::vector::metric::{Metrics, VectorMetric, metric_named};
use crate::vector::quantizer::{Quantizer, Quantizers, built_in_quantizers, quantizer_named};

/// The metrics and quantizers registered, beside the built-in.
#[derive(Clone, Default)]
pub(crate) struct SharedVectors {
    metrics: Arc<RwLock<Vec<Arc<dyn VectorMetric>>>>,
    quantizers: Arc<RwLock<Vec<Arc<dyn Quantizer>>>>,
}

impl SharedVectors {
    /// Every metric, the built-in first, as they are registered when it is
    /// called: what VECTOR_DISTANCE and a vector index read.
    pub(crate) fn metrics(&self) -> Metrics {
        let shared = Arc::clone(&self.metrics);
        let builtin = crate::vector::metric::built_in_metrics();
        Arc::new(move || {
            let mut all = builtin();
            all.extend(shared.read().unwrap_or_else(PoisonError::into_inner).iter().cloned());
            all
        })
    }

    /// Every quantizer, the built-in first, as they are registered when it
    /// is called.
    pub(crate) fn quantizers(&self) -> Quantizers {
        let shared = Arc::clone(&self.quantizers);
        let builtin = built_in_quantizers();
        Arc::new(move || {
            let mut all = builtin();
            all.extend(shared.read().unwrap_or_else(PoisonError::into_inner).iter().cloned());
            all
        })
    }

    fn add_metric(&self, metric: Arc<dyn VectorMetric>) -> Result<()> {
        if metric_named(&self.metrics(), metric.name()).is_some() {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] a metric {} is registered already", metric.name())));
        }
        if metric.formats().is_empty() {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] the metric {} measures no format", metric.name())));
        }
        self.metrics.write().unwrap_or_else(PoisonError::into_inner).push(metric);
        Ok(())
    }

    fn add_quantizer(&self, quantizer: Arc<dyn Quantizer>) -> Result<()> {
        if quantizer_named(&self.quantizers(), quantizer.name()).is_some() {
            return Err(SqlError::new(Code::Api, format!("[EXTENSION] a quantizer {} is registered already", quantizer.name())));
        }
        self.quantizers.write().unwrap_or_else(PoisonError::into_inner).push(quantizer);
        Ok(())
    }
}

/// That the columns `keyed` of index `name` of method `m` are of types it
/// keys (`IndexMethod::check_columns`): by default values grouped, as
/// keys are [ISO/IEC 9075-2:2023 §9.12 SR3], of an XML method one XML
/// column (RULES.md D106); and \[EXTENSION\] are no vectors, if `m` keys by
/// value, by `=` or an order: a vector has neither (design document §17,
/// RULES.md D93). A method of its own search, a vector index's, keys them.
///
/// # Errors
///
/// SQLCODE -103 if one is not, or is a vector; -900 as the method says.
pub(super) fn keyed_by<'a>(m: &dyn IndexMethod, keyed: impl Iterator<Item = &'a ColumnDesc>, name: &str) -> Result<()> {
    let what = format!("index {name}");
    let types: Vec<_> = keyed.map(|column| column.data_type.clone()).collect();
    m.check_columns(&types, &what)?;
    if !m.supports(CompOp::Eq) {
        return Ok(());
    }
    types.iter().try_for_each(|t| crate::binder::orderless_operand(t, &what))
}

/// \[EXTENSION\] That the routine `desc`, if a distance function of SQL
/// (design document §8), is one: its body binds over its two vectors to a
/// number, and its name is no metric's built in or registered, which
/// VECTOR_DISTANCE would otherwise name in its place.
///
/// # Errors
///
/// SQLCODE -103 if its body is no number of them or it is named as a
/// metric is.
pub(super) fn distance_defined(st: &State, desc: &SqlRoutineDesc) -> Result<()> {
    if desc.distance.is_none() {
        return Ok(());
    }
    crate::binder::sql_metric(st, desc)?;
    let metrics = st.vectors.metrics();
    let qualified = format!("{}.{}", desc.name.auth, desc.name.ident);
    if let Some(m) = [desc.name.ident.as_str(), &qualified].into_iter().find(|n| metric_named(&metrics, n).is_some()) {
        let message = format!("[EXTENSION] a distance function is named as no metric is, and {m} is one");
        return Err(SqlError::new(Code::SyntaxRule, message));
    }
    Ok(())
}

/// Whether `name` is a regular identifier in upper case, as a plug-in's
/// name a statement writes as a key word must be.
fn upper_identifier(name: &str) -> bool {
    authorization_identifier(name).ok().as_deref() == Some(name)
}

/// Whether `name` may name a metric: a regular identifier in upper case,
/// or two joined by a period, a distance function's schema and its own
/// name, as `DISTANCE schema.udf_name` writes it (§10.2).
fn metric_name(name: &str) -> bool {
    match name.split_once('.') {
        Some((schema, own)) => upper_identifier(schema) && upper_identifier(own),
        None => upper_identifier(name),
    }
}

impl State {
    /// \[EXTENSION\] The functions of VECTOR, built in as full text's are
    /// (RULES.md D93), VECTOR_DISTANCE naming every metric registered when
    /// it is called. Of no name another built-in function has, so none is
    /// refused.
    pub(super) fn vector_functions(&mut self) {
        let metrics = self.vectors.metrics();
        for f in crate::vector::functions_with(&metrics) {
            let _ = self.functions.add_scalar(f);
        }
        for f in crate::vector::aggregates() {
            let _ = self.functions.add_aggregate(f);
        }
    }
}

impl Database {
    /// \[EXTENSION\] Registers a distance metric, which VECTOR_DISTANCE
    /// and a vector index then name (design document §6.1 R6.2, §10.2,
    /// RULES.md D93). Its name is a regular identifier in upper case, or a
    /// distance function's, a schema's name and its own joined by a
    /// period, which an index names by `DISTANCE schema.udf_name`. Its
    /// contract is `VectorMetric`'s; VECTOR_DISTANCE refuses a result that
    /// breaks it with SQLCODE -910.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is not so written, it measures no format,
    /// or a metric of its name is built in or registered already.
    pub fn register_metric(&self, metric: Box<dyn VectorMetric>) -> Result<()> {
        let metric: Arc<dyn VectorMetric> = Arc::from(metric);
        let name = metric.name().to_owned();
        if !metric_name(&name) {
            let message = format!("[EXTENSION] a metric is named by a regular identifier in upper case, or schema.name, not {name}");
            return self.registered((Extension::Metric, &name), Err(SqlError::new(Code::Api, message)));
        }
        let done = self.snapshot().and_then(|st| st.vectors.add_metric(metric));
        self.registered((Extension::Metric, &name), done)
    }

    /// \[EXTENSION\] Registers a quantizer, which a vector index then names
    /// to compress its vectors (design document §7.2, RULES.md D93). Its
    /// contract is `Quantizer`'s.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its name is no regular identifier in upper case, or
    /// a quantizer of its name is built in or registered already.
    pub fn register_quantizer(&self, quantizer: Box<dyn Quantizer>) -> Result<()> {
        let quantizer: Arc<dyn Quantizer> = Arc::from(quantizer);
        let name = quantizer.name().to_owned();
        if !upper_identifier(&name) {
            let message = format!("[EXTENSION] a quantizer is named by a regular identifier in upper case, not {name}");
            return self.registered((Extension::Quantizer, &name), Err(SqlError::new(Code::Api, message)));
        }
        let done = self.snapshot().and_then(|st| st.vectors.add_quantizer(quantizer));
        self.registered((Extension::Quantizer, &name), done)
    }

    /// Registers the vector index methods over the file `file` of `pool`,
    /// of the database's metrics and quantizers.
    pub(super) fn register_vector_methods(&self, pool: &Arc<BufferPool>, file: FileId) -> Result<()> {
        let st = self.snapshot()?;
        let (plugins, rankings) = ((st.vectors.metrics(), st.vectors.quantizers()), st.rankings.closure());
        drop(st);
        crate::index::vector::vector_methods(pool, file, &plugins, &rankings).into_iter().try_for_each(|m| self.register_index_method(m))
    }
}
