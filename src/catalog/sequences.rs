//! Sequence generators, as the catalog describes them [ISO/IEC 9075-2:2023
//! §4.29, §11.72]. A descriptor holds what its definition and alterations
//! made. The current base value is not in it, as no transaction controls
//! that \[§4.29.1\]. It is the counter of `db/sequences/counters.rs`, which
//! the descriptor names by its epoch.

use super::QualifiedName;
use super::domains::{UsageDesc, usage_held};
use crate::value::DataType;

/// [ISO/IEC 9075-2:2023 §4.29.1] A sequence generator's descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceDesc {
    /// The `<sequence generator name>`.
    pub name: QualifiedName,
    /// Its data type, exact numeric of scale 0 \[§11.72 SR6\].
    pub data_type: DataType,
    /// The start value.
    pub start: i128,
    /// The minimum value.
    pub minimum: i128,
    /// The maximum value.
    pub maximum: i128,
    /// The increment, not 0: a descending generator's is negative.
    pub increment: i128,
    /// CYCLE, rather than NO CYCLE.
    pub cycle: bool,
    /// \[DESIGN\] The epoch of its counter: given anew by its definition and
    /// by each ALTER that restarts it, never twice for one name.
    pub epoch: u64,
    /// \[DESIGN\] The current base value its counter began at in that
    /// epoch: the start value, or the restart value.
    pub origin: i128,
    /// USAGE on it \[§11.72 GR2\]: each grant.
    pub usage: Vec<UsageDesc>,
}

impl SequenceDesc {
    /// Whether `auth` holds USAGE on it, and whether grantably, as of a
    /// domain (`DomainDesc::usage_of`).
    pub fn usage_of(&self, auth: &str, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, auth, owner)
    }

    /// Whether it ascends: its increment is positive \[§4.29.1\].
    pub fn ascending(&self) -> bool {
        self.increment > 0
    }
}
