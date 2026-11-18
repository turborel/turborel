//! Each area's mutations, in the order they run, after the core's.

use super::Mutation;

/// The core's mutations, then those of the mechanisms beside it.
pub fn core() -> [&'static [Mutation]; 11] {
    [
        &super::MUTATIONS,
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
    ]
}

/// Every area's mutations, in order: the first half's, then the second's.
pub fn all() -> impl Iterator<Item = &'static [Mutation]> {
    first().into_iter().chain(second())
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

/// The second half of the areas, in order.
fn second() -> [&'static [Mutation]; 28] {
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
        &super::savepoints::SAVEPOINTS,
        &super::vector_indexes::VECTOR_INDEXES,
        &super::vector_embed::VECTOR_EMBED,
        &super::vector_hybrid::VECTOR_HYBRID,
        &super::vector::VECTOR,
    ]
}

/// The areas after the rest, in order: projection's, JSON's, expression indexes', full-text's
/// and T621's, then the spatial areas', the planar engine's, stored CHECKs' calls, replication's and the server's.
pub fn late() -> impl Iterator<Item = &'static [Mutation]> {
    let json: [&'static [Mutation]; 10] = [
        &super::projection::PROJECTION,
        &super::json::JSON,
        &super::json_path::JSON_PATH,
        &super::json_type::JSON_TYPE,
        &super::json_index::JSON_INDEX,
        &super::expression_index::EXPRESSION_INDEX,
        &super::json_table::JSON_TABLE,
        &super::fulltext::FULLTEXT,
        &super::sqlmm_fulltext::SQLMM_FULLTEXT,
        &super::t621::T621,
    ];
    let last: [&'static [Mutation]; 21] = [
        &super::near::NEAR,
        &super::rtree::RTREE,
        &super::spatial_regions::SPATIAL_REGIONS,
        &super::geometry::GEOMETRY,
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
        &super::check_rebind::CHECK_REBIND,
        &super::FILL,
        &super::ZONES,
        &super::STATS,
        &super::WAITS,
        &super::VR,
        &super::SERVER,
    ];
    json.into_iter().chain(last)
}
