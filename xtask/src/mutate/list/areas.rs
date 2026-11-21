//! Each area's mutations, in the order they run, after the core's.

mod altered_tables;
mod analyze;
mod analyze_more;
mod batches;
mod budget;
mod bulk;
// The count's, kept beside the other areas' files.
mod client_sync;
mod cost_model;
#[path = "count.rs"]
mod count;
mod declared_types;
// The decode kernels' mutations, beside the areas as `list.rs` holds no
// more modules.
#[path = "decode.rs"]
mod decode;
mod distinct_aggregates;
mod expr;
// The estimator's, beside the areas as `list.rs` holds no more modules.
#[path = "estimates.rs"]
mod estimates;
mod group_table;
mod insert_values;
mod join_estimates;
mod join_table;
mod joins;
mod late;
mod monitoring;
mod oltp;
mod orders;
mod parallel;
mod range_joins;
mod rewrites;
mod set_types;
mod skipping;
mod sorts;
mod statistics;
mod subquery_plans;
mod tpcc;
mod txn_layer;
// Unnesting's, UUID's and xxHash's mutations, here as `list.rs` holds no more
// modules.
mod mark_joins;
mod semi_joins;
mod unnesting;
mod uuid;
mod xxhash;

use super::Mutation;

/// The core's mutations, then those of the mechanisms beside it.
pub fn core() -> [&'static [Mutation]; 18] {
    [
        &super::MUTATIONS,
        &super::assertions::ASSERTIONS,
        &super::hash::HASH,
        &super::rekey::REKEY,
        &super::KEPT,
        &super::READERS,
        &super::READS,
        &super::COLLATIONS,
        &super::TAILORINGS,
        &super::sql2023::SQL2023,
        &super::regex::REGEX,
        &super::SHADOW,
        &super::backup::BACKUP,
        &oltp::OLTP,
        &tpcc::TPCC,
        &txn_layer::TXN_LAYER,
        &monitoring::MONITORING,
        &bulk::BULK,
    ]
}

/// Every area's mutations, in order: the first half's, the second's, then
/// the planner's newest.
pub fn all() -> impl Iterator<Item = &'static [Mutation]> {
    first().into_iter().chain(second()).chain(planner())
}

/// The first half of the areas, in order.
fn first() -> [&'static [Mutation]; 25] {
    [
        &super::predicates::PREDICATES,
        &super::psm::PSM,
        &super::modules::MODULES,
        &super::ptf::PTF,
        &super::sequences::SEQUENCES,
        &super::generated::GENERATED,
        &super::truncate::TRUNCATE,
        &super::contents::CONTENTS,
        &super::nulls::NULLS,
        &super::with::WITH,
        &super::search_cycle::SEARCH_CYCLE,
        &super::composite::COMPOSITE,
        &super::distinct::DISTINCT,
        &super::bigint::BIGINT,
        &super::distinct_predicate::DISTINCT_PREDICATE,
        &super::partitioned_joins::PARTITIONED_JOINS,
        &super::grouping_sets::GROUPING_SETS,
        &super::listagg::LISTAGG,
        &super::null_ordering::NULL_ORDERING,
        &super::windows::WINDOWS,
        &super::window_values::WINDOW_VALUES,
        &super::window_nested::WINDOW_NESTED,
        &super::window_distinct::WINDOW_DISTINCT,
        &super::width_bucket::WIDTH_BUCKET,
        &super::filter::FILTER,
    ]
}

