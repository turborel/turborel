//! The mutations of a row store's zone maintenance (RULES.md R-TEST-9):
//! a write staling its zone only when a bound may move inward, a zone settled
//! from its rows' newest versions, and the rest after a pass.

use super::{Mutation, Package};

/// The mutations of zone maintenance, in the order they run.
pub const ZONES: [Mutation; 6] = [
    Mutation {
        name: "zone-stale-inside",
        package: Package::Core,
        breaks: "zone maintenance: a value changed strictly inside its zone's bounds stales the zone, summed up again for nothing",
        file: "src/engine/rows/zones.rs",
        original: "            !same && !bounds.as_ref().is_some_and(|bounds| reached(bounds, was, now))",
        replacement: "            !same",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_write_inside_its_zones_bounds_stales_nothing"],
    },
    Mutation {
        name: "zone-stale-kept",
        package: Package::Core,
        breaks: "zone maintenance: a write keeping a value on its zone's bound stales the zone, summed up again for nothing",
        file: "src/engine/rows/zones.rs",
        original: "            !same && !bounds.as_ref().is_some_and(|bounds| reached(bounds, was, now))",
        replacement: "            !bounds.as_ref().is_some_and(|bounds| reached(bounds, was, now))",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_write_inside_its_zones_bounds_stales_nothing"],
    },
    Mutation {
        name: "zone-stale-grown",
        package: Package::Core,
        breaks: "zone maintenance: a value on the greatest bound grown past it stales the zone, so a running total keeps it summed up again",
        file: "src/engine/rows/zones.rs",
        original: "compare(was, greatest) == Some(Ordering::Less) || moved == Some(Ordering::Greater);",
        replacement: "compare(was, greatest) == Some(Ordering::Less);",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_value_growing_past_its_zones_bound_stales_nothing"],
    },
    Mutation {
        name: "zone-unstale-inward",
        package: Package::Core,
        breaks: "zone maintenance: a value on the least bound moved inside it leaves the zone not stale, wider than its rows for good",
        file: "src/engine/rows/zones.rs",
        original: "compare(least, was) == Some(Ordering::Less) || moved == Some(Ordering::Less);",
        replacement: "compare(least, was) == Some(Ordering::Less) || moved.is_some();",
        integration: &[],
        unit: &["engine::rows::zones::tests::a_value_growing_past_its_zones_bound_stales_nothing"],
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
        original: "            self.rest();",
        replacement: "",
        integration: &[],
        unit: &["engine::rows::zones::rested::zones_rest_after_a_pass_over_every_zone_due"],
    },
];
