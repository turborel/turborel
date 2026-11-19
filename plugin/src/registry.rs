//! \[EXTENSION\] A plug-in's objects, by kind and name (RULES.md D220):
//! what a program builds and hands to `serve`.

use std::collections::HashMap;

use super::kinds::{
    Aggregate, Authenticator, Collation, Credentials, Diagnostics, Embedder, Metric, PartitionStrategy, Procedure, Quantizer, Ranking,
    Scalar, SystemTable, TableFunction, Target, Type, XQueryFunction,
};
use super::objects::Held;
use super::rule::Rule;
use super::storage::{Engine, IndexMethod};
use super::{Answer, PluginError};

/// The code of an optimizer rule's kind, as WELCOME gives it.
const RULE_CODE: u8 = 37;

/// The objects of one kind, by name.
pub(super) type Of<T> = HashMap<String, Box<T>>;

/// A plug-in: its version and its objects, each by the name its manifest
/// declares, as the database writes it in a CALL (an identifier in upper
/// case unless delimited; an XQuery function's expanded name as written).
#[derive(Default)]
pub struct Plugin {
    pub(super) version: String,
    pub(super) scalars: Of<dyn Scalar>,
    pub(super) aggregates: Of<dyn Aggregate>,
    pub(super) tables: Of<dyn TableFunction>,
    pub(super) procedures: Of<dyn Procedure>,
    pub(super) metrics: Of<dyn Metric>,
    pub(super) embedders: Of<dyn Embedder>,
    pub(super) authenticators: Of<dyn Authenticator>,
    pub(super) targets: Of<dyn Target>,
    pub(super) strategies: Of<dyn PartitionStrategy>,
    pub(super) rankings: Of<dyn Ranking>,
    pub(super) quantizers: Of<dyn Quantizer>,
    pub(super) collations: Of<dyn Collation>,
    pub(super) system_tables: Of<dyn SystemTable>,
    pub(super) xquery: Of<dyn XQueryFunction>,
    pub(super) types: Of<dyn Type>,
    pub(super) engines: Of<dyn Engine>,
    pub(super) methods: Of<dyn IndexMethod>,
    pub(super) credentials: Of<dyn Credentials>,
    pub(super) diagnostics: Of<dyn Diagnostics>,
    pub(super) rules: Of<dyn Rule>,
    /// What calls keep between them.
    pub(super) held: Held,
    /// The host objects, by kind's code and name (RULES.md D229).
    pub(super) hosted: HashMap<(u8, String), Box<dyn super::host::HostObject>>,
}

/// Adds a builder method for a kind.
macro_rules! builder {
    ($(#[$doc:meta])* $method:ident, $field:ident, $kind:ident) => {
        $(#[$doc])*
        #[must_use]
        pub fn $method(mut self, name: &str, object: impl $kind + 'static) -> Plugin {
            self.$field.insert(name.to_owned(), Box::new(object));
            self
        }
    };
}

impl Plugin {
    /// A plug-in of no object yet, of `version`, which WELCOME gives.
    pub fn new(version: &str) -> Plugin {
        Plugin { version: version.to_owned(), ..Plugin::default() }
    }

    builder!(
        /// With the scalar function `name`.
        scalar, scalars, Scalar
    );
    builder!(
        /// With the aggregate function `name`.
        aggregate, aggregates, Aggregate
    );
    builder!(
        /// With the table function `name`.
        table_function, tables, TableFunction
    );
    builder!(
        /// With the procedure `name`.
        procedure, procedures, Procedure
    );
    builder!(
        /// With the metric `name`.
        metric, metrics, Metric
    );
    builder!(
        /// With the embedding model `name`.
        embedder, embedders, Embedder
    );
    builder!(
        /// With the authenticator of mechanism `name`.
        authenticator, authenticators, Authenticator
    );
    builder!(
        /// With the backup target of scheme `name`.
        target, targets, Target
    );
    builder!(
        /// With the partitioning strategy `name`.
        partition_strategy, strategies, PartitionStrategy
    );
    builder!(
        /// With the ranking `name`.
        ranking, rankings, Ranking
    );
    builder!(
        /// With the quantizer `name`.
        quantizer, quantizers, Quantizer
    );
    builder!(
        /// With the collation `name`.
        collation, collations, Collation
    );
    builder!(
        /// With the system table `name`.
        system_table, system_tables, SystemTable
    );
    builder!(
        /// With the XQuery function `name`, written `{uri}local`.
        xquery_function, xquery, XQueryFunction
    );
    builder!(
        /// With the type `name`.
        data_type, types, Type
    );
    builder!(
        /// With the storage engine `name`.
        engine, engines, Engine
    );
    builder!(
        /// With the index method `name`.
        index_method, methods, IndexMethod
    );
    builder!(
        /// With the credentials `name`.
        credentials, credentials, Credentials
    );
    builder!(
        /// With the sink of events `name`.
        diagnostics, diagnostics, Diagnostics
    );
    builder!(
        /// With the optimizer rule `name` (RULES.md D275).
        rule, rules, Rule
    );

    /// The codes of the kinds it has an object of, as WELCOME gives them.
    pub(super) fn kinds(&self) -> Vec<u8> {
        let counts = [
            self.scalars.len(),
            self.aggregates.len(),
            self.tables.len(),
            self.procedures.len(),
            self.metrics.len(),
            self.embedders.len(),
            self.authenticators.len(),
            self.targets.len(),
            self.strategies.len(),
            self.rankings.len(),
            self.quantizers.len(),
            self.collations.len(),
            self.system_tables.len(),
            self.xquery.len(),
            self.types.len(),
            self.engines.len(),
            self.methods.len(),
            self.credentials.len(),
            self.diagnostics.len(),
        ];
        let mut kinds: Vec<u8> = (1_u8..).zip(counts).filter(|(_, n)| *n > 0).map(|(k, _)| k).collect();
        kinds.extend(self.hosted.keys().map(|(k, _)| *k));
        kinds.extend((!self.rules.is_empty()).then_some(RULE_CODE));
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }
}

/// The object of `name` in `all`.
///
/// # Errors
///
/// SQLSTATE 38000 if there is none.
pub(super) fn find<'a, T: ?Sized>(all: &'a mut Of<T>, name: &str) -> Answer<&'a mut Box<T>> {
    all.get_mut(name).ok_or_else(|| PluginError::external(&format!("the plug-in has no object {name} of that kind")))
}