/// The second half of the areas, in order, unnesting's last.
fn second() -> [&'static [Mutation]; 45] {
    [
        &super::hypothetical::HYPOTHETICAL,
        &super::distribution::DISTRIBUTION,
        &super::order_by::ORDER_BY,
        &super::dependencies::DEPENDENCIES,
        &super::limits::LIMITS,
        &super::bounded::BOUNDED_FETCH,
        &super::patterns::PATTERNS,
        &super::merge::MERGE,
        &super::diagnostics::DIAGNOSTICS,
        &super::triggers::TRIGGERS,
        &super::routines::ROUTINES,
        &super::overloading::OVERLOADING,
        &super::roles::ROLES,
        &super::array_targets::ARRAY_TARGETS,
        &super::explain::EXPLAIN,
        &super::view_defaults::VIEW_DEFAULTS,
        &super::lobs::LOBS,
        &super::multisets::MULTISETS,
        &super::structured::STRUCTURED,
        &super::temporal::TEMPORAL,
        &super::temporal_references::TEMPORAL_REFERENCES,
        &super::system_versioned::SYSTEM_VERSIONED,
        &super::rename::RENAME,
        &altered_tables::ALTERED_TABLES,
        &super::savepoints::SAVEPOINTS,
        &super::graph_catalog::GRAPH_CATALOG,
        &super::graph_native::GRAPH_NATIVE,
        &super::vector_indexes::VECTOR_INDEXES,
        &super::vector_embed::VECTOR_EMBED,
        &super::vector_hybrid::VECTOR_HYBRID,
        &super::vector::VECTOR,
        &super::delta_tables::DELTA_TABLES,
        &count::COUNT,
        &distinct_aggregates::DISTINCT_AGGREGATES,
        &rewrites::REWRITES,
        &unnesting::UNNESTING,
        &subquery_plans::SUBQUERY_PLANS,
        &insert_values::INSERT_VALUES,
        &set_types::SET_TYPES,
        &expr::EXPR,
        &sorts::SORTS,
        &estimates::ESTIMATES,
        &analyze::ANALYZE,
        &analyze_more::ANALYZE_MORE,
        &statistics::STATISTICS,
    ]
}

/// The planner's newest areas, in order: join estimates, range joins, the
/// cost model and orders.
fn planner() -> [&'static [Mutation]; 4] {
    [&join_estimates::JOIN_ESTIMATES, &range_joins::RANGE_JOINS, &cost_model::COST_MODEL, &orders::ORDERS]
}

/// and T621's, then the spatial areas', the planar engine's, the memory budget's, replication's, the server's, the transport's, the decode kernels' and late materialization's.
pub fn late() -> impl Iterator<Item = &'static [Mutation]> {
    let json: [&'static [Mutation]; 16] = [
        &super::projection::PROJECTION,
        &super::json::JSON,
        &super::json_path::JSON_PATH,
        &super::json_type::JSON_TYPE,
        &super::json_index::JSON_INDEX,
        &super::expression_index::EXPRESSION_INDEX,
        &super::json_table::JSON_TABLE,
        &super::fulltext::FULLTEXT,
        &super::fulltext_languages::FULLTEXT_LANGUAGES,
        &super::sqlmm_fulltext::SQLMM_FULLTEXT,
        &super::t621::T621,
        &super::graph_syntax::GRAPH_SYNTAX,
        &super::graph_search::GRAPH_SEARCH,
        &super::graph_search::algorithms::GRAPH_ALGO,
        &super::graph_gql::GRAPH_GQL,
        &super::graph_search::choice::GRAPH_SEARCH_CHOICE,
    ];
    let xml: [&'static [Mutation]; 17] = [
        &super::guarded::GUARDED,
        &super::xquery::XQUERY,
        &super::xquery_functions::XQUERY_FUNCTIONS,
        &super::xquery_statics::XQUERY_STATICS,
        &super::xml_publishing::XML_PUBLISHING,
        &super::xml_composite::XML_COMPOSITE,
        &super::xml_map::XML_MAP,
        &super::xml_index::XML_INDEX,
        &super::xquery_eval::XQUERY_EVAL,
        &super::xml_schema::XML_SCHEMA,
        &super::xml_schema_catalog::XML_SCHEMA_CATALOG,
        &super::short_names::SHORT_NAMES,
        &super::information::INFORMATION,
        &super::stamps::STAMPS,
        &super::xml_scoped::XML_SCOPED,
        &super::xquery_modules::XQUERY_MODULES,
        &super::xml::XML,
    ];
    json.into_iter().chain(xml).chain(plugins()).chain(last()).chain(closing())
}

