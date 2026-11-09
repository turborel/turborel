//! The mutations of a row store's zone maintenance (RULES.md R-TEST-9):
//! a write staling its zone only when a bound may move, a zone settled
//! from its rows' newest versions, and the rest after a pass.

use super::{Mutation, Package};

/// The mutations of zone maintenance, in the order they run.
pub const ZONES: [Mutation; 4] = [
    Mutation {
        name: "zone-stale-inside",
        package: Package::Core,
        breaks: "zone maintenance: a value changed strictly inside its zone's bounds stales the zone, summed up again for nothing",
        file: "src/engine/rows/zones.rs",
        original: "            !same && !inside",
        replacement: "            !same",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_write_inside_its_zones_bounds_stales_nothing"],
    },
    Mutation {
        name: "zone-stale-kept",
        package: Package::Core,
        breaks: "zone maintenance: a write keeping a value on its zone's bound stales the zone, summed up again for nothing",
        file: "src/engine/rows/zones.rs",
        original: "            !same && !inside",
        replacement: "            !inside",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_write_inside_its_zones_bounds_stales_nothing"],
    },
    Mutation {
        name: "zone-settled-uncounted",
        package: Package::Core,
        breaks: "zone maintenance: a zone settled alone keeps counting rows deleted, so a scan's share is guessed high",
        file: "src/engine/rows/zones/summed.rs",
        original: "                None => zone.rows = rows,",
        replacement: "                None => {}",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_zone_unsettled_alone_is_settled_with_its_bounds_kept"],
    },
    Mutation {
        name: "zone-unrested",
        package: Package::Core,
        breaks: "zone maintenance: the zones never rest, so steady writes keep the maintenance thread busy",
        file: "src/engine/rows/zones/summed.rs",
        original: "            self.zones.rest();",
        replacement: "",
        integration: &[],
        unit: &["engine::rows::zones::tests::zones_rest_after_a_pass_over_every_zone_due"],
    },
];
