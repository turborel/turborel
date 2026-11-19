//! \[EXTENSION\] The host kinds' bridges (RULES.md D229, D246 to D254):
//! each host object served over its `HostLink` as the trait of its point.
//! A failed call stops the node (`plugin::hosted`), so a bridge that must
//! answer, a checksum or the time, answers what does no harm after it.

pub mod page;
pub mod store;
pub mod time;
pub mod trust;