/// The transport's, parallel queries', the client's step with its
/// server, the decode kernels', declared types', batches', late
/// materialization's, the table of groups', the join table's, XXH3's,
/// semi joins', mark joins' and skipping's last, as `last` holds no more.
fn closing() -> [&'static [Mutation]; 13] {
    [
        &super::transport::TRANSPORT,
        &parallel::PARALLEL,
        &client_sync::CLIENT_SYNC,
        &decode::DECODE,
        &declared_types::DECLARED_TYPES,
        &batches::BATCHES,
        &late::LATE,
        &group_table::GROUP_TABLE,
        &join_table::JOIN_TABLE,
        &xxhash::XXHASH,
        &semi_joins::SEMI_JOINS,
        &mark_joins::MARK_JOINS,
        &skipping::SKIPPING,
    ]
}

/// External plug-ins' areas, in order: the plug-ins', the packages', the
/// package files', the kinds', the SDK's, host plug-ins', the
/// replication and archive host kinds', plug-in settings' and buffer
/// eviction's, and the planner's.
fn plugins() -> [&'static [Mutation]; 9] {
    [
        &super::plugins::PLUGINS,
        &super::plugins::PACKAGES,
        &super::plugins::files::FILES,
        &super::plugins::kinds::PLUGIN_KINDS,
        &super::plugins::sdk::SDK,
        &super::plugins::hosted::HOSTED,
        &super::plugins::replication::REPLICATION_PLUGINS,
        &super::plugins::settings::SETTINGS_PLUGINS,
        &super::plugins::planner::PLANNER,
    ]
}

/// The last areas, in order: graph compatibility's, the spatial areas', the planar engine's, the
/// adjacency index's, GRAPH_TABLE's and its joins', stored CHECKs' calls, simple features', partitioning's, UUID's,
/// the memory budget's,
/// the command-line client's, replication's, the server's, the native protocol's, the client library's,
/// join enumeration's and the standard's gaps a benchmark found.
fn last() -> [&'static [Mutation]; 48] {
    [
        &super::graph_compat::GRAPH_COMPAT,
        &super::near::NEAR,
        &super::rtree::RTREE,
        &super::spatial_regions::SPATIAL_REGIONS,
        &super::rtree_windows::RTREE_WINDOWS,
        &super::rtree_costs::RTREE_COSTS,
        &super::rtree_pack::RTREE_PACK,
        &super::rtree_joins::RTREE_JOINS,
        &super::geometry::GEOMETRY,
        &super::geometry::orientation::ORIENTATION,
        &super::geometry::cells::CELLS,
        &super::geometry::buckets::BUCKETS,
        &super::geometry::filed::FILED,
        &super::geometry::reach::REACH,
        &super::geometry::within::WITHIN,
        &budget::BUDGET,
        &super::spatial_types::SPATIAL_TYPES,
        &super::spatial_curves::SPATIAL_CURVES,
        &super::spatial_routines::SPATIAL_ROUTINES,
        &super::spatial_mutators::SPATIAL_MUTATORS,
        &super::spatial_planar::SPATIAL_PLANAR,
        &super::spatial_srs::SPATIAL_SRS,
        &super::spatial_support::SPATIAL_SUPPORT,
        &super::spatial_solids::SPATIAL_SOLIDS,
        &super::spatial_topology::SPATIAL_TOPOLOGY,
        &super::spatial_lr::SPATIAL_LR,
        &super::spatial_network::SPATIAL_NETWORK,
        &super::graph_adjacency::GRAPH_ADJACENCY,
        &super::graph_table::GRAPH_TABLE,
        &super::graph_table::joins::GRAPH_JOINS,
        &super::graph_table::indexes::GRAPH_INDEXES,
        &super::graph_table::statistics::GRAPH_STATISTICS,
        &super::graph_iterations::GRAPH_ITERATIONS,
        &super::check_rebind::CHECK_REBIND,
        &super::simple_features::SIMPLE_FEATURES,
        &super::partitioning::PARTITIONING,
        &uuid::UUID,
        &super::cli::CLI,
        &super::FILL,
        &super::ZONES,
        &super::STATS,
        &super::WAITS,
        &super::VR,
        &super::SERVER,
        &super::server::native::NATIVE,
        &super::client::CLIENT,
        &joins::JOINS,
        &super::std_gaps::STD_GAPS,
    ]
}
