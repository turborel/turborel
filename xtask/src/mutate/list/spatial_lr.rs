//! Mutations of linear referencing (RULES.md R-TEST-9): locating a
//! position along an element and the catalog of its methods
//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15, §21].

use super::{Mutation, Package};

/// Linear referencing's measures, offsets and catalog, each broken so
/// its requirement tests fail.
pub const SPATIAL_LR: [Mutation; 8] = [
    Mutation {
        name: "lr-absolute-start",
        package: Package::Core,
        breaks: "an absolute measure is taken from the element's start, not from its start value",
        file: "src/spatial/lr/locate.rs",
        original: "ABSOLUTE => to_coords(&d.along, &self.lrm.units, self.cat)? - self.start()?,",
        replacement: "ABSOLUTE => to_coords(&d.along, &self.lrm.units, self.cat)?,",
        integration: &[],
        unit: &["spatial::lr::tests::locating::absolute_measures"],
    },
    Mutation {
        name: "lr-distance-past-end",
        package: Package::Core,
        breaks: "a distance past the element's end is held to its end",
        file: "src/spatial/lr/locate.rs",
        original: "if s < -tolerance || s > self.path.length() + tolerance {",
        replacement: "if s < -tolerance {",
        integration: &[],
        unit: &["spatial::lr::tests::locating::point_exceptions"],
    },
    Mutation {
        name: "lr-interpolative-scale",
        package: Package::Core,
        breaks: "an interpolative measure is not scaled to the element's length",
        file: "src/spatial/lr/locate.rs",
        original: "INTERPOLATIVE => self.along_default(&d.along)? * self.path.length(),",
        replacement: "INTERPOLATIVE => self.along_default(&d.along)?,",
        integration: &[],
        unit: &["spatial::lr::tests::locating::interpolative_measures"],
    },
    Mutation {
        name: "lr-foot-nearest",
        package: Package::Core,
        breaks: "ST_LRPosition takes the first segment's foot, not the nearest",
        file: "src/spatial/lr/measure.rs",
        original: "if best.is_none_or(|f| distance < f.distance) {",
        replacement: "if best.is_none() {",
        integration: &[],
        unit: &["spatial::lr::tests::locating::lr_position_inverts_point"],
    },
    Mutation {
        name: "lr-m-bracket",
        package: Package::Core,
        breaks: "an m value above a segment's is found on it",
        file: "src/spatial/lr/measure.rs",
        original: "if m < ma.min(mb) || m > ma.max(mb) {",
        replacement: "if m < ma.min(mb) {",
        integration: &[],
        unit: &["spatial::lr::tests::locating::local_interpolative_measures"],
    },
    Mutation {
        name: "lr-towards-needs-from",
        package: Package::Core,
        breaks: "ST_TowardsRefName sets a towards referent with no from referent",
        file: "src/spatial/lr/distance.rs",
        original: "if atowardsreferentname.is_some() && !has_from {",
        replacement: "if atowardsreferentname.is_some() && has_from && !has_from {",
        integration: &[],
        unit: &["spatial::lr::tests::build::distance_expressions"],
    },
    Mutation {
        name: "lr-lateral-with-vector",
        package: Package::Core,
        breaks: "a lateral offset is set beside a vector offset",
        file: "src/spatial/lr/distance.rs",
        original: "if this.is_some_and(|d| d.vector.is_some()) {\n            return Err(failed(SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET, \"§15.9.8",
        replacement: "if this.is_some_and(|d| d.vector.is_some() && d.lateral.is_some()) {\n            return Err(failed(SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET, \"§15.9.8",
        integration: &[],
        unit: &["spatial::lr::tests::build::offsets_exclude_each_other"],
    },
    Mutation {
        name: "lr-define-checks-id",
        package: Package::Core,
        breaks: "ST_DEFINELRM keeps an LRM_ID other than its definition's ST_LRMID",
        file: "src/spatial/lr/sql/catalog.rs",
        original: "if lrm.id != id {",
        replacement: "if lrm.id != id && lrm.id == id {",
        integration: &["spatial_lr::catalog_defines_methods_and_columns"],
        unit: &[],
    },
];
