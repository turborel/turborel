//! \[EXTENSION\] The object of each kind an extension declares, bridged
//! onto the trait its kind has (RULES.md D169, D220).

use std::sync::Arc;

use super::{Link, Resolve, auth, backup, collation, engine, index, language, partition, process, quantizer, ranking, rule, system};
use super::{aggregate, embedder, metric, procedure, resolved, scalar, table};
use super::{types, xquery};
use crate::error::Result;
use crate::functions::builtin::{Procedure, TableFunction};
use crate::functions::{AggregateFunction, ScalarFunction};
use crate::plugin::errors::invalid_manifest;
use crate::plugin::manifest::{Declared, ObjectKind};
use crate::plugin::pool::Pool;
use crate::vector::embed::Embedder;
use crate::vector::metric::VectorMetric;

/// One object of an extension, bridged.
#[derive(Clone)]
pub enum Bridged {
    /// A scalar function.
    Scalar(Arc<dyn ScalarFunction>),
    /// An aggregate function.
    Aggregate(Arc<dyn AggregateFunction>),
    /// A table function.
    Table(Arc<dyn TableFunction>),
    /// A procedure.
    Procedure(Arc<dyn Procedure>),
    /// A vector metric.
    Metric(Arc<dyn VectorMetric>),
    /// An embedding model.
    Embedding(Arc<dyn Embedder>),
    /// An authentication mechanism.
    Authenticator(Arc<auth::PluginAuthenticator>),
    /// A backup target's scheme.
    Target(Arc<backup::PluginTargets>),
    /// A partitioning strategy.
    Partition(Arc<partition::PluginStrategy>),
    /// A ranking of full-text search.
    Ranking(Arc<ranking::PluginRanking>),
    /// A vector quantizer.
    Quantizer(Arc<quantizer::PluginQuantizer>),
    /// A collation.
    Collation(Arc<collation::PluginCollation>),
    /// A system table.
    SystemTable(Arc<system::PluginSystemTable>),
    /// An XQuery function.
    XQuery(Arc<xquery::PluginXQuery>),
    /// A maker of users' verifiers.
    Credentials(Arc<process::PluginCredentials>),
    /// A sink of events.
    Diagnostics(Arc<process::PluginDiagnostics>),
    /// A type, as the process keeps it.
    Type(&'static types::PluginType),
    /// A storage engine.
    Engine(Arc<engine::PluginEngine>),
    /// An index method.
    IndexMethod(Arc<index::PluginIndexMethod>),
    /// An optimizer rule.
    Rule(Arc<rule::PluginRule>),
    /// A language of full-text search.
    Language(Arc<language::PluginLanguage>),
}

/// The object `d` declares, calling `pool`.
pub(super) fn bridge(d: &Declared, pool: &Arc<Pool>, resolve: Resolve<'_>) -> Result<Bridged> {
    let link = Link { pool: Arc::clone(pool), kind: d.kind.code(), object: d.name.clone() };
    Ok(match d.kind {
        ObjectKind::Scalar => Bridged::Scalar(Arc::new(scalar::PluginScalar::new(d, link, resolve)?)),
        ObjectKind::Aggregate => Bridged::Aggregate(Arc::new(aggregate::PluginAggregate::new(d, link, resolve)?)),
        ObjectKind::Table => Bridged::Table(Arc::new(table::PluginTable::new(d, link, resolve)?)),
        ObjectKind::Procedure => Bridged::Procedure(Arc::new(procedure::PluginProcedure::new(d, link, resolve)?)),
        ObjectKind::Metric => Bridged::Metric(Arc::new(metric::PluginMetric::new(d, link)?)),
        ObjectKind::Embedding => Bridged::Embedding(Arc::new(embedder::PluginEmbedder::new(d, link)?)),
        ObjectKind::Authenticator => Bridged::Authenticator(Arc::new(auth::PluginAuthenticator::new(link))),
        ObjectKind::Target => Bridged::Target(Arc::new(backup::PluginTargets::new(link))),
        ObjectKind::Partition => Bridged::Partition(Arc::new(partition::PluginStrategy::new(link))),
        ObjectKind::Ranking => Bridged::Ranking(Arc::new(ranking::PluginRanking::new(link))),
        ObjectKind::Quantizer => Bridged::Quantizer(Arc::new(quantizer::PluginQuantizer::new(link))),
        ObjectKind::Collation => {
            let version = d.version.clone().unwrap_or_default();
            Bridged::Collation(Arc::new(collation::PluginCollation::new(link, version, d.batch)))
        }
        ObjectKind::SystemTable => {
            let columns = d.columns.iter().map(|(n, t)| Ok((n.clone(), super::checked_type(resolve(t)?)?))).collect::<Result<_>>()?;
            Bridged::SystemTable(Arc::new(system::PluginSystemTable::new(link, columns, d.batch)))
        }
        ObjectKind::XQuery => {
            let result = d.returns.as_ref().ok_or_else(|| invalid_manifest("an XQuery function RETURNS a type"))?;
            Bridged::XQuery(Arc::new(xquery::PluginXQuery::new(link, resolved(&d.parameters, resolve)?, &resolve(result)?)?))
        }
        ObjectKind::Credentials => Bridged::Credentials(Arc::new(process::PluginCredentials::new(link))),
        ObjectKind::Diagnostics => Bridged::Diagnostics(Arc::new(process::PluginDiagnostics::new(link, d.batch)?)),
        ObjectKind::Type => {
            let (made, _) = types::type_of(pool.extension(), &d.name, d.ordered());
            types::attach(made, pool);
            Bridged::Type(made)
        }
        ObjectKind::Engine => Bridged::Engine(Arc::new(engine::PluginEngine::new(link, d.batch))),
        ObjectKind::IndexMethod => Bridged::IndexMethod(Arc::new(index::PluginIndexMethod::new(link, d.supports.clone(), d.batch))),
        ObjectKind::Rule => Bridged::Rule(Arc::new(rule::PluginRule::new(d, link))),
        ObjectKind::Language => Bridged::Language(Arc::new(language::PluginLanguage::new(d, link)?)),
    })
}
