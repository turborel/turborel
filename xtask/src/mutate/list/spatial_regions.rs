//! The mutations of spatial serializable read regions (RULES.md R-TEST-9,
//! D133): the index naming no region, a region of the wrong column, a
//! NULL in every region, an empty geometry away from the origin, regions
//! never merged or merged without their earlier envelopes, and regions
//! ignored by changes kept as bounds.

use super::{Mutation, Package};

const OUTSIDE: &str = "geometry::index::regions::geometry_reads_conflict_only_with_writes_in_their_window";
const NULLS: &str = "geometry::index::regions::geometry_regions_handle_nulls_and_empty_geometries";
const HULL: &str = "geometry::index::regions::geometry_regions_past_the_budget_merge_into_their_hull";

/// The mutations of spatial read regions, in the order they run.
pub const SPATIAL_REGIONS: [Mutation; 7] = [
    Mutation {
        name: "region-not-named",
        package: Package::Core,
        breaks: "a geometry index names no region, so its reads are of the whole table",
        file: "src/index/rtree.rs",
        original: "geometry::region(probe, self.srid?)",
        replacement: "geometry::region(probe, self.srid?).and(None)",
        integration: &[
            OUTSIDE,
            "geometry::index::regions::geometry_moves_across_a_region_border_conflict",
            "geometry::index::regions::geometry_join_probes_record_each_window",
        ],
        unit: &[],
    },
    Mutation {
        name: "region-wrong-column",
        package: Package::Core,
        breaks: "a region is matched against the table's first column, not the key's",
        file: "src/exec/read.rs",
        original: "Region::new(c, envelopes)",
        replacement: "Region::new(c.saturating_sub(c), envelopes)",
        integration: &[OUTSIDE],
        unit: &[],
    },
    Mutation {
        name: "region-holds-nulls",
        package: Package::Core,
        breaks: "a row whose geometry is NULL is in every region",
        file: "src/storage/region.rs",
        original: "        if value.is_null() {\n            return false;",
        replacement: "        if value.is_null() {\n            return true;",
        integration: &[NULLS],
        unit: &["storage::region::tests::regions_match_rows_by_their_keyed_envelopes"],
    },
    Mutation {
        name: "region-empty-elsewhere",
        package: Package::Core,
        breaks: "an empty geometry is matched away from the origin the index keys it at",
        file: "src/storage/region.rs",
        original: "Geometry::from_value(value).ok().and_then(|g| keyed_envelope(&g).ok())",
        replacement: "Geometry::from_value(value).ok().and_then(|g| g.bounds().or_else(|| Bounds::point([1e9_f64; DIMENSIONS]).ok()))",
        integration: &[NULLS],
        unit: &["storage::region::tests::regions_match_rows_by_their_keyed_envelopes"],
    },
    Mutation {
        name: "region-never-merged",
        package: Package::Core,
        breaks: "regions grow past TRANSACTIONS.TRACKED_ROWS envelopes, unmerged",
        file: "src/engine/rows/mvcc/reads.rs",
        original: "if self.envelopes > tracked {",
        replacement: "if self.envelopes > tracked.saturating_mul(1_000) {",
        integration: &[HULL],
        unit: &[],
    },
    Mutation {
        name: "region-merge-forgets",
        package: Package::Core,
        breaks: "merging keeps a column's last envelope, not the hull of all of them",
        file: "src/storage/region.rs",
        original: "Some(kept) => kept.envelopes = kept.hull().map_or_else(|| vec![hull], |h| vec![h.union(hull)]),",
        replacement: "Some(kept) => kept.envelopes = vec![hull],",
        integration: &[HULL],
        unit: &["storage::region::tests::merged_regions_are_a_hull_of_each_column"],
    },
    Mutation {
        name: "region-bounds-ignored",
        package: Package::Core,
        breaks: "changes kept as each column's bounds are matched by conditions alone, never a region",
        file: "src/engine/rows/mvcc/reads.rs",
        original: "Changed::Bounds(bounds) => !regions.is_empty() || conditions",
        replacement: "Changed::Bounds(bounds) => conditions",
        integration: &["geometry::index::regions::geometry_regions_match_changes_kept_as_bounds"],
        unit: &[],
    },
];
