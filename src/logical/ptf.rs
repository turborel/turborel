//! [ISO/IEC 9075-2:2023 §7.6, §9.25] A `<PTF derived table>`, bound: the
//! invocation of a polymorphic table function compiled as it was bound
//! (`db/ptf/compile.rs`), its table arguments' rows its inputs, and its
//! arguments' values; generic over the plans of its inputs, of the logical
//! plan and the physical one.

use std::sync::Arc;

use super::Expr;
use crate::db::ptf::Call;

/// [ISO/IEC 9075-2:2023 §7.6 SR12a, §9.25] The rows of a `<PTF derived
/// table>`: of its proper result columns, then of each table argument its
/// columns of PASS THROUGH, else its partitioning columns.
#[derive(Debug, Clone)]
pub struct Polymorphic<P> {
    /// The invocation, compiled, and its component procedures, bound.
    pub(crate) call: Arc<Call>,
    /// Of each parameter, the value of its pre-compilation argument [§9.18
    /// SR12f]: of a generic table parameter null, of a descriptor
    /// parameter null if its argument is, else some value not null.
    pub arguments: Vec<Expr>,
    /// The rows of each table argument, each of its columns and then the
    /// keys that partition and order it.
    pub inputs: Vec<P>,
    /// Its columns' display names.
    pub names: Vec<String>,
}

impl<P> Polymorphic<P> {
    /// It, its inputs those `f` gives of each.
    pub fn map<Q>(self, f: impl FnMut(P) -> Q) -> Polymorphic<Q> {
        Polymorphic { call: self.call, arguments: self.arguments, inputs: self.inputs.into_iter().map(f).collect(), names: self.names }
    }

    /// The name of the function it invokes, as a plan shows it.
    pub fn function(&self) -> String {
        self.call.name.to_string()
    }
}
