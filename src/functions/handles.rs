//! Shared handles to the functions registered.

use std::fmt;
use std::sync::Arc;

use super::{AggregateFunction, ScalarFunction};

/// A shared handle to a registered scalar function.
#[derive(Clone)]
pub struct ScalarRef(pub Arc<dyn ScalarFunction>);
/// A shared handle to a registered aggregate function.
#[derive(Clone)]
pub struct AggregateRef(pub Arc<dyn AggregateFunction>);

impl fmt::Debug for ScalarRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.name())
    }
}
impl fmt::Debug for AggregateRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.name())
    }
}
